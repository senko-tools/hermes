//! `GET /instance`: public information shown when a client adds this instance.

use std::sync::Arc;

use axum::extract::State;
use hermes_proto::PROTOCOL_VERSION;
use hermes_proto::api::{InstanceResponse, InstanceVoice};
use hermes_proto::gateway::InstanceLimits;

use super::Json;
use crate::state::AppState;

pub async fn get(State(state): State<Arc<AppState>>) -> Json<InstanceResponse> {
    let config = &state.config;
    Json(InstanceResponse {
        name: config.instance.name.clone(),
        description: config.instance.description.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        registration_mode: config.registration.mode,
        limits: InstanceLimits {
            max_upload_mb: config.storage.max_upload_mb,
            max_message_length: config.limits.max_message_length,
        },
        voice: InstanceVoice {
            enabled: config.voice.enabled,
        },
    })
}
