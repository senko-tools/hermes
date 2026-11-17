-- Indexes for lookups and foreign keys that no primary key, UNIQUE constraint or index served,
-- so each of these used to scan the whole table. SQLite does not index foreign keys itself; a
-- parent row's delete looks up its children (ON DELETE CASCADE / SET NULL, or the plain FK check).
-- Columns that are only children of `users(id)` stay unindexed: users are disabled, never deleted.

-- Message attachments by message (`messages::attachments`, `count_attachments`, unlinking on a
-- soft delete, the SET NULL when a channel's messages go) and, with `message_id IS NULL`,
-- the hourly sweep of expired unattached uploads by `created_at`.
CREATE INDEX idx_attachments_message ON attachments(message_id, created_at);

-- A user's sessions (`GET /auth/sessions`; revoking them on a password change or when an
-- account is disabled).
CREATE INDEX idx_sessions_user ON sessions(user_id);

-- Mentions inbox: a user's direct mentions, mentions of their roles and @everyone / @here,
-- newest first; `message_id` makes it a covering index.
CREATE INDEX idx_message_mentions_target ON message_mentions(kind, target_id, message_id);

-- Deleting a space: its channels and roles (also the space's channels in the ban purge).
CREATE INDEX idx_channels_space ON channels(space_id);
CREATE INDEX idx_roles_space ON roles(space_id);

-- Deleting a category: its children's `parent_id` is set to NULL.
CREATE INDEX idx_channels_parent ON channels(parent_id);

-- Deleting a channel: its read states (the primary key starts with `user_id`).
CREATE INDEX idx_read_states_channel ON read_states(channel_id);

-- Deleting a role: its member assignments and channel overrides (both primary keys start with
-- another column).
CREATE INDEX idx_member_roles_role ON member_roles(role_id);
CREATE INDEX idx_channel_overrides_role ON channel_overrides(role_id);

-- Whether an upload is still an avatar or a space icon (the sweep above, and the FK check when an
-- upload is deleted).
CREATE INDEX idx_users_avatar ON users(avatar_id);
CREATE INDEX idx_spaces_icon ON spaces(icon_id);
