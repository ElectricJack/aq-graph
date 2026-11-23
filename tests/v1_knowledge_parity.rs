//! Knowledge-core assertions translated from agent-queue2 tests, through HTTP only.
//! See docs/compatibility/knowledge.md for source names, deliberate boundary changes,
//! and requirements that these tests add to the original aq-graph baseline.
//! No v1 modules, database tables, daemon, provider, or live checkout are used.
use aq_graph::{service, store::Store};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Api {
    app: Router,
    workspace: Uuid,
    admin: String,
}
impl Api {
    async fn new() -> Self {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("knowledge parity tests require AQ_GRAPH_TEST_DATABASE_URL pointing to disposable PostgreSQL");
        let store = Store::connect(&url, 4)
            .await
            .expect("connect isolated PostgreSQL");
        store.migrate().await.expect("migrate isolated PostgreSQL");
        let workspace = Uuid::new_v4();
        let admin = format!("admin-{}", Uuid::new_v4());
        store
            .bootstrap(workspace, "operator", &admin)
            .await
            .expect("bootstrap test workspace");
        Self {
            app: service::router(store),
            workspace,
            admin,
        }
    }
    fn as_actor<'a>(&'a self, token: &'a str) -> Actor<'a> {
        Actor { api: self, token }
    }
    async fn writer(&self, actor: &str) -> String {
        let token = format!("writer-{}", Uuid::new_v4());
        let (status, result) = self
            .as_actor(&self.admin)
            .call(
                Method::POST,
                "/credentials",
                Some(json!({"actor_id":actor,"token":token,"capability":"write"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        token
    }
    async fn history(&self, id: Uuid) -> Value {
        let (status, result) = self
            .as_actor(&self.admin)
            .call(Method::GET, &format!("/nodes/{id}/history?limit=200"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        result
    }
}
struct Actor<'a> {
    api: &'a Api,
    token: &'a str,
}
impl Actor<'_> {
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(format!("/v1/workspaces/{}{path}", self.api.workspace))
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
            .unwrap();
        let response = self.api.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), service::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).expect("structured JSON contract response"),
        )
    }
    async fn mutation(
        &self,
        key: &str,
        reason: Option<&str>,
        operations: Vec<Value>,
    ) -> (StatusCode, Value) {
        self.call(
            Method::POST,
            "/mutations",
            Some(json!({"idempotency_key":key,"reason":reason,"operations":operations})),
        )
        .await
    }
    async fn create(&self, title: &str) -> Value {
        let id = Uuid::new_v4();
        let (status,result)=self.mutation(&Uuid::new_v4().to_string(),None,vec![json!({"op":"create_node","id":id,"kind":"knowledge","knowledge_type":"decision","title":title,"body":"Original\r\nα evidence","provenance":[]})]).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        self.node(id).await
    }
    async fn node(&self, id: Uuid) -> Value {
        let (status, node) = self.call(Method::GET, &format!("/nodes/{id}"), None).await;
        assert_eq!(status, StatusCode::OK, "{node}");
        node
    }
    async fn verify(&self, node: &Value, key: &str, verification: &str) -> (StatusCode, Value) {
        self.mutation(key,Some("Compared the named source with this exact revision"),vec![json!({"op":"verify_knowledge","id":node["id"],"expected_version":node["version"],"verification":verification,"evidence":[source("evidence")]})]).await
    }
}
fn source(id: &str) -> Value {
    json!({"source_id":id,"kind":"url","url":"https://example.test/evidence","observed_at":"2026-10-30T00:00:00Z","retained":false})
}
fn id(node: &Value) -> Uuid {
    Uuid::parse_str(node["id"].as_str().unwrap()).unwrap()
}
fn patch(node: &Value, patch: Value) -> Value {
    json!({"op":"update_node","id":node["id"],"expected_version":node["version"],"patch":patch})
}
fn revisions(history: &Value) -> Vec<&Value> {
    history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["category"] == "revision")
        .collect()
}

