//! REST request and response bodies (PLAN.md section 10) that are not plain models.

use serde::{Deserialize, Serialize};

use crate::id::Id;
use crate::models::CurrentUser;

/// `POST /auth/register`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_code: Option<String>,
    pub device_name: String,
}

/// `POST /auth/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub device_name: String,
}

/// Response of register and login. `token` is sent as `Authorization: Bearer <token>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: CurrentUser,
}

/// One entry of `GET /auth/sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: Id,
    pub device_name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    /// The session making the request.
    pub current: bool,
}

/// `POST /users/@me/password`. Revokes all other sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// `POST /spaces/{id}/invites`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInviteRequest {
    /// `None` or `0` = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    /// `None` = 7 days, `0` = never expires, at most 30 days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
}

impl CreateInviteRequest {
    /// Lifetime used when `expires_in_secs` is absent.
    pub const DEFAULT_EXPIRES_IN_SECS: u64 = 7 * 24 * 60 * 60;
    /// Longest lifetime other than "never".
    pub const MAX_EXPIRES_IN_SECS: u64 = 30 * 24 * 60 * 60;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::models::User;

    #[test]
    fn register_invite_code_is_optional() {
        let req: RegisterRequest = serde_json::from_value(json!({
            "username": "senko", "password": "hunter22", "device_name": "PC"
        }))
        .unwrap();
        assert_eq!(req.invite_code, None);
        assert!(
            serde_json::to_value(&req)
                .unwrap()
                .get("invite_code")
                .is_none()
        );
    }

    #[test]
    fn auth_response_flattens_the_user() {
        let res = AuthResponse {
            token: "hms_x".into(),
            user: CurrentUser {
                user: User {
                    id: Id(5),
                    username: "senko".into(),
                    display_name: None,
                    avatar: None,
                    status_text: None,
                },
                instance_admin: true,
            },
        };
        let value = serde_json::to_value(&res).unwrap();
        assert_eq!(value["user"]["id"], "5");
        assert_eq!(value["user"]["instance_admin"], true);
        assert_eq!(serde_json::from_value::<AuthResponse>(value).unwrap(), res);
    }

    #[test]
    fn create_invite_defaults() {
        let req: CreateInviteRequest = serde_json::from_value(json!({})).unwrap();
        assert_eq!(req, CreateInviteRequest::default());
        assert_eq!(serde_json::to_value(&req).unwrap(), json!({}));
    }
}
