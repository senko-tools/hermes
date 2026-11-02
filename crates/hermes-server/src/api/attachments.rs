//! `/attachments`: uploads, downloads and thumbnails (PLAN.md sections 10 and 13).

use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use axum::body::{Body, HttpBody};
use axum::extract::{Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use hermes_proto::ErrorCode;
use hermes_proto::id::Id;
use hermes_proto::models::Attachment;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::validate;
use super::{ApiResult, Error, Json, Path, perms};
use crate::auth::Auth;
use crate::db;
use crate::db::attachments::AttachmentRow;
use crate::state::AppState;
use crate::time::now_ms;

/// Longest filename, in bytes.
const MAX_FILENAME_BYTES: usize = 255;
/// Longest stored `Content-Type`.
const MAX_CONTENT_TYPE_LEN: usize = 127;
const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";
const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Raw body with `Content-Type` and `X-Filename` (URL-encoded) → `Attachment`, unattached until
/// a message (or avatar/icon) uses it.
pub async fn upload(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<Attachment>)> {
    let raw_name = headers
        .get("x-filename")
        .ok_or_else(|| validate::field_error("X-Filename", "is required"))?;
    let filename = raw_name
        .to_str()
        .map_err(|_| "must be URL-encoded".to_owned())
        .and_then(filename)
        .map_err(|problem| validate::field_error("X-Filename", problem))?;
    let declared_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map_or_else(|| DEFAULT_CONTENT_TYPE.to_owned(), content_type);

    let storage = &state.config.storage;
    let max = u64::from(storage.max_upload_mb) * MIB;
    let too_large = || {
        Error::new(
            ErrorCode::PayloadTooLarge,
            format!("Files may be at most {} MB", storage.max_upload_mb),
        )
    };
    let declared_len = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared_len.is_some_and(|len| len > max) {
        return Err(too_large());
    }

    let id = state.ids.next();
    let part = state.files.tmp_path(id, ".part");
    let thumb_part = state.files.tmp_path(id, ".thumb.part");
    let result = receive(&part, body, max).await;
    let (sha256, size) = match result {
        Ok(Some(received)) => received,
        Ok(None) => {
            remove(&part);
            return Err(too_large());
        }
        Err(err) => {
            remove(&part);
            return Err(err);
        }
    };
    let quota = u64::from(storage.max_total_storage_gb) * GIB;
    if !state.files.reserve(size, quota) {
        remove(&part);
        return Err(Error::new(
            ErrorCode::StorageFull,
            "The server's storage for uploads is full",
        ));
    }

    let stored = store(
        &state,
        &auth,
        id,
        filename,
        declared_type,
        &part,
        &thumb_part,
        sha256,
        size,
    )
    .await;
    remove(&part);
    remove(&thumb_part);
    match stored {
        Ok(row) => Ok((StatusCode::CREATED, Json(row.to_wire()))),
        Err(err) => {
            state.files.release(size);
            state.files.remove(&sha256, id);
            Err(err)
        }
    }
}

/// Streams `body` into `path`. `None` when it is larger than `max` bytes.
async fn receive(path: &FsPath, mut body: Body, max: u64) -> ApiResult<Option<([u8; 32], u64)>> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|err| Error::internal(format!("create {}: {err}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame
            .map_err(|err| Error::new(ErrorCode::BadRequest, format!("Upload failed: {err}")))?;
        let Ok(data) = frame.into_data() else {
            continue; // trailers
        };
        size += data.len() as u64;
        if size > max {
            return Ok(None);
        }
        hasher.update(&data);
        file.write_all(&data)
            .await
            .map_err(|err| Error::internal(format!("write {}: {err}", path.display())))?;
    }
    file.sync_all()
        .await
        .map_err(|err| Error::internal(format!("write {}: {err}", path.display())))?;
    Ok(Some((hasher.finalize().into(), size)))
}

/// Probes the received file, moves it (and its thumbnail) into place and inserts the row.
#[allow(clippy::too_many_arguments)]
async fn store(
    state: &AppState,
    auth: &Auth,
    id: Id,
    filename: String,
    declared_type: String,
    part: &FsPath,
    thumb_part: &FsPath,
    sha256: [u8; 32],
    size: u64,
) -> ApiResult<AttachmentRow> {
    let thumb = state
        .config
        .storage
        .thumbnails
        .then(|| thumb_part.to_owned());
    let image = state.files.probe(part.to_owned(), thumb).await;
    let path = state.files.path(&sha256, id);
    let io_err = |action: &str, path: &FsPath| {
        let detail = format!("{action} {}", path.display());
        move |err: std::io::Error| Error::internal(format!("{detail}: {err}"))
    };
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(io_err("create", dir))?;
    }
    tokio::fs::rename(part, &path)
        .await
        .map_err(io_err("move upload to", &path))?;
    let has_thumb = image.as_ref().is_some_and(|i| i.has_thumb);
    if has_thumb {
        let thumb = state.files.thumb_path(&sha256, id);
        tokio::fs::rename(thumb_part, &thumb)
            .await
            .map_err(io_err("move thumbnail to", &thumb))?;
    }
    // An image's type comes from its bytes; anything else keeps the declared type but is
    // always served as a download.
    let content_type = image
        .as_ref()
        .map_or(declared_type, |i| i.content_type.to_owned());
    let row = AttachmentRow {
        id,
        uploader_id: auth.user_id,
        message_id: None,
        filename,
        content_type,
        size,
        width: image.as_ref().map(|i| i.width),
        height: image.as_ref().map(|i| i.height),
        has_thumb,
        sha256,
        created_at: now_ms(),
    };
    db::attachments::insert(&state.db.write, &row).await?;
    Ok(row)
}

