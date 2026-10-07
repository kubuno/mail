-- Portable list columns (kubuno-db §2.8).
--
-- MySQL/MariaDB and SQLite have no array type, so the three list columns the
-- module keeps become JSON arrays — the representation the code now reads and
-- writes on every engine (`#[sqlx(json)]` / `JsonVec`, filtered with
-- `json_array_contains`). The values are converted in place: a UUID[] becomes an
-- array of the UUIDs' text form, a TEXT[] an array of strings. No index ever
-- covered these columns, so there is none to swap.

-- messages.label_ids  UUID[] -> jsonb
ALTER TABLE mail.messages ALTER COLUMN label_ids DROP DEFAULT;
ALTER TABLE mail.messages ALTER COLUMN label_ids TYPE jsonb USING to_jsonb(label_ids);
ALTER TABLE mail.messages ALTER COLUMN label_ids SET DEFAULT '[]'::jsonb;

-- aliases.destinations  TEXT[] -> jsonb. The column CHECK (cardinality(...) > 0)
-- cannot survive the type change; it is re-stated over the JSON array.
ALTER TABLE mail.aliases DROP CONSTRAINT IF EXISTS aliases_destinations_check;
ALTER TABLE mail.aliases ALTER COLUMN destinations TYPE jsonb USING to_jsonb(destinations);
ALTER TABLE mail.aliases ADD CONSTRAINT aliases_destinations_check
    CHECK (CASE WHEN jsonb_typeof(destinations) = 'array'
                THEN jsonb_array_length(destinations) > 0
                ELSE FALSE END);

-- mailing_lists.allowed_senders  TEXT[] -> jsonb
ALTER TABLE mail.mailing_lists ALTER COLUMN allowed_senders DROP DEFAULT;
ALTER TABLE mail.mailing_lists ALTER COLUMN allowed_senders TYPE jsonb USING to_jsonb(allowed_senders);
ALTER TABLE mail.mailing_lists ALTER COLUMN allowed_senders SET DEFAULT '[]'::jsonb;

-- Fix-forward for 000023 on a schema-prefixed install: its lookup of the
-- single-column UNIQUE(domain) constraint was keyed on the literal schema name
-- 'mail', so with a prefix it found nothing and left the constraint in place,
-- which rejects every DKIM rotation. Keyed on the table's regclass instead (the
-- prefix rewrite does reach this literal). A no-op where 000023 already did it.
DO $$
DECLARE c TEXT;
BEGIN
    FOR c IN
        SELECT con.conname
          FROM pg_constraint con
         WHERE con.conrelid = 'mail.dkim_keys_all'::regclass
           AND con.contype  = 'u'
           AND con.conkey   = ARRAY[(
               SELECT attnum FROM pg_attribute
                WHERE attrelid = con.conrelid AND attname = 'domain'
           )]::smallint[]
    LOOP
        EXECUTE format('ALTER TABLE mail.dkim_keys_all DROP CONSTRAINT %I', c);
    END LOOP;
END $$;
