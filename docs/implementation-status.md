# Implementation status

The standalone aq-graph baseline is implemented in Rust with PostgreSQL. Agent Q is not required to run it. The current release is 0.1.0, HTTP API v1, and schema migration 0008.

## Available behavior

- Task and knowledge records with explicit schemas, stable identities, workspace scope, optimistic version checks, and tombstones.
- Typed dependency, hierarchy, and informational relationships; combined cycle protection, single-parent/depth limits, atomic reparenting, bounded trees, and intrinsic readiness that respects draft ancestors and container children.
- Shared content classification, typed provenance, verification/dispute, summaries pinned to source revisions, nullable validity/review dates, retirement reasons/successors and query filters. Knowledge author ownership and task execution fields remain explicit kind-specific rules.
- Bounded context retrieval, exact pinned revisions, current-record pagination, and full-text record search with shared kind/validity/review filters.
- A common task/knowledge logical revision engine: content, lifecycle, relationship membership, parents and tombstones; exact revision reads, node/relationship diffs and restore with an identical opt-in lifecycle flag for both kinds.
- Atomic mutation batches, persistent draft change sets, rollback-only validation previews, and atomic draft application.
- Scoped read/write/admin credentials, hashed token storage, revocation, and protection against removing the last administrator.
- Same-key retry receipts, structured API/CLI errors, request limits and deadlines, bounded connection pools, readiness checks, and graceful shutdown.
- Native CLI, OpenAPI contract, standalone Docker Compose configuration, CI and release workflow definitions, backup/restore scripts, load fixtures, and an example Python consumer.

Record edits acquire row locks before checking expected versions. Graph mutations use serializable transactions with bounded retries to protect invariants. Ordinary content changes do not acquire a workspace-wide lock. Current reads do not replay historical revisions. Relationship changes are represented by explicit removal and addition within one atomic batch; arbitrary edits to an existing relationship are not exposed.

## Verification

The current schema-8 aggregate suite passed **125 tests, 0 failed/ignored**, including **101 against real PostgreSQL 17**. New coverage checks readiness-projection backfill without history changes, direct SQL lifecycle/edge mutation invariants, proven overlapping concurrent writes, and administrative repair of corrupted/missing projection rows. Search performs 1,296 comparisons against the full reference query across custom/generic plans, filters, term types and UUID cursor boundaries. Existing graph, shared-field, revision, transport, CLI and concurrency tests remain included. Nine Python benchmark-accounting/observation tests pass.

Formatting and strict Clippy pass for schema 8. Python compilation, shell syntax, local documentation links and OpenAPI references were checked in the preceding validation. The schema-7 CLI and service were exercised over real HTTP, including summaries, nullable dates, shared verification and successor retirement. The current schema was also backed up and restored through the actual PostgreSQL 17 Compose stack, with exact table/API comparisons and post-restore writes. This local drill does not replace a VPS/ingress and off-host recovery rehearsal.

Performance methodology is in [benchmarking](benchmark.md), with [local results and raw evidence](performance/README.md). The historical schema-0003 100,000-node and 500,000-edge run sustained 200 requests/second and a 1,000/second burst with successful-request p95 around 9 ms and no server or transport failures. Deliberate mutation conflicts are counted separately. Maximum-load tail latency rises materially; these measurements do not establish production capacity on a small VPS or performance of the schema-0005 hierarchy or schema-0006 revision engine. A [schema-5 smoke comparison](performance/schema5-smoke/README.md) on 1,000 nodes and 5,000 edges caught a readiness-plan regression: bounded ancestor joins reduced local warm-cache p95 from about 55 ms to 6.2 ms with identical results. This debug-build, sequential check is historical schema-5 evidence, not a current schema-6 or concurrent capacity benchmark.

## Deployment and migration limits

The actual Dockerfile and isolated Compose stack have now been exercised with PostgreSQL 17.11. The parent independently verified startup, nonroot/read-only operation, restart, same-schema image recreation, exact backup/restoration of records and history, retry receipts and new writes after restoration. An internal-only networking configuration that prevented host ingress was fixed. CI now uses the same reproducible [deployment/recovery drill](deployment-validation.md); remote GitHub workflows, cross-schema upgrades, hosting purchase and public production deployment remain unperformed.

Production ingress must enforce connection and slow-client write/idle limits in addition to application request limits. Configure off-host backup storage and test recovery on the deployment target. The recorded reference host is a shared 24-thread Ryzen desktop with service, database and generator colocated. A production VPS reference, agreed p99 budget, controlled cold-cache behavior and larger growth scenarios remain unvalidated. The current performance acceptance targets are not all met.

