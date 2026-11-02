//! Graph-owned committed data observations. Never execution or wake authority.
mod store;
use crate::domain::{Error, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;
pub const MAX_REQUEST: usize = 65_536;
pub const MAX_RESPONSE: usize = 262_144;
pub const MAX_SELECTORS: usize = 16;
pub const MAX_GROUPS: usize = 16;
pub const MAX_LEASE_MS: i64 = 3_600_000;
pub const RETRY_RETENTION_MS: i64 = 3_600_000;
pub const MAX_CONTROLS: i64 = 1024;
pub const MAX_ACTOR_CONTROLS: i64 = 256;
pub const MAX_CONTROL_BYTES: i64 = 16_777_216;
pub const MAX_RECEIPT_BYTES: usize = 65_536;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Contract {
    #[serde(rename = "graph-committed-observation-1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub workspace_id: Uuid,
    pub stream_id: Uuid,
    pub generation: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub scope: Scope,
    pub committed_sequence: String,
    pub group_id: Option<Uuid>,
}
impl Cursor {
    pub fn encode(&self) -> Result<String> {
        self.validate()?;
        Ok(URL_SAFE_NO_PAD.encode(canonical(self)?))
    }
    pub fn decode(value: &str) -> Result<Self> {
        check(value.len() <= 1024, "observation cursor bound")?;
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| bad("observation cursor encoding"))?;
        let c: Self =
            serde_json::from_slice(&bytes).map_err(|_| bad("observation cursor fields"))?;
        c.validate()?;
        check(
            c.encode()? == value,
            "observation cursor canonical encoding",
        )?;
        Ok(c)
    }
    fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        let seq = decimal(&self.committed_sequence, false)?;
        check(
            (seq == 0) == self.group_id.is_none(),
            "observation cursor anchor",
        )?;
        if let Some(id) = self.group_id {
            nonnil(id)?;
        }
        Ok(())
    }
}
impl Scope {
    fn validate(&self) -> Result<()> {
        nonnil(self.workspace_id)?;
        nonnil(self.stream_id)?;
        decimal(&self.generation, true)?;
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selector {
    Task {
        member_id: Uuid,
        task_id: Uuid,
        starting_logical_revision: String,
    },
    Dependency {
        member_id: Uuid,
        edge_id: Uuid,
        edge_version: String,
        prerequisite_id: Uuid,
        dependent_id: Uuid,
        starting_logical_revision: String,
    },
}
impl Selector {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Task {
                member_id,
                task_id,
                starting_logical_revision,
            } => {
                nonnil(*member_id)?;
                nonnil(*task_id)?;
                decimal(starting_logical_revision, true)?;
            }
            Self::Dependency {
                member_id,
                edge_id,
                edge_version,
                prerequisite_id,
                dependent_id,
                starting_logical_revision,
            } => {
                nonnil(*member_id)?;
                for id in [edge_id, prerequisite_id, dependent_id] {
                    nonnil(*id)?;
                }
                check(
                    prerequisite_id != dependent_id,
                    "observation dependency self-edge",
                )?;
                decimal(edge_version, true)?;
                decimal(starting_logical_revision, true)?;
            }
        }
        Ok(())
    }
    fn member(&self) -> Uuid {
        match self {
            Self::Task { member_id, .. } | Self::Dependency { member_id, .. } => *member_id,
        }
    }
}
/// Unpinned identities: this read establishes actual revisions and cursor together.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotTarget {
    Task {
        task_id: Uuid,
    },
    Dependency {
        edge_id: Uuid,
        prerequisite_id: Uuid,
        dependent_id: Uuid,
    },
}
impl SnapshotTarget {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Task { task_id } => nonnil(*task_id),
            Self::Dependency {
                edge_id,
                prerequisite_id,
                dependent_id,
            } => {
                for id in [edge_id, prerequisite_id, dependent_id] {
                    nonnil(*id)?;
                }
                check(
                    prerequisite_id != dependent_id,
                    "observation dependency self-edge",
                )
            }
        }
    }
}
impl From<&Selector> for SnapshotTarget {
    fn from(s: &Selector) -> Self {
        match *s {
            Selector::Task { task_id, .. } => Self::Task { task_id },
            Selector::Dependency {
                edge_id,
                prerequisite_id,
                dependent_id,
                ..
            } => Self::Dependency {
                edge_id,
                prerequisite_id,
                dependent_id,
            },
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRequest {
    pub contract: Contract,
    pub targets: Vec<SnapshotTarget>,
}
impl SnapshotRequest {
    fn validate(&self) -> Result<()> {
        check(
            !self.targets.is_empty() && self.targets.len() <= MAX_SELECTORS,
            "observation snapshot target count",
        )?;
        let mut seen = std::collections::BTreeSet::new();
        for target in &self.targets {
            target.validate()?;
            check(
                seen.insert(canonical(target)?),
                "observation duplicate snapshot target",
            )?;
        }
        check(
            canonical(self)?.len() <= MAX_REQUEST,
            "observation request bytes",
        )
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        check(
            !bytes.is_empty() && bytes.len() <= MAX_REQUEST,
            "observation request bytes",
        )?;
        serde_json::from_slice(bytes).map_err(|_| bad("observation closed snapshot fields"))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRead {
    pub contract: Contract,
    pub request_digest: String,
    pub status: Status,
    pub scope: Scope,
    pub snapshot: Option<Snapshot>,
    pub gap: Option<Gap>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    pub contract: Contract,
    pub subscriber_id: Uuid,
    pub condition_digest: String,
    pub expected_scope: Scope,
    pub from_cursor: Option<Cursor>,
    pub original_expires_at_ms: String,
    pub selectors: Vec<Selector>,
}
impl RegisterRequest {
    pub fn validate(&self, workspace: Uuid, now: i64) -> Result<()> {
        nonnil(self.subscriber_id)?;
        self.expected_scope.validate()?;
        if let Some(cursor) = &self.from_cursor {
            cursor.validate()?;
            check(
                cursor.scope == self.expected_scope,
                "observation starting cursor scope",
            )?;
        }
        check(
            self.expected_scope.workspace_id == workspace,
            "observation workspace mismatch",
        )?;
        raw_digest(&self.condition_digest)?;
        let expires = decimal(&self.original_expires_at_ms, true)?;
        check(
            expires.saturating_sub(now) <= MAX_LEASE_MS,
            "observation original lease bound",
        )?;
        check(
            !self.selectors.is_empty() && self.selectors.len() <= MAX_SELECTORS,
            "observation selector count",
        )?;
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.selectors {
            s.validate()?;
            check(seen.insert(s.member()), "observation duplicate member")?;
        }
        check(
            canonical(self)?.len() <= MAX_REQUEST,
            "observation request bytes",
        )
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        check(
            !bytes.is_empty() && bytes.len() <= MAX_REQUEST,
            "observation request bytes",
        )?;
        serde_json::from_slice(bytes).map_err(|_| bad("observation closed request fields"))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Task,
    Knowledge,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Draft,
    Active,
    Completed,
    Canceled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Blocks,
    #[serde(rename = "parent-child")]
    ParentChild,
    References,
    MotivatedBy,
    Produces,
    Supports,
    Contradicts,
    Supersedes,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeProjection {
    pub id: Uuid,
    pub kind: NodeKind,
    pub version: String,
    pub logical_revision: String,
    pub task_state: Option<TaskState>,
    pub deleted: bool,
    pub retired: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EdgeProjection {
    pub id: Uuid,
    pub edge_type: Relation,
    pub source: Uuid,
    pub target: Uuid,
    pub source_revision: Option<String>,
    pub target_revision: Option<String>,
    pub version: String,
    pub deleted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservedObject {
    Node {
        id: Uuid,
        value: Option<NodeProjection>,
    },
    Edge {
        id: Uuid,
        value: Option<EdgeProjection>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotContent {
    pub scope: Scope,
    pub captured_cursor: Cursor,
    pub objects: Vec<ObservedObject>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub content: SnapshotContent,
    pub snapshot_digest: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
pub enum RevisionKind {
    #[serde(rename = "graph_logical_record_revision_1")]
    GraphLogicalRecordRevision1,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
pub enum ReferenceKind {
    #[serde(rename = "graph_observation_subscription")]
    Subscription,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
pub enum ReferenceVersion {
    #[serde(rename = "1")]
    V1,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
pub enum Authority {
    #[serde(rename = "none")]
    None,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    JournalCapacity,
    TransactionFactLimit,
    DomainInvariant,
    Retention,
    CoverageGap,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub kind: ReferenceKind,
    pub id: String,
    pub version: ReferenceVersion,
    pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptContent {
    pub contract: Contract,
    pub subscriber_id: Uuid,
    pub actor_id: String,
    pub request_digest: String,
    pub condition_digest: String,
    pub selectors: Vec<Selector>,
    pub scope: Scope,
    pub original_expires_at_ms: String,
    pub snapshot: Snapshot,
    pub initial_cursor: Cursor,
    pub revision_kind: RevisionKind,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub content: ReceiptContent,
    pub reference: Reference,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Available,
    Gap,
    Expired,
    Released,
    SourceChanged,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    pub reason: GapReason,
    pub requested_after: Option<Cursor>,
    pub lost_from_sequence: String,
    pub retained_floor: String,
    pub through: Cursor,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub contract: Contract,
    pub status: Status,
    pub scope: Scope,
    pub receipt: Option<Receipt>,
    pub gap: Option<Gap>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransitionClass {
    Created,
    LifecycleOnly,
    RecordChanged,
    MultipleChanges,
    Deleted,
    DomainInvariantFailure,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "object_kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Fact {
    Node {
        object_id: Uuid,
        before: Option<NodeProjection>,
        after: Option<NodeProjection>,
        mutation_count: String,
        transition_class: TransitionClass,
    },
    Edge {
        object_id: Uuid,
        before: Option<EdgeProjection>,
        after: Option<EdgeProjection>,
        mutation_count: String,
        transition_class: TransitionClass,
    },
}
impl Fact {
    /// Historical Graph transition metadata only; AQ must recheck current eligibility.
    pub fn terminal_from(&self, starting_revision: &str) -> Result<Option<&NodeProjection>> {
        decimal(starting_revision, true)?;
        if let Self::Node {
            object_id,
            before: Some(b),
            after: Some(a),
            mutation_count,
            transition_class: TransitionClass::LifecycleOnly,
        } = self
            && *object_id == b.id
            && b.id == a.id
            && mutation_count == "1"
            && b.kind == NodeKind::Task
            && a.kind == NodeKind::Task
            && b.logical_revision == starting_revision
            && !b.deleted
            && !b.retired
            && !a.deleted
            && !a.retired
            && matches!(b.task_state, Some(TaskState::Draft | TaskState::Active))
            && matches!(
                a.task_state,
                Some(TaskState::Completed | TaskState::Canceled)
            )
            && Some(decimal(&a.logical_revision, true)?)
                == decimal(&b.logical_revision, true)?.checked_add(1)
            && Some(decimal(&a.version, true)?) == decimal(&b.version, true)?.checked_add(1)
        {
            return Ok(Some(a));
        }
        Ok(None)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitGroup {
    pub scope: Scope,
    pub committed_sequence: String,
    pub group_id: Uuid,
    pub facts: Vec<Fact>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pull {
    pub contract: Contract,
    pub lookup_only: bool,
    pub status: Status,
    pub receipt: Receipt,
    pub scope: Scope,
    pub after: Cursor,
    pub through: Cursor,
    pub groups: Vec<CommitGroup>,
    pub next_cursor: Option<Cursor>,
    pub retained_floor: String,
    pub gap: Option<Gap>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub selectors: usize,
    pub groups: usize,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub journal_bytes: i64,
    pub journal_batches: i64,
    pub journal_facts: i64,
    pub facts_per_commit: i64,
    pub retained_controls: i64,
    pub actor_retained_controls: i64,
    pub control_bytes: i64,
    pub max_lease_ms: i64,
    pub retry_retention_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub contract: Contract,
    pub scope: Scope,
    pub available: bool,
    pub coverage: Status,
    pub head: Cursor,
    pub retained_floor: String,
    pub limits: Limits,
    pub authority: Authority,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullQuery {
    pub after: Option<String>,
    pub through: Option<String>,
    pub receipt_digest: Option<String>,
    pub request_digest: Option<String>,
    pub limit: Option<usize>,
    pub max_bytes: Option<usize>,
}
impl PullQuery {
    pub(crate) fn limits(&self) -> Result<(usize, usize)> {
        let n = self.limit.unwrap_or(MAX_GROUPS);
        let b = self.max_bytes.unwrap_or(MAX_RESPONSE);
        check(
            (1..=MAX_GROUPS).contains(&n) && (MAX_RECEIPT_BYTES..=MAX_RESPONSE).contains(&b),
            "observation page limits",
        )?;
        Ok((n, b))
    }
}
/// The owner, never a request timestamp, supplies lease time. Sample after waits
/// and again at the final response decision; the original absolute expiry stays fixed.
pub(crate) trait OwnerClock: Send + Sync {
    fn now_ms(&self) -> i64;
}
#[derive(Clone, Copy)]
pub(crate) struct SystemClock;
impl OwnerClock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}
// Fixed clocks exist only in the component test build, preserving deterministic
// original-expiry vectors. Production HTTP always uses SystemClock.
#[cfg(test)]
impl OwnerClock for i64 {
    fn now_ms(&self) -> i64 {
        *self
    }
}
#[derive(Clone)]
pub(crate) struct CredentialIdentity(pub String);
pub(crate) fn identity(token: &str) -> CredentialIdentity {
    CredentialIdentity(hex::encode(Sha256::digest(token.as_bytes())))
}
pub fn schema() -> serde_json::Value {
    let mut schema =
        serde_json::to_value(schemars::schema_for!(Document)).expect("observation schema");
    schema["$id"] = serde_json::json!("urn:aq-graph:graph-committed-observation-1");
    schema
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Document {
    SnapshotRequest(Box<SnapshotRequest>),
    SnapshotRead(Box<SnapshotRead>),
    Request(Box<RegisterRequest>),
    Registration(Box<Registration>),
    Pull(Box<Pull>),
    Capabilities(Box<Capabilities>),
}
pub(crate) fn bad(s: &str) -> Error {
    Error::Validation(s.into())
}
pub(crate) fn check(v: bool, s: &str) -> Result<()> {
    if v { Ok(()) } else { Err(bad(s)) }
}
pub(crate) fn decimal(s: &str, positive: bool) -> Result<i64> {
    check(
        !s.is_empty()
            && s.len() <= 19
            && s.bytes().all(|b| b.is_ascii_digit())
            && (s.len() == 1 || !s.starts_with('0')),
        "observation canonical decimal",
    )?;
    let n = s
        .parse::<i64>()
        .map_err(|_| bad("observation decimal range"))?;
    check(!positive || n > 0, "observation positive revision")?;
    Ok(n)
}
fn nonnil(id: Uuid) -> Result<()> {
    check(!id.is_nil(), "observation nonnil identity")
}
pub(crate) fn raw_digest(s: &str) -> Result<&str> {
    let raw = s
        .strip_prefix("sha256:")
        .ok_or_else(|| bad("observation digest"))?;
    check(
        raw.len() == 64
            && raw
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "observation digest",
    )?;
    Ok(raw)
}
pub(crate) fn canonical<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    let mut value = serde_json::to_value(v).map_err(|_| bad("observation encoding"))?;
    value.sort_all_objects();
    serde_json::to_vec(&value).map_err(|_| bad("observation encoding"))
}
pub fn digest<T: Serialize>(v: &T) -> Result<String> {
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(canonical(v)?))
    ))
}

#[cfg(test)]
mod tests;
