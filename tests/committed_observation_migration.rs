//! Actual schema8→9 additive baseline, isolated and rolled back completely.
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;
#[tokio::test]
async fn observation_upgrade_preserves_old_domain_and_history_and_starts_new_baseline() {
    let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
        .expect("disposable UUID-owned Graph database required");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("observation_upgrade_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema};SET LOCAL search_path TO {schema},pg_catalog"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    for sql in [
        include_str!("../migrations/0001_initial.sql"),
        include_str!("../migrations/0002_workspace_credential_scope.sql"),
        include_str!("../migrations/0003_knowledge_pagination.sql"),
        include_str!("../migrations/0004_graph_contract.sql"),
        include_str!("../migrations/0005_knowledge_contract.sql"),
        include_str!("../migrations/0006_unified_revisions.sql"),
        include_str!("../migrations/0007_shared_node_fields.sql"),
        include_str!("../migrations/0008_ready_projection.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&mut *tx).await.unwrap();
    }
    let workspace = Uuid::new_v4();
    let node = Uuid::new_v4();
    let change = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces(id) VALUES($1)")
        .bind(workspace)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,created_at,updated_at) VALUES($1,$2,'task',1,1,'retained old task','','','active',clock_timestamp(),clock_timestamp())").bind(node).bind(workspace).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO change_sets(id,workspace_id,actor_id,operations,status) VALUES($1,$2,'original','[]','applied')").bind(change).bind(workspace).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,change_set_id) SELECT workspace_id,id,1,to_jsonb(n)-'search_document','original','created',$2 FROM nodes n WHERE id=$1").bind(node).bind(change).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,change_set_id,data) SELECT workspace_id,ARRAY[id],$2,'revision','original','created',$3,to_jsonb(n)-'search_document' FROM nodes n WHERE id=$1").bind(node).bind(Uuid::new_v4()).bind(change).execute(&mut *tx).await.unwrap();
    let snap = "SELECT jsonb_build_object('nodes',(SELECT jsonb_agg(to_jsonb(n) ORDER BY id) FROM nodes n),'revisions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY node_id,revision) FROM node_revisions r),'history',(SELECT jsonb_agg(to_jsonb(h) ORDER BY sequence) FROM history_entries h),'ready',(SELECT jsonb_agg(to_jsonb(p) ORDER BY node_id) FROM node_ready_projection p))";
    let before: Value = sqlx::query_scalar(snap).fetch_one(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0009_committed_observation.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let after: Value = sqlx::query_scalar(snap).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(before, after);
    let baseline: (i64, i64, i64) = sqlx::query_as(
        "SELECT sequence,batch_count,fact_count FROM graph_observation_heads WHERE workspace_id=$1",
    )
    .bind(workspace)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(baseline, (0, 0, 0));
    sqlx::query("UPDATE nodes SET task_state='completed',version=2,content_revision=2 WHERE id=$1")
        .bind(node)
        .execute(&mut *tx)
        .await
        .unwrap();
    let fact:(String,String,String)=sqlx::query_as("SELECT before_fact->>'logical_revision',after_fact->>'logical_revision',transition_class FROM graph_observation_facts WHERE workspace_id=$1").bind(workspace).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(fact, ("1".into(), "2".into(), "lifecycle_only".into()));
    tx.rollback().await.unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1)")
            .bind(&schema)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!exists);
    pool.close().await;
}
