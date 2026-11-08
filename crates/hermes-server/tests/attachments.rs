//! Attachments: upload, download, thumbnails, access rules, limits and cleanup.

mod common;

use std::io::Cursor;

use common::{Res, TestServer, start, start_with};
use hermes_proto::api::PICTURE_MAX_BYTES;
use hermes_server::config::RegistrationMode;
use hermes_server::files;
use hermes_server::time::now_ms;
use serde_json::{Value, json};

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image::RgbImage::from_pixel(width, height, image::Rgb([10, 120, 200]))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_owned()
}

async fn status(server: &TestServer, path: &str, token: Option<&str>) -> u16 {
    server.get_raw(path, token).await.status().as_u16()
}

#[tokio::test]
async fn image_upload_download_and_thumbnail() {
    let server = start("attachments-image", RegistrationMode::Open).await;
    let (owner, _, _) = server.owner().await;
    let bytes = png(1000, 500);
    // The declared type is ignored for images: the bytes decide.
    let res = server
        .upload(
            &owner,
            "cat%20pic.png",
            "application/octet-stream",
            bytes.clone(),
        )
        .await;
    assert_eq!(res.status.as_u16(), 201, "{}", res.body);
    let attachment = res.body;
    assert_eq!(
        attachment,
        json!({
            "id": attachment["id"], "filename": "cat pic.png", "content_type": "image/png",
            "size": bytes.len(), "width": 1000, "height": 500, "has_thumb": true
        })
    );
    let aid = id(&attachment);

    let res = server
        .get_raw(&format!("/attachments/{aid}/cat%20pic.png"), Some(&owner))
        .await;
    assert_eq!(res.status().as_u16(), 200);
    let headers = res.headers().clone();
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("inline; ")
    );
    assert_eq!(res.bytes().await.unwrap(), bytes);

    let res = server
        .get_raw(&format!("/attachments/{aid}/thumb"), Some(&owner))
        .await;
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(res.headers()["content-type"], "image/webp");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    let thumb = image::load_from_memory(&res.bytes().await.unwrap()).unwrap();
    assert_eq!((thumb.width(), thumb.height()), (400, 200));

    // Ranges work (large downloads can resume).
    let res = server
        .client()
        .get(format!("{}/attachments/{aid}/cat%20pic.png", server.base))
        .bearer_auth(&owner)
        .header("range", "bytes=0-3")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 206);
    assert_eq!(&res.bytes().await.unwrap()[..], &bytes[..4]);

    // Wrong filename, no token, unknown ID.
    let wrong = format!("/attachments/{aid}/dog.png");
    assert_eq!(status(&server, &wrong, Some(&owner)).await, 404);
    let right = format!("/attachments/{aid}/cat%20pic.png");
    assert_eq!(status(&server, &right, None).await, 401);
    assert_eq!(
        status(&server, "/attachments/12345/x", Some(&owner)).await,
        404
    );
}

#[tokio::test]
async fn other_files_are_downloads_without_thumbnail() {
    let server = start("attachments-other", RegistrationMode::Open).await;
    let (owner, _, _) = server.owner().await;
    let html = b"<script>alert(1)</script>".to_vec();
    let attachment = server
        .upload(
            &owner,
            "page.html",
            "Text/HTML; charset=utf-8",
            html.clone(),
        )
        .await
        .ok();
    assert_eq!(attachment["content_type"], "text/html");
    assert_eq!(attachment["width"], Value::Null);
    assert_eq!(attachment["has_thumb"], false);
    let aid = id(&attachment);

    let res = server
        .get_raw(&format!("/attachments/{aid}/page.html"), Some(&owner))
        .await;
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(res.headers()["content-type"], "text/html");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        res.headers()["content-disposition"],
        "attachment; filename=\"page.html\"; filename*=UTF-8''page.html"
    );
    assert_eq!(res.bytes().await.unwrap(), html);

    // An SVG is not treated as an image either.
    let svg = server
        .upload(
            &owner,
            "x.svg",
            "image/svg+xml",
            br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#.to_vec(),
        )
        .await
        .ok();
    assert_eq!(svg["width"], Value::Null);
    let res = server
        .get_raw(&format!("/attachments/{}/x.svg", id(&svg)), Some(&owner))
        .await;
    assert!(
        res.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; ")
    );
    assert_eq!(
        status(&server, &format!("/attachments/{aid}/thumb"), Some(&owner)).await,
        404
    );

    // Empty files are fine.
    let empty = server.upload(&owner, "empty", "", vec![]).await.ok();
    assert_eq!(empty["size"], 0);
    assert_eq!(empty["content_type"], "application/octet-stream");
}

