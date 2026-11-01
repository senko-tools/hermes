//! Themes (PLAN.md section 15.5): TOML files in `<config dir>/themes/`, hot-reloaded, plus the
//! built-in `dark`, `light` and `high-contrast`. A user theme only needs the keys it changes;
//! the others come from `dark`.

pub mod style;

use std::path::{Path, PathBuf};

use iced::Color;
use serde::Deserialize;

pub const DEFAULT: &str = "dark";
pub const THEMES_DIR: &str = "themes";

/// Built-in themes: name and TOML text.
pub const BUILTIN: [(&str, &str); 3] = [
    ("dark", include_str!("../../assets/themes/dark.toml")),
    ("light", include_str!("../../assets/themes/light.toml")),
    (
        "high-contrast",
        include_str!("../../assets/themes/high-contrast.toml"),
    ),
];

/// A color written as `#rrggbb` or `#rrggbbaa`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct Hex(pub Color);

impl TryFrom<String> for Hex {
    type Error = String;

    fn try_from(value: String) -> Result<Self, String> {
        parse_hex(&value)
            .map(Hex)
            .ok_or_else(|| format!("`{value}` is not a color (use #rrggbb or #rrggbbaa)"))
    }
}

pub fn parse_hex(text: &str) -> Option<Color> {
    let hex = text.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Color::from_rgba8(
        byte(0)?,
        byte(2)?,
        byte(4)?,
        f32::from(a) / 255.0,
    ))
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    #[serde(skip)]
    pub name: String,
    pub general: General,
    pub text: Text,
    pub buttons: Buttons,
    pub sidebar: Sidebar,
    pub pane: Pane,
    pub voice: Voice,
    pub roles: Roles,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct General {
    pub background: Hex,
    pub border: Hex,
    pub horizontal_rule: Hex,
    pub unread_indicator: Hex,
    pub scrollbar: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub primary: Hex,
    pub secondary: Hex,
    pub tertiary: Hex,
    pub success: Hex,
    pub error: Hex,
    pub warning: Hex,
    pub info: Hex,
    pub link: Hex,
    pub mention_bg: Hex,
    pub code_bg: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Buttons {
    pub primary: Button,
    pub secondary: Button,
    pub danger: Button,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Button {
    pub background: Hex,
    pub background_hover: Hex,
    pub text: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidebar {
    pub background: Hex,
    pub selected: Hex,
    pub hover: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pane {
    pub header_bg: Hex,
    pub focused_border: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Voice {
    pub speaking_ring: Hex,
    pub muted: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    pub default_color: Hex,
}

/// A theme and what was wrong with its file.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub theme: Theme,
    pub warnings: Vec<String>,
}

impl Theme {
    /// The built-in `dark` theme.
    pub fn dark() -> Theme {
        Self::builtin(DEFAULT).expect("dark is built in")
    }

    pub fn builtin(name: &str) -> Option<Theme> {
        let (_, text) = BUILTIN.iter().find(|(n, _)| *n == name)?;
        let mut theme: Theme = toml::from_str(text).expect("built-in themes are valid");
        theme.name = name.to_owned();
        Some(theme)
    }

    /// Parses a theme file. Missing keys use the `dark` values and unknown keys are ignored;
    /// both are reported as warnings.
    pub fn parse(name: &str, text: &str) -> Result<Loaded, String> {
        let user: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut merged: toml::Table = toml::from_str(BUILTIN[0].1).expect("dark is valid");
        let mut warnings = Vec::new();
        overlay(&mut merged, &user, "", &mut warnings);
        let base: toml::Table = toml::from_str(BUILTIN[0].1).expect("dark is valid");
        missing(&base, &user, "", &mut warnings);
        let mut theme: Theme = toml::Value::Table(merged)
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())?;
        theme.name = name.to_owned();
        Ok(Loaded { theme, warnings })
    }

    /// Loads theme `name`: `<themes dir>/<name>.toml` first, then the built-ins. A theme that
    /// cannot be loaded falls back to the built-in of that name, else `dark`, with a warning.
    pub fn load(name: &str, themes_dir: Option<&Path>) -> Loaded {
        let mut warnings = Vec::new();
        if let Some(path) = themes_dir.and_then(|dir| theme_file(dir, name)) {
            match std::fs::read_to_string(&path) {
                Ok(text) => match Theme::parse(name, &text) {
                    Ok(mut loaded) => {
                        for w in &mut loaded.warnings {
                            *w = format!("theme `{name}`: {w}");
                        }
                        return loaded;
                    }
                    Err(err) => warnings.push(format!("theme `{name}` not loaded: {err}")),
                },
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => warnings.push(format!("theme `{name}` not loaded: {err}")),
            }
        }
        let theme = match Theme::builtin(name) {
            Some(theme) => theme,
            None => {
                if warnings.is_empty() {
                    warnings.push(format!("theme `{name}` not found, using `{DEFAULT}`"));
                }
                Theme::dark()
            }
        };
        Loaded { theme, warnings }
    }
}

/// `<dir>/<name>.toml`, or `None` if `name` could leave the directory.
fn theme_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ');
    valid.then(|| dir.join(format!("{name}.toml")))
}

/// Names of every theme: the built-ins, then the files in `themes_dir`, sorted.
pub fn available(themes_dir: Option<&Path>) -> Vec<String> {
    let mut names: Vec<String> = BUILTIN.iter().map(|(n, _)| (*n).to_owned()).collect();
    let mut user: Vec<String> = themes_dir
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "toml" {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_owned();
            theme_file(Path::new(""), &stem).map(|_| stem)
        })
        .filter(|name| !names.contains(name))
        .collect();
    user.sort();
    user.dedup();
    names.append(&mut user);
    names
}

/// Copies the keys of `user` into `base`; keys `base` does not have become warnings.
fn overlay(base: &mut toml::Table, user: &toml::Table, path: &str, warnings: &mut Vec<String>) {
    for (key, value) in user {
        let key_path = join(path, key);
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(u)) => {
                overlay(b, u, &key_path, warnings);
            }
            (Some(toml::Value::Table(_)), _) => {
                warnings.push(format!("`{key_path}` must be a table (ignored)"));
            }
            (Some(slot), _) => *slot = value.clone(),
            (None, _) => warnings.push(format!("unknown key `{key_path}` (ignored)")),
        }
    }
}

