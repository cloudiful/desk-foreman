-- Owner-scoped lookup for the private agent-service gateway (issue 749 P2b).
--
-- Scoped by both application and environment so a foreign or unknown id is
-- simply absent. Returns only what admission needs: the authorized
-- generation, the provision candidate generation, the immutable manager
-- binding, and the checked state. No external user id, state volume
-- reference, or any credential material is selected here.
SELECT
    generation,
    container_generation,
    assigned_manager_id,
    state
FROM execution_environments
WHERE environment_id = $1
  AND application_id = $2
LIMIT 1;
