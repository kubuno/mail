DROP INDEX IF EXISTS mail.idx_mail_accounts_kind;
DROP INDEX IF EXISTS mail.idx_mail_accounts_mailbox;

ALTER TABLE mail.accounts
    DROP COLUMN IF EXISTS mailbox_id,
    DROP COLUMN IF EXISTS kind;
