//! Complete before/after draft diffs, including relationship-owned neighbors.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn required_id(id: Option<Uuid>) -> Result<Uuid> {
    id.ok_or_else(|| Error::Internal("draft identity missing".into()))
}
fn structural(kind: EdgeType) -> bool {
    matches!(kind, EdgeType::Blocks | EdgeType::ParentChild)
}
fn related(nodes: &mut BTreeSet<Uuid>, edges: &mut BTreeSet<Uuid>, edge: &Edge) {
    nodes.extend([edge.source, edge.target]);
    edges.insert(edge.id);
}

pub(super) async fn before(tx: &mut Tx<'_>, workspace: Uuid, ops: &[Mutation]) -> Result<Value> {
    let mut nodes = BTreeSet::new();
    let mut edges = BTreeSet::new();
    let mut removals = BTreeSet::new();
    let mut parents = BTreeSet::new();
    let mut restores = vec![];
    let mut planned = BTreeMap::new();
    for op in ops {
        match op {
            Mutation::CreateNode { id, .. } => {
                nodes.insert(required_id(*id)?);
            }
            Mutation::UpdateNode { id, .. }
            | Mutation::DeleteNode { id, .. }
            | Mutation::VerifyKnowledge { id, .. }
            | Mutation::VerifyNode { id, .. }
            | Mutation::SetSummary { id, .. } => {
                nodes.insert(*id);
            }
            Mutation::RestoreNode { id, revision, .. } => {
                nodes.insert(*id);
                restores.push((*id, *revision));
            }
            Mutation::SetParent { id, parent_id, .. } => {
                nodes.insert(*id);
                nodes.extend(*parent_id);
                parents.insert(*id);
            }
            Mutation::AddEdge {
                id,
                source,
                target,
                edge_type,
                ..
            } => {
                let id = required_id(*id)?;
                edges.insert(id);
                nodes.insert(*source);
                if structural(*edge_type) {
                    nodes.insert(*target);
                }
                planned.insert(id, (*source, *target));
            }
            Mutation::RemoveEdge { id, .. } => {
                edges.insert(*id);
                removals.insert(*id);
            }
        }
    }
    if !removals.is_empty() {
        let ids: Vec<_> = removals.iter().copied().collect();
        let values = sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=ANY($2)",
        )
        .bind(workspace)
        .bind(ids)
        .fetch_all(&mut **tx)
        .await?;
        for value in values {
            let edge: Edge = decode(value)?;
            related(&mut nodes, &mut edges, &edge);
        }
        // A removed edge may first be created by an earlier operation in this
        // same draft. Its endpoints still belong in the reviewable before set.
        for id in removals {
            if let Some((source, target)) = planned.get(&id) {
                nodes.extend([*source, *target]);
            }
        }
    }
    let parent_children: Vec<_> = parents.iter().copied().collect();
    let mut prior_parent_ids = vec![];
    if !parents.is_empty() {
        prior_parent_ids=sqlx::query_scalar::<_,Uuid>("SELECT id FROM edges WHERE workspace_id=$1 AND target=ANY($2) AND edge_type='parent-child' ORDER BY id LIMIT 20001").bind(workspace).bind(&parent_children).fetch_all(&mut **tx).await?;
        if prior_parent_ids.len() > 20000 {
            return Err(Error::Validation(
                "parent relationship history is too large for draft preview; use a smaller draft"
                    .into(),
            ));
        }
        let ids: Vec<_> = parents.into_iter().collect();
        let values=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND target=ANY($2) AND edge_type='parent-child' AND NOT deleted").bind(workspace).bind(ids).fetch_all(&mut **tx).await?;
        for value in values {
            let edge: Edge = decode(value)?;
            related(&mut nodes, &mut edges, &edge);
        }
    }
    if !restores.is_empty() {
        let ids: Vec<_> = restores.iter().map(|(id, _)| *id).collect();
        let values=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND NOT deleted AND (source=ANY($2) OR (target=ANY($2) AND edge_type IN ('blocks','parent-child')))").bind(workspace).bind(ids).fetch_all(&mut **tx).await?;
        for value in values {
            let edge: Edge = decode(value)?;
            related(&mut nodes, &mut edges, &edge);
        }
        for (id, revision) in restores {
            match revision_links::load_links(tx, workspace, id, revision).await {
                Ok(historical) => {
                    for edge in historical {
                        related(&mut nodes, &mut edges, &edge);
                    }
                }
                // Execution remains authoritative for invalid, newly-created,
                // or legacy-unavailable revision requests.
                Err(Error::NotFound | Error::Conflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
    }
    let node_ids: Vec<_> = nodes.iter().copied().collect();
    let edge_ids: Vec<_> = edges.iter().copied().collect();
    let node_rows = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND id=ANY($2)",
    )
    .bind(workspace)
    .bind(&node_ids)
    .fetch_all(&mut **tx)
    .await?;
    let edge_rows = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=ANY($2)",
    )
    .bind(workspace)
    .bind(&edge_ids)
    .fetch_all(&mut **tx)
    .await?;
    let node_rows: BTreeMap<Uuid, Value> = node_rows
        .into_iter()
        .map(|v| Ok((decode(v["id"].clone())?, v)))
        .collect::<Result<_>>()?;
    let edge_rows: BTreeMap<Uuid, Value> = edge_rows
        .into_iter()
        .map(|v| Ok((decode(v["id"].clone())?, v)))
        .collect::<Result<_>>()?;
    let nodes: Vec<_> = node_ids
        .into_iter()
        .map(|id| json!({"id":id,"before":node_rows.get(&id).cloned().unwrap_or(Value::Null)}))
        .collect();
    let edges: Vec<_> = edge_ids
        .into_iter()
        .map(|id| json!({"id":id,"before":edge_rows.get(&id).cloned().unwrap_or(Value::Null)}))
        .collect();
    let preview = json!({"nodes":nodes,"edges":edges,"_parent_children":parent_children,"_prior_parent_ids":prior_parent_ids});
    check_response(&preview)?;
    Ok(preview)
}

