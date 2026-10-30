# Current performance evidence

- [Schema 7 full baseline](schema7-load/README.md): 100,000 nodes / 500,000 edges, release build and PostgreSQL 17; failed the proposed latency and 200/s workload targets.
- [Fresh optimized-query experiment](schema7-load-optimized/README.md): severe early overload during overlapping maintenance; interrupted open-loop phase, retained as failed/incomplete evidence.
- [Post-maintenance steady-state followup](schema7-load-optimized-steady/README.md): the same optimized binary after explicit VACUUM/ANALYZE on the previously written fixture. Its conditions differ from the original baseline; it does not establish fresh-load or maintenance acceptance.

The [benchmark protocol](../benchmark.md) defines the workload and interpretation limits. The small [schema-7 smoke](schema7-smoke/README.md) is operational evidence only. The older measurements below remain historical.

# Historical schema-3 performance results

These measurements apply to the earlier schema-3 contract and binary recorded below. They predate schema-4/5 parent/readiness, endpoint-matrix and knowledge-authorization changes. They are **not capacity or latency evidence for the current schema-7 implementation**. Raw artifacts remain unchanged historical evidence; current compatibility tests and a small fixture smoke do not replace a full representative load rerun.

Measured 2026-10-30T06:58:51Z through 2026-10-30T07:01:54Z. This is a local development-machine measurement, not a production VPS capacity guarantee.

The final release build sustained 200 scheduled requests/second and a ten-second 1,000/second burst without HTTP 5xx, transport failures, timeouts, or dropped client arrivals. Successful-request p95 was 8.95 ms at the sustained rate. Fixed-concurrency tests reached about 2,200–2,300 attempts/second, with substantially higher tail latency at 200 continuously busy clients. A successful throughput figure does not mean every offered mutation succeeded: hot-record version conflicts and invalid/duplicate dependencies were deliberately injected and remain counted below.

## Reference environment

- AMD Ryzen 9 5900X, 12 physical cores / 24 logical CPUs; about 31.3 GiB host memory; shared developer machine, with service, PostgreSQL and Python load generator on the same host.
- Ubuntu 24.04 under WSL2, Linux 6.18.33.2, local virtual disk. No VPS, container resource cap, TLS, Cloudflare, or WAN traversal in this measurement.
- Rust 1.96.1, optimized release build, PostgreSQL 16.15, schema migrations 0001–0003; 20 service database connections, 256 admitted handlers, default PostgreSQL shared_buffers 128 MiB.
- PostgreSQL fsync, synchronous_commit and full_page_writes enabled. track_io_timing disabled. The service emitted completion logs throughout the run.
- Binary SHA-256: `69f55b8727082cce631f074d866c5286e56d52aa741e82eaae5c8b82d71b864b`. Cargo.lock records dependency versions.

The [fixture manifest](fixture.json) records 100,000 nodes, 500,000 edges, 200,000 content revisions and 700,000 history entries before traffic. Seed data occupies 1.28 GiB including indexes. Content, history and identifiers are explicitly synthetic, created offline; the load test writes through the authenticated public API. The initial seed is not an import-throughput benchmark.

## Throughput and latency

Latencies are milliseconds, measured by persistent HTTP clients. Open-loop latency starts at scheduled arrival, including client queue delay. All-attempt distributions include deliberate rejections. There were no 5xx responses or transport failures in these phases.

| Phase | Attempts/s | Successful/s | All p95 | Success p95 | All p99 | 409 / 400 counts |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| warm_50 | 2241 | 2100 | 41.65 | 41.98 | 51.65 | 3245 / 985 |
| warm_100 | 2286 | 2142 | 85.94 | 86.08 | 111.52 | 3337 / 996 |
| warm_200 | 2255 | 2109 | 218.49 | 219.05 | 316.02 | 3416 / 1013 |
| sustained | 200 | 191 | 8.91 | 8.95 | 9.70 | 230 / 54 |
| burst | 1000 | 929 | 8.70 | 8.75 | 10.25 | 525 / 186 |
| recovery | 200 | 186 | 9.04 | 9.08 | 10.04 | 314 / 99 |

