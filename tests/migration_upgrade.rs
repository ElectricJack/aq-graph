//! A real schema-7 database upgrades without rewriting any durable graph data.
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn upgrade_from_previous_schema_preserves_data() {
    let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
        .expect("set AQ_GRAPH_TEST_DATABASE_URL to disposable PostgreSQL");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("release_upgrade_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema}; SET LOCAL search_path TO {schema},pg_catalog; SET LOCAL statement_timeout='10s';"
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
    sqlx::raw_sql(
        r#"
INSERT INTO workspaces(id) VALUES
 ('11111111-1111-4111-8111-111111111111'),('22222222-2222-4222-8222-222222222222');
INSERT INTO credentials(workspace_id,actor_id,token_hash,capability)
SELECT id,'original-author',md5(id::text)||md5(id::text),'admin' FROM workspaces;
INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,
 task_state,content_type,verification,retired,retirement_reason,deleted,created_at,updated_at,
 created_by,labels,metadata,provenance,last_verified_by,last_verified_at)
SELECT lpad(to_hex(g),32,'0')::uuid,
 CASE WHEN g=12 THEN '22222222-2222-4222-8222-222222222222'::uuid ELSE '11111111-1111-4111-8111-111111111111'::uuid END,
 CASE WHEN g IN (8,9) THEN 'knowledge' ELSE 'task' END,2,2,'Record '||g,'Preserve body '||g,
 CASE WHEN g IN (8,9) THEN '' ELSE 'Keep acceptance criteria' END,
 CASE WHEN g IN (8,9) THEN NULL WHEN g=3 THEN 'completed' WHEN g=4 THEN 'canceled' WHEN g=5 THEN 'draft' ELSE 'active' END,
 CASE WHEN g=8 THEN 'decision' ELSE 'note' END,
 CASE WHEN g=8 THEN 'verified' ELSE 'unverified' END,g IN (7,9),
 CASE WHEN g IN (7,9) THEN 'Superseded record' END,g=6,
 '2026-01-01Z'::timestamptz,'2026-02-01Z'::timestamptz,'original-author','["release-fixture"]','{"preserve":true}',
 CASE WHEN g=8 THEN '[{"source_id":"evidence","kind":"url","url":"https://example.invalid/evidence","observed_at":"2026-01-01T00:00:00Z","retained":false}]'::jsonb ELSE '[]'::jsonb END,
 CASE WHEN g=8 THEN 'verifier' END,CASE WHEN g=8 THEN '2026-01-02Z'::timestamptz END
FROM generate_series(1,12) g;
INSERT INTO change_sets(id,workspace_id,actor_id,name,reason,operations,status,result,applied_at)
SELECT id,id,'original-author','Historical changes','fixture','[]','applied','{"preserve":true}','2026-02-01Z' FROM workspaces;
INSERT INTO change_sets(id,workspace_id,actor_id,name,operations,status)
VALUES('33333333-3333-4333-8333-333333333333','11111111-1111-4111-8111-111111111111','writer','Unapplied draft','[]','draft');
INSERT INTO link_sets(id,workspace_id,node_id) SELECT id,workspace_id,id FROM nodes;
INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id,link_set_id)
SELECT n.workspace_id,n.id,r,(to_jsonb(n)-'search_document')||jsonb_build_object('version',r,'content_revision',r),
 'original-author',CASE WHEN r=1 THEN 'created' ELSE 'updated' END,'Preserve historical reason',n.workspace_id,n.id
FROM nodes n CROSS JOIN generate_series(1,2) r;
UPDATE nodes SET summary='Historical decision summary',summary_of_revision=1,
 valid_from='2026-01-01Z',valid_until='2027-01-01Z',review_at='2026-12-01Z'
WHERE id='00000000-0000-0000-0000-000000000008';
UPDATE node_revisions r SET snapshot=to_jsonb(n)-'search_document' FROM nodes n
WHERE r.node_id=n.id AND r.revision=n.content_revision;
INSERT INTO edges(id,workspace_id,source,target,edge_type,version,deleted,created_at,target_revision)
SELECT lpad(to_hex(100+e),32,'0')::uuid,'11111111-1111-4111-8111-111111111111',
 lpad(to_hex(s),32,'0')::uuid,lpad(to_hex(t),32,'0')::uuid,kind,1,dead,'2026-02-01Z',
 CASE WHEN kind='references' THEN 1 END
FROM (VALUES (1,1,2,'parent-child',false),(2,1,3,'parent-child',false),
 (3,10,6,'parent-child',false),(4,11,5,'parent-child',true),
 (5,3,2,'blocks',false),(6,2,8,'references',false)) AS fixture(e,s,t,kind,dead);
INSERT INTO edge_versions(workspace_id,id,version,snapshot) SELECT workspace_id,id,version,to_jsonb(e) FROM edges e;
INSERT INTO link_set_members(workspace_id,set_id,edge_id,edge_version)
SELECT n.workspace_id,n.id,e.id,e.version FROM nodes n JOIN edges e ON e.workspace_id=n.workspace_id
 AND (e.source=n.id OR (e.target=n.id AND e.edge_type IN ('blocks','parent-child'))) WHERE NOT e.deleted;
INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data)
SELECT workspace_id,ARRAY[node_id],gen_random_uuid(),'revision',actor_id,operation,reason,change_set_id,snapshot FROM node_revisions;
INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,change_set_id,data)
SELECT workspace_id,ARRAY[source,target],gen_random_uuid(),'edge','original-author','created',workspace_id,to_jsonb(e) FROM edges e;
INSERT INTO idempotency_receipts(workspace_id,actor_id,operation,key,payload_hash,result)
SELECT id,'original-author','apply','upgrade-replay',repeat('a',64),'{"preserve":"receipt"}' FROM workspaces;
"#,
    )
    .execute(&mut *tx)
    .await
    .unwrap();

    // Full rows include CAS versions, timestamps, summaries, provenance, search
    // vectors, tombstones and immutable relationships, rather than just counts.
    let tables = [
        "workspaces",
        "credentials",
        "nodes",
        "edges",
        "node_revisions",
        "edge_versions",
        "link_sets",
        "link_set_members",
        "history_entries",
        "change_sets",
        "idempotency_receipts",
    ];
    let mut before = Vec::new();
    for table in tables {
        let rows: Vec<Value> = sqlx::query_scalar(&format!(
            "SELECT to_jsonb(t) FROM {table} t ORDER BY to_jsonb(t)::text"
        ))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        before.push(rows);
    }

    sqlx::raw_sql(include_str!("../migrations/0008_ready_projection.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();

    for (table, expected) in tables.into_iter().zip(before) {
        let after: Vec<Value> = sqlx::query_scalar(&format!(
            "SELECT to_jsonb(t) FROM {table} t ORDER BY to_jsonb(t)::text"
        ))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        assert_eq!(after, expected, "upgrade changed durable table {table}");
    }
    let projection: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_array(node_id,live_child_count,eligible,guard) FROM node_ready_projection ORDER BY node_id",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let expected: Vec<Value> = (1..=12)
        .map(|id| {
            json!([
                Uuid::from_u128(id),
                if id == 1 { 2 } else { 0 },
                !matches!(id, 3..=9),
                0
            ])
        })
        .collect();
    assert_eq!(projection, expected);
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT node_id FROM node_ready_projection WHERE eligible AND live_child_count=0 ORDER BY node_id",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(candidates, [2, 10, 11, 12].map(Uuid::from_u128));
    let discrepancies = sqlx::raw_sql(include_str!("../scripts/check-ready-projection.sql"))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(discrepancies.is_empty());
    tx.rollback().await.unwrap();
    let remains: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1)")
            .bind(schema)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!remains, "the entire private schema must be rolled back");
    pool.close().await;
}
