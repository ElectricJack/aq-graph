//! Shared record policy for live mutations and rollback-only proposal previews.
//! External source descriptors are attributed evidence references, never proof of
//! remote retention, access rights, truth, or an Agent Q review's approval.
use crate::domain::*;
use serde_json::Value;
mod evidence;
use evidence::merge_evidence;
pub use evidence::validate_sources;

fn invalid(message: &str) -> Error {
    Error::Validation(message.into())
}
fn require_workspace(p: &Principal, n: &Node) -> Result<()> {
    if p.workspace_id != n.workspace_id {
        return Err(Error::NotFound);
    }
    Ok(())
}

/// Read/write capability is checked at the mutation boundary; this adds the
/// knowledge-specific creator and protected-revision checks.
pub fn authorize_edit(p: &Principal, n: &Node) -> Result<()> {
    p.require_write()?;
    require_workspace(p, n)?;
    if p.capability == Capability::Admin {
        return Ok(());
    }
    let edit_protected = n.retired || n.verification != Some(Verification::Unverified);
    let other_author = n.kind == NodeKind::Knowledge && n.created_by != p.actor_id;
    if edit_protected || other_author {
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
    require_workspace(p, n)?;
    validate_assertion(n, verification, evidence, reason)?;
    validate_sources(evidence)?;
    n.provenance = merge_evidence(&n.provenance, evidence)?;
    n.verification = Some(verification);
    n.last_verified_by = Some(p.actor_id.clone());
    n.last_verified_at = Some(server_time());
    Ok(())
}

fn validate_assertion(
    n: &Node,
    verification: Verification,
    evidence: &[Value],
    reason: Option<&str>,
) -> Result<()> {
    if n.deleted || n.retired {
        return Err(invalid("Verification requires a live nonretired node"));
    }
    if verification == Verification::Unverified || evidence.is_empty() {
        return Err(invalid(
            "Verification requires verified/disputed status and named evidence",
        ));
    }
    if reason.is_none_or(invalid_reason) {
        return Err(invalid(
            "Verification requires a nonempty reason of at most 4096 bytes",
        ));
    }
    Ok(())
}

pub fn validate_retirement(n: &Node) -> Result<()> {
    if n.successor_id.is_some() && !n.retired {
        return Err(invalid("A successor requires a retired node"));
    }
    if n.successor_id == Some(n.id) {
        return Err(invalid("A node cannot name itself as its successor"));
    }

    validate_retirement_reason(n.retired, n.retirement_reason.as_deref())
}

fn invalid_reason(reason: &str) -> bool {
    reason.trim().is_empty() || reason.len() > 4096
}

fn validate_retirement_reason(retired: bool, reason: Option<&str>) -> Result<()> {
    if reason.is_some_and(invalid_reason) {
        return Err(invalid("Retirement reason must contain 1..4096 bytes"));
    }
    if !retired && reason.is_some() {
        return Err(invalid("A retirement reason requires a retired node"));
    }
    if retired && reason.is_none() {
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
    validate_task_fields(n)?;
    let verified = !matches!(
        (n.verification, &n.last_verified_by, &n.last_verified_at),
        (Some(Verification::Unverified), None, None)
    );
    let retired = n.retired || n.retirement_reason.is_some();
    if verified || retired {
        return Err(invalid(
            "Create active unverified nodes; use explicit verification or retirement operations",
        ));
    }
    let has_summary = n.summary.is_some() || n.summary_of_revision.is_some();
    if has_summary || n.successor_id.is_some() {
        return Err(invalid(
            "Create without summary or successor; use explicit revision operations",
        ));
    }
    Ok(())
}

fn validate_task_fields(n: &Node) -> Result<()> {
    let invalid_task_fields = match n.kind {
        NodeKind::Task => n.task_state.is_none(),
        NodeKind::Knowledge => n.task_state.is_some() || !n.acceptance_criteria.is_empty(),
    };
    if invalid_task_fields {
        return Err(invalid(
            "Task lifecycle and acceptance criteria apply only to tasks",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod shared_policy_tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;
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
