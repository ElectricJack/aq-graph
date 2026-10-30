import pathlib,json,importlib.util,types,urllib.parse
spec=importlib.util.spec_from_file_location('benchmark','scripts/benchmark.py');b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
p=pathlib.Path('.local/schema7-load-optimized');out=pathlib.Path('docs/performance/schema7-load-optimized');manifest=json.loads((out/'fixture.json').read_text())
w=b.Worker(types.SimpleNamespace(endpoint='http://127.0.0.1:18745',timeout=12,token=(p/'token').read_text().strip()),manifest,0)
results=[]
for name,term in [('common','authentication'),('selective','12345'),('missing','absentbenchmarktermzz')]:
    samples=[]
    for _ in range(10):
        status,value,elapsed,size=w.request('GET','/search?q='+urllib.parse.quote(term)+'&limit=20')
        samples.append(dict(operation=name,status=str(status),error_code=value.get('error',{}).get('code'),latency_ms=elapsed*1000,bytes=size))
    results.append(dict(name=name,term=term,samples=samples,summary=b.summarize(samples,sum(s['latency_ms'] for s in samples)/1000)))
w.close()
(out/'search-probes.json').write_text(json.dumps({'method':'Ten sequential warm HTTP requests per term after main workload; diagnostic selectivity comparison, not sustained throughput or target acceptance','results':results},indent=2)+'\n')
