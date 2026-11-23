//! Apply ordered graph mutations inside the caller's transaction.
use super::*;

pub(super) struct MutationBatch<'a> {
    principal: &'a Principal,
    authority: Principal,
    change: Uuid,
    reason: Option<&'a str>,
    preview: bool,
    deleted_nodes: Vec<Uuid>,
}

struct ExpectedVersion {
    id: Uuid,
    version: i64,
}
impl ExpectedVersion {
    fn new(id: Uuid, version: i64) -> Self {
        Self { id, version }
    }
    async fn lock_node(&self, tx: &mut Tx<'_>, workspace: Uuid) -> Result<Node> {
        let node = load_node_write(tx, workspace, self.id).await?;
        expected(node.version, self.version)?;
        Ok(node)
    }
}

struct VerificationAssertion<'a> {
    state: Verification,
    evidence: &'a [Value],
}
struct DerivedSummary<'a> {
    revision: i64,
    text: Option<&'a str>,
}
impl DerivedSummary<'_> {
    fn already_applied(&self, node: &Node) -> bool {
        let same_revision = self.text.is_none() || node.summary_of_revision == Some(self.revision);
        self.text == node.summary.as_deref() && same_revision
    }
}
struct PendingEdge<'a> {
    id: Option<Uuid>,
    endpoints: compat_graph::ProposedEdge,
    source_revision: Option<i64>,
    target_revision: Option<i64>,
    description: Option<&'a str>,
}

