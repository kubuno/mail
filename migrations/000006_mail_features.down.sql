ALTER TABLE mail.threads  DROP COLUMN IF EXISTS is_important;
ALTER TABLE mail.threads  DROP COLUMN IF EXISTS snoozed_until;
ALTER TABLE mail.drafts   DROP COLUMN IF EXISTS scheduled_at;
ALTER TABLE mail.messages DROP COLUMN IF EXISTS list_unsubscribe;
