//! Core graph contracts translated from AQ v1; orchestration stays outside this module.
use super::*;

pub(super) async fn validate_edge(
    tx: &mut Tx<'_>,
    p: &Principal,
    source: Uuid,
    target: Uuid,
    edge_type: EdgeType,
) -> Result<()> {
    if source == target {
        return Err(Error::Validation("self links are not allowed".into()));
    }
    let a = load_node(tx, p.workspace_id, source).await?;
    let b = load_node(tx, p.workspace_id, target).await?;
    if a.deleted || b.deleted {
        return Err(Error::Conflict(
            "relationships require live endpoints".into(),
        ));
    }
    let valid = match edge_type {
        EdgeType::Blocks | EdgeType::ParentChild => {
            a.kind == NodeKind::Task && b.kind == NodeKind::Task
        }
        EdgeType::MotivatedBy | EdgeType::Produces => {
            a.kind == NodeKind::Task && b.kind == NodeKind::Knowledge
        }
        EdgeType::Supports | EdgeType::Contradicts | EdgeType::Supersedes => {
            a.kind == NodeKind::Knowledge && b.kind == NodeKind::Knowledge
        }
        EdgeType::References => true,
    };
    if !valid {
        return Err(Error::Validation(format!(
            "invalid endpoint kinds for {edge_type}"
        )));
    }
    if matches!(edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
        let cycle=sqlx::query_scalar::<_,bool>("WITH RECURSIVE reachable(id) AS (SELECT $2::uuid UNION SELECT e.target FROM edges e JOIN reachable r ON e.source=r.id WHERE e.workspace_id=$1 AND NOT e.deleted AND e.edge_type IN ('blocks','parent-child')) SELECT EXISTS(SELECT 1 FROM reachable WHERE id=$3)").bind(p.workspace_id).bind(target).bind(source).fetch_one(&mut **tx).await?;
        if cycle {
            return Err(Error::Validation(
                "relationship would create a blocking/structural cycle".into(),
            ));
        }
    }
    if edge_type == EdgeType::ParentChild {
        if matches!(
            a.task_state,
            Some(TaskState::Completed | TaskState::Canceled)
        ) {
            return Err(Error::Conflict(
                "terminal parent refuses new children".into(),
            ));
        }
        let parent=sqlx::query_scalar::<_,Uuid>("SELECT source FROM edges WHERE workspace_id=$1 AND target=$2 AND edge_type='parent-child' AND NOT deleted").bind(p.workspace_id).bind(target).fetch_optional(&mut **tx).await?;
        if parent.is_some() {
            return Err(Error::Conflict(
                "child already has a parent; use set_parent to reparent".into(),
            ));
        }
        let depth=sqlx::query_scalar::<_,i32>("WITH RECURSIVE ancestors(id,depth) AS (SELECT $2::uuid,1 UNION ALL SELECT e.source,a.depth+1 FROM edges e JOIN ancestors a ON e.target=a.id WHERE e.workspace_id=$1 AND e.edge_type='parent-child' AND NOT e.deleted AND a.depth<4) SELECT max(depth) FROM ancestors").bind(p.workspace_id).bind(source).fetch_one(&mut **tx).await?;
        let height=sqlx::query_scalar::<_,i32>("WITH RECURSIVE descendants(id,depth) AS (SELECT $2::uuid,1 UNION ALL SELECT e.target,d.depth+1 FROM edges e JOIN descendants d ON e.source=d.id WHERE e.workspace_id=$1 AND e.edge_type='parent-child' AND NOT e.deleted AND d.depth<4) SELECT max(depth) FROM descendants").bind(p.workspace_id).bind(target).fetch_one(&mut **tx).await?;
        if depth + height > 3 {
            return Err(Error::Validation(
                "structural depth exceeds maximum of three levels".into(),
            ));
        }
    }
    Ok(())
}

pub(super) async fn set_parent(
    tx: &mut Tx<'_>,
    p: &Principal,
    change: Uuid,
    id: Uuid,
    expected_version: i64,
    parent_id: Option<Uuid>,
    reason: Option<&str>,
) -> Result<Value> {
    let mut child = load_node_write(tx, p.workspace_id, id).await?;
    expected(child.version, expected_version)?;
    if child.kind != NodeKind::Task {
        return Err(Error::Validation(
            "only tasks can have structural parents".into(),
        ));
    }
    if child.deleted {
        return Err(Error::Conflict(
            "deleted task must be restored before reparenting".into(),
        ));
    }
    let current=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND target=$2 AND edge_type='parent-child' AND NOT deleted FOR UPDATE").bind(p.workspace_id).bind(id).fetch_optional(&mut **tx).await?;
    let current: Option<Edge> = current.map(decode).transpose()?;
    let old_parent = current.as_ref().map(|e| e.source);
    if old_parent == parent_id {
        return encode(&child);
    }
    if let Some(mut e) = current {
        e.deleted = true;
        e.version += 1;
        sqlx::query("UPDATE edges SET deleted=true,version=$3 WHERE workspace_id=$1 AND id=$2")
            .bind(p.workspace_id)
            .bind(e.id)
            .bind(e.version)
            .execute(&mut **tx)
            .await?;
        history_entry(
            tx,
            p,
            change,
            vec![e.source, e.target],
            "edge",
            "removed",
            reason,
            encode(&e)?,
        )
        .await?;
    }
    if let Some(parent) = parent_id {
        validate_edge(tx, p, parent, id, EdgeType::ParentChild).await?;
        let e: Edge = decode(
            json!({"id":Uuid::new_v4(),"workspace_id":p.workspace_id,"source":parent,"target":id,"edge_type":"parent-child","source_revision":null,"target_revision":null,"description":null,"version":1,"deleted":false,"created_at":server_time()}),
        )?;
        sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,source_revision,target_revision,version,deleted,created_at) VALUES($1,$2,$3,$4,'parent-child',NULL,NULL,1,false,$5)").bind(e.id).bind(p.workspace_id).bind(parent).bind(id).bind(e.created_at).execute(&mut **tx).await?;
        history_entry(
            tx,
            p,
            change,
            vec![parent, id],
            "edge",
            "added",
            reason,
            encode(&e)?,
        )
        .await?;
    }
    let mut changed = vec![id];
    changed.extend(old_parent);
    changed.extend(parent_id);
    checkpoint_nodes(tx, p, change, changed, "reparented", reason).await?;
    child = load_node(tx, p.workspace_id, id).await?;
    history_entry(
        tx,
        p,
        change,
        vec![id],
        "lifecycle",
        "reparented",
        reason,
        json!({"from_parent":old_parent,"to_parent":parent_id,"version":child.version}),
    )
    .await?;
    encode(&child)
}

