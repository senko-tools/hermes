-- STREAM (bit 38, screen sharing) is part of @everyone's defaults; give it to existing spaces'
-- @everyone roles too (the @everyone role's id is the space's id).
UPDATE roles SET permissions = permissions | 274877906944 WHERE id = space_id;
