ALTER TABLE users
    ADD COLUMN IF NOT EXISTS workspace_root TEXT NULL;

UPDATE users
SET workspace_root = CONCAT('/workspace/users/', user_id)
WHERE workspace_root IS NULL;

DELETE FROM mcp_tokens
WHERE created_by IS NULL;

ALTER TABLE mcp_tokens
    ALTER COLUMN created_by SET NOT NULL;

CREATE INDEX IF NOT EXISTS mcp_tokens_created_by_idx
    ON mcp_tokens (created_by);
