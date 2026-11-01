INSERT INTO users (user_id, login_name, password_hash, display_name, email, timezone, is_admin) VALUES (817301, 'p4k2-admin', 'unused', 'admin', 'p4k2@example.invalid', 'UTC', true);
INSERT INTO web_sessions (session_id, user_id, expires_at) VALUES ('00000000-0000-4000-8000-000000000301', 817301, now() + interval '1 day');
INSERT INTO applications (application_id, name) VALUES (817301, 'p4k2');
