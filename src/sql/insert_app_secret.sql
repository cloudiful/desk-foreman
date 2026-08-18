INSERT INTO app_secret (secret_name, secret_value)
VALUES ($1, $2)
ON CONFLICT (secret_name) DO NOTHING
RETURNING secret_value;
