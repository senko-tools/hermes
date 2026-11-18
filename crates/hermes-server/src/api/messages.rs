//! `/channels/{id}/messages`, pins, reactions, typing and ack, and their gateway events.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::api::{
    AckRequest, BulkDeleteRequest, EditMessageRequest, HistoryQuery, SendMessageRequest,
};
use hermes_proto::e2ee::{Envelope, MAX_ENVELOPE_BYTES};
use hermes_proto::gateway::{ReactionEvent, ServerEvent};
use hermes_proto::id::Id;
use hermes_proto::models::{AuditAction, ChannelKind, Mentions, Message, MessageKind, ReadState};
use hermes_proto::permissions::Permissions;
use hermes_proto::{ApiError, ErrorCode};
use serde_json::json;
use sqlx::{Sqlite, SqliteExecutor, Transaction};

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, Query, audit, perms};
use crate::auth::Auth;
use crate::db;
use crate::db::messages::MessageRow;
use crate::gateway::bus::Audience;
use crate::messages::{self, MAX_REACTION_EMOJI, broadcast_copy};
use crate::ratelimit::{self, Action};
use crate::state::{AppState, Cache};
use crate::time::now_ms;

/// What the handlers need to know about a message channel and the caller's permissions in it.
#[derive(Debug, Clone, Copy)]
struct Target {
    perms: Permissions,
    space_id: Option<Id>,
    kind: ChannelKind,
    slowmode_secs: u32,
}

impl Target {
    fn require(&self, perm: Permissions) -> ApiResult<()> {
        if self.perms.contains(perm) {
            Ok(())
        } else {
            Err(perms::missing(perm.difference(self.perms), "channel"))
        }
    }
}

/// A channel with messages where the caller has `VIEW_CHANNEL` (else `NOT_FOUND`) and `perm`.
fn target(cache: &Cache, channel_id: Id, user_id: Id, perm: Permissions) -> ApiResult<Target> {
    let access = perms::require_channel(cache, channel_id, user_id, perm)?;
    let channel = access.channel;
    if !channel.kind.has_messages() {
        return Err(Error::new(
            ErrorCode::BadRequest,
            "Categories have no messages",
        ));
    }
    Ok(Target {
        perms: access.perms,
        space_id: channel.space_id,
        kind: channel.kind,
        slowmode_secs: channel.slowmode_secs,
    })
}

/// A message of the channel that is not deleted; `NOT_FOUND` otherwise. Handlers that change the
/// message check it with their write transaction, so a concurrent delete cannot slip in between
/// (the write pool has a single connection): otherwise an edit could restore a deleted message's
/// content, or a pin or reaction land on its tombstone.
async fn live_message(
    e: impl SqliteExecutor<'_>,
    channel_id: Id,
    message_id: Id,
) -> ApiResult<MessageRow> {
    db::messages::get(e, channel_id, message_id)
        .await?
        .filter(|m| !m.deleted)
        .ok_or_else(|| Error::not_found("message"))
}

/// A write transaction for a handler that reads the message before it writes. It takes the
/// database's write lock at once (`BEGIN IMMEDIATE`): a deferred one would fail with
/// `SQLITE_BUSY_SNAPSHOT`, which the busy timeout does not retry, if another process (the
/// `hermes-server user` commands) wrote between the read and the write.
async fn checking_tx(state: &AppState) -> ApiResult<Transaction<'static, Sqlite>> {
    Ok(state.db.write.begin_with("BEGIN IMMEDIATE").await?)
}

async fn one_to_wire(state: &AppState, row: MessageRow, viewer: Id) -> ApiResult<Message> {
    messages::to_wire(state, vec![row], viewer)
        .await?
        .pop()
        .ok_or_else(|| Error::internal("message disappeared while loading"))
}

