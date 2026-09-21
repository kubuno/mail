-- ── Automatic forwarding rules (re-send incoming mail elsewhere) ────────────
--
-- Why this exists
--   The client already offered a "Forwarding" editor (a list of destination
--   addresses, each enable/disable, plus "keep or archive the local copy"), but
--   it lived entirely in localStorage: the config never reached the server, so
--   no message was ever re-sent. This table is the server-side half — the rules
--   the delivery path reads on every incoming message to decide what to forward.
--
-- One row per destination address
--   A user may forward to several addresses (a colleague, a personal account),
--   so a rule is (user, forward_to): one row per address, each independently
--   enabled/disabled. `keep_copy` is stored per-row because it travels with the
--   rule on the wire, but the UI keeps it uniform across a user's addresses.
--
-- Anti-loop is NOT optional (RFC 3834, spirit of .forward)
--   Forwarding that answers the wrong thing forwards it forever: two mailboxes
--   forwarding to each other, or a bounce re-sent as a fresh message. The
--   delivery-side guard (see `services::forwarding`) refuses to forward anything
--   that already carries our `X-Kubuno-Forwarded` marker (the loop signal), any
--   auto-submitted message, a bounce (`<>` / MAILER-DAEMON), or a destination
--   that is the mailbox itself. The forwarded copy is stamped with the marker so
--   the far side (or a re-entry into local delivery) stops the loop at depth one.

CREATE TABLE IF NOT EXISTS mail.forwarding_rules (
    user_id    UUID         NOT NULL,
    -- The destination address a copy is re-sent to. Lower-cased on write.
    forward_to VARCHAR(320) NOT NULL,
    enabled    BOOLEAN      NOT NULL DEFAULT TRUE,
    -- FALSE archives the local Kubuno copy (out of the inbox) once the message
    -- has actually been forwarded; TRUE leaves it in the inbox.
    keep_copy  BOOLEAN      NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    -- One rule per (user, destination): re-saving the same address updates it
    -- rather than duplicating it.
    PRIMARY KEY (user_id, forward_to)
);

-- The delivery path reads every rule of the recipient on each incoming message.
CREATE INDEX IF NOT EXISTS idx_mail_forwarding_rules_user
    ON mail.forwarding_rules (user_id);
