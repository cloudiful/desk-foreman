-- Private per-test gate for the cold-start cancellation proof (issue 824 P1r).
--
-- The session-level advisory lock is held by an independent connection. This
-- trigger takes the same lock at *transaction* level, so it blocks exactly
-- while the gate is held and releases itself when the surrounding transaction
-- ends. It fires before the environment UPDATE writes anything, which is after
-- the same transaction has already inserted the start job and moved the task to
-- `starting`: aborting there proves the whole cold-start transaction rolls
-- back, rather than a query that never started.
--
-- Objects are created only inside the owning test's own isolated database and
-- disappear with it.

CREATE OR REPLACE FUNCTION df_cold_start_cancel_gate() RETURNS trigger AS $gate$
BEGIN
    PERFORM pg_advisory_xact_lock(824_000_001);
    RETURN NEW;
END;
$gate$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS df_cold_start_cancel_gate_trigger ON execution_environments;

CREATE TRIGGER df_cold_start_cancel_gate_trigger
    BEFORE UPDATE ON execution_environments
    FOR EACH ROW
    WHEN (NEW.state = 'starting' AND NEW.container_generation IS NOT NULL)
    EXECUTE FUNCTION df_cold_start_cancel_gate();
