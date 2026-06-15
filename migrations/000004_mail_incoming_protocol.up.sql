ALTER TABLE mail.accounts
    ADD COLUMN IF NOT EXISTS incoming_protocol VARCHAR(4) NOT NULL DEFAULT 'imap'
        CHECK (incoming_protocol IN ('imap', 'pop3'));
