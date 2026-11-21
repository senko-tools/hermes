//! `GET /instance`: public information shown when a client adds this instance.

use std::sync::Arc;

use axum::extract::State;
use hermes_proto::PROTOCOL_VERSION;
use hermes_proto::api::{InstanceResponse, InstanceVoice};

use super::Json;
use crate::state::AppState;

pub async fn get(State(state): State<Arc<AppState>>) -> Json<InstanceResponse> {
    let config = &state.config;
    Json(InstanceResponse {
        name: config.instance.name.clone(),
        description: config.instance.description.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        registration_mode: state.registration_mode(),
        limits: config.instance_limits(),
        voice: InstanceVoice {
            enabled: config.voice.enabled,
        },
        features: features(),
    })
}

/// What this server can do beyond protocol 2, for `GET /instance` and `ready`.
pub fn features() -> Vec<String> {
    hermes_proto::compat::features::ALL
        .iter()
        .map(|f| (*f).to_owned())
        .collect()
}
