use super::*;
use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskListQuery {
    workspace_ids: Vec<Uuid>,
    limit: u16,
    q: Option<String>,
    state: Option<TaskState>,
    before: Option<(DateTime<Utc>, Uuid)>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct TaskList {
    items: Vec<TaskListItem>,
    unavailable_workspaces: Vec<Uuid>,
    has_more: bool,
}

#[derive(Deserialize, Serialize)]
struct TaskListItem {
    id: Uuid,
    workspace_id: Uuid,
    title: String,
    task_state: TaskState,
    updated_at: DateTime<Utc>,
}

impl Store {
    pub(crate) async fn task_list(
        &self,
        token: &str,
        mut query: TaskListQuery,
    ) -> Result<TaskList> {
        if query.workspace_ids.is_empty()
            || query.workspace_ids.len() > 4096
            || query.workspace_ids.iter().any(Uuid::is_nil)
            || !(1..=64).contains(&query.limit)
            || query.before.is_some_and(|(_, id)| id.is_nil())
            || query.q.as_ref().is_some_and(|text| {
                text.trim().is_empty()
                    || text.chars().count() > 256
                    || text.chars().any(char::is_control)
            })
        {
            return Err(Error::Validation("Invalid task list parameters".into()));
        }
        query.workspace_ids.sort_unstable();
        query.workspace_ids.dedup();
        let (before_time, before_id) = query.before.unzip();
        // Authorization and task rows share a statement snapshot. Each workspace
        // contributes at most one page to the global merge, regardless of history size.
        let raw = sqlx::query_scalar::<_, Value>(
            "WITH requested AS (SELECT unnest($1::uuid[]) AS workspace_id),
                  access AS MATERIALIZED (
                      SELECT r.workspace_id, c.workspace_id IS NOT NULL AS allowed
                      FROM requested r LEFT JOIN credentials c
                        ON c.workspace_id=r.workspace_id AND c.token_hash=$2
                  ), selected AS (
                      SELECT task.* FROM (SELECT workspace_id FROM access WHERE allowed) a
                      CROSS JOIN LATERAL (
                          SELECT id, workspace_id, title, task_state, updated_at FROM nodes n
                          WHERE n.workspace_id=a.workspace_id AND kind='task' AND NOT deleted AND NOT retired
                            AND ($3::text IS NULL OR task_state=$3)
                            AND ($4::text IS NULL OR search_document @@ websearch_to_tsquery('english',$4))
                            AND (updated_at,id) < (COALESCE($5::timestamptz,'infinity'),COALESCE($6::uuid,'ffffffff-ffff-ffff-ffff-ffffffffffff'))
                          ORDER BY updated_at DESC, id DESC LIMIT $7
                      ) task ORDER BY task.updated_at DESC, task.id DESC LIMIT $7
                  )
             SELECT jsonb_build_object(
                 'items', COALESCE((SELECT jsonb_agg(to_jsonb(s) ORDER BY updated_at DESC,id DESC) FROM selected s),'[]'),
                 'unavailable_workspaces', ARRAY(SELECT workspace_id FROM access WHERE NOT allowed ORDER BY workspace_id),
                 'has_more', false
             )",
        )
        .bind(&query.workspace_ids)
        .bind(hash(token.as_bytes()))
        .bind(query.state.map(TaskState::as_str))
        .bind(query.q)
        .bind(before_time)
        .bind(before_id)
        .bind(i64::from(query.limit) + 1)
        .persistent(false)
        .fetch_one(&self.pool)
        .await?;
        let mut page: TaskList = decode(raw)?;
        if page.unavailable_workspaces.len() == query.workspace_ids.len() {
            return Err(Error::Unauthorized);
        }
        page.has_more = page.items.len() > usize::from(query.limit);
        page.items.truncate(usize::from(query.limit));
        Ok(page)
    }
}
