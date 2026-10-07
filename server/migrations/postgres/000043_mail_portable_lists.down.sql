-- Back to native arrays. ALTER COLUMN ... USING cannot hold a subquery, so each
-- column is rebuilt through a temporary column.

ALTER TABLE mail.messages ADD COLUMN label_ids_arr UUID[] NOT NULL DEFAULT '{}';
UPDATE mail.messages
   SET label_ids_arr = ARRAY(SELECT jsonb_array_elements_text(label_ids)::uuid);
ALTER TABLE mail.messages DROP COLUMN label_ids;
ALTER TABLE mail.messages RENAME COLUMN label_ids_arr TO label_ids;

ALTER TABLE mail.aliases DROP CONSTRAINT IF EXISTS aliases_destinations_check;
ALTER TABLE mail.aliases ADD COLUMN destinations_arr TEXT[];
UPDATE mail.aliases
   SET destinations_arr = ARRAY(SELECT jsonb_array_elements_text(destinations));
ALTER TABLE mail.aliases DROP COLUMN destinations;
ALTER TABLE mail.aliases RENAME COLUMN destinations_arr TO destinations;
ALTER TABLE mail.aliases ALTER COLUMN destinations SET NOT NULL;
ALTER TABLE mail.aliases ADD CONSTRAINT aliases_destinations_check
    CHECK (cardinality(destinations) > 0);

ALTER TABLE mail.mailing_lists ADD COLUMN allowed_senders_arr TEXT[] NOT NULL DEFAULT '{}';
UPDATE mail.mailing_lists
   SET allowed_senders_arr = ARRAY(SELECT jsonb_array_elements_text(allowed_senders));
ALTER TABLE mail.mailing_lists DROP COLUMN allowed_senders;
ALTER TABLE mail.mailing_lists RENAME COLUMN allowed_senders_arr TO allowed_senders;
