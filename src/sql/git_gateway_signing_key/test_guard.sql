-- Identity proof for the issue-owned signing-key test database (issue 1180 P39).
-- Read before any cleanup DELETE so the signing-key test can never mutate a
-- shared or production singleton.
SELECT
    current_database() AS database_name,
    shobj_description(
        (SELECT oid FROM pg_database WHERE datname = current_database()),
        'pg_database'
    ) AS ownership_comment;
