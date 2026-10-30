//! Derived ready state survives backfill and every underlying SQL mutation path.
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn projection_backfill_and_mutations_match_source_without_changing_history() {
    let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("ready_projection_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema}; SET LOCAL search_path TO {schema},pg_catalog"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    for migration in [
        include_str!("../migrations/0001_initial.sql"),
        include_str!("../migrations/0002_workspace_credential_scope.sql"),
        include_str!("../migrations/0003_knowledge_pagination.sql"),
        include_str!("../migrations/0004_graph_contract.sql"),
        include_str!("../migrations/0005_knowledge_contract.sql"),
        include_str!("../migrations/0006_unified_revisions.sql"),
        include_str!("../migrations/0007_shared_node_fields.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&mut *tx).await.unwrap();
    }
    sqlx::raw_sql("INSERT INTO workspaces(id) VALUES('00000000-0000-0000-0000-000000000001');
INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,content_type,verification,created_at,updated_at)
SELECT lpad(to_hex(g),32,'0')::uuid,'00000000-0000-0000-0000-000000000001','task',1,1,'Task','','','active','note','unverified',clock_timestamp(),clock_timestamp() FROM generate_series(1,5) g;
INSERT INTO edges(id,workspace_id,source,target,edge_type,version,created_at) VALUES('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000003','parent-child',1,clock_timestamp());")
        .execute(&mut *tx).await.unwrap();
    let change = Uuid::new_v4();
    sqlx::query("INSERT INTO change_sets(id,workspace_id,actor_id,operations,status) VALUES($1,'00000000-0000-0000-0000-000000000001','migration-test','[]','applied')").bind(change).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,change_set_id) SELECT workspace_id,id,1,to_jsonb(n)-'search_document','migration-test','created',$1 FROM nodes n").bind(change).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,change_set_id,data) SELECT workspace_id,ARRAY[id],gen_random_uuid(),'revision','migration-test','created',$1,to_jsonb(n)-'search_document' FROM nodes n").bind(change).execute(&mut *tx).await.unwrap();
    let before: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('nodes',(SELECT jsonb_agg(to_jsonb(n) ORDER BY id) FROM nodes n),'revisions',(SELECT jsonb_agg(to_jsonb(r)) FROM node_revisions r),'history',(SELECT jsonb_agg(to_jsonb(h)) FROM history_entries h))").fetch_one(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0008_ready_projection.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let after: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('nodes',(SELECT jsonb_agg(to_jsonb(n) ORDER BY id) FROM nodes n),'revisions',(SELECT jsonb_agg(to_jsonb(r)) FROM node_revisions r),'history',(SELECT jsonb_agg(to_jsonb(h)) FROM history_entries h))").fetch_one(&mut *tx).await.unwrap();
    assert_eq!(before, after);
    let guards_before: Vec<i64> =
        sqlx::query_scalar("SELECT guard FROM node_ready_projection ORDER BY node_id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    sqlx::raw_sql("UPDATE nodes SET title='Body-only edit',task_state=task_state,deleted=deleted,retired=retired").execute(&mut *tx).await.unwrap();
    let guards_after: Vec<i64> =
        sqlx::query_scalar("SELECT guard FROM node_ready_projection ORDER BY node_id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        guards_before, guards_after,
        "ordinary persisted content must not churn projection rows"
    );
    for mutation in [
        "SELECT 1",
        "UPDATE nodes SET deleted=true WHERE id='00000000-0000-0000-0000-000000000003'",
        "UPDATE nodes SET deleted=false WHERE id='00000000-0000-0000-0000-000000000003'",
        "UPDATE edges SET source='00000000-0000-0000-0000-000000000002'",
        "UPDATE edges SET target='00000000-0000-0000-0000-000000000004'",
        "UPDATE edges SET deleted=true",
        "UPDATE edges SET deleted=false",
        "UPDATE edges SET edge_type='blocks'",
        "UPDATE edges SET edge_type='parent-child'",
        "UPDATE nodes SET retired=true,retirement_reason='projection test',task_state='draft' WHERE id='00000000-0000-0000-0000-000000000002'",
        "DELETE FROM edges",
        "INSERT INTO edges(id,workspace_id,source,target,edge_type,version,created_at) VALUES('00000000-0000-0000-0000-000000000011','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000003','parent-child',1,clock_timestamp())",
        "UPDATE nodes SET deleted=true WHERE id IN ('00000000-0000-0000-0000-000000000003','00000000-0000-0000-0000-000000000004')",
        "UPDATE nodes SET deleted=false WHERE deleted",
    ] {
        sqlx::raw_sql(mutation).execute(&mut *tx).await.unwrap();
        let valid: bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM nodes n FULL JOIN node_ready_projection p ON p.workspace_id=n.workspace_id AND p.node_id=n.id WHERE n.id IS NULL OR p.node_id IS NULL OR p.eligible IS DISTINCT FROM (n.kind='task' AND n.task_state='active' AND NOT n.deleted AND NOT n.retired) OR p.live_child_count IS DISTINCT FROM (SELECT count(*) FROM edges e JOIN nodes c ON c.workspace_id=e.workspace_id AND c.id=e.target WHERE e.workspace_id=n.workspace_id AND e.source=n.id AND e.edge_type='parent-child' AND NOT e.deleted AND NOT c.deleted))").fetch_one(&mut *tx).await.unwrap();
        assert!(valid, "projection drift after {mutation}");
    }
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn concurrent_child_lifecycle_and_edges_serialize_without_lost_counts() {
    use aq_graph::store::Store;
    let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&url, 4).await.unwrap();
    store.migrate().await.unwrap();
    let workspace = Uuid::new_v4();
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces(id) VALUES($1)")
        .bind(workspace)
        .execute(&store.pool)
        .await
        .unwrap();
    for id in [parent, child] {
        sqlx::query("INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,content_type,verification,created_at,updated_at) VALUES($1,$2,'task',1,1,'Concurrent task','','','active','note','unverified',clock_timestamp(),clock_timestamp())")
            .bind(id).bind(workspace).execute(&store.pool).await.unwrap();
    }
    // Hold the edge contribution guard, then race deletion. The latter must
    // observe the newly committed edge and remove exactly its contribution.
    let edge = Uuid::new_v4();
    let mut tx = store.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,version,created_at) VALUES($1,$2,$3,$4,'parent-child',1,clock_timestamp())")
        .bind(edge).bind(workspace).bind(parent).bind(child).execute(&mut *tx).await.unwrap();
    let owner: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut competing = store.pool.acquire().await.unwrap();
    let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *competing)
        .await
        .unwrap();
    let deletion = tokio::spawn(async move {
        sqlx::query("UPDATE nodes SET deleted=true WHERE id=$1")
            .bind(child)
            .execute(&mut *competing)
            .await
            .unwrap();
    });
    wait_for_blocker(&store.pool, waiter, owner).await;
    tx.commit().await.unwrap();
    deletion.await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT live_child_count FROM node_ready_projection WHERE workspace_id=$1 AND node_id=$2",
    )
    .bind(workspace)
    .bind(parent)
    .fetch_one(&store.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    // Reverse the overlap: resurrection owns the guard before edge deletion.
    let mut tx = store.pool.begin().await.unwrap();
    sqlx::query("UPDATE nodes SET deleted=false WHERE id=$1")
        .bind(child)
        .execute(&mut *tx)
        .await
        .unwrap();
    let owner: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut competing = store.pool.acquire().await.unwrap();
    let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *competing)
        .await
        .unwrap();
    let deletion = tokio::spawn(async move {
        sqlx::query("DELETE FROM edges WHERE id=$1")
            .bind(edge)
            .execute(&mut *competing)
            .await
            .unwrap();
    });
    wait_for_blocker(&store.pool, waiter, owner).await;
    tx.commit().await.unwrap();
    deletion.await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT live_child_count FROM node_ready_projection WHERE workspace_id=$1 AND node_id=$2",
    )
    .bind(workspace)
    .bind(parent)
    .fetch_one(&store.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    sqlx::query("DELETE FROM nodes WHERE workspace_id=$1")
        .bind(workspace)
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workspaces WHERE id=$1")
        .bind(workspace)
        .execute(&store.pool)
        .await
        .unwrap();
}

