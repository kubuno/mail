DROP TRIGGER IF EXISTS pgp_contacts_updated_at ON mail.pgp_contacts;
DROP TRIGGER IF EXISTS pgp_keys_updated_at ON mail.pgp_keys;
DROP TABLE IF EXISTS mail.pgp_contacts;
DROP TABLE IF EXISTS mail.pgp_keys;
