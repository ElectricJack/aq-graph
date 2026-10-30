//! PostgreSQL authority for current graph state and selective immutable history.
use crate::domain::*;
use crate::knowledge_rules;
mod compat_graph;
mod preview;
mod queries;
mod restore;
mod revision_links;
mod successors;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}
type Tx<'a> = Transaction<'a, Postgres>;
const MAX_OPERATIONS: usize = 100;
const MAX_ATTEMPTS: usize = 8;

fn hash(v: &[u8]) -> String {
    hex::encode(Sha256::digest(v))
}
fn encode<T: Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Error::Internal(e.to_string()))
}
fn decode<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error::Internal(e.to_string()))
}
fn tag<T: Serialize>(v: &T) -> Result<String> {
    encode(v)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::Internal("invalid enum".into()))
}
fn can_write(p: &Principal) -> Result<()> {
    match p.capability {
        Capability::Write | Capability::Admin => Ok(()),
        _ => Err(Error::Forbidden),
    }
}
fn admin(p: &Principal) -> Result<()> {
    match p.capability {
        Capability::Admin => Ok(()),
        _ => Err(Error::Forbidden),
    }
}
fn retryable(e: &Error) -> bool {
    match e {
        Error::Database(sqlx::Error::Database(e)) => {
            matches!(e.code().as_deref(), Some("40001" | "40P01"))
        }
        _ => false,
    }
}
fn normalize_db(e: Error) -> Error {
    match e {
        Error::Database(sqlx::Error::Database(ref d))
            if matches!(d.code().as_deref(), Some("23505")) =>
        {
            Error::Conflict("identity or relationship already exists".into())
        }
        Error::Database(sqlx::Error::Database(ref d))
            if matches!(d.code().as_deref(), Some("57014" | "55P03")) =>
        {
            Error::Overloaded
        }
        e => e,
    }
}
fn limit(q: &ListQuery) -> Result<i64> {
    if q.limit == 0 || q.limit > 200 {
        return Err(Error::Validation("limit must be between 1 and 200".into()));
    }
    Ok(i64::from(q.limit))
}
fn cursor(q: &ListQuery) -> Result<Option<Uuid>> {
    q.cursor
        .as_ref()
        .map(|s| Uuid::parse_str(s).map_err(|_| Error::Validation("invalid cursor".into())))
        .transpose()
}
fn check_key(k: &str) -> Result<()> {
    validate_key(k)
}
fn check_operations(ops: &[Mutation]) -> Result<()> {
    validate_operations(ops)?;
    if ops.is_empty() || ops.len() > MAX_OPERATIONS {
        return Err(Error::Validation(
            "a batch must contain 1..100 operations".into(),
        ));
    }
    if serde_json::to_vec(ops)
        .map_err(|e| Error::Validation(e.to_string()))?
        .len()
        > 1_048_576
    {
        return Err(Error::Validation("batch exceeds 1 MiB".into()));
    }
    Ok(())
}

