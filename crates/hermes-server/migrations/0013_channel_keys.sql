-- End-to-end encryption of server channels (docs/e2ee.md). The server stores sealed keys and who
-- may get them; it cannot open any of them.

-- The numbered keys of a channel. A new one starts when someone loses access.
CREATE TABLE channel_key_epochs (
  channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  epoch      INTEGER NOT NULL,
  created_by INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (channel_id, epoch)
) WITHOUT ROWID;

-- One epoch's key, sealed by a device for one device.
CREATE TABLE channel_keys (
  channel_id    INTEGER NOT NULL,
  epoch         INTEGER NOT NULL,
  user_id       INTEGER NOT NULL,
  device_id     TEXT NOT NULL,
  sender_id     INTEGER NOT NULL,
  sender_device TEXT NOT NULL,
  eph           TEXT NOT NULL,
  key           TEXT NOT NULL,
  sig           TEXT NOT NULL,
  PRIMARY KEY (channel_id, epoch, user_id, device_id),
  FOREIGN KEY (channel_id, epoch) REFERENCES channel_key_epochs(channel_id, epoch) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX idx_channel_keys_device ON channel_keys(user_id, device_id);

-- A device that has no keys for a channel yet, until someone hands them over.
CREATE TABLE channel_key_requests (
  channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  device_id  TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (channel_id, user_id, device_id)
) WITHOUT ROWID;

-- Who hands keys over ('automatic' or 'approved'), and whether new members can read the past. A
-- server sets the default; a channel may override either (NULL = the server's).
ALTER TABLE spaces ADD COLUMN key_handover TEXT NOT NULL DEFAULT 'automatic';
ALTER TABLE spaces ADD COLUMN new_members_read_history INTEGER NOT NULL DEFAULT 1;
ALTER TABLE channels ADD COLUMN key_handover TEXT;
ALTER TABLE channels ADD COLUMN new_members_read_history INTEGER;
