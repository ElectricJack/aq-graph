//! Shared record fields have the same public API semantics on tasks and knowledge.
//! PostgreSQL is required: no skipped database checks or v1 imports.
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
            "shared node tests require disposable PostgreSQL via AQ_GRAPH_TEST_DATABASE_URL",
        );
        let store = Store::connect(&url, 4).await.expect("connect PostgreSQL");
        store.migrate().await.expect("migrate PostgreSQL");
        let workspace = Uuid::new_v4();
        let token = format!("shared-{}", Uuid::new_v4());
        store
            .bootstrap(workspace, "shared-author", &token)
            .await
            .expect("bootstrap isolated workspace");
        Self {
            app: service::router(store),
            workspace,
            token,
        }
    }
    async fn call_as(
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
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call_as(&self.token, method, path, body).await
    }
    async fn get(&self, path: &str) -> Value {
        let (status, v) = self.call("GET", path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {v}");
        v
    }
    async fn mutation_as(
        &self,
        token: &str,
        key: &str,
        operations: Vec<Value>,
    ) -> (StatusCode, Value) {
        self.call_as(token,"POST","/mutations",Some(json!({"idempotency_key":key,"reason":"Shared record field contract","operations":operations}))).await
    }
    async fn mutation(&self, key: &str, operations: Vec<Value>) -> (StatusCode, Value) {
        self.mutation_as(&self.token, key, operations).await
    }
    async fn change(&self, operations: Vec<Value>) -> Value {
        let (status, v) = self.mutation(&Uuid::new_v4().to_string(), operations).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        v
    }
    async fn create(&self, kind: &str, title: &str) -> Value {
        self.create_as(&self.token, kind, title, json!({})).await
    }
    async fn create_as(&self, token: &str, kind: &str, title: &str, extra: Value) -> Value {
        let id = Uuid::new_v4();
        let mut op = json!({"op":"create_node","id":id,"kind":kind,"title":title,"body":"sharedneedle original body","content_type":"decision"});
        if kind == "task" {
            op["task_state"] = json!("active");
        }
        op.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let (status, v) = self
            .mutation_as(token, &Uuid::new_v4().to_string(), vec![op])
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        self.node(id).await
    }
    async fn node(&self, id: Uuid) -> Value {
        self.get(&format!("/nodes/{id}")).await
    }
    async fn writer(&self, actor: &str) -> String {
        let token = format!("writer-{}", Uuid::new_v4());
        let (status, v) = self
            .call(
                "POST",
                "/credentials",
                Some(json!({"actor_id":actor,"token":token,"capability":"write"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        token
    }
    async fn verify(&self, n: &Value, verification: &str) -> Value {
        self.change(vec![json!({"op":"verify_node","id":n["id"],"expected_version":n["version"],"verification":verification,"evidence":[evidence()]})]).await
    }
    async fn history(&self, n: &Value) -> Value {
        self.get(&format!(
            "/nodes/{}/history?limit=200",
            n["id"].as_str().unwrap()
        ))
        .await
    }
}
fn id(n: &Value) -> Uuid {
    Uuid::parse_str(n["id"].as_str().unwrap()).unwrap()
}
fn patch(n: &Value, p: Value) -> Value {
    json!({"op":"update_node","id":n["id"],"expected_version":n["version"],"patch":p})
}
fn summary(n: &Value, source_revision: i64, text: Value) -> Value {
    json!({"op":"set_summary","id":n["id"],"expected_version":n["version"],"source_revision":source_revision,"summary":text})
}
fn revision(n: &Value) -> i64 {
    n["content_revision"].as_i64().unwrap()
}
fn evidence() -> Value {
    json!({"source_id":"evidence","kind":"url","url":"https://example.test/evidence","observed_at":"2026-10-30T00:00:00Z","retained":false})
}
fn has(page: &Value, n: &Value) -> bool {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["id"] == n["id"])
}

#[tokio::test]
async fn both_kinds_have_canonical_content_type_provenance_and_independent_verification() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let writer = api.writer("writer").await;
        let n = api
            .create_as(
                &writer,
                kind,
                "Shared finding",
                json!({"provenance":[evidence()]}),
            )
            .await;
        assert_eq!(n["content_type"], "decision");
        assert!(
            n.get("knowledge_type").is_none(),
            "output uses the shared canonical field"
        );
        assert_eq!(n["verification"], "unverified");
        assert_eq!(n["provenance"], json!([evidence()]));
        let(status,error)=api.mutation_as(&writer,"writer-verify",vec![json!({"op":"verify_node","id":n["id"],"expected_version":n["version"],"verification":"verified","evidence":[evidence()]})]).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "verification is privileged for both kinds: {error}"
        );
        api.verify(&n, "verified").await;
        let verified = api.node(id(&n)).await;
        assert_eq!(verified["verification"], "verified");
        assert_eq!(verified["last_verified_by"], "shared-author");
        assert_eq!(
            verified["task_state"], n["task_state"],
            "verification never completes or activates a task"
        );
        let (status, error) = api
            .mutation_as(
                &writer,
                "protected-edit",
                vec![patch(&verified, json!({"body":"protected overwrite"}))],
            )
            .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "protected tasks and knowledge require administrator correction: {error}"
        );
        api.change(vec![patch(
            &verified,
            json!({"body":"Administrator correction"}),
        )])
        .await;
        let corrected = api.node(id(&n)).await;
        assert_eq!(corrected["verification"], "unverified");
        assert!(corrected["last_verified_by"].is_null());
        assert_eq!(corrected["provenance"], verified["provenance"]);
        let mut legacy = json!({"knowledge_type":"fact"});
        // Use the legacy alias without the canonical key; both keys together are ambiguous.
        let id = Uuid::new_v4();
        legacy.as_object_mut().unwrap().extend(
            json!({"op":"create_node","id":id,"kind":kind,"title":"Legacy type alias"})
                .as_object()
                .unwrap()
                .clone(),
        );
        api.change(vec![legacy]).await;
        let alias = api.node(id).await;
        assert_eq!(alias["content_type"], "fact");
        assert!(alias.get("knowledge_type").is_none());
        let ambiguous = Uuid::new_v4();
        let(status,error)=api.mutation("ambiguous-type-alias",vec![json!({"op":"create_node","id":ambiguous,"kind":kind,"title":"Ambiguous category","content_type":"fact","knowledge_type":"note"})]).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "canonical and legacy aliases cannot disagree: {error}"
        );
        assert_eq!(
            api.call("GET", &format!("/nodes/{ambiguous}"), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn shared_summaries_pin_current_revision_preserve_noops_and_invalidate_on_change() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let n = api.create(kind, "Summary source").await;
        let base = revision(&n);
        let operation = summary(&n, base, json!("A derived concise description"));
        let (status, receipt) = api.mutation("summarize", vec![operation.clone()]).await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
        let summarized = api.node(id(&n)).await;
        assert_eq!(summarized["summary"], "A derived concise description");
        assert_eq!(summarized["summary_of_revision"], base);
        assert!(revision(&summarized) > base);
        let exact = api
            .get(&format!(
                "/nodes/{}/revisions/{}",
                n["id"].as_str().unwrap(),
                revision(&summarized)
            ))
            .await;
        assert_eq!(exact["node"]["summary_of_revision"], base);
        assert_eq!(
            api.mutation("summarize", vec![operation]).await,
            (StatusCode::OK, receipt)
        );
        api.change(vec![patch(&summarized, json!({"body":summarized["body"]}))])
            .await;
        assert_eq!(
            api.node(id(&n)).await,
            summarized,
            "a no-op does not invalidate its summary"
        );
        let before = api.history(&n).await;
        let (status, error) = api
            .mutation(
                "oversized-summary",
                vec![summary(
                    &summarized,
                    revision(&summarized),
                    json!("é".repeat(4097)),
                )],
            )
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "summary limit counts UTF-8 bytes: {error}"
        );
        let (status, error) = api
            .mutation(
                "stale-summary-source",
                vec![summary(
                    &summarized,
                    base,
                    json!("Must not summarize a stale base"),
                )],
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(api.history(&n).await, before);
        api.change(vec![patch(
            &summarized,
            json!({"body":"Changed source body"}),
        )])
        .await;
        let changed = api.node(id(&n)).await;
        assert!(changed["summary"].is_null());
        assert!(changed["summary_of_revision"].is_null());
        api.change(vec![json!({"op":"restore_node","id":n["id"],"expected_version":changed["version"],"revision":revision(&summarized)})]).await;
        let restored = api.node(id(&n)).await;
        assert_eq!(restored["body"], n["body"]);
        assert!(
            restored["summary"].is_null(),
            "restoration is a new source change, not resurrection of a derived summary"
        );
        assert!(restored["summary_of_revision"].is_null());
        assert_eq!(
            api.get(&format!(
                "/nodes/{}/revisions/{}",
                n["id"].as_str().unwrap(),
                revision(&summarized)
            ))
            .await,
            exact,
            "old derived text and its citation remain exact history"
        );
        api.change(vec![summary(
            &restored,
            revision(&restored),
            json!("Fresh summary"),
        )])
        .await;
        let fresh = api.node(id(&n)).await;
        api.change(vec![summary(&fresh, revision(&fresh), Value::Null)])
            .await;
        let cleared = api.node(id(&n)).await;
        assert!(cleared["summary"].is_null());
        assert!(cleared["summary_of_revision"].is_null());
    }
}

