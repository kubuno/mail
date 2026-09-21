-- Naive Bayes spam classifier, per user.
--
-- The model is a token frequency table: for each token we keep how many spam
-- and ham (non-spam) messages it appeared in. Classification combines the most
-- informative tokens (Paul Graham, "A Plan for Spam"). Everything is scoped by
-- user_id so each account owner gets a personal model.

CREATE TABLE IF NOT EXISTS mail.spam_tokens (
    user_id     UUID    NOT NULL,
    token       TEXT    NOT NULL,
    spam_count  INTEGER NOT NULL DEFAULT 0,   -- spam messages containing the token
    ham_count   INTEGER NOT NULL DEFAULT 0,   -- ham messages containing the token
    PRIMARY KEY (user_id, token)
);

-- Per-user corpus size + auto-classification settings.
CREATE TABLE IF NOT EXISTS mail.spam_stats (
    user_id        UUID PRIMARY KEY,
    spam_messages  INTEGER NOT NULL DEFAULT 0,   -- total spam messages trained
    ham_messages   INTEGER NOT NULL DEFAULT 0,   -- total ham messages trained
    auto_classify  BOOLEAN NOT NULL DEFAULT TRUE, -- move suspected spam automatically
    threshold      REAL    NOT NULL DEFAULT 0.95, -- probability above which we move to spam
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Per-message training guard (NULL = never trained, 0 = trained as ham, 1 = spam).
-- Lets us re-train idempotently when the user corrects a classification.
ALTER TABLE mail.messages ADD COLUMN IF NOT EXISTS spam_trained SMALLINT;

-- Last computed spam probability for an inbox message (NULL = not scored).
-- Surfaced in the UI as a "probably spam" hint below the auto-move threshold.
ALTER TABLE mail.messages ADD COLUMN IF NOT EXISTS spam_score REAL;
