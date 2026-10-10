-- Lock the environment before expiring any linked operation row. Preparation
-- acknowledgements use the same environment-first order before task/job writes.
SELECT 1 AS locked
FROM execution_environments
WHERE application_id = $1
  AND environment_id = $2
FOR UPDATE;
