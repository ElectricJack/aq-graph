use super::*;
use crate::{
    domain::{MutationResult, Principal},
    service,
    store::Store,
};
use serde_json::{Value, json};
use sqlx::Row;
use std::time::{Duration, Instant};

fn scope() -> Scope {
    Scope {
        workspace_id: Uuid::from_u128(1),
        stream_id: Uuid::from_u128(2),
        generation: "1".into(),
    }
}
fn start() -> Cursor {
    Cursor {
        scope: scope(),
        committed_sequence: "0".into(),
        group_id: None,
    }
}
fn request(cursor: Cursor, id: Uuid, now: i64) -> RegisterRequest {
    RegisterRequest {
        contract: Contract::V1,
        subscriber_id: Uuid::new_v4(),
        condition_digest: format!("sha256:{}", "a".repeat(64)),
        expected_scope: cursor.scope.clone(),
        from_cursor: Some(cursor),
        original_expires_at_ms: (now + 1000).to_string(),
        selectors: vec![Selector::Task {
            member_id: Uuid::from_u128(4),
            task_id: id,
            starting_logical_revision: "1".into(),
        }],
    }
}
#[test]
fn cursor_and_revision_encoding_is_canonical_closed_and_bounded() {
    let c = start();
    assert_eq!(Cursor::decode(&c.encode().unwrap()).unwrap(), c);
    for value in ["", "00", "01", "+1", "-1", "1.0", "9223372036854775808"] {
        assert!(decimal(value, false).is_err(), "{value}");
    }
    assert!(decimal("0", true).is_err());
    let mut invalid = c.clone();
    invalid.committed_sequence = "1".into();
    assert!(invalid.encode().is_err());
    let mut value = serde_json::to_value(&c).unwrap();
    value["extra"] = json!(true);
    assert!(Cursor::decode(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())).is_err());
    let reordered = URL_SAFE_NO_PAD.encode(format!(
        "{{\"scope\":{},\"group_id\":null,\"committed_sequence\":\"0\"}}",
        serde_json::to_string(&c.scope).unwrap()
    ));
    assert!(Cursor::decode(&reordered).is_err());
    assert!(Cursor::decode(&"x".repeat(1025)).is_err());
}
#[test]
fn requests_reject_foreign_scope_duplicates_unknown_fields_and_unbounded_lease() {
    let req = request(start(), Uuid::from_u128(3), 100);
    req.validate(scope().workspace_id, 100).unwrap();
    assert!(req.validate(Uuid::from_u128(4), 100).is_err());
    let mut bad = req.clone();
    bad.selectors.push(bad.selectors[0].clone());
    assert!(bad.validate(scope().workspace_id, 100).is_err());
    let mut bad = req.clone();
    bad.original_expires_at_ms = (100 + MAX_LEASE_MS + 1).to_string();
    assert!(bad.validate(scope().workspace_id, 100).is_err());
    let mut v = serde_json::to_value(&req).unwrap();
    v["extra"] = json!(true);
    assert!(RegisterRequest::decode(&serde_json::to_vec(&v).unwrap()).is_err());
    let raw = serde_json::to_string(&req).unwrap();
    assert!(
        RegisterRequest::decode(
            raw.replacen(
                "\"subscriber_id\":",
                "\"subscriber_id\":\"00000000-0000-0000-0000-000000000003\",\"subscriber_id\":",
                1
            )
            .as_bytes()
        )
        .is_err()
    );
    let snap = SnapshotRequest {
        contract: Contract::V1,
        targets: vec![SnapshotTarget::Task {
            task_id: Uuid::from_u128(3),
        }],
    };
    snap.validate().unwrap();
    let mut v = serde_json::to_value(&snap).unwrap();
    v["targets"][0]["revision"] = json!("1");
    assert!(SnapshotRequest::decode(&serde_json::to_vec(&v).unwrap()).is_err());
    assert!(raw_digest(&format!("sha256:{}", "A".repeat(64))).is_err());
    assert_eq!(
        digest(&json!({"b":2,"a":1})).unwrap(),
        digest(&json!({"a":1,"b":2})).unwrap()
    );
}
#[test]
fn terminal_proof_never_accepts_intermediate_or_changed_basis() {
    let before = NodeProjection {
        id: Uuid::from_u128(3),
        kind: NodeKind::Task,
        version: "1".into(),
        logical_revision: "1".into(),
        task_state: Some(TaskState::Active),
        deleted: false,
        retired: false,
    };
    let mut after = before.clone();
    after.version = "2".into();
    after.logical_revision = "2".into();
    after.task_state = Some(TaskState::Completed);
    let mut f = Fact::Node {
        object_id: before.id,
        before: Some(before),
        after: Some(after),
        mutation_count: "1".into(),
        transition_class: TransitionClass::LifecycleOnly,
    };
    assert!(f.terminal_from("1").unwrap().is_some());
    assert!(f.terminal_from("2").unwrap().is_none());
    if let Fact::Node { mutation_count, .. } = &mut f {
        *mutation_count = "2".into();
    }
    assert!(f.terminal_from("1").unwrap().is_none());
    if let Fact::Node {
        mutation_count,
        transition_class,
        ..
    } = &mut f
    {
        *mutation_count = "1".into();
        *transition_class = TransitionClass::RecordChanged;
    }
    assert!(f.terminal_from("1").unwrap().is_none());
    if let Fact::Node {
        transition_class,
        object_id,
        ..
    } = &mut f
    {
        *transition_class = TransitionClass::LifecycleOnly;
        *object_id = Uuid::from_u128(4);
    }
    assert!(f.terminal_from("1").unwrap().is_none());
}
#[test]
fn additive_schema_is_generated_closed_and_bounds_pages() {
    let schema = schema();
    assert_eq!(
        schema["$defs"]["RegisterRequest"]["additionalProperties"],
        false
    );
    assert_eq!(
        schema["$defs"]["SnapshotRequest"]["additionalProperties"],
        false
    );
    assert_eq!(schema["$defs"]["Cursor"]["additionalProperties"], false);
    assert_eq!(
        schema["$defs"]["Contract"]["enum"][0],
        "graph-committed-observation-1"
    );
    let frozen: Value = serde_json::from_str(include_str!(
        "../../protocol/graph-committed-observation-1/schema.json"
    ))
    .unwrap();
    assert_eq!(
        schema, frozen,
        "regenerate only this additive profile intentionally"
    );
    let read: SnapshotRead = serde_json::from_str(include_str!(
        "../../protocol/graph-committed-observation-1/snapshot-response.json"
    ))
    .unwrap();
    assert_eq!(
        read.snapshot.as_ref().unwrap().snapshot_digest,
        digest(&read.snapshot.as_ref().unwrap().content).unwrap()
    );
    let raw: Value = serde_json::from_str(include_str!(
        "../../protocol/graph-committed-observation-1/snapshot-response.json"
    ))
    .unwrap();
    assert!(
        serde_json::from_value::<Document>(raw).is_ok(),
        "generated schema family describes raw HTTP DTOs, no artificial envelope"
    );
    for q in [
        PullQuery {
            limit: Some(0),
            ..Default::default()
        },
        PullQuery {
            limit: Some(17),
            ..Default::default()
        },
        PullQuery {
            max_bytes: Some(262145),
            ..Default::default()
        },
        PullQuery {
            max_bytes: Some(65535),
            ..Default::default()
        },
    ] {
        assert!(q.limits().is_err());
    }
}