// v1 tests/test_knowledge_authorization.py::test_workers_read_project_knowledge_but_only_edit_own
#[tokio::test]
async fn knowledge_author_owns_unverified_edits_and_admin_can_correct() {
    let api = Api::new().await;
    let alice = api.writer("alice").await;
    let author = api.as_actor(&alice);
    let bob = api.writer("bob").await;
    let other_writer = api.as_actor(&bob);
    let original = author.create("Author finding").await;
    let record = id(&original);
    assert_eq!(
        original["created_by"], "alice",
        "author comes from authenticated credential"
    );
    assert_eq!(
        other_writer.node(record).await,
        original,
        "workspace writers may read each other's findings"
    );
    let before = api.history(record).await;
    let (status, error) = other_writer
        .mutation(
            "hijack",
            None,
            vec![patch(&original, json!({"body":"hijack"}))],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert_eq!(
        api.history(record).await,
        before,
        "denied edit leaves no history"
    );
    let (status, result) = author
        .mutation(
            "author",
            None,
            vec![patch(&original, json!({"body":"author correction"}))],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let current = author.node(record).await;
    let (status, result) = api
        .as_actor(&api.admin)
        .mutation(
            "operator",
            Some("Corrected after review"),
            vec![patch(&current, json!({"body":"operator correction"}))],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        author.node(record).await["created_by"],
        "alice",
        "correction cannot transfer authorship"
    );
    let history = api.history(record).await;
    assert_eq!(
        revisions(&history)
            .iter()
            .map(|e| e["actor_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["alice", "alice", "operator"]
    );
}

// v1 tests/test_knowledge_service.py::test_snapshot_limits_before_sql (verification case)
// v1 tests/test_knowledge_authorization.py::test_protected_mutations_require_supervisor_and_reset_verification
#[tokio::test]
async fn ordinary_creation_and_patch_cannot_self_assert_verified_or_disputed() {
    let api = Api::new().await;
    let alice = api.writer("alice").await;
    let author = api.as_actor(&alice);
    for state in ["verified", "disputed"] {
        let record = Uuid::new_v4();
        let (status,error)=author.mutation(&format!("create-{state}"),None,vec![json!({"op":"create_node","id":record,"kind":"knowledge","title":"Self asserted","verification":state,"provenance":[source("source")]})]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(
            author
                .call(Method::GET, &format!("/nodes/{record}"), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let original = author.create("Unverified assertion").await;
        let (status, error) = author
            .mutation(
                &format!("patch-{state}"),
                None,
                vec![patch(&original, json!({"verification":state}))],
            )
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "dedicated verification operation required: {error}"
        );
        assert_eq!(
            author.node(id(&original)).await["verification"],
            "unverified"
        );
    }
}

// v1 tests/test_knowledge_authority.py::test_dispute_requires_privilege_and_named_evidence
// v1 tests/test_record_schema.py::test_valid_verification_retirement_and_sources_at_database_boundary
#[tokio::test]
async fn verification_requires_admin_named_evidence_reason_and_server_attribution() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    let alice = api.writer("alice").await;
    let author = api.as_actor(&alice);
    let original = author.create("Evidence-backed assertion").await;
    let (status, error) = author.verify(&original, "worker-verify", "verified").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    for (key, reason, evidence) in [
        ("empty", Some("Compared source"), json!([])),
        ("reason", Some("   "), json!([source("one")])),
    ] {
        let (status,error)=administrator.mutation(key,reason,vec![json!({"op":"verify_knowledge","id":original["id"],"expected_version":original["version"],"verification":"verified","evidence":evidence})]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    }
    let (status, result) = administrator.verify(&original, "verify", "verified").await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let current = author.node(id(&original)).await;
    assert_eq!(current["verification"], "verified");
    assert_eq!(current["last_verified_by"], "operator");
    chrono::DateTime::parse_from_rfc3339(
        current["last_verified_at"]
            .as_str()
            .expect("server verification timestamp"),
    )
    .unwrap();
    assert_eq!(current["provenance"], json!([source("evidence")]));
    assert_eq!(
        administrator
            .verify(&original, "verify", "verified")
            .await
            .1,
        result,
        "replay precedes stale expected version"
    );
    assert_eq!(
        revisions(&api.history(id(&original)).await).len(),
        2,
        "verification appends exactly one complete snapshot"
    );
}

// v1 tests/test_knowledge_authority.py::test_guarded_edit_resets_verification_and_revokes_grant
// v1 tests/test_knowledge_authorization.py::test_protected_mutations_require_supervisor_and_reset_verification
#[tokio::test]
async fn protected_content_requires_admin_and_correction_invalidates_verification() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    let alice = api.writer("alice").await;
    let author = api.as_actor(&alice);
    let original = author.create("Protected knowledge").await;
    let (status, result) = administrator.verify(&original, "verify", "verified").await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let verified = author.node(id(&original)).await;
    let (status, error) = author
        .mutation(
            "protected-edit",
            None,
            vec![patch(&verified, json!({"body":"worker overwrite"}))],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "even the author must propose protected corrections: {error}"
    );
    let (status, result) = administrator
        .mutation(
            "noop",
            None,
            vec![patch(&verified, json!({"body":verified["body"]}))],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        author.node(id(&original)).await,
        verified,
        "no-op must preserve verification"
    );
    let (status, result) = administrator
        .mutation(
            "correction",
            Some("Evidence no longer covers revised claim"),
            vec![patch(&verified, json!({"body":"changed assertion"}))],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let corrected = author.node(id(&original)).await;
    assert_verification_cleared(&corrected, &verified);
    let history = api.history(id(&original)).await;
    assert!(
        revisions(&history)
            .iter()
            .any(|e| e["data"]["verification"] == "verified"),
        "old verified evidence remains immutable: {history}"
    );
}

// v1 tests/test_knowledge_service.py::test_snapshot_limits_before_sql (sources cases)
// v1 tests/test_record_schema.py::test_invalid_source_field_types_and_duplicate_link_ids_are_rejected
#[tokio::test]
async fn provenance_descriptors_are_typed_unique_and_corrected_retry_is_possible() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    assert_invalid_provenance_retries(&administrator).await;
    let original = administrator.create("Typed provenance patch").await;
    let (status, error) = administrator
        .mutation(
            "malformed-update",
            None,
            vec![patch(&original, json!({"provenance":[{}]}))],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "updates must validate descriptor shapes too: {error}"
    );
    assert_eq!(
        administrator.node(id(&original)).await,
        original,
        "invalid provenance patch is atomic"
    );
    let (status, result) = administrator
        .mutation(
            "malformed-update",
            None,
            vec![patch(
                &original,
                json!({"provenance":[source("corrected")]}),
            )],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "corrected update reuses rejected key: {result}"
    );
}

// v1 tests/test_knowledge_service.py::test_exact_history_retire_restore_and_summary_invalidation
// v1 tests/test_record_schema.py::test_valid_verification_retirement_and_sources_at_database_boundary
#[tokio::test]
async fn retirement_requires_admin_reason_and_restore_preserves_exact_original_content() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    let alice = api.writer("alice").await;
    let author = api.as_actor(&alice);
    let original = author.create("Retained decision needle").await;
    let target = administrator.create("Retirement evidence target").await;
    let (status,linked)=author.mutation("retirement-link",None,vec![json!({"op":"add_edge","source":original["id"],"target":target["id"],"edge_type":"supports"})]).await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    let existing_edge = linked["results"][0].clone();
    // Relationship edits now share the same record revision/CAS sequence.
    let original = author.node(id(&original)).await;

    assert_retire_permissions(&administrator, &author, &original).await;
    let (status, result) = administrator
        .mutation(
            "retire",
            Some("Replaced after investigation"),
            vec![patch(
                &original,
                json!({"retired":true,"retirement_reason":"Replaced after investigation"}),
            )],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let retired = author.node(id(&original)).await;
    assert_retired_search(&author, &retired).await;
    assert_retired_writes_forbidden(&administrator, &retired, &original, &existing_edge).await;
    let (status,result)=administrator.mutation("restore",Some("Restore original finding"),vec![json!({"op":"restore_node","id":original["id"],"expected_version":retired["version"],"revision":original["content_revision"],"include_lifecycle":true})]).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let restored = author.node(id(&original)).await;
    assert_restored_content(&restored, &retired);
}

// v1 tests/test_knowledge_authority.py::test_dispute_requires_privilege_and_named_evidence
#[tokio::test]
async fn disputed_knowledge_is_hidden_by_default_search_but_explicitly_retrievable() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    let original = administrator.create("Disputed needle assertion").await;
    let (status, result) = administrator.verify(&original, "dispute", "disputed").await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        administrator.node(id(&original)).await["verification"],
        "disputed"
    );
    let (status, search) = administrator
        .call(Method::GET, "/search?q=needle", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{search}");
    assert_eq!(search["items"], json!([]));
    let (status, search) = administrator
        .call(Method::GET, "/search?q=needle&include_disputed=true", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{search}");
    assert_eq!(search["items"].as_array().unwrap().len(), 1);
}

// v1 tests/test_knowledge_proposals.py::test_worker_proposes_other_record_but_cannot_decide_even_with_grant
#[tokio::test]
async fn knowledge_proposal_can_target_another_author_but_only_admin_applies_it() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    let alice = api.writer("alice").await;
    let author = api.as_actor(&alice);
    let bob = api.writer("bob").await;
    let other_writer = api.as_actor(&bob);
    let original = author.create("Proposed correction").await;
    let (status,draft)=other_writer.call(Method::POST,"/changesets",Some(json!({"name":"Review correction","idempotency_key":"proposal","operations":[patch(&original,json!({"body":"Reviewed proposal"}))]}))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "writers can propose corrections to another author's finding: {draft}"
    );
    let draft_id = draft["id"].as_str().unwrap();
    let path = format!("/changesets/{draft_id}/apply");
    assert_eq!(
        author.node(id(&original)).await,
        original,
        "proposal is outside live state"
    );
    let (status, error) = other_writer
        .call(
            Method::POST,
            &path,
            Some(json!({"idempotency_key":"accept"})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "proposal author cannot approve own proposal: {error}"
    );
    let (status, result) = administrator
        .call(
            Method::POST,
            &path,
            Some(json!({"idempotency_key":"accept"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let corrected = author.node(id(&original)).await;
    assert_eq!(corrected["body"], "Reviewed proposal");
    assert_eq!(corrected["created_by"], "alice");
    assert_eq!(
        administrator
            .call(
                Method::POST,
                &path,
                Some(json!({"idempotency_key":"accept"}))
            )
            .await
            .1,
        result
    );
}

// v1 tests/test_knowledge_authorization.py::test_replay_rechecks_claim_project_and_grant
#[tokio::test]
async fn knowledge_receipt_replay_rechecks_live_capability_and_revocation() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    let alice = api.writer("alice").await;
    let author = api.as_actor(&alice);
    let record = Uuid::new_v4();
    let operation =
        json!({"op":"create_node","id":record,"kind":"knowledge","title":"Receipt finding"});
    let (status, committed) = author
        .mutation("create", None, vec![operation.clone()])
        .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    let (status, result) = administrator
        .call(
            Method::POST,
            "/credentials",
            Some(json!({"actor_id":"alice","token":alice,"capability":"read"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (status, error) = author
        .mutation("create", None, vec![operation.clone()])
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert_revoked_replay(&administrator, &author, operation).await;
    assert_eq!(revisions(&api.history(record).await).len(), 1);
}

// v1 tests/test_knowledge_authorization.py::test_protected_mutations_require_supervisor_and_reset_verification
// extended to link edits using src/records/auth.py::RecordAccess.editable and
// src/records/service.py::RecordService.mutate_links_on (the shared source guard).
#[tokio::test]
async fn knowledge_source_links_enforce_author_and_protected_revision_rules() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    let alice = api.writer("alice").await;
    let author = api.as_actor(&alice);
    let bob = api.writer("bob").await;
    let other_writer = api.as_actor(&bob);
    let original = author.create("Knowledge link source").await;
    let first = other_writer.create("First evidence target").await;
    let second = other_writer.create("Second evidence target").await;
    let add = |target: &Value| json!({"op":"add_edge","source":original["id"],"target":target["id"],"edge_type":"supports"});
    let (status, error) = other_writer
        .mutation("foreign-source", None, vec![add(&first)])
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "target authorship does not authorize changing another source: {error}"
    );
    let (status, linked) = author
        .mutation("author-source", None, vec![add(&first)])
        .await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    let current = author.node(id(&original)).await;
    assert_link_advances_revision(&current, &original);
    let edge = &linked["results"][0];
    let (status, result) = administrator
        .verify(&current, "protect-source", "verified")
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (status, error) = author
        .mutation("protected-source", None, vec![add(&second)])
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "protected links require administrator: {error}"
    );
    let (status, result) = administrator
        .mutation(
            "reviewed-source",
            Some("Revised supporting evidence"),
            vec![add(&second)],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let changed = author.node(id(&original)).await;
    assert_eq!(changed["verification"], "unverified");
    assert!(changed["version"].as_i64().unwrap() > current["version"].as_i64().unwrap());
    assert_protected_link_removal(&administrator, &author, &changed, edge).await;
}

// v1 tests/test_knowledge_authorization.py::test_replay_rechecks_claim_project_and_grant
// extends the same live-authority fence to privileged proposal decisions/verification.
#[tokio::test]
async fn privileged_knowledge_replays_recheck_admin_after_downgrade() {
    let api = Api::new().await;
    let administrator = api.as_actor(&api.admin);
    let reviewer = format!("reviewer-{}", Uuid::new_v4());
    let review_session = api.as_actor(&reviewer);
    let grant =
        |capability: &str| json!({"actor_id":"reviewer","token":reviewer,"capability":capability});
    let (status, result) = administrator
        .call(Method::POST, "/credentials", Some(grant("admin")))
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let original = administrator.create("Reviewed correction").await;
    let (status,draft)=review_session.call(Method::POST,"/changesets",Some(json!({"name":"Privileged proposal","idempotency_key":"draft","operations":[patch(&original,json!({"body":"Reviewed correction body"}))]}))).await;
    assert_eq!(status, StatusCode::OK, "{draft}");
    let path = format!("/changesets/{}/apply", draft["id"].as_str().unwrap());
    let (status, applied) = review_session
        .call(
            Method::POST,
            &path,
            Some(json!({"idempotency_key":"apply"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let corrected = administrator.node(id(&original)).await;
    let (status, verified) = review_session
        .verify(&corrected, "verify", "verified")
        .await;
    assert_eq!(status, StatusCode::OK, "{verified}");
    let before = api.history(id(&original)).await;
    let (status, result) = administrator
        .call(Method::POST, "/credentials", Some(grant("write")))
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (status, error) = review_session
        .call(
            Method::POST,
            &path,
            Some(json!({"idempotency_key":"apply"})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an old apply receipt is not continuing administrator authority: {error}"
    );
    let (status, error) = review_session
        .verify(&corrected, "verify", "verified")
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an old verification receipt is not continuing administrator authority: {error}"
    );
    assert_eq!(
        api.history(id(&original)).await,
        before,
        "failed replays cannot append history"
    );
}

fn assert_verification_cleared(corrected: &Value, verified: &Value) {
    assert_eq!(corrected["verification"], "unverified");
    assert!(corrected["last_verified_by"].is_null());
    assert!(corrected["last_verified_at"].is_null());
    assert_eq!(
        corrected["provenance"], verified["provenance"],
        "evidence descriptor is retained even when assertion is invalidated"
    );
}

async fn assert_invalid_provenance_retries(administrator: &Actor<'_>) {
    let invalid = vec![
        json!([{}]),
        json!([{"source_id":"task","kind":"task","task_id":123}]),
        json!([source("same"), source("same")]),
        json!([{"source_id":"git","kind":"git","repository":"https://example.test/repo.git","commit":"main","path":"decision.md"}]),
        json!([{"source_id":"url","kind":"url","url":"https://example.test/decision"}]),
        json!([{"source_id":"url","kind":"url","url":"file:///etc/passwd","observed_at":"2026-10-30T00:00:00Z","retained":false}]),
    ];
    for (i, provenance) in invalid.into_iter().enumerate() {
        let record = Uuid::new_v4();
        let key = format!("invalid-{i}");
        let mut op = json!({"op":"create_node","id":record,"kind":"knowledge","title":"Source shape","provenance":provenance});
        let (status, error) = administrator.mutation(&key, None, vec![op.clone()]).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "invalid source must fail before commitment: {error}"
        );
        assert_eq!(
            administrator
                .call(Method::GET, &format!("/nodes/{record}"), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        op["provenance"] = json!([source("valid")]);
        let (status, result) = administrator.mutation(&key, None, vec![op]).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "rejected payload does not burn receipt key: {result}"
        );
    }
}

async fn assert_retire_permissions(
    administrator: &Actor<'_>,
    author: &Actor<'_>,
    original: &Value,
) {
    for (key, reason) in [
        ("blank-active-reason", "   ".to_string()),
        ("oversized-active-reason", "x".repeat(4097)),
        ("active-reason", "Not yet retired".to_string()),
    ] {
        let (status, error) = administrator
            .mutation(
                key,
                None,
                vec![patch(original, json!({"retirement_reason":reason}))],
            )
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "retirement reasons are bounded and belong to retired state: {error}"
        );
    }
    let (status, error) = author
        .mutation(
            "retire-worker",
            Some("Obsolete"),
            vec![patch(
                original,
                json!({"retired":true,"retirement_reason":"Obsolete"}),
            )],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    let (status, error) = administrator
        .mutation(
            "retire-without-reason",
            None,
            vec![patch(original, json!({"retired":true}))],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
}

async fn assert_retired_search(author: &Actor<'_>, retired: &Value) {
    assert_eq!(retired["retired"], true);
    assert_eq!(retired["deleted"], false);
    assert_eq!(retired["retirement_reason"], "Replaced after investigation");
    assert_eq!(
        author.call(Method::GET, "/search?q=needle", None).await.1["items"],
        json!([])
    );
    assert_eq!(
        author
            .call(Method::GET, "/search?q=needle&include_retired=true", None)
            .await
            .1["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

async fn assert_retired_writes_forbidden(
    administrator: &Actor<'_>,
    retired: &Value,
    original: &Value,
    existing_edge: &Value,
) {
    let (status, error) = administrator
        .mutation(
            "edit-retired",
            None,
            vec![patch(retired, json!({"body":"rewrite retired"}))],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "restore retired records before editing: {error}"
    );
    let other = administrator.create("Second retirement target").await;
    let (status,error)=administrator.mutation("retired-add",None,vec![json!({"op":"add_edge","source":original["id"],"target":other["id"],"edge_type":"supports"})]).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "restore retired source before adding links: {error}"
    );
    let (status,error)=administrator.mutation("retired-remove",None,vec![json!({"op":"remove_edge","id":existing_edge["id"],"expected_version":existing_edge["version"]})]).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "restore retired source before removing links: {error}"
    );
}

fn assert_restored_content(restored: &Value, retired: &Value) {
    assert_eq!(restored["body"], "Original\r\nα evidence");
    assert_eq!(restored["retired"], false);
    assert!(restored["retirement_reason"].is_null());
    assert_eq!(restored["verification"], "unverified");
    assert!(
        restored["content_revision"].as_i64().unwrap()
            > retired["content_revision"].as_i64().unwrap()
    );
}

fn assert_link_advances_revision(current: &Value, original: &Value) {
    assert!(
        current["version"].as_i64().unwrap() > original["version"].as_i64().unwrap(),
        "unverified links advance the common CAS version"
    );
    assert!(
        current["content_revision"].as_i64().unwrap()
            > original["content_revision"].as_i64().unwrap(),
        "links belong to the same immutable revision history as planning content"
    );
}

async fn assert_protected_link_removal(
    administrator: &Actor<'_>,
    author: &Actor<'_>,
    changed: &Value,
    edge: &Value,
) {
    let (status, result) = administrator
        .verify(changed, "protect-removal", "verified")
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let remove = json!({"op":"remove_edge","id":edge["id"],"expected_version":edge["version"]});
    let (status, error) = author
        .mutation("protected-removal", None, vec![remove.clone()])
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    let (status, result) = administrator
        .mutation(
            "reviewed-removal",
            Some("Removed stale evidence link"),
            vec![remove],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(author.node(id(changed)).await["verification"], "unverified");
}

async fn assert_revoked_replay(administrator: &Actor<'_>, author: &Actor<'_>, operation: Value) {
    assert_eq!(
        administrator
            .call(Method::DELETE, "/credentials/alice", None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        author.mutation("create", None, vec![operation]).await.0,
        StatusCode::UNAUTHORIZED
    );
}
