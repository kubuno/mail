-- Autoriser le dossier « archive » (hors boîte, conservé dans « Tous les messages »).
ALTER TABLE mail.messages DROP CONSTRAINT IF EXISTS messages_folder_check;
ALTER TABLE mail.messages ADD CONSTRAINT messages_folder_check
  CHECK (folder IN ('inbox','sent','drafts','spam','trash','custom','archive'));
