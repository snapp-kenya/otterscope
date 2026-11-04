-- Bearer tokens are stored as digests, carry a capability, and record use.
--
-- 0017 kept the secret itself as the primary key, so a database (or a backup,
-- or the token listing) handed out working credentials. `id` is now only an
-- identifier and `token_hash` is the SHA-256 that a presented token is matched
-- against. SQLite has no SHA-256, so a row that predates this migration keeps
-- its secret in `id` for one moment; the store hashes it and assigns a fresh
-- id on open (see Store.hashLegacyTokens). Until then token_hash is NULL and
-- the row cannot authenticate. `scope` defaults to read so the upgrade cannot
-- silently widen an existing token to admin.
ALTER TABLE read_tokens RENAME COLUMN token TO id;
ALTER TABLE read_tokens ADD COLUMN token_hash TEXT;
ALTER TABLE read_tokens ADD COLUMN scope TEXT NOT NULL DEFAULT 'read';
ALTER TABLE read_tokens ADD COLUMN last_used_ns INTEGER;
CREATE UNIQUE INDEX read_tokens_hash_idx ON read_tokens (token_hash);
