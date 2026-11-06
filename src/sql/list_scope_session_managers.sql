-- Configured runner managers observed before a scoped intent
-- (issue 1180).
--
-- Every configured manager is listed regardless of enabled/online/heartbeat
-- state: a manager that cannot be observed must block admission rather than be
-- assumed session-free. The list is a snapshot; no manager created later can
-- have received a pre-fence resource dispatch under this control plane.
SELECT runner_manager_id
FROM runner_managers
ORDER BY runner_manager_id;
