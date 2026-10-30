import pathlib,json
root=pathlib.Path('docs/performance');before=json.loads((root/'schema7-load/results.json').read_text());after=json.loads((root/'schema7-load-optimized/results.json').read_text())
rows=[]
for a,b in zip(before['phases']+before['open_loop']['phases'],after['phases']+after['open_loop']['phases']):
 assert a['name']==b['name']
 rows.append(dict(name=a['name'],baseline=dict(attempts_per_second=a['requests_per_second'],statuses=a['statuses'],p95_ms=a['all_request_latency']['p95_ms'],p99_ms=a['all_request_latency']['p99_ms'],client_queue_dropped=a['client_queue_dropped']),optimized=dict(attempts_per_second=b['requests_per_second'],statuses=b['statuses'],p95_ms=b['all_request_latency']['p95_ms'],p99_ms=b['all_request_latency']['p99_ms'],client_queue_dropped=b['client_queue_dropped']),throughput_ratio=b['requests_per_second']/a['requests_per_second'],p95_ratio=b['all_request_latency']['p95_ms']/a['all_request_latency']['p95_ms']))
result=dict(method='Sequential runs on fresh semantically identical deterministic fixture, same hardware/workload/durability; query/JIT runtime changes plus minor shared-clock harness correction; no randomization or repeated statistical trials, no causal isolation among combined optimizations',phases=rows)
(root/'schema7-load-optimized/comparison.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
