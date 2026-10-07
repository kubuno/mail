-- ── Personal e-mail templates (reusable canned messages) ────────────────────
--
-- Why this exists
--   The "New message" menu offers ready-made drafts: a user saves a subject +
--   body once and inserts it into any compose window later. These lived only in
--   the client before; this table is the server-side, per-user store so the
--   templates follow the user across devices.
--
-- Ownership
--   Every row is scoped to `user_id`; every query filters by it. A user only
--   ever sees and mutates their own templates.

CREATE TABLE IF NOT EXISTS mail.email_templates (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id     UUID         NOT NULL,
    -- Human label shown in the template picker; unique per user (see index).
    name        VARCHAR(255) NOT NULL,
    -- Pre-filled subject; may be empty. 998 = the RFC 5322 line-length ceiling.
    subject     VARCHAR(998) NOT NULL DEFAULT '',
    -- Pre-filled body, stored as the sanitized HTML the composer edits.
    body_html   TEXT         NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- One template per (user, name), case-insensitive on the name. Expressed as a
-- unique index because the key is on an expression (lower(name)).
CREATE UNIQUE INDEX IF NOT EXISTS uq_mail_email_templates_user_name
    ON mail.email_templates (user_id, lower(name));

-- Every listing filters by owner.
CREATE INDEX IF NOT EXISTS idx_mail_email_templates_user
    ON mail.email_templates (user_id);

-- Bump `updated_at` on every change, reusing the schema's shared trigger fn.
DROP TRIGGER IF EXISTS email_templates_updated_at ON mail.email_templates;
CREATE TRIGGER email_templates_updated_at
    BEFORE UPDATE ON mail.email_templates
    FOR EACH ROW EXECUTE FUNCTION mail.set_updated_at();
