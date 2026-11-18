-- Until when a member may only read (Unix milliseconds).
ALTER TABLE space_members ADD COLUMN timeout_until INTEGER;
