-- Back to one key per domain: drop the retired/staged keys, then restore the
-- table under its original name and unique constraint.
DROP VIEW IF EXISTS mail.dkim_keys;

DELETE FROM mail.dkim_keys_all WHERE NOT is_active;

DROP INDEX IF EXISTS mail.uq_mail_dkim_active_per_domain;
DROP INDEX IF EXISTS mail.uq_mail_dkim_domain_selector;

ALTER TABLE mail.dkim_keys_all RENAME TO dkim_keys;
ALTER TABLE mail.dkim_keys DROP COLUMN IF EXISTS is_active;
ALTER TABLE mail.dkim_keys ADD CONSTRAINT dkim_keys_domain_key UNIQUE (domain);
