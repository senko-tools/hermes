//! Permission bits and the permission algorithm (PLAN.md section 8).
//!
//! On the wire `Permissions` is a decimal string (like IDs), so that bits above 2^53 survive
//! JavaScript clients. Unknown bits from a newer peer are dropped when decoding.

use std::fmt;

use bitflags::bitflags;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::id::Id;
use crate::models::{Channel, ChannelKind, Member, Role, Space};

bitflags! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
    pub struct Permissions: u64 {
        /// All permissions, bypasses overrides.
        const ADMINISTRATOR = 1 << 0;
        /// Edit name, icon, description.
        const MANAGE_SPACE = 1 << 1;
        /// Create/edit/delete roles below own highest role.
        const MANAGE_ROLES = 1 << 2;
        /// Create/edit/delete/reorder channels and overrides.
        const MANAGE_CHANNELS = 1 << 3;
        const KICK_MEMBERS = 1 << 4;
        const BAN_MEMBERS = 1 << 5;
        const CREATE_INVITES = 1 << 6;
        /// View/delete others' invites.
        const MANAGE_INVITES = 1 << 7;
        /// Own nickname.
        const CHANGE_NICKNAME = 1 << 8;
        /// Others' nicknames.
        const MANAGE_NICKNAMES = 1 << 9;
        const VIEW_AUDIT_LOG = 1 << 10;

        /// See the channel and (for voice) who is in it.
        const VIEW_CHANNEL = 1 << 16;
        const SEND_MESSAGES = 1 << 17;
        /// Read messages sent before you opened the channel.
        const READ_HISTORY = 1 << 18;
        const ATTACH_FILES = 1 << 19;
        /// Add *new* reaction emoji (joining an existing one only needs VIEW_CHANNEL).
        const ADD_REACTIONS = 1 << 20;
        /// `@everyone`, `@here`, non-mentionable roles.
        const MENTION_EVERYONE = 1 << 21;
        /// Delete others' messages, pin.
        const MANAGE_MESSAGES = 1 << 22;
        const BYPASS_SLOWMODE = 1 << 23;

        /// Join voice.
        const CONNECT = 1 << 32;
        /// Send audio.
        const SPEAK = 1 << 33;
        /// Without it, push-to-talk is required.
        const USE_VOICE_ACTIVITY = 1 << 34;
        /// Server mute.
        const MUTE_MEMBERS = 1 << 35;
        /// Server deafen.
        const DEAFEN_MEMBERS = 1 << 36;
        /// Move/disconnect users in voice.
        const MOVE_MEMBERS = 1 << 37;
    }
}

impl Permissions {
    /// Default permissions of a new space's `@everyone` role.
    pub const DEFAULT_EVERYONE: Permissions = Permissions::CHANGE_NICKNAME
        .union(Permissions::CREATE_INVITES)
        .union(Permissions::VIEW_CHANNEL)
        .union(Permissions::SEND_MESSAGES)
        .union(Permissions::READ_HISTORY)
        .union(Permissions::ATTACH_FILES)
        .union(Permissions::ADD_REACTIONS)
        .union(Permissions::CONNECT)
        .union(Permissions::SPEAK)
        .union(Permissions::USE_VOICE_ACTIVITY);

    /// What every recipient of a DM or group DM has.
    pub const DM_RECIPIENT: Permissions = Permissions::VIEW_CHANNEL
        .union(Permissions::SEND_MESSAGES)
        .union(Permissions::READ_HISTORY)
        .union(Permissions::ATTACH_FILES)
        .union(Permissions::ADD_REACTIONS)
        .union(Permissions::CONNECT)
        .union(Permissions::SPEAK)
        .union(Permissions::USE_VOICE_ACTIVITY);

    /// Applies an override: `(self & !deny) | allow`.
    pub const fn apply(self, allow: Permissions, deny: Permissions) -> Permissions {
        self.difference(deny).union(allow)
    }
}

impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.bits())
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PermissionsVisitor;

        impl Visitor<'_> for PermissionsVisitor {
            type Value = Permissions;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a permission bitfield as a decimal string")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Permissions, E> {
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(E::invalid_value(de::Unexpected::Str(v), &self));
                }
                v.parse()
                    .map(Permissions::from_bits_truncate)
                    .map_err(|_| E::invalid_value(de::Unexpected::Str(v), &self))
            }
        }

        deserializer.deserialize_str(PermissionsVisitor)
    }
}

fn find_role(roles: &[Role], id: Id) -> Option<&Role> {
    roles.iter().find(|r| r.id == id)
}

