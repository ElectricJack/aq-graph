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

pub(super) struct Restore<'a> {
    pub principal: &'a Principal,
    pub authority: &'a Principal,
    pub change: Uuid,
    pub id: Uuid,
    pub expected_version: i64,
    pub revision: i64,
    pub include_lifecycle: bool,
    pub reason: Option<&'a str>,
    pub preview: bool,
}

struct LinkChanges {
    remove: Vec<Edge>,
    add: Vec<Edge>,
    has_historical_edges: bool,
}

impl LinkChanges {
    fn unchanged(&self) -> bool {
        self.remove.is_empty() && self.add.is_empty()
    }
}

impl Restore<'_> {
    pub(super) async fn apply(&self, tx: &mut Tx<'_>) -> Result<Node> {
        let before = load_node_write(tx, self.principal.workspace_id, self.id).await?;
        expected(before.version, self.expected_version)?;
        knowledge_rules::authorize_edit(self.authority, &before)?;
        let protected = before.kind == NodeKind::Knowledge || before.retired;
        if protected || self.include_lifecycle {
            self.require_admin(tx).await?;
        }
        let old: Node =
            decode(load_revision(tx, self.principal.workspace_id, self.id, self.revision).await?)?;
        let snapshot = encode(&old)?;
        let links = self.link_changes(tx).await?;
        self.authorize_structural_edges(tx, &links).await?;
        let mut n = restore_fields(&before, &snapshot, self.include_lifecycle)?;
        self.validate_node(tx, &before, &mut n).await?;
        if !durable_changed(&before, &n)? && links.unchanged() {
            return Ok(before);
        }
        if n.deleted && links.has_historical_edges {
            return Err(Error::Conflict("cannot restore active relationships on a deleted node; explicitly restore lifecycle to a live revision".into()));
        }
        knowledge_rules::reset_verification(&mut n);
        knowledge_rules::clear_summary(&mut n);
        save_node(tx, &n, false).await?;
        let changed = self.apply_links(tx, links).await?;
        self.check_lifecycle(tx, &n).await?;
        self.checkpoint_restoration(tx, &before, changed).await
    }

    async fn checkpoint_restoration(
        &self,
        tx: &mut Tx<'_>,
        before: &Node,
        changed: Vec<Uuid>,
    ) -> Result<Node> {
        checkpoint_nodes(
            tx,
            self.principal,
            self.change,
            changed,
            "restored",
            self.reason,
        )
        .await?;
        let n = load_node(tx, self.principal.workspace_id, self.id).await?;
        if (before.task_state, before.retired, before.deleted)
            != (n.task_state, n.retired, n.deleted)
        {
            history_entry(tx,self.principal,self.change,vec![self.id],"lifecycle","restored",self.reason,json!({"from_revision":self.revision,"version":n.version,"task_state":n.task_state,"retired":n.retired,"deleted":n.deleted})).await?;
        }
        Ok(n)
    }

    async fn apply_links(&self, tx: &mut Tx<'_>, links: LinkChanges) -> Result<Vec<Uuid>> {
        let mut changed = vec![self.id];
        for edge in links.remove {
            changed.extend(self.remove_edge(tx, edge).await?);
        }
        for edge in links.add {
            changed.extend(self.restore_edge(tx, edge).await?);
        }
        Ok(changed)
    }

    async fn require_admin(&self, tx: &mut Tx<'_>) -> Result<()> {
        self.authority.require_admin()?;
        if !self.preview {
            live_authority(tx, self.principal, true).await?;
        }
        Ok(())
    }
    async fn link_changes(&self, tx: &mut Tx<'_>) -> Result<LinkChanges> {
        let historical =
            revision_links::load_links(tx, self.principal.workspace_id, self.id, self.revision)
                .await?;
        let values=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND NOT deleted AND (source=$2 OR (target=$2 AND edge_type IN ('blocks','parent-child'))) ORDER BY id FOR UPDATE").bind(self.principal.workspace_id).bind(self.id).fetch_all(&mut **tx).await?;
        let current: Vec<Edge> = values.into_iter().map(decode).collect::<Result<_>>()?;
        let current: BTreeMap<_, _> = current.into_iter().map(|e| (e.id, e)).collect();
        let wanted: BTreeMap<_, _> = historical.into_iter().map(|e| (e.id, e)).collect();
        let remove = changed_links(&current, &wanted);
        let add = changed_links(&wanted, &current);
        if remove.len() + add.len() > 100 {
            return Err(Error::Validation(
                "restore changes more than 100 relationships".into(),
            ));
        }
        Ok(LinkChanges {
            remove,
            add,
            has_historical_edges: !wanted.is_empty(),
        })
    }
    async fn authorize_structural_edges(&self, tx: &mut Tx<'_>, links: &LinkChanges) -> Result<()> {
        for edge in links.remove.iter().chain(&links.add) {
            if !matches!(edge.edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
                continue;
            }
            for endpoint in [edge.source, edge.target] {
                let node = load_node_write(tx, self.principal.workspace_id, endpoint).await?;
                knowledge_rules::authorize_edit(self.authority, &node)?;
            }
        }
        Ok(())
    }
    async fn validate_node(&self, tx: &mut Tx<'_>, before: &Node, n: &mut Node) -> Result<()> {
        knowledge_rules::validate_sources(&n.provenance)?;
        if before.retired != n.retired || before.successor_id != n.successor_id {
            self.require_admin(tx).await?;
        }
        if n.content_type.is_none() {
            n.content_type = Some(KnowledgeType::Note);
        }
        knowledge_rules::validate_retirement(n)?;
        knowledge_rules::validate_temporal(n)?;
        if let Some(successor) = n.successor_id {
            let successor = load_node(tx, self.principal.workspace_id, successor).await?;
            if successor.deleted {
                return Err(Error::Conflict("successor cannot be deleted".into()));
            }
        }
        Ok(())
    }
    async fn authorize_source(&self, tx: &mut Tx<'_>, source_id: Uuid) -> Result<()> {
        let source = load_node_write(tx, self.principal.workspace_id, source_id).await?;
        knowledge_rules::authorize_edit(self.authority, &source)?;
        if source.retired && source.id != self.id {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    async fn remove_edge(&self, tx: &mut Tx<'_>, mut e: Edge) -> Result<Vec<Uuid>> {
        self.authorize_source(tx, e.source).await?;
        e.deleted = true;
        e.version += 1;
        sqlx::query("UPDATE edges SET deleted=true,version=$3 WHERE workspace_id=$1 AND id=$2")
            .bind(self.principal.workspace_id)
            .bind(e.id)
            .bind(e.version)
            .execute(&mut **tx)
            .await?;
        history_entry(
            tx,
            self.principal,
            self.change,
            vec![e.source, e.target],
            "edge",
            "removed",
            self.reason,
            encode(&e)?,
        )
        .await?;
        Ok(relationship_nodes(&e))
    }
    async fn restore_edge(&self, tx: &mut Tx<'_>, mut e: Edge) -> Result<Vec<Uuid>> {
        self.authorize_source(tx, e.source).await?;
        compat_graph::ProposedEdge {
            source: e.source,
            target: e.target,
            edge_type: e.edge_type,
        }
        .validate(tx, self.principal)
        .await?;
        for (node, pin) in [(e.source, e.source_revision), (e.target, e.target_revision)] {
            if let Some(pin) = pin {
                load_revision(tx, self.principal.workspace_id, node, pin).await?;
            }
        }
        let head = load_edge(tx, self.principal.workspace_id, e.id).await?;
        if !head.deleted {
            return Err(Error::Conflict(
                "restored relationship identity is already active".into(),
            ));
        }
        e.version = head.version + 1;
        e.deleted = false;
        e.created_at = head.created_at;
        sqlx::query("UPDATE edges SET source=$3,target=$4,edge_type=$5,source_revision=$6,target_revision=$7,description=$8,version=$9,deleted=false WHERE workspace_id=$1 AND id=$2").bind(self.principal.workspace_id).bind(e.id).bind(e.source).bind(e.target).bind(tag(&e.edge_type)?).bind(e.source_revision).bind(e.target_revision).bind(&e.description).bind(e.version).execute(&mut **tx).await?;
        history_entry(
            tx,
            self.principal,
            self.change,
            vec![e.source, e.target],
            "edge",
            "restored",
            self.reason,
            encode(&e)?,
        )
        .await?;
        Ok(relationship_nodes(&e))
    }
    async fn check_lifecycle(&self, tx: &mut Tx<'_>, n: &Node) -> Result<()> {
        if n.task_state == Some(TaskState::Completed) {
            compat_graph::validate_completion(tx, self.principal, self.id).await?;
        }
        if n.deleted {
            let incident=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM edges WHERE workspace_id=$1 AND NOT deleted AND (source=$2 OR target=$2))").bind(self.principal.workspace_id).bind(self.id).fetch_one(&mut **tx).await?;
            if incident {
                return Err(Error::Conflict(
                    "remove active relationships before restoring a tombstone".into(),
                ));
            }
        }
        Ok(())
    }
}
fn restore_fields(before: &Node, snapshot: &Value, include_lifecycle: bool) -> Result<Node> {
    let mut value = encode(before)?;
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
    decode(value)
}

fn changed_links(from: &BTreeMap<Uuid, Edge>, to: &BTreeMap<Uuid, Edge>) -> Vec<Edge> {
    from.values()
        .filter(|edge| to.get(&edge.id).is_none_or(|other| !same_link(edge, other)))
        .cloned()
        .collect()
}
