-- Migration: credentials_key_name
-- Adds an optional key_name column to the credentials table.
--
-- For api_key_header credentials: the HTTP header name (e.g. "X-API-Key").
-- For api_key_query credentials: the query parameter name (e.g. "api_key").
-- For bearer and basic credentials: NULL (not applicable).

ALTER TABLE credentials ADD COLUMN key_name TEXT;
