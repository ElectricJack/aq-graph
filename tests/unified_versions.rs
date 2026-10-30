//! Common public version-control contract for tasks and knowledge.
//! Real PostgreSQL is mandatory; no test silently skips database assertions.
use aq_graph::{service, store::Store};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Api {
    app: Router,
    workspace: Uuid,
    token: String,
}
impl Api {
    async fn new() -> Self {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL").expect(
            "unified version tests require AQ_GRAPH_TEST_DATABASE_URL for disposable PostgreSQL",
        );
        let store = Store::connect(&url, 4).await.expect("connect PostgreSQL");
        store.migrate().await.expect("migrate PostgreSQL");
        let workspace = Uuid::new_v4();
        let token = format!("versions-{}", Uuid::new_v4());
        store
            .bootstrap(workspace, "version-author", &token)
            .await
            .expect("bootstrap isolated workspace");
        Self {
            app: service::router(store),
            workspace,
            token,
        }
    }
    async fn request_as(
        &self,
        token: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(format!("/v1/workspaces/{}{path}", self.workspace))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), service::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON API response"),
        )
    }
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.request_as(&self.token, method, path, body).await
    }
    async fn get(&self, path: &str) -> Value {
        let (status, value) = self.request("GET", path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {value}");
        value
    }
    async fn mutation(&self, key: &str, operations: Vec<Value>) -> (StatusCode, Value) {
        self.request("POST","/mutations",Some(json!({"idempotency_key":key,"reason":"Unified version contract","operations":operations}))).await
    }
    async fn change(&self, operations: Vec<Value>) -> Value {
        let (status, value) = self.mutation(&Uuid::new_v4().to_string(), operations).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        value
    }
    async fn create(&self, kind: &str, title: &str) -> Value {
        let id = Uuid::new_v4();
        let mut op = json!({"op":"create_node","id":id,"kind":kind,"title":title,"body":"Original\r\nα text"});
        if kind == "task" {
            op["task_state"] = json!("active");
        } else {
            op["knowledge_type"] = json!("decision");
        }
        self.change(vec![op]).await;
        self.node(id).await
    }
    async fn node(&self, id: Uuid) -> Value {
        self.get(&format!("/nodes/{id}")).await
    }
    async fn snapshot(&self, id: Uuid, revision: i64) -> Value {
        self.get(&format!("/nodes/{id}/revisions/{revision}")).await
    }
    async fn history(&self, id: Uuid) -> Value {
        self.get(&format!("/nodes/{id}/history?limit=200")).await
    }
}
fn id(n: &Value) -> Uuid {
    Uuid::parse_str(n["id"].as_str().unwrap()).unwrap()
}
fn revision(n: &Value) -> i64 {
    n["content_revision"].as_i64().unwrap()
}
fn version(n: &Value) -> i64 {
    n["version"].as_i64().unwrap()
}
fn patch(n: &Value, patch: Value) -> Value {
    json!({"op":"update_node","id":n["id"],"expected_version":n["version"],"patch":patch})
}
fn restore(n: &Value, revision: i64, lifecycle: Option<bool>) -> Value {
    let mut op = json!({"op":"restore_node","id":n["id"],"expected_version":n["version"],"revision":revision});
    if let Some(flag) = lifecycle {
        op["include_lifecycle"] = json!(flag);
    }
    op
}
fn links(snapshot: &Value) -> &Vec<Value> {
    snapshot["links"]
        .as_array()
        .expect("new revisions retain exact owned relationships")
}
fn revision_count(history: &Value) -> usize {
    history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["category"] == "revision")
        .count()
}
fn assert_advanced(before: &Value, after: &Value) {
    assert!(
        version(after) > version(before),
        "authoritative mutation advances CAS token: {before} -> {after}"
    );
    assert!(
        revision(after) > revision(before),
        "every durable mutation creates a common revision: {before} -> {after}"
    );
}

