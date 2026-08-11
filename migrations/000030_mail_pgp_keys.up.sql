-- OpenPGP / GPG support (server-side half of the hybrid design).
--
-- Two tables, mirroring the credential pattern of mail.accounts: the private key
-- is stored AES-256-GCM encrypted at rest (BYTEA + separate nonce), exactly like
-- imap_password / smtp_password. Public material is armored text.

-- The user's own OpenPGP identities (one row per keypair).
CREATE TABLE IF NOT EXISTS mail.pgp_keys (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id         UUID NOT NULL,
    -- Address this identity signs/encrypts for (primary User ID address). A user
    -- may hold several identities for several of their addresses.
    email           VARCHAR(500),
    -- Uppercase hex fingerprint, no spaces (the stable identifier).
    fingerprint     VARCHAR(64) NOT NULL,
    -- Armored public certificate (shareable) …
    public_key      TEXT NOT NULL,
    -- … and the armored secret key, AES-256-GCM encrypted at rest.
    private_key     BYTEA NOT NULL,
    private_key_nonce BYTEA NOT NULL,
    -- Default identity used when several exist for the same address.
    is_default      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, fingerprint)
);

CREATE INDEX IF NOT EXISTS idx_mail_pgp_keys_user  ON mail.pgp_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_mail_pgp_keys_email ON mail.pgp_keys(user_id, lower(email));

DROP TRIGGER IF EXISTS pgp_keys_updated_at ON mail.pgp_keys;
CREATE TRIGGER pgp_keys_updated_at
    BEFORE UPDATE ON mail.pgp_keys
    FOR EACH ROW EXECUTE FUNCTION mail.set_updated_at();

-- Public keys collected for correspondents (manual import, WKD, Autocrypt header,
-- or a key attached to a received message). All public — no encryption at rest.
CREATE TABLE IF NOT EXISTS mail.pgp_contacts (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id         UUID NOT NULL,
    email           VARCHAR(500) NOT NULL,
    fingerprint     VARCHAR(64) NOT NULL,
    public_key      TEXT NOT NULL,
    -- How we learned this key (drives trust/UX and refresh policy).
    source          VARCHAR(16) NOT NULL DEFAULT 'manual'
                        CHECK (source IN ('manual', 'wkd', 'autocrypt', 'attached')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- One stored key per correspondent address (a rotated key replaces it). Expressed
-- as a unique index because the key is on an expression (lower(email)).
CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_pgp_contacts_lookup ON mail.pgp_contacts(user_id, lower(email));

DROP TRIGGER IF EXISTS pgp_contacts_updated_at ON mail.pgp_contacts;
CREATE TRIGGER pgp_contacts_updated_at
    BEFORE UPDATE ON mail.pgp_contacts
    FOR EACH ROW EXECUTE FUNCTION mail.set_updated_at();