#[tokio::test]
async fn access_follows_the_message_channel() {
    let server = start("attachments-access", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (outsider, _) = server.register("outsider", None).await;
    let general = server.channel_id(&space_id, "general");

    let aid = id(&server
        .upload(&owner, "a.txt", "text/plain", b"hello".to_vec())
        .await
        .ok());
    let path = format!("/attachments/{aid}/a.txt");
    // Unattached: only the uploader.
    assert_eq!(status(&server, &path, Some(&owner)).await, 200);
    assert_eq!(status(&server, &path, Some(&alice)).await, 404);
    // Someone else cannot attach it either.
    server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&alice),
            json!({ "content": "x", "nonce": "n", "attachment_ids": [aid] }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    let message = server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&owner),
            json!({ "content": "file", "nonce": "n", "attachment_ids": [aid] }),
        )
        .await
        .ok();
    assert_eq!(message["attachments"][0]["id"], json!(aid));
    assert_eq!(status(&server, &path, Some(&alice)).await, 200);
    assert_eq!(status(&server, &path, Some(&outsider)).await, 404);

    // Without VIEW_CHANNEL on the channel the file is gone for alice too.
    server
        .put(
            &format!("/channels/{general}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": hermes_proto::permissions::Permissions::VIEW_CHANNEL.bits().to_string() }),
        )
        .await
        .ok();
    assert_eq!(status(&server, &path, Some(&alice)).await, 404);
    assert_eq!(status(&server, &path, Some(&owner)).await, 200);

    // A deleted message unlinks its files: only the uploader keeps access until cleanup.
    server
        .delete(
            &format!("/channels/{general}/messages/{}", id(&message)),
            Some(&owner),
        )
        .await
        .ok();
    assert_eq!(status(&server, &path, Some(&owner)).await, 200);

    // Avatars are visible to every user.
    let avatar = id(&server
        .upload(&owner, "me.png", "image/png", png(8, 8))
        .await
        .ok());
    server
        .sql(&format!(
            "UPDATE users SET avatar_id = {avatar} WHERE username = 'owner'"
        ))
        .await;
    let avatar_path = format!("/attachments/{avatar}/me.png");
    assert_eq!(status(&server, &avatar_path, Some(&outsider)).await, 200);
}

#[tokio::test]
async fn validation_and_size_limits() {
    let server = start_with("attachments-limits", RegistrationMode::Open, |state| {
        state.config.storage.max_upload_mb = 1;
    })
    .await;
    let (owner, _, _) = server.owner().await;
    let mib = 1024 * 1024;

    server
        .upload(&owner, "full.bin", "application/octet-stream", vec![7; mib])
        .await
        .ok();
    server
        .upload(
            &owner,
            "big.bin",
            "application/octet-stream",
            vec![7; mib + 1],
        )
        .await
        .err(413, "PAYLOAD_TOO_LARGE");

    for bad in ["", "..", "a%0Ab", "%FF"] {
        let res: Res = server
            .upload(&owner, bad, "text/plain", b"x".to_vec())
            .await;
        res.err(400, "VALIDATION_FAILED");
        assert!(
            res.body["error"]["fields"]["X-Filename"].is_string(),
            "{}",
            res.body
        );
    }
    // Missing header.
    let res = server
        .client()
        .post(format!("{}/attachments", server.base))
        .bearer_auth(&owner)
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 400);

    // Nothing of the rejected uploads is left behind.
    let tmp = server
        .state
        .config
        .storage
        .data_dir
        .join("files")
        .join("tmp");
    assert_eq!(std::fs::read_dir(tmp).unwrap().count(), 0);
}

#[tokio::test]
async fn storage_quota() {
    const GIB: u64 = 1024 * 1024 * 1024;
    let server = start_with("attachments-quota", RegistrationMode::Open, |state| {
        state.config.storage.max_total_storage_gb = 1;
        assert!(state.files.reserve(GIB - 10, 0));
    })
    .await;
    let (owner, _, _) = server.owner().await;
    server
        .upload(&owner, "a", "text/plain", vec![1; 11])
        .await
        .err(507, "STORAGE_FULL");
    server
        .upload(&owner, "b", "text/plain", vec![1; 10])
        .await
        .ok();
    assert_eq!(server.state.files.used(), GIB);
}

