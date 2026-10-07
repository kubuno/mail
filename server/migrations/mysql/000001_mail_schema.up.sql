-- MySQL / MariaDB — the `mail` database is created by kubuno-db's
-- `ensure_schema` before the migrator runs, so there is no CREATE DATABASE here.
-- This single file declares the FINAL shape the PostgreSQL side reached across
-- its 000001..000043 migrations, stated once (a MySQL/MariaDB install starts
-- empty: there is no history to replay).
--
-- Differences from PostgreSQL, and why:
--   * UUID -> BINARY(16), TIMESTAMPTZ -> DATETIME(6) (every value UTC: the pool
--     pins `time_zone = '+00:00'`), JSONB -> JSON, BYTEA -> BLOB/LONGBLOB,
--     REAL -> FLOAT (f32), TEXT that is keyed or indexed -> VARCHAR(n) (MySQL
--     cannot key a TEXT column without a prefix), large bodies -> LONGTEXT
--     (MySQL's TEXT stops at 64 KiB).
--   * No DEFAULT on ids: the process generates every key (kubuno_db::new_id).
--   * Literal defaults on TEXT/JSON columns are written as expressions
--     (`DEFAULT ('')`), the form Oracle MySQL accepts; MariaDB accepts it too.
--   * Array columns (messages.label_ids, aliases.destinations,
--     mailing_lists.allowed_senders) are JSON arrays, as on PostgreSQL since 000043.
--   * Partial indexes do not exist: a WHERE-filtered index becomes a plain index
--     over the filtered columns; a partial UNIQUE index (one active DKIM key per
--     domain, one catch-all per domain) becomes a UNIQUE key over a VIRTUAL
--     generated column that is NULL outside the predicate (NULLs never collide).
--   * Expression unique indexes on lower(x) become a VIRTUAL generated column
--     `x_lc` + a UNIQUE key (MariaDB has no functional key parts).
--   * updated_at triggers -> ON UPDATE CURRENT_TIMESTAMP(6) on the same tables.
--   * The two native SEQUENCEs (modseq, local_uid) become rows of
--     `change_counter`, incremented by the BEFORE triggers on `messages` exactly
--     where PostgreSQL called nextval(); the QRESYNC tombstone trigger and the
--     address-uniqueness trigger are re-stated in MySQL's trigger dialect.
--   * No NOTIFY trigger: MySQL has no LISTEN, IMAP IDLE polls instead.
--   * No purge_greylist() function: the purge is a plain DELETE issued by the
--     process on every engine.

-- ── Change counters (replace message_modseq_seq / message_local_uid_seq) ────
CREATE TABLE mail.change_counter (
    domain VARCHAR(190) NOT NULL PRIMARY KEY,
    n      BIGINT       NOT NULL
);
INSERT INTO mail.change_counter (domain, n) VALUES ('modseq', 0), ('local_uid', 0);

-- ── Local addresses (000024) — created first: accounts reference mailboxes ───
CREATE TABLE mail.mailboxes (
    id            BINARY(16)   NOT NULL PRIMARY KEY,
    address       VARCHAR(320) NOT NULL,
    domain        VARCHAR(255) NOT NULL,
    user_id       BINARY(16)   NOT NULL,
    display_name  VARCHAR(255) NULL,
    quota_bytes   BIGINT       NOT NULL DEFAULT 0 CHECK (quota_bytes >= 0),
    is_active     BOOLEAN      NOT NULL DEFAULT TRUE,
    comment       TEXT         NULL,
    created_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    UNIQUE KEY mailboxes_address_key (address)
);
CREATE INDEX idx_mail_mailboxes_user   ON mail.mailboxes(user_id);
CREATE INDEX idx_mail_mailboxes_domain ON mail.mailboxes(domain);

CREATE TABLE mail.aliases (
    id            BINARY(16)   NOT NULL PRIMARY KEY,
    address       VARCHAR(320) NOT NULL,
    domain        VARCHAR(255) NOT NULL,
    destinations  JSON         NOT NULL,
    is_catch_all  BOOLEAN      NOT NULL DEFAULT FALSE,
    is_active     BOOLEAN      NOT NULL DEFAULT TRUE,
    comment       TEXT         NULL,
    created_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    -- NULL unless this row is a catch-all: the UNIQUE key below is then "at most
    -- one catch-all per domain", PostgreSQL's partial unique index.
    catch_all_domain VARCHAR(255) GENERATED ALWAYS AS (CASE WHEN is_catch_all THEN domain END) VIRTUAL,
    UNIQUE KEY aliases_address_key (address),
    UNIQUE KEY idx_mail_aliases_catch_all (catch_all_domain),
    CONSTRAINT aliases_destinations_check
        CHECK (JSON_TYPE(destinations) = 'ARRAY' AND JSON_LENGTH(destinations) > 0)
);
CREATE INDEX idx_mail_aliases_domain ON mail.aliases(domain);

CREATE TABLE mail.mailing_lists (
    id              BINARY(16)   NOT NULL PRIMARY KEY,
    address         VARCHAR(320) NOT NULL,
    domain          VARCHAR(255) NOT NULL,
    name            VARCHAR(255) NOT NULL,
    post_policy     VARCHAR(20)  NOT NULL DEFAULT 'internal'
                        CHECK (post_policy IN ('anyone', 'members', 'internal', 'allowed')),
    allowed_senders JSON         NOT NULL DEFAULT ('[]'),
    is_active       BOOLEAN      NOT NULL DEFAULT TRUE,
    comment         TEXT         NULL,
    created_at      DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at      DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    UNIQUE KEY mailing_lists_address_key (address)
);
CREATE INDEX idx_mail_mailing_lists_domain ON mail.mailing_lists(domain);

CREATE TABLE mail.mailing_list_members (
    list_id    BINARY(16)   NOT NULL,
    address    VARCHAR(320) NOT NULL,
    added_at   DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (list_id, address),
    FOREIGN KEY (list_id) REFERENCES mail.mailing_lists(id) ON DELETE CASCADE
);

CREATE TABLE mail.domain_policies (
    domain              VARCHAR(255) NOT NULL PRIMARY KEY,
    default_quota_bytes BIGINT       NOT NULL DEFAULT 0 CHECK (default_quota_bytes >= 0),
    max_mailboxes       INT          NOT NULL DEFAULT 0 CHECK (max_mailboxes >= 0),
    comment             TEXT         NULL,
    created_at          DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6)
);

