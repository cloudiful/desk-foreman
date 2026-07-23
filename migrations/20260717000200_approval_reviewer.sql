CREATE TABLE IF NOT EXISTS approval_settings (
    settings_id SMALLINT PRIMARY KEY CHECK (settings_id = 1),
    endpoint TEXT NULL,
    model TEXT NULL,
    timeout_ms BIGINT NOT NULL DEFAULT 10000,
    max_input_bytes BIGINT NOT NULL DEFAULT 131072,
    max_concurrent BIGINT NOT NULL DEFAULT 8,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO approval_settings (settings_id)
VALUES (1)
ON CONFLICT (settings_id) DO NOTHING;

ALTER TABLE applications
    ADD COLUMN IF NOT EXISTS approval_mode TEXT NOT NULL DEFAULT 'inherit',
    ADD COLUMN IF NOT EXISTS approval_endpoint TEXT NULL,
    ADD COLUMN IF NOT EXISTS approval_model TEXT NULL;

ALTER TABLE applications
    DROP CONSTRAINT IF EXISTS applications_approval_mode_check;

ALTER TABLE applications
    ADD CONSTRAINT applications_approval_mode_check
    CHECK (approval_mode IN ('inherit', 'disabled', 'enabled'));
