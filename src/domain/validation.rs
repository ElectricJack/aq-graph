//! Validation of mutation requests and user-supplied record content.
use super::*;

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
    ops.iter().try_for_each(validate_operation)
}

fn validate_operation(op: &Mutation) -> Result<()> {
    if let Some(version) = expected_version(op) {
        validate_version(version)?;
    }
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
            validate_initial_state(
                *kind,
                task_state.is_some(),
                !acceptance_criteria.is_empty(),
                *verification,
            )?;
        }
        Mutation::UpdateNode { patch, .. } => validate_patch(patch)?,
        Mutation::DeleteNode { .. } | Mutation::RemoveEdge { .. } | Mutation::SetParent { .. } => {}
        Mutation::VerifyKnowledge { evidence, .. } | Mutation::VerifyNode { evidence, .. } => {
            validate_provenance(evidence)?
        }
        Mutation::SetSummary {
            source_revision,
            summary,
            ..
        } => {
            validate_version(*source_revision)?;
            if let Some(summary) = summary {
                validate_summary_text(summary)?;
            }
        }
        Mutation::RestoreNode { revision, .. } => validate_version(*revision)?,
        Mutation::AddEdge {
            source,
            target,
            source_revision,
            target_revision,
            edge_type,
            description,
            ..
        } => {
            validate_edge_fields(
                (*source, *target),
                (*source_revision, *target_revision),
                *edge_type,
                description.as_deref(),
            )?;
        }
    }
    Ok(())
}

fn expected_version(op: &Mutation) -> Option<i64> {
    match op {
        Mutation::CreateNode { .. } | Mutation::AddEdge { .. } => None,
        Mutation::UpdateNode {
            expected_version, ..
        }
        | Mutation::DeleteNode {
            expected_version, ..
        }
        | Mutation::RemoveEdge {
            expected_version, ..
        }
        | Mutation::SetParent {
            expected_version, ..
        }
        | Mutation::VerifyKnowledge {
            expected_version, ..
        }
        | Mutation::VerifyNode {
            expected_version, ..
        }
        | Mutation::SetSummary {
            expected_version, ..
        }
        | Mutation::RestoreNode {
            expected_version, ..
        } => Some(*expected_version),
    }
}

fn validate_initial_state(
    kind: NodeKind,
    has_state: bool,
    has_criteria: bool,
    verification: Option<Verification>,
) -> Result<()> {
    let task_fields = has_state || has_criteria;
    if kind == NodeKind::Knowledge && task_fields {
        return Err(Error::Validation(
            "task fields cannot be set on knowledge".into(),
        ));
    }
    if verification.is_some_and(|state| state != Verification::Unverified) {
        return Err(Error::Validation(
            "create unverified records; use verify_node for verification assertions".into(),
        ));
    }
    Ok(())
}

fn validate_edge_fields(
    (source, target): (Uuid, Uuid),
    (source_revision, target_revision): (Option<i64>, Option<i64>),
    edge_type: EdgeType,
    description: Option<&str>,
) -> Result<()> {
    if description.is_some_and(|v| v.len() > 4096) {
        return Err(Error::Validation(
            "edge description exceeds 4096 bytes".into(),
        ));
    }
    if source == target {
        return Err(Error::Validation("self links are not allowed".into()));
    }
    for rev in [source_revision, target_revision].into_iter().flatten() {
        validate_version(rev)?;
    }
    let pinned = source_revision.is_some() || target_revision.is_some();
    if matches!(edge_type, EdgeType::Blocks | EdgeType::ParentChild) && pinned {
        return Err(Error::Validation(
            "structural edges must reference current task state".into(),
        ));
    }
    Ok(())
}

fn validate_patch(patch: &NodePatch) -> Result<()> {
    if let Some(title) = &patch.title {
        validate_title(title)?;
    }
    validate_content_size(
        patch.body.as_deref().unwrap_or_default(),
        patch.acceptance_criteria.as_deref().unwrap_or_default(),
    )?;
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
    if let (FieldPatch::Set(from), FieldPatch::Set(until)) = (&patch.valid_from, &patch.valid_until)
    {
        validate_validity(Some(*from), Some(*until))?;
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
    let valid_label = |label: &String| !label.trim().is_empty() && label.len() <= 128;
    if labels.len() > 64 || !labels.iter().all(valid_label) {
        Err(Error::Validation(
            "at most 64 labels of 1..128 bytes are allowed".into(),
        ))
    } else {
        Ok(())
    }
}
pub(super) fn validate_metadata(metadata: &Value) -> Result<()> {
    let map = metadata
        .as_object()
        .ok_or_else(|| Error::Validation("metadata must be an object".into()))?;
    let valid_key = |key: &String| key.contains(':') && key.len() <= 128;
    if !map.keys().all(valid_key) || metadata.to_string().len() > 16_384 {
        return Err(Error::Validation(
            "metadata requires namespaced keys (e.g. aq:priority) and at most 16384 bytes".into(),
        ));
    }
    Ok(())
}
fn validate_provenance(provenance: &[Value]) -> Result<()> {
    let exceeds_limits = provenance.len() > 64
        || serde_json::to_vec(provenance)
            .map_err(|e| Error::Internal(e.to_string()))?
            .len()
            > 16_384;
    if exceeds_limits || !provenance.iter().all(Value::is_object) {
        return Err(Error::Validation(
            "provenance must have at most 64 objects and 16384 bytes".into(),
        ));
    }
    crate::knowledge_rules::validate_sources(provenance)
}
fn validate_content_size(body: &str, acceptance: &str) -> Result<()> {
    if body.len() > 262_144 || acceptance.len() > 16_384 {
        return Err(Error::Validation("content exceeds maximum size".into()));
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
    validate_content_size(body, acceptance)?;
    validate_labels(labels)?;
    validate_metadata(metadata)?;
    validate_provenance(provenance)
}
