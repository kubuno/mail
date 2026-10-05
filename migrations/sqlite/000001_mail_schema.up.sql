-- SQLite — `mail` is an ATTACHed database file, attached on every pooled
-- connection by kubuno-db, so the qualified names below resolve as they do on
-- the other two engines. This single file declares the FINAL shape the
-- PostgreSQL side reached across its 000001..000043 migrations, stated once
-- (a SQLite install starts empty: there is no history to replay).
--
-- Differences from PostgreSQL, and why:
--   * UUID -> BLOB, TIMESTAMPTZ / DATE -> TEXT, JSONB -> TEXT (JSON), BYTEA ->
--     BLOB, BOOLEAN -> INTEGER (0/1), REAL stays REAL — as sqlx encodes/decodes
--     on SQLite.
--   * Timestamp defaults are written in the exact form sqlx binds a
--     `DateTime<Utc>` (`YYYY-MM-DDTHH:MM:SS.fff+00:00`), so a defaulted value and
--     a bound one compare and sort correctly as text.
--   * No DEFAULT on ids: the process generates every key (kubuno_db::new_id).
--   * Array columns (messages.label_ids, aliases.destinations,
--     mailing_lists.allowed_senders) are JSON arrays, as on PostgreSQL since 000043.
--   * Partial and expression indexes ARE kept (SQLite supports both); lower()
--     folds ASCII only.
--   * Foreign-key REFERENCES and trigger bodies are unqualified (SQLite resolves
--     them inside the trigger's own database).
--   * updated_at, the modseq/local_uid counters, the QRESYNC tombstones and the
--     address-uniqueness check are hand-written triggers. SQLite cannot assign
--     NEW.* in a BEFORE trigger, so the counters are stamped by AFTER triggers
--     that update the row in place (recursive_triggers is off, so that update
--     does not fire them again); read `local_uid`/`modseq` back with a SELECT,
--     not RETURNING, which does not see AFTER-trigger changes.
--   * No NOTIFY trigger (no LISTEN: IMAP IDLE polls) and no purge_greylist()
--     function (the purge is a plain DELETE issued by the process).

-- ── Change counters (replace message_modseq_seq / message_local_uid_seq) ────
CREATE TABLE mail.change_counter (
    domain TEXT    NOT NULL PRIMARY KEY,
    n      INTEGER NOT NULL
);
INSERT INTO mail.change_counter (domain, n) VALUES ('modseq', 0), ('local_uid', 0);

-- ── Local addresses (000024) ─────────────────────────────────────────────────
CREATE TABLE mail.mailboxes (
    id            BLOB    NOT NULL PRIMARY KEY,
    address       TEXT    NOT NULL UNIQUE,
    domain        TEXT    NOT NULL,
    user_id       BLOB    NOT NULL,
    display_name  TEXT,
    quota_bytes   INTEGER NOT NULL DEFAULT 0 CHECK (quota_bytes >= 0),
    is_active     INTEGER NOT NULL DEFAULT 1,
    comment       TEXT,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE INDEX mail.idx_mail_mailboxes_user   ON mailboxes(user_id);
CREATE INDEX mail.idx_mail_mailboxes_domain ON mailboxes(domain);

CREATE TABLE mail.aliases (
    id            BLOB    NOT NULL PRIMARY KEY,
    address       TEXT    NOT NULL UNIQUE,
    domain        TEXT    NOT NULL,
    destinations  TEXT    NOT NULL
        CONSTRAINT aliases_destinations_check
        CHECK (json_valid(destinations) AND json_type(destinations) = 'array'
               AND json_array_length(destinations) > 0),
    is_catch_all  INTEGER NOT NULL DEFAULT 0,
    is_active     INTEGER NOT NULL DEFAULT 1,
    comment       TEXT,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE INDEX mail.idx_mail_aliases_domain ON aliases(domain);
CREATE UNIQUE INDEX mail.idx_mail_aliases_catch_all ON aliases(domain) WHERE is_catch_all;

CREATE TABLE mail.mailing_lists (
    id              BLOB    NOT NULL PRIMARY KEY,
    address         TEXT    NOT NULL UNIQUE,
    domain          TEXT    NOT NULL,
    name            TEXT    NOT NULL,
    post_policy     TEXT    NOT NULL DEFAULT 'internal'
                        CHECK (post_policy IN ('anyone', 'members', 'internal', 'allowed')),
    allowed_senders TEXT    NOT NULL DEFAULT '[]',
    is_active       INTEGER NOT NULL DEFAULT 1,
    comment         TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE INDEX mail.idx_mail_mailing_lists_domain ON mailing_lists(domain);

CREATE TABLE mail.mailing_list_members (
    list_id    BLOB NOT NULL REFERENCES mailing_lists(id) ON DELETE CASCADE,
    address    TEXT NOT NULL,
    added_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    PRIMARY KEY (list_id, address)
);

CREATE TABLE mail.domain_policies (
    domain              TEXT    NOT NULL PRIMARY KEY,
    default_quota_bytes INTEGER NOT NULL DEFAULT 0 CHECK (default_quota_bytes >= 0),
    max_mailboxes       INTEGER NOT NULL DEFAULT 0 CHECK (max_mailboxes >= 0),
    comment             TEXT,
    created_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);

-- An address may be a mailbox, an alias OR a list — never two at once.
CREATE TRIGGER mail.mailboxes_address_free_ins BEFORE INSERT ON mailboxes
WHEN EXISTS (SELECT 1 FROM aliases WHERE address = NEW.address)
  OR EXISTS (SELECT 1 FROM mailing_lists WHERE address = NEW.address)
BEGIN SELECT RAISE(ABORT, 'address already in use by another address object'); END;

CREATE TRIGGER mail.mailboxes_address_free_upd BEFORE UPDATE OF address ON mailboxes
WHEN EXISTS (SELECT 1 FROM aliases WHERE address = NEW.address)
  OR EXISTS (SELECT 1 FROM mailing_lists WHERE address = NEW.address)
BEGIN SELECT RAISE(ABORT, 'address already in use by another address object'); END;

CREATE TRIGGER mail.aliases_address_free_ins BEFORE INSERT ON aliases
WHEN EXISTS (SELECT 1 FROM mailboxes WHERE address = NEW.address)
  OR EXISTS (SELECT 1 FROM mailing_lists WHERE address = NEW.address)
BEGIN SELECT RAISE(ABORT, 'address already in use by another address object'); END;

CREATE TRIGGER mail.aliases_address_free_upd BEFORE UPDATE OF address ON aliases
WHEN EXISTS (SELECT 1 FROM mailboxes WHERE address = NEW.address)
  OR EXISTS (SELECT 1 FROM mailing_lists WHERE address = NEW.address)
BEGIN SELECT RAISE(ABORT, 'address already in use by another address object'); END;

CREATE TRIGGER mail.mailing_lists_address_free_ins BEFORE INSERT ON mailing_lists
WHEN EXISTS (SELECT 1 FROM mailboxes WHERE address = NEW.address)
  OR EXISTS (SELECT 1 FROM aliases WHERE address = NEW.address)
BEGIN SELECT RAISE(ABORT, 'address already in use by another address object'); END;

CREATE TRIGGER mail.mailing_lists_address_free_upd BEFORE UPDATE OF address ON mailing_lists
WHEN EXISTS (SELECT 1 FROM mailboxes WHERE address = NEW.address)
  OR EXISTS (SELECT 1 FROM aliases WHERE address = NEW.address)
BEGIN SELECT RAISE(ABORT, 'address already in use by another address object'); END;

-- ── Accounts (000002, 000004, 000015, 000025) ────────────────────────────────
CREATE TABLE mail.accounts (
    id                  BLOB    NOT NULL PRIMARY KEY,
    user_id             BLOB    NOT NULL,
    name                TEXT    NOT NULL,
    email_address       TEXT    NOT NULL,
    imap_host           TEXT    NOT NULL,
    imap_port           INTEGER NOT NULL DEFAULT 993,
    imap_security       TEXT    NOT NULL DEFAULT 'ssl'
                            CHECK (imap_security IN ('ssl', 'starttls', 'none')),
    imap_username       TEXT    NOT NULL,
    imap_password       BLOB    NOT NULL,
    imap_password_nonce BLOB    NOT NULL,
    smtp_host           TEXT    NOT NULL,
    smtp_port           INTEGER NOT NULL DEFAULT 587,
    smtp_security       TEXT    NOT NULL DEFAULT 'starttls'
                            CHECK (smtp_security IN ('ssl', 'starttls', 'none')),
    smtp_username       TEXT    NOT NULL,
    smtp_password       BLOB    NOT NULL,
    smtp_password_nonce BLOB    NOT NULL,
    is_default          INTEGER NOT NULL DEFAULT 0,
    is_active           INTEGER NOT NULL DEFAULT 1,
    last_sync_at        TEXT,
    last_error          TEXT,
    created_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    incoming_protocol   TEXT    NOT NULL DEFAULT 'imap'
                            CHECK (incoming_protocol IN ('imap', 'pop3')),
    auth_kind           TEXT    NOT NULL DEFAULT 'password'
                            CHECK (auth_kind IN ('password', 'oauth_google', 'oauth_microsoft')),
    oauth_refresh_token BLOB,
    oauth_refresh_nonce BLOB,
    oauth_access_token  BLOB,
    oauth_access_nonce  BLOB,
    oauth_expires_at    TEXT,
    kind                TEXT    NOT NULL DEFAULT 'external'
                            CHECK (kind IN ('external', 'local')),
    mailbox_id          BLOB    REFERENCES mailboxes(id) ON DELETE SET NULL
);
CREATE INDEX mail.idx_mail_accounts_user   ON accounts(user_id);
CREATE INDEX mail.idx_mail_accounts_active ON accounts(user_id, is_active) WHERE is_active = 1;
CREATE UNIQUE INDEX mail.idx_mail_accounts_mailbox ON accounts(mailbox_id) WHERE mailbox_id IS NOT NULL;
CREATE INDEX mail.idx_mail_accounts_kind   ON accounts(user_id, kind);

CREATE TABLE mail.labels (
    id                      BLOB    NOT NULL PRIMARY KEY,
    account_id              BLOB    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    user_id                 BLOB    NOT NULL,
    name                    TEXT    NOT NULL,
    color                   TEXT,
    imap_folder             TEXT,
    is_system               INTEGER NOT NULL DEFAULT 0,
    position                INTEGER NOT NULL DEFAULT 0,
    created_at              TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    list_visibility         TEXT    NOT NULL DEFAULT 'show'
        CONSTRAINT mail_labels_list_visibility_chk CHECK (list_visibility IN ('show', 'unread', 'hide')),
    message_list_visibility TEXT    NOT NULL DEFAULT 'show'
        CONSTRAINT mail_labels_msg_visibility_chk CHECK (message_list_visibility IN ('show', 'hide')),
    UNIQUE (account_id, name)
);
CREATE INDEX mail.idx_mail_labels_account ON labels(account_id);
CREATE INDEX mail.idx_mail_labels_user    ON labels(user_id);

-- ── Threads, messages, drafts ────────────────────────────────────────────────
CREATE TABLE mail.threads (
    id                BLOB    NOT NULL PRIMARY KEY,
    account_id        BLOB    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    user_id           BLOB    NOT NULL,
    subject           TEXT    NOT NULL DEFAULT '',
    message_count     INTEGER NOT NULL DEFAULT 0,
    unread_count      INTEGER NOT NULL DEFAULT 0,
    has_attachments   INTEGER NOT NULL DEFAULT 0,
    is_starred        INTEGER NOT NULL DEFAULT 0,
    snippet           TEXT,
    last_message_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    created_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    last_sender_name  TEXT,
    last_sender_email TEXT    NOT NULL DEFAULT '',
    is_important      INTEGER NOT NULL DEFAULT 0,
    snoozed_until     TEXT,
    is_muted          INTEGER NOT NULL DEFAULT 0,
    category          TEXT
        CONSTRAINT mail_threads_category_chk
        CHECK (category IS NULL OR category IN ('main', 'social', 'notifications', 'promotions')),
    category_pinned   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX mail.idx_mail_threads_account   ON threads(account_id, last_message_at DESC);
CREATE INDEX mail.idx_mail_threads_user      ON threads(user_id);
CREATE INDEX mail.idx_mail_threads_starred   ON threads(account_id) WHERE is_starred = 1;
CREATE INDEX mail.idx_mail_threads_important ON threads(user_id) WHERE is_important = 1;
CREATE INDEX mail.idx_mail_threads_snoozed   ON threads(snoozed_until) WHERE snoozed_until IS NOT NULL;
CREATE INDEX mail.idx_mail_threads_muted     ON threads(user_id) WHERE is_muted = 1;
CREATE INDEX mail.idx_mail_threads_category  ON threads(user_id, category, last_message_at DESC);

CREATE TABLE mail.messages (
    id               BLOB    NOT NULL PRIMARY KEY,
    thread_id        BLOB    NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    account_id       BLOB    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    user_id          BLOB    NOT NULL,
    message_id       TEXT,
    in_reply_to      TEXT,
    imap_uid         INTEGER,
    imap_folder      TEXT    NOT NULL DEFAULT 'INBOX',
    from_name        TEXT,
    from_email       TEXT    NOT NULL,
    to_addresses     TEXT    NOT NULL DEFAULT '[]',
    cc_addresses     TEXT    NOT NULL DEFAULT '[]',
    bcc_addresses    TEXT    NOT NULL DEFAULT '[]',
    reply_to         TEXT,
    subject          TEXT    NOT NULL DEFAULT '',
    body_text        TEXT,
    body_html        TEXT,
    attachments      TEXT    NOT NULL DEFAULT '[]',
    is_read          INTEGER NOT NULL DEFAULT 0,
    is_starred       INTEGER NOT NULL DEFAULT 0,
    is_deleted       INTEGER NOT NULL DEFAULT 0,
    folder           TEXT    NOT NULL DEFAULT 'inbox'
        CONSTRAINT messages_folder_check
        CHECK (folder IN ('inbox', 'sent', 'drafts', 'spam', 'trash', 'custom', 'archive')),
    label_ids        TEXT    NOT NULL DEFAULT '[]',
    sent_at          TEXT,
    received_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    list_unsubscribe TEXT,
    spam_trained     INTEGER,
    spam_score       REAL,
    mailed_by        TEXT,
    signed_by        TEXT,
    security         TEXT,
    category         TEXT
        CONSTRAINT mail_messages_category_chk
        CHECK (category IS NULL OR category IN ('main', 'social', 'notifications', 'promotions')),
    -- Assigned by the AFTER INSERT trigger from change_counter when NULL.
    local_uid        INTEGER,
    -- Stamped by the AFTER INSERT/UPDATE triggers from change_counter.
    modseq           INTEGER NOT NULL DEFAULT 0,
    auth_dmarc       TEXT,
    pgp_raw          BLOB,
    structured_data  TEXT,
    invite_response  TEXT,
    UNIQUE (account_id, imap_folder, imap_uid)
);
CREATE INDEX mail.idx_mail_messages_thread      ON messages(thread_id, received_at DESC);
CREATE INDEX mail.idx_mail_messages_account     ON messages(account_id, folder, received_at DESC);
CREATE INDEX mail.idx_mail_messages_user        ON messages(user_id);
CREATE INDEX mail.idx_mail_messages_unread      ON messages(account_id, folder) WHERE is_read = 0 AND is_deleted = 0;
CREATE INDEX mail.idx_mail_messages_starred     ON messages(account_id) WHERE is_starred = 1;
CREATE INDEX mail.idx_mail_messages_imap_folder ON messages(account_id, imap_folder, received_at DESC);
CREATE UNIQUE INDEX mail.idx_mail_messages_local_uid ON messages(local_uid);
CREATE INDEX mail.idx_mail_messages_modseq      ON messages(user_id, folder, modseq);

CREATE TABLE mail.drafts (
    id            BLOB NOT NULL PRIMARY KEY,
    account_id    BLOB NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    user_id       BLOB NOT NULL,
    to_addresses  TEXT NOT NULL DEFAULT '[]',
    cc_addresses  TEXT NOT NULL DEFAULT '[]',
    bcc_addresses TEXT NOT NULL DEFAULT '[]',
    subject       TEXT NOT NULL DEFAULT '',
    body_html     TEXT NOT NULL DEFAULT '',
    reply_to_id   BLOB REFERENCES messages(id) ON DELETE SET NULL,
    attachments   TEXT NOT NULL DEFAULT '[]',
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    scheduled_at  TEXT
);
CREATE INDEX mail.idx_mail_drafts_account   ON drafts(account_id);
CREATE INDEX mail.idx_mail_drafts_user      ON drafts(user_id);
CREATE INDEX mail.idx_mail_drafts_scheduled ON drafts(scheduled_at) WHERE scheduled_at IS NOT NULL;

CREATE TABLE mail.thread_labels (
    thread_id BLOB NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    label_id  BLOB NOT NULL REFERENCES labels(id)  ON DELETE CASCADE,
    PRIMARY KEY (thread_id, label_id)
);

-- ── CONDSTORE modseq, local_uid, QRESYNC tombstones (000018, 000021) ────────
CREATE TABLE mail.message_tombstones (
    user_id   BLOB    NOT NULL,
    folder    TEXT    NOT NULL,
    local_uid INTEGER NOT NULL,
    modseq    INTEGER NOT NULL,
    PRIMARY KEY (user_id, folder, local_uid)
);
CREATE INDEX mail.idx_mail_tombstones_modseq ON message_tombstones(user_id, folder, modseq);

-- nextval() of the two PostgreSQL sequences, stamped right after the insert.
CREATE TRIGGER mail.mail_messages_bump_modseq_ins AFTER INSERT ON messages
BEGIN
    UPDATE change_counter SET n = n + 1 WHERE domain = 'modseq';
    UPDATE change_counter SET n = n + 1 WHERE domain = 'local_uid' AND NEW.local_uid IS NULL;
    UPDATE messages
       SET modseq    = (SELECT n FROM change_counter WHERE domain = 'modseq'),
           local_uid = COALESCE(NEW.local_uid,
                                (SELECT n FROM change_counter WHERE domain = 'local_uid'))
     WHERE id = NEW.id;
END;

-- Every update takes a fresh modseq; a message leaving a folder (moved, or
-- soft-deleted) leaves a tombstone stamped with it, and arriving in a folder
-- clears any tombstone there.
CREATE TRIGGER mail.mail_messages_bump_modseq_upd AFTER UPDATE ON messages
BEGIN
    UPDATE change_counter SET n = n + 1 WHERE domain = 'modseq';
    UPDATE messages
       SET modseq = (SELECT n FROM change_counter WHERE domain = 'modseq')
     WHERE id = NEW.id;
    INSERT INTO message_tombstones (user_id, folder, local_uid, modseq)
        SELECT OLD.user_id, OLD.folder, OLD.local_uid,
               (SELECT n FROM change_counter WHERE domain = 'modseq')
         WHERE OLD.local_uid IS NOT NULL
           AND (NEW.folder IS NOT OLD.folder OR (NEW.is_deleted AND NOT OLD.is_deleted))
        ON CONFLICT (user_id, folder, local_uid) DO UPDATE SET modseq = excluded.modseq;
    DELETE FROM message_tombstones
     WHERE user_id = NEW.user_id AND folder = NEW.folder AND local_uid = NEW.local_uid;
END;

-- ── Filters, blocked senders, spam model, address index (000009..000012) ─────
CREATE TABLE mail.filters (
    id               BLOB    NOT NULL PRIMARY KEY,
    user_id          BLOB    NOT NULL,
    account_id       BLOB    REFERENCES accounts(id) ON DELETE CASCADE,
    from_contains    TEXT,
    to_contains      TEXT,
    subject_contains TEXT,
    query_contains   TEXT,
    act_archive      INTEGER NOT NULL DEFAULT 0,
    act_mark_read    INTEGER NOT NULL DEFAULT 0,
    act_star         INTEGER NOT NULL DEFAULT 0,
    act_important    INTEGER NOT NULL DEFAULT 0,
    act_trash        INTEGER NOT NULL DEFAULT 0,
    act_spam         INTEGER NOT NULL DEFAULT 0,
    act_label_id     BLOB    REFERENCES labels(id) ON DELETE SET NULL,
    position         INTEGER NOT NULL DEFAULT 0,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE INDEX mail.idx_mail_filters_user ON filters(user_id);

CREATE TABLE mail.blocked_senders (
    id         BLOB NOT NULL PRIMARY KEY,
    user_id    BLOB NOT NULL,
    email      TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    UNIQUE (user_id, email)
);
CREATE INDEX mail.idx_mail_blocked_user ON blocked_senders(user_id);

CREATE TABLE mail.spam_tokens (
    user_id    BLOB    NOT NULL,
    token      TEXT    NOT NULL,
    spam_count INTEGER NOT NULL DEFAULT 0,
    ham_count  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, token)
);

CREATE TABLE mail.spam_stats (
    user_id       BLOB    NOT NULL PRIMARY KEY,
    spam_messages INTEGER NOT NULL DEFAULT 0,
    ham_messages  INTEGER NOT NULL DEFAULT 0,
    auto_classify INTEGER NOT NULL DEFAULT 1,
    threshold     REAL    NOT NULL DEFAULT 0.95,
    updated_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);

CREATE TABLE mail.address_index (
    user_id      BLOB    NOT NULL,
    email        TEXT    NOT NULL,
    name         TEXT,
    use_count    INTEGER NOT NULL DEFAULT 1,
    last_used_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    PRIMARY KEY (user_id, email)
);
CREATE INDEX mail.idx_mail_addr_prefix ON address_index(user_id, email);

-- ── OAuth (000015, 000016) ───────────────────────────────────────────────────
CREATE TABLE mail.oauth_states (
    state        TEXT NOT NULL PRIMARY KEY,
    user_id      BLOB NOT NULL,
    provider     TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    redirect_uri TEXT
);

-- ── Full sync bookkeeping (000017) ───────────────────────────────────────────
CREATE TABLE mail.folder_sync (
    account_id      BLOB    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    imap_folder     TEXT    NOT NULL,
    folder          TEXT    NOT NULL,
    uid_low         INTEGER,
    uid_high        INTEGER,
    backfill_done   INTEGER NOT NULL DEFAULT 0,
    messages_synced INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    last_sync_at    TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    PRIMARY KEY (account_id, imap_folder)
);
CREATE INDEX mail.idx_mail_folder_sync_account ON folder_sync(account_id);

-- ── Served-protocol credentials and sessions (000018, 000021) ────────────────
CREATE TABLE mail.mailbox_credentials (
    id               BLOB    NOT NULL PRIMARY KEY,
    user_id          BLOB    NOT NULL,
    username         TEXT    NOT NULL UNIQUE,
    password_hash    TEXT    NOT NULL,
    label            TEXT,
    last_used_at     TEXT,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    scram_salt       BLOB,
    scram_iterations INTEGER,
    scram_stored_key BLOB,
    scram_server_key BLOB
);
CREATE INDEX mail.idx_mail_mailbox_credentials_user ON mailbox_credentials(user_id);

CREATE TABLE mail.server_sessions (
    id         BLOB    NOT NULL PRIMARY KEY,
    protocol   TEXT    NOT NULL CHECK (protocol IN ('smtp', 'imap', 'pop3')),
    user_id    BLOB,
    username   TEXT,
    peer       TEXT    NOT NULL,
    authed     INTEGER NOT NULL DEFAULT 0,
    commands   INTEGER NOT NULL DEFAULT 0,
    error      TEXT,
    started_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    ended_at   TEXT
);
CREATE INDEX mail.idx_mail_server_sessions_started ON server_sessions(started_at DESC);

-- ── Outbound queue and DKIM keys (000019, 000023) ────────────────────────────
CREATE TABLE mail.outbound_messages (
    id            BLOB    NOT NULL PRIMARY KEY,
    user_id       BLOB,
    account_id    BLOB,
    envelope_from TEXT    NOT NULL,
    raw           BLOB    NOT NULL,
    is_dsn        INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    expires_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now', '+5 days'))
);

CREATE TABLE mail.outbound_recipients (
    id              BLOB    NOT NULL PRIMARY KEY,
    message_id      BLOB    NOT NULL REFERENCES outbound_messages(id) ON DELETE CASCADE,
    recipient       TEXT    NOT NULL,
    domain          TEXT    NOT NULL,
    status          TEXT    NOT NULL DEFAULT 'queued'
                        CHECK (status IN ('queued', 'delivering', 'sent', 'deferred', 'bounced')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    last_code       INTEGER,
    last_reason     TEXT,
    locked_by       BLOB,
    locked_until    TEXT,
    delivered_at    TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE INDEX mail.idx_mail_outbound_claimable ON outbound_recipients(next_attempt_at)
    WHERE status IN ('queued', 'deferred');
CREATE INDEX mail.idx_mail_outbound_message   ON outbound_recipients(message_id);

CREATE TABLE mail.dkim_keys_all (
    id                BLOB    NOT NULL PRIMARY KEY,
    domain            TEXT    NOT NULL,
    selector          TEXT    NOT NULL,
    algorithm         TEXT    NOT NULL DEFAULT 'rsa-sha256'
                          CHECK (algorithm IN ('rsa-sha256', 'ed25519-sha256')),
    private_key_enc   BLOB    NOT NULL,
    private_key_nonce BLOB    NOT NULL,
    public_key        TEXT    NOT NULL,
    created_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    is_active         INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX mail.uq_mail_dkim_domain_selector   ON dkim_keys_all(domain, selector);
CREATE UNIQUE INDEX mail.uq_mail_dkim_active_per_domain ON dkim_keys_all(domain) WHERE is_active;

-- The active signing key per domain (read by the signer).
CREATE VIEW mail.dkim_keys AS
    SELECT id, domain, selector, algorithm,
           private_key_enc, private_key_nonce, public_key, created_at
      FROM dkim_keys_all
     WHERE is_active;

-- ── Greylisting (000022) ─────────────────────────────────────────────────────
CREATE TABLE mail.greylist (
    client_net TEXT NOT NULL,
    sender     TEXT NOT NULL,
    recipient  TEXT NOT NULL,
    first_seen TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    last_seen  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    passed_at  TEXT,
    PRIMARY KEY (client_net, sender, recipient)
);
CREATE INDEX mail.mail_greylist_last_seen_idx ON greylist(last_seen);

-- ── Outbound relay singleton (000026) ────────────────────────────────────────
CREATE TABLE mail.outbound_relay (
    id             INTEGER NOT NULL DEFAULT 1 PRIMARY KEY CHECK (id = 1),
    enabled        INTEGER NOT NULL DEFAULT 0,
    host           TEXT    NOT NULL DEFAULT '',
    port           INTEGER NOT NULL DEFAULT 25 CHECK (port BETWEEN 1 AND 65535),
    security       TEXT    NOT NULL DEFAULT 'none'
                       CHECK (security IN ('none', 'starttls', 'tls')),
    username       TEXT    NOT NULL DEFAULT '',
    password_enc   BLOB,
    password_nonce BLOB,
    updated_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
INSERT INTO mail.outbound_relay (id, enabled) VALUES (1, 0);

-- ── Sender avatars (000027) ──────────────────────────────────────────────────
CREATE TABLE mail.sender_avatars (
    domain     TEXT    NOT NULL PRIMARY KEY,
    source     TEXT    NOT NULL DEFAULT 'bimi',
    mime       TEXT,
    bytes      BLOB,
    not_found  INTEGER NOT NULL DEFAULT 0,
    fetched_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    expires_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now', '+7 days'))
);
CREATE INDEX mail.idx_mail_sender_avatars_expiry ON sender_avatars(expires_at);

-- ── OpenPGP (000030) ─────────────────────────────────────────────────────────
CREATE TABLE mail.pgp_keys (
    id                BLOB    NOT NULL PRIMARY KEY,
    user_id           BLOB    NOT NULL,
    email             TEXT,
    fingerprint       TEXT    NOT NULL,
    public_key        TEXT    NOT NULL,
    private_key       BLOB    NOT NULL,
    private_key_nonce BLOB    NOT NULL,
    is_default        INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    UNIQUE (user_id, fingerprint)
);
CREATE INDEX mail.idx_mail_pgp_keys_user  ON pgp_keys(user_id);
CREATE INDEX mail.idx_mail_pgp_keys_email ON pgp_keys(user_id, lower(email));

CREATE TABLE mail.pgp_contacts (
    id          BLOB NOT NULL PRIMARY KEY,
    user_id     BLOB NOT NULL,
    email       TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    public_key  TEXT NOT NULL,
    source      TEXT NOT NULL DEFAULT 'manual'
                    CHECK (source IN ('manual', 'wkd', 'autocrypt', 'attached')),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE UNIQUE INDEX mail.idx_mail_pgp_contacts_lookup ON pgp_contacts(user_id, lower(email));

-- ── Vacation responder, forwarding, send-as, idempotency (000032..000035) ────
CREATE TABLE mail.vacation_responders (
    user_id       BLOB    NOT NULL PRIMARY KEY,
    enabled       INTEGER NOT NULL DEFAULT 0,
    start_date    TEXT,
    end_date      TEXT,
    subject       TEXT    NOT NULL DEFAULT '',
    message_html  TEXT    NOT NULL DEFAULT '',
    contacts_only INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);

CREATE TABLE mail.vacation_sent (
    user_id    BLOB NOT NULL,
    from_email TEXT NOT NULL,
    sent_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    PRIMARY KEY (user_id, from_email)
);
CREATE INDEX mail.idx_mail_vacation_sent_sent_at ON vacation_sent(sent_at);

CREATE TABLE mail.forwarding_rules (
    user_id    BLOB    NOT NULL,
    forward_to TEXT    NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1,
    keep_copy  INTEGER NOT NULL DEFAULT 1,
    created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    PRIMARY KEY (user_id, forward_to)
);
CREATE INDEX mail.idx_mail_forwarding_rules_user ON forwarding_rules(user_id);

CREATE TABLE mail.send_as_addresses (
    id                      BLOB    NOT NULL PRIMARY KEY,
    user_id                 BLOB    NOT NULL,
    email                   TEXT    NOT NULL,
    display_name            TEXT    NOT NULL DEFAULT '',
    verified                INTEGER NOT NULL DEFAULT 0,
    verification_code       TEXT,
    verification_expires_at TEXT,
    treat_as_alias          INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at              TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE UNIQUE INDEX mail.uq_mail_send_as_user_email ON send_as_addresses(user_id, lower(email));
CREATE INDEX mail.idx_mail_send_as_user ON send_as_addresses(user_id);

CREATE TABLE mail.send_idempotency (
    user_id       BLOB NOT NULL,
    "key"         TEXT NOT NULL,
    response_json TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    PRIMARY KEY (user_id, "key")
);
CREATE INDEX mail.idx_mail_send_idempotency_created_at ON send_idempotency(created_at);

-- ── Delegation, POP/IMAP policy, templates, groups, images (000036..000042) ──
CREATE TABLE mail.delegations (
    id               BLOB    NOT NULL PRIMARY KEY,
    grantor_user_id  BLOB    NOT NULL,
    grantor_email    TEXT    NOT NULL DEFAULT '',
    delegate_user_id BLOB    NOT NULL,
    delegate_email   TEXT    NOT NULL,
    status           TEXT    NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'accepted', 'revoked')),
    can_send         INTEGER NOT NULL DEFAULT 1,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    accepted_at      TEXT,
    updated_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE UNIQUE INDEX mail.uq_mail_delegations_pair ON delegations(grantor_user_id, delegate_user_id);
CREATE INDEX mail.idx_mail_delegations_grantor  ON delegations(grantor_user_id);
CREATE INDEX mail.idx_mail_delegations_delegate ON delegations(delegate_user_id);

CREATE TABLE mail.pop_imap_settings (
    user_id           BLOB    NOT NULL PRIMARY KEY,
    imap_enabled      INTEGER NOT NULL DEFAULT 1,
    imap_expunge_mode TEXT    NOT NULL DEFAULT 'wait'
                          CHECK (imap_expunge_mode IN ('auto', 'wait')),
    imap_auto_expunge INTEGER NOT NULL DEFAULT 0,
    imap_purge_mode   TEXT    NOT NULL DEFAULT 'trash'
                          CHECK (imap_purge_mode IN ('archive', 'trash', 'delete')),
    imap_folder_limit INTEGER NOT NULL DEFAULT 0 CHECK (imap_folder_limit >= 0),
    pop_enabled       INTEGER NOT NULL DEFAULT 1,
    pop_mode          TEXT    NOT NULL DEFAULT 'all'
                          CHECK (pop_mode IN ('all', 'from_now')),
    pop_from_uid      INTEGER NOT NULL DEFAULT 0,
    pop_post_action   TEXT    NOT NULL DEFAULT 'mark_read'
                          CHECK (pop_post_action IN ('keep', 'mark_read', 'archive', 'delete')),
    created_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);

CREATE TABLE mail.email_templates (
    id         BLOB NOT NULL PRIMARY KEY,
    user_id    BLOB NOT NULL,
    name       TEXT NOT NULL,
    subject    TEXT NOT NULL DEFAULT '',
    body_html  TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE UNIQUE INDEX mail.uq_mail_email_templates_user_name ON email_templates(user_id, lower(name));
CREATE INDEX mail.idx_mail_email_templates_user ON email_templates(user_id);

CREATE TABLE mail.recipient_groups (
    id         BLOB NOT NULL PRIMARY KEY,
    user_id    BLOB NOT NULL,
    name       TEXT NOT NULL,
    members    TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);
CREATE UNIQUE INDEX mail.uq_mail_recipient_groups_user_name ON recipient_groups(user_id, lower(name));
CREATE INDEX mail.idx_mail_recipient_groups_user ON recipient_groups(user_id);

CREATE TABLE mail.image_allowed_senders (
    id         BLOB NOT NULL PRIMARY KEY,
    user_id    BLOB NOT NULL,
    email      TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    UNIQUE (user_id, email)
);
CREATE INDEX mail.idx_mail_image_allowed_user ON image_allowed_senders(user_id);

-- ── updated_at, where PostgreSQL keeps it with a BEFORE UPDATE trigger ───────
-- (accounts, drafts, mailboxes, aliases, mailing_lists, domain_policies,
-- pgp_keys, pgp_contacts, send_as_addresses, delegations, email_templates,
-- recipient_groups). Like PostgreSQL's trigger, every update stamps it.
CREATE TRIGGER mail.accounts_updated_at AFTER UPDATE ON accounts
BEGIN UPDATE accounts SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.drafts_updated_at AFTER UPDATE ON drafts
BEGIN UPDATE drafts SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.mailboxes_touch AFTER UPDATE ON mailboxes
BEGIN UPDATE mailboxes SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.aliases_touch AFTER UPDATE ON aliases
BEGIN UPDATE aliases SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.mailing_lists_touch AFTER UPDATE ON mailing_lists
BEGIN UPDATE mailing_lists SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.domain_policies_touch AFTER UPDATE ON domain_policies
BEGIN UPDATE domain_policies SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE domain = NEW.domain; END;
CREATE TRIGGER mail.pgp_keys_updated_at AFTER UPDATE ON pgp_keys
BEGIN UPDATE pgp_keys SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.pgp_contacts_updated_at AFTER UPDATE ON pgp_contacts
BEGIN UPDATE pgp_contacts SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.send_as_updated_at AFTER UPDATE ON send_as_addresses
BEGIN UPDATE send_as_addresses SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.delegations_updated_at AFTER UPDATE ON delegations
BEGIN UPDATE delegations SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.email_templates_updated_at AFTER UPDATE ON email_templates
BEGIN UPDATE email_templates SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER mail.recipient_groups_updated_at AFTER UPDATE ON recipient_groups
BEGIN UPDATE recipient_groups SET updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') WHERE id = NEW.id; END;
