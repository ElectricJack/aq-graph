//! Source-backed core graph parity. Each test names its v1 antecedent.
//! These tests intentionally expose current gaps; no v1 Python/runtime is imported.
use aq_graph::{service, store::Store};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Client {
    app: Router,
    workspace: Uuid,
    token: String,
}
impl Client {
    async fn new() -> Self {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("set AQ_GRAPH_TEST_DATABASE_URL to disposable PostgreSQL");
        let store = Store::connect(&url, 8).await.unwrap();
        store.migrate().await.unwrap();
        let workspace = Uuid::new_v4();
        let token = format!("v1-parity-{}", Uuid::new_v4());
        store.bootstrap(workspace, "parity", &token).await.unwrap();
        Self {
            app: service::router(store),
            workspace,
            token,
        }
    }
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(format!("/v1/workspaces/{}{path}", self.workspace))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", self.token))
            .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
            .unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), service::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn mutation(&self, ops: Vec<Value>) -> (StatusCode, Value) {
        self.request(
            "POST",
            "/mutations",
            Some(json!({"idempotency_key":Uuid::new_v4().to_string(),"operations":ops})),
        )
        .await
    }
    async fn success(&self, ops: Vec<Value>) -> Value {
        let (status, result) = self.mutation(ops).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        result
    }
    async fn node(&self, kind: &str, state: &str) -> Uuid {
        let id = Uuid::new_v4();
        let mut op = json!({"op":"create_node","id":id,"kind":kind,"title":format!("{kind} {id}")});
        if kind == "task" {
            op["task_state"] = json!(state);
        }
        self.success(vec![op]).await;
        id
    }
    async fn context(&self, id: Uuid) -> Value {
        let (status, v) = self
            .request(
                "GET",
                &format!("/nodes/{id}/context?depth=1&limit=100"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        v
    }
    async fn show(&self, id: Uuid) -> Value {
        let (status, v) = self.request("GET", &format!("/nodes/{id}"), None).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        v
    }
    async fn parents(&self, child: Uuid) -> Vec<Uuid> {
        self.context(child).await["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["edge_type"] == "parent-child" && e["target"] == json!(child))
            .map(|e| serde_json::from_value(e["source"].clone()).unwrap())
            .collect()
    }
}
fn edge(source: Uuid, target: Uuid, kind: &str) -> Value {
    json!({"op":"add_edge","source":source,"target":target,"edge_type":kind})
}
fn set_parent(child: Uuid, version: i64, parent: Option<Uuid>) -> Value {
    json!({"op":"set_parent","id":child,"expected_version":version,"parent_id":parent})
}
fn patch(id: Uuid, version: i64, patch: Value) -> Value {
    json!({"op":"update_node","id":id,"expected_version":version,"patch":patch})
}
fn rejected(status: StatusCode, value: &Value) {
    assert!(
        matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT | StatusCode::NOT_FOUND
        ),
        "expected a domain refusal; received {status}: {value}"
    );
    assert!(value["error"]["code"].is_string(), "{value}");
}

// v1 tests/test_record_links.py::test_matrix_self_duplicate_pin_and_atomic_batch;
// endpoint matrix source: src/records/service.py::RecordService._plan_links.
#[tokio::test]
async fn produces_and_motivated_by_require_task_to_knowledge() {
    let c = Client::new().await;
    let task = c.node("task", "active").await;
    let a = c.node("knowledge", "").await;
    let b = c.node("knowledge", "").await;
    for kind in ["produces", "motivated_by"] {
        c.success(vec![edge(task, a, kind)]).await;
        let (s, v) = c.mutation(vec![edge(a, b, kind)]).await;
        assert_eq!(
            s,
            StatusCode::BAD_REQUEST,
            "{kind} must reject knowledge→knowledge: {v}"
        );
        assert!(
            c.context(b).await["edges"].as_array().unwrap().is_empty(),
            "refused edge must leave no state"
        );
    }
}

// v1 tests/test_record_links.py::test_matrix_self_duplicate_pin_and_atomic_batch;
// endpoint matrix source: src/records/service.py::RecordService._plan_links.
#[tokio::test]
async fn evidence_relationships_require_two_knowledge_endpoints() {
    let c = Client::new().await;
    let task = c.node("task", "active").await;
    let a = c.node("knowledge", "").await;
    let b = c.node("knowledge", "").await;
    for kind in ["supports", "contradicts", "supersedes"] {
        c.success(vec![edge(a, b, kind)]).await;
        let (s, v) = c.mutation(vec![edge(task, b, kind)]).await;
        assert_eq!(
            s,
            StatusCode::BAD_REQUEST,
            "{kind} must reject task→knowledge: {v}"
        );
    }
}

// v1 tests/test_record_links.py::test_execution_edges_refused_before_any_write.
// v2 unifies mutation transport, but knowledge must still never be an execution endpoint.
#[tokio::test]
async fn structural_edges_refuse_knowledge_endpoints_before_writes() {
    let c = Client::new().await;
    let a = c.node("knowledge", "").await;
    let b = c.node("knowledge", "").await;
    for kind in ["blocks", "parent-child"] {
        let (s, v) = c.mutation(vec![edge(a, b, kind)]).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    }
    assert!(c.context(a).await["edges"].as_array().unwrap().is_empty());
}

// v1 tests/test_record_links.py::test_matrix_self_duplicate_pin_and_atomic_batch.
#[tokio::test]
async fn invalid_informational_batch_rolls_back_and_does_not_consume_identity() {
    let c = Client::new().await;
    let a = c.node("knowledge", "").await;
    let b = c.node("knowledge", "").await;
    for ops in [
        vec![edge(a, a, "references")],
        vec![edge(a, b, "references"), edge(a, b, "references")],
        vec![
            json!({"op":"add_edge","source":a,"target":b,"edge_type":"references","target_revision":999}),
        ],
    ] {
        let (s, v) = c.mutation(ops).await;
        rejected(s, &v);
        assert!(c.context(a).await["edges"].as_array().unwrap().is_empty());
    }
    c.success(vec![edge(a, b, "supports")]).await;
}

// v1 tests/test_record_links.py::test_task_link_token_and_replay_do_not_touch_execution_or_target;
// v1 tests/test_record_graph.py::test_execution_and_record_links_are_distinct_and_progress_unchanged.
#[tokio::test]
async fn informational_links_preserve_task_state_content_and_intrinsic_readiness() {
    let c = Client::new().await;
    let parent = c.node("task", "active").await;
    let child = c.node("task", "active").await;
    let finding = c.node("knowledge", "").await;
    c.success(vec![edge(parent, child, "parent-child")]).await;
    let before = c.show(child).await;
    let target_before = c.show(finding).await;
    let (_, ready_before) = c.request("GET", "/ready", None).await;
    let request = json!({"idempotency_key":"informational-replay","operations":[edge(child,finding,"produces")]});
    let (s, result) = c.request("POST", "/mutations", Some(request.clone())).await;
    assert_eq!(s, StatusCode::OK, "{result}");
    let (s, replay) = c.request("POST", "/mutations", Some(request)).await;
    assert_eq!(s, StatusCode::OK, "{replay}");
    assert_eq!(replay, result);
    assert_eq!(c.show(child).await, before);
    assert_eq!(c.show(finding).await, target_before);
    let (_, ready_after) = c.request("GET", "/ready", None).await;
    assert_eq!(ready_after, ready_before);
    let context = c.context(child).await;
    let kinds: Vec<_> = context["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["edge_type"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"produces") && kinds.contains(&"parent-child"));
}

// v1 tests/test_record_links.py::test_concurrent_duplicate_has_one_winner.
#[tokio::test]
async fn concurrent_duplicate_informational_edges_have_one_winner() {
    let c = Client::new().await;
    let a = c.node("knowledge", "").await;
    let b = c.node("knowledge", "").await;
    let (x, y) = tokio::join!(
        c.mutation(vec![edge(a, b, "references")]),
        c.mutation(vec![edge(a, b, "references")])
    );
    assert_eq!(
        [x.0, y.0].iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "{x:?} {y:?}"
    );
    assert_eq!(
        [x.0, y.0]
            .iter()
            .filter(|s| **s == StatusCode::CONFLICT)
            .count(),
        1,
        "{x:?} {y:?}"
    );
    assert_eq!(c.context(a).await["edges"].as_array().unwrap().len(), 1);
}

// v1 tests/test_record_graph.py::test_graph_omits_cross_project_endpoint_and_does_not_leak_counts.
// Adaptation: v2 forbids cross-workspace links instead of allowing a privileged import then redacting.
#[tokio::test]
async fn cross_workspace_link_refusal_never_discloses_private_context() {
    let visible = Client::new().await;
    let private = Client::new().await;
    let task = visible.node("task", "active").await;
    let evidence = private.node("knowledge", "").await;
    let (s, v) = visible
        .mutation(vec![edge(task, evidence, "references")])
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    let context = visible.context(task).await;
    assert!(context["edges"].as_array().unwrap().is_empty());
    assert!(!context.to_string().contains(&evidence.to_string()));
    assert!(!context["truncated"].as_bool().unwrap());
}

// v1 tests/test_hierarchy_queries.py::TestSetParent.test_reparent_replaces_the_single_edge.
// Raw relationship operations must not bypass the single-parent invariant.
#[tokio::test]
async fn child_cannot_have_two_live_parents() {
    let c = Client::new().await;
    let a = c.node("task", "active").await;
    let b = c.node("task", "active").await;
    let child = c.node("task", "active").await;
    c.success(vec![edge(a, child, "parent-child")]).await;
    let (s, v) = c.mutation(vec![edge(b, child, "parent-child")]).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "second live parent must be refused: {v}"
    );
    assert_eq!(c.parents(child).await, vec![a]);
}

// v1 tests/test_hierarchy_queries.py::TestSetParent.test_writes_edge_and_pointer_together;
// ::TestSetParent.test_reparent_replaces_the_single_edge; ::TestSetParent.test_to_root_clears_both.
#[tokio::test]
async fn set_parent_atomically_reparents_and_detaches() {
    let c = Client::new().await;
    let a = c.node("task", "active").await;
    let b = c.node("task", "active").await;
    let child = c.node("task", "active").await;
    c.success(vec![set_parent(child, 1, Some(a))]).await;
    assert_eq!(c.parents(child).await, vec![a]);
    let version = c.show(child).await["version"].as_i64().unwrap();
    c.success(vec![set_parent(child, version, Some(b))]).await;
    assert_eq!(c.parents(child).await, vec![b]);
    let version = c.show(child).await["version"].as_i64().unwrap();
    c.success(vec![set_parent(child, version, None)]).await;
    assert!(c.parents(child).await.is_empty());
}

// v1 tests/test_hierarchy_queries.py::TestSetParent.test_rejections (self, cycle, missing, depth);
// ::TestSetParentBulk.test_depth_cap_enforced. Structural root depth is one; cap is three.
#[tokio::test]
async fn parent_depth_cap_is_three_and_rejected_move_preserves_the_forest() {
    let c = Client::new().await;
    let a = c.node("task", "active").await;
    let b = c.node("task", "active").await;
    let d = c.node("task", "active").await;
    let leaf = c.node("task", "active").await;
    c.success(vec![edge(a, b, "parent-child"), edge(b, d, "parent-child")])
        .await;
    let (s, v) = c.mutation(vec![edge(d, leaf, "parent-child")]).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "depth four must be refused: {v}"
    );
    assert!(c.parents(leaf).await.is_empty());
    for op in [
        edge(a, a, "parent-child"),
        edge(d, a, "parent-child"),
        edge(Uuid::new_v4(), leaf, "parent-child"),
    ] {
        let (s, v) = c.mutation(vec![op]).await;
        rejected(s, &v);
    }
}

