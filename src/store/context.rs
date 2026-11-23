//! Breadth-first context reads, including pinned revisions and response size limits.
use super::*;
use std::collections::{HashMap, HashSet};

impl Store {
    pub async fn context(&self, p: &Principal, id: Uuid, q: ContextQuery) -> Result<Context> {
        q.validate()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='4s'")
            .execute(&mut *tx)
            .await?;
        let node = load_node(&mut tx, p.workspace_id, id).await?;
        let mut traversal = Traversal::new(node, q)?;
        for _ in 0..traversal.query.depth {
            if traversal.frontier.is_empty() {
                break;
            }
            traversal.expand(&mut tx).await?;
        }
        traversal.load_pins(&mut tx).await?;
        traversal.trim_response()?;
        tx.commit().await?;
        Ok(traversal.result)
    }
}

struct Traversal {
    result: Context,
    query: ContextQuery,
    seen: HashSet<Uuid>,
    edge_ids: HashSet<Uuid>,
    frontier: Vec<Uuid>,
}

impl Traversal {
    fn new(node: Node, query: ContextQuery) -> Result<Self> {
        let id = node.id;
        let result = Context {
            node,
            nodes: vec![],
            edges: vec![],
            pinned_revisions: vec![],
            truncated: false,
        };
        if encode(&result)?.to_string().len() > query.max_bytes as usize {
            return Err(Error::Validation(
                "root record exceeds max_bytes; request a larger response".into(),
            ));
        }
        Ok(Self {
            result,
            query,
            seen: HashSet::from([id]),
            edge_ids: HashSet::new(),
            frontier: vec![id],
        })
    }

