//! Validate replacement pointers against the final atomic batch state.
use super::*;

pub(super) async fn validate(tx: &mut Tx<'_>, p: &Principal, ops: &[Mutation]) -> Result<()> {
    let relevant = ops.iter().any(|op| matches!(op,
        Mutation::UpdateNode { patch, .. } if !patch.successor_id.is_keep() || patch.retired.is_some()))
        || ops.iter().any(|op| matches!(op, Mutation::RemoveEdge { .. } | Mutation::DeleteNode { .. } | Mutation::RestoreNode { .. }));
    if !relevant {
        return Ok(());
    }
    let mut roots = Vec::new();
    for op in ops {
        match op {
            Mutation::UpdateNode { id, .. }
            | Mutation::DeleteNode { id, .. }
            | Mutation::RestoreNode { id, .. } => roots.push(*id),
            Mutation::AddEdge { source, target, .. } => roots.extend([*source, *target]),
            Mutation::RemoveEdge { id, .. } => {
                let edge = load_edge(tx, p.workspace_id, *id).await?;
                roots.extend([edge.source, edge.target]);
            }
            _ => {}
        }
    }
    roots.sort_unstable();
    roots.dedup();
    // Check only changed nodes and pointers into changed nodes. The partial
    // successor index avoids scanning unrelated graph records on each write.
    let invalid = sqlx::query_scalar::<_, bool>("SELECT EXISTS (
        SELECT 1 FROM nodes n LEFT JOIN nodes s ON s.workspace_id=n.workspace_id AND s.id=n.successor_id
        WHERE n.workspace_id=$1 AND n.successor_id IS NOT NULL
        AND (n.id=ANY($2) OR n.successor_id=ANY($2))
        AND (NOT n.retired OR s.id IS NULL OR s.deleted OR NOT EXISTS (
            SELECT 1 FROM edges e WHERE e.workspace_id=n.workspace_id AND e.source=n.successor_id
            AND e.target=n.id AND e.edge_type='supersedes' AND NOT e.deleted)))")
        .bind(p.workspace_id).bind(&roots).fetch_one(&mut **tx).await?;
    if invalid {
        return Err(Error::Conflict("successor must be an existing nondeleted node with a live successor-to-retired supersedes link".into()));
    }
    // UNION deduplicates visited pairs, terminating even if a proposed cycle exists.
    let cycle = sqlx::query_scalar::<_, bool>("WITH RECURSIVE walk(root,id) AS (
        SELECT id,successor_id FROM nodes WHERE workspace_id=$1 AND id=ANY($2) AND successor_id IS NOT NULL
        UNION SELECT w.root,n.successor_id FROM walk w JOIN nodes n ON n.workspace_id=$1 AND n.id=w.id WHERE n.successor_id IS NOT NULL
        ) SELECT EXISTS(SELECT 1 FROM walk WHERE root=id)")
        .bind(p.workspace_id).bind(&roots).fetch_one(&mut **tx).await?;
    if cycle {
        return Err(Error::Conflict(
            "successor pointers cannot form a cycle".into(),
        ));
    }
    Ok(())
}
