-- invites.creator_id becomes nullable: NULL = created by the server itself (the first-start setup
-- invite, `hermes-server invite create`), which happens before any user exists.
-- SQLite cannot change a column's constraints, so the table is rebuilt. Nothing references it.
CREATE TABLE invites_new (
  code        TEXT NOT NULL PRIMARY KEY,               -- 10 chars, base62 (NOT NULL: SQLite allows NULL keys otherwise)
  space_id    INTEGER REFERENCES spaces(id) ON DELETE CASCADE, -- NULL = instance-only invite
  creator_id  INTEGER REFERENCES users(id) ON DELETE SET NULL, -- NULL = created by the server
  max_uses    INTEGER,                                 -- NULL = unlimited
  uses        INTEGER NOT NULL DEFAULT 0,
  expires_at  INTEGER,                                 -- NULL = never
  created_at  INTEGER NOT NULL
);
INSERT INTO invites_new (code, space_id, creator_id, max_uses, uses, expires_at, created_at)
  SELECT code, space_id, creator_id, max_uses, uses, expires_at, created_at FROM invites;
DROP TABLE invites;
ALTER TABLE invites_new RENAME TO invites;
CREATE INDEX invites_space ON invites(space_id);
