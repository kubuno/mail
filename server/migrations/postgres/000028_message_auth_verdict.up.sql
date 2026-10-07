-- Authentication verdict of an inbound message (DMARC/SPF/DKIM), kept so the
-- reader can decide whether a BRAND LOGO may be shown next to the sender.
--
-- Showing a brand's logo on a message that failed authentication would lend
-- visual credibility to phishing — the very thing the logo is supposed to rule
-- out. BIMI requires DMARC to pass; without this column the client cannot tell.
-- NULL = unknown (synced or legacy message): treated as "not authenticated".
ALTER TABLE mail.messages
    ADD COLUMN IF NOT EXISTS auth_dmarc VARCHAR(8);
