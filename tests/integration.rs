//! Published-API contract tests against a real PostgreSQL database.
//! These deliberately fail when AQ_GRAPH_TEST_DATABASE_URL is missing; they never skip.
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
        let url=std::env::var("AQ_GRAPH_TEST_DATABASE_URL").expect("Integration tests require a real PostgreSQL database: set AQ_GRAPH_TEST_DATABASE_URL (see docs/api.md)");
        let store = Store::connect(&url, 8)
            .await
            .expect("connect PostgreSQL test database");
        store
            .migrate()
            .await
            .expect("migrate PostgreSQL test database");
        let workspace = Uuid::new_v4();
        let token = format!("test-{}", Uuid::new_v4());
        store
            .bootstrap(workspace, "contract-test", &token)
            .await
            .expect("bootstrap isolated test workspace");
        Self {
            app: service::router(store),
            workspace,
            token,
        }
    }
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.as_token(method, path, body, Some(&self.token)).await
    }
    async fn as_token(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
        let path = format!("/v1/workspaces/{}{path}", self.workspace);
        self.raw(
            method,
            &path,
            body.map(|v| v.to_string()).unwrap_or_default(),
            token,
        )
        .await
    }
    async fn raw(
        &self,
        method: &str,
        path: &str,
        body: String,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.headers().get("x-aq-graph-api-version").unwrap(),
            "1"
        );
        let status = response.status();
        let bytes = to_bytes(response.into_body(), service::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            panic!("non-JSON API response: {}", String::from_utf8_lossy(&bytes))
        });
        (status, value)
    }
    async fn mutation(&self, key: &str, operations: Vec<Value>) -> (StatusCode, Value) {
        self.request(
            "POST",
            "/mutations",
            Some(json!({"idempotency_key":key,"operations":operations})),
        )
        .await
    }
    async fn create(&self, id: Uuid, kind: &str, state: &str, title: &str) -> Value {
        let op = if kind == "task" {
            json!({"op":"create_node","id":id,"kind":"task","title":title,"task_state":state})
        } else {
            json!({"op":"create_node","id":id,"kind":"knowledge","title":title,"body":"authentication security evidence","knowledge_type":"decision"})
        };
        let (status, result) = self.mutation(&Uuid::new_v4().to_string(), vec![op]).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        let (status, node) = self.request("GET", &format!("/nodes/{id}"), None).await;
        assert_eq!(status, StatusCode::OK, "{node}");
        node
    }
}
fn patch(id: Uuid, version: i64, content: Value) -> Value {
    json!({"op":"update_node","id":id,"expected_version":version,"patch":content})
}
fn edge(source: Uuid, target: Uuid, kind: &str) -> Value {
    json!({"op":"add_edge","source":source,"target":target,"edge_type":kind})
}
fn items(value: &Value) -> &Vec<Value> {
    value["items"].as_array().expect("API page has items array")
}

