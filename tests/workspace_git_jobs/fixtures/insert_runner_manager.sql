INSERT INTO runner_managers (name, endpoint, access_token_hash)
VALUES ($1, $2, $3)
RETURNING runner_manager_id;
