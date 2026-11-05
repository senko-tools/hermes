//! Client configuration (PLAN.md section 17): `config.toml` in the config directory, or next to
//! the executable in portable mode. Unknown keys are warnings, not errors. Changes made by the
//! clients go through `toml_edit`, so comments in the file are kept.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::address::HostPort;

pub const CONFIG_FILE: &str = "config.toml";
pub const KNOWN_HOSTS_FILE: &str = "known_hosts.toml";
pub const TOKENS_FILE: &str = "tokens.toml";

/// Where the client keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub dir: PathBuf,
    /// `config.toml` was found next to the executable.
    pub portable: bool,
}

impl ConfigPaths {
    /// Portable mode if `config.toml` exists next to the executable, otherwise
    /// `%APPDATA%\Hermes`, `~/.config/hermes` or `~/Library/Application Support/Hermes`.
    pub fn detect() -> Option<Self> {
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            && exe_dir.join(CONFIG_FILE).is_file()
        {
            return Some(ConfigPaths {
                dir: exe_dir,
                portable: true,
            });
        }
        let base = directories::BaseDirs::new()?;
        let name = if cfg!(any(windows, target_os = "macos")) {
            "Hermes"
        } else {
            "hermes"
        };
        Some(ConfigPaths {
            dir: base.config_dir().join(name),
            portable: false,
        })
    }

    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        ConfigPaths {
            dir: dir.into(),
            portable: false,
        }
    }

    pub fn config(&self) -> PathBuf {
        self.dir.join(CONFIG_FILE)
    }

    pub fn known_hosts(&self) -> PathBuf {
        self.dir.join(KNOWN_HOSTS_FILE)
    }

    pub fn tokens(&self) -> PathBuf {
        self.dir.join(TOKENS_FILE)
    }

    pub fn user_volumes(&self) -> PathBuf {
        self.dir.join(crate::volumes::USER_VOLUMES_FILE)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub general: General,
    pub font: Font,
    pub sidebar: Sidebar,
    pub buffer: Buffer,
    pub voice: Voice,
    pub notifications: Notifications,
    /// GUI keybinds, action = "key combo".
    pub keys: BTreeMap<String, String>,
    pub tui: Tui,
    pub instances: Vec<InstanceEntry>,
    /// Saved pane layout (managed by the GUI).
    pub layout: Vec<toml::Table>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            general: General::default(),
            font: Font::default(),
            sidebar: Sidebar::default(),
            buffer: Buffer::default(),
            voice: Voice::default(),
            notifications: Notifications::default(),
            keys: [
                ("command_bar", "Ctrl+K"),
                ("toggle_mute", "Ctrl+Shift+M"),
                ("toggle_deafen", "Ctrl+Shift+D"),
            ]
            .into_iter()
            .map(|(a, k)| (a.to_owned(), k.to_owned()))
            .collect(),
            tui: Tui::default(),
            instances: Vec::new(),
            layout: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub theme: String,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub check_for_updates: bool,
}