/// Warns about every key of `base` that `user` leaves out.
fn missing(base: &toml::Table, user: &toml::Table, path: &str, warnings: &mut Vec<String>) {
    for (key, value) in base {
        let key_path = join(path, key);
        match (value, user.get(key)) {
            (toml::Value::Table(b), Some(toml::Value::Table(u))) => {
                missing(b, u, &key_path, warnings);
            }
            (_, None) => warnings.push(format!("`{key_path}` not set, using the `dark` value")),
            _ => {}
        }
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hermes-gui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtins_parse_with_every_key() {
        for (name, text) in BUILTIN {
            let loaded = Theme::parse(name, text).unwrap();
            assert!(loaded.warnings.is_empty(), "{name}: {:?}", loaded.warnings);
            assert_eq!(loaded.theme, Theme::builtin(name).unwrap());
        }
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex("#ff8000"), Some(Color::from_rgb8(255, 128, 0)));
        let c = parse_hex("#00000080").unwrap();
        assert!((c.a - 128.0 / 255.0).abs() < 1e-6);
        for bad in ["ff8000", "#ff80", "#gg0000", "#ff800000ff", ""] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn partial_theme_uses_dark_values_and_warns() {
        let loaded =
            Theme::parse("mine", "[text]\nprimary = \"#ff0000\"\n[extra]\nx = 1\n").unwrap();
        let dark = Theme::dark();
        assert_eq!(loaded.theme.name, "mine");
        assert_eq!(loaded.theme.text.primary.0, Color::from_rgb8(255, 0, 0));
        assert_eq!(loaded.theme.text.secondary, dark.text.secondary);
        assert_eq!(loaded.theme.sidebar, dark.sidebar);
        assert!(loaded.warnings.iter().any(|w| w.contains("`extra`")));
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.contains("`text.secondary`"))
        );
        assert!(loaded.warnings.iter().any(|w| w.contains("`sidebar`")));
    }

    #[test]
    fn bad_color_is_an_error() {
        let err = Theme::parse("x", "[text]\nprimary = \"red\"\n").unwrap_err();
        assert!(err.contains("not a color"), "{err}");
    }

    #[test]
    fn user_theme_overrides_builtin_and_falls_back() {
        let dir = temp_dir("themes");
        std::fs::write(
            dir.join("light.toml"),
            "[general]\nbackground = \"#123456\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("broken.toml"), "not toml [").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();

        let light = Theme::load("light", Some(&dir));
        assert_eq!(
            light.theme.general.background.0,
            Color::from_rgb8(0x12, 0x34, 0x56)
        );

        let broken = Theme::load("broken", Some(&dir));
        assert_eq!(broken.theme, Theme::dark());
        assert!(
            broken.warnings[0].contains("not loaded"),
            "{:?}",
            broken.warnings
        );

        let unknown = Theme::load("nope", Some(&dir));
        assert_eq!(unknown.theme, Theme::dark());
        assert!(unknown.warnings[0].contains("not found"));

        let escape = Theme::load("../light", Some(&dir));
        assert_eq!(escape.theme, Theme::dark());

        assert_eq!(
            available(Some(&dir)),
            ["dark", "light", "high-contrast", "broken"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
