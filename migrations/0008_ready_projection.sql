-- Internal derived state: never included in node JSON or durable revisions.
-- Migration DDL locks exclude writers throughout backfill/trigger installation.
LOCK TABLE nodes, edges IN SHARE ROW EXCLUSIVE MODE;
CREATE TABLE node_ready_projection (
 workspace_id uuid NOT NULL,
 node_id uuid NOT NULL,
 live_child_count bigint NOT NULL DEFAULT 0 CHECK (live_child_count>=0),
 eligible boolean NOT NULL,
 guard bigint NOT NULL DEFAULT 0,
 PRIMARY KEY(workspace_id,node_id),
 FOREIGN KEY(workspace_id,node_id) REFERENCES nodes(workspace_id,id) ON DELETE CASCADE
);
CREATE INDEX node_ready_leaves ON node_ready_projection(workspace_id,node_id)
 WHERE eligible AND live_child_count=0;
INSERT INTO node_ready_projection(workspace_id,node_id,live_child_count,eligible)
SELECT n.workspace_id,n.id,COALESCE(c.children,0),
       n.kind='task' AND n.task_state='active' AND NOT n.deleted AND NOT n.retired
FROM nodes n LEFT JOIN (
 SELECT e.workspace_id,e.source,count(*) children FROM edges e
 JOIN nodes child ON child.workspace_id=e.workspace_id AND child.id=e.target
 WHERE e.edge_type='parent-child' AND NOT e.deleted AND NOT child.deleted
 GROUP BY e.workspace_id,e.source
) c ON c.workspace_id=n.workspace_id AND c.source=n.id;

CREATE FUNCTION ready_projection_node() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' THEN
  INSERT INTO node_ready_projection(workspace_id,node_id,eligible)
  VALUES(NEW.workspace_id,NEW.id,NEW.kind='task' AND NEW.task_state='active' AND NOT NEW.deleted AND NOT NEW.retired);
 ELSE
  -- A real guard write, shared with edge mutations, prevents lost contributions
  -- when a child deletion/resurrection races an edge insert/delete. Under stale
  -- repeatable-read/serializable snapshots PostgreSQL rejects the writer.
  UPDATE node_ready_projection SET guard=guard+1,
   eligible=NEW.kind='task' AND NEW.task_state='active' AND NOT NEW.deleted AND NOT NEW.retired
  WHERE workspace_id=NEW.workspace_id AND node_id=NEW.id;
  IF OLD.deleted IS DISTINCT FROM NEW.deleted THEN
   UPDATE node_ready_projection p SET live_child_count=p.live_child_count+CASE WHEN NEW.deleted THEN -1 ELSE 1 END
   FROM edges e WHERE e.workspace_id=NEW.workspace_id AND e.target=NEW.id
    AND e.edge_type='parent-child' AND NOT e.deleted
    AND p.workspace_id=e.workspace_id AND p.node_id=e.source;
  END IF;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER ready_projection_node_insert AFTER INSERT ON nodes
 FOR EACH ROW EXECUTE FUNCTION ready_projection_node();
CREATE TRIGGER ready_projection_node_update AFTER UPDATE OF deleted,kind,task_state,retired ON nodes
 FOR EACH ROW WHEN (OLD.deleted IS DISTINCT FROM NEW.deleted OR OLD.kind IS DISTINCT FROM NEW.kind
  OR OLD.task_state IS DISTINCT FROM NEW.task_state OR OLD.retired IS DISTINCT FROM NEW.retired)
 EXECUTE FUNCTION ready_projection_node();

CREATE FUNCTION ready_projection_edge() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE old_live boolean := false; new_live boolean := false;
BEGIN
 -- Lock both targets in identity order before checking lifecycle. A guard write
 -- also forces stale snapshots to fail instead of observing outdated deletion.
 IF TG_OP='UPDATE' THEN
  PERFORM 1 FROM node_ready_projection WHERE
   (workspace_id=OLD.workspace_id AND node_id=OLD.target) OR
   (workspace_id=NEW.workspace_id AND node_id=NEW.target)
   ORDER BY workspace_id,node_id FOR UPDATE;
 END IF;
 IF TG_OP<>'INSERT' AND OLD.edge_type='parent-child' AND NOT OLD.deleted THEN
  UPDATE node_ready_projection SET guard=guard+1 WHERE workspace_id=OLD.workspace_id AND node_id=OLD.target;
  SELECT NOT deleted INTO old_live FROM nodes WHERE workspace_id=OLD.workspace_id AND id=OLD.target;
  IF old_live THEN
   UPDATE node_ready_projection SET live_child_count=live_child_count-1
    WHERE workspace_id=OLD.workspace_id AND node_id=OLD.source;
  END IF;
 END IF;
 IF TG_OP<>'DELETE' AND NEW.edge_type='parent-child' AND NOT NEW.deleted THEN
  UPDATE node_ready_projection SET guard=guard+1 WHERE workspace_id=NEW.workspace_id AND node_id=NEW.target;
  SELECT NOT deleted INTO new_live FROM nodes WHERE workspace_id=NEW.workspace_id AND id=NEW.target;
  IF new_live THEN
   UPDATE node_ready_projection SET live_child_count=live_child_count+1
    WHERE workspace_id=NEW.workspace_id AND node_id=NEW.source;
  END IF;
 END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER ready_projection_edge_change AFTER INSERT OR DELETE OR UPDATE OF workspace_id,source,target,edge_type,deleted ON edges
 FOR EACH ROW EXECUTE FUNCTION ready_projection_edge();
