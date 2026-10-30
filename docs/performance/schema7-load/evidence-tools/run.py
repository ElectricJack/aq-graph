import pathlib,subprocess,time,json,os
p=pathlib.Path('.local/schema7-load'); out=pathlib.Path('docs/performance/schema7-load')
subprocess.run(['python3',str(p/'metadata.py')],check=True)
subprocess.run(['python3',str(p/'snapshot.py'),'database-before'],check=True)
(p/'stop-collect').unlink(missing_ok=True)
collector=subprocess.Popen(['python3',str(p/'collect.py')],stdout=(p/'collector.log').open('w'),stderr=subprocess.STDOUT)
args=['python3','scripts/benchmark.py','run','--endpoint','http://127.0.0.1:18745','--manifest',str(out/'fixture.json'),'--token-file',str(p/'token'),'--duration','30','--warmup','10','--concurrency','50','100','200','--rate','200','--burst-seconds','10','--open-workers','200','--timeout','12','--output',str(out/'results.json')]
(out/'commands.json').write_text(json.dumps({'benchmark':args,'seed':['python3','scripts/benchmark.py','seed','--nodes','100000','--edges','500000','--token-file',str(p/'token'),'--psql',str(p/'psql'),'--manifest',str(out/'fixture.json')],'note':'DATABASE_URL supplied only through environment for seed; local psql wrapper delegates to dedicated container'},indent=2)+'\n')
load=subprocess.Popen(args,stdout=(out/'run.log').open('w'),stderr=subprocess.STDOUT)
monitors=[]
for pid,name in [(int((p/'service.pid').read_text()),'service-resources'),(load.pid,'generator-resources')]:
    monitors.append(subprocess.Popen(['python3','scripts/observe.py','--pid',str(pid),'--seconds','600','--output',str(out/(name+'.json'))]))
code=load.wait()
(p/'stop-collect').touch()
collector.wait(timeout=10)
for monitor in monitors:
    monitor.terminate();monitor.wait()
subprocess.run(['python3',str(p/'snapshot.py'),'database-after'],check=True)
print('benchmark exit',code)
raise SystemExit(code)