#[tokio::test]
async fn verification_and_owned_link_changes_invalidate_shared_summaries() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let n = api.create(kind, "Summary invalidation").await;
        api.change(vec![summary(
            &n,
            revision(&n),
            json!("Before verification"),
        )])
        .await;
        let n = api.node(id(&n)).await;
        api.verify(&n, "verified").await;
        let verified = api.node(id(&n)).await;
        assert!(verified["summary"].is_null());
        assert!(verified["summary_of_revision"].is_null());
        api.change(vec![summary(
            &verified,
            revision(&verified),
            json!("Before link edit"),
        )])
        .await;
        let summarized = api.node(id(&n)).await;
        let target = api.create("knowledge", "Evidence target").await;
        api.change(vec![json!({"op":"add_edge","source":n["id"],"target":target["id"],"edge_type":"references"})]).await;
        let linked = api.node(id(&n)).await;
        assert!(revision(&linked) > revision(&summarized));
        assert!(linked["summary"].is_null());
        assert!(linked["summary_of_revision"].is_null());
    }
}

#[tokio::test]
async fn dates_are_nullable_ordered_normalized_and_do_not_schedule_tasks() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let n = api.create(kind, "Descriptive dates").await;
        api.change(vec![patch(&n,json!({"valid_from":"2030-01-01T03:00:00.123456789+03:00","valid_until":"2030-01-02T00:00:00Z","review_at":"2030-01-01T00:00:00Z"}))]).await;
        let dated = api.node(id(&n)).await;
        assert_eq!(dated["valid_from"], "2030-01-01T00:00:00.123456Z");
        assert_eq!(dated["task_state"], n["task_state"]);
        if kind == "task" {
            assert!(
                has(&api.get("/ready").await, &dated),
                "future descriptive validity is not a scheduling gate"
            );
        }
        let before = api.history(&n).await;
        let(status,error)=api.mutation("reversed-dates",vec![patch(&dated,json!({"valid_from":"2030-01-03T00:00:00Z","valid_until":"2030-01-01T00:00:00Z"}))]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(api.node(id(&n)).await, dated);
        assert_eq!(api.history(&n).await, before);
        let (status, error) = api
            .mutation(
                "timezone-required",
                vec![patch(&dated, json!({"review_at":"2030-01-01"}))],
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        api.change(vec![patch(
            &dated,
            json!({"valid_from":null,"valid_until":null,"review_at":null}),
        )])
        .await;
        let cleared = api.node(id(&n)).await;
        for field in ["valid_from", "valid_until", "review_at"] {
            assert!(
                cleared[field].is_null(),
                "explicit null clears {field} on {kind}"
            );
        }
        let old = api
            .get(&format!(
                "/nodes/{}/revisions/{}",
                n["id"].as_str().unwrap(),
                revision(&dated)
            ))
            .await;
        assert_eq!(
            old["node"]["valid_from"], dated["valid_from"],
            "date clearing cannot alter a retained revision"
        );
    }
}