#[tokio::test]
async fn one_person_cannot_pass_their_storage() {
    let server = start_with("attachments-user-quota", RegistrationMode::Open, |state| {
        state.config.storage.max_user_storage_mb = 1;
    })
    .await;
    const MIB: usize = 1024 * 1024;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    server
        .upload(&alice, "a", "text/plain", vec![1; MIB - 10])
        .await
        .ok();
    let used = server.state.files.used();
    server
        .upload(&alice, "b", "text/plain", vec![1; 11])
        .await
        .err(507, "STORAGE_FULL");
    assert_eq!(
        server.state.files.used(),
        used,
        "a refused upload is not counted"
    );
    server
        .upload(&alice, "c", "text/plain", vec![1; 10])
        .await
        .ok();
    // Someone else has their own allowance.
    server
        .upload(&owner, "d", "text/plain", vec![1; 11])
        .await
        .ok();
}

#[tokio::test]
async fn text_counts_towards_the_limit_only_when_asked_to() {
    const GIB: u64 = 1024 * 1024 * 1024;
    let server = start_with("attachments-text-quota", RegistrationMode::Open, |state| {
        state.config.storage.max_total_storage_gb = 1;
        state.config.storage.limit_includes_messages = true;
        assert!(state.files.reserve(GIB - 10, 0));
    })
    .await;
    let (owner, _, _) = server.owner().await;
    // The database is far larger than the 10 bytes left, so nothing fits.
    server
        .upload(&owner, "a", "text/plain", vec![1; 5])
        .await
        .err(507, "STORAGE_FULL");
}

#[tokio::test]
async fn thumbnails_can_be_disabled() {
    let server = start_with("attachments-nothumbs", RegistrationMode::Open, |state| {
        state.config.storage.thumbnails = false;
    })
    .await;
    let (owner, _, _) = server.owner().await;
    let attachment = server
        .upload(&owner, "a.png", "image/png", png(600, 600))
        .await
        .ok();
    assert_eq!(
        (&attachment["width"], &attachment["has_thumb"]),
        (&json!(600), &json!(false))
    );
}

#[tokio::test]
async fn cleanup_deletes_only_old_unattached_uploads() {
    let server = start("attachments-cleanup", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let general = server.channel_id(&space_id, "general");
    let upload = |name: &'static str| {
        let (server, owner) = (&server, &owner);
        async move {
            id(&server
                .upload(owner, name, "image/png", png(500, 500))
                .await
                .ok())
        }
    };
    let (old, attached, avatar, fresh) = (
        upload("old.png").await,
        upload("attached.png").await,
        upload("avatar.png").await,
        upload("fresh.png").await,
    );
    server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&owner),
            json!({ "content": "", "nonce": "n", "attachment_ids": [attached] }),
        )
        .await
        .ok();
    server
        .sql(&format!(
            "UPDATE users SET avatar_id = {avatar} WHERE username = 'owner'"
        ))
        .await;
    server
        .sql(&format!(
            "UPDATE attachments SET created_at = 0 WHERE id IN ({old}, {attached}, {avatar})"
        ))
        .await;
    let used = server.state.files.used();
    let row = hermes_server::db::attachments::get(&server.state.db.read, old.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let (file, thumb) = (
        server.state.files.path(&row.sha256, row.id),
        server.state.files.thumb_path(&row.sha256, row.id),
    );
    assert!(file.is_file() && thumb.is_file());

    assert_eq!(files::cleanup(&server.state, now_ms()).await.unwrap(), 1);
    assert!(!file.exists() && !thumb.exists());
    assert_eq!(server.state.files.used(), used - row.size);
    for (aid, name, expected) in [
        (old, "old.png", 404),
        (attached, "attached.png", 200),
        (avatar, "avatar.png", 200),
        (fresh, "fresh.png", 200),
    ] {
        let path = format!("/attachments/{aid}/{name}");
        assert_eq!(
            status(&server, &path, Some(&owner)).await,
            expected,
            "{name}"
        );
    }
    assert_eq!(files::cleanup(&server.state, now_ms()).await.unwrap(), 0);
}

