import os
import json,subprocess,time,pathlib
output=pathlib.Path(os.environ['AQ_BENCH_OUTPUT'])/'postgres-resources.json'
samples=[]
query="""SELECT json_build_object('utc',clock_timestamp(),'database',(SELECT row_to_json(s) FROM pg_stat_database s WHERE datname=current_database()),'vacuum_progress',(SELECT coalesce(json_agg(to_jsonb(v)),'[]'::json) FROM pg_stat_progress_vacuum v WHERE datname=current_database()),'maintenance_tables',(SELECT json_agg(json_build_object('name',relname,'dead_tuples',n_dead_tup,'last_autovacuum',last_autovacuum,'autovacuum_count',autovacuum_count,'vacuum_count',vacuum_count)) FROM pg_stat_user_tables),'activity',(SELECT json_agg(s) FROM (SELECT state,wait_event_type,wait_event,count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() GROUP BY 1,2,3) s),'locks',(SELECT json_agg(s) FROM (SELECT mode,granted,count(*) FROM pg_locks WHERE database=(SELECT oid FROM pg_database WHERE datname=current_database()) GROUP BY 1,2) s))"""
while not (pathlib.Path(os.environ['AQ_BENCH_HELPER'])/'stop-collect').exists():
    started=time.time()
    pg=subprocess.run(['docker','exec','aq-graph-schema8-pg','psql','-U','aq_graph','-d','aq_graph_bench_schema8','-Atc',query],capture_output=True,text=True)
    stats=subprocess.run(['docker','stats','--no-stream','--format','{{json .}}','aq-graph-schema8-pg'],capture_output=True,text=True)
    samples.append(dict(epoch_seconds=started,postgres=json.loads(pg.stdout) if pg.returncode==0 else {'collector_error':True},container=json.loads(stats.stdout) if stats.returncode==0 else {'collector_error':True}))
    output.write_text(json.dumps({'measurement':'PostgreSQL container resources and cumulative stats; sampling subprocess overhead and 2s docker stats interval; no query text or credentials','samples':samples},indent=2)+'\n')
    time.sleep(max(0,2-(time.time()-started)))
