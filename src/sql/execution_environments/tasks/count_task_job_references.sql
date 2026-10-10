-- Count durable jobs that reference one task (issue 753 P1).
--
-- A non-zero count means something was dispatched for this task, so a cancel
-- cannot void it as never-started.
SELECT COUNT(*) AS references
FROM execution_environment_jobs
WHERE native_task_id = $1;