// v1 tests/test_hierarchy_queries.py::test_set_parent_rejects_blocking_dependency_cycle_on_both_backends.
// waits-for is deliberately adapted to unconditional blocks; conditional/container waits remain AQ-owned.
#[tokio::test]
async fn structural_and_blocking_edges_cannot_jointly_close_a_cycle() {
    let c = Client::new().await;
    let parent = c.node("task", "active").await;
    let child = c.node("task", "active").await;
    c.success(vec![edge(child, parent, "blocks")]).await;
    let (s, v) = c.mutation(vec![edge(parent, child, "parent-child")]).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "mixed-family dependency cycle must be refused: {v}"
    );
    assert!(c.parents(child).await.is_empty());
}

// v1 tests/test_hierarchy_queries.py::TestSetParent.test_child_is_withheld_while_container_is_defined.
#[tokio::test]
async fn draft_parent_withholds_active_child_until_parent_activation() {
    let c = Client::new().await;
    let grandparent = c.node("task", "draft").await;
    let parent = c.node("task", "draft").await;
    let child = c.node("task", "active").await;
    c.success(vec![
        edge(grandparent, parent, "parent-child"),
        edge(parent, child, "parent-child"),
    ])
    .await;
    let (status, ready) = c.request("GET", "/ready", None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert!(
        ready["items"].as_array().unwrap().is_empty(),
        "draft parent must withhold active leaf: {ready}"
    );

    // Strengthen the v1 direct-parent assertion across the maximum supported
    // three-level hierarchy: activating the parent alone must not bypass its
    // still-draft ancestor, even though the leaf's immediate parent is active.
    c.success(vec![patch(parent, 1, json!({"task_state":"active"}))])
        .await;
    let (status, ready) = c.request("GET", "/ready", None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert!(
        ready["items"].as_array().unwrap().is_empty(),
        "draft grandparent must withhold the leaf beneath an active parent: {ready}"
    );

    c.success(vec![patch(grandparent, 1, json!({"task_state":"active"}))])
        .await;
    let (status, ready) = c.request("GET", "/ready", None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let ids: Vec<_> = ready["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].clone())
        .collect();
    assert_eq!(
        ids,
        vec![json!(child)],
        "only the active leaf is executable; both active containers stay excluded: {ready}"
    );
}

// v1 tests/test_hierarchy_queries.py::TestSetParent.test_completed_container_refuses_children;
// ::TestSetParentBulk.test_completed_parent_rejected.
#[tokio::test]
async fn completed_parent_refuses_new_children() {
    let c = Client::new().await;
    let parent = c.node("task", "completed").await;
    let child = c.node("task", "active").await;
    let (s, v) = c.mutation(vec![edge(parent, child, "parent-child")]).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "completed parent must be closed: {v}"
    );
    assert!(c.parents(child).await.is_empty());
}

// v1 tests/test_hierarchy_queries.py::TestOpenChildrenGuard.test_transition_to_completed_refused_with_open_child.
#[tokio::test]
async fn open_children_prevent_normal_parent_completion() {
    let c = Client::new().await;
    let parent = c.node("task", "active").await;
    let child = c.node("task", "active").await;
    c.success(vec![edge(parent, child, "parent-child")]).await;
    let (s, v) = c
        .mutation(vec![patch(parent, 1, json!({"task_state":"completed"}))])
        .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "open children must block normal completion: {v}"
    );
    assert_eq!(c.show(parent).await["task_state"], "active");
}

// v1 tests/test_hierarchy_queries.py::TestOpenChildrenGuard.test_terminal_children_do_not_block.
// v1 FAILED is adapted only here to v2 canceled (terminal, not successful prerequisite).
#[tokio::test]
async fn completed_and_canceled_children_allow_parent_completion() {
    let c = Client::new().await;
    let parent = c.node("task", "active").await;
    let a = c.node("task", "active").await;
    let b = c.node("task", "active").await;
    c.success(vec![
        edge(parent, a, "parent-child"),
        edge(parent, b, "parent-child"),
        patch(a, 1, json!({"task_state":"completed"})),
        patch(b, 1, json!({"task_state":"canceled"})),
    ])
    .await;
    c.success(vec![patch(parent, 1, json!({"task_state":"completed"}))])
        .await;
    assert_eq!(c.show(parent).await["task_state"], "completed");
}

// v1 tests/test_hierarchy_queries.py::TestReads.test_tree_shape_and_depth_bound;
// ::TestStructureReads.test_depth_height_and_subtree_on_a_three_level_chain.
// New JSON route specified in docs/compatibility/graph.md, no Rust type dependency.
#[tokio::test]
async fn tree_query_returns_structural_children_with_a_depth_bound() {
    let c = Client::new().await;
    let a = c.node("task", "active").await;
    let b = c.node("task", "active").await;
    let d = c.node("task", "active").await;
    let note = c.node("knowledge", "").await;
    c.success(vec![
        edge(a, b, "parent-child"),
        edge(b, d, "parent-child"),
        edge(a, note, "references"),
    ])
    .await;
    let (s, tree) = c
        .request("GET", &format!("/nodes/{a}/tree?depth=2&limit=50"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{tree}");
    assert_eq!(tree["node"]["id"], json!(a));
    assert_eq!(tree["children"].as_array().unwrap().len(), 1);
    assert_eq!(tree["children"][0]["node"]["id"], json!(b));
    assert_eq!(tree["children"][0]["children"][0]["node"]["id"], json!(d));
    let (s, tree) = c
        .request("GET", &format!("/nodes/{a}/tree?depth=1&limit=50"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{tree}");
    assert_eq!(tree["children"][0]["children"], json!([]));
}

// v1 tests/test_dependency_queries.py::TestAddDependencyIdempotent.test_second_add_is_noop_and_does_not_raise.
#[tokio::test]
async fn repeated_structural_dependency_add_is_a_noop_even_with_a_new_request_key() {
    let c = Client::new().await;
    let prerequisite = c.node("task", "active").await;
    let dependent = c.node("task", "active").await;
    let first = c
        .success(vec![edge(prerequisite, dependent, "blocks")])
        .await;
    let (s, second) = c
        .mutation(vec![edge(prerequisite, dependent, "blocks")])
        .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "v1 dependency addition is idempotent by endpoints/type: {second}"
    );
    assert_eq!(first["results"][0]["id"], second["results"][0]["id"]);
    assert_eq!(second["results"][0]["version"], 1);
    assert_eq!(
        c.context(dependent).await["edges"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

// v1 tests/test_dependency_queries.py::TestAddDependencyIdempotent.test_description_is_persisted_on_the_edge.
#[tokio::test]
async fn relationship_description_round_trips_through_context() {
    let c = Client::new().await;
    let prerequisite = c.node("task", "active").await;
    let dependent = c.node("task", "active").await;
    let mut op = edge(prerequisite, dependent, "blocks");
    op["description"] = json!("Needs the authentication compatibility fix");
    c.success(vec![op]).await;
    assert_eq!(
        c.context(dependent).await["edges"][0]["description"],
        "Needs the authentication compatibility fix"
    );
}

// v1 tests/test_record_links.py::test_exact_link_history_pins_floating_and_tombstones;
// v1 tests/test_record_graph.py::test_pinned_graph_edges_and_historical_removal_are_exact.
// v2 exposes immutable incident edge history instead of promising whole-graph time travel.
#[tokio::test]
async fn pinned_and_floating_links_keep_exact_revision_and_removal_history() {
    let c = Client::new().await;
    let a = c.node("knowledge", "").await;
    let b = c.node("knowledge", "").await;
    let added=c.success(vec![json!({"op":"add_edge","source":a,"target":b,"edge_type":"references","target_revision":1}),edge(a,b,"references")]).await;
    let pinned = added["results"][0].clone();
    c.success(vec![patch(b, 1, json!({"body":"new target bytes"}))])
        .await;
    let context = c.context(a).await;
    assert!(
        context["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == json!(b) && n["body"] == "new target bytes")
    );
    assert!(
        context["pinned_revisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == json!(b) && n["content_revision"] == 1 && n["body"] == "")
    );
    assert!(
        context["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["target_revision"].is_null())
    );
    c.success(vec![
        json!({"op":"remove_edge","id":pinned["id"],"expected_version":1}),
    ])
    .await;
    assert_eq!(c.context(a).await["edges"].as_array().unwrap().len(), 1);
    let (s, history) = c
        .request("GET", &format!("/nodes/{a}/history?limit=100"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{history}");
    let exact: Vec<_> = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["category"] == "edge" && h["data"]["id"] == pinned["id"])
        .collect();
    assert_eq!(exact.len(), 2);
    assert_eq!(exact[0]["operation"], "added");
    assert_eq!(exact[1]["operation"], "removed");
    assert!(exact.iter().all(|h| h["data"]["target_revision"] == 1));
    assert_eq!(exact[1]["data"]["deleted"], true);
}

// v1 tests/test_hierarchy_queries.py::test_set_parent_cycle_check_follows_long_blocking_chains.
// Its 300-edge regression must not be truncated to structural_depth*64 (192).
#[tokio::test]
async fn mixed_cycle_check_follows_a_three_hundred_edge_blocking_chain() {
    let c = Client::new().await;
    let root = Uuid::new_v4();
    let chain: Vec<_> = (0..300).map(|_| Uuid::new_v4()).collect();
    let ids: Vec<_> = std::iter::once(root).chain(chain.iter().copied()).collect();
    for chunk in ids.chunks(100) {
        c.success(chunk.iter().map(|id|json!({"op":"create_node","id":id,"kind":"task","title":"blocking chain","task_state":"active"})).collect()).await;
    }
    let mut predecessor = root;
    let mut links = vec![];
    for id in &chain {
        links.push(edge(predecessor, *id, "blocks"));
        predecessor = *id;
    }
    for chunk in links.chunks(100) {
        c.success(chunk.to_vec()).await;
    }
    let (s, v) = c
        .mutation(vec![edge(*chain.last().unwrap(), root, "parent-child")])
        .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "300-hop mixed cycle must be refused: {v}"
    );
    assert!(c.parents(root).await.is_empty());
}

// v1 tests/test_hierarchy_queries.py::TestSetParent.test_rejections.
// Transactional extension: a failed replacement must retain its original membership and token.
#[tokio::test]
async fn refused_reparent_restores_original_edge_history_and_version() {
    let c = Client::new().await;
    let parent = c.node("task", "active").await;
    let closed = c.node("task", "completed").await;
    let child = c.node("task", "active").await;
    c.success(vec![set_parent(child, 1, Some(parent))]).await;
    let before = c.show(child).await;
    let version = before["version"].as_i64().unwrap();
    let (_, history_before) = c
        .request("GET", &format!("/nodes/{child}/history"), None)
        .await;
    for proposed in [Some(closed), Some(child), Some(Uuid::new_v4())] {
        let (s, v) = c.mutation(vec![set_parent(child, version, proposed)]).await;
        rejected(s, &v);
        assert_eq!(c.parents(child).await, vec![parent]);
        assert_eq!(c.show(child).await, before);
        let (_, history_after) = c
            .request("GET", &format!("/nodes/{child}/history"), None)
            .await;
        assert_eq!(history_after, history_before);
    }
}

// v1 tests/test_hierarchy_queries.py::TestOpenChildrenGuard.test_transition_to_completed_refused_with_open_child.
// Concurrency extension to the same invariant: a concurrent insert cannot pass between check and commit.
#[tokio::test]
async fn concurrent_parent_completion_and_new_child_cannot_both_commit() {
    let c = Client::new().await;
    for _ in 0..5 {
        let parent = c.node("task", "active").await;
        let child = c.node("task", "active").await;
        let (completion, membership) = tokio::join!(
            c.mutation(vec![patch(parent, 1, json!({"task_state":"completed"}))]),
            c.mutation(vec![edge(parent, child, "parent-child")])
        );
        assert!(
            completion.0 == StatusCode::OK || membership.0 == StatusCode::OK,
            "{completion:?} {membership:?}"
        );
        assert!(
            !(completion.0 == StatusCode::OK && membership.0 == StatusCode::OK),
            "open child committed underneath completed parent"
        );
        let node = c.show(parent).await;
        let context = c.context(parent).await;
        assert!(
            !(node["task_state"] == "completed"
                && context["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["edge_type"] == "parent-child" && e["target"] == json!(child)))
        );
    }
}