fn remove(path: &FsPath) {
    let _ = std::fs::remove_file(path);
}

/// The file. Images are shown inline, everything else is forced to download.
pub async fn download(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((id, name)): Path<(Id, String)>,
    request: Request,
) -> ApiResult<Response> {
    let row = readable(&state, id, auth.user_id).await?;
    if row.filename != name {
        return Err(Error::not_found("attachment"));
    }
    let path = state.files.path(&row.sha256, row.id);
    let disposition = if row.is_image() {
        "inline"
    } else {
        "attachment"
    };
    serve(
        path,
        request,
        &row.content_type,
        &content_disposition(disposition, &row.filename),
    )
    .await
}

/// The WebP thumbnail of an image (≤ 400×400).
pub async fn thumb(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<Id>,
    request: Request,
) -> ApiResult<Response> {
    let row = readable(&state, id, auth.user_id).await?;
    if !row.has_thumb {
        return Err(Error::not_found("thumbnail"));
    }
    let path = state.files.thumb_path(&row.sha256, row.id);
    let stem = row
        .filename
        .rsplit_once('.')
        .map_or(&*row.filename, |(s, _)| s);
    let disposition = content_disposition("inline", &format!("{stem}.thumb.webp"));
    serve(path, request, "image/webp", &disposition).await
}

/// The upload if `user_id` may read it (`NOT_FOUND` otherwise): attached → `VIEW_CHANNEL` in the
/// message's channel; an avatar or icon → any user; otherwise only the uploader.
async fn readable(state: &AppState, id: Id, user_id: Id) -> ApiResult<AttachmentRow> {
    let not_found = || Error::not_found("attachment");
    let row = db::attachments::get(&state.db.read, id)
        .await?
        .ok_or_else(not_found)?;
    let allowed = match row.message_id {
        Some(message_id) => {
            match db::attachments::message_channel(&state.db.read, message_id).await? {
                // `perms::channel` requires VIEW_CHANNEL.
                Some(channel_id) => perms::channel(&state.cache(), channel_id, user_id).is_ok(),
                None => false,
            }
        }
        None => {
            row.uploader_id == user_id
                || db::attachments::is_avatar_or_icon(&state.db.read, id).await?
        }
    };
    if allowed { Ok(row) } else { Err(not_found()) }
}

