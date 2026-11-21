-- The wide picture at the top of a user's profile, like the avatar an image they uploaded.
ALTER TABLE users ADD COLUMN banner_id INTEGER REFERENCES attachments(id);
