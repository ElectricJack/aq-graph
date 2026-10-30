//! Shared wire types and validation. No dependency on Agent Q runtime concepts.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Error>;

/// PostgreSQL timestamptz precision, shared by mutation responses and snapshots.
pub(crate) fn server_time() -> DateTime<Utc> {
    chrono::SubsecRound::trunc_subsecs(Utc::now(), 6)
}

/// Explicit patch semantics: omitted, cleared with JSON null, or set to a value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FieldPatch<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}
impl<T> FieldPatch<T> {
    pub fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
    pub fn as_set(&self) -> Option<&T> {
        if let Self::Set(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn apply_to(&self, target: &mut Option<T>)
    where
        T: Clone,
    {
        match self {
            Self::Keep => {}
            Self::Clear => *target = None,
            Self::Set(value) => *target = Some(value.clone()),
        }
    }
}
impl<T: Serialize> Serialize for FieldPatch<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for FieldPatch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        })
    }
}
pub fn normalize_time(value: DateTime<Utc>) -> DateTime<Utc> {
    chrono::SubsecRound::trunc_subsecs(value, 6)
}
fn deserialize_optional_time<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<DateTime<Utc>>, D::Error> {
    Ok(Option::<DateTime<Utc>>::deserialize(deserializer)?.map(normalize_time))
}
fn deserialize_time_patch<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<FieldPatch<DateTime<Utc>>, D::Error> {
    Ok(match deserialize_optional_time(deserializer)? {
        Some(value) => FieldPatch::Set(value),
        None => FieldPatch::Clear,
    })
}
pub fn validate_validity(from: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> Result<()> {
    if from
        .zip(until)
        .is_some_and(|(from, until)| normalize_time(from) >= normalize_time(until))
    {
        return Err(Error::Validation(
            "valid_from must precede valid_until at microsecond precision".into(),
        ));
    }
    Ok(())
}
pub fn validate_summary_text(summary: &str) -> Result<()> {
    if summary.trim().is_empty() || summary.len() > 8192 {
        return Err(Error::Validation(
            "summary must contain 1..8192 bytes".into(),
        ));
    }
    Ok(())
}

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
    #[serde(default, alias = "knowledge_type")]
    pub content_type: Option<KnowledgeType>,
    pub verification: Option<Verification>,
    pub retired: bool,
    pub provenance: Vec<Value>,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default = "legacy_author")]
    pub created_by: String,
    #[serde(default)]
    pub last_verified_by: Option<String>,
    #[serde(default)]
    pub last_verified_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub retirement_reason: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub summary_of_revision: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_time")]
    pub valid_from: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_optional_time")]
    pub valid_until: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_optional_time")]
    pub review_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub successor_id: Option<Uuid>,
}

fn legacy_author() -> String {
    "legacy:unknown".into()
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
    #[serde(default)]
    pub description: Option<String>,
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
    #[serde(rename = "knowledge_type", alias = "content_type")]
    pub content_type: Option<KnowledgeType>,
    pub verification: Option<Verification>,
    pub retired: Option<bool>,
    pub provenance: Option<Vec<Value>>,
    pub retirement_reason: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "FieldPatch::is_keep",
        deserialize_with = "deserialize_time_patch"
    )]
    pub valid_from: FieldPatch<DateTime<Utc>>,
    #[serde(
        default,
        skip_serializing_if = "FieldPatch::is_keep",
        deserialize_with = "deserialize_time_patch"
    )]
    pub valid_until: FieldPatch<DateTime<Utc>>,
    #[serde(
        default,
        skip_serializing_if = "FieldPatch::is_keep",
        deserialize_with = "deserialize_time_patch"
    )]
    pub review_at: FieldPatch<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "FieldPatch::is_keep")]
    pub successor_id: FieldPatch<Uuid>,
}

