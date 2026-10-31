-- Initial schema (PLAN.md section 7.3).
CREATE TABLE users (
  id              INTEGER PRIMARY KEY,
  username        TEXT NOT NULL UNIQUE COLLATE NOCASE,  -- 2..32 chars, [a-z0-9_.]
  display_name    TEXT,                                 -- 1..32 chars
  password_hash   TEXT NOT NULL,                        -- argon2id PHC string
  avatar_id       INTEGER REFERENCES attachments(id),
  status_text     TEXT,                                 -- 0..128 chars
  instance_admin  INTEGER NOT NULL DEFAULT 0,
  disabled        INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL
);

CREATE TABLE sessions (
  id           INTEGER PRIMARY KEY,
  user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  token_hash   BLOB NOT NULL UNIQUE,                    -- SHA-256 of the bearer token
  device_name  TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  last_ip      TEXT
);

CREATE TABLE spaces (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,                            -- 2..64
  description TEXT,                                     -- 0..512
  icon_id     INTEGER REFERENCES attachments(id),
  owner_id    INTEGER NOT NULL REFERENCES users(id),
  created_at  INTEGER NOT NULL
);

CREATE TABLE space_members (
  space_id  INTEGER NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
  user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  nickname  TEXT,
  joined_at INTEGER NOT NULL,
  PRIMARY KEY (space_id, user_id)
);

CREATE TABLE roles (
  id          INTEGER PRIMARY KEY,                      -- @everyone role id == space id
  space_id    INTEGER NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  color       INTEGER,                                  -- 0xRRGGBB or NULL
  hoist       INTEGER NOT NULL DEFAULT 0,               -- show separately in member list
  mentionable INTEGER NOT NULL DEFAULT 0,
  position    INTEGER NOT NULL,                         -- higher = more powerful; @everyone = 0
  permissions INTEGER NOT NULL                          -- u64 bitfield stored as i64
);

CREATE TABLE member_roles (
  space_id INTEGER NOT NULL,
  user_id  INTEGER NOT NULL,
  role_id  INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  PRIMARY KEY (space_id, user_id, role_id),
  FOREIGN KEY (space_id, user_id) REFERENCES space_members(space_id, user_id) ON DELETE CASCADE
);

CREATE TABLE channels (
  id             INTEGER PRIMARY KEY,
  space_id       INTEGER REFERENCES spaces(id) ON DELETE CASCADE,  -- NULL for dm/group_dm
  kind           TEXT NOT NULL CHECK (kind IN ('text','voice','category','dm','group_dm')),
  parent_id      INTEGER REFERENCES channels(id) ON DELETE SET NULL, -- category
  name           TEXT,                                 -- NULL for dm
  topic          TEXT,
  position       INTEGER NOT NULL DEFAULT 0,
  bitrate        INTEGER,                              -- voice only, bits/s
  user_limit     INTEGER,                              -- voice only, 0/NULL = unlimited
  slowmode_secs  INTEGER NOT NULL DEFAULT 0,
  owner_id       INTEGER REFERENCES users(id),         -- group_dm owner
  last_message_id INTEGER,
  created_at     INTEGER NOT NULL
);

CREATE TABLE channel_overrides (
  channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  role_id    INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  allow      INTEGER NOT NULL DEFAULT 0,
  deny       INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (channel_id, role_id)
);

CREATE TABLE dm_recipients (
  channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (channel_id, user_id)
);

CREATE TABLE messages (
  id          INTEGER PRIMARY KEY,
  channel_id  INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  author_id   INTEGER NOT NULL REFERENCES users(id),
  content     TEXT NOT NULL,                           -- 0..4000 chars (raw markup)
  reply_to_id INTEGER,                                 -- not a FK: replied message may be deleted
  edited_at   INTEGER,
  deleted     INTEGER NOT NULL DEFAULT 0,              -- soft delete; content cleared
  pinned      INTEGER NOT NULL DEFAULT 0,
  kind        TEXT NOT NULL DEFAULT 'default'          -- 'default' | 'system' (join, call started, pin, ...)
);
CREATE INDEX messages_channel_id ON messages(channel_id, id);

CREATE TABLE message_mentions (                         -- for mention counts and "mentions" inbox
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL CHECK (kind IN ('user','role','everyone','here')),
  target_id  INTEGER,
  PRIMARY KEY (message_id, kind, target_id)
);

CREATE TABLE attachments (
  id           INTEGER PRIMARY KEY,
  uploader_id  INTEGER NOT NULL REFERENCES users(id),
  message_id   INTEGER REFERENCES messages(id) ON DELETE SET NULL, -- NULL until attached (or avatar/icon)
  filename     TEXT NOT NULL,
  content_type TEXT NOT NULL,
  size         INTEGER NOT NULL,
  width        INTEGER,
  height       INTEGER,
  has_thumb    INTEGER NOT NULL DEFAULT 0,
  sha256       BLOB NOT NULL,
  created_at   INTEGER NOT NULL
);

CREATE TABLE reactions (
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  emoji      TEXT NOT NULL,                            -- Unicode emoji sequence
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (message_id, emoji, user_id)
);

CREATE TABLE read_states (
  user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  channel_id    INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  last_read_id  INTEGER NOT NULL DEFAULT 0,
  mention_count INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (user_id, channel_id)
);

CREATE TABLE invites (
  code        TEXT PRIMARY KEY,                        -- 10 chars, base62
  space_id    INTEGER REFERENCES spaces(id) ON DELETE CASCADE, -- NULL = instance-only invite
  creator_id  INTEGER NOT NULL REFERENCES users(id),
  max_uses    INTEGER,                                 -- NULL = unlimited
  uses        INTEGER NOT NULL DEFAULT 0,
  expires_at  INTEGER,                                 -- NULL = never
  created_at  INTEGER NOT NULL
);

CREATE TABLE bans (
  space_id   INTEGER NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  reason     TEXT,
  banned_by  INTEGER NOT NULL REFERENCES users(id),
  created_at INTEGER NOT NULL,
  PRIMARY KEY (space_id, user_id)
);

CREATE TABLE notification_settings (
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_id  INTEGER NOT NULL,                         -- space id or channel id
  level      TEXT NOT NULL CHECK (level IN ('all','mentions','none')),
  muted_until INTEGER,                                 -- NULL = not muted, -1 = forever
  PRIMARY KEY (user_id, target_id)
);

CREATE TABLE audit_log (
  id         INTEGER PRIMARY KEY,
  space_id   INTEGER NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
  actor_id   INTEGER NOT NULL REFERENCES users(id),
  action     TEXT NOT NULL,                            -- e.g. 'member_kick', 'role_update', 'channel_delete'
  target_id  INTEGER,
  details    TEXT NOT NULL DEFAULT '{}',               -- JSON
  created_at INTEGER NOT NULL
);
CREATE INDEX audit_log_space ON audit_log(space_id, id);