The [AQ integration guide](aq-integration.md) and Python adapter establish the client boundary. AQ v2 is a fresh start: wire the adapter to a new workspace and validate the v2 workflows. V1 import, legacy-history ingestion, and supported migration/cutover tooling are excluded from aq-graph. An optional owner-specific transfer would be an external one-off script. No existing AQ code or database was changed.

Semantic search, asynchronous indexing, offline replicas and merging, whole-graph time travel, execution claims, and permanent erasure are deferred as specified. No synthetic benchmark history should be mistaken for a legacy migration capability.

## V1 compatibility boundary

[Specification 0002](specs/0002-v1-behavioral-compatibility.md) records the first compatibility increment and supersedes the original organizational-only hierarchy behavior. The [source inventory](compatibility/README.md) tracks 793 test definitions from 48 pinned v1 files; it does not claim all were translated or executed. Schema 6 adds tested historical relationship restoration to the common revision engine. Remaining differences to evaluate for v2 include canonical content digests, optional ranked retrieval, global sharing and source revocation, and review authority. These are design candidates, not automatic v1-parity obligations; v1 migration tooling is excluded. Scheduling, claims, gates, and automatic container settlement remain AQ responsibilities.

## Common revision compatibility update

[Specification 0003](specs/0003-unified-node-versioning.md) defines the shared task/knowledge revision contract. The existing `content_revision` wire field now advances for every meaningful logical-record mutation. Its snapshot includes current node fields, all outgoing relationships, and incoming structural membership (`blocks`/`parent-child`); target contents are not recursively frozen. Exact reads disclose unavailable legacy relationship sets rather than inventing empty sets. Migration 0006 preserves legacy history and appends an attributed complete baseline.

Default restore preserves current lifecycle, retirement reason and tombstone; both task and knowledge commands expose `--include-lifecycle` for deliberate historical lifecycle restoration. Kind-specific validation and current authorization still apply, and meaningful knowledge restores reset verification. The schema-5 compatibility inventory and historical measurements remain provenance for that earlier increment; counts and passing scenarios are not automatically evidence for this revised contract.

Historical schema-6 runtime smoke: seven CLI unit tests and two process-contract tests passed, including exact revision reads and identical restore payloads/options for both kinds. The [schema-6 smoke](performance/schema6-smoke/README.md) seeded 1,000 nodes/5,000 edges with complete current baselines and passed 80 sequential ready/search/task-write/knowledge-write requests. Both kinds retained exact old snapshots and reused unchanged relationship sets. This debug-build smoke is operational evidence, not a concurrent production capacity claim.

## Approved shared node contract

[Specification 0004](specs/0004-shared-node-model.md) defines the common field contract implemented by schema 0007. Both kinds use canonical `content_type`, typed provenance, explicit evidence-backed verification, revision-pinned summaries, nullable validity/review dates and retirement/successor metadata. Task execution state and acceptance criteria remain task-only. Dates are descriptive filters rather than deadlines; verification never completes a task. Search defaults to both kinds, with explicit kind/date/retirement/dispute controls. Successor→old supersedes edges may connect either kind and are checked with pointer updates against the final atomic batch.

The CLI exposes flattened common create/update options, conflict-checked date clearing, shared `verify_node`, retirement with existing or atomically created successor edges, and `summary` set/clear with expected version and source revision. Current focused CLI verification passed nine unit tests and two process-contract tests; they cover exact payloads for both kinds, canonical input names, nullable dates, summary pinning, successor batch order and shared list/search parameters. The [schema-7 smoke](performance/schema7-smoke/README.md) passed 80 sequential read/write attempts plus real CLI shared-field workflows on 1,000 nodes/5,000 edges. Its debug-build timings were higher than the separate schema-6 observations and are explicitly not a controlled speed comparison or production capacity claim. One readiness plan retained bounded joins without recursive union/JIT. Earlier performance artifacts remain historical.

## Full-scale performance and deployment validation

The [schema-7 baseline](performance/schema7-load/README.md) used a release build, PostgreSQL 17.11 with durability enabled, 100,000 nodes, 500,000 edges and 300,000 initial revisions. It completed roughly 170–180 attempts/second under fixed concurrency and dropped sustained 200/s offers. Exact prepared plans exposed readiness scanning more than 50,000 parents before returning a page and generic history plans scanning roughly 700,000–800,000 unrelated entries.

