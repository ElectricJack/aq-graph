//! Validation and merging of portable evidence descriptors.
use super::*;
use chrono::DateTime;
use std::collections::HashSet;
use uuid::Uuid;
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
        validate_descriptor(source, &mut ids)?;
    }
    Ok(())
}

fn validate_descriptor<'a>(source: &'a Value, ids: &mut HashSet<&'a str>) -> Result<()> {
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
    let (required, allowed) = descriptor_fields(kind)?;
    check_descriptor_fields(source, map, required, allowed)?;
    check_evidence_ids(source, kind)?;
    if kind == "url" {
        check_observed_url(source)?;
    }
    Ok(())
}

fn descriptor_fields(kind: &str) -> Result<(&'static [&'static str], &'static [&'static str])> {
    Ok(match kind {
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
    })
}

fn check_descriptor_fields(
    source: &Value,
    map: &serde_json::Map<String, Value>,
    required: &[&str],
    allowed: &[&str],
) -> Result<()> {
    if map
        .keys()
        .any(|k| !["source_id", "kind"].contains(&k.as_str()) && !allowed.contains(&k.as_str()))
    {
        return Err(invalid("Evidence fields must match the descriptor kind"));
    }
    for field in allowed {
        let value = map.get(*field).filter(|v| !v.is_null());
        match value {
            Some(value) => validate_descriptor_value(source, field, value)?,
            None if required.contains(field) => {
                return Err(invalid("Evidence descriptor is missing a required field"));
            }
            None => {}
        }
    }
    Ok(())
}

fn validate_descriptor_value(source: &Value, field: &str, value: &Value) -> Result<()> {
    match field {
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
    Ok(())
}

fn check_evidence_ids(source: &Value, kind: &str) -> Result<()> {
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
    check_artifact_id(source)?;
    Ok(())
}

fn check_artifact_id(source: &Value) -> Result<()> {
    if let Some(artifact) = source.get("artifact_id").filter(|v| !v.is_null())
        && artifact
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .is_none()
    {
        return Err(invalid("Evidence artifact_id must be a UUID"));
    }
    Ok(())
}

fn check_observed_url(source: &Value) -> Result<()> {
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
    Ok(())
}

pub(super) fn merge_evidence(previous: &[Value], evidence: &[Value]) -> Result<Vec<Value>> {
    // Merge evidence by stable source_id while preserving all previous sources.
    let mut sources = previous.to_vec();
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
    Ok(sources)
}
