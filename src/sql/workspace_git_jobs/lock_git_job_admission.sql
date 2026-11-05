-- Serialize global Git-job capacity accounting. Taken only after the
-- per-binding row lock; never take the global lock first.
SELECT pg_advisory_xact_lock(hashtext('desk-foreman:workspace-git-jobs'));
