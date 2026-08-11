DROP TABLE IF EXISTS mail.oauth_states;

ALTER TABLE mail.accounts
    DROP COLUMN IF EXISTS oauth_expires_at,
    DROP COLUMN IF EXISTS oauth_access_nonce,
    DROP COLUMN IF EXISTS oauth_access_token,
    DROP COLUMN IF EXISTS oauth_refresh_nonce,
    DROP COLUMN IF EXISTS oauth_refresh_token,
    DROP COLUMN IF EXISTS auth_kind;
