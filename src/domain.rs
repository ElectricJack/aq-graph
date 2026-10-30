//! Shared wire types and validation. No dependency on Agent Q runtime concepts.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid request: {0}")]
    Validation(String),
    #[error("record not found")]
    NotFound,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("authentication required")]
    Unauthorized,
    #[error("insufficient capability")]
    Forbidden,
    #[error("service capacity exceeded; retry later")]
    Overloaded,
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("internal operation failed")]
    Internal(String),
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $wire)] $variant),+ }
        impl $name {
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $wire),+ } }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
        impl std::str::FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                match s { $($wire => Ok(Self::$variant)),+, _ => Err(Error::Validation(format!("invalid {}: {s}", stringify!($name)))) }
            }
        }
    }
}

string_enum!(Capability { Read => "read", Write => "write", Admin => "admin" });
string_enum!(NodeKind { Task => "task", Knowledge => "knowledge" });
string_enum!(TaskState { Draft => "draft", Active => "active", Completed => "completed", Canceled => "canceled" });
string_enum!(KnowledgeType { Fact => "fact", Decision => "decision", Policy => "policy", Procedure => "procedure", Incident => "incident", Reference => "reference", Note => "note" });
string_enum!(Verification { Unverified => "unverified", Verified => "verified", Disputed => "disputed" });
string_enum!(EdgeType { Blocks => "blocks", ParentChild => "parent-child", References => "references", MotivatedBy => "motivated_by", Produces => "produces", Supports => "supports", Contradicts => "contradicts", Supersedes => "supersedes" });

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub actor_id: String,
    pub workspace_id: Uuid,
    pub capability: Capability,
}

