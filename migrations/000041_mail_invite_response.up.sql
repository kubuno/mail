-- The user's RSVP to a calendar invitation (iMIP), remembered so the card shows
-- the chosen answer on reload, the way Gmail does.
ALTER TABLE mail.messages ADD COLUMN invite_response TEXT;
COMMENT ON COLUMN mail.messages.invite_response IS
  'RSVP to a calendar invite: accepted | tentative | declined (NULL = not answered).';
