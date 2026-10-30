-- Administrative repair of derived state; blocks graph writers until commit.
-- Run with psql -v ON_ERROR_STOP=1 against the intended aq-graph database.
BEGIN;
SET LOCAL lock_timeout='5s';
LOCK TABLE nodes,edges,node_ready_projection IN SHARE ROW EXCLUSIVE MODE;
INSERT INTO node_ready_projection(workspace_id,node_id,live_child_count,eligible)
SELECT n.workspace_id,n.id,COALESCE(c.children,0),
       n.kind='task' AND n.task_state='active' AND NOT n.deleted AND NOT n.retired
FROM nodes n LEFT JOIN (
 SELECT e.workspace_id,e.source,count(*) AS children FROM edges e
 JOIN nodes child ON child.workspace_id=e.workspace_id AND child.id=e.target
 WHERE e.edge_type='parent-child' AND NOT e.deleted AND NOT child.deleted
 GROUP BY e.workspace_id,e.source
) c ON c.workspace_id=n.workspace_id AND c.source=n.id
ON CONFLICT(workspace_id,node_id) DO UPDATE
SET live_child_count=EXCLUDED.live_child_count,eligible=EXCLUDED.eligible;
COMMIT;
ANALYZE node_ready_projection;