Fixed-concurrency phases run for thirty seconds each. Open-loop sustained and recovery phases each offer 200 requests/second for thirty seconds; burst offers 1,000/second for ten seconds. Every open-loop arrival was enqueued. Status 409 comes from version/duplicate conflicts; status 400 comes from deliberate invalid dependency insertions. Complete counts, error codes, first-touch/warmup/setup traffic, timings and per-operation distributions are in the [raw results](results.json).

The 200 continuously busy client phase exceeds the proposed 25/100 ms latency envelopes for some operations. Treat this as saturation evidence, not an SLO pass at arbitrary request rates. The reference production arrival rate and p99 budget must be selected before capacity acceptance.

At the 200 requests/second sustained phase:

| Operation | Count | Success p50 | Success p95 | Success p99 | Errors |
| --- | ---: | ---: | ---: | ---: | ---: |
| get | 1485 | 0.93 | 1.11 | 1.40 | 0 |
| ready | 894 | 2.05 | 2.41 | 2.81 | 0 |
| context | 909 | 2.33 | 2.73 | 3.28 | 0 |
| independent_write | 598 | 4.15 | 5.11 | 7.44 | 0 |
| search | 878 | 8.63 | 9.94 | 10.98 | 0 |
| history | 562 | 2.77 | 3.29 | 3.67 | 0 |
| same_key_retry | 165 | 1.50 | 1.73 | 1.77 | 0 |

The benchmark's dependency mutations repeatedly contest one pair and mostly fail by design. These numbers do not establish throughput for bulk successful dependency insertion across a large DAG. The suite separately verifies correctness of concurrent successful and conflicting graph changes.

## Resource and history costs

The sampled service process peaked at 34.5 MiB RSS and 262% of one logical CPU over one-second intervals. [Process samples](resources.json) exclude PostgreSQL and client-generator memory/CPU and are not total deployment sizing. There was no application memory cap inferred from this result.

After workload traffic, PostgreSQL held 226,213 content revisions and 726,214 history entries; total database size was 1.46 GiB. [Post-run sizes and counters](database-after.json) include table/index sizes. File growth includes history, receipts, change-set payloads, indexes and free space; it is not solely useful retained content. Vacuum behavior and long-term retention need a longer-duration run.

Twenty separate optimized CLI processes fetching a record in a small smoke workspace had p50 5.69 ms and p95 6.14 ms, including startup and loopback HTTP. [CLI samples](cli-startup.json) used warm OS caches and are separate from the large-fixture workload.

## Query-plan finding

The initial search query used a primary-key ordered scan and filtered through 80,000 task records before reaching knowledge IDs. A partial knowledge index alone did not reliably change that plan. The implementation now materializes matching IDs, orders and limits those IDs, then fetches only selected record bodies. Rare terms retain the full-text GIN path.

The [earlier baseline](baseline-before-search-fix.json) reached roughly 1,300 attempts/second; its 50-client successful-request p95 was about 109 ms. The final 50-client p95 is about 42 ms with about 2,240 attempts/second. Each run started from its own seeded database; neither is an isolated microbenchmark or guarantee of the same gain on different data distributions.

## Reproduction and limits

Follow the [benchmark protocol](../benchmark.md). The final run used:

```sh
python3 scripts/benchmark.py seed --nodes 100000 --edges 500000 \
  --token-file .local/benchmark-token --manifest .local/benchmark-verified-fixture.json
python3 scripts/benchmark.py run --endpoint http://127.0.0.1:18741 \
  --token-file .local/benchmark-token --manifest .local/benchmark-verified-fixture.json \
  --output .local/benchmark-verified-results.json --duration 30 --warmup 10 \
  --concurrency 50 100 200 --rate 200 --burst-seconds 10
```

Use an empty disposable `aq_graph_bench*` database and apply migrations first. Configure its connection through DATABASE_URL. A nonstandard PostgreSQL installation also needs `--psql /path/to/psql` and its library search path. `scripts/observe.py --pid PID --seconds 190 --output resources.json` collects the Linux process sample independently.

These results are client-observed, not a separately aggregated service-only SLO. Service logs contain handler processing duration, but pool waits and database lock/I/O timing were not independently quantified. Controlled cold-cache, explicit maintenance, long soak, million-node growth, multi-host, Docker/PostgreSQL17, and VPS/Cloudflare runs remain unperformed. Runtime correctness, restart and native backup restoration were checked separately; those are not load-test restart measurements.