#[tokio::test]
async fn shared_list_search_apply_kind_validity_review_and_workspace_filters() {
    let api = Api::new().await;
    let mut inside = vec![];
    for kind in ["task", "knowledge"] {
        for (title, dates) in [
            (
                "Inside",
                json!({"valid_from":"2030-01-01T00:00:00Z","valid_until":"2030-01-02T00:00:00Z","review_at":"2030-01-01T00:00:00Z"}),
            ),
            (
                "Expired",
                json!({"valid_from":"2029-01-01T00:00:00Z","valid_until":"2030-01-01T00:00:00Z","review_at":"2029-12-31T00:00:00Z"}),
            ),
            (
                "Future",
                json!({"valid_from":"2030-01-02T00:00:00Z","review_at":"2030-01-02T00:00:00Z"}),
            ),
            ("Unbounded", json!({})),
        ] {
            let n = api.create_as(&api.token, kind, title, dates).await;
            if title == "Inside" {
                inside.push(n);
            }
        }
    }
    assert_eq!(
        api.get("/search?q=sharedneedle").await["items"]
            .as_array()
            .unwrap()
            .len(),
        8,
        "search defaults to both kinds"
    );
    for route in ["/nodes?", "/search?q=sharedneedle&"] {
        assert_eq!(
            api.get(&format!("{route}valid_at=2030-01-01T00:00:00Z"))
                .await["items"]
                .as_array()
                .unwrap()
                .len(),
            4,
            "validity is half-open and null bounds unbounded"
        );
        assert_eq!(
            api.get(&format!("{route}review_due_at=2030-01-01T00:00:00Z"))
                .await["items"]
                .as_array()
                .unwrap()
                .len(),
            4,
            "review filter includes exact boundary and excludes null review_at"
        );
        let filters = "valid_at=2030-01-01T00:00:00Z&review_due_at=2030-01-01T00:00:00Z";
        let page = api.get(&format!("{route}{filters}")).await;
        assert_eq!(page["items"].as_array().unwrap().len(), 2);
        assert!(inside.iter().all(|n| has(&page, n)));
        let tasks = api.get(&format!("{route}{filters}&kind=task")).await;
        assert_eq!(tasks["items"].as_array().unwrap().len(), 1);
        assert_eq!(tasks["items"][0]["kind"], "task");
        let first = api.get(&format!("{route}{filters}&limit=1")).await;
        let cursor = first["next_cursor"].as_str().unwrap();
        let next = api
            .get(&format!("{route}{filters}&limit=1&cursor={cursor}"))
            .await;
        assert_eq!(next["items"].as_array().unwrap().len(), 1);
        assert_ne!(first["items"][0]["id"], next["items"][0]["id"]);
        assert!(next["next_cursor"].is_null());
    }
    let foreign = Api::new().await;
    assert!(
        foreign.get("/nodes?valid_at=2030-01-01T00:00:00Z").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        foreign
            .get("/search?q=sharedneedle&review_due_at=2030-01-01T00:00:00Z")
            .await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for path in [
        "/nodes?valid_at=2030-01-01",
        "/search?q=sharedneedle&review_due_at=bad",
        "/search?q=sharedneedle&kind=unknown",
    ] {
        let (status, error) = api.call("GET", path, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    }
}

#[tokio::test]
async fn retirement_and_dispute_filters_are_shared_without_conflating_completion() {
    let api = Api::new().await;
    let mut tasks = vec![];
    for kind in ["task", "knowledge"] {
        let active = api.create(kind, "Active finding").await;
        let retired = api.create(kind, "Retired finding").await;
        let disputed = api.create(kind, "Disputed finding").await;
        api.change(vec![patch(
            &retired,
            json!({"retired":true,"retirement_reason":"Obsolete planning"}),
        )])
        .await;
        api.verify(&disputed, "disputed").await;
        if kind == "task" {
            tasks = vec![active, retired, disputed];
        }
    }
    assert_eq!(
        api.get("/nodes").await["items"].as_array().unwrap().len(),
        4,
        "list hides retired but includes disputed for either kind"
    );
    assert_eq!(
        api.get("/nodes?include_retired=true").await["items"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    for (flags, count) in [
        ("", 2),
        ("&include_disputed=true", 4),
        ("&include_retired=true", 4),
        ("&include_retired=true&include_disputed=true", 6),
    ] {
        assert_eq!(
            api.get(&format!("/search?q=sharedneedle{flags}")).await["items"]
                .as_array()
                .unwrap()
                .len(),
            count
        );
    }
    let ready = api.get("/ready").await;
    assert!(has(&ready, &tasks[0]));
    assert!(!has(&ready, &tasks[1]));
    assert!(
        has(&ready, &tasks[2]),
        "disputed evidence is not an implicit execution-state transition"
    );
    let retired = api.node(id(&tasks[1])).await;
    assert_eq!(retired["task_state"], "active");
    assert_eq!(retired["retired"], true);
    assert_eq!(retired["deleted"], false);
    let child = api.create("task", "Retired ancestor child").await;
    let parent = api.create("task", "Retired ancestor").await;
    api.change(vec![json!({"op":"set_parent","id":child["id"],"expected_version":child["version"],"parent_id":parent["id"]})]).await;
    let parent = api.node(id(&parent)).await;
    api.change(vec![patch(
        &parent,
        json!({"retired":true,"retirement_reason":"Container archived"}),
    )])
    .await;
    assert!(
        !has(&api.get("/ready").await, &child),
        "retired ancestors withhold descendants"
    );
}

#[tokio::test]
async fn successor_relationships_accept_both_kinds_and_retain_forward_chains() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let old = api.create(kind, "Old finding").await;
        let successor = api
            .create(
                if kind == "task" { "knowledge" } else { "task" },
                "Successor",
            )
            .await;
        let add=api.change(vec![json!({"op":"add_edge","source":successor["id"],"target":old["id"],"edge_type":"supersedes"})]).await;
        let edge = add["results"][0].clone();
        api.change(vec![patch(
            &old,
            json!({"retired":true,"retirement_reason":"Replaced","successor_id":successor["id"]}),
        )])
        .await;
        let retired = api.node(id(&old)).await;
        assert_eq!(retired["successor_id"], successor["id"]);
        assert_eq!(retired["retired"], true);
        assert_eq!(retired["task_state"], old["task_state"]);
        let history = api.history(&old).await;
        let (status, error) = api
            .mutation(
                "orphan-successor",
                vec![
                    json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]}),
                ],
            )
            .await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
            "named successor cannot lose its supersedes edge: {error}"
        );
        assert_eq!(api.history(&old).await, history);
        let successor = api.node(id(&successor)).await;
        let head = api.create(kind, "New head").await;
        api.change(vec![json!({"op":"add_edge","source":head["id"],"target":successor["id"],"edge_type":"supersedes"})]).await;
        api.change(vec![patch(
            &successor,
            json!({"retired":true,"retirement_reason":"Forwarded again","successor_id":head["id"]}),
        )])
        .await;
        assert_eq!(api.node(id(&old)).await["successor_id"], successor["id"]);
        assert_eq!(
            api.node(id(&successor)).await["successor_id"],
            head["id"],
            "a successor may itself retire with a forward chain"
        );
    }
}

