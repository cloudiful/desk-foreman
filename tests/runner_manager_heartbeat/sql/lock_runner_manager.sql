SELECT runner_manager_id
FROM runner_managers
WHERE runner_manager_id = $1
FOR UPDATE;
