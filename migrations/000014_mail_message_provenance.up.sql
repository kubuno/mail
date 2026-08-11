-- Provenance headers shown in the message "details" panel: who to reply to,
-- which host actually sent the mail, who signed it (DKIM) and whether the last
-- hop was encrypted. Kept as plain text — they are display-only.
ALTER TABLE mail.messages
    ADD COLUMN IF NOT EXISTS reply_to  TEXT,
    ADD COLUMN IF NOT EXISTS mailed_by TEXT,
    ADD COLUMN IF NOT EXISTS signed_by TEXT,
    ADD COLUMN IF NOT EXISTS security  TEXT;

-- Category assigned by dropping a conversation onto a tab. NULL = fall back to
-- the client-side heuristic based on the sender.
ALTER TABLE mail.threads
    ADD COLUMN IF NOT EXISTS category TEXT;

ALTER TABLE mail.threads DROP CONSTRAINT IF EXISTS mail_threads_category_chk;
ALTER TABLE mail.threads
    ADD CONSTRAINT mail_threads_category_chk
    CHECK (category IS NULL OR category IN ('main', 'social', 'notifications', 'promotions'));
