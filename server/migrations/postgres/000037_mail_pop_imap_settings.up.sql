-- ── POP / IMAP access policy (Gmail's "Forwarding and POP/IMAP") ────────────
--
-- Why this exists
--   The settings tab already offered POP and IMAP controls — enable/disable each
--   protocol, what to do with a message once a POP client has fetched it, how an
--   IMAP deletion is committed — but they lived entirely in localStorage. This
--   instance IS the IMAP/POP3 server (see `src/server/`), yet nothing it served
--   ever consulted those preferences. This table is the server-side half: one
--   row per user, read at login and honoured by the protocol handlers.
--
-- ⚠️ Defaults preserve the CURRENT behaviour, not Gmail's out-of-the-box one
--   The live instance already serves IMAP and POP3 to any mailbox credential
--   with no per-user gate, so a missing row (every existing user) MUST keep that
--   exact behaviour or active mail clients break at the next poll. Concretely:
--     * `imap_enabled  DEFAULT TRUE`  — IMAP has always been open.
--     * `pop_enabled   DEFAULT TRUE`  — POP has always been open. Gmail ships POP
--        OFF, but this instance already answers POP logins, so defaulting to off
--        would silently cut off every current POP client. Compatibility wins; the
--        UI lets a user turn POP off explicitly.
--     * `pop_mode      DEFAULT 'all'` — the maildrop has always shown every inbox
--        message.
--     * `pop_post_action DEFAULT 'mark_read'` — RETR has always marked the
--        fetched message read; the post-action reproduces that, now configurable.
--     * `imap_expunge_mode DEFAULT 'wait'` (and `imap_auto_expunge FALSE`) — the
--        server has always followed standard IMAP: a `\Deleted` flag waits for an
--        explicit EXPUNGE. Gmail's default is 'auto', but changing the live
--        server's expunge timing under a running client is exactly what the
--        compatibility rule forbids, so the stored default stays 'wait'.
--     * `imap_purge_mode DEFAULT 'trash'` — EXPUNGE has always moved the message
--        to Trash (`store::set_deleted`).
--     * `imap_folder_limit DEFAULT 0` — no per-folder cap has ever been applied.
--
--   `imap_auto_expunge` and `imap_expunge_mode` encode the same decision (the
--   bool is `mode = 'auto'`); both are stored and kept consistent by the writer,
--   the mode being canonical.

CREATE TABLE IF NOT EXISTS mail.pop_imap_settings (
    user_id           UUID        PRIMARY KEY,

    -- ── IMAP ────────────────────────────────────────────────────────────────
    -- Whether this mailbox may be reached over IMAP at all. FALSE makes the
    -- server refuse the session after a valid authentication (Gmail behaviour).
    imap_enabled      BOOLEAN     NOT NULL DEFAULT TRUE,
    -- How a `\Deleted` flag is committed: 'wait' = standard IMAP (commit on the
    -- client's EXPUNGE); 'auto' = commit immediately on STORE (Gmail's
    -- auto-expunge). Kept in step with `imap_auto_expunge`.
    imap_expunge_mode TEXT        NOT NULL DEFAULT 'wait'
                          CHECK (imap_expunge_mode IN ('auto', 'wait')),
    imap_auto_expunge BOOLEAN     NOT NULL DEFAULT FALSE,
    -- Where a message goes when it is expunged from its last visible folder.
    imap_purge_mode   TEXT        NOT NULL DEFAULT 'trash'
                          CHECK (imap_purge_mode IN ('archive', 'trash', 'delete')),
    -- Maximum messages exposed per IMAP folder; 0 = unlimited. When capped, the
    -- most recent N (highest UIDs) are served.
    imap_folder_limit INTEGER     NOT NULL DEFAULT 0 CHECK (imap_folder_limit >= 0),

    -- ── POP3 ────────────────────────────────────────────────────────────────
    -- Whether this mailbox may be relieved over POP3. FALSE makes the server
    -- refuse the POP login (-ERR) even with a valid credential.
    pop_enabled       BOOLEAN     NOT NULL DEFAULT TRUE,
    -- 'all' = every inbox message is in the maildrop; 'from_now' = only messages
    -- whose `local_uid` is greater than `pop_from_uid` (a cursor snapped to the
    -- current top of the inbox the moment the user chose "from now on").
    pop_mode          TEXT        NOT NULL DEFAULT 'all'
                          CHECK (pop_mode IN ('all', 'from_now')),
    pop_from_uid      BIGINT      NOT NULL DEFAULT 0,
    -- What happens to a message once a POP session has RETRieved it and QUIT:
    -- keep it untouched, mark it read, archive it (out of the inbox), or delete
    -- it (to Trash).
    pop_post_action   TEXT        NOT NULL DEFAULT 'mark_read'
                          CHECK (pop_post_action IN ('keep', 'mark_read', 'archive', 'delete')),

    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
