DROP TRIGGER IF EXISTS mailing_lists_address_free ON mail.mailing_lists;
DROP TRIGGER IF EXISTS aliases_address_free       ON mail.aliases;
DROP TRIGGER IF EXISTS mailboxes_address_free     ON mail.mailboxes;
DROP FUNCTION IF EXISTS mail.assert_address_is_free();

DROP TRIGGER IF EXISTS domain_policies_touch ON mail.domain_policies;
DROP TRIGGER IF EXISTS mailing_lists_touch   ON mail.mailing_lists;
DROP TRIGGER IF EXISTS aliases_touch         ON mail.aliases;
DROP TRIGGER IF EXISTS mailboxes_touch       ON mail.mailboxes;
-- `touch_updated_at` is left in place: it is generic and another migration may
-- have started using it. Dropping a function somebody else depends on is how a
-- rollback breaks more than it undoes.

DROP TABLE IF EXISTS mail.domain_policies;
DROP TABLE IF EXISTS mail.mailing_list_members;
DROP TABLE IF EXISTS mail.mailing_lists;
DROP TABLE IF EXISTS mail.aliases;
DROP TABLE IF EXISTS mail.mailboxes;