struct Fixture {
    store: Store,
    p: Principal,
    c: CredentialIdentity,
    token: String,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("actual observation tests require an isolated disposable PostgreSQL database");
        let store = Store::connect(&url, 16).await.unwrap();
        store.migrate().await.unwrap();
        let token = format!("observation-{}", Uuid::new_v4());
        let workspace = Uuid::new_v4();
        store
            .bootstrap(workspace, "observation-test", &token)
            .await
            .unwrap();
        let p = store.authenticate(&token, workspace).await.unwrap();
        let c = identity(&token);
        Self { store, p, c, token }
    }
    async fn apply(&self, ops: Vec<Value>) -> MutationResult {
        self.store
            .mutate(
                &self.p,
                serde_json::from_value(
                    json!({"idempotency_key":Uuid::new_v4().to_string(),"operations":ops}),
                )
                .unwrap(),
            )
            .await
            .unwrap()
    }
    async fn node(&self) -> Uuid {
        let id = Uuid::new_v4();
        self.apply(vec![json!({"op":"create_node","id":id,"kind":"task","title":"observed task","task_state":"active"})]).await;
        id
    }
    async fn capture(&self, id: Uuid) -> Snapshot {
        let read = self
            .store
            .observation_snapshot(
                &self.p,
                &self.c,
                &SnapshotRequest {
                    contract: Contract::V1,
                    targets: vec![SnapshotTarget::Task { task_id: id }],
                },
            )
            .await
            .unwrap();
        assert_eq!(read.status, Status::Available);
        let snap = read.snapshot.unwrap();
        assert_eq!(digest(&snap.content).unwrap(), snap.snapshot_digest);
        snap
    }
    async fn register(&self, cursor: Cursor, id: Uuid, now: i64) -> (RegisterRequest, Receipt) {
        let req = request(cursor, id, now);
        let r = self
            .store
            .observation_register(&self.p, &self.c, &req, now)
            .await
            .unwrap();
        assert_eq!(r.status, Status::Available);
        (req, r.receipt.unwrap())
    }
    async fn pull(&self, r: &Receipt, after: &Cursor, limit: usize) -> Pull {
        self.store
            .observation_pull(
                &self.p,
                &self.c,
                r.content.subscriber_id,
                &PullQuery {
                    after: Some(after.encode().unwrap()),
                    receipt_digest: Some(r.reference.digest.clone()),
                    limit: Some(limit),
                    ..Default::default()
                },
                101,
            )
            .await
            .unwrap()
    }
    async fn state(&self, id: Uuid, version: i64, state: &str) {
        self.apply(vec![json!({"op":"update_node","id":id,"expected_version":version,"patch":{"task_state":state}})]).await;
    }
    async fn head(&self) -> Cursor {
        self.store
            .observation_capabilities(&self.p, &self.c)
            .await
            .unwrap()
            .head
    }
    async fn raw_update(&self, id: Uuid, sql: &str) {
        sqlx::query(sql)
            .bind(id)
            .execute(&self.store.pool)
            .await
            .unwrap();
    }
}
#[tokio::test]
async fn committed_observation_registration_recovers_separate_commit_completion_then_reopen() {
    let f = Fixture::new().await;
    let id = f.node().await;
    let initial = f.capture(id).await;
    f.state(id, 1, "completed").await;
    f.state(id, 2, "active").await;
    let (req, r) = f
        .register(initial.content.captured_cursor.clone(), id, 100)
        .await;
    let page = f.pull(&r, &initial.content.captured_cursor, 16).await;
    assert_eq!(page.status, Status::Available);
    assert_eq!(page.groups.len(), 2);
    assert!(
        page.groups[0].facts[0]
            .terminal_from("1")
            .unwrap()
            .is_some()
    );
    assert!(
        page.groups[1].facts[0]
            .terminal_from("1")
            .unwrap()
            .is_none()
    );
    assert_eq!(page.next_cursor.as_ref(), Some(&page.through));
    let again = f
        .store
        .observation_register(&f.p, &f.c, &req, 102)
        .await
        .unwrap();
    assert_eq!(
        digest(&again.receipt.unwrap()).unwrap(),
        digest(&r).unwrap()
    );
    let lookup = f
        .store
        .observation_pull(
            &f.p,
            &f.c,
            req.subscriber_id,
            &PullQuery {
                request_digest: Some(r.content.request_digest.clone()),
                ..Default::default()
            },
            102,
        )
        .await
        .unwrap();
    assert!(lookup.lookup_only);
    assert!(lookup.groups.is_empty());
    assert!(lookup.next_cursor.is_none());
    assert_eq!(lookup.through, initial.content.captured_cursor);
    let mut changed = req.clone();
    changed.condition_digest = format!("sha256:{}", "b".repeat(64));
    assert!(
        f.store
            .observation_register(&f.p, &f.c, &changed, 102)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn committed_observation_same_transaction_transient_never_qualifies() {
    let f = Fixture::new().await;
    let id = f.node().await;
    let initial = f.capture(id).await;
    let (_, r) = f
        .register(initial.content.captured_cursor.clone(), id, 100)
        .await;
    f.apply(vec![
        json!({"op":"update_node","id":id,"expected_version":1,"patch":{"task_state":"completed"}}),
        json!({"op":"update_node","id":id,"expected_version":2,"patch":{"task_state":"active"}}),
    ])
    .await;
    let page = f.pull(&r, &initial.content.captured_cursor, 16).await;
    assert_eq!(page.groups.len(), 1);
    assert!(
        page.groups[0].facts[0]
            .terminal_from("1")
            .unwrap()
            .is_none()
    );
    let Fact::Node {
        before: Some(b),
        after: Some(a),
        mutation_count,
        transition_class,
        ..
    } = &page.groups[0].facts[0]
    else {
        panic!("node fact")
    };
    assert_eq!(b.logical_revision, "1");
    assert_eq!(a.logical_revision, "3");
    assert_eq!(mutation_count, "2");
    assert_eq!(*transition_class, TransitionClass::MultipleChanges);
    assert_eq!(a.task_state, Some(TaskState::Active));
}
#[tokio::test]
async fn committed_observation_normal_completion_automatic_invalidations_are_lifecycle() {
    let f = Fixture::new().await;
    let id = f.node().await;
    // The actual Store's normal update clears a summary, not an invented lifecycle-only SQL path.
    f.apply(vec![json!({"op":"set_summary","id":id,"expected_version":1,"summary":"prior summary","source_revision":1})]).await;
    let node = f.store.get_node(&f.p, id).await.unwrap();
    assert!(node.summary.is_some());
    let initial = f.capture(id).await;
    let mut req = request(initial.content.captured_cursor.clone(), id, 100);
    let Selector::Task {
        starting_logical_revision,
        ..
    } = &mut req.selectors[0]
    else {
        unreachable!()
    };
    *starting_logical_revision = node.content_revision.to_string();
    let r = f
        .store
        .observation_register(&f.p, &f.c, &req, 100)
        .await
        .unwrap()
        .receipt
        .unwrap();
    f.state(id, node.version, "completed").await;
    let page = f.pull(&r, &initial.content.captured_cursor, 16).await;
    assert_eq!(page.groups.len(), 1);
    assert!(
        page.groups[0].facts[0]
            .terminal_from(&node.content_revision.to_string())
            .unwrap()
            .is_some()
    );
    assert!(f.store.get_node(&f.p, id).await.unwrap().summary.is_none());
}
#[tokio::test]
async fn committed_observation_group_paging_uses_fixed_through_and_no_partial_groups() {
    let f = Fixture::new().await;
    let id = f.node().await;
    let initial = f.capture(id).await;
    let (_, r) = f
        .register(initial.content.captured_cursor.clone(), id, 100)
        .await;
    f.state(id, 1, "completed").await;
    let through = f.head().await;
    f.state(id, 2, "active").await;
    let q = PullQuery {
        after: Some(initial.content.captured_cursor.encode().unwrap()),
        through: Some(through.encode().unwrap()),
        receipt_digest: Some(r.reference.digest.clone()),
        limit: Some(1),
        ..Default::default()
    };
    let page = f
        .store
        .observation_pull(&f.p, &f.c, r.content.subscriber_id, &q, 101)
        .await
        .unwrap();
    assert_eq!(page.groups.len(), 1);
    assert_eq!(page.through, through);
    assert_eq!(page.next_cursor, Some(through.clone()));
    let all = f.pull(&r, &initial.content.captured_cursor, 1).await;
    assert_eq!(all.groups.len(), 1);
    assert_ne!(all.next_cursor.as_ref(), Some(&all.through));
    let next = f.pull(&r, all.next_cursor.as_ref().unwrap(), 1).await;
    assert_eq!(next.groups.len(), 1);
    let empty = f.pull(&r, next.next_cursor.as_ref().unwrap(), 1).await;
    assert!(empty.groups.is_empty());
    assert_eq!(empty.next_cursor.as_ref(), Some(&empty.through));
}
#[tokio::test]
async fn committed_observation_rollback_noop_preview_and_other_workspace_do_not_advance() {
    let f = Fixture::new().await;
    let id = f.node().await;
    let before = f.head().await;
    f.state(id, 1, "active").await;
    assert_eq!(f.head().await, before);
    let mut tx = f.store.pool.begin().await.unwrap();
    sqlx::query("UPDATE nodes SET title='rolled back',version=version+1,content_revision=content_revision+1 WHERE id=$1").bind(id).execute(&mut *tx).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(f.head().await, before);
    let g = Fixture::new().await;
    g.node().await;
    assert_eq!(f.head().await, before);
    let draft=f.store.create_draft(&f.p,serde_json::from_value(json!({"name":"observation preview","idempotency_key":Uuid::new_v4().to_string(),"operations":[{"op":"update_node","id":id,"expected_version":1,"patch":{"title":"preview"}}]})).unwrap()).await.unwrap();
    f.store
        .validate_draft(&f.p, serde_json::from_value(draft["id"].clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(f.head().await, before);
}
#[tokio::test]
async fn committed_observation_head_serializes_commit_and_snapshot_without_row_lock_inversion() {
    let f = Fixture::new().await;
    let a = f.node().await;
    let b = f.node().await;
    let before = f.head().await;
    let mut tx = f.store.pool.begin().await.unwrap();
    sqlx::query("UPDATE nodes SET title='first',version=version+1,content_revision=content_revision+1 WHERE id=$1").bind(a).execute(&mut *tx).await.unwrap();
    let second_store = f.store.clone();
    let (started, starting) = tokio::sync::oneshot::channel();
    let second = tokio::spawn(async move {
        let mut connection = second_store.pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        started.send(pid).unwrap();
        sqlx::query("UPDATE nodes SET title='second',version=version+1,content_revision=content_revision+1 WHERE id=$1").bind(b).execute(&mut *connection).await
    });
    let pid = starting.await.unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        let waiting:bool=sqlx::query_scalar("SELECT COALESCE((SELECT wait_event_type='Lock' AND state='active' FROM pg_stat_activity WHERE pid=$1),false)").bind(pid).fetch_one(&f.store.pool).await.unwrap();
        if waiting {
            break;
        }
        assert!(
            Instant::now() < until,
            "second actual database writer never reached lock barrier"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(!second.is_finished());
    tx.commit().await.unwrap();
    second.await.unwrap().unwrap();
    let after = f.head().await;
    assert_eq!(
        decimal(&after.committed_sequence, false).unwrap(),
        decimal(&before.committed_sequence, false).unwrap() + 2
    );
    // Writer already owns a node row but has not reached its AFTER trigger. A
    // snapshot holding head may still read the previous MVCC node immediately.
    let mut writer = f.store.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
        .bind(a)
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    let snap = tokio::time::timeout(Duration::from_secs(1), f.capture(a))
        .await
        .expect("snapshot plain reads cannot wait for node lock");
    assert_eq!(snap.content.captured_cursor, after);
    writer.rollback().await.unwrap();
}
#[tokio::test]
async fn committed_observation_raw_dml_coverage_edges_identity_and_truncate_guard() {
    let f = Fixture::new().await;
    let a = f.node().await;
    let b = f.node().await;
    let before = f.head().await;
    let edge = Uuid::new_v4();
    f.apply(vec![
        json!({"op":"add_edge","id":edge,"source":a,"target":b,"edge_type":"blocks"}),
    ])
    .await;
    let snap = f
        .store
        .observation_snapshot(
            &f.p,
            &f.c,
            &SnapshotRequest {
                contract: Contract::V1,
                targets: vec![SnapshotTarget::Dependency {
                    edge_id: edge,
                    prerequisite_id: a,
                    dependent_id: b,
                }],
            },
        )
        .await
        .unwrap()
        .snapshot
        .unwrap();
    assert_eq!(snap.content.objects.len(), 3);
    assert_ne!(snap.content.captured_cursor, before);
    let mut req = request(snap.content.captured_cursor.clone(), a, 100);
    req.selectors = vec![Selector::Dependency {
        member_id: Uuid::from_u128(4),
        edge_id: edge,
        edge_version: "1".into(),
        prerequisite_id: a,
        dependent_id: b,
        starting_logical_revision: "1".into(),
    }];
    let r = f
        .store
        .observation_register(&f.p, &f.c, &req, 100)
        .await
        .unwrap()
        .receipt
        .unwrap();
    sqlx::query("UPDATE edges SET deleted=true,version=version+1 WHERE id=$1")
        .bind(edge)
        .execute(&f.store.pool)
        .await
        .unwrap();
    let page = f.pull(&r, &snap.content.captured_cursor, 16).await;
    assert_eq!(page.groups.len(), 1);
    assert!(
        page.groups[0]
            .facts
            .iter()
            .any(|f| matches!(f,Fact::Edge {object_id,..} if *object_id==edge))
    );
    assert!(
        sqlx::query("UPDATE nodes SET id=$2 WHERE id=$1")
            .bind(a)
            .bind(Uuid::new_v4())
            .execute(&f.store.pool)
            .await
            .is_err()
    );
    let mut tx = f.store.pool.begin().await.unwrap();
    assert!(
        sqlx::query("TRUNCATE nodes CASCADE")
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    f.raw_update(a,"UPDATE nodes SET title='raw covered',version=version+1,content_revision=content_revision+1 WHERE id=$1").await;
    assert_eq!(
        f.store
            .observation_capabilities(&f.p, &f.c)
            .await
            .unwrap()
            .coverage,
        Status::Available
    );
}
#[tokio::test]
async fn committed_observation_journal_quota_retains_explicit_gap_without_stopping_graph_writes() {
    let f = Fixture::new().await;
    let id = f.node().await;
    let snap = f.capture(id).await;
    let (_, r) = f
        .register(snap.content.captured_cursor.clone(), id, 100)
        .await;
    // Exact head accounting boundary fixture; does not allocate fake domain facts.
    sqlx::query("UPDATE graph_observation_heads SET journal_bytes=16777216 WHERE workspace_id=$1")
        .bind(f.p.workspace_id)
        .execute(&f.store.pool)
        .await
        .unwrap();
    f.state(id, 1, "completed").await;
    let first = f.head().await;
    for _ in 0..8 {
        let node = f.store.get_node(&f.p, id).await.unwrap();
        let state = if node.task_state == Some(crate::domain::TaskState::Completed) {
            "active"
        } else {
            "completed"
        };
        f.state(id, node.version, state).await;
    }
    let page = f.pull(&r, &snap.content.captured_cursor, 16).await;
    assert_eq!(page.status, Status::Gap);
    assert!(page.groups.is_empty());
    assert!(page.next_cursor.is_none());
    assert_eq!(
        page.gap.unwrap().lost_from_sequence,
        first.committed_sequence
    );
    let row=sqlx::query("SELECT batch_count,journal_bytes,(SELECT count(*) FROM graph_observation_batches WHERE workspace_id=$1) n FROM graph_observation_heads WHERE workspace_id=$1").bind(f.p.workspace_id).fetch_one(&f.store.pool).await.unwrap();
    assert_eq!(row.get::<i64, _>("batch_count"), row.get::<i64, _>("n"));
    assert_eq!(row.get::<i64, _>("journal_bytes"), 16777216);
    let read = f
        .store
        .observation_snapshot(
            &f.p,
            &f.c,
            &SnapshotRequest {
                contract: Contract::V1,
                targets: vec![SnapshotTarget::Task { task_id: id }],
            },
        )
        .await
        .unwrap();
    assert_eq!(read.status, Status::Gap);
    assert!(read.snapshot.is_none());
}
#[tokio::test]
async fn committed_observation_invalid_writer_fails_source_closed_but_preserves_domain_commit() {
    let f = Fixture::new().await;
    let id = f.node().await;
    f.raw_update(
        id,
        "UPDATE nodes SET title='unsupported unchanged revision' WHERE id=$1",
    )
    .await;
    assert_eq!(
        f.store.get_node(&f.p, id).await.unwrap().title,
        "unsupported unchanged revision"
    );
    assert_eq!(
        f.store
            .observation_capabilities(&f.p, &f.c)
            .await
            .unwrap()
            .coverage,
        Status::Gap
    );
}
#[tokio::test]
async fn committed_observation_original_lease_release_reset_and_revocation_are_fenced() {
    let f = Fixture::new().await;
    let id = f.node().await;
    let snap = f.capture(id).await;
    let (req, r) = f
        .register(snap.content.captured_cursor.clone(), id, 100)
        .await;
    let expired = f
        .store
        .observation_register(&f.p, &f.c, &req, 1100)
        .await
        .unwrap();
    assert_eq!(expired.status, Status::Expired);
    assert_eq!(
        expired.receipt.unwrap().reference.digest,
        r.reference.digest
    );
    f.store
        .observation_release(&f.p, &f.c, req.subscriber_id, &r.reference.digest)
        .await
        .unwrap();
    f.store
        .observation_release(&f.p, &f.c, req.subscriber_id, &r.reference.digest)
        .await
        .unwrap();
    assert_eq!(
        f.pull(&r, &snap.content.captured_cursor, 16).await.status,
        Status::Released
    );
    let generation: i64 = sqlx::query_scalar("SELECT graph_observation_reset($1,1)")
        .bind(f.p.workspace_id)
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(generation, 2);
    assert_eq!(
        f.pull(&r, &snap.content.captured_cursor, 16).await.status,
        Status::SourceChanged
    );
    assert_eq!(
        f.store
            .observation_register(&f.p, &f.c, &req, 102)
            .await
            .unwrap()
            .status,
        Status::SourceChanged
    );
    let replacement = format!("replacement-{}", Uuid::new_v4());
    f.store
        .grant(
            &f.p,
            &f.p.actor_id,
            &replacement,
            crate::domain::Capability::Admin,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store.observation_capabilities(&f.p, &f.c).await,
        Err(Error::Unauthorized)
    ));
}
#[tokio::test]
async fn committed_observation_control_capacity_counts_released_and_cleans_bounded_expired() {
    let f = Fixture::new().await;
    let id = f.node().await;
    let snap = f.capture(id).await;
    // Real distinct subscriptions, not a forged control_count, reach actor quota.
    for _ in 0..MAX_ACTOR_CONTROLS {
        let (_, r) = f
            .register(snap.content.captured_cursor.clone(), id, 100)
            .await;
        f.store
            .observation_release(&f.p, &f.c, r.content.subscriber_id, &r.reference.digest)
            .await
            .unwrap();
    }
    let req = request(snap.content.captured_cursor.clone(), id, 100);
    assert!(matches!(
        f.store.observation_register(&f.p, &f.c, &req, 100).await,
        Err(Error::Overloaded)
    ));
    let row = sqlx::query(
        "SELECT control_count,control_bytes FROM graph_observation_heads WHERE workspace_id=$1",
    )
    .bind(f.p.workspace_id)
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("control_count"), MAX_ACTOR_CONTROLS);
    assert!(row.get::<i64, _>("control_bytes") <= MAX_CONTROL_BYTES);
    let later = 100 + 1000 + RETRY_RETENTION_MS + 1;
    let req = request(snap.content.captured_cursor, id, later);
    f.store
        .observation_register(&f.p, &f.c, &req, later)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM graph_observation_subscriptions WHERE workspace_id=$1",
    )
    .bind(f.p.workspace_id)
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(count, MAX_ACTOR_CONTROLS - 32 + 1);
}
#[tokio::test]
async fn committed_observation_http_is_actual_authenticated_closed_and_receipt_owned() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let f = Fixture::new().await;
    let app = service::router(f.store.clone());
    let id = f.node().await;
    let uri = format!("/v1/workspaces/{}/observations/snapshot", f.p.workspace_id);
    let body = json!({"contract":"graph-committed-observation-1","targets":[{"kind":"task","task_id":id}]});
    for (token, expected) in [(None, 401), (Some(f.token.as_str()), 200)] {
        let mut request = Request::builder()
            .method("POST")
            .uri(&uri)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let res = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), expected);
        if expected == 200 {
            let bytes = to_bytes(res.into_body(), MAX_RESPONSE).await.unwrap();
            let read: SnapshotRead = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(read.status, Status::Available);
        }
    }
    let initial = f.capture(id).await;
    let req = request(
        initial.content.captured_cursor,
        id,
        chrono::Utc::now().timestamp_millis(),
    );
    let register_uri = format!(
        "/v1/workspaces/{}/observations/subscriptions",
        f.p.workspace_id
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(register_uri)
                .header("authorization", format!("Bearer {}", f.token))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let registration: Registration =
        serde_json::from_slice(&to_bytes(res.into_body(), MAX_RESPONSE).await.unwrap()).unwrap();
    let receipt = registration.receipt.unwrap();
    let lookup_uri = format!(
        "/v1/workspaces/{}/observations/subscriptions/{}/changes?request_digest={}",
        f.p.workspace_id, req.subscriber_id, receipt.content.request_digest
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&lookup_uri)
                .header("authorization", format!("Bearer {}", f.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let lookup: Pull =
        serde_json::from_slice(&to_bytes(res.into_body(), MAX_RESPONSE).await.unwrap()).unwrap();
    assert!(lookup.lookup_only);
    assert_eq!(lookup.receipt.reference.digest, receipt.reference.digest);
    let release_uri = format!(
        "/v1/workspaces/{}/observations/subscriptions/{}?receipt_digest={}",
        f.p.workspace_id, req.subscriber_id, receipt.reference.digest
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(release_uri)
                .header("authorization", format!("Bearer {}", f.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 204);
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(lookup_uri)
                .header("authorization", format!("Bearer {}", f.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let lookup: Pull =
        serde_json::from_slice(&to_bytes(res.into_body(), MAX_RESPONSE).await.unwrap()).unwrap();
    assert_eq!(lookup.status, Status::Released);
    let missing = format!(
        "/v1/workspaces/{}/observations/subscriptions/{}/changes?request_digest=sha256:{}",
        f.p.workspace_id,
        Uuid::new_v4(),
        "a".repeat(64)
    );
    let res = app
        .oneshot(
            Request::builder()
                .uri(missing)
                .header("authorization", format!("Bearer {}", f.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 410);
}
#[tokio::test]
async fn committed_observation_bounded_latency_smoke_records_real_owner_operations() {
    let f = Fixture::new().await;
    let id = f.node().await;
    let mut writes = vec![];
    let mut snapshots = vec![];
    for version in 1..=24 {
        let now = Instant::now();
        f.state(
            id,
            version,
            if version % 2 == 1 {
                "completed"
            } else {
                "active"
            },
        )
        .await;
        writes.push(now.elapsed());
        let now = Instant::now();
        f.capture(id).await;
        snapshots.push(now.elapsed());
    }
    writes.sort();
    snapshots.sort();
    eprintln!(
        "observation isolated smoke n=24 write_p50_us={} write_p95_us={} snapshot_p50_us={} snapshot_p95_us={} (not a production capacity claim)",
        writes[12].as_micros(),
        writes[22].as_micros(),
        snapshots[12].as_micros(),
        snapshots[22].as_micros()
    );
    assert!(writes.iter().all(|n| *n < Duration::from_secs(4)));
    assert!(snapshots.iter().all(|n| *n < Duration::from_secs(4)));
}

#[tokio::test]
async fn committed_observation_transaction_fact_limit_has_reserved_gap_and_bounded_facts() {
    let f = Fixture::new().await;
    // Ordinary SQL INSERT writer runs the same triggers as Store/API. Its domain
    // transaction is intentionally larger than the 100-operation API envelope.
    sqlx::query("INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,created_at,updated_at,created_by) SELECT gen_random_uuid(),$1,'task',1,1,'bounded journal source','','','active',clock_timestamp(),clock_timestamp(),'observation-test' FROM generate_series(1,4097)")
        .bind(f.p.workspace_id).execute(&f.store.pool).await.unwrap();
    let caps = f.store.observation_capabilities(&f.p, &f.c).await.unwrap();
    assert_eq!(caps.coverage, Status::Gap);
    let row=sqlx::query("SELECT h.fact_count,h.batch_count,h.gap_reason,(SELECT count(*) FROM nodes WHERE workspace_id=$1) nodes,(SELECT count(*) FROM graph_observation_facts WHERE workspace_id=$1) facts FROM graph_observation_heads h WHERE h.workspace_id=$1")
        .bind(f.p.workspace_id).fetch_one(&f.store.pool).await.unwrap();
    assert_eq!(row.get::<i64, _>("fact_count"), 4096);
    assert_eq!(row.get::<i64, _>("facts"), 4096);
    assert_eq!(row.get::<i64, _>("nodes"), 4097);
    assert_eq!(row.get::<String, _>("gap_reason"), "transaction_fact_limit");
    assert_eq!(row.get::<i64, _>("batch_count"), 1);
    f.node().await;
    assert_eq!(
        f.store
            .observation_capabilities(&f.p, &f.c)
            .await
            .unwrap()
            .coverage,
        Status::Gap
    );
}

#[tokio::test]
async fn committed_observation_selector_kind_is_part_of_object_identity() {
    let f = Fixture::new().await;
    let task = f.node().await;
    let other = f.node().await;
    // Node and edge primary keys have independent UUID namespaces. A deliberately
    // equal UUID must not turn an edge-only update into a selected Task event.
    f.apply(vec![
        json!({"op":"add_edge","id":task,"source":task,"target":other,"edge_type":"references"}),
    ])
    .await;
    let snap = f.capture(task).await;
    let (_, receipt) = f
        .register(snap.content.captured_cursor.clone(), task, 100)
        .await;
    sqlx::query("UPDATE edges SET deleted=true,version=version+1 WHERE id=$1")
        .bind(task)
        .execute(&f.store.pool)
        .await
        .unwrap();
    let page = f.pull(&receipt, &snap.content.captured_cursor, 16).await;
    assert!(page.groups.is_empty());
    assert_eq!(page.next_cursor.as_ref(), Some(&page.through));
}

#[tokio::test]
async fn committed_observation_receipts_are_actor_scoped_and_current_readers_can_subscribe() {
    let f = Fixture::new().await;
    let task = f.node().await;
    let snap = f.capture(task).await;
    let (_, receipt) = f
        .register(snap.content.captured_cursor.clone(), task, 100)
        .await;
    let token = format!("observation-reader-{}", Uuid::new_v4());
    f.store
        .grant(&f.p, "reader", &token, crate::domain::Capability::Read)
        .await
        .unwrap();
    let reader = f
        .store
        .authenticate(&token, f.p.workspace_id)
        .await
        .unwrap();
    let current = identity(&token);
    let q = PullQuery {
        request_digest: Some(receipt.content.request_digest.clone()),
        ..Default::default()
    };
    assert!(matches!(
        f.store
            .observation_pull(&reader, &current, receipt.content.subscriber_id, &q, 101)
            .await,
        Err(Error::NotFound)
    ));
    let req = request(snap.content.captured_cursor.clone(), task, 100);
    let mine = f
        .store
        .observation_register(&reader, &current, &req, 100)
        .await
        .unwrap();
    assert_eq!(mine.status, Status::Available);
    assert_eq!(mine.receipt.unwrap().content.actor_id, "reader");
    let g = Fixture::new().await;
    assert!(matches!(
        f.store
            .observation_pull(&g.p, &g.c, receipt.content.subscriber_id, &q, 101)
            .await,
        Err(Error::NotFound)
    ));
}
#[tokio::test]
async fn committed_observation_without_starting_boundary_never_rebinds_stale_pin() {
    let f = Fixture::new().await;
    let task = f.node().await;
    let starting = f.capture(task).await;
    f.state(task, 1, "completed").await;
    f.state(task, 2, "active").await;
    let mut req = request(starting.content.captured_cursor, task, 100);
    req.from_cursor = None;
    let response = f
        .store
        .observation_register(&f.p, &f.c, &req, 100)
        .await
        .unwrap();
    let receipt = response.receipt.unwrap();
    let Selector::Task {
        starting_logical_revision,
        ..
    } = &receipt.content.selectors[0]
    else {
        unreachable!()
    };
    assert_eq!(starting_logical_revision, "1");
    let ObservedObject::Node {
        value: Some(actual),
        ..
    } = &receipt.content.snapshot.content.objects[0]
    else {
        unreachable!()
    };
    assert_eq!(actual.logical_revision, "3");
    assert_eq!(
        receipt.content.initial_cursor,
        receipt.content.snapshot.content.captured_cursor
    );
    let page = f.pull(&receipt, &receipt.content.initial_cursor, 16).await;
    assert!(page.groups.is_empty());
    // This is a changed pin, not evidence that no completion happened. The adapter
    // MUST refuse/reconcile; Graph does not replace selector1 with snapshot3.
}

#[tokio::test]
async fn committed_observation_scope_deletion_is_unavailable_and_recreation_changes_stream() {
    let f = Fixture::new().await;
    let task = Uuid::new_v4();
    // An ordinary DML-only node has no immutable legacy history FKs to remove.
    // Its actual source journal/subscription are retained until scope deletion.
    sqlx::query("INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,created_at,updated_at) VALUES($1,$2,'task',1,1,'deleted scope fixture','','','active',clock_timestamp(),clock_timestamp())").bind(task).bind(f.p.workspace_id).execute(&f.store.pool).await.unwrap();
    let snap = f.capture(task).await;
    let (request, receipt) = f
        .register(snap.content.captured_cursor.clone(), task, 100)
        .await;
    f.raw_update(
        task,
        "UPDATE nodes SET task_state='completed',version=2,content_revision=2 WHERE id=$1",
    )
    .await;
    let retained: (i64, i64) = sqlx::query_as(
        "SELECT batch_count,control_count FROM graph_observation_heads WHERE workspace_id=$1",
    )
    .bind(f.p.workspace_id)
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(retained, (2, 1));
    let mut tx = f.store.pool.begin().await.unwrap();
    sqlx::query("DELETE FROM nodes WHERE workspace_id=$1")
        .bind(f.p.workspace_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM credentials WHERE workspace_id=$1")
        .bind(f.p.workspace_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workspaces WHERE id=$1")
        .bind(f.p.workspace_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let left:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM graph_observation_heads WHERE workspace_id=$1),(SELECT count(*) FROM graph_observation_batches WHERE workspace_id=$1),(SELECT count(*) FROM graph_observation_facts WHERE workspace_id=$1),(SELECT count(*) FROM graph_observation_subscriptions WHERE workspace_id=$1)").bind(f.p.workspace_id).fetch_one(&f.store.pool).await.unwrap();
    assert_eq!(left, (0, 0, 0, 0));
    assert!(matches!(
        f.store
            .observation_pull(
                &f.p,
                &f.c,
                request.subscriber_id,
                &PullQuery {
                    request_digest: Some(receipt.content.request_digest.clone()),
                    ..Default::default()
                },
                101
            )
            .await,
        Err(Error::Unauthorized)
    ));
    f.store
        .bootstrap(f.p.workspace_id, &f.p.actor_id, &f.token)
        .await
        .unwrap();
    let recreated = f.head().await;
    assert_ne!(recreated.scope.stream_id, snap.content.scope.stream_id);
    assert_eq!(recreated.committed_sequence, "0");
    let stale = f
        .store
        .observation_register(&f.p, &f.c, &request, 101)
        .await
        .unwrap();
    assert_eq!(stale.status, Status::SourceChanged);
    assert!(stale.receipt.is_none());
    assert!(matches!(
        f.store
            .observation_pull(
                &f.p,
                &f.c,
                request.subscriber_id,
                &PullQuery {
                    after: Some(snap.content.captured_cursor.encode().unwrap()),
                    receipt_digest: Some(receipt.reference.digest),
                    ..Default::default()
                },
                101
            )
            .await,
        Err(Error::NotFound)
    ));
    let controls: i64 = sqlx::query_scalar(
        "SELECT control_count FROM graph_observation_heads WHERE workspace_id=$1",
    )
    .bind(f.p.workspace_id)
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(
        controls, 0,
        "old source scope must not install a fresh subscription"
    );
}
