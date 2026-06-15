-- Filtres / règles automatiques : appliquent des actions aux messages entrants.
CREATE TABLE IF NOT EXISTS mail.filters (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id          UUID NOT NULL,
    account_id       UUID REFERENCES mail.accounts(id) ON DELETE CASCADE,  -- NULL = tous les comptes
    -- Conditions (toutes celles renseignées doivent matcher ; insensible casse/accents)
    from_contains    TEXT,
    to_contains      TEXT,
    subject_contains TEXT,
    query_contains   TEXT,
    -- Actions
    act_archive      BOOLEAN NOT NULL DEFAULT FALSE,
    act_mark_read    BOOLEAN NOT NULL DEFAULT FALSE,
    act_star         BOOLEAN NOT NULL DEFAULT FALSE,
    act_important    BOOLEAN NOT NULL DEFAULT FALSE,
    act_trash        BOOLEAN NOT NULL DEFAULT FALSE,
    act_spam         BOOLEAN NOT NULL DEFAULT FALSE,
    act_label_id     UUID REFERENCES mail.labels(id) ON DELETE SET NULL,
    position         INTEGER NOT NULL DEFAULT 0,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_mail_filters_user ON mail.filters(user_id);