async fn wait_for_blocker(pool: &sqlx::PgPool, waiter: i32, owner: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT $2 = ANY(pg_blocking_pids($1))")
                .bind(waiter)
                .bind(owner)
                .fetch_one(pool)
                .await
                .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("competing mutation must reach the held database lock");
}

#[tokio::test]
async fn administrative_check_and_rebuild_repair_missing_and_corrupt_rows() {
    let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let schema = format!("ready_rebuild_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema}; SET search_path TO {schema},pg_catalog; BEGIN;"
    ))
    .execute(&mut *conn)
    .await
    .unwrap();
    for migration in [
        include_str!("../migrations/0001_initial.sql"),
        include_str!("../migrations/0002_workspace_credential_scope.sql"),
        include_str!("../migrations/0003_knowledge_pagination.sql"),
        include_str!("../migrations/0004_graph_contract.sql"),
        include_str!("../migrations/0005_knowledge_contract.sql"),
        include_str!("../migrations/0006_unified_revisions.sql"),
        include_str!("../migrations/0007_shared_node_fields.sql"),
        include_str!("../migrations/0008_ready_projection.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&mut *conn).await.unwrap();
    }
    sqlx::raw_sql("INSERT INTO workspaces(id) VALUES('00000000-0000-0000-0000-000000000001');
INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,content_type,verification,created_at,updated_at)
SELECT lpad(to_hex(g),32,'0')::uuid,'00000000-0000-0000-0000-000000000001','task',1,1,'Repair task','','','active','note','unverified',clock_timestamp(),clock_timestamp() FROM generate_series(1,3) g;
INSERT INTO edges(id,workspace_id,source,target,edge_type,version,created_at) VALUES('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000002','parent-child',1,clock_timestamp());
INSERT INTO change_sets(id,workspace_id,actor_id,operations,status) VALUES('00000000-0000-0000-0000-000000000020','00000000-0000-0000-0000-000000000001','repair-test','[]','applied');
INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,change_set_id) SELECT workspace_id,id,1,to_jsonb(n)-'search_document','repair-test','created','00000000-0000-0000-0000-000000000020' FROM nodes n;
INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,change_set_id,data) SELECT workspace_id,ARRAY[id],gen_random_uuid(),'revision','repair-test','created','00000000-0000-0000-0000-000000000020',to_jsonb(n)-'search_document' FROM nodes n;
COMMIT;")
        .execute(&mut *conn).await.unwrap();
    let snapshot = "SELECT jsonb_build_object('nodes',(SELECT jsonb_agg(to_jsonb(n) ORDER BY id) FROM nodes n),'revisions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY node_id,revision) FROM node_revisions r),'history',(SELECT jsonb_agg(to_jsonb(h) ORDER BY sequence) FROM history_entries h))";
    let before: serde_json::Value = sqlx::query_scalar(snapshot)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let check = include_str!("../scripts/check-ready-projection.sql");
    assert!(
        sqlx::query(check)
            .fetch_all(&mut *conn)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::raw_sql("UPDATE node_ready_projection SET live_child_count=99,eligible=false WHERE node_id='00000000-0000-0000-0000-000000000001'; DELETE FROM node_ready_projection WHERE node_id='00000000-0000-0000-0000-000000000003';").execute(&mut *conn).await.unwrap();
    assert_eq!(
        sqlx::query(check)
            .fetch_all(&mut *conn)
            .await
            .unwrap()
            .len(),
        2
    );
    // Execute the actual standalone administrative script, including its lock,
    // transaction boundaries and ANALYZE, on this private schema connection.
    sqlx::raw_sql(include_str!("../scripts/rebuild-ready-projection.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        sqlx::query(check)
            .fetch_all(&mut *conn)
            .await
            .unwrap()
            .is_empty()
    );
    let after: serde_json::Value = sqlx::query_scalar(snapshot)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(before, after);
    sqlx::raw_sql(&format!(
        "SET search_path TO public; DROP SCHEMA {schema} CASCADE"
    ))
    .execute(&mut *conn)
    .await
    .unwrap();
}
