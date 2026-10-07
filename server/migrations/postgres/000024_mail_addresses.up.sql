-- ── Local addresses: mailboxes, aliases and distribution lists ───────────────
--
-- Why this exists
--   Until now this instance had no notion of an address it OWNS. A recipient was
--   resolved by guessing: first `mailbox_credentials.username` (a login, which
--   happens to look like an address), then `accounts.email_address` (the user's
--   account at ANOTHER provider, which we merely poll). Both are side effects of
--   features built for something else, and neither can express what running a
--   mail domain actually requires — an alias, a catch-all, a distribution list,
--   a quota, or simply an address that exists while its owner is away.
--
--   So the address becomes a first-class object. `mail.mailboxes` is the
--   authoritative list of what this instance accepts and files locally; the two
--   older paths stay as a fallback so nothing that works today stops working.
--
-- Resolution order at RCPT (see `server/deliver.rs`)
--   1. mailbox (exact address)          → file into that account
--   2. alias   (exact address)          → expand to its destinations
--   3. list    (exact address)          → expand to its members
--   4. catch-all alias (`@domain`)      → expand
--   5. legacy: mailbox_credentials, then accounts
--   6. otherwise: 550, no such user here
--
--   Expansion is RECURSIVE and therefore bounded — see the depth and fan-out
--   limits in the resolver. An alias pointing at itself, or two aliases pointing
--   at each other, is a configuration mistake somebody WILL make; it must cost a
--   rejected recipient, never a loop that takes the server down.

-- ── Mailboxes ────────────────────────────────────────────────────────────────
-- One local address, filed into one Kubuno account. The account is the owner;
-- deleting it in the core does not cascade here (the module owns no foreign key
-- into `core`), so a mailbox whose owner is gone is reported by the panel rather
-- than silently swallowing mail.
CREATE TABLE IF NOT EXISTS mail.mailboxes (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Always stored lowercased: addresses are compared case-insensitively on the
    -- domain, and in practice on the local part too. Doing it at write time is
    -- what lets every lookup be a plain equality on an indexed column.
    address       VARCHAR(320) NOT NULL,
    domain        VARCHAR(255) NOT NULL,
    user_id       UUID NOT NULL,
    display_name  VARCHAR(255),
    -- 0 = no limit. Enforced at delivery, and reported by the panel.
    quota_bytes   BIGINT NOT NULL DEFAULT 0 CHECK (quota_bytes >= 0),
    -- An inactive mailbox still EXISTS: mail to it is refused with a permanent
    -- "mailbox disabled" rather than "no such user", and its owner keeps what was
    -- already delivered. Deleting is the destructive operation; this is not.
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    comment       TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (address)
);

CREATE INDEX IF NOT EXISTS idx_mail_mailboxes_user   ON mail.mailboxes(user_id);
CREATE INDEX IF NOT EXISTS idx_mail_mailboxes_domain ON mail.mailboxes(domain);

-- ── Aliases ──────────────────────────────────────────────────────────────────
-- An address that is not a place but a redirection. `address` is either a whole
-- address (`contact@example.com`) or `@example.com` — the catch-all, which
-- matches anything in that domain that nothing else matched.
--
-- Destinations may be local or remote. A remote destination only leaves the
-- instance if outbound delivery is enabled; that is deliberate — an alias
-- forwarding outside is an outgoing message, and it obeys the same switch.
CREATE TABLE IF NOT EXISTS mail.aliases (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    address       VARCHAR(320) NOT NULL,
    domain        VARCHAR(255) NOT NULL,
    -- One or more destinations. An empty array is refused: an alias that expands
    -- to nothing is a black hole, and a black hole must be spelled out, which is
    -- what `is_active = false` is for.
    destinations  TEXT[] NOT NULL CHECK (cardinality(destinations) > 0),
    -- True when `address` is `@domain`. Stored rather than derived so the
    -- resolver can order its lookups without parsing.
    is_catch_all  BOOLEAN NOT NULL DEFAULT FALSE,
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    comment       TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (address)
);

CREATE INDEX IF NOT EXISTS idx_mail_aliases_domain ON mail.aliases(domain);
-- At most one catch-all per domain: two would make delivery depend on row order.
CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_aliases_catch_all
    ON mail.aliases(domain) WHERE is_catch_all;

-- ── Distribution lists ───────────────────────────────────────────────────────
-- An address that expands to a membership. Separate from an alias because the
-- question "who is allowed to post here" only exists for a list, and answering
-- it wrong is how an internal list becomes a spam relay.
CREATE TABLE IF NOT EXISTS mail.mailing_lists (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    address       VARCHAR(320) NOT NULL,
    domain        VARCHAR(255) NOT NULL,
    name          VARCHAR(255) NOT NULL,
    -- Who may send TO the list:
    --   'anyone'   — open (a public contact list; expect abuse)
    --   'members'  — only an address that is a member
    --   'internal' — only a sender in one of the instance's local domains
    --   'allowed'  — only the addresses in `allowed_senders`
    post_policy   VARCHAR(20) NOT NULL DEFAULT 'internal'
                      CHECK (post_policy IN ('anyone', 'members', 'internal', 'allowed')),
    allowed_senders TEXT[] NOT NULL DEFAULT '{}',
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    comment       TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (address)
);