/// `VIEW_CHANNEL` + `READ_HISTORY`. Oldest first; deleted messages are included as tombstones.
pub async fn history(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<Message>>> {
    target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL | Permissions::READ_HISTORY,
    )?;
    let anchors = [q.before, q.after, q.around];
    if anchors.iter().flatten().count() > 1 {
        return Err(Error::new(
            ErrorCode::BadRequest,
            "Use only one of before, after and around",
        ));
    }
    let db = &state.db.read;
    let limit = q.limit();
    let rows = match (q.after, q.around) {
        (Some(after), _) => db::messages::after(db, channel_id, after, limit).await?,
        (_, Some(around)) => {
            let mut rows = db::messages::before(db, channel_id, Some(around), limit / 2).await?;
            let rest = limit - rows.len() as u32;
            let from = Id(around.0.saturating_sub(1));
            rows.extend(db::messages::after(db, channel_id, from, rest).await?);
            rows
        }
        _ => db::messages::before(db, channel_id, q.before, limit).await?,
    };
    Ok(Json(messages::to_wire(&state, rows, auth.user_id).await?))
}

/// `VIEW_CHANNEL` + `READ_HISTORY`.
pub async fn get(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id)): Path<(Id, Id)>,
) -> ApiResult<Json<Message>> {
    target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL | Permissions::READ_HISTORY,
    )?;
    let row = db::messages::get(&state.db.read, channel_id, message_id)
        .await?
        .ok_or_else(|| Error::not_found("message"))?;
    Ok(Json(one_to_wire(&state, row, auth.user_id).await?))
}

/// An encrypted message as it is stored: JSON, and the `content` next to it must be empty, since a
/// client that also sent the words would defeat the point.
fn sealed_form(e2ee: Option<&Envelope>, content: &str) -> ApiResult<Option<String>> {
    let Some(e2ee) = e2ee else {
        return Ok(None);
    };
    if !content.is_empty() {
        return Err(validate::field_error(
            "content",
            "must be empty in an encrypted message",
        ));
    }
    let json =
        serde_json::to_string(e2ee).map_err(|_| Error::internal("envelope not serializable"))?;
    if json.len() > MAX_ENVELOPE_BYTES {
        return Err(validate::field_error("e2ee", "is too large"));
    }
    Ok(Some(json))
}

