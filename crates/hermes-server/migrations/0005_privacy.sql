-- Who may start a DM with a user ("everyone" | "spaces" | "nobody"), and whether DMs from
-- everyone else become message requests instead of being refused.
ALTER TABLE users ADD COLUMN dm_from TEXT NOT NULL DEFAULT 'everyone';
ALTER TABLE users ADD COLUMN message_requests INTEGER NOT NULL DEFAULT 1;

-- The DM waits in this recipient's message requests until they accept it.
ALTER TABLE dm_recipients ADD COLUMN request INTEGER NOT NULL DEFAULT 0;
