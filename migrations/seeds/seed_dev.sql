-- Development seed data.
-- Idempotent: safe to run multiple times; uses INSERT ... ON CONFLICT DO NOTHING.
-- Provides one test user, one test server, one encrypted credential, and one
-- server token sufficient for local development and integration tests.
--
-- Usage:
--   psql $DATABASE_URL -f migrations/seeds/seed_dev.sql
--   or
--   just db-seed

-- ─── Test user ───────────────────────────────────────────────────────────────
-- Clerk ID and email match the local dev Clerk test account.
INSERT INTO users (id, clerk_id, email, plan)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'user_dev_test_clerk_id',
    'dev@example.com',
    'pro'
)
ON CONFLICT DO NOTHING;

-- ─── Test MCP server ─────────────────────────────────────────────────────────
-- A minimal "active" server so the gateway can serve requests immediately after
-- seeding without requiring the builder UI flow.
INSERT INTO mcp_servers (id, user_id, name, slug, config_json, status)
VALUES (
    '00000000-0000-0000-0000-000000000002',
    '00000000-0000-0000-0000-000000000001',
    'Dev GitHub Server',
    'dev-github',
    '{
        "tools": [
            {
                "name": "list_repos",
                "description": "List GitHub repositories for a user",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "username": { "type": "string", "description": "GitHub username" }
                    },
                    "required": ["username"]
                },
                "upstream": {
                    "method": "GET",
                    "url": "https://api.github.com/users/{username}/repos"
                }
            }
        ],
        "rate_limit": { "calls_per_minute": 100 }
    }',
    'active'
)
ON CONFLICT DO NOTHING;

-- ─── Encrypted credential ─────────────────────────────────────────────────────
-- Placeholder encrypted payload. The actual bytes are a dummy AES-256-GCM
-- ciphertext that does NOT decrypt to anything useful — replace via the Platform
-- API or credential-injector in real dev usage. The iv column stores the 12-byte
-- GCM nonce; encrypted_payload stores the ciphertext (without the IV prepended,
-- since the mcp-crypto library returns iv || ciphertext concatenated and the
-- service layer splits them on write).
--
-- To generate real values for local testing, run:
--   cargo run --example encrypt_credential -- "Bearer ghp_your_token_here"
INSERT INTO credentials (id, server_id, auth_type, encrypted_payload, iv)
VALUES (
    '00000000-0000-0000-0000-000000000003',
    '00000000-0000-0000-0000-000000000002',
    'bearer',
    -- 32-byte placeholder ciphertext (AES-256-GCM output for empty plaintext with dummy key)
    decode('000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000', 'hex'),
    -- 12-byte placeholder IV / nonce
    decode('000000000000000000000000', 'hex')
)
ON CONFLICT DO NOTHING;

-- ─── Server token ─────────────────────────────────────────────────────────────
-- SHA-256 hash of the literal string "dev-token-do-not-use-in-production".
-- MCP clients connecting to the dev gateway should send:
--   Authorization: Bearer dev-token-do-not-use-in-production
INSERT INTO server_tokens (id, server_id, token_hash, description)
VALUES (
    '00000000-0000-0000-0000-000000000004',
    '00000000-0000-0000-0000-000000000002',
    -- SHA-256("dev-token-do-not-use-in-production")
    'f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b',
    'Development test token — NOT FOR PRODUCTION USE'
)
ON CONFLICT DO NOTHING;
