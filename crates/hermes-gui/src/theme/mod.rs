//! Themes (PLAN.md section 15.5, extended in docs/DECISIONS.md): TOML files in
//! `<config dir>/themes/`, hot-reloaded, plus the built-ins. A user theme only needs the keys it
//! changes; the others come from `dark`, except the [`DERIVED`] sections, which a file that leaves
//! them out gets from its own older keys (so themes written before they existed look the same).

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
    pub presence: Presence,
    pub roles: Roles,
    pub surface: Surface,
    pub accent: Accent,
    pub icons: Icons,
    pub channel: Channel,
    pub shape: Shape,
}

/// Sections added after the first theme format. Keys a file leaves out are derived from its
/// older keys instead of copied from `dark`, and are not reported as missing.
pub const DERIVED: [&str; 5] = ["surface", "accent", "icons", "channel", "shape"];

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

/// Status dots: online, idle, do not disturb, offline or invisible.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presence {
    pub online: Hex,
    pub idle: Hex,
    pub dnd: Hex,
    pub offline: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    pub default_color: Hex,
}

/// Background levels, darkest to lightest.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    /// Behind the sidebar footer (voice and user panels).
    pub deep: Hex,
    /// Chat and page content.
    pub main: Hex,
    /// Sidebar, member list, settings navigation.
    pub sidebar: Hex,
    /// Inputs, the composer, cards and code.
    pub raised: Hex,
    /// Menus, tooltips and other floating panels.
    pub overlay: Hex,
    /// A hovered row or button.
    pub hover: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accent {
    /// Switches, focus lines, selected icons.
    pub base: Hex,
    /// Brighter: hovered accent things, the space name.
    pub highlight: Hex,
    /// Quieter: secondary accents.
    pub muted: Hex,
    /// A see-through tint for selections.
    pub glow: Hex,
    /// The border of a focused input.
    pub focus: Hex,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Icons {
    pub default: Hex,
    /// Hovered.
    pub active: Hex,
}

/// The states of a sidebar row: the bar on its left edge, its background, and its background
/// while hovered.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub selected_border: Hex,
    pub selected_bg: Hex,
    pub selected_hover: Hex,
    pub unread_border: Hex,
    pub unread_bg: Hex,
    pub unread_hover: Hex,
    pub mention_border: Hex,
    pub mention_bg: Hex,
    pub mention_hover: Hex,
    /// The voice channel or call I am in.
    pub connected_border: Hex,
    pub connected_bg: Hex,
    pub connected_hover: Hex,
}

/// Corner radii and the width of the row state bar, in pixels.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    /// Buttons, inputs, rows.
    pub radius: u8,
    /// Cards, dialogs, the composer.
    pub radius_large: u8,
    pub indicator_width: u8,
}

