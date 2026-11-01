use super::*;
use crate::{domain::Principal, store::Store};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
type Tx<'a> = Transaction<'a, Postgres>;
struct Head {
    scope: Scope,
    sequence: i64,
    group: Option<Uuid>,
    floor: i64,
    coverage: String,
    gap_start: Option<i64>,
    gap_reason: Option<String>,
    controls: i64,
    control_bytes: i64,
}
impl Head {
    fn cursor(&self) -> Cursor {
        Cursor {
            scope: self.scope.clone(),
            committed_sequence: self.sequence.to_string(),
            group_id: self.group,
        }
    }
    fn gap(&self, after: Option<Cursor>, reason: &str) -> Gap {
        Gap {
            reason: match reason {
                "journal_capacity" => GapReason::JournalCapacity,
                "transaction_fact_limit" => GapReason::TransactionFactLimit,
                "domain_invariant" => GapReason::DomainInvariant,
                "retention" => GapReason::Retention,
                _ => GapReason::CoverageGap,
            },
            requested_after: after.clone(),
            lost_from_sequence: self
                .gap_start
                .unwrap_or(self.floor.saturating_add(1))
                .to_string(),
            retained_floor: self.floor.to_string(),
            through: self.cursor(),
        }
    }
}
async fn begin<'a>(s: &'a Store, p: &Principal, c: &CredentialIdentity) -> Result<Tx<'a>> {
    let mut tx = s.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='4s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='5s'")
        .execute(&mut *tx)
        .await?;
    // Exact authenticated token, not merely the stale actor/capability label.
    // Lock only this credential; snapshot domain reads below NEVER lock node/edge rows.
    let actual:Option<String>=sqlx::query_scalar("SELECT capability FROM credentials WHERE workspace_id=$1 AND actor_id=$2 AND token_hash=$3 FOR SHARE").bind(p.workspace_id).bind(&p.actor_id).bind(&c.0).fetch_optional(&mut *tx).await?;
    match actual.as_deref() {
        Some("read" | "write" | "admin") => {}
        _ => return Err(Error::Unauthorized),
    }
    Ok(tx)
}
async fn head(tx: &mut Tx<'_>, w: Uuid, lock: bool) -> Result<Head> {
    let query = if lock {
        "SELECT * FROM graph_observation_heads WHERE workspace_id=$1 FOR UPDATE"
    } else {
        "SELECT * FROM graph_observation_heads WHERE workspace_id=$1"
    };
    let r = sqlx::query(query)
        .bind(w)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| bad("observation owner unsupported"))?;
    Ok(Head {
        scope: Scope {
            workspace_id: w,
            stream_id: r.get("stream_id"),
            generation: r.get::<i64, _>("generation").to_string(),
        },
        sequence: r.get("sequence"),
        group: r.get("group_id"),
        floor: r.get("retained_floor"),
        coverage: r.get("coverage"),
        gap_start: r.get("gap_start"),
        gap_reason: r.get("gap_reason"),
        controls: r.get("control_count"),
        control_bytes: r.get("control_bytes"),
    })
}
async fn validate_anchor(tx: &mut Tx<'_>, h: &Head, cursor: &Cursor) -> Result<()> {
    cursor.validate()?;
    check(cursor.scope == h.scope, "observation cursor scope mismatch")?;
    let seq = decimal(&cursor.committed_sequence, false)?;
    check(
        seq >= h.floor && seq <= h.sequence,
        "observation cursor retained range",
    )?;
    if seq > 0 {
        let anchor:Option<Uuid>=sqlx::query_scalar("SELECT group_id FROM graph_observation_batches WHERE workspace_id=$1 AND generation=$2 AND sequence=$3").bind(h.scope.workspace_id).bind(decimal(&h.scope.generation,true)?).bind(seq).fetch_optional(&mut **tx).await?;
        check(
            anchor == cursor.group_id,
            "observation cursor anchor mismatch",
        )?;
    }
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| Error::Internal("invalid stored observation data".into()))
}
fn bounded<T: Serialize>(v: &T, max: usize) -> Result<()> {
    check(canonical(v)?.len() <= max, "observation response limit")
}
async fn snapshot(tx: &mut Tx<'_>, h: &Head, selectors: &[SnapshotTarget]) -> Result<Snapshot> {
    let mut targets = std::collections::BTreeSet::new();
    for s in selectors {
        match s {
            SnapshotTarget::Task { task_id } => {
                targets.insert(("node", *task_id));
            }
            SnapshotTarget::Dependency {
                edge_id,
                prerequisite_id,
                dependent_id,
                ..
            } => {
                targets.insert(("edge", *edge_id));
                targets.insert(("node", *prerequisite_id));
                targets.insert(("node", *dependent_id));
            }
        }
    }
    let mut objects = vec![];
    for (kind, id) in targets {
        if kind == "node" {
            let value:Option<Value>=sqlx::query_scalar("SELECT graph_observation_node_projection(n) FROM nodes n WHERE workspace_id=$1 AND id=$2").bind(h.scope.workspace_id).bind(id).fetch_optional(&mut **tx).await?;
            objects.push(ObservedObject::Node {
                id,
                value: value.map(decode).transpose()?,
            });
        } else {
            let value:Option<Value>=sqlx::query_scalar("SELECT graph_observation_edge_projection(e) FROM edges e WHERE workspace_id=$1 AND id=$2").bind(h.scope.workspace_id).bind(id).fetch_optional(&mut **tx).await?;
            objects.push(ObservedObject::Edge {
                id,
                value: value.map(decode).transpose()?,
            });
        }
    }
    let content = SnapshotContent {
        scope: h.scope.clone(),
        captured_cursor: h.cursor(),
        objects,
    };
    Ok(Snapshot {
        snapshot_digest: digest(&content)?,
        content,
    })
}
fn registration(h: &Head, status: Status, receipt: Option<Receipt>) -> Registration {
    Registration {
        contract: Contract::V1,
        status,
        scope: h.scope.clone(),
        receipt,
        gap: None,
    }
}
fn retryable(e: &Error) -> bool {
    matches!(e,Error::Database(sqlx::Error::Database(d))if matches!(d.code().as_deref(),Some("40001"|"40P01")))
}
impl Store {
    pub(crate) async fn observation_capabilities(
        &self,
        p: &Principal,
        c: &CredentialIdentity,
    ) -> Result<Capabilities> {
        let mut tx = begin(self, p, c).await?;
        let h = head(&mut tx, p.workspace_id, false).await?;
        let out = Capabilities {
            contract: Contract::V1,
            scope: h.scope.clone(),
            available: h.coverage == "complete",
            coverage: if h.coverage == "complete" {
                Status::Available
            } else {
                Status::Gap
            },
            head: h.cursor(),
            retained_floor: h.floor.to_string(),
            limits: Limits {
                selectors: MAX_SELECTORS,
                groups: MAX_GROUPS,
                request_bytes: MAX_REQUEST,
                response_bytes: MAX_RESPONSE,
                journal_bytes: 16_777_216,
                journal_batches: 32768,
                journal_facts: 65536,
                facts_per_commit: 4096,
                retained_controls: MAX_CONTROLS,
                actor_retained_controls: MAX_ACTOR_CONTROLS,
                control_bytes: MAX_CONTROL_BYTES,
                max_lease_ms: MAX_LEASE_MS,
                retry_retention_ms: RETRY_RETENTION_MS,
            },
            authority: Authority::None,
        };
        tx.commit().await?;
        Ok(out)
    }
    pub(crate) async fn observation_snapshot(
        &self,
        p: &Principal,
        c: &CredentialIdentity,
        req: &SnapshotRequest,
    ) -> Result<SnapshotRead> {
        req.validate()?;
        for attempt in 0..8 {
            let result = async {
                let mut tx = begin(self, p, c).await?;
                let h = head(&mut tx, p.workspace_id, true).await?;
                // A writer owns node -> head. We only take head -> plain MVCC reads.
                // RR rejects a stale head acquisition; retry the WHOLE boundary.
                let mut out = SnapshotRead {
                    contract: Contract::V1,
                    request_digest: digest(req)?,
                    status: Status::Available,
                    scope: h.scope.clone(),
                    snapshot: None,
                    gap: None,
                };
                if h.coverage != "complete" {
                    out.status = Status::Gap;
                    out.gap = Some(h.gap(None, h.gap_reason.as_deref().unwrap_or("coverage_gap")));
                } else {
                    out.snapshot = Some(snapshot(&mut tx, &h, &req.targets).await?);
                }
                bounded(&out, MAX_RESPONSE)?;
                tx.commit().await?;
                Ok(out)
            }
            .await;
            match result {
                Err(e) if retryable(&e) && attempt < 7 => {
                    tokio::time::sleep(std::time::Duration::from_millis(16 << attempt.min(4)))
                        .await;
                }
                Err(e) if retryable(&e) => return Err(Error::Overloaded),
                result => return result,
            }
        }
        Err(Error::Overloaded)
    }
    pub(crate) async fn observation_register(
        &self,
        p: &Principal,
        c: &CredentialIdentity,
        req: &RegisterRequest,
        now: i64,
    ) -> Result<Registration> {
        req.validate(p.workspace_id, now)?;
        for attempt in 0..8 {
            match self.observation_register_once(p, c, req, now).await {
                Err(e) if retryable(&e) && attempt < 7 => {
                    tokio::time::sleep(std::time::Duration::from_millis(16 << attempt.min(4))).await
                }
                Err(e) if retryable(&e) => return Err(Error::Overloaded),
                v => return v,
            }
        }
        Err(Error::Overloaded)
    }
    async fn observation_register_once(
        &self,
        p: &Principal,
        c: &CredentialIdentity,
        req: &RegisterRequest,
        now: i64,
    ) -> Result<Registration> {
        let mut tx = begin(self, p, c).await?;
        let mut h = head(&mut tx, p.workspace_id, true).await?;
        if req.expected_scope != h.scope {
            return Ok(registration(&h, Status::SourceChanged, None));
        }
        let request_digest = digest(req)?;
        let old=sqlx::query("SELECT request_digest,receipt,released,original_expires_at_ms FROM graph_observation_subscriptions WHERE workspace_id=$1 AND actor_id=$2 AND subscriber_id=$3").bind(p.workspace_id).bind(&p.actor_id).bind(req.subscriber_id).fetch_optional(&mut *tx).await?;
        if let Some(row) = old {
            check(
                row.get::<String, _>("request_digest") == raw_digest(&request_digest)?,
                "observation subscriber payload conflict",
            )?;
            let status = if h.coverage != "complete" {
                Status::Gap
            } else if row.get::<bool, _>("released") {
                Status::Released
            } else if now >= row.get::<i64, _>("original_expires_at_ms") {
                Status::Expired
            } else {
                Status::Available
            };
            let mut out = registration(&h, status, Some(decode(row.get("receipt"))?));
            if status == Status::Gap {
                out.gap = Some(h.gap(None, h.gap_reason.as_deref().unwrap_or("coverage_gap")));
            }
            bounded(&out, MAX_RESPONSE)?;
            tx.commit().await?;
            return Ok(out);
        }
        if decimal(&req.original_expires_at_ms, true)? <= now {
            return Ok(registration(&h, Status::Expired, None));
        }
        if h.coverage != "complete" {
            let mut out = registration(&h, Status::Gap, None);
            out.gap = Some(h.gap(None, h.gap_reason.as_deref().unwrap_or("coverage_gap")));
            return Ok(out);
        }
        // Controls, including retained receipts/released tombstones, are finite. Cleanup
        // only runs after the original lease AND published retry horizon have elapsed.
        let removed=sqlx::query("WITH doomed AS (SELECT workspace_id,actor_id,subscriber_id FROM graph_observation_subscriptions WHERE workspace_id=$1 AND retain_until_ms<$2 ORDER BY retain_until_ms,subscriber_id LIMIT 32), gone AS (DELETE FROM graph_observation_subscriptions s USING doomed d WHERE s.workspace_id=d.workspace_id AND s.actor_id=d.actor_id AND s.subscriber_id=d.subscriber_id RETURNING s.retained_bytes) SELECT count(*) AS n,COALESCE(sum(retained_bytes),0)::bigint AS bytes FROM gone").bind(p.workspace_id).bind(now).fetch_one(&mut *tx).await?;
        let n: i64 = removed.get("n");
        let bytes: i64 = removed.get("bytes");
        if n > 0 {
            sqlx::query("UPDATE graph_observation_heads SET control_count=control_count-$2,control_bytes=control_bytes-$3 WHERE workspace_id=$1").bind(p.workspace_id).bind(n).bind(bytes).execute(&mut *tx).await?;
            h.controls -= n;
            h.control_bytes -= bytes;
        }
        let actor_count:i64=sqlx::query_scalar("SELECT count(*) FROM graph_observation_subscriptions WHERE workspace_id=$1 AND actor_id=$2").bind(p.workspace_id).bind(&p.actor_id).fetch_one(&mut *tx).await?;
        if h.controls >= MAX_CONTROLS || actor_count >= MAX_ACTOR_CONTROLS {
            tx.commit().await?;
            return Err(Error::Overloaded);
        }
        if let Some(cursor) = &req.from_cursor {
            if decimal(&cursor.committed_sequence, false)? < h.floor {
                let mut out = registration(&h, Status::Gap, None);
                out.gap = Some(h.gap(Some(cursor.clone()), "retention"));
                return Ok(out);
            }
        }
        let initial_cursor = if let Some(cursor) = &req.from_cursor {
            validate_anchor(&mut tx, &h, cursor).await?;
            cursor.clone()
        } else {
            h.cursor()
        };
        let content = ReceiptContent {
            contract: Contract::V1,
            subscriber_id: req.subscriber_id,
            actor_id: p.actor_id.clone(),
            request_digest: request_digest.clone(),
            condition_digest: req.condition_digest.clone(),
            selectors: req.selectors.clone(),
            scope: h.scope.clone(),
            original_expires_at_ms: req.original_expires_at_ms.clone(),
            snapshot: snapshot(
                &mut tx,
                &h,
                &req.selectors
                    .iter()
                    .map(SnapshotTarget::from)
                    .collect::<Vec<_>>(),
            )
            .await?,
            initial_cursor,
            revision_kind: RevisionKind::GraphLogicalRecordRevision1,
        };
        let receipt_digest = digest(&content)?;
        let receipt = Receipt {
            content,
            reference: Reference {
                kind: ReferenceKind::Subscription,
                id: format!(
                    "aq.graph.observation.subscription/{}",
                    raw_digest(&receipt_digest)?
                ),
                version: ReferenceVersion::V1,
                digest: receipt_digest.clone(),
            },
        };
        bounded(&receipt, MAX_RECEIPT_BYTES)?;
        let request = canonical(req)?;
        let retained = 128 + request.len() + canonical(&receipt)?.len();
        if h.control_bytes + retained as i64 > MAX_CONTROL_BYTES {
            tx.commit().await?;
            return Err(Error::Overloaded);
        }
        let expires = decimal(&req.original_expires_at_ms, true)?;
        let retain_until = expires
            .checked_add(RETRY_RETENTION_MS)
            .ok_or_else(|| bad("observation retention time"))?;
        sqlx::query("INSERT INTO graph_observation_subscriptions(workspace_id,actor_id,subscriber_id,request_digest,receipt_digest,request,receipt,original_expires_at_ms,retain_until_ms,retained_bytes) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(p.workspace_id).bind(&p.actor_id).bind(req.subscriber_id).bind(raw_digest(&request_digest)?).bind(raw_digest(&receipt_digest)?).bind(serde_json::to_value(req).map_err(|_|bad("observation request encoding"))?).bind(serde_json::to_value(&receipt).map_err(|_|bad("observation receipt encoding"))?).bind(expires).bind(retain_until).bind(retained as i64).execute(&mut *tx).await?;
        sqlx::query("UPDATE graph_observation_heads SET control_count=control_count+1,control_bytes=control_bytes+$2 WHERE workspace_id=$1").bind(p.workspace_id).bind(retained as i64).execute(&mut *tx).await?;
        let out = registration(&h, Status::Available, Some(receipt));
        bounded(&out, MAX_RESPONSE)?;
        tx.commit().await?;
        Ok(out)
    }
    pub(crate) async fn observation_pull(
        &self,
        p: &Principal,
        c: &CredentialIdentity,
        id: Uuid,
        q: &PullQuery,
        now: i64,
    ) -> Result<Pull> {
        let (limit, max_bytes) = q.limits()?;
        let mut tx = begin(self, p, c).await?;
        let h = head(&mut tx, p.workspace_id, false).await?;
        let row=sqlx::query("SELECT receipt,receipt_digest,request_digest,released,original_expires_at_ms FROM graph_observation_subscriptions WHERE workspace_id=$1 AND actor_id=$2 AND subscriber_id=$3").bind(p.workspace_id).bind(&p.actor_id).bind(id).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
        if let Some(expected) = &q.request_digest {
            check(
                raw_digest(expected)? == row.get::<String, _>("request_digest"),
                "observation request reference mismatch",
            )?;
        }
        if let Some(expected) = &q.receipt_digest {
            check(
                raw_digest(expected)? == row.get::<String, _>("receipt_digest"),
                "observation receipt reference mismatch",
            )?;
        }
        let receipt: Receipt = decode(row.get("receipt"))?;
        let after = if let Some(cursor) = &q.after {
            check(
                q.receipt_digest.is_some(),
                "observation exact receipt required for replay",
            )?;
            Cursor::decode(cursor)?
        } else {
            receipt.content.initial_cursor.clone()
        };
        let mut out = Pull {
            contract: Contract::V1,
            lookup_only: q.after.is_none(),
            status: Status::Available,
            receipt,
            scope: h.scope.clone(),
            after: after.clone(),
            through: h.cursor(),
            groups: vec![],
            next_cursor: None,
            retained_floor: h.floor.to_string(),
            gap: None,
        };
        if after.scope != h.scope || out.receipt.content.scope != h.scope {
            out.status = Status::SourceChanged;
            bounded(&out, max_bytes)?;
            tx.commit().await?;
            return Ok(out);
        }
        if row.get::<bool, _>("released") {
            out.status = Status::Released;
        } else if now >= row.get::<i64, _>("original_expires_at_ms") {
            out.status = Status::Expired;
        }
        if out.status != Status::Available {
            bounded(&out, max_bytes)?;
            tx.commit().await?;
            return Ok(out);
        }
        let sequence = decimal(&after.committed_sequence, false)?;
        check(
            sequence
                >= decimal(
                    &out.receipt.content.initial_cursor.committed_sequence,
                    false,
                )?,
            "observation replay precedes original subscription boundary",
        )?;
        check(sequence <= h.sequence, "observation future cursor")?;
        if sequence < h.floor || h.coverage != "complete" {
            out.status = Status::Gap;
            out.gap = Some(h.gap(Some(after), h.gap_reason.as_deref().unwrap_or("retention")));
            bounded(&out, max_bytes)?;
            tx.commit().await?;
            return Ok(out);
        }
        if sequence > 0 {
            let anchor:Option<Uuid>=sqlx::query_scalar("SELECT group_id FROM graph_observation_batches WHERE workspace_id=$1 AND generation=$2 AND sequence=$3").bind(p.workspace_id).bind(decimal(&h.scope.generation,true)?).bind(sequence).fetch_optional(&mut *tx).await?;
            check(
                anchor == after.group_id,
                "observation cursor anchor mismatch",
            )?;
        }
        // No-after is an exact original receipt lookup, never a replacement snapshot.
        if q.after.is_none() {
            check(
                q.request_digest.is_some() && q.through.is_none(),
                "observation exact request required for receipt lookup",
            )?;
            out.through = out.receipt.content.initial_cursor.clone();
            bounded(&out, max_bytes)?;
            tx.commit().await?;
            return Ok(out);
        }
        if let Some(through) = &q.through {
            let cursor = Cursor::decode(through)?;
            validate_anchor(&mut tx, &h, &cursor).await?;
            check(
                decimal(&cursor.committed_sequence, false)? >= sequence,
                "observation through precedes after",
            )?;
            out.through = cursor;
        }
        let through_sequence = decimal(&out.through.committed_sequence, false)?;
        let ids: Vec<Uuid> = out
            .receipt
            .content
            .selectors
            .iter()
            .flat_map(Selector::ids)
            .collect();
        let batches=sqlx::query("SELECT b.sequence,b.group_id FROM graph_observation_batches b WHERE b.workspace_id=$1 AND b.generation=$2 AND b.sequence>$3 AND b.sequence<=$4 AND (b.kind='gap' OR EXISTS(SELECT 1 FROM graph_observation_facts f WHERE f.workspace_id=b.workspace_id AND f.generation=b.generation AND f.sequence=b.sequence AND f.object_id=ANY($5))) ORDER BY b.sequence LIMIT $6").bind(p.workspace_id).bind(decimal(&h.scope.generation,true)?).bind(sequence).bind(through_sequence).bind(&ids).bind((limit+1)as i64).fetch_all(&mut *tx).await?;
        let mut cut = false;
        for batch in &batches {
            if out.groups.len() == limit {
                cut = true;
                break;
            }
            let seq: i64 = batch.get("sequence");
            let group_id: Uuid = batch.get("group_id");
            let rows=sqlx::query("SELECT object_kind,object_id,before_fact,after_fact,mutation_count,transition_class FROM graph_observation_facts WHERE workspace_id=$1 AND generation=$2 AND sequence=$3 AND object_id=ANY($4) ORDER BY object_kind,object_id").bind(p.workspace_id).bind(decimal(&h.scope.generation,true)?).bind(seq).bind(&ids).fetch_all(&mut *tx).await?;
            let mut facts = vec![];
            for r in rows {
                let kind: String = r.get("object_kind");
                let object_id = r.get("object_id");
                let b: Option<Value> = r.get("before_fact");
                let a: Option<Value> = r.get("after_fact");
                let mutation_count = r.get::<i64, _>("mutation_count").to_string();
                let transition_class: TransitionClass =
                    decode(Value::String(r.get("transition_class")))?;
                facts.push(if kind == "node" {
                    Fact::Node {
                        object_id,
                        before: b.map(decode).transpose()?,
                        after: a.map(decode).transpose()?,
                        mutation_count,
                        transition_class,
                    }
                } else {
                    Fact::Edge {
                        object_id,
                        before: b.map(decode).transpose()?,
                        after: a.map(decode).transpose()?,
                        mutation_count,
                        transition_class,
                    }
                });
            }
            out.groups.push(CommitGroup {
                scope: h.scope.clone(),
                committed_sequence: seq.to_string(),
                group_id,
                facts,
            });
            if canonical(&out)?.len() > max_bytes {
                out.groups.pop();
                check(
                    !out.groups.is_empty(),
                    "observation complete group exceeds page limit",
                )?;
                cut = true;
                break;
            }
        }
        out.next_cursor = Some(if cut {
            let last = out
                .groups
                .last()
                .ok_or_else(|| bad("observation page cannot advance"))?;
            Cursor {
                scope: h.scope.clone(),
                committed_sequence: last.committed_sequence.clone(),
                group_id: Some(last.group_id),
            }
        } else {
            out.through.clone()
        });
        bounded(&out, max_bytes)?;
        tx.commit().await?;
        Ok(out)
    }
    pub(crate) async fn observation_release(
        &self,
        p: &Principal,
        c: &CredentialIdentity,
        id: Uuid,
        receipt_digest: &str,
    ) -> Result<()> {
        let mut tx = begin(self, p, c).await?;
        head(&mut tx, p.workspace_id, true).await?;
        let changed=sqlx::query("UPDATE graph_observation_subscriptions SET released=true WHERE workspace_id=$1 AND actor_id=$2 AND subscriber_id=$3 AND receipt_digest=$4").bind(p.workspace_id).bind(&p.actor_id).bind(id).bind(raw_digest(receipt_digest)?).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(Error::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
}