pub(super) async fn validate_completion(tx: &mut Tx<'_>, p: &Principal, id: Uuid) -> Result<()> {
    let open=sqlx::query_scalar::<_,Uuid>("SELECT n.id FROM edges e JOIN nodes n ON n.id=e.target AND n.workspace_id=e.workspace_id WHERE e.workspace_id=$1 AND e.source=$2 AND e.edge_type='parent-child' AND NOT e.deleted AND NOT n.deleted AND n.task_state NOT IN ('completed','canceled') ORDER BY n.id LIMIT 1").bind(p.workspace_id).bind(id).fetch_optional(&mut **tx).await?;
    if let Some(child) = open {
        return Err(Error::Conflict(format!(
            "open child {child} prevents parent completion"
        )));
    }
    Ok(())
}

impl Store {
    pub async fn tree(&self, p: &Principal, id: Uuid, q: ContextQuery) -> Result<Value> {
        q.validate()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let root = load_node(&mut tx, p.workspace_id, id).await?;
        if root.kind != NodeKind::Task {
            return Err(Error::Validation("tree root must be a task".into()));
        }
        let mut bytes = json!({"node":root,"children":[],"truncated":false})
            .to_string()
            .len();
        if bytes > q.max_bytes as usize {
            return Err(Error::Validation("root record exceeds max_bytes".into()));
        }
        let mut entries = std::collections::BTreeMap::<Uuid, (Node, Uuid)>::new();
        let mut frontier = vec![id];
        let mut truncated = false;
        for _ in 0..q.depth {
            if frontier.is_empty() {
                break;
            }
            let available = (q.max_bytes as usize).saturating_sub(bytes);
            let bundle=sqlx::query_scalar::<_,Value>("WITH candidates AS (SELECT e.source,n.id,to_jsonb(n)-'search_document' AS node FROM edges e JOIN nodes n ON n.id=e.target AND n.workspace_id=e.workspace_id WHERE e.workspace_id=$1 AND e.source=ANY($2) AND e.edge_type='parent-child' AND NOT e.deleted AND NOT n.deleted ORDER BY n.id LIMIT $3), sized AS (SELECT source,id,node,sum(octet_length(node::text)+64) OVER(ORDER BY id) AS cumulative FROM candidates) SELECT jsonb_build_object('entries',COALESCE(jsonb_agg(jsonb_build_object('source',source,'node',node) ORDER BY id) FILTER(WHERE cumulative<=$4),'[]'::jsonb),'total',count(*)) FROM sized").bind(p.workspace_id).bind(&frontier).bind(i64::from(q.limit)-entries.len() as i64).bind(available as i64).fetch_one(&mut *tx).await?;
            let rows: Vec<Value> = decode(bundle["entries"].clone())?;
            if bundle["total"].as_u64().unwrap_or(0) > rows.len() as u64 {
                truncated = true;
            }
            frontier.clear();
            for row in rows {
                let node: Node = decode(row["node"].clone())?;
                let size = json!({"node":node,"children":[],"truncated":false})
                    .to_string()
                    .len()
                    + 1;
                if entries.len() + 1 >= q.limit as usize || bytes + size > q.max_bytes as usize {
                    truncated = true;
                    continue;
                }
                bytes += size;
                frontier.push(node.id);
                entries.insert(node.id, (node, decode(row["source"].clone())?));
            }
        }
        if !frontier.is_empty() {
            let beyond=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.id=e.target AND n.workspace_id=e.workspace_id WHERE e.workspace_id=$1 AND e.source=ANY($2) AND e.edge_type='parent-child' AND NOT e.deleted AND NOT n.deleted)").bind(p.workspace_id).bind(&frontier).fetch_one(&mut *tx).await?;
            truncated |= beyond;
        }
        fn branch(node: &Node, entries: &std::collections::BTreeMap<Uuid, (Node, Uuid)>) -> Value {
            let children: Vec<_> = entries
                .values()
                .filter(|(_, parent)| *parent == node.id)
                .map(|(child, _)| branch(child, entries))
                .collect();
            json!({"node":node,"children":children,"truncated":false})
        }
        let mut result = branch(&root, &entries);
        result["truncated"] = json!(truncated);
        tx.commit().await?;
        Ok(result)
    }
}
