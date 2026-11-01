//! Server configuration: `hermes.toml` plus environment overrides (PLAN.md section 9.2).
//!
//! Sources, from lowest to highest priority:
//! 1. built-in defaults ([`Config::default`]),
//! 2. the TOML file (a missing file is not an error, so a container can use env vars only),
//! 3. `SERVER_PORT` as `network.port` (Pterodactyl/Pelican inject it, PLAN.md section 18.5),
//! 4. `HERMES__<SECTION>__<KEY>` environment variables, e.g. `HERMES__NETWORK__PORT=7700`.
//!
//! So `HERMES__NETWORK__PORT` always wins over `SERVER_PORT`, and `SERVER_PORT` wins over the
//! file: the egg's `init` writes `port = 7700` into the file, and the panel's allocation must
//! still apply.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
pub use hermes_proto::api::RegistrationMode;
use serde::{Deserialize, Serialize};

/// The commented default `hermes.toml` written by `hermes-server init`.
pub const DEFAULT_TEMPLATE: &str = include_str!("default-config.toml");

/// Prefix of the environment variables that override config keys.
pub const ENV_PREFIX: &str = "HERMES__";

/// Port fallback injected by Pterodactyl/Pelican.
pub const SERVER_PORT_ENV: &str = "SERVER_PORT";

