-- First-provision binding: select the enabled+online+recently-seen manager
-- internally (issue 742 P3). It remains the environment's immutable binding.
SELECT runner_manager_id
FROM runner_managers
WHERE enabled = TRUE
  AND status = 'online'
  AND last_seen_at > NOW() - ($1::double precision * INTERVAL '1 second')
ORDER BY last_seen_at DESC NULLS LAST, runner_manager_id ASC
LIMIT 1;
