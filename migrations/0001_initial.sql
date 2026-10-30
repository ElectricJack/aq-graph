CREATE TABLE workspaces (
 id uuid PRIMARY KEY,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE credentials (
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 actor_id text NOT NULL,
 token_hash text NOT NULL UNIQUE,
 capability text NOT NULL CHECK (capability IN ('read','write','admin')),
 PRIMARY KEY (workspace_id,actor_id)
);
CREATE TABLE nodes (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 kind text NOT NULL CHECK (kind IN ('task','knowledge')),
 version bigint NOT NULL CHECK (version>0),
 content_revision bigint NOT NULL CHECK (content_revision>0),
 title text NOT NULL,
 body text NOT NULL,
 acceptance_criteria text NOT NULL,
 labels jsonb NOT NULL DEFAULT '[]',
 metadata jsonb NOT NULL DEFAULT '{}',
 task_state text CHECK (task_state IN ('draft','active','completed','canceled')),
 knowledge_type text CHECK (knowledge_type IN ('fact','decision','policy','procedure','incident','reference','note')),
 verification text CHECK (verification IN ('unverified','verified','disputed')),
 retired boolean NOT NULL DEFAULT false,
 provenance jsonb NOT NULL DEFAULT '[]',
 deleted boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL,
 updated_at timestamptz NOT NULL,
 search_document tsvector GENERATED ALWAYS AS (to_tsvector('english',title || ' ' || body)) STORED,
 CHECK ((kind='task' AND task_state IS NOT NULL AND knowledge_type IS NULL AND verification IS NULL)
     OR (kind='knowledge' AND task_state IS NULL AND knowledge_type IS NOT NULL AND verification IS NOT NULL)),
 UNIQUE (workspace_id,id)
);
CREATE INDEX nodes_scope_page ON nodes(workspace_id,id) WHERE NOT deleted;
CREATE INDEX nodes_scope_kind_page ON nodes(workspace_id,kind,id) WHERE NOT deleted;
CREATE INDEX nodes_ready ON nodes(workspace_id,task_state,id) WHERE NOT deleted AND kind='task';
CREATE INDEX nodes_search ON nodes USING gin(search_document) WHERE NOT deleted AND kind='knowledge';
CREATE TABLE change_sets (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 actor_id text NOT NULL,
 name text,
 reason text,
 operations jsonb NOT NULL,
 status text NOT NULL CHECK(status IN ('draft','applied')),
 result jsonb,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 applied_at timestamptz
);
CREATE INDEX change_sets_scope ON change_sets(workspace_id,id);
CREATE TABLE node_revisions (
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 node_id uuid NOT NULL REFERENCES nodes(id),
 revision bigint NOT NULL,
 snapshot jsonb NOT NULL,
 actor_id text NOT NULL,
 operation text NOT NULL,
 reason text,
 change_set_id uuid NOT NULL REFERENCES change_sets(id),
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 PRIMARY KEY(node_id,revision),
 UNIQUE(workspace_id,node_id,revision)
);
CREATE TABLE edges (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 source uuid NOT NULL,
 target uuid NOT NULL,
 edge_type text NOT NULL CHECK(edge_type IN ('blocks','parent-child','references','motivated_by','produces','supports','contradicts','supersedes')),
 source_revision bigint,
 target_revision bigint,
 version bigint NOT NULL CHECK(version>0),
 deleted boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL,
 FOREIGN KEY(workspace_id,source) REFERENCES nodes(workspace_id,id),
 FOREIGN KEY(workspace_id,target) REFERENCES nodes(workspace_id,id),
 FOREIGN KEY(workspace_id,source,source_revision) REFERENCES node_revisions(workspace_id,node_id,revision),
 FOREIGN KEY(workspace_id,target,target_revision) REFERENCES node_revisions(workspace_id,node_id,revision)
);
CREATE INDEX edges_outgoing ON edges(workspace_id,source,edge_type,target) WHERE NOT deleted;
CREATE INDEX edges_incoming ON edges(workspace_id,target,edge_type,source) WHERE NOT deleted;
CREATE UNIQUE INDEX edges_unique_active ON edges(workspace_id,source,target,edge_type,COALESCE(source_revision,0),COALESCE(target_revision,0)) WHERE NOT deleted;
CREATE TABLE history_entries (
 sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 node_ids uuid[] NOT NULL,
 entry_id uuid NOT NULL UNIQUE,
 category text NOT NULL CHECK(category IN ('revision','lifecycle','edge')),
 actor_id text NOT NULL,
 operation text NOT NULL,
 reason text,
 change_set_id uuid NOT NULL REFERENCES change_sets(id),
 data jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX history_nodes ON history_entries USING gin(node_ids);
CREATE INDEX history_scope_sequence ON history_entries(workspace_id,sequence);
CREATE TABLE idempotency_receipts (
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 actor_id text NOT NULL,
 operation text NOT NULL,
 key text NOT NULL,
 payload_hash text NOT NULL,
 result jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 PRIMARY KEY(workspace_id,actor_id,operation,key)
);
-- Receipts are retained indefinitely in the pilot; any future cleanup must first
-- establish a documented retry window. Durable history is never pruned here.
