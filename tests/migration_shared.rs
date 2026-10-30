//! Shared fields upgrade: real migration SQL in rollback-only private schemas.
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;
const BASE: [&str; 6] = [
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_workspace_credential_scope.sql"),
    include_str!("../migrations/0003_knowledge_pagination.sql"),
    include_str!("../migrations/0004_graph_contract.sql"),
    include_str!("../migrations/0005_knowledge_contract.sql"),
    include_str!("../migrations/0006_unified_revisions.sql"),
];
const UPGRADE: &str = include_str!("../migrations/0007_shared_node_fields.sql");
struct Fixture {
    pool: PgPool,
    tx: Transaction<'static, Postgres>,
    schema: String,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("set AQ_GRAPH_TEST_DATABASE_URL to disposable PostgreSQL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let schema = format!("shared_migration_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE SCHEMA {schema}; SET LOCAL search_path TO {schema},pg_catalog; SET LOCAL statement_timeout='10s';")).execute(&mut *tx).await.unwrap();
        for migration in &BASE[..5] {
            sqlx::raw_sql(migration).execute(&mut *tx).await.unwrap();
        }
        sqlx::raw_sql("INSERT INTO workspaces(id) VALUES('11111111-1111-4111-8111-111111111111'),('22222222-2222-4222-8222-222222222222'); INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,knowledge_type,verification,created_at,updated_at,created_by) SELECT lpad(to_hex(g),32,'0')::uuid,CASE WHEN g=4 THEN '22222222-2222-4222-8222-222222222222'::uuid ELSE '11111111-1111-4111-8111-111111111111'::uuid END,CASE WHEN g=3 THEN 'knowledge' ELSE 'task' END,1,1,'Authentication record '||g,'preserved body','',CASE WHEN g=3 THEN NULL WHEN g=2 THEN 'completed' ELSE 'active' END,CASE WHEN g=3 THEN 'decision' END,CASE WHEN g=3 THEN 'unverified' END,'2026-01-01Z'::timestamptz,'2026-01-02Z'::timestamptz,'original-author' FROM generate_series(1,4) g;").execute(&mut *tx).await.unwrap();
        let change = Uuid::new_v4();
        sqlx::query("INSERT INTO change_sets(id,workspace_id,actor_id,operations,status) VALUES($1,'11111111-1111-4111-8111-111111111111','original-author','[]','applied')").bind(change).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,change_set_id) SELECT workspace_id,id,1,to_jsonb(n)-'search_document','original-author','created',$1 FROM nodes n").bind(change).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,version,created_at) VALUES($1,'11111111-1111-4111-8111-111111111111',$2,$3,'blocks',1,clock_timestamp())").bind(Uuid::from_u128(10)).bind(Uuid::from_u128(1)).bind(Uuid::from_u128(2)).execute(&mut *tx).await.unwrap();
        sqlx::raw_sql(BASE[5]).execute(&mut *tx).await.unwrap();
        Self { pool, tx, schema }
    }
    async fn upgrade(&mut self) {
        sqlx::raw_sql(UPGRADE).execute(&mut *self.tx).await.unwrap();
    }
    async fn histories(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('revisions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY node_id,revision) FROM node_revisions r WHERE operation<>'shared_fields_baseline'),'history',(SELECT jsonb_agg(to_jsonb(h) ORDER BY sequence) FROM history_entries h WHERE operation<>'shared_fields_baseline'),'changesets',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM change_sets c WHERE name IS DISTINCT FROM 'Shared node field baseline'),'links',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM link_sets s),'members',(SELECT jsonb_agg(to_jsonb(m) ORDER BY set_id,edge_id) FROM link_set_members m),'edge_versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY id,version) FROM edge_versions v))").fetch_one(&mut *self.tx).await.unwrap()
    }
    async fn nodes(&mut self) -> Vec<Value> {
        sqlx::query_scalar("SELECT to_jsonb(n)-'search_document' FROM nodes n ORDER BY id")
            .fetch_all(&mut *self.tx)
            .await
            .unwrap()
    }
    async fn reject_sql(&mut self, sql: &str, code: &str) {
        sqlx::raw_sql("SAVEPOINT invalid_change")
            .execute(&mut *self.tx)
            .await
            .unwrap();
        let error = sqlx::raw_sql(sql)
            .execute(&mut *self.tx)
            .await
            .expect_err("constraint must reject invalid shared fields");
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some(code),
            "{error}"
        );
        sqlx::raw_sql("ROLLBACK TO SAVEPOINT invalid_change; RELEASE SAVEPOINT invalid_change")
            .execute(&mut *self.tx)
            .await
            .unwrap();
    }
    async fn cleanup(self) {
        self.tx.rollback().await.unwrap();
        let remains: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1)")
                .bind(self.schema)
                .fetch_one(&self.pool)
                .await
                .unwrap();
        assert!(!remains);
        self.pool.close().await;
    }
}

