# Schema 7 full fixture load evidence

This is a local release-build experiment on a shared desktop Linux environment, using an isolated PostgreSQL 17 container and loopback HTTP. It does not establish VPS, TLS, Cloudflare or production capacity. Fixture and workload scripts are the repository versions accompanying this directory.

## Baseline result

**The baseline misses the proposed latency and 200/s capacity targets.** Completed throughput counts expected conflicts/validation failures as attempts; successful throughput is lower. Fixed-stage all-attempt client latency is:

| Clients | Attempts/s | p50 ms | p95 ms | p99 ms | HTTP 200 / 400 / 409 |
| --- | ---: | ---: | ---: | ---: | --- |
| 50 | 170.45 | 226.6 | 691.2 | 860.5 | 4957 / 96 / 146 |
| 100 | 180.32 | 491.7 | 934.6 | 1089.1 | 5275 / 102 / 169 |
| 200 | 171.75 | 1111.1 | 1654.5 | 1843.7 | 5043 / 106 / 192 |

| Open-loop phase | Offered | Completed | Client drops | p50 / p95 / p99 ms | Tail seconds |
| --- | ---: | ---: | ---: | --- | ---: |
| 200/s sustained | 6000 | 5521 | 479 | 1871 / 3013 / 3527 | 2.32 |
| 1000/s burst | 10000 | 1767 | 8233 | 2272 / 2973 / 3245 | 2.75 |
| 200/s recovery | 6000 | 5231 | 769 | 2182 / 2705 / 2971 | 2.47 |

The queue remains bounded, but the recovery interval still drops arrivals and does not meet the target. Open-loop attempts completed per completion window are 170.83/s, 138.54/s and 161.11/s, respectively; these overlapping windows must not be added. Completed requests have approximately 19–20% writes, including retries and rejected mutations. All statuses and per-operation error codes remain in `results.json`; the first-touch stage had two HTTP 503 overloads. Later stages have only 200, 400 validation and 409 conflict responses. No client transport failure or socket timeout occurred.

Service guard p95 at 50 clients is 292 ms for gets, 892 ms ready, 304 ms context, 605 ms search, 412 ms history and 315 ms across mutation subtypes. Every provisional operation latency target is missed at that stage. Pool pressure raises even cheap-operation latency; client HTTP and service timing agree closely there. The baseline's epoch clock was recorded just after barrier release, so a few first responses per fixed phase fall outside service grouping windows (35 of 5341 at 200 clients, fewer than 1%). Raw logs retain them, and subsequent harness code records the clock in the barrier action before releasing workers. `benchmark-measured.py` retains the exact baseline harness. Service route grouping also combines measured recovery GETs with ordinary gets and all mutation subtypes together.

Forced prepared-plan diagnostics in `prepared-plans.json` show ready at 249/234 ms custom/generic. It inspects about 50,749 active candidates to find 21 ready rows because early IDs are clustered structural parents. History is 0.61–0.67 ms with custom GIN plans but 472–513 ms with generic plans that inspect approximately 700,000–800,000 irrelevant history entries before filtering. These forced plans demonstrate a plausible repeated-prepared-statement bottleneck; they do not identify the actual cached plan on each service connection. Common search takes 96/151 ms custom/generic because `authentication` matches the full fixture; selective and missing terms take about 0.6–1.4 ms. This corpus does not support generalizing common-term cost to selective indexed search. The retained sequential search probes and literal read plans are diagnostic complements.


Post-baseline read-only diagnostics compare a readiness candidate that subtracts live structural-parent IDs from active IDs, then evaluates remaining ancestor/blocker predicates through narrow lateral fetches. Nine representative full-JSON page comparisons match the original, including null and low/middle/high cursors and limits 1/21/101; this is fixture parity, not exhaustive lifecycle proof. Candidate custom/generic with JIT off take 79/61 ms. With JIT on, the generic candidate takes 106 ms, including 28 ms compiling 89 functions; its custom plan does not trigger JIT and takes 73 ms. Baseline readiness plans have no JIT section. These isolated probes motivate both query selection and a measured JIT setting, but do not replace a tested runtime change or fresh-fixture concurrent rerun. `ready-candidate-parity.json`, `ready-candidate-jit-plans.json` and the candidate SQL retain the evidence.

PostgreSQL peaks at 1869% CPU (18.7 logical cores). Service and generator peaks are 45% and 60% of one core; observed peak RSS is 40.8 MiB and 133.6 MiB respectively. PostgreSQL container samples remain about 2.5 GiB, with DataFileRead, BufferIo and BufferMapping wait events among sampled observations. This supports a database bottleneck on this host, while preserving the missing direct pool-wait measurement as a limitation.

The main workload adds 2979 node revisions, 2980 history entries and one new edge/version. Successful duplicate dependency calls therefore are not newly committed edges. Database allocation grows 27,811,840 bytes (26.5 MiB); major table growth is 7.2 MiB history, 7.0 MiB receipts, 6.2 MiB revisions and 5.9 MiB change sets. Allocation growth includes indexes/TOAST/free space and is not a precise per-write logical amplification factor. Before/after counts and table/index sizes are retained in `database-before.json` and `database-after.json`. This short growth sample does not establish long-term storage behavior.

