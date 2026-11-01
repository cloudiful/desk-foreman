CREATE FUNCTION p4k1_fail_second_mark() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.container_id = 'owner-b' AND NEW.stopped_at IS NOT NULL THEN
        RAISE EXCEPTION 'injected second mark failure';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER p4k1_fail_second_mark BEFORE UPDATE ON runner_principal_admission
FOR EACH ROW EXECUTE FUNCTION p4k1_fail_second_mark();