/// A sealed reaction: `e2-` and hex, long enough to hold one and short enough to store.
fn is_sealed_reaction(emoji: &str) -> bool {
    emoji.strip_prefix("e2-").is_some_and(|hex| {
        (48..=256).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// A reaction has to be an emoji, or one sealed by a client.
fn check_emoji(emoji: &str) -> ApiResult<()> {
    if emojis::get(emoji).is_none() && !is_sealed_reaction(emoji) {
        return Err(validate::field_error("emoji", "must be a Unicode emoji"));
    }
    Ok(())
}

/// Mentions of people, and of roles, one encrypted message may claim.
const MAX_TOLD_MENTIONS: usize = 100;

/// What a message mentions: read from its text, or as told for an encrypted one.
fn mentions_of(content: &str, encrypted: bool, told: Option<&Mentions>) -> Mentions {
    if encrypted {
        let mut told = told.cloned().unwrap_or_default();
        for list in [&mut told.users, &mut told.roles] {
            list.sort();
            list.dedup();
            list.truncate(MAX_TOLD_MENTIONS);
        }
        told
    } else {
        hermes_proto::markup::extract_mentions(content)
    }
}

/// An encrypted message in a server channel has to use the channel's current key, and one in a
/// DM none of them.
async fn check_epoch(
    state: &AppState,
    channel_id: Id,
    kind: ChannelKind,
    e2ee: Option<&Envelope>,
) -> ApiResult<()> {
    let Some(envelope) = e2ee else {
        return Ok(());
    };
    if kind.is_private() {
        return match envelope.epoch {
            None => Ok(()),
            Some(_) => Err(validate::field_error("e2ee", "must not name a channel key")),
        };
    }
    let current = db::channel_keys::current_epoch(&state.db.read, channel_id).await?;
    if envelope.epoch != Some(current) || current == 0 {
        return Err(Error::new(
            ErrorCode::Conflict,
            "This channel has a newer key",
        ));
    }
    Ok(())
}

/// DMs must be encrypted (`privacy.require_dm_encryption`), and so must server channels
/// (`privacy.require_channel_encryption`).
fn check_encryption(state: &AppState, kind: ChannelKind, e2ee: Option<&String>) -> ApiResult<()> {
    if kind.is_private() {
        if e2ee.is_none() && state.config.privacy.require_dm_encryption {
            return Err(Error::new(
                ErrorCode::BadRequest,
                "Direct messages are end-to-end encrypted here: update Hermes to send them",
            ));
        }
    } else if e2ee.is_none() && state.config.privacy.require_channel_encryption {
        return Err(Error::new(
            ErrorCode::BadRequest,
            "Channels are end-to-end encrypted here: update Hermes to write in them",
        ));
    }
    Ok(())
}

/// `SEND_MESSAGES` (+ `ATTACH_FILES` with attachments); slowmode unless `BYPASS_SLOWMODE`.
/// Mentions are stored and raise the mentioned users' mention counts.
/// The reply to a send whose nonce already made message `id`.
async fn send_again(
    state: &Arc<AppState>,
    channel_id: Id,
    id: Id,
    nonce: String,
    viewer: Id,
) -> ApiResult<Json<Message>> {
    let row = db::messages::get(&state.db.read, channel_id, id)
        .await?
        .ok_or_else(|| Error::not_found("message"))?;
    let mut message = one_to_wire(state, row, viewer).await?;
    message.nonce = Some(nonce);
    Ok(Json(message))
}

pub async fn send(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
    Json(req): Json<SendMessageRequest>,
) -> ApiResult<Json<Message>> {
    let mut fields = Fields::new();
    let max = state.config.limits.max_message_length as usize;
    fields.check("content", validate::message_content(&req.content, max));
    let nonce_chars = req.nonce.chars().count();
    if !(1..=SendMessageRequest::MAX_NONCE_CHARS).contains(&nonce_chars)
        || req.nonce.chars().any(char::is_control)
    {
        fields.add(
            "nonce",
            format!(
                "must be 1 to {} characters",
                SendMessageRequest::MAX_NONCE_CHARS
            ),
        );
    }
    let mut attachment_ids = req.attachment_ids.clone();
    attachment_ids.sort();
    attachment_ids.dedup();
    if attachment_ids.len() > SendMessageRequest::MAX_ATTACHMENTS {
        fields.add(
            "attachment_ids",
            format!(
                "at most {} attachments",
                SendMessageRequest::MAX_ATTACHMENTS
            ),
        );
    }
    if req.content.trim().is_empty() && attachment_ids.is_empty() && req.e2ee.is_none() {
        fields.add("content", "must not be empty");
    }
    fields.finish()?;
    let e2ee = sealed_form(req.e2ee.as_ref(), &req.content)?;

    let (resolved, slowmode, kind) = {
        let cache = state.cache();
        let t = target(&cache, channel_id, auth.user_id, Permissions::SEND_MESSAGES)?;
        check_encryption(&state, t.kind, e2ee.as_ref())?;
        if !attachment_ids.is_empty() {
            t.require(Permissions::ATTACH_FILES)?;
        }
        let raw = mentions_of(&req.content, e2ee.is_some(), req.mentions.as_ref());
        let max_mentions = state.config.limits.max_mentions_per_message as usize;
        if max_mentions > 0 && raw.users.len() + raw.roles.len() > max_mentions {
            return Err(validate::field_error("content", "mentions too many people"));
        }
        if t.kind.is_private()
            && cache.channels.get(&channel_id).is_some_and(|c| {
                c.recipient_ids
                    .iter()
                    .any(|id| *id != auth.user_id && cache.blocked(*id, auth.user_id))
            })
        {
            return Err(Error::new(
                ErrorCode::MissingPermission,
                "This user does not accept direct messages from you",
            ));
        }
        let resolved = messages::resolve_mentions(&cache, channel_id, auth.user_id, t.perms, raw);
        let slowmode = Some(t.slowmode_secs)
            .filter(|&secs| secs > 0 && !t.perms.contains(Permissions::BYPASS_SLOWMODE));
        (resolved, slowmode, t.kind)
    };
    // A retry after a lost reply gets the message it already made, whatever has changed since.
    if let Some(id) =
        db::messages::by_nonce(&state.db.read, channel_id, auth.user_id, &req.nonce).await?
    {
        return send_again(&state, channel_id, id, req.nonce, auth.user_id).await;
    }
    check_epoch(&state, channel_id, kind, req.e2ee.as_ref()).await?;
    if let Some(reply_id) = req.reply_to_id
        && db::messages::get(&state.db.read, channel_id, reply_id)
            .await?
            .is_none()
    {
        return Err(validate::field_error(
            "reply_to_id",
            "must be a message in this channel",
        ));
    }
    state
        .message_limiter
        .check(
            &(channel_id, auth.user_id),
            state.config.limits.messages_per_5s,
            ratelimit::MESSAGES_PERIOD,
        )
        .map_err(|wait| Error(ApiError::rate_limited(ratelimit::retry_ms(wait))))?;
    let now = now_ms();
    if let Some(secs) = slowmode {
        state
            .slowmode
            .try_send(channel_id, auth.user_id, secs, now)
            .map_err(|wait_ms| Error(ApiError::rate_limited(wait_ms)))?;
    }

    let row = MessageRow {
        id: state.ids.next(),
        channel_id,
        author_id: auth.user_id,
        content: req.content,
        reply_to_id: req.reply_to_id,
        edited_at: None,
        deleted: false,
        pinned: false,
        kind: MessageKind::Default,
        e2ee,
    };
    let mut tx = state.db.write.begin().await?;
    // Checked again here: the single writer settles two copies arriving together.
    if let Some(id) = db::messages::by_nonce(&mut *tx, channel_id, auth.user_id, &req.nonce).await?
    {
        drop(tx);
        return send_again(&state, channel_id, id, req.nonce, auth.user_id).await;
    }
    db::messages::insert(&mut tx, &row).await?;
    db::messages::remember_nonce(&mut *tx, &row, &req.nonce, now).await?;
    for &attachment_id in &attachment_ids {
        if !db::messages::link_attachment(&mut tx, row.id, attachment_id, auth.user_id).await? {
            return Err(validate::field_error(
                "attachment_ids",
                format!("{attachment_id} is not an unattached upload of yours"),
            ));
        }
    }
    db::messages::set_mentions(&mut tx, row.id, &resolved.mentions).await?;
    db::messages::set_last_message_id(&mut tx, channel_id, row.id).await?;
    db::messages::add_mention_counts(&mut tx, channel_id, &resolved.notify, row.id).await?;
    tx.commit().await?;
    if let Some(cached) = state.cache_mut().channels.get_mut(&channel_id) {
        let last = &mut cached.channel.last_message_id;
        *last = (*last).max(Some(row.id));
    }
    if !kind.is_private() {
        crate::gateway::last_online::active(&state, auth.user_id);
    }
    if kind.is_private() {
        // Answering a message request accepts it.
        super::dms::accept_request(&state, channel_id, auth.user_id).await?;
    }
    if kind == ChannelKind::Dm {
        super::dms::reopen(&state, channel_id).await?;
    }

    let mut message = one_to_wire(&state, row, auth.user_id).await?;
    message.nonce = Some(req.nonce);
    // The nonce goes only to the author's sessions.
    state.publish_except(
        ServerEvent::MessageCreate(broadcast_copy(&message)),
        Audience::Channel(channel_id),
        auth.user_id,
    );
    state.publish(
        ServerEvent::MessageCreate(message.clone()),
        Audience::User(auth.user_id),
    );
    Ok(Json(message))
}

/// Author only. Mentions are updated, but mention counts are not raised again.
pub async fn edit(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id)): Path<(Id, Id)>,
    Json(req): Json<EditMessageRequest>,
) -> ApiResult<Json<Message>> {
    state.throttle(auth.user_id, Action::Edit)?;
    let max = state.config.limits.max_message_length as usize;
    validate::message_content(&req.content, max)
        .map_err(|problem| validate::field_error("content", problem))?;
    let e2ee = sealed_form(req.e2ee.as_ref(), &req.content)?;
    let t = target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::SEND_MESSAGES,
    )?;
    check_encryption(&state, t.kind, e2ee.as_ref())?;
    check_epoch(&state, channel_id, t.kind, req.e2ee.as_ref()).await?;
    let mut tx = checking_tx(&state).await?;
    let mut row = live_message(&mut *tx, channel_id, message_id).await?;
    if row.author_id != auth.user_id || row.kind != MessageKind::Default {
        return Err(Error::new(
            ErrorCode::MissingPermission,
            "You can only edit your own messages",
        ));
    }
    if req.content.trim().is_empty()
        && e2ee.is_none()
        && db::messages::count_attachments(&mut *tx, message_id).await? == 0
    {
        return Err(validate::field_error("content", "must not be empty"));
    }
    let raw = mentions_of(&req.content, e2ee.is_some(), req.mentions.as_ref());
    let resolved =
        messages::resolve_mentions(&state.cache(), channel_id, auth.user_id, t.perms, raw);

    let now = now_ms();
    db::messages::edit_content(&mut tx, message_id, &req.content, e2ee.as_deref(), now).await?;
    db::messages::set_mentions(&mut tx, message_id, &resolved.mentions).await?;
    tx.commit().await?;
    row.content = req.content;
    row.e2ee = e2ee;
    row.edited_at = Some(now);
    let message = one_to_wire(&state, row, auth.user_id).await?;
    state.publish(
        ServerEvent::MessageUpdate(broadcast_copy(&message)),
        Audience::Channel(channel_id),
    );
    Ok(Json(message))
}

