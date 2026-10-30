# Reproducing the performance workload

`scripts/benchmark.py` uses Python 3.10+ standard-library HTTP clients with persistent per-thread connections. The offline seed uses `psql` and a dedicated PostgreSQL database whose name starts `aq_graph_bench`. It refuses a database that already contains nodes. It never connects to Agent Q data. Credentials come from `AQ_GRAPH_TOKEN` or an owner-only `--token-file`; the report contains no token or database URL.

## Prepare an isolated fixture

Create an empty benchmark database, run the release service once to apply migrations, then seed. Do not configure service bootstrap for the seed workspace: the fixture creates its own benchmark principal. Choose a new random benchmark token through a secret file/environment, not a command argument.

```sh
cargo build --locked --release
# Create aq_graph_bench using an existing local PostgreSQL administrator.
# DATABASE_URL must name only that disposable benchmark database.
export DATABASE_URL='postgres://USER@127.0.0.1/aq_graph_bench'
AQ_GRAPH_BIND=127.0.0.1:18741 target/release/aq-graph serve
# In a second terminal, with the same DATABASE_URL and benchmark token configured:
python3 scripts/benchmark.py seed --nodes 100000 --edges 500000 \
  --manifest benchmark-fixture.json
python3 scripts/benchmark.py run --endpoint http://127.0.0.1:18741 \
  --manifest benchmark-fixture.json --output benchmark-results.json
```

Supply `--psql /absolute/path/to/psql` if needed. The seed accepts a PostgreSQL URL from `DATABASE_URL` (or `--database-url-env`), passes credentials through child environment variables, and prints only fixture counts. The required database-name guard and empty-node check precede all fixture writes. `--nodes 1000 --edges 5000` gives a small development fixture; `--nodes 1000000 --edges 5000000` is the planned growth scenario, subject to machine capacity. Seed and analyze time are recorded separately and are not ingestion-performance claims.

The seed manifest records exact committed counts and database bytes. The default synthetic corpus has:

- 100,000 nodes: 80% task, 20% knowledge. Structural parents are active/draft; remaining task states cycle through completed, active, draft and canceled. The manifest records actual state counts. Knowledge types cycle through all seven supported types; every knowledge record starts unverified and active, with a typed URL source and explicit synthetic creator. The fixture does not invent verification attribution or retirement authority. Protected/retired semantics are exercised by compatibility tests, not this unverified-content throughput corpus.
- Bodies built from meaningful authentication/operations vocabulary: 75% approximately 0.5 KiB, 20% approximately 2 KiB, 5% approximately 8 KiB, plus record/revision suffixes. Titles, labels, acceptance criteria, namespaced metadata and synthetic provenance are present.
- One initial full snapshot per node and ten additional full snapshots on 10% of nodes: 200,000 content revisions. The current snapshot equals the latest seeded revision. Historical content differs by an explicit synthetic revision suffix. Every revision has an associated immutable history entry.
- Up to 500,000 typed edges: at five slots per node, two slots prefer `blocks`, one assigns each non-root task exactly one parent in a three-level forest, and the rest are informational. Structural and blocking arcs point from smaller to larger UUIDs, so combined reachability is acyclic. Structural parents are never completed with open children. Relationship kinds obey the endpoint matrix; `produces` connects tasks to knowledge hubs. Cross-kind pairs otherwise become `references`. A subset targets twenty knowledge hubs, creating a skewed degree distribution. Actual type counts appear in the manifest. Self-links are omitted, so **use the manifest's committed edge count**, not the requested count. Every edge has an immutable addition history entry.

All fixture rows use deterministic IDs and an attributed `benchmark` actor, explicitly synthetic timestamps/provenance and a single synthetic seed change set. Seeding bypasses interactive batch/cycle validation to make fixture creation practical; it is not a demonstration of supported large imports, historical ingestion or real user history. Live workload mutations pass through the authenticated public API with ordinary history and durability enabled.

## Workload and report

Default phases are first-touch application traffic (five seconds), ten seconds of warmup, and thirty seconds each with 50, 100 and 200 fixed-concurrency clients. First-touch means the first application load after setup; it does **not** flush OS or PostgreSQL caches. Version preflight requests also warm record/cache paths. Do not label it a controlled cold-cache test.

Base request selection is 80% reads and 20% writes in expectation:

