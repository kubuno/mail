-- « Ignorer » (mute) : la conversation reste hors boîte même à l'arrivée de nouveaux messages.
ALTER TABLE mail.threads ADD COLUMN IF NOT EXISTS is_muted BOOLEAN NOT NULL DEFAULT FALSE;
CREATE INDEX IF NOT EXISTS idx_mail_threads_muted ON mail.threads(user_id) WHERE is_muted = TRUE;
