ALTER TABLE mail.messages DROP COLUMN IF EXISTS spam_score;
ALTER TABLE mail.messages DROP COLUMN IF EXISTS spam_trained;
DROP TABLE IF EXISTS mail.spam_stats;
DROP TABLE IF EXISTS mail.spam_tokens;
