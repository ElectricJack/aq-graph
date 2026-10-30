WITH candidates AS MATERIALIZED (
SELECT id FROM (
SELECT id FROM nodes WHERE workspace_id=$1 AND kind='task' AND task_state='active' AND NOT deleted AND ($2::uuid IS NULL OR id>$2)
EXCEPT
SELECT pc.source FROM edges pc JOIN nodes child ON child.id=pc.target AND child.workspace_id=pc.workspace_id
WHERE pc.workspace_id=$1 AND pc.edge_type='parent-child' AND NOT pc.deleted AND NOT child.deleted AND ($2::uuid IS NULL OR pc.source>$2)
) leaves ORDER BY id
)
SELECT to_jsonb(n)-'search_document' FROM candidates c
CROSS JOIN LATERAL (SELECT n.* FROM nodes n WHERE n.workspace_id=$1 AND n.id=c.id AND NOT n.retired
AND NOT EXISTS (
 SELECT 1 FROM edges pe JOIN nodes parent ON parent.id=pe.source AND parent.workspace_id=pe.workspace_id
 LEFT JOIN edges ge ON ge.workspace_id=pe.workspace_id AND ge.target=parent.id AND ge.edge_type='parent-child' AND NOT ge.deleted
 LEFT JOIN nodes grandparent ON grandparent.id=ge.source AND grandparent.workspace_id=ge.workspace_id
 WHERE pe.workspace_id=n.workspace_id AND pe.target=n.id AND pe.edge_type='parent-child' AND NOT pe.deleted
 AND (parent.deleted OR parent.retired OR parent.task_state='draft' OR grandparent.deleted OR grandparent.retired OR grandparent.task_state='draft'))
AND NOT EXISTS (SELECT 1 FROM edges e JOIN nodes prerequisite ON prerequisite.id=e.source WHERE e.workspace_id=n.workspace_id AND e.target=n.id AND e.edge_type='blocks' AND NOT e.deleted AND (prerequisite.deleted OR prerequisite.task_state IS DISTINCT FROM 'completed')) LIMIT 1) n ORDER BY c.id LIMIT $3