#!/usr/bin/env python3
"""Disposable, isolated Compose deployment/recovery drill (Docker + Python stdlib)."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import urllib.request
import uuid

if not __debug__:
    raise SystemExit('Run without Python optimization; validation assertions are required')

ROOT = Path(__file__).resolve().parents[1]
PROJECT = os.environ.get('AQ_GRAPH_SMOKE_PROJECT', 'aq-graph-validation')
PORT = int(os.environ.get('AQ_GRAPH_SMOKE_PORT', '18746'))
IMAGE = os.environ.get('AQ_GRAPH_SMOKE_IMAGE', 'aq-graph:validation')
if not re.fullmatch(r'aq-graph-validation[a-z0-9_-]*', PROJECT) or not (1 <= PORT <= 65535) or PORT == 8741:
    raise SystemExit('Use a dedicated validation project and nondefault port')
lock = open('/tmp/' + PROJECT + '.lock', 'w')
fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
ENV = dict({k:v for k,v in os.environ.items() if not k.startswith('COMPOSE_')}, COMPOSE_PROJECT_NAME=PROJECT, AQ_GRAPH_PORT=str(PORT), AQ_GRAPH_IMAGE=IMAGE)

def run(*args, check=True, capture=False):
    return subprocess.run(args, cwd=work, env=ENV, check=check, text=True,
                          stdout=subprocess.PIPE if capture else None,
                          stderr=subprocess.PIPE if capture else None)

def compose(*args, **kwargs):
    return run('docker', 'compose', '--project-directory', str(work), '--file', str(work / 'compose.yaml'), '--project-name', PROJECT, *args, **kwargs)

work = Path(tempfile.mkdtemp(prefix='aq-graph-deployment-'))
started = time.monotonic()
owned = False
try:
    # Never attach to or clean up a stack that existed before this invocation.
    existing = subprocess.check_output(['docker', 'ps', '-aq', '--filter', f'label=com.docker.compose.project={PROJECT}'], text=True)
    volumes = subprocess.check_output(['docker', 'volume', 'ls', '-q', '--filter', f'label=com.docker.compose.project={PROJECT}'], text=True)
    networks = subprocess.check_output(['docker', 'network', 'ls', '-q', '--filter', f'label=com.docker.compose.project={PROJECT}'], text=True)
    if existing.strip() or volumes.strip() or networks.strip():
        raise SystemExit('Validation project already exists; refusing to touch it')
    for name in ('compose.yaml',):
        shutil.copy(ROOT / name, work / name)
    (work / 'scripts').mkdir()
    for name in ('local-init.sh', 'backup.sh', 'restore.sh'):
        shutil.copy(ROOT / 'scripts' / name, work / 'scripts' / name)
    run('bash', 'scripts/local-init.sh')
    assert (work / '.local').stat().st_mode & 0o777 == 0o700
    assert (work / '.local/token').stat().st_mode & 0o777 == 0o600
    assert json.loads((work / '.local/config.json').read_text())['endpoint'] == f'http://127.0.0.1:{PORT}'
    secrets = {p.name: p.read_bytes() for p in (work / '.local').iterdir()}
    run('bash', 'scripts/local-init.sh')
    assert secrets == {p.name: p.read_bytes() for p in (work / '.local').iterdir()}
    if not os.environ.get('AQ_GRAPH_SMOKE_SKIP_BUILD'):
        subprocess.run(['docker', 'build', '-t', IMAGE, str(ROOT)], check=True)
    compose('config', '--quiet')
    owned = True
    compose('up', '-d', '--no-build', '--wait', '--wait-timeout', '180')
    graph = compose('ps', '-q', 'graph', capture=True).stdout.strip()
    inspection = json.loads(subprocess.check_output(['docker', 'inspect', graph], text=True))[0]
    assert inspection['Config']['Image'] == IMAGE
    assert inspection['HostConfig']['ReadonlyRootfs']
    assert inspection['Config']['User'].split(':')[0] != '0'
    assert compose('exec', '-T', 'graph', 'sh', '-c', 'touch /must-fail', check=False, capture=True).returncode != 0
    pg = compose('ps', '-q', 'postgres', capture=True).stdout.strip()
    pg_info = json.loads(subprocess.check_output(['docker', 'inspect', pg], text=True))[0]
    assert not pg_info['HostConfig']['PortBindings']
    token = (work / '.local/token').read_text()
    base = f'http://127.0.0.1:{PORT}/v1/workspaces/00000000-0000-4000-8000-000000000001'
    def api(path, data=None):
        req = urllib.request.Request(base + path, data=json.dumps(data).encode() if data is not None else None,
            headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=15) as response:
            return json.load(response)
    def mutate(key, operations):
        return api('/mutations', {'idempotency_key': key, 'operations': operations})
    a, b, edge, child, parent_edge = (str(uuid.uuid4()) for _ in range(5))
    request = {'idempotency_key':'drill-create', 'operations':[
        {'op':'create_node','id':a,'kind':'task','title':'Recovery task','task_state':'active'},
        {'op':'create_node','id':b,'kind':'knowledge','title':'Recovery evidence'},
        {'op':'create_node','id':child,'kind':'task','title':'Recovery child','task_state':'active'},
        {'op':'add_edge','id':edge,'source':a,'target':b,'edge_type':'references','target_revision':1},
        {'op':'add_edge','id':parent_edge,'source':a,'target':child,'edge_type':'parent-child'}]}
    receipt = api('/mutations', request)
    n = api('/nodes/' + a)
    mutate('drill-summary', [{'op':'set_summary','id':a,'expected_version':n['version'], 'source_revision':n['content_revision'],'summary':'Retained summary'}])
    draft = api('/changesets', {'idempotency_key':'drill-draft','name':'Retained proposal','operations':[
        {'op':'update_node','id':b,'expected_version':1,'patch':{'body':'Proposed edit'}}]})
    draft_id = draft.get('id', draft.get('change_set_id'))
    assert draft_id, draft
    paths = ['/nodes/'+a, '/nodes/'+b, '/nodes/'+a+'/history?limit=100', '/nodes/'+a+'/revisions/2',
             '/nodes/'+a+'/revisions/3', '/nodes/'+a+'/revisions/4', '/nodes/'+a+'/diff?from=1&to=3',
             '/nodes/'+child, '/ready?limit=20', '/changesets/'+draft_id]
    expected = {p:api(p) for p in paths}
    assert [n['id'] for n in expected['/ready?limit=20']['items']] == [child]
    def verify():
        assert {p:api(p) for p in paths} == expected
        assert api('/mutations', request) == receipt
    compose('restart', 'graph')
    compose('up', '-d', '--no-build', '--wait', '--wait-timeout', '120')
    verify()
    compose('up', '-d', '--no-build', '--force-recreate', 'graph', '--wait', '--wait-timeout', '120')
    verify()
    def database_snapshot(database):
        tables = compose('exec','-T','postgres','psql','-U','aq_graph','-d',database,'-Atc',
            "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename",capture=True).stdout.splitlines()
        return {table: compose('exec','-T','postgres','psql','-U','aq_graph','-d',database,'-Atc',
            f"SELECT count(*), md5(COALESCE(string_agg(row_to_json(t)::text, chr(10) ORDER BY row_to_json(t)::text), '')) FROM \"{table}\" t",capture=True).stdout.strip() for table in tables}
    source_snapshot = database_snapshot('aq_graph')
    run('bash', 'scripts/backup.sh', '.local/drill.dump')
    assert run('bash', 'scripts/backup.sh', '.local/drill.dump', check=False, capture=True).returncode != 0
    (work / '.local/checksum-only.dump.sha256').write_text('must remain')
    assert run('bash','scripts/backup.sh','.local/checksum-only.dump',check=False,capture=True).returncode != 0
    assert (work / '.local/checksum-only.dump.sha256').read_text() == 'must remain'
    # Corrupt checksum and valid-checksum invalid archive both fail safely.
    invalid = work / '.local/invalid.dump'
    invalid.write_bytes(b'not a PostgreSQL archive')
    Path(str(invalid)+'.sha256').write_text('0'*64+'\n')
    assert run('bash','scripts/restore.sh',str(invalid),'aq_graph_restore_bad_checksum',check=False,capture=True).returncode != 0
    Path(str(invalid)+'.sha256').write_text(hashlib.sha256(invalid.read_bytes()).hexdigest()+'\n')
    assert run('bash','scripts/restore.sh',str(invalid),'aq_graph_restore_bad_archive',check=False,capture=True).returncode != 0
    run('bash','scripts/restore.sh','.local/drill.dump','aq_graph_restore_drill')
    assert run('bash','scripts/restore.sh','.local/drill.dump','aq_graph_restore_drill',check=False,capture=True).returncode != 0
    assert run('bash','scripts/restore.sh','.local/drill.dump','aq_graph',check=False,capture=True).returncode != 0
    assert database_snapshot('aq_graph_restore_drill') == source_snapshot
    # Run the same image against the clone, without bootstrap, then verify exact API values.
    compose('stop','graph')
    compose('run','-d','--no-deps','--service-ports','--name',PROJECT+'-restored','--entrypoint','/bin/sh','graph','-ec',
        'unset AQ_GRAPH_BOOTSTRAP_WORKSPACE AQ_GRAPH_BOOTSTRAP_ACTOR AQ_GRAPH_BOOTSTRAP_TOKEN_FILE; export DATABASE_URL="postgres://aq_graph:$(cat /run/secrets/postgres_password)@postgres:5432/aq_graph_restore_drill"; exec aq-graph serve')
    for _ in range(60):
        try:
            verify()
            break
        except (OSError, AssertionError):
            time.sleep(1)
    else:
        raise AssertionError('Restored service did not verify')
    restored_write = mutate('drill-restored-write',[{'op':'create_node','kind':'task','title':'Write on restored clone'}])
    run('docker','restart',PROJECT+'-restored')
    for _ in range(60):
        try:
            assert mutate('drill-restored-write',[{'op':'create_node','kind':'task','title':'Write on restored clone'}]) == restored_write
            verify()
            break
        except OSError:
            time.sleep(1)
    else:
        raise AssertionError('Restored restart did not verify')
    print(json.dumps({'result':'passed','image':IMAGE,'project':PROJECT,'port':PORT,
                      'checks':['credential rerun','nonroot/read-only','private PostgreSQL','fresh start','restart','same-schema recreate','exact revisions/links/history/draft/summary','ready leaf projection','receipt replay','backup/restore','invalid restore','existing destination refusal','restored write'],
                      'database_snapshot': source_snapshot,
                      'image_id': inspection['Image'],
                      'postgres_version': compose('exec','-T','postgres','psql','-U','aq_graph','-d','aq_graph','-Atc','SHOW server_version',capture=True).stdout.strip(),
                      'elapsed_seconds':round(time.monotonic()-started,2)}, indent=2))
finally:
    if owned:
        # Compose labels restrict cleanup to resources created by this invocation.
        compose('down','--volumes','--remove-orphans')
    shutil.rmtree(work)
