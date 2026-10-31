-- Full-text search over message content (PLAN.md section 7.3).
-- External-content FTS5 table: it stores only the index, the text stays in `messages`.
CREATE VIRTUAL TABLE messages_fts USING fts5(content, content='messages', content_rowid='id');

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.id, old.content);
END;

-- Soft deletes clear `content`, so they go through this trigger too.
CREATE TRIGGER messages_fts_update AFTER UPDATE OF content ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.id, old.content);
  INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;
