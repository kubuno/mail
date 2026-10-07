ALTER TABLE mail.labels DROP CONSTRAINT IF EXISTS mail_labels_list_visibility_chk;
ALTER TABLE mail.labels DROP CONSTRAINT IF EXISTS mail_labels_msg_visibility_chk;
ALTER TABLE mail.labels
    DROP COLUMN IF EXISTS list_visibility,
    DROP COLUMN IF EXISTS message_list_visibility;
