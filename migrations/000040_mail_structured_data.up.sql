-- schema.org structured data (JSON-LD + ICS invites) extracted from incoming
-- mail at sync time, à la Gmail. Stored verbatim as an array of schema nodes;
-- the frontend maps them to rich cards (events, flights, hotels, orders…).
ALTER TABLE mail.messages ADD COLUMN structured_data JSONB;
COMMENT ON COLUMN mail.messages.structured_data IS
  'schema.org JSON-LD + ICS invite nodes extracted at sync (Gmail-style rich cards).';
