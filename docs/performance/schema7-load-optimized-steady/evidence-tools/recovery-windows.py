import pathlib,json,gzip,datetime,re,importlib.util
out=pathlib.Path('docs/performance/schema7-load-optimized-steady');report=json.loads((out/'results.json').read_text())
spec=importlib.util.spec_from_file_location('benchmark','scripts/benchmark.py');b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
rows=[]
for line in gzip.decompress((out/'service-requests.log.gz').read_bytes()).decode().splitlines():
 if 'request completed' not in line:continue
 epoch=datetime.datetime.fromisoformat(line.split()[0].replace('Z','+00:00')).timestamp();fields=dict(re.findall(r'(method|route|status|duration_ms)=([^ ]+)',line))
 rows.append(dict(epoch=epoch,operation=fields['method']+' '+fields['route'],status=fields['status'],latency_ms=float(fields['duration_ms']),error_code=None,bytes=0))
phase=report['open_loop']['phases'][-1];start=phase['started_epoch_seconds'];windows=[]
for offset in (0,10,20):
 samples=[row for row in rows if start+offset<=row['epoch']<start+offset+10]
 windows.append(dict(start_offset_seconds=offset,end_offset_seconds=offset+10,**b.summarize(samples,10)))
(out/'recovery-service-windows.json').write_text(json.dumps({'method':'Ten-second non-overlapping service-completion windows after recovery arrivals begin; can include prior burst requests, does not measure request-origin client queue drain','windows':windows},indent=2)+'\n')
print([(w['start_offset_seconds'],w['completed'],w['all_request_latency']) for w in windows])
