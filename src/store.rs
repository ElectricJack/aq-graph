//! PostgreSQL authority for current graph state and selective immutable history.
use crate::domain::*;
use crate::knowledge_rules;
mod compat_graph;
mod context;
mod journal;
use journal::{ChangeSet, HistoryEvent, Journal};
mod credentials;
mod patch;
mod policy;
use credentials::live_authority;
use patch::patch_node;
use policy::{requires_admin, touches_knowledge};
mod drafts;
mod receipts;
use receipts::Receipt;
mod mutations;
use mutations::MutationBatch;
mod preview;
mod queries;
mod restore;
mod revision_links;
mod successors;
mod work_list;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;
pub(crate) use work_list::{TaskList, TaskListQuery};

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
                    // Bounded OLTP queries must not pay per-execution JIT compilation
                    // for high estimated cardinalities in generic prepared plans.
                    sqlx::query("SET jit=off").execute(&mut *conn).await?;
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
        fetch_record(&self.pool, NODE_BY_ID, p.workspace_id, id).await
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
        self.ready_with_first_ready(p, q, false).await
    }

    pub async fn ready_with_first_ready(
        &self,
        p: &Principal,
        q: ListQuery,
        include_first_ready: bool,
    ) -> Result<Page<Node>> {
        let n = limit(&q)?;
        let after = cursor(&q)?;
        self.ready_page(
            p,
            after.unwrap_or(Uuid::nil()),
            Uuid::from_u128(u128::MAX),
            after,
            n,
            include_first_ready,
        )
        .await
    }

    /// Check the same readiness rules as a page at one exact task identity.
    pub async fn ready_task(&self, p: &Principal, task: Uuid) -> Result<Page<Node>> {
        self.ready_page(p, task, task, None, 1, false).await
    }

    async fn ready_page(
        &self,
        p: &Principal,
        first: Uuid,
        last: Uuid,
        after: Option<Uuid>,
        n: i64,
        include_first_ready: bool,
    ) -> Result<Page<Node>> {
        // Schema 0004 and every structural mutation enforce at most three levels.
        // Two indexed ancestor joins suffice; a per-candidate recursive CTE can
        // cause repeated full node scans and JIT overhead even on small graphs.
        // A future hierarchy-depth increase must also extend this predicate.
        // The transactional projection supplies an ordered, narrow leaf index.
        // Keep the lateral barrier so bodies are fetched only after the guards.
        let vs=sqlx::query_scalar::<_,Value>("WITH candidates AS NOT MATERIALIZED (
SELECT node_id AS id FROM node_ready_projection
WHERE workspace_id=$1 AND eligible AND live_child_count=0
AND node_id >= $2 AND node_id <= $3
AND ($4::uuid IS NULL OR node_id>$4)
)
SELECT (to_jsonb(n)-'search_document') || jsonb_build_object('first_ready_at', f.first_ready_at, 'first_ready_work_class', f.work_class) FROM candidates c
CROSS JOIN LATERAL (SELECT n.* FROM nodes n WHERE n.workspace_id=$1 AND n.id=c.id AND NOT n.retired
AND NOT EXISTS (
 SELECT 1 FROM edges pe JOIN nodes parent ON parent.id=pe.source AND parent.workspace_id=pe.workspace_id
 LEFT JOIN edges ge ON ge.workspace_id=pe.workspace_id AND ge.target=parent.id AND ge.edge_type='parent-child' AND NOT ge.deleted
 LEFT JOIN nodes grandparent ON grandparent.id=ge.source AND grandparent.workspace_id=ge.workspace_id
 WHERE pe.workspace_id=n.workspace_id AND pe.target=n.id AND pe.edge_type='parent-child' AND NOT pe.deleted
 AND (parent.deleted OR parent.retired OR parent.task_state='draft' OR grandparent.deleted OR grandparent.retired OR grandparent.task_state='draft'))
AND NOT EXISTS (SELECT 1 FROM edges e JOIN nodes prerequisite ON prerequisite.id=e.source WHERE e.workspace_id=n.workspace_id AND e.target=n.id AND e.edge_type='blocks' AND NOT e.deleted AND (prerequisite.deleted OR prerequisite.task_state IS DISTINCT FROM 'completed')) LIMIT 1) n LEFT JOIN task_first_ready f ON $6 AND f.workspace_id=$1 AND f.node_id=c.id ORDER BY c.id LIMIT $5").bind(p.workspace_id).bind(first).bind(last).bind(after).bind(n+1).bind(include_first_ready).fetch_all(&self.pool).await?;
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
        // Isolate membership selection from cursor/limit planning. Generic plans
        // otherwise scan most workspace history in sequence order, or intersect
        // the tiny GIN match with a bitmap of every sequence after the cursor.
        let mut items=sqlx::query_scalar::<_,Value>("WITH matching AS MATERIALIZED (SELECT sequence FROM history_entries WHERE workspace_id=$1 AND node_ids @> ARRAY[$2]::uuid[]), selected AS MATERIALIZED (SELECT sequence FROM matching WHERE sequence>$3 ORDER BY sequence LIMIT $4) SELECT to_jsonb(h)-'workspace_id'-'node_ids' FROM selected s JOIN history_entries h ON h.sequence=s.sequence ORDER BY s.sequence").bind(p.workspace_id).bind(id).bind(after).bind(n+1).fetch_all(&self.pool).await?;
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
        p.require_write()?;
        let payload = mutation_fingerprint(&req)?;
        retry_write(|| self.mutate_once(p, &req, &payload)).await
    }

    /// Look up a committed mutation without reserving its key or executing it.
    /// An absent receipt is a snapshot and does not rule out an in-flight write.
    pub async fn mutation_receipt(
        &self,
        p: &Principal,
        req: MutationRequest,
    ) -> Result<Option<MutationResult>> {
        p.require_write()?;
        let payload = mutation_fingerprint(&req)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        authorize_mutation(&mut tx, p, &req.operations).await?;
        let receipt = Receipt {
            principal: p,
            operation: "mutate",
            key: &req.idempotency_key,
            payload: &payload,
        };
        let result = (receipt.load(&mut tx).await?).map(decode).transpose()?;
        tx.commit().await?;
        Ok(result)
    }

    async fn mutate_once(
        &self,
        p: &Principal,
        req: &MutationRequest,
        payload: &str,
    ) -> Result<MutationResult> {
        let mut tx = self.mutation_tx(&req.operations).await?;
        authorize_mutation(&mut tx, p, &req.operations).await?;
        let receipt = Receipt {
            principal: p,
            operation: "mutate",
            key: &req.idempotency_key,
            payload,
        };
        if let Some(v) = receipt.load_or_reserve(&mut tx).await? {
            return decode(v);
        }
        let id = Uuid::new_v4();
        Journal {
            principal: p,
            change: id,
            reason: req.reason.as_deref(),
        }
        .insert(
            &mut tx,
            ChangeSet {
                name: None,
                operations: &req.operations,
                status: "applied",
            },
        )
        .await?;
        let results = MutationBatch::new(p, id, req.reason.as_deref(), false)
            .run(&mut tx, &req.operations)
            .await?;
        let response = MutationResult {
            change_set_id: id,
            results,
        };
        check_response(&response)?;
        save_result(&mut tx, id, &response).await?;
        receipt.save(&mut tx, encode(&response)?).await?;
        tx.commit().await?;
        Ok(response)
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
fn mutation_fingerprint(req: &MutationRequest) -> Result<String> {
    req.validate()?;
    check_operations(&req.operations)?;
    let payload = serde_json::to_vec(req).map_err(|e| Error::Validation(e.to_string()))?;
    Ok(hash(&payload))
}
async fn authorize_mutation(tx: &mut Tx<'_>, p: &Principal, ops: &[Mutation]) -> Result<()> {
    live_authority(tx, p, p.capability == Capability::Admin).await?;
    if requires_admin(tx, p, ops).await? {
        p.require_admin()?;
        live_authority(tx, p, true).await?;
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
const NODE_BY_ID: &str =
    "SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND id=$2";
async fn fetch_record<'a, T: DeserializeOwned>(
    db: impl sqlx::Executor<'a, Database = Postgres>,
    sql: &'static str,
    workspace: Uuid,
    id: Uuid,
) -> Result<T> {
    let record = sqlx::query_scalar::<_, Value>(sql)
        .bind(workspace)
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(Error::NotFound)?;
    decode(record)
}
async fn load_node(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid) -> Result<Node> {
    fetch_record(&mut **tx, NODE_BY_ID, workspace, id).await
}
async fn load_node_write(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid) -> Result<Node> {
    fetch_record(&mut **tx, "SELECT to_jsonb(n)-'search_document' FROM nodes n WHERE workspace_id=$1 AND id=$2 FOR UPDATE", workspace, id).await
}
async fn load_edge(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid) -> Result<Edge> {
    fetch_record(
        &mut **tx,
        "SELECT to_jsonb(e) FROM edges e WHERE workspace_id=$1 AND id=$2",
        workspace,
        id,
    )
    .await
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

fn relationship_nodes(e: &Edge) -> Vec<Uuid> {
    if matches!(e.edge_type, EdgeType::Blocks | EdgeType::ParentChild) {
        vec![e.source, e.target]
    } else {
        vec![e.source]
    }
}

async fn save_result(tx: &mut Tx<'_>, id: Uuid, result: &MutationResult) -> Result<()> {
    sqlx::query("UPDATE change_sets SET status='applied',result=$2,applied_at=clock_timestamp() WHERE id=$1").bind(id).bind(encode(result)?).execute(&mut **tx).await?;
    Ok(())
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
