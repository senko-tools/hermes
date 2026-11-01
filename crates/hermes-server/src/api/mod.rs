//! REST API under `/api/v1` (PLAN.md section 10): routers per resource, validation and the
//! mapping of errors to HTTP responses.

mod admin;
mod attachments;
mod audit;
mod auth;
mod channels;
mod dms;
mod instance;
mod invites;
mod members;
mod messages;
pub mod pages;
mod perms;
mod roles;
mod search;
mod spaces;
mod users;
mod validate;
mod voice;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::connect_info::Connected;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::serve::IncomingStream;
use hermes_proto::id::Id;
use hermes_proto::{ApiError, ErrorBody, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::net::TcpListener;

use crate::ratelimit;
use crate::state::AppState;
use crate::tls::{Tls, TlsListener};

/// Largest JSON request body; uploads (`POST /attachments`) stream and have their own limit.
pub const JSON_BODY_LIMIT: usize = 256 * 1024;

/// The `/api/v1` router plus everything outside it. Must run inside a tokio runtime (the rate
/// limiters start cleanup tasks).
pub fn router(state: Arc<AppState>) -> Router {
    let trusted = &state.config.network.trusted_proxies;
    let limits = &state.config.limits;
    let api = Router::new()
        .route("/gateway", get(crate::gateway::handler))
        .route("/instance", get(instance::get))
        .route("/auth/register", post(auth::register))
        .route(
            "/auth/login",
            ratelimit::limit_login(post(auth::login), trusted, limits.login_attempts_per_minute),
        )
        .route("/auth/logout", post(auth::logout))
        .route("/auth/sessions", get(auth::list_sessions))
        .route("/auth/sessions/{id}", delete(auth::delete_session))
        .route("/users/@me", get(users::me).patch(users::update_me))
        .route("/users/@me/password", post(users::change_password))
        .route("/users/@me/mentions", get(users::mentions))
        .route(
            "/users/@me/notification-settings/{target_id}",
            put(users::put_notification_setting).delete(users::delete_notification_setting),
        )
        .route("/users/{id}", get(users::get))
        .route("/search", get(search::search))
        .route("/admin/users", get(admin::list_users))
        .route("/admin/users/{id}", patch(admin::update_user))
        .route("/spaces", post(spaces::create))
        .route("/spaces/{id}", patch(spaces::update).delete(spaces::delete))
        .route("/spaces/{id}/transfer", post(spaces::transfer))
        .route("/spaces/{id}/members", get(members::list))
        .route(
            "/spaces/{id}/members/{uid}",
            patch(members::update).delete(members::remove),
        )
        .route("/spaces/{id}/bans", get(members::list_bans))
        .route(
            "/spaces/{id}/bans/{uid}",
            put(members::ban).delete(members::unban),
        )
        .route("/spaces/{id}/audit-log", get(audit::list))
        .route("/spaces/{id}/roles", post(roles::create))
        .route("/spaces/{id}/roles/order", put(roles::reorder))
        .route(
            "/spaces/{id}/roles/{rid}",
            patch(roles::update).delete(roles::delete),
        )
        .route("/spaces/{id}/channels", post(channels::create))
        .route("/spaces/{id}/channels/order", put(channels::reorder))
        .route(
            "/channels/{id}",
            patch(channels::update).delete(channels::delete),
        )
        .route(
            "/channels/{id}/overrides/{role_id}",
            put(channels::put_override).delete(channels::delete_override),
        )
        .route(
            "/channels/{id}/messages",
            get(messages::history).post(messages::send),
        )
        .route(
            "/channels/{id}/messages/{mid}",
            get(messages::get)
                .patch(messages::edit)
                .delete(messages::delete),
        )
        .route(
            "/channels/{id}/messages/{mid}/pin",
            put(messages::pin).delete(messages::unpin),
        )
        .route("/channels/{id}/pins", get(messages::pins))
        .route(
            "/channels/{id}/messages/{mid}/reactions/{emoji}",
            put(messages::add_reaction).delete(messages::remove_own_reaction),
        )
        .route(
            "/channels/{id}/messages/{mid}/reactions/{emoji}/{uid}",
            delete(messages::remove_reaction),
        )
        .route("/channels/{id}/typing", post(messages::typing))
        .route("/channels/{id}/ack", post(messages::ack))
        .route("/attachments", post(attachments::upload))
        .route("/attachments/{id}/thumb", get(attachments::thumb))
        .route("/attachments/{id}/{filename}", get(attachments::download))
        .route("/dms", get(dms::list).post(dms::create))
        .route(
            "/dms/{id}/recipients/{uid}",
            put(dms::add_recipient).delete(dms::remove_recipient),
        )
        .route(
            "/spaces/{id}/invites",
            post(invites::create).get(invites::list),
        )
        .route(
            "/invites/{code}",
            get(invites::preview)
                .post(invites::join)
                .delete(invites::delete),
        )
        .route("/voice/join", post(voice::join))
        .route("/voice/leave", post(voice::leave))
        .route("/voice/@me", patch(voice::update_me))
        .route(
            "/spaces/{id}/voice-states/{uid}",
            patch(voice::update_state),
        )
        .route("/channels/{id}/call/ring", post(voice::ring))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(JSON_BODY_LIMIT));
    let api = ratelimit::limit_rest(api, trusted, limits.requests_per_second);
    Router::new()
        .nest("/api/v1", api)
        .route("/healthz", get(healthz))
        .route("/", get(pages::landing))
        .route("/invite/{code}", get(pages::invite))
        .with_state(state)
}

