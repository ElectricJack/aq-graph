-- Additive committed observation owner. Existing record/history formats unchanged.
-- Exclude all ordinary DML throughout baseline/head installation.
LOCK TABLE workspaces, nodes, edges IN SHARE ROW EXCLUSIVE MODE;
CREATE TABLE graph_observation_heads (
 workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
 stream_id uuid NOT NULL DEFAULT gen_random_uuid(),
 generation bigint NOT NULL DEFAULT 1 CHECK(generation>0),
 sequence bigint NOT NULL DEFAULT 0 CHECK(sequence>=0),
 group_id uuid,
 writer_xid xid8,
 coverage text NOT NULL DEFAULT 'complete' CHECK(coverage IN('complete','gap')),
 gap_start bigint,
 gap_reason text CHECK(gap_reason IN('journal_capacity','transaction_fact_limit','domain_invariant')),
 retained_floor bigint NOT NULL DEFAULT 0 CHECK(retained_floor>=0),
 batch_count bigint NOT NULL DEFAULT 0 CHECK(batch_count BETWEEN 0 AND 32768),
 fact_count bigint NOT NULL DEFAULT 0 CHECK(fact_count BETWEEN 0 AND 65536),
 journal_bytes bigint NOT NULL DEFAULT 0 CHECK(journal_bytes BETWEEN 0 AND 16777216),
 control_count bigint NOT NULL DEFAULT 0 CHECK(control_count BETWEEN 0 AND 1024),
 control_bytes bigint NOT NULL DEFAULT 0 CHECK(control_bytes BETWEEN 0 AND 16777216),
 CHECK((sequence=0)=(group_id IS NULL)),
 CHECK((coverage='complete')=(gap_start IS NULL AND gap_reason IS NULL))
);
INSERT INTO graph_observation_heads(workspace_id) SELECT id FROM workspaces;
CREATE TABLE graph_observation_batches (
 workspace_id uuid NOT NULL REFERENCES graph_observation_heads(workspace_id),
 generation bigint NOT NULL CHECK(generation>0),
 sequence bigint NOT NULL CHECK(sequence>0),
 group_id uuid NOT NULL,
 kind text NOT NULL DEFAULT 'complete' CHECK(kind IN('complete','gap')),
 fact_count bigint NOT NULL DEFAULT 0 CHECK(fact_count BETWEEN 0 AND 4096),
 PRIMARY KEY(workspace_id,generation,sequence),
 UNIQUE(workspace_id,generation,group_id)
);
CREATE TABLE graph_observation_facts (
 workspace_id uuid NOT NULL,
 generation bigint NOT NULL,
 sequence bigint NOT NULL,
 object_kind text NOT NULL CHECK(object_kind IN('node','edge')),
 object_id uuid NOT NULL,
 before_fact jsonb,
 after_fact jsonb,
 mutation_count bigint NOT NULL CHECK(mutation_count>0),
 transition_class text NOT NULL CHECK(transition_class IN('created','lifecycle_only','record_changed','multiple_changes','deleted','domain_invariant_failure')),
 retained_bytes bigint NOT NULL CHECK(retained_bytes BETWEEN 1 AND 4096),
 PRIMARY KEY(workspace_id,generation,sequence,object_kind,object_id),
 FOREIGN KEY(workspace_id,generation,sequence) REFERENCES graph_observation_batches ON DELETE CASCADE
);
CREATE INDEX graph_observation_object ON graph_observation_facts(workspace_id,generation,object_id,sequence);
CREATE TABLE graph_observation_subscriptions (
 workspace_id uuid NOT NULL REFERENCES graph_observation_heads(workspace_id),
 actor_id text NOT NULL CHECK(octet_length(actor_id) BETWEEN 1 AND 200),
 subscriber_id uuid NOT NULL,
 request_digest text NOT NULL CHECK(request_digest ~ '^[0-9a-f]{64}$'),
 receipt_digest text NOT NULL CHECK(receipt_digest ~ '^[0-9a-f]{64}$'),
 request jsonb NOT NULL,
 receipt jsonb NOT NULL,
 original_expires_at_ms bigint NOT NULL CHECK(original_expires_at_ms>0),
 retain_until_ms bigint NOT NULL CHECK(retain_until_ms>=original_expires_at_ms),
 released boolean NOT NULL DEFAULT false,
 retained_bytes bigint NOT NULL CHECK(retained_bytes BETWEEN 1 AND 131072),
 PRIMARY KEY(workspace_id,actor_id,subscriber_id)
);
CREATE INDEX graph_observation_control_expiry ON graph_observation_subscriptions(workspace_id,retain_until_ms,subscriber_id);