#[tokio::test]
async fn task_and_knowledge_share_exact_relationship_versions_and_restore() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let initial = api.create(kind, "Original title").await;
        let source = id(&initial);
        let target = api.create("knowledge", "Pinned target").await;
        let first = api.snapshot(source, revision(&initial)).await;
        assert_eq!(
            first["node"], initial,
            "an exact revision must match the persisted current node at that same version"
        );
        assert_eq!(first["relationships_available"], true);
        assert!(links(&first).is_empty());
        api.change(vec![patch(
            &initial,
            json!({"title":"Edited title","body":"Edited content"}),
        )])
        .await;
        let edited = api.node(source).await;
        assert_advanced(&initial, &edited);
        let edge_id = Uuid::new_v4();
        api.change(vec![json!({"op":"add_edge","id":edge_id,"source":source,"target":target["id"],"edge_type":"references","target_revision":target["content_revision"],"description":"Retained relationship explanation"})]).await;
        let linked = api.node(source).await;
        assert_advanced(&edited, &linked);
        let retained = api.snapshot(source, revision(&linked)).await;
        assert_eq!(
            retained["node"], linked,
            "link checkpoints retain exact persisted node fields"
        );
        assert_eq!(links(&retained).len(), 1);
        let original_edge = links(&retained)[0].clone();
        assert_eq!(original_edge["id"], json!(edge_id));
        assert_eq!(original_edge["target_revision"], target["content_revision"]);
        assert_eq!(
            original_edge["description"],
            "Retained relationship explanation"
        );
        assert_eq!(
            api.snapshot(source, revision(&initial)).await,
            first,
            "old snapshots remain exact after content and link edits"
        );
        let diff = api
            .get(&format!(
                "/nodes/{source}/diff?from={}&to={}",
                revision(&edited),
                revision(&linked)
            ))
            .await;
        assert!(
            diff["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["field"] == "links"),
            "relationship changes belong to common diff: {diff}"
        );
        let removed=api.change(vec![json!({"op":"remove_edge","id":edge_id,"expected_version":original_edge["version"]})]).await;
        let removed_edge = removed["results"][0].clone();
        let unlinked = api.node(source).await;
        assert_advanced(&linked, &unlinked);
        assert!(links(&api.snapshot(source, revision(&unlinked)).await).is_empty());
        let operation = restore(&unlinked, revision(&linked), None);
        let (status, receipt) = api
            .mutation("restore-original-link", vec![operation.clone()])
            .await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
        let restored = api.node(source).await;
        assert_advanced(&unlinked, &restored);
        let current = api.snapshot(source, revision(&restored)).await;
        assert_eq!(links(&current).len(), 1);
        let edge = &links(&current)[0];
        for field in [
            "id",
            "source",
            "target",
            "edge_type",
            "source_revision",
            "target_revision",
            "description",
            "created_at",
        ] {
            assert_eq!(
                edge[field], original_edge[field],
                "restore preserves edge identity/content field {field}"
            );
        }
        assert!(
            edge["version"].as_i64().unwrap() > removed_edge["version"].as_i64().unwrap(),
            "edge versions never rewind on restoration"
        );
        assert_eq!(edge["deleted"], false);
        let history = api.history(source).await;
        assert_eq!(
            revision_count(&history),
            5,
            "create/edit/add/remove/restore each creates one snapshot: {history}"
        );
        assert_eq!(
            api.mutation("restore-original-link", vec![operation]).await,
            (StatusCode::OK, receipt),
            "same-key retry survives stale CAS"
        );
        assert_eq!(api.history(source).await, history);
        api.change(vec![restore(&restored, revision(&restored), Some(false))])
            .await;
        assert_eq!(
            api.node(source).await,
            restored,
            "unchanged restore is a no-op"
        );
        assert_eq!(api.history(source).await, history);
        assert_eq!(
            api.snapshot(source, revision(&linked)).await,
            retained,
            "reactivation never changes a historical edge version"
        );
    }
}

#[tokio::test]
async fn task_and_knowledge_restore_preserve_lifecycle_unless_explicitly_requested() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let initial = api.create(kind, "Lifecycle baseline").await;
        let lifecycle = if kind == "task" {
            json!({"body":"New lifecycle planning","task_state":"completed"})
        } else {
            json!({"body":"New lifecycle planning","retired":true,"retirement_reason":"Superseded finding"})
        };
        api.change(vec![patch(&initial, lifecycle)]).await;
        let changed = api.node(id(&initial)).await;
        assert_advanced(&initial, &changed);
        let historical = api.snapshot(id(&initial), revision(&changed)).await;
        if kind == "task" {
            assert_eq!(historical["node"]["task_state"], "completed");
        } else {
            assert_eq!(historical["node"]["retired"], true);
        }
        api.change(vec![restore(&changed, revision(&initial), None)])
            .await;
        let preserved = api.node(id(&initial)).await;
        assert_advanced(&changed, &preserved);
        assert_eq!(preserved["body"], initial["body"]);
        for field in ["task_state", "retired", "retirement_reason", "deleted"] {
            assert_eq!(
                preserved[field], changed[field],
                "default restore preserves {field} for {kind}"
            );
        }
        api.change(vec![restore(&preserved, revision(&initial), Some(true))])
            .await;
        let full = api.node(id(&initial)).await;
        assert_advanced(&preserved, &full);
        for field in ["task_state", "retired", "retirement_reason", "deleted"] {
            assert_eq!(
                full[field], initial[field],
                "explicit lifecycle restore restores {field} for {kind}"
            );
        }
        if kind == "knowledge" {
            assert_eq!(full["verification"], "unverified");
            assert!(full["last_verified_by"].is_null());
            assert!(full["last_verified_at"].is_null());
        }
        assert_eq!(
            api.snapshot(id(&initial), revision(&changed)).await,
            historical
        );
    }
}

