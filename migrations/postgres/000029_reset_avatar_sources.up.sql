-- One-off reset of the sender-avatar cache.
--
-- Rows written before the authorisation gate existed all carry source = 'bimi',
-- because the INSERT hard-coded it. That mislabels neutral marks (a mailbox
-- provider's icon, a favicon) as brand logos, so they would now be withheld on
-- unauthenticated mail for no reason. Emptying the cache is enough: every entry
-- is re-resolved on demand, this time with its real source.
DELETE FROM mail.sender_avatars;
