DROP INDEX IF EXISTS mail.idx_mail_messages_local_uid;
ALTER TABLE mail.messages DROP COLUMN IF EXISTS local_uid;
DROP SEQUENCE IF EXISTS mail.message_local_uid_seq;

DROP TABLE IF EXISTS mail.server_sessions;
DROP TABLE IF EXISTS mail.mailbox_credentials;
