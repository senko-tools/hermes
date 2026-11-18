-- Whether others may see when someone was last online.
ALTER TABLE users ADD COLUMN show_last_online INTEGER NOT NULL DEFAULT 1;

-- Addresses were never used for anything.
UPDATE sessions SET last_ip = NULL;

-- Text removed from the search index is overwritten too.
INSERT INTO messages_fts(messages_fts, rank) VALUES('secure-delete', 1);
