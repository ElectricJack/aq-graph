//! Core graph contracts translated from AQ v1; orchestration stays outside this module.
use super::*;

pub(super) struct ProposedEdge {
    pub source: Uuid,
    pub target: Uuid,
    pub edge_type: EdgeType,
}
impl ProposedEdge {
    pub(super) async fn validate(&self, tx: &mut Tx<'_>, p: &Principal) -> Result<()> {
        let Self {
            source,
            target,
            edge_type,
        } = *self;
        if source == target {
            return Err(Error::Validation("self links are not allowed".into()));
        }
        let a = load_node(tx, p.workspace_id, source).await?;
        let b = load_node(tx, p.workspace_id, target).await?;
        validate_endpoints(&a, &b, edge_type)?;
        if matches!(edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
            self.check_cycle(tx, p.workspace_id).await?;
        }
        if edge_type == EdgeType::ParentChild {
            check_parent(tx, &a, target).await?;
        }
        Ok(())
    }
    async fn check_cycle(&self, tx: &mut Tx<'_>, workspace: Uuid) -> Result<()> {
        let cycle=sqlx::query_scalar::<_,bool>("WITH RECURSIVE reachable(id) AS (SELECT $2::uuid UNION SELECT e.target FROM edges e JOIN reachable r ON e.source=r.id WHERE e.workspace_id=$1 AND NOT e.deleted AND e.edge_type IN ('blocks','parent-child')) SELECT EXISTS(SELECT 1 FROM reachable WHERE id=$3)").bind(workspace).bind(self.target).bind(self.source).fetch_one(&mut **tx).await?;
        if cycle {
            return Err(Error::Validation(
                "relationship would create a blocking/structural cycle".into(),
            ));
        }
        Ok(())
    }
}
fn validate_endpoints(a: &Node, b: &Node, edge_type: EdgeType) -> Result<()> {
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
        EdgeType::Supports | EdgeType::Contradicts => {
            a.kind == NodeKind::Knowledge && b.kind == NodeKind::Knowledge
        }
        EdgeType::References | EdgeType::Supersedes => true,
    };
    if !valid {
        return Err(Error::Validation(format!(
            "invalid endpoint kinds for {edge_type}"
        )));
    }
    Ok(())
}
async fn check_parent(tx: &mut Tx<'_>, parent: &Node, target: Uuid) -> Result<()> {
    if matches!(
        parent.task_state,
        Some(TaskState::Completed | TaskState::Canceled)
    ) {
        return Err(Error::Conflict(
            "terminal parent refuses new children".into(),
        ));
    }
    let current_parent=sqlx::query_scalar::<_,Uuid>("SELECT source FROM edges WHERE workspace_id=$1 AND target=$2 AND edge_type='parent-child' AND NOT deleted").bind(parent.workspace_id).bind(target).fetch_optional(&mut **tx).await?;
    if current_parent.is_some() {
        return Err(Error::Conflict(
            "child already has a parent; use set_parent to reparent".into(),
        ));
    }
    let depth=sqlx::query_scalar::<_,i32>("WITH RECURSIVE ancestors(id,depth) AS (SELECT $2::uuid,1 UNION ALL SELECT e.source,a.depth+1 FROM edges e JOIN ancestors a ON e.target=a.id WHERE e.workspace_id=$1 AND e.edge_type='parent-child' AND NOT e.deleted AND a.depth<4) SELECT max(depth) FROM ancestors").bind(parent.workspace_id).bind(parent.id).fetch_one(&mut **tx).await?;
    let height=sqlx::query_scalar::<_,i32>("WITH RECURSIVE descendants(id,depth) AS (SELECT $2::uuid,1 UNION ALL SELECT e.target,d.depth+1 FROM edges e JOIN descendants d ON e.source=d.id WHERE e.workspace_id=$1 AND e.edge_type='parent-child' AND NOT e.deleted AND d.depth<4) SELECT max(depth) FROM descendants").bind(parent.workspace_id).bind(target).fetch_one(&mut **tx).await?;
    if depth + height > 3 {
        return Err(Error::Validation(
            "structural depth exceeds maximum of three levels".into(),
        ));
    }
    Ok(())
}

