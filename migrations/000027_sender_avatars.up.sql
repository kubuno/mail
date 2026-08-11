-- Cache of sender avatars resolved from public, domain-level sources (BIMI).
-- Keyed by DOMAIN, never by address: nothing personal is stored, and one lookup
-- serves every sender of that domain. Failures are cached too (`not_found`), so
-- a domain without BIMI is not queried again on every message opened.
CREATE TABLE IF NOT EXISTS mail.sender_avatars (
    domain      VARCHAR(255) PRIMARY KEY,
    source      VARCHAR(16)  NOT NULL DEFAULT 'bimi',
    mime        VARCHAR(64),
    bytes       BYTEA,
    not_found   BOOLEAN      NOT NULL DEFAULT FALSE,
    fetched_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    expires_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW() + INTERVAL '7 days'
);

CREATE INDEX IF NOT EXISTS idx_mail_sender_avatars_expiry ON mail.sender_avatars(expires_at);
