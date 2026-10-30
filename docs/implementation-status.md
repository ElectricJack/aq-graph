# Implementation status

The standalone aq-graph baseline is implemented in Rust with PostgreSQL. Agent Q is not required to run it. The current release is 0.1.0, HTTP API v1, and schema migration 0006.

## Available behavior

- Task and knowledge records with explicit schemas, stable identities, workspace scope, optimistic version checks, and tombstones.
- Typed dependency, hierarchy, and informational relationships; combined cycle protection, single-parent/depth limits, atomic reparenting, bounded trees, and intrinsic readiness that respects draft ancestors and container children.
- Knowledge author ownership, privileged evidence-backed verification/dispute, correction invalidation, reasoned retirement, typed provenance, and explicit retired/disputed search flags.
- Bounded context retrieval, exact pinned revisions, current-record pagination, and full-text knowledge search.
- A common task/knowledge logical revision engine: content, lifecycle, relationship membership, parents and tombstones; exact revision reads, node/relationship diffs and restore with an identical opt-in lifecycle flag for both kinds.
- Atomic mutation batches, persistent draft change sets, rollback-only validation previews, and atomic draft application.
- Scoped read/write/admin credentials, hashed token storage, revocation, and protection against removing the last administrator.
- Same-key retry receipts, structured API/CLI errors, request limits and deadlines, bounded connection pools, readiness checks, and graceful shutdown.
- Native CLI, OpenAPI contract, standalone Docker Compose configuration, CI and release workflow definitions, backup/restore scripts, load fixtures, and an example Python consumer.

Record edits acquire row locks before checking expected versions. Graph mutations use serializable transactions with bounded retries to protect invariants. Ordinary content changes do not acquire a workspace-wide lock. Current reads do not replay historical revisions. Relationship changes are represented by explicit removal and addition within one atomic batch; arbitrary edits to an existing relationship are not exposed.

## Verification

The schema-6 implementation passes **91 tests**, including **74 against real PostgreSQL**, with no skipped database tests. This includes nine shared task/knowledge versioning scenarios, four baseline-migration tests, two complete draft-preview tests, immutable relationship-set reuse/representation checks, receipt compatibility, and the existing graph, knowledge, CLI, transport and concurrency suites. Exact historical node snapshots match current reads at the same revision, including timestamp precision.

Formatting, strict Clippy, Python compilation, shell syntax, local documentation links, and OpenAPI references were checked. The current schema-6 CLI and service were exercised over real HTTP. The earlier baseline also exercised the Python adapter and native PostgreSQL dump/restore; those older recovery checks do not replace a fresh deployment-target backup/recovery rehearsal for the expanded schema.

Performance methodology is in [benchmarking](benchmark.md), with [local results and raw evidence](performance/README.md). The historical schema-0003 100,000-node and 500,000-edge run sustained 200 requests/second and a 1,000/second burst with successful-request p95 around 9 ms and no server or transport failures. Deliberate mutation conflicts are counted separately. Maximum-load tail latency rises materially; these measurements do not establish production capacity on a small VPS or performance of the schema-0005 hierarchy or schema-0006 revision engine. A [schema-5 smoke comparison](performance/schema5-smoke/README.md) on 1,000 nodes and 5,000 edges caught a readiness-plan regression: bounded ancestor joins reduced local warm-cache p95 from about 55 ms to 6.2 ms with identical results. This debug-build, sequential check is historical schema-5 evidence, not a current schema-6 or concurrent capacity benchmark.

## Deployment and migration limits

Docker is unavailable in the implementation environment, so the Docker image and Compose lifecycle have not been executed locally. The CI workflow includes image, Compose, and restore checks; workflow definitions are present but have not been run on a remote repository. Native runtime tests used PostgreSQL 16.15, while Compose and CI target PostgreSQL 17. No external release, hosting purchase, or production deployment has occurred.

Production ingress must enforce connection and slow-client write/idle limits in addition to application request limits. Configure off-host backup storage and test recovery on the deployment target. Benchmark reference hardware, the production p99 budget, controlled cold-cache behavior, and larger growth scenarios remain to be validated before production acceptance.

The [AQ integration guide](aq-integration.md) and Python adapter establish the client boundary. AQ v2 is a fresh start: wire the adapter to a new workspace and validate the v2 workflows. V1 import, legacy-history ingestion, and supported migration/cutover tooling are excluded from aq-graph. An optional owner-specific transfer would be an external one-off script. No existing AQ code or database was changed.

Semantic search, asynchronous indexing, offline replicas and merging, whole-graph time travel, execution claims, and permanent erasure are deferred as specified. No synthetic benchmark history should be mistaken for a legacy migration capability.

## V1 compatibility boundary

[Specification 0002](specs/0002-v1-behavioral-compatibility.md) records the first compatibility increment and supersedes the original organizational-only hierarchy behavior. The [source inventory](compatibility/README.md) tracks 793 test definitions from 48 pinned v1 files; it does not claim all were translated or executed. Schema 6 adds tested historical relationship restoration to the common revision engine. Remaining differences to evaluate for v2 include canonical content digests, richer shared node fields/search, global sharing and source revocation, and review authority. These are design candidates, not automatic v1-parity obligations; v1 migration tooling is excluded. Scheduling, claims, gates, and automatic container settlement remain AQ responsibilities.

## Common revision compatibility update

[Specification 0003](specs/0003-unified-node-versioning.md) defines the shared task/knowledge revision contract. The existing `content_revision` wire field now advances for every meaningful logical-record mutation. Its snapshot includes current node fields, all outgoing relationships, and incoming structural membership (`blocks`/`parent-child`); target contents are not recursively frozen. Exact reads disclose unavailable legacy relationship sets rather than inventing empty sets. Migration 0006 preserves legacy history and appends an attributed complete baseline.

Default restore preserves current lifecycle, retirement reason and tombstone; both task and knowledge commands expose `--include-lifecycle` for deliberate historical lifecycle restoration. Kind-specific validation and current authorization still apply, and meaningful knowledge restores reset verification. The schema-5 compatibility inventory and historical measurements remain provenance for that earlier increment; counts and passing scenarios are not automatically evidence for this revised contract.

Current schema-6 runtime smoke: seven CLI unit tests and two process-contract tests passed, including exact revision reads and identical restore payloads/options for both kinds. The [schema-6 smoke](performance/schema6-smoke/README.md) seeded 1,000 nodes/5,000 edges with complete current baselines and passed 80 sequential ready/search/task-write/knowledge-write requests. Both kinds retained exact old snapshots and reused unchanged relationship sets. This debug-build smoke is operational evidence, not a concurrent production capacity claim.

## Shared node structure under discussion

The runtime already uses one `nodes` table and one Rust `Node` type. Its API still restricts several fields by kind. The proposed next simplification is a shared field contract for sources, summaries, validity/review dates and replacement relationships, with explicit task execution/planning properties where needed. This is a design direction, not an implemented expansion of the current schema.