impl<'a> MutationBatch<'a> {
    fn journal(&self) -> Journal<'_> {
        Journal {
            principal: self.principal,
            change: self.change,
            reason: self.reason,
        }
    }

    pub(super) fn new(
        principal: &'a Principal,
        change: Uuid,
        reason: Option<&'a str>,
        preview: bool,
    ) -> Self {
        let mut authority = principal.clone();
        if preview {
            authority.capability = Capability::Admin;
        }
        Self {
            principal,
            authority,
            change,
            reason,
            preview,
            deleted_nodes: vec![],
        }
    }
    pub(super) async fn run(mut self, tx: &mut Tx<'_>, ops: &[Mutation]) -> Result<Vec<Value>> {
        check_operations(ops)?;
        let mut results = Vec::with_capacity(ops.len());
        for op in ops {
            results.push(self.apply(tx, op).await?);
        }
        if !self.deleted_nodes.is_empty() {
            let referenced=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.id=e.source OR n.id=e.target WHERE e.workspace_id=$1 AND NOT e.deleted AND n.deleted AND n.id=ANY($2))").bind(self.principal.workspace_id).bind(&self.deleted_nodes).fetch_one(&mut **tx).await?;
            if referenced {
                return Err(Error::Conflict(
                    "remove active relationships before deleting their endpoints".into(),
                ));
            }
        }
        successors::validate(tx, self.principal, ops).await?;
        Ok(results)
    }
    async fn apply(&mut self, tx: &mut Tx<'_>, op: &Mutation) -> Result<Value> {
        match op {
            Mutation::CreateNode {
                id,
                kind,
                title,
                body,
                acceptance_criteria,
                labels,
                metadata,
                task_state,
                content_type,
                verification,
                provenance,
                valid_from,
                valid_until,
                review_at,
            } => {
                let now = server_time();
                let n = Node {
                    id: id.unwrap_or_else(Uuid::new_v4),
                    workspace_id: self.principal.workspace_id,
                    kind: *kind,
                    version: 1,
                    content_revision: 1,
                    title: title.clone(),
                    body: body.clone(),
                    acceptance_criteria: acceptance_criteria.clone(),
                    labels: labels.clone(),
                    metadata: metadata.clone(),
                    task_state: if *kind == NodeKind::Task {
                        Some(task_state.unwrap_or(TaskState::Draft))
                    } else {
                        None
                    },
                    content_type: Some(content_type.unwrap_or(KnowledgeType::Note)),
                    verification: Some(verification.unwrap_or(Verification::Unverified)),
                    summary: None,
                    summary_of_revision: None,
                    valid_from: *valid_from,
                    valid_until: *valid_until,
                    review_at: *review_at,
                    successor_id: None,
                    retired: false,
                    provenance: provenance.clone(),
                    deleted: false,
                    created_at: now,
                    updated_at: now,
                    created_by: self.principal.actor_id.clone(),
                    last_verified_by: None,
                    last_verified_at: None,
                    retirement_reason: None,
                };
                self.persist_creation(tx, n).await
            }
            Mutation::UpdateNode {
                id,
                expected_version,
                patch,
            } => {
                self.update_node(tx, ExpectedVersion::new(*id, *expected_version), patch)
                    .await
            }
            Mutation::DeleteNode {
                id,
                expected_version,
            } => {
                self.delete_node(tx, ExpectedVersion::new(*id, *expected_version))
                    .await
            }
            Mutation::RestoreNode {
                id,
                expected_version,
                revision: rev,
                include_lifecycle,
            } => {
                let restore = restore::Restore {
                    principal: self.principal,
                    authority: &self.authority,
                    change: self.change,
                    id: *id,
                    expected_version: *expected_version,
                    revision: *rev,
                    include_lifecycle: *include_lifecycle,
                    reason: self.reason,
                    preview: self.preview,
                };
                let n = restore.apply(tx).await?;
                encode(&n)
            }
            Mutation::AddEdge {
                id,
                source,
                target,
                edge_type,
                source_revision,
                target_revision,
                description,
            } => {
                self.add_edge(
                    tx,
                    PendingEdge {
                        id: *id,
                        endpoints: compat_graph::ProposedEdge {
                            source: *source,
                            target: *target,
                            edge_type: *edge_type,
                        },
                        source_revision: *source_revision,
                        target_revision: *target_revision,
                        description: description.as_deref(),
                    },
                )
                .await
            }
            Mutation::SetParent {
                id,
                expected_version,
                parent_id,
            } => {
                compat_graph::Reparent {
                    principal: self.principal,
                    authority: &self.authority,
                    change: self.change,
                    reason: self.reason,
                    id: *id,
                    expected_version: *expected_version,
                    parent_id: *parent_id,
                }
                .apply(tx)
                .await
            }
            Mutation::VerifyNode {
                id,
                expected_version,
                verification,
                evidence,
            }
            | Mutation::VerifyKnowledge {
                id,
                expected_version,
                verification,
                evidence,
            } => {
                self.verify_node(
                    tx,
                    ExpectedVersion::new(*id, *expected_version),
                    VerificationAssertion {
                        state: *verification,
                        evidence,
                    },
                )
                .await
            }
            Mutation::SetSummary {
                id,
                expected_version,
                source_revision,
                summary,
            } => {
                self.set_summary(
                    tx,
                    ExpectedVersion::new(*id, *expected_version),
                    DerivedSummary {
                        revision: *source_revision,
                        text: summary.as_deref(),
                    },
                )
                .await
            }
            Mutation::RemoveEdge {
                id,
                expected_version,
            } => {
                self.remove_edge(tx, ExpectedVersion::new(*id, *expected_version))
                    .await
            }
        }
    }
    async fn persist_creation(&self, tx: &mut Tx<'_>, n: Node) -> Result<Value> {
        knowledge_rules::validate_create(&n)?;
        save_node(tx, &n, true).await?;
        self.journal().revision(tx, &n, "created").await?;
        if let Some(state) = n.task_state {
            self.journal()
                .record(
                    tx,
                    HistoryEvent {
                        nodes: vec![n.id],
                        category: "lifecycle",
                        operation: "created",
                        data: json!({"from":null,"to":state,"version":n.version}),
                    },
                )
                .await?;
        }
        encode(&n)
    }
    async fn update_node(
        &self,
        tx: &mut Tx<'_>,
        target: ExpectedVersion,
        patch: &NodePatch,
    ) -> Result<Value> {
        let before = target.lock_node(tx, self.principal.workspace_id).await?;
        self.authorize_patch(tx, &before, patch).await?;
        let mut n = before.clone();
        patch_node(&mut n, patch)?;
        self.validate_patched(tx, &n).await?;
        self.save_patch(tx, &before, n).await
    }
    async fn authorize_patch(
        &self,
        tx: &mut Tx<'_>,
        before: &Node,
        patch: &NodePatch,
    ) -> Result<()> {
        if before.deleted {
            return Err(Error::Conflict(
                "record is deleted; restore it before editing".into(),
            ));
        }
        knowledge_rules::authorize_edit(&self.authority, before)?;
        if before.retired && !patch.retirement_metadata_only() {
            return Err(Error::Forbidden);
        }
        self.authorize_retirement(tx, patch).await?;
        if patch.task_state == Some(TaskState::Completed)
            && before.task_state != Some(TaskState::Completed)
        {
            compat_graph::validate_completion(tx, self.principal, before.id).await?;
        }
        Ok(())
    }
    async fn authorize_retirement(&self, tx: &mut Tx<'_>, patch: &NodePatch) -> Result<()> {
        if patch.changes_retirement() {
            self.authority.require_admin()?;
            if !self.preview {
                live_authority(tx, self.principal, true).await?;
            }
        }
        Ok(())
    }
    async fn validate_patched(&self, tx: &mut Tx<'_>, n: &Node) -> Result<()> {
        knowledge_rules::validate_retirement(n)?;
        knowledge_rules::validate_temporal(n)?;
        if let Some(successor) = n.successor_id {
            let successor = load_node(tx, self.principal.workspace_id, successor).await?;
            if successor.deleted {
                return Err(Error::Conflict("successor cannot be deleted".into()));
            }
        }
        knowledge_rules::validate_sources(&n.provenance)?;
        Ok(())
    }
    async fn save_patch(&self, tx: &mut Tx<'_>, before: &Node, mut n: Node) -> Result<Value> {
        let content = durable_changed(before, &n)?;
        if content {
            knowledge_rules::reset_verification(&mut n);
            knowledge_rules::clear_summary(&mut n);
        }
        let lifecycle = before.task_state != n.task_state;
        if !content && !lifecycle {
            return encode(&n);
        }
        n.version += 1;
        n.updated_at = server_time();
        if content {
            n.content_revision += 1;
        }
        save_node(tx, &n, false).await?;
        if content {
            self.journal().revision(tx, &n, "updated").await?;
        }
        if lifecycle {
            self.journal().record(tx, HistoryEvent { nodes: vec![n.id], category: "lifecycle", operation: "transitioned", data: json!({"from":before.task_state,"to":n.task_state,"version":n.version}) })
            .await?;
        }
        encode(&n)
    }
    async fn delete_node(&mut self, tx: &mut Tx<'_>, target: ExpectedVersion) -> Result<Value> {
        let mut n = target.lock_node(tx, self.principal.workspace_id).await?;
        knowledge_rules::authorize_edit(&self.authority, &n)?;
        if !n.deleted {
            knowledge_rules::reset_verification(&mut n);
            knowledge_rules::clear_summary(&mut n);
            n.deleted = true;
            if n.kind == NodeKind::Task {
                n.successor_id = None;
            }
            n.version += 1;
            n.content_revision += 1;
            n.updated_at = server_time();
            save_node(tx, &n, false).await?;
            self.journal().revision(tx, &n, "deleted").await?;
            self.journal()
                .record(
                    tx,
                    HistoryEvent {
                        nodes: vec![n.id],
                        category: "lifecycle",
                        operation: "deleted",
                        data: json!({"deleted":true,"version":n.version}),
                    },
                )
                .await?;
            if n.kind == NodeKind::Task {
                self.journal().remove_task_links(tx, n.id).await?;
            }
            self.deleted_nodes.push(n.id);
        }
        encode(&n)
    }
    async fn add_edge(&self, tx: &mut Tx<'_>, edge: PendingEdge<'_>) -> Result<Value> {
        let a = load_node_write(tx, self.principal.workspace_id, edge.endpoints.source).await?;
        knowledge_rules::authorize_edit(&self.authority, &a)?;
        if a.retired {
            return Err(Error::Forbidden);
        }
        if let Some(existing) = edge.existing(tx, self.principal.workspace_id).await? {
            return Ok(existing);
        }
        edge.endpoints.validate(tx, self.principal).await?;
        self.validate_edge_target(tx, &a, &edge).await?;
        edge.validate_pins(tx, self.principal.workspace_id).await?;
        let e = edge.materialize(self.principal.workspace_id);
        sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,source_revision,target_revision,version,deleted,created_at,description) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)").bind(e.id).bind(e.workspace_id).bind(e.source).bind(e.target).bind(tag(&e.edge_type)?).bind(e.source_revision).bind(e.target_revision).bind(e.version).bind(e.deleted).bind(e.created_at).bind(&e.description).execute(&mut **tx).await?;
        self.journal()
            .record(
                tx,
                HistoryEvent {
                    nodes: vec![e.source, e.target],
                    category: "edge",
                    operation: "added",
                    data: encode(&e)?,
                },
            )
            .await?;
        self.journal()
            .checkpoint(tx, relationship_nodes(&e), "links_changed")
            .await?;
        encode(&e)
    }
    async fn validate_edge_target(
        &self,
        tx: &mut Tx<'_>,
        a: &Node,
        edge: &PendingEdge<'_>,
    ) -> Result<()> {
        let b = load_node(tx, self.principal.workspace_id, edge.endpoints.target).await?;
        if edge.structural() {
            knowledge_rules::authorize_edit(&self.authority, &b)?;
            if b.retired {
                return Err(Error::Forbidden);
            }
        }
        if a.deleted || b.deleted {
            return Err(Error::Conflict(
                "relationships require live endpoints".into(),
            ));
        }
        let both_tasks = a.kind == NodeKind::Task && b.kind == NodeKind::Task;
        if edge.structural() && !both_tasks {
            return Err(Error::Validation(
                "structural relationships require task endpoints".into(),
            ));
        }
        Ok(())
    }
    async fn verify_node(
        &self,
        tx: &mut Tx<'_>,
        target: ExpectedVersion,
        assertion: VerificationAssertion<'_>,
    ) -> Result<Value> {
        if !self.preview {
            live_authority(tx, self.principal, true).await?;
        }
        let mut n = target.lock_node(tx, self.principal.workspace_id).await?;
        knowledge_rules::verify(
            &mut n,
            &self.authority,
            assertion.state,
            assertion.evidence,
            self.reason,
        )?;
        knowledge_rules::clear_summary(&mut n);
        n.version += 1;
        n.content_revision += 1;
        n.updated_at = server_time();
        save_node(tx, &n, false).await?;
        self.journal().revision(tx, &n, "verified").await?;
        encode(&n)
    }
    async fn set_summary(
        &self,
        tx: &mut Tx<'_>,
        target: ExpectedVersion,
        summary: DerivedSummary<'_>,
    ) -> Result<Value> {
        let mut n = target.lock_node(tx, self.principal.workspace_id).await?;
        knowledge_rules::authorize_edit(&self.authority, &n)?;
        if n.deleted || n.retired {
            return Err(Error::Conflict(
                "summary requires a live nonretired node".into(),
            ));
        }
        // Repeating an identical derived value does not manufacture history.
        if summary.already_applied(&n) {
            return encode(&n);
        }
        knowledge_rules::set_summary(&mut n, summary.revision, summary.text)?;
        knowledge_rules::reset_verification(&mut n);
        n.version += 1;
        n.content_revision += 1;
        n.updated_at = server_time();
        knowledge_rules::validate_summary(&n)?;
        save_node(tx, &n, false).await?;
        self.journal().revision(tx, &n, "summary_changed").await?;
        encode(&n)
    }
    async fn remove_edge(&self, tx: &mut Tx<'_>, target: ExpectedVersion) -> Result<Value> {
        let mut e = load_edge(tx, self.principal.workspace_id, target.id).await?;
        expected(e.version, target.version)?;
        let source_node = load_node_write(tx, self.principal.workspace_id, e.source).await?;
        knowledge_rules::authorize_edit(&self.authority, &source_node)?;
        if source_node.retired {
            return Err(Error::Forbidden);
        }
        if !e.deleted {
            if matches!(e.edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
                let target = load_node_write(tx, self.principal.workspace_id, e.target).await?;
                knowledge_rules::authorize_edit(&self.authority, &target)?;
            }
            e.deleted = true;
            e.version += 1;
            sqlx::query("UPDATE edges SET deleted=true,version=$3 WHERE workspace_id=$1 AND id=$2")
                .bind(self.principal.workspace_id)
                .bind(e.id)
                .bind(e.version)
                .execute(&mut **tx)
                .await?;
            self.journal()
                .record(
                    tx,
                    HistoryEvent {
                        nodes: vec![e.source, e.target],
                        category: "edge",
                        operation: "removed",
                        data: encode(&e)?,
                    },
                )
                .await?;
            self.journal()
                .checkpoint(tx, relationship_nodes(&e), "links_changed")
                .await?;
        }
        encode(&e)
    }
}

