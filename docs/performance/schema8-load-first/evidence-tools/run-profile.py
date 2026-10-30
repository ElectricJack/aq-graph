#!/usr/bin/env python3
"""Explicitly invoked full schema8 profile; never starts service or changes PG settings."""
import argparse, importlib.util, json, os, pathlib, shutil, subprocess, sys, time
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--output',required=True)
p.add_argument('--fixture',required=True)
p.add_argument('--service-pid',type=int,required=True)
p.add_argument('--service-log',required=True)
p.add_argument('--endpoint',default='http://127.0.0.1:18747')
p.add_argument('--condition',required=True)
p.add_argument('--configuration-json',help='File containing declared service settings; no secrets')
p.add_argument('--maintenance-after-seconds',type=float,help='Optional third profile: vacuum during warm_200 phase after this many seconds')
a=p.parse_args()
if a.maintenance_after_seconds is not None and not 0<=a.maintenance_after_seconds<20:p.error('maintenance delay must be 0..20 seconds to leave measured overlap time')
root=pathlib.Path(__file__).resolve().parent; tools=root/'evidence-tools';out=pathlib.Path(a.output).resolve()
if out.exists() and any(out.iterdir()):p.error('Output directory must be empty to preserve prior evidence')
out.mkdir(parents=True,exist_ok=True)
env=dict(os.environ,AQ_BENCH_HELPER=str(root),AQ_BENCH_OUTPUT=str(out),AQ_BENCH_SERVICE_LOG=str(pathlib.Path(a.service_log).resolve()))
config=json.loads(pathlib.Path(a.configuration_json).read_text()) if a.configuration_json else {}
config.update(fixture_condition=a.condition,transport='loopback HTTP; no TLS',postgres_memory_tuning='unchanged by profile helper',observation='boundary SQL, 2s PostgreSQL/container samples and 1s service/generator samples')
env['AQ_BENCH_CONFIGURATION']=json.dumps(config)
# Guard the target URL without writing it to reports or process arguments.
spec=importlib.util.spec_from_file_location('benchmark','scripts/benchmark.py');b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
dbargs=argparse.Namespace(database_url_env='DATABASE_URL',psql=str(root/'psql'))
b.database_sql(dbargs)
shutil.copy2(a.fixture,out/'fixture.json');shutil.copy2('scripts/benchmark.py',out/'benchmark-measured.py')
shutil.copytree(tools,out/'evidence-tools',ignore=shutil.ignore_patterns('__pycache__'))
shutil.copy2(__file__,out/'evidence-tools'/'run-profile.py')
def tool(name,*args):subprocess.run([sys.executable,str(tools/(name+'.py')),*args],env=env,check=True)
tool('metadata');tool('snapshot','database-before')
(root/'stop-collect').unlink(missing_ok=True)
args=[sys.executable,'scripts/benchmark.py','run','--endpoint',a.endpoint,'--manifest',str(out/'fixture.json'),'--token-file',str(root/'token'),'--duration','30','--warmup','10','--concurrency','50','100','200','--rate','200','--burst-seconds','10','--open-workers','200','--timeout','12','--capture-database-state','--psql',str(root/'psql'),'--output',str(out/'results.json')]
(out/'commands.json').write_text(json.dumps({'benchmark':args,'helper_argv':sys.argv,'database_url':'inherited DATABASE_URL; omitted','maintenance_after_seconds':a.maintenance_after_seconds},indent=2)+'\n')
collector=subprocess.Popen([sys.executable,str(tools/'collect.py')],env=env,stdout=(out/'collector.log').open('w'),stderr=subprocess.STDOUT)
load=subprocess.Popen(args,env=env,stdout=(out/'run.log').open('w'),stderr=subprocess.STDOUT)
monitors=[subprocess.Popen([sys.executable,'scripts/observe.py','--pid',str(pid),'--seconds','1800','--output',str(out/(name+'.json'))]) for pid,name in [(a.service_pid,'service-resources'),(load.pid,'generator-resources')]]
maintenance=None
try:
 if a.maintenance_after_seconds is not None:
  # Detect the phase-start progress line, not a guessed time since process launch.
  while load.poll() is None and 'Running warm_200:' not in (out/'run.log').read_text():time.sleep(.1)
  if load.poll() is not None:raise RuntimeError('Load ended before maintenance overlap phase')
  time.sleep(a.maintenance_after_seconds)
  maintenance=subprocess.Popen([sys.executable,'scripts/benchmark.py','maintain','--psql',str(root/'psql'),'--output',str(out/'maintenance.json')],env=env,stdout=(out/'maintenance.log').open('w'),stderr=subprocess.STDOUT)
 code=load.wait()
 if maintenance and maintenance.wait()!=0:raise RuntimeError('Maintenance failed')
finally:
 if load.poll() is None:load.terminate();load.wait()
 (root/'stop-collect').touch()
 try:
  collector_code=collector.wait(timeout=15)
 finally:
  if collector.poll() is None:collector.terminate();collector.wait()
  for monitor in monitors:monitor.terminate();monitor.wait()
 if collector_code:raise RuntimeError('PostgreSQL collector failed; inspect collector.log')
tool('snapshot','database-after')
if code:raise SystemExit(code)
tool('service-summary')
# Read-only diagnostic plans run after measured workload, never before fresh load.
tool('prepared-plans')
print('Completed',out)
