//! The only web pages the server has (PLAN.md section 9.6): the landing page `GET /` and the
//! invite page `GET /invite/{code}`. Self-contained HTML embedded in the binary: no JavaScript,
//! no external resources (the space icon is inlined as a `data:` URI).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderValue, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hermes_proto::ErrorCode;
use hermes_proto::models::{Invite, SpacePreview};

use super::invites::invite_preview;
use crate::db;
use crate::invites;
use crate::state::AppState;

const TEMPLATE: &str = include_str!("page.html");
/// Where the "Download Hermes" buttons point.
pub const DOWNLOAD_URL: &str = "https://github.com/senko-tools/hermes/releases/latest";
/// Inline styles and `data:` images only; nothing else may load or run.
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; \
                   base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// `GET /`: the instance name, how to join and the download link.
pub async fn landing(State(state): State<Arc<AppState>>) -> Response {
    let instance = &state.config.instance;
    let name = escape(&instance.name);
    let description = match instance.description.trim() {
        "" => String::new(),
        text => format!("<p>{}</p>\n", escape(text)),
    };
    let address = escape(&invites::address(&state.config));
    let body = format!(
        r#"<div class="icon" aria-hidden="true">{initial}</div>
<p class="eyebrow">Hermes chat server</p>
<h1>{name}</h1>
{description}<p>To join, open an invite link from a member, or add
<span class="address">{address}</span> in the Hermes app.</p>
<div class="actions">
<a class="button primary" href="{DOWNLOAD_URL}">Download Hermes</a>
</div>"#,
        initial = initial(&instance.name),
    );
    page(StatusCode::OK, &instance.name, &body, "no-cache")
}

/// `GET /invite/{code}`: what the invite is for, an "Open in Hermes" button with the
/// `hermes://` link, and the download link. Invalid or expired codes get a friendly 404.
pub async fn invite(State(state): State<Arc<AppState>>, Path(code): Path<String>) -> Response {
    match invite_preview(&state, &code).await {
        Ok(invite) => valid_invite(&state, &invite).await,
        Err(err) if err.0.code == ErrorCode::InviteInvalid => invalid_invite(&state),
        Err(_) => page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong",
            "<p class=\"eyebrow\">Hermes</p>\n<h1>Something went wrong</h1>\n\
             <p>Please try again in a moment.</p>",
            "no-store",
        ),
    }
}

async fn valid_invite(state: &AppState, invite: &Invite) -> Response {
    let config = &state.config;
    let instance = escape(&config.instance.name);
    let link = invites::links(config, state.fingerprint.as_ref(), &invite.code);
    let (icon, eyebrow, title, details) = match &invite.space {
        Some(space) => (
            space_icon(state, space).await,
            "You have been invited to join",
            space.name.as_str(),
            format!(
                "{} · on {instance}",
                match space.member_count {
                    1 => "1 member".to_owned(),
                    n => format!("{n} members"),
                }
            ),
        ),
        None => (
            initial(&config.instance.name),
            "You have been invited to",
            config.instance.name.as_str(),
            "Create your account on this Hermes server.".to_owned(),
        ),
    };
    let body = format!(
        r#"<div class="icon" aria-hidden="true">{icon}</div>
<p class="eyebrow">{eyebrow}</p>
<h1>{title}</h1>
<p>{details}</p>
<div class="actions">
<a class="button primary" href="{app}">Open in Hermes</a>
<a class="button" href="{DOWNLOAD_URL}">Download Hermes</a>
</div>
<p class="muted">No Hermes yet? Install it, then open this link again.</p>"#,
        title = escape(title),
        app = escape(&link.app),
    );
    page(StatusCode::OK, title, &body, "no-store")
}

fn invalid_invite(state: &AppState) -> Response {
    let body = format!(
        r#"<p class="eyebrow">{instance}</p>
<h1>This invite is not valid</h1>
<p>It may have expired, been used up or been deleted. Ask for a new invite link.</p>
<div class="actions">
<a class="button" href="{DOWNLOAD_URL}">Download Hermes</a>
</div>"#,
        instance = escape(&state.config.instance.name),
    );
    page(StatusCode::NOT_FOUND, "Invalid invite", &body, "no-store")
}

/// The space icon's thumbnail as an inline image, or the first letter of its name.
async fn space_icon(state: &AppState, space: &SpacePreview) -> String {
    let fallback = || initial(&space.name);
    let Some(icon) = &space.icon else {
        return fallback();
    };
    let Ok(Some(row)) = db::attachments::get(&state.db.read, icon.id).await else {
        return fallback();
    };
    if !row.has_thumb {
        return fallback();
    }
    match tokio::fs::read(state.files.thumb_path(&row.sha256, row.id)).await {
        Ok(bytes) => format!(
            r#"<img src="data:image/webp;base64,{}" alt="">"#,
            STANDARD.encode(bytes)
        ),
        Err(err) => {
            tracing::warn!("space icon thumbnail of {}: {err}", space.id);
            fallback()
        }
    }
}

/// The first character of `name`, upper-cased and escaped.
fn initial(name: &str) -> String {
    let first: String = name
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "H".to_owned());
    escape(&first)
}

fn page(status: StatusCode, title: &str, body: &str, cache: &'static str) -> Response {
    let html = TEMPLATE
        .replace("{{title}}", &escape(title))
        .replace("{{body}}", body);
    let mut response = (status, html).into_response();
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    // Invite codes are in the URL; do not leak them to the download site.
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// Escapes text for HTML element content and quoted attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(
            escape(r#"<a href="x">Tom & Jerry's</a>"#),
            "&lt;a href=&quot;x&quot;&gt;Tom &amp; Jerry&#39;s&lt;/a&gt;"
        );
    }

    #[test]
    fn initials() {
        assert_eq!(initial("  hermes"), "H");
        assert_eq!(initial("ñandú"), "Ñ");
        assert_eq!(initial("<x>"), "&lt;");
        assert_eq!(initial(""), "H");
    }

    #[test]
    fn template_has_both_placeholders() {
        assert_eq!(TEMPLATE.matches("{{title}}").count(), 1);
        assert_eq!(TEMPLATE.matches("{{body}}").count(), 1);
        assert!(!TEMPLATE.contains("<script"));
        assert!(!TEMPLATE.contains("http://") && !TEMPLATE.contains("https://"));
    }
}
