ALTER TABLE nodes ADD COLUMN created_by text NOT NULL DEFAULT 'legacy:unknown';
ALTER TABLE nodes ADD COLUMN last_verified_by text;
ALTER TABLE nodes ADD COLUMN last_verified_at timestamptz;
ALTER TABLE nodes ADD COLUMN retirement_reason text;

-- Preserve historical payloads. Recover original authors only from recorded evidence.
UPDATE nodes n SET created_by = r.actor_id
FROM node_revisions r WHERE r.node_id=n.id AND r.revision=1;

DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM nodes WHERE kind='knowledge' AND verification <> 'unverified') THEN
  RAISE EXCEPTION 'knowledge compatibility preflight failed: existing verified/disputed records lack verification attribution; explicitly reconcile them before upgrading';
 END IF;
 IF EXISTS(SELECT 1 FROM nodes WHERE kind='knowledge' AND retired) THEN
  RAISE EXCEPTION 'knowledge compatibility preflight failed: existing retired records lack retirement reasons; explicitly reconcile them before upgrading';
 END IF;
END $$;

ALTER TABLE nodes ADD CONSTRAINT knowledge_verification_attribution CHECK (
 kind <> 'knowledge' OR
 (verification='unverified' AND last_verified_by IS NULL AND last_verified_at IS NULL) OR
 (verification IN ('verified','disputed') AND last_verified_by IS NOT NULL AND last_verified_at IS NOT NULL AND jsonb_array_length(provenance)>0)
);
ALTER TABLE nodes ADD CONSTRAINT knowledge_retirement_reason CHECK (
 NOT retired OR (retirement_reason IS NOT NULL AND length(btrim(retirement_reason))>0)
);
