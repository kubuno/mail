-- Senders whose remote images the user chose to always display ("Always show
-- images from X"), the counterpart of mail.blocked_senders. An entry is either a
-- full address (info@news.example) or a domain (@news.example), lowercased.
CREATE TABLE IF NOT EXISTS mail.image_allowed_senders (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL,
    email      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, email)
);
CREATE INDEX IF NOT EXISTS idx_mail_image_allowed_user ON mail.image_allowed_senders(user_id);
