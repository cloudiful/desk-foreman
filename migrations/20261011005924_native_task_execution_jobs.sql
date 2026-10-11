-- Durable native-task start/stop execution jobs and state (issue 764 P1).
--
-- Start and task-aware stop remain control-plane intents. A task keeps its
-- exclusivity anchors until a typed stop proves environment and preparation
-- quiescence and the application explicitly releases it.

-- 1. Native start is a typed job kind. Both preparation and start jobs require
-- a native task link; legacy ensure/stop rows remain compatible.
ALTER TABLE execution_environment_jobs
    DROP CONSTRAINT execution_environment_jobs_job_kind_check;
ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_job_kind_check CHECK (
        job_kind IN ('ensure', 'stop', 'prepare_native_task', 'start_native_task')
    );

ALTER TABLE execution_environment_jobs
    DROP CONSTRAINT execution_environment_jobs_native_kind_requires_task;
ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_native_kind_requires_task CHECK (
        (job_kind IN ('prepare_native_task', 'start_native_task') AND native_task_id IS NOT NULL)
        OR job_kind = 'stop'
        OR native_task_id IS NULL
    );

CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_jobs_active_native_start_key
    ON execution_environment_jobs (native_task_id)
    WHERE job_kind = 'start_native_task'
      AND state IN ('pending', 'claimed', 'uncertain');

-- 2. The task and environment retain typed start/stop job anchors.
ALTER TABLE execution_environment_tasks
    ADD COLUMN start_job_id UUID NULL
        REFERENCES execution_environment_jobs (job_id) ON DELETE RESTRICT;
ALTER TABLE execution_environment_tasks
    ADD COLUMN stop_job_id UUID NULL
        REFERENCES execution_environment_jobs (job_id) ON DELETE RESTRICT;

-- 3. `starting` is an active task state. The migration is additive and leaves
-- all previous migrations immutable.
ALTER TABLE execution_environment_tasks
    DROP CONSTRAINT execution_environment_tasks_state_check;
ALTER TABLE execution_environment_tasks
    ADD CONSTRAINT execution_environment_tasks_state_check CHECK (
        state IN (
            'admitted', 'preparing', 'prepared', 'starting',
            'running', 'stopping', 'uncertain', 'released'
        )
    );

DROP INDEX IF EXISTS execution_environment_tasks_active_environment_key;
CREATE UNIQUE INDEX execution_environment_tasks_active_environment_key
    ON execution_environment_tasks (environment_id)
    WHERE state IN (
        'admitted', 'preparing', 'prepared', 'starting',
        'running', 'stopping', 'uncertain'
    );

DROP INDEX IF EXISTS execution_environment_tasks_active_binding_key;
CREATE UNIQUE INDEX execution_environment_tasks_active_binding_key
    ON execution_environment_tasks (workspace_binding_id)
    WHERE state IN (
        'admitted', 'preparing', 'prepared', 'starting',
        'running', 'stopping', 'uncertain'
    );
