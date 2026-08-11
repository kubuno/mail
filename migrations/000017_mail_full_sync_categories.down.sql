DROP TABLE IF EXISTS mail.folder_sync;

DROP INDEX IF EXISTS mail.idx_mail_messages_imap_folder;
DROP INDEX IF EXISTS mail.idx_mail_threads_category;

ALTER TABLE mail.threads  DROP COLUMN IF EXISTS category_pinned;

ALTER TABLE mail.messages DROP CONSTRAINT IF EXISTS mail_messages_category_chk;
ALTER TABLE mail.messages DROP COLUMN IF EXISTS category;
