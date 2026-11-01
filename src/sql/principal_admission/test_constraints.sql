DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid IN ('runner_principal_admission'::regclass, 'runner_owner_stop_fence'::regclass) AND contype = 'f') THEN RAISE EXCEPTION 'ledger must not have foreign keys'; END IF;
  BEGIN INSERT INTO runner_owner_stop_fence(owner_key, application_id, workspace_binding_id, principal_digest) VALUES ('wrong', 1, 2, repeat('a',64)); RAISE EXCEPTION 'bad key accepted'; EXCEPTION WHEN check_violation THEN NULL; END;
  BEGIN INSERT INTO runner_owner_stop_fence(owner_key, application_id, workspace_binding_id, principal_digest) VALUES ('application_principal:1:2:' || repeat('A',64), 1, 2, repeat('A',64)); RAISE EXCEPTION 'bad digest accepted'; EXCEPTION WHEN check_violation THEN NULL; END;
  BEGIN INSERT INTO runner_principal_admission(owner_key, application_id, workspace_binding_id, principal_digest, runner_manager_id, container_id) VALUES ('application_principal:1:2:' || repeat('a',64), 1, 2, repeat('a',64), 1, 'bad id'); RAISE EXCEPTION 'bad container accepted'; EXCEPTION WHEN check_violation THEN NULL; END;
  BEGIN INSERT INTO runner_principal_admission(owner_key, application_id, workspace_binding_id, principal_digest, runner_manager_id, container_id) VALUES ('application_principal:1:2:' || repeat('a',64), 1, 2, repeat('a',64), 0, 'id'); RAISE EXCEPTION 'bad manager accepted'; EXCEPTION WHEN check_violation THEN NULL; END;
END $$;