-- An address may be a mailbox, an alias OR a list — never two at once. The error
-- is raised as a duplicate-key error (1062 / SQLSTATE 23000), the class
-- PostgreSQL's `unique_violation` maps to.
CREATE TRIGGER mail.mailboxes_address_free_ins BEFORE INSERT ON mail.mailboxes FOR EACH ROW
BEGIN
    IF EXISTS (SELECT 1 FROM mail.aliases WHERE address = NEW.address)
       OR EXISTS (SELECT 1 FROM mail.mailing_lists WHERE address = NEW.address) THEN
        SIGNAL SQLSTATE '23000' SET MESSAGE_TEXT = 'address already in use by another address object', MYSQL_ERRNO = 1062;
    END IF;
END;

CREATE TRIGGER mail.mailboxes_address_free_upd BEFORE UPDATE ON mail.mailboxes FOR EACH ROW
BEGIN
    IF NEW.address <> OLD.address AND (
           EXISTS (SELECT 1 FROM mail.aliases WHERE address = NEW.address)
        OR EXISTS (SELECT 1 FROM mail.mailing_lists WHERE address = NEW.address)) THEN
        SIGNAL SQLSTATE '23000' SET MESSAGE_TEXT = 'address already in use by another address object', MYSQL_ERRNO = 1062;
    END IF;
END;

CREATE TRIGGER mail.aliases_address_free_ins BEFORE INSERT ON mail.aliases FOR EACH ROW
BEGIN
    IF EXISTS (SELECT 1 FROM mail.mailboxes WHERE address = NEW.address)
       OR EXISTS (SELECT 1 FROM mail.mailing_lists WHERE address = NEW.address) THEN
        SIGNAL SQLSTATE '23000' SET MESSAGE_TEXT = 'address already in use by another address object', MYSQL_ERRNO = 1062;
    END IF;
END;

CREATE TRIGGER mail.aliases_address_free_upd BEFORE UPDATE ON mail.aliases FOR EACH ROW
BEGIN
    IF NEW.address <> OLD.address AND (
           EXISTS (SELECT 1 FROM mail.mailboxes WHERE address = NEW.address)
        OR EXISTS (SELECT 1 FROM mail.mailing_lists WHERE address = NEW.address)) THEN
        SIGNAL SQLSTATE '23000' SET MESSAGE_TEXT = 'address already in use by another address object', MYSQL_ERRNO = 1062;
    END IF;
END;

CREATE TRIGGER mail.mailing_lists_address_free_ins BEFORE INSERT ON mail.mailing_lists FOR EACH ROW
BEGIN
    IF EXISTS (SELECT 1 FROM mail.mailboxes WHERE address = NEW.address)
       OR EXISTS (SELECT 1 FROM mail.aliases WHERE address = NEW.address) THEN
        SIGNAL SQLSTATE '23000' SET MESSAGE_TEXT = 'address already in use by another address object', MYSQL_ERRNO = 1062;
    END IF;
END;

CREATE TRIGGER mail.mailing_lists_address_free_upd BEFORE UPDATE ON mail.mailing_lists FOR EACH ROW
BEGIN
    IF NEW.address <> OLD.address AND (
           EXISTS (SELECT 1 FROM mail.mailboxes WHERE address = NEW.address)
        OR EXISTS (SELECT 1 FROM mail.aliases WHERE address = NEW.address)) THEN
        SIGNAL SQLSTATE '23000' SET MESSAGE_TEXT = 'address already in use by another address object', MYSQL_ERRNO = 1062;
    END IF;
END;

