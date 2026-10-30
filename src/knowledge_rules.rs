//! Shared record policy for live mutations and rollback-only proposal previews.
//! External source descriptors are attributed evidence references, never proof of
//! remote retention, access rights, truth, or an Agent Q review's approval.
use crate::domain::*;
use chrono::DateTime;
use serde_json::Value;
use std::collections::HashSet;
use uuid::Uuid;

fn invalid(message: &str) -> Error {
    Error::Validation(message.into())
}
fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| invalid("Evidence descriptor requires nonempty string fields"))
}
fn hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Check v1-compatible portable descriptor shapes, without dereferencing URLs,
/// repositories, paths, or any AQ database identity supplied by a caller.
pub fn validate_sources(sources: &[Value]) -> Result<()> {
    if sources.len() > 64
        || serde_json::to_vec(sources)
            .map_err(|_| invalid("Invalid evidence JSON"))?
            .len()
            > 16_384
    {
        return Err(invalid(
            "Evidence permits at most 64 descriptors and 16384 bytes",
        ));
    }
    let mut ids = HashSet::new();
    for source in sources {
        let map = source
            .as_object()
            .ok_or_else(|| invalid("Evidence descriptors must be objects"))?;
        let source_id = text(source, "source_id")?;
        if source_id.len() > 128 || !ids.insert(source_id) {
            return Err(invalid(
                "Evidence source_id must be unique and contain 1..128 bytes",
            ));
        }
        let kind = text(source, "kind")?;
        let (required, allowed): (&[&str], &[&str]) = match kind {
            "task" => (&["task_id"], &["task_id", "attempt_id", "session_id"]),
            "review" => (
                &["review_id", "revision", "sha256"],
                &["review_id", "revision", "sha256"],
            ),
            "git" => (
                &["repository", "commit", "path"],
                &["repository", "commit", "path", "blob_sha256"],
            ),
            "artifact" => (&["artifact_id", "sha256"], &["artifact_id", "sha256"]),
            "url" => (
                &["url", "observed_at", "retained"],
                &["url", "observed_at", "retained", "artifact_id", "sha256"],
            ),
            "legacy" => (
                &["snapshot_id", "store", "key", "sha256"],
                &["snapshot_id", "store", "key", "sha256"],
            ),
            _ => return Err(invalid("Unsupported evidence descriptor kind")),
        };
        if map
            .keys()
            .any(|k| k != "source_id" && k != "kind" && !allowed.contains(&k.as_str()))
        {
            return Err(invalid("Evidence fields must match the descriptor kind"));
        }
        for field in allowed {
            let Some(value) = map.get(*field).filter(|v| !v.is_null()) else {
                if required.contains(field) {
                    return Err(invalid("Evidence descriptor is missing a required field"));
                }
                continue;
            };
            match *field {
                "revision" if value.as_i64().is_some_and(|v| v > 0) => {}
                "revision" => {
                    return Err(invalid(
                        "Evidence review revision must be a positive integer",
                    ));
                }
                "retained" if value.is_boolean() => {}
                "retained" => return Err(invalid("Evidence retained field must be boolean")),
                _ => {
                    text(source, field)?;
                }
            }
        }
        for field in ["sha256", "blob_sha256"] {
            if let Some(value) = source.get(field).filter(|v| !v.is_null())
                && !value.as_str().is_some_and(|s| hex(s, &[64]))
            {
                return Err(invalid(
                    "Evidence SHA-256 must contain 64 lowercase hexadecimal characters",
                ));
            }
        }
        if kind == "git" && !hex(text(source, "commit")?, &[40, 64]) {
            return Err(invalid(
                "Git evidence requires a full hexadecimal commit identity",
            ));
        }
        if let Some(artifact) = source.get("artifact_id").filter(|v| !v.is_null())
            && artifact
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .is_none()
        {
            return Err(invalid("Evidence artifact_id must be a UUID"));
        }
        if kind == "url" {
            let url = reqwest::Url::parse(text(source, "url")?)
                .map_err(|_| invalid("Evidence URL must be absolute HTTP or HTTPS"))?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(invalid("Evidence URL must be absolute HTTP or HTTPS"));
            }
            DateTime::parse_from_rfc3339(text(source, "observed_at")?)
                .map_err(|_| invalid("Evidence observed_at must be RFC3339 with timezone"))?;
            if source["retained"] == true && source.get("artifact_id").is_none_or(Value::is_null) {
                return Err(invalid(
                    "Retained URL evidence requires an artifact reference",
                ));
            }
        }
    }
    Ok(())
}

