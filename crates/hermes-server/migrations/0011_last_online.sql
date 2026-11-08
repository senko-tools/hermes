-- When someone was last online, for their profile (gateway/last_online.rs). Empty until first known.
ALTER TABLE users ADD COLUMN last_online_at INTEGER;
