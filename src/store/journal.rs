//! Record the immutable history of an authorized changeset.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Journal<'a> {
    pub principal: &'a Principal,
    pub change: Uuid,
    pub reason: Option<&'a str>,
}
pub(super) struct HistoryEvent<'a> {
    pub nodes: Vec<Uuid>,
    pub category: &'a str,
    pub operation: &'a str,
    pub data: Value,
}
pub(super) struct ChangeSet<'a> {
    pub name: Option<&'a str>,
    pub operations: &'a [Mutation],
    pub status: &'a str,
}

impl Journal<'_> {
    pub(super) async fn record(&self, tx: &mut Tx<'_>, event: HistoryEvent<'_>) -> Result<()> {
        let HistoryEvent {
            nodes,
            category,
            operation,
            data,
        } = event;
        if category == "edge" {
            let e: Edge = decode(data.clone())?;
            sqlx::query("INSERT INTO edge_versions(workspace_id,id,version,snapshot) SELECT workspace_id,id,version,to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=$2 AND version=$3 ON CONFLICT(workspace_id,id,version) DO NOTHING").bind(self.principal.workspace_id).bind(e.id).bind(e.version).execute(&mut **tx).await?;
        }
        sqlx::query("INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(self.principal.workspace_id).bind(nodes).bind(Uuid::new_v4()).bind(category).bind(&self.principal.actor_id).bind(operation).bind(self.reason).bind(self.change).bind(data).execute(&mut **tx).await?;
        Ok(())
    }
    pub(super) async fn revision(&self, tx: &mut Tx<'_>, n: &Node, operation: &str) -> Result<()> {
        let snapshot = encode(n)?;
        let link_set = revision_links::capture_links(tx, self.principal, n.id).await?;
        sqlx::query("INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id,link_set_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(self.principal.workspace_id).bind(n.id).bind(n.content_revision).bind(&snapshot).bind(&self.principal.actor_id).bind(operation).bind(self.reason).bind(self.change).bind(link_set).execute(&mut **tx).await?;
        self.record(
            tx,
            HistoryEvent {
                nodes: vec![n.id],
                category: "revision",
                operation,
                data: snapshot,
            },
        )
        .await
    }
    pub(super) async fn checkpoint(
        &self,
        tx: &mut Tx<'_>,
        ids: Vec<Uuid>,
        operation: &str,
    ) -> Result<()> {
        let mut ids = ids;
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let mut n = load_node_write(tx, self.principal.workspace_id, id).await?;
            knowledge_rules::reset_verification(&mut n);
            knowledge_rules::clear_summary(&mut n);
            n.version += 1;
            n.content_revision += 1;
            n.updated_at = server_time();
            save_node(tx, &n, false).await?;
            self.revision(tx, &n, operation).await?;
        }
        Ok(())
    }
    /// Deleting a task also removes every incident relationship atomically. Other
    /// tasks survive, including children and tasks that depended on this one.
    pub(super) async fn remove_task_links(&self, tx: &mut Tx<'_>, task: Uuid) -> Result<()> {
        let pointers = sqlx::query_scalar::<_, Value>("SELECT to_jsonb(n) FROM nodes n WHERE workspace_id=$1 AND successor_id=$2 ORDER BY id FOR UPDATE")
        .bind(self.principal.workspace_id).bind(task).fetch_all(&mut **tx).await?;
        for row in pointers {
            let mut n: Node = decode(row)?;
            n.successor_id = None;
            n.version += 1;
            n.content_revision += 1;
            n.updated_at = server_time();
            knowledge_rules::reset_verification(&mut n);
            knowledge_rules::clear_summary(&mut n);
            save_node(tx, &n, false).await?;
            self.revision(tx, &n, "successor_removed").await?;
        }
        let rows = sqlx::query_scalar::<_, Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND (source=$2 OR target=$2) AND NOT deleted ORDER BY id FOR UPDATE")
        .bind(self.principal.workspace_id).bind(task).fetch_all(&mut **tx).await?;
        for row in rows {
            let mut e: Edge = decode(row)?;
            e.deleted = true;
            e.version += 1;
            sqlx::query("UPDATE edges SET deleted=true,version=$3 WHERE workspace_id=$1 AND id=$2")
                .bind(self.principal.workspace_id)
                .bind(e.id)
                .bind(e.version)
                .execute(&mut **tx)
                .await?;
            self.record(
                tx,
                HistoryEvent {
                    nodes: vec![e.source, e.target],
                    category: "edge",
                    operation: "removed",
                    data: encode(&e)?,
                },
            )
            .await?;
            // The deleted node already has its deletion revision; only checkpoint
            // surviving endpoints, so its returned receipt version stays exact.
            let survivors = relationship_nodes(&e)
                .into_iter()
                .filter(|id| *id != task)
                .collect();
            self.checkpoint(tx, survivors, "links_changed").await?;
        }
        Ok(())
    }
    pub(super) async fn insert(&self, tx: &mut Tx<'_>, changeset: ChangeSet<'_>) -> Result<()> {
        if self.reason.is_some_and(|v| v.len() > 4096) {
            return Err(Error::Validation("reason exceeds 4096 bytes".into()));
        }
        sqlx::query("INSERT INTO change_sets(id,workspace_id,actor_id,name,reason,operations,status) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(self.change).bind(self.principal.workspace_id).bind(&self.principal.actor_id).bind(changeset.name).bind(self.reason).bind(encode(&changeset.operations)?).bind(changeset.status).execute(&mut **tx).await?;
        Ok(())
    }
}
