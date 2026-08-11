-- The redirect URI sent to the provider at `start` must be reused verbatim
-- for the code exchange at `callback` (providers reject any mismatch), so it
-- is persisted alongside the CSRF state.
ALTER TABLE mail.oauth_states ADD COLUMN IF NOT EXISTS redirect_uri TEXT;
