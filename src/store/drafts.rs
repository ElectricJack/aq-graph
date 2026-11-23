//! Draft review, rollback-only previews and atomic application.
use super::*;
impl Store {
    pub async fn create_draft(&self, p: &Principal, req: DraftRequest) -> Result<Value> {
        p.require_write()?;
        check_key(&req.idempotency_key)?;
        check_operations(&req.operations)?;
        if req.name.trim().is_empty() || req.name.len() > 200 {
            return Err(Error::Validation(
                "draft name must contain 1..200 bytes".into(),
            ));
        }
        let payload =
            hash(&serde_json::to_vec(&req).map_err(|e| Error::Validation(e.to_string()))?);
        retry_write(|| async {
            let mut tx = self.write_tx_with(false).await?;
            live_authority(&mut tx, p, p.capability == Capability::Admin).await?;
            let receipt = Receipt {
                principal: p,
                operation: "create_draft",
                key: &req.idempotency_key,
                payload: &payload,
            };
            if let Some(v) = receipt.load_or_reserve(&mut tx).await? {
                return Ok(v);
            }
            let id = Uuid::new_v4();
            // Assign proposed identities once so preview and application resolve identical nodes.
            let operations = assign_ids(&req.operations)?;
            Journal {
                principal: p,
                change: id,
                reason: req.reason.as_deref(),
            }
            .insert(
                &mut tx,
                ChangeSet {
                    name: Some(&req.name),
                    operations: &operations,
                    status: "draft",
                },
            )
            .await?;
            let result = load_draft(&mut tx, p.workspace_id, id).await?;
            check_response(&result)?;
            receipt.save(&mut tx, result.clone()).await?;
            tx.commit().await?;
            Ok(result)
        })
        .await
    }
    pub async fn get_draft(&self, p: &Principal, id: Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        load_draft(&mut tx, p.workspace_id, id).await
    }
    pub async fn validate_draft(&self, p: &Principal, id: Uuid) -> Result<Value> {
        p.require_write()?;
        retry_write(|| async {
            let mut tx = self.write_tx().await?;
            live_authority(&mut tx, p, p.capability == Capability::Admin).await?;
            let draft = load_draft(&mut tx, p.workspace_id, id).await?;
            if draft["status"] != "draft" {
                return Err(Error::Conflict(
                    "change set has already been applied".into(),
                ));
            }
            let operations: Vec<Mutation> = decode(draft["operations"].clone())?;
            let before = preview::before(&mut tx, p.workspace_id, &operations).await?;
            let results = MutationBatch::new(p, id, draft["reason"].as_str(), true)
                .run(&mut tx, &operations)
                .await?;
            let preview = preview::after(&mut tx, p.workspace_id, id, before).await?;
            tx.rollback().await?;
            Ok(json!({"valid":true,"change_set_id":id,"results":results,"preview":preview}))
        })
        .await
    }
    pub async fn apply_draft(&self, p: &Principal, id: Uuid, key: &str) -> Result<MutationResult> {
        p.require_write()?;
        check_key(key)?;
        let operation = "apply_draft";
        let payload = hash(id.as_bytes());
        retry_write(|| async {
            let mut tx = self.write_tx().await?;
            live_authority(&mut tx, p, p.capability == Capability::Admin).await?;
            let policy_draft = load_draft(&mut tx, p.workspace_id, id).await?;
            let policy_operations: Vec<Mutation> = decode(policy_draft["operations"].clone())?;
            if touches_knowledge(&mut tx, p, &policy_operations).await?
                || requires_admin(&mut tx, p, &policy_operations).await?
            {
                p.require_admin()?;
                live_authority(&mut tx, p, true).await?;
            }
            let receipt = Receipt {
                principal: p,
                operation,
                key,
                payload: &payload,
            };
            if let Some(v) = receipt.load_or_reserve(&mut tx).await? {
                return decode(v);
            }
            let draft = load_draft(&mut tx, p.workspace_id, id).await?;
            if draft["status"] != "draft" {
                return Err(Error::Conflict(
                    "change set has already been applied".into(),
                ));
            }
            let operations: Vec<Mutation> = decode(draft["operations"].clone())?;
            let results = MutationBatch::new(p, id, draft["reason"].as_str(), false)
                .run(&mut tx, &operations)
                .await?;
            let response = MutationResult {
                change_set_id: id,
                results,
            };
            check_response(&response)?;
            save_result(&mut tx, id, &response).await?;
            receipt.save(&mut tx, encode(&response)?).await?;
            tx.commit().await?;
            Ok(response)
        })
        .await
    }
}
async fn load_draft(tx: &mut Tx<'_>, workspace: Uuid, id: Uuid) -> Result<Value> {
    sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(c) FROM change_sets c WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::NotFound)
}
fn assign_ids(ops: &[Mutation]) -> Result<Vec<Mutation>> {
    let mut value = encode(&ops)?;
    for op in value
        .as_array_mut()
        .ok_or_else(|| Error::Internal("invalid operations".into()))?
    {
        if matches!(op["op"].as_str(), Some("create_node" | "add_edge")) && op["id"].is_null() {
            op["id"] = json!(Uuid::new_v4());
        }
    }
    decode(value)
}
