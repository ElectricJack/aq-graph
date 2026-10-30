//! Upgrade invariants against isolated v3 schemas, never operator/AQ data.
//! Every fixture (including a rejected upgrade) is rolled back and its schema absence checked.
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

const V1: &str = include_str!("../migrations/0001_initial.sql");
const V2: &str = include_str!("../migrations/0002_workspace_credential_scope.sql");
const V3: &str = include_str!("../migrations/0003_knowledge_pagination.sql");
const GRAPH: &str = include_str!("../migrations/0004_graph_contract.sql");
const KNOWLEDGE: &str = include_str!("../migrations/0005_knowledge_contract.sql");

struct Fixture {
    pool: PgPool,
    tx: Transaction<'static, Postgres>,
    schema: String,
    workspace: Uuid,
}
impl Fixture {
    async fn new(nodes: i32) -> Self {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("set AQ_GRAPH_TEST_DATABASE_URL to a disposable PostgreSQL database");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let schema = format!("migration_contract_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE SCHEMA {schema}; SET LOCAL search_path TO {schema},pg_catalog; SET LOCAL statement_timeout='10s';")).execute(&mut *tx).await.unwrap();
        for migration in [V1, V2, V3] {
            sqlx::raw_sql(migration).execute(&mut *tx).await.unwrap();
        }
        let workspace = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces(id) VALUES($1)")
            .bind(workspace)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,created_at,updated_at) SELECT lpad(to_hex(g),32,'0')::uuid,$1,'task',1,1,'legacy task','','','active',clock_timestamp(),clock_timestamp() FROM generate_series(1,$2) g").bind(workspace).bind(nodes).execute(&mut *tx).await.unwrap();
        Self {
            pool,
            tx,
            schema,
            workspace,
        }
    }
    async fn edge(&mut self, source: u128, target: u128, kind: &str) {
        sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,version,created_at) VALUES($1,$2,$3,$4,$5,1,clock_timestamp())").bind(Uuid::new_v4()).bind(self.workspace).bind(Uuid::from_u128(source)).bind(Uuid::from_u128(target)).bind(kind).execute(&mut *self.tx).await.unwrap();
    }
    async fn knowledge(&mut self, id: u128, status: &str, retired: bool) {
        sqlx::query("UPDATE nodes SET kind='knowledge',task_state=NULL,knowledge_type='note',verification=$2,retired=$3 WHERE id=$1").bind(Uuid::from_u128(id)).bind(status).bind(retired).execute(&mut *self.tx).await.unwrap();
    }
    async fn apply(&mut self, migration: &str) {
        sqlx::raw_sql(migration)
            .execute(&mut *self.tx)
            .await
            .unwrap();
    }
    async fn reject(&mut self, migration: &str, message: &str) {
        sqlx::raw_sql("SAVEPOINT rejected_upgrade")
            .execute(&mut *self.tx)
            .await
            .unwrap();
        let error = sqlx::raw_sql(migration)
            .execute(&mut *self.tx)
            .await
            .expect_err("invalid legacy data must prevent upgrade");
        let database = error
            .as_database_error()
            .expect("migration refusal is a PostgreSQL exception");
        assert_eq!(database.code().as_deref(), Some("P0001"), "{error}");
        assert!(
            database.message().contains(message),
            "expected {message:?}, got {error}"
        );
        sqlx::raw_sql(
            "ROLLBACK TO SAVEPOINT rejected_upgrade; RELEASE SAVEPOINT rejected_upgrade;",
        )
        .execute(&mut *self.tx)
        .await
        .unwrap();
    }
    async fn has_column(&mut self, table: &str, column: &str) -> bool {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=$1 AND table_name=$2 AND column_name=$3)").bind(&self.schema).bind(table).bind(column).fetch_one(&mut *self.tx).await.unwrap()
    }
    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('nodes',(SELECT jsonb_agg(to_jsonb(n) ORDER BY id) FROM nodes n),'edges',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM edges e))").fetch_one(&mut *self.tx).await.unwrap()
    }
    async fn histories(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('revisions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY node_id,revision) FROM node_revisions r),'history',(SELECT jsonb_agg(to_jsonb(h) ORDER BY sequence) FROM history_entries h),'changesets',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM change_sets c),'receipts',(SELECT jsonb_agg(to_jsonb(i) ORDER BY key) FROM idempotency_receipts i))").fetch_one(&mut *self.tx).await.unwrap()
    }
    async fn cleanup(self) {
        self.tx.rollback().await.unwrap();
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1)")
                .bind(&self.schema)
                .fetch_one(&self.pool)
                .await
                .unwrap();
        assert!(!exists, "transaction fixture leaked its schema");
        self.pool.close().await;
    }
    async fn seed_history(&mut self) {
        let change = Uuid::new_v4();
        sqlx::query("INSERT INTO change_sets(id,workspace_id,actor_id,reason,operations,status) VALUES($1,$2,'original-author','legacy reason','[]','applied')").bind(change).bind(self.workspace).execute(&mut *self.tx).await.unwrap();
        for (id, revision, actor) in [
            (1u128, 1i64, "original-author"),
            (3, 1, "first-author"),
            (3, 2, "later-editor"),
        ] {
            sqlx::query("INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id) SELECT workspace_id,id,$2,to_jsonb(n)-'search_document',$3,'created','retained original payload',$4 FROM nodes n WHERE id=$1").bind(Uuid::from_u128(id)).bind(revision).bind(actor).bind(change).execute(&mut *self.tx).await.unwrap();
            sqlx::query("INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data) SELECT workspace_id,ARRAY[node_id],$3,'revision',actor_id,operation,reason,change_set_id,snapshot FROM node_revisions WHERE node_id=$1 AND revision=$2").bind(Uuid::from_u128(id)).bind(revision).bind(Uuid::new_v4()).execute(&mut *self.tx).await.unwrap();
        }
        sqlx::query("INSERT INTO idempotency_receipts(workspace_id,actor_id,operation,key,payload_hash,result) VALUES($1,'original-author','mutate','legacy-key','legacy-hash','{\"outcome\":\"retained\"}')").bind(self.workspace).execute(&mut *self.tx).await.unwrap();
    }
}

