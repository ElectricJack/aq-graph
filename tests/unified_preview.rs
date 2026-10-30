//! Draft validation previews every relationship owner and rolls back live state.
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
        let token = format!("preview-{}", Uuid::new_v4());
        store
            .bootstrap(workspace, "preview-author", &token)
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
        let body = to_bytes(response.into_body(), service::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn get(&self, path: &str) -> Value {
        let (s, v) = self.request("GET", path, None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }
    async fn node(&self, id: Uuid) -> Value {
        self.get(&format!("/nodes/{id}")).await
    }
    async fn change(&self, operations: Vec<Value>) -> Value {
        let (s, v) = self
            .request(
                "POST",
                "/mutations",
                Some(json!({"idempotency_key":Uuid::new_v4().to_string(),"operations":operations})),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }
    async fn task(&self) -> Uuid {
        let id = Uuid::new_v4();
        self.change(vec![json!({"op":"create_node","id":id,"kind":"task","title":"preview task","task_state":"active"})]).await;
        id
    }
    async fn draft(&self, operations: Vec<Value>) -> Uuid {
        let(s,v)=self.request("POST","/changesets",Some(json!({"name":"review complete graph changes","idempotency_key":Uuid::new_v4().to_string(),"operations":operations}))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        serde_json::from_value(v["id"].clone()).unwrap()
    }
    async fn validate(&self, draft: Uuid) -> Value {
        let (s, v) = self
            .request("POST", &format!("/changesets/{draft}/validate"), None)
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["valid"], true);
        v["preview"].clone()
    }
}
fn entry<'a>(preview: &'a Value, kind: &str, id: Uuid) -> &'a Value {
    preview[kind]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(id))
        .unwrap_or_else(|| panic!("missing {kind} entry for {id}: {preview}"))
}

#[tokio::test]
async fn restore_preview_includes_removed_restored_edges_and_neighbor_versions_then_rolls_back() {
    let api = Api::new().await;
    let old_prerequisite = api.task().await;
    let new_prerequisite = api.task().await;
    let task = api.task().await;
    let old_edge = Uuid::new_v4();
    let new_edge = Uuid::new_v4();
    api.change(vec![json!({"op":"add_edge","id":old_edge,"source":old_prerequisite,"target":task,"edge_type":"blocks"})]).await;
    let desired = api.node(task).await["content_revision"].as_i64().unwrap();
    api.change(vec![json!({"op":"remove_edge","id":old_edge,"expected_version":1}),json!({"op":"add_edge","id":new_edge,"source":new_prerequisite,"target":task,"edge_type":"blocks"})]).await;
    let mut before = vec![];
    for id in [task, old_prerequisite, new_prerequisite] {
        before.push((
            id,
            api.node(id).await,
            api.get(&format!("/nodes/{id}/history?limit=200")).await,
        ));
    }
    let revision_before = api.get(&format!("/nodes/{task}/revisions/{desired}")).await;
    let current = api.node(task).await;
    let draft=api.draft(vec![json!({"op":"restore_node","id":task,"expected_version":current["version"],"revision":desired})]).await;
    let preview = api.validate(draft).await;
    assert_eq!(
        preview["nodes"].as_array().unwrap().len(),
        3,
        "both prerequisite owners must be reviewable"
    );
    for (id, node, history) in &before {
        let row = entry(&preview, "nodes", *id);
        assert_eq!(row["before"]["version"], node["version"]);
        assert!(
            row["after"]["version"].as_i64().unwrap() > node["version"].as_i64().unwrap(),
            "neighbor mutation missing: {row}"
        );
        assert_eq!(
            api.node(*id).await,
            *node,
            "validation must roll back neighbor versions"
        );
        assert_eq!(
            api.get(&format!("/nodes/{id}/history?limit=200")).await,
            *history,
            "validation must roll back neighbor history"
        );
    }
    let restored = entry(&preview, "edges", old_edge);
    assert_eq!(restored["before"]["deleted"], true);
    assert_eq!(restored["after"]["deleted"], false);
    assert_eq!(restored["before"]["version"], 2);
    assert_eq!(restored["after"]["version"], 3);
    let removed = entry(&preview, "edges", new_edge);
    assert_eq!(removed["before"]["deleted"], false);
    assert_eq!(removed["after"]["deleted"], true);
    assert_eq!(removed["after"]["version"], 2);
    assert_eq!(
        api.get(&format!("/nodes/{task}/revisions/{desired}")).await,
        revision_before,
        "old relationship snapshots stay immutable"
    );
    let links = api.get(&format!("/nodes/{task}/context")).await;
    assert_eq!(links["edges"].as_array().unwrap().len(), 1);
    assert_eq!(links["edges"][0]["id"], json!(new_edge));
    let (s, result) = api
        .request(
            "POST",
            &format!("/changesets/{draft}/apply"),
            Some(json!({"idempotency_key":"apply-reviewed-restore"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{result}");
    for (id, _, _) in before {
        assert_eq!(
            api.node(id).await["version"],
            entry(&preview, "nodes", id)["after"]["version"],
            "applied version must agree with validated deterministic mutations"
        );
    }
}

#[tokio::test]
async fn preview_tracks_new_nodes_and_generated_parent_edges_even_when_reparented_in_one_draft() {
    let api = Api::new().await;
    let first = api.task().await;
    let second = api.task().await;
    let child = Uuid::new_v4();
    let first_before = api.node(first).await;
    let second_before = api.node(second).await;
    let draft=api.draft(vec![json!({"op":"create_node","id":child,"kind":"task","title":"new reviewed child","task_state":"active"}),json!({"op":"set_parent","id":child,"expected_version":1,"parent_id":first}),json!({"op":"set_parent","id":child,"expected_version":2,"parent_id":second})]).await;
    let preview = api.validate(draft).await;
    let new_child = entry(&preview, "nodes", child);
    assert!(new_child["before"].is_null());
    assert_eq!(new_child["after"]["id"], json!(child));
    assert_eq!(new_child["after"]["version"], 3);
    let edges = preview["edges"].as_array().unwrap();
    assert_eq!(
        edges.len(),
        2,
        "both generated identities must appear, including the first removed later"
    );
    assert!(
        edges
            .iter()
            .all(|row| row["before"].is_null() && row["after"]["target"] == json!(child))
    );
    assert!(
        edges
            .iter()
            .any(|row| row["after"]["source"] == json!(first) && row["after"]["deleted"] == true)
    );
    assert!(
        edges
            .iter()
            .any(|row| row["after"]["source"] == json!(second) && row["after"]["deleted"] == false)
    );
    for (id, before) in [(first, first_before), (second, second_before)] {
        let row = entry(&preview, "nodes", id);
        assert!(row["after"]["version"].as_i64().unwrap() > before["version"].as_i64().unwrap());
        assert_eq!(api.node(id).await, before);
        assert!(
            api.get(&format!("/nodes/{id}/context")).await["edges"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        api.request("GET", &format!("/nodes/{child}"), None).await.0,
        StatusCode::NOT_FOUND,
        "validation-created node must not leak into live graph"
    );
    assert!(
        preview.get("_parent_children").is_none() && preview.get("_prior_parent_ids").is_none(),
        "internal discovery metadata must not enter API output"
    );
}