pub(super) async fn after(
    tx: &mut Tx<'_>,
    workspace: Uuid,
    _change: Uuid,
    preview: Value,
) -> Result<Value> {
    let mut nodes = BTreeMap::<Uuid, Value>::new();
    let mut edges = BTreeMap::<Uuid, Value>::new();
    for (category, items) in [("nodes", &mut nodes), ("edges", &mut edges)] {
        for entry in preview[category]
            .as_array()
            .ok_or_else(|| Error::Internal("invalid preview".into()))?
        {
            items.insert(decode(entry["id"].clone())?, entry["before"].clone());
        }
    }
    let parent_children: Vec<Uuid> = decode(preview["_parent_children"].clone())?;
    let prior_parent_ids: Vec<Uuid> = decode(preview["_prior_parent_ids"].clone())?;
    // SetParent allocates its edge during execution. Compare identity sets to
    // discover only new rows, even when removed by another draft operation.
    // No workspace-wide history query or additional schema index is needed.
    let generated=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND target=ANY($2) AND edge_type='parent-child' AND NOT (id=ANY($3)) ORDER BY id").bind(workspace).bind(parent_children).bind(prior_parent_ids).fetch_all(&mut **tx).await?;
    for value in generated {
        let edge: Edge = decode(value)?;
        edges.entry(edge.id).or_insert(Value::Null);
        if !nodes.contains_key(&edge.source) || !nodes.contains_key(&edge.target) {
            return Err(Error::Internal(
                "draft preview omitted an affected relationship owner".into(),
            ));
        }
    }
    let node_ids: Vec<_> = nodes.keys().copied().collect();
    let edge_ids: Vec<_> = edges.keys().copied().collect();
    let node_rows = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND id=ANY($2)",
    )
    .bind(workspace)
    .bind(&node_ids)
    .fetch_all(&mut **tx)
    .await?;
    let edge_rows = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=ANY($2)",
    )
    .bind(workspace)
    .bind(&edge_ids)
    .fetch_all(&mut **tx)
    .await?;
    let node_rows: BTreeMap<Uuid, Value> = node_rows
        .into_iter()
        .map(|v| Ok((decode(v["id"].clone())?, v)))
        .collect::<Result<_>>()?;
    let edge_rows: BTreeMap<Uuid, Value> = edge_rows
        .into_iter()
        .map(|v| Ok((decode(v["id"].clone())?, v)))
        .collect::<Result<_>>()?;
    let nodes:Vec<_>=nodes.into_iter().map(|(id,before)|json!({"id":id,"before":before,"after":node_rows.get(&id).cloned().unwrap_or(Value::Null)})).collect();
    let edges:Vec<_>=edges.into_iter().map(|(id,before)|json!({"id":id,"before":before,"after":edge_rows.get(&id).cloned().unwrap_or(Value::Null)})).collect();
    let preview = json!({"nodes":nodes,"edges":edges});
    check_response(&preview)?;
    Ok(preview)
}
