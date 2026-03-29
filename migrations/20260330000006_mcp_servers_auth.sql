-- Migration: mcp_servers_auth
-- Adds per-server Bearer token columns to mcp_servers so the gateway can
-- validate incoming requests without querying the server_tokens table on every
-- request. The raw token is never stored; only the Argon2id PHC hash.
--
-- token_hash   : Argon2id PHC string ($argon2id$...) produced by generate_token().
--               NULL means no token has been configured for this server yet.
-- token_prefix : First 8 characters of the raw token, safe to include in logs.
--               NULL when token_hash is NULL.
--
-- Adding 'suspended' to the status check constraint so the gateway can
-- distinguish a suspended server (403) from an inactive one.

ALTER TABLE mcp_servers
    ADD COLUMN token_hash   TEXT,
    ADD COLUMN token_prefix CHAR(8);

-- Extend the status check constraint to include 'suspended'.
-- The original inline CHECK on the column is automatically named
-- mcp_servers_status_check by PostgreSQL's naming convention.
ALTER TABLE mcp_servers
    DROP CONSTRAINT mcp_servers_status_check;

ALTER TABLE mcp_servers
    ADD CONSTRAINT mcp_servers_status_check
    CHECK (status IN ('draft', 'active', 'inactive', 'suspended'));
