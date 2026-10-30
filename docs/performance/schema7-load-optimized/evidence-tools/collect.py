import json,subprocess,time,pathlib
output=pathlib.Path('docs/performance/schema7-load-optimized/postgres-resources.json')
samples=[]
query="""SELECT json_build_object('utc',clock_timestamp(),'database',(SELECT row_to_json(s) FROM pg_stat_database s WHERE datname=current_database()),'activity',(SELECT json_agg(s) FROM (SELECT state,wait_event_type,wait_event,count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() GROUP BY 1,2,3) s),'locks',(SELECT json_agg(s) FROM (SELECT mode,granted,count(*) FROM pg_locks WHERE database=(SELECT oid FROM pg_database WHERE datname=current_database()) GROUP BY 1,2) s))"""
while not pathlib.Path('.local/schema7-load-optimized/stop-collect').exists():
    started=time.time()
    pg=subprocess.run(['docker','exec','aq-graph-schema7-load-pg','psql','-U','postgres','-d','aq_graph_bench_schema7','-Atc',query],capture_output=True,text=True)
    stats=subprocess.run(['docker','stats','--no-stream','--format','{{json .}}','aq-graph-schema7-load-pg'],capture_output=True,text=True)
    samples.append(dict(epoch_seconds=started,postgres=json.loads(pg.stdout) if pg.returncode==0 else {'collector_error':True},container=json.loads(stats.stdout) if stats.returncode==0 else {'collector_error':True}))
    output.write_text(json.dumps({'measurement':'PostgreSQL container resources and cumulative stats; sampling subprocess overhead and 2s docker stats interval; no query text or credentials','samples':samples},indent=2)+'\n')
    time.sleep(max(0,2-(time.time()-started)))
