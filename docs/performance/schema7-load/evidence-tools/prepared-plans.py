import pathlib,re,subprocess,json,uuid
out=pathlib.Path('docs/performance/schema7-load')
store=pathlib.Path('src/store.rs').read_text();search=pathlib.Path('src/store/queries.rs').read_text()
def extract(source):return re.search(r'query_scalar::<_,Value>\(\s*"(.*?)"\s*\)',source,re.S)[1]
ready=extract(store.split('pub async fn ready(',1)[1]);history=extract(store.split('pub async fn history(',1)[1]);search=extract(search.split('pub async fn search_nodes(',1)[1])
workspace="'00000000-0000-4000-8000-000000000002'"
cases=[dict(name='ready',sql=ready,types='uuid,uuid,bigint',args=workspace+',NULL,21')]
for index in (10,50000,99990):
    cases.append(dict(name='history_'+str(index),sql=history,types='uuid,uuid,bigint,bigint',args=workspace+",'"+str(uuid.UUID(int=index))+"',0,21"))
for name,term in [('common','authentication'),('selective','12345'),('missing','absentbenchmarktermzz')]:
    cases.append(dict(name='search_'+name,sql=search,types='uuid,text,boolean,boolean,timestamptz,timestamptz,text,uuid,bigint',args=workspace+",NULL,false,false,NULL,NULL,'"+term+"',NULL,21"))
plans=[]
for case in cases:
    for mode in ('force_custom_plan','force_generic_plan'):
        sql='SET plan_cache_mode='+mode+'; PREPARE benchmark_plan('+case['types']+') AS '+case['sql']+'; EXPLAIN (ANALYZE,BUFFERS,SETTINGS,FORMAT JSON) EXECUTE benchmark_plan('+case['args']+');'
        raw=subprocess.check_output(['docker','exec','aq-graph-schema7-load-pg','psql','-U','postgres','-d','aq_graph_bench_schema7','-qAtc',sql],text=True)
        result=json.loads(raw)
        plans.append(dict(**case,plan_cache_mode=mode,plan=result))
        print(case['name'],mode,result[0]['Execution Time'],flush=True)
        (out/'prepared-plans.json').write_text(json.dumps({'method':'Exact current service SQL/parameter order, explicit PostgreSQL inferred parameter types, forced custom and generic prepared read-only plans after workload; EXPLAIN ANALYZE BUFFERS SETTINGS. Forced plans diagnose possibilities; they do not identify which per-connection plan was chosen during load.','plans':plans},indent=2)+'\n')
