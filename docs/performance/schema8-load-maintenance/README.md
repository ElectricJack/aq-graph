# Schema-8 maintained fixture and maintenance under writes

This 2026-10-30 local release run completed 200/s sustained, 1,000/s burst and 200/s recovery offers with no client drops, timeouts or HTTP 5xx responses. It also retained actual changed-node history while ordinary `VACUUM (ANALYZE)` ran during the 200-client phase. Higher fixed concurrency still exceeded provisional latency targets; this is local workload evidence, not production acceptance or a controlled measurement of vacuum's benefit.

The run spans 18:22:06–18:25:03 UTC. [Commands](commands.json), [environment](environment.json), [measured harness](benchmark-measured.py), [results](results.json), [service timings](service-latency.json), [resource samples](postgres-resources.json) and [exact prepared plans](prepared-plans.json) retain the method and raw evidence. CPU/memory are shared WSL2 desktop resources without a dedicated CPU allocation or container memory cap. PostgreSQL durability settings remain enabled; service pool connections disable JIT. No PostgreSQL memory settings changed: shared buffers 128 MiB, work memory 4 MiB, maintenance memory 64 MiB, autovacuum memory inherited, effective cache size 4 GiB. Effective cache size is a planner estimate, not an allocation.

## Starting condition and limits on comparison

This reused the 100,000-node database after the preceding [first profile](../schema8-load-first/), with 500,001 edges, 320,611 revisions and 820,612 history entries at the [starting snapshot](database-before.json). The [fixture manifest](fixture.json) describes the original seed, not these later logical counts. An explicit [pre-run vacuum/analyze](pre-run-maintenance.json) ran before this profile, followed by a second explicit [vacuum/analyze under load](maintenance.json). Autovacuum stayed enabled.

The service remained running: existing pool/prepared statement and application caches, PostgreSQL/OS caches, prior writes/history, dead tuples and maintenance state distinguish this from the first profile. The same server memory configuration does not make these matched controls. These results cannot attribute a speedup to vacuum, identify a single cause of any difference, or establish fresh-start behavior. Query plans were collected after measured load and are diagnostic forced custom/generic plans, not proof of which plan each live connection selected.

## Load and latency

Fixed phases used 50/100/200 persistent clients for 30 seconds after first-touch/warmup. Open-loop arrivals then offered 200/s for 30 seconds, 1,000/s for 10 seconds and 200/s for 30 seconds. Base selection remains 80% reads/20% writes in expectation, with separately counted recovery reads, expected conflicts, validation rejects and replay/no-op successes.

| Phase | Completed attempts/s | All-attempt p95 ms | HTTP 200 | HTTP 409 | HTTP 400 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 50 clients | 1,681.3 | 54.90 | 48,074 | 1,673 | 751 |
| 100 clients | 1,661.0 | 123.44 | 47,445 | 1,712 | 753 |
| 200 clients, vacuum overlaps | 1,657.0 | 307.03 | 47,321 | 1,771 | 772 |
| Sustained 200/s | 200 | 11.37 | 5,755 | 158 | 87 |
| Burst 1,000/s | 1,000 | 15.80 | 9,450 | 370 | 180 |
| Recovery 200/s | 200 | 11.42 | 5,695 | 212 | 93 |

All 6,000/10,000/6,000 open-loop offers completed; expected rejects remain failed attempts rather than being discarded. Socket timeout, transport errors, client drops and HTTP 5xx counts were zero. Rates in the last three rows are rounded; exact completion-window rates/tails appear in the report.

Successful client-observed p95 milliseconds:

| Phase | Get | Ready | Context | Search | History | Independent update |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 50 clients | 30.02 | 56.43 | 57.06 | 56.16 | 55.32 | 64.25 |
| 100 clients | 78.58 | 134.02 | 130.83 | 133.90 | 133.03 | 132.60 |
| 200 clients | 202.50 | 344.11 | 337.33 | 346.66 | 336.38 | 294.79 |
| Sustained | 2.30 | 6.18 | 5.93 | 4.69 | 5.02 | 14.28 |
| Burst | 3.81 | 9.66 | 8.66 | 8.15 | 8.61 | 20.58 |
| Recovery | 2.35 | 6.16 | 5.94 | 4.64 | 5.02 | 15.55 |

Open-loop operations met the provisional p95 budgets on this machine. Fixed concurrency missed get's 25 ms budget even at 50 clients, ready/context/update budgets at 100 clients, and search's 200 ms budget at 200 clients. The [service report](service-latency.json) measures acceptance through response serialization separately: open-loop sustained/burst/recovery successful ready p95 was 5.16/6.84/5.21 ms, search 3.65/4.92/3.64 ms, and the combined mutation route 13.24/18.30/13.77 ms. Service windows group requests by completion time and can include prior-phase tails; mutation subtypes cannot be separated from those route logs. No pool wait-duration distribution or agreed production hardware/p99 budget is established.

## Verified writes during maintenance

The explicit live maintenance invocation ran from **18:23:27.653663 to 18:23:37.103922 UTC**, 9.450 seconds, inside the 200-client interval (18:23:22.652976–18:23:53.014107). Its start follows the phase announcement by five seconds. Boundary observations run asynchronously during open-loop traffic; they do not insert a drain pause between burst and recovery. Sampling itself consumes resources.

The [route overlap artifact](maintenance-overlap.json) records 2,113 successful mutation completions during that invocation, and correctly marks independent-write overlap unproven from route logs alone: replay and dependency no-op requests can return success. A subsequent read-only database audit provides stronger evidence. Persisted history contains 1,467 `revision/updated` rows whose `created_at` falls within the invocation; 1,436 persisted node revisions have independent-worker edit titles, across 200 distinct nodes.

To exclude command startup/teardown, a narrower interval uses the earliest table `last_vacuum` and the final table `last_analyze` timestamp recorded by the maintenance snapshot: **18:23:27.678893–18:23:37.096812 UTC**. Within it, **1,427 independent-worker revisions and 1,427 matching updated history rows** persisted. This establishes actual changed-history writes during the vacuum/analyze command, beyond successful replay/no-op responses. These insertion timestamps use `clock_timestamp()`; they are not exact transaction commit timestamps, and the command processes tables sequentially, so this does not imply every write overlapped vacuum of its own table.

The read-only count can be reproduced on the disposable benchmark database:

```sql
SELECT count(*) FROM node_revisions
WHERE created_at >= '2026-10-30T18:23:27.678893Z'
  AND created_at <= '2026-10-30T18:23:37.096812Z'
  AND snapshot->>'title' LIKE 'Benchmark worker % edit %';
-- 1427
SELECT count(*) FROM history_entries
WHERE created_at >= '2026-10-30T18:23:27.678893Z'
  AND created_at <= '2026-10-30T18:23:37.096812Z'
  AND category='revision' AND operation='updated'
  AND data->>'title' LIKE 'Benchmark worker % edit %';
-- 1427
```

The [ending snapshot](database-after.json) contains 340,480 revisions and 840,481 history entries: 19,869 additional revisions/history entries during this profile. Database bytes grew from 2,779,150,003 to 2,949,576,371. Sampled peak service RSS was 37,528 KiB; generator RSS was 162,624 KiB. PostgreSQL/container samples are separate and must not be confused with service-only RSS. Ordinary vacuum/analyze preserved these writes; this run does not test restart/replay recovery, managed production deployment paths, or a controlled maintenance-versus-no-maintenance comparison.

The independent read-only history audit and exact SQL are retained in [maintenance-history-overlap.json](maintenance-history-overlap.json).