| Operation | Base share | Bounds / behavior |
| --- | --- | --- |
| Get | 25% | Random current node |
| Ready | 15% | 20 results |
| Context | 15% | Depth 1, 30 records, 128 KiB |
| Full-text knowledge search | 15% | Authentication query, 20 results |
| History | 10% | Revision-rich node, 20 entries |
| Independent update | 10% | Per-worker disjoint node, expected version |
| Hot-record update | 4% | Shared node, version conflicts expected |
| Same-key retry | 3% | Identical previous independent-mutation payload |
| Competing dependency addition | 3% | Opposite arcs on one pair, cycle/duplicate rejects expected |

Each worker reads its current independent/hot-node versions before a fixed phase, outside its measured interval; setup request counts/statuses are included in the report. Open-loop workers receive the same version preflight before scheduled arrivals. A mutation conflict schedules a separately measured `version_refresh` GET on that worker's next turn. Recovery reads change the achieved read/write ratio, and their counts appear explicitly. Versions are never guessed forward. Same-key replay results never reset a current cached version backward. Validation/conflict rejects remain in error and latency totals; successful updates and successful dependency additions have separate per-operation success distributions.

The open-loop run offers 200 requests/second for thirty seconds, 1,000/second for ten seconds, then 200/second for thirty seconds of recovery. Arrivals follow a deterministic schedule independent of response time. Two hundred worker threads and at most twice as many pending requests bound the client queue. The harness records arrivals it cannot enqueue as `client_queue_dropped`; these are client-capacity failures, not server overload responses. Scheduled-arrival-to-completion latency includes client queue delay. Phase request throughput divides completed attempts by the phase completion window, including any tail after its declared arrival duration; tail time is reported and overlapping phase windows must not be added. `open_loop_wall_seconds` includes drain and setup, allowing readers to distinguish offered rate and completed throughput. Traffic continues directly from burst into recovery without a drain pause.

For every phase the JSON output includes p50/p95/p99 over **all completed attempts**, success-only latency, status and error-code counts, timeouts, response bytes, offered/completed counts, throughput, failed offers (including client queue drops) and per-operation distributions. Open-loop output also separates scheduled arrivals, client queue drops and setup. Transport failures remain counted. Successful-request latency alone cannot establish capacity. A tiny `--duration` run checks harness operation but is not sustained-load acceptance evidence.

The harness measures client-observed HTTP latency. It excludes CLI process startup, Cloudflare/TLS deployment paths (unless the supplied endpoint includes them), and separate service-side queue instrumentation. Measure CLI startup separately, for example with an external command timing tool against `aq-graph --json task show UUID`. The service's structured completion logs provide acceptance-to-response timing; correlate them with workload windows, and collect pool wait metrics separately. Do not substitute client latency for service acceptance latency.

## Evidence and acceptance

Publish the original fixture manifest and report alongside a concise interpretation. Declare CPU model/allocation, physical/container memory limit, storage/filesystem, OS, Rust and PostgreSQL versions, pool size, request limits, release build, TLS/network placement and database durability settings. PostgreSQL's `fsync`, `synchronous_commit` and `full_page_writes` must remain enabled for representative results.

Collect host/container CPU and peak memory, PostgreSQL lock waits, I/O timing, pool waits and query plans during each phase. Useful starting points are `pidstat`/`vmstat`/`iostat`, container resource metrics, `pg_stat_activity`, `pg_locks`, `pg_stat_database` and `EXPLAIN (ANALYZE, BUFFERS)` on representative **read** queries. Use `track_io_timing` where permitted and report its setting. Record table/index sizes before and after to quantify disk growth and history amplification. Do not run write-bearing EXPLAIN ANALYZE on the authoritative graph.

Controlled cold-cache runs require a disposable host/database and an explicit cache-reset method. Background maintenance needs a separately timed maintenance phase. Restart tests need an explicit service restart between measured phases, idempotent replay and recovery checks. The included harness does not automatically perform these operations. Lack of those runs remains a stated gap.

Proposed service p95 targets are under 25 ms for immediate gets, under 100 ms for ready/context and small history-preserving mutations, and under 200 ms for indexed knowledge search. The reference hardware and a service p99 acceptance budget still need to be agreed. A local run, a warm cache, or successful requests amid many conflicts is not a production-readiness claim. Report overload/recovery honestly, including failed capacity targets and missing telemetry. Measured results, where available, belong beside this protocol with exact commands and raw artifacts.

The [retained schema-3 measurements](performance/README.md) are historical: they precede the schema-5 compatibility contract and use a different fixture distribution. Do not apply their capacity claims to the current binary. A new full-scale benchmark is required after the stronger readiness and knowledge rules; a small seed/query smoke establishes only operational correctness.
