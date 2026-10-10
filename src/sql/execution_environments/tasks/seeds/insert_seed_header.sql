-- Seal one snapshot header (issue 753 P2a).
--
-- `ON CONFLICT DO NOTHING` makes a racing seal harmless: the environment row
-- lock already serializes seals of the same task, and a loser re-reads the
-- committed winner and compares content instead of raising a unique violation.
INSERT INTO execution_environment_task_seeds (
    task_id,
    manifest_sha256,
    file_count,
    total_bytes
)
VALUES ($1, $2, $3, $4)
ON CONFLICT DO NOTHING
RETURNING
    task_id,
    manifest_sha256,
    file_count,
    total_bytes;
