ALTER TABLE mail.threads DROP CONSTRAINT IF EXISTS mail_threads_category_chk;
ALTER TABLE mail.threads DROP COLUMN IF EXISTS category;
ALTER TABLE mail.messages
    DROP COLUMN IF EXISTS reply_to,
    DROP COLUMN IF EXISTS mailed_by,
    DROP COLUMN IF EXISTS signed_by,
    DROP COLUMN IF EXISTS security;
