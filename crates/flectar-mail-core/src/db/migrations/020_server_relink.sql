-- When an account moves to another IMAP server or protocol, a mailbox is
-- recreated, or a folder disappears from the server, the mail that came from
-- a server loses its UIDs. Each message waits here to be matched by
-- Message-ID with its copy on the server. Once every folder has its history,
-- those still waiting are no longer on the server.
ALTER TABLE messages ADD COLUMN relink_pending INTEGER NOT NULL DEFAULT 0
  CHECK (relink_pending IN (0, 1));
CREATE INDEX idx_messages_relink_pending ON messages(account_id)
  WHERE relink_pending = 1;

-- IMAP mail that already has no UID, such as what an earlier switch from
-- JMAP left, waits the same way. The account's own drafts and the sent mail
-- only this device holds keep their place.
UPDATE messages SET relink_pending = 1
WHERE uid IS NULL
  AND account_id IN (SELECT id FROM accounts WHERE mail_protocol = 'imap')
  AND (jmap_id IS NOT NULL OR is_outgoing = 0)
  AND NOT EXISTS (SELECT 1 FROM drafts_meta d WHERE d.message_id = messages.id);

-- A folder the server no longer lists. It is not synced, and goes once the
-- mail in it has been matched elsewhere or removed.
ALTER TABLE folders ADD COLUMN missing_on_server INTEGER NOT NULL DEFAULT 0
  CHECK (missing_on_server IN (0, 1));