#[tokio::test]
async fn graph_upgrade_accepts_a_valid_forest_and_adds_description_without_history_rewrite() {
    let mut f = Fixture::new(4).await;
    f.edge(1, 2, "parent-child").await;
    f.edge(2, 3, "blocks").await;
    f.seed_history().await;
    let before = f.histories().await;
    f.apply(GRAPH).await;
    assert!(f.has_column("edges", "description").await);
    assert_eq!(f.histories().await, before);
    let index:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_indexes WHERE schemaname=$1 AND indexname='edges_one_active_parent')").bind(&f.schema).fetch_one(&mut *f.tx).await.unwrap();
    assert!(index);
    f.cleanup().await;
}

#[tokio::test]
async fn graph_upgrade_refuses_multiple_parents_without_choosing_one() {
    let mut f = Fixture::new(3).await;
    f.edge(1, 3, "parent-child").await;
    f.edge(2, 3, "parent-child").await;
    let before = f.snapshot().await;
    f.reject(GRAPH, "multiple live parents").await;
    assert_eq!(f.snapshot().await, before);
    assert!(!f.has_column("edges", "description").await);
    f.cleanup().await;
}

#[tokio::test]
async fn graph_upgrade_refuses_fourth_level_without_dropping_membership() {
    let mut f = Fixture::new(4).await;
    for (a, b) in [(1, 2), (2, 3), (3, 4)] {
        f.edge(a, b, "parent-child").await;
    }
    let before = f.snapshot().await;
    f.reject(GRAPH, "exceeds three levels").await;
    assert_eq!(f.snapshot().await, before);
    f.cleanup().await;
}