-- ── Accounts (000002, 000004, 000015, 000025) ────────────────────────────────
CREATE TABLE mail.accounts (
    id                  BINARY(16)   NOT NULL PRIMARY KEY,
    user_id             BINARY(16)   NOT NULL,
    name                VARCHAR(255) NOT NULL,
    email_address       VARCHAR(500) NOT NULL,
    imap_host           VARCHAR(255) NOT NULL,
    imap_port           INT          NOT NULL DEFAULT 993,
    imap_security       VARCHAR(10)  NOT NULL DEFAULT 'ssl'
                            CHECK (imap_security IN ('ssl', 'starttls', 'none')),
    imap_username       VARCHAR(500) NOT NULL,
    imap_password       BLOB         NOT NULL,
    imap_password_nonce BLOB         NOT NULL,
    smtp_host           VARCHAR(255) NOT NULL,
    smtp_port           INT          NOT NULL DEFAULT 587,
    smtp_security       VARCHAR(10)  NOT NULL DEFAULT 'starttls'
                            CHECK (smtp_security IN ('ssl', 'starttls', 'none')),
    smtp_username       VARCHAR(500) NOT NULL,
    smtp_password       BLOB         NOT NULL,
    smtp_password_nonce BLOB         NOT NULL,
    is_default          BOOLEAN      NOT NULL DEFAULT FALSE,
    is_active           BOOLEAN      NOT NULL DEFAULT TRUE,
    last_sync_at        DATETIME(6)  NULL,
    last_error          TEXT         NULL,
    created_at          DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    incoming_protocol   VARCHAR(4)   NOT NULL DEFAULT 'imap'
                            CHECK (incoming_protocol IN ('imap', 'pop3')),
    auth_kind           VARCHAR(20)  NOT NULL DEFAULT 'password'
                            CHECK (auth_kind IN ('password', 'oauth_google', 'oauth_microsoft')),
    oauth_refresh_token BLOB         NULL,
    oauth_refresh_nonce BLOB         NULL,
    oauth_access_token  BLOB         NULL,
    oauth_access_nonce  BLOB         NULL,
    oauth_expires_at    DATETIME(6)  NULL,
    kind                VARCHAR(10)  NOT NULL DEFAULT 'external'
                            CHECK (kind IN ('external', 'local')),
    mailbox_id          BINARY(16)   NULL,
    -- One account per mailbox (several NULLs never collide on MySQL).
    UNIQUE KEY idx_mail_accounts_mailbox (mailbox_id),
    FOREIGN KEY (mailbox_id) REFERENCES mail.mailboxes(id) ON DELETE SET NULL
);
CREATE INDEX idx_mail_accounts_user   ON mail.accounts(user_id);
CREATE INDEX idx_mail_accounts_active ON mail.accounts(user_id, is_active);
CREATE INDEX idx_mail_accounts_kind   ON mail.accounts(user_id, kind);

CREATE TABLE mail.labels (
    id                      BINARY(16)   NOT NULL PRIMARY KEY,
    account_id              BINARY(16)   NOT NULL,
    user_id                 BINARY(16)   NOT NULL,
    name                    VARCHAR(255) NOT NULL,
    color                   VARCHAR(7)   NULL,
    imap_folder             VARCHAR(500) NULL,
    is_system               BOOLEAN      NOT NULL DEFAULT FALSE,
    position                INT          NOT NULL DEFAULT 0,
    created_at              DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    list_visibility         VARCHAR(10)  NOT NULL DEFAULT 'show',
    message_list_visibility VARCHAR(10)  NOT NULL DEFAULT 'show',
    UNIQUE KEY labels_account_id_name_key (account_id, name),
    CONSTRAINT mail_labels_list_visibility_chk CHECK (list_visibility IN ('show', 'unread', 'hide')),
    CONSTRAINT mail_labels_msg_visibility_chk  CHECK (message_list_visibility IN ('show', 'hide')),
    FOREIGN KEY (account_id) REFERENCES mail.accounts(id) ON DELETE CASCADE
);
CREATE INDEX idx_mail_labels_account ON mail.labels(account_id);
CREATE INDEX idx_mail_labels_user    ON mail.labels(user_id);

-- ── Threads, messages, drafts (000003, 000005..000008, 000011, 000014, 000017,
--    000018, 000021, 000028, 000031, 000040..000043) ──────────────────────────
CREATE TABLE mail.threads (
    id                BINARY(16)   NOT NULL PRIMARY KEY,
    account_id        BINARY(16)   NOT NULL,
    user_id           BINARY(16)   NOT NULL,
    subject           TEXT         NOT NULL DEFAULT (''),
    message_count     INT          NOT NULL DEFAULT 0,
    unread_count      INT          NOT NULL DEFAULT 0,
    has_attachments   BOOLEAN      NOT NULL DEFAULT FALSE,
    is_starred        BOOLEAN      NOT NULL DEFAULT FALSE,
    snippet           TEXT         NULL,
    last_message_at   DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    created_at        DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    last_sender_name  VARCHAR(500) NULL,
    last_sender_email VARCHAR(500) NOT NULL DEFAULT '',
    is_important      BOOLEAN      NOT NULL DEFAULT FALSE,
    snoozed_until     DATETIME(6)  NULL,
    is_muted          BOOLEAN      NOT NULL DEFAULT FALSE,
    category          VARCHAR(20)  NULL,
    category_pinned   BOOLEAN      NOT NULL DEFAULT FALSE,
    CONSTRAINT mail_threads_category_chk
        CHECK (category IS NULL OR category IN ('main', 'social', 'notifications', 'promotions')),
    FOREIGN KEY (account_id) REFERENCES mail.accounts(id) ON DELETE CASCADE
);
CREATE INDEX idx_mail_threads_account   ON mail.threads(account_id, last_message_at DESC);
CREATE INDEX idx_mail_threads_user      ON mail.threads(user_id);
CREATE INDEX idx_mail_threads_starred   ON mail.threads(account_id, is_starred);
CREATE INDEX idx_mail_threads_important ON mail.threads(user_id, is_important);
CREATE INDEX idx_mail_threads_snoozed   ON mail.threads(snoozed_until);
CREATE INDEX idx_mail_threads_muted     ON mail.threads(user_id, is_muted);
CREATE INDEX idx_mail_threads_category  ON mail.threads(user_id, category, last_message_at DESC);