/// The author, or `MANAGE_MESSAGES` (audited). A soft delete: the message stays as a tombstone.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id)): Path<(Id, Id)>,
) -> ApiResult<StatusCode> {
    let t = target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL,
    )?;
    let mut tx = checking_tx(&state).await?;
    let row = live_message(&mut *tx, channel_id, message_id).await?;
    let own = row.author_id == auth.user_id;
    if !own {
        t.require(Permissions::MANAGE_MESSAGES)?;
    }
    let stored = db::attachments::stored_of_message(&mut *tx, message_id).await?;
    db::messages::soft_delete(&mut tx, message_id).await?;
    if let Some(space_id) = t.space_id.filter(|_| !own) {
        audit::record(
            &mut tx,
            &state,
            space_id,
            auth.user_id,
            AuditAction::MessageDelete,
            Some(message_id),
            json!({ "channel_id": channel_id, "author_id": row.author_id }),
        )
        .await?;
    }
    tx.commit().await?;
    crate::files::purge(&state, stored).await;
    state.publish(
        ServerEvent::MessageDelete {
            id: message_id,
            channel_id,
        },
        Audience::Channel(channel_id),
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn pin(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id)): Path<(Id, Id)>,
) -> ApiResult<StatusCode> {
    set_pinned(&state, auth, channel_id, message_id, true).await
}

