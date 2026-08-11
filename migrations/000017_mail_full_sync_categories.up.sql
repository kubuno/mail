-- Full mailbox synchronisation + categories assigned at write time.
--
-- Until now the inbox category was recomputed by a SQL CASE on every listing
-- (and mirrored again in the frontend). It is now decided once, when the
-- message is stored, and simply read back afterwards.

-- ── Per-message category ──────────────────────────────────────────────────────
ALTER TABLE mail.messages
    ADD COLUMN IF NOT EXISTS category TEXT;

ALTER TABLE mail.messages DROP CONSTRAINT IF EXISTS mail_messages_category_chk;
ALTER TABLE mail.messages
    ADD CONSTRAINT mail_messages_category_chk
    CHECK (category IS NULL OR category IN ('main', 'social', 'notifications', 'promotions'));

-- ── Thread category: tell a user pin apart from the derived value ─────────────
-- Rows that already carry a category got it by drag & drop onto a tab, so they
-- are pins; everything else will be filled in from its messages below.
ALTER TABLE mail.threads
    ADD COLUMN IF NOT EXISTS category_pinned BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE mail.threads SET category_pinned = TRUE WHERE category IS NOT NULL;

-- ── Backfill: same heuristic as services::categorize, applied once ────────────
UPDATE mail.messages SET category = CASE
    WHEN from_email ~* '(twitter|facebook|linkedin|instagram|tiktok|youtube|pinterest|snapchat|meta\.com|x\.com)' THEN 'social'
    WHEN from_email ~* '(notification|alert|update|security|account|billing|no.?reply.*notif)'                    THEN 'notifications'
    WHEN from_email ~* '(no.?reply|newsletter|noreply|promo|marketing|info@|hello@|contact@|deals?@|offers?@)'     THEN 'promotions'
    ELSE 'main'
END
WHERE category IS NULL;

UPDATE mail.threads t SET category = CASE
    WHEN t.last_sender_email ~* '(twitter|facebook|linkedin|instagram|tiktok|youtube|pinterest|snapchat|meta\.com|x\.com)' THEN 'social'
    WHEN t.last_sender_email ~* '(notification|alert|update|security|account|billing|no.?reply.*notif)'                    THEN 'notifications'
    WHEN t.last_sender_email ~* '(no.?reply|newsletter|noreply|promo|marketing|info@|hello@|contact@|deals?@|offers?@)'     THEN 'promotions'
    ELSE 'main'
END
WHERE t.category IS NULL;

-- Reading a tab is now an indexed equality test instead of a CASE over the table.
CREATE INDEX IF NOT EXISTS idx_mail_threads_category
    ON mail.threads(user_id, category, last_message_at DESC);

-- ── Custom IMAP folders ───────────────────────────────────────────────────────
-- Messages living in a provider folder that is not one of the five well-known
-- ones keep folder = 'custom'; `imap_folder` carries the real name, so listings
-- filter on the pair.
CREATE INDEX IF NOT EXISTS idx_mail_messages_imap_folder
    ON mail.messages(account_id, imap_folder, received_at DESC);

-- ── Full-sync bookkeeping ─────────────────────────────────────────────────────
-- One row per (account, IMAP folder). `uid_high` is how far forward we have
-- read, `uid_low` how far back; backfill walks downwards batch by batch across
-- sync runs until it reaches the bottom, so a large mailbox is downloaded
-- entirely without any single run holding it all in memory.
CREATE TABLE IF NOT EXISTS mail.folder_sync (
    account_id      UUID NOT NULL REFERENCES mail.accounts(id) ON DELETE CASCADE,
    imap_folder     VARCHAR(500) NOT NULL,
    folder          VARCHAR(50)  NOT NULL,
    uid_low         BIGINT,
    uid_high        BIGINT,
    backfill_done   BOOLEAN NOT NULL DEFAULT FALSE,
    messages_synced INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    last_sync_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (account_id, imap_folder)
);

CREATE INDEX IF NOT EXISTS idx_mail_folder_sync_account
    ON mail.folder_sync(account_id);

-- Existing accounts were synced with the old 200-newest-only strategy: seed the
-- forward cursor from what is already stored so the next run backfills the rest
-- instead of re-downloading it.
INSERT INTO mail.folder_sync (account_id, imap_folder, folder, uid_low, uid_high, backfill_done)
SELECT account_id,
       imap_folder,
       MIN(folder),
       MIN(imap_uid),
       MAX(imap_uid),
       FALSE
FROM mail.messages
WHERE imap_uid IS NOT NULL
GROUP BY account_id, imap_folder
ON CONFLICT DO NOTHING;
