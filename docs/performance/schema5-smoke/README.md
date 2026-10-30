# Schema-5 fixture and readiness query smoke

This is an operational and query-plan check on a **1,000-node, 5,000-edge disposable fixture**, not a capacity benchmark. It ran on 2026-10-30 against local PostgreSQL 16.15 with fsync/synchronous_commit/full_page_writes enabled. The service was a Rust debug build, with a 20-connection pool and warm local caches. The [older schema-3 load results](../README.md) remain historical evidence and do not describe this contract.

The updated fixture contains 1,000 nodes, 5,000 relationships, 2,000 revisions and 7,000 history entries. Knowledge has typed URL provenance, a recorded synthetic author and unverified active state. Structural edges form a three-level forest, each child has one parent, and every blocking/structural edge points forward in UUID order. Structural parents are active/draft, avoiding completed parents with open children. The [manifest](fixture.json) records exact node-state/edge-type counts, sizes and durability settings. Independent SQL checks found zero multiple-parent children, zero backward structural/blocking arcs, maximum depth 3, zero verified/disputed knowledge, and zero completed structural parents.

## Finding and correction

The first schema-5 readiness SQL used a correlated recursive ancestor CTE for each candidate task. Its [before plan](plans-before-ready-fix.json) recorded:

- Total estimated cost 358,881.44, triggering PostgreSQL JIT compilation of 57 functions (18.498 ms).
- 586 recursive-ancestor executions and 334 repeated filtered node scans, despite each candidate having at most two ancestors.
- One EXPLAIN ANALYZE execution of 128.228 ms. Twenty client requests had p50~53.03ms/p95~55.17ms, including HTTP and debug serialization.

The correction keeps the existing child/prerequisite predicates but replaces per-candidate recursion with parent/grandparent joins. Schema 0004 and every structural mutation enforce at most three hierarchy levels; the two-ancestor predicate is valid because that bound is enforced. Raising the depth limit requires extending this query. Both exact parameterized SQL statements and fixture-literal variants are retained with the respective plans, making the predicate difference reviewable.

After rebuilding/restarting the service on the **same unchanged fixture**, the [after plan](plans.json) estimated cost 1,403.74, used no JIT, and executed in 3.403 ms. Twenty HTTP requests had p50~5.11ms/p95~6.20ms. The complete ready result contained 20 node IDs and no next cursor, and was **identical before and after**: [before identities](ready-identities-before.json), [after identities](ready-identities-after.json).

| After-fix operation | Attempts | HTTP 200 | Client p50 | Client p95 | Single SQL execution |
| --- | ---: | ---: | ---: | ---: | ---: |
| Ready,limit 20 | 20 | 20 | 5.11 ms | 6.20 ms | 3.403 ms |
| Search authentication,limit 20 | 20 | 20 | 3.72 ms | 4.30 ms | 0.789 ms |
| Structural tree,depth 2,limit 20 | 20 | 20 | 3.29 ms | 3.90 ms |Not separately measured |

The [before HTTP observations](requests-before-ready-fix.json) and [after observations](requests.json) retain all smoke summaries. The service remained on loopback and used a separate `aq_graph_bench_parity` database; no AQ database or live graph was accessed.

## Limits and reproduction

This is a small debug-build smoke on a shared machine, with one recorded EXPLAIN execution and 20 requests per operation. The before execution also had different cache/JIT state. The numbers identify and check a concrete query-plan problem; they are not isolated performance attribution, an SLO result, large-DAG throughput, concurrent-update behavior or production capacity. The existing 74-test regression and translated graph tests, including draft-grandparent withholding, separately check behavior. Full release-build 100k/500k mixed load must be rerun before claiming current capacity.

Create a fresh disposable schema-5 database, run the current service to migrate it, then seed:

```sh
DATABASE_URL=postgres://USER@127.0.0.1/aq_graph_bench_parity \
  python3 scripts/benchmark.py seed --nodes 1000 --edges 5000 \
  --token-file /private/benchmark-token --manifest /tmp/fixture.json
```

Pass `--psql /path/to/psql` if needed. The seed refuses non-benchmark database names and nonempty node tables. Run the exact fixture-literal read SQL from the retained plans with `EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)` using the declared database; query authorized HTTP ready/search/tree with the same workspace and bounded arguments. Never seed or run write-bearing plan analysis against an authoritative database.
