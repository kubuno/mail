-- Real-time change notifications for IMAP IDLE.
--
-- IDLE is the feature every modern client expects: the server pushes changes
-- instead of the client polling. The source of those changes is any write to
-- mail.messages — a new delivery, a flag change, an expunge — from wherever it
-- comes (the web UI, another IMAP session, the sync worker). A trigger turns
-- each such write into a PostgreSQL NOTIFY that the IMAP session LISTENs for.

CREATE OR REPLACE FUNCTION mail.notify_message_change() RETURNS trigger AS $$
DECLARE
    payload TEXT;
    uid     UUID;
    fold    TEXT;
BEGIN
    -- On DELETE the changed row is OLD; otherwise NEW.
    IF (TG_OP = 'DELETE') THEN
        uid  := OLD.user_id;
        fold := OLD.folder;
    ELSE
        uid  := NEW.user_id;
        fold := NEW.folder;
    END IF;

    -- Small JSON payload: which user and which folder changed, so a listening
    -- session only reacts to its own selected mailbox. NOTIFY payloads are
    -- capped at 8000 bytes; this is tiny.
    payload := json_build_object('user_id', uid, 'folder', fold)::text;
    PERFORM pg_notify('mail_changes', payload);

    -- A second notify for the OTHER folder when a message moves between folders
    -- (its source mailbox loses it, the destination gains it).
    IF (TG_OP = 'UPDATE' AND NEW.folder IS DISTINCT FROM OLD.folder) THEN
        PERFORM pg_notify('mail_changes',
            json_build_object('user_id', OLD.user_id, 'folder', OLD.folder)::text);
    END IF;

    RETURN NULL; -- AFTER trigger: return value is ignored.
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS mail_messages_notify ON mail.messages;
CREATE TRIGGER mail_messages_notify
    AFTER INSERT OR UPDATE OR DELETE ON mail.messages
    FOR EACH ROW EXECUTE FUNCTION mail.notify_message_change();
