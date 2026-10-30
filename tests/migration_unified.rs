//! Unified task/knowledge revision migration, using rollback-only private schemas.
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

const BASE: [&str; 5] = [
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_workspace_credential_scope.sql"),
    include_str!("../migrations/0003_knowledge_pagination.sql"),
    include_str!("../migrations/0004_graph_contract.sql"),
    include_str!("../migrations/0005_knowledge_contract.sql"),
];
const UPGRADE: &str = include_str!("../migrations/0006_unified_revisions.sql");
struct Fixture {
    pool: PgPool,
    tx: Transaction<'static, Postgres>,
    schema: String,
    workspace: Uuid,
    foreign: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("set AQ_GRAPH_TEST_DATABASE_URL to a disposable PostgreSQL database");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let schema = format!("unified_migration_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE SCHEMA {schema}; SET LOCAL search_path TO {schema},pg_catalog; SET LOCAL statement_timeout='10s';")).execute(&mut *tx).await.unwrap();
        for sql in BASE {
            sqlx::raw_sql(sql).execute(&mut *tx).await.unwrap();
        }
        let workspace = Uuid::new_v4();
        let foreign = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces(id) VALUES($1),($2)")
            .bind(workspace)
            .bind(foreign)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,knowledge_type,verification,deleted,created_at,updated_at,created_by) SELECT lpad(to_hex(g),32,'0')::uuid,CASE WHEN g<=5 THEN $1 ELSE $2 END,CASE WHEN g IN (3,4) THEN 'knowledge' ELSE 'task' END,CASE WHEN g=1 THEN 3 ELSE 1 END,1,'original '||g,'original body '||g,'',CASE WHEN g NOT IN (3,4) THEN 'active' END,CASE WHEN g IN (3,4) THEN 'note' END,CASE WHEN g IN (3,4) THEN 'unverified' END,g=5,'2026-01-01T00:00:00Z','2026-01-02T00:00:00Z','legacy-author' FROM generate_series(1,7) g").bind(workspace).bind(foreign).execute(&mut *tx).await.unwrap();
        let mut fixture = Self {
            pool,
            tx,
            schema,
            workspace,
            foreign,
        };
        for (id, source, target, kind, version, deleted) in [
            (10, 1, 2, "blocks", 3, false),
            (11, 1, 2, "parent-child", 2, false),
            (12, 1, 3, "produces", 1, false),
            (13, 3, 4, "references", 1, false),
            (14, 4, 3, "supports", 1, false),
            (15, 2, 4, "references", 2, true),
            (16, 4, 1, "references", 1, false),
            (20, 6, 7, "references", 1, false),
        ] {
            let ws = if source <= 5 { workspace } else { foreign };
            sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,version,deleted,created_at,description) VALUES($1,$2,$3,$4,$5,$6,$7,'2026-01-01T00:00:00Z','original edge description')").bind(Uuid::from_u128(id)).bind(ws).bind(Uuid::from_u128(source)).bind(Uuid::from_u128(target)).bind(kind).bind(version).bind(deleted).execute(&mut *fixture.tx).await.unwrap();
        }
        for ws in [workspace, foreign] {
            let change = Uuid::new_v4();
            sqlx::query("INSERT INTO change_sets(id,workspace_id,actor_id,reason,operations,status,result) VALUES($1,$2,'legacy-author','original reason','[]','applied','{\"legacy\":true}')").bind(change).bind(ws).execute(&mut *fixture.tx).await.unwrap();
            sqlx::query("INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id) SELECT workspace_id,id,1,to_jsonb(n)-'search_document','legacy-author','created','original reason',$2 FROM nodes n WHERE workspace_id=$1").bind(ws).bind(change).execute(&mut *fixture.tx).await.unwrap();
            sqlx::query("INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data) SELECT workspace_id,ARRAY[node_id],md5(node_id::text)::uuid,'revision','legacy-author','created','original reason',$2,snapshot FROM node_revisions WHERE workspace_id=$1").bind(ws).bind(change).execute(&mut *fixture.tx).await.unwrap();
            sqlx::query("INSERT INTO idempotency_receipts(workspace_id,actor_id,operation,key,payload_hash,result) VALUES($1,'legacy-author','mutate','original-key','original-hash','{\"legacy\":true}')").bind(ws).execute(&mut *fixture.tx).await.unwrap();
        }
        fixture
    }
    async fn legacy(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('revisions',(SELECT jsonb_agg(to_jsonb(r)-'link_set_id' ORDER BY node_id,revision) FROM node_revisions r WHERE actor_id='legacy-author'),'history',(SELECT jsonb_agg(to_jsonb(h) ORDER BY sequence) FROM history_entries h WHERE actor_id='legacy-author'),'changesets',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM change_sets c WHERE actor_id='legacy-author'),'receipts',(SELECT jsonb_agg(to_jsonb(i) ORDER BY workspace_id,key) FROM idempotency_receipts i))").fetch_one(&mut *self.tx).await.unwrap()
    }
    async fn nodes(&mut self) -> Vec<Value> {
        sqlx::query_scalar("SELECT to_jsonb(n)-'search_document' FROM nodes n ORDER BY id")
            .fetch_all(&mut *self.tx)
            .await
            .unwrap()
    }
    async fn upgrade(&mut self) {
        sqlx::raw_sql(UPGRADE).execute(&mut *self.tx).await.unwrap();
    }
    async fn members(&mut self, node: u128) -> Vec<(Uuid, i64)> {
        sqlx::query_as("SELECT m.edge_id,m.edge_version FROM node_revisions r JOIN link_set_members m ON m.workspace_id=r.workspace_id AND m.set_id=r.link_set_id WHERE r.node_id=$1 ORDER BY r.revision DESC,m.edge_id").bind(Uuid::from_u128(node)).fetch_all(&mut *self.tx).await.unwrap()
    }
    async fn cleanup(self) {
        self.tx.rollback().await.unwrap();
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1)")
                .bind(&self.schema)
                .fetch_one(&self.pool)
                .await
                .unwrap();
        assert!(!exists);
        self.pool.close().await;
    }
}

