-- Credentials are authenticated within a workspace; identical independently
-- provisioned secrets in separate workspaces must not conflict globally.
ALTER TABLE credentials DROP CONSTRAINT credentials_token_hash_key;
ALTER TABLE credentials ADD CONSTRAINT credentials_workspace_token_hash_key
    UNIQUE(workspace_id,token_hash);
