//! Transaction and workspace invariants exercised against authoritative PostgreSQL.
use aq_graph::{domain::*, store::Store};
use serde_json::{Value, json};
use uuid::Uuid;

struct Fixture {
    store: Store,
    p: Principal,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("set AQ_GRAPH_TEST_DATABASE_URL to a disposable PostgreSQL database");
        let store = Store::connect(&url, 16).await.unwrap();
        store.migrate().await.unwrap();
        let workspace_id = Uuid::new_v4();
        let token = format!("invariant-{}", Uuid::new_v4());
        store
            .bootstrap(workspace_id, "admin", &token)
            .await
            .unwrap();
        let p = store.authenticate(&token, workspace_id).await.unwrap();
        Self { store, p }
    }
    async fn apply(&self, key: &str, ops: Vec<Value>) -> Result<MutationResult> {
        self.store
            .mutate(
                &self.p,
                serde_json::from_value(json!({"idempotency_key":key,"operations":ops})).unwrap(),
            )
            .await
    }
    async fn node(&self, id: Uuid, title: &str, kind: &str) -> Node {
        let result = self
            .apply(
                &Uuid::new_v4().to_string(),
                vec![json!({"op":"create_node","id":id,"kind":kind,"title":title})],
            )
            .await
            .unwrap();
        serde_json::from_value(result.results[0].clone()).unwrap()
    }
    async fn draft(&self, ops: Vec<Value>) -> Uuid {
        let result=self.store.create_draft(&self.p,serde_json::from_value(json!({"name":"proposal","idempotency_key":Uuid::new_v4().to_string(),"operations":ops})).unwrap()).await.unwrap();
        serde_json::from_value(result["id"].clone()).unwrap()
    }
}
fn link(id: Uuid, a: Uuid, b: Uuid) -> Value {
    json!({"op":"add_edge","id":id,"source":a,"target":b,"edge_type":"references"})
}
fn update(id: Uuid, version: i64, patch: Value) -> Value {
    json!({"op":"update_node","id":id,"expected_version":version,"patch":patch})
}

#[tokio::test]
async fn concurrent_endpoint_delete_and_edge_creation_cannot_orphan() {
    let f = Fixture::new().await;
    // Exercise multiple independent races without a workspace lock. Both legal
    // serialization orders are accepted; an orphan is never accepted.
    for _ in 0..12 {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let e = Uuid::new_v4();
        f.node(a, "source", "task").await;
        f.node(b, "target", "task").await;
        let delete_key = Uuid::new_v4().to_string();
        let add_key = Uuid::new_v4().to_string();
        let deletion = f.apply(
            &delete_key,
            vec![json!({"op":"delete_node","id":a,"expected_version":1})],
        );
        let addition = f.apply(&add_key, vec![link(e, a, b)]);
        let (deleted, added) = tokio::join!(deletion, addition);
        assert!(
            deleted.is_ok() || added.is_ok(),
            "one serialization order must succeed: {deleted:?} {added:?}"
        );
        assert!(!(deleted.is_ok() && added.is_ok()), "both cannot commit");
        let orphan:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.id=e.source OR n.id=e.target WHERE e.workspace_id=$1 AND NOT e.deleted AND n.deleted)").bind(f.p.workspace_id).fetch_one(&f.store.pool).await.unwrap();
        assert!(!orphan);
    }
}

#[tokio::test]
async fn concurrent_admin_self_demotions_preserve_an_administrator() {
    let f = Fixture::new().await;
    let second_token = format!("second-{}", Uuid::new_v4());
    f.store
        .grant(&f.p, "second", &second_token, Capability::Admin)
        .await
        .unwrap();
    let second = f
        .store
        .authenticate(&second_token, f.p.workspace_id)
        .await
        .unwrap();
    let first_token = format!("demoted-{}", Uuid::new_v4());
    let second_replacement = format!("demoted-{}", Uuid::new_v4());
    let (a, b) = tokio::join!(
        f.store
            .grant(&f.p, "admin", &first_token, Capability::Write),
        f.store
            .grant(&second, "second", &second_replacement, Capability::Write)
    );
    assert!(
        a.is_ok() ^ b.is_ok(),
        "exactly one self-demotion may commit: {a:?} {b:?}"
    );
    let admins: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM credentials WHERE workspace_id=$1 AND capability='admin'",
    )
    .bind(f.p.workspace_id)
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(admins, 1);
}

