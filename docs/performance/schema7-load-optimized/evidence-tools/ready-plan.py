import pathlib,re,subprocess,json
source=pathlib.Path('src/store.rs').read_text().split('pub async fn ready(',1)[1]
sql=re.search(r'query_scalar::<_,Value>\("(.*?)"\)',source,re.S)[1]
sql=sql.replace('$1',"'00000000-0000-4000-8000-000000000002'").replace('$2','NULL').replace('$3','21')
result=json.loads(subprocess.check_output(['docker','exec','aq-graph-schema7-load-pg','psql','-U','postgres','-d','aq_graph_bench_schema7','-Atc','SET jit=off; EXPLAIN (ANALYZE,BUFFERS,SETTINGS,FORMAT JSON) '+sql],text=True))
pathlib.Path('docs/performance/schema7-load-optimized/ready-plan.json').write_text(json.dumps({'method':'Exact current ready SQL, literal default workspace/cursor/limit; read-only post-workload EXPLAIN ANALYZE BUFFERS SETTINGS; not generic prepared plan','sql':sql,'plan':result},indent=2)+'\n')
