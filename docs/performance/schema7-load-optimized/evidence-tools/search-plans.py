import json,subprocess,pathlib,time,sys
out=pathlib.Path('docs/performance/schema7-load-optimized')
query="""WITH matching AS MATERIALIZED (SELECT id FROM nodes WHERE workspace_id='00000000-0000-4000-8000-000000000002' AND NOT deleted AND NOT retired AND verification IS DISTINCT FROM 'disputed' AND search_document @@ websearch_to_tsquery('english',%s)), selected AS MATERIALIZED (SELECT id FROM matching ORDER BY id LIMIT 21) SELECT to_jsonb(n)-'search_document' FROM selected JOIN nodes n ON n.id=selected.id AND n.workspace_id='00000000-0000-4000-8000-000000000002' ORDER BY selected.id"""
plans=[]
for name,term in [('common','authentication'),('selective','12345'),('missing','absentbenchmarktermzz')]:
    sql='SET jit=off; EXPLAIN (ANALYZE,BUFFERS,SETTINGS,FORMAT JSON) '+query%("'"+term+"'")
    result=json.loads(subprocess.check_output(['docker','exec','aq-graph-schema7-load-pg','psql','-U','postgres','-d','aq_graph_bench_schema7','-Atc',sql],text=True))
    plans.append(dict(name=name,term=term,plan=result))
(out/'search-plans.json').write_text(json.dumps({'method':'Read-only literal EXPLAIN ANALYZE BUFFERS SETTINGS on shared search SQL with default optional filters simplified; literal/custom plan, not proof of prepared generic plan or concurrent performance','plans':plans},indent=2)+'\n')
