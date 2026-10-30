//! Shared current-state list/search predicates for tasks and knowledge.
use super::*;

impl Store {
    /// Lists retain disputed records; retirement is an explicit opt-in for both kinds.
    pub async fn list_filtered(
        &self,
        p: &Principal,
        filters: NodeFilters,
        q: ListQuery,
    ) -> Result<Page<Node>> {
        q.validate()?;
        let n = limit(&q)?;
        let values=sqlx::query_scalar::<_,Value>(
            "SELECT to_jsonb(n)-'search_document' FROM nodes n
             WHERE workspace_id=$1 AND NOT deleted
               AND ($2::text IS NULL OR kind=$2)
               AND ($3 OR NOT retired)
               AND ($4::timestamptz IS NULL OR ((valid_from IS NULL OR valid_from <= $4) AND (valid_until IS NULL OR valid_until > $4)))
               AND ($5::timestamptz IS NULL OR (review_at IS NOT NULL AND review_at <= $5))
               AND ($6::uuid IS NULL OR id>$6)
             ORDER BY id LIMIT $7")
            .bind(p.workspace_id)
            .bind(filters.kind.map(|kind|kind.as_str()))
            .bind(filters.include_retired)
            .bind(filters.valid_at.map(|at|chrono::SubsecRound::trunc_subsecs(at,6)))
            .bind(filters.review_due_at.map(|at|chrono::SubsecRound::trunc_subsecs(at,6)))
            .bind(cursor(&q)?)
            .bind(n+1)
            .fetch_all(&self.pool).await?;
        node_page(values, n)
    }

    /// Search both kinds unless explicitly filtered. Select candidates before
    /// ordering/fetching bodies so LIMIT cannot induce a full primary-key walk.
    pub async fn search_nodes(
        &self,
        p: &Principal,
        text: &str,
        filters: NodeFilters,
        q: ListQuery,
    ) -> Result<Page<Node>> {
        q.validate()?;
        let n = limit(&q)?;
        if text.trim().is_empty() || text.len() > 1024 {
            return Err(Error::Validation(
                "search text must contain 1..1024 bytes".into(),
            ));
        }
        let values=sqlx::query_scalar::<_,Value>(
            "WITH matching AS MATERIALIZED (
                SELECT id FROM nodes
                 WHERE workspace_id=$1 AND NOT deleted
                   AND ($2::text IS NULL OR kind=$2)
                   AND ($3 OR NOT retired)
                   AND ($4 OR verification IS DISTINCT FROM 'disputed')
                   AND ($5::timestamptz IS NULL OR ((valid_from IS NULL OR valid_from <= $5) AND (valid_until IS NULL OR valid_until > $5)))
                   AND ($6::timestamptz IS NULL OR (review_at IS NOT NULL AND review_at <= $6))
                   AND search_document @@ websearch_to_tsquery('english',$7)
                   AND ($8::uuid IS NULL OR id>$8)
             ), selected AS MATERIALIZED (SELECT id FROM matching ORDER BY id LIMIT $9)
             SELECT to_jsonb(n)-'search_document' FROM selected
               JOIN nodes n ON n.id=selected.id AND n.workspace_id=$1 ORDER BY selected.id")
            .bind(p.workspace_id)
            .bind(filters.kind.map(|kind|kind.as_str()))
            .bind(filters.include_retired)
            .bind(filters.include_disputed)
            .bind(filters.valid_at.map(|at|chrono::SubsecRound::trunc_subsecs(at,6)))
            .bind(filters.review_due_at.map(|at|chrono::SubsecRound::trunc_subsecs(at,6)))
            .bind(text)
            .bind(cursor(&q)?)
            .bind(n+1)
            .fetch_all(&self.pool).await?;
        node_page(values, n)
    }
}
