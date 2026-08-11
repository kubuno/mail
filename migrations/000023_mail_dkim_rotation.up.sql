-- ── DKIM key rotation: several selectors per domain ──────────────────────────
--
-- Why this exists
--   000019 allowed exactly one key per domain (`domain TEXT NOT NULL UNIQUE`),
--   published under a selector hard-coded to "kubuno". That makes rotation
--   impossible without a gap: replacing the key invalidates every signature
--   still in flight and every message a receiver re-verifies from its cache,
--   because the DNS record cannot change at the same instant as the key.
--   The way rotation is actually done is: publish the new key under a NEW
--   selector, wait for DNS to propagate, switch signing to it, and only then
--   retire the old one. That needs two keys for one domain to coexist.
--
-- Why a view rather than a plain column
--   `server/signing.rs` picks the signing key with
--       SELECT ... FROM mail.dkim_keys WHERE domain = $1
--   and takes the first row. With several rows per domain that query becomes
--   non-deterministic, and signing with a retired key whose record has been
--   removed means every outgoing message fails DKIM — the exact outage this
--   migration is meant to avoid. So the storage is renamed to `dkim_keys_all`
--   and `mail.dkim_keys` becomes the ACTIVE key per domain: the old query keeps
--   returning exactly one, correct row, unchanged and by construction.
--   Anything that manages keys (the admin handlers) reads/writes
--   `mail.dkim_keys_all`; anything that signs reads `mail.dkim_keys`.

ALTER TABLE mail.dkim_keys
    ADD COLUMN IF NOT EXISTS is_active BOOLEAN NOT NULL DEFAULT TRUE;

-- The pre-existing key of each domain stays the signing key (is_active defaults
-- to TRUE above), but `domain` alone may no longer be unique. The constraint is
-- looked up rather than named: `dkim_keys_domain_key` is what PostgreSQL
-- generates for a column-level UNIQUE, and a deployment where it was named
-- otherwise would silently keep the constraint and reject every rotation.
DO $$
DECLARE constraint_name TEXT;
BEGIN
    SELECT con.conname INTO constraint_name
      FROM pg_constraint con
      JOIN pg_class rel ON rel.oid = con.conrelid
      JOIN pg_namespace nsp ON nsp.oid = rel.relnamespace
     WHERE nsp.nspname = 'mail'
       AND rel.relname = 'dkim_keys'
       AND con.contype  = 'u'
       AND con.conkey   = ARRAY[(
           SELECT attnum FROM pg_attribute
            WHERE attrelid = rel.oid AND attname = 'domain'
       )]::smallint[];
    IF constraint_name IS NOT NULL THEN
        EXECUTE format('ALTER TABLE mail.dkim_keys DROP CONSTRAINT %I', constraint_name);
    END IF;
END $$;

ALTER TABLE mail.dkim_keys RENAME TO dkim_keys_all;

-- A selector identifies the record inside the domain: (domain, selector) is the
-- DNS name, so it is what must be unique.
CREATE UNIQUE INDEX IF NOT EXISTS uq_mail_dkim_domain_selector
    ON mail.dkim_keys_all(domain, selector);

-- At most one key signs for a domain at a time. Enforced here rather than in
-- the handler: two active keys would make the view below return two rows and
-- put the signer back into the ambiguity this migration removes.
CREATE UNIQUE INDEX IF NOT EXISTS uq_mail_dkim_active_per_domain
    ON mail.dkim_keys_all(domain) WHERE is_active;

CREATE OR REPLACE VIEW mail.dkim_keys AS
    SELECT id, domain, selector, algorithm,
           private_key_enc, private_key_nonce, public_key, created_at
      FROM mail.dkim_keys_all
     WHERE is_active;

COMMENT ON VIEW mail.dkim_keys IS
    'Active signing key per domain. Read by the outbound signer; key management goes through mail.dkim_keys_all.';
