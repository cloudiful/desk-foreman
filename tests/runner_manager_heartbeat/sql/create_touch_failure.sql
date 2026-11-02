CREATE TABLE runner_manager_touch_failure (fail BOOLEAN NOT NULL);
INSERT INTO runner_manager_touch_failure (fail) VALUES (FALSE);

CREATE FUNCTION fail_runner_manager_touch() RETURNS trigger AS $$
BEGIN
    IF (SELECT fail FROM runner_manager_touch_failure LIMIT 1) THEN
        RAISE EXCEPTION 'forced runner manager touch failure';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER runner_manager_touch_failure_trigger
BEFORE UPDATE ON runner_managers
FOR EACH ROW EXECUTE FUNCTION fail_runner_manager_touch();