#[tokio::test]
async fn migration_appends_attributed_baselines_for_both_kinds_and_preserves_legacy_history() {
    let mut f = Fixture::new().await;
    let history = f.legacy().await;
    let before = f.nodes().await;
    f.upgrade().await;
    assert_eq!(f.legacy().await, history);
    let after = f.nodes().await;
    assert_eq!(before.len(), after.len());
    for (old, new) in before.iter().zip(&after) {
        assert_eq!(
            new["version"].as_i64().unwrap(),
            old["version"].as_i64().unwrap() + 1
        );
        assert_eq!(
            new["content_revision"].as_i64().unwrap(),
            old["content_revision"].as_i64().unwrap() + 1
        );
        let mut old = old.clone();
        let mut new = new.clone();
        for field in ["version", "content_revision", "updated_at"] {
            old.as_object_mut().unwrap().remove(field);
            new.as_object_mut().unwrap().remove(field);
        }
        assert_eq!(
            old, new,
            "baseline must preserve durable fields/lifecycle/author/deletion"
        );
    }
    let baseline_count:i64=sqlx::query_scalar("SELECT count(*) FROM node_revisions WHERE actor_id='system:migration' AND operation='baseline' AND link_set_id IS NOT NULL").fetch_one(&mut *f.tx).await.unwrap();
    assert_eq!(baseline_count, 7);
    let snapshots_match:bool=sqlx::query_scalar("SELECT bool_and(r.snapshot=to_jsonb(n)-'search_document') FROM node_revisions r JOIN nodes n ON n.id=r.node_id WHERE r.actor_id='system:migration'").fetch_one(&mut *f.tx).await.unwrap();
    assert!(snapshots_match);
    let events:i64=sqlx::query_scalar("SELECT count(*) FROM history_entries WHERE actor_id='system:migration' AND operation='baseline'").fetch_one(&mut *f.tx).await.unwrap();
    assert_eq!(events, 7);
    let groups: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM change_sets WHERE actor_id='system:migration' AND status='applied'",
    )
    .fetch_one(&mut *f.tx)
    .await
    .unwrap();
    assert_eq!(
        groups, 2,
        "one atomic migration group per affected workspace"
    );
    f.cleanup().await;
}

#[tokio::test]
async fn migration_keeps_legacy_link_availability_null_instead_of_fabricating_old_membership() {
    let mut f = Fixture::new().await;
    f.upgrade().await;
    let rows: Vec<(i64, Option<Uuid>)> = sqlx::query_as(
        "SELECT revision,link_set_id FROM node_revisions WHERE node_id=$1 ORDER BY revision",
    )
    .bind(Uuid::from_u128(1))
    .fetch_all(&mut *f.tx)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], (1, None));
    assert_eq!(rows[1].0, 2);
    assert!(rows[1].1.is_some());
    let legacy:i64=sqlx::query_scalar("SELECT count(*) FROM node_revisions WHERE actor_id='legacy-author' AND link_set_id IS NULL").fetch_one(&mut *f.tx).await.unwrap();
    assert_eq!(legacy, 7);
    let empty: Uuid = sqlx::query_scalar(
        "SELECT link_set_id FROM node_revisions WHERE node_id=$1 AND revision=2",
    )
    .bind(Uuid::from_u128(5))
    .fetch_one(&mut *f.tx)
    .await
    .unwrap();
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM link_set_members WHERE set_id=$1")
        .bind(empty)
        .fetch_one(&mut *f.tx)
        .await
        .unwrap();
    assert_eq!(
        members, 0,
        "known empty baseline differs from unavailable legacy NULL"
    );
    f.cleanup().await;
}

