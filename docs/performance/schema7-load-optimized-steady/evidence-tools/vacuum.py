import subprocess,time,pathlib,json
out=pathlib.Path('docs/performance/schema7-load-optimized-steady')
base=['docker','exec','aq-graph-schema7-load-pg','psql','-U','postgres','-d','aq_graph_bench_schema7','-qAtc']
query="""SELECT json_build_object('utc',clock_timestamp(),'tables',(SELECT json_agg(t) FROM (SELECT c.relname,c.relpages,c.relallvisible,c.reltuples,s.n_live_tup,s.n_dead_tup,s.n_ins_since_vacuum,s.n_mod_since_analyze,s.last_autovacuum,s.autovacuum_count,s.last_vacuum,s.vacuum_count,s.last_analyze,s.analyze_count FROM pg_class c JOIN pg_stat_user_tables s ON s.relid=c.oid WHERE c.relnamespace='public'::regnamespace AND c.relkind='r' ORDER BY c.relname) t),'vacuum_progress',(SELECT json_agg(t) FROM pg_stat_progress_vacuum t WHERE datname=current_database()),'activity',(SELECT json_agg(t) FROM (SELECT state,wait_event_type,wait_event,count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() GROUP BY 1,2,3) t))"""
def snapshot():return json.loads(subprocess.check_output(base+[query],text=True))
before=snapshot();started_epoch=time.time();started=time.monotonic()
r=subprocess.run(base+['VACUUM (ANALYZE, VERBOSE);'],capture_output=True,text=True)
elapsed=time.monotonic()-started
(out/'maintenance.log').write_text(r.stdout+r.stderr)
after=snapshot()
(out/'maintenance.json').write_text(json.dumps(dict(command=base+['VACUUM (ANALYZE, VERBOSE);'],started_epoch_seconds=started_epoch,finished_epoch_seconds=time.time(),elapsed_seconds=elapsed,returncode=r.returncode,before=before,after=after,method='Explicit all-table benchmark database VACUUM ANALYZE after aborted load and before steady-state followup; no graph logical mutations, no validation database changes. Creates a declared warm/analyzed/all-visible condition, not a controlled comparison to baseline.'),indent=2)+'\n')
print('VACUUM exit',r.returncode,'seconds',elapsed)
print([(t['relname'],t['relpages'],t['relallvisible'],t['n_dead_tup']) for t in after['tables']])
raise SystemExit(r.returncode)
