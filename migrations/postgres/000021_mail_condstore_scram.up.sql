-- CONDSTORE/QRESYNC modification sequences, QRESYNC vanished tombstones, and
-- SASL SCRAM-SHA-256 secrets.

-- ── modseq: a monotonic counter bumped on every message change ────────────────
-- CONDSTORE (RFC 7162) lets a client sync flags efficiently: it remembers a
-- HIGHESTMODSEQ and asks only for what changed since. Every write to a message
-- gets a fresh, ever-increasing modseq; the folder's HIGHESTMODSEQ is the max
-- over its messages (and its tombstones, below).
CREATE SEQUENCE IF NOT EXISTS mail.message_modseq_seq;

ALTER TABLE mail.messages
    ADD COLUMN IF NOT EXISTS modseq BIGINT NOT NULL DEFAULT nextval('mail.message_modseq_seq');

CREATE INDEX IF NOT EXISTS idx_mail_messages_modseq
    ON mail.messages(user_id, folder, modseq);

-- BEFORE trigger: stamp a new modseq on insert and on every update, so any
-- change (flags, folder move, deletion) advances it. Separate from the AFTER
-- NOTIFY trigger of 000020.
CREATE OR REPLACE FUNCTION mail.bump_modseq() RETURNS trigger AS $$
BEGIN
    NEW.modseq := nextval('mail.message_modseq_seq');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS mail_messages_bump_modseq ON mail.messages;
CREATE TRIGGER mail_messages_bump_modseq
    BEFORE INSERT OR UPDATE ON mail.messages
    FOR EACH ROW EXECUTE FUNCTION mail.bump_modseq();

-- ── QRESYNC vanished tombstones ───────────────────────────────────────────────
-- When a message leaves a folder (moved out, or deleted), QRESYNC's VANISHED
-- (EARLIER) response must be able to tell a reconnecting client which UIDs are
-- gone since its last modseq. A soft delete/move erases the row's old folder,
-- so we record the departure here.
CREATE TABLE IF NOT EXISTS mail.message_tombstones (
    user_id   UUID   NOT NULL,
    folder    TEXT   NOT NULL,
    local_uid BIGINT NOT NULL,
    modseq    BIGINT NOT NULL,
    PRIMARY KEY (user_id, folder, local_uid)
);

CREATE INDEX IF NOT EXISTS idx_mail_tombstones_modseq
    ON mail.message_tombstones(user_id, folder, modseq);

-- Record a tombstone when a message's folder changes or it is deleted. The
-- departure is stamped with the NEW modseq (bumped just above).
CREATE OR REPLACE FUNCTION mail.record_tombstone() RETURNS trigger AS $$
BEGIN
    IF (NEW.folder IS DISTINCT FROM OLD.folder) OR
       (NEW.is_deleted AND NOT OLD.is_deleted) THEN
        IF OLD.local_uid IS NOT NULL THEN
            INSERT INTO mail.message_tombstones (user_id, folder, local_uid, modseq)
            VALUES (OLD.user_id, OLD.folder, OLD.local_uid, NEW.modseq)
            ON CONFLICT (user_id, folder, local_uid)
                DO UPDATE SET modseq = EXCLUDED.modseq;
        END IF;
    END IF;
    -- Arriving in a new folder clears any prior tombstone for that (folder, uid).
    DELETE FROM mail.message_tombstones
        WHERE user_id = NEW.user_id AND folder = NEW.folder AND local_uid = NEW.local_uid;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS mail_messages_tombstone ON mail.messages;
CREATE TRIGGER mail_messages_tombstone
    AFTER UPDATE ON mail.messages
    FOR EACH ROW EXECUTE FUNCTION mail.record_tombstone();

-- ── SASL SCRAM-SHA-256 secrets ────────────────────────────────────────────────
-- SCRAM proves knowledge of the password without ever sending it, and a leak of
-- these columns does not reveal it (an attacker would have to break PBKDF2).
-- Derived from the password once, at credential creation, alongside the Argon2
-- hash (which the plain LOGIN/PLAIN path still uses).
ALTER TABLE mail.mailbox_credentials
    ADD COLUMN IF NOT EXISTS scram_salt       BYTEA,
    ADD COLUMN IF NOT EXISTS scram_iterations INTEGER,
    ADD COLUMN IF NOT EXISTS scram_stored_key BYTEA,
    ADD COLUMN IF NOT EXISTS scram_server_key BYTEA;
