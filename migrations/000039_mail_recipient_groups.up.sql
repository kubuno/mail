-- ── Personal recipient groups (private distribution lists) ──────────────────
--
-- Why this exists
--   The "New message" menu lets a user address a whole named group at once
--   (e.g. "Family", "Team"). These are PERSONAL: they are not shared aliases on
--   the server, just a per-user convenience that expands into individual
--   recipients when composing. This table is the per-user, cross-device store.
--
-- Ownership
--   Every row is scoped to `user_id`; every query filters by it. A user only
--   ever sees and mutates their own groups.
--
-- Members shape
--   `members` is a JSON array of objects `{ "email": "a@b.com", "name": "A B" }`
--   where `name` is optional. Emails are validated, normalized (trim +
--   lower-case) and de-duplicated server-side before the row is written, so the
--   stored array never carries an invalid or duplicate address.

CREATE TABLE IF NOT EXISTS mail.recipient_groups (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id     UUID         NOT NULL,
    -- Human label shown in the group picker; unique per user (see index).
    name        VARCHAR(255) NOT NULL,
    -- JSON array of { email, name? }. Defaults to the empty list.
    members     JSONB        NOT NULL DEFAULT '[]',
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- One group per (user, name), case-insensitive on the name. Expressed as a
-- unique index because the key is on an expression (lower(name)).
CREATE UNIQUE INDEX IF NOT EXISTS uq_mail_recipient_groups_user_name
    ON mail.recipient_groups (user_id, lower(name));

-- Every listing filters by owner.
CREATE INDEX IF NOT EXISTS idx_mail_recipient_groups_user
    ON mail.recipient_groups (user_id);

-- Bump `updated_at` on every change, reusing the schema's shared trigger fn.
DROP TRIGGER IF EXISTS recipient_groups_updated_at ON mail.recipient_groups;
CREATE TRIGGER recipient_groups_updated_at
    BEFORE UPDATE ON mail.recipient_groups
    FOR EACH ROW EXECUTE FUNCTION mail.set_updated_at();
