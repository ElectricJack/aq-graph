# Core graph parity with AQ v1

Source checkout: `/home/jkern/dev/agent-queue2`, commit
`9a9b8e8ef992d1ff699755a9383ed3c91e2844ea`. The source repository was read only;
no daemon, operator database, migration, or Python test suite was run.

`tests/v1_graph_parity.rs` exercises the published aq-graph JSON API against a
real disposable PostgreSQL database. It imports no v1 application code. Initial
baseline execution recorded **7 passing and 13 failing tests** in 1.57 seconds
(`.local/v1-graph-red.txt`, untracked local evidence). Failures are assertions
about returned behavior, rather than missing Rust methods or ignored tests.

After integrating the graph helpers and migrations, the expanded **22-test suite
passes** against fresh `aq_graph_v1_graph_green_0004` in 2.44 seconds
(`.local/v1-graph-green.txt`). The two later tests extend the same v1 invariants
with atomic failed-reparent and concurrent completion/child-insertion checks.
Independent migration audits passed seven isolated-schema cases: valid graph,
multi-parent refusal, fourth-level refusal, invalid endpoint refusal, mixed cycle
refusal, valid 300-hop chain, and 300-hop mixed-cycle refusal.

## Identity, direction, and lifecycle adaptations

V1 project scope becomes an authenticated v2 workspace. Qualified task/knowledge
identities become opaque node UUIDs. V1 dependency storage `(dependent,
prerequisite)` becomes the v2 directed edge `prerequisite blocks dependent`;
`set_parent(child,parent)` becomes `parent parent-child child`. No cross-workspace
edge is imported: requests fail without disclosing the foreign endpoint.

The tests map DEFINED to draft, READY/IN_PROGRESS to active, COMPLETED to
completed, and FAILED to canceled **only for terminal-child completion checks**.
Canceled prerequisites remain unresolved; this is not a general import mapping
from failed execution attempts. Worker claims, route availability, Git delivery,
waits-for container conditions, sessions, and resource release remain AQ-owned.
The former tests' private Python locks, derived parent pointer, and child-name
allocation are not API requirements. The durable parent edge is authoritative.

V1 source-link revisions/link tokens become separate versioned edge records and
actor/workspace/operation idempotency receipts. Informational additions leave
node content and lifecycle unchanged. Exact pinned payloads and immutable edge
addition/removal history are asserted. Restoring historical link sets as part of
a knowledge content restore, source-level link tokens, and redaction are distinct
remaining contracts; the current graph tests do not claim those behaviors.

## Translated assertions

Source names below are relative to `tests/` in the pinned v1 checkout. Rust names
are in `tests/v1_graph_parity.rs`. A source test can contribute several assertions;
a selected assertion translation does not claim its entire original workflow.

