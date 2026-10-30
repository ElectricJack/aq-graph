# V1 behavioral compatibility inventory

Schema 6 introduces common logical revisions. Current focused runs passed 22 graph, 11 knowledge and 9 common-revision scenarios on real disposable PostgreSQL. Each source row was reviewed separately: only eight complete portable definitions retain translated status; partial source functions remain open. Earlier schema-5 evidence is retained separately.

This inventory compares evidence from Agent Queue v1 with selected aq-graph behaviors. AQ v2 is a fresh start: the map is a reference and test-evidence inventory, not a requirement to reproduce every v1 feature or reach 100% parity. V1 import/history migration is explicitly outside aq-graph; any optional owner-specific transfer belongs in an external one-off script. Internal aq-graph schema upgrade tests remain applicable. This inventory does not certify full parity. The source is pinned to **`agent-queue2` commit `9a9b8e8ef992d1ff699755a9383ed3c91e2844ea`**. Inspection used immutable Git blobs and Python AST parsing only: no v1 imports, test collection, test execution, daemon commands, database access or source changes.

The machine-readable [inventory](v1-test-inventory.json) covers direct `tests/test_record*.py`, `test_knowledge*.py`, `test_hierarchy*.py`, `test_dependency_queries.py`, and `test_task_graph*.py`, plus `tests/perf/test_hierarchy_statements.py`. Other v1 test families are outside this specific inventory and remain unassessed. This is a source-function map, not a guessed list of thousands of pytest IDs.

## Counts and scope

The pinned selection contains **48 files and 793 test definitions**. There are 101 parameterized definitions. Static AST inspection resolves decorator combination counts for 784 definitions (sum 1,147 combinations), while nine have dynamic values. Fixture parameterization, generated fixture manifests, plugin collection, skips and indirect expansion are not executed. **Neither 793 nor 1,147 is the number of collected or passing pytest cases.**

| Category | Definitions | Initial disposition |
| --- | ---: | --- |
| Graph core | 66 | Portable graph invariants and retrieval; exact assertion translation required |
| Knowledge core | 96 | Content/history/protection/evidence/proposal invariants; exact translation required |
| AQ integration | 264 | Command registration, YAML/spec graph parsing, harness/prompt delivery, dashboard and vault adapters |
| AQ orchestration | 110 | Session/claim/routing policy, resource release, automatic container settlement and LLM budgets |
| Migration | 115 | V1 import/cutover/schema cases are excluded; separately grouped redaction cases remain deferred |
| Optional integration | 21 | Capture/export/outbox consumers absent from the synchronous baseline |
| Optional providers | 119 | Semantic indexing, legacy memory adapters, ranking, paid generation and provider budgets |
| Performance | 2 | V1 SQLAlchemy statement-count budgets require separate v2 plan/budget tests |

Initial triage recorded 162 core definitions needing translation. The schema-5 increment verified 8 complete portable source definitions and left 154 open/partial. After schema-6 source/target re-review and passing focused runs, **8 definitions are translated, 154 remain open/partial, 525 remain deferred, and 106 v1 migration/compatibility definitions are excluded from product scope**. Historical verification is retained per mapped row. Deferral records a component boundary or a feature that needs a v2 scope decision. Exclusion records behavior that is outside the product roadmap. Neither is implementation or parity credit. Full v1 parity is not a v2 delivery goal. Selected portable assertions from an otherwise deferred subsystem may be brought into graph-core scope; such reclassification must retain the original source identity and document omitted obligations.

The priority counts are P0: 73, P1: 89, P2: 117, P3: 514. P0 targets authorization, protected knowledge, provenance, revisions/CAS/replay, relationship validity and atomicity. P1 covers remaining graph reads, hierarchy and portable command behavior. P2 covers migration and performance. P3 records AQ integration/orchestration and optional systems. These are historical comparison/translation priorities rather than implementation commitments, severity scores or proof of coverage. Excluded cases are not scheduled work.

## What the map proves

Each row records its exact source path and class-qualified function name, pinned line range and function SHA-256, a small sample of actual assertions, called operations, parameter decorators, category, priority, disposition, reason and exact Rust target functions where available. Source files have independent SHA-256 hashes. The assertion/call samples are navigation aids, not substitutes for inspecting the complete pinned function and fixtures.

The allowed dispositions are:

| Status | Meaning |
| --- | --- |
| `needs_translation` | Untranslated, failed, not yet run, or only selected source assertions covered. An existing test with a similar name is insufficient. |
| `existing_coverage` | An existing v2 test has been inspected against every portable obligation of this source function and passed a recorded focused command. |
| `translated` | A source-backed v2 translation covers every portable obligation and passed a recorded focused command. Identity/wire adaptations and excluded AQ-only mechanics must be explicit. |
| `deferred` | A component boundary or feature requiring a separate v2 scope decision. This is not parity credit. |
| `excluded` | Outside the product roadmap, including supported v1 import/cutover compatibility. Any optional owner-specific transfer is an external script. |

`candidate_v2_tests` identifies authored or potentially reusable tests without upgrading coverage claims. A source function can contribute assertions to several Rust tests; a Rust test can exercise assertions from several source functions. These many-to-many links must not inflate the number of fully translated source cases.

`coverage_scope: selected_assertions` keeps the source row at `needs_translation` even after those assertions pass. For example, the v1 snapshot-limit function has fifteen decorator cases: verification and typed-provenance translations do not also prove its metadata, body, summary, time-window and tag limits. An HTTP invariant test also does not prove v1 direct-SQL trigger protections against database writers. A knowledge correction test that resets verification does not prove AQ review/authority-grant revocation.