/// Opus accepts bitrates from 6 to 510 kbit/s.
const OPUS_BITRATE_RANGE: std::ops::RangeInclusive<u32> = 6_000..=510_000;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub instance: InstanceConfig,
    pub network: NetworkConfig,
    pub tls: TlsConfig,
    pub storage: StorageConfig,
    pub registration: RegistrationConfig,
    pub limits: LimitsConfig,
    pub privacy: PrivacyConfig,
    pub voice: VoiceConfig,
    pub log: LogConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstanceConfig {
    #[serde(deserialize_with = "lossy::string")]
    pub name: String,
    #[serde(deserialize_with = "lossy::string")]
    pub description: String,
    /// Domain or public IP that clients use; needed for correct invite links.
    #[serde(deserialize_with = "lossy::string")]
    pub public_host: String,
    /// 0 = same as `network.port`; see [`Config::public_port`].
    pub public_port: u16,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            name: "Hermes".into(),
            description: String::new(),
            public_host: String::new(),
            public_port: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub bind: IpAddr,
    /// TCP (HTTPS/WSS) and UDP (QUIC voice) both use this port.
    pub port: u16,
    pub trusted_proxies: Vec<IpAddr>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 7700,
            trusted_proxies: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsConfig {
    pub mode: TlsMode,
    /// Empty = not set.
    #[serde(deserialize_with = "lossy::path")]
    pub cert_file: PathBuf,
    /// Empty = not set.
    #[serde(deserialize_with = "lossy::path")]
    pub key_file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    #[default]
    SelfSigned,
    Files,
    /// Plain HTTP/WS, only behind a TLS-terminating reverse proxy.
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    #[serde(deserialize_with = "lossy::path")]
    pub data_dir: PathBuf,
    pub max_upload_mb: u32,
    /// 0 = unlimited.
    pub max_total_storage_gb: u32,
    pub thumbnails: bool,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("./data"),
            max_upload_mb: 25,
            max_total_storage_gb: 0,
            thumbnails: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RegistrationConfig {
    pub mode: RegistrationMode,
    pub min_password_length: u32,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            mode: RegistrationMode::InviteOnly,
            min_password_length: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    /// 0 = only instance admins may create spaces.
    pub max_spaces_per_user: u32,
    pub max_message_length: u32,
    pub max_group_dm_size: u32,
    /// Per user per channel.
    pub messages_per_5s: u32,
    /// Per session, REST.
    pub requests_per_second: u32,
    /// Per IP.
    pub login_attempts_per_minute: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_spaces_per_user: 10,
            max_message_length: 4000,
            max_group_dm_size: 10,
            messages_per_5s: 5,
            requests_per_second: 30,
            login_attempts_per_minute: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrivacyConfig {
    pub dm_policy: DmPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    #[default]
    SharedSpace,
    Anyone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VoiceConfig {
    pub enabled: bool,
    /// Bits/s, per new voice channel.
    pub default_bitrate: u32,
    pub max_bitrate: u32,
    pub max_users_per_room: u32,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_bitrate: 64_000,
            max_bitrate: 128_000,
            max_users_per_room: 25,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    /// A level name or a `tracing_subscriber::EnvFilter` string.
    #[serde(deserialize_with = "lossy::string")]
    pub level: String,
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: LogFormat::Pretty,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

/// A loaded and validated configuration.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    /// Whether the config file existed (if not, only defaults and env vars were used).
    pub file_found: bool,
    /// Problems that do not stop the server, e.g. an empty `instance.public_host`.
    pub warnings: Vec<String>,
}

/// Every problem found while loading or validating, so they can all be fixed in one go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub errors: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, error) in self.errors.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "- {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

/// Loads the config from `path` (if it exists) and the environment, then validates it.
pub fn load(path: &Path) -> Result<Loaded, ConfigError> {
    let file_found = path.exists();
    let server_port = server_port().map_err(|error| ConfigError {
        errors: vec![error],
    })?;
    let config = figment(path, file_found, server_port)
        .extract::<Config>()
        .map_err(|err| ConfigError {
            errors: err.into_iter().map(|e| e.to_string()).collect(),
        })?;
    let (errors, warnings) = config.check();
    if !errors.is_empty() {
        return Err(ConfigError { errors });
    }
    Ok(Loaded {
        config,
        file_found,
        warnings,
    })
}

fn figment(path: &Path, file_found: bool, server_port: Option<u16>) -> Figment {
    let mut figment = Figment::from(Serialized::defaults(Config::default()));
    if file_found {
        // `file_exact`: `Toml::file` would also search parent directories.
        figment = figment.merge(Toml::file_exact(path));
    }
    if let Some(port) = server_port {
        figment = figment.merge(Serialized::default("network.port", port));
    }
    figment.merge(Env::prefixed(ENV_PREFIX).split("__"))
}

/// Reads `SERVER_PORT`; unset or empty means "not given".
fn server_port() -> Result<Option<u16>, String> {
    let Some(value) = std::env::var_os(SERVER_PORT_ENV) else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    match value.parse::<u16>() {
        Ok(port) if port != 0 => Ok(Some(port)),
        _ => Err(format!(
            "{SERVER_PORT_ENV} must be a port number between 1 and 65535, got {value:?}"
        )),
    }
}

impl Config {
    /// The port clients connect to: `instance.public_port`, or `network.port` when that is 0.
    pub fn public_port(&self) -> u16 {
        match self.instance.public_port {
            0 => self.network.port,
            port => port,
        }
    }

    /// Returns `(errors, warnings)`. The server must not start with errors.
    pub fn check(&self) -> (Vec<String>, Vec<String>) {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut at_least_one = |key: &str, value: u32| {
            if value == 0 {
                errors.push(format!("{key} must be at least 1"));
            }
        };
        at_least_one("storage.max_upload_mb", self.storage.max_upload_mb);
        at_least_one(
            "registration.min_password_length",
            self.registration.min_password_length,
        );
        at_least_one("limits.max_message_length", self.limits.max_message_length);
        at_least_one("limits.messages_per_5s", self.limits.messages_per_5s);
        at_least_one(
            "limits.requests_per_second",
            self.limits.requests_per_second,
        );
        at_least_one(
            "limits.login_attempts_per_minute",
            self.limits.login_attempts_per_minute,
        );
        at_least_one("voice.max_users_per_room", self.voice.max_users_per_room);

        if self.instance.name.trim().is_empty() {
            errors.push("instance.name must not be empty".into());
        }
        if self.instance.public_host.trim().is_empty() {
            warnings.push(
                "instance.public_host is empty: invite links will not work for other people \
                 until it is set to the domain or public IP clients use"
                    .into(),
            );
        }
        if self.network.port == 0 {
            errors.push("network.port must be between 1 and 65535".into());
        }

        let cert_set = !self.tls.cert_file.as_os_str().is_empty();
        let key_set = !self.tls.key_file.as_os_str().is_empty();
        match self.tls.mode {
            TlsMode::Files => {
                if !cert_set {
                    errors.push("tls.cert_file must be set when tls.mode = \"files\"".into());
                }
                if !key_set {
                    errors.push("tls.key_file must be set when tls.mode = \"files\"".into());
                }
            }
            TlsMode::SelfSigned | TlsMode::Off if cert_set || key_set => {
                warnings.push(
                    "tls.cert_file and tls.key_file are ignored unless tls.mode = \"files\"".into(),
                );
            }
            TlsMode::SelfSigned | TlsMode::Off => {}
        }

        if self.storage.data_dir.as_os_str().is_empty() {
            errors.push("storage.data_dir must not be empty".into());
        }
        if self.limits.max_group_dm_size < 2 {
            errors.push("limits.max_group_dm_size must be at least 2".into());
        }

        for (key, value) in [
            ("voice.default_bitrate", self.voice.default_bitrate),
            ("voice.max_bitrate", self.voice.max_bitrate),
        ] {
            if !OPUS_BITRATE_RANGE.contains(&value) {
                errors.push(format!(
                    "{key} must be between {} and {} bits/s, got {value}",
                    OPUS_BITRATE_RANGE.start(),
                    OPUS_BITRATE_RANGE.end()
                ));
            }
        }
        if self.voice.default_bitrate > self.voice.max_bitrate {
            errors.push("voice.default_bitrate must not be greater than voice.max_bitrate".into());
        }

        if let Err(err) = tracing_subscriber::EnvFilter::try_new(&self.log.level) {
            errors.push(format!(
                "log.level {:?} is not a valid level or filter: {err}",
                self.log.level
            ));
        }

        (errors, warnings)
    }
}

/// Env var values are parsed like TOML values, so `HERMES__INSTANCE__NAME=1337` arrives as a
/// number. These accept any scalar for string-like fields.
mod lossy {
    use std::fmt;
    use std::path::PathBuf;

    use serde::Deserializer;
    use serde::de::{self, Visitor};

    pub fn string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        deserializer.deserialize_any(LossyString)
    }

    pub fn path<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
        string(deserializer).map(PathBuf::from)
    }

    struct LossyString;

    impl Visitor<'_> for LossyString {
        type Value = String;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_owned())
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_i128<E: de::Error>(self, v: i128) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_u128<E: de::Error>(self, v: u128) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_char<E: de::Error>(self, v: char) -> Result<String, E> {
            Ok(v.to_string())
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::result_large_err,
    reason = "figment::Jail closures must return figment::Result"
)]
mod tests {
    use figment::Jail;

    use super::*;

    const FILE: &str = "hermes.toml";

    /// Runs `f` in a fresh temp dir without inherited `HERMES*` / `SERVER_PORT` variables.
    ///
    /// Other variables stay: the environment is process-wide, and on Windows other tests running
    /// in parallel cannot create sockets while `SystemRoot` is missing (WSA error 10106). So the
    /// environment is only cleared when a config variable is actually set, and everything else
    /// is put back right away; the jail restores all of it afterwards.
    fn jail(f: impl FnOnce(&mut Jail) -> figment::Result<()>) {
        Jail::expect_with(|jail| {
            let is_config = |key: &str| key.starts_with("HERMES") || key == SERVER_PORT_ENV;
            let vars: Vec<(String, String)> = std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect();
            if vars.iter().any(|(key, _)| is_config(key)) {
                jail.clear_env();
                for (key, value) in vars.into_iter().filter(|(key, _)| !is_config(key)) {
                    jail.set_env(key, value);
                }
            }
            f(jail)
        });
    }

    fn load_ok() -> Loaded {
        load(Path::new(FILE)).unwrap_or_else(|err| panic!("config should load:\n{err}"))
    }

    fn load_err() -> Vec<String> {
        load(Path::new(FILE))
            .expect_err("config should fail")
            .errors
    }

    #[test]
    fn template_parses_to_defaults() {
        let parsed: Config = toml::from_str(DEFAULT_TEMPLATE).unwrap();
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn defaults_match_plan() {
        let c = Config::default();
        assert_eq!(c.instance.name, "Hermes");
        assert_eq!(c.network.bind.to_string(), "0.0.0.0");
        assert_eq!(c.network.port, 7700);
        assert_eq!(c.tls.mode, TlsMode::SelfSigned);
        assert_eq!(c.storage.data_dir, PathBuf::from("./data"));
        assert_eq!(c.registration.mode, RegistrationMode::InviteOnly);
        assert_eq!(c.privacy.dm_policy, DmPolicy::SharedSpace);
        assert_eq!(c.voice.default_bitrate, 64_000);
        assert_eq!(c.log.format, LogFormat::Pretty);
    }

    #[test]
    fn missing_file_uses_defaults() {
        jail(|_| {
            let loaded = load_ok();
            assert!(!loaded.file_found);
            assert_eq!(loaded.config, Config::default());
            assert_eq!(loaded.warnings.len(), 1, "public_host warning");
            Ok(())
        });
    }

    #[test]
    fn file_overrides_defaults() {
        jail(|jail| {
            jail.create_file(
                FILE,
                r#"
                [instance]
                name = "Senko"
                public_host = "hermes.senko.tools"
                [network]
                bind = "::"
                port = 7800
                trusted_proxies = ["10.0.0.1"]
                [tls]
                mode = "off"
                "#,
            )?;
            let loaded = load_ok();
            assert!(loaded.file_found);
            assert!(loaded.warnings.is_empty());
            let c = loaded.config;
            assert_eq!(c.instance.name, "Senko");
            assert_eq!(c.network.bind.to_string(), "::");
            assert_eq!(c.network.port, 7800);
            assert_eq!(c.network.trusted_proxies.len(), 1);
            assert_eq!(c.tls.mode, TlsMode::Off);
            // Untouched sections keep their defaults.
            assert_eq!(c.voice, VoiceConfig::default());
            Ok(())
        });
    }

    #[test]
    fn env_overrides_file() {
        jail(|jail| {
            jail.create_file(FILE, "[network]\nport = 7800\n[tls]\nmode = \"off\"")?;
            jail.set_env("HERMES__NETWORK__PORT", 7900);
            jail.set_env("HERMES__TLS__MODE", "files");
            jail.set_env("HERMES__TLS__CERT_FILE", "/etc/hermes/cert.pem");
            jail.set_env("HERMES__TLS__KEY_FILE", "/etc/hermes/key.pem");
            jail.set_env("HERMES__INSTANCE__PUBLIC_HOST", "hermes.senko.tools");
            jail.set_env("HERMES__REGISTRATION__MODE", "open");
            jail.set_env("HERMES__NETWORK__TRUSTED_PROXIES", r#"["10.0.0.1", "::1"]"#);
            let c = load_ok().config;
            assert_eq!(c.network.port, 7900);
            assert_eq!(c.tls.mode, TlsMode::Files);
            assert_eq!(c.tls.cert_file, PathBuf::from("/etc/hermes/cert.pem"));
            assert_eq!(c.instance.public_host, "hermes.senko.tools");
            assert_eq!(c.registration.mode, RegistrationMode::Open);
            assert_eq!(c.network.trusted_proxies.len(), 2);
            Ok(())
        });
    }

    #[test]
    fn env_only_container() {
        jail(|jail| {
            jail.set_env("HERMES__INSTANCE__NAME", "Container");
            jail.set_env("HERMES__STORAGE__MAX_UPLOAD_MB", 100);
            jail.set_env("HERMES__VOICE__ENABLED", false);
            let loaded = load_ok();
            assert!(!loaded.file_found);
            assert_eq!(loaded.config.instance.name, "Container");
            assert_eq!(loaded.config.storage.max_upload_mb, 100);
            assert!(!loaded.config.voice.enabled);
            Ok(())
        });
    }

    #[test]
    fn env_values_that_look_like_numbers_still_fill_strings() {
        jail(|jail| {
            jail.set_env("HERMES__INSTANCE__NAME", 1337);
            jail.set_env("HERMES__INSTANCE__DESCRIPTION", true);
            jail.set_env("HERMES__INSTANCE__PUBLIC_HOST", "203.0.113.7");
            let c = load_ok().config;
            assert_eq!(c.instance.name, "1337");
            assert_eq!(c.instance.description, "true");
            assert_eq!(c.instance.public_host, "203.0.113.7");
            Ok(())
        });
    }

    #[test]
    fn empty_env_value_clears_a_string() {
        jail(|jail| {
            jail.create_file(FILE, "[instance]\npublic_host = \"example.org\"")?;
            jail.set_env("HERMES__INSTANCE__PUBLIC_HOST", "");
            assert_eq!(load_ok().config.instance.public_host, "");
            Ok(())
        });
    }

    #[test]
    fn server_port_overrides_file() {
        jail(|jail| {
            jail.create_file(FILE, "[network]\nport = 7700")?;
            jail.set_env("SERVER_PORT", 25565);
            assert_eq!(load_ok().config.network.port, 25565);
            Ok(())
        });
    }

    #[test]
    fn hermes_port_wins_over_server_port() {
        jail(|jail| {
            jail.set_env("SERVER_PORT", 25565);
            jail.set_env("HERMES__NETWORK__PORT", 7800);
            assert_eq!(load_ok().config.network.port, 7800);
            Ok(())
        });
    }

    #[test]
    fn invalid_server_port_is_an_error() {
        jail(|jail| {
            jail.set_env("SERVER_PORT", 70000);
            let errors = load_err();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("SERVER_PORT"), "{errors:?}");
            Ok(())
        });
    }

    #[test]
    fn empty_server_port_is_ignored() {
        jail(|jail| {
            jail.set_env("SERVER_PORT", "");
            assert_eq!(load_ok().config.network.port, 7700);
            Ok(())
        });
    }

    #[test]
    fn public_port_falls_back_to_network_port() {
        let mut c = Config::default();
        assert_eq!(c.public_port(), 7700);
        c.instance.public_port = 443;
        assert_eq!(c.public_port(), 443);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        jail(|jail| {
            jail.create_file(FILE, "[network]\nprot = 7800")?;
            let errors = load_err();
            assert!(errors[0].contains("prot"), "{errors:?}");
            Ok(())
        });
        jail(|jail| {
            jail.set_env("HERMES__NETWROK__PORT", 7800);
            let errors = load_err();
            assert!(errors[0].contains("netwrok"), "{errors:?}");
            Ok(())
        });
    }

    #[test]
    fn bad_types_are_rejected() {
        jail(|jail| {
            jail.create_file(
                FILE,
                "[network]\nbind = \"localhost\"\n[tls]\nmode = \"letsencrypt\"",
            )?;
            let errors = load_err();
            assert!(!errors.is_empty());
            Ok(())
        });
    }

    #[test]
    fn syntax_errors_are_reported() {
        jail(|jail| {
            jail.create_file(FILE, "[network\nport = 1")?;
            assert!(!load_err().is_empty());
            Ok(())
        });
    }

    #[test]
    fn all_validation_errors_are_collected() {
        jail(|jail| {
            jail.create_file(
                FILE,
                r#"
                [instance]
                name = " "
                [network]
                port = 0
                [tls]
                mode = "files"
                [storage]
                data_dir = ""
                max_upload_mb = 0
                [registration]
                min_password_length = 0
                [limits]
                max_message_length = 0
                max_group_dm_size = 1
                messages_per_5s = 0
                requests_per_second = 0
                login_attempts_per_minute = 0
                [voice]
                default_bitrate = 600000
                max_bitrate = 1000
                max_users_per_room = 0
                [log]
                level = "info,=error"
                "#,
            )?;
            let errors = load_err();
            for key in [
                "instance.name",
                "network.port",
                "tls.cert_file",
                "tls.key_file",
                "storage.data_dir",
                "storage.max_upload_mb",
                "registration.min_password_length",
                "limits.max_message_length",
                "limits.max_group_dm_size",
                "limits.messages_per_5s",
                "limits.requests_per_second",
                "limits.login_attempts_per_minute",
                "voice.default_bitrate must be between",
                "voice.max_bitrate must be between",
                "voice.default_bitrate must not be greater",
                "voice.max_users_per_room",
                "log.level",
            ] {
                assert!(
                    errors.iter().any(|e| e.contains(key)),
                    "no error for {key}: {errors:#?}"
                );
            }
            assert_eq!(errors.len(), 17, "{errors:#?}");
            Ok(())
        });
    }

    #[test]
    fn tls_files_with_paths_is_valid() {
        jail(|jail| {
            jail.create_file(
                FILE,
                "[tls]\nmode = \"files\"\ncert_file = \"c.pem\"\nkey_file = \"k.pem\"",
            )?;
            assert_eq!(load_ok().config.tls.mode, TlsMode::Files);
            Ok(())
        });
    }

    #[test]
    fn unused_tls_paths_warn() {
        jail(|jail| {
            jail.create_file(FILE, "[tls]\ncert_file = \"c.pem\"")?;
            let warnings = load_ok().warnings;
            assert!(
                warnings.iter().any(|w| w.contains("ignored")),
                "{warnings:?}"
            );
            Ok(())
        });
    }

    #[test]
    fn effective_config_round_trips_through_toml() {
        let mut c = Config::default();
        c.network.trusted_proxies = vec!["10.0.0.1".parse().unwrap()];
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), c);
    }
}
