//! Permission checks for handlers (PLAN.md section 13: every REST handler checks permissions the
//! same way). Spaces and channels a user cannot see answer `NOT_FOUND`, so nobody can probe which
//! ones exist; missing permissions on visible ones answer `MISSING_PERMISSION`.

use hermes_proto::ErrorCode;
use hermes_proto::id::Id;
use hermes_proto::models::{Channel, Member, Role};
use hermes_proto::permissions::{self, Permissions};

use super::{ApiResult, Error};
use crate::state::{Cache, SpaceState};

/// `MISSING_PERMISSION` naming the missing bits, e.g. "You need MANAGE_ROLES in this space".
pub fn missing(perm: Permissions, scope: &str) -> Error {
    let names: Vec<&str> = perm.iter_names().map(|(name, _)| name).collect();
    Error::new(
        ErrorCode::MissingPermission,
        format!("You need {} in this {scope}", names.join(" and ")),
    )
}

/// The caller's view of a space they are a member of.
#[derive(Debug)]
pub struct SpaceAccess<'a> {
    pub space: &'a SpaceState,
    pub actor: Member,
    /// Space-level permissions (no channel overrides).
    pub perms: Permissions,
}

/// `NOT_FOUND` unless the space exists and `user_id` is a member.
pub fn space(cache: &Cache, space_id: Id, user_id: Id) -> ApiResult<SpaceAccess<'_>> {
    let not_found = || Error::not_found("space");
    let space = cache.spaces.get(&space_id).ok_or_else(not_found)?;
    let actor = cache.member(space_id, user_id).ok_or_else(not_found)?;
    let perms = permissions::compute(&space.space, &space.roles, &actor, None);
    Ok(SpaceAccess {
        space,
        actor,
        perms,
    })
}

/// [`space`] plus [`SpaceAccess::require`].
pub fn require_space(
    cache: &Cache,
    space_id: Id,
    user_id: Id,
    perm: Permissions,
) -> ApiResult<SpaceAccess<'_>> {
    let access = space(cache, space_id, user_id)?;
    access.require(perm)?;
    Ok(access)
}

impl SpaceAccess<'_> {
    pub fn require(&self, perm: Permissions) -> ApiResult<()> {
        if self.perms.contains(perm) {
            Ok(())
        } else {
            Err(missing(perm.difference(self.perms), "space"))
        }
    }

    pub fn is_owner(&self) -> bool {
        self.actor.user.id == self.space.space.owner_id
    }

    /// The position of the caller's highest role (`i32::MAX` for the owner).
    pub fn highest(&self) -> i32 {
        permissions::highest_position(&self.space.space, &self.space.roles, &self.actor)
    }

    /// Hierarchy rule for acting on another member.
    pub fn outranks_member(&self, target: &Member) -> bool {
        permissions::can_manage_member(&self.space.space, &self.space.roles, &self.actor, target)
    }

    /// Hierarchy rule for editing, deleting or assigning a role.
    pub fn outranks_role(&self, role: &Role) -> bool {
        permissions::can_manage_role(&self.space.space, &self.space.roles, &self.actor, role)
    }

    /// Whether the caller may grant (or deny) `bits` in a role or override: only bits they
    /// have themselves. Owners and administrators have every bit.
    pub fn may_grant(&self, bits: Permissions) -> bool {
        self.perms.contains(bits)
    }

    /// `MISSING_PERMISSION` for a hierarchy violation.
    pub fn hierarchy_error(&self, what: &str) -> Error {
        Error::new(
            ErrorCode::MissingPermission,
            format!("You can only manage {what} below your highest role"),
        )
    }
}

/// The caller's view of a channel they can see.
#[derive(Debug)]
pub struct ChannelAccess<'a> {
    pub channel: &'a Channel,
    pub perms: Permissions,
}

/// `NOT_FOUND` unless the channel exists and `user_id` has `VIEW_CHANNEL` in it.
pub fn channel(cache: &Cache, channel_id: Id, user_id: Id) -> ApiResult<ChannelAccess<'_>> {
    let perms = cache.channel_permissions(channel_id, user_id);
    let state = cache
        .channels
        .get(&channel_id)
        .filter(|_| perms.contains(Permissions::VIEW_CHANNEL))
        .ok_or_else(|| Error::not_found("channel"))?;
    Ok(ChannelAccess {
        channel: &state.channel,
        perms,
    })
}

/// [`channel`] plus [`ChannelAccess::require`].
pub fn require_channel(
    cache: &Cache,
    channel_id: Id,
    user_id: Id,
    perm: Permissions,
) -> ApiResult<ChannelAccess<'_>> {
    let access = channel(cache, channel_id, user_id)?;
    access.require(perm)?;
    Ok(access)
}

impl ChannelAccess<'_> {
    pub fn require(&self, perm: Permissions) -> ApiResult<()> {
        if self.perms.contains(perm) {
            Ok(())
        } else {
            Err(missing(perm.difference(self.perms), "channel"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_names_every_bit() {
        let err = missing(
            Permissions::MANAGE_CHANNELS | Permissions::MANAGE_ROLES,
            "channel",
        );
        assert_eq!(err.0.code, ErrorCode::MissingPermission);
        assert_eq!(
            err.0.message,
            "You need MANAGE_ROLES and MANAGE_CHANNELS in this channel"
        );
    }
}
