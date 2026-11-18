-- A ban that ends by itself.
ALTER TABLE bans ADD COLUMN expires_at INTEGER;

-- Two-factor login: the secret in use, and one waiting to be confirmed.
ALTER TABLE users ADD COLUMN totp_secret TEXT;
ALTER TABLE users ADD COLUMN totp_pending TEXT;

CREATE TABLE user_blocks (
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  blocked_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, blocked_id)
) WITHOUT ROWID;

-- `excerpt` is what the reporter chose to show: the server cannot read encrypted messages.
CREATE TABLE reports (
  id          INTEGER PRIMARY KEY,
  space_id    INTEGER REFERENCES spaces(id) ON DELETE CASCADE,
  channel_id  INTEGER NOT NULL,
  message_id  INTEGER NOT NULL,
  reporter_id INTEGER NOT NULL,
  author_id   INTEGER NOT NULL,
  reason      TEXT NOT NULL,
  excerpt     TEXT NOT NULL,
  created_at  INTEGER NOT NULL
);
CREATE INDEX reports_space ON reports(space_id, id);

CREATE TABLE admin_log (
  id         INTEGER PRIMARY KEY,
  actor_id   INTEGER NOT NULL,
  target_id  INTEGER,
  action     TEXT NOT NULL,
  details    TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

-- Settings an instance admin changed while the server runs; they win over the config file.
CREATE TABLE instance_settings (
  key   TEXT PRIMARY KEY NOT NULL,
  value TEXT NOT NULL
) WITHOUT ROWID;
