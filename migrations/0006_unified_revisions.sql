-- Existing node revision snapshots and history are never rewritten. A NULL link
-- set explicitly means that the legacy revision has no reconstructible links.
ALTER TABLE edges ADD CONSTRAINT edges_workspace_identity UNIQUE(workspace_id,id);
CREATE TABLE edge_versions (
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    id uuid NOT NULL,
    version bigint NOT NULL CHECK(version>0),
    snapshot jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(workspace_id,id,version),
    FOREIGN KEY(workspace_id,id) REFERENCES edges(workspace_id,id),
    CHECK ((snapshot->>'id')::uuid=id
        AND (snapshot->>'workspace_id')::uuid=workspace_id
        AND (snapshot->>'version')::bigint=version)
);
CREATE TABLE link_sets (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    node_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY(workspace_id,node_id) REFERENCES nodes(workspace_id,id),
    UNIQUE(workspace_id,id),
    UNIQUE(workspace_id,node_id,id)
);
CREATE INDEX link_sets_node ON link_sets(workspace_id,node_id,created_at);
CREATE TABLE link_set_members (
    workspace_id uuid NOT NULL,
    set_id uuid NOT NULL,
    edge_id uuid NOT NULL,
    edge_version bigint NOT NULL,
    PRIMARY KEY(set_id,edge_id),
    FOREIGN KEY(workspace_id,set_id) REFERENCES link_sets(workspace_id,id),
    FOREIGN KEY(workspace_id,edge_id,edge_version)
        REFERENCES edge_versions(workspace_id,id,version)
);
ALTER TABLE node_revisions ADD COLUMN link_set_id uuid;
ALTER TABLE node_revisions ADD CONSTRAINT node_revision_link_set_owner
    FOREIGN KEY(workspace_id,node_id,link_set_id)
    REFERENCES link_sets(workspace_id,node_id,id);

-- Establish an honest current-state boundary for both record kinds. Legacy
-- revisions retain NULL; current records gain a fresh attributed baseline.
CREATE TEMP TABLE compat6_changes ON COMMIT DROP AS
    SELECT workspace_id,gen_random_uuid() AS change_set_id
    FROM (SELECT DISTINCT workspace_id FROM nodes) workspace_nodes;
ALTER TABLE compat6_changes ADD PRIMARY KEY(workspace_id);
INSERT INTO change_sets(id,workspace_id,actor_id,name,reason,operations,status,result,applied_at)
SELECT change_set_id,workspace_id,'system:migration','Unified revision baseline',
       'Migration 0006 captured current content and links; legacy link snapshots remain unavailable',
       '[]','applied',jsonb_build_object('change_set_id',change_set_id,'results','[]'::jsonb),clock_timestamp()
FROM compat6_changes;
CREATE TEMP TABLE compat6_sets ON COMMIT DROP AS
    SELECT id AS node_id,workspace_id,gen_random_uuid() AS set_id FROM nodes;
ALTER TABLE compat6_sets ADD PRIMARY KEY(node_id);
INSERT INTO link_sets(id,workspace_id,node_id)
    SELECT set_id,workspace_id,node_id FROM compat6_sets;
INSERT INTO edge_versions(workspace_id,id,version,snapshot)
    SELECT workspace_id,id,version,to_jsonb(e) FROM edges e WHERE NOT deleted;
INSERT INTO link_set_members(workspace_id,set_id,edge_id,edge_version)
SELECT s.workspace_id,s.set_id,e.id,e.version FROM compat6_sets s
JOIN edges e ON e.workspace_id=s.workspace_id
    AND (e.source=s.node_id OR (e.target=s.node_id AND e.edge_type IN ('blocks','parent-child')))
WHERE NOT e.deleted;
UPDATE nodes SET version=version+1,content_revision=content_revision+1,updated_at=clock_timestamp();
INSERT INTO node_revisions(workspace_id,node_id,revision,snapshot,actor_id,operation,reason,change_set_id,link_set_id)
SELECT n.workspace_id,n.id,n.content_revision,to_jsonb(n)-'search_document','system:migration','baseline',
       'Migration 0006 captured current content and links; legacy link snapshots remain unavailable',
       c.change_set_id,s.set_id
FROM nodes n JOIN compat6_changes c ON c.workspace_id=n.workspace_id
JOIN compat6_sets s ON s.node_id=n.id;
INSERT INTO history_entries(workspace_id,node_ids,entry_id,category,actor_id,operation,reason,change_set_id,data)
SELECT n.workspace_id,ARRAY[n.id],gen_random_uuid(),'revision','system:migration','baseline',
       'Migration 0006 captured current content and links; legacy link snapshots remain unavailable',
       c.change_set_id,to_jsonb(n)-'search_document'
FROM nodes n JOIN compat6_changes c ON c.workspace_id=n.workspace_id;
