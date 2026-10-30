# Implementation status

The standalone aq-graph baseline is implemented in Rust with PostgreSQL. Agent Q is not required to run it. The current release is 0.1.0, HTTP API v1, and schema migration 0005.

## Available behavior

- Task and knowledge records with explicit schemas, stable identities, workspace scope, optimistic version checks, and tombstones.
- Typed dependency, hierarchy, and informational relationships; combined cycle protection, single-parent/depth limits, atomic reparenting, bounded trees, and intrinsic readiness that respects draft ancestors and container children.
- Knowledge author ownership, privileged evidence-backed verification/dispute, correction invalidation, reasoned retirement, typed provenance, and explicit retired/disputed search flags.
- Bounded context retrieval, exact pinned revisions, current-record pagination, and full-text knowledge search.
- Selective content revisions, lifecycle and relationship events, history, diffs, and restoration that preserves current task state.
- Atomic mutation batches, persistent draft change sets, rollback-only validation previews, and atomic draft application.
- Scoped read/write/admin credentials, hashed token storage, revocation, and protection against removing the last administrator.
- Same-key retry receipts, structured API/CLI errors, request limits and deadlines, bounded connection pools, readiness checks, and graceful shutdown.
- Native CLI, OpenAPI contract, standalone Docker Compose configuration, CI and release workflow definitions, backup/restore scripts, load fixtures, and an example Python consumer.

Record edits acquire row locks before checking expected versions. Graph mutations use serializable transactions with bounded retries to protect invariants. Ordinary content changes do not acquire a workspace-wide lock. Current reads do not replay historical revisions. Relationship changes are represented by explicit removal and addition within one atomic batch; arbitrary edits to an existing relationship are not exposed.

## Verification

The full Rust suite passes **74 tests**, including **58 against real PostgreSQL**. The first [v1 compatibility increment](compatibility/README.md) adds 22 graph scenarios, 11 knowledge scenarios, 10 upgrade-safety regressions, and three CLI tests to the original 28-test baseline. No database tests are skipped. Coverage includes competing updates and cycle insertions, delete/link races, concurrent administrator demotions, authorization isolation, draft replay and rollback, selective history, exact pinned content, oversized response rollback, full-text pagination, CLI retry keys, and transport overload/deadline behavior.

Formatting, strict Clippy, shell syntax, Python compilation, YAML parsing, local documentation links, and OpenAPI schemas were checked. The optimized CLI and Python adapter were exercised over real HTTP. A native PostgreSQL dump was restored into a separate database and checked through the API for current state, links, exact history, authentication, and idempotent replay.

Performance methodology is in [benchmarking](benchmark.md), with [local results and raw evidence](performance/README.md). The historical schema-0003 100,000-node and 500,000-edge run sustained 200 requests/second and a 1,000/second burst with successful-request p95 around 9 ms and no server or transport failures. Deliberate mutation conflicts are counted separately. Maximum-load tail latency rises materially; these measurements do not establish production capacity on a small VPS or performance of the new schema-0005 hierarchy rules. A [schema-5 smoke comparison](performance/schema5-smoke/README.md) on 1,000 nodes and 5,000 edges caught a readiness-plan regression: bounded ancestor joins reduced local warm-cache p95 from about 55 ms to 6.2 ms with identical results. This debug-build, sequential check is not a concurrent capacity benchmark.

## Deployment and migration limits

Docker is unavailable in the implementation environment, so the Docker image and Compose lifecycle have not been executed locally. The CI workflow includes image, Compose, and restore checks; workflow definitions are present but have not been run on a remote repository. Native runtime tests used PostgreSQL 16.15, while Compose and CI target PostgreSQL 17. No external release, hosting purchase, or production deployment has occurred.

Production ingress must enforce connection and slow-client write/idle limits in addition to application request limits. Configure off-host backup storage and test recovery on the deployment target. Benchmark reference hardware, the production p99 budget, controlled cold-cache behavior, and larger growth scenarios remain to be validated before production acceptance.

The [AQ integration guide](aq-integration.md) and Python adapter establish the client boundary. Wiring the adapter into a selected AQ project, implementing a legacy-history import/export format, and executing a reconciled pilot cutover remain separate migration work. No existing AQ code or database was changed.

Semantic search, asynchronous indexing, offline replicas and merging, whole-graph time travel, execution claims, and permanent erasure are deferred as specified. No synthetic benchmark history should be mistaken for a legacy migration capability.

## V1 compatibility boundary

[Specification 0002](specs/0002-v1-behavioral-compatibility.md) records the first compatibility increment and supersedes the original organizational-only hierarchy behavior. The [source inventory](compatibility/README.md) tracks 793 test definitions from 48 pinned v1 files; it does not claim all were translated or executed. Remaining obligations include historical link restoration, canonical content digests, richer knowledge fields/search, global sharing and source revocation, review authority, and migration tooling. Scheduling, claims, gates, and automatic container settlement remain AQ responsibilities.