#[tokio::test]
async fn workspace_scope_covers_records_history_edges_drafts_and_tokens() {
    let a = Fixture::new().await;
    let b = Fixture::new().await;
    let own = Uuid::new_v4();
    let foreign = Uuid::new_v4();
    a.node(own, "own", "task").await;
    b.node(foreign, "private", "knowledge").await;
    let draft = b
        .draft(vec![update(foreign, 1, json!({"title":"private edit"}))])
        .await;
    assert!(matches!(
        a.store.get_node(&a.p, foreign).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        a.store.history(&a.p, foreign, ListQuery::default()).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        a.store.diff(&a.p, foreign, 1, 1).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        a.store.get_draft(&a.p, draft).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        a.store.validate_draft(&a.p, draft).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        a.store.apply_draft(&a.p, draft, "foreign-apply").await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        a.apply("foreign-edge", vec![link(Uuid::new_v4(), own, foreign)])
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        a.apply(
            "foreign-pin",
            vec![json!({"op":"restore_node","id":foreign,"expected_version":1,"revision":1})]
        )
        .await,
        Err(Error::NotFound)
    ));
    let token = format!("shared-secret-{}", Uuid::new_v4());
    a.store
        .grant(&a.p, "same-secret", &token, Capability::Read)
        .await
        .unwrap();
    b.store
        .grant(&b.p, "same-secret", &token, Capability::Write)
        .await
        .unwrap();
    assert_eq!(
        a.store
            .authenticate(&token, a.p.workspace_id)
            .await
            .unwrap()
            .capability,
        Capability::Read
    );
    assert_eq!(
        b.store
            .authenticate(&token, b.p.workspace_id)
            .await
            .unwrap()
            .capability,
        Capability::Write
    );
    assert!(matches!(
        a.store.authenticate(&token, Uuid::new_v4()).await,
        Err(Error::Unauthorized)
    ));
}