impl Store {
    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self> {
        if max_connections == 0 || max_connections > 128 {
            return Err(Error::Validation("pool size must be 1..128".into()));
        }
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(2))
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout='4s'")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("SET lock_timeout='2s'")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("migration failed: {e}")))
    }
    pub async fn bootstrap(&self, workspace: Uuid, actor: &str, token: &str) -> Result<()> {
        validate_credential(actor, token)?;
        let mut tx = self.pool.begin().await?;
        // Initialization only: never replace credentials on an existing workspace.
        let inserted = sqlx::query("INSERT INTO workspaces(id) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(workspace)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if inserted == 0 {
            let existing=sqlx::query("SELECT actor_id,capability FROM credentials WHERE workspace_id=$1 AND token_hash=$2").bind(workspace).bind(hash(token.as_bytes())).fetch_optional(&mut *tx).await?;
            if !existing.is_some_and(|r| {
                r.get::<String, _>("actor_id") == actor
                    && r.get::<String, _>("capability") == "admin"
            }) {
                return Err(Error::Conflict(
                    "workspace exists; bootstrap cannot replace its credentials".into(),
                ));
            }
        } else {
            sqlx::query("INSERT INTO credentials(workspace_id,actor_id,token_hash,capability) VALUES($1,$2,$3,'admin')").bind(workspace).bind(actor).bind(hash(token.as_bytes())).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn authenticate(&self, token: &str, workspace: Uuid) -> Result<Principal> {
        let row = sqlx::query(
            "SELECT actor_id,capability FROM credentials WHERE workspace_id=$1 AND token_hash=$2",
        )
        .bind(workspace)
        .bind(hash(token.as_bytes()))
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::Unauthorized)?;
        Ok(Principal {
            actor_id: row.get("actor_id"),
            workspace_id: workspace,
            capability: decode(json!(row.get::<String, _>("capability")))?,
        })
    }
    pub async fn grant(
        &self,
        p: &Principal,
        actor: &str,
        token: &str,
        capability: Capability,
    ) -> Result<()> {
        admin(p)?;
        validate_credential(actor, token)?;
        retry_write(|| self.grant_once(p, actor, token, capability)).await
    }
    async fn grant_once(
        &self,
        p: &Principal,
        actor: &str,
        token: &str,
        capability: Capability,
    ) -> Result<()> {
        let mut tx = self.write_tx().await?;
        live_authority(&mut tx, p, true).await?;
        sqlx::query("INSERT INTO credentials(workspace_id,actor_id,token_hash,capability) VALUES($1,$2,$3,$4) ON CONFLICT(workspace_id,actor_id) DO UPDATE SET token_hash=EXCLUDED.token_hash,capability=EXCLUDED.capability").bind(p.workspace_id).bind(actor).bind(hash(token.as_bytes())).bind(tag(&capability)?).execute(&mut *tx).await.map_err(|e|normalize_db(e.into()))?;
        // Administrators cannot accidentally lock out their entire workspace.
        ensure_admin(&mut tx, p.workspace_id).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn revoke(&self, p: &Principal, actor: &str) -> Result<()> {
        admin(p)?;
        retry_write(|| self.revoke_once(p, actor)).await
    }
    async fn revoke_once(&self, p: &Principal, actor: &str) -> Result<()> {
        let mut tx = self.write_tx().await?;
        live_authority(&mut tx, p, true).await?;
        let n = sqlx::query("DELETE FROM credentials WHERE workspace_id=$1 AND actor_id=$2")
            .bind(p.workspace_id)
            .bind(actor)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(Error::NotFound);
        }
        ensure_admin(&mut tx, p.workspace_id).await?;
        tx.commit().await?;
        Ok(())
    }
    async fn write_tx(&self) -> Result<Tx<'_>> {
        self.write_tx_with(true).await
    }
    async fn mutation_tx(&self, ops: &[Mutation]) -> Result<Tx<'_>> {
        let structural = ops.iter().any(|op| {
            matches!(
                op,
                Mutation::AddEdge { .. }
                    | Mutation::RemoveEdge { .. }
                    | Mutation::DeleteNode { .. }
                    | Mutation::SetParent { .. }
                    | Mutation::RestoreNode { .. }
                    | Mutation::UpdateNode {
                        patch: NodePatch {
                            task_state: Some(_),
                            ..
                        },
                        ..
                    }
            )
        });
        let successor_change = ops.iter().any(|op| matches!(op,
            Mutation::UpdateNode { patch, .. } if patch.retired.is_some() || !patch.successor_id.is_keep()));
        self.write_tx_with(structural || successor_change).await
    }
    async fn write_tx_with(&self, serializable: bool) -> Result<Tx<'_>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(if serializable {
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"
        } else {
            "SET TRANSACTION ISOLATION LEVEL READ COMMITTED"
        })
        .execute(&mut *tx)
        .await?;
        sqlx::query("SET LOCAL statement_timeout='4s'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL lock_timeout='2s'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL idle_in_transaction_session_timeout='5s'")
            .execute(&mut *tx)
            .await?;
        Ok(tx)
    }
    pub async fn get_node(&self, p: &Principal, id: Uuid) -> Result<Node> {
        let v = sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND id=$2",
        )
        .bind(p.workspace_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::NotFound)?;
        decode(v)
    }
    pub async fn list_nodes(
        &self,
        p: &Principal,
        kind: Option<NodeKind>,
        q: ListQuery,
    ) -> Result<Page<Node>> {
        self.list_filtered(
            p,
            NodeFilters {
                kind,
                ..Default::default()
            },
            q,
        )
        .await
    }
    pub async fn ready(&self, p: &Principal, q: ListQuery) -> Result<Page<Node>> {
        let n = limit(&q)?;
        // Schema 0004 and every structural mutation enforce at most three levels.
        // Two indexed ancestor joins suffice; a per-candidate recursive CTE can
        // cause repeated full node scans and JIT overhead even on small graphs.
        // A future hierarchy-depth increase must also extend this predicate.
        let vs=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND kind='task' AND task_state='active' AND NOT deleted AND NOT retired AND ($2::uuid IS NULL OR id>$2)
AND NOT EXISTS (SELECT 1 FROM edges pc JOIN nodes child ON child.id=pc.target AND child.workspace_id=pc.workspace_id WHERE pc.workspace_id=n.workspace_id AND pc.source=n.id AND pc.edge_type='parent-child' AND NOT pc.deleted AND NOT child.deleted)
AND NOT EXISTS (
 SELECT 1 FROM edges pe JOIN nodes parent ON parent.id=pe.source AND parent.workspace_id=pe.workspace_id
 LEFT JOIN edges ge ON ge.workspace_id=pe.workspace_id AND ge.target=parent.id AND ge.edge_type='parent-child' AND NOT ge.deleted
 LEFT JOIN nodes grandparent ON grandparent.id=ge.source AND grandparent.workspace_id=ge.workspace_id
 WHERE pe.workspace_id=n.workspace_id AND pe.target=n.id AND pe.edge_type='parent-child' AND NOT pe.deleted
 AND (parent.deleted OR parent.retired OR parent.task_state='draft' OR grandparent.deleted OR grandparent.retired OR grandparent.task_state='draft'))
AND NOT EXISTS (SELECT 1 FROM edges e JOIN nodes prerequisite ON prerequisite.id=e.source WHERE e.workspace_id=n.workspace_id AND e.target=n.id AND e.edge_type='blocks' AND NOT e.deleted AND (prerequisite.deleted OR prerequisite.task_state IS DISTINCT FROM 'completed')) ORDER BY id LIMIT $3").bind(p.workspace_id).bind(cursor(&q)?).bind(n+1).fetch_all(&self.pool).await?;
        node_page(vs, n)
    }
    pub async fn search(&self, p: &Principal, text: &str, q: ListQuery) -> Result<Page<Node>> {
        self.search_filtered(p, text, q, false, false).await
    }
    pub async fn search_filtered(
        &self,
        p: &Principal,
        text: &str,
        q: ListQuery,
        include_retired: bool,
        include_disputed: bool,
    ) -> Result<Page<Node>> {
        self.search_nodes(
            p,
            text,
            NodeFilters {
                include_retired,
                include_disputed,
                ..Default::default()
            },
            q,
        )
        .await
    }
    pub async fn context(&self, p: &Principal, id: Uuid, q: ContextQuery) -> Result<Context> {
        q.validate()?;
        if q.depth > 5
            || q.limit == 0
            || q.limit > 200
            || q.max_bytes < 1024
            || q.max_bytes > 1_048_576
            || !matches!(q.direction.as_str(), "both" | "incoming" | "outgoing")
        {
            return Err(Error::Validation("context requires depth <=5, limit 1..200, max_bytes 1024..1048576, and direction both/incoming/outgoing".into()));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='4s'")
            .execute(&mut *tx)
            .await?;
        let node = load_node(&mut tx, p.workspace_id, id).await?;
        let mut result = Context {
            node,
            nodes: vec![],
            edges: vec![],
            pinned_revisions: vec![],
            truncated: false,
        };
        if encode(&result)?.to_string().len() > q.max_bytes as usize {
            return Err(Error::Validation(
                "root record exceeds max_bytes; request a larger response".into(),
            ));
        }
        let mut seen = std::collections::HashSet::from([id]);
        let mut edge_ids = std::collections::HashSet::new();
        let mut frontier = vec![id];
        let edge_cap = i64::from(q.limit) * 4;
        for _ in 0..q.depth {
            if frontier.is_empty() {
                break;
            }
            let vs=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND NOT deleted AND (($2 IN ('both','outgoing') AND source=ANY($3)) OR ($2 IN ('both','incoming') AND target=ANY($3))) ORDER BY id LIMIT $4").bind(p.workspace_id).bind(&q.direction).bind(&frontier).bind(edge_cap+1).fetch_all(&mut *tx).await?;
            if vs.len() > edge_cap as usize {
                result.truncated = true;
            }
            let edges: Vec<Edge> = vs
                .into_iter()
                .take(edge_cap as usize)
                .map(decode)
                .collect::<Result<_>>()?;
            let candidate_ids: Vec<Uuid> = edges
                .iter()
                .flat_map(|e| [e.source, e.target])
                .filter(|v| !seen.contains(v))
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let available =
                (q.max_bytes as usize).saturating_sub(encode(&result)?.to_string().len());
            // Admit neighboring documents by byte budget in PostgreSQL, rather
            // than transfer hundreds of full-size bodies and trim them afterward.
            let bundle=sqlx::query_scalar::<_,Value>("WITH candidates AS (SELECT id,to_jsonb(n)-'search_document' AS document FROM nodes n WHERE workspace_id=$1 AND id=ANY($2) AND NOT deleted ORDER BY id LIMIT $3), sized AS (SELECT id,document,sum(octet_length(document::text)) OVER(ORDER BY id) AS cumulative FROM candidates) SELECT jsonb_build_object('documents',COALESCE(jsonb_agg(document ORDER BY id) FILTER(WHERE cumulative<=$4),'[]'::jsonb),'total',count(*)) FROM sized").bind(p.workspace_id).bind(&candidate_ids).bind((q.limit as usize-seen.len()+1) as i64).bind(available as i64).fetch_one(&mut *tx).await?;
            let vs: Vec<Value> = decode(bundle["documents"].clone())?;
            if bundle["total"].as_u64().unwrap_or(0) > vs.len() as u64 {
                result.truncated = true;
            }
            frontier.clear();
            for value in vs {
                let n: Node = decode(value)?;
                if seen.len() >= q.limit as usize {
                    result.truncated = true;
                    break;
                }
                seen.insert(n.id);
                frontier.push(n.id);
                result.nodes.push(n);
            }
            for e in edges {
                if seen.contains(&e.source) && seen.contains(&e.target) && edge_ids.insert(e.id) {
                    if result.edges.len() >= edge_cap as usize {
                        result.truncated = true;
                    } else {
                        result.edges.push(e);
                    }
                }
            }
        }
        // Inspect pin sizes without materializing snapshots. The byte cap also
        // bounds intermediate database-to-service transfer, even for high-degree
        // nodes whose individual pinned revisions contain large documents.
        let requested: std::collections::HashSet<(Uuid, i64)> = result
            .edges
            .iter()
            .flat_map(|e| {
                [
                    e.source_revision.map(|r| (e.source, r)),
                    e.target_revision.map(|r| (e.target, r)),
                ]
            })
            .flatten()
            .collect();
        if !requested.is_empty() {
            let pins: Vec<Value> = requested
                .iter()
                .map(|(id, r)| json!({"node_id":id,"revision":r}))
                .collect();
            let rows=sqlx::query("SELECT DISTINCT r.node_id,r.revision,octet_length(r.snapshot::text) AS bytes FROM jsonb_to_recordset($2) AS pin(node_id uuid,revision bigint) JOIN node_revisions r ON r.node_id=pin.node_id AND r.revision=pin.revision WHERE r.workspace_id=$1").bind(p.workspace_id).bind(json!(pins)).fetch_all(&mut *tx).await?;
            let sizes: std::collections::HashMap<(Uuid, i64), usize> = rows
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
            let mut included = std::collections::HashSet::new();
            let mut bytes = 0;
            result.edges.retain(|e| {
                let new: std::collections::HashSet<_> = [
                    e.source_revision.map(|r| (e.source, r)),
                    e.target_revision.map(|r| (e.target, r)),
                ]
                .into_iter()
                .flatten()
                .filter(|pin| !included.contains(pin))
                .collect();
                let extra: usize = new.iter().map(|pin| sizes[pin]).sum();
                if bytes + extra > q.max_bytes as usize {
                    result.truncated = true;
                    false
                } else {
                    bytes += extra;
                    included.extend(new);
                    true
                }
            });
            let pins: Vec<Value> = included
                .iter()
                .map(|(id, r)| json!({"node_id":id,"revision":r}))
                .collect();
            result.pinned_revisions=sqlx::query_scalar::<_,Value>("SELECT r.snapshot FROM jsonb_to_recordset($2) AS pin(node_id uuid,revision bigint) JOIN node_revisions r ON r.node_id=pin.node_id AND r.revision=pin.revision WHERE r.workspace_id=$1 ORDER BY r.node_id,r.revision").bind(p.workspace_id).bind(json!(pins)).fetch_all(&mut *tx).await?;
        }
        // Serialize each removed item once. The running upper bound avoids
        // repeatedly serializing a broad bundle on the async request worker.
        let mut response_bytes = encode(&result)?.to_string().len();
        while response_bytes > q.max_bytes as usize {
            result.truncated = true;
            if let Some(n) = result.nodes.pop() {
                response_bytes = response_bytes.saturating_sub(encode(&n)?.to_string().len());
                seen.remove(&n.id);
                let mut retained = vec![];
                for e in std::mem::take(&mut result.edges) {
                    if seen.contains(&e.source) && seen.contains(&e.target) {
                        retained.push(e);
                    } else {
                        response_bytes =
                            response_bytes.saturating_sub(encode(&e)?.to_string().len());
                    }
                }
                result.edges = retained;
            } else if let Some(e) = result.edges.pop() {
                response_bytes = response_bytes.saturating_sub(encode(&e)?.to_string().len());
            } else {
                // Conservative subtraction leaves comma overhead in the estimate.
                if encode(&result)?.to_string().len() <= q.max_bytes as usize {
                    break;
                }
                return Err(Error::Validation("root record exceeds max_bytes".into()));
            }
            let mut retained = vec![];
            for r in std::mem::take(&mut result.pinned_revisions) {
                if result.edges.iter().any(|e| {
                    (json!(e.source) == r["id"]
                        && e.source_revision
                            .is_some_and(|v| json!(v) == r["content_revision"]))
                        || (json!(e.target) == r["id"]
                            && e.target_revision
                                .is_some_and(|v| json!(v) == r["content_revision"]))
                }) {
                    retained.push(r);
                } else {
                    response_bytes = response_bytes.saturating_sub(r.to_string().len());
                }
            }
            result.pinned_revisions = retained;
        }
        tx.commit().await?;
        Ok(result)
    }
    pub async fn history(&self, p: &Principal, id: Uuid, q: ListQuery) -> Result<Value> {
        self.get_node(p, id).await?;
        let n = limit(&q)?;
        let after = q
            .cursor
            .as_deref()
            .map(|s| {
                s.parse::<i64>()
                    .map_err(|_| Error::Validation("invalid history cursor".into()))
            })
            .transpose()?
            .unwrap_or(0);
        let mut items=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(h)-'workspace_id'-'node_ids' FROM history_entries h WHERE workspace_id=$1 AND node_ids @> ARRAY[$2]::uuid[] AND sequence>$3 ORDER BY sequence LIMIT $4").bind(p.workspace_id).bind(id).bind(after).bind(n+1).fetch_all(&self.pool).await?;
        let more = items.len() > n as usize;
        items.truncate(n as usize);
        let next = if more {
            items
                .last()
                .and_then(|v| v["sequence"].as_i64())
                .map(|v| v.to_string())
        } else {
            None
        };
        Ok(json!({"items":items,"next_cursor":next}))
    }
    pub async fn get_revision(&self, p: &Principal, id: Uuid, rev: i64) -> Result<Value> {
        if rev < 1 {
            return Err(Error::Validation("revision must be positive".into()));
        }
        let mut tx = self.pool.begin().await?;
        let snapshot = encode(&decode::<Node>(
            load_revision(&mut tx, p.workspace_id, id, rev).await?,
        )?)?;
        let available=sqlx::query_scalar::<_,bool>("SELECT link_set_id IS NOT NULL FROM node_revisions WHERE workspace_id=$1 AND node_id=$2 AND revision=$3").bind(p.workspace_id).bind(id).bind(rev).fetch_one(&mut *tx).await?;
        let links = if available {
            encode(&revision_links::load_links(&mut tx, p.workspace_id, id, rev).await?)?
        } else {
            Value::Null
        };
        Ok(json!({"node":snapshot,"links":links,"relationships_available":available}))
    }
    pub async fn diff(&self, p: &Principal, id: Uuid, from: i64, to: i64) -> Result<Value> {
        let before = self.get_revision(p, id, from).await?;
        let after = self.get_revision(p, id, to).await?;
        let mut changes = vec![];
        for field in DURABLE_FIELDS {
            if before["node"][field] != after["node"][field] {
                changes.push(json!({"field":field,"before":before["node"][field],"after":after["node"][field]}));
            }
        }
        let available =
            before["relationships_available"] == true && after["relationships_available"] == true;
        if available && before["links"] != after["links"] {
            changes.push(json!({"field":"links","before":before["links"],"after":after["links"]}));
        }
        Ok(
            json!({"node_id":id,"from":from,"to":to,"before":before["node"],"after":after["node"],"before_links":before["links"],"after_links":after["links"],"relationships_available":available,"changes":changes}),
        )
    }
    pub async fn mutate(&self, p: &Principal, req: MutationRequest) -> Result<MutationResult> {
        can_write(p)?;
        req.validate()?;
        check_operations(&req.operations)?;
        let payload =
            hash(&serde_json::to_vec(&req).map_err(|e| Error::Validation(e.to_string()))?);
        for attempt in 0..MAX_ATTEMPTS {
            let outcome =
                tokio::time::timeout(Duration::from_secs(6), self.mutate_once(p, &req, &payload))
                    .await
                    .map_err(|_| Error::Overloaded)?;
            match outcome {
                Err(ref e) if retryable(e) && attempt + 1 < MAX_ATTEMPTS => {
                    tokio::time::sleep(retry_delay(attempt)).await
                }
                Err(ref e) if retryable(e) => return Err(Error::Overloaded),
                v => return v.map_err(normalize_db),
            }
        }
        Err(Error::Overloaded)
    }
    async fn mutate_once(
        &self,
        p: &Principal,
        req: &MutationRequest,
        payload: &str,
    ) -> Result<MutationResult> {
        let mut tx = self.mutation_tx(&req.operations).await?;
        live_authority(&mut tx, p, p.capability == Capability::Admin).await?;
        if requires_admin(&mut tx, p, &req.operations).await? {
            admin(p)?;
            live_authority(&mut tx, p, true).await?;
        }
        if let Some(v) = receipt(&mut tx, p, "mutate", &req.idempotency_key, payload).await? {
            return decode(v);
        }
        lock_receipt(&mut tx, p, "mutate", &req.idempotency_key).await?;
        if let Some(v) = receipt(&mut tx, p, "mutate", &req.idempotency_key, payload).await? {
            return decode(v);
        }
        let id = Uuid::new_v4();
        insert_changeset(
            &mut tx,
            p,
            id,
            None,
            req.reason.as_deref(),
            &req.operations,
            "applied",
        )
        .await?;
        let results = execute(
            &mut tx,
            p,
            id,
            req.reason.as_deref(),
            &req.operations,
            false,
        )
        .await?;
        let response = MutationResult {
            change_set_id: id,
            results,
        };
        check_response(&response)?;
        save_result(&mut tx, id, &response).await?;
        save_receipt(
            &mut tx,
            p,
            "mutate",
            &req.idempotency_key,
            payload,
            encode(&response)?,
        )
        .await?;
        tx.commit().await?;
        Ok(response)
    }
    pub async fn create_draft(&self, p: &Principal, req: DraftRequest) -> Result<Value> {
        can_write(p)?;
        check_key(&req.idempotency_key)?;
        check_operations(&req.operations)?;
        if req.name.trim().is_empty() || req.name.len() > 200 {
            return Err(Error::Validation(
                "draft name must contain 1..200 bytes".into(),
            ));
        }
        let payload =
            hash(&serde_json::to_vec(&req).map_err(|e| Error::Validation(e.to_string()))?);
        for attempt in 0..MAX_ATTEMPTS {
            let outcome: Result<Value> = tokio::time::timeout(Duration::from_secs(6), async {
                let mut tx = self.write_tx_with(false).await?;
                live_authority(&mut tx, p, p.capability == Capability::Admin).await?;
                if let Some(v) =
                    receipt(&mut tx, p, "create_draft", &req.idempotency_key, &payload).await?
                {
                    return Ok(v);
                }
                lock_receipt(&mut tx, p, "create_draft", &req.idempotency_key).await?;
                if let Some(v) =
                    receipt(&mut tx, p, "create_draft", &req.idempotency_key, &payload).await?
                {
                    return Ok(v);
                }
                let id = Uuid::new_v4();
                // Assign proposed identities once so preview and application resolve identical nodes.
                let operations = assign_ids(&req.operations)?;
                insert_changeset(
                    &mut tx,
                    p,
                    id,
                    Some(&req.name),
                    req.reason.as_deref(),
                    &operations,
                    "draft",
                )
                .await?;
                let result = load_draft(&mut tx, p.workspace_id, id).await?;
                check_response(&result)?;
                save_receipt(
                    &mut tx,
                    p,
                    "create_draft",
                    &req.idempotency_key,
                    &payload,
                    result.clone(),
                )
                .await?;
                tx.commit().await?;
                Ok(result)
            })
            .await
            .map_err(|_| Error::Overloaded)?;
            match outcome {
                Err(ref e) if retryable(e) && attempt + 1 < MAX_ATTEMPTS => {
                    tokio::time::sleep(retry_delay(attempt)).await
                }
                Err(ref e) if retryable(e) => return Err(Error::Overloaded),
                v => return v.map_err(normalize_db),
            }
        }
        Err(Error::Overloaded)
    }
    pub async fn get_draft(&self, p: &Principal, id: Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        load_draft(&mut tx, p.workspace_id, id).await
    }
    pub async fn validate_draft(&self, p: &Principal, id: Uuid) -> Result<Value> {
        can_write(p)?;
        for attempt in 0..MAX_ATTEMPTS {
            let outcome: Result<Value> = tokio::time::timeout(Duration::from_secs(6), async {
                let mut tx = self.write_tx().await?;
                live_authority(&mut tx, p, p.capability == Capability::Admin).await?;
                let draft = load_draft(&mut tx, p.workspace_id, id).await?;
                if draft["status"] != "draft" {
                    return Err(Error::Conflict(
                        "change set has already been applied".into(),
                    ));
                }
                let operations: Vec<Mutation> = decode(draft["operations"].clone())?;
                let before = preview::before(&mut tx, p.workspace_id, &operations).await?;
                let results =
                    execute(&mut tx, p, id, draft["reason"].as_str(), &operations, true).await?;
                let preview = preview::after(&mut tx, p.workspace_id, id, before).await?;
                tx.rollback().await?;
                Ok(json!({"valid":true,"change_set_id":id,"results":results,"preview":preview}))
            })
            .await
            .map_err(|_| Error::Overloaded)?;
            match outcome {
                Err(ref e) if retryable(e) && attempt + 1 < MAX_ATTEMPTS => {
                    tokio::time::sleep(retry_delay(attempt)).await
                }
                Err(ref e) if retryable(e) => return Err(Error::Overloaded),
                v => return v.map_err(normalize_db),
            }
        }
        Err(Error::Overloaded)
    }
    pub async fn apply_draft(&self, p: &Principal, id: Uuid, key: &str) -> Result<MutationResult> {
        can_write(p)?;
        check_key(key)?;
        let operation = "apply_draft";
        let payload = hash(id.as_bytes());
        for attempt in 0..MAX_ATTEMPTS {
            let outcome: Result<MutationResult> =
                tokio::time::timeout(Duration::from_secs(6), async {
                    let mut tx = self.write_tx().await?;
                    live_authority(&mut tx, p, p.capability == Capability::Admin).await?;
                    let policy_draft = load_draft(&mut tx, p.workspace_id, id).await?;
                    let policy_operations: Vec<Mutation> =
                        decode(policy_draft["operations"].clone())?;
                    if touches_knowledge(&mut tx, p, &policy_operations).await?
                        || requires_admin(&mut tx, p, &policy_operations).await?
                    {
                        admin(p)?;
                        live_authority(&mut tx, p, true).await?;
                    }
                    if let Some(v) = receipt(&mut tx, p, operation, key, &payload).await? {
                        return decode(v);
                    }
                    lock_receipt(&mut tx, p, operation, key).await?;
                    if let Some(v) = receipt(&mut tx, p, operation, key, &payload).await? {
                        return decode(v);
                    }
                    let draft = load_draft(&mut tx, p.workspace_id, id).await?;
                    if draft["status"] != "draft" {
                        return Err(Error::Conflict(
                            "change set has already been applied".into(),
                        ));
                    }
                    let operations: Vec<Mutation> = decode(draft["operations"].clone())?;
                    let results =
                        execute(&mut tx, p, id, draft["reason"].as_str(), &operations, false)
                            .await?;
                    let response = MutationResult {
                        change_set_id: id,
                        results,
                    };
                    check_response(&response)?;
                    save_result(&mut tx, id, &response).await?;
                    save_receipt(&mut tx, p, operation, key, &payload, encode(&response)?).await?;
                    tx.commit().await?;
                    Ok(response)
                })
                .await
                .map_err(|_| Error::Overloaded)?;
            match outcome {
                Err(ref e) if retryable(e) && attempt + 1 < MAX_ATTEMPTS => {
                    tokio::time::sleep(retry_delay(attempt)).await
                }
                Err(ref e) if retryable(e) => return Err(Error::Overloaded),
                v => return v.map_err(normalize_db),
            }
        }
        Err(Error::Overloaded)
    }
}
const DURABLE_FIELDS: &[&str] = &[
    "title",
    "body",
    "acceptance_criteria",
    "labels",
    "metadata",
    "content_type",
    "summary",
    "summary_of_revision",
    "valid_from",
    "valid_until",
    "review_at",
    "successor_id",
    "verification",
    "retired",
    "provenance",
    "last_verified_at",
    "last_verified_by",
    "retirement_reason",
    "task_state",
    "deleted",
];
fn validate_credential(actor: &str, token: &str) -> Result<()> {
    if actor.trim().is_empty() || actor.len() > 200 || token.len() < 32 || token.len() > 4096 {
        return Err(Error::Validation(
            "actor must contain 1..200 bytes and credential 32..4096 bytes".into(),
        ));
    }
    Ok(())
}
async fn live_authority(tx: &mut Tx<'_>, p: &Principal, need_admin: bool) -> Result<()> {
    let cap = sqlx::query_scalar::<_, String>(
        "SELECT capability FROM credentials WHERE workspace_id=$1 AND actor_id=$2",
    )
    .bind(p.workspace_id)
    .bind(&p.actor_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::Unauthorized)?;
    if (need_admin && cap != "admin") || (!need_admin && cap == "read") {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn ensure_admin(tx: &mut Tx<'_>, workspace: Uuid) -> Result<()> {
    let yes = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM credentials WHERE workspace_id=$1 AND capability='admin')",
    )
    .bind(workspace)
    .fetch_one(&mut **tx)
    .await?;
    if !yes {
        return Err(Error::Conflict(
            "workspace must retain at least one administrator".into(),
        ));
    }
    Ok(())
}
fn node_page(vs: Vec<Value>, n: i64) -> Result<Page<Node>> {
    let more = vs.len() > n as usize;
    let items: Vec<Node> = vs
        .into_iter()
        .take(n as usize)
        .map(decode)
        .collect::<Result<_>>()?;
    let next_cursor = if more {
        items.last().map(|v| v.id.to_string())
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}
async fn load_node(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid) -> Result<Node> {
    let v = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::NotFound)?;
    decode(v)
}
async fn load_node_write(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid) -> Result<Node> {
    let v=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(workspace).bind(id).fetch_optional(&mut **tx).await?.ok_or(Error::NotFound)?;
    decode(v)
}
async fn load_edge(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid) -> Result<Edge> {
    let v = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::NotFound)?;
    decode(v)
}
async fn load_revision(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid, revision: i64) -> Result<Value> {
    sqlx::query_scalar::<_, Value>(
        "SELECT snapshot FROM node_revisions WHERE workspace_id=$1 AND node_id=$2 AND revision=$3",
    )
    .bind(workspace)
    .bind(id)
    .bind(revision)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::NotFound)
}
async fn save_node(tx: &mut Tx<'_>, n: &Node, create: bool) -> Result<()> {
    // JSON is only a transport into explicitly typed PostgreSQL columns.
    let v = encode(n)?;
    if create {
        sqlx::query("INSERT INTO nodes (id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,labels,metadata,task_state,content_type,verification,retired,provenance,deleted,created_at,updated_at,created_by,last_verified_by,last_verified_at,retirement_reason,summary,summary_of_revision,valid_from,valid_until,review_at,successor_id) SELECT id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,labels,metadata,task_state,content_type,verification,retired,provenance,deleted,created_at,updated_at,created_by,last_verified_by,last_verified_at,retirement_reason,summary,summary_of_revision,valid_from,valid_until,review_at,successor_id FROM jsonb_populate_record(NULL::nodes,$1)").bind(v).execute(&mut **tx).await?;
    } else {
        sqlx::query("UPDATE nodes n SET version=r.version,content_revision=r.content_revision,title=r.title,body=r.body,acceptance_criteria=r.acceptance_criteria,labels=r.labels,metadata=r.metadata,task_state=r.task_state,content_type=r.content_type,verification=r.verification,retired=r.retired,provenance=r.provenance,deleted=r.deleted,updated_at=r.updated_at,last_verified_by=r.last_verified_by,last_verified_at=r.last_verified_at,retirement_reason=r.retirement_reason,summary=r.summary,summary_of_revision=r.summary_of_revision,valid_from=r.valid_from,valid_until=r.valid_until,review_at=r.review_at,successor_id=r.successor_id FROM jsonb_populate_record(NULL::nodes,$1) r WHERE n.id=r.id AND n.workspace_id=r.workspace_id").bind(v).execute(&mut **tx).await?;
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
async fn history_entry(
    tx: &mut Tx<'_>,
    p: &Principal,
    change: Uuid,
    nodes: Vec<Uuid>,
    category: &str,
    operation: &str,
    reason: Option<&str>,
    data: Value,
) -> Result<()> {
    if category == "edge" {
        let e: Edge = decode(data.clone())?;
        sqlx::query("INSERT INTO edge_versions(workspace_id,id,version,snapshot) SELECT workspace_id,id,version,to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=$2 AND version=$3 ON CONFLICT(workspace_id,id,version) DO NOTHING").bind(p.workspace_id).bind(e.id).bind(e.version).execute(&mut **tx).await?;
    }
    sqlx::query("INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(p.workspace_id).bind(nodes).bind(Uuid::new_v4()).bind(category).bind(&p.actor_id).bind(operation).bind(reason).bind(change).bind(data).execute(&mut **tx).await?;
    Ok(())
}
async fn revision(
    tx: &mut Tx<'_>,
    p: &Principal,
    change: Uuid,
    n: &Node,
    operation: &str,
    reason: Option<&str>,
) -> Result<()> {
    let snapshot = encode(n)?;
    let link_set = revision_links::capture_links(tx, p, n.id).await?;
    sqlx::query("INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id,link_set_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(p.workspace_id).bind(n.id).bind(n.content_revision).bind(&snapshot).bind(&p.actor_id).bind(operation).bind(reason).bind(change).bind(link_set).execute(&mut **tx).await?;
    history_entry(
        tx,
        p,
        change,
        vec![n.id],
        "revision",
        operation,
        reason,
        snapshot,
    )
    .await
}
fn expected(actual: i64, wanted: i64) -> Result<()> {
    if actual != wanted {
        Err(Error::Conflict(format!(
            "expected version {wanted}, current version {actual}"
        )))
    } else {
        Ok(())
    }
}
fn durable_changed(a: &Node, b: &Node) -> Result<bool> {
    let a = encode(a)?;
    let b = encode(b)?;
    Ok(DURABLE_FIELDS.iter().any(|f| a[f] != b[f]))
}
fn retirement_metadata_only(patch: &NodePatch) -> bool {
    patch.title.is_none()
        && patch.body.is_none()
        && patch.acceptance_criteria.is_none()
        && patch.labels.is_none()
        && patch.metadata.is_none()
        && patch.task_state.is_none()
        && patch.content_type.is_none()
        && patch.verification.is_none()
        && patch.provenance.is_none()
        && patch.valid_from.is_keep()
        && patch.valid_until.is_keep()
        && patch.review_at.is_keep()
        && patch.retired != Some(false)
}
fn patch_node(n: &mut Node, patch: &NodePatch) -> Result<()> {
    if n.kind == NodeKind::Knowledge
        && (patch.task_state.is_some() || patch.acceptance_criteria.is_some())
    {
        return Err(Error::Validation(
            "task fields cannot be set on knowledge".into(),
        ));
    }
    if let Some(v) = &patch.title {
        n.title = v.clone();
    }
    if let Some(v) = &patch.body {
        n.body = v.clone();
    }
    if let Some(v) = &patch.acceptance_criteria {
        n.acceptance_criteria = v.clone();
    }
    if let Some(v) = &patch.labels {
        n.labels = v.clone();
    }
    if let Some(v) = &patch.metadata {
        n.metadata = v.clone();
    }
    if let Some(v) = patch.task_state {
        n.task_state = Some(v);
    }
    if let Some(v) = patch.content_type {
        n.content_type = Some(v);
    }
    if patch.verification.is_some() {
        return Err(Error::Validation(
            "use verify_node to change verification".into(),
        ));
    }
    if let Some(v) = &patch.retirement_reason {
        n.retirement_reason = Some(v.clone());
    }
    if let Some(v) = patch.retired {
        n.retired = v;
    }
    if let Some(v) = &patch.provenance {
        n.provenance = v.clone();
    }
    patch.valid_from.apply_to(&mut n.valid_from);
    patch.valid_until.apply_to(&mut n.valid_until);
    patch.review_at.apply_to(&mut n.review_at);
    patch.successor_id.apply_to(&mut n.successor_id);
    Ok(())
}
async fn requires_admin(tx: &mut Tx<'_>, p: &Principal, ops: &[Mutation]) -> Result<bool> {
    for op in ops {
        match op {
            Mutation::VerifyKnowledge { .. } | Mutation::VerifyNode { .. } => return Ok(true),
            Mutation::UpdateNode { patch, .. }
                if patch.retired.is_some()
                    || patch.retirement_reason.is_some()
                    || !patch.successor_id.is_keep() =>
            {
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
                let created_kind = ops.iter().find_map(|op| match op {
                    Mutation::CreateNode {
                        id: Some(created_id),
                        kind,
                        ..
                    } if created_id == id => Some(*kind),
                    _ => None,
                });
                let kind = match created_kind {
                    Some(kind) => kind,
                    None => load_node(tx, p.workspace_id, *id).await?.kind,
                };
                if kind == NodeKind::Knowledge {
                    return Ok(true);
                }
            }
            _ => {}
        }
    }
    Ok(false)
}
async fn touches_knowledge(tx: &mut Tx<'_>, p: &Principal, ops: &[Mutation]) -> Result<bool> {
    let mut created = std::collections::HashMap::new();
    for op in ops {
        if let Mutation::CreateNode {
            id: Some(id), kind, ..
        } = op
        {
            created.insert(*id, *kind);
        }
    }
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
            Mutation::RemoveEdge { id, .. } => {
                let created_source = ops.iter().find_map(|op| match op {
                    Mutation::AddEdge {
                        id: Some(created_id),
                        source,
                        ..
                    } if created_id == id => Some(*source),
                    _ => None,
                });
                match created_source {
                    Some(source) => source,
                    None => load_edge(tx, p.workspace_id, *id).await?.source,
                }
            }
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
async fn checkpoint_nodes(
    tx: &mut Tx<'_>,
    p: &Principal,
    change: Uuid,
    ids: Vec<Uuid>,
    operation: &str,
    reason: Option<&str>,
) -> Result<()> {
    let mut ids = ids;
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        let mut n = load_node_write(tx, p.workspace_id, id).await?;
        knowledge_rules::reset_verification(&mut n);
        knowledge_rules::clear_summary(&mut n);
        n.version += 1;
        n.content_revision += 1;
        n.updated_at = server_time();
        save_node(tx, &n, false).await?;
        revision(tx, p, change, &n, operation, reason).await?;
    }
    Ok(())
}
fn relationship_nodes(e: &Edge) -> Vec<Uuid> {
    if matches!(e.edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
        vec![e.source, e.target]
    } else {
        vec![e.source]
    }
}
async fn execute(
    tx: &mut Tx<'_>,
    p: &Principal,
    change: Uuid,
    reason: Option<&str>,
    ops: &[Mutation],
    preview: bool,
) -> Result<Vec<Value>> {
    let mut edit_authority = p.clone();
    if preview {
        edit_authority.capability = Capability::Admin;
    }
    check_operations(ops)?;
    let mut results = Vec::with_capacity(ops.len());
    let mut deleted_nodes = vec![];
    for op in ops {
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
                    workspace_id: p.workspace_id,
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
                    created_by: p.actor_id.clone(),
                    last_verified_by: None,
                    last_verified_at: None,
                    retirement_reason: None,
                };
                knowledge_rules::validate_create(&n)?;
                save_node(tx, &n, true).await?;
                revision(tx, p, change, &n, "created", reason).await?;
                if let Some(state) = n.task_state {
                    history_entry(
                        tx,
                        p,
                        change,
                        vec![n.id],
                        "lifecycle",
                        "created",
                        reason,
                        json!({"from":null,"to":state,"version":n.version}),
                    )
                    .await?;
                }
                results.push(encode(&n)?);
            }
            Mutation::UpdateNode {
                id,
                expected_version,
                patch,
            } => {
                let before = load_node_write(tx, p.workspace_id, *id).await?;
                expected(before.version, *expected_version)?;
                if before.deleted {
                    return Err(Error::Conflict(
                        "record is deleted; restore it before editing".into(),
                    ));
                }
                knowledge_rules::authorize_edit(&edit_authority, &before)?;
                if before.retired && !retirement_metadata_only(patch) {
                    return Err(Error::Forbidden);
                }
                if patch.retired.is_some()
                    || patch.retirement_reason.is_some()
                    || !patch.successor_id.is_keep()
                {
                    edit_authority.require_admin()?;
                    if !preview {
                        live_authority(tx, p, true).await?;
                    }
                }
                if patch.task_state == Some(TaskState::Completed)
                    && before.task_state != Some(TaskState::Completed)
                {
                    compat_graph::validate_completion(tx, p, *id).await?;
                }
                let mut n = before.clone();
                patch_node(&mut n, patch)?;
                knowledge_rules::validate_retirement(&n)?;
                knowledge_rules::validate_temporal(&n)?;
                if let Some(successor) = n.successor_id {
                    let successor = load_node(tx, p.workspace_id, successor).await?;
                    if successor.deleted {
                        return Err(Error::Conflict("successor cannot be deleted".into()));
                    }
                }
                knowledge_rules::validate_sources(&n.provenance)?;
                let content = durable_changed(&before, &n)?;
                if content {
                    knowledge_rules::reset_verification(&mut n);
                    knowledge_rules::clear_summary(&mut n);
                }
                let lifecycle = before.task_state != n.task_state;
                if content || lifecycle {
                    n.version += 1;
                    n.updated_at = server_time();
                    if content {
                        n.content_revision += 1;
                    }
                    save_node(tx, &n, false).await?;
                    if content {
                        revision(tx, p, change, &n, "updated", reason).await?;
                    }
                    if lifecycle {
                        history_entry(
                            tx,
                            p,
                            change,
                            vec![n.id],
                            "lifecycle",
                            "transitioned",
                            reason,
                            json!({"from":before.task_state,"to":n.task_state,"version":n.version}),
                        )
                        .await?;
                    }
                }
                results.push(encode(&n)?);
            }
            Mutation::DeleteNode {
                id,
                expected_version,
            } => {
                let mut n = load_node_write(tx, p.workspace_id, *id).await?;
                expected(n.version, *expected_version)?;
                knowledge_rules::authorize_edit(&edit_authority, &n)?;
                if !n.deleted {
                    knowledge_rules::reset_verification(&mut n);
                    knowledge_rules::clear_summary(&mut n);
                    n.deleted = true;
                    n.version += 1;
                    n.content_revision += 1;
                    n.updated_at = server_time();
                    save_node(tx, &n, false).await?;
                    revision(tx, p, change, &n, "deleted", reason).await?;
                    history_entry(
                        tx,
                        p,
                        change,
                        vec![n.id],
                        "lifecycle",
                        "deleted",
                        reason,
                        json!({"deleted":true,"version":n.version}),
                    )
                    .await?;
                    deleted_nodes.push(n.id);
                }
                results.push(encode(&n)?);
            }
            Mutation::RestoreNode {
                id,
                expected_version,
                revision: rev,
                include_lifecycle,
            } => {
                let n = restore::restore_node(
                    tx,
                    p,
                    &edit_authority,
                    change,
                    *id,
                    *expected_version,
                    *rev,
                    *include_lifecycle,
                    reason,
                    preview,
                )
                .await?;
                results.push(encode(&n)?);
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
                let a = load_node_write(tx, p.workspace_id, *source).await?;
                knowledge_rules::authorize_edit(&edit_authority, &a)?;
                if a.retired {
                    return Err(Error::Forbidden);
                }
                if matches!(edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
                    let existing=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND source=$2 AND target=$3 AND edge_type=$4 AND NOT deleted").bind(p.workspace_id).bind(source).bind(target).bind(tag(edge_type)?).fetch_optional(&mut **tx).await?;
                    if let Some(existing) = existing {
                        results.push(existing);
                        continue;
                    }
                }
                compat_graph::validate_edge(tx, p, *source, *target, *edge_type).await?;
                let b = load_node(tx, p.workspace_id, *target).await?;
                if matches!(edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
                    knowledge_rules::authorize_edit(&edit_authority, &b)?;
                    if b.retired {
                        return Err(Error::Forbidden);
                    }
                }
                if a.deleted || b.deleted {
                    return Err(Error::Conflict(
                        "relationships require live endpoints".into(),
                    ));
                }
                if matches!(edge_type, EdgeType::Blocks | EdgeType::ParentChild)
                    && (a.kind != NodeKind::Task || b.kind != NodeKind::Task)
                {
                    return Err(Error::Validation(
                        "structural relationships require task endpoints".into(),
                    ));
                }
                for (node, rev) in [(*source, *source_revision), (*target, *target_revision)] {
                    if let Some(r) = rev {
                        load_revision(tx, p.workspace_id, node, r)
                            .await
                            .map_err(|e| match e {
                                Error::NotFound => {
                                    Error::Validation("pinned revision is unavailable".into())
                                }
                                e => e,
                            })?;
                    }
                }
                let e = Edge {
                    id: id.unwrap_or_else(Uuid::new_v4),
                    workspace_id: p.workspace_id,
                    source: *source,
                    target: *target,
                    edge_type: *edge_type,
                    source_revision: *source_revision,
                    target_revision: *target_revision,
                    version: 1,
                    deleted: false,
                    created_at: server_time(),
                    description: description.clone(),
                };
                sqlx::query("INSERT INTO edges(id,workspace_id,source,target,edge_type,source_revision,target_revision,version,deleted,created_at,description) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)").bind(e.id).bind(e.workspace_id).bind(e.source).bind(e.target).bind(tag(&e.edge_type)?).bind(e.source_revision).bind(e.target_revision).bind(e.version).bind(e.deleted).bind(e.created_at).bind(&e.description).execute(&mut **tx).await?;
                history_entry(
                    tx,
                    p,
                    change,
                    vec![*source, *target],
                    "edge",
                    "added",
                    reason,
                    encode(&e)?,
                )
                .await?;
                checkpoint_nodes(
                    tx,
                    p,
                    change,
                    relationship_nodes(&e),
                    "links_changed",
                    reason,
                )
                .await?;
                results.push(encode(&e)?);
            }
            Mutation::SetParent {
                id,
                expected_version,
                parent_id,
            } => {
                results.push(
                    compat_graph::set_parent(
                        tx,
                        p,
                        &edit_authority,
                        change,
                        *id,
                        *expected_version,
                        *parent_id,
                        reason,
                    )
                    .await?,
                );
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
                if !preview {
                    live_authority(tx, p, true).await?;
                }
                let mut n = load_node_write(tx, p.workspace_id, *id).await?;
                expected(n.version, *expected_version)?;
                knowledge_rules::verify(&mut n, &edit_authority, *verification, evidence, reason)?;
                knowledge_rules::clear_summary(&mut n);
                n.version += 1;
                n.content_revision += 1;
                n.updated_at = server_time();
                save_node(tx, &n, false).await?;
                revision(tx, p, change, &n, "verified", reason).await?;
                results.push(encode(&n)?);
            }
            Mutation::SetSummary {
                id,
                expected_version,
                source_revision,
                summary,
            } => {
                let mut n = load_node_write(tx, p.workspace_id, *id).await?;
                expected(n.version, *expected_version)?;
                knowledge_rules::authorize_edit(&edit_authority, &n)?;
                if n.deleted || n.retired {
                    return Err(Error::Conflict(
                        "summary requires a live nonretired node".into(),
                    ));
                }
                // Repeating an identical derived value does not manufacture history.
                if n.summary == *summary
                    && (summary.is_none() || n.summary_of_revision == Some(*source_revision))
                {
                    results.push(encode(&n)?);
                    continue;
                }
                knowledge_rules::set_summary(&mut n, *source_revision, summary.as_deref())?;
                knowledge_rules::reset_verification(&mut n);
                n.version += 1;
                n.content_revision += 1;
                n.updated_at = server_time();
                knowledge_rules::validate_summary(&n)?;
                save_node(tx, &n, false).await?;
                revision(tx, p, change, &n, "summary_changed", reason).await?;
                results.push(encode(&n)?);
            }
            Mutation::RemoveEdge {
                id,
                expected_version,
            } => {
                let mut e = load_edge(tx, p.workspace_id, *id).await?;
                expected(e.version, *expected_version)?;
                let source_node = load_node_write(tx, p.workspace_id, e.source).await?;
                knowledge_rules::authorize_edit(&edit_authority, &source_node)?;
                if source_node.retired {
                    return Err(Error::Forbidden);
                }
                if !e.deleted {
                    if matches!(e.edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
                        let target = load_node_write(tx, p.workspace_id, e.target).await?;
                        knowledge_rules::authorize_edit(&edit_authority, &target)?;
                    }
                    e.deleted = true;
                    e.version += 1;
                    sqlx::query(
                        "UPDATE edges SET deleted=true,version=$3 WHERE workspace_id=$1 AND id=$2",
                    )
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
                    checkpoint_nodes(
                        tx,
                        p,
                        change,
                        relationship_nodes(&e),
                        "links_changed",
                        reason,
                    )
                    .await?;
                }
                results.push(encode(&e)?);
            }
        }
    }
    if !deleted_nodes.is_empty() {
        let referenced=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.id=e.source OR n.id=e.target WHERE e.workspace_id=$1 AND NOT e.deleted AND n.deleted AND n.id=ANY($2))").bind(p.workspace_id).bind(deleted_nodes).fetch_one(&mut **tx).await?;
        if referenced {
            return Err(Error::Conflict(
                "remove active relationships before deleting their endpoints".into(),
            ));
        }
    }
    successors::validate(tx, p, ops).await?;
    Ok(results)
}
async fn lock_receipt(tx: &mut Tx<'_>, p: &Principal, operation: &str, key: &str) -> Result<()> {
    // Reserving the unique receipt is also a transaction-scoped key lock. An
    // overlapping SERIALIZABLE contender retries with a fresh snapshot after commit.
    sqlx::query("INSERT INTO idempotency_receipts(workspace_id,actor_id,operation,key,payload_hash,result) VALUES($1,$2,$3,$4,'','null') ON CONFLICT(workspace_id,actor_id,operation,key) DO UPDATE SET key=EXCLUDED.key").bind(p.workspace_id).bind(&p.actor_id).bind(operation).bind(key).execute(&mut **tx).await?;
    Ok(())
}
async fn receipt(
    tx: &mut Tx<'_>,
    p: &Principal,
    operation: &str,
    key: &str,
    payload: &str,
) -> Result<Option<Value>> {
    let row=sqlx::query("SELECT payload_hash,result FROM idempotency_receipts WHERE workspace_id=$1 AND actor_id=$2 AND operation=$3 AND key=$4").bind(p.workspace_id).bind(&p.actor_id).bind(operation).bind(key).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let old: String = row.get("payload_hash");
    if old.is_empty() {
        return Ok(None);
    }
    if old != payload {
        return Err(Error::Conflict(
            "idempotency key was used with a different payload".into(),
        ));
    }
    Ok(Some(row.get("result")))
}
async fn save_receipt(
    tx: &mut Tx<'_>,
    p: &Principal,
    operation: &str,
    key: &str,
    payload: &str,
    result: Value,
) -> Result<()> {
    sqlx::query("UPDATE idempotency_receipts SET payload_hash=$5,result=$6 WHERE workspace_id=$1 AND actor_id=$2 AND operation=$3 AND key=$4").bind(p.workspace_id).bind(&p.actor_id).bind(operation).bind(key).bind(payload).bind(result).execute(&mut **tx).await?;
    Ok(())
}
async fn insert_changeset(
    tx: &mut Tx<'_>,
    p: &Principal,
    id: Uuid,
    name: Option<&str>,
    reason: Option<&str>,
    ops: &[Mutation],
    status: &str,
) -> Result<()> {
    if reason.is_some_and(|v| v.len() > 4096) {
        return Err(Error::Validation("reason exceeds 4096 bytes".into()));
    }
    sqlx::query("INSERT INTO change_sets(id,workspace_id,actor_id,name,reason,operations,status) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id).bind(p.workspace_id).bind(&p.actor_id).bind(name).bind(reason).bind(encode(&ops)?).bind(status).execute(&mut **tx).await?;
    Ok(())
}
async fn save_result(tx: &mut Tx<'_>, id: Uuid, result: &MutationResult) -> Result<()> {
    sqlx::query("UPDATE change_sets SET status='applied',result=$2,applied_at=clock_timestamp() WHERE id=$1").bind(id).bind(encode(result)?).execute(&mut **tx).await?;
    Ok(())
}
async fn load_draft(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid) -> Result<Value> {
    sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(c) FROM change_sets c WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::NotFound)
}
fn assign_ids(ops: &[Mutation]) -> Result<Vec<Mutation>> {
    let mut value = encode(&ops)?;
    for op in value
        .as_array_mut()
        .ok_or_else(|| Error::Internal("invalid operations".into()))?
    {
        if matches!(op["op"].as_str(), Some("create_node" | "add_edge")) && op["id"].is_null() {
            op["id"] = json!(Uuid::new_v4());
        }
    }
    decode(value)
}
fn check_response<T: Serialize>(response: &T) -> Result<()> {
    if serde_json::to_vec(response)
        .map_err(|e| Error::Internal(e.to_string()))?
        .len()
        > 2_097_152
    {
        return Err(Error::Validation(
            "mutation result exceeds 2 MiB; use a smaller batch".into(),
        ));
    }
    Ok(())
}

async fn retry_write<T, F, Fut>(mut operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    for attempt in 0..MAX_ATTEMPTS {
        let outcome = tokio::time::timeout(Duration::from_secs(6), operation())
            .await
            .map_err(|_| Error::Overloaded)?;
        match outcome {
            Err(ref e) if retryable(e) && attempt + 1 < MAX_ATTEMPTS => {
                tokio::time::sleep(retry_delay(attempt)).await
            }
            Err(ref e) if retryable(e) => return Err(Error::Overloaded),
            v => return v.map_err(normalize_db),
        }
    }
    Err(Error::Overloaded)
}

fn retry_delay(attempt: usize) -> Duration {
    let jitter = u64::from(Uuid::new_v4().as_bytes()[0]) % 17;
    Duration::from_millis((16_u64 << attempt.min(4)) + jitter)
}
