-- Credentials for the SMTP / IMAP / POP3 services this module offers.
--
-- Deliberately NOT the Kubuno account password: a mail client stores what it is
-- given, in clear or nearly so, and hands it to whatever host it is pointed at.
-- A dedicated per-user secret can be revoked on its own and never unlocks the
-- rest of the platform. Same reasoning as the app passwords providers hand out.
CREATE TABLE IF NOT EXISTS mail.mailbox_credentials (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id       UUID NOT NULL,
    -- What the client sends as its login: the address of one of the user's
    -- accounts, lowercased.
    username      VARCHAR(320) NOT NULL,
    -- Argon2id. The plaintext is shown once, at creation, and never stored.
    password_hash TEXT NOT NULL,
    label         VARCHAR(120),
    last_used_at  TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (username)
);

CREATE INDEX IF NOT EXISTS idx_mail_mailbox_credentials_user
    ON mail.mailbox_credentials(user_id);

-- Sessions of the served protocols, for the admin to see who connects and to
-- make a failing client diagnosable. Kept small on purpose: no message content,
-- no credential, just who/when/what.
CREATE TABLE IF NOT EXISTS mail.server_sessions (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    protocol    VARCHAR(10) NOT NULL CHECK (protocol IN ('smtp', 'imap', 'pop3')),
    user_id     UUID,
    username    VARCHAR(320),
    peer        VARCHAR(100) NOT NULL,
    authed      BOOLEAN NOT NULL DEFAULT FALSE,
    commands    INTEGER NOT NULL DEFAULT 0,
    error       TEXT,
    started_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_mail_server_sessions_started
    ON mail.server_sessions(started_at DESC);

-- IMAP and POP3 need a stable, ascending identifier per message. `imap_uid` is
-- the UID the REMOTE server gave us: it only means something inside that
-- provider's folder, and two accounts collide on it. This one is ours.
ALTER TABLE mail.messages
    ADD COLUMN IF NOT EXISTS local_uid BIGINT;

CREATE SEQUENCE IF NOT EXISTS mail.message_local_uid_seq OWNED BY mail.messages.local_uid;

ALTER TABLE mail.messages
    ALTER COLUMN local_uid SET DEFAULT nextval('mail.message_local_uid_seq');

-- Existing rows get one in reception order, so the sequence stays meaningful.
UPDATE mail.messages m
   SET local_uid = nextval('mail.message_local_uid_seq')
  FROM (SELECT id FROM mail.messages WHERE local_uid IS NULL ORDER BY received_at, id) ordered
 WHERE m.id = ordered.id;

CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_messages_local_uid
    ON mail.messages(local_uid);
