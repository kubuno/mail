-- Idempotency keys for POST /send.
--
-- A retried send (a mobile client re-sending after a flaky network) that
-- carries the same `Idempotency-Key` header must not deliver the mail twice.
-- The first request CLAIMS `(user_id, key)` and, once the send succeeds, stores
-- its JSON response here; a repeat with the same key replays that stored
-- response without sending again. Two concurrent requests race through an
-- `INSERT … ON CONFLICT DO NOTHING`, so only one ever sends.
--
-- Rows are short-lived: each keyed send lazily purges entries older than ~24 h,
-- so the table stays small without a scheduled job.
CREATE TABLE IF NOT EXISTS mail.send_idempotency (
    user_id       UUID        NOT NULL,
    key           TEXT        NOT NULL,
    -- NULL while the first send is still in flight; the stored response once it
    -- has succeeded. A repeat sees NULL and refuses (send in progress), or the
    -- response and replays it.
    response_json JSONB,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, key)
);

-- Supports the lazy TTL purge (`WHERE created_at < NOW() - INTERVAL '24 hours'`).
CREATE INDEX IF NOT EXISTS idx_mail_send_idempotency_created_at
    ON mail.send_idempotency(created_at);
