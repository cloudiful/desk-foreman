SELECT
    approval_api_key_ciphertext AS api_key_ciphertext,
    approval_api_key_nonce AS api_key_nonce,
    approval_api_key_key_version AS api_key_key_version
FROM applications
WHERE application_id = $1
LIMIT 1;
