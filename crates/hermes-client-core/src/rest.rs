//! REST client for `/api/v1` (PLAN.md section 10): one typed method per endpoint.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use hermes_proto::api::{
    AckRequest, AuthResponse, BanRequest, ChangePasswordRequest, ChannelPosition,
    CreateChannelRequest, CreateDmRequest, CreateInviteRequest, CreateRoleRequest,
    CreateSpaceRequest, EditMessageRequest, HistoryQuery, InstanceResponse, LoginRequest,
    OverrideRequest, RegisterRequest, RolePosition, SendMessageRequest, SessionInfo,
    TransferSpaceRequest, UpdateChannelRequest, UpdateMemberRequest, UpdateRoleRequest,
    UpdateSpaceRequest,
};
use hermes_proto::gateway::SpaceSnapshot;
use hermes_proto::id::Id;
use hermes_proto::models::{
    Attachment, AuditEntry, Ban, Channel, CurrentUser, Invite, Member, Message, Role, Space,
};
use hermes_proto::{ApiError, ErrorBody, ErrorCode};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::address::HostPort;
use crate::tls::{self, Trust};

/// Requests that take longer fail (uploads have their own, longer limit).
const TIMEOUT: Duration = Duration::from_secs(30);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The server answered with an error body.
    Api(ApiError),
    /// No answer: connection refused, DNS, timeout, TLS (also a changed certificate).
    Network(String),
    /// An answer that could not be understood.
    Decode(String),
}

