#!/usr/bin/env python3
"""Reproducible offline fixture + bounded mixed HTTP load; Python 3.10+ stdlib only."""
import argparse
import concurrent.futures
import hashlib
import http.client
import json
import math
import os
import pathlib
import platform
import random
import socket
import statistics
import subprocess
import threading
import time
import urllib.parse
import uuid
from collections import Counter, defaultdict

WORKSPACE = '00000000-0000-4000-8000-000000000002'
CHANGESET = '00000000-0000-4000-8000-000000000003'

def node_id(index):
    return str(uuid.UUID(int=index))

def seed(args):
    if args.nodes < 100 or args.edges < 0 or args.edges > args.nodes * 5:
        raise SystemExit('Use at least 100 nodes and at most 5 edges per node.')
    token = credential(args)
    database_url = os.environ.get(args.database_url_env)
    if not database_url:
        raise SystemExit(f'{args.database_url_env} is required')
    database=urllib.parse.urlsplit(database_url)
    if database.scheme not in ('postgres','postgresql') or not database.hostname:
        raise SystemExit('Benchmark DATABASE_URL must be a PostgreSQL URL with host and database')
    env=dict(os.environ)
    for variable in ('PGHOST','PGPORT','PGUSER','PGPASSWORD','PGDATABASE','PGOPTIONS','PGSERVICE'):
        env.pop(variable,None)
    env.update(PGHOST=database.hostname,PGPORT=str(database.port or 5432),PGDATABASE=urllib.parse.unquote(database.path.lstrip('/')),PGOPTIONS='-c timezone=UTC')
    if database.username is not None:
        env['PGUSER']=urllib.parse.unquote(database.username)
    if database.password is not None:
        env['PGPASSWORD']=urllib.parse.unquote(database.password)
    query=urllib.parse.parse_qs(database.query)
    for name,variable in (('sslmode','PGSSLMODE'),('sslrootcert','PGSSLROOTCERT'),('sslcert','PGSSLCERT'),('sslkey','PGSSLKEY')):
        if name in query:
            env[variable]=query[name][-1]
    def sql(statement):
        return subprocess.check_output([args.psql, '-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1', '-c', statement], env=env, text=True).strip()
    database = sql('SELECT current_database()')
    if not database.startswith('aq_graph_bench'):
        raise SystemExit('Refusing fixture writes: dedicated database must start aq_graph_bench')
    if sql('SELECT count(*) FROM nodes') != '0':
        raise SystemExit('Refusing fixture writes: database already contains nodes; use a fresh benchmark database')
    task_count = args.nodes * 4 // 5
    root_count = max(1, task_count // 3)
    # MD5-derived UUIDs are deterministic fixture identities, never credentials.
    # Sequential node UUIDs let independent benchmark clients select identical data.
    script = f"""
BEGIN;
INSERT INTO workspaces(id) VALUES ('{WORKSPACE}') ON CONFLICT DO NOTHING;
INSERT INTO credentials(workspace_id,actor_id,token_hash,capability)
VALUES('{WORKSPACE}','benchmark','{hashlib.sha256(token.encode()).hexdigest()}','admin')
ON CONFLICT(workspace_id,actor_id) DO UPDATE SET token_hash=excluded.token_hash;
INSERT INTO change_sets(id,workspace_id,actor_id,name,reason,operations,status,result,applied_at)
VALUES('{CHANGESET}','{WORKSPACE}','benchmark','offline synthetic fixture','Benchmark only: generated full snapshots and edge history','[]','applied','{{}}',clock_timestamp());
INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,labels,metadata,task_state,content_type,verification,retired,provenance,created_at,updated_at,created_by,valid_from,valid_until,review_at)
SELECT lpad(to_hex(g),32,'0')::uuid,'{WORKSPACE}',CASE WHEN g<={task_count} THEN 'task' ELSE 'knowledge' END,
 CASE WHEN g%10=0 THEN 11 ELSE 1 END,CASE WHEN g%10=0 THEN 11 ELSE 1 END,
 CASE WHEN g<={task_count} THEN 'Implement authentication component ' ELSE 'Authentication decision reference ' END || g,
 repeat('Authentication authorization PostgreSQL reliability evidence and migration procedure. ',CASE WHEN g%20=0 THEN 96 WHEN g%4=0 THEN 24 ELSE 6 END) || 'Record ' || g || ' current revision',
 CASE WHEN g<={task_count} THEN 'Review evidence; implement; verify expected behavior and recovery.' ELSE '' END,
 jsonb_build_array('component-'||(g%30),'priority-'||(g%4)),jsonb_build_object('benchmark:seed',42,'benchmark:index',g),
 CASE WHEN g>{task_count} THEN NULL WHEN g<={root_count*2} THEN CASE WHEN g%20=9 THEN 'draft' ELSE 'active' END WHEN g%10<6 THEN 'completed' WHEN g%10<9 THEN 'active' WHEN g%20=9 THEN 'draft' ELSE 'canceled' END,
 (ARRAY['fact','decision','policy','procedure','incident','reference','note'])[1+g%7],
 'unverified',
 false,
 jsonb_build_array(jsonb_build_object('source_id','benchmark-'||g,'kind','url','url','https://example.invalid/benchmark/'||g,'observed_at','2026-01-01T00:00:00Z','retained',false)),
 '2026-01-01T00:00:00Z'::timestamptz,'2026-01-01T00:00:00Z'::timestamptz,'benchmark',
 CASE WHEN g%4=0 THEN '2026-01-01T00:00:00Z'::timestamptz END,
 CASE WHEN g%4=0 THEN '2027-01-01T00:00:00Z'::timestamptz END,
 CASE WHEN g%5=0 THEN '2026-10-01T00:00:00Z'::timestamptz END
FROM generate_series(1,{args.nodes}) g;
INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id,created_at)
SELECT n.workspace_id,n.id,r,
 (to_jsonb(n)-'search_document') || jsonb_build_object('version',r,'content_revision',r,'body',CASE WHEN r=n.content_revision THEN n.body ELSE n.body || ' historical revision '||r END),
 'benchmark',CASE WHEN r=1 THEN 'create' ELSE 'update' END,'Offline synthetic history','{CHANGESET}',n.created_at+make_interval(secs=>r)
FROM nodes n CROSS JOIN LATERAL generate_series(1,n.content_revision::integer) r
WHERE n.workspace_id='{WORKSPACE}';
INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data,created_at)
SELECT workspace_id,ARRAY[node_id],md5(node_id::text||':'||revision)::uuid,'revision',actor_id,operation,reason,change_set_id,snapshot,created_at FROM node_revisions WHERE workspace_id='{WORKSPACE}';
-- One structural slot per node forms a forest of at most three levels.
-- Each child has one parent, all structural/blocking arcs point to larger UUIDs.
-- Structural parent states are active/draft, never completed with open children.
-- Information edges respect the endpoint matrix and include knowledge hubs.
WITH slots AS (
 SELECT i,1+((i-1)%{args.nodes}) AS s,(i-1)/{args.nodes} AS slot FROM generate_series(1,{args.edges}) i
), endpoints AS (
 SELECT *,CASE WHEN slot%5=4 THEN {task_count}+1+(s%20) ELSE 1+((s+37*(slot+1))%{args.nodes}) END AS t FROM slots
), typed AS (
 SELECT *,CASE WHEN s<={task_count} AND t<={task_count} AND slot%5<2 THEN 'blocks'
 WHEN s>{root_count} AND s<={task_count} AND slot%5=2 THEN 'parent-child'
 WHEN s<={task_count} AND t>{task_count} AND slot%5=4 THEN 'produces' ELSE 'references' END AS typ FROM endpoints
)
INSERT INTO edges(id,workspace_id,source,target,edge_type,version,deleted,created_at)
SELECT md5('edge:'||i)::uuid,'{WORKSPACE}',
 lpad(to_hex(CASE WHEN typ='parent-child' THEN CASE WHEN s<={root_count*2} THEN 1+((s-{root_count}-1)%{root_count}) ELSE {root_count}+1+((s-{root_count*2}-1)%{root_count}) END WHEN typ='blocks' THEN least(s,t) ELSE s END),32,'0')::uuid,
 lpad(to_hex(CASE WHEN typ='parent-child' THEN s WHEN typ='blocks' THEN greatest(s,t) ELSE t END),32,'0')::uuid,
 typ,1,false,'2026-01-01T00:00:00Z' FROM typed WHERE s<>t;
-- Establish a complete current relationship baseline after the synthetic graph
-- is assembled. Earlier synthetic revisions retain NULL/unknown link sets.
INSERT INTO edge_versions(workspace_id,id,version,snapshot)
SELECT workspace_id,id,version,to_jsonb(e) FROM edges e WHERE workspace_id='{WORKSPACE}';
INSERT INTO link_sets(id,workspace_id,node_id)
SELECT md5('benchmark-link-set:'||id)::uuid,workspace_id,id FROM nodes WHERE workspace_id='{WORKSPACE}';
INSERT INTO link_set_members(workspace_id,set_id,edge_id,edge_version)
SELECT s.workspace_id,s.id,e.id,e.version FROM link_sets s
JOIN edges e ON e.workspace_id=s.workspace_id
 AND (e.source=s.node_id OR (e.target=s.node_id AND e.edge_type IN ('blocks','parent-child')))
WHERE s.workspace_id='{WORKSPACE}' AND NOT e.deleted;
UPDATE nodes SET version=version+1,content_revision=content_revision+1,
 updated_at='2026-01-01T00:00:12Z' WHERE workspace_id='{WORKSPACE}';
INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id,link_set_id,created_at)
SELECT n.workspace_id,n.id,n.content_revision,to_jsonb(n)-'search_document','benchmark','baseline',
 'Offline synthetic current-state baseline; earlier relationship sets unavailable','{CHANGESET}',s.id,n.updated_at
FROM nodes n JOIN link_sets s ON s.workspace_id=n.workspace_id AND s.node_id=n.id
WHERE n.workspace_id='{WORKSPACE}';
INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data,created_at)
SELECT n.workspace_id,ARRAY[n.id],md5('benchmark-baseline:'||n.id)::uuid,'revision','benchmark','baseline',
 'Offline synthetic current-state baseline; earlier relationship sets unavailable','{CHANGESET}',to_jsonb(n)-'search_document',n.updated_at
FROM nodes n WHERE n.workspace_id='{WORKSPACE}';
INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data,created_at)
SELECT workspace_id,ARRAY[source,target],md5('history:'||id)::uuid,'edge','benchmark','add_edge','Offline synthetic edge history','{CHANGESET}',to_jsonb(e),created_at FROM edges e WHERE workspace_id='{WORKSPACE}';
COMMIT;
ANALYZE;
SELECT json_build_object('nodes',(SELECT count(*) FROM nodes),'edges',(SELECT count(*) FROM edges),'revisions',(SELECT count(*) FROM node_revisions),'history_entries',(SELECT count(*) FROM history_entries),'edge_versions',(SELECT count(*) FROM edge_versions),'link_sets',(SELECT count(*) FROM link_sets),'link_memberships',(SELECT count(*) FROM link_set_members),'database_bytes',pg_database_size(current_database()),'node_states',(SELECT json_object_agg(state,n) FROM (SELECT COALESCE(task_state,'knowledge:unverified') state,count(*) n FROM nodes GROUP BY 1) states),'edge_types',(SELECT json_object_agg(edge_type,n) FROM (SELECT edge_type,count(*) n FROM edges GROUP BY 1) types),'table_bytes',(SELECT json_object_agg(relname,pg_total_relation_size(oid)) FROM pg_class WHERE relnamespace='public'::regnamespace AND relkind='r'));
"""
    started = time.monotonic()
    result = subprocess.run([args.psql, '-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1'], input=script, env=env, text=True, capture_output=True)
    if result.returncode:
        # Avoid exposing connection-string diagnostics from libpq.
        raise SystemExit('Offline fixture failed. Writes are transactional, but a post-commit analyze/report failure may leave a committed fixture; inspect the isolated database before retrying.')
    counts = json.loads(result.stdout.strip().splitlines()[-1])
    configuration=json.loads(sql("SELECT json_build_object('postgres_version',version(),'fsync',current_setting('fsync'),'synchronous_commit',current_setting('synchronous_commit'),'full_page_writes',current_setting('full_page_writes'),'track_io_timing',current_setting('track_io_timing'),'timezone',current_setting('TimeZone'),'max_connections',current_setting('max_connections'),'schema_version',(SELECT max(version) FROM _sqlx_migrations))"))
    manifest = {'postgres_configuration':configuration,'format_version':4,'baseline_revision_offset':1,'semantic_profile':'schema7 shared fields: common classifications/typed sources/unverified defaults,25percent validity windows,20percent review markers; complete relationship baselines and three-level forest','seed':42,'workspace':WORKSPACE,'nodes':args.nodes,'edges_requested':args.edges,'counts':counts,'fixture_method':'offline SQL, synthetic snapshots and relationship history; not ingestion throughput','elapsed_seconds':time.monotonic()-started}
    pathlib.Path(args.manifest).write_text(json.dumps(manifest, indent=2)+'\n')
    print(json.dumps(manifest, indent=2))

def credential(args):
    if args.token_file:
        path = pathlib.Path(args.token_file)
        if path.stat().st_mode & 0o077:
            raise SystemExit('Token file must be owner-only (chmod 600)')
        value = path.read_text().strip()
    else:
        value = os.environ.get('AQ_GRAPH_TOKEN','').strip()
    if not value:
        raise SystemExit('Set AQ_GRAPH_TOKEN or --token-file; credentials never appear in reports')
    return value

class Worker:
    def __init__(self, args, manifest, index):
        self.args, self.manifest = args, manifest
        self.random = random.Random(42+index)
        self.index = index
        self.counter = 0
        self.node_versions = {}
        self.receipt = None
        self.needs_refresh = set()
        url = urllib.parse.urlsplit(args.endpoint)
        if url.scheme not in ('http','https') or not url.hostname or url.username or url.password:
            raise SystemExit('Endpoint must be HTTP(S) without embedded credentials')
        cls = http.client.HTTPSConnection if url.scheme == 'https' else http.client.HTTPConnection
        self.connection = cls(url.hostname,url.port,timeout=args.timeout)
        self.prefix = url.path.rstrip('/')+f'/v1/workspaces/{manifest["workspace"]}'
        self.token = args.token

    def request(self, method, path, body=None):
        encoded = json.dumps(body).encode() if body is not None else None
        started = time.perf_counter()
        try:
            self.connection.request(method,self.prefix+path,body=encoded,headers={'Authorization':'Bearer '+self.token,'Content-Type':'application/json','X-AQ-Graph-API-Version':'1'})
            response = self.connection.getresponse()
            raw = response.read()
            elapsed = time.perf_counter()-started
            try:
                value = json.loads(raw)
            except ValueError:
                value = {}
            return response.status,value,elapsed,len(raw)
        except (OSError,http.client.HTTPException) as error:
            elapsed = time.perf_counter()-started
            self.connection.close()
            return 'timeout' if isinstance(error,(TimeoutError,socket.timeout)) else 'transport',{},elapsed,0

    def prepare(self):
        records=[]
        for record in (min(self.manifest['nodes'],1+self.index*2),2):
            status,value,elapsed,size=self.request('GET','/nodes/'+node_id(record))
            if isinstance(status,int) and 200<=status<300 and isinstance(value,dict) and 'version' in value:
                self.node_versions[record]=value['version']
            records.append({'operation':'setup_version_get','status':str(status),'error_code':value.get('error',{}).get('code'),'latency_ms':elapsed*1000,'bytes':size})
        return records

    def operation(self):
        self.counter += 1
        if self.needs_refresh:
            record=self.needs_refresh.pop()
            status,value,elapsed,size=self.request('GET','/nodes/'+node_id(record))
            if isinstance(status,int) and 200<=status<300 and isinstance(value,dict) and 'version' in value:
                self.node_versions[record]=value['version']
            else:
                self.needs_refresh.add(record)
            return {'operation':'version_refresh','status':str(status),'error_code':value.get('error',{}).get('code'),'latency_ms':elapsed*1000,'bytes':size}
        choice = self.random.randrange(100)
        total = self.manifest['nodes']
        index = self.random.randint(1,total)
        path = '/nodes/'+node_id(index)
        # Exactly 80% read, 20% write in expectation. Writes include conflicts and
        # idempotent retries; these count as offered operations, not hidden setup.
        if choice < 25:
            kind,method,body = 'get','GET',None
        elif choice < 40:
            kind,method,path,body = 'ready','GET','/ready?limit=20',None
        elif choice < 55:
            kind,method,path,body = 'context','GET',path+'/context?depth=1&limit=30&max_bytes=131072',None
        elif choice < 70:
            kind,method,path,body = 'search','GET','/search?q=authentication&limit=20',None
        elif choice < 80:
            kind,method,path,body = 'history','GET','/nodes/'+node_id(10*self.random.randint(1,total//10))+'/history?limit=20',None
        else:
            method,path = 'POST','/mutations'
            key = str(uuid.uuid4())
            if choice < 90:
                # Disjoint per-worker records; versions are refreshed from successful
                # mutation output. A stale initial version remains a reported error.
                record = min(total, 1+self.index*2)
                version = self.node_versions.get(record,(11 if record%10==0 else 1)+self.manifest.get('baseline_revision_offset',0))
                kind = 'independent_write'
                body = {'idempotency_key':key,'operations':[{'op':'update_node','id':node_id(record),'expected_version':version,'patch':{'title':f'Benchmark worker {self.index} edit {self.counter}'}}]}
                self.receipt = body
            elif choice < 94:
                kind = 'hot_write'
                record = 2
                version = self.node_versions.get(record,1+self.manifest.get('baseline_revision_offset',0))
                body = {'idempotency_key':key,'operations':[{'op':'update_node','id':node_id(record),'expected_version':version,'patch':{'title':f'Hot record {self.index}/{self.counter}'}}]}
            elif choice < 97 and self.receipt:
                kind,body = 'same_key_retry',self.receipt
                record = None
            else:
                kind,record = 'dependency_write',None
                # Opposite competing arcs on two dedicated tasks exercise invariant
                # protection. Duplicate edges/cycles are intentionally visible.
                a,b = (total*4//5-1,total*4//5) if self.index%2 else (total*4//5,total*4//5-1)
                body = {'idempotency_key':key,'operations':[{'op':'add_edge','source':node_id(a),'target':node_id(b),'edge_type':'blocks'}]}
        status,value,elapsed,size = self.request(method,path,body)
        if choice >= 80 and record is not None:
            if isinstance(status,int) and 200<=status<300:
                # API result is the current node, or a result containing its version.
                results = value.get('results',[])
                first = results[0] if results else {}
                if isinstance(first,dict):
                    self.node_versions[record] = first.get('version',version+1)
            elif status == 409:
                # Refresh on the next measured operation. Do not guess version
                # advancement or hide recovery HTTP traffic from request counts.
                self.needs_refresh.add(record)
        code = value.get('error',{}).get('code') if isinstance(value,dict) else None
        return {'operation':kind,'status':str(status),'error_code':code,'latency_ms':elapsed*1000,'bytes':size}

    def close(self):
        self.connection.close()

def percentile(values, percent):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0,math.ceil(percent/100*len(ordered))-1)]

def summarize(samples,elapsed,offered=None,dropped=0):
    latencies = [r['latency_ms'] for r in samples]
    successes = [r['latency_ms'] for r in samples if r['status'].startswith('2')]
    def distribution(values):
        return {f'p{p}_ms':percentile(values,p) for p in (50,95,99)}
    groups = defaultdict(list)
    for sample in samples:
        groups[sample['operation']].append(sample)
    result = {'elapsed_seconds':elapsed,'offered':len(samples)+dropped if offered is None else offered,'completed':len(samples),'client_queue_dropped':dropped,'requests_per_second':len(samples)/elapsed,'success_per_second':len(successes)/elapsed,'errors':len(samples)-len(successes),'failed_offers':len(samples)-len(successes)+dropped,'timeouts':sum(r['status']=='timeout' for r in samples),'statuses':dict(Counter(r['status'] for r in samples)),'error_codes':dict(Counter(r['error_code'] for r in samples if r['error_code'])),'all_request_latency':distribution(latencies),'successful_request_latency':distribution(successes),'response_bytes':sum(r['bytes'] for r in samples),'operations':{}}
    for name,rows in groups.items():
        result['operations'][name] = {'count':len(rows),'errors':sum(not r['status'].startswith('2') for r in rows),'statuses':dict(Counter(r['status'] for r in rows)),'successful_latency':distribution([r['latency_ms'] for r in rows if r['status'].startswith('2')]),**distribution([r['latency_ms'] for r in rows])}
    return result

def fixed(args,manifest,concurrency,duration):
    barrier = threading.Barrier(concurrency+1)
    def client(index):
        worker = Worker(args,manifest,index)
        setup=worker.prepare()
        records=[]
        barrier.wait()
        deadline=time.monotonic()+duration
        while time.monotonic()<deadline:
            records.append(worker.operation())
        worker.close()
        return records,setup
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
        futures=[pool.submit(client,index) for index in range(concurrency)]
        barrier.wait()
        started=time.monotonic()
        results=[future.result() for future in futures]
        samples=[r for rows,_ in results for r in rows]
        setup=[r for _,rows in results for r in rows]
    result=summarize(samples,time.monotonic()-started)
    result['setup_requests']={'count':len(setup),'statuses':dict(Counter(r['status'] for r in setup)),'latency':{'p95_ms':percentile([r['latency_ms'] for r in setup],95)},'timing':'before measured interval; explicit version reads'}
    return result

def arrival(args,manifest):
    # Deterministic fixed-rate arrivals, without an unbounded executor queue.
    # Queue delay is included from the scheduled arrival through completion.
    samples=[]
    local=threading.local()
    lock=threading.Lock()
    next_index=iter(range(1000000))
    workers=[Worker(args,manifest,index) for index in range(args.open_workers)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=min(args.open_workers,50)) as setup_pool:
        setup=[r for rows in setup_pool.map(lambda worker:worker.prepare(),workers) for r in rows]
    def execute(scheduled):
        if not hasattr(local,'worker'):
            with lock:
                local.worker=workers[next(next_index)]
        sample=local.worker.operation()
        completed=time.monotonic()
        sample['latency_ms']=(completed-scheduled)*1000
        sample['_completed_at']=completed
        return sample
    phases=[('sustained',args.rate,args.duration),('burst',args.rate*5,args.burst_seconds),('recovery',args.rate,args.duration)]
    output=[]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.open_workers) as pool:
        pending=set()
        for name,rate,duration in phases:
            samples=[]
            phase_futures=[]
            dropped=0
            started=time.monotonic()
            offered=math.ceil(rate*duration)
            for index in range(offered):
                scheduled=started+index/rate
                delay=scheduled-time.monotonic()
                if delay>0:
                    time.sleep(delay)
                completed={future for future in pending if future.done()}
                pending-=completed
                if len(pending)>=args.open_workers*2:
                    dropped+=1
                    continue
                future=pool.submit(execute,scheduled)
                pending.add(future)
                phase_futures.append(future)
            # Requests may overlap the next phase; collect after all arrivals rather
            # than inserting a recovery pause between burst and recovery.
            output.append((name,started,duration,offered,dropped,phase_futures))
            remaining=started+duration-time.monotonic()
            if remaining>0:
                time.sleep(remaining)
        phases_result=[]
        for name,started,duration,offered,dropped,futures in output:
            samples=[future.result() for future in futures]
            # Include tail completion in throughput, while arrivals in the next
            # phase remain continuous. Phase completion windows can overlap.
            completion_window=max(duration,max((sample['_completed_at']-started for sample in samples),default=duration))
            phases_result.append({'name':name,'arrival_duration_seconds':duration,'tail_seconds':completion_window-duration,**summarize(samples,completion_window,offered,dropped)})
    for worker in workers:
        worker.close()
    return {'setup_requests':{'count':len(setup),'statuses':dict(Counter(r['status'] for r in setup)),'timing':'before scheduled arrivals'},'phases':phases_result}

def hardware():
    result={'platform':platform.platform(),'cpu_count':os.cpu_count(),'python':platform.python_version()}
    for file,key in (('/sys/fs/cgroup/cpu.max','cgroup_cpu_max'),('/sys/fs/cgroup/memory.max','cgroup_memory_max')):
        try:
            result[key]=pathlib.Path(file).read_text().strip()
        except OSError:
            pass
    try:
        for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines():
            if line.startswith('model name'):
                result['cpu_model']=line.partition(':')[2].strip()
                break
        for line in pathlib.Path('/proc/meminfo').read_text().splitlines():
            if line.startswith('MemTotal:'):
                result['host_memory_total']=line.partition(':')[2].strip()
    except OSError:
        pass
    return result

def run(args):
    args.token=credential(args)
    manifest=json.loads(pathlib.Path(args.manifest).read_text())
    output={'format_version':1,'started_utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'fixture':manifest,'machine':hardware(),'method':{'seed':42,'duration_seconds':args.duration,'concurrency':args.concurrency,'arrival_rate':args.rate,'burst_multiplier':5,'burst_seconds':args.burst_seconds,'open_workers':args.open_workers,'timeout_seconds':args.timeout,'read_write_ratio':'80/20 base selection in expectation; measured version_refresh reads add recovery traffic','latency':'client-observed HTTP, persistent per-thread connections; all-response and success-only distributions','cold_phase':'first application requests; OS/PostgreSQL caches are not forcibly cleared','writes':'independent updates, hot conflict, identical-payload same-key retries and competing dependency insertions; expected validation/conflict errors remain reported','service_latency':'not instrumented by this harness; do not infer service acceptance latency from HTTP client times'},'phases':[]}
    for label,duration,concurrency in [('first_touch',min(5,args.duration),args.concurrency[0]),('warmup',args.warmup,args.concurrency[0])]+[(f'warm_{c}',args.duration,c) for c in args.concurrency]:
        print(f'Running {label}: {concurrency} clients / {duration}s',flush=True)
        output['phases'].append({'name':label,'concurrency':concurrency,**fixed(args,manifest,concurrency,duration)})
        pathlib.Path(args.output).write_text(json.dumps(output,indent=2)+'\n')
    started=time.monotonic()
    output['open_loop']=arrival(args,manifest)
    output['open_loop_wall_seconds']=time.monotonic()-started
    output['finished_utc']=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())
    pathlib.Path(args.output).write_text(json.dumps(output,indent=2)+'\n')
    print(f'Wrote {args.output}',flush=True)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    sub=parser.add_subparsers(dest='command',required=True)
    for command in ('seed','run'):
        p=sub.add_parser(command)
        p.add_argument('--token-file')
        p.add_argument('--manifest',default='benchmark-fixture.json')
        if command=='seed':
            p.add_argument('--nodes',type=int,default=100000)
            p.add_argument('--edges',type=int,default=500000)
            p.add_argument('--database-url-env',default='DATABASE_URL')
            p.add_argument('--psql',default='psql')
        else:
            p.add_argument('--endpoint',default='http://127.0.0.1:8741')
            p.add_argument('--duration',type=float,default=30)
            p.add_argument('--warmup',type=float,default=10)
            p.add_argument('--concurrency',type=int,nargs='+',default=[50,100,200])
            p.add_argument('--rate',type=float,default=200)
            p.add_argument('--burst-seconds',type=float,default=10)
            p.add_argument('--open-workers',type=int,default=200)
            p.add_argument('--timeout',type=float,default=12)
            p.add_argument('--output',default='benchmark-results.json')
    args=parser.parse_args()
    if args.command=='run' and (args.duration<=0 or args.warmup<=0 or args.rate<=0 or args.burst_seconds<=0 or args.timeout<=0 or args.open_workers<=0 or any(c<=0 for c in args.concurrency)):
        parser.error('durations, rates and client counts must be positive')
    (seed if args.command=='seed' else run)(args)

if __name__=='__main__':
    main()
