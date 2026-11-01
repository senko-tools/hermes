//! The landing page `/` and the invite page `/invite/{code}`.

mod common;

use std::io::Cursor;

use common::{TestServer, start, start_with};
use hermes_server::config::RegistrationMode;
use hermes_server::tls::Fingerprint;
use serde_json::json;

const DOWNLOAD: &str = "https://github.com/senko-tools/hermes/releases/latest";

/// `GET path` outside `/api/v1`: `(status, html)`, checking the security headers.
async fn page(server: &TestServer, path: &str) -> (u16, String) {
    let res = server
        .client()
        .get(format!("{}{path}", server.root))
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    assert_eq!(headers["content-type"], "text/html; charset=utf-8");
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(csp.starts_with("default-src 'none'"), "{csp}");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    let html = res.text().await.unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(!html.contains("<script"), "no JavaScript");
    (status, html)
}

async fn invite_code(server: &TestServer, token: &str, space_id: &str) -> String {
    let invite = server
        .post(
            &format!("/spaces/{space_id}/invites"),
            Some(token),
            json!({}),
        )
        .await
        .ok();
    invite["code"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn landing_page() {
    let server = start_with("pages-landing", RegistrationMode::Open, |state| {
        state.config.instance.name = "<b>Tom & Jerry's</b>".into();
        state.config.instance.description = "Friends only".into();
        state.config.instance.public_host = "chat.example.org".into();
        state.config.instance.public_port = 7800;
    })
    .await;
    let (status, html) = page(&server, "/").await;
    assert_eq!(status, 200);
    assert!(html.contains("<title>&lt;b&gt;Tom &amp; Jerry&#39;s&lt;/b&gt;</title>"));
    assert!(html.contains("<h1>&lt;b&gt;Tom &amp; Jerry&#39;s&lt;/b&gt;</h1>"));
    assert!(!html.contains("<b>Tom"));
    assert!(html.contains("<p>Friends only</p>"));
    assert!(html.contains("chat.example.org:7800"));
    assert!(html.contains(DOWNLOAD));
}

#[tokio::test]
async fn invite_page_for_a_space() {
    let server = start("pages-invite", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let code = invite_code(&server, &owner, &space_id).await;
    let (status, html) = page(&server, &format!("/invite/{code}")).await;
    assert_eq!(status, 200);
    assert!(html.contains("You have been invited to join"));
    assert!(html.contains("<h1>Test Instance</h1>"));
    assert!(html.contains("1 member · on Test Instance"));
    assert!(html.contains(&format!(r#"href="hermes://localhost/invite/{code}""#)));
    assert!(html.contains(DOWNLOAD));
    // No icon yet: the first letter.
    assert!(html.contains(r#"<div class="icon" aria-hidden="true">T</div>"#));

    server.member(&owner, &space_id, "alice").await;
    let (_, html) = page(&server, &format!("/invite/{code}")).await;
    assert!(html.contains("2 members"));

    // With an icon, its thumbnail is inlined.
    let mut png = Cursor::new(Vec::new());
    image::RgbImage::from_pixel(64, 64, image::Rgb([1, 2, 3]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let icon = server
        .upload(&owner, "icon.png", "image/png", png.into_inner())
        .await
        .ok();
    server
        .patch(
            &format!("/spaces/{space_id}"),
            Some(&owner),
            json!({ "icon_id": icon["id"] }),
        )
        .await
        .ok();
    let (_, html) = page(&server, &format!("/invite/{code}")).await;
    assert!(
        html.contains(r#"<img src="data:image/webp;base64,"#),
        "{html}"
    );
}

#[tokio::test]
async fn invite_page_carries_the_fingerprint_when_self_signed() {
    let server = start_with("pages-fingerprint", RegistrationMode::Open, |state| {
        state.fingerprint = Some(Fingerprint([0xab; 32]));
        state.config.instance.public_host = "hermes.example.org".into();
    })
    .await;
    let (owner, _, space_id) = server.owner().await;
    let code = invite_code(&server, &owner, &space_id).await;
    let (_, html) = page(&server, &format!("/invite/{code}")).await;
    let fp = format!("sha256:{}", "ab".repeat(32));
    assert!(
        html.contains(&format!(
            r#"href="hermes://hermes.example.org/invite/{code}#fp={fp}""#
        )),
        "{html}"
    );
}

#[tokio::test]
async fn account_invite_and_invalid_invites() {
    let server = start("pages-invalid", RegistrationMode::Open).await;
    // The setup invite is for creating an account, not for a space.
    let (status, html) = page(&server, &format!("/invite/{}", server.setup_code)).await;
    assert_eq!(status, 200);
    assert!(html.contains("Create your account on this Hermes server."));
    assert!(html.contains("<h1>Test Instance</h1>"));

    let (owner, _, space_id) = server.owner().await;
    // Used up now.
    let (status, html) = page(&server, &format!("/invite/{}", server.setup_code)).await;
    assert_eq!(status, 404);
    assert!(html.contains("This invite is not valid"));

    for code in ["nope", "AAAAAAAAAA", "%3Cscript%3E"] {
        let (status, html) = page(&server, &format!("/invite/{code}")).await;
        assert_eq!(status, 404, "{code}");
        assert!(html.contains(DOWNLOAD));
    }

    let code = invite_code(&server, &owner, &space_id).await;
    server
        .sql(&format!(
            "UPDATE invites SET expires_at = 1 WHERE code = '{code}'"
        ))
        .await;
    let (status, _) = page(&server, &format!("/invite/{code}")).await;
    assert_eq!(status, 404);
}