/// The member's explicitly assigned roles that exist in `roles` (never `@everyone`).
fn member_roles<'a>(
    space: &'a Space,
    roles: &'a [Role],
    member: &'a Member,
) -> impl Iterator<Item = &'a Role> + 'a {
    member
        .role_ids
        .iter()
        .filter(move |&&id| id != space.id)
        .filter_map(move |&id| find_role(roles, id))
}

/// Computes a member's permissions in a space, or in one of its channels (PLAN.md section 8.2).
///
/// `roles` are the space's roles; the `@everyone` role is the one whose ID equals the space ID.
/// `channel` must belong to `space`. DMs do not use this; see [`Permissions::DM_RECIPIENT`].
pub fn compute(
    space: &Space,
    roles: &[Role],
    member: &Member,
    channel: Option<&Channel>,
) -> Permissions {
    if member.user.id == space.owner_id {
        return Permissions::all();
    }

    let mut base = find_role(roles, space.id)
        .map(|r| r.permissions)
        .unwrap_or_default();
    for role in member_roles(space, roles, member) {
        base |= role.permissions;
    }
    if base.contains(Permissions::ADMINISTRATOR) {
        return Permissions::all();
    }

    let Some(channel) = channel else {
        return base;
    };

    let mut p = base;
    // 1) @everyone override
    if let Some(o) = channel.overrides.iter().find(|o| o.role_id == space.id) {
        p = p.apply(o.allow, o.deny);
    }
    // 2) all member role overrides combined
    let mut allow = Permissions::empty();
    let mut deny = Permissions::empty();
    for role_id in member.role_ids.iter().filter(|&&id| id != space.id) {
        if let Some(o) = channel.overrides.iter().find(|o| o.role_id == *role_id) {
            allow |= o.allow;
            deny |= o.deny;
        }
    }
    p = p.apply(allow, deny);
    // 3) implicit rules
    if !p.contains(Permissions::VIEW_CHANNEL) {
        return Permissions::empty();
    }
    if channel.kind == ChannelKind::Voice && !p.contains(Permissions::CONNECT) {
        p.remove(Permissions::SPEAK | Permissions::USE_VOICE_ACTIVITY);
    }
    p
}

/// The position of the member's highest role (`@everyone` = 0). The owner ranks above everything.
pub fn highest_position(space: &Space, roles: &[Role], member: &Member) -> i32 {
    if member.user.id == space.owner_id {
        return i32::MAX;
    }
    member_roles(space, roles, member)
        .map(|r| r.position)
        .fold(0, i32::max)
}

/// Hierarchy rule: `actor` may kick/ban/edit `target` only if its highest role is strictly higher.
/// The owner may act on everyone else; nobody may act on the owner.
pub fn can_manage_member(space: &Space, roles: &[Role], actor: &Member, target: &Member) -> bool {
    if target.user.id == space.owner_id {
        return false;
    }
    highest_position(space, roles, actor) > highest_position(space, roles, target)
}