    async fn expand(&mut self, tx: &mut Tx<'_>) -> Result<()> {
        let edge_cap = i64::from(self.query.limit) * 4;
        let edge_rows = sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(e)
             FROM edges e
             WHERE workspace_id=$1 AND NOT deleted AND (($2 IN ('both','outgoing') AND source=ANY($3)) OR ($2 IN ('both','incoming') AND target=ANY($3)))
             ORDER BY id
             LIMIT $4"
        )
        .bind(self.result.node.workspace_id)
        .bind(&self.query.direction)
        .bind(&self.frontier)
        .bind(edge_cap+1)
        .fetch_all(&mut **tx)
        .await?;
        if edge_rows.len() > edge_cap as usize {
            self.result.truncated = true;
        }
        let edges: Vec<Edge> = edge_rows
            .into_iter()
            .take(edge_cap as usize)
            .map(decode)
            .collect::<Result<_>>()?;
        self.load_neighbors(tx, &edges).await?;
        for edge in edges {
            if self.seen.contains(&edge.source) && self.seen.contains(&edge.target) {
                self.include_edge(edge, edge_cap as usize);
            }
        }
        Ok(())
    }

    fn include_edge(&mut self, edge: Edge, limit: usize) {
        if !self.edge_ids.insert(edge.id) {
            return;
        }
        if self.result.edges.len() >= limit {
            self.result.truncated = true;
        } else {
            self.result.edges.push(edge);
        }
    }
    async fn load_neighbors(&mut self, tx: &mut Tx<'_>, edges: &[Edge]) -> Result<()> {
        let candidate_ids: Vec<Uuid> = edges
            .iter()
            .flat_map(|e| [e.source, e.target])
            .filter(|v| !self.seen.contains(v))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let available =
            (self.query.max_bytes as usize).saturating_sub(encode(&self.result)?.to_string().len());
        // Admit neighboring documents by byte budget in PostgreSQL, rather
        // than transfer hundreds of full-size bodies and trim them afterward.
        let bundle = sqlx::query_scalar::<_, Value>(
            "WITH candidates AS (SELECT id,to_jsonb(n)-'search_document' AS document
             FROM nodes n
             WHERE workspace_id=$1 AND id=ANY($2) AND NOT deleted
             ORDER BY id
             LIMIT $3), sized AS (SELECT id,document,sum(octet_length(document::text)) OVER(ORDER BY id) AS cumulative
             FROM candidates) SELECT jsonb_build_object('documents',COALESCE(jsonb_agg(document
             ORDER BY id) FILTER(WHERE cumulative<=$4),'[]'::jsonb),'total',count(*))
             FROM sized"
        )
        .bind(self.result.node.workspace_id)
        .bind(&candidate_ids)
        .bind((self.query.limit as usize-self.seen.len()+1) as i64)
        .bind(available as i64)
        .fetch_one(&mut **tx)
        .await?;
        let documents: Vec<Value> = decode(bundle["documents"].clone())?;
        if bundle["total"].as_u64().unwrap_or(0) > documents.len() as u64 {
            self.result.truncated = true;
        }
        self.frontier.clear();
        for value in documents {
            let n: Node = decode(value)?;
            if self.seen.len() >= self.query.limit as usize {
                self.result.truncated = true;
                break;
            }
            self.seen.insert(n.id);
            self.frontier.push(n.id);
            self.result.nodes.push(n);
        }
        Ok(())
    }
    // Check snapshot sizes before fetching pinned documents.
    async fn load_pins(&mut self, tx: &mut Tx<'_>) -> Result<()> {
        let requested: HashSet<_> = self.result.edges.iter().flat_map(edge_pins).collect();
        if requested.is_empty() {
            return Ok(());
        }
        let pins: Vec<Value> = requested
            .iter()
            .map(|(id, r)| json!({"node_id":id,"revision":r}))
            .collect();
        let rows = sqlx::query(
            "SELECT DISTINCT r.node_id,r.revision,octet_length(r.snapshot::text) AS bytes
             FROM jsonb_to_recordset($2) AS pin(node_id uuid,revision bigint)
             JOIN node_revisions r ON r.node_id=pin.node_id AND r.revision=pin.revision
             WHERE r.workspace_id=$1",
        )
        .bind(self.result.node.workspace_id)
        .bind(json!(pins))
        .fetch_all(&mut **tx)
        .await?;
        let sizes: HashMap<(Uuid, i64), usize> = rows
            .into_iter()
            .map(|r| {
                (
                    (r.get("node_id"), r.get("revision")),
                    r.get::<i32, _>("bytes") as usize,
                )
            })
            .collect();
        if requested.iter().any(|pin| !sizes.contains_key(pin)) {
            return Err(Error::Conflict("pinned revision is unavailable".into()));
        }
        let included = self.retain_pins(&sizes);
        let pins: Vec<Value> = included
            .iter()
            .map(|(id, r)| json!({"node_id":id,"revision":r}))
            .collect();
        self.result.pinned_revisions = sqlx::query_scalar::<_, Value>(
            "SELECT r.snapshot
             FROM jsonb_to_recordset($2) AS pin(node_id uuid,revision bigint)
             JOIN node_revisions r ON r.node_id=pin.node_id AND r.revision=pin.revision
             WHERE r.workspace_id=$1
             ORDER BY r.node_id,r.revision",
        )
        .bind(self.result.node.workspace_id)
        .bind(json!(pins))
        .fetch_all(&mut **tx)
        .await?;
        Ok(())
    }

    fn retain_pins(&mut self, sizes: &HashMap<(Uuid, i64), usize>) -> HashSet<(Uuid, i64)> {
        let mut included = HashSet::new();
        let mut bytes = 0;
        self.result.edges.retain(|e| {
            let new: HashSet<_> = edge_pins(e).filter(|pin| !included.contains(pin)).collect();
            let extra: usize = new.iter().map(|pin| sizes[pin]).sum();
            if bytes + extra > self.query.max_bytes as usize {
                self.result.truncated = true;
                false
            } else {
                bytes += extra;
                included.extend(new);
                true
            }
        });
        included
    }

    fn trim_response(&mut self) -> Result<()> {
        // Serialize each removed item once. The running upper bound avoids
        // repeatedly serializing a broad bundle on the async request worker.
        let mut response_bytes = encode(&self.result)?.to_string().len();
        while response_bytes > self.query.max_bytes as usize {
            self.result.truncated = true;
            if let Some(n) = self.result.nodes.pop() {
                response_bytes = response_bytes.saturating_sub(encode(&n)?.to_string().len());
                self.seen.remove(&n.id);
                response_bytes = self.remove_dangling_edges(response_bytes)?;
            } else if let Some(e) = self.result.edges.pop() {
                response_bytes = response_bytes.saturating_sub(encode(&e)?.to_string().len());
            } else {
                // Conservative subtraction leaves comma overhead in the estimate.
                if encode(&self.result)?.to_string().len() <= self.query.max_bytes as usize {
                    break;
                }
                return Err(Error::Validation("root record exceeds max_bytes".into()));
            }
            response_bytes = self.remove_dangling_pins(response_bytes);
        }
        Ok(())
    }

    fn remove_dangling_edges(&mut self, mut response_bytes: usize) -> Result<usize> {
        let mut retained = vec![];
        for e in std::mem::take(&mut self.result.edges) {
            if self.seen.contains(&e.source) && self.seen.contains(&e.target) {
                retained.push(e);
            } else {
                response_bytes = response_bytes.saturating_sub(encode(&e)?.to_string().len());
            }
        }
        self.result.edges = retained;
        Ok(response_bytes)
    }

    fn remove_dangling_pins(&mut self, mut response_bytes: usize) -> usize {
        self.result.pinned_revisions.retain(|r| {
            let mut pins = self.result.edges.iter().flat_map(edge_pins);
            let referenced =
                pins.any(|(id, rev)| json!(id) == r["id"] && json!(rev) == r["content_revision"]);
            if !referenced {
                response_bytes = response_bytes.saturating_sub(r.to_string().len());
            }
            referenced
        });
        response_bytes
    }
}

fn edge_pins(e: &Edge) -> impl Iterator<Item = (Uuid, i64)> {
    [
        e.source_revision.map(|rev| (e.source, rev)),
        e.target_revision.map(|rev| (e.target, rev)),
    ]
    .into_iter()
    .flatten()
}
