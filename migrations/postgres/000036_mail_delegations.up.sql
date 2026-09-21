-- ── Account delegation (Gmail-style "grant access to your account") ──────────
--
-- Why this exists
--   A user (the GRANTOR) can let another user of this instance (the DELEGATE)
--   read their mailbox and send messages on their behalf, WITHOUT sharing a
--   password. This is the server-side authority behind the settings UI, which
--   previously stored delegation preferences in localStorage only (nothing was
--   ever enforced). It mirrors Gmail's flow: the grantor invites, the delegate
--   accepts, and either side (grantor by revoking, delegate by declining) can
--   end it at any time.
--
-- Enforcement
--   Access is granted ONLY by a row whose `status = 'accepted'`. A 'pending'
--   invitation and a 'revoked' delegation confer no access whatsoever. The
--   check lives in one place server-side (services::delegation::resolve_acting_user)
--   and is reused by every delegated read/send route.
--
-- Identities
--   `grantor_user_id` / `delegate_user_id` are core account ids (the module never
--   reads `core.users`; the delegate is resolved through the core's internal
--   directory at grant time). The two email columns are cached copies for display
--   and for building the `Sender:` header of a delegated send, so the common path
--   needs no round-trip to the core.

CREATE TABLE IF NOT EXISTS mail.delegations (
    id                UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- The account being delegated (the mailbox owner).
    grantor_user_id   UUID         NOT NULL,
    -- Cached address of the grantor (the `From:` of a delegated send) and shown
    -- to the delegate so they know which mailbox they were granted.
    grantor_email     VARCHAR(320) NOT NULL DEFAULT '',
    -- The account granted access.
    delegate_user_id  UUID         NOT NULL,
    -- Cached address of the delegate (resolved from the core directory at grant
    -- time), shown to the grantor and used as the `Sender:` of a delegated send.
    delegate_email    VARCHAR(320) NOT NULL,
    -- 'pending'  : invited, not yet accepted — NO access.
    -- 'accepted' : active — the delegate may read and (if can_send) send.
    -- 'revoked'  : ended by either party — NO access.
    status            VARCHAR(20)  NOT NULL DEFAULT 'pending'
                          CHECK (status IN ('pending', 'accepted', 'revoked')),
    -- Whether the delegate may SEND on the grantor's behalf (reading is always
    -- allowed on an accepted delegation). Default on, like Gmail.
    can_send          BOOLEAN      NOT NULL DEFAULT TRUE,
    created_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    -- When the delegate accepted (NULL until then / after a revoke resets it).
    accepted_at       TIMESTAMPTZ,
    updated_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- One delegation per (grantor, delegate). A revoked row is reused (set back to
-- 'pending') when the grantor re-invites the same person, so this stays unique.
CREATE UNIQUE INDEX IF NOT EXISTS uq_mail_delegations_pair
    ON mail.delegations (grantor_user_id, delegate_user_id);

-- "Delegations I granted" (grantor view) and "accounts I can access" (delegate
-- view) each hit one of these.
CREATE INDEX IF NOT EXISTS idx_mail_delegations_grantor
    ON mail.delegations (grantor_user_id);
CREATE INDEX IF NOT EXISTS idx_mail_delegations_delegate
    ON mail.delegations (delegate_user_id);

-- Bump `updated_at` on every change, reusing the schema's shared trigger fn.
DROP TRIGGER IF EXISTS delegations_updated_at ON mail.delegations;
CREATE TRIGGER delegations_updated_at
    BEFORE UPDATE ON mail.delegations
    FOR EACH ROW EXECUTE FUNCTION mail.set_updated_at();