CREATE TABLE mail.messages (
    id               BINARY(16)   NOT NULL PRIMARY KEY,
    thread_id        BINARY(16)   NOT NULL,
    account_id       BINARY(16)   NOT NULL,
    user_id          BINARY(16)   NOT NULL,
    message_id       VARCHAR(500) NULL,
    in_reply_to      VARCHAR(500) NULL,
    imap_uid         BIGINT       NULL,
    imap_folder      VARCHAR(500) NOT NULL DEFAULT 'INBOX',
    from_name        VARCHAR(500) NULL,
    from_email       VARCHAR(500) NOT NULL,
    to_addresses     JSON         NOT NULL DEFAULT ('[]'),
    cc_addresses     JSON         NOT NULL DEFAULT ('[]'),
    bcc_addresses    JSON         NOT NULL DEFAULT ('[]'),
    reply_to         VARCHAR(500) NULL,
    subject          TEXT         NOT NULL DEFAULT (''),
    body_text        LONGTEXT     NULL,
    body_html        LONGTEXT     NULL,
    attachments      JSON         NOT NULL DEFAULT ('[]'),
    is_read          BOOLEAN      NOT NULL DEFAULT FALSE,
    is_starred       BOOLEAN      NOT NULL DEFAULT FALSE,
    is_deleted       BOOLEAN      NOT NULL DEFAULT FALSE,
    folder           VARCHAR(50)  NOT NULL DEFAULT 'inbox',
    label_ids        JSON         NOT NULL DEFAULT ('[]'),
    sent_at          DATETIME(6)  NULL,
    received_at      DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    created_at       DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    list_unsubscribe TEXT         NULL,
    spam_trained     SMALLINT     NULL,
    spam_score       FLOAT        NULL,
    mailed_by        TEXT         NULL,
    signed_by        TEXT         NULL,
    security         TEXT         NULL,
    category         VARCHAR(20)  NULL,
    -- Assigned by the BEFORE INSERT trigger from change_counter when NULL.
    local_uid        BIGINT       NULL,
    -- Stamped by the BEFORE INSERT/UPDATE triggers from change_counter.
    modseq           BIGINT       NOT NULL DEFAULT 0,
    auth_dmarc       VARCHAR(8)   NULL,
    pgp_raw          LONGBLOB     NULL,
    structured_data  JSON         NULL,
    invite_response  TEXT         NULL,
    UNIQUE KEY messages_account_id_imap_folder_imap_uid_key (account_id, imap_folder, imap_uid),
    UNIQUE KEY idx_mail_messages_local_uid (local_uid),
    CONSTRAINT messages_folder_check
        CHECK (folder IN ('inbox', 'sent', 'drafts', 'spam', 'trash', 'custom', 'archive')),
    CONSTRAINT mail_messages_category_chk
        CHECK (category IS NULL OR category IN ('main', 'social', 'notifications', 'promotions')),
    FOREIGN KEY (thread_id)  REFERENCES mail.threads(id)  ON DELETE CASCADE,
    FOREIGN KEY (account_id) REFERENCES mail.accounts(id) ON DELETE CASCADE
);
CREATE INDEX idx_mail_messages_thread      ON mail.messages(thread_id, received_at DESC);
CREATE INDEX idx_mail_messages_account     ON mail.messages(account_id, folder, received_at DESC);
CREATE INDEX idx_mail_messages_user        ON mail.messages(user_id);
CREATE INDEX idx_mail_messages_unread      ON mail.messages(account_id, folder, is_read, is_deleted);
CREATE INDEX idx_mail_messages_starred     ON mail.messages(account_id, is_starred);
CREATE INDEX idx_mail_messages_imap_folder ON mail.messages(account_id, imap_folder, received_at DESC);
CREATE INDEX idx_mail_messages_modseq      ON mail.messages(user_id, folder, modseq);

CREATE TABLE mail.drafts (
    id            BINARY(16)  NOT NULL PRIMARY KEY,
    account_id    BINARY(16)  NOT NULL,
    user_id       BINARY(16)  NOT NULL,
    to_addresses  JSON        NOT NULL DEFAULT ('[]'),
    cc_addresses  JSON        NOT NULL DEFAULT ('[]'),
    bcc_addresses JSON        NOT NULL DEFAULT ('[]'),
    subject       TEXT        NOT NULL DEFAULT (''),
    body_html     LONGTEXT    NOT NULL DEFAULT (''),
    reply_to_id   BINARY(16)  NULL,
    attachments   JSON        NOT NULL DEFAULT ('[]'),
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    scheduled_at  DATETIME(6) NULL,
    FOREIGN KEY (account_id)  REFERENCES mail.accounts(id) ON DELETE CASCADE,
    FOREIGN KEY (reply_to_id) REFERENCES mail.messages(id) ON DELETE SET NULL
);
CREATE INDEX idx_mail_drafts_account   ON mail.drafts(account_id);
CREATE INDEX idx_mail_drafts_user      ON mail.drafts(user_id);
CREATE INDEX idx_mail_drafts_scheduled ON mail.drafts(scheduled_at);

CREATE TABLE mail.thread_labels (
    thread_id BINARY(16) NOT NULL,
    label_id  BINARY(16) NOT NULL,
    PRIMARY KEY (thread_id, label_id),
    FOREIGN KEY (thread_id) REFERENCES mail.threads(id) ON DELETE CASCADE,
    FOREIGN KEY (label_id)  REFERENCES mail.labels(id)  ON DELETE CASCADE
);

