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

    /// Probe a bounded UUID prefix for common terms, then use the indexed full
    /// search only if needed. Both branches share one statement snapshot. The
    /// materialized fallback protects rare/missing terms from unbounded PK walks.
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
            "WITH prefix AS MATERIALIZED (
                SELECT id, workspace_id, kind, deleted, retired, verification,
                       valid_from, valid_until, review_at, search_document FROM nodes
                 WHERE workspace_id=$1 AND NOT deleted
                   AND id >= COALESCE($8::uuid, '00000000-0000-0000-0000-000000000000'::uuid)
                   AND ($8::uuid IS NULL OR id > $8)
                 ORDER BY id LIMIT $10
             ), prefix_matches AS MATERIALIZED (
                SELECT id FROM prefix WHERE workspace_id=$1 AND NOT deleted
                   AND ($2::text IS NULL OR kind=$2)
                   AND ($3 OR NOT retired)
                   AND ($4 OR verification IS DISTINCT FROM 'disputed')
                   AND ($5::timestamptz IS NULL OR ((valid_from IS NULL OR valid_from <= $5) AND (valid_until IS NULL OR valid_until > $5)))
                   AND ($6::timestamptz IS NULL OR (review_at IS NOT NULL AND review_at <= $6))
                   AND search_document @@ websearch_to_tsquery('english',$7)
             ), need_fallback AS MATERIALIZED (
                SELECT (SELECT count(*) FROM prefix_matches) < $9
                   AND (SELECT count(*) FROM prefix) = $10 AS needed
             ), matching AS MATERIALIZED (
                SELECT id FROM nodes
                 WHERE (SELECT needed FROM need_fallback)
                   AND workspace_id=$1 AND NOT deleted
                   AND ($2::text IS NULL OR kind=$2)
                   AND ($3 OR NOT retired)
                   AND ($4 OR verification IS DISTINCT FROM 'disputed')
                   AND ($5::timestamptz IS NULL OR ((valid_from IS NULL OR valid_from <= $5) AND (valid_until IS NULL OR valid_until > $5)))
                   AND ($6::timestamptz IS NULL OR (review_at IS NOT NULL AND review_at <= $6))
                   AND search_document @@ websearch_to_tsquery('english',$7)
                   AND id > (SELECT id FROM prefix ORDER BY id DESC LIMIT 1)
             ), selected AS MATERIALIZED (
                SELECT id FROM (SELECT id FROM prefix_matches UNION ALL SELECT id FROM matching) candidates
                 ORDER BY id LIMIT $9
             )
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
            .bind(256_i64.max(n+1))
            .fetch_all(&self.pool).await?;
        node_page(values, n)
    }
}