#[tokio::test]
async fn invalid_successors_and_deletions_roll_back_atomically() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let foreign = Api::new().await;
        let old = api.create(kind, "Atomic successor source").await;
        let candidate = api.create(kind, "No relationship yet").await;
        let probe = Uuid::new_v4();
        let before = api.history(&old).await;
        let(status,error)=api.mutation("missing-successor-edge",vec![json!({"op":"create_node","id":probe,"kind":"task","title":"Must roll back"}),patch(&old,json!({"retired":true,"retirement_reason":"Missing link","successor_id":candidate["id"]}))]).await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
            "{error}"
        );
        assert_eq!(
            api.call("GET", &format!("/nodes/{probe}"), None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(api.node(id(&old)).await, old);
        assert_eq!(api.history(&old).await, before);
        let foreign_node = foreign.create(kind, "Private successor").await;
        let(status,foreign_error)=api.mutation("foreign-successor",vec![patch(&old,json!({"retired":true,"retirement_reason":"Wrong workspace","successor_id":foreign_node["id"]}))]).await;
        assert!(status.is_client_error());
        let(status,unknown_error)=api.mutation("unknown-successor",vec![patch(&old,json!({"retired":true,"retirement_reason":"Wrong workspace","successor_id":Uuid::new_v4()}))]).await;
        assert!(status.is_client_error());
        assert_eq!(
            foreign_error, unknown_error,
            "foreign identity must not expose private successor existence"
        );
        let added=api.change(vec![json!({"op":"add_edge","source":candidate["id"],"target":old["id"],"edge_type":"supersedes"})]).await;
        let edge = added["results"][0].clone();
        let candidate_now = api.node(id(&candidate)).await;
        api.change(vec![patch(&old,json!({"retired":true,"retirement_reason":"Named successor","successor_id":candidate["id"]}))]).await;
        let(status,error)=api.mutation("delete-successor",vec![json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]}),json!({"op":"delete_node","id":candidate["id"],"expected_version":candidate_now["version"].as_i64().unwrap()+1})]).await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
            "cannot delete referenced successor by removing its relation in the same batch: {error}"
        );
        assert_eq!(api.node(id(&candidate)).await["deleted"], false);
        assert_eq!(api.node(id(&old)).await["successor_id"], candidate["id"]);
        let retired = api.node(id(&old)).await;
        api.change(vec![patch(&retired,json!({"successor_id":null})),json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]}),json!({"op":"delete_node","id":candidate["id"],"expected_version":candidate_now["version"].as_i64().unwrap()+1})]).await;
        assert!(
            api.node(id(&old)).await["successor_id"].is_null(),
            "explicit nullable pointer correction permits removing its dependency"
        );
        assert_eq!(api.node(id(&candidate)).await["deleted"], true);
    }
}