CREATE FUNCTION graph_observation_workspace() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 INSERT INTO graph_observation_heads(workspace_id) VALUES(NEW.id);
 RETURN NEW;
END $$;
CREATE TRIGGER graph_observation_workspace_insert AFTER INSERT ON workspaces
 FOR EACH ROW EXECUTE FUNCTION graph_observation_workspace();

CREATE FUNCTION graph_observation_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='TRUNCATE' THEN
  RAISE EXCEPTION 'observation generation maintenance required' USING ERRCODE='0A000';
 END IF;
 IF NEW.id IS DISTINCT FROM OLD.id OR NEW.workspace_id IS DISTINCT FROM OLD.workspace_id THEN
  RAISE EXCEPTION 'graph identity and workspace are immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER graph_observation_node_identity BEFORE UPDATE OF id,workspace_id ON nodes
 FOR EACH ROW EXECUTE FUNCTION graph_observation_identity();
CREATE TRIGGER graph_observation_edge_identity BEFORE UPDATE OF id,workspace_id ON edges
 FOR EACH ROW EXECUTE FUNCTION graph_observation_identity();
CREATE TRIGGER graph_observation_node_truncate BEFORE TRUNCATE ON nodes
 FOR EACH STATEMENT EXECUTE FUNCTION graph_observation_identity();
CREATE TRIGGER graph_observation_edge_truncate BEFORE TRUNCATE ON edges
 FOR EACH STATEMENT EXECUTE FUNCTION graph_observation_identity();

CREATE FUNCTION graph_observation_node_projection(n nodes) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
 SELECT jsonb_build_object('id',n.id,'kind',n.kind,'version',n.version::text,
 'logical_revision',n.content_revision::text,'task_state',n.task_state,'deleted',n.deleted,'retired',n.retired)
$$;
CREATE FUNCTION graph_observation_edge_projection(e edges) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
 SELECT jsonb_build_object('id',e.id,'edge_type',e.edge_type,'source',e.source,'target',e.target,
 'source_revision',e.source_revision::text,'target_revision',e.target_revision::text,
 'version',e.version::text,'deleted',e.deleted)
$$;

-- The head lock serializes first allocation through COMMIT, not just statement order.
-- A missing head under a pre-migration snapshot MUST retry, never bypass capture.
CREATE FUNCTION graph_observation_capture(w uuid,k text,i uuid,b jsonb,a jsonb,c text)
 RETURNS void LANGUAGE plpgsql AS $$
DECLARE h graph_observation_heads%ROWTYPE; f graph_observation_facts%ROWTYPE;
 x xid8 := pg_current_xact_id(); bytes bigint; oldbytes bigint := 0; fresh boolean;
 batchfacts bigint; firstbefore jsonb; countchanges bigint; finalclass text;
