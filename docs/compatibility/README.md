# V1 behavioral compatibility inventory

This inventory translates evidence from Agent Queue v1 into aq-graph's standalone contract. It does not certify full parity. The source is pinned to **`agent-queue2` commit `9a9b8e8ef992d1ff699755a9383ed3c91e2844ea`**. Inspection used immutable Git blobs and Python AST parsing only: no v1 imports, test collection, test execution, daemon commands, database access or source changes.

The machine-readable [inventory](v1-test-inventory.json) covers direct `tests/test_record*.py`, `test_knowledge*.py`, `test_hierarchy*.py`, `test_dependency_queries.py`, and `test_task_graph*.py`, plus `tests/perf/test_hierarchy_statements.py`. Other v1 test families are outside this specific inventory and remain unassessed. This is a source-function map, not a guessed list of thousands of pytest IDs.

## Counts and scope

The pinned selection contains **48 files and 793 test definitions**. There are 101 parameterized definitions. Static AST inspection resolves decorator combination counts for 784 definitions (sum 1,147 combinations), while nine have dynamic values. Fixture parameterization, generated fixture manifests, plugin collection, skips and indirect expansion are not executed. **Neither 793 nor 1,147 is the number of collected or passing pytest cases.**

| Category | Definitions | Initial disposition |
| --- | ---: | --- |
| Graph core | 66 | Portable graph invariants and retrieval; exact assertion translation required |
| Knowledge core | 96 | Content/history/protection/evidence/proposal invariants; exact translation required |
| AQ integration | 264 | Command registration, YAML/spec graph parsing, harness/prompt delivery, dashboard and vault adapters |
| AQ orchestration | 110 | Session/claim/routing policy, resource release, automatic container settlement and LLM budgets |
| Migration | 115 | Legacy aliases, inventories, cutover, export evidence, erasure policy and v1 schema tooling |
| Optional integration | 21 | Capture/export/outbox consumers absent from the synchronous baseline |
| Optional providers | 119 | Semantic indexing, legacy memory adapters, ranking, paid generation and provider budgets |
| Performance | 2 | V1 SQLAlchemy statement-count budgets require separate v2 plan/budget tests |

Initial triage recorded 162 core definitions needing translation. After inspecting exact portable obligations and green focused runs, **8 source definitions are translated, 154 remain open (including partial coverage), and 631 are explicitly deferred**. Deferral records the current component boundary and a concrete reason; it does not declare those behaviors unnecessary or implemented. Broader AQ parity requires separate adapters/features and an explicit scope decision. Selected portable assertions from an otherwise deferred subsystem may be brought into graph-core scope; such reclassification must retain the original source identity and document omitted obligations.

The priority counts are P0: 73, P1: 89, P2: 117, P3: 514. P0 targets authorization, protected knowledge, provenance, revisions/CAS/replay, relationship validity and atomicity. P1 covers remaining graph reads, hierarchy and portable command behavior. P2 covers migration and performance. P3 records AQ integration/orchestration and optional systems. These are translation priorities rather than severity scores or proof of coverage.

## What the map proves

Each row records its exact source path and class-qualified function name, pinned line range and function SHA-256, a small sample of actual assertions, called operations, parameter decorators, category, priority, disposition, reason and exact Rust target functions where available. Source files have independent SHA-256 hashes. The assertion/call samples are navigation aids, not substitutes for inspecting the complete pinned function and fixtures.

The allowed dispositions are:

| Status | Meaning |
| --- | --- |
| `needs_translation` | Untranslated, failed, not yet run, or only selected source assertions covered. An existing test with a similar name is insufficient. |
| `existing_coverage` | An existing v2 test has been inspected against every portable obligation of this source function and passed a recorded focused command. |
| `translated` | A source-backed v2 translation covers every portable obligation and passed a recorded focused command. Identity/wire adaptations and excluded AQ-only mechanics must be explicit. |
| `deferred` | A concrete component/scope boundary or separate feature is recorded. This is not parity credit. |

`candidate_v2_tests` identifies authored or potentially reusable tests without upgrading coverage claims. A source function can contribute assertions to several Rust tests; a Rust test can exercise assertions from several source functions. These many-to-many links must not inflate the number of fully translated source cases.

`coverage_scope: selected_assertions` keeps the source row at `needs_translation` even after those assertions pass. For example, the v1 snapshot-limit function has fifteen decorator cases: verification and typed-provenance translations do not also prove its metadata, body, summary, time-window and tag limits. An HTTP invariant test also does not prove v1 direct-SQL trigger protections against database writers. A knowledge correction test that resets verification does not prove AQ review/authority-grant revocation.

## Executable translations and exposed gaps

The [graph translation](graph.md) maps 22 Rust scenarios in [v1_graph_parity.rs](../../tests/v1_graph_parity.rs). Its initial baseline run was **7 passing and 13 failing**; every new scenario ran. Initial red-run evidence is retained; the final graph run passed **22/22 scenarios**, including two later invariant extensions. These are selected scenario counts, not 22 fully translated source functions. The failing assertions identified endpoint-kind validation, single structural parent, bounded hierarchy depth/tree retrieval, mixed dependency/parent cycles, ancestor activation/completion guards, duplicate structural insertion and edge descriptions. Passing selected assertions cover parts of graph atomicity, scope isolation, informational separation and exact pins. They do not certify the whole corresponding v1 source function.

The [knowledge translation](knowledge.md) maps 11 Rust scenarios in [v1_knowledge_parity.rs](../../tests/v1_knowledge_parity.rs). Initial translation targets include author ownership, protected edit/reset, privileged evidence-backed verification/dispute, typed provenance, reasoned retirement/exact restoration, proposal decision authority and live authorization before replay. The initial nine-scenario baseline was **1 passing and 8 failing** on real PostgreSQL. The final **11/11 scenarios pass**, including source-link authorization and privileged-replay extensions. Runtime results and exact source correspondences are recorded per target; partial source functions remain open.

This pass deliberately surfaces stronger requirements rather than changing expected assertions to match missing behavior. The core contract was revised to satisfy the selected translated assertions; the full v1 inventory is not complete. In particular, parent membership/activation and completion guards are explicit compatibility additions; they were not all properties of the initial structural-only v2 baseline. Lifecycle mapping from v1 DEFINED/READY/IN_PROGRESS/FAILED to v2 draft/active/completed/canceled is limited to documented assertions and must not silently reinterpret real execution outcomes during migration.

Still-open core obligations include canonical content digests/ordering and temporal knowledge fields, complete snapshot limits, filter-bound/ranked search, exact global-sharing/source revocation, review-bound authority, richer proposal decisions/rejection, derived-summary freshness, retirement successor consistency, direct-SQL append-only protection, historical link restoration and full source-function fixture arms. AQ sessions/claims, automatic settlement, unsupported dependency kinds, semantic/provider/generation systems and migration tooling retain explicit boundaries in their own rows.

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
