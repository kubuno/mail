-- ── Outbound relay (smarthost): send through a relay instead of direct-to-MX ──
--
-- Why this exists
--   The outbound worker delivers by resolving each recipient's MX and connecting
--   to it on port 25. That is the right default for a server with a public IP —
--   but this instance runs behind a residential line where port 25 is blocked
--   outbound, and its only route to the internet is a VPS reachable over a
--   private tunnel. Direct-to-MX cannot work from here.
--
--   A relay fixes it: the worker hands every remote message to ONE authenticated
--   (or trusted-network) SMTP host — the VPS's own Postfix — which then reaches
--   the world from a public IP with a good reputation. This is Postfix's
--   `relayhost`, and Dovecot/mail clients' "outgoing server".
--
-- Why a table, not a `[[settings]]` block
--   A relay may need a password, and settings live in `core.settings` as plain
--   JSONB an administrator can read back. A transport password does not belong
--   there. So the relay config is a module-owned singleton row, its password
--   encrypted at rest with the module's `MailCrypto` (the same treatment account
--   passwords get), and the admin API never returns the cleartext.
--
--   `username`/`password` are OPTIONAL: relaying over the private tunnel to a
--   Postfix that trusts that network needs no credentials at all — host and port
--   are enough. They exist for an authenticated public relay (a transactional
--   provider, an ISP submission service).

CREATE TABLE IF NOT EXISTS mail.outbound_relay (
    -- Singleton: one relay for the instance. The fixed id makes the upsert a
    -- plain ON CONFLICT and forbids a second, ambiguous row.
    id              BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id = TRUE),

    enabled         BOOLEAN NOT NULL DEFAULT FALSE,
    host            VARCHAR(255) NOT NULL DEFAULT '',
    port            INTEGER NOT NULL DEFAULT 25 CHECK (port BETWEEN 1 AND 65535),
    -- How TLS is used TO the relay. 'none' is legitimate ONLY over a trusted
    -- private link (the tunnel to the VPS); a public relay must be 'starttls' or
    -- 'tls', or the password below would cross the internet in the clear.
    security        VARCHAR(10) NOT NULL DEFAULT 'none'
                        CHECK (security IN ('none', 'starttls', 'tls')),
    username        VARCHAR(255) NOT NULL DEFAULT '',
    -- AES-256-GCM, written only when a password is set. Never returned by the API.
    password_enc    BYTEA,
    password_nonce  BYTEA,

    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The row exists from the start, disabled: the worker reads it every cycle and
-- an absent row would be one more "is it null" branch on a hot path.
INSERT INTO mail.outbound_relay (id, enabled) VALUES (TRUE, FALSE)
    ON CONFLICT (id) DO NOTHING;
