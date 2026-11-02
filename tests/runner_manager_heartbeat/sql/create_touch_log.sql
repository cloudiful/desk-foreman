CREATE TABLE runner_manager_touch_log (
    runner_manager_id BIGINT NOT NULL,
    touched_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE FUNCTION log_runner_manager_touch() RETURNS trigger AS $$
BEGIN
    INSERT INTO runner_manager_touch_log (runner_manager_id)
    VALUES (NEW.runner_manager_id);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER runner_manager_touch_log_trigger
AFTER UPDATE ON runner_managers
FOR EACH ROW EXECUTE FUNCTION log_runner_manager_touch();
