-- A recipient who closed a 1:1 DM no longer lists it; a new message or call reopens it.
ALTER TABLE dm_recipients ADD COLUMN closed INTEGER NOT NULL DEFAULT 0;
