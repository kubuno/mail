-- OAuth2 (XOAUTH2) authentication for mail accounts (Gmail, Microsoft).
-- Accounts keep working with passwords by default; OAuth accounts store
-- AES-256-GCM encrypted refresh/access tokens instead.
ALTER TABLE mail.accounts
    ADD COLUMN IF NOT EXISTS auth_kind TEXT NOT NULL DEFAULT 'password'
        CHECK (auth_kind IN ('password', 'oauth_google', 'oauth_microsoft')),
    ADD COLUMN IF NOT EXISTS oauth_refresh_token BYTEA,
    ADD COLUMN IF NOT EXISTS oauth_refresh_nonce BYTEA,
    ADD COLUMN IF NOT EXISTS oauth_access_token  BYTEA,
    ADD COLUMN IF NOT EXISTS oauth_access_nonce  BYTEA,
    ADD COLUMN IF NOT EXISTS oauth_expires_at    TIMESTAMPTZ;

-- Short-lived CSRF `state` values for the OAuth authorization-code flow.
-- Rows older than 10 minutes are purged opportunistically by the handlers.
CREATE TABLE IF NOT EXISTS mail.oauth_states (
    state      TEXT PRIMARY KEY,
    user_id    UUID NOT NULL,
    provider   TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
