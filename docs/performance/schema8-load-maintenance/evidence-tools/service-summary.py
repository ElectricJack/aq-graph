import os
import pathlib,json,re,datetime,gzip,importlib.util,collections
p=pathlib.Path(os.environ['AQ_BENCH_HELPER']);out=pathlib.Path(os.environ['AQ_BENCH_OUTPUT'])
spec=importlib.util.spec_from_file_location('benchmark','scripts/benchmark.py');b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
report=json.loads((out/'results.json').read_text())
raw=pathlib.Path(os.environ['AQ_BENCH_SERVICE_LOG']).read_bytes()
minimum=datetime.datetime.fromisoformat(report['started_utc'].replace('Z','+00:00')).timestamp()
raw=b'\n'.join(line for line in raw.splitlines() if line and datetime.datetime.fromisoformat(line.split()[0].decode().replace('Z','+00:00')).timestamp()>=minimum)+b'\n'
with gzip.open(out/'service-requests.log.gz','wb') as f:f.write(raw)
rows=[]
for line in raw.decode().splitlines():
    if 'request completed' not in line:continue
    timestamp=line.split()[0]
    epoch=datetime.datetime.fromisoformat(timestamp.replace('Z','+00:00')).timestamp()
    fields=dict(re.findall(r'(method|route|status|duration_ms)=([^ ]+)',line))
    if not fields:continue
    rows.append(dict(epoch=epoch,route=fields['route'],method=fields['method'],status=fields['status'],latency_ms=float(fields['duration_ms']),error_code=None,operation=fields['method']+' '+fields['route'],bytes=0))
windows=[]
for phase in report['phases']:
    windows.append((phase['name'],phase['started_epoch_seconds'],phase['finished_epoch_seconds']))
for phase in report['open_loop']['phases']:
    windows.append(('open_'+phase['name']+'_arrival_window',phase['started_epoch_seconds'],phase['arrival_finished_epoch_seconds']))
last=report['open_loop']['phases'][-1]
if last['tail_seconds']>0:windows.append(('open_drain',last['arrival_finished_epoch_seconds'],last['completion_finished_epoch_seconds']))
phases=[]
for name,start,end in windows:
    samples=[r for r in rows if start<=r['epoch']<end]
    phases.append(dict(name=name,start_epoch_seconds=start,end_epoch_seconds=end,**b.summarize(samples,end-start)))
(out/'service-latency.json').write_text(json.dumps({'measurement':'Service guard entry through response body serialization at request completion; includes admission/auth/DB/pool but not socket delivery. Assigned by completion timestamp, not request origin phase. Open-loop groups use non-overlapping arrival windows, may include prior phase tail. Mutation route combines all mutation subtypes. Raw logs retained compressed.','unassigned_requests':sum(not any(start<=r['epoch']<end for _,start,end in windows) for r in rows),'phases':phases},indent=2)+'\n')
print('service request logs',len(rows))
maintenance_file=out/'maintenance.json'
if maintenance_file.exists():
    maintenance=json.loads(maintenance_file.read_text())
    start=maintenance['started_epoch_seconds'];end=maintenance.get('finished_epoch_seconds',start)
    overlap=[r for r in rows if start<=r['epoch']<=end and r['method']=='POST' and r['status'].startswith('2')]
    (out/'maintenance-overlap.json').write_text(json.dumps({'start_epoch_seconds':start,'end_epoch_seconds':end,'successful_mutation_completions_during_maintenance':len(overlap),'successful_independent_writes_proven':False,'limitation':'Service route logs combine mutation subtypes; successful mutation completions can include replay/no-op. Independent-write overlap needs subtype or history evidence.'},indent=2)+'\n')
