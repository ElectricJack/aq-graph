//! Immutable per-record relationship snapshots shared by tasks and knowledge.
use super::*;

/// Snapshot owned outgoing edges plus incoming execution/structural membership.
/// The transaction must already protect the affected nodes/graph invariants.
pub(super) async fn capture_links(tx: &mut Tx<'_>, p: &Principal, node_id: Uuid) -> Result<Uuid> {
    load_node(tx, p.workspace_id, node_id).await?;
    sqlx::query("INSERT INTO edge_versions(workspace_id,id,version,snapshot) SELECT e.workspace_id,e.id,e.version,to_jsonb(e) FROM edges e WHERE e.workspace_id=$1 AND NOT e.deleted AND (e.source=$2 OR (e.target=$2 AND e.edge_type IN ('blocks','parent-child'))) ON CONFLICT(workspace_id,id,version) DO NOTHING").bind(p.workspace_id).bind(node_id).execute(&mut **tx).await?;
    let inconsistent=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM edges e JOIN edge_versions v ON v.workspace_id=e.workspace_id AND v.id=e.id AND v.version=e.version WHERE e.workspace_id=$1 AND NOT e.deleted AND (e.source=$2 OR (e.target=$2 AND e.edge_type IN ('blocks','parent-child'))) AND jsonb_populate_record(NULL::edges,v.snapshot) IS DISTINCT FROM e)").bind(p.workspace_id).bind(node_id).fetch_one(&mut **tx).await?;
    if inconsistent {
        return Err(Error::Internal(
            "edge changed without advancing its immutable version".into(),
        ));
    }
    let previous=sqlx::query_scalar::<_,Uuid>("SELECT link_set_id FROM node_revisions WHERE workspace_id=$1 AND node_id=$2 AND link_set_id IS NOT NULL ORDER BY revision DESC LIMIT 1").bind(p.workspace_id).bind(node_id).fetch_optional(&mut **tx).await?;
    if let Some(set) = previous {
        let same=sqlx::query_scalar::<_,bool>("WITH current_members AS MATERIALIZED (SELECT id,version FROM edges WHERE workspace_id=$1 AND NOT deleted AND (source=$2 OR (target=$2 AND edge_type IN ('blocks','parent-child')))), prior_members AS MATERIALIZED (SELECT edge_id AS id,edge_version AS version FROM link_set_members WHERE workspace_id=$1 AND set_id=$3) SELECT NOT EXISTS((SELECT id,version FROM current_members EXCEPT SELECT id,version FROM prior_members) UNION ALL (SELECT id,version FROM prior_members EXCEPT SELECT id,version FROM current_members))").bind(p.workspace_id).bind(node_id).bind(set).fetch_one(&mut **tx).await?;
        if same {
            return Ok(set);
        }
    }
    let set = Uuid::new_v4();
    sqlx::query("INSERT INTO link_sets(id,workspace_id,node_id) VALUES($1,$2,$3)")
        .bind(set)
        .bind(p.workspace_id)
        .bind(node_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO link_set_members(workspace_id,set_id,edge_id,edge_version) SELECT workspace_id,$3,id,version FROM edges WHERE workspace_id=$1 AND NOT deleted AND (source=$2 OR (target=$2 AND edge_type IN ('blocks','parent-child')))").bind(p.workspace_id).bind(node_id).bind(set).execute(&mut **tx).await?;
    Ok(set)
}

