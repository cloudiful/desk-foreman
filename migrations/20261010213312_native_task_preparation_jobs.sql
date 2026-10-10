-- Original-manager durable task preparation (issue 753 P2b).
--
-- Extends the durable job record with a typed native-task link and one new job
-- kind, adds the task's link to its preparation job, and makes `prepared` a
-- first-class active task state so a task with a materialized checkout keeps
-- its exclusive environment and workspace exactly like the other active states.
--
-- Additive forward migration only: no prior migration is edited. The existing
-- ensure/stop kinds keep a nullable `native_task_id`, and every existing
-- lifecycle query selects its own explicit column set.

-- 1. Job kind gains the preparation kind.
ALTER TABLE execution_environment_jobs
    DROP CONSTRAINT execution_environment_jobs_job_kind_check;
ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_job_kind_check CHECK (
        job_kind IN ('ensure', 'stop', 'prepare_native_task')
    );

-- 2. A native preparation job must name its task; ensure/stop may still leave it
-- NULL, because a future stop links a task only when one exists.
ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_native_kind_requires_task CHECK (
        (job_kind = 'prepare_native_task' AND native_task_id IS NOT NULL)
        OR (job_kind <> 'prepare_native_task')
    );

-- 3. One active preparation job per task, so a second concurrent preparation
-- can never be enqueued; the loser replays or conflicts.
CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_jobs_active_preparation_key
    ON execution_environment_jobs (native_task_id)
    WHERE job_kind = 'prepare_native_task'
      AND state IN ('pending', 'claimed', 'uncertain');

-- 4. The task's link to its preparation job, and the `prepared` active state.
ALTER TABLE execution_environment_tasks
    ADD COLUMN prepare_job_id UUID NULL
        REFERENCES execution_environment_jobs (job_id) ON DELETE RESTRICT;

ALTER TABLE execution_environment_tasks
    DROP CONSTRAINT execution_environment_tasks_state_check;
ALTER TABLE execution_environment_tasks
    ADD CONSTRAINT execution_environment_tasks_state_check CHECK (
        state IN (
            'admitted', 'preparing', 'prepared',
            'running', 'stopping', 'uncertain', 'released'
        )
    );

-- A task may only point at a preparation job it owns.
ALTER TABLE execution_environment_tasks
    ADD CONSTRAINT execution_environment_tasks_prepare_job_fk
        FOREIGN KEY (prepare_job_id)
        REFERENCES execution_environment_jobs (job_id) ON DELETE RESTRICT;

-- 5. `prepared` occupies the environment and the binding like every other
-- active state, so the partial unique indexes and the guard queries agree.
DROP INDEX IF EXISTS execution_environment_tasks_active_environment_key;
CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_tasks_active_environment_key
    ON execution_environment_tasks (environment_id)
    WHERE state IN ('admitted', 'preparing', 'prepared', 'running', 'stopping', 'uncertain');

DROP INDEX IF EXISTS execution_environment_tasks_active_binding_key;
CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_tasks_active_binding_key
    ON execution_environment_tasks (workspace_binding_id)
    WHERE state IN ('admitted', 'preparing', 'prepared', 'running', 'stopping', 'uncertain');
