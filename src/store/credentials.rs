//! Workspace credential management and transaction-time authority checks.
use super::*;
impl Store {
    pub async fn bootstrap(&self, workspace: Uuid, actor: &str, token: &str) -> Result<()> {
        validate_credential(actor, token)?;
        let mut tx = self.pool.begin().await?;
        // Initialization only: never replace credentials on an existing workspace.
        let inserted = sqlx::query("INSERT INTO workspaces(id) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(workspace)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if inserted == 0 {
            let existing=sqlx::query("SELECT actor_id,capability FROM credentials WHERE workspace_id=$1 AND token_hash=$2").bind(workspace).bind(hash(token.as_bytes())).fetch_optional(&mut *tx).await?;
            if !existing.is_some_and(|r| {
                r.get::<String, _>("actor_id") == actor
                    && r.get::<String, _>("capability") == "admin"
            }) {
                return Err(Error::Conflict(
                    "workspace exists; bootstrap cannot replace its credentials".into(),
                ));
            }
        } else {
            sqlx::query("INSERT INTO credentials(workspace_id,actor_id,token_hash,capability) VALUES($1,$2,$3,'admin')").bind(workspace).bind(actor).bind(hash(token.as_bytes())).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn authenticate(&self, token: &str, workspace: Uuid) -> Result<Principal> {
        let row = sqlx::query(
            "SELECT actor_id,capability FROM credentials WHERE workspace_id=$1 AND token_hash=$2",
        )
        .bind(workspace)
        .bind(hash(token.as_bytes()))
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::Unauthorized)?;
        Ok(Principal {
            actor_id: row.get("actor_id"),
            workspace_id: workspace,
            capability: decode(json!(row.get::<String, _>("capability")))?,
        })
    }
    pub async fn grant(
        &self,
        p: &Principal,
        actor: &str,
        token: &str,
        capability: Capability,
    ) -> Result<()> {
        p.require_admin()?;
        validate_credential(actor, token)?;
        retry_write(|| async {
        let mut tx = self.write_tx().await?;
        live_authority(&mut tx, p, true).await?;
        sqlx::query("INSERT INTO credentials(workspace_id,actor_id,token_hash,capability) VALUES($1,$2,$3,$4) ON CONFLICT(workspace_id,actor_id) DO UPDATE SET token_hash=EXCLUDED.token_hash,capability=EXCLUDED.capability").bind(p.workspace_id).bind(actor).bind(hash(token.as_bytes())).bind(tag(&capability)?).execute(&mut *tx).await.map_err(|e|normalize_db(e.into()))?;
        // Administrators cannot accidentally lock out their entire workspace.
        ensure_admin(&mut tx, p.workspace_id).await?;
        tx.commit().await?;
        Ok(())
        }).await
    }
    pub async fn revoke(&self, p: &Principal, actor: &str) -> Result<()> {
        p.require_admin()?;
        retry_write(|| self.revoke_once(p, actor)).await
    }
    async fn revoke_once(&self, p: &Principal, actor: &str) -> Result<()> {
        let mut tx = self.write_tx().await?;
        live_authority(&mut tx, p, true).await?;
        let n = sqlx::query("DELETE FROM credentials WHERE workspace_id=$1 AND actor_id=$2")
            .bind(p.workspace_id)
            .bind(actor)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(Error::NotFound);
        }
        ensure_admin(&mut tx, p.workspace_id).await?;
        tx.commit().await?;
        Ok(())
    }
}
fn validate_credential(actor: &str, token: &str) -> Result<()> {
    let invalid_actor = actor.trim().is_empty() || actor.len() > 200;
    if invalid_actor || !(32..=4096).contains(&token.len()) {
        return Err(Error::Validation(
            "actor must contain 1..200 bytes and credential 32..4096 bytes".into(),
        ));
    }
    Ok(())
}
pub(super) async fn live_authority(tx: &mut Tx<'_>, p: &Principal, need_admin: bool) -> Result<()> {
    let cap = sqlx::query_scalar::<_, String>(
        "SELECT capability FROM credentials WHERE workspace_id=$1 AND actor_id=$2",
    )
    .bind(p.workspace_id)
    .bind(&p.actor_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::Unauthorized)?;
    let denied = if need_admin {
        cap != "admin"
    } else {
        cap == "read"
    };
    if denied {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn ensure_admin(tx: &mut Tx<'_>, workspace: Uuid) -> Result<()> {
    let yes = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM credentials WHERE workspace_id=$1 AND capability='admin')",
    )
    .bind(workspace)
    .fetch_one(&mut **tx)
    .await?;
    if !yes {
        return Err(Error::Conflict(
            "workspace must retain at least one administrator".into(),
        ));
    }
    Ok(())
}