fn is_false(value: &bool) -> bool {
    !*value
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
        #[serde(default, rename = "knowledge_type", alias = "content_type")]
        content_type: Option<KnowledgeType>,
        #[serde(default)]
        verification: Option<Verification>,
        #[serde(default)]
        provenance: Vec<Value>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_time"
        )]
        valid_from: Option<DateTime<Utc>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_time"
        )]
        valid_until: Option<DateTime<Utc>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_time"
        )]
        review_at: Option<DateTime<Utc>>,
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
        #[serde(default, skip_serializing_if = "is_false")]
        include_lifecycle: bool,
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
        #[serde(default)]
        description: Option<String>,
    },
    SetParent {
        id: Uuid,
        expected_version: i64,
        parent_id: Option<Uuid>,
    },
    VerifyKnowledge {
        id: Uuid,
        expected_version: i64,
        verification: Verification,
        evidence: Vec<Value>,
    },
    VerifyNode {
        id: Uuid,
        expected_version: i64,
        verification: Verification,
        evidence: Vec<Value>,
    },
    SetSummary {
        id: Uuid,
        expected_version: i64,
        source_revision: i64,
        summary: Option<String>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeFilters {
    #[serde(default)]
    pub kind: Option<NodeKind>,
    #[serde(default)]
    pub include_retired: bool,
    #[serde(default)]
    pub include_disputed: bool,
    #[serde(default, deserialize_with = "deserialize_optional_time")]
    pub valid_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_optional_time")]
    pub review_due_at: Option<DateTime<Utc>>,
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
                verification,
                provenance,
                valid_from,
                valid_until,
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
                validate_validity(*valid_from, *valid_until)?;
                if *kind == NodeKind::Knowledge
                    && (task_state.is_some() || !acceptance_criteria.is_empty())
                {
                    return Err(Error::Validation(
                        "task fields cannot be set on knowledge".into(),
                    ));
                }
                if verification.is_some_and(|state| state != Verification::Unverified) {
                    return Err(Error::Validation(
                        "create unverified records; use verify_node for verification assertions"
                            .into(),
                    ));
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
                if patch.body.as_ref().is_some_and(|s| s.len() > 262_144)
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
                if patch
                    .retirement_reason
                    .as_ref()
                    .is_some_and(|v| v.len() > 4096)
                {
                    return Err(Error::Validation(
                        "retirement reason exceeds 4096 bytes".into(),
                    ));
                }
                if let Some(provenance) = &patch.provenance {
                    validate_provenance(provenance)?;
                }
                if let (FieldPatch::Set(from), FieldPatch::Set(until)) =
                    (&patch.valid_from, &patch.valid_until)
                {
                    validate_validity(Some(*from), Some(*until))?;
                }
            }
            Mutation::DeleteNode {
                expected_version, ..
            }
            | Mutation::RemoveEdge {
                expected_version, ..
            }
            | Mutation::SetParent {
                expected_version, ..
            } => validate_version(*expected_version)?,
            Mutation::VerifyKnowledge {
                expected_version,
                evidence,
                ..
            }
            | Mutation::VerifyNode {
                expected_version,
                evidence,
                ..
            } => {
                validate_version(*expected_version)?;
                validate_provenance(evidence)?;
            }
            Mutation::SetSummary {
                expected_version,
                source_revision,
                summary,
                ..
            } => {
                validate_version(*expected_version)?;
                validate_version(*source_revision)?;
                if let Some(summary) = summary {
                    validate_summary_text(summary)?;
                }
            }
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
                description,
                ..
            } => {
                if description.as_ref().is_some_and(|v| v.len() > 4096) {
                    return Err(Error::Validation(
                        "edge description exceeds 4096 bytes".into(),
                    ));
                }
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
    crate::knowledge_rules::validate_sources(provenance)
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
    if body.len() > 262_144 || acceptance.len() > 16_384 {
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
    fn default_restore_preserves_pre_upgrade_receipt_payload_shape() {
        let legacy = serde_json::json!({"op":"restore_node","id":Uuid::nil(),"expected_version":4,"revision":2});
        let request: Mutation = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), legacy);
        let mut explicit = legacy.clone();
        explicit["include_lifecycle"] = serde_json::json!(false);
        let request: Mutation = serde_json::from_value(explicit.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), legacy);
        explicit["include_lifecycle"] = serde_json::json!(true);
        let request: Mutation = serde_json::from_value(explicit.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), explicit);
    }
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

