# Implementation status

The standalone aq-graph baseline is implemented in Rust with PostgreSQL. Agent Q is not required to run it. The current release is 0.1.0, HTTP API v1, and schema migration 0007.

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

The current schema-7 aggregate suite passed **119 tests, 0 failed/ignored**, including **95 against real PostgreSQL**. It includes thirteen shared API scenarios, four schema-7 migration cases, two shared-authority regressions, eleven storage invariants, nine CLI unit tests and two process-contract tests, alongside existing graph, revision, transport and concurrency coverage. The measured command took 25.71 seconds including incremental compilation; an intentional HTTP deadline test accounts for ten seconds. This is test-run duration, not a request latency or capacity measurement.

Formatting, strict Clippy, Python compilation, shell syntax, local documentation links, and OpenAPI references were checked. The current schema-7 CLI and service were exercised over real HTTP, including summaries, nullable dates, shared verification and successor retirement. The earlier baseline also exercised the Python adapter and native PostgreSQL dump/restore; those older recovery checks do not replace a fresh deployment-target backup/recovery rehearsal for the expanded schema.

Performance methodology is in [benchmarking](benchmark.md), with [local results and raw evidence](performance/README.md). The historical schema-0003 100,000-node and 500,000-edge run sustained 200 requests/second and a 1,000/second burst with successful-request p95 around 9 ms and no server or transport failures. Deliberate mutation conflicts are counted separately. Maximum-load tail latency rises materially; these measurements do not establish production capacity on a small VPS or performance of the schema-0005 hierarchy or schema-0006 revision engine. A [schema-5 smoke comparison](performance/schema5-smoke/README.md) on 1,000 nodes and 5,000 edges caught a readiness-plan regression: bounded ancestor joins reduced local warm-cache p95 from about 55 ms to 6.2 ms with identical results. This debug-build, sequential check is historical schema-5 evidence, not a current schema-6 or concurrent capacity benchmark.

## Deployment and migration limits

Docker is unavailable in the implementation environment, so the Docker image and Compose lifecycle have not been executed locally. The CI workflow includes image, Compose, and restore checks; workflow definitions are present but have not been run on a remote repository. Native runtime tests used PostgreSQL 16.15, while Compose and CI target PostgreSQL 17. No external release, hosting purchase, or production deployment has occurred.

Production ingress must enforce connection and slow-client write/idle limits in addition to application request limits. Configure off-host backup storage and test recovery on the deployment target. Benchmark reference hardware, the production p99 budget, controlled cold-cache behavior, and larger growth scenarios remain to be validated before production acceptance.

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
