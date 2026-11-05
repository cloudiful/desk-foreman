SELECT
    job_id,
    token,
    state,
    cleanup_confirmed,
    claimed_at,
    terminal_at,
    reserved_bytes,
    principal_digest
FROM workspace_git_jobs
WHERE job_id = $1;