impl Default for General {
    fn default() -> Self {
        General {
            theme: "dark".into(),
            start_minimized: false,
            close_to_tray: true,
            check_for_updates: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Font {
    pub family: String,
    pub mono_family: String,
    pub size: u16,
    pub ui_scale: f64,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            family: "JetBrainsMono Nerd Font".into(),
            mono_family: "JetBrainsMono Nerd Font".into(),
            size: 13,
            ui_scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidebar {
    pub position: String,
    pub width: u16,
    pub show_voice_users: bool,
    /// Width of the member list (desktop client).
    pub member_list_width: u16,
}

impl Default for Sidebar {
    fn default() -> Self {
        Sidebar {
            position: "left".into(),
            width: 240,
            show_voice_users: true,
            member_list_width: 232,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Buffer {
    pub show_avatars: bool,
    pub timestamp_format: String,
    pub group_minutes: u32,
    pub enter_sends: bool,
    pub image_previews: bool,
    pub show_typing: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer {
            show_avatars: true,
            timestamp_format: "%H:%M".into(),
            group_minutes: 7,
            enter_sends: true,
            image_previews: true,
            show_typing: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Voice {
    pub join_on: String,
    pub input_device: String,
    pub output_device: String,
    pub input_volume: f64,
    pub output_volume: f64,
    pub mode: String,
    pub ptt_key: String,
    pub ptt_release_delay_ms: u32,
    pub vad_sensitivity: i32,
    pub noise_suppression: bool,
    pub auto_gain: bool,
    pub sounds: bool,
    pub sounds_volume: f64,
    /// The screen to share (its system name); empty = the primary one.
    pub share_screen: String,
    /// The largest size a share is sent at: `720p`, `1080p`, `1440p`, `2160p` or `native`.
    pub share_resolution: String,
    /// Frames per second a share is sent at (at most 60).
    pub share_fps: u32,
    /// What a share keeps when it cannot keep up: `smoothness` (the frame rate), `mixed` or
    /// `clarity` (sharp, full-size pictures).
    pub share_preference: String,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            join_on: "double_click".into(),
            input_device: "default".into(),
            output_device: "default".into(),
            input_volume: 1.0,
            output_volume: 1.0,
            mode: "voice_activity".into(),
            ptt_key: String::new(),
            ptt_release_delay_ms: 150,
            vad_sensitivity: -50,
            noise_suppression: true,
            auto_gain: false,
            sounds: true,
            sounds_volume: 0.6,
            share_screen: String::new(),
            share_resolution: "1080p".into(),
            share_fps: 60,
            share_preference: "smoothness".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub enabled: bool,
    pub sound: bool,
    pub show_preview: bool,
    pub flash_taskbar: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications {
            enabled: true,
            sound: true,
            show_preview: true,
            flash_taskbar: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tui {
    /// Normal-mode keys, action = "key". Missing actions use the TUI's defaults.
    pub keys: BTreeMap<String, String>,
}

/// One saved instance. The token is not stored here (keyring).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceEntry {
    /// `host`, plus `:port` when not 7700.
    pub id: String,
    pub url: HostPort,
    pub username: String,
    #[serde(default = "yes")]
    pub autoconnect: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug)]
pub enum ConfigError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, String),
    /// `set` was given an unknown key or a value of the wrong type.
    InvalidSetting(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(path, err) => write!(f, "{}: {err}", path.display()),
            ConfigError::Parse(path, err) => write!(f, "{}: {err}", path.display()),
            ConfigError::InvalidSetting(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A loaded config and the warnings found while loading it.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: ClientConfig,
    pub warnings: Vec<String>,
}

impl ClientConfig {
    /// Parses config text; unknown keys become warnings.
    pub fn parse(text: &str) -> Result<Loaded, String> {
        let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let config: ClientConfig = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())?;
        let mut warnings = Vec::new();
        let defaults = toml::Value::try_from(ClientConfig::default()).expect("defaults serialize");
        unknown_keys(&toml::Value::Table(table), &defaults, "", &mut warnings);
        Ok(Loaded { config, warnings })
    }

    /// Loads `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Loaded, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|e| ConfigError::Parse(path.to_owned(), e)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
                config: ClientConfig::default(),
                warnings: Vec::new(),
            }),
            Err(err) => Err(ConfigError::Io(path.to_owned(), err)),
        }
    }

    pub fn instance(&self, id: &str) -> Option<&InstanceEntry> {
        self.instances.iter().find(|i| i.id == id)
    }
}

/// Tables whose keys are free-form.
const OPEN_TABLES: [&str; 2] = ["keys", "tui.keys"];

fn unknown_keys(value: &toml::Value, known: &toml::Value, path: &str, out: &mut Vec<String>) {
    if OPEN_TABLES.contains(&path) || path == "layout" {
        return;
    }
    match (value, known) {
        (toml::Value::Table(table), toml::Value::Table(known)) => {
            for (key, child) in table {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match known.get(key) {
                    Some(known_child) => unknown_keys(child, known_child, &child_path, out),
                    None => out.push(format!("unknown key `{child_path}` (ignored)")),
                }
            }
        }
        (toml::Value::Array(items), _) if path == "instances" => {
            let known = toml::Value::try_from(InstanceEntry {
                id: String::new(),
                url: HostPort {
                    host: "x".into(),
                    port: 1,
                },
                username: String::new(),
                autoconnect: true,
            })
            .expect("instance serializes");
            for item in items {
                unknown_keys(item, &known, "instances[]", out);
            }
        }
        _ => {}
    }
}

/// Writes `config.toml`, changing only what differs, so comments and formatting stay.
pub fn save(path: &Path, config: &ClientConfig) -> Result<(), ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(ConfigError::Io(path.to_owned(), err)),
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigError::Parse(path.to_owned(), e.to_string()))?;
    // `toml` writes nested structs as `[section]` tables, like a hand-written file.
    let new: toml_edit::DocumentMut = toml::to_string_pretty(config)
        .map_err(|e| ConfigError::InvalidSetting(e.to_string()))?
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigError::InvalidSetting(e.to_string()))?;
    merge(doc.as_table_mut(), new.as_table());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(parent.to_owned(), e))?;
    }
    write_atomic(path, doc.to_string().as_bytes()).map_err(|e| ConfigError::Io(path.to_owned(), e))
}

