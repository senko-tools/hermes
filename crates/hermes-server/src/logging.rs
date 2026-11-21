//! The server's log: a base level (`log.level`) plus topics that can be shown or hidden while the
//! server runs (`log.topics`, the console's `log` command).
//!
//! Each topic logs at info level under its own target ([`CONNECTIONS`], [`REQUESTS`], …). The
//! filter is the base level followed by one directive per topic (`hermes::voice=info` or
//! `hermes::voice=off`), so a topic shows or hides whatever the base level says.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, reload};

use crate::config::{LogConfig, LogFormat, LogTopic};

/// Every TCP/TLS connection opened and closed.
pub const CONNECTIONS: &str = "hermes::connections";
/// Every HTTP request.
pub const REQUESTS: &str = "hermes::requests";
/// Gateway sessions.
pub const GATEWAY: &str = "hermes::gateway";
/// Voice joins and leaves.
pub const VOICE: &str = "hermes::voice";
/// Sign-ins and accounts.
pub const AUTH: &str = "hermes::auth";

/// The tracing target of `topic`.
pub fn target(topic: LogTopic) -> &'static str {
    match topic {
        LogTopic::Connections => CONNECTIONS,
        LogTopic::Requests => REQUESTS,
        LogTopic::Gateway => GATEWAY,
        LogTopic::Voice => VOICE,
        LogTopic::Auth => AUTH,
    }
}

/// What is shown now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// A level name or an `EnvFilter` string.
    pub level: String,
    pub topics: BTreeSet<LogTopic>,
}

impl Shown {
    /// The filter: the base level, then each topic on or off.
    pub fn filter(&self) -> Result<EnvFilter, String> {
        let mut directives = self.level.trim().to_owned();
        for topic in LogTopic::ALL {
            let level = if self.topics.contains(&topic) {
                "info"
            } else {
                "off"
            };
            if !directives.is_empty() {
                directives.push(',');
            }
            directives.push_str(&format!("{}={level}", target(topic)));
        }
        EnvFilter::try_new(&directives).map_err(|e| e.to_string())
    }
}

struct Control {
    shown: Shown,
    handle: reload::Handle<EnvFilter, Registry>,
}

static CONTROL: OnceLock<Mutex<Control>> = OnceLock::new();

/// Starts logging to stdout as `log` says. Later calls do nothing.
pub fn init(log: &LogConfig) {
    if CONTROL.get().is_some() {
        return;
    }
    let shown = Shown {
        level: log.level.clone(),
        topics: log.topics.iter().copied().collect(),
    };
    let filter = shown
        .filter()
        .expect("log.level is checked by config validation");
    let (filter, handle) = reload::Layer::new(filter);
    let registry = tracing_subscriber::registry().with(filter);
    let result = match log.format {
        LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer()).try_init(),
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .try_init(),
    };
    if result.is_ok() {
        let _ = CONTROL.set(Mutex::new(Control { shown, handle }));
    }
}

fn control() -> Option<std::sync::MutexGuard<'static, Control>> {
    CONTROL
        .get()
        .map(|c| c.lock().unwrap_or_else(PoisonError::into_inner))
}

/// What is shown now; `None` before [`init`].
pub fn shown() -> Option<Shown> {
    control().map(|c| c.shown.clone())
}

/// Shows exactly `next`, if its level is valid.
pub fn show(next: Shown) -> Result<(), String> {
    let mut control = control().ok_or("logging is not running")?;
    let filter = next.filter()?;
    control.handle.reload(filter).map_err(|e| e.to_string())?;
    control.shown = next;
    Ok(())
}

/// `user_id` for a log line: `name (id)`, or the ID alone for a user the cache does not know.
pub fn who(state: &crate::state::AppState, user_id: hermes_proto::id::Id) -> String {
    match state.cache().users.get(&user_id) {
        Some(user) => format!("{} ({user_id})", user.user.username),
        None => user_id.to_string(),
    }
}

/// `channel_id` for a log line: `#name (id)`, or the ID alone.
pub fn channel(state: &crate::state::AppState, channel_id: hermes_proto::id::Id) -> String {
    match state.cache().channels.get(&channel_id) {
        Some(c) => match c.channel.name.as_deref().filter(|n| !n.is_empty()) {
            Some(name) => format!("#{name} ({channel_id})"),
            None => format!("a call ({channel_id})"),
        },
        None => channel_id.to_string(),
    }
}

/// `d` for the log: seconds, minutes or hours.
pub fn duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min {} s", secs / 60, secs % 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_follow_the_base_level() {
        let shown = Shown {
            level: "warn".into(),
            topics: [LogTopic::Voice].into(),
        };
        let filter = shown.filter().unwrap().to_string();
        assert!(filter.contains("hermes::voice=info"), "{filter}");
        assert!(filter.contains("hermes::requests=off"), "{filter}");
        assert!(filter.contains("warn"), "{filter}");

        let bad = Shown {
            level: "hermes_server=loud".into(),
            topics: BTreeSet::new(),
        };
        assert!(bad.filter().is_err());
    }
}
