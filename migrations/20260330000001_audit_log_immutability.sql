-- Migration: audit_log_immutability
-- Adds a PostgreSQL trigger that raises an exception on any UPDATE or DELETE
-- on the audit_log table, enforcing its append-only, immutable character.
--
-- Application-level immutability is enforced here at the database level so
-- that even direct psql access or future code paths cannot mutate audit
-- records without explicitly removing this trigger.

CREATE OR REPLACE FUNCTION prevent_audit_log_modification()
    RETURNS TRIGGER
    LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION
        'audit_log is immutable: UPDATE and DELETE are not permitted (action=%, id=%)',
        TG_OP,
        OLD.id;
END;
$$;

CREATE TRIGGER audit_log_immutable
    BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION prevent_audit_log_modification();
