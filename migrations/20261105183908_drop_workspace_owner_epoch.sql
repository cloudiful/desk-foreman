ALTER TABLE workspace_git_jobs DROP COLUMN owner_epoch;
ALTER TABLE runner_owner_stop_fence DROP COLUMN owner_epoch;
ALTER TABLE runner_principal_admission DROP COLUMN owner_epoch;
ALTER TABLE workspace_runners DROP COLUMN owner_epoch;
ALTER TABLE workspace_bindings DROP COLUMN lifecycle_epoch;
