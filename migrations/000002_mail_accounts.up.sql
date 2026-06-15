-- Comptes mail (IMAP + SMTP) par utilisateur
CREATE TABLE IF NOT EXISTS mail.accounts (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id         UUID NOT NULL,
    -- Identité
    name            VARCHAR(255) NOT NULL,           -- nom affiché (ex: "Martin Pro")
    email_address   VARCHAR(500) NOT NULL,
    -- IMAP
    imap_host       VARCHAR(255) NOT NULL,
    imap_port       INTEGER NOT NULL DEFAULT 993,
    imap_security   VARCHAR(10) NOT NULL DEFAULT 'ssl'
                        CHECK (imap_security IN ('ssl', 'starttls', 'none')),
    imap_username   VARCHAR(500) NOT NULL,
    imap_password   BYTEA NOT NULL,                 -- AES-256-GCM chiffré
    imap_password_nonce BYTEA NOT NULL,
    -- SMTP
    smtp_host       VARCHAR(255) NOT NULL,
    smtp_port       INTEGER NOT NULL DEFAULT 587,
    smtp_security   VARCHAR(10) NOT NULL DEFAULT 'starttls'
                        CHECK (smtp_security IN ('ssl', 'starttls', 'none')),
    smtp_username   VARCHAR(500) NOT NULL,
    smtp_password   BYTEA NOT NULL,
    smtp_password_nonce BYTEA NOT NULL,
    -- État
    is_default      BOOLEAN NOT NULL DEFAULT FALSE,
    is_active       BOOLEAN NOT NULL DEFAULT TRUE,
    last_sync_at    TIMESTAMPTZ,
    last_error      TEXT,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_mail_accounts_user   ON mail.accounts(user_id);
CREATE INDEX IF NOT EXISTS idx_mail_accounts_active ON mail.accounts(user_id, is_active) WHERE is_active = TRUE;

-- Trigger updated_at
CREATE OR REPLACE FUNCTION mail.set_updated_at()
RETURNS TRIGGER AS $$
BEGIN NEW.updated_at = NOW(); RETURN NEW; END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS accounts_updated_at ON mail.accounts;
CREATE TRIGGER accounts_updated_at
    BEFORE UPDATE ON mail.accounts
    FOR EACH ROW EXECUTE FUNCTION mail.set_updated_at();

-- Labels/dossiers personnalisés
CREATE TABLE IF NOT EXISTS mail.labels (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    account_id  UUID NOT NULL REFERENCES mail.accounts(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL,
    name        VARCHAR(255) NOT NULL,
    color       VARCHAR(7),                         -- hex color (#rrggbb)
    imap_folder VARCHAR(500),                       -- dossier IMAP correspondant
    is_system   BOOLEAN NOT NULL DEFAULT FALSE,     -- Inbox, Sent, Drafts, Spam, Trash
    position    INTEGER NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (account_id, name)
);

CREATE INDEX IF NOT EXISTS idx_mail_labels_account ON mail.labels(account_id);
CREATE INDEX IF NOT EXISTS idx_mail_labels_user    ON mail.labels(user_id);
