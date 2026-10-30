import pathlib,re,subprocess,json,uuid
source=pathlib.Path('src/store.rs').read_text().split('pub async fn history(',1)[1]
base=re.search(r'query_scalar::<_,Value>\("(.*?)"\)',source,re.S)[1]
plans=[]
for index in (10,50000,99990):
    sql=base.replace('$1',"'00000000-0000-4000-8000-000000000002'").replace('$2',"'"+str(uuid.UUID(int=index))+"'").replace('$3','0').replace('$4','21')
    result=json.loads(subprocess.check_output(['docker','exec','aq-graph-schema7-load-pg','psql','-U','postgres','-d','aq_graph_bench_schema7','-Atc','SET jit=off; EXPLAIN (ANALYZE,BUFFERS,SETTINGS,FORMAT JSON) '+sql],text=True))
    plans.append(dict(node_index=index,sql=sql,plan=result))
pathlib.Path('docs/performance/schema7-load-optimized/history-plans.json').write_text(json.dumps({'method':'Exact history-entry read SQL on low/middle/high UUID revision-rich nodes, literal no-cursor/default limit; endpoint also performs get-node; read-only post-workload EXPLAIN ANALYZE BUFFERS SETTINGS; not generic prepared plan','plans':plans},indent=2)+'\n')