#[tokio::test]
async fn task_and_knowledge_deletion_has_a_revision_and_restore_requires_explicit_undelete() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let initial = api.create(kind, "Tombstone baseline").await;
        api.change(vec![
            json!({"op":"delete_node","id":initial["id"],"expected_version":initial["version"]}),
        ])
        .await;
        let deleted = api.node(id(&initial)).await;
        assert_advanced(&initial, &deleted);
        let tombstone = api.snapshot(id(&initial), revision(&deleted)).await;
        assert_eq!(tombstone["node"]["deleted"], true);
        api.change(vec![restore(&deleted, revision(&initial), None)])
            .await;
        let still_deleted = api.node(id(&initial)).await;
        assert_eq!(
            still_deleted["deleted"], true,
            "default restore never implicitly undeletes {kind}"
        );
        api.change(vec![restore(
            &still_deleted,
            revision(&initial),
            Some(true),
        )])
        .await;
        let active = api.node(id(&initial)).await;
        assert_advanced(&deleted, &active);
        assert_eq!(active["deleted"], false);
        assert_eq!(
            api.snapshot(id(&initial), revision(&deleted)).await,
            tombstone,
            "tombstone history survives undelete"
        );
    }
}

#[tokio::test]
async fn owned_structural_membership_is_versioned_but_incoming_information_is_derived() {
    let api = Api::new().await;
    let parent = api.create("task", "Parent").await;
    let child = api.create("task", "Child").await;
    let blocker = api.create("task", "Prerequisite").await;
    let observer = api.create("task", "Independent observer").await;
    api.change(vec![json!({"op":"set_parent","id":child["id"],"expected_version":child["version"],"parent_id":parent["id"]})]).await;
    let parented = api.node(id(&child)).await;
    let parent_now = api.node(id(&parent)).await;
    assert_advanced(&child, &parented);
    assert_advanced(&parent, &parent_now);
    api.change(vec![
        json!({"op":"add_edge","source":blocker["id"],"target":child["id"],"edge_type":"blocks"}),
    ])
    .await;
    let blocked = api.node(id(&child)).await;
    assert_advanced(&parented, &blocked);
    assert_advanced(&blocker, &api.node(id(&blocker)).await);
    let snapshot = api.snapshot(id(&child), revision(&blocked)).await;
    assert_eq!(
        links(&snapshot).len(),
        2,
        "incoming parent membership and prerequisites belong to task revision"
    );
    let parent_edge = links(&snapshot)
        .iter()
        .find(|e| e["edge_type"] == "parent-child")
        .unwrap()
        .clone();
    api.change(vec![json!({"op":"add_edge","source":observer["id"],"target":child["id"],"edge_type":"references"})]).await;
    assert_eq!(
        api.node(id(&child)).await,
        blocked,
        "derived incoming informational link is not an edit to its target"
    );
    assert_eq!(api.snapshot(id(&child), revision(&blocked)).await, snapshot);
    api.change(vec![json!({"op":"set_parent","id":child["id"],"expected_version":blocked["version"],"parent_id":null})]).await;
    let detached = api.node(id(&child)).await;
    assert_advanced(&blocked, &detached);
    api.change(vec![restore(&detached, revision(&blocked), None)])
        .await;
    let restored = api.node(id(&child)).await;
    let current = api.snapshot(id(&child), revision(&restored)).await;
    assert_eq!(links(&current).len(), 2);
    let restored_parent = links(&current)
        .iter()
        .find(|e| e["edge_type"] == "parent-child")
        .unwrap();
    assert_eq!(restored_parent["id"], parent_edge["id"]);
    assert!(
        restored_parent["version"].as_i64().unwrap() > parent_edge["version"].as_i64().unwrap()
    );
    let context = api
        .get(&format!("/nodes/{}/context", child["id"].as_str().unwrap()))
        .await;
    assert!(
        context["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"] == observer["id"]),
        "restoring owned relationships preserves unrelated inbound information"
    );
}