pub(super) struct Reparent<'a> {
    pub principal: &'a Principal,
    pub authority: &'a Principal,
    pub change: Uuid,
    pub reason: Option<&'a str>,
    pub id: Uuid,
    pub expected_version: i64,
    pub parent_id: Option<Uuid>,
}
impl Reparent<'_> {
    pub(super) async fn apply(&self, tx: &mut Tx<'_>) -> Result<Value> {
        let (id, parent_id) = (self.id, self.parent_id);
        let p = self.principal;
        let child = self.lock_child(tx).await?;
        let current=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND target=$2 AND edge_type='parent-child' AND NOT deleted FOR UPDATE").bind(p.workspace_id).bind(id).fetch_optional(&mut **tx).await?;
        let current: Option<Edge> = current.map(decode).transpose()?;
        let old_parent = current.as_ref().map(|e| e.source);
        if old_parent == parent_id {
            return encode(&child);
        }
        for parent in [old_parent, parent_id].into_iter().flatten() {
            let node = load_node_write(tx, p.workspace_id, parent).await?;
            knowledge_rules::authorize_edit(self.authority, &node)?;
            if node.retired {
                return Err(Error::Forbidden);
            }
        }
        if let Some(edge) = current {
            self.detach(tx, edge).await?;
        }
        if let Some(parent) = parent_id {
            self.attach(tx, parent).await?;
        }
        let mut changed = vec![id];
        changed.extend(old_parent);
        changed.extend(parent_id);
        checkpoint_nodes(tx, p, self.change, changed, "reparented", self.reason).await?;
        let child = load_node(tx, p.workspace_id, id).await?;
        history_entry(
            tx,
            p,
            self.change,
            vec![id],
            "lifecycle",
            "reparented",
            self.reason,
            json!({"from_parent":old_parent,"to_parent":parent_id,"version":child.version}),
        )
        .await?;
        encode(&child)
    }

    async fn lock_child(&self, tx: &mut Tx<'_>) -> Result<Node> {
        let child = load_node_write(tx, self.principal.workspace_id, self.id).await?;
        expected(child.version, self.expected_version)?;
        if child.kind != NodeKind::Task {
            return Err(Error::Validation(
                "only tasks can have structural parents".into(),
            ));
        }
        knowledge_rules::authorize_edit(self.authority, &child)?;
        if child.retired {
            return Err(Error::Forbidden);
        }
        if child.deleted {
            return Err(Error::Conflict(
                "deleted task must be restored before reparenting".into(),
            ));
        }
        Ok(child)
    }
    async fn detach(&self, tx: &mut Tx<'_>, mut e: Edge) -> Result<()> {
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
        Ok(())
    }
    async fn attach(&self, tx: &mut Tx<'_>, parent: Uuid) -> Result<()> {
        ProposedEdge {
            source: parent,
            target: self.id,
            edge_type: EdgeType::ParentChild,
        }
        .validate(tx, self.principal)
        .await?;
        let e: Edge = decode(
            json!({"id":Uuid::new_v4(),"workspace_id":self.principal.workspace_id,"source":parent,"target":self.id,"edge_type":"parent-child","source_revision":null,"target_revision":null,"description":null,"version":1,"deleted":false,"created_at":server_time()}),
        )?;
        sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,source_revision,target_revision,version,deleted,created_at) VALUES($1,$2,$3,$4,'parent-child',NULL,NULL,1,false,$5)").bind(e.id).bind(self.principal.workspace_id).bind(parent).bind(self.id).bind(e.created_at).execute(&mut **tx).await?;
        history_entry(
            tx,
            self.principal,
            self.change,
            vec![parent, self.id],
            "edge",
            "added",
            self.reason,
            encode(&e)?,
        )
        .await?;
        Ok(())
    }
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
        let bytes = json!({"node":root,"children":[],"truncated":false})
            .to_string()
            .len();
        if bytes > q.max_bytes as usize {
            return Err(Error::Validation("root record exceeds max_bytes".into()));
        }
        let mut page = TreePage {
            query: q,
            bytes,
            entries: Default::default(),
            frontier: vec![id],
            truncated: false,
        };
        for _ in 0..page.query.depth {
            if page.frontier.is_empty() {
                break;
            }
            page.expand(&mut tx, p.workspace_id).await?;
        }
        if !page.frontier.is_empty() {
            let beyond=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.id=e.target AND n.workspace_id=e.workspace_id WHERE e.workspace_id=$1 AND e.source=ANY($2) AND e.edge_type='parent-child' AND NOT e.deleted AND NOT n.deleted)").bind(p.workspace_id).bind(&page.frontier).fetch_one(&mut *tx).await?;
            page.truncated |= beyond;
        }
        let mut result = branch(&root, &page.entries);
        result["truncated"] = json!(page.truncated);
        tx.commit().await?;
        Ok(result)
    }
}

struct TreePage {
    query: ContextQuery,
    bytes: usize,
    entries: std::collections::BTreeMap<Uuid, (Node, Uuid)>,
    frontier: Vec<Uuid>,
    truncated: bool,
}
impl TreePage {
    async fn expand(&mut self, tx: &mut Tx<'_>, workspace: Uuid) -> Result<()> {
        let available = (self.query.max_bytes as usize).saturating_sub(self.bytes);
        let bundle=sqlx::query_scalar::<_,Value>("WITH candidates AS (SELECT e.source,n.id,to_jsonb(n)-'search_document' AS node FROM edges e JOIN nodes n ON n.id=e.target AND n.workspace_id=e.workspace_id WHERE e.workspace_id=$1 AND e.source=ANY($2) AND e.edge_type='parent-child' AND NOT e.deleted AND NOT n.deleted ORDER BY n.id LIMIT $3), sized AS (SELECT source,id,node,sum(octet_length(node::text)+64) OVER(ORDER BY id) AS cumulative FROM candidates) SELECT jsonb_build_object('entries',COALESCE(jsonb_agg(jsonb_build_object('source',source,'node',node) ORDER BY id) FILTER(WHERE cumulative<=$4),'[]'::jsonb),'total',count(*)) FROM sized").bind(workspace).bind(&self.frontier).bind(i64::from(self.query.limit)-self.entries.len() as i64).bind(available as i64).fetch_one(&mut **tx).await?;
        let rows: Vec<Value> = decode(bundle["entries"].clone())?;
        if bundle["total"].as_u64().unwrap_or(0) > rows.len() as u64 {
            self.truncated = true;
        }
        self.frontier.clear();
        for row in rows {
            let node: Node = decode(row["node"].clone())?;
            let size = json!({"node":node,"children":[],"truncated":false})
                .to_string()
                .len()
                + 1;
            if self.entries.len() + 1 >= self.query.limit as usize
                || self.bytes + size > self.query.max_bytes as usize
            {
                self.truncated = true;
                continue;
            }
            self.bytes += size;
            self.frontier.push(node.id);
            self.entries
                .insert(node.id, (node, decode(row["source"].clone())?));
        }
        Ok(())
    }
}
fn branch(node: &Node, entries: &std::collections::BTreeMap<Uuid, (Node, Uuid)>) -> Value {
    let children: Vec<_> = entries
        .values()
        .filter(|(_, parent)| *parent == node.id)
        .map(|(child, _)| branch(child, entries))
        .collect();
    json!({"node":node,"children":children,"truncated":false})
}
