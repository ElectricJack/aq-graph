-- Make provenance, classification, verification and retirement common node
-- fields. Historical JSON snapshots retain their original keys and values.
ALTER TABLE nodes DROP CONSTRAINT nodes_check;
ALTER TABLE nodes RENAME COLUMN knowledge_type TO content_type;
ALTER TABLE nodes RENAME CONSTRAINT nodes_knowledge_type_check TO nodes_content_type_check;
ALTER TABLE nodes DROP CONSTRAINT knowledge_verification_attribution;
ALTER TABLE nodes DROP CONSTRAINT knowledge_retirement_reason;
ALTER TABLE nodes ADD COLUMN summary text;
ALTER TABLE nodes ADD COLUMN summary_of_revision bigint;
ALTER TABLE nodes ADD COLUMN valid_from timestamptz;
ALTER TABLE nodes ADD COLUMN valid_until timestamptz;
ALTER TABLE nodes ADD COLUMN review_at timestamptz;
ALTER TABLE nodes ADD COLUMN successor_id uuid;

-- Record exactly which current task values require a new material baseline.
-- Reuse the current immutable link set rather than fabricating old membership.
CREATE TEMP TABLE compat7_tasks ON COMMIT DROP AS
    SELECT n.id,n.workspace_id,r.link_set_id FROM nodes n
    LEFT JOIN node_revisions r ON r.workspace_id=n.workspace_id
        AND r.node_id=n.id AND r.revision=n.content_revision
    WHERE n.kind='task' AND (n.content_type IS NULL OR n.verification IS NULL);
ALTER TABLE compat7_tasks ADD PRIMARY KEY(id);
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM compat7_tasks WHERE link_set_id IS NULL) THEN
        RAISE EXCEPTION 'shared field migration refused: current task has no exact link snapshot';
    END IF;
END $$;
UPDATE nodes SET content_type=COALESCE(content_type,'note'),
    verification=COALESCE(verification,'unverified')
    WHERE kind='task' AND (content_type IS NULL OR verification IS NULL);
ALTER TABLE nodes ALTER COLUMN content_type SET DEFAULT 'note';
ALTER TABLE nodes ALTER COLUMN content_type SET NOT NULL;
ALTER TABLE nodes ALTER COLUMN verification SET DEFAULT 'unverified';
ALTER TABLE nodes ALTER COLUMN verification SET NOT NULL;
ALTER TABLE nodes ADD CONSTRAINT nodes_task_fields CHECK (
    (kind='task' AND task_state IS NOT NULL)
    OR (kind='knowledge' AND task_state IS NULL AND acceptance_criteria='')
);
ALTER TABLE nodes ADD CONSTRAINT nodes_verification_attribution CHECK (
    (verification='unverified' AND last_verified_by IS NULL AND last_verified_at IS NULL)
    OR (verification IN ('verified','disputed') AND last_verified_by IS NOT NULL
        AND last_verified_at IS NOT NULL AND jsonb_array_length(provenance)>0)
);
ALTER TABLE nodes ADD CONSTRAINT nodes_retirement_reason CHECK (
    (NOT retired AND retirement_reason IS NULL)
    OR (retired AND retirement_reason IS NOT NULL
        AND length(btrim(retirement_reason))>0 AND octet_length(retirement_reason)<=4096)
);
ALTER TABLE nodes ADD CONSTRAINT nodes_summary_pair CHECK (
    (summary IS NULL AND summary_of_revision IS NULL)
    OR (summary IS NOT NULL AND summary_of_revision IS NOT NULL
        AND length(btrim(summary))>0 AND octet_length(summary)<=8192
        AND summary_of_revision>0 AND summary_of_revision<content_revision)
);
ALTER TABLE nodes ADD CONSTRAINT nodes_summary_source FOREIGN KEY(workspace_id,id,summary_of_revision)
    REFERENCES node_revisions(workspace_id,node_id,revision);
ALTER TABLE nodes ADD CONSTRAINT nodes_validity_range CHECK (
    valid_from IS NULL OR valid_until IS NULL OR valid_from<valid_until
);
ALTER TABLE nodes ADD CONSTRAINT nodes_successor_scope FOREIGN KEY(workspace_id,successor_id)
    REFERENCES nodes(workspace_id,id);
ALTER TABLE nodes ADD CONSTRAINT nodes_successor_retired CHECK (successor_id IS NULL OR retired);
ALTER TABLE nodes ADD CONSTRAINT nodes_successor_not_self CHECK (successor_id IS NULL OR successor_id<>id);
-- Full-text matching now covers both node kinds.
DROP INDEX nodes_search;
CREATE INDEX nodes_search ON nodes USING gin(search_document) WHERE NOT deleted;
CREATE INDEX nodes_valid_from ON nodes(workspace_id,valid_from,id)
    WHERE NOT deleted AND valid_from IS NOT NULL;
CREATE INDEX nodes_valid_until ON nodes(workspace_id,valid_until,id)
    WHERE NOT deleted AND valid_until IS NOT NULL;
CREATE INDEX nodes_review_at ON nodes(workspace_id,review_at,id)
    WHERE NOT deleted AND review_at IS NOT NULL;
CREATE INDEX nodes_successor ON nodes(workspace_id,successor_id)
    WHERE successor_id IS NOT NULL;

CREATE TEMP TABLE compat7_changes ON COMMIT DROP AS
    SELECT workspace_id,gen_random_uuid() AS change_set_id
    FROM (SELECT DISTINCT workspace_id FROM compat7_tasks) scopes;
ALTER TABLE compat7_changes ADD PRIMARY KEY(workspace_id);
INSERT INTO change_sets(id,workspace_id,actor_id,name,reason,operations,status,result,applied_at)
SELECT change_set_id,workspace_id,'system:migration','Shared node field baseline',
       'Migration 0007 added explicit unverified/note task defaults; existing history and links retained',
       '[]','applied',jsonb_build_object('change_set_id',change_set_id,'results','[]'::jsonb),clock_timestamp()
FROM compat7_changes;
UPDATE nodes n SET version=version+1,content_revision=content_revision+1,updated_at=clock_timestamp()
    FROM compat7_tasks t WHERE n.id=t.id;
INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id,link_set_id)
SELECT n.workspace_id,n.id,n.content_revision,to_jsonb(n)-'search_document','system:migration','shared_fields_baseline',
       'Migration 0007 added explicit unverified/note task defaults; existing history and links retained',
       c.change_set_id,t.link_set_id
FROM nodes n JOIN compat7_tasks t ON t.id=n.id
JOIN compat7_changes c ON c.workspace_id=n.workspace_id;
INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data)
SELECT n.workspace_id,ARRAY[n.id],gen_random_uuid(),'revision','system:migration','shared_fields_baseline',
       'Migration 0007 added explicit unverified/note task defaults; existing history and links retained',
       c.change_set_id,to_jsonb(n)-'search_document'
FROM nodes n JOIN compat7_tasks t ON t.id=n.id
JOIN compat7_changes c ON c.workspace_id=n.workspace_id;
