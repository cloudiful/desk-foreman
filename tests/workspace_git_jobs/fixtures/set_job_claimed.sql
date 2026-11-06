UPDATE workspace_git_jobs
SET state = 'claimed',
    claimed_at = NOW(),
    updated_at = NOW()
WHERE job_id = $1
  AND state = 'pending'
RETURNING job_id;
