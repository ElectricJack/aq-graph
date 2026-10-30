This schema-8 profile completed the 100,000-node, 500,000-edge workload with no transport failures, timeouts, client queue drops, or read errors. At scheduled 200 requests/s, the 1,000 requests/s burst, and recovery at 200 requests/s, every measured service route met the provisional p95 budgets in [the benchmark protocol](../../benchmark.md). These are local results on shared desktop hardware, not production acceptance or a controlled speedup over earlier profiles.

The fixture was seeded offline under schema 7, then upgraded through the schema-8 readiness backfill. Before load it contained 300,000 revisions, 800,000 history entries, 500,000 edge versions, 100,000 link sets and 713,221 link memberships. This is the first application workload after that seed/upgrade, with no explicit VACUUM. It is not a cold-cache run or a direct schema-8 bulk-ingestion measurement. Automatic maintenance remained enabled: the first boundary sample reports nodes with zero all-visible pages and 99,334 estimated dead tuples, while history had already autovacuumed at 18:18:30 UTC and reported all 103,927 estimated heap pages visible. Maintenance continued during load. [Boundary states](results.json), [before](database-before.json), [after](database-after.json), and [periodic PostgreSQL samples](postgres-resources.json) retain those conditions; tuple/page statistics are estimates.

The release service used a 20-connection pool, 256 inflight limit, 10-second request deadline, and JIT disabled per service connection. PostgreSQL retained its recorded configuration, including 128 MiB shared buffers and 4 MiB work memory. Requests used loopback HTTP without TLS on a shared WSL2 Ryzen 9 5900X host with 24 logical CPUs and approximately 31 GiB RAM; neither service nor database had dedicated CPU or container memory caps. [Environment metadata](environment.json) records the complete settings, binary digest and dirty source state based on commit `f2a2e9f50a6dbe7ab479fc951ebb704a06535e40`. The measured source SHA256 is `3324486322be05ee84f75628022fa5e3ce53ae34739b84c759d8bdd0a3f3fffc`; its exact file list and hashing method are recorded there.

[Client results](results.json) include every completed attempt, including expected validation/conflict responses. Open-loop latency includes delay from the scheduled arrival; HTTP and client queue distributions are also retained. Fixed-client throughput includes recovery reads. The following p95 values are milliseconds and include successful and failed attempts within each operation.

| Phase | Completed | Completed/s | All p95 | Ready p95 | Search p95 | History p95 | Independent write p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| First touch, 50 clients, 5 s | 8,780 | 1,746.61 | 52.48 | 55.03 | 51.38 | 54.60 | 60.79 |
| Warmup, 50 clients, 10 s | 18,183 | 1,813.24 | 50.69 | 52.48 | 50.90 | 53.94 | 57.51 |
| Fixed 50 clients, 30 s | 51,772 | 1,724.31 | 53.13 | 54.32 | 53.96 | 54.23 | 62.77 |
| Fixed 100 clients, 30 s | 53,556 | 1,782.50 | 114.92 | 125.03 | 122.64 | 127.38 | 118.91 |
| Fixed 200 clients, 30 s | 51,060 | 1,694.17 | 299.55 | 333.02 | 333.99 | 338.17 | 291.49 |
| Scheduled 200/s, 30 s | 6,000 | 200.00 | 11.20 | 5.97 | 4.47 | 5.62 | 14.40 |
| Scheduled 1,000/s burst, 10 s | 10,000 | 998.89 | 14.98 | 8.72 | 6.20 | 7.41 | 20.41 |
| Scheduled 200/s recovery, 30 s | 6,000 | 200.00 | 11.55 | 6.39 | 4.98 | 6.11 | 15.86 |

The burst offered and completed all 10,000 requests, with an 11.15 ms completion tail. Sustained/burst/recovery reported 211/550/304 non-2xx responses, respectively: only dependency validation (400) and optimistic hot-write conflicts (409). Independent writes, identical-payload same-key retries and reads all succeeded in every phase. Operation counts and status counts sum to completed totals; offered equals completed plus dropped in each open-loop phase. The fixed 200-client run saturates the system: its client p95 exceeds several provisional latency budgets despite zero dropped requests. Closed-loop throughput is not a claim that those latency budgets hold at that rate.

[Service telemetry](service-latency.json) separately measures guard entry through response serialization, including admission/authentication/pool/database work and excluding socket delivery. Service p95 across all routes was 10.62/13.82/10.90 ms for the sustained/burst/recovery arrival windows. Their service ready p95 was 4.96/6.43/5.34 ms, search 3.47/4.46/4.00 ms, and combined mutation route 12.90/16.97/13.50 ms. At fixed 200 clients the combined mutation service p95 was 156.38 ms, exceeding the provisional 100 ms mutation budget; other measured service routes stayed below their applicable budgets. Client and service latency therefore need separate reporting. Service windows are assigned by completion time and can contain prior-phase tails; mutation telemetry combines all subtypes. The [compressed raw service log](service-requests.log.gz) accounts for 206,651 requests: 205,351 measured client completions plus 1,300 setup reads. The 1,315 requests outside service phase windows comprise those setup reads and 15 completions crossing window boundaries.

[Exact prepared plans](prepared-plans.json) were collected after load using current service SQL, parameter order and explicit parameter types, with JIT disabled. Forced custom/generic plans are diagnostic possibilities, not proof of which plan each pooled connection selected during load.

| Query | Custom execution ms | Generic execution ms | Verified plan behavior |
| --- | ---: | ---: | --- |
| Ready, 21-row fetch | 2.586 | 2.259 | Ordered `node_ready_leaves` index-only scan; 84 candidates examined; 54 heap fetches in each mode |
| History, node 10 | 1.516 | 1.368 | Bounded current history query |
| History, node 50,000 | 1.522 | 1.602 | Bounded current history query |
| History, node 99,990 | 1.520 | 1.618 | Bounded current history query |
| Common search `authentication` | 1.918 | 3.067 | Prefix examines 256 rows; fallback child scan never executes |
| Selective search `12345` | 3.563 | 3.030 | Fallback uses `nodes_search` GIN; one matching row |
| Missing search `absentbenchmarktermzz` | 2.725 | 4.359 | Fallback uses `nodes_search` GIN; zero matching rows |

The common-term path avoids materializing the full matching corpus, while selective/missing queries retain the GIN fallback in both forced modes. The prefix chose `nodes_pkey` in this single-workspace fixture; this does not establish plan behavior for sparse workspaces in a large multi-workspace database. Query semantics and both plan modes also have differential test coverage, including both kinds, lifecycle/date filters, negative/missing/stop terms, nil UUIDs, cursor boundaries, and exactly 256/257 remaining live rows. The readiness projection and administrative repair have independent backfill, mutation, concurrency, and drift-repair tests.

[Commands](commands.json), the [measured harness copy](benchmark-measured.py), and [evidence helpers](evidence-tools/) retain how this profile was produced. [Service](service-resources.json), [generator](generator-resources.json), and PostgreSQL observations capture resource use. No reference hardware or p99 acceptance budget has been agreed. Earlier schema-7 profiles have different maintenance/cache conditions, so these measurements do not establish a normalized speedup.
