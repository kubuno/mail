ALTER TABLE mail.threads
  DROP COLUMN IF EXISTS last_sender_name,
  DROP COLUMN IF EXISTS last_sender_email;
