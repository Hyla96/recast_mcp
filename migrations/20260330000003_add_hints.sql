-- Migration: add_hints
-- Adds hint columns to credentials and server_tokens for non-sensitive display.
--
-- The hint is computed at creation time from the plaintext value and stored
-- alongside the encrypted payload. Since the value is not reversible from
-- the ciphertext or hash, the hint must be persisted independently.
--
-- Credentials hint: first 4 chars + "****" (e.g. "supe****").
-- Server tokens hint: first 12 chars of the token + "****"
--   (e.g. "mcp_live_XXX****").

ALTER TABLE credentials   ADD COLUMN hint TEXT;
ALTER TABLE server_tokens ADD COLUMN hint TEXT;