/// Read/write capability is checked at the mutation boundary; this adds the
/// knowledge-specific creator and protected-revision checks.
pub fn authorize_edit(p: &Principal, n: &Node) -> Result<()> {
    p.require_write()?;
    if p.workspace_id != n.workspace_id {
        return Err(Error::NotFound);
    }
    if p.capability == Capability::Admin {
        return Ok(());
    }
    if n.retired
        || n.verification != Some(Verification::Unverified)
        || (n.kind == NodeKind::Knowledge && n.created_by != p.actor_id)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub fn reset_verification(n: &mut Node) {
    n.verification = Some(Verification::Unverified);
    n.last_verified_by = None;
    n.last_verified_at = None;
}

/// Verification is an explicit privileged assertion about the current revision.
/// The caller owns version checks, history insertion and transaction commit.
pub fn verify(
    n: &mut Node,
    p: &Principal,
    verification: Verification,
    evidence: &[Value],
    reason: Option<&str>,
) -> Result<()> {
    p.require_admin()?;
    if p.workspace_id != n.workspace_id {
        return Err(Error::NotFound);
    }
    if n.deleted || n.retired {
        return Err(invalid("Verification requires a live nonretired node"));
    }
    if verification == Verification::Unverified || evidence.is_empty() {
        return Err(invalid(
            "Verification requires verified/disputed status and named evidence",
        ));
    }
    if reason.is_none_or(|reason| reason.trim().is_empty() || reason.len() > 4096) {
        return Err(invalid(
            "Verification requires a nonempty reason of at most 4096 bytes",
        ));
    }
    validate_sources(evidence)?;
    // Merge evidence by stable source_id while preserving all previous sources.
    let mut sources = n.provenance.clone();
    for evidence in evidence {
        if let Some(existing) = sources
            .iter_mut()
            .find(|source| source["source_id"] == evidence["source_id"])
        {
            *existing = evidence.clone();
        } else {
            sources.push(evidence.clone());
        }
    }
    validate_sources(&sources)?;
    n.provenance = sources;
    n.verification = Some(verification);
    n.last_verified_by = Some(p.actor_id.clone());
    n.last_verified_at = Some(server_time());
    Ok(())
}

pub fn validate_retirement(n: &Node) -> Result<()> {
    if n.successor_id.is_some() && !n.retired {
        return Err(invalid("A successor requires a retired node"));
    }
    if n.successor_id == Some(n.id) {
        return Err(invalid("A node cannot name itself as its successor"));
    }

    if n.retirement_reason
        .as_ref()
        .is_some_and(|reason| reason.trim().is_empty() || reason.len() > 4096)
    {
        return Err(invalid("Retirement reason must contain 1..4096 bytes"));
    }
    if !n.retired && n.retirement_reason.is_some() {
        return Err(invalid("A retirement reason requires a retired node"));
    }
    if n.retired
        && n.retirement_reason
            .as_ref()
            .is_none_or(|reason| reason.trim().is_empty() || reason.len() > 4096)
    {
        return Err(invalid(
            "Retirement requires a nonempty reason of at most 4096 bytes",
        ));
    }
    Ok(())
}

pub fn validate_temporal(n: &Node) -> Result<()> {
    validate_validity(n.valid_from, n.valid_until)
}

/// Summaries are derived assertions tied to the exact source material revision.
/// Mutation code clears them for every other material edit and restore.
pub fn clear_summary(n: &mut Node) {
    n.summary = None;
    n.summary_of_revision = None;
}
pub fn set_summary(n: &mut Node, source_revision: i64, summary: Option<&str>) -> Result<()> {
    if source_revision != n.content_revision {
        return Err(Error::Conflict(format!(
            "summary source revision {source_revision} is stale; current revision is {}",
            n.content_revision
        )));
    }
    match summary {
        None => clear_summary(n),
        Some(summary) => {
            validate_summary_text(summary)?;
            n.summary = Some(summary.to_owned());
            n.summary_of_revision = Some(source_revision);
        }
    }
    Ok(())
}
pub fn validate_summary(n: &Node) -> Result<()> {
    match (&n.summary, n.summary_of_revision) {
        (None, None) => Ok(()),
        (Some(summary), Some(source)) if source > 0 && source < n.content_revision => {
            validate_summary_text(summary)
        }
        _ => Err(invalid(
            "Summary text and an earlier same-node source revision must be present together",
        )),
    }
}

pub fn validate_create(n: &Node) -> Result<()> {
    validate_sources(&n.provenance)?;
    validate_retirement(n)?;
    validate_temporal(n)?;
    if n.content_type.is_none() {
        return Err(invalid("Every new node requires a content classification"));
    }
    if (n.kind == NodeKind::Task && n.task_state.is_none())
        || (n.kind == NodeKind::Knowledge
            && (n.task_state.is_some() || !n.acceptance_criteria.is_empty()))
    {
        return Err(invalid(
            "Task lifecycle and acceptance criteria apply only to tasks",
        ));
    }
    if n.verification != Some(Verification::Unverified)
        || n.last_verified_by.is_some()
        || n.last_verified_at.is_some()
        || n.retired
        || n.retirement_reason.is_some()
    {
        return Err(invalid(
            "Create active unverified nodes; use explicit verification or retirement operations",
        ));
    }
    if n.summary.is_some() || n.summary_of_revision.is_some() || n.successor_id.is_some() {
        return Err(invalid(
            "Create without summary or successor; use explicit revision operations",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod shared_policy_tests {
    use super::*;
    use serde_json::json;
    fn node(kind: NodeKind) -> Node {
        serde_json::from_value(json!({"id":Uuid::new_v4(),"workspace_id":Uuid::nil(),"kind":kind,"version":1,"content_revision":1,"title":"shared record","body":"","acceptance_criteria":"","labels":[],"metadata":{},"task_state":if kind==NodeKind::Task{Some(TaskState::Active)}else{None},"content_type":"note","verification":"unverified","retired":false,"provenance":[],"deleted":false,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","created_by":"creator"})).unwrap()
    }
    fn principal(actor: &str, capability: Capability) -> Principal {
        Principal {
            actor_id: actor.into(),
            workspace_id: Uuid::nil(),
            capability,
        }
    }
    #[test]
    fn shared_task_writers_do_not_bypass_verification_or_retirement_protection() {
        let mut task = node(NodeKind::Task);
        let writer = principal("other-writer", Capability::Write);
        let admin = principal("admin", Capability::Admin);
        assert!(
            authorize_edit(&writer, &task).is_ok(),
            "task authorship does not restrict ordinary shared writing"
        );
        task.verification = Some(Verification::Verified);
        assert!(matches!(
            authorize_edit(&writer, &task),
            Err(Error::Forbidden)
        ));
        assert!(authorize_edit(&admin, &task).is_ok());
        reset_verification(&mut task);
        task.retired = true;
        task.retirement_reason = Some("superseded procedure".into());
        assert!(validate_retirement(&task).is_ok());
        assert!(
            matches!(authorize_edit(&writer, &task), Err(Error::Forbidden)),
            "retired tasks are protected too"
        );
        assert!(authorize_edit(&admin, &task).is_ok());
        let knowledge = node(NodeKind::Knowledge);
        assert!(matches!(
            authorize_edit(&writer, &knowledge),
            Err(Error::Forbidden)
        ));
        assert!(authorize_edit(&principal("creator", Capability::Write), &knowledge).is_ok());
    }
    #[test]
    fn verification_and_summary_rules_apply_identically_to_both_kinds() {
        let evidence = vec![
            json!({"source_id":"evidence","kind":"url","url":"https://example.invalid/evidence","observed_at":"2026-10-30T00:00:00Z","retained":false}),
        ];
        for kind in [NodeKind::Task, NodeKind::Knowledge] {
            let mut n = node(kind);
            assert!(validate_create(&n).is_ok());
            verify(
                &mut n,
                &principal("admin", Capability::Admin),
                Verification::Verified,
                &evidence,
                Some("checked evidence"),
            )
            .unwrap();
            assert_eq!(n.verification, Some(Verification::Verified));
            assert_eq!(n.last_verified_by.as_deref(), Some("admin"));
            assert_eq!(
                n.last_verified_at.unwrap().timestamp_subsec_nanos() % 1000,
                0
            );
            reset_verification(&mut n);
            assert_eq!(n.verification, Some(Verification::Unverified));
            assert!(n.last_verified_by.is_none() && n.last_verified_at.is_none());
            set_summary(&mut n, 1, Some("Human-authored summary")).unwrap();
            n.content_revision = 2;
            assert!(validate_summary(&n).is_ok());
            assert_eq!(n.summary_of_revision, Some(1));
            assert!(matches!(
                set_summary(&mut n, 1, Some("stale")),
                Err(Error::Conflict(_))
            ));
            set_summary(&mut n, 2, None).unwrap();
            assert!(n.summary.is_none() && n.summary_of_revision.is_none());
            assert!(validate_summary(&n).is_ok());
            assert!(set_summary(&mut n, 2, Some("  ")).is_err());
            n.successor_id = Some(Uuid::new_v4());
            assert!(validate_retirement(&n).is_err());
            n.retired = true;
            n.retirement_reason = Some("new evidence".into());
            assert!(validate_retirement(&n).is_ok());
            n.successor_id = Some(n.id);
            assert!(validate_retirement(&n).is_err());
        }
    }
}