#[tokio::test]
async fn shared_fields_keep_task_workspace_writes_and_knowledge_author_permissions() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let alice = api.writer("alice").await;
        let bob = api.writer("bob").await;
        let original = api
            .create_as(&alice, kind, "Shared field permissions", json!({}))
            .await;
        let (status, result) = api
            .mutation_as(
                &bob,
                "other-writer",
                vec![patch(&original, json!({"content_type":"procedure"}))],
            )
            .await;
        assert_eq!(
            status,
            if kind == "task" {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "shared fields retain kind-specific edit ownership: {result}"
        );
        let current = api.node(id(&original)).await;
        assert_eq!(current["created_by"], "alice");
        let (status, error) = api
            .mutation_as(
                &alice,
                "author-retirement",
                vec![patch(
                    &current,
                    json!({"retired":true,"retirement_reason":"Author archive"}),
                )],
            )
            .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "retirement needs admin for both kinds: {error}"
        );
        let(status,error)=api.mutation_as(&alice,"privileged-restore",vec![json!({"op":"restore_node","id":current["id"],"expected_version":current["version"],"revision":original["content_revision"],"include_lifecycle":true})]).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "lifecycle restoration is admin-only even when its fields happen to be unchanged: {error}"
        );
        assert_eq!(
            api.node(id(&original)).await,
            current,
            "denied common-field operations do not mutate state"
        );
    }
}