-- ── CONDSTORE modseq, local_uid, QRESYNC tombstones (000018, 000021) ────────
CREATE TABLE mail.message_tombstones (
    user_id   BINARY(16)  NOT NULL,
    folder    VARCHAR(50) NOT NULL,
    local_uid BIGINT      NOT NULL,
    modseq    BIGINT      NOT NULL,
    PRIMARY KEY (user_id, folder, local_uid)
);
CREATE INDEX idx_mail_tombstones_modseq ON mail.message_tombstones(user_id, folder, modseq);

-- nextval() of the two PostgreSQL sequences: the counter row is incremented and
-- re-read in the same transaction (its row lock orders concurrent writers).
CREATE TRIGGER mail.mail_messages_bump_modseq_ins BEFORE INSERT ON mail.messages FOR EACH ROW
BEGIN
    UPDATE mail.change_counter SET n = n + 1 WHERE domain = 'modseq';
    SET NEW.modseq = (SELECT n FROM mail.change_counter WHERE domain = 'modseq');
    IF NEW.local_uid IS NULL THEN
        UPDATE mail.change_counter SET n = n + 1 WHERE domain = 'local_uid';
        SET NEW.local_uid = (SELECT n FROM mail.change_counter WHERE domain = 'local_uid');
    END IF;
END;

CREATE TRIGGER mail.mail_messages_bump_modseq_upd BEFORE UPDATE ON mail.messages FOR EACH ROW
BEGIN
    UPDATE mail.change_counter SET n = n + 1 WHERE domain = 'modseq';
    SET NEW.modseq = (SELECT n FROM mail.change_counter WHERE domain = 'modseq');
END;

-- A message leaving a folder (moved, or soft-deleted) leaves a tombstone stamped
-- with its new modseq; arriving in a folder clears any tombstone there.
CREATE TRIGGER mail.mail_messages_tombstone AFTER UPDATE ON mail.messages FOR EACH ROW
BEGIN
    IF (NOT (NEW.folder <=> OLD.folder)) OR (NEW.is_deleted AND NOT OLD.is_deleted) THEN
        IF OLD.local_uid IS NOT NULL THEN
            INSERT INTO mail.message_tombstones (user_id, folder, local_uid, modseq)
            VALUES (OLD.user_id, OLD.folder, OLD.local_uid, NEW.modseq)
            ON DUPLICATE KEY UPDATE modseq = NEW.modseq;
        END IF;
    END IF;
    DELETE FROM mail.message_tombstones
     WHERE user_id = NEW.user_id AND folder = NEW.folder AND local_uid = NEW.local_uid;
END;

-- ── Filters, blocked senders, spam model, address index (000009..000012) ─────
CREATE TABLE mail.filters (
    id               BINARY(16)  NOT NULL PRIMARY KEY,
    user_id          BINARY(16)  NOT NULL,
    account_id       BINARY(16)  NULL,
    from_contains    TEXT        NULL,
    to_contains      TEXT        NULL,
    subject_contains TEXT        NULL,
    query_contains   TEXT        NULL,
    act_archive      BOOLEAN     NOT NULL DEFAULT FALSE,
    act_mark_read    BOOLEAN     NOT NULL DEFAULT FALSE,
    act_star         BOOLEAN     NOT NULL DEFAULT FALSE,
    act_important    BOOLEAN     NOT NULL DEFAULT FALSE,
    act_trash        BOOLEAN     NOT NULL DEFAULT FALSE,
    act_spam         BOOLEAN     NOT NULL DEFAULT FALSE,
    act_label_id     BINARY(16)  NULL,
    position         INT         NOT NULL DEFAULT 0,
    created_at       DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    FOREIGN KEY (account_id)   REFERENCES mail.accounts(id) ON DELETE CASCADE,
    FOREIGN KEY (act_label_id) REFERENCES mail.labels(id)   ON DELETE SET NULL
);
CREATE INDEX idx_mail_filters_user ON mail.filters(user_id);

CREATE TABLE mail.blocked_senders (
    id         BINARY(16)   NOT NULL PRIMARY KEY,
    user_id    BINARY(16)   NOT NULL,
    email      VARCHAR(500) NOT NULL,
    created_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    UNIQUE KEY blocked_senders_user_id_email_key (user_id, email)
);
CREATE INDEX idx_mail_blocked_user ON mail.blocked_senders(user_id);

CREATE TABLE mail.spam_tokens (
    user_id    BINARY(16)   NOT NULL,
    -- Words are capped at 30 characters by the tokenizer; `from:<address>`
    -- tokens carry a whole address.
    token      VARCHAR(700) NOT NULL,
    spam_count INT          NOT NULL DEFAULT 0,
    ham_count  INT          NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, token)
);

CREATE TABLE mail.spam_stats (
    user_id       BINARY(16)  NOT NULL PRIMARY KEY,
    spam_messages INT         NOT NULL DEFAULT 0,
    ham_messages  INT         NOT NULL DEFAULT 0,
    auto_classify BOOLEAN     NOT NULL DEFAULT TRUE,
    threshold     FLOAT       NOT NULL DEFAULT 0.95,
    updated_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
);

CREATE TABLE mail.address_index (
    user_id      BINARY(16)   NOT NULL,
    email        VARCHAR(700) NOT NULL,
    name         TEXT         NULL,
    use_count    INT          NOT NULL DEFAULT 1,
    last_used_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (user_id, email)
);
-- Prefix search (email LIKE 'term%'); PostgreSQL needed text_pattern_ops for it.
CREATE INDEX idx_mail_addr_prefix ON mail.address_index(user_id, email);