#[cfg(test)]
mod shared_field_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tri_state_patches_preserve_missing_null_and_values() {
        let absent: NodePatch = serde_json::from_value(json!({})).unwrap();
        assert!(matches!(absent.valid_from, FieldPatch::Keep));
        assert!(matches!(absent.successor_id, FieldPatch::Keep));
        let absent = serde_json::to_value(absent).unwrap();
        assert!(absent.get("valid_from").is_none());
        assert!(absent.get("successor_id").is_none());
        let clear: NodePatch =
            serde_json::from_value(json!({"valid_from":null,"successor_id":null})).unwrap();
        assert!(matches!(clear.valid_from, FieldPatch::Clear));
        assert!(matches!(clear.successor_id, FieldPatch::Clear));
        assert!(serde_json::to_value(clear).unwrap()["valid_from"].is_null());
        let set: NodePatch = serde_json::from_value(
            json!({"valid_from":"2026-10-30T03:00:00.123456789-07:00","successor_id":Uuid::nil()}),
        )
        .unwrap();
        assert_eq!(
            set.valid_from.as_set().unwrap().to_rfc3339(),
            "2026-10-30T10:00:00.123456+00:00"
        );
        assert_eq!(set.successor_id.as_set(), Some(&Uuid::nil()));
        for bad in [
            json!({"review_at":"2026-10-30"}),
            json!({"review_at":"2026-10-30T10:00:00"}),
            json!({"review_at":false}),
        ] {
            assert!(serde_json::from_value::<NodePatch>(bad).is_err());
        }
    }
    #[test]
    fn content_type_aliases_preserve_normalized_legacy_receipts() {
        let old = json!({"op":"create_node","id":null,"kind":"task","title":"task","body":"","acceptance_criteria":"","labels":[],"metadata":{},"task_state":null,"knowledge_type":null,"verification":null,"provenance":[]});
        let parsed: Mutation = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), old);
        let mut preferred = old.clone();
        preferred.as_object_mut().unwrap().remove("knowledge_type");
        preferred["content_type"] = json!("decision");
        let mut legacy = old.clone();
        legacy["knowledge_type"] = json!("decision");
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<Mutation>(preferred).unwrap()).unwrap(),
            legacy
        );
        let old_patch = json!({"title":null,"body":null,"acceptance_criteria":null,"labels":null,"metadata":null,"task_state":null,"knowledge_type":null,"verification":null,"retired":null,"provenance":null,"retirement_reason":null});
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<NodePatch>(old_patch.clone()).unwrap())
                .unwrap(),
            old_patch
        );
        assert!(
            serde_json::from_value::<NodePatch>(
                json!({"knowledge_type":"note","content_type":"fact"})
            )
            .is_err(),
            "aliases cannot silently overwrite each other"
        );
    }
    #[test]
    fn validity_is_strict_after_microsecond_normalization_and_tasks_accept_common_fields() {
        let create:Mutation=serde_json::from_value(json!({"op":"create_node","kind":"task","title":"task","content_type":"procedure","verification":"unverified","provenance":[],"valid_from":"2026-10-30T00:00:00.0000001Z","valid_until":"2026-10-30T00:00:00.0000009Z"})).unwrap();
        assert!(validate_operations(&[create]).is_err());
        let create:Mutation=serde_json::from_value(json!({"op":"create_node","kind":"task","title":"task","content_type":"procedure","verification":"unverified","provenance":[],"valid_from":"2026-10-30T00:00:00Z","valid_until":"2026-10-30T00:00:00.000001Z"})).unwrap();
        assert!(validate_operations(&[create]).is_ok());
    }
}
