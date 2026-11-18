-- Indexes for lookups and foreign keys that no primary key, UNIQUE constraint or index served,
-- so each of these used to scan the whole table (0014 already covers attachments by message,
-- sessions by user, channels by space, mention lookups by target, avatars and icons). SQLite does
-- not index foreign keys itself; a parent row's delete looks up its children (ON DELETE CASCADE /
-- SET NULL, or the plain FK check). Columns that are only children of `users(id)` stay unindexed:
-- users are disabled, never deleted.

-- The hourly sweep of expired unattached uploads (`message_id IS NULL`, by `created_at`); the
-- partial index from 0014 leaves those rows out.
CREATE INDEX IF NOT EXISTS attachments_unattached ON attachments(created_at) WHERE message_id IS NULL;

-- Mentions inbox (`db::messages::mentioning`): a user's direct mentions, mentions of their roles
-- and @everyone / @here, walked in `message_id` order; 0014's `(target_id, kind)` index neither
-- orders by `message_id` nor covers it.
CREATE INDEX IF NOT EXISTS message_mentions_inbox ON message_mentions(kind, target_id, message_id);

-- Deleting a space: its roles.
CREATE INDEX IF NOT EXISTS roles_space ON roles(space_id);

-- Deleting a category: its children's `parent_id` is set to NULL.
CREATE INDEX IF NOT EXISTS channels_parent ON channels(parent_id) WHERE parent_id IS NOT NULL;

-- Deleting a channel: its read states (the primary key starts with `user_id`).
CREATE INDEX IF NOT EXISTS read_states_channel ON read_states(channel_id);

-- Deleting a role: its member assignments and channel overrides (both primary keys start with
-- another column).
CREATE INDEX IF NOT EXISTS member_roles_role ON member_roles(role_id);
CREATE INDEX IF NOT EXISTS channel_overrides_role ON channel_overrides(role_id);
