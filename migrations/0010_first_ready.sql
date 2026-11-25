-- Legacy identities have no complete readiness history. Only identities
-- created after tracking begins can acquire a first-ready clock.
LOCK TABLE nodes, edges IN SHARE ROW EXCLUSIVE MODE;

CREATE TABLE task_first_ready (
    workspace_id uuid NOT NULL,
    node_id uuid NOT NULL,
    first_ready_at timestamptz,
    work_class text CHECK (length(work_class) BETWEEN 1 AND 128),
    PRIMARY KEY (workspace_id, node_id),
    FOREIGN KEY (workspace_id, node_id) REFERENCES nodes(workspace_id, id) ON DELETE CASCADE
);

CREATE FUNCTION track_task_first_ready() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO task_first_ready (workspace_id, node_id) VALUES (NEW.workspace_id, NEW.id);
    RETURN NULL;
END $$;
CREATE TRIGGER track_task_first_ready AFTER INSERT ON nodes
    FOR EACH ROW WHEN (NEW.kind = 'task') EXECUTE FUNCTION track_task_first_ready();

CREATE FUNCTION record_task_first_ready(workspace uuid, root uuid, include_descendants boolean)
RETURNS void LANGUAGE sql AS $$
    WITH RECURSIVE affected(id, depth) AS (
        SELECT root, 0
        UNION ALL
        SELECT e.target, a.depth+1 FROM affected a
        JOIN edges e ON e.workspace_id=workspace AND e.source=a.id
            AND e.edge_type='parent-child' AND NOT e.deleted
        WHERE include_descendants AND a.depth<2
    )
    UPDATE task_first_ready clock
    SET first_ready_at=clock_timestamp(),
        work_class=CASE WHEN jsonb_typeof(task.metadata->'aq:work_class')='string'
             AND length(task.metadata->>'aq:work_class') BETWEEN 1 AND 128
             AND (task.metadata->>'aq:work_class') !~ '[[:cntrl:]]'
            THEN task.metadata->>'aq:work_class' END
    FROM node_ready_projection projection
    JOIN nodes task ON task.workspace_id=projection.workspace_id AND task.id=projection.node_id
    WHERE clock.workspace_id=workspace AND clock.node_id IN (SELECT id FROM affected)
      AND clock.first_ready_at IS NULL
      AND projection.workspace_id=workspace AND projection.node_id=clock.node_id
      AND projection.eligible AND projection.live_child_count=0
      AND NOT EXISTS (
          SELECT 1 FROM edges pe
          JOIN nodes parent ON parent.workspace_id=workspace AND parent.id=pe.source
          LEFT JOIN edges ge ON ge.workspace_id=workspace AND ge.target=parent.id
              AND ge.edge_type='parent-child' AND NOT ge.deleted
          LEFT JOIN nodes grandparent ON grandparent.workspace_id=workspace AND grandparent.id=ge.source
          WHERE pe.workspace_id=workspace AND pe.target=clock.node_id
            AND pe.edge_type='parent-child' AND NOT pe.deleted
            AND (parent.deleted OR parent.retired OR parent.task_state='draft'
                 OR grandparent.deleted OR grandparent.retired OR grandparent.task_state='draft')
      )
      AND NOT EXISTS (
          SELECT 1 FROM edges e JOIN nodes prerequisite ON prerequisite.id=e.source
          WHERE e.workspace_id=workspace AND e.target=clock.node_id
            AND e.edge_type='blocks' AND NOT e.deleted
            AND (prerequisite.deleted OR prerequisite.task_state IS DISTINCT FROM 'completed')
      )
$$;

-- Deferred triggers inspect the final graph, so temporarily ready intermediate
-- states inside a batch never start a clock. Check only the affected task,
-- ancestors whose leaf changed, descendants and direct dependents.
CREATE FUNCTION record_first_ready_node_change() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE related record;
DECLARE unblocked_descendants boolean := false;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        unblocked_descendants :=
            (OLD.deleted OR OLD.retired OR OLD.task_state IS NOT DISTINCT FROM 'draft')
            AND NOT NEW.deleted AND NOT NEW.retired AND NEW.task_state IS DISTINCT FROM 'draft';
    END IF;
    PERFORM record_task_first_ready(NEW.workspace_id, NEW.id, unblocked_descendants);
    IF TG_OP = 'UPDATE' THEN
        IF NOT OLD.deleted AND NEW.deleted THEN
            FOR related IN SELECT source FROM edges WHERE workspace_id=NEW.workspace_id
                AND target=NEW.id AND edge_type='parent-child' AND NOT deleted LOOP
                PERFORM record_task_first_ready(NEW.workspace_id, related.source, false);
            END LOOP;
        END IF;
        IF NOT NEW.deleted AND NEW.task_state='completed'
            AND (OLD.deleted OR OLD.task_state IS DISTINCT FROM 'completed') THEN
            FOR related IN SELECT target FROM edges WHERE workspace_id=NEW.workspace_id
                AND source=NEW.id AND edge_type='blocks' AND NOT deleted LOOP
                PERFORM record_task_first_ready(NEW.workspace_id, related.target, false);
            END LOOP;
        END IF;
    END IF;
    RETURN NULL;
END $$;

CREATE CONSTRAINT TRIGGER first_ready_node_insert AFTER INSERT ON nodes
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.kind='task')
    EXECUTE FUNCTION record_first_ready_node_change();
CREATE CONSTRAINT TRIGGER first_ready_node_update AFTER UPDATE OF deleted,kind,task_state,retired ON nodes
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    WHEN (OLD.deleted IS DISTINCT FROM NEW.deleted OR OLD.kind IS DISTINCT FROM NEW.kind
          OR OLD.task_state IS DISTINCT FROM NEW.task_state OR OLD.retired IS DISTINCT FROM NEW.retired)
    EXECUTE FUNCTION record_first_ready_node_change();

CREATE FUNCTION record_first_ready_edge_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- Removing an existing edge can create a ready leaf or remove a guard.
    -- Adding an edge only constrains readiness.
    IF OLD.edge_type='parent-child' THEN
        PERFORM record_task_first_ready(OLD.workspace_id, OLD.source, false);
        PERFORM record_task_first_ready(OLD.workspace_id, OLD.target, true);
    ELSIF OLD.edge_type='blocks' THEN
        PERFORM record_task_first_ready(OLD.workspace_id, OLD.target, false);
    END IF;
    RETURN NULL;
END $$;

CREATE CONSTRAINT TRIGGER first_ready_edge_change
    AFTER DELETE OR UPDATE OF workspace_id,source,target,edge_type,deleted ON edges
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NOT OLD.deleted)
    EXECUTE FUNCTION record_first_ready_edge_change();