/// `GET /healthz`, for Docker and uptime checks.
async fn healthz() -> &'static str {
    "ok"
}

async fn not_found() -> Error {
    Error::new(ErrorCode::NotFound, "Unknown endpoint")
}

/// Serves `app` on `tcp`, terminating TLS unless `tls` is [`Tls::Off`], until `shutdown`
/// completes and all connections have finished. Handlers can extract `ConnectInfo<PeerAddr>`.
pub async fn serve(
    tcp: TcpListener,
    tls: &Tls,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    let service = app.into_make_service_with_connect_info::<PeerAddr>();
    match tls {
        Tls::Off => {
            axum::serve(tcp, service)
                .with_graceful_shutdown(shutdown)
                .await
        }
        Tls::Rustls { config, .. } => {
            axum::serve(TlsListener::new(tcp, config.clone())?, service)
                .with_graceful_shutdown(shutdown)
                .await
        }
    }
}

/// The TCP peer of a connection. Use [`AppState::client_ip`] for the client's address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

impl Connected<IncomingStream<'_, TcpListener>> for PeerAddr {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        PeerAddr(*stream.remote_addr())
    }
}

impl Connected<IncomingStream<'_, TlsListener>> for PeerAddr {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        PeerAddr(*stream.remote_addr())
    }
}

/// An error response: `{ "error": { … } }` with the status of its code.
#[derive(Debug)]
pub struct Error(pub ApiError);

pub type ApiResult<T> = Result<T, Error>;

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Error(ApiError::new(code, message))
    }

    /// Logs `detail` and responds with a generic `INTERNAL` error.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::error!("internal error: {detail}");
        Error::new(ErrorCode::Internal, "Internal server error")
    }

    pub fn not_found(what: &str) -> Self {
        Error::new(ErrorCode::NotFound, format!("Unknown {what}"))
    }

    pub fn missing_permission(permission: &str) -> Self {
        Error::new(
            ErrorCode::MissingPermission,
            format!("You need {permission} in this space"),
        )
    }
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::internal(format!("database: {err}"))
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, axum::Json(ErrorBody::from(self.0))).into_response()
    }
}

/// `axum::Json` with rejections turned into `BAD_REQUEST` error bodies.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Json<T> {
    type Rejection = Error;

    async fn from_request(req: Request, state: &S) -> ApiResult<Self> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Json(value)),
            Err(rejection) => Err(json_rejection(rejection)),
        }
    }
}

fn json_rejection(rejection: JsonRejection) -> Error {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return Error::new(
            ErrorCode::PayloadTooLarge,
            format!(
                "Request bodies may be at most {} KiB",
                JSON_BODY_LIMIT / 1024
            ),
        );
    }
    Error::new(ErrorCode::BadRequest, rejection.body_text())
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::extract::Path` with rejections turned into `BAD_REQUEST` error bodies.
#[derive(Debug, Clone, Copy)]
pub struct Path<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for Path<T> {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> ApiResult<Self> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Path(value)),
            Err(rejection) => Err(path_rejection(rejection)),
        }
    }
}

fn path_rejection(rejection: PathRejection) -> Error {
    Error::new(ErrorCode::BadRequest, rejection.body_text())
}

/// `axum::extract::Query` with rejections turned into `BAD_REQUEST` error bodies.
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Query<T> {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> ApiResult<Self> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Query(value)),
            Err(rejection) => Err(query_rejection(rejection)),
        }
    }
}

fn query_rejection(rejection: QueryRejection) -> Error {
    Error::new(ErrorCode::BadRequest, rejection.body_text())
}

/// A user ID path segment that may also be `@me`.
pub fn user_ref(segment: &str, me: Id) -> ApiResult<Id> {
    if segment == "@me" {
        return Ok(me);
    }
    segment
        .parse()
        .map_err(|_| Error::new(ErrorCode::BadRequest, "Invalid user ID"))
}