-- ── OAuth (000015, 000016) ───────────────────────────────────────────────────
CREATE TABLE mail.oauth_states (
    state        VARCHAR(255) NOT NULL PRIMARY KEY,
    user_id      BINARY(16)   NOT NULL,
    provider     TEXT         NOT NULL,
    created_at   DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    redirect_uri TEXT         NULL
);

-- ── Full sync bookkeeping (000017) ───────────────────────────────────────────
CREATE TABLE mail.folder_sync (
    account_id      BINARY(16)   NOT NULL,
    imap_folder     VARCHAR(500) NOT NULL,
    folder          VARCHAR(50)  NOT NULL,
    uid_low         BIGINT       NULL,
    uid_high        BIGINT       NULL,
    backfill_done   BOOLEAN      NOT NULL DEFAULT FALSE,
    messages_synced INT          NOT NULL DEFAULT 0,
    last_error      TEXT         NULL,
    last_sync_at    DATETIME(6)  NULL,
    created_at      DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (account_id, imap_folder),
    FOREIGN KEY (account_id) REFERENCES mail.accounts(id) ON DELETE CASCADE
);
CREATE INDEX idx_mail_folder_sync_account ON mail.folder_sync(account_id);

-- ── Served-protocol credentials and sessions (000018, 000021) ────────────────
CREATE TABLE mail.mailbox_credentials (
    id               BINARY(16)   NOT NULL PRIMARY KEY,
    user_id          BINARY(16)   NOT NULL,
    username         VARCHAR(320) NOT NULL,
    password_hash    TEXT         NOT NULL,
    label            VARCHAR(120) NULL,
    last_used_at     DATETIME(6)  NULL,
    created_at       DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    scram_salt       BLOB         NULL,
    scram_iterations INT          NULL,
    scram_stored_key BLOB         NULL,
    scram_server_key BLOB         NULL,
    UNIQUE KEY mailbox_credentials_username_key (username)
);
CREATE INDEX idx_mail_mailbox_credentials_user ON mail.mailbox_credentials(user_id);

CREATE TABLE mail.server_sessions (
    id         BINARY(16)   NOT NULL PRIMARY KEY,
    protocol   VARCHAR(10)  NOT NULL CHECK (protocol IN ('smtp', 'imap', 'pop3')),
    user_id    BINARY(16)   NULL,
    username   VARCHAR(320) NULL,
    peer       VARCHAR(100) NOT NULL,
    authed     BOOLEAN      NOT NULL DEFAULT FALSE,
    commands   INT          NOT NULL DEFAULT 0,
    error      TEXT         NULL,
    started_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    ended_at   DATETIME(6)  NULL
);
CREATE INDEX idx_mail_server_sessions_started ON mail.server_sessions(started_at DESC);

-- ── Outbound queue and DKIM keys (000019, 000023) ────────────────────────────
CREATE TABLE mail.outbound_messages (
    id            BINARY(16)  NOT NULL PRIMARY KEY,
    user_id       BINARY(16)  NULL,
    account_id    BINARY(16)  NULL,
    envelope_from TEXT        NOT NULL,
    raw           LONGBLOB    NOT NULL,
    is_dsn        BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at    DATETIME(6) NOT NULL DEFAULT (CURRENT_TIMESTAMP(6) + INTERVAL 5 DAY)
);

CREATE TABLE mail.outbound_recipients (
    id              BINARY(16)  NOT NULL PRIMARY KEY,
    message_id      BINARY(16)  NOT NULL,
    recipient       TEXT        NOT NULL,
    domain          TEXT        NOT NULL,
    status          VARCHAR(16) NOT NULL DEFAULT 'queued'
                        CHECK (status IN ('queued', 'delivering', 'sent', 'deferred', 'bounced')),
    attempts        INT         NOT NULL DEFAULT 0,
    next_attempt_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    last_code       INT         NULL,
    last_reason     TEXT        NULL,
    locked_by       BINARY(16)  NULL,
    locked_until    DATETIME(6) NULL,
    delivered_at    DATETIME(6) NULL,
    created_at      DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    FOREIGN KEY (message_id) REFERENCES mail.outbound_messages(id) ON DELETE CASCADE
);
-- The claim query: due rows in a claimable status.
CREATE INDEX idx_mail_outbound_claimable ON mail.outbound_recipients(next_attempt_at, status);
CREATE INDEX idx_mail_outbound_message   ON mail.outbound_recipients(message_id);

CREATE TABLE mail.dkim_keys_all (
    id                BINARY(16)   NOT NULL PRIMARY KEY,
    domain            VARCHAR(255) NOT NULL,
    selector          VARCHAR(255) NOT NULL,
    algorithm         VARCHAR(20)  NOT NULL DEFAULT 'rsa-sha256'
                          CHECK (algorithm IN ('rsa-sha256', 'ed25519-sha256')),
    private_key_enc   BLOB         NOT NULL,
    private_key_nonce BLOB         NOT NULL,
    public_key        TEXT         NOT NULL,
    created_at        DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    is_active         BOOLEAN      NOT NULL DEFAULT TRUE,
    -- NULL unless active: "at most one active key per domain".
    active_domain     VARCHAR(255) GENERATED ALWAYS AS (CASE WHEN is_active THEN domain END) VIRTUAL,
    UNIQUE KEY uq_mail_dkim_domain_selector (domain, selector),
    UNIQUE KEY uq_mail_dkim_active_per_domain (active_domain)
);

