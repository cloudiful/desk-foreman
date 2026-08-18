SELECT secret_value
FROM app_secret
WHERE secret_name = $1
LIMIT 1;
