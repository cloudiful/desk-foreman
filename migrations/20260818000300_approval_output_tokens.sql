ALTER TABLE approval_settings
    ADD COLUMN IF NOT EXISTS max_output_tokens BIGINT NOT NULL DEFAULT 1024;

ALTER TABLE applications
    ADD COLUMN IF NOT EXISTS approval_max_output_tokens BIGINT NULL;
