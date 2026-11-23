//! Apply changes to the user-editable fields of a record.
use super::*;
pub(super) fn patch_node(n: &mut Node, patch: &NodePatch) -> Result<()> {
    if n.kind == NodeKind::Knowledge && patch.has_task_fields() {
        return Err(Error::Validation(
            "task fields cannot be set on knowledge".into(),
        ));
    }
    apply_content(n, patch);
    if patch.verification.is_some() {
        return Err(Error::Validation(
            "use verify_node to change verification".into(),
        ));
    }
    if let Some(v) = &patch.retirement_reason {
        n.retirement_reason = Some(v.clone());
    }
    n.retired = patch.retired.unwrap_or(n.retired);
    replace_if_present(&mut n.provenance, &patch.provenance);
    patch.valid_from.apply_to(&mut n.valid_from);
    patch.valid_until.apply_to(&mut n.valid_until);
    patch.review_at.apply_to(&mut n.review_at);
    patch.successor_id.apply_to(&mut n.successor_id);
    Ok(())
}

fn replace_if_present<T: Clone>(current: &mut T, replacement: &Option<T>) {
    if let Some(value) = replacement {
        current.clone_from(value);
    }
}

fn apply_content(n: &mut Node, patch: &NodePatch) {
    replace_if_present(&mut n.title, &patch.title);
    replace_if_present(&mut n.body, &patch.body);
    replace_if_present(&mut n.acceptance_criteria, &patch.acceptance_criteria);
    replace_if_present(&mut n.labels, &patch.labels);
    replace_if_present(&mut n.metadata, &patch.metadata);
    n.task_state = patch.task_state.or(n.task_state);
    n.content_type = patch.content_type.or(n.content_type);
}

impl NodePatch {
    pub(super) fn changes_retirement(&self) -> bool {
        self.retired.is_some() || self.retirement_reason.is_some() || !self.successor_id.is_keep()
    }
    fn has_task_fields(&self) -> bool {
        self.task_state.is_some() || self.acceptance_criteria.is_some()
    }
    fn has_dates(&self) -> bool {
        !self.valid_from.is_keep() || !self.valid_until.is_keep() || !self.review_at.is_keep()
    }
    fn has_content(&self) -> bool {
        self.title.is_some()
            || self.body.is_some()
            || self.labels.is_some()
            || self.metadata.is_some()
    }
    fn has_evidence(&self) -> bool {
        self.content_type.is_some() || self.verification.is_some() || self.provenance.is_some()
    }
    pub(super) fn retirement_metadata_only(&self) -> bool {
        let content_change = self.has_content() || self.has_evidence() || self.has_task_fields();
        !content_change && !self.has_dates() && self.retired != Some(false)
    }
}
