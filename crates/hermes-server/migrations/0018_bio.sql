-- A short "About me" on the profile, shown when someone opens it. Empty: none.
ALTER TABLE users ADD COLUMN bio TEXT NOT NULL DEFAULT '';