impl Error {
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Error::Api(e) => Some(e.code),
            _ => None,
        }
    }

    /// Whether retrying later may succeed (the request may not have reached the server).
    pub fn is_network(&self) -> bool {
        matches!(self, Error::Network(_))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Api(e) => f.write_str(&e.message),
            Error::Network(e) => write!(f, "could not reach the server: {e}"),
            Error::Decode(e) => write!(f, "unexpected answer from the server: {e}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn network(err: reqwest::Error) -> Error {
    // The full chain names the real cause (e.g. a fingerprint mismatch inside the TLS error).
    let mut text = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    Error::Network(text)
}

/// Percent-encodes one path segment or header value (everything but unreserved characters).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A REST client for one instance, optionally signed in.
#[derive(Debug, Clone)]
pub struct Rest {
    http: reqwest::Client,
    base: Arc<str>,
    token: Option<Arc<str>>,
}

impl Rest {
    pub fn new(host: &HostPort, trust: Trust) -> Result<Self> {
        let config = tls::client_config(trust, true).map_err(|e| Error::Network(e.to_string()))?;
        Self::with_tls((*config).clone(), host)
    }

    pub(crate) fn with_tls(config: rustls::ClientConfig, host: &HostPort) -> Result<Self> {
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(config)
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("hermes-client-core/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(network)?;
        Ok(Rest {
            http,
            base: format!("{}/api/v1", host.url()).into(),
            token: None,
        })
    }

    /// The same client, sending `token`.
    pub fn with_token(&self, token: &str) -> Self {
        Rest {
            token: Some(token.into()),
            ..self.clone()
        }
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .timeout(TIMEOUT);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        req
    }

    async fn send(req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let res = req.send().await.map_err(network)?;
        if res.status().is_success() {
            return Ok(res);
        }
        let status = res.status();
        let bytes = res.bytes().await.map_err(network)?;
        match serde_json::from_slice::<ErrorBody>(&bytes) {
            Ok(body) => Err(Error::Api(body.error)),
            Err(_) => Err(Error::Decode(format!(
                "HTTP {status}: {}",
                String::from_utf8_lossy(&bytes)
            ))),
        }
    }

    async fn json<T: DeserializeOwned>(req: reqwest::RequestBuilder) -> Result<T> {
        let res = Self::send(req).await?;
        let bytes = res.bytes().await.map_err(network)?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Decode(e.to_string()))
    }

    async fn call<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let mut req = self.request(method, path);
        if let Some(body) = body {
            req = req.json(body);
        }
        Self::json(req).await
    }

    async fn empty<B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<()> {
        let mut req = self.request(method, path);
        if let Some(body) = body {
            req = req.json(body);
        }
        let res = Self::send(req).await?;
        debug_assert!(res.status() == StatusCode::NO_CONTENT || res.status().is_success());
        Ok(())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.call::<(), T>(Method::GET, path, None).await
    }

    // ---- instance and auth ---------------------------------------------------------------

    pub async fn instance(&self) -> Result<InstanceResponse> {
        self.get("/instance").await
    }

    pub async fn register(&self, req: &RegisterRequest) -> Result<AuthResponse> {
        self.call(Method::POST, "/auth/register", Some(req)).await
    }

    pub async fn login(&self, req: &LoginRequest) -> Result<AuthResponse> {
        self.call(Method::POST, "/auth/login", Some(req)).await
    }

    pub async fn logout(&self) -> Result<()> {
        self.empty::<()>(Method::POST, "/auth/logout", None).await
    }

    pub async fn sessions(&self) -> Result<Vec<SessionInfo>> {
        self.get("/auth/sessions").await
    }

    pub async fn delete_session(&self, id: Id) -> Result<()> {
        self.empty::<()>(Method::DELETE, &format!("/auth/sessions/{id}"), None)
            .await
    }

    pub async fn me(&self) -> Result<CurrentUser> {
        self.get("/users/@me").await
    }

    pub async fn change_password(&self, req: &ChangePasswordRequest) -> Result<()> {
        self.empty(Method::POST, "/users/@me/password", Some(req))
            .await
    }

    // ---- spaces, members, bans, audit log ------------------------------------------------

    pub async fn create_space(&self, name: &str) -> Result<SpaceSnapshot> {
        let req = CreateSpaceRequest { name: name.into() };
        self.call(Method::POST, "/spaces", Some(&req)).await
    }

    pub async fn update_space(&self, space: Id, req: &UpdateSpaceRequest) -> Result<Space> {
        self.call(Method::PATCH, &format!("/spaces/{space}"), Some(req))
            .await
    }

    pub async fn delete_space(&self, space: Id) -> Result<()> {
        self.empty::<()>(Method::DELETE, &format!("/spaces/{space}"), None)
            .await
    }

    pub async fn transfer_space(&self, space: Id, user_id: Id) -> Result<()> {
        let req = TransferSpaceRequest { user_id };
        self.empty(
            Method::POST,
            &format!("/spaces/{space}/transfer"),
            Some(&req),
        )
        .await
    }

    /// One page of members, ordered by user ID.
    pub async fn members(&self, space: Id, after: Option<Id>, limit: u32) -> Result<Vec<Member>> {
        let after = after.map_or_else(String::new, |id| format!("&after={id}"));
        self.get(&format!("/spaces/{space}/members?limit={limit}{after}"))
            .await
    }

    /// All members, page by page.
    pub async fn all_members(&self, space: Id) -> Result<Vec<Member>> {
        let mut all = Vec::new();
        let mut after = None;
        loop {
            let page = self.members(space, after, 100).await?;
            let done = page.len() < 100;
            after = page.last().map(|m| m.user.id);
            all.extend(page);
            if done || after.is_none() {
                return Ok(all);
            }
        }
    }

    /// `user` may be `None` for `@me`.
    pub async fn update_member(
        &self,
        space: Id,
        user: Option<Id>,
        req: &UpdateMemberRequest,
    ) -> Result<Member> {
        let user = user.map_or_else(|| "@me".to_owned(), |u| u.to_string());
        self.call(
            Method::PATCH,
            &format!("/spaces/{space}/members/{user}"),
            Some(req),
        )
        .await
    }

    pub async fn kick(&self, space: Id, user: Id) -> Result<()> {
        self.empty::<()>(
            Method::DELETE,
            &format!("/spaces/{space}/members/{user}"),
            None,
        )
        .await
    }

    pub async fn leave_space(&self, space: Id) -> Result<()> {
        self.empty::<()>(
            Method::DELETE,
            &format!("/spaces/{space}/members/@me"),
            None,
        )
        .await
    }

    pub async fn bans(&self, space: Id) -> Result<Vec<Ban>> {
        self.get(&format!("/spaces/{space}/bans")).await
    }

    pub async fn ban(&self, space: Id, user: Id, req: &BanRequest) -> Result<()> {
        self.empty(
            Method::PUT,
            &format!("/spaces/{space}/bans/{user}"),
            Some(req),
        )
        .await
    }

    pub async fn unban(&self, space: Id, user: Id) -> Result<()> {
        self.empty::<()>(
            Method::DELETE,
            &format!("/spaces/{space}/bans/{user}"),
            None,
        )
        .await
    }

    pub async fn audit_log(
        &self,
        space: Id,
        before: Option<Id>,
        limit: u32,
    ) -> Result<Vec<AuditEntry>> {
        let before = before.map_or_else(String::new, |id| format!("&before={id}"));
        self.get(&format!("/spaces/{space}/audit-log?limit={limit}{before}"))
            .await
    }

    // ---- roles ----------------------------------------------------------------------------

    pub async fn create_role(&self, space: Id, req: &CreateRoleRequest) -> Result<Role> {
        self.call(Method::POST, &format!("/spaces/{space}/roles"), Some(req))
            .await
    }

    pub async fn update_role(&self, space: Id, role: Id, req: &UpdateRoleRequest) -> Result<Role> {
        self.call(
            Method::PATCH,
            &format!("/spaces/{space}/roles/{role}"),
            Some(req),
        )
        .await
    }

    pub async fn delete_role(&self, space: Id, role: Id) -> Result<()> {
        self.empty::<()>(
            Method::DELETE,
            &format!("/spaces/{space}/roles/{role}"),
            None,
        )
        .await
    }

    pub async fn reorder_roles(&self, space: Id, order: &[RolePosition]) -> Result<Vec<Role>> {
        self.call(
            Method::PUT,
            &format!("/spaces/{space}/roles/order"),
            Some(&order),
        )
        .await
    }

    // ---- channels -------------------------------------------------------------------------

    pub async fn create_channel(&self, space: Id, req: &CreateChannelRequest) -> Result<Channel> {
        self.call(
            Method::POST,
            &format!("/spaces/{space}/channels"),
            Some(req),
        )
        .await
    }

    pub async fn reorder_channels(
        &self,
        space: Id,
        order: &[ChannelPosition],
    ) -> Result<Vec<Channel>> {
        self.call(
            Method::PUT,
            &format!("/spaces/{space}/channels/order"),
            Some(&order),
        )
        .await
    }

    pub async fn update_channel(&self, channel: Id, req: &UpdateChannelRequest) -> Result<Channel> {
        self.call(Method::PATCH, &format!("/channels/{channel}"), Some(req))
            .await
    }

    /// Deletes a channel; on a group DM this leaves it.
    pub async fn delete_channel(&self, channel: Id) -> Result<()> {
        self.empty::<()>(Method::DELETE, &format!("/channels/{channel}"), None)
            .await
    }

    pub async fn set_override(&self, channel: Id, role: Id, req: &OverrideRequest) -> Result<()> {
        self.empty(
            Method::PUT,
            &format!("/channels/{channel}/overrides/{role}"),
            Some(req),
        )
        .await
    }

    pub async fn delete_override(&self, channel: Id, role: Id) -> Result<()> {
        self.empty::<()>(
            Method::DELETE,
            &format!("/channels/{channel}/overrides/{role}"),
            None,
        )
        .await
    }

    // ---- messages -------------------------------------------------------------------------

    /// History, oldest first.
    pub async fn messages(&self, channel: Id, query: &HistoryQuery) -> Result<Vec<Message>> {
        let mut params = vec![format!("limit={}", query.limit())];
        for (name, value) in [
            ("before", query.before),
            ("after", query.after),
            ("around", query.around),
        ] {
            if let Some(id) = value {
                params.push(format!("{name}={id}"));
            }
        }
        self.get(&format!(
            "/channels/{channel}/messages?{}",
            params.join("&")
        ))
        .await
    }

    pub async fn message(&self, channel: Id, message: Id) -> Result<Message> {
        self.get(&format!("/channels/{channel}/messages/{message}"))
            .await
    }

    pub async fn send_message(&self, channel: Id, req: &SendMessageRequest) -> Result<Message> {
        self.call(
            Method::POST,
            &format!("/channels/{channel}/messages"),
            Some(req),
        )
        .await
    }

    pub async fn edit_message(&self, channel: Id, message: Id, content: &str) -> Result<Message> {
        let req = EditMessageRequest {
            content: content.into(),
        };
        self.call(
            Method::PATCH,
            &format!("/channels/{channel}/messages/{message}"),
            Some(&req),
        )
        .await
    }

    pub async fn delete_message(&self, channel: Id, message: Id) -> Result<()> {
        self.empty::<()>(
            Method::DELETE,
            &format!("/channels/{channel}/messages/{message}"),
            None,
        )
        .await
    }

    pub async fn pin(&self, channel: Id, message: Id) -> Result<()> {
        self.empty::<()>(
            Method::PUT,
            &format!("/channels/{channel}/messages/{message}/pin"),
            None,
        )
        .await
    }

    pub async fn unpin(&self, channel: Id, message: Id) -> Result<()> {
        self.empty::<()>(
            Method::DELETE,
            &format!("/channels/{channel}/messages/{message}/pin"),
            None,
        )
        .await
    }

    pub async fn pins(&self, channel: Id) -> Result<Vec<Message>> {
        self.get(&format!("/channels/{channel}/pins")).await
    }

    pub async fn react(&self, channel: Id, message: Id, emoji: &str) -> Result<()> {
        let path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}",
            encode(emoji)
        );
        self.empty::<()>(Method::PUT, &path, None).await
    }

    pub async fn unreact(&self, channel: Id, message: Id, emoji: &str) -> Result<()> {
        let path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}",
            encode(emoji)
        );
        self.empty::<()>(Method::DELETE, &path, None).await
    }

    /// Removes someone else's reaction (`MANAGE_MESSAGES`).
    pub async fn remove_reaction(
        &self,
        channel: Id,
        message: Id,
        emoji: &str,
        user: Id,
    ) -> Result<()> {
        let path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}/{user}",
            encode(emoji)
        );
        self.empty::<()>(Method::DELETE, &path, None).await
    }

    pub async fn typing(&self, channel: Id) -> Result<()> {
        self.empty::<()>(Method::POST, &format!("/channels/{channel}/typing"), None)
            .await
    }

    pub async fn ack(&self, channel: Id, message_id: Id) -> Result<()> {
        let req = AckRequest { message_id };
        self.empty(
            Method::POST,
            &format!("/channels/{channel}/ack"),
            Some(&req),
        )
        .await
    }

    // ---- attachments ----------------------------------------------------------------------

    pub async fn upload(
        &self,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<Attachment> {
        let req = self
            .request(Method::POST, "/attachments")
            .timeout(UPLOAD_TIMEOUT)
            .header("content-type", content_type)
            .header("x-filename", encode(filename))
            .body(bytes);
        Self::json(req).await
    }

    pub async fn download(&self, attachment: Id, filename: &str) -> Result<Vec<u8>> {
        let req = self
            .request(
                Method::GET,
                &format!("/attachments/{attachment}/{}", encode(filename)),
            )
            .timeout(UPLOAD_TIMEOUT);
        let res = Self::send(req).await?;
        Ok(res.bytes().await.map_err(network)?.to_vec())
    }

    pub async fn thumbnail(&self, attachment: Id) -> Result<Vec<u8>> {
        let res =
            Self::send(self.request(Method::GET, &format!("/attachments/{attachment}/thumb")))
                .await?;
        Ok(res.bytes().await.map_err(network)?.to_vec())
    }

    // ---- DMs ------------------------------------------------------------------------------

    pub async fn dms(&self) -> Result<Vec<Channel>> {
        self.get("/dms").await
    }

    /// One recipient reuses the 1:1 DM; more create a group DM.
    pub async fn create_dm(&self, recipient_ids: Vec<Id>) -> Result<Channel> {
        let req = CreateDmRequest { recipient_ids };
        self.call(Method::POST, "/dms", Some(&req)).await
    }

    pub async fn add_recipient(&self, dm: Id, user: Id) -> Result<()> {
        self.empty::<()>(Method::PUT, &format!("/dms/{dm}/recipients/{user}"), None)
            .await
    }

    pub async fn remove_recipient(&self, dm: Id, user: Id) -> Result<()> {
        self.empty::<()>(
            Method::DELETE,
            &format!("/dms/{dm}/recipients/{user}"),
            None,
        )
        .await
    }

    // ---- invites --------------------------------------------------------------------------

    pub async fn create_invite(&self, space: Id, req: &CreateInviteRequest) -> Result<Invite> {
        self.call(Method::POST, &format!("/spaces/{space}/invites"), Some(req))
            .await
    }

    pub async fn invites(&self, space: Id) -> Result<Vec<Invite>> {
        self.get(&format!("/spaces/{space}/invites")).await
    }

    pub async fn invite_preview(&self, code: &str) -> Result<Invite> {
        self.get(&format!("/invites/{}", encode(code))).await
    }

    pub async fn join_invite(&self, code: &str) -> Result<SpaceSnapshot> {
        self.call::<(), _>(Method::POST, &format!("/invites/{}", encode(code)), None)
            .await
    }

    pub async fn delete_invite(&self, code: &str) -> Result<()> {
        self.empty::<()>(Method::DELETE, &format!("/invites/{}", encode(code)), None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding() {
        assert_eq!(encode("😀"), "%F0%9F%98%80");
        assert_eq!(encode("a b/c.png"), "a%20b%2Fc.png");
        assert_eq!(encode("Ab-9_~."), "Ab-9_~.");
    }
}