#[tokio::test]
async fn named_successor_cycles_are_refused_without_banning_informational_edge_cycles() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let a = api.create(kind, "Successor A").await;
        let b = api.create(kind, "Successor B").await;
        api.change(vec![
            json!({"op":"add_edge","source":a["id"],"target":b["id"],"edge_type":"supersedes"}),
            json!({"op":"add_edge","source":b["id"],"target":a["id"],"edge_type":"supersedes"}),
        ])
        .await;
        let a = api.node(id(&a)).await;
        let b = api.node(id(&b)).await;
        api.change(vec![patch(
            &a,
            json!({"retired":true,"retirement_reason":"Forward to B","successor_id":b["id"]}),
        )])
        .await;
        let before = api.history(&b).await;
        let(status,error)=api.mutation("named-successor-cycle",vec![patch(&b,json!({"retired":true,"retirement_reason":"Would cycle back","successor_id":a["id"]}))]).await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
            "A -> B -> A named lifecycle chain is invalid: {error}"
        );
        assert_eq!(api.node(id(&b)).await, b);
        assert_eq!(api.history(&b).await, before);
    }
}

#[tokio::test]
async fn summary_draft_preview_is_complete_rollback_only_and_matches_application() {
    for kind in ["task", "knowledge"] {
        let api = Api::new().await;
        let n = api.create(kind, "Summary proposal source").await;
        let before = api.history(&n).await;
        let(status,draft)=api.call("POST","/changesets",Some(json!({"name":"Review derived summary","idempotency_key":"summary-proposal","operations":[summary(&n,revision(&n),json!("A reviewed derived description"))]}))).await;
        assert_eq!(status, StatusCode::OK, "{draft}");
        let path = format!("/changesets/{}", draft["id"].as_str().unwrap());
        let (status, validated) = api.call("POST", &format!("{path}/validate"), None).await;
        assert_eq!(status, StatusCode::OK, "{validated}");
        let preview = validated["preview"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == n["id"])
            .unwrap();
        assert_eq!(preview["before"]["summary"], Value::Null);
        assert_eq!(
            preview["after"]["summary"],
            "A reviewed derived description"
        );
        assert_eq!(
            preview["after"]["summary_of_revision"],
            n["content_revision"]
        );
        assert_eq!(api.node(id(&n)).await, n);
        assert_eq!(
            api.history(&n).await,
            before,
            "preview must not publish summary revisions"
        );
        let (status, applied) = api
            .call(
                "POST",
                &format!("{path}/apply"),
                Some(json!({"idempotency_key":"summary-apply"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{applied}");
        let current = api.node(id(&n)).await;
        for field in [
            "summary",
            "summary_of_revision",
            "version",
            "content_revision",
        ] {
            assert_eq!(
                current[field], preview["after"][field],
                "applied {field} agrees with reviewed mutation"
            );
        }
        assert_eq!(
            api.call(
                "POST",
                &format!("{path}/apply"),
                Some(json!({"idempotency_key":"summary-apply"}))
            )
            .await,
            (StatusCode::OK, applied)
        );
    }
}

#[tokio::test]
async fn protected_tasks_require_admin_for_both_structural_endpoints_and_parent_membership() {
    let api = Api::new().await;
    let writer = api.writer("writer").await;
    let source = api.create("task", "Structural source").await;
    let target = api.create("task", "Protected target").await;
    let parent = api.create("task", "Candidate parent").await;
    api.verify(&target, "verified").await;
    let protected = api.node(id(&target)).await;
    let add =
        json!({"op":"add_edge","source":source["id"],"target":target["id"],"edge_type":"blocks"});
    let before = api.history(&source).await;
    let (status, error) = api
        .mutation_as(&writer, "protected-target-add", vec![add.clone()])
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "source write access cannot bypass a protected structural target: {error}"
    );
    assert_eq!(api.history(&source).await, before);
    let(status,error)=api.mutation_as(&writer,"protected-parent-assignment",vec![json!({"op":"set_parent","id":target["id"],"expected_version":protected["version"],"parent_id":parent["id"]})]).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    let linked = api.change(vec![add]).await;
    let edge = linked["results"][0].clone();
    let current = api.node(id(&target)).await;
    assert_eq!(
        current["verification"], "unverified",
        "admin structural edit invalidates target verification"
    );
    api.verify(&current, "verified").await;
    let (status, error) = api
        .mutation_as(
            &writer,
            "protected-target-remove",
            vec![json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]})],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "removal must check protected target too: {error}"
    );
    api.change(vec![
        json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]}),
    ])
    .await;
    let child = api.node(id(&target)).await;
    api.verify(&parent, "verified").await;
    let(status,error)=api.mutation_as(&writer,"protected-parent-target",vec![json!({"op":"set_parent","id":child["id"],"expected_version":child["version"],"parent_id":parent["id"]})]).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "reparenting checks the protected parent: {error}"
    );
    api.change(vec![json!({"op":"set_parent","id":child["id"],"expected_version":child["version"],"parent_id":parent["id"]})]).await;
    assert_eq!(api.node(id(&parent)).await["verification"], "unverified");
}

