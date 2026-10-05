-- ── Vacation responder (out-of-office auto-reply) ───────────────────────────
--
-- Why this exists
--   The client already offered an "out-of-office" editor (subject, rich-text
--   body, active window, contacts-only), but it lived entirely in localStorage:
--   the config never reached the server, so no auto-reply was ever sent. These
--   two tables are the server-side half — the persisted config and the
--   rate-limit ledger the delivery path reads on every incoming message.
--
-- Anti-loop is NOT optional (RFC 3834)
--   An auto-responder that answers the wrong thing answers it forever: two
--   responders bouncing "I'm away" at each other, or a reply to a mailing list
--   that fans out to thousands. The delivery-side guard (see
--   `services::vacation`) refuses to answer anything that looks automatic, and
--   `vacation_sent` bounds even a legitimate exchange to ONE reply per sender
--   per interval — Gmail's four-day rule.

-- One responder configuration per user (the current user owns exactly one).
CREATE TABLE IF NOT EXISTS mail.vacation_responders (
    user_id       UUID        PRIMARY KEY,
    enabled       BOOLEAN     NOT NULL DEFAULT FALSE,
    -- First / last active day. `start_date` NULL means "no lower bound";
    -- `end_date` NULL means "until turned off". Stored as DATE: the window is a
    -- calendar decision, evaluated against the server's current date.
    start_date    DATE,
    end_date      DATE,
    subject       TEXT        NOT NULL DEFAULT '',
    message_html  TEXT        NOT NULL DEFAULT '',
    -- Reply only to correspondents the user already knows (see the service note
    -- on what "contacts" resolves to for the mail module).
    contacts_only BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Rate-limit ledger: the last time we auto-replied to a given sender on behalf
-- of a given user. One row per (user, sender), upserted on each reply, so the
-- table stays proportional to the number of distinct correspondents rather than
-- to the mail volume. The delivery path reads `sent_at` to enforce the
-- "at most one reply per sender per N days" rule.
CREATE TABLE IF NOT EXISTS mail.vacation_sent (
    user_id    UUID        NOT NULL,
    from_email TEXT        NOT NULL,
    sent_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, from_email)
);

-- Sweeping stale rows (a sender not written to in a long while) can scan by age.
CREATE INDEX IF NOT EXISTS idx_mail_vacation_sent_sent_at
    ON mail.vacation_sent (sent_at);
