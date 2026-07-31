-- Per-label display settings, mirroring the two Gmail toggles:
--   list_visibility         — in the labels sidebar: always / only when unread / never
--   message_list_visibility — chip shown on message rows: always / never
ALTER TABLE mail.labels
    ADD COLUMN IF NOT EXISTS list_visibility         TEXT NOT NULL DEFAULT 'show',
    ADD COLUMN IF NOT EXISTS message_list_visibility TEXT NOT NULL DEFAULT 'show';

ALTER TABLE mail.labels
    DROP CONSTRAINT IF EXISTS mail_labels_list_visibility_chk;
ALTER TABLE mail.labels
    ADD CONSTRAINT mail_labels_list_visibility_chk
    CHECK (list_visibility IN ('show', 'unread', 'hide'));

ALTER TABLE mail.labels
    DROP CONSTRAINT IF EXISTS mail_labels_msg_visibility_chk;
ALTER TABLE mail.labels
    ADD CONSTRAINT mail_labels_msg_visibility_chk
    CHECK (message_list_visibility IN ('show', 'hide'));