CREATE INDEX IF NOT EXISTS idx_mail_mailing_lists_domain ON mail.mailing_lists(domain);

CREATE TABLE IF NOT EXISTS mail.mailing_list_members (
    list_id    UUID NOT NULL REFERENCES mail.mailing_lists(id) ON DELETE CASCADE,
    address    VARCHAR(320) NOT NULL,
    added_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (list_id, address)
);

-- ── Per-domain policy ────────────────────────────────────────────────────────
-- NOT the list of local domains — that stays the `server_domains` setting, which
-- every code path already reads. Two competing answers to "is this domain ours"
-- is exactly the kind of split that delivers mail nowhere. This table only holds
-- what a domain may additionally SAY about itself, and a row for a domain that
-- is no longer served is inert (the panel shows it as such).
CREATE TABLE IF NOT EXISTS mail.domain_policies (
    domain              VARCHAR(255) PRIMARY KEY,
    -- Applied to a mailbox created without an explicit quota. 0 = no limit.
    default_quota_bytes BIGINT NOT NULL DEFAULT 0 CHECK (default_quota_bytes >= 0),
    -- 0 = unlimited. Refused at creation time, never enforced retroactively.
    max_mailboxes       INTEGER NOT NULL DEFAULT 0 CHECK (max_mailboxes >= 0),
    comment             TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Keeping `updated_at` honest ──────────────────────────────────────────────
CREATE OR REPLACE FUNCTION mail.touch_updated_at() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS mailboxes_touch        ON mail.mailboxes;
DROP TRIGGER IF EXISTS aliases_touch          ON mail.aliases;
DROP TRIGGER IF EXISTS mailing_lists_touch    ON mail.mailing_lists;
DROP TRIGGER IF EXISTS domain_policies_touch  ON mail.domain_policies;

CREATE TRIGGER mailboxes_touch       BEFORE UPDATE ON mail.mailboxes
    FOR EACH ROW EXECUTE FUNCTION mail.touch_updated_at();
CREATE TRIGGER aliases_touch         BEFORE UPDATE ON mail.aliases
    FOR EACH ROW EXECUTE FUNCTION mail.touch_updated_at();
CREATE TRIGGER mailing_lists_touch   BEFORE UPDATE ON mail.mailing_lists
    FOR EACH ROW EXECUTE FUNCTION mail.touch_updated_at();
CREATE TRIGGER domain_policies_touch BEFORE UPDATE ON mail.domain_policies
    FOR EACH ROW EXECUTE FUNCTION mail.touch_updated_at();

-- An address may be a mailbox, an alias OR a list — never two at once, or
-- delivery would depend on which table the resolver happened to read first.
-- PostgreSQL has no cross-table unique constraint, so this is enforced by the
-- handlers AND checked here, cheaply, whenever a row is written.
CREATE OR REPLACE FUNCTION mail.assert_address_is_free() RETURNS TRIGGER AS $$
DECLARE
    taken TEXT;
BEGIN
    SELECT 'boîte' INTO taken FROM mail.mailboxes
        WHERE address = NEW.address AND TG_TABLE_NAME <> 'mailboxes' LIMIT 1;
    IF taken IS NULL THEN
        SELECT 'alias' INTO taken FROM mail.aliases
            WHERE address = NEW.address AND TG_TABLE_NAME <> 'aliases' LIMIT 1;
    END IF;
    IF taken IS NULL THEN
        SELECT 'liste' INTO taken FROM mail.mailing_lists
            WHERE address = NEW.address AND TG_TABLE_NAME <> 'mailing_lists' LIMIT 1;
    END IF;
    IF taken IS NOT NULL THEN
        RAISE EXCEPTION 'adresse % déjà utilisée par un objet de type %', NEW.address, taken
            USING ERRCODE = 'unique_violation';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS mailboxes_address_free     ON mail.mailboxes;
DROP TRIGGER IF EXISTS aliases_address_free       ON mail.aliases;
DROP TRIGGER IF EXISTS mailing_lists_address_free ON mail.mailing_lists;

CREATE TRIGGER mailboxes_address_free     BEFORE INSERT OR UPDATE OF address ON mail.mailboxes
    FOR EACH ROW EXECUTE FUNCTION mail.assert_address_is_free();
CREATE TRIGGER aliases_address_free       BEFORE INSERT OR UPDATE OF address ON mail.aliases
    FOR EACH ROW EXECUTE FUNCTION mail.assert_address_is_free();
CREATE TRIGGER mailing_lists_address_free BEFORE INSERT OR UPDATE OF address ON mail.mailing_lists
    FOR EACH ROW EXECUTE FUNCTION mail.assert_address_is_free();
