-- Identity proof for the task-owned issue database (issue 1180 DFI3a1a).
-- Read before the first migration on the connection.
SELECT
    current_database() AS database_name,
    current_setting('server_version') AS server_version,
    shobj_description(
        (SELECT oid FROM pg_database WHERE datname = current_database()),
        'pg_database'
    ) AS ownership_comment;