## Reproduction and isolation

The PostgreSQL resource is `aq-graph-schema7-load-pg`, named volume `aq-graph-schema7-load-data`, database `aq_graph_bench_schema7`, bound only to `127.0.0.1:55434`. The release service binds `127.0.0.1:18745` with pool size 20. Owner-only ignored files under `.local/schema7-load` hold the generated database password and benchmark token. No AQ v1 daemon, repository or data is involved. A separate validation database in the same container was used before the measured interval; its tests and all Docker builds were stopped before timed load.

Setup uses `cargo build --locked --release`, then:

```sh
docker run -d --name aq-graph-schema7-load-pg \
  --env-file .local/schema7-load/pg.env \
  -p 127.0.0.1:55434:5432 \
  --mount type=volume,source=aq-graph-schema7-load-data,target=/var/lib/postgresql/data \
  postgres:17 -c track_io_timing=on -c max_connections=160
```

The secret environment file defines `POSTGRES_PASSWORD` and `POSTGRES_DB=aq_graph_bench_schema7`. The service receives its database URL through its environment, with `AQ_GRAPH_BIND=127.0.0.1:18745`, `AQ_GRAPH_POOL_SIZE=20`, and `RUST_LOG=info`. It starts once against the empty database to apply migrations. The seed's `--psql .local/schema7-load/psql` wrapper is exactly:

```sh
#!/bin/sh
exec docker exec -i aq-graph-schema7-load-pg psql -U postgres -d aq_graph_bench_schema7 "$@"
```

Seed `DATABASE_URL` identifies this disposable database; the wrapper uses container-local administration. Exact public seed/workload arguments appear in `commands.json`; `evidence-tools/` retains the collection/analysis scripts used from `.local/schema7-load` (copy there to replay; secret inputs are supplied separately). Credentials are supplied through the owner-only token file. The fixture commits 100,000 nodes, 500,000 edges, 300,000 revisions, 800,000 history entries and 713,221 relationship memberships. Seeding took 94.65 seconds including analyze; this bypasses API validation and is not an ingestion benchmark.

An initial attempt exposed a pathological OR join in offline relationship baseline construction on freshly loaded, unanalyzed relations. That transaction was canceled, and only the benchmark database was dropped and recreated before the retained fixture. The corrected seed analyzes edges/link sets and uses equivalent disjoint outgoing/incoming equality joins. No canceled-load table or index bloat is carried into this fixture.

## Measurement boundaries and remaining gaps

`environment.json` records hardware, Rust/PostgreSQL identity, source and release binary digests, timeouts and container limits. Durability (`fsync`, `synchronous_commit`, `full_page_writes`) and I/O timing remain enabled. The application has a 256-request admission limit, 10-second service deadline, two-second pool acquisition and lock timeouts, four-second statement timeout and six-second mutation timeout. The client socket timeout is 12 seconds per blocking I/O, not a total request deadline.

The workload runs first-touch application traffic for five seconds, ten seconds of warmup, then thirty seconds each at 50/100/200 clients; open-loop phases offer 200/s for thirty seconds, 1,000/s for ten seconds and 200/s for thirty seconds. No forced cache reset occurs. Setup version reads and seed/analyze warm the database. All errors, expected hot conflicts, dependency validation failures, transport failures and queue drops remain reported. Mutations include ordinary immutable history/durability work.

The report's client latency includes HTTP. Open-loop total latency includes client scheduling/queue delay, with HTTP and residual client delay distributions separately retained. The `client_queue` residual is total minus raw-body HTTP time, so it also includes small operation-selection/JSON processing overhead; it is not pure executor queue wait. Service guard completion logs are retained compressed; their latency includes admission/authentication/database/pool work and serialization, excludes socket delivery, and is grouped by completion timestamp. Open-loop service groups use non-overlapping arrival windows, so a prior phase's tail may complete in a later group. They cannot be equated to request-origin client phase groups. Mutation routes combine all mutation subtypes. Phase epoch windows permit independent correlation.

Service and generator `/proc` CPU/RSS/I/O samples exclude PostgreSQL; PostgreSQL container CPU/memory/block I/O and cumulative database/wait-event/lock snapshots are sampled separately about every two seconds. Sampling can miss short waits. The default 200 open-loop workers remain below the service's 256-request admission limit, so this burst primarily tests bounded client shedding and does not establish admission-limit rejection behavior. Pool wait duration is not instrumented; PostgreSQL wait events are not pool-wait telemetry. Read-only search plans simplify optional default filters to literals and do not prove prepared generic-plan performance. Sequential term probes are diagnostic, not sustained acceptance. Controlled cold caches, separately timed maintenance, larger growth fixtures and an agreed reference host/p99 budget remain outside this run. Restart/backup deployment validation is retained separately by the parent task.