#[tokio::test]
async fn task_defaults_receive_new_baselines_without_rewriting_history_or_owned_links() {
    let mut f = Fixture::new().await;
    let history = f.histories().await;
    let before = f.nodes().await;
    f.upgrade().await;
    assert_eq!(f.histories().await, history);
    let after = f.nodes().await;
    for (old, new) in before.iter().zip(after.iter()) {
        assert!(new.get("knowledge_type").is_none());
        let task = old["kind"] == "task";
        assert_eq!(
            new["version"].as_i64().unwrap(),
            old["version"].as_i64().unwrap() + i64::from(task)
        );
        assert_eq!(
            new["content_revision"].as_i64().unwrap(),
            old["content_revision"].as_i64().unwrap() + i64::from(task)
        );
        assert_eq!(
            new["content_type"],
            if task {
                Value::String("note".into())
            } else {
                old["knowledge_type"].clone()
            }
        );
        assert_eq!(new["verification"], "unverified");
        for field in [
            "id",
            "workspace_id",
            "kind",
            "title",
            "body",
            "task_state",
            "created_at",
            "created_by",
            "deleted",
            "provenance",
        ] {
            assert_eq!(new[field], old[field], "{field}");
        }
        for field in [
            "summary",
            "summary_of_revision",
            "valid_from",
            "valid_until",
            "review_at",
            "successor_id",
        ] {
            assert!(new[field].is_null());
        }
    }
    let baseline:i64=sqlx::query_scalar("SELECT count(*) FROM node_revisions WHERE operation='shared_fields_baseline' AND actor_id='system:migration'").fetch_one(&mut *f.tx).await.unwrap();
    assert_eq!(baseline, 3);
    let same_links:bool=sqlx::query_scalar("SELECT bool_and(new.link_set_id=old.link_set_id) FROM node_revisions new JOIN node_revisions old ON old.node_id=new.node_id AND old.revision=new.revision-1 WHERE new.operation='shared_fields_baseline'").fetch_one(&mut *f.tx).await.unwrap();
    assert!(same_links);
    let predicate:String=sqlx::query_scalar("SELECT pg_get_expr(indpred,indrelid) FROM pg_index WHERE indexrelid='nodes_search'::regclass").fetch_one(&mut *f.tx).await.unwrap();
    assert!(
        !predicate.contains("knowledge"),
        "full-text index must include task content too"
    );
    f.cleanup().await;
}

