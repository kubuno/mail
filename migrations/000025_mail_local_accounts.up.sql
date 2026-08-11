-- ── Local accounts: a hosted mailbox, as the mail client sees it ─────────────
--
-- Why this exists
--   Creating a mailbox in the admin panel (`mail.mailboxes`) said "this address
--   belongs to this user" — but the mail CLIENT only ever knew `mail.accounts`,
--   the EXTERNAL accounts a user configures against Gmail and the like. So an
--   assigned mailbox appeared nowhere: no inbox, no "From" identity, no line in
--   the settings, and — worse — a message to it was filed into the owner's
--   default external account, or refused outright when they had none.
--
--   The fix is to let a hosted mailbox BE an account of its owner. A local
--   account is an ordinary `mail.accounts` row with `kind = 'local'`: it appears
--   in the client's account list and its "From" selector exactly like any other,
--   its mail is filed into it, and the user needs no IMAP/SMTP configuration
--   because the instance itself is the server.
--
-- Why `kind` and a sentinel, not nullable columns
--   `mail.accounts` has eight NOT NULL external-transport columns
--   (imap_host, …, smtp_password), read as a non-optional struct on every hot
--   path. Making them nullable would ripple through every reader. A local
--   account instead carries harmless sentinels — empty host, `security = 'none'`,
--   an encrypted empty password — and is told apart by `kind`. The transport
--   columns are simply never used for it: the client hides them, the sync worker
--   skips it, and sending goes through the instance's own outbound queue.
--
-- Lifecycle (enforced by the address handlers, not by SQL)
--   * create a mailbox → its local account is created in the same transaction;
--   * rename / disable a mailbox → the account's name / is_active follow;
--   * delete a mailbox → `mailbox_id` becomes NULL (ON DELETE SET NULL) and the
--     handler deactivates the account. The messages already filed into it are
--     KEPT — `mail.messages` cascades from the ACCOUNT, not from the mailbox, so
--     detaching the mailbox leaves them untouched. Deleting the address must not
--     shred the mail that arrived at it.

ALTER TABLE mail.accounts
    ADD COLUMN IF NOT EXISTS kind VARCHAR(10) NOT NULL DEFAULT 'external'
        CHECK (kind IN ('external', 'local')),
    -- The mailbox this account hosts, when it is local. SET NULL on delete so a
    -- removed address leaves its mail behind rather than cascading it away.
    ADD COLUMN IF NOT EXISTS mailbox_id UUID REFERENCES mail.mailboxes(id) ON DELETE SET NULL;

-- One account per mailbox: a mailbox is one person's address, and two accounts
-- claiming it would make delivery pick one by row order.
CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_accounts_mailbox
    ON mail.accounts(mailbox_id) WHERE mailbox_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_mail_accounts_kind
    ON mail.accounts(user_id, kind);