pub async fn unpin(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id)): Path<(Id, Id)>,
) -> ApiResult<StatusCode> {
    set_pinned(&state, auth, channel_id, message_id, false).await
}

/// Deletes several messages at once: one's own, or anyone's with `MANAGE_MESSAGES`.
pub async fn bulk_delete(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
    Json(req): Json<BulkDeleteRequest>,
) -> ApiResult<StatusCode> {
    if req.ids.len() > BulkDeleteRequest::MAX_IDS {
        return Err(validate::field_error("ids", "are too many"));
    }
    let t = target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL,
    )?;
    let moderator = t.perms.contains(Permissions::MANAGE_MESSAGES);
    let mut stored = Vec::new();
    for id in &req.ids {
        stored.extend(db::attachments::stored_of_message(&state.db.read, *id).await?);
    }
    let author = (!moderator).then_some(auth.user_id);
    let mut tx = state.db.write.begin().await?;
    let mut ids = db::messages::soft_delete_many(&mut tx, channel_id, &req.ids, author).await?;
    if let (Some(space_id), true) = (t.space_id, moderator && !ids.is_empty()) {
        audit::record(
            &mut tx,
            &state,
            space_id,
            auth.user_id,
            AuditAction::MessageDeleteBulk,
            None,
            json!({ "channel_id": channel_id, "count": ids.len() }),
        )
        .await?;
    }
    tx.commit().await?;
    crate::files::purge(&state, stored).await;
    if !ids.is_empty() {
        ids.sort();
        state.publish(
            ServerEvent::MessageDeleteBulk { ids, channel_id },
            Audience::Channel(channel_id),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `MANAGE_MESSAGES`; in DMs any recipient. Pinning twice is not an error.
async fn set_pinned(
    state: &AppState,
    auth: Auth,
    channel_id: Id,
    message_id: Id,
    pinned: bool,
) -> ApiResult<StatusCode> {
    let t = target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL,
    )?;
    if !t.kind.is_private() {
        t.require(Permissions::MANAGE_MESSAGES)?;
    }
    let mut tx = checking_tx(state).await?;
    let mut row = live_message(&mut *tx, channel_id, message_id).await?;
    if row.pinned == pinned {
        return Ok(StatusCode::NO_CONTENT);
    }
    db::messages::set_pinned(&mut *tx, message_id, pinned).await?;
    if let Some(space_id) = t.space_id {
        audit::record(
            &mut tx,
            state,
            space_id,
            auth.user_id,
            if pinned {
                AuditAction::MessagePin
            } else {
                AuditAction::MessageUnpin
            },
            Some(message_id),
            json!({ "channel_id": channel_id }),
        )
        .await?;
    }
    tx.commit().await?;
    row.pinned = pinned;
    let message = one_to_wire(state, row, auth.user_id).await?;
    state.publish(
        ServerEvent::MessageUpdate(broadcast_copy(&message)),
        Audience::Channel(channel_id),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `VIEW_CHANNEL` + `READ_HISTORY`. Newest first.
pub async fn pins(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
) -> ApiResult<Json<Vec<Message>>> {
    target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL | Permissions::READ_HISTORY,
    )?;
    let rows = db::messages::pinned(&state.db.read, channel_id).await?;
    Ok(Json(messages::to_wire(&state, rows, auth.user_id).await?))
}

/// Joining an existing reaction needs `VIEW_CHANNEL`; a new emoji also needs `ADD_REACTIONS`,
/// and a message can have at most [`MAX_REACTION_EMOJI`] different ones.
pub async fn add_reaction(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id, emoji)): Path<(Id, Id, String)>,
) -> ApiResult<StatusCode> {
    state.throttle(auth.user_id, Action::React)?;
    check_emoji(&emoji)?;
    let t = target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL,
    )?;
    // The write transaction serializes the checks (message not deleted, at most 20 emoji) with
    // the insert.
    let mut tx = checking_tx(&state).await?;
    live_message(&mut *tx, channel_id, message_id).await?;
    let existing = db::messages::reaction_emoji(&mut *tx, message_id).await?;
    if !existing.contains(&emoji) {
        t.require(Permissions::ADD_REACTIONS)?;
        if existing.len() >= MAX_REACTION_EMOJI {
            return Err(Error::new(
                ErrorCode::BadRequest,
                format!("A message can have at most {MAX_REACTION_EMOJI} different reactions"),
            ));
        }
    }
    let added =
        db::messages::add_reaction(&mut *tx, message_id, &emoji, auth.user_id, now_ms()).await?;
    tx.commit().await?;
    if added {
        publish_reaction(&state, true, channel_id, message_id, emoji, auth.user_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Removes the caller's own reaction. Removing one that does not exist is not an error.
pub async fn remove_own_reaction(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id, emoji)): Path<(Id, Id, String)>,
) -> ApiResult<StatusCode> {
    target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL,
    )?;
    live_message(&state.db.read, channel_id, message_id).await?;
    if db::messages::remove_reaction(&state.db.write, message_id, &emoji, auth.user_id).await? {
        publish_reaction(&state, false, channel_id, message_id, emoji, auth.user_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `MANAGE_MESSAGES`: removes someone else's reaction.
pub async fn remove_reaction(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id, emoji, user_id)): Path<(Id, Id, String, Id)>,
) -> ApiResult<StatusCode> {
    target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::MANAGE_MESSAGES,
    )?;
    live_message(&state.db.read, channel_id, message_id).await?;
    if db::messages::remove_reaction(&state.db.write, message_id, &emoji, user_id).await? {
        publish_reaction(&state, false, channel_id, message_id, emoji, user_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn publish_reaction(
    state: &AppState,
    added: bool,
    channel_id: Id,
    message_id: Id,
    emoji: String,
    user_id: Id,
) {
    let reaction = ReactionEvent {
        message_id,
        channel_id,
        emoji,
        user_id,
    };
    let event = if added {
        ServerEvent::ReactionAdd(reaction)
    } else {
        ServerEvent::ReactionRemove(reaction)
    };
    state.publish(event, Audience::Channel(channel_id));
}

/// `SEND_MESSAGES`. Sends `typing_start` to everyone else who can see the channel.
pub async fn typing(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
) -> ApiResult<StatusCode> {
    state.throttle(auth.user_id, Action::Typing)?;
    {
        let cache = state.cache();
        let t = target(&cache, channel_id, auth.user_id, Permissions::SEND_MESSAGES)?;
        // Whoever has not accepted a message request does not see presence, or typing either.
        if t.kind.is_private()
            && cache.channels.get(&channel_id).is_some_and(|c| {
                let audience = cache.presence_audience(auth.user_id);
                c.recipient_ids.iter().any(|id| !audience.contains(id))
            })
        {
            return Ok(StatusCode::NO_CONTENT);
        }
    }
    state.publish_except(
        ServerEvent::TypingStart {
            channel_id,
            user_id: auth.user_id,
            timestamp: now_ms(),
        },
        Audience::Channel(channel_id),
        auth.user_id,
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `VIEW_CHANNEL`. Sets the read position to the message (it may move backwards, to mark a
/// channel unread) and resets the mention count.
pub async fn ack(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
    Json(req): Json<AckRequest>,
) -> ApiResult<StatusCode> {
    target(
        &state.cache(),
        channel_id,
        auth.user_id,
        Permissions::VIEW_CHANNEL,
    )?;
    if db::messages::get(&state.db.read, channel_id, req.message_id)
        .await?
        .is_none()
    {
        return Err(Error::not_found("message"));
    }
    db::messages::set_read(&state.db.write, auth.user_id, channel_id, req.message_id).await?;
    state.publish(
        ServerEvent::ReadStateUpdate(ReadState {
            channel_id,
            last_read_id: req.message_id,
            mention_count: 0,
        }),
        Audience::User(auth.user_id),
    );
    Ok(StatusCode::NO_CONTENT)
}