#[tokio::test]
async fn graph_upgrade_refuses_invalid_endpoint_kinds_without_reinterpreting_relationships() {
    let mut f = Fixture::new(2).await;
    f.edge(1, 2, "produces").await;
    let before = f.snapshot().await;
    f.reject(GRAPH, "invalid active relationship endpoint")
        .await;
    assert_eq!(f.snapshot().await, before);
    f.cleanup().await;
}

#[tokio::test]
async fn graph_upgrade_refuses_a_cycle_spanning_individually_acyclic_families() {
    let mut f = Fixture::new(2).await;
    f.edge(1, 2, "blocks").await;
    f.edge(2, 1, "parent-child").await;
    let before = f.snapshot().await;
    f.reject(GRAPH, "combined blocking/structural cycle").await;
    assert_eq!(f.snapshot().await, before);
    f.cleanup().await;
}

#[tokio::test]
async fn graph_upgrade_walks_a_valid_three_hundred_hop_blocking_chain() {
    let mut f = Fixture::new(301).await;
    for id in 1..=300 {
        f.edge(id, id + 1, "blocks").await;
    }
    f.apply(GRAPH).await;
    assert!(f.has_column("edges", "description").await);
    f.cleanup().await;
}

#[tokio::test]
async fn graph_upgrade_refuses_a_three_hundred_hop_mixed_cycle() {
    let mut f = Fixture::new(301).await;
    for id in 1..=300 {
        f.edge(id, id + 1, "blocks").await;
    }
    f.edge(301, 1, "parent-child").await;
    let before = f.snapshot().await;
    f.reject(GRAPH, "combined blocking/structural cycle").await;
    assert_eq!(f.snapshot().await, before);
    f.cleanup().await;
}

#[tokio::test]
async fn knowledge_upgrade_recovers_only_evidenced_creation_authors_and_preserves_all_history() {
    let mut f = Fixture::new(3).await;
    for id in 1..=3 {
        f.knowledge(id, "unverified", false).await;
    }
    f.seed_history().await;
    f.apply(GRAPH).await;
    let before = f.histories().await;
    f.apply(KNOWLEDGE).await;
    assert_eq!(f.histories().await, before);
    let authors: Vec<String> = sqlx::query_scalar("SELECT created_by FROM nodes ORDER BY id")
        .fetch_all(&mut *f.tx)
        .await
        .unwrap();
    assert_eq!(
        authors,
        vec!["original-author", "legacy:unknown", "first-author"]
    );
    let attributed:i64=sqlx::query_scalar("SELECT count(*) FROM nodes WHERE last_verified_by IS NOT NULL OR last_verified_at IS NOT NULL OR retirement_reason IS NOT NULL").fetch_one(&mut *f.tx).await.unwrap();
    assert_eq!(
        attributed, 0,
        "upgrade must not invent verification or retirement claims"
    );
    f.cleanup().await;
}

#[tokio::test]
async fn knowledge_upgrade_refuses_unattributed_verified_and_disputed_assertions_atomically() {
    for status in ["verified", "disputed"] {
        let mut f = Fixture::new(3).await;
        f.knowledge(1, status, false).await;
        f.seed_history().await;
        f.apply(GRAPH).await;
        let before = f.snapshot().await;
        let history = f.histories().await;
        f.reject(KNOWLEDGE, "lack verification attribution").await;
        assert_eq!(f.snapshot().await, before);
        assert_eq!(f.histories().await, history);
        assert!(
            !f.has_column("nodes", "created_by").await,
            "failed migration must roll back its preceding author backfill and DDL"
        );
        f.cleanup().await;
    }
}

#[tokio::test]
async fn knowledge_upgrade_refuses_unexplained_legacy_retirement_without_losing_content() {
    let mut f = Fixture::new(3).await;
    f.knowledge(1, "unverified", true).await;
    f.seed_history().await;
    f.apply(GRAPH).await;
    let before = f.snapshot().await;
    let history = f.histories().await;
    f.reject(KNOWLEDGE, "lack retirement reasons").await;
    assert_eq!(f.snapshot().await, before);
    assert_eq!(f.histories().await, history);
    assert!(!f.has_column("nodes", "created_by").await);
    f.cleanup().await;
}
