-- Tags like name#1234, and the friend system (docs/DECISIONS.md, 2026-11-07).
--
-- `users.username` now holds the whole handle: `name#1234`, or `name` alone for the accounts the
-- instance's owner lets go without a tag. The column stays UNIQUE COLLATE NOCASE, which makes the
-- pair unique, and needs no rebuild of a table that half the schema points at. '#' is not allowed
-- in names, so the handle always splits back. Names were unique until now, so any tag is free.
UPDATE users
SET username = username || '#' || printf('%04d', abs(random()) % 9999 + 1)
WHERE instance_admin = 0;

-- One row per pair of users. `accepted_at` NULL: a request from `requester_id` to the other.
CREATE TABLE relationships (
  user_low     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  user_high    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  requester_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at   INTEGER NOT NULL,
  accepted_at  INTEGER,
  PRIMARY KEY (user_low, user_high),
  CHECK (user_low < user_high)
);
CREATE INDEX idx_relationships_high ON relationships(user_high);
