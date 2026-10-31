//! First start (PLAN.md section 9.1): the setup invite and the default space of the first user.

use hermes_proto::id::Id;
use hermes_proto::models::{Channel, ChannelKind, Role, Space};
use hermes_proto::permissions::Permissions;
use sqlx::SqliteConnection;

use crate::db;
use crate::invites::{self, InviteLinks};
use crate::state::{AppState, SpaceState};
use crate::time::{HOUR_MS, now_ms};
use crate::tls::Fingerprint;

/// How long the setup invite stays valid.
pub const SETUP_INVITE_LIFETIME_MS: i64 = 24 * HOUR_MS;

/// If nobody has registered yet, makes sure a usable setup invite exists (reusing the one from a
/// previous start) and prints its link in a framed block. Returns the links, or `None` when the
/// instance already has users.
pub async fn first_start(
    state: &AppState,
    fingerprint: Option<&Fingerprint>,
) -> Result<Option<InviteLinks>, sqlx::Error> {
    if db::auth::count_users(&state.db.read).await? > 0 {
        return Ok(None);
    }
    let now = now_ms();
    let invite = match db::invites::usable_server_instance_invite(&state.db.read, now).await? {
        Some(invite) => invite,
        None => {
            let expires_at = Some(now + SETUP_INVITE_LIFETIME_MS);
            invites::create(&state.db.write, None, None, Some(1), expires_at, now).await?
        }
    };
    let links = invites::links(&state.config, fingerprint, &invite.code);
    print_banner(&links.web);
    Ok(Some(links))
}

/// Printed to stdout (not the log) so it stands out whatever the log format is.
fn print_banner(link: &str) {
    let rule = "=".repeat(66);
    println!(
        "{rule}\n Hermes first-time setup\n Open this link in the Hermes client to create the \
         OWNER account:\n {link}\n (single use, valid for 24 hours; run `hermes-server invite \
         create`\n  for a new one)\n{rule}"
    );
}

/// The first user's space: named after `instance.name`, with `@everyone`, `#general` (text) and
/// `General` (voice).
#[derive(Debug, Clone)]
pub struct DefaultSpace {
    pub space: Space,
    pub everyone: Role,
    pub channels: Vec<Channel>,
}

impl DefaultSpace {
    pub fn new(state: &AppState, owner_id: Id) -> Self {
        let space_id = state.ids.next();
        let space = Space {
            id: space_id,
            name: state.config.instance.name.clone(),
            description: None,
            icon: None,
            owner_id,
        };
        let everyone = Role {
            id: space_id,
            space_id,
            name: "@everyone".into(),
            color: None,
            hoist: false,
            mentionable: false,
            position: 0,
            permissions: Permissions::DEFAULT_EVERYONE,
        };
        let channel = |kind, name: &str, position, bitrate| Channel {
            id: state.ids.next(),
            space_id: Some(space_id),
            kind,
            parent_id: None,
            name: Some(name.to_owned()),
            topic: None,
            position,
            bitrate,
            user_limit: None,
            slowmode_secs: 0,
            overrides: Vec::new(),
            recipients: Vec::new(),
            owner_id: None,
            last_message_id: None,
        };
        let channels = vec![
            channel(ChannelKind::Text, "general", 0, None),
            channel(
                ChannelKind::Voice,
                "General",
                1,
                Some(state.config.voice.default_bitrate),
            ),
        ];
        DefaultSpace {
            space,
            everyone,
            channels,
        }
    }

    /// Writes the space, its role and channels, and the owner's membership.
    pub async fn insert(&self, conn: &mut SqliteConnection, now: i64) -> Result<(), sqlx::Error> {
        db::spaces::insert_space(conn, &self.space, now).await?;
        db::spaces::insert_role(conn, &self.everyone).await?;
        db::spaces::insert_member(conn, self.space.id, self.space.owner_id, now).await?;
        for channel in &self.channels {
            db::spaces::insert_channel(conn, channel, now).await?;
        }
        Ok(())
    }

    /// Adds everything to the cache, after the transaction committed.
    pub fn cache(self, state: &AppState, now: i64) {
        let mut cache = state.cache_mut();
        let (space_id, owner_id) = (self.space.id, self.space.owner_id);
        cache.insert_space(SpaceState::new(self.space, vec![self.everyone]));
        cache.insert_member(space_id, owner_id, now);
        for channel in self.channels {
            cache.insert_channel(channel, Vec::new());
        }
    }
}
