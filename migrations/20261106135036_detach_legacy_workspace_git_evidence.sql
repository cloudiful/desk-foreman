-- Detach the inert legacy workspace Git evidence from generic binding and
-- manager cleanup. Only the two restrictive foreign keys are dropped; the
-- historical table and its rows remain as proof.
ALTER TABLE workspace_git_jobs
    DROP CONSTRAINT workspace_git_jobs_binding_application_fk;

ALTER TABLE workspace_git_jobs
    DROP CONSTRAINT workspace_git_jobs_assigned_manager_id_fkey;
