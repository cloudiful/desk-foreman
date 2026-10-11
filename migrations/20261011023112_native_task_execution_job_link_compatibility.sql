-- Preserve nullable task links for legacy ensure/stop jobs while requiring
-- typed task ownership for every native preparation and start job.
ALTER TABLE execution_environment_jobs
    DROP CONSTRAINT execution_environment_jobs_native_kind_requires_task;

ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_native_kind_requires_task CHECK (
        job_kind NOT IN ('prepare_native_task', 'start_native_task')
        OR native_task_id IS NOT NULL
    );
