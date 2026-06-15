-- Expéditeurs bloqués : leurs messages entrants vont directement au spam.
CREATE TABLE IF NOT EXISTS mail.blocked_senders (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL,
    email      TEXT NOT NULL,   -- stocké en minuscules
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, email)
);
CREATE INDEX IF NOT EXISTS idx_mail_blocked_user ON mail.blocked_senders(user_id);