-- The active signing key per domain (read by the signer).
CREATE VIEW mail.dkim_keys AS
    SELECT id, domain, selector, algorithm,
           private_key_enc, private_key_nonce, public_key, created_at
      FROM mail.dkim_keys_all
     WHERE is_active;

-- ── Greylisting (000022) ─────────────────────────────────────────────────────
CREATE TABLE mail.greylist (
    client_net VARCHAR(64)  NOT NULL,
    sender     VARCHAR(320) NOT NULL,
    recipient  VARCHAR(320) NOT NULL,
    first_seen DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    last_seen  DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    passed_at  DATETIME(6)  NULL,
    PRIMARY KEY (client_net, sender, recipient)
);
CREATE INDEX mail_greylist_last_seen_idx ON mail.greylist(last_seen);

-- ── Outbound relay singleton (000026) ────────────────────────────────────────
CREATE TABLE mail.outbound_relay (
    id             BOOLEAN      NOT NULL DEFAULT TRUE PRIMARY KEY CHECK (id = TRUE),
    enabled        BOOLEAN      NOT NULL DEFAULT FALSE,
    host           VARCHAR(255) NOT NULL DEFAULT '',
    port           INT          NOT NULL DEFAULT 25 CHECK (port BETWEEN 1 AND 65535),
    security       VARCHAR(10)  NOT NULL DEFAULT 'none'
                       CHECK (security IN ('none', 'starttls', 'tls')),
    username       VARCHAR(255) NOT NULL DEFAULT '',
    password_enc   BLOB         NULL,
    password_nonce BLOB         NULL,
    updated_at     DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
);
INSERT INTO mail.outbound_relay (id, enabled) VALUES (TRUE, FALSE);

-- ── Sender avatars (000027) ──────────────────────────────────────────────────
CREATE TABLE mail.sender_avatars (
    domain     VARCHAR(255) NOT NULL PRIMARY KEY,
    source     VARCHAR(16)  NOT NULL DEFAULT 'bimi',
    mime       VARCHAR(64)  NULL,
    bytes      LONGBLOB     NULL,
    not_found  BOOLEAN      NOT NULL DEFAULT FALSE,
    fetched_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at DATETIME(6)  NOT NULL DEFAULT (CURRENT_TIMESTAMP(6) + INTERVAL 7 DAY)
);
CREATE INDEX idx_mail_sender_avatars_expiry ON mail.sender_avatars(expires_at);

-- ── OpenPGP (000030) ─────────────────────────────────────────────────────────
CREATE TABLE mail.pgp_keys (
    id                BINARY(16)   NOT NULL PRIMARY KEY,
    user_id           BINARY(16)   NOT NULL,
    email             VARCHAR(500) NULL,
    fingerprint       VARCHAR(64)  NOT NULL,
    public_key        MEDIUMTEXT   NOT NULL,
    private_key       LONGBLOB     NOT NULL,
    private_key_nonce BLOB         NOT NULL,
    is_default        BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at        DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at        DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    email_lc          VARCHAR(500) GENERATED ALWAYS AS (LOWER(email)) VIRTUAL,
    UNIQUE KEY pgp_keys_user_id_fingerprint_key (user_id, fingerprint)
);
CREATE INDEX idx_mail_pgp_keys_user  ON mail.pgp_keys(user_id);
CREATE INDEX idx_mail_pgp_keys_email ON mail.pgp_keys(user_id, email_lc);

CREATE TABLE mail.pgp_contacts (
    id          BINARY(16)   NOT NULL PRIMARY KEY,
    user_id     BINARY(16)   NOT NULL,
    email       VARCHAR(500) NOT NULL,
    fingerprint VARCHAR(64)  NOT NULL,
    public_key  MEDIUMTEXT   NOT NULL,
    source      VARCHAR(16)  NOT NULL DEFAULT 'manual'
                    CHECK (source IN ('manual', 'wkd', 'autocrypt', 'attached')),
    created_at  DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at  DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    email_lc    VARCHAR(500) GENERATED ALWAYS AS (LOWER(email)) VIRTUAL,
    UNIQUE KEY idx_mail_pgp_contacts_lookup (user_id, email_lc)
);

-- ── Vacation responder, forwarding, send-as, idempotency (000032..000035) ────
CREATE TABLE mail.vacation_responders (
    user_id       BINARY(16)  NOT NULL PRIMARY KEY,
    enabled       BOOLEAN     NOT NULL DEFAULT FALSE,
    start_date    DATE        NULL,
    end_date      DATE        NULL,
    subject       TEXT        NOT NULL DEFAULT (''),
    message_html  LONGTEXT    NOT NULL DEFAULT (''),
    contacts_only BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
);

CREATE TABLE mail.vacation_sent (
    user_id    BINARY(16)   NOT NULL,
    from_email VARCHAR(700) NOT NULL,
    sent_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (user_id, from_email)
);
CREATE INDEX idx_mail_vacation_sent_sent_at ON mail.vacation_sent(sent_at);

CREATE TABLE mail.forwarding_rules (
    user_id    BINARY(16)   NOT NULL,
    forward_to VARCHAR(320) NOT NULL,
    enabled    BOOLEAN      NOT NULL DEFAULT TRUE,
    keep_copy  BOOLEAN      NOT NULL DEFAULT TRUE,
    created_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (user_id, forward_to)
);
CREATE INDEX idx_mail_forwarding_rules_user ON mail.forwarding_rules(user_id);

