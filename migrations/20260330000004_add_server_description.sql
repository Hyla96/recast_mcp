-- Migration: add_server_description
-- Adds an optional human-readable description column to mcp_servers.
-- Existing rows default to NULL (no migration of existing data needed).

ALTER TABLE mcp_servers ADD COLUMN description TEXT;