/// Copies `new` into `doc`: changed values are replaced (keeping their decor), arrays of tables
/// are replaced whole. Keys missing from `new` are left alone (unknown keys stay in the file).
fn merge(doc: &mut toml_edit::Table, new: &toml_edit::Table) {
    for (key, item) in new.iter() {
        match (doc.get_mut(key), item) {
            (Some(toml_edit::Item::Table(existing)), toml_edit::Item::Table(new_table)) => {
                merge(existing, new_table);
            }
            (Some(toml_edit::Item::Value(existing)), toml_edit::Item::Value(new_value)) => {
                if existing.to_string().trim() != new_value.to_string().trim() {
                    let decor = existing.decor().clone();
                    *existing = new_value.clone();
                    *existing.decor_mut() = decor;
                }
            }
            (Some(existing), _) => {
                if existing.to_string() != item.to_string() {
                    *existing = item.clone();
                }
            }
            (None, toml_edit::Item::ArrayOfTables(a)) if a.is_empty() => {}
            (None, toml_edit::Item::Value(toml_edit::Value::Array(a))) if a.is_empty() => {}
            (None, _) => {
                doc.insert(key, item.clone());
            }
        }
    }
}

/// Changes one setting given as a dotted key (`buffer.group_minutes`) and a TOML value
/// (`10`, `true`, `"text"`; anything that does not parse is taken as a string). Returns the new
/// config, or an error if the key is unknown or the value has the wrong type.
pub fn set_value(
    config: &ClientConfig,
    key: &str,
    value: &str,
) -> Result<ClientConfig, ConfigError> {
    let mut root = toml::Value::try_from(config).expect("config serializes");
    let parsed = toml::from_str::<toml::Table>(&format!("v = {value}"))
        .ok()
        .and_then(|mut t| t.remove("v"))
        .unwrap_or_else(|| toml::Value::String(value.to_owned()));
    let parts: Vec<&str> = key.split('.').collect();
    let open = OPEN_TABLES
        .iter()
        .any(|t| key.starts_with(t) && key.len() > t.len() && key.as_bytes()[t.len()] == b'.');
    let mut node = &mut root;
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let table = node
            .as_table_mut()
            .ok_or_else(|| ConfigError::InvalidSetting(format!("`{key}` is not a setting")))?;
        if last {
            if !table.contains_key(*part) && !open {
                return Err(ConfigError::InvalidSetting(format!(
                    "unknown setting `{key}`"
                )));
            }
            if table.get(*part).is_some_and(toml::Value::is_table) {
                return Err(ConfigError::InvalidSetting(format!("`{key}` is a section")));
            }
            table.insert((*part).to_owned(), parsed.clone());
            break;
        }
        node = table
            .get_mut(*part)
            .ok_or_else(|| ConfigError::InvalidSetting(format!("unknown setting `{key}`")))?;
    }
    root.try_into()
        .map_err(|e: toml::de::Error| ConfigError::InvalidSetting(format!("`{key}`: {e}")))
}