#[tokio::test]
async fn default_task_restore_cannot_bypass_protected_neighbor_authority() {
    let api = Api::new().await;
    let writer = api.writer("writer").await;
    let source = api.create("task", "Historical prerequisite").await;
    let target = api.create("task", "Unprotected restored task").await;
    let added=api.change(vec![json!({"op":"add_edge","source":source["id"],"target":target["id"],"edge_type":"blocks"})]).await;
    let edge = &added["results"][0];
    let historical = api.node(id(&target)).await;
    api.change(vec![
        json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]}),
    ])
    .await;
    let source = api.node(id(&source)).await;
    api.verify(&source, "verified").await;
    let current = api.node(id(&target)).await;
    let history = api.history(&current).await;
    let restore = json!({"op":"restore_node","id":current["id"],"expected_version":current["version"],"revision":historical["content_revision"]});
    let (status, error) = api
        .mutation_as(&writer, "restore-protected-neighbor", vec![restore.clone()])
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "ordinary default restore cannot edit a protected prerequisite by restoring its incoming link: {error}"
    );
    assert_eq!(api.node(id(&target)).await, current);
    assert_eq!(api.history(&current).await, history);
    api.change(vec![restore]).await;
    assert_eq!(api.node(id(&source)).await["verification"], "unverified");
}
