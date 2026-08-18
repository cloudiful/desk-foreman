ALTER TABLE approval_settings
    ADD COLUMN IF NOT EXISTS enabled BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN IF NOT EXISTS api_key_ciphertext BYTEA NULL,
    ADD COLUMN IF NOT EXISTS api_key_nonce BYTEA NULL,
    ADD COLUMN IF NOT EXISTS api_key_key_version SMALLINT NULL;

ALTER TABLE applications
    ADD COLUMN IF NOT EXISTS approval_timeout_ms BIGINT NULL,
    ADD COLUMN IF NOT EXISTS approval_max_input_bytes BIGINT NULL,
    ADD COLUMN IF NOT EXISTS approval_max_concurrent BIGINT NULL,
    ADD COLUMN IF NOT EXISTS approval_api_key_ciphertext BYTEA NULL,
    ADD COLUMN IF NOT EXISTS approval_api_key_nonce BYTEA NULL,
    ADD COLUMN IF NOT EXISTS approval_api_key_key_version SMALLINT NULL;
