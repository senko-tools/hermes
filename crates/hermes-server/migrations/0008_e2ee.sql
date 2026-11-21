-- End-to-end encryption (docs/e2ee.md). The server holds public keys and ciphertext, nothing else.
--
-- Rows stay after revocation, so old messages can still be checked against the key that signed
-- them. `session_id` is the session that registered the device: signing it out revokes the device.
CREATE TABLE devices (
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  device_id  TEXT NOT NULL,
  sign_key   TEXT NOT NULL,
  dh_key     TEXT NOT NULL,
  signature  TEXT NOT NULL,
  session_id INTEGER,
  created_at INTEGER NOT NULL,
  revoked_at INTEGER,
  PRIMARY KEY (user_id, device_id)
);
CREATE INDEX idx_devices_session ON devices(session_id);

-- The encrypted form of a message, as JSON; `content` is empty then, so search never sees it.
ALTER TABLE messages ADD COLUMN e2ee TEXT;