| V1 source test | Rust test | Assertion scope |
| --- | --- | --- |
| `test_record_links.py::test_matrix_self_duplicate_pin_and_atomic_batch` | `produces_and_motivated_by_require_task_to_knowledge` | Valid Task→Knowledge and invalid Knowledge→Knowledge; endpoint matrix additionally verified against `RecordService._plan_links` |
| `test_record_links.py::test_matrix_self_duplicate_pin_and_atomic_batch` | `evidence_relationships_require_two_knowledge_endpoints` | supports/contradicts/supersedes require Knowledge→Knowledge; helper-source matrix extension |
| `test_record_links.py::test_execution_edges_refused_before_any_write` | `structural_edges_refuse_knowledge_endpoints_before_writes` | blocks/parent-child cannot use knowledge; separate informational transport adapted to unified mutation API |
| `test_record_links.py::test_matrix_self_duplicate_pin_and_atomic_batch` | `invalid_informational_batch_rolls_back_and_does_not_consume_identity` | Self, duplicate pair, unavailable pin, atomic rollback, subsequent valid operation |
| `test_record_links.py::test_task_link_token_and_replay_do_not_touch_execution_or_target`; `test_record_graph.py::test_execution_and_record_links_are_distinct_and_progress_unchanged` | `informational_links_preserve_task_state_content_and_intrinsic_readiness` | Replay identical result; source/target nodes and readiness unchanged; both relationship families visible |
| `test_record_links.py::test_concurrent_duplicate_has_one_winner` | `concurrent_duplicate_informational_edges_have_one_winner` | Two distinct requests, one relationship, one successful writer; HTTP conflict replaces v1 revision-conflict code |
| `test_record_graph.py::test_graph_omits_cross_project_endpoint_and_does_not_leak_counts` | `cross_workspace_link_refusal_never_discloses_private_context` | Stronger write refusal instead of privileged cross-project fixture import; no foreign ID/count signal |
| `test_hierarchy_queries.py::TestSetParent.test_reparent_replaces_the_single_edge` | `child_cannot_have_two_live_parents` | Raw edge API cannot bypass single-parent membership |
| `test_hierarchy_queries.py::TestSetParent.test_writes_edge_and_pointer_together`; `TestSetParent.test_reparent_replaces_the_single_edge`; `TestSetParent.test_to_root_clears_both` | `set_parent_atomically_reparents_and_detaches` | Atomic parent membership, replacement, and root detachment; no redundant parent pointer requirement |
| `test_hierarchy_queries.py::TestSetParent.test_rejections`; `TestSetParentBulk.test_depth_cap_enforced` | `parent_depth_cap_is_three_and_rejected_move_preserves_the_forest` | Three structural levels, root depth one; self/cycle/missing rejection; rejected move leaves membership unchanged |
| `test_hierarchy_queries.py::test_set_parent_rejects_blocking_dependency_cycle_on_both_backends` | `structural_and_blocking_edges_cannot_jointly_close_a_cycle` | Combined blocking/structural acyclicity; v1 waits-for adapted to unconditional blocks for this cycle assertion only |
| `test_hierarchy_queries.py::TestSetParent.test_child_is_withheld_while_container_is_defined` | `draft_parent_withholds_active_child_until_parent_activation` | Ancestor activation gates leaf readiness |
| `test_hierarchy_queries.py::TestSetParent.test_completed_container_refuses_children`; `TestSetParentBulk.test_completed_parent_rejected` | `completed_parent_refuses_new_children` | Terminal parent refuses ordinary new membership |
| `test_hierarchy_queries.py::TestOpenChildrenGuard.test_transition_to_completed_refused_with_open_child` | `open_children_prevent_normal_parent_completion` | Normal completion cannot strand an open child; failed mutation leaves lifecycle unchanged |
| `test_hierarchy_queries.py::TestOpenChildrenGuard.test_terminal_children_do_not_block` | `completed_and_canceled_children_allow_parent_completion` | Both terminal child states permit parent completion; no execution auto-settlement claim |
| `test_hierarchy_queries.py::TestReads.test_tree_shape_and_depth_bound`; `TestStructureReads.test_depth_height_and_subtree_on_a_three_level_chain` | `tree_query_returns_structural_children_with_a_depth_bound` | Recursive structural children and bounded depth; excludes informational edges; no separate depth/height scalar endpoint claim |
| `test_dependency_queries.py::TestAddDependencyIdempotent.test_second_add_is_noop_and_does_not_raise` | `repeated_structural_dependency_add_is_a_noop_even_with_a_new_request_key` | Repeated blocks insertion returns the existing identity/version with no duplicate relationship |
| `test_dependency_queries.py::TestAddDependencyIdempotent.test_description_is_persisted_on_the_edge` | `relationship_description_round_trips_through_context` | Description persists on supported blocks edge; v1 discovered-from is not silently remapped |
| `test_record_links.py::test_exact_link_history_pins_floating_and_tombstones`; `test_record_graph.py::test_pinned_graph_edges_and_historical_removal_are_exact` | `pinned_and_floating_links_keep_exact_revision_and_removal_history` | Floating current target, exact old pin, removal visibility, immutable addition/removal snapshots; excludes historical-link restore |
| `test_hierarchy_queries.py::test_set_parent_cycle_check_follows_long_blocking_chains` | `mixed_cycle_check_follows_a_three_hundred_edge_blocking_chain` | Combined reachability traverses 300 blocking hops; structural depth cap cannot truncate cycle detection |

Two additional transaction/concurrency extensions were added after the initial
20-test red capture:

| V1 source invariant | Rust test | Additional assertion |
| --- | --- | --- |
| `test_hierarchy_queries.py::TestSetParent.test_rejections` | `refused_reparent_restores_original_edge_history_and_version` | Refused closed/self/missing reparent restores original membership, full node, and history |
| `test_hierarchy_queries.py::TestOpenChildrenGuard.test_transition_to_completed_refused_with_open_child` | `concurrent_parent_completion_and_new_child_cannot_both_commit` | Five races enforce the same completion invariant at commit under concurrent child insertion |

## Endpoint matrix and hierarchy contract

| Relationship | Source kind | Target kind |
| --- | --- | --- |
| references | task or knowledge | task or knowledge |
| motivated_by, produces | task | knowledge |
| supports, contradicts, supersedes | knowledge | knowledge |
| blocks, parent-child | task | task |

Self links are invalid. Structural edges cannot pin content revisions. Each task
has at most one active structural parent. Structural depth is at most three
nodes, including the root. Moving a subtree must check parent depth plus subtree
height. Combined blocks/parent-child reachability is acyclic, without using the
small structural depth cap to bound blocking traversal. Terminal parents refuse
new children. Normal completion refuses nonterminal direct children.

