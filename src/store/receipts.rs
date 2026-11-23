//! Idempotency receipts for mutations and reviewable changesets.
use super::*;
pub(super) struct Receipt<'a> {
    pub principal: &'a Principal,
    pub operation: &'a str,
    pub key: &'a str,
    pub payload: &'a str,
}
impl Receipt<'_> {
    pub(super) async fn load_or_reserve(&self, tx: &mut Tx<'_>) -> Result<Option<Value>> {
        if let Some(result) = self.load(tx).await? {
            return Ok(Some(result));
        }
        // Reserving the unique receipt is also a transaction-scoped key lock. An
        // overlapping SERIALIZABLE contender retries with a fresh snapshot after commit.
        sqlx::query("INSERT INTO idempotency_receipts(workspace_id,actor_id,operation,key,payload_hash,result) VALUES($1,$2,$3,$4,'','null') ON CONFLICT(workspace_id,actor_id,operation,key) DO UPDATE SET key=EXCLUDED.key").bind(self.principal.workspace_id).bind(&self.principal.actor_id).bind(self.operation).bind(self.key).execute(&mut **tx).await?;
        self.load(tx).await
    }
    pub(super) async fn load(&self, tx: &mut Tx<'_>) -> Result<Option<Value>> {
        let row=sqlx::query("SELECT payload_hash,result FROM idempotency_receipts WHERE workspace_id=$1 AND actor_id=$2 AND operation=$3 AND key=$4").bind(self.principal.workspace_id).bind(&self.principal.actor_id).bind(self.operation).bind(self.key).fetch_optional(&mut **tx).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let old: String = row.get("payload_hash");
        if old.is_empty() {
            return Ok(None);
        }
        if old != self.payload {
            return Err(Error::Conflict(
                "idempotency key was used with a different payload".into(),
            ));
        }
        Ok(Some(row.get("result")))
    }
    pub(super) async fn save(&self, tx: &mut Tx<'_>, result: Value) -> Result<()> {
        sqlx::query("UPDATE idempotency_receipts SET payload_hash=$5,result=$6 WHERE workspace_id=$1 AND actor_id=$2 AND operation=$3 AND key=$4").bind(self.principal.workspace_id).bind(&self.principal.actor_id).bind(self.operation).bind(self.key).bind(self.payload).bind(result).execute(&mut **tx).await?;
        Ok(())
    }
}