impl PendingEdge<'_> {
    fn structural(&self) -> bool {
        matches!(
            self.endpoints.edge_type,
            EdgeType::Blocks | EdgeType::ParentChild
        )
    }
    async fn existing(&self, tx: &mut Tx<'_>, workspace: Uuid) -> Result<Option<Value>> {
        if !self.structural() {
            return Ok(None);
        }
        Ok(sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND source=$2 AND target=$3 AND edge_type=$4 AND NOT deleted").bind(workspace).bind(self.endpoints.source).bind(self.endpoints.target).bind(tag(&self.endpoints.edge_type)?).fetch_optional(&mut **tx).await?)
    }
    async fn validate_pins(&self, tx: &mut Tx<'_>, workspace: Uuid) -> Result<()> {
        for (node, rev) in [
            (self.endpoints.source, self.source_revision),
            (self.endpoints.target, self.target_revision),
        ] {
            if let Some(r) = rev {
                load_revision(tx, workspace, node, r)
                    .await
                    .map_err(|e| match e {
                        Error::NotFound => {
                            Error::Validation("pinned revision is unavailable".into())
                        }
                        e => e,
                    })?;
            }
        }
        Ok(())
    }
    fn materialize(self, workspace: Uuid) -> Edge {
        Edge {
            id: self.id.unwrap_or_else(Uuid::new_v4),
            workspace_id: workspace,
            source: self.endpoints.source,
            target: self.endpoints.target,
            edge_type: self.endpoints.edge_type,
            source_revision: self.source_revision,
            target_revision: self.target_revision,
            version: 1,
            deleted: false,
            created_at: server_time(),
            description: self.description.map(str::to_owned),
        }
    }
}
