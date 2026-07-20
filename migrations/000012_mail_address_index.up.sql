-- Address index for recipient autocompletion: one row per (user, email) with a
-- usage counter and the best known display name. Kept up to date incrementally
-- (sync + send) so suggestions never need to scan mail.messages.
CREATE TABLE IF NOT EXISTS mail.address_index (
    user_id      UUID NOT NULL,
    email        TEXT NOT NULL,
    name         TEXT,
    use_count    INTEGER NOT NULL DEFAULT 1,
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, email)
);

-- Prefix search support (email LIKE 'term%').
CREATE INDEX IF NOT EXISTS idx_mail_addr_prefix ON mail.address_index (user_id, email text_pattern_ops);

-- Backfill from existing mail: senders…
INSERT INTO mail.address_index (user_id, email, name, use_count, last_used_at)
SELECT user_id,
       LOWER(from_email),
       MAX(NULLIF(from_name, '')),
       COUNT(*),
       MAX(received_at)
FROM mail.messages
WHERE from_email LIKE '%@%'
GROUP BY user_id, LOWER(from_email)
ON CONFLICT (user_id, email) DO NOTHING;

-- …and recipients (To + Cc, JSONB arrays of {email, name}).
INSERT INTO mail.address_index (user_id, email, name, use_count, last_used_at)
SELECT m.user_id,
       LOWER(a->>'email'),
       MAX(NULLIF(a->>'name', '')),
       COUNT(*),
       MAX(m.received_at)
FROM mail.messages m,
     LATERAL jsonb_array_elements(
       COALESCE(m.to_addresses, '[]'::jsonb) || COALESCE(m.cc_addresses, '[]'::jsonb)
     ) AS a
WHERE a->>'email' LIKE '%@%'
GROUP BY m.user_id, LOWER(a->>'email')
ON CONFLICT (user_id, email) DO UPDATE SET
    use_count = mail.address_index.use_count + EXCLUDED.use_count,
    name      = COALESCE(mail.address_index.name, EXCLUDED.name);
