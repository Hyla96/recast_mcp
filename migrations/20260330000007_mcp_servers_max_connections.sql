-- Add per-server connection limit column.
--
-- Default of 50 matches the gateway constant DEFAULT_SERVER_MAX_CONNECTIONS.
-- Existing rows receive the default value automatically.

ALTER TABLE mcp_servers
    ADD COLUMN max_connections INTEGER NOT NULL DEFAULT 50;
