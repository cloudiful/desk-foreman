-- Retain the authorization fence that admitted each task-aware stop job.
ALTER TABLE execution_environment_jobs
    ADD COLUMN admission_generation BIGINT;

ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_admission_generation_nonnegative
    CHECK (admission_generation IS NULL OR admission_generation >= 0);