#[tokio::test]
async fn migration_snapshots_outgoing_and_incoming_structural_edges_at_exact_versions() {
    let mut f = Fixture::new().await;
    let edges: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(e) FROM edges e WHERE NOT deleted ORDER BY id")
            .fetch_all(&mut *f.tx)
            .await
            .unwrap();
    f.upgrade().await;
    for (node, expected) in [
        (1, vec![(10, 3), (11, 2), (12, 1)]),
        (2, vec![(10, 3), (11, 2)]),
        (3, vec![(13, 1)]),
        (4, vec![(14, 1), (16, 1)]),
        (5, vec![]),
        (6, vec![(20, 1)]),
        (7, vec![]),
    ] {
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(id, version)| (Uuid::from_u128(id), version))
            .collect();
        assert_eq!(
            f.members(node).await,
            expected,
            "node {node} owns outgoing plus structural incoming only"
        );
    }
    let versions: Vec<Value> = sqlx::query_scalar("SELECT snapshot FROM edge_versions ORDER BY id")
        .fetch_all(&mut *f.tx)
        .await
        .unwrap();
    assert_eq!(
        versions, edges,
        "snapshots preserve exact identity, version, endpoints, pins, description, and timestamp"
    );
    // Later live edits cannot silently update old snapshots or membership.
    sqlx::query(
        "UPDATE edges SET version=version+1,description='changed later',deleted=true WHERE id=$1",
    )
    .bind(Uuid::from_u128(10))
    .execute(&mut *f.tx)
    .await
    .unwrap();
    let old: Value =
        sqlx::query_scalar("SELECT snapshot FROM edge_versions WHERE id=$1 AND version=3")
            .bind(Uuid::from_u128(10))
            .fetch_one(&mut *f.tx)
            .await
            .unwrap();
    assert_eq!(old["description"], "original edge description");
    assert_eq!(old["deleted"], false);
    assert_eq!(
        f.members(2).await,
        vec![(Uuid::from_u128(10), 3), (Uuid::from_u128(11), 2)]
    );
    f.cleanup().await;
}

#[tokio::test]
async fn migration_schema_enforces_workspace_and_record_ownership_of_snapshots() {
    let mut f = Fixture::new().await;
    f.upgrade().await;
    let set: Uuid = sqlx::query_scalar(
        "SELECT link_set_id FROM node_revisions WHERE node_id=$1 AND revision=2",
    )
    .bind(Uuid::from_u128(1))
    .fetch_one(&mut *f.tx)
    .await
    .unwrap();
    let foreign_set: Uuid = sqlx::query_scalar(
        "SELECT link_set_id FROM node_revisions WHERE node_id=$1 AND revision=2",
    )
    .bind(Uuid::from_u128(6))
    .fetch_one(&mut *f.tx)
    .await
    .unwrap();
    for (workspace, edge_id, version) in [
        (f.foreign, 20, 1),
        (f.workspace, 20, 1),
        (f.workspace, 16, 999),
    ] {
        sqlx::raw_sql("SAVEPOINT invalid_member")
            .execute(&mut *f.tx)
            .await
            .unwrap();
        let error=sqlx::query("INSERT INTO link_set_members(workspace_id,set_id,edge_id,edge_version) VALUES($1,$2,$3,$4)").bind(workspace).bind(set).bind(Uuid::from_u128(edge_id)).bind(version).execute(&mut *f.tx).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23503")
        );
        sqlx::raw_sql("ROLLBACK TO SAVEPOINT invalid_member; RELEASE SAVEPOINT invalid_member")
            .execute(&mut *f.tx)
            .await
            .unwrap();
    }
    for invalid in [foreign_set, set] {
        sqlx::raw_sql("SAVEPOINT invalid_owner")
            .execute(&mut *f.tx)
            .await
            .unwrap();
        let error =
            sqlx::query("UPDATE node_revisions SET link_set_id=$1 WHERE node_id=$2 AND revision=2")
                .bind(invalid)
                .bind(Uuid::from_u128(2))
                .execute(&mut *f.tx)
                .await
                .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23503")
        );
        sqlx::raw_sql("ROLLBACK TO SAVEPOINT invalid_owner; RELEASE SAVEPOINT invalid_owner")
            .execute(&mut *f.tx)
            .await
            .unwrap();
    }
    f.cleanup().await;
}