## Executable translations and exposed gaps

The [graph translation](graph.md) maps 22 Rust scenarios in [v1_graph_parity.rs](../../tests/v1_graph_parity.rs). Its initial baseline run was **7 passing and 13 failing**; every new scenario ran. Initial red-run evidence is retained; the earlier schema-5 graph run passed **22/22 scenarios**, including two later invariant extensions. These are selected scenario counts, not 22 fully translated source functions. The failing assertions identified endpoint-kind validation, single structural parent, bounded hierarchy depth/tree retrieval, mixed dependency/parent cycles, ancestor activation/completion guards, duplicate structural insertion and edge descriptions. Passing selected assertions cover parts of graph atomicity, scope isolation, informational separation and exact pins. They do not certify the whole corresponding v1 source function.

The [knowledge translation](knowledge.md) maps 11 Rust scenarios in [v1_knowledge_parity.rs](../../tests/v1_knowledge_parity.rs). Initial translation targets include author ownership, protected edit/reset, privileged evidence-backed verification/dispute, typed provenance, reasoned retirement/exact restoration, proposal decision authority and live authorization before replay. The initial nine-scenario baseline was **1 passing and 8 failing** on real PostgreSQL. The earlier schema-5 run passed **11/11 scenarios**, including source-link authorization and privileged-replay extensions. Runtime results and exact source correspondences are recorded per target; partial source functions remain open.

This pass deliberately surfaces stronger requirements rather than changing expected assertions to match missing behavior. The core contract was revised to satisfy the selected translated assertions; the full v1 inventory is not complete. In particular, parent membership/activation and completion guards are explicit compatibility additions; they were not all properties of the initial structural-only v2 baseline. Lifecycle mapping from v1 DEFINED/READY/IN_PROGRESS/FAILED to v2 draft/active/completed/canceled is limited to documented assertions and must not silently reinterpret real execution outcomes during migration.

Remaining core differences to evaluate against v2 needs include canonical content digests/ordering, complete snapshot limits, filter-bound/ranked search, exact global-sharing/source revocation, review-bound authority, richer proposal decisions/rejection, direct-SQL append-only protection and full source-function fixture arms. Historical relationship restoration is now verified by common schema-6 scenarios for both kinds; this closes selected previously missing assertions without certifying whole v1 link-token/table workflows; unknown legacy link sets remain unavailable rather than being invented. AQ sessions/claims, automatic settlement, unsupported dependency kinds, semantic/provider/generation systems retain explicit boundaries in their own rows. V1 migration tooling is excluded rather than deferred to an aq-graph implementation milestone.

Schema 7 implements shared task/knowledge validity and review fields, revision-bound summary invalidation, and retirement successor consistency; see the [shared-node specification](../specs/0004-shared-node-model.md) and `tests/shared_nodes.rs`. These are v2 contract tests, not a claim that every related v1 source function has been translated. The source dispositions and excluded importer scope remain unchanged. Search now includes both node kinds by default; knowledge-only comparisons use an explicit kind filter. Lifecycle restoration requires admin for both kinds.

## Reproduce and maintain

Validate all local target references, dispositions and counts without a v1 checkout:

```sh
python3 scripts/check-parity-map.py
```

Also verify every source file/function hash, assertion sample and definition against the pinned read-only checkout:

```sh
python3 scripts/check-parity-map.py --source-root /home/jkern/dev/agent-queue2
```

Refresh deterministic AST evidence while preserving unchanged reviewed dispositions:

```sh
python3 scripts/check-parity-map.py --source-root /home/jkern/dev/agent-queue2 --refresh
```

The recorded revision remains fixed on refresh. An intentional source update requires `--revision FULL_COMMIT_SHA`, review of new/changed definitions, and a fresh assertion-level coverage audit. Changed source files lose prior coverage credit, including changes to fixtures or parameter decorators outside a function body. The script never imports v1 or runs pytest. Dynamic parameter values remain unresolved rather than being evaluated for a cosmetically larger count.

A verified status requires exact Rust function references, assertion correspondence, `coverage_scope: portable_obligations`, a passing result and its focused command. The checker rejects missing targets, stale summary/source evidence, duplicate source IDs and unsupported statuses. It checks recorded evidence structure; it does not rerun Rust tests or independently establish that a recorded pass is truthful.

Run executable translations only against an explicitly disposable v2 PostgreSQL database:

```sh
AQ_GRAPH_TEST_DATABASE_URL=postgres://USER@127.0.0.1/aq_graph_test \
  cargo test --locked --test v1_graph_parity --test v1_knowledge_parity
```

A missing database URL is a failure, not a skip. Keep initial red-run evidence and final focused test results with the implementation review. Never change the v1 source or operator database to make a translation pass. No live AQ cutover/import is authorized by this inventory.

## Schema-6 revision contract

Tasks and knowledge now share logical record snapshots that include lifecycle and relationship membership, using the existing `content_revision` field as the common revision identifier. All outgoing edges and incoming structural edges are captured; structural changes version both endpoints, while informational changes version only their source. Exact reads expose legacy relationship unavailability. The common restore operation restores content/relationships and preserves current lifecycle by default; `include_lifecycle` deliberately restores historical lifecycle/tombstone state for either kind. Current kind-specific validation, authorization and knowledge-verification reset still apply. Migration 0006 adds an attributed current baseline without rewriting historical evidence. This supersedes the initial selective/content-only revision boundary, Current focused 9 common-revision scenarios pass; exact target assertions and verification commands are recorded per mapped source function. No prior whole-suite result is used as evidence for this update.