CREATE TABLE mail.send_as_addresses (
    id                      BINARY(16)   NOT NULL PRIMARY KEY,
    user_id                 BINARY(16)   NOT NULL,
    email                   VARCHAR(320) NOT NULL,
    display_name            VARCHAR(255) NOT NULL DEFAULT '',
    verified                BOOLEAN      NOT NULL DEFAULT FALSE,
    verification_code       VARCHAR(16)  NULL,
    verification_expires_at DATETIME(6)  NULL,
    treat_as_alias          BOOLEAN      NOT NULL DEFAULT TRUE,
    created_at              DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at              DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    email_lc                VARCHAR(320) GENERATED ALWAYS AS (LOWER(email)) VIRTUAL,
    UNIQUE KEY uq_mail_send_as_user_email (user_id, email_lc)
);
CREATE INDEX idx_mail_send_as_user ON mail.send_as_addresses(user_id);

CREATE TABLE mail.send_idempotency (
    user_id       BINARY(16)   NOT NULL,
    `key`         VARCHAR(255) NOT NULL,
    response_json JSON         NULL,
    created_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (user_id, `key`)
);
CREATE INDEX idx_mail_send_idempotency_created_at ON mail.send_idempotency(created_at);

-- ── Delegation, POP/IMAP policy, templates, groups, images (000036..000042) ──
CREATE TABLE mail.delegations (
    id               BINARY(16)   NOT NULL PRIMARY KEY,
    grantor_user_id  BINARY(16)   NOT NULL,
    grantor_email    VARCHAR(320) NOT NULL DEFAULT '',
    delegate_user_id BINARY(16)   NOT NULL,
    delegate_email   VARCHAR(320) NOT NULL,
    status           VARCHAR(20)  NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'accepted', 'revoked')),
    can_send         BOOLEAN      NOT NULL DEFAULT TRUE,
    created_at       DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    accepted_at      DATETIME(6)  NULL,
    updated_at       DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    UNIQUE KEY uq_mail_delegations_pair (grantor_user_id, delegate_user_id)
);
CREATE INDEX idx_mail_delegations_grantor  ON mail.delegations(grantor_user_id);
CREATE INDEX idx_mail_delegations_delegate ON mail.delegations(delegate_user_id);

CREATE TABLE mail.pop_imap_settings (
    user_id           BINARY(16)  NOT NULL PRIMARY KEY,
    imap_enabled      BOOLEAN     NOT NULL DEFAULT TRUE,
    imap_expunge_mode VARCHAR(8)  NOT NULL DEFAULT 'wait'
                          CHECK (imap_expunge_mode IN ('auto', 'wait')),
    imap_auto_expunge BOOLEAN     NOT NULL DEFAULT FALSE,
    imap_purge_mode   VARCHAR(10) NOT NULL DEFAULT 'trash'
                          CHECK (imap_purge_mode IN ('archive', 'trash', 'delete')),
    imap_folder_limit INT         NOT NULL DEFAULT 0 CHECK (imap_folder_limit >= 0),
    pop_enabled       BOOLEAN     NOT NULL DEFAULT TRUE,
    pop_mode          VARCHAR(10) NOT NULL DEFAULT 'all'
                          CHECK (pop_mode IN ('all', 'from_now')),
    pop_from_uid      BIGINT      NOT NULL DEFAULT 0,
    pop_post_action   VARCHAR(10) NOT NULL DEFAULT 'mark_read'
                          CHECK (pop_post_action IN ('keep', 'mark_read', 'archive', 'delete')),
    created_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
);

CREATE TABLE mail.email_templates (
    id         BINARY(16)   NOT NULL PRIMARY KEY,
    user_id    BINARY(16)   NOT NULL,
    name       VARCHAR(255) NOT NULL,
    subject    VARCHAR(998) NOT NULL DEFAULT '',
    body_html  LONGTEXT     NOT NULL DEFAULT (''),
    created_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    name_lc    VARCHAR(255) GENERATED ALWAYS AS (LOWER(name)) VIRTUAL,
    UNIQUE KEY uq_mail_email_templates_user_name (user_id, name_lc)
);
CREATE INDEX idx_mail_email_templates_user ON mail.email_templates(user_id);

CREATE TABLE mail.recipient_groups (
    id         BINARY(16)   NOT NULL PRIMARY KEY,
    user_id    BINARY(16)   NOT NULL,
    name       VARCHAR(255) NOT NULL,
    members    JSON         NOT NULL DEFAULT ('[]'),
    created_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    name_lc    VARCHAR(255) GENERATED ALWAYS AS (LOWER(name)) VIRTUAL,
    UNIQUE KEY uq_mail_recipient_groups_user_name (user_id, name_lc)
);
CREATE INDEX idx_mail_recipient_groups_user ON mail.recipient_groups(user_id);

CREATE TABLE mail.image_allowed_senders (
    id         BINARY(16)   NOT NULL PRIMARY KEY,
    user_id    BINARY(16)   NOT NULL,
    email      VARCHAR(500) NOT NULL,
    created_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    UNIQUE KEY image_allowed_senders_user_id_email_key (user_id, email)
);
CREATE INDEX idx_mail_image_allowed_user ON mail.image_allowed_senders(user_id);