That schema-7 optimization selected narrow leaf identities before fetching full records; history isolated node membership before sequence pagination. Per-connection JIT is disabled to avoid measured compilation overhead in the new generic readiness plan. Exact result parity and custom/generic plan tests preserve workspace, ordering, retirement, ancestor and prerequisite rules. Those query versions still enumerated candidate identities or a node's history; they were not constant-cost reads on every distribution.

The [fresh optimized experiment](performance/schema7-load-optimized/README.md) suffered severe early overload and was interrupted during open-loop traffic. Automatic vacuum overlapped the slow period; missing pre-run visibility data means the cause is not proven. This failed evidence remains visible. It is not superseded by the subsequent warm result.

The distinct [post-maintenance run](performance/schema7-load-optimized-steady/README.md), after explicit VACUUM/ANALYZE on the previously written fixture, completed all 6,000 sustained offers at 200/s with no client drops or HTTP 5xx. Expected validation/conflict responses remain counted. Client p95 was 3.9 ms for gets, 9.7 ms for context, and 19.7 ms for independent updates. Readiness (164.8 ms) and broad search (305.2 ms) still missed their 100/200 ms targets. The 1,000/s burst dropped 6,824 offers at the bounded client queue; recovery dropped one. Fixed 50/100/200-client latency targets also remain unmet. This documents steady-state operation under those conditions, not full performance or production acceptance, and its maintenance/cache conditions prevent a normalized speedup claim over the original baseline.

That schema-7 run motivated the schema-8 readiness projection and bounded common-term search path described below. Pool wait duration, larger growth/soak workloads, real VPS resources and TLS/Cloudflare remain unvalidated. The current 200-worker load harness does not establish behavior at the 256-request server admission limit.

The parent independently reviewed the Sol agents' changes, reran the complete tests and Docker recovery drill, audited raw request counts and verified that retained source/binary digests match the final runtime. [Check evidence](artifacts/validation/2026-10-30-checks.json) and [deployment evidence](deployment-validation.md) record the commands and limits. All disposable validation services, containers and volumes were removed; no AQ v1 data or public deployment was touched.


## Schema 8 query and maintenance work

Ready queries use a narrow, indexed projection of active-task eligibility and live child counts. Database triggers update it atomically with node lifecycle and edge changes, including restoration; ordinary body edits avoid projection writes. Current ancestor/prerequisite checks remain authoritative. The migration backfills existing data without changing node revisions or history; administrator check/rebuild SQL is documented in [operations](operations.md).

Search probes the next 256 live workspace UUIDs and uses the existing materialized full-text fallback only when that prefix cannot fill the requested page. Both paths share one statement snapshot and preserve filters, ordering and pagination. The probe is bounded by rows, not bytes; selective queries still pay its cost, and broadly matching suffixes with restrictive filters can still require substantial work.

The benchmark can now capture memory/autovacuum settings, planner visibility estimates and maintenance progress. Explicit seed vacuuming remains opt-in, and a guarded maintenance command can overlap live traffic. Asynchronous open-loop observations do not pause arrival phases. These observations improve diagnosis; they do not make temporal correlation proof of causation.

Two full 100,000-node/500,000-edge schema-8 profiles use the identical release binary and default PostgreSQL memory settings: [first application load after backfill](performance/schema8-load-first/README.md) and [explicit maintenance overlap](performance/schema8-load-maintenance/README.md). Both complete all 6,000 sustained, 10,000 burst and 6,000 recovery offers without client drops, HTTP 5xx or timeouts; deliberate 400/409 rejects remain counted. All measured service routes fit their provisional p95 budgets in those arrival-rate windows. First-profile client p95 at 200/s is ready 5.97 ms, search 4.47 ms and independent update 14.40 ms; burst p95 is ready 8.72 ms, search 6.20 ms and independent update 20.41 ms.

This does not establish every target: 200 continuously busy clients saturate the system and exceed the mutation service budget, with substantially larger client latency. Sparse multi-workspace data, restrictive search filters, larger graphs/history, a long soak, controlled cold caches, pool wait instrumentation, server admission shedding, and actual VPS/TLS/Cloudflare deployment remain unvalidated. Earlier and current fixture/maintenance conditions differ, so no normalized speedup or general production capacity claim follows. [Independent checks](artifacts/validation/2026-10-30-schema8-checks.json) record tests and source/binary verification.

The schema-8 Docker build/recovery drill also passes, including exact projection table equality and leaf readiness before/after restart and restore. [Deployment evidence](artifacts/deployment/2026-10-30-schema8-recovery.json) records the image and checks.

All schema-8 disposable native services, PostgreSQL container storage and deployment-drill resources were removed after validation. The built validation image and local artifacts remain available; AQ v1 resources were untouched.
