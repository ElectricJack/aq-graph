-- Read-only: zero rows means the derived readiness state matches current graph state.
WITH expected AS (
 SELECT n.workspace_id,n.id AS node_id,
        n.kind='task' AND n.task_state='active' AND NOT n.deleted AND NOT n.retired AS eligible,
        COALESCE(c.children,0) AS live_child_count
 FROM nodes n LEFT JOIN (
  SELECT e.workspace_id,e.source,count(*) AS children FROM edges e
  JOIN nodes child ON child.workspace_id=e.workspace_id AND child.id=e.target
  WHERE e.edge_type='parent-child' AND NOT e.deleted AND NOT child.deleted
  GROUP BY e.workspace_id,e.source
 ) c ON c.workspace_id=n.workspace_id AND c.source=n.id
)
SELECT COALESCE(e.workspace_id,p.workspace_id) AS workspace_id,
       COALESCE(e.node_id,p.node_id) AS node_id,
       e.live_child_count AS expected_children,p.live_child_count AS actual_children,
       e.eligible AS expected_eligible,p.eligible AS actual_eligible
FROM expected e FULL JOIN node_ready_projection p USING(workspace_id,node_id)
WHERE e.node_id IS NULL OR p.node_id IS NULL
   OR e.live_child_count IS DISTINCT FROM p.live_child_count
   OR e.eligible IS DISTINCT FROM p.eligible
ORDER BY workspace_id,node_id;