impl Principal {
    pub fn require_write(&self) -> Result<()> {
        match self.capability {
            Capability::Read => Err(Error::Forbidden),
            _ => Ok(()),
        }
    }
    pub fn require_admin(&self) -> Result<()> {
        if self.capability == Capability::Admin {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub kind: NodeKind,
    pub version: i64,
    pub content_revision: i64,
    pub title: String,
    pub body: String,
    pub acceptance_criteria: String,
    pub labels: Vec<String>,
    pub metadata: Value,
    pub task_state: Option<TaskState>,
    pub knowledge_type: Option<KnowledgeType>,
    pub verification: Option<Verification>,
    pub retired: bool,
    pub provenance: Vec<Value>,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub source: Uuid,
    pub target: Uuid,
    pub edge_type: EdgeType,
    pub source_revision: Option<i64>,
    pub target_revision: Option<i64>,
    pub version: i64,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub acceptance_criteria: Option<String>,
    pub labels: Option<Vec<String>>,
    pub metadata: Option<Value>,
    pub task_state: Option<TaskState>,
    pub knowledge_type: Option<KnowledgeType>,
    pub verification: Option<Verification>,
    pub retired: Option<bool>,
    pub provenance: Option<Vec<Value>>,
}

fn empty_object() -> Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mutation {
    CreateNode {
        #[serde(default)]
        id: Option<Uuid>,
        kind: NodeKind,
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        acceptance_criteria: String,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default = "empty_object")]
        metadata: Value,
        #[serde(default)]
        task_state: Option<TaskState>,
        #[serde(default)]
        knowledge_type: Option<KnowledgeType>,
        #[serde(default)]
        verification: Option<Verification>,
        #[serde(default)]
        provenance: Vec<Value>,
    },
    UpdateNode {
        id: Uuid,
        expected_version: i64,
        patch: NodePatch,
    },
    DeleteNode {
        id: Uuid,
        expected_version: i64,
    },
    RestoreNode {
        id: Uuid,
        expected_version: i64,
        revision: i64,
    },
    AddEdge {
        #[serde(default)]
        id: Option<Uuid>,
        source: Uuid,
        target: Uuid,
        edge_type: EdgeType,
        #[serde(default)]
        source_revision: Option<i64>,
        #[serde(default)]
        target_revision: Option<i64>,
    },
    RemoveEdge {
        id: Uuid,
        expected_version: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationRequest {
    pub idempotency_key: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub operations: Vec<Mutation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftRequest {
    pub name: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub operations: Vec<Mutation>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationResult {
    pub change_set_id: Uuid,
    pub results: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

pub fn default_limit() -> u32 {
    50
}
pub fn default_depth() -> u32 {
    1
}
pub fn default_bytes() -> u32 {
    262_144
}
pub fn default_direction() -> String {
    "both".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub cursor: Option<String>,
}
impl Default for ListQuery {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            cursor: None,
        }
    }
}
impl ListQuery {
    pub fn validate(&self) -> Result<()> {
        if self.limit == 0 || self.limit > 200 {
            return Err(Error::Validation("limit must be between 1 and 200".into()));
        }
        if self.cursor.as_ref().is_some_and(|c| c.len() > 256) {
            return Err(Error::Validation("cursor too long".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextQuery {
    #[serde(default = "default_depth")]
    pub depth: u32,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default = "default_bytes")]
    pub max_bytes: u32,
    #[serde(default = "default_direction")]
    pub direction: String,
}
impl Default for ContextQuery {
    fn default() -> Self {
        Self {
            depth: 1,
            limit: 50,
            max_bytes: default_bytes(),
            direction: default_direction(),
        }
    }
}
impl ContextQuery {
    pub fn validate(&self) -> Result<()> {
        if self.depth > 5
            || self.limit == 0
            || self.limit > 200
            || !(1024..=1_048_576).contains(&self.max_bytes)
        {
            return Err(Error::Validation(
                "context requires depth 0..5, limit 1..200, max_bytes 1024..1048576".into(),
            ));
        }
        if !["both", "outgoing", "incoming"].contains(&self.direction.as_str()) {
            return Err(Error::Validation(
                "direction must be both, outgoing, or incoming".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub node: Node,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub pinned_revisions: Vec<Value>,
    pub truncated: bool,
}

impl MutationRequest {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.idempotency_key)?;
        validate_operations(&self.operations)?;
        if self.reason.as_ref().is_some_and(|r| r.len() > 4096) {
            return Err(Error::Validation("reason exceeds 4096 bytes".into()));
        }
        Ok(())
    }
}

pub fn validate_key(key: &str) -> Result<()> {
    if key.trim().is_empty() || key.len() > 128 {
        return Err(Error::Validation(
            "idempotency_key must contain 1..128 bytes".into(),
        ));
    }
    Ok(())
}

pub fn validate_operations(ops: &[Mutation]) -> Result<()> {
    if ops.is_empty() || ops.len() > 100 {
        return Err(Error::Validation(
            "a change set must have 1..100 operations".into(),
        ));
    }
    for op in ops {
        match op {
            Mutation::CreateNode {
                kind,
                title,
                body,
                acceptance_criteria,
                labels,
                metadata,
                task_state,
                knowledge_type,
                verification,
                provenance,
                ..
            } => {
                validate_content(
                    title,
                    body,
                    acceptance_criteria,
                    labels,
                    metadata,
                    provenance,
                )?;
                match kind {
                    NodeKind::Task
                        if knowledge_type.is_some()
                            || verification.is_some()
                            || !provenance.is_empty() =>
                    {
                        return Err(Error::Validation(
                            "knowledge fields cannot be set on a task".into(),
                        ));
                    }
                    NodeKind::Knowledge
                        if task_state.is_some() || !acceptance_criteria.is_empty() =>
                    {
                        return Err(Error::Validation(
                            "task fields cannot be set on knowledge".into(),
                        ));
                    }
                    _ => {}
                }
            }
            Mutation::UpdateNode {
                expected_version,
                patch,
                ..
            } => {
                validate_version(*expected_version)?;
                if let Some(title) = &patch.title {
                    validate_title(title)?;
                }
                if patch.body.as_ref().is_some_and(|s| s.len() > 65_536)
                    || patch
                        .acceptance_criteria
                        .as_ref()
                        .is_some_and(|s| s.len() > 16_384)
                {
                    return Err(Error::Validation("content exceeds maximum size".into()));
                }
                if let Some(labels) = &patch.labels {
                    validate_labels(labels)?;
                }
                if let Some(metadata) = &patch.metadata {
                    validate_metadata(metadata)?;
                }
                if let Some(provenance) = &patch.provenance {
                    validate_provenance(provenance)?;
                }
            }
            Mutation::DeleteNode {
                expected_version, ..
            }
            | Mutation::RemoveEdge {
                expected_version, ..
            } => validate_version(*expected_version)?,
            Mutation::RestoreNode {
                expected_version,
                revision,
                ..
            } => {
                validate_version(*expected_version)?;
                validate_version(*revision)?;
            }
            Mutation::AddEdge {
                source,
                target,
                source_revision,
                target_revision,
                edge_type,
                ..
            } => {
                if source == target {
                    return Err(Error::Validation("self links are not allowed".into()));
                }
                for rev in [source_revision, target_revision].into_iter().flatten() {
                    validate_version(*rev)?;
                }
                if matches!(edge_type, EdgeType::Blocks | EdgeType::ParentChild)
                    && (source_revision.is_some() || target_revision.is_some())
                {
                    return Err(Error::Validation(
                        "structural edges must reference current task state".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_version(version: i64) -> Result<()> {
    if version < 1 {
        Err(Error::Validation(
            "version and revision must be positive".into(),
        ))
    } else {
        Ok(())
    }
}
fn validate_title(title: &str) -> Result<()> {
    if title.trim().is_empty() || title.len() > 512 {
        Err(Error::Validation("title must contain 1..512 bytes".into()))
    } else {
        Ok(())
    }
}
fn validate_labels(labels: &[String]) -> Result<()> {
    if labels.len() > 64 || labels.iter().any(|l| l.trim().is_empty() || l.len() > 128) {
        Err(Error::Validation(
            "at most 64 labels of 1..128 bytes are allowed".into(),
        ))
    } else {
        Ok(())
    }
}
fn validate_metadata(metadata: &Value) -> Result<()> {
    let map = metadata
        .as_object()
        .ok_or_else(|| Error::Validation("metadata must be an object".into()))?;
    if map.keys().any(|k| !k.contains(':') || k.len() > 128) || metadata.to_string().len() > 16_384
    {
        return Err(Error::Validation(
            "metadata requires namespaced keys (e.g. aq:priority) and at most 16384 bytes".into(),
        ));
    }
    Ok(())
}
fn validate_provenance(provenance: &[Value]) -> Result<()> {
    if provenance.len() > 64
        || serde_json::to_vec(provenance)
            .map_err(|e| Error::Internal(e.to_string()))?
            .len()
            > 16_384
        || provenance.iter().any(|v| !v.is_object())
    {
        return Err(Error::Validation(
            "provenance must have at most 64 objects and 16384 bytes".into(),
        ));
    }
    Ok(())
}
pub fn validate_content(
    title: &str,
    body: &str,
    acceptance: &str,
    labels: &[String],
    metadata: &Value,
    provenance: &[Value],
) -> Result<()> {
    validate_title(title)?;
    if body.len() > 65_536 || acceptance.len() > 16_384 {
        return Err(Error::Validation("content exceeds maximum size".into()));
    }
    validate_labels(labels)?;
    validate_metadata(metadata)?;
    validate_provenance(provenance)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_unknown_fields_and_invalid_batches() {
        let v = serde_json::json!({"op":"create_node","kind":"task","title":"hello","actor_id":"spoof"});
        assert!(serde_json::from_value::<Mutation>(v).is_err());
        assert!(validate_operations(&[]).is_err());
        assert!(validate_key(" ").is_err());
    }
    #[test]
    fn enforce_graph_limits_and_namespaces() {
        assert!(
            ContextQuery {
                depth: 6,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(validate_metadata(&serde_json::json!({"priority":2})).is_err());
        assert!(validate_metadata(&serde_json::json!({"aq:priority":2})).is_ok());
    }
}
