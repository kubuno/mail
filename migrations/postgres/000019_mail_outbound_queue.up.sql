-- Reliable outbound delivery + DKIM signing keys (wave B).
--
-- The reliability invariants are Postfix's, rebuilt on PostgreSQL instead of
-- queue files: durability before ack, per-recipient state updated in the SAME
-- transaction as the external effect (idempotency, no double send), atomic
-- claim with SKIP LOCKED + orphan reclaim via a lease, bounded exponential
-- backoff + expiry, and poison isolation. See project_mail_server_pro_roadmap.

-- ── One queued message: the exact bytes to transmit, plus its envelope ────────
CREATE TABLE IF NOT EXISTS mail.outbound_messages (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id       UUID,
    account_id    UUID,
    -- Envelope sender (MAIL FROM). Empty string = the null return path <> used
    -- by every DSN, so a failed DSN cannot itself generate a DSN.
    envelope_from TEXT NOT NULL,
    -- The full RFC 5322 message as it will go on the wire, already DKIM-signed.
    -- Kept verbatim so what we sign is exactly what we send.
    raw           BYTEA NOT NULL,
    -- A DSN (bounce/delay notice). Its own failure is dropped, never re-notified.
    is_dsn        BOOLEAN NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- maximal_queue_lifetime: past this, undelivered recipients are bounced and
    -- the message stops being retried.
    expires_at    TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '5 days'
);

-- ── Per-recipient delivery state — the idempotency unit ───────────────────────
-- One row per (message, recipient). Its status is advanced in the same
-- transaction that records the remote server's answer, so a crash never double
-- sends nor loses a recipient.
CREATE TABLE IF NOT EXISTS mail.outbound_recipients (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    message_id      UUID NOT NULL REFERENCES mail.outbound_messages(id) ON DELETE CASCADE,
    recipient       TEXT NOT NULL,
    -- Destination domain, for per-destination concurrency/backoff.
    domain          TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'queued'
                        CHECK (status IN ('queued', 'delivering', 'sent', 'deferred', 'bounced')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    -- Eligible for a delivery attempt when next_attempt_at <= now(). Backoff
    -- pushes this into the future; the claim query filters on it.
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Last SMTP reply seen, for diagnostics and the eventual DSN.
    last_code       INTEGER,
    last_reason     TEXT,
    -- Worker lease: a claimed row carries the worker id and a deadline. A dead
    -- worker's rows become claimable again once locked_until passes (orphan
    -- reclaim) — nothing stays stuck in 'delivering' forever.
    locked_by       UUID,
    locked_until    TIMESTAMPTZ,
    delivered_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The claim query: rows that are due and not currently leased.
CREATE INDEX IF NOT EXISTS idx_mail_outbound_claimable
    ON mail.outbound_recipients(next_attempt_at)
    WHERE status IN ('queued', 'deferred');

CREATE INDEX IF NOT EXISTS idx_mail_outbound_message
    ON mail.outbound_recipients(message_id);

-- ── DKIM signing keys ─────────────────────────────────────────────────────────
-- One key per signing domain. The private key is encrypted at rest with the
-- module's MailCrypto (AES-GCM), like the IMAP/SMTP account passwords — a
-- signing key is a credential and must never sit in the clear. The public key
-- is kept so the admin console can show the exact DNS TXT record to publish.
CREATE TABLE IF NOT EXISTS mail.dkim_keys (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    domain              TEXT NOT NULL UNIQUE,
    selector            TEXT NOT NULL,
    -- 'rsa-sha256' (widely required) or 'ed25519-sha256'.
    algorithm           TEXT NOT NULL DEFAULT 'rsa-sha256'
                            CHECK (algorithm IN ('rsa-sha256', 'ed25519-sha256')),
    private_key_enc     BYTEA NOT NULL,
    private_key_nonce   BYTEA NOT NULL,
    -- Base64 public key for the DNS TXT (v=DKIM1; k=...; p=<public_key>).
    public_key          TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
