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

#[derive(Default)]
struct AffectedRecords {
    nodes: BTreeSet<Uuid>,
    edges: BTreeSet<Uuid>,
    parents: BTreeSet<Uuid>,
    removals: BTreeSet<Uuid>,
    planned: BTreeMap<Uuid, (Uuid, Uuid)>,
    restores: Vec<(Uuid, i64)>,
}

impl AffectedRecords {
    fn include_edge_rows(&mut self, rows: Vec<Value>) -> Result<()> {
        for row in rows {
            let edge: Edge = decode(row)?;
            related(&mut self.nodes, &mut self.edges, &edge);
        }
        Ok(())
    }

    fn include_mutation(&mut self, op: &Mutation) -> Result<()> {
        match op {
            Mutation::CreateNode { id, .. } => {
                self.nodes.insert(required_id(*id)?);
            }
            Mutation::UpdateNode { id, .. }
            | Mutation::DeleteNode { id, .. }
            | Mutation::VerifyKnowledge { id, .. }
            | Mutation::VerifyNode { id, .. }
            | Mutation::SetSummary { id, .. } => {
                self.nodes.insert(*id);
            }
            Mutation::RestoreNode { id, revision, .. } => {
                self.nodes.insert(*id);
                self.restores.push((*id, *revision));
            }
            Mutation::SetParent { id, parent_id, .. } => {
                self.nodes.insert(*id);
                self.nodes.extend(*parent_id);
                self.parents.insert(*id);
            }
            Mutation::AddEdge {
                id,
                source,
                target,
                edge_type,
                ..
            } => {
                let id = required_id(*id)?;
                self.edges.insert(id);
                self.nodes.insert(*source);
                if structural(*edge_type) {
                    self.nodes.insert(*target);
                }
                self.planned.insert(id, (*source, *target));
            }
            Mutation::RemoveEdge { id, .. } => {
                self.edges.insert(*id);
                self.removals.insert(*id);
            }
        }
        Ok(())
    }

    async fn collect_removed_edges(&mut self, tx: &mut Tx<'_>, workspace: Uuid) -> Result<()> {
        if self.removals.is_empty() {
            return Ok(());
        }
        let ids: Vec<_> = self.removals.iter().copied().collect();
        let values = sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=ANY($2)",
        )
        .bind(workspace)
        .bind(ids)
        .fetch_all(&mut **tx)
        .await?;
        self.include_edge_rows(values)?;
        // A removed edge may first be created by an earlier operation in this
        // same draft. Its endpoints still belong in the reviewable before set.
        for id in &self.removals {
            if let Some((source, target)) = self.planned.get(id) {
                self.nodes.extend([*source, *target]);
            }
        }
        Ok(())
    }
    async fn collect_parents(
        &mut self,
        tx: &mut Tx<'_>,
        workspace: Uuid,
    ) -> Result<(Vec<Uuid>, Vec<Uuid>)> {
        let parent_children: Vec<_> = self.parents.iter().copied().collect();
        let mut prior_parent_ids = vec![];
        if !self.parents.is_empty() {
            prior_parent_ids=sqlx::query_scalar::<_,Uuid>("SELECT id FROM edges WHERE workspace_id=$1 AND target=ANY($2) AND edge_type='parent-child' ORDER BY id LIMIT 20001").bind(workspace).bind(&parent_children).fetch_all(&mut **tx).await?;
            if prior_parent_ids.len() > 20000 {
                return Err(Error::Validation(
                "parent relationship history is too large for draft preview; use a smaller draft"
                    .into(),
            ));
            }

            let values=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND target=ANY($2) AND edge_type='parent-child' AND NOT deleted").bind(workspace).bind(&parent_children).fetch_all(&mut **tx).await?;
            self.include_edge_rows(values)?;
        }
        Ok((parent_children, prior_parent_ids))
    }
    async fn collect_restored_edges(&mut self, tx: &mut Tx<'_>, workspace: Uuid) -> Result<()> {
        if self.restores.is_empty() {
            return Ok(());
        }
        let ids: Vec<_> = self.restores.iter().map(|(id, _)| *id).collect();
        let values=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND NOT deleted AND (source=ANY($2) OR (target=ANY($2) AND edge_type IN ('blocks','parent-child')))").bind(workspace).bind(ids).fetch_all(&mut **tx).await?;
        self.include_edge_rows(values)?;
        self.collect_historical_links(tx, workspace).await?;
        Ok(())
    }
    async fn collect_historical_links(&mut self, tx: &mut Tx<'_>, workspace: Uuid) -> Result<()> {
        for (id, revision) in self.restores.iter().copied() {
            match revision_links::load_links(tx, workspace, id, revision).await {
                Ok(historical) => {
                    for edge in historical {
                        related(&mut self.nodes, &mut self.edges, &edge);
                    }
                }
                // Execution remains authoritative for invalid, newly-created,
                // or legacy-unavailable revision requests.
                Err(Error::NotFound | Error::Conflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

pub(super) async fn before(tx: &mut Tx<'_>, workspace: Uuid, ops: &[Mutation]) -> Result<Value> {
    let mut affected = AffectedRecords::default();
    for op in ops {
        affected.include_mutation(op)?;
    }
    affected.collect_removed_edges(tx, workspace).await?;
    let (parent_children, prior_parent_ids) = affected.collect_parents(tx, workspace).await?;
    affected.collect_restored_edges(tx, workspace).await?;
    let node_ids: Vec<_> = affected.nodes.iter().copied().collect();
    let edge_ids: Vec<_> = affected.edges.iter().copied().collect();
    let (node_rows, edge_rows) = read_rows(tx, workspace, &node_ids, &edge_ids).await?;
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
    let nodes = before_values(&preview["nodes"])?;
    let mut edges = before_values(&preview["edges"])?;
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
    let (node_rows, edge_rows) = read_rows(tx, workspace, &node_ids, &edge_ids).await?;
    let nodes:Vec<_>=nodes.into_iter().map(|(id,before)|json!({"id":id,"before":before,"after":node_rows.get(&id).cloned().unwrap_or(Value::Null)})).collect();
    let edges:Vec<_>=edges.into_iter().map(|(id,before)|json!({"id":id,"before":before,"after":edge_rows.get(&id).cloned().unwrap_or(Value::Null)})).collect();
    let preview = json!({"nodes":nodes,"edges":edges});
    check_response(&preview)?;
    Ok(preview)
}

async fn read_rows(
    tx: &mut Tx<'_>,
    workspace: Uuid,
    node_ids: &[Uuid],
    edge_ids: &[Uuid],
) -> Result<(BTreeMap<Uuid, Value>, BTreeMap<Uuid, Value>)> {
    let node_rows = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND id=ANY($2)",
    )
    .bind(workspace)
    .bind(node_ids)
    .fetch_all(&mut **tx)
    .await?;
    let edge_rows = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=ANY($2)",
    )
    .bind(workspace)
    .bind(edge_ids)
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
    Ok((node_rows, edge_rows))
}

fn before_values(entries: &Value) -> Result<BTreeMap<Uuid, Value>> {
    let entries = entries
        .as_array()
        .ok_or_else(|| Error::Internal("invalid preview".into()))?;
    entries
        .iter()
        .map(|entry| Ok((decode(entry["id"].clone())?, entry["before"].clone())))
        .collect()
}