/// Streams `path` (with range and conditional request support) with the safety headers.
async fn serve(
    path: PathBuf,
    request: Request,
    content_type: &str,
    disposition: &str,
) -> ApiResult<Response> {
    let response = match ServeFile::new(&path).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(Error::internal(format!(
            "attachment file {} is missing",
            path.display()
        )));
    }
    let mut response = response.into_response();
    let headers = response.headers_mut();
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    if status.is_success() {
        let content_type = HeaderValue::from_str(content_type)
            .unwrap_or(HeaderValue::from_static(DEFAULT_CONTENT_TYPE));
        headers.insert(CONTENT_TYPE, content_type);
        if let Ok(value) = HeaderValue::from_str(disposition) {
            headers.insert(CONTENT_DISPOSITION, value);
        }
    }
    Ok(response)
}

/// `X-Filename` → the stored filename: percent-decoded UTF-8, without any directory part,
/// trimmed, no control characters or text direction overrides, at most 255 bytes.
fn filename(raw: &str) -> Result<String, String> {
    let decoded = percent_decode(raw).ok_or("must be URL-encoded UTF-8")?;
    let name = decoded
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    if name.is_empty() || name == "." || name == ".." {
        return Err("must be a file name".into());
    }
    validate::name_characters(name)?;
    if name.len() > MAX_FILENAME_BYTES {
        return Err(format!("must be at most {MAX_FILENAME_BYTES} bytes"));
    }
    Ok(name.to_owned())
}

/// Decodes `%XX` escapes; `None` for broken escapes or invalid UTF-8.
fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The declared `Content-Type` without parameters, lowercased; `application/octet-stream` if it
/// is not a plausible `type/subtype`.
fn content_type(raw: &str) -> String {
    let essence = raw.split(';').next().unwrap_or_default().trim();
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match essence.split_once('/') {
        Some((kind, sub)) if token(kind) && token(sub) && essence.len() <= MAX_CONTENT_TYPE_LEN => {
            essence.to_ascii_lowercase()
        }
        _ => DEFAULT_CONTENT_TYPE.to_owned(),
    }
}

/// `inline` or `attachment` with an ASCII fallback name and the exact UTF-8 name (RFC 6266).
fn content_disposition(kind: &str, filename: &str) -> String {
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for b in filename.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_are_decoded_and_cleaned() {
        assert_eq!(filename("cat%20pic.png").unwrap(), "cat pic.png");
        assert_eq!(filename("%E7%8C%AB.txt").unwrap(), "猫.txt");
        assert_eq!(filename("..%2F..%2Fetc%2Fpasswd").unwrap(), "passwd");
        assert_eq!(filename("C:%5Cdir%5Ca.txt").unwrap(), "a.txt");
        assert_eq!(filename("  a.txt ").unwrap(), "a.txt");
        assert!(filename("").is_err());
        assert!(filename("..").is_err());
        assert!(filename("dir%2F").is_err());
        assert!(filename("a%0Ab").is_err());
        // U+202E RIGHT-TO-LEFT OVERRIDE would make "invoice\u{202E}fdp.exe" read as a PDF.
        assert!(filename("invoice%E2%80%AEfdp.exe").is_err());
        // Emoji with zero-width joiners are fine.
        assert!(filename("%F0%9F%91%A9%E2%80%8D%F0%9F%92%BB.png").is_ok());
        assert!(filename("%ZZ").is_err());
        assert!(filename("%FF").is_err());
        assert!(filename(&"a".repeat(256)).is_err());
        assert!(filename(&"a".repeat(255)).is_ok());
    }

    #[test]
    fn content_types_are_normalized() {
        assert_eq!(content_type("Text/Plain; charset=utf-8"), "text/plain");
        assert_eq!(content_type("application/pdf"), "application/pdf");
        assert_eq!(content_type("nonsense"), DEFAULT_CONTENT_TYPE);
        assert_eq!(content_type("a/b c"), DEFAULT_CONTENT_TYPE);
        assert_eq!(content_type("/x"), DEFAULT_CONTENT_TYPE);
    }

    #[test]
    fn disposition_has_ascii_fallback_and_utf8_name() {
        assert_eq!(
            content_disposition("attachment", "my \"file\".txt"),
            "attachment; filename=\"my _file_.txt\"; filename*=UTF-8''my%20%22file%22.txt"
        );
        assert_eq!(
            content_disposition("inline", "猫.png"),
            "inline; filename=\"_.png\"; filename*=UTF-8''%E7%8C%AB.png"
        );
    }
}
