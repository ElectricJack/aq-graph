# Shared revision engine smoke (schema 6)

This is a small operational check of the current common task/knowledge revision engine, not a capacity benchmark. It ran on 2026-10-30 against local PostgreSQL 16.15 with a Rust debug build, warm caches, sequential requests and a 20-connection pool. PostgreSQL fsync, synchronous_commit and full_page_writes were enabled. The [fixture](fixture.json) records settings, exact counts and schema version. Earlier [schema-3 load measurements](../README.md) and the [schema-5 readiness comparison](../schema5-smoke/README.md) are historical and do not certify this revision-write policy.

The updated offline fixture contains 1,000 nodes, 5,000 edges, 3,000 logical revisions and 8,000 history entries. It records 5,000 immutable edge versions, 1,000 complete relationship sets and 7,021 memberships. Each synthetic current node has an explicitly attributed complete baseline after graph assembly. Earlier synthetic revisions retain unknown relationship sets; the seed does not fabricate past membership. Typed URL evidence includes `retained:false`, and the seed fixes its PostgreSQL session timezone to UTC for reproducible timestamps. Structural membership remains a three-level single-parent forest with an acyclic combined dependency graph.

The successful run made 20 requests for each operation:

| Operation | Successful | Client p50 | Client p95 | Max |
| --- | ---: | ---: | ---: | ---: |
| Ready, limit 20 | 20/20 | 6.02 ms | 7.30 ms | 17.51 ms |
| Search authentication, limit 20 | 20/20 | 3.78 ms | 4.35 ms | 5.51 ms |
| Task content edit with logical revision | 20/20 | 7.03 ms | 9.00 ms | 14.00 ms |
| Knowledge content edit with logical revision | 20/20 | 7.45 ms | 8.03 ms | 12.72 ms |

These times include loopback HTTP, SQL/history work and debug serialization. Twenty sequential observations on a small shared machine cannot establish service SLOs, overload behavior, concurrent performance or large-graph capacity. No p99 claim is made from this sample.

The [raw observations](results.json) also verify for both record kinds:

- CAS version and logical revision advance by 20 after 20 meaningful edits.
- Exact baseline snapshots remain byte-for-byte equal as JSON after later edits.
- Complete relationship membership is retained across content edits: six relationships on the selected task and five on the selected knowledge node.
- Legacy revision reads return `relationships_available:false` with `links:null` rather than current links or an invented empty set.
- Diffs disclose relationship availability and unchanged before/after link sets while reporting title changes.
- A real CLI `revision ID --revision 2 --json` reads the exact task baseline successfully.

After the 40 writes, PostgreSQL contains 3,040 node revisions and 8,040 history entries. Immutable edge-version count remains 5,000, relationship-set count remains 1,000, and memberships remain 7,021: unchanged relationship snapshots are reused rather than copied for every content edit. This check exercises common read/write/history behavior; restore modes and graph/authority conflicts are exercised by the separate focused version-control tests.

Reproduce the fixture in a newly migrated disposable database whose name starts `aq_graph_bench`:

```sh
DATABASE_URL=postgres://USER@127.0.0.1/aq_graph_bench_unified6 \
  python3 scripts/benchmark.py seed --nodes 1000 --edges 5000 \
  --token-file /private/benchmark-token --manifest /tmp/unified6-fixture.json
```

Run the current service against that database, then use the authenticated API/CLI to read current/exact revisions, perform expected-version title edits and compare prior snapshots and relationship-set counts. Record the actual binary/build, database settings and sample counts. The general [benchmark protocol](../../benchmark.md) provides the mixed fixed-concurrency/open-loop harness for a future optimized full-scale run; this smoke does not replace it.
