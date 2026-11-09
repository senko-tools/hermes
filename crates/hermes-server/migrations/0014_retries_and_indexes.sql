-- A message sent twice because its reply was lost makes one message.
CREATE TABLE message_nonces (
  channel_id INTEGER NOT NULL,
  author_id  INTEGER NOT NULL,
  nonce      TEXT NOT NULL,
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (channel_id, author_id, nonce)
) WITHOUT ROWID;
CREATE INDEX message_nonces_message ON message_nonces(message_id);
CREATE INDEX message_nonces_created ON message_nonces(created_at);

-- Columns that history, deletes, cascades and cleanup look things up by.
CREATE INDEX attachments_message ON attachments(message_id) WHERE message_id IS NOT NULL;
CREATE INDEX attachments_uploader ON attachments(uploader_id);
CREATE INDEX messages_author ON messages(author_id, id);
CREATE INDEX messages_pinned ON messages(channel_id, id) WHERE pinned = 1;
CREATE INDEX message_mentions_target ON message_mentions(target_id, kind);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE INDEX channels_space ON channels(space_id) WHERE space_id IS NOT NULL;
CREATE INDEX users_avatar ON users(avatar_id) WHERE avatar_id IS NOT NULL;
CREATE INDEX users_banner ON users(banner_id) WHERE banner_id IS NOT NULL;
CREATE INDEX spaces_icon ON spaces(icon_id) WHERE icon_id IS NOT NULL;
