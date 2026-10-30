"""Focused harness accounting tests; run python3 -m unittest discover -s scripts."""
import importlib.util
import pathlib
import types
import threading
import time
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('benchmark', pathlib.Path(__file__).with_name('benchmark.py'))
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)


class HarnessTests(unittest.TestCase):
    def worker(self):
        worker = benchmark.Worker(types.SimpleNamespace(endpoint='http://localhost',timeout=1,token='test'), {'nodes':1000,'workspace':benchmark.WORKSPACE}, 0)
        worker.request = Mock()
        return worker

    def test_failed_preflight_is_measured_recovery_without_guessed_version(self):
        worker = self.worker()
        worker.request.return_value = (503, {'error':{'code':'overloaded'}}, .01, 10)
        setup = worker.prepare()
        self.assertEqual(len(setup), 2)
        self.assertEqual(worker.node_versions, {})
        sample = worker.operation()
        self.assertEqual(sample['operation'], 'version_refresh')
        self.assertEqual(worker.request.call_args.args[0], 'GET')
        self.assertTrue(worker.needs_refresh)

    def test_missing_success_version_requires_refresh(self):
        worker = self.worker()
        worker.node_versions = {1:7, 2:9}
        worker.random = Mock()
        worker.random.randrange.return_value = 80
        worker.random.randint.return_value = 1
        worker.request.return_value = (200, {'results':[{}]}, .01, 10)
        worker.operation()
        self.assertEqual(worker.node_versions[1], 7)
        self.assertEqual(worker.needs_refresh, {1})

    def test_replay_never_resets_newer_current_version(self):
        worker = self.worker()
        worker.node_versions = {1:19}
        worker.receipt = {'idempotency_key':'same','operations':[]}
        worker.random = Mock()
        worker.random.randrange.return_value = 95
        worker.random.randint.return_value = 1
        worker.request.return_value = (200, {'results':[{'version':7}]}, .01, 10)
        self.assertEqual(worker.operation()['operation'], 'same_key_retry')
        self.assertEqual(worker.node_versions[1], 19)

    def test_fixed_uses_requested_number_of_distinct_clients(self):
        for count in (50, 100, 200):
            seen = set()
            prepared = set()
            closed = set()
            lock = threading.Lock()
            class Client:
                def __init__(self, args, manifest, index):
                    self.index = index
                    with lock:
                        seen.add(index)
                def prepare(self):
                    with lock:
                        prepared.add(self.index)
                    return []
                def operation(self):
                    time.sleep(.005)
                    return dict(operation='get', status='200', error_code=None, latency_ms=5, bytes=1)
                def close(self):
                    with lock:
                        closed.add(self.index)
            with patch.object(benchmark, 'Worker', Client):
                result = benchmark.fixed(None, {}, count, .01)
            self.assertEqual(seen, set(range(count)))
            # A shared deadline may expire before a worker gets CPU on busy CI.
            # Initialization and cleanup, not a 10ms scheduling assumption, prove
            # that the requested number of distinct clients was actually used.
            self.assertEqual(prepared, set(range(count)))
            self.assertEqual(closed, set(range(count)))
            self.assertEqual(result['offered'], result['completed'])

    def test_open_loop_reports_bounded_queue_drops_and_scheduled_latency(self):
        class Client:
            def __init__(self, *args):
                pass
            def prepare(self):
                return []
            def operation(self):
                time.sleep(.02)
                return dict(operation='get', status='200', error_code=None, latency_ms=20, bytes=1)
            def close(self):
                pass
        observations=[]
        args = types.SimpleNamespace(open_workers=2,rate=400,duration=.02,burst_seconds=.02,snapshot=observations.append)
        with patch.object(benchmark, 'Worker', Client):
            result = benchmark.arrival(args, {})
        self.assertEqual(observations,['sustained_arrival_boundary_requested','burst_arrival_boundary_requested','recovery_arrival_boundary_requested','open_loop_after_arrivals_before_drain_requested','open_loop_after_drain'])
        phases = result['phases']
        self.assertEqual([p['offered'] for p in phases], [8,40,8])
        self.assertGreater(sum(p['client_queue_dropped'] for p in phases), 0)
        for phase in phases:
            self.assertEqual(phase['offered'], phase['completed']+phase['client_queue_dropped'])
            if phase['completed']:
                self.assertIn('client_queue', phase)
                self.assertIn('http_latency', phase)

    def test_database_guard_precedes_maintenance(self):
        args = types.SimpleNamespace(database_url_env='BENCH_TEST_URL',psql='psql')
        with patch.dict(benchmark.os.environ, {'BENCH_TEST_URL':'postgres://localhost/production'}):
            with patch.object(benchmark.subprocess,'check_output',return_value='production\n') as sql:
                with self.assertRaises(SystemExit):
                    benchmark.database_sql(args)
        self.assertEqual(sql.call_count,1)
        self.assertEqual(sql.call_args.args[0][-1],'SELECT current_database()')

    def test_sql_failure_redacts_connection_diagnostics(self):
        args=types.SimpleNamespace(database_url_env='BENCH_TEST_URL',psql='psql')
        failure=benchmark.subprocess.CalledProcessError(1,['psql'],stderr='private credentials')
        with patch.dict(benchmark.os.environ,{'BENCH_TEST_URL':'postgres://localhost/aq_graph_bench_test'}), \
             patch.object(benchmark.subprocess,'check_output',side_effect=failure):
            with self.assertRaises(SystemExit) as raised:
                benchmark.database_sql(args)
        self.assertNotIn('private',str(raised.exception))

    def test_maintenance_records_before_after_without_transaction(self):
        args = types.SimpleNamespace(output='unused.json')
        sql = Mock()
        with patch.object(benchmark,'database_sql',return_value=sql), \
             patch.object(benchmark,'database_state',side_effect=[{'sample':'before'},{'sample':'after'}]), \
             patch.object(benchmark.pathlib.Path,'write_text') as write:
            benchmark.maintain(args)
        sql.assert_called_once_with('VACUUM (ANALYZE)')
        report = benchmark.json.loads(write.call_args.args[0])
        self.assertEqual(report['before']['sample'],'before')
        self.assertEqual(report['after']['sample'],'after')
        self.assertGreaterEqual(report['finished_epoch_seconds'],report['started_epoch_seconds'])

    def test_failures_drops_and_queue_latency_remain_in_report(self):
        samples = [dict(operation='get',status='200',error_code=None,latency_ms=100,http_latency_ms=20,client_queue_ms=80,bytes=5),
                   dict(operation='hot_write',status='409',error_code='version_conflict',latency_ms=300,http_latency_ms=200,client_queue_ms=100,bytes=8),
                   dict(operation='get',status='timeout',error_code=None,latency_ms=900,http_latency_ms=800,client_queue_ms=100,bytes=0)]
        report = benchmark.summarize(samples, 2, offered=5, dropped=2)
        self.assertEqual(report['failed_offers'], 4)
        self.assertEqual(report['all_request_latency']['p95_ms'], 900)
        self.assertEqual(report['successful_request_latency']['p95_ms'], 100)
        self.assertEqual(report['http_latency']['p95_ms'], 800)
        self.assertEqual(report['client_queue']['p50_ms'], 100)
        self.assertEqual(report['operations']['hot_write']['error_codes'], {'version_conflict':1})


if __name__ == '__main__':
    unittest.main()