#[tokio::test]
async fn failed_batch_leaves_no_current_state_history_or_receipt() {
    let f = Fixture::new().await;
    let id = Uuid::new_v4();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM change_sets WHERE workspace_id=$1")
        .bind(f.p.workspace_id)
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    let key = "atomic-failure";
    let result = f
        .apply(
            key,
            vec![
                json!({"op":"create_node","id":id,"kind":"task","title":"must roll back"}),
                update(id, 99, json!({"title":"conflicting"})),
            ],
        )
        .await;
    assert!(matches!(result, Err(Error::Conflict(_))));
    assert!(matches!(
        f.store.get_node(&f.p, id).await,
        Err(Error::NotFound)
    ));
    for table in [
        "node_revisions",
        "history_entries",
        "idempotency_receipts",
        "link_sets",
        "link_set_members",
        "edge_versions",
    ] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE workspace_id=$1"
        ))
        .bind(f.p.workspace_id)
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "{table} left an orphan");
    }
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM change_sets WHERE workspace_id=$1")
        .bind(f.p.workspace_id)
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    // An uncommitted failure does not consume the key.
    f.apply(
        key,
        vec![json!({"op":"create_node","id":id,"kind":"task","title":"retry corrected batch"})],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn same_draft_concurrent_different_keys_applies_once() {
    let f = Fixture::new().await;
    let id = Uuid::new_v4();
    f.node(id, "original", "task").await;
    let draft = f
        .draft(vec![update(
            id,
            1,
            json!({"title":"applied once","task_state":"active"}),
        )])
        .await;
    let (a, b) = tokio::join!(
        f.store.apply_draft(&f.p, draft, "apply-one"),
        f.store.apply_draft(&f.p, draft, "apply-two")
    );
    assert!(a.is_ok() ^ b.is_ok(), "one apply must win: {a:?} {b:?}");
    assert!(matches!(
        if a.is_err() { a } else { b },
        Err(Error::Conflict(_))
    ));
    let n = f.store.get_node(&f.p, id).await.unwrap();
    assert_eq!(n.version, 2);
    assert_eq!(n.content_revision, 2);
    assert_eq!(n.task_state, Some(TaskState::Active));
    let revisions: i64 = sqlx::query_scalar("SELECT count(*) FROM node_revisions WHERE node_id=$1")
        .bind(id)
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(revisions, 2);
}

#[tokio::test]
async fn draft_preview_restore_tombstone_and_ordered_dependency_resolution() {
    let f = Fixture::new().await;
    let id = Uuid::new_v4();
    let neighbor = Uuid::new_v4();
    let edge = Uuid::new_v4();
    f.node(id, "original", "task").await;
    f.node(neighbor, "neighbor", "task").await;
    f.apply(
        "edit",
        vec![
            update(id, 1, json!({"title":"revised","task_state":"completed"})),
            link(edge, id, neighbor),
        ],
    )
    .await
    .unwrap();
    let draft = f
        .draft(vec![
            json!({"op":"restore_node","id":id,"expected_version":3,"revision":1}),
        ])
        .await;
    let preview = f.store.validate_draft(&f.p, draft).await.unwrap();
    let node_preview = preview["preview"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == json!(id))
        .unwrap();
    assert_eq!(node_preview["before"]["title"], "revised");
    assert_eq!(node_preview["after"]["title"], "original");
    let edge_preview = preview["preview"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(edge))
        .unwrap();
    assert_eq!(edge_preview["before"]["deleted"], false);
    assert_eq!(edge_preview["after"]["deleted"], true);
    let untouched = f.store.get_node(&f.p, id).await.unwrap();
    assert_eq!(untouched.title, "revised");
    assert_eq!(untouched.version, 3);
    f.store
        .apply_draft(&f.p, draft, "restore-content")
        .await
        .unwrap();
    let restored = f.store.get_node(&f.p, id).await.unwrap();
    assert_eq!(restored.title, "original");
    assert_eq!(restored.task_state, Some(TaskState::Completed));
    assert_eq!(restored.content_revision, 4);
    // Restoring revision one also removes the later informational relationship.
    assert!(
        f.store
            .context(&f.p, id, ContextQuery::default())
            .await
            .unwrap()
            .edges
            .is_empty()
    );
    let replacement_edge = Uuid::new_v4();
    f.apply(
        "replacement-link",
        vec![link(replacement_edge, id, neighbor)],
    )
    .await
    .unwrap();
    let linked = f.store.get_node(&f.p, id).await.unwrap();
    assert!(matches!(
        f.apply(
            "cannot-delete-referenced",
            vec![json!({"op":"delete_node","id":id,"expected_version":linked.version})]
        )
        .await,
        Err(Error::Conflict(_))
    ));
    // The final proposed graph is validated, so an edge removal can follow the
    // tombstone operation in the same indivisible batch.
    let deletion = f
        .draft(vec![
            json!({"op":"delete_node","id":id,"expected_version":linked.version}),
            json!({"op":"remove_edge","id":replacement_edge,"expected_version":1}),
        ])
        .await;
    f.store.validate_draft(&f.p, deletion).await.unwrap();
    assert!(!f.store.get_node(&f.p, id).await.unwrap().deleted);
    f.store
        .apply_draft(&f.p, deletion, "delete-and-unlink")
        .await
        .unwrap();
    let deleted = f.store.get_node(&f.p, id).await.unwrap();
    assert!(deleted.deleted);
    assert_eq!(deleted.version, linked.version + 2);
    f.apply(
        "undelete",
        vec![json!({"op":"restore_node","id":id,"expected_version":deleted.version,"revision":2,"include_lifecycle":true})],
    )
    .await
    .unwrap();
    let n = f.store.get_node(&f.p, id).await.unwrap();
    assert!(!n.deleted);
    assert_eq!(n.title, "revised");
    assert_eq!(n.task_state, Some(TaskState::Completed));
    assert_eq!(n.version, deleted.version + 1);
    assert_eq!(n.content_revision, deleted.content_revision + 1);
}

#[tokio::test]
async fn oversized_mutation_response_rolls_back_before_commit() {
    let f = Fixture::new().await;
    let id = Uuid::new_v4();
    let body = "x".repeat(65_536);
    f.apply(
        "large-content",
        vec![json!({"op":"create_node","id":id,"kind":"knowledge","title":"large","body":body})],
    )
    .await
    .unwrap();
    let mut operations = vec![update(
        id,
        1,
        json!({"title":"changed before cap rejection"}),
    )];
    operations
        .extend((0..39).map(|_| update(id, 2, json!({"title":"changed before cap rejection"}))));
    let result = f.apply("oversized-response", operations).await;
    assert!(
        matches!(result,Err(Error::Validation(ref text)) if text.contains("result exceeds")),
        "{result:?}"
    );
    let n = f.store.get_node(&f.p, id).await.unwrap();
    assert_eq!(n.version, 1);
    assert_eq!(n.content_revision, 1);
    assert_eq!(n.title, "large");
    let receipt:i64=sqlx::query_scalar("SELECT count(*) FROM idempotency_receipts WHERE workspace_id=$1 AND key='oversized-response'").bind(f.p.workspace_id).fetch_one(&f.store.pool).await.unwrap();
    assert_eq!(receipt, 0);
}

#[tokio::test]
async fn broad_large_pinned_context_obeys_byte_budget_without_current_fallback() {
    let f = Fixture::new().await;
    let root = Uuid::new_v4();
    f.node(root, "root", "task").await;
    for index in 0..5 {
        let id = Uuid::new_v4();
        f.apply(&format!("large-pin-{index}"),vec![json!({"op":"create_node","id":id,"kind":"knowledge","title":format!("evidence {index}"),"body":"a ".repeat(8192)}),json!({"op":"add_edge","source":root,"target":id,"edge_type":"references","source_revision":1,"target_revision":1}),update(id,1,json!({"title":"current changed title"}))]).await.unwrap();
    }
    let context = f
        .store
        .context(
            &f.p,
            root,
            ContextQuery {
                max_bytes: 50_000,
                limit: 200,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(context.truncated);
    assert!(serde_json::to_vec(&context).unwrap().len() <= 50_000);
    for edge in &context.edges {
        for (node, rev) in [
            (edge.source, edge.source_revision),
            (edge.target, edge.target_revision),
        ] {
            if let Some(rev) = rev {
                let pin = context
                    .pinned_revisions
                    .iter()
                    .find(|p| p["id"] == json!(node) && p["content_revision"] == rev)
                    .expect("every retained pin resolves exactly");
                if node != root {
                    assert_ne!(pin["title"], "current changed title");
                }
            }
        }
    }
    let tiny = f
        .store
        .context(
            &f.p,
            root,
            ContextQuery {
                max_bytes: 1024,
                limit: 200,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(tiny.truncated);
    assert!(tiny.nodes.is_empty());
    assert!(tiny.edges.is_empty());
    assert!(serde_json::to_vec(&tiny).unwrap().len() <= 1024);
}

#[tokio::test]
async fn fulltext_search_paginates_matching_live_knowledge_and_preserves_rare_terms() {
    let f = Fixture::new().await;
    let knowledge: Vec<_> = (0..14).map(|_| Uuid::new_v4()).collect();
    let mut operations:Vec<_>=knowledge.iter().enumerate().map(|(index,id)|json!({"op":"create_node","id":id,"kind":"knowledge","title":format!("Authentication evidence {index}"),"body":if index==0{"Unicorn authentication evidence"}else{"Authentication evidence"}})).collect();
    operations.extend((0..5).map(|_|json!({"op":"create_node","kind":"task","title":"Authentication task must not enter knowledge search"})));
    f.apply("search-fixture", operations).await.unwrap();
    f.apply(
        "search-retirement-and-delete",
        vec![
            update(
                knowledge[1],
                1,
                json!({"retired":true,"retirement_reason":"Superseded test finding"}),
            ),
            json!({"op":"delete_node","id":knowledge[2],"expected_version":1}),
        ],
    )
    .await
    .unwrap();
    let mut cursor = None;
    let mut actual = vec![];
    loop {
        let page = f
            .store
            .search_nodes(
                &f.p,
                "authentication",
                NodeFilters {
                    kind: Some(NodeKind::Knowledge),
                    ..Default::default()
                },
                ListQuery { limit: 3, cursor },
            )
            .await
            .unwrap();
        assert!(
            page.items
                .iter()
                .all(|n| n.kind == NodeKind::Knowledge && !n.deleted && !n.retired)
        );
        actual.extend(page.items.into_iter().map(|n| n.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(
            actual.len() <= knowledge.len(),
            "pagination did not advance"
        );
    }
    let mut expected: Vec<_> = knowledge
        .iter()
        .copied()
        .filter(|id| *id != knowledge[1] && *id != knowledge[2])
        .collect();
    expected.sort();
    assert_eq!(actual, expected);
    let rare = f
        .store
        .search(&f.p, "unicorn", ListQuery::default())
        .await
        .unwrap();
    assert_eq!(rare.items.len(), 1);
    assert_eq!(rare.items[0].id, knowledge[0]);
    assert!(
        f.store
            .search(&f.p, "nonexistentrareword", ListQuery::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn concurrent_retirement_and_successor_link_removal_cannot_break_pointer() {
    let f = Fixture::new().await;
    for _ in 0..10 {
        let old = Uuid::new_v4();
        let successor = Uuid::new_v4();
        let edge = Uuid::new_v4();
        f.node(old, "old assertion", "task").await;
        f.node(successor, "replacement", "knowledge").await;
        f.apply(&Uuid::new_v4().to_string(), vec![json!({"op":"add_edge","id":edge,"source":successor,"target":old,"edge_type":"supersedes"})]).await.unwrap();
        let retire_key = Uuid::new_v4().to_string();
        let remove_key = Uuid::new_v4().to_string();
        let (retired, removed) = tokio::join!(
            f.apply(
                &retire_key,
                vec![update(
                    old,
                    1,
                    json!({"retired":true,"retirement_reason":"replaced","successor_id":successor})
                )]
            ),
            f.apply(
                &remove_key,
                vec![json!({"op":"remove_edge","id":edge,"expected_version":1})]
            )
        );
        assert!(
            retired.is_ok() || removed.is_ok(),
            "one serialization must succeed: {retired:?} {removed:?}"
        );
        assert!(
            !(retired.is_ok() && removed.is_ok()),
            "cannot commit a dangling successor"
        );
        let dangling: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes n WHERE n.workspace_id=$1 AND n.successor_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM edges e WHERE e.workspace_id=n.workspace_id AND e.source=n.successor_id AND e.target=n.id AND e.edge_type='supersedes' AND NOT e.deleted))").bind(f.p.workspace_id).fetch_one(&f.store.pool).await.unwrap();
        assert!(!dangling);
    }
}

#[tokio::test]
async fn competing_summary_and_content_edit_never_attaches_stale_summary() {
    let f = Fixture::new().await;
    for kind in ["task", "knowledge"] {
        for _ in 0..8 {
            let id = Uuid::new_v4();
            f.node(id, "original", kind).await;
            let summary_key = Uuid::new_v4().to_string();
            let edit_key = Uuid::new_v4().to_string();
            let (summary, edit) = tokio::join!(
                f.apply(&summary_key, vec![json!({"op":"set_summary","id":id,"expected_version":1,"source_revision":1,"summary":"original summary"})]),
                f.apply(&edit_key, vec![update(id,1,json!({"title":"new material"}))])
            );
            assert_ne!(summary.is_ok(), edit.is_ok(), "exactly one CAS wins");
            let node = f.store.get_node(&f.p, id).await.unwrap();
            if node.title == "new material" {
                assert!(node.summary.is_none());
            } else {
                assert_eq!(node.summary_of_revision, Some(1));
            }
            assert_eq!(node.content_revision, 2);
        }
    }
}
