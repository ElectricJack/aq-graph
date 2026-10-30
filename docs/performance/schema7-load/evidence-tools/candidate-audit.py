import pathlib,re,subprocess,json,time
out=pathlib.Path('docs/performance/schema7-load')
baseline=json.loads((out/'prepared-plans.json').read_text())['plans'];old=next(x['sql'] for x in baseline if x['name']=='ready');candidate=pathlib.Path('/tmp/aq-ready-candidate.sql').read_text()
workspace="'00000000-0000-4000-8000-000000000002'"
def call(sql):return json.loads(subprocess.check_output(['docker','exec','aq-graph-schema7-load-pg','psql','-U','postgres','-d','aq_graph_bench_schema7','-qAtc',sql],text=True))
parity=[]
for cursor,limit in [(None,1),(None,21),(None,101),(1,21),(26666,21),(53333,21),(70000,21),(79999,21),(99999,21)]:
    param_cursor='NULL' if cursor is None else "'"+str(__import__('uuid').UUID(int=cursor))+"'"
    def bind(q):return q.replace('$1',workspace).replace('$2',param_cursor).replace('$3',str(limit))
    q="SET jit=off; WITH old(value) AS MATERIALIZED ("+bind(old)+"), new(value) AS MATERIALIZED ("+bind(candidate)+") SELECT json_build_object('old_count',(SELECT count(*) FROM old),'new_count',(SELECT count(*) FROM new),'difference_count',(SELECT count(*) FROM ((SELECT value FROM old EXCEPT SELECT value FROM new) UNION ALL (SELECT value FROM new EXCEPT SELECT value FROM old)) d),'same_ordered_page',COALESCE((SELECT jsonb_agg(value ORDER BY value->>'id') FROM old),'[]'::jsonb)=COALESCE((SELECT jsonb_agg(value ORDER BY value->>'id') FROM new),'[]'::jsonb));"
    started=time.monotonic();result=call(q);parity.append(dict(cursor_index=cursor,limit=limit,elapsed_seconds=time.monotonic()-started,**result));print('parity',cursor,limit,result,flush=True)
    (out/'ready-candidate-parity.json').write_text(json.dumps({'method':'Original measured ready SQL versus EXCEPT/lateral candidate, full JSON values and ordered ID pages compared within each statement snapshot on retained fixture; read-only jit=off; representative cursors/limits, not exhaustive lifecycle cases','original_sql':old,'candidate_sql':candidate,'cases':parity},indent=2)+'\n')
plans=[]
for jit in ('on','off'):
    for mode in ('force_custom_plan','force_generic_plan'):
        q='SET jit='+jit+'; SET plan_cache_mode='+mode+'; PREPARE candidate(uuid,uuid,bigint) AS '+candidate+'; EXPLAIN (ANALYZE,BUFFERS,SETTINGS,FORMAT JSON) EXECUTE candidate('+workspace+',NULL,21);'
        plan=call(q);plans.append(dict(jit=jit,plan_cache_mode=mode,plan=plan));print('plan',jit,mode,plan[0]['Execution Time'],plan[0].get('JIT'),flush=True)
        (out/'ready-candidate-jit-plans.json').write_text(json.dumps({'method':'Identical candidate SQL with explicit jit on/off and forced custom/generic prepared plans; isolated post-baseline warm diagnostic, not concurrent performance','sql':candidate,'plans':plans},indent=2)+'\n')
