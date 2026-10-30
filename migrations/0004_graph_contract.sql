-- Upgrade only a valid structural forest; never discard or silently reparent
-- existing relationships to satisfy the compatibility contract.
DO $$
DECLARE
    total_vertices bigint;
    visited_vertices bigint := 0;
    frontier_size bigint;
    decrement record;
    remaining_degree bigint;
BEGIN
    IF EXISTS (
        SELECT 1 FROM edges e
        JOIN nodes a ON a.id=e.source AND a.workspace_id=e.workspace_id
        JOIN nodes b ON b.id=e.target AND b.workspace_id=e.workspace_id
        WHERE NOT e.deleted AND (
            a.deleted OR b.deleted OR e.source=e.target OR
            (e.edge_type IN ('blocks','parent-child') AND (a.kind<>'task' OR b.kind<>'task')) OR
            (e.edge_type IN ('motivated_by','produces') AND (a.kind<>'task' OR b.kind<>'knowledge')) OR
            (e.edge_type IN ('supports','contradicts','supersedes') AND (a.kind<>'knowledge' OR b.kind<>'knowledge'))
        )
    ) THEN
        RAISE EXCEPTION 'graph contract migration refused: invalid active relationship endpoint';
    END IF;
    IF EXISTS(SELECT 1 FROM edges WHERE NOT deleted AND edge_type='parent-child'
              GROUP BY workspace_id,target HAVING count(*)>1) THEN
        RAISE EXCEPTION 'graph contract migration refused: child has multiple live parents';
    END IF;
    IF EXISTS(
        WITH RECURSIVE depth(workspace_id,id,level) AS (
            SELECT DISTINCT workspace_id,source,1 FROM edges
            WHERE NOT deleted AND edge_type='parent-child'
            UNION ALL
            SELECT e.workspace_id,e.target,d.level+1 FROM depth d
            JOIN edges e ON e.workspace_id=d.workspace_id AND e.source=d.id
            WHERE NOT e.deleted AND e.edge_type='parent-child' AND d.level<4
        ) SELECT 1 FROM depth WHERE level=4
    ) THEN
        RAISE EXCEPTION 'graph contract migration refused: parent hierarchy exceeds three levels or contains a cycle';
    END IF;
    -- Linear graph work: each vertex enters a frontier once; each outgoing
    -- arc contributes one indegree decrement. No transitive closure is built.
    -- PostgreSQL statement_timeout bounds the complete audit and rolls back a
    -- graph too large to audit inside the configured migration budget.
    CREATE TEMP TABLE compat4_arcs ON COMMIT DROP AS
        SELECT source,target FROM edges
        WHERE NOT deleted AND edge_type IN ('blocks','parent-child');
    CREATE INDEX compat4_arcs_source ON compat4_arcs(source);
    CREATE TEMP TABLE compat4_vertices ON COMMIT DROP AS
        SELECT ids.id,COALESCE(incoming.degree,0)::bigint AS degree
        FROM (SELECT source AS id FROM compat4_arcs UNION SELECT target FROM compat4_arcs) ids
        LEFT JOIN (SELECT target,count(*) AS degree FROM compat4_arcs GROUP BY target) incoming
            ON incoming.target=ids.id;
    ALTER TABLE compat4_vertices ADD PRIMARY KEY(id);
    CREATE TEMP TABLE compat4_frontier(id uuid PRIMARY KEY) ON COMMIT DROP;
    CREATE TEMP TABLE compat4_next(id uuid PRIMARY KEY) ON COMMIT DROP;
    SELECT count(*) INTO total_vertices FROM compat4_vertices;
    INSERT INTO compat4_frontier SELECT id FROM compat4_vertices WHERE degree=0;
    LOOP
        SELECT count(*) INTO frontier_size FROM compat4_frontier;
        EXIT WHEN frontier_size=0;
        visited_vertices:=visited_vertices+frontier_size;
        FOR decrement IN
            SELECT a.target,count(*) AS degree
            FROM compat4_frontier f
            CROSS JOIN LATERAL (
                SELECT target FROM compat4_arcs WHERE source=f.id OFFSET 0
            ) a GROUP BY a.target
        LOOP
            -- Parameterized arc reads and exact primary-key updates avoid a
            -- full remaining-graph scan on every layer of a long chain.
            UPDATE compat4_vertices SET degree=degree-decrement.degree
                WHERE id=decrement.target RETURNING degree INTO remaining_degree;
            IF remaining_degree=0 THEN
                INSERT INTO compat4_next VALUES(decrement.target);
            END IF;
        END LOOP;
        TRUNCATE compat4_frontier;
        INSERT INTO compat4_frontier SELECT id FROM compat4_next;
        TRUNCATE compat4_next;
    END LOOP;
    IF visited_vertices<>total_vertices THEN
        RAISE EXCEPTION 'graph contract migration refused: combined blocking/structural cycle';
    END IF;
END $$;
CREATE UNIQUE INDEX edges_one_active_parent ON edges(workspace_id,target)
    WHERE NOT deleted AND edge_type='parent-child';
ALTER TABLE edges ADD COLUMN description text;
ALTER TABLE edges ADD CONSTRAINT edges_description_limit
    CHECK (description IS NULL OR octet_length(description)<=4096);
