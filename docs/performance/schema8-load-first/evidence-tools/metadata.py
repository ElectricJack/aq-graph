import os
import subprocess,json,pathlib,hashlib,os,platform,importlib.util,time
out=pathlib.Path(os.environ['AQ_BENCH_OUTPUT'])
def cmd(args):return subprocess.check_output(args,text=True).strip()
files=sorted(set(cmd(['git','ls-files','src','migrations','Cargo.toml','Cargo.lock','scripts/benchmark.py','scripts/observe.py']).splitlines()) | {str(f) for root in ('src','migrations') for f in pathlib.Path(root).rglob('*') if f.is_file()})
h=hashlib.sha256()
for f in sorted(files):h.update(f.encode()+b'\0'+pathlib.Path(f).read_bytes()+b'\0')
spec=importlib.util.spec_from_file_location('benchmark','scripts/benchmark.py');b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
metadata=dict(captured_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),git_commit=cmd(['git','rev-parse','HEAD']),dirty_paths=cmd(['git','status','--porcelain']).splitlines(),source_sha256=h.hexdigest(),source_files=sorted(files),source_digest_method='SHA256 over sorted tracked plus untracked src,migrations,Cargo.toml,Cargo.lock,benchmark.py,observe.py; path NUL content NUL',binary_sha256=hashlib.sha256(pathlib.Path('target/release/aq-graph').read_bytes()).hexdigest(),hardware=b.hardware(),rustc=cmd(['rustc','--version']),cargo=cmd(['cargo','--version']),filesystem=cmd(['findmnt','-T','.', '-o','SOURCE,FSTYPE,OPTIONS']),disk=cmd(['lsblk','-e','7','-o','NAME,MODEL,TYPE,SIZE,ROTA']),docker_version=cmd(['docker','--version']),postgres_image=cmd(['docker','inspect','--format','{{.Image}}','aq-graph-schema8-pg']),postgres_limits=json.loads(cmd(['docker','inspect','--format','{"memory":{{.HostConfig.Memory}},"nano_cpus":{{.HostConfig.NanoCpus}},"cpu_period":{{.HostConfig.CpuPeriod}},"cpu_quota":{{.HostConfig.CpuQuota}}}','aq-graph-schema8-pg'])),configuration=json.loads(os.environ.get('AQ_BENCH_CONFIGURATION','{}')))
(out/'environment.json').write_text(json.dumps(metadata,indent=2)+'\n')
