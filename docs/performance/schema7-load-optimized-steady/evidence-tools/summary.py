import pathlib,json
p=pathlib.Path('docs/performance/schema7-load-optimized-steady')
report=json.loads((p/'results.json').read_text());before=json.loads((p/'database-before.json').read_text());after=json.loads((p/'database-after.json').read_text())
write_names={'independent_write','hot_write','same_key_retry','dependency_write'}
phases=[]
for phase in report['phases']+report.get('open_loop',{}).get('phases',[]):
    writes=sum(v['count'] for k,v in phase['operations'].items() if k in write_names)
    phases.append(dict(name=phase['name'],completed=phase['completed'],completed_writes=writes,completed_reads=phase['completed']-writes,write_fraction=writes/phase['completed'] if phase['completed'] else None,errors=phase['errors'],failed_offers=phase['failed_offers'],throughput=phase['requests_per_second'],p50_ms=phase['all_request_latency']['p50_ms'],p95_ms=phase['all_request_latency']['p95_ms'],p99_ms=phase['all_request_latency']['p99_ms'],statuses=phase['statuses'],client_queue_dropped=phase['client_queue_dropped']))
resources={}
for name in ('service','generator'):
    x=json.loads((p/(name+'-resources.json')).read_text());samples=x['samples'];resources[name]=dict(peak_observed_rss_kib=x['peak_rss_kib'],peak_high_water_rss_kib=max(s.get('high_water_rss_kib',0) for s in samples),cpu_seconds_observed=samples[-1]['cpu_seconds']-samples[0]['cpu_seconds'],peak_cpu_percent_of_one_core=max(s.get('cpu_percent_of_one_core',0) for s in samples),max_threads=max(s['threads'] for s in samples))
pg=json.loads((p/'postgres-resources.json').read_text())['samples'];resources['postgres']=dict(peak_container_cpu_percent=sum([]) if not pg else max(float(s['container'].get('CPUPerc','0%').strip('%')) for s in pg),container_memory_samples=[s['container'].get('MemUsage') for s in pg],wait_event_samples=[{'epoch_seconds':s['epoch_seconds'],'activity':s['postgres'].get('activity')} for s in pg if any(a.get('wait_event_type') not in (None,'Client') for a in (s['postgres'].get('activity') or []))])
summary=dict(phases=phases,resources=resources,growth=dict(database_bytes=after['database_bytes']-before['database_bytes'],counts={k:after['counts'][k]-v for k,v in before['counts'].items()},tables={r['relname']:r['total_bytes']-next(x['total_bytes'] for x in before['tables'] if x['relname']==r['relname']) for r in after['tables']}))
(p/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary['phases'],indent=2));print('growth',summary['growth'])