/// Writes through a temporary file and a rename, so a crash never leaves half a file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Watches `config.toml` and sends the newly loaded config on every change (debounced by
/// 250 ms). Returns the watcher, which must be kept alive.
pub fn watch(
    path: PathBuf,
    tx: mpsc::UnboundedSender<Result<Loaded, ConfigError>>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::Watcher;

    let (raw_tx, mut raw_rx) = mpsc::unbounded_channel();
    let file_name = path.file_name().map(|n| n.to_owned());
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res
            && (event.kind.is_modify() || event.kind.is_create())
            && event
                .paths
                .iter()
                .any(|p| p.file_name() == file_name.as_deref())
        {
            let _ = raw_tx.send(());
        }
    })?;
    let dir = path.parent().unwrap_or(Path::new(".")).to_owned();
    std::fs::create_dir_all(&dir).map_err(notify::Error::io)?;
    watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
    tokio::spawn(async move {
        let mut last: Option<String> = std::fs::read_to_string(&path).ok();
        while raw_rx.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(250)).await;
            while raw_rx.try_recv().is_ok() {}
            let text = std::fs::read_to_string(&path).ok();
            if text == last {
                continue;
            }
            last = text;
            if tx.send(ClientConfig::load(&path)).is_err() {
                break;
            }
        }
    });
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_plan() {
        let c = ClientConfig::default();
        assert_eq!(c.general.theme, "dark");
        assert_eq!(c.buffer.group_minutes, 7);
        assert_eq!(c.voice.vad_sensitivity, -50);
        assert_eq!(c.keys["command_bar"], "Ctrl+K");
        // Round trip.
        let text = toml::to_string(&c).unwrap();
        assert_eq!(ClientConfig::parse(&text).unwrap().config, c);
    }

    #[test]
    fn unknown_keys_are_warnings() {
        let loaded = ClientConfig::parse(
            r#"
            colour = "red"
            [buffer]
            group_minutes = 3
            groop = 1
            [keys]
            anything = "F1"
            [tui.keys]
            join_voice = "J"
            [[instances]]
            id = "example.com"
            url = "https://example.com:7700"
            username = "alice"
            token = "should not be here"
            "#,
        )
        .unwrap();
        assert_eq!(loaded.config.buffer.group_minutes, 3);
        assert_eq!(loaded.config.instances[0].url.host, "example.com");
        assert!(loaded.config.instances[0].autoconnect);
        assert_eq!(
            loaded.warnings,
            [
                "unknown key `buffer.groop` (ignored)",
                "unknown key `colour` (ignored)",
                "unknown key `instances[].token` (ignored)",
            ]
        );
    }

    #[test]
    fn wrong_types_are_errors() {
        assert!(ClientConfig::parse("[buffer]\ngroup_minutes = \"seven\"").is_err());
    }

    #[test]
    fn set_value_checks_keys_and_types() {
        let c = ClientConfig::default();
        let c = set_value(&c, "buffer.group_minutes", "10").unwrap();
        assert_eq!(c.buffer.group_minutes, 10);
        let c = set_value(&c, "general.theme", "light").unwrap();
        assert_eq!(c.general.theme, "light");
        let c = set_value(&c, "tui.keys.join_voice", "j").unwrap();
        assert_eq!(c.tui.keys["join_voice"], "j");
        assert!(set_value(&c, "buffer.nope", "1").is_err());
        assert!(set_value(&c, "buffer", "1").is_err());
        assert!(set_value(&c, "buffer.group_minutes", "many").is_err());
    }

    #[test]
    fn save_keeps_comments_and_unknown_keys() {
        let dir = std::env::temp_dir().join(format!("hermes-cc-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(CONFIG_FILE);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            "# my settings\n[buffer]\ngroup_minutes = 3 # short groups\nfuture_key = 1\n",
        )
        .unwrap();
        let mut c = ClientConfig::load(&path).unwrap().config;
        c.buffer.group_minutes = 5;
        c.instances.push(InstanceEntry {
            id: "example.com".into(),
            url: HostPort::parse("example.com").unwrap(),
            username: "alice".into(),
            autoconnect: true,
        });
        save(&path, &c).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("group_minutes = 5 # short groups"), "{text}");
        assert!(text.contains("future_key = 1"), "{text}");
        assert!(text.contains("[[instances]]"), "{text}");
        assert_eq!(ClientConfig::load(&path).unwrap().config, c);

        c.instances.clear();
        save(&path, &c).unwrap();
        assert!(
            ClientConfig::load(&path)
                .unwrap()
                .config
                .instances
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
