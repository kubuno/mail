ALTER TABLE mail.mailbox_credentials
    DROP COLUMN IF EXISTS scram_salt,
    DROP COLUMN IF EXISTS scram_iterations,
    DROP COLUMN IF EXISTS scram_stored_key,
    DROP COLUMN IF EXISTS scram_server_key;

DROP TRIGGER IF EXISTS mail_messages_tombstone ON mail.messages;
DROP FUNCTION IF EXISTS mail.record_tombstone();
DROP TABLE IF EXISTS mail.message_tombstones;

DROP TRIGGER IF EXISTS mail_messages_bump_modseq ON mail.messages;
DROP FUNCTION IF EXISTS mail.bump_modseq();

DROP INDEX IF EXISTS mail.idx_mail_messages_modseq;
ALTER TABLE mail.messages DROP COLUMN IF EXISTS modseq;
DROP SEQUENCE IF EXISTS mail.message_modseq_seq;
