-- Migration: initial_schema
-- Creates the five core tables: users, mcp_servers, credentials,
-- server_tokens, audit_log, with indexes and updated_at triggers.

-- Ensure pgcrypto extension is available for gen_random_uuid().
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ─── updated_at trigger function ─────────────────────────────────────────────
-- Shared trigger function; installed once, referenced by all triggers below.
CREATE OR REPLACE FUNCTION trigger_set_updated_at()
    RETURNS TRIGGER
    LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

-- ─── users ───────────────────────────────────────────────────────────────────
-- Platform users authenticated via Clerk. Minimal data: auth identity lives in
-- Clerk; we store only what the platform needs for routing and billing.
CREATE TABLE users (
    id         UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    clerk_id   TEXT        NOT NULL UNIQUE,
    email      TEXT        NOT NULL UNIQUE,
    plan       TEXT        NOT NULL DEFAULT 'community'
                           CHECK (plan IN ('community', 'pro', 'team', 'enterprise')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TRIGGER users_set_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION trigger_set_updated_at();

-- ─── mcp_servers ─────────────────────────────────────────────────────────────
-- Each row is one user-created MCP server configuration. config_json stores
-- the full tool definitions, upstream URLs, field mappings, and rate limit
-- overrides; kept as JSONB so the gateway can load it in a single query.
CREATE TABLE mcp_servers (
    id          UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name        TEXT        NOT NULL,
    slug        TEXT        NOT NULL,
    config_json JSONB       NOT NULL DEFAULT '{}',
    status      TEXT        NOT NULL DEFAULT 'draft'
                            CHECK (status IN ('draft', 'active', 'inactive')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, slug)
);

CREATE TRIGGER mcp_servers_set_updated_at
    BEFORE UPDATE ON mcp_servers
    FOR EACH ROW EXECUTE FUNCTION trigger_set_updated_at();

-- Gateway hot-path: slug lookup restricted to active servers only.
-- EXPLAIN ANALYZE must show index scan (no seq scan) for:
--   SELECT ... FROM mcp_servers WHERE slug = $1 AND status = 'active'
CREATE INDEX idx_mcp_servers_slug_active
    ON mcp_servers (slug)
    WHERE status = 'active';

-- FK join acceleration.
CREATE INDEX idx_mcp_servers_user_id ON mcp_servers (user_id);

-- ─── credentials ─────────────────────────────────────────────────────────────
-- AES-256-GCM encrypted credential payloads. The gateway never reads these
-- directly; the credential-injector sidecar holds the only decryption path.
-- Per-row IV (12 bytes / 96 bits for GCM) ensures identical plaintexts produce
-- different ciphertexts. encrypted_payload stores IV || ciphertext contiguously
-- (the mcp-crypto library prepends the IV before storing).
CREATE TABLE credentials (
    id                UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    server_id         UUID        NOT NULL REFERENCES mcp_servers (id) ON DELETE CASCADE,
    auth_type         TEXT        NOT NULL
                                  CHECK (auth_type IN
                                         ('bearer', 'api_key_header',
                                          'api_key_query', 'basic')),
    encrypted_payload BYTEA       NOT NULL,
    iv                BYTEA       NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- FK join acceleration.
CREATE INDEX idx_credentials_server_id ON credentials (server_id);

-- ─── server_tokens ───────────────────────────────────────────────────────────
-- Per-server Bearer tokens issued to MCP clients. The raw token is never
-- stored; only the SHA-256 hash is persisted. The gateway validates incoming
-- Authorization headers by hashing the presented value and comparing.
CREATE TABLE server_tokens (
    id          UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    server_id   UUID        NOT NULL REFERENCES mcp_servers (id) ON DELETE CASCADE,
    token_hash  TEXT        NOT NULL UNIQUE,
    description TEXT,
    is_active   BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at  TIMESTAMPTZ
);

-- Gateway hot-path: token validation restricted to active tokens only.
-- EXPLAIN ANALYZE must show index scan (no seq scan) for:
--   SELECT ... FROM server_tokens WHERE token_hash = $1 AND is_active = true
CREATE INDEX idx_server_tokens_hash_active
    ON server_tokens (token_hash)
    WHERE is_active = TRUE;

-- FK join acceleration.
CREATE INDEX idx_server_tokens_server_id ON server_tokens (server_id);

-- ─── audit_log ───────────────────────────────────────────────────────────────
-- Immutable, append-only audit trail. No UPDATE or DELETE should ever touch
-- this table (enforce via application policy; row-level security in growth tier).
-- actor_id is NULL for system-generated events (e.g., SSRF blocks).
CREATE TABLE audit_log (
    id          UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id    UUID,
    action      TEXT        NOT NULL,
    resource_id UUID,
    metadata    JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Query patterns: look up by actor, resource, or time range.
CREATE INDEX idx_audit_log_actor_id    ON audit_log (actor_id)    WHERE actor_id IS NOT NULL;
CREATE INDEX idx_audit_log_resource_id ON audit_log (resource_id) WHERE resource_id IS NOT NULL;
CREATE INDEX idx_audit_log_created_at  ON audit_log (created_at DESC);
