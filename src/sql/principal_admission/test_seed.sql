INSERT INTO applications (application_id, name) VALUES (817001, 'p4j1-ledger') ON CONFLICT DO NOTHING;
INSERT INTO workspace_bindings (workspace_binding_id, application_id, external_user_id, workspace_key, external_user_hash, workspace_root)
VALUES (817002, 817001, 'p4j1', 'p4j1', 'p4j1', '/tmp/p4j1') ON CONFLICT DO NOTHING;
INSERT INTO runner_managers (runner_manager_id, name, endpoint, access_token_hash)
VALUES (817003, 'p4j1', 'test', 'p4j1') ON CONFLICT DO NOTHING;
