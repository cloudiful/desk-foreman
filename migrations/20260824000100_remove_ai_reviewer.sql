DROP TABLE IF EXISTS approval_settings;
DROP TABLE IF EXISTS app_secret;

ALTER TABLE applications
    DROP CONSTRAINT IF EXISTS applications_approval_mode_check;

ALTER TABLE applications
    DROP COLUMN IF EXISTS approval_mode,
    DROP COLUMN IF EXISTS approval_endpoint,
    DROP COLUMN IF EXISTS approval_model,
    DROP COLUMN IF EXISTS approval_timeout_ms,
    DROP COLUMN IF EXISTS approval_max_input_bytes,
    DROP COLUMN IF EXISTS approval_max_concurrent,
    DROP COLUMN IF EXISTS approval_max_output_tokens,
    DROP COLUMN IF EXISTS approval_api_key_ciphertext,
    DROP COLUMN IF EXISTS approval_api_key_nonce,
    DROP COLUMN IF EXISTS approval_api_key_key_version;
