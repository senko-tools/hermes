-- Searching for parts of words ("crypted" finds "encrypted"): a second index of every three
-- letters of each message. Like `messages_fts` it stores only the index; empty (encrypted or
-- deleted) messages are left out.
CREATE VIRTUAL TABLE messages_trigram USING fts5(
  content, content='messages', content_rowid='id', tokenize='trigram'
);

INSERT INTO messages_trigram(rowid, content)
  SELECT id, content FROM messages WHERE content <> '';

DROP TRIGGER messages_fts_insert;
DROP TRIGGER messages_fts_delete;
DROP TRIGGER messages_fts_update;

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages WHEN new.content <> '' BEGIN
  INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
  INSERT INTO messages_trigram(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages WHEN old.content <> '' BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.id, old.content);
  INSERT INTO messages_trigram(messages_trigram, rowid, content)
    VALUES ('delete', old.id, old.content);
END;

-- The old text always leaves both indexes before the new text enters them.
CREATE TRIGGER messages_fts_update AFTER UPDATE OF content ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, content)
    SELECT 'delete', old.id, old.content WHERE old.content <> '';
  INSERT INTO messages_trigram(messages_trigram, rowid, content)
    SELECT 'delete', old.id, old.content WHERE old.content <> '';
  INSERT INTO messages_fts(rowid, content)
    SELECT new.id, new.content WHERE new.content <> '';
  INSERT INTO messages_trigram(rowid, content)
    SELECT new.id, new.content WHERE new.content <> '';
END;