#[tokio::test]
async fn common_revision_reads_and_restores_enforce_scope_version_and_permissions() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let foreign = Api::new().await;
        let initial = api.create(kind, "Scoped revision").await;
        assert_eq!(
            foreign
                .request(
                    "GET",
                    &format!(
                        "/nodes/{}/revisions/{}",
                        initial["id"].as_str().unwrap(),
                        revision(&initial)
                    ),
                    None
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let (status, error) = foreign
            .mutation(
                "foreign-restore",
                vec![restore(&initial, revision(&initial), None)],
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
        for rev in ["0", "-1", "not-a-number"] {
            let (status, error) = api
                .request(
                    "GET",
                    &format!("/nodes/{}/revisions/{rev}", initial["id"].as_str().unwrap()),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        }
        assert_eq!(
            api.request(
                "GET",
                &format!(
                    "/nodes/{}/revisions/999999",
                    initial["id"].as_str().unwrap()
                ),
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        api.change(vec![patch(&initial, json!({"body":"Concurrent content"}))])
            .await;
        let before = api.history(id(&initial)).await;
        let (status, error) = api
            .mutation(
                "stale-restore",
                vec![restore(&initial, revision(&initial), None)],
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(api.history(id(&initial)).await, before);
        let reader = format!("reader-{}", Uuid::new_v4());
        let (status, result) = api
            .request(
                "POST",
                "/credentials",
                Some(json!({"actor_id":"reader","token":reader,"capability":"read"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(
            api.request_as(
                &reader,
                "GET",
                &format!(
                    "/nodes/{}/revisions/{}",
                    initial["id"].as_str().unwrap(),
                    revision(&initial)
                ),
                None
            )
            .await
            .0,
            StatusCode::OK
        );
        let current = api.node(id(&initial)).await;
        let (status,error)=api.request_as(&reader,"POST","/mutations",Some(json!({"idempotency_key":"read-only","operations":[restore(&current,revision(&initial),Some(true))]}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    }
}

#[tokio::test]
async fn restoring_historical_relationships_revalidates_cycles_and_rolls_back_entire_batch() {
    let api = Api::new().await;
    let a = api.create("task", "A").await;
    let b = api.create("task", "B").await;
    let d = api.create("task", "D").await;
    // Historical D -> A -> B is valid; after removing A -> B, B -> D is also valid.
    api.change(vec![
        json!({"op":"add_edge","source":d["id"],"target":a["id"],"edge_type":"blocks"}),
    ])
    .await;
    let added = api
        .change(vec![
            json!({"op":"add_edge","source":a["id"],"target":b["id"],"edge_type":"blocks"}),
        ])
        .await;
    let edge = added["results"][0].clone();
    let historical = api.node(id(&a)).await;
    api.change(vec![
        json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]}),
    ])
    .await;
    api.change(vec![
        json!({"op":"add_edge","source":b["id"],"target":d["id"],"edge_type":"blocks"}),
    ])
    .await;
    let current = api.node(id(&a)).await;
    let histories = [
        api.history(id(&a)).await,
        api.history(id(&b)).await,
        api.history(id(&d)).await,
    ];
    let probe = Uuid::new_v4();
    let (status, error) = api
        .mutation(
            "invalid-cycle-restore",
            vec![
                json!({"op":"create_node","id":probe,"kind":"knowledge","title":"Must roll back"}),
                restore(&current, revision(&historical), None),
            ],
        )
        .await;
    assert!(
        matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
        "restore must refuse cycle D -> A -> B -> D: {error}"
    );
    assert_eq!(
        api.request("GET", &format!("/nodes/{probe}"), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(api.node(id(&a)).await, current);
    for (record, history) in [id(&a), id(&b), id(&d)].into_iter().zip(histories) {
        assert_eq!(
            api.history(record).await,
            history,
            "failed restore cannot leave endpoint revisions or edge history"
        );
    }
}

#[tokio::test]
async fn restoring_a_verified_knowledge_snapshot_never_reissues_its_trusted_badge() {
    let api = Api::new().await;
    let initial = api.create("knowledge", "Verified historical claim").await;
    api.change(vec![json!({"op":"verify_knowledge","id":initial["id"],"expected_version":initial["version"],"verification":"verified","evidence":[{"source_id":"evidence","kind":"url","url":"https://example.test/evidence","observed_at":"2026-10-30T00:00:00Z","retained":false}]})]).await;
    let verified = api.node(id(&initial)).await;
    let exact = api.snapshot(id(&initial), revision(&verified)).await;
    assert_eq!(exact["node"]["verification"], "verified");
    assert_eq!(exact["node"]["last_verified_by"], "version-author");
    api.change(vec![patch(
        &verified,
        json!({"body":"A later corrected assertion"}),
    )])
    .await;
    let corrected = api.node(id(&initial)).await;
    assert_eq!(corrected["verification"], "unverified");
    api.change(vec![restore(&corrected, revision(&verified), Some(true))])
        .await;
    let restored = api.node(id(&initial)).await;
    assert_advanced(&corrected, &restored);
    assert_eq!(restored["body"], verified["body"]);
    assert_eq!(restored["provenance"], verified["provenance"]);
    assert_eq!(restored["verification"], "unverified");
    assert!(restored["last_verified_by"].is_null());
    assert!(restored["last_verified_at"].is_null());
    assert_eq!(
        api.snapshot(id(&initial), revision(&verified)).await,
        exact,
        "historical attribution remains exact while restored content needs a fresh assertion"
    );
}

#[tokio::test]
async fn both_kinds_reject_restoring_links_to_deleted_targets_without_partial_history() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let source = api.create(kind, "Live source").await;
        let target = api.create("knowledge", "Later deleted target").await;
        let result=api.change(vec![json!({"op":"add_edge","source":source["id"],"target":target["id"],"edge_type":"references"})]).await;
        let edge = &result["results"][0];
        let linked = api.node(id(&source)).await;
        api.change(vec![
            json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]}),
        ])
        .await;
        api.change(vec![
            json!({"op":"delete_node","id":target["id"],"expected_version":target["version"]}),
        ])
        .await;
        let current = api.node(id(&source)).await;
        let history = api.history(id(&source)).await;
        let target_history = api.history(id(&target)).await;
        let probe = Uuid::new_v4();
        let (status,error)=api.mutation("missing-target-restore",vec![json!({"op":"create_node","id":probe,"kind":"knowledge","title":"No partial create"}),restore(&current,revision(&linked),None)]).await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
            "historical edge cannot reconnect a deleted live target: {error}"
        );
        assert_eq!(
            api.request("GET", &format!("/nodes/{probe}"), None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(api.node(id(&source)).await, current);
        assert_eq!(api.history(id(&source)).await, history);
        assert_eq!(api.history(id(&target)).await, target_history);
        let (status, result) = api
            .mutation(
                "missing-target-restore",
                vec![patch(&current, json!({"body":"Corrected retry"}))],
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "failed restore rolls back its idempotency receipt: {result}"
        );
    }
}

#[tokio::test]
async fn both_kinds_restore_with_cas_conflicts_and_concurrent_idempotent_receipts() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let initial = api.create(kind, "Concurrent restore").await;
        api.change(vec![patch(&initial, json!({"body":"Changed before race"}))])
            .await;
        let current = api.node(id(&initial)).await;
        let operation = restore(&current, revision(&initial), None);
        let (a, b) = tokio::join!(
            api.mutation("restore-a", vec![operation.clone()]),
            api.mutation("restore-b", vec![operation])
        );
        assert!(
            matches!(
                (a.0, b.0),
                (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
            ),
            "different-key same-version restores have one CAS winner: {a:?} {b:?}"
        );
        let restored = api.node(id(&initial)).await;
        api.change(vec![patch(
            &restored,
            json!({"body":"Second racing round"}),
        )])
        .await;
        let current = api.node(id(&initial)).await;
        let operation = restore(&current, revision(&initial), None);
        let before = revision_count(&api.history(id(&initial)).await);
        let (a, b) = tokio::join!(
            api.mutation("same-restore", vec![operation.clone()]),
            api.mutation("same-restore", vec![operation])
        );
        assert_eq!(a.0, StatusCode::OK, "{a:?}");
        assert_eq!(
            a, b,
            "same-key concurrent restore returns one committed receipt"
        );
        assert_eq!(
            revision_count(&api.history(id(&initial)).await),
            before + 1,
            "same-key restore appends exactly one revision"
        );
    }
}
