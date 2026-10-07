-- Fonctionnalités : Important, En attente (snooze), Envoi programmé, Abonnements.
ALTER TABLE mail.threads  ADD COLUMN IF NOT EXISTS is_important  BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE mail.threads  ADD COLUMN IF NOT EXISTS snoozed_until TIMESTAMPTZ;
ALTER TABLE mail.drafts   ADD COLUMN IF NOT EXISTS scheduled_at  TIMESTAMPTZ;
ALTER TABLE mail.messages ADD COLUMN IF NOT EXISTS list_unsubscribe TEXT;

CREATE INDEX IF NOT EXISTS idx_mail_threads_important ON mail.threads(user_id) WHERE is_important = TRUE;
CREATE INDEX IF NOT EXISTS idx_mail_threads_snoozed   ON mail.threads(snoozed_until) WHERE snoozed_until IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_mail_drafts_scheduled  ON mail.drafts(scheduled_at) WHERE scheduled_at IS NOT NULL;
