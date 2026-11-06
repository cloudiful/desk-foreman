UPDATE workspace_git_jobs
SET deadline_at = $2,
    updated_at = NOW()
WHERE job_id = $1
RETURNING job_id;
