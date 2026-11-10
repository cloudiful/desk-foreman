-- Plaintext workspace Git upstream credentials and the automatic DB-backed
-- capability signing key (issue 1180 P39).
--
-- Upstream credentials are now stored as plaintext by explicit human choice.
-- The legacy `credential_ciphertext` column is retained only so an old row is
-- detectable; a row carries either a plaintext secret or a legacy ciphertext,
-- never both. Existing rows are not converted or discarded by this migration.
--
-- The gateway signing key is a single process-independent database row. Rust
-- generates the 32 random bytes on first startup and this table preserves them
-- across restarts and instances.

ALTER TABLE workspace_git_upstreams
    ADD COLUMN credential_secret TEXT NULL;

ALTER TABLE workspace_git_upstreams
    DROP CONSTRAINT workspace_git_upstreams_credential_shape;

ALTER TABLE workspace_git_upstreams
    ADD CONSTRAINT workspace_git_upstreams_credential_shape CHECK (
        (
            credential_username IS NULL
            AND credential_secret IS NULL
            AND credential_ciphertext IS NULL
            AND credential_updated_at IS NULL
        )
        OR (
            credential_username IS NOT NULL
            AND credential_updated_at IS NOT NULL
            AND (
                (
                    credential_secret IS NOT NULL
                    AND credential_ciphertext IS NULL
                )
                OR (
                    credential_secret IS NULL
                    AND credential_ciphertext IS NOT NULL
                )
            )
        )
    );

ALTER TABLE workspace_git_upstreams
    ADD CONSTRAINT workspace_git_upstreams_credential_secret_bounded CHECK (
        credential_secret IS NULL
        OR char_length(credential_secret) BETWEEN 1 AND 8192
    );

CREATE TABLE IF NOT EXISTS git_gateway_signing_key (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE,
    signing_key BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT git_gateway_signing_key_singleton CHECK (singleton),
    CONSTRAINT git_gateway_signing_key_length CHECK (octet_length(signing_key) = 32)
);
