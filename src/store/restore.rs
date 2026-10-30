//! Atomic revision restoration, shared by both node kinds.
use super::*;
use std::collections::BTreeMap;

fn same_link(a: &Edge, b: &Edge) -> bool {
    a.id == b.id
        && a.source == b.source
        && a.target == b.target
        && a.edge_type == b.edge_type
        && a.source_revision == b.source_revision
        && a.target_revision == b.target_revision
        && a.description == b.description
        && a.deleted == b.deleted
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn restore_node(
    tx: &mut Tx<'_>,
    p: &Principal,
    authority: &Principal,
    change: Uuid,
    id: Uuid,
    wanted_version: i64,
    rev: i64,
    include_lifecycle: bool,
    reason: Option<&str>,
    preview: bool,
) -> Result<Node> {
    let before = load_node_write(tx, p.workspace_id, id).await?;
    expected(before.version, wanted_version)?;
    knowledge_rules::authorize_edit(authority, &before)?;
    if before.kind == NodeKind::Knowledge || include_lifecycle || before.retired {
        authority.require_admin()?;
        if !preview {
            live_authority(tx, p, true).await?;
        }
    }
    let old: Node = decode(load_revision(tx, p.workspace_id, id, rev).await?)?;
    let snapshot = encode(&old)?;
    let historical = revision_links::load_links(tx, p.workspace_id, id, rev).await?;
    let values=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND NOT deleted AND (source=$2 OR (target=$2 AND edge_type IN ('blocks','parent-child'))) ORDER BY id FOR UPDATE").bind(p.workspace_id).bind(id).fetch_all(&mut **tx).await?;
    let current: Vec<Edge> = values.into_iter().map(decode).collect::<Result<_>>()?;
    let current: BTreeMap<_, _> = current.into_iter().map(|e| (e.id, e)).collect();
    let wanted: BTreeMap<_, _> = historical.into_iter().map(|e| (e.id, e)).collect();
    let remove: Vec<_> = current
        .values()
        .filter(|e| wanted.get(&e.id).is_none_or(|w| !same_link(e, w)))
        .cloned()
        .collect();
    let add: Vec<_> = wanted
        .values()
        .filter(|e| current.get(&e.id).is_none_or(|c| !same_link(e, c)))
        .cloned()
        .collect();
    if remove.len() + add.len() > 100 {
        return Err(Error::Validation(
            "restore changes more than 100 relationships".into(),
        ));
    }
    for edge in remove.iter().chain(add.iter()) {
        if matches!(edge.edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
            for endpoint in [edge.source, edge.target] {
                let node = load_node_write(tx, p.workspace_id, endpoint).await?;
                knowledge_rules::authorize_edit(authority, &node)?;
            }
        }
    }
    let mut value = encode(&before)?;
    for field in DURABLE_FIELDS {
        if matches!(
            *field,
            "verification"
                | "last_verified_at"
                | "last_verified_by"
                | "summary"
                | "summary_of_revision"
        ) {
            continue;
        }
        if !include_lifecycle
            && matches!(
                *field,
                "task_state" | "retired" | "retirement_reason" | "successor_id" | "deleted"
            )
        {
            continue;
        }
        value[*field] = snapshot[*field].clone();
    }
    let mut n: Node = decode(value)?;
    knowledge_rules::validate_sources(&n.provenance)?;
    if before.retired != n.retired || before.successor_id != n.successor_id {
        authority.require_admin()?;
        if !preview {
            live_authority(tx, p, true).await?;
        }
    }
    if n.content_type.is_none() {
        n.content_type = Some(KnowledgeType::Note);
    }
    knowledge_rules::validate_retirement(&n)?;
    knowledge_rules::validate_temporal(&n)?;
    if let Some(successor) = n.successor_id {
        let successor = load_node(tx, p.workspace_id, successor).await?;
        if successor.deleted {
            return Err(Error::Conflict("successor cannot be deleted".into()));
        }
    }
    if !durable_changed(&before, &n)? && remove.is_empty() && add.is_empty() {
        return Ok(before);
    }
    if n.deleted && !wanted.is_empty() {
        return Err(Error::Conflict("cannot restore active relationships on a deleted node; explicitly restore lifecycle to a live revision".into()));
    }
    knowledge_rules::reset_verification(&mut n);
    knowledge_rules::clear_summary(&mut n);
    save_node(tx, &n, false).await?;
    let mut changed = vec![id];
    for mut e in remove {
        let source = load_node_write(tx, p.workspace_id, e.source).await?;
        knowledge_rules::authorize_edit(authority, &source)?;
        if source.retired && source.id != id {
            return Err(Error::Forbidden);
        }
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
        changed.extend(relationship_nodes(&e));
    }
    for mut e in add {
        let source = load_node_write(tx, p.workspace_id, e.source).await?;
        knowledge_rules::authorize_edit(authority, &source)?;
        if source.retired && source.id != id {
            return Err(Error::Forbidden);
        }
        compat_graph::validate_edge(tx, p, e.source, e.target, e.edge_type).await?;
        for (node, pin) in [(e.source, e.source_revision), (e.target, e.target_revision)] {
            if let Some(pin) = pin {
                load_revision(tx, p.workspace_id, node, pin).await?;
            }
        }
        let head = load_edge(tx, p.workspace_id, e.id).await?;
        if !head.deleted {
            return Err(Error::Conflict(
                "restored relationship identity is already active".into(),
            ));
        }
        e.version = head.version + 1;
        e.deleted = false;
        e.created_at = head.created_at;
        sqlx::query("UPDATE edges SET source=$3,target=$4,edge_type=$5,source_revision=$6,target_revision=$7,description=$8,version=$9,deleted=false WHERE workspace_id=$1 AND id=$2").bind(p.workspace_id).bind(e.id).bind(e.source).bind(e.target).bind(tag(&e.edge_type)?).bind(e.source_revision).bind(e.target_revision).bind(&e.description).bind(e.version).execute(&mut **tx).await?;
        history_entry(
            tx,
            p,
            change,
            vec![e.source, e.target],
            "edge",
            "restored",
            reason,
            encode(&e)?,
        )
        .await?;
        changed.extend(relationship_nodes(&e));
    }
    if n.task_state == Some(TaskState::Completed) {
        compat_graph::validate_completion(tx, p, id).await?;
    }
    if n.deleted {
        let incident=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM edges WHERE workspace_id=$1 AND NOT deleted AND (source=$2 OR target=$2))").bind(p.workspace_id).bind(id).fetch_one(&mut **tx).await?;
        if incident {
            return Err(Error::Conflict(
                "remove active relationships before restoring a tombstone".into(),
            ));
        }
    }
    checkpoint_nodes(tx, p, change, changed, "restored", reason).await?;
    let n = load_node(tx, p.workspace_id, id).await?;
    if before.task_state != n.task_state
        || before.retired != n.retired
        || before.deleted != n.deleted
    {
        history_entry(tx,p,change,vec![id],"lifecycle","restored",reason,json!({"from_revision":rev,"version":n.version,"task_state":n.task_state,"retired":n.retired,"deleted":n.deleted})).await?;
    }
    Ok(n)
}