/// Resolve precisely the requested record revision. Legacy NULL sets are an
/// explicit availability failure, never a request to read current relationships.
pub(super) async fn load_links(
    tx: &mut Tx<'_>,
    workspace: Uuid,
    node_id: Uuid,
    revision: i64,
) -> Result<Vec<Edge>> {
    let set=sqlx::query_scalar::<_,Option<Uuid>>("SELECT link_set_id FROM node_revisions WHERE workspace_id=$1 AND node_id=$2 AND revision=$3").bind(workspace).bind(node_id).bind(revision).fetch_optional(&mut **tx).await?.ok_or(Error::NotFound)?;
    let set = set.ok_or_else(|| {
        Error::Conflict("relationship snapshot unavailable for legacy revision".into())
    })?;
    let owned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM link_sets WHERE workspace_id=$1 AND node_id=$2 AND id=$3)",
    )
    .bind(workspace)
    .bind(node_id)
    .bind(set)
    .fetch_one(&mut **tx)
    .await?;
    if !owned {
        return Err(Error::Conflict(
            "relationship snapshot unavailable for revision".into(),
        ));
    }
    let values=sqlx::query_scalar::<_,Value>("SELECT v.snapshot FROM link_set_members m JOIN edge_versions v ON v.workspace_id=m.workspace_id AND v.id=m.edge_id AND v.version=m.edge_version WHERE m.workspace_id=$1 AND m.set_id=$2 ORDER BY m.edge_id").bind(workspace).bind(set).fetch_all(&mut **tx).await?;
    values.into_iter().map(decode).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exact_links_reuse_unchanged_sets_and_keep_prior_versions_after_live_edits() {
        let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL")
            .expect("set AQ_GRAPH_TEST_DATABASE_URL to disposable PostgreSQL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let schema = format!("revision_links_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(&format!(
            "CREATE SCHEMA {schema}; SET LOCAL search_path TO {schema},pg_catalog;"
        ))
        .execute(&mut *tx)
        .await
        .unwrap();
        for migration in [
            include_str!("../../migrations/0001_initial.sql"),
            include_str!("../../migrations/0002_workspace_credential_scope.sql"),
            include_str!("../../migrations/0003_knowledge_pagination.sql"),
            include_str!("../../migrations/0004_graph_contract.sql"),
            include_str!("../../migrations/0005_knowledge_contract.sql"),
            include_str!("../../migrations/0006_unified_revisions.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&mut *tx).await.unwrap();
        }
        let workspace = Uuid::new_v4();
        let p = Principal {
            workspace_id: workspace,
            actor_id: "revision-author".into(),
            capability: Capability::Admin,
        };
        sqlx::query("INSERT INTO workspaces(id) VALUES($1)")
            .bind(workspace)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,created_at,updated_at,created_by) SELECT lpad(to_hex(g),32,'0')::uuid,$1,'task',1,1,'task','','','active',clock_timestamp(),clock_timestamp(),'revision-author' FROM generate_series(1,3) g").bind(workspace).execute(&mut *tx).await.unwrap();
        for (id, source, target, kind) in [
            (10, 1, 2, "blocks"),
            (11, 2, 3, "references"),
            (12, 3, 1, "references"),
        ] {
            sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,version,created_at) VALUES($1,$2,$3,$4,$5,1,clock_timestamp())").bind(Uuid::from_u128(id)).bind(workspace).bind(Uuid::from_u128(source)).bind(Uuid::from_u128(target)).bind(kind).execute(&mut *tx).await.unwrap();
        }
        let change = Uuid::new_v4();
        sqlx::query("INSERT INTO change_sets(id,workspace_id,actor_id,operations,status) VALUES($1,$2,'revision-author','[]','applied')").bind(change).bind(workspace).execute(&mut *tx).await.unwrap();
        let first = capture_links(&mut tx, &p, Uuid::from_u128(1))
            .await
            .unwrap();
        let dependent = capture_links(&mut tx, &p, Uuid::from_u128(2))
            .await
            .unwrap();
        for (id, set) in [(1, Some(first)), (2, Some(dependent)), (3, None)] {
            sqlx::query("INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,change_set_id,link_set_id) SELECT workspace_id,id,1,to_jsonb(n)-'search_document','revision-author','created',$2,$3 FROM nodes n WHERE id=$1").bind(Uuid::from_u128(id)).bind(change).bind(set).execute(&mut *tx).await.unwrap();
        }
        let links = load_links(&mut tx, workspace, Uuid::from_u128(1), 1)
            .await
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].id,
            Uuid::from_u128(10),
            "incoming informational links stay derived"
        );
        let links = load_links(&mut tx, workspace, Uuid::from_u128(2), 1)
            .await
            .unwrap();
        assert_eq!(
            links.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![Uuid::from_u128(10), Uuid::from_u128(11)],
            "incoming blocks belongs to dependent revision"
        );
        // Historical serializers can encode the same PostgreSQL timestamp
        // using another offset or fractional width. Compare typed edge rows.
        sqlx::query("UPDATE edge_versions SET snapshot=jsonb_set(snapshot,'{created_at}',to_jsonb(to_char((snapshot->>'created_at')::timestamptz AT TIME ZONE 'Etc/GMT+8','YYYY-MM-DD\"T\"HH24:MI:SS.US')||'-08:00')) WHERE id=$1").bind(Uuid::from_u128(10)).execute(&mut *tx).await.unwrap();
        assert_eq!(
            capture_links(&mut tx, &p, Uuid::from_u128(1))
                .await
                .unwrap(),
            first,
            "equivalent offsets must not look like a same-version overwrite"
        );
        let live = load_edge(&mut tx, workspace, Uuid::from_u128(10))
            .await
            .unwrap();
        assert_eq!(
            load_links(&mut tx, workspace, Uuid::from_u128(1), 1)
                .await
                .unwrap()[0]
                .created_at,
            live.created_at
        );
        sqlx::query(
            "UPDATE nodes SET title='content changed',version=2,content_revision=2 WHERE id=$1",
        )
        .bind(Uuid::from_u128(1))
        .execute(&mut *tx)
        .await
        .unwrap();
        assert_eq!(
            capture_links(&mut tx, &p, Uuid::from_u128(1))
                .await
                .unwrap(),
            first,
            "content-only edit must reuse immutable membership"
        );
        sqlx::query("UPDATE edges SET version=2,description='new live description' WHERE id=$1")
            .bind(Uuid::from_u128(10))
            .execute(&mut *tx)
            .await
            .unwrap();
        let changed = capture_links(&mut tx, &p, Uuid::from_u128(1))
            .await
            .unwrap();
        assert_ne!(changed, first);
        assert_ne!(
            capture_links(&mut tx, &p, Uuid::from_u128(2))
                .await
                .unwrap(),
            dependent,
            "incoming structural version change affects dependent"
        );
        let old = load_links(&mut tx, workspace, Uuid::from_u128(1), 1)
            .await
            .unwrap();
        assert_eq!(old[0].version, 1);
        assert!(
            old[0].description.is_none(),
            "historic links cannot follow current payload"
        );
        assert!(
            matches!(load_links(&mut tx,workspace,Uuid::from_u128(3),1).await,Err(Error::Conflict(message)) if message.contains("legacy"))
        );
        assert!(matches!(
            load_links(&mut tx, Uuid::new_v4(), Uuid::from_u128(3), 1).await,
            Err(Error::NotFound)
        ));
        assert!(matches!(
            load_links(&mut tx, workspace, Uuid::from_u128(1), 999).await,
            Err(Error::NotFound)
        ));
        sqlx::query("UPDATE edges SET description='illegal same-version overwrite' WHERE id=$1")
            .bind(Uuid::from_u128(10))
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(matches!(
            capture_links(&mut tx, &p, Uuid::from_u128(1)).await,
            Err(Error::Internal(_))
        ));
        tx.rollback().await.unwrap();
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1)")
                .bind(schema)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!exists);
        pool.close().await;
    }
}
