# Schema-7 shared-field smoke

This is bounded operational evidence for the shared task/knowledge model, not a capacity or regression benchmark. It ran on 2026-10-30 using local PostgreSQL 16.15, a Rust debug build and warm caches on a shared development machine. The [fixture manifest](fixture.json) records schema 7, UTC session timestamps, production-equivalent durability settings, 1,000 nodes, 5,000 edges, 3,000 revisions and 8,000 history entries. Current records have common classifications, typed URL sources and unverified defaults; 25% have bounded validity windows and 20% have review markers.

The first phase made 20 sequential requests per operation:

| Operation | Successful | Client p50 | Client p95 | Maximum |
| --- | ---: | ---: | ---: | ---: |
| Ready,limit 20 | 20/20 | 14.29 ms | 18.77 ms | 38.00 ms |
| Search across both kinds,limit 20 | 20/20 | 8.97 ms | 11.56 ms | 14.69 ms |
| Task title edit and revision | 20/20 | 14.40 ms | 23.49 ms | 26.21 ms |
| Knowledge title edit and revision | 20/20 | 16.58 ms | 45.38 ms | 48.80 ms |

All 80 attempts succeeded. These observations are materially slower than the separate schema-6 smoke. The fixture now has more shared metadata/provenance on tasks, and search covers both kinds; compiler/tests and other host activity may also have affected the run. The runs are not controlled comparisons, so these results neither attribute a regression nor establish speed parity. A release-build, controlled full-scale mixed-load run remains outstanding. Twenty samples per operation do not support a production p99 claim.

One later [readiness plan](ready-plan.json), captured after the CLI workflow on 1,004 nodes, executed in 6.839 ms. It contains **no recursive union and no JIT compilation**. The bounded parent/grandparent join remains in place; this check found no return of the old per-candidate recursive-ancestor plan. It is a single warm EXPLAIN result, not a latency distribution.

The [raw results](results.json) retain measured values, stage-specific database counts and proofs that both kinds advance CAS/revision, preserve exact old snapshots and reuse unchanged relationship sets. The 40 content edits grew revisions/history by 40 while edge versions, link sets and memberships stayed unchanged. Results from the subsequent real CLI workflows are recorded separately from those counts.

Real CLI commands additionally passed for **both kinds**:

- Canonical `content_type` and typed provenance on creation, with task-only acceptance criteria.
- Shared summary set/clear, source-revision pinning and invalidation by metadata changes.
- Explicit null clearing of all three dates, administrator `verify_node`, and attributed verification.
- Cross-kind successor edges added atomically with retirement; retirement using an already existing edge creates no duplicate or unnecessary revision.
- Default retired-search exclusion, explicit inclusion, and shared validity/review list filters.

No v1 source, daemon or database was used. Data was synthetic in the isolated `aq_graph_bench_shared7` database; the smoke service was stopped afterward. The general [benchmark protocol](../../benchmark.md) documents how to seed and run a full controlled workload. Earlier schema 3/5/6 artifacts remain unchanged historical evidence.
