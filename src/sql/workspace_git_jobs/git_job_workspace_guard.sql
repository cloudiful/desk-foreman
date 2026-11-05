-- Any pending, claimed, or uncertain Git job blocks binding writes,
-- acquire, takeover, archive, and cleanup. The id is for internal logging
-- only and is never returned to a transport caller.
SELECT job_id
FROM workspace_git_jobs
WHERE workspace_binding_id = $1
  AND state IN ('pending', 'claimed', 'uncertain')
LIMIT 1;