/// Hierarchy rule for roles: `actor` may edit, delete or assign `role` only if its highest role is
/// strictly higher than the role (owner exempt).
pub fn can_manage_role(space: &Space, roles: &[Role], actor: &Member, role: &Role) -> bool {
    highest_position(space, roles, actor) > role.position
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Override;
    use crate::models::tests::user;
    use serde_json::json;

    const SPACE: Id = Id(100);
    const OWNER: u64 = 1;
    const MOD: Id = Id(201);
    const MEMBER_ROLE: Id = Id(202);
    const ADMIN: Id = Id(203);

    type P = Permissions;

    fn space() -> Space {
        Space {
            id: SPACE,
            name: "space".into(),
            description: None,
            icon: None,
            owner_id: Id(OWNER),
        }
    }

    fn role(id: Id, position: i32, permissions: P) -> Role {
        Role {
            id,
            space_id: SPACE,
            name: format!("role{}", id.0),
            color: None,
            hoist: false,
            mentionable: false,
            position,
            permissions,
        }
    }

    fn roles() -> Vec<Role> {
        vec![
            role(SPACE, 0, P::DEFAULT_EVERYONE),
            role(MOD, 2, P::KICK_MEMBERS | P::MANAGE_MESSAGES),
            role(MEMBER_ROLE, 1, P::MENTION_EVERYONE),
            role(ADMIN, 3, P::ADMINISTRATOR),
        ]
    }

    fn member(user_id: u64, role_ids: &[Id]) -> Member {
        Member {
            space_id: SPACE,
            user: user(user_id),
            nickname: None,
            role_ids: role_ids.to_vec(),
            joined_at: 0,
        }
    }

    fn channel(kind: ChannelKind, overrides: &[(Id, P, P)]) -> Channel {
        Channel {
            id: Id(300),
            space_id: Some(SPACE),
            kind,
            parent_id: None,
            name: Some("general".into()),
            topic: None,
            position: 0,
            bitrate: None,
            user_limit: None,
            slowmode_secs: 0,
            overrides: overrides
                .iter()
                .map(|&(role_id, allow, deny)| Override {
                    role_id,
                    allow,
                    deny,
                })
                .collect(),
            recipients: vec![],
            owner_id: None,
            last_message_id: None,
        }
    }

    fn text(overrides: &[(Id, P, P)]) -> Channel {
        channel(ChannelKind::Text, overrides)
    }

    #[test]
    fn bit_positions_match_plan() {
        let table: &[(P, u32)] = &[
            (P::ADMINISTRATOR, 0),
            (P::MANAGE_SPACE, 1),
            (P::MANAGE_ROLES, 2),
            (P::MANAGE_CHANNELS, 3),
            (P::KICK_MEMBERS, 4),
            (P::BAN_MEMBERS, 5),
            (P::CREATE_INVITES, 6),
            (P::MANAGE_INVITES, 7),
            (P::CHANGE_NICKNAME, 8),
            (P::MANAGE_NICKNAMES, 9),
            (P::VIEW_AUDIT_LOG, 10),
            (P::VIEW_CHANNEL, 16),
            (P::SEND_MESSAGES, 17),
            (P::READ_HISTORY, 18),
            (P::ATTACH_FILES, 19),
            (P::ADD_REACTIONS, 20),
            (P::MENTION_EVERYONE, 21),
            (P::MANAGE_MESSAGES, 22),
            (P::BYPASS_SLOWMODE, 23),
            (P::CONNECT, 32),
            (P::SPEAK, 33),
            (P::USE_VOICE_ACTIVITY, 34),
            (P::MUTE_MEMBERS, 35),
            (P::DEAFEN_MEMBERS, 36),
            (P::MOVE_MEMBERS, 37),
        ];
        for &(flag, bit) in table {
            assert_eq!(flag.bits(), 1 << bit, "{flag:?}");
        }
        assert_eq!(P::all().iter().count(), table.len());
    }

    #[test]
    fn default_everyone_and_dm_sets() {
        assert_eq!(
            P::DEFAULT_EVERYONE,
            P::CHANGE_NICKNAME
                | P::CREATE_INVITES
                | P::VIEW_CHANNEL
                | P::SEND_MESSAGES
                | P::READ_HISTORY
                | P::ATTACH_FILES
                | P::ADD_REACTIONS
                | P::CONNECT
                | P::SPEAK
                | P::USE_VOICE_ACTIVITY
        );
        assert_eq!(
            P::DM_RECIPIENT,
            P::DEFAULT_EVERYONE - P::CHANGE_NICKNAME - P::CREATE_INVITES
        );
    }

    #[test]
    fn json_is_a_decimal_string() {
        let p = P::VIEW_CHANNEL | P::MOVE_MEMBERS;
        let value = serde_json::to_value(p).unwrap();
        assert_eq!(value, json!(((1u64 << 16) | (1 << 37)).to_string()));
        assert_eq!(serde_json::from_value::<P>(value).unwrap(), p);
        assert_eq!(serde_json::to_value(P::empty()).unwrap(), json!("0"));
    }

    #[test]
    fn json_drops_unknown_bits_and_rejects_garbage() {
        let raw = (1u64 << 63) | (1 << 11) | 1;
        let p: P = serde_json::from_value(json!(raw.to_string())).unwrap();
        assert_eq!(p, P::ADMINISTRATOR);
        for bad in [json!(5), json!(""), json!("-1"), json!("1.0"), json!(null)] {
            assert!(serde_json::from_value::<P>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn owner_gets_everything_even_when_denied() {
        let c = text(&[(SPACE, P::empty(), P::all())]);
        let owner = member(OWNER, &[]);
        assert_eq!(compute(&space(), &roles(), &owner, None), P::all());
        assert_eq!(compute(&space(), &roles(), &owner, Some(&c)), P::all());
    }

    #[test]
    fn administrator_bypasses_overrides() {
        let c = text(&[(SPACE, P::empty(), P::all()), (ADMIN, P::empty(), P::all())]);
        let m = member(2, &[ADMIN]);
        assert_eq!(compute(&space(), &roles(), &m, Some(&c)), P::all());
    }

    #[test]
    fn administrator_on_everyone_gives_everyone_all() {
        let mut rs = roles();
        rs[0].permissions |= P::ADMINISTRATOR;
        assert_eq!(compute(&space(), &rs, &member(2, &[]), None), P::all());
    }

    #[test]
    fn base_is_union_of_roles() {
        let m = member(2, &[MOD, MEMBER_ROLE]);
        assert_eq!(
            compute(&space(), &roles(), &m, None),
            P::DEFAULT_EVERYONE | P::KICK_MEMBERS | P::MANAGE_MESSAGES | P::MENTION_EVERYONE
        );
    }

    #[test]
    fn unknown_and_duplicate_everyone_role_ids_are_ignored() {
        let m = member(2, &[Id(999), SPACE]);
        assert_eq!(compute(&space(), &roles(), &m, None), P::DEFAULT_EVERYONE);
    }

    #[test]
    fn missing_everyone_role_means_nothing() {
        let rs: Vec<Role> = roles().into_iter().filter(|r| !r.is_everyone()).collect();
        assert_eq!(compute(&space(), &rs, &member(2, &[]), None), P::empty());
    }

    #[test]
    fn no_overrides_keeps_base() {
        let m = member(2, &[MOD]);
        assert_eq!(
            compute(&space(), &roles(), &m, Some(&text(&[]))),
            compute(&space(), &roles(), &m, None)
        );
    }

    #[test]
    fn everyone_override_allows_and_denies() {
        let c = text(&[(SPACE, P::MANAGE_MESSAGES, P::SEND_MESSAGES)]);
        let p = compute(&space(), &roles(), &member(2, &[]), Some(&c));
        assert!(p.contains(P::MANAGE_MESSAGES));
        assert!(!p.contains(P::SEND_MESSAGES));
        assert!(p.contains(P::VIEW_CHANNEL));
    }

    #[test]
    fn role_override_beats_everyone_override() {
        // @everyone denies SEND_MESSAGES, the member role allows it again.
        let c = text(&[
            (SPACE, P::empty(), P::SEND_MESSAGES),
            (MEMBER_ROLE, P::SEND_MESSAGES, P::empty()),
        ]);
        let with_role = compute(&space(), &roles(), &member(2, &[MEMBER_ROLE]), Some(&c));
        let without = compute(&space(), &roles(), &member(3, &[]), Some(&c));
        assert!(with_role.contains(P::SEND_MESSAGES));
        assert!(!without.contains(P::SEND_MESSAGES));
    }

    #[test]
    fn role_override_deny_beats_everyone_allow() {
        let c = text(&[
            (SPACE, P::ATTACH_FILES, P::empty()),
            (MOD, P::empty(), P::ATTACH_FILES),
        ]);
        let p = compute(&space(), &roles(), &member(2, &[MOD]), Some(&c));
        assert!(!p.contains(P::ATTACH_FILES));
    }

    #[test]
    fn allow_wins_over_deny_between_member_roles() {
        // Role overrides are combined first, then applied as (p & !deny) | allow.
        let c = text(&[
            (MOD, P::empty(), P::SEND_MESSAGES),
            (MEMBER_ROLE, P::SEND_MESSAGES, P::empty()),
        ]);
        let p = compute(
            &space(),
            &roles(),
            &member(2, &[MOD, MEMBER_ROLE]),
            Some(&c),
        );
        assert!(p.contains(P::SEND_MESSAGES));
        // Same role allowing and denying a bit: allow wins too.
        let c = text(&[(MOD, P::SEND_MESSAGES, P::SEND_MESSAGES)]);
        let p = compute(&space(), &roles(), &member(2, &[MOD]), Some(&c));
        assert!(p.contains(P::SEND_MESSAGES));
    }

    #[test]
    fn overrides_of_roles_the_member_lacks_are_ignored() {
        let c = text(&[(MOD, P::empty(), P::all())]);
        let p = compute(&space(), &roles(), &member(2, &[MEMBER_ROLE]), Some(&c));
        assert!(p.contains(P::SEND_MESSAGES));
    }

    #[test]
    fn hidden_channel_gives_nothing() {
        let c = text(&[(SPACE, P::empty(), P::VIEW_CHANNEL)]);
        let m = member(2, &[MOD]);
        assert_eq!(compute(&space(), &roles(), &m, Some(&c)), P::empty());
        // ...unless a role override lets the member see it.
        let c = text(&[
            (SPACE, P::empty(), P::VIEW_CHANNEL),
            (MOD, P::VIEW_CHANNEL, P::empty()),
        ]);
        let p = compute(&space(), &roles(), &m, Some(&c));
        assert!(p.contains(P::VIEW_CHANNEL | P::SEND_MESSAGES | P::KICK_MEMBERS));
    }

    #[test]
    fn voice_without_connect_cannot_speak() {
        let deny_connect = [(SPACE, P::empty(), P::CONNECT)];
        let m = member(2, &[]);

        let voice = channel(ChannelKind::Voice, &deny_connect);
        let p = compute(&space(), &roles(), &m, Some(&voice));
        assert!(!p.intersects(P::CONNECT | P::SPEAK | P::USE_VOICE_ACTIVITY));
        assert!(p.contains(P::VIEW_CHANNEL | P::SEND_MESSAGES));

        // The rule only applies to voice channels.
        let p = compute(&space(), &roles(), &m, Some(&text(&deny_connect)));
        assert!(p.contains(P::SPEAK | P::USE_VOICE_ACTIVITY));

        let voice = channel(ChannelKind::Voice, &[]);
        let p = compute(&space(), &roles(), &m, Some(&voice));
        assert!(p.contains(P::CONNECT | P::SPEAK | P::USE_VOICE_ACTIVITY));
    }

    #[test]
    fn every_bit_through_every_override_layer() {
        for bit in P::all().iter() {
            if bit == P::ADMINISTRATOR {
                continue;
            }
            let m = member(2, &[MEMBER_ROLE]);
            // Base without the bit; VIEW_CHANNEL kept so the channel stays visible.
            let mut rs = roles();
            rs[0].permissions = P::VIEW_CHANNEL;
            rs[2].permissions = P::empty();
            let view = P::VIEW_CHANNEL;

            let c = text(&[(SPACE, bit, P::empty())]);
            assert!(
                compute(&space(), &rs, &m, Some(&c)).contains(bit),
                "{bit:?}"
            );
            let c = text(&[(MEMBER_ROLE, bit, P::empty())]);
            assert!(
                compute(&space(), &rs, &m, Some(&c)).contains(bit),
                "{bit:?}"
            );

            rs[0].permissions = P::all() - P::ADMINISTRATOR;
            if bit != view {
                let c = text(&[(SPACE, P::empty(), bit)]);
                let p = compute(&space(), &rs, &m, Some(&c));
                assert!(!p.contains(bit) && p.contains(view), "{bit:?}");
                let c = text(&[(MEMBER_ROLE, P::empty(), bit)]);
                let p = compute(&space(), &rs, &m, Some(&c));
                assert!(!p.contains(bit) && p.contains(view), "{bit:?}");
            }
        }
    }

    #[test]
    fn hierarchy() {
        let s = space();
        let rs = roles();
        let owner = member(OWNER, &[]);
        let admin = member(2, &[ADMIN]);
        let moderator = member(3, &[MOD, MEMBER_ROLE]);
        let plain = member(4, &[MEMBER_ROLE]);
        let nobody = member(5, &[]);
        let other_mod = member(6, &[MOD]);

        assert_eq!(highest_position(&s, &rs, &owner), i32::MAX);
        assert_eq!(highest_position(&s, &rs, &moderator), 2);
        assert_eq!(highest_position(&s, &rs, &nobody), 0);

        assert!(can_manage_member(&s, &rs, &owner, &admin));
        assert!(can_manage_member(&s, &rs, &admin, &moderator));
        assert!(can_manage_member(&s, &rs, &moderator, &plain));
        assert!(can_manage_member(&s, &rs, &plain, &nobody));
        // Strictly lower only.
        assert!(!can_manage_member(&s, &rs, &moderator, &other_mod));
        assert!(!can_manage_member(&s, &rs, &nobody, &nobody));
        assert!(!can_manage_member(&s, &rs, &plain, &moderator));
        // Nobody acts on the owner.
        assert!(!can_manage_member(&s, &rs, &admin, &owner));
        assert!(!can_manage_member(&s, &rs, &owner, &owner));

        let mod_role = &rs[1];
        assert!(can_manage_role(&s, &rs, &owner, mod_role));
        assert!(can_manage_role(&s, &rs, &admin, mod_role));
        assert!(!can_manage_role(&s, &rs, &moderator, mod_role));
        assert!(can_manage_role(&s, &rs, &moderator, &rs[2]));
        assert!(!can_manage_role(&s, &rs, &nobody, &rs[0]));
    }
}
