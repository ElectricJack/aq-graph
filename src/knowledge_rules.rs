//! Knowledge policy shared by live mutations and rollback-only proposal previews.
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
    if n.kind == NodeKind::Task {
        return Ok(());
    }
    if p.capability == Capability::Admin {
        return Ok(());
    }
    if n.created_by != p.actor_id || n.verification != Some(Verification::Unverified) {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub fn reset_verification(n: &mut Node) {
    if n.kind == NodeKind::Knowledge {
        n.verification = Some(Verification::Unverified);
        n.last_verified_by = None;
        n.last_verified_at = None;
    }
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
    if n.kind != NodeKind::Knowledge || n.deleted || n.retired {
        return Err(invalid("Verification requires live active knowledge"));
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
    if n.kind == NodeKind::Task && (n.retired || n.retirement_reason.is_some()) {
        return Err(invalid("Retirement fields apply only to knowledge"));
    }
    if n.retirement_reason
        .as_ref()
        .is_some_and(|reason| reason.trim().is_empty() || reason.len() > 4096)
    {
        return Err(invalid("Retirement reason must contain 1..4096 bytes"));
    }
    if !n.retired && n.retirement_reason.is_some() {
        return Err(invalid("A retirement reason requires retired knowledge"));
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

pub fn validate_create(n: &Node) -> Result<()> {
    validate_sources(&n.provenance)?;
    validate_retirement(n)?;
    if n.kind == NodeKind::Knowledge
        && (n.verification != Some(Verification::Unverified)
            || n.last_verified_by.is_some()
            || n.last_verified_at.is_some()
            || n.retired
            || n.retirement_reason.is_some())
    {
        return Err(invalid(
            "Create active unverified knowledge; use explicit verification or retirement operations",
        ));
    }
    Ok(())
}
