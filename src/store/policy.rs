//! Resolve permission requirements before replaying receipts or applying a draft.
use super::*;
pub(super) async fn requires_admin(
    tx: &mut Tx<'_>,
    p: &Principal,
    ops: &[Mutation],
) -> Result<bool> {
    for op in ops {
        match op {
            Mutation::VerifyKnowledge { .. } | Mutation::VerifyNode { .. } => return Ok(true),
            Mutation::UpdateNode { patch, .. } if patch.changes_retirement() => {
                return Ok(true);
            }
            Mutation::RestoreNode {
                id,
                include_lifecycle,
                ..
            } => {
                if *include_lifecycle {
                    return Ok(true);
                }
                let kind = restored_kind(tx, p, ops, *id).await?;
                if kind == NodeKind::Knowledge {
                    return Ok(true);
                }
            }
            _ => {}
        }
    }
    Ok(false)
}
pub(super) async fn touches_knowledge(
    tx: &mut Tx<'_>,
    p: &Principal,
    ops: &[Mutation],
) -> Result<bool> {
    let created = created_nodes(ops);
    for op in ops {
        let id = match op {
            Mutation::CreateNode {
                kind: NodeKind::Knowledge,
                ..
            }
            | Mutation::VerifyKnowledge { .. }
            | Mutation::VerifyNode { .. } => return Ok(true),
            Mutation::CreateNode { .. } | Mutation::SetParent { .. } => continue,
            Mutation::UpdateNode { id, .. }
            | Mutation::DeleteNode { id, .. }
            | Mutation::RestoreNode { id, .. }
            | Mutation::SetSummary { id, .. } => *id,
            Mutation::AddEdge { source, .. } => *source,
            Mutation::RemoveEdge { id, .. } => removed_edge_source(tx, p, ops, *id).await?,
        };
        let kind = match created.get(&id) {
            Some(kind) => *kind,
            None => load_node(tx, p.workspace_id, id).await?.kind,
        };
        if kind == NodeKind::Knowledge {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn restored_kind(
    tx: &mut Tx<'_>,
    p: &Principal,
    ops: &[Mutation],
    id: Uuid,
) -> Result<NodeKind> {
    let created_kind = ops.iter().find_map(|op| match op {
        Mutation::CreateNode {
            id: Some(created_id),
            kind,
            ..
        } if *created_id == id => Some(*kind),
        _ => None,
    });
    let kind = match created_kind {
        Some(kind) => kind,
        None => load_node(tx, p.workspace_id, id).await?.kind,
    };
    Ok(kind)
}

async fn removed_edge_source(
    tx: &mut Tx<'_>,
    p: &Principal,
    ops: &[Mutation],
    id: Uuid,
) -> Result<Uuid> {
    let created_source = ops.iter().find_map(|op| match op {
        Mutation::AddEdge {
            id: Some(created_id),
            source,
            ..
        } if *created_id == id => Some(*source),
        _ => None,
    });
    Ok(match created_source {
        Some(source) => source,
        None => load_edge(tx, p.workspace_id, id).await?.source,
    })
}

fn created_nodes(ops: &[Mutation]) -> std::collections::HashMap<Uuid, NodeKind> {
    let mut created = std::collections::HashMap::new();
    for op in ops {
        if let Mutation::CreateNode {
            id: Some(id), kind, ..
        } = op
        {
            created.insert(*id, *kind);
        }
    }
    created
}
