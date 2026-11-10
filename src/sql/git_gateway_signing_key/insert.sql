INSERT INTO git_gateway_signing_key (singleton, signing_key)
VALUES (TRUE, $1)
ON CONFLICT (singleton) DO NOTHING