#[tokio::test]
async fn shared_constraints_enforce_validity_summary_sources_and_successor_scope() {
    let mut f = Fixture::new().await;
    f.upgrade().await;
    for sql in [
        "UPDATE nodes SET valid_from='2026-01-01Z',valid_until='2026-01-01Z' WHERE id='00000000-0000-0000-0000-000000000001'",
        "UPDATE nodes SET valid_from='2026-01-02Z',valid_until='2026-01-01Z' WHERE id='00000000-0000-0000-0000-000000000001'",
        "UPDATE nodes SET summary='unpaired' WHERE id='00000000-0000-0000-0000-000000000001'",
        "UPDATE nodes SET summary=' ',summary_of_revision=2 WHERE id='00000000-0000-0000-0000-000000000001'",
        "UPDATE nodes SET summary=repeat('é',4097),summary_of_revision=2 WHERE id='00000000-0000-0000-0000-000000000001'",
        "UPDATE nodes SET summary='current is not an earlier source',summary_of_revision=3 WHERE id='00000000-0000-0000-0000-000000000001'",
        "UPDATE nodes SET successor_id='00000000-0000-0000-0000-000000000002' WHERE id='00000000-0000-0000-0000-000000000001'",
        "UPDATE nodes SET retired=true,retirement_reason='why',successor_id=id WHERE id='00000000-0000-0000-0000-000000000001'",
    ] {
        f.reject_sql(sql, "23514").await;
    }
    f.reject_sql("UPDATE nodes SET retired=true,retirement_reason='why',successor_id='00000000-0000-0000-0000-000000000004' WHERE id='00000000-0000-0000-0000-000000000001'","23503").await;
    // Revision 3 exists for task 1, but not for knowledge 3: a summary cannot
    // borrow another record's revision just because its integer is available.
    f.reject_sql("UPDATE nodes SET content_revision=4,summary='wrong record source',summary_of_revision=3 WHERE id='00000000-0000-0000-0000-000000000003'","23503").await;
    sqlx::raw_sql("UPDATE nodes SET summary='valid summary',summary_of_revision=2,valid_from='2026-01-01Z',valid_until='2026-01-02Z',review_at='2026-01-01 12:00Z' WHERE id='00000000-0000-0000-0000-000000000001'; UPDATE nodes SET summary=NULL,summary_of_revision=NULL,valid_from=NULL,valid_until=NULL,review_at=NULL WHERE id='00000000-0000-0000-0000-000000000001'; UPDATE nodes SET retired=true,retirement_reason='superseded',successor_id='00000000-0000-0000-0000-000000000002' WHERE id='00000000-0000-0000-0000-000000000001';").execute(&mut *f.tx).await.unwrap();
    f.cleanup().await;
}

#[tokio::test]
async fn shared_task_verification_requires_real_attribution_and_common_evidence() {
    let mut f = Fixture::new().await;
    f.upgrade().await;
    f.reject_sql(
        "UPDATE nodes SET verification='verified' WHERE id='00000000-0000-0000-0000-000000000001'",
        "23514",
    )
    .await;
    sqlx::raw_sql("UPDATE nodes SET verification='verified',last_verified_by='admin',last_verified_at=clock_timestamp(),provenance='[{\"source_id\":\"e\",\"kind\":\"url\",\"url\":\"https://example.invalid/evidence\",\"observed_at\":\"2026-10-30T00:00:00Z\",\"retained\":false}]',content_type='procedure' WHERE id='00000000-0000-0000-0000-000000000001'").execute(&mut *f.tx).await.unwrap();
    let state: String = sqlx::query_scalar("SELECT verification FROM nodes WHERE id=$1")
        .bind(Uuid::from_u128(1))
        .fetch_one(&mut *f.tx)
        .await
        .unwrap();
    assert_eq!(state, "verified");
    f.cleanup().await;
}

#[tokio::test]
async fn shared_upgrade_refuses_to_fabricate_a_missing_current_link_snapshot() {
    let mut f = Fixture::new().await;
    sqlx::query("UPDATE node_revisions SET link_set_id=NULL WHERE node_id=$1 AND revision=2")
        .bind(Uuid::from_u128(1))
        .execute(&mut *f.tx)
        .await
        .unwrap();
    let before = f.nodes().await;
    let history = f.histories().await;
    f.reject_sql(UPGRADE, "P0001").await;
    assert_eq!(f.nodes().await, before);
    assert_eq!(f.histories().await, history);
    let old_column:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=$1 AND table_name='nodes' AND column_name='knowledge_type')").bind(&f.schema).fetch_one(&mut *f.tx).await.unwrap();
    assert!(
        old_column,
        "failed upgrade must roll back its column rename and DDL"
    );
    f.cleanup().await;
}