This parity baseline tightens two explicit initial aq-graph choices: independent
acyclicity of each relationship family becomes combined acyclicity, and pure
organizational parenthood gains draft-ancestor withholding and container
exclusion from executable readiness. Those changes need corresponding public
API/spec updates; they must not be described as preexisting v2 behavior.

Informational duplicate additions conflict. Blocks duplicate additions are
idempotent by endpoint/type independently of request keys, matching v1's
dependency writer. Repeated same-key operations still replay committed receipts.

## JSON additions used by the tests

`set_parent` is a mutation operation:

```json
{"op":"set_parent","id":"CHILD_UUID","expected_version":1,"parent_id":"PARENT_UUID"}
```

`parent_id:null` detaches to the root. An unchanged parent is a no-op. A changed
membership advances the child's version, without creating a duplicate content
revision, and records the edge changes and reparent lifecycle event in the same
change set. A failed move rolls back all three. The parent setter must run with
the same serializable invariant checks as raw structural edge changes.

`GET /nodes/{id}/tree?depth=2&limit=50&max_bytes=262144` returns recursively:

```json
{"node":{},"children":[{"node":{},"children":[],"truncated":false}],"truncated":false}
```

Depth counts edges from the root. Limit counts the root. Only live structural
membership appears; informational relationships do not consume child positions.
Record and byte omissions set root truncation. A depth boundary with omitted structural descendants also sets truncation. The response has one consistent database
snapshot and the same workspace/authority checks as other record reads.

Edge mutation/response adds an optional `description` string of at most 4096
bytes, separate from node body and graph readiness. Its value belongs in immutable
edge history as well as current edge responses.

## Storage integration requirements

The durable [`tests/migration_contracts.rs`](../../tests/migration_contracts.rs)
suite automates the seven graph preflight fixtures and three knowledge upgrade
contracts (10 passing tests). It builds schema v3 in a unique transactional schema,
applies the actual migration SQL, checks refusal messages and rollback of DDL and
backfills, and verifies schema cleanup. It additionally checks original-author
recovery, the unknown-author fallback, immutable history/receipt payloads, and
refusal of unattributed verification or unexplained retirement. These are upgrade
regressions, not additional claims of translated v1 behavior.

Migration `0004_graph_contract.sql` adds the description column and active-parent
unique index. It explicitly refuses invalid active endpoint kinds, existing multi-parent or over-depth/cyclic
structural data, and mixed blocking/structural cycles, rather than choosing a parent or dropping history. The mixed-cycle audit uses a temporary indegree/frontier graph, visits each vertex and arc once,
and obeys the connection statement deadline; it never materializes transitive closure. A migration
failure requires an explicit reconciliation plan. The initial red-test database
contains deliberately invalid parentage and must not be reused as a clean green
fixture.

`src/store/compat_graph.rs` implements endpoint validation, combined cycle checks,
depth/subtree validation, atomic parent mutation, completion validation, and tree
retrieval. Main-store integration must call those helpers from live mutation,
draft validation, and draft apply paths. Completion transitions must use
serializable transactions too: a read-committed no-children check cannot protect
against a concurrently committed child insertion. Node state and edge history
must commit atomically. Containers are withheld from `/ready`; draft ancestors
withhold descendants. These scope-anchored predicates augment the existing
blocking-prerequisite check:

```sql
AND NOT EXISTS (
    SELECT 1 FROM edges pc
    JOIN nodes child ON child.id = pc.target AND child.workspace_id = pc.workspace_id
    WHERE pc.workspace_id = n.workspace_id AND pc.source = n.id
      AND pc.edge_type = 'parent-child' AND NOT pc.deleted AND NOT child.deleted
)
AND NOT EXISTS (
    WITH RECURSIVE ancestor(id) AS (
        SELECT source FROM edges
        WHERE workspace_id = n.workspace_id AND target = n.id
          AND edge_type = 'parent-child' AND NOT deleted
        UNION
        SELECT e.source FROM edges e JOIN ancestor a ON e.target = a.id
        WHERE e.workspace_id = n.workspace_id
          AND e.edge_type = 'parent-child' AND NOT e.deleted
    )
    SELECT 1 FROM ancestor a JOIN nodes parent ON parent.id = a.id
    WHERE parent.deleted OR parent.task_state = 'draft'
)
```

Not translated in this graph pass: source worker-held-task grants, feature flags,
route/claim eligibility, internal array-bind instrumentation, cached pointer
implementation, task name allocation, container metadata flags, execution waves,
waits-for observer/container semantics, discovered-from and other legacy edge
kinds, lifecycle auto-settlement, administrative force completion, workspace locks,
agent pointers, live-session abandonment, global knowledge scope, historical link
restoration, and redaction. Their source definitions remain inventory entries;
passing the translated assertions does not establish full AQ parity.
