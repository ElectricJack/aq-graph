//! Regenerate ONLY the additive graph-committed-observation-1 profile.
use aq_graph::observation::*;
use std::{error::Error, fs, path::Path};
use uuid::Uuid;
fn write(path: &Path, name: &str, value: &impl serde::Serialize) -> Result<(), Box<dyn Error>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path.join(name), bytes)?;
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args().nth(1).ok_or("output directory required")?;
    let path = Path::new(&dir);
    fs::create_dir_all(path)?;
    let scope = Scope {
        workspace_id: Uuid::from_u128(1),
        stream_id: Uuid::from_u128(2),
        generation: "1".into(),
    };
    let cursor = Cursor {
        scope: scope.clone(),
        committed_sequence: "0".into(),
        group_id: None,
    };
    let node = Uuid::from_u128(3);
    let req = SnapshotRequest {
        contract: Contract::V1,
        targets: vec![SnapshotTarget::Task { task_id: node }],
    };
    let content = SnapshotContent {
        scope: scope.clone(),
        captured_cursor: cursor.clone(),
        objects: vec![ObservedObject::Node {
            id: node,
            value: Some(NodeProjection {
                id: node,
                kind: NodeKind::Task,
                version: "1".into(),
                logical_revision: "1".into(),
                task_state: Some(TaskState::Active),
                deleted: false,
                retired: false,
            }),
        }],
    };
    let snapshot = Snapshot {
        snapshot_digest: digest(&content)?,
        content,
    };
    write(path, "schema.json", &schema())?;
    write(path, "snapshot-request.json", &req)?;
    write(
        path,
        "snapshot-response.json",
        &SnapshotRead {
            contract: Contract::V1,
            request_digest: digest(&req)?,
            status: Status::Available,
            scope: scope.clone(),
            snapshot: Some(snapshot),
            gap: None,
        },
    )?;
    write(
        path,
        "register-request.json",
        &RegisterRequest {
            contract: Contract::V1,
            subscriber_id: Uuid::from_u128(4),
            condition_digest: format!("sha256:{}", "a".repeat(64)),
            expected_scope: scope,
            from_cursor: Some(cursor),
            original_expires_at_ms: "2000000000000".into(),
            selectors: vec![Selector::Task {
                member_id: "dependency-0".into(),
                task_id: node,
                starting_logical_revision: "1".into(),
            }],
        },
    )?;
    Ok(())
}
