//! REST error codes and the error body (PLAN.md section 10).
//!
//! ```json
//! { "error": { "code": "MISSING_PERMISSION", "message": "…", "retry_after_ms": null } }
//! ```

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    BadRequest,
    /// Comes with a `fields` map (field name → problem).
    ValidationFailed,
    Unauthorized,
    InvalidCredentials,
    AccountDisabled,
    MissingPermission,
    NotFound,
    /// For example "username taken".
    Conflict,
    /// HTTP 429, `retry_after_ms` is set.
    RateLimited,
    PayloadTooLarge,
    StorageFull,
    RegistrationClosed,
    InviteInvalid,
    Banned,
    VoiceFull,
    VoiceDisabled,
    Internal,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 17] = [
        ErrorCode::BadRequest,
        ErrorCode::ValidationFailed,
        ErrorCode::Unauthorized,
        ErrorCode::InvalidCredentials,
        ErrorCode::AccountDisabled,
        ErrorCode::MissingPermission,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::RateLimited,
        ErrorCode::PayloadTooLarge,
        ErrorCode::StorageFull,
        ErrorCode::RegistrationClosed,
        ErrorCode::InviteInvalid,
        ErrorCode::Banned,
        ErrorCode::VoiceFull,
        ErrorCode::VoiceDisabled,
        ErrorCode::Internal,
    ];

    /// The HTTP status the server responds with for this code.
    pub const fn http_status(self) -> u16 {
        match self {
            ErrorCode::BadRequest | ErrorCode::ValidationFailed => 400,
            ErrorCode::Unauthorized | ErrorCode::InvalidCredentials => 401,
            ErrorCode::AccountDisabled
            | ErrorCode::MissingPermission
            | ErrorCode::RegistrationClosed
            | ErrorCode::Banned
            | ErrorCode::VoiceDisabled => 403,
            ErrorCode::NotFound | ErrorCode::InviteInvalid => 404,
            ErrorCode::Conflict | ErrorCode::VoiceFull => 409,
            ErrorCode::PayloadTooLarge => 413,
            ErrorCode::RateLimited => 429,
            ErrorCode::Internal => 500,
            ErrorCode::StorageFull => 507,
        }
    }

    /// The wire name, e.g. `"MISSING_PERMISSION"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "BAD_REQUEST",
            ErrorCode::ValidationFailed => "VALIDATION_FAILED",
            ErrorCode::Unauthorized => "UNAUTHORIZED",
            ErrorCode::InvalidCredentials => "INVALID_CREDENTIALS",
            ErrorCode::AccountDisabled => "ACCOUNT_DISABLED",
            ErrorCode::MissingPermission => "MISSING_PERMISSION",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::Conflict => "CONFLICT",
            ErrorCode::RateLimited => "RATE_LIMITED",
            ErrorCode::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            ErrorCode::StorageFull => "STORAGE_FULL",
            ErrorCode::RegistrationClosed => "REGISTRATION_CLOSED",
            ErrorCode::InviteInvalid => "INVITE_INVALID",
            ErrorCode::Banned => "BANNED",
            ErrorCode::VoiceFull => "VOICE_FULL",
            ErrorCode::VoiceDisabled => "VOICE_DISABLED",
            ErrorCode::Internal => "INTERNAL",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The inner `error` object of an error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub retry_after_ms: Option<u64>,
    /// Only for `VALIDATION_FAILED`: field name → what is wrong with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<BTreeMap<String, String>>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError {
            code,
            message: message.into(),
            retry_after_ms: None,
            fields: None,
        }
    }

    pub fn rate_limited(retry_after_ms: u64) -> Self {
        ApiError {
            retry_after_ms: Some(retry_after_ms),
            ..ApiError::new(ErrorCode::RateLimited, "You are being rate limited")
        }
    }

    pub fn validation(fields: BTreeMap<String, String>) -> Self {
        ApiError {
            fields: Some(fields),
            ..ApiError::new(ErrorCode::ValidationFailed, "Some fields are invalid")
        }
    }

    pub const fn http_status(&self) -> u16 {
        self.code.http_status()
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

/// The full error response body: `{ "error": { … } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

impl From<ApiError> for ErrorBody {
    fn from(error: ApiError) -> Self {
        ErrorBody { error }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_names_match_serde() {
        for code in ErrorCode::ALL {
            let json = serde_json::to_value(code).unwrap();
            assert_eq!(json, json!(code.as_str()));
            assert_eq!(serde_json::from_value::<ErrorCode>(json).unwrap(), code);
        }
    }

    #[test]
    fn rate_limited_is_429() {
        assert_eq!(ErrorCode::RateLimited.http_status(), 429);
        assert_eq!(ApiError::rate_limited(1500).retry_after_ms, Some(1500));
    }

    #[test]
    fn statuses_are_errors() {
        for code in ErrorCode::ALL {
            assert!((400..600).contains(&code.http_status()), "{code}");
        }
    }

    #[test]
    fn body_matches_plan_example() {
        let body = ErrorBody::from(ApiError::new(
            ErrorCode::MissingPermission,
            "You need SEND_MESSAGES in this channel",
        ));
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({ "error": {
                "code": "MISSING_PERMISSION",
                "message": "You need SEND_MESSAGES in this channel",
                "retry_after_ms": null
            }})
        );
    }

    #[test]
    fn validation_fields() {
        let mut fields = BTreeMap::new();
        fields.insert("username".to_owned(), "too short".to_owned());
        let body = ErrorBody::from(ApiError::validation(fields));
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["error"]["code"], "VALIDATION_FAILED");
        assert_eq!(value["error"]["fields"]["username"], "too short");
        assert_eq!(serde_json::from_value::<ErrorBody>(value).unwrap(), body);
    }
}
