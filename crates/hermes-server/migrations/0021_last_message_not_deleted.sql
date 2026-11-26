-- A channel's `last_message_id` is its newest message that is not deleted (#86). Deleting a
-- message used to leave it pointing at the deleted one, so the channel stayed unread with nothing
-- in it; since then every deletion recomputes it. This fixes the channels left that way.
UPDATE channels SET last_message_id =
  (SELECT MAX(m.id) FROM messages m WHERE m.channel_id = channels.id AND m.deleted = 0)
WHERE last_message_id IS NOT NULL
  AND NOT EXISTS (
    SELECT 1 FROM messages m WHERE m.id = channels.last_message_id AND m.deleted = 0
  );