BEGIN
 SELECT * INTO h FROM graph_observation_heads WHERE workspace_id=w FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'observation head snapshot changed' USING ERRCODE='40001'; END IF;
 IF h.writer_xid IS DISTINCT FROM x THEN
  IF h.sequence=9223372036854775807 THEN
   RAISE EXCEPTION 'observation generation exhausted' USING ERRCODE='54000';
  END IF;
  h.sequence:=h.sequence+1; h.group_id:=gen_random_uuid(); h.writer_xid:=x;
  IF h.coverage='complete' THEN
   IF h.batch_count>=32768 OR h.journal_bytes+256>16777216 THEN
    h.coverage:='gap'; h.gap_start:=h.sequence; h.gap_reason:='journal_capacity';
   ELSE
    INSERT INTO graph_observation_batches(workspace_id,generation,sequence,group_id)
     VALUES(w,h.generation,h.sequence,h.group_id);
    h.batch_count:=h.batch_count+1; h.journal_bytes:=h.journal_bytes+256;
   END IF;
  END IF;
  UPDATE graph_observation_heads SET sequence=h.sequence,group_id=h.group_id,writer_xid=x,
   coverage=h.coverage,gap_start=h.gap_start,gap_reason=h.gap_reason,
   batch_count=h.batch_count,journal_bytes=h.journal_bytes WHERE workspace_id=w;
 END IF;
 -- Held coverage uses only the reserved head row, even for arbitrarily many writes.
 IF h.coverage='gap' THEN RETURN; END IF;
 SELECT * INTO f FROM graph_observation_facts WHERE workspace_id=w AND generation=h.generation
  AND sequence=h.sequence AND object_kind=k AND object_id=i;
 fresh:=NOT FOUND;
 firstbefore:=CASE WHEN fresh THEN b ELSE f.before_fact END;
 countchanges:=CASE WHEN fresh THEN 1 ELSE f.mutation_count+1 END;
 finalclass:=CASE WHEN c='domain_invariant_failure' OR (NOT fresh AND f.transition_class='domain_invariant_failure')
  THEN 'domain_invariant_failure' WHEN countchanges>1 THEN 'multiple_changes' ELSE c END;
 bytes:=128+octet_length(COALESCE(firstbefore::text,'null'))+octet_length(COALESCE(a::text,'null'));
 IF NOT fresh THEN oldbytes:=f.retained_bytes; END IF;
 SELECT fact_count INTO batchfacts FROM graph_observation_batches
  WHERE workspace_id=w AND generation=h.generation AND sequence=h.sequence;
 IF bytes>4096 OR (fresh AND (batchfacts>=4096 OR h.fact_count>=65536))
  OR h.journal_bytes-oldbytes+bytes>16777216 OR finalclass='domain_invariant_failure' THEN
  UPDATE graph_observation_heads SET coverage='gap',gap_start=h.sequence,
   gap_reason=CASE WHEN finalclass='domain_invariant_failure' THEN 'domain_invariant'
    WHEN batchfacts>=4096 THEN 'transaction_fact_limit' ELSE 'journal_capacity' END WHERE workspace_id=w;
  UPDATE graph_observation_batches SET kind='gap' WHERE workspace_id=w AND generation=h.generation AND sequence=h.sequence;
  RETURN;
 END IF;
 INSERT INTO graph_observation_facts(workspace_id,generation,sequence,object_kind,object_id,
  before_fact,after_fact,mutation_count,transition_class,retained_bytes)
 VALUES(w,h.generation,h.sequence,k,i,firstbefore,a,countchanges,finalclass,bytes)
 ON CONFLICT(workspace_id,generation,sequence,object_kind,object_id) DO UPDATE SET
  after_fact=EXCLUDED.after_fact,mutation_count=EXCLUDED.mutation_count,
  transition_class=EXCLUDED.transition_class,retained_bytes=EXCLUDED.retained_bytes;
 UPDATE graph_observation_heads SET journal_bytes=journal_bytes-oldbytes+bytes,
  fact_count=fact_count+CASE WHEN fresh THEN 1 ELSE 0 END WHERE workspace_id=w;
 IF fresh THEN UPDATE graph_observation_batches SET fact_count=fact_count+1
  WHERE workspace_id=w AND generation=h.generation AND sequence=h.sequence; END IF;
END $$;

