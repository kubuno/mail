-- Greylisting (RFC 6647 §2.2) for the reception listener.
--
-- The whole idea in one line: a real MTA has a queue and retries, a spam engine
-- usually does not. So the FIRST time a (client, sender, recipient) triplet is
-- seen we answer 450 and remember it; the retry that arrives a few minutes
-- later is delivered, and the triplet is then trusted for as long as it keeps
-- being used. It costs the legitimate sender one delay, once.
--
-- The triplet is the unit deliberately: keying on the client alone would let a
-- spammer warm up one address and then blast every mailbox, and keying on the
-- sender alone would be forged away in seconds.

CREATE TABLE IF NOT EXISTS mail.greylist (
    -- Normalised client address: IPv4 truncated to its /24, IPv6 to its /64.
    -- Large senders retry from a DIFFERENT host of the same farm, so keying on
    -- the exact address would make the retry look like a new triplet and defer
    -- the message forever. This is postgrey's `--lookup-by-subnet` default.
    client_net TEXT        NOT NULL,
    -- Envelope sender, lowercased. The empty string is the null return path
    -- <> that every DSN uses.
    sender     TEXT        NOT NULL,
    -- Envelope recipient, lowercased.
    recipient  TEXT        NOT NULL,
    -- When the triplet was first refused. The retry delay is measured from here.
    first_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Last time the triplet was looked up, whatever the answer. Drives the
    -- purge: a triplet nobody uses any more is not worth a row.
    last_seen  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Set the moment the triplet passed. NULL = still on probation. Once set,
    -- every later message of that triplet is delivered without delay.
    passed_at  TIMESTAMPTZ,
    PRIMARY KEY (client_net, sender, recipient)
);

-- The lookup at RCPT time is on the whole triplet, which the primary key
-- already serves. This second index is for the purge, which scans by age only:
-- without it, expiring an old row means a sequential scan of the table on a
-- listener that is answering a live SMTP session.
CREATE INDEX IF NOT EXISTS mail_greylist_last_seen_idx ON mail.greylist (last_seen);

-- Drops every triplet that has fallen out of its window.
--
-- Two lifetimes, because the two kinds of row mean opposite things:
--   * a row still on probation (`passed_at IS NULL`) is only useful for the
--     length of the retry window — past it the sender clearly never retried,
--     and a fresh attempt should start its probation over;
--   * a row that PASSED is a trust decision, kept as long as it is being used
--     (`last_seen`), so a correspondent who writes every week is never delayed
--     twice. postgrey keeps these for 35 days; the caller decides.
--
-- Returns the number of rows removed, so the caller can log it.
CREATE OR REPLACE FUNCTION mail.purge_greylist(
    p_probation INTERVAL,
    p_trust     INTERVAL
) RETURNS INTEGER AS $$
DECLARE
    removed INTEGER;
BEGIN
    DELETE FROM mail.greylist
     WHERE (passed_at IS NULL     AND first_seen < NOW() - p_probation)
        OR (passed_at IS NOT NULL AND last_seen  < NOW() - p_trust);
    GET DIAGNOSTICS removed = ROW_COUNT;
    RETURN removed;
END;
$$ LANGUAGE plpgsql;
