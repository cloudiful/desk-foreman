-- Encrypted workspace Git upstream credentials and the credential reset
-- (issue 18 P1).
--
-- A stored upstream credential is one authenticated ciphertext frame in
-- `credential_ciphertext`, bound to the row that holds it. The plaintext
-- `credential_secret` column is removed, so no plaintext secret is persisted.
--
-- Existing credential material is cleared in the same transaction: the
-- username is preserved so the row reports an explicit
-- `credential_resupply_required` state instead of forwarding anonymously, and
-- the capability generation of every affected row is incremented once so a
-- capability minted for the old material stops working. Rows that never held a
-- credential keep their generation and stay anonymous.

ALTER TABLE workspace_git_upstreams
    DROP CONSTRAINT workspace_git_upstreams_credential_shape;

ALTER TABLE workspace_git_upstreams
    DROP CONSTRAINT workspace_git_upstreams_credential_secret_bounded;

UPDATE workspace_git_upstreams
SET credential_ciphertext = NULL,
    credential_updated_at = NULL,
    capability_generation = capability_generation + 1,
    updated_at = NOW()
WHERE credential_secret IS NOT NULL
   OR credential_ciphertext IS NOT NULL
   OR credential_updated_at IS NOT NULL;

ALTER TABLE workspace_git_upstreams
    DROP COLUMN credential_secret;

ALTER TABLE workspace_git_upstreams
    ADD CONSTRAINT workspace_git_upstreams_credential_shape CHECK (
        (
            credential_ciphertext IS NULL
            AND credential_updated_at IS NULL
        )
        OR (
            credential_username IS NOT NULL
            AND credential_ciphertext IS NOT NULL
            AND credential_updated_at IS NOT NULL
        )
    );

ALTER TABLE workspace_git_upstreams
    ADD CONSTRAINT workspace_git_upstreams_credential_ciphertext_bounded CHECK (
        credential_ciphertext IS NULL
        OR octet_length(credential_ciphertext) <= 65536
    );