CREATE FUNCTION graph_observation_node() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE b jsonb; a jsonb; c text := 'record_changed'; w uuid; i uuid;
BEGIN
 IF TG_OP='UPDATE' AND OLD IS NOT DISTINCT FROM NEW THEN RETURN NULL; END IF;
 IF TG_OP<>'INSERT' THEN b:=graph_observation_node_projection(OLD); w:=OLD.workspace_id; i:=OLD.id; END IF;
 IF TG_OP<>'DELETE' THEN a:=graph_observation_node_projection(NEW); w:=NEW.workspace_id; i:=NEW.id; END IF;
 IF TG_OP='INSERT' THEN c:='created';
 ELSIF TG_OP='DELETE' THEN c:='deleted';
 ELSIF NEW.version<=OLD.version OR NEW.content_revision<OLD.content_revision THEN c:='domain_invariant_failure';
 ELSIF OLD.kind='task' AND NEW.kind='task' AND OLD.task_state IS DISTINCT FROM NEW.task_state
  AND NEW.version=OLD.version+1 AND NEW.content_revision=OLD.content_revision+1
  AND ROW(OLD.title,OLD.body,OLD.acceptance_criteria,OLD.labels,OLD.metadata,OLD.content_type,
   OLD.valid_from,OLD.valid_until,OLD.review_at,OLD.successor_id,OLD.retired,OLD.deleted,
   OLD.provenance,OLD.retirement_reason,OLD.created_at,OLD.created_by)
   IS NOT DISTINCT FROM
   ROW(NEW.title,NEW.body,NEW.acceptance_criteria,NEW.labels,NEW.metadata,NEW.content_type,
   NEW.valid_from,NEW.valid_until,NEW.review_at,NEW.successor_id,NEW.retired,NEW.deleted,
   NEW.provenance,NEW.retirement_reason,NEW.created_at,NEW.created_by)
  AND (ROW(OLD.verification,OLD.last_verified_by,OLD.last_verified_at)
   IS NOT DISTINCT FROM ROW(NEW.verification,NEW.last_verified_by,NEW.last_verified_at)
   OR (NEW.verification='unverified' AND NEW.last_verified_by IS NULL AND NEW.last_verified_at IS NULL))
  AND ((NEW.summary IS NOT DISTINCT FROM OLD.summary AND NEW.summary_of_revision IS NOT DISTINCT FROM OLD.summary_of_revision)
   OR (NEW.summary IS NULL AND NEW.summary_of_revision IS NULL)) THEN c:='lifecycle_only';
 END IF;
 PERFORM graph_observation_capture(w,'node',i,b,a,c);
 RETURN NULL;
END $$;
CREATE TRIGGER graph_observation_node_change AFTER INSERT OR UPDATE OR DELETE ON nodes
 FOR EACH ROW EXECUTE FUNCTION graph_observation_node();
CREATE FUNCTION graph_observation_edge() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE b jsonb; a jsonb; c text := 'record_changed'; w uuid; i uuid;
BEGIN
 IF TG_OP='UPDATE' AND OLD IS NOT DISTINCT FROM NEW THEN RETURN NULL; END IF;
 IF TG_OP<>'INSERT' THEN b:=graph_observation_edge_projection(OLD); w:=OLD.workspace_id; i:=OLD.id; END IF;
 IF TG_OP<>'DELETE' THEN a:=graph_observation_edge_projection(NEW); w:=NEW.workspace_id; i:=NEW.id; END IF;
 IF TG_OP='INSERT' THEN c:='created'; ELSIF TG_OP='DELETE' THEN c:='deleted';
 ELSIF NEW.version<=OLD.version THEN c:='domain_invariant_failure'; END IF;
 PERFORM graph_observation_capture(w,'edge',i,b,a,c);
 RETURN NULL;
END $$;
CREATE TRIGGER graph_observation_edge_change AFTER INSERT OR UPDATE OR DELETE ON edges
 FOR EACH ROW EXECUTE FUNCTION graph_observation_edge();

-- Operator-only reset: restores/reseeds must rotate before publishing source support.
-- No remote or application domain work belongs under this lock. Not an HTTP route.
CREATE FUNCTION graph_observation_reset(w uuid,expected_generation bigint) RETURNS bigint
 LANGUAGE plpgsql AS $$
DECLARE h graph_observation_heads%ROWTYPE;
BEGIN
 SELECT * INTO h FROM graph_observation_heads WHERE workspace_id=w FOR UPDATE;
 IF NOT FOUND OR h.generation<>expected_generation OR h.generation=9223372036854775807 THEN
  RAISE EXCEPTION 'observation generation mismatch' USING ERRCODE='40001'; END IF;
 DELETE FROM graph_observation_batches WHERE workspace_id=w;
 UPDATE graph_observation_heads SET generation=generation+1,sequence=0,group_id=NULL,writer_xid=NULL,
  coverage='complete',gap_start=NULL,gap_reason=NULL,retained_floor=0,
  batch_count=0,fact_count=0,journal_bytes=0 WHERE workspace_id=w;
 RETURN h.generation+1;
END $$;
REVOKE ALL ON FUNCTION graph_observation_reset(uuid,bigint) FROM PUBLIC;
