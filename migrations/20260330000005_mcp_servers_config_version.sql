-- Migration: mcp_servers_config_version
-- Adds config_version column to mcp_servers for LISTEN/NOTIFY-based hot-reload.
-- Each UPDATE increments config_version so the gateway can discard out-of-order
-- notifications. A trigger publishes every INSERT/UPDATE/DELETE as a JSON
-- payload on the 'mcp_server_changes' PostgreSQL channel.

-- ── config_version column ─────────────────────────────────────────────────────

ALTER TABLE mcp_servers
    ADD COLUMN config_version BIGINT NOT NULL DEFAULT 1;

-- ── Updated trigger_set_updated_at ────────────────────────────────────────────
-- Replace the shared trigger function to also increment config_version on UPDATE.
-- The CREATE OR REPLACE ensures all tables using this function pick up the change.

CREATE OR REPLACE FUNCTION trigger_set_updated_at()
    RETURNS TRIGGER
    LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at = NOW();
    -- Increment config_version on every row update so notifications carry a
    -- monotonically increasing version that receivers can use to discard stale
    -- out-of-order deliveries.
    IF TG_TABLE_NAME = 'mcp_servers' AND TG_OP = 'UPDATE' THEN
        NEW.config_version = OLD.config_version + 1;
    END IF;
    RETURN NEW;
END;
$$;

-- ── NOTIFY trigger function ───────────────────────────────────────────────────

CREATE OR REPLACE FUNCTION notify_mcp_server_changes()
    RETURNS TRIGGER
    LANGUAGE plpgsql
AS $$
DECLARE
    payload TEXT;
    sid     UUID;
    ver     BIGINT;
BEGIN
    IF TG_OP = 'DELETE' THEN
        sid := OLD.id;
        ver := OLD.config_version;
    ELSE
        sid := NEW.id;
        ver := NEW.config_version;
    END IF;

    -- Payload format: {"server_id":"<uuid>","op":"insert|update|delete","config_version":<n>}
    payload := json_build_object(
        'server_id',      sid::text,
        'op',             lower(TG_OP),
        'config_version', ver
    )::text;

    PERFORM pg_notify('mcp_server_changes', payload);

    -- AFTER trigger; return value is ignored for row-level triggers.
    RETURN NULL;
END;
$$;

-- ── Attach notify trigger to mcp_servers ─────────────────────────────────────

CREATE TRIGGER mcp_servers_notify_changes
    AFTER INSERT OR UPDATE OR DELETE ON mcp_servers
    FOR EACH ROW EXECUTE FUNCTION notify_mcp_server_changes();
