//! Privileged task draft receipts and no-op restoration preserve shared policy.
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
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("set AQ_GRAPH_TEST_DATABASE_URL to disposable PostgreSQL");
        let store = Store::connect(&url, 4).await.unwrap();
        store.migrate().await.unwrap();
        let workspace = Uuid::new_v4();
        let token = format!("authority-{}", Uuid::new_v4());
        store
            .bootstrap(workspace, "authority-author", &token)
            .await
            .unwrap();
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
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
            .unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), service::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn get(&self, path: &str) -> Value {
        let (s, v) = self.request("GET", path, None).await;
        assert_eq!(s, StatusCode::OK, "{path}: {v}");
        v
    }
    async fn change(&self, ops: Vec<Value>) -> Value {
        let(s,v)=self.request("POST","/mutations",Some(json!({"idempotency_key":Uuid::new_v4().to_string(),"reason":"Checked shared authority action","operations":ops}))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }
    async fn node(&self, id: Uuid) -> Value {
        self.get(&format!("/nodes/{id}")).await
    }
    async fn create(&self, kind: &str) -> Value {
        let id = Uuid::new_v4();
        let mut op = json!({"op":"create_node","id":id,"kind":kind,"title":"Shared authority record","body":"Exact material body","provenance":[{"source_id":"evidence","kind":"url","url":"https://example.invalid/checked","observed_at":"2026-10-30T00:00:00Z","retained":false}]});
        if kind == "task" {
            op["task_state"] = json!("active");
        }
        self.change(vec![op]).await;
        self.node(id).await
    }
}

#[tokio::test]
async fn applied_task_retirement_draft_receipt_is_not_replayable_after_admin_downgrade() {
    let api = Api::new().await;
    let task = api.create("task").await;
    let id: Uuid = serde_json::from_value(task["id"].clone()).unwrap();
    let(s,draft)=api.request("POST","/changesets",Some(json!({"name":"Retire approved task","idempotency_key":"retirement-draft","operations":[{"op":"update_node","id":id,"expected_version":task["version"],"patch":{"retired":true,"retirement_reason":"Task is obsolete"}}]}))).await;
    assert_eq!(s, StatusCode::OK, "{draft}");
    let path = format!("/changesets/{}/apply", draft["id"].as_str().unwrap());
    let apply = json!({"idempotency_key":"apply-task-retirement"});
    let (s, committed) = api.request("POST", &path, Some(apply.clone())).await;
    assert_eq!(s, StatusCode::OK, "{committed}");
    let (s, replayed) = api.request("POST", &path, Some(apply.clone())).await;
    assert_eq!(s, StatusCode::OK, "{replayed}");
    assert_eq!(replayed, committed);
    let before = api.node(id).await;
    assert_eq!(before["retired"], true);
    let history = api.get(&format!("/nodes/{id}/history?limit=200")).await;
    let(s,v)=api.request("POST","/credentials",Some(json!({"actor_id":"backup-admin","token":format!("backup-{}",Uuid::new_v4()),"capability":"admin"}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = api
        .request(
            "POST",
            "/credentials",
            Some(json!({"actor_id":"authority-author","token":api.token,"capability":"write"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, error) = api.request("POST", &path, Some(apply)).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "a committed receipt cannot bypass current privileged task policy: {error}"
    );
    assert_eq!(error["error"]["code"], "forbidden");
    assert_eq!(api.node(id).await, before);
    assert_eq!(
        api.get(&format!("/nodes/{id}/history?limit=200")).await,
        history
    );
}

#[tokio::test]
async fn true_noop_historical_restore_preserves_summary_and_verification_without_new_history() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let original = api.create(kind).await;
        let id: Uuid = serde_json::from_value(original["id"].clone()).unwrap();
        api.change(vec![json!({"op":"set_summary","id":id,"expected_version":original["version"],"source_revision":original["content_revision"],"summary":"An exact human-authored summary"})]).await;
        let summarized = api.node(id).await;
        assert!(summarized["summary"].is_string());
        let history = api.get(&format!("/nodes/{id}/history?limit=200")).await;
        api.change(vec![json!({"op":"restore_node","id":id,"expected_version":summarized["version"],"revision":original["content_revision"]})]).await;
        assert_eq!(
            api.node(id).await,
            summarized,
            "a true no-op restore cannot invalidate a current summary or advance its tokens"
        );
        assert_eq!(
            api.get(&format!("/nodes/{id}/history?limit=200")).await,
            history
        );
        // Verification reuses the exact evidence already retained at creation,
        // clears the summary, and creates a revision. Restoring the earlier
        // identical material/evidence is a true no-op and preserves the assertion.
        api.change(vec![json!({"op":"verify_node","id":id,"expected_version":summarized["version"],"verification":"verified","evidence":[{"source_id":"evidence","kind":"url","url":"https://example.invalid/checked","observed_at":"2026-10-30T00:00:00Z","retained":false}]})]).await;
        let verified = api.node(id).await;
        assert_eq!(verified["verification"], "verified");
        assert!(verified["summary"].is_null());
        let history = api.get(&format!("/nodes/{id}/history?limit=200")).await;
        api.change(vec![json!({"op":"restore_node","id":id,"expected_version":verified["version"],"revision":original["content_revision"]})]).await;
        assert_eq!(
            api.node(id).await,
            verified,
            "a true no-op restore cannot clear current authenticated verification"
        );
        assert_eq!(
            api.get(&format!("/nodes/{id}/history?limit=200")).await,
            history
        );
    }
}