#[tokio::test]
async fn authentication_workspace_capability_and_protocol_errors() {
    let c = Client::new().await;
    assert_eq!(
        c.as_token("GET", "/nodes", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        c.as_token("GET", "/nodes", None, Some("invalid")).await.0,
        StatusCode::UNAUTHORIZED
    );
    let foreign = format!("/v1/workspaces/{}/nodes", Uuid::new_v4());
    assert_eq!(
        c.raw("GET", &foreign, String::new(), Some(&c.token))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let foreign_client = Client::new().await;
    let foreign_node = Uuid::new_v4();
    foreign_client
        .create(foreign_node, "task", "active", "private")
        .await;
    let (status, error) = c
        .request("GET", &format!("/nodes/{foreign_node}"), None)
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign record identity must not escape scope: {error}"
    );
    let (status, error) = c
        .request("DELETE", "/credentials/contract-test", None)
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "last administrator must remain: {error}"
    );
    let reader = format!("reader-{}", Uuid::new_v4());
    assert_eq!(
        c.request(
            "POST",
            "/credentials",
            Some(json!({"actor_id":"reader","token":reader,"capability":"read"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status,error)=c.as_token("POST","/mutations",Some(json!({"idempotency_key":"denied","operations":[{"op":"create_node","kind":"task","title":"denied"}]})),Some(&reader)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert_eq!(error["error"]["code"], "forbidden");
    assert_eq!(
        c.as_token("GET", "/nodes", None, Some(&reader)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        c.request("DELETE", "/credentials/reader", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        c.as_token("GET", "/nodes", None, Some(&reader)).await.0,
        StatusCode::UNAUTHORIZED
    );
    for path in [
        "/nodes/not-a-uuid",
        "/nodes/%FF",
        "/nodes?limit=not-a-number",
        "/nodes?limit=201",
    ] {
        let (status, value) = c.request("GET", path, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
        assert!(value["error"]["code"].is_string());
    }
    let path = format!("/v1/workspaces/{}/mutations", c.workspace);
    assert_eq!(
        c.raw("POST", &path, "{".into(), Some(&c.token)).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        c.raw(
            "POST",
            &path,
            "x".repeat(service::MAX_REQUEST_BYTES + 1),
            Some(&c.token)
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let (status, error) = c.raw("GET", "/missing", String::new(), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "not_found");
}

#[tokio::test]
async fn selective_history_version_conflicts_noops_and_content_restore() {
    let c = Client::new().await;
    let id = Uuid::new_v4();
    let node = c.create(id, "task", "draft", "original").await;
    let v = node["version"].as_i64().unwrap();
    let (_, before) = c
        .request("GET", &format!("/nodes/{id}/history"), None)
        .await;
    assert_eq!(
        c.mutation("noop", vec![patch(id, v, json!({"title":"original"}))])
            .await
            .0,
        StatusCode::OK
    );
    let (_, noop) = c
        .request("GET", &format!("/nodes/{id}/history"), None)
        .await;
    assert_eq!(before, noop, "no-op edit must not create history");
    assert_eq!(
        c.mutation(
            "activate",
            vec![patch(id, v, json!({"task_state":"active"}))]
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, active) = c.request("GET", &format!("/nodes/{id}"), None).await;
    assert_eq!(
        active["content_revision"], node["content_revision"],
        "lifecycle mutation must not copy durable content"
    );
    assert!(active["version"].as_i64().unwrap() > v);
    let (status, err) = c
        .mutation("stale", vec![patch(id, v, json!({"title":"lost update"}))])
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    let version = active["version"].as_i64().unwrap();
    assert_eq!(
        c.mutation(
            "content",
            vec![patch(
                id,
                version,
                json!({"title":"revised","task_state":"completed"})
            )]
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, current) = c.request("GET", &format!("/nodes/{id}"), None).await;
    let (_, history) = c
        .request("GET", &format!("/nodes/{id}/history"), None)
        .await;
    let revisions = items(&history)
        .iter()
        .filter(|e| e["category"] == "revision")
        .count();
    let events = items(&history)
        .iter()
        .filter(|e| e["category"] == "lifecycle")
        .count();
    assert_eq!(revisions, 2, "{history}");
    assert!(events >= 2, "{history}");
    assert!(
        items(&history)
            .iter()
            .all(|e| e["actor_id"] == "contract-test" && e["change_set_id"].is_string())
    );
    let (_, diff) = c
        .request(
            "GET",
            &format!(
                "/nodes/{id}/diff?from={}&to={}",
                node["content_revision"], current["content_revision"]
            ),
            None,
        )
        .await;
    assert!(
        diff["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["field"] == "title"),
        "{diff}"
    );
    let restore = json!({"op":"restore_node","id":id,"expected_version":current["version"],"revision":node["content_revision"]});
    let (status, result) = c.mutation("restore", vec![restore]).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (_, restored) = c.request("GET", &format!("/nodes/{id}"), None).await;
    assert_eq!(restored["title"], "original");
    assert_eq!(restored["task_state"], "completed");
    assert!(
        restored["content_revision"].as_i64().unwrap()
            > current["content_revision"].as_i64().unwrap()
    );
    let (_, page) = c
        .request("GET", &format!("/nodes/{id}/history?limit=1"), None)
        .await;
    assert_eq!(items(&page).len(), 1);
    assert!(page["next_cursor"].is_string());
    let (_, page2) = c
        .request(
            "GET",
            &format!(
                "/nodes/{id}/history?limit=1&cursor={}",
                page["next_cursor"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_ne!(items(&page)[0]["entry_id"], items(&page2)[0]["entry_id"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_idempotency_and_same_record_edit_conflict() {
    let c = Client::new().await;
    let id = Uuid::new_v4();
    let operations = vec![json!({"op":"create_node","id":id,"kind":"task","title":"once"})];
    let requests = (0..12)
        .map(|_| {
            let app = c.app.clone();
            let token = c.token.clone();
            let workspace = c.workspace;
            let operations = operations.clone();
            tokio::spawn(async move {
                Client {
                    app,
                    workspace,
                    token,
                }
                .mutation("same-key", operations)
                .await
            })
        })
        .collect::<Vec<_>>();
    let mut receipt = None;
    for request in requests {
        let (status, result) = request.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{result}");
        if let Some(ref expected) = receipt {
            assert_eq!(expected, &result)
        } else {
            receipt = Some(result)
        }
    }
    let (_, history) = c
        .request("GET", &format!("/nodes/{id}/history"), None)
        .await;
    assert_eq!(
        items(&history)
            .iter()
            .filter(|e| e["category"] == "revision")
            .count(),
        1
    );
    let (status, _) = c
        .mutation(
            "same-key",
            vec![json!({"op":"create_node","kind":"task","title":"different payload"})],
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, node) = c.request("GET", &format!("/nodes/{id}"), None).await;
    let v = node["version"].as_i64().unwrap();
    let (a, b) = tokio::join!(
        c.mutation("edit-a", vec![patch(id, v, json!({"title":"a"}))]),
        c.mutation("edit-b", vec![patch(id, v, json!({"title":"b"}))])
    );
    assert!(
        matches!(
            (a.0, b.0),
            (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
        ),
        "{a:?} {b:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_cycle_insertions_and_readiness_semantics() {
    let c = Client::new().await;
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    c.create(a, "task", "active", "a").await;
    c.create(b, "task", "active", "b").await;
    let (first, second) = tokio::join!(
        c.mutation("a-b", vec![edge(a, b, "blocks")]),
        c.mutation("b-a", vec![edge(b, a, "blocks")])
    );
    assert_eq!(
        [first.0, second.0]
            .iter()
            .filter(|s| **s == StatusCode::OK)
            .count(),
        1,
        "{first:?} {second:?}"
    );
    let loser = if first.0 == StatusCode::OK {
        second.0
    } else {
        first.0
    };
    assert!(loser == StatusCode::CONFLICT || loser == StatusCode::BAD_REQUEST);
    let (_, ready) = c.request("GET", "/ready", None).await;
    assert_eq!(items(&ready).len(), 1, "{ready}");
    let prerequisite = if first.0 == StatusCode::OK { a } else { b };
    let (_, node) = c
        .request("GET", &format!("/nodes/{prerequisite}"), None)
        .await;
    assert_eq!(
        c.mutation(
            "cancel",
            vec![patch(
                prerequisite,
                node["version"].as_i64().unwrap(),
                json!({"task_state":"canceled"})
            )]
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, ready) = c.request("GET", "/ready", None).await;
    assert!(
        items(&ready).is_empty(),
        "canceled prerequisite remains unresolved"
    );
    let (_, node) = c
        .request("GET", &format!("/nodes/{prerequisite}"), None)
        .await;
    assert_eq!(
        c.mutation(
            "complete",
            vec![patch(
                prerequisite,
                node["version"].as_i64().unwrap(),
                json!({"task_state":"completed"})
            )]
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, ready) = c.request("GET", "/ready", None).await;
    assert_eq!(items(&ready).len(), 1);
    let x = Uuid::new_v4();
    let y = Uuid::new_v4();
    c.create(x, "task", "active", "x").await;
    c.create(y, "task", "active", "y").await;
    let (a, b) = tokio::join!(
        c.mutation("parent-a", vec![edge(x, y, "parent-child")]),
        c.mutation("parent-b", vec![edge(y, x, "parent-child")])
    );
    assert_eq!(
        [a.0, b.0].iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "{a:?} {b:?}"
    );
}

#[tokio::test]
async fn draft_validation_rollback_atomic_apply_stale_and_restart() {
    let c = Client::new().await;
    let id = Uuid::new_v4();
    let proposal = json!({"name":"reviewed plan","idempotency_key":"draft-create","operations":[{"op":"create_node","id":id,"kind":"task","title":"proposed","task_state":"active"}]});
    let (status, draft) = c
        .request("POST", "/changesets", Some(proposal.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{draft}");
    let draft_id = draft["id"].as_str().unwrap();
    assert_eq!(
        c.request("GET", &format!("/nodes/{id}"), None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, validation) = c
        .request("POST", &format!("/changesets/{draft_id}/validate"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{validation}");
    assert_eq!(validation["valid"], true);
    assert_eq!(
        c.request("GET", &format!("/nodes/{id}"), None).await.0,
        StatusCode::NOT_FOUND,
        "validation must roll back live records and their history"
    );
    let path = format!("/changesets/{draft_id}/apply");
    let (status, result) = c
        .request("POST", &path, Some(json!({"idempotency_key":"apply"})))
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        c.request("POST", &path, Some(json!({"idempotency_key":"apply"})))
            .await
            .1,
        result
    );
    let (_, node) = c.request("GET", &format!("/nodes/{id}"), None).await;
    let bad = Uuid::new_v4();
    let (status, result) = c
        .mutation(
            "rollback-batch",
            vec![
                json!({"op":"create_node","id":bad,"kind":"task","title":"must rollback"}),
                patch(id, 9999, json!({"title":"stale"})),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{result}");
    assert_eq!(
        c.request("GET", &format!("/nodes/{bad}"), None).await.0,
        StatusCode::NOT_FOUND
    );
    let stale = json!({"name":"stale","idempotency_key":"stale-draft","operations":[patch(id,node["version"].as_i64().unwrap(),json!({"title":"draft title"}))]});
    let (_, draft) = c.request("POST", "/changesets", Some(stale)).await;
    let draft_id = draft["id"].as_str().unwrap();
    c.mutation(
        "intervening",
        vec![patch(
            id,
            node["version"].as_i64().unwrap(),
            json!({"title":"newer"}),
        )],
    )
    .await;
    assert_eq!(
        c.request(
            "POST",
            &format!("/changesets/{draft_id}/apply"),
            Some(json!({"idempotency_key":"stale-apply"}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    // Construct a fresh store/router: credentials, receipts, and current records survive process state loss.
    let store = Store::connect(&std::env::var("AQ_GRAPH_TEST_DATABASE_URL").unwrap(), 2)
        .await
        .unwrap();
    let restarted = Client {
        app: service::router(store),
        workspace: c.workspace,
        token: c.token.clone(),
    };
    assert_eq!(
        restarted
            .request("GET", &format!("/nodes/{id}"), None)
            .await
            .1["title"],
        "newer"
    );
    assert_eq!(
        restarted
            .request("POST", &path, Some(json!({"idempotency_key":"apply"})))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn pinned_revisions_deletion_search_and_bounded_context() {
    let c = Client::new().await;
    let task = Uuid::new_v4();
    let knowledge = Uuid::new_v4();
    let task_node = c.create(task, "task", "active", "implementation").await;
    let original = c
        .create(knowledge, "knowledge", "", "authentication decision")
        .await;
    let unavailable = json!({"op":"add_edge","source":task,"target":knowledge,"edge_type":"references","target_revision":999999});
    let (status, error) = c.mutation("unavailable-pin", vec![unavailable]).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "missing revision must never resolve to current content: {error}"
    );
    let pinned = json!({"op":"add_edge","source":task,"target":knowledge,"edge_type":"references","target_revision":original["content_revision"]});
    assert_eq!(c.mutation("pin", vec![pinned]).await.0, StatusCode::OK);
    assert_eq!(
        c.mutation(
            "edit-knowledge",
            vec![patch(
                knowledge,
                original["version"].as_i64().unwrap(),
                json!({"title":"current authentication policy"})
            )]
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, context) = c
        .request("GET", &format!("/nodes/{task}/context"), None)
        .await;
    assert!(
        context["pinned_revisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["title"] == original["title"]),
        "{context}"
    );
    let (_, matches) = c
        .request("GET", "/search?q=authentication&limit=1", None)
        .await;
    assert_eq!(items(&matches).len(), 1, "{matches}");
    let (_, current) = c.request("GET", &format!("/nodes/{knowledge}"), None).await;
    let (status, _) = c
        .mutation(
            "bad-delete",
            vec![json!({"op":"delete_node","id":knowledge,"expected_version":current["version"]})],
        )
        .await;
    assert!(
        status == StatusCode::CONFLICT || status == StatusCode::BAD_REQUEST,
        "active references must reject deletion"
    );
    let edge_id = context["edges"][0]["id"].clone();
    let edge_version = context["edges"][0]["version"].clone();
    let (status, result) = c
        .mutation(
            "resolve-delete",
            vec![
                json!({"op":"remove_edge","id":edge_id,"expected_version":edge_version}),
                json!({"op":"delete_node","id":knowledge,"expected_version":current["version"]}),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (_, deleted) = c.request("GET", &format!("/nodes/{knowledge}"), None).await;
    assert_eq!(deleted["deleted"], true);
    let (_, matches) = c.request("GET", "/search?q=authentication", None).await;
    assert!(items(&matches).is_empty());
    let (_, history) = c
        .request("GET", &format!("/nodes/{knowledge}/history"), None)
        .await;
    assert!(!items(&history).is_empty());
    for i in 0..4 {
        let linked = Uuid::new_v4();
        c.create(linked, "task", "active", &format!("linked {i}"))
            .await;
        c.mutation(&format!("link-{i}"), vec![edge(task, linked, "references")])
            .await;
    }
    let (_, small) = c
        .request(
            "GET",
            &format!("/nodes/{task}/context?limit=1&depth=2"),
            None,
        )
        .await;
    assert_eq!(small["truncated"], true, "{small}");
    assert!(small["nodes"].as_array().unwrap().len() <= 1);
    let (status, error) = c
        .request("GET", &format!("/nodes/{task}/context?depth=99"), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    let (status, error) = c
        .request("GET", &format!("/nodes/{task}/context?max_bytes=1"), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(
        c.request("GET", &format!("/nodes/{task}"), None).await.1["version"],
        task_node["version"],
        "link edits do not modify durable node content"
    );
}
