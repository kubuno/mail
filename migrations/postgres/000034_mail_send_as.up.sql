-- ── "Send mail as" addresses (ownership-verified sender identities) ─────────
--
-- Why this exists
--   The settings UI let a user list arbitrary "send as" addresses, but they
--   lived in localStorage: nothing proved the user actually controlled the
--   address, so the instance could have been turned into an open spoofer. This
--   table is the server-side half, modelled on Gmail's flow: adding an address
--   sends a confirmation code TO that address, and the address becomes usable as
--   a sender only once the code is entered back.
--
-- Ownership
--   Every row is scoped to `user_id`; every query filters by it. A user only
--   ever sees and mutates their own identities.
--
-- The verification code is a SECRET at rest
--   `verification_code` is never returned by any read endpoint (the list route
--   selects the public columns only). It is cleared the moment the address is
--   verified, so a verified row carries no live code.

CREATE TABLE IF NOT EXISTS mail.send_as_addresses (
    id                       UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id                  UUID         NOT NULL,
    -- The address the user wants to send mail as.
    email                    VARCHAR(320) NOT NULL,
    -- Display name shown to recipients; may be empty.
    display_name             VARCHAR(255) NOT NULL DEFAULT '',
    -- Only a verified address may actually be used as a sender.
    verified                 BOOLEAN      NOT NULL DEFAULT FALSE,
    -- The pending confirmation code (NULL once verified or never issued). Secret:
    -- never surfaced by any endpoint.
    verification_code        VARCHAR(16),
    -- When the pending code stops being accepted.
    verification_expires_at  TIMESTAMPTZ,
    -- Treat the identity as an alias (reply from it) rather than a distinct
    -- account. Mirrors Gmail's "treat as an alias" checkbox; default on.
    treat_as_alias           BOOLEAN      NOT NULL DEFAULT TRUE,
    created_at               TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- One identity per (user, address), case-insensitive on the address. Expressed
-- as a unique index because the key is on an expression (lower(email)).
CREATE UNIQUE INDEX IF NOT EXISTS uq_mail_send_as_user_email
    ON mail.send_as_addresses (user_id, lower(email));

-- Every listing filters by owner.
CREATE INDEX IF NOT EXISTS idx_mail_send_as_user
    ON mail.send_as_addresses (user_id);

-- `updated_at` is the resend anti-abuse clock: it is bumped on every UPDATE, so
-- the last code regeneration is exactly `updated_at`.
DROP TRIGGER IF EXISTS send_as_updated_at ON mail.send_as_addresses;
CREATE TRIGGER send_as_updated_at
    BEFORE UPDATE ON mail.send_as_addresses
    FOR EACH ROW EXECUTE FUNCTION mail.set_updated_at();
