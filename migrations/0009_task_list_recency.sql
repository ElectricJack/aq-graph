CREATE INDEX nodes_task_recency ON nodes(workspace_id, updated_at DESC, id DESC)
    WHERE kind='task' AND NOT deleted AND NOT retired;
CREATE INDEX nodes_task_state_recency ON nodes(workspace_id, task_state, updated_at DESC, id DESC)
    WHERE kind='task' AND NOT deleted AND NOT retired;