impl Shape {
    pub const MAX_RADIUS: u8 = 16;
    pub const MAX_RADIUS_LARGE: u8 = 24;
    pub const MAX_INDICATOR: u8 = 6;
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
        theme.derive_missing(&user);
        theme.clamp_shape(&mut warnings);
        Ok(Loaded { theme, warnings })
    }

    /// Fills the [`DERIVED`] keys `user` leaves out from the theme's older keys.
    fn derive_missing(&mut self, user: &toml::Table) {
        let d = self.derived();
        let has = |section: &str, key: &str| {
            user.get(section)
                .and_then(|s| s.as_table())
                .is_some_and(|s| s.contains_key(key))
        };
        macro_rules! fill {
            ($($section:ident . $key:ident),* $(,)?) => {
                $(if !has(stringify!($section), stringify!($key)) {
                    self.$section.$key = d.$section.$key.clone();
                })*
            };
        }
        fill!(
            surface.deep,
            surface.main,
            surface.sidebar,
            surface.raised,
            surface.overlay,
            surface.hover,
            accent.base,
            accent.highlight,
            accent.muted,
            accent.glow,
            accent.focus,
            icons.default,
            icons.active,
            channel.selected_border,
            channel.selected_bg,
            channel.selected_hover,
            channel.unread_border,
            channel.unread_bg,
            channel.unread_hover,
            channel.mention_border,
            channel.mention_bg,
            channel.mention_hover,
            channel.connected_border,
            channel.connected_bg,
            channel.connected_hover,
            shape.radius,
            shape.radius_large,
            shape.indicator_width,
        );
    }

    /// The [`DERIVED`] sections as the older keys imply them (how the app looked before they
    /// existed).
    fn derived(&self) -> Theme {
        let none = Hex(Color::TRANSPARENT);
        let accent = self.buttons.primary.background.0;
        let mut d = self.clone();
        d.surface = Surface {
            deep: Hex(style::mix(self.sidebar.background.0, Color::BLACK, 0.12)),
            main: self.general.background,
            sidebar: self.sidebar.background,
            raised: self.text.code_bg,
            overlay: self.sidebar.background,
            hover: self.sidebar.hover,
        };
        d.accent = Accent {
            base: Hex(accent),
            highlight: self.buttons.primary.background_hover,
            muted: Hex(style::mix(accent, self.general.background.0, 0.35)),
            glow: Hex(accent.scale_alpha(0.15)),
            focus: Hex(accent),
        };
        d.icons = Icons {
            default: self.text.secondary,
            active: self.text.primary,
        };
        d.channel = Channel {
            selected_border: none,
            selected_bg: self.sidebar.selected,
            selected_hover: self.sidebar.selected,
            unread_border: self.general.unread_indicator,
            unread_bg: none,
            unread_hover: self.sidebar.hover,
            mention_border: self.general.unread_indicator,
            mention_bg: none,
            mention_hover: self.sidebar.hover,
            connected_border: self.voice.speaking_ring,
            connected_bg: self.sidebar.selected,
            connected_hover: self.sidebar.selected,
        };
        d.shape = Shape {
            radius: 6,
            radius_large: 10,
            indicator_width: 3,
        };
        d
    }

    fn clamp_shape(&mut self, warnings: &mut Vec<String>) {
        let mut clamp = |value: &mut u8, max: u8, key: &str| {
            if *value > max {
                warnings.push(format!("`shape.{key}` is at most {max} (using {max})"));
                *value = max;
            }
        };
        clamp(&mut self.shape.radius, Shape::MAX_RADIUS, "radius");
        clamp(
            &mut self.shape.radius_large,
            Shape::MAX_RADIUS_LARGE,
            "radius_large",
        );
        clamp(
            &mut self.shape.indicator_width,
            Shape::MAX_INDICATOR,
            "indicator_width",
        );
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

/// Warns about every key of `base` that `user` leaves out ([`DERIVED`] sections excepted).
fn missing(base: &toml::Table, user: &toml::Table, path: &str, warnings: &mut Vec<String>) {
    for (key, value) in base {
        if path.is_empty() && DERIVED.contains(&key.as_str()) {
            continue;
        }
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

    /// `text` without the [`DERIVED`] sections: a theme file from before they existed.
    fn old_format(text: &str) -> String {
        let mut table: toml::Table = toml::from_str(text).unwrap();
        for section in DERIVED {
            table.remove(section);
        }
        toml::to_string(&table).unwrap()
    }

    #[test]
    fn old_theme_files_derive_the_new_sections_from_their_own_keys() {
        for (name, text) in BUILTIN {
            let loaded = Theme::parse(name, &old_format(text)).unwrap();
            assert!(loaded.warnings.is_empty(), "{name}: {:?}", loaded.warnings);
            let (old, new) = (loaded.theme, Theme::builtin(name).unwrap());
            // The built-ins write out what derivation gives, rounded to 8 bits.
            assert_eq!(
                format!("{:?}", rgba8(&old)),
                format!("{:?}", rgba8(&new)),
                "{name}"
            );
        }

        let loaded = Theme::parse("mine", "[sidebar]\nbackground = \"#ff0000\"\n").unwrap();
        assert_eq!(loaded.theme.surface.sidebar.0, Color::from_rgb8(255, 0, 0));
        assert_eq!(loaded.theme.surface.overlay.0, Color::from_rgb8(255, 0, 0));
        let loaded = Theme::parse(
            "mine",
            "[sidebar]\nbackground = \"#ff0000\"\n[surface]\nsidebar = \"#00ff00\"\n",
        )
        .unwrap();
        assert_eq!(loaded.theme.surface.sidebar.0, Color::from_rgb8(0, 255, 0));
    }

    /// Every color of the new sections as 8-bit RGBA, for comparing derived values.
    fn rgba8(theme: &Theme) -> Vec<[u8; 4]> {
        let s = &theme.surface;
        let a = &theme.accent;
        let c = &theme.channel;
        [
            s.deep,
            s.main,
            s.sidebar,
            s.raised,
            s.overlay,
            s.hover,
            a.base,
            a.highlight,
            a.muted,
            a.glow,
            a.focus,
            theme.icons.default,
            theme.icons.active,
            c.selected_border,
            c.selected_bg,
            c.selected_hover,
            c.unread_border,
            c.unread_bg,
            c.unread_hover,
            c.mention_border,
            c.mention_bg,
            c.mention_hover,
            c.connected_border,
            c.connected_bg,
            c.connected_hover,
        ]
        .iter()
        .map(|h| h.0.into_rgba8())
        .chain(std::iter::once([
            theme.shape.radius,
            theme.shape.radius_large,
            theme.shape.indicator_width,
            0,
        ]))
        .collect()
    }

    #[test]
    fn shape_values_are_clamped() {
        let loaded = Theme::parse("x", "[shape]\nradius = 99\nindicator_width = 2\n").unwrap();
        assert_eq!(loaded.theme.shape.radius, Shape::MAX_RADIUS);
        assert_eq!(loaded.theme.shape.indicator_width, 2);
        assert!(
            loaded.warnings.iter().any(|w| w.contains("shape.radius")),
            "{:?}",
            loaded.warnings
        );
        assert!(Theme::parse("x", "[shape]\nradius = -1\n").is_err());
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