#[tokio::test]
async fn profile_banners_are_my_images_and_visible_to_everyone() {
    let server = start("attachments-banner", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (outsider, _) = server.register("outsider", None).await;
    let (mut ws, _) = server.identify(&owner).await;

    let banner = id(&server
        .upload(&alice, "wide.png", "image/png", png(50, 20))
        .await
        .ok());
    let me = server
        .patch("/users/@me", Some(&alice), json!({ "banner_id": banner }))
        .await
        .ok();
    assert_eq!(
        me["banner"],
        json!({ "id": banner, "filename": "wide.png" })
    );
    let event = ws.recv_op("user_update").await;
    assert_eq!(event["d"]["id"], alice_id);
    assert_eq!(event["d"]["banner"]["id"], banner.as_str());
    let path = format!("/attachments/{banner}/wide.png");
    assert_eq!(status(&server, &path, Some(&outsider)).await, 200);

    // Only an image I uploaded.
    let theirs = id(&server
        .upload(&owner, "theirs.png", "image/png", png(50, 20))
        .await
        .ok());
    server
        .patch("/users/@me", Some(&alice), json!({ "banner_id": theirs }))
        .await
        .err(400, "VALIDATION_FAILED");
    let text = id(&server
        .upload(&alice, "notes.txt", "text/plain", b"hi".to_vec())
        .await
        .ok());
    server
        .patch("/users/@me", Some(&alice), json!({ "banner_id": text }))
        .await
        .err(400, "VALIDATION_FAILED");

    // Cleared with null; the avatar is untouched.
    let me = server
        .patch("/users/@me", Some(&alice), json!({ "banner_id": null }))
        .await
        .ok();
    assert_eq!(me.get("banner"), None);
}

#[tokio::test]
async fn avatars_banners_and_icons_are_limited_but_chat_images_are_not() {
    let server = start("attachments-picture-limit", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    // A real picture, padded past its end to the wanted size.
    let sized = |len: usize| {
        let mut bytes = png(50, 20);
        bytes.resize(len, 0);
        bytes
    };
    let limit = PICTURE_MAX_BYTES as usize;
    let too_big = id(&server
        .upload(&owner, "big.png", "image/png", sized(limit + 1))
        .await
        .ok());
    let fits = id(&server
        .upload(&owner, "fits.png", "image/png", sized(limit))
        .await
        .ok());

    let space = format!("/spaces/{space_id}");
    for (path, field) in [
        ("/users/@me", "avatar_id"),
        ("/users/@me", "banner_id"),
        (space.as_str(), "icon_id"),
    ] {
        let res: Res = server
            .patch(path, Some(&owner), json!({ field: too_big }))
            .await;
        res.err(400, "VALIDATION_FAILED");
        assert!(
            res.body["error"]["fields"][field].is_string(),
            "{}",
            res.body
        );
        server
            .patch(path, Some(&owner), json!({ field: fits }))
            .await
            .ok();
    }
}

#[tokio::test]
async fn sounds_keep_the_waveform_and_length_their_uploader_measured() {
    let server = start("attachments-measured", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let general = server.channel_id(&space_id, "general");
    let measured = [("x-waveform", "00ff7f"), ("x-duration-ms", "61500")];

    let sound = server
        .upload_with(
            &owner,
            "song.mp3",
            "audio/mpeg",
            b"ID3 not really".to_vec(),
            &measured,
        )
        .await
        .ok();
    assert_eq!(sound["waveform"], json!([0, 255, 127]));
    assert_eq!(sound["duration_ms"], 61500);

    // In the message too.
    let message = server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&owner),
            json!({ "content": "", "attachment_ids": [id(&sound)], "nonce": "n" }),
        )
        .await
        .ok();
    assert_eq!(message["attachments"][0]["waveform"], json!([0, 255, 127]));
    assert_eq!(message["attachments"][0]["duration_ms"], 61500);

    // A picture has neither, whatever is claimed.
    let picture = server
        .upload_with(&owner, "a.png", "image/png", png(4, 4), &measured)
        .await
        .ok();
    assert_eq!(picture.get("waveform"), None);
    assert_eq!(picture.get("duration_ms"), None);

    // Nonsense is refused.
    for bad in [
        ("x-waveform", "0"),
        ("x-waveform", "zz"),
        ("x-duration-ms", "-5"),
    ] {
        server
            .upload_with(&owner, "s.mp3", "audio/mpeg", b"x".to_vec(), &[bad])
            .await
            .err(400, "VALIDATION_FAILED");
    }
    let long = "ab".repeat(257);
    server
        .upload_with(
            &owner,
            "s.mp3",
            "audio/mpeg",
            b"x".to_vec(),
            &[("x-waveform", &long)],
        )
        .await
        .err(400, "VALIDATION_FAILED");
}
