-- Threads (conversation grouping)
CREATE TABLE IF NOT EXISTS mail.threads (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    account_id      UUID NOT NULL REFERENCES mail.accounts(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL,
    subject         TEXT NOT NULL DEFAULT '',
    message_count   INTEGER NOT NULL DEFAULT 0,
    unread_count    INTEGER NOT NULL DEFAULT 0,
    has_attachments BOOLEAN NOT NULL DEFAULT FALSE,
    is_starred      BOOLEAN NOT NULL DEFAULT FALSE,
    snippet         TEXT,                           -- extrait du dernier message
    last_message_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_mail_threads_account ON mail.threads(account_id, last_message_at DESC);
CREATE INDEX IF NOT EXISTS idx_mail_threads_user    ON mail.threads(user_id);
CREATE INDEX IF NOT EXISTS idx_mail_threads_starred ON mail.threads(account_id) WHERE is_starred = TRUE;

-- Messages individuels
CREATE TABLE IF NOT EXISTS mail.messages (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    thread_id       UUID NOT NULL REFERENCES mail.threads(id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES mail.accounts(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL,
    -- Enveloppe
    message_id      VARCHAR(500),                   -- Message-ID RFC 5322
    in_reply_to     VARCHAR(500),
    imap_uid        BIGINT,                         -- UID IMAP dans le dossier
    imap_folder     VARCHAR(500) NOT NULL DEFAULT 'INBOX',
    -- Adresses
    from_name       VARCHAR(500),
    from_email      VARCHAR(500) NOT NULL,
    to_addresses    JSONB NOT NULL DEFAULT '[]',    -- [{name, email}]
    cc_addresses    JSONB NOT NULL DEFAULT '[]',
    bcc_addresses   JSONB NOT NULL DEFAULT '[]',
    reply_to        VARCHAR(500),
    -- Contenu
    subject         TEXT NOT NULL DEFAULT '',
    body_text       TEXT,                           -- texte brut
    body_html       TEXT,                           -- HTML assaini (ammonia)
    -- Pièces jointes (métadonnées, contenu dans kubuno-storage)
    attachments     JSONB NOT NULL DEFAULT '[]',    -- [{name, mime, size, storage_path}]
    -- État
    is_read         BOOLEAN NOT NULL DEFAULT FALSE,
    is_starred      BOOLEAN NOT NULL DEFAULT FALSE,
    is_deleted      BOOLEAN NOT NULL DEFAULT FALSE,
    -- Dossier courant
    folder          VARCHAR(50) NOT NULL DEFAULT 'inbox'
                        CHECK (folder IN ('inbox', 'sent', 'drafts', 'spam', 'trash', 'custom')),
    label_ids       UUID[] NOT NULL DEFAULT '{}',
    -- Timestamps
    sent_at         TIMESTAMPTZ,
    received_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Contrainte unicité IMAP
    UNIQUE (account_id, imap_folder, imap_uid)
);

CREATE INDEX IF NOT EXISTS idx_mail_messages_thread  ON mail.messages(thread_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_mail_messages_account ON mail.messages(account_id, folder, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_mail_messages_user    ON mail.messages(user_id);
CREATE INDEX IF NOT EXISTS idx_mail_messages_unread  ON mail.messages(account_id, folder) WHERE is_read = FALSE AND is_deleted = FALSE;
CREATE INDEX IF NOT EXISTS idx_mail_messages_starred ON mail.messages(account_id) WHERE is_starred = TRUE;

-- Brouillons
CREATE TABLE IF NOT EXISTS mail.drafts (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    account_id      UUID NOT NULL REFERENCES mail.accounts(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL,
    -- Contenu partiel
    to_addresses    JSONB NOT NULL DEFAULT '[]',
    cc_addresses    JSONB NOT NULL DEFAULT '[]',
    bcc_addresses   JSONB NOT NULL DEFAULT '[]',
    subject         TEXT NOT NULL DEFAULT '',
    body_html       TEXT NOT NULL DEFAULT '',
    -- Réponse à
    reply_to_id     UUID REFERENCES mail.messages(id) ON DELETE SET NULL,
    -- Pièces jointes temporaires (storage paths)
    attachments     JSONB NOT NULL DEFAULT '[]',
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_mail_drafts_account ON mail.drafts(account_id);
CREATE INDEX IF NOT EXISTS idx_mail_drafts_user    ON mail.drafts(user_id);

DROP TRIGGER IF EXISTS drafts_updated_at ON mail.drafts;
CREATE TRIGGER drafts_updated_at
    BEFORE UPDATE ON mail.drafts
    FOR EACH ROW EXECUTE FUNCTION mail.set_updated_at();

-- Association threads ↔ labels
CREATE TABLE IF NOT EXISTS mail.thread_labels (
    thread_id   UUID NOT NULL REFERENCES mail.threads(id) ON DELETE CASCADE,
    label_id    UUID NOT NULL REFERENCES mail.labels(id) ON DELETE CASCADE,
    PRIMARY KEY (thread_id, label_id)
);
