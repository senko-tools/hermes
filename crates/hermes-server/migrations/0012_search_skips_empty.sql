-- Encrypted and deleted messages have no text: they take no room in the search index.
DROP TRIGGER messages_fts_insert;
DROP TRIGGER messages_fts_delete;
DROP TRIGGER messages_fts_update;

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages WHEN new.content <> '' BEGIN
  INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages WHEN old.content <> '' BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.id, old.content);
END;

-- One trigger, so the old text always leaves the index before the new text enters it.
CREATE TRIGGER messages_fts_update AFTER UPDATE OF content ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, content)
    SELECT 'delete', old.id, old.content WHERE old.content <> '';
  INSERT INTO messages_fts(rowid, content)
    SELECT new.id, new.content WHERE new.content <> '';
END;
