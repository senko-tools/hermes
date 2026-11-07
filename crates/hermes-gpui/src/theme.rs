//! Themes (PLAN.md section 15.5, extended in docs/DECISIONS.md): TOML files in
//! `<config dir>/themes/`, hot-reloaded, plus the built-ins. A user theme only needs the keys it
//! changes; the others come from `dark`, except the [`DERIVED`] sections, which a file that leaves
//! them out gets from its own older keys (so themes written before they existed look the same).

use std::path::{Path, PathBuf};

use std::cell::RefCell;

use gpui_kit::{Pixels, Rgba, SharedString};
use hermes_client_core::proto::models::Status;
use serde::Deserialize;

pub const DEFAULT: &str = "dark";
pub const THEMES_DIR: &str = "themes";

/// Built-in themes: name and TOML text.
pub const BUILTIN: [(&str, &str); 4] = [
    ("dark", include_str!("../assets/themes/dark.toml")),
    ("light", include_str!("../assets/themes/light.toml")),
    (
        "high-contrast",
        include_str!("../assets/themes/high-contrast.toml"),
    ),
    ("neko", include_str!("../assets/themes/neko.toml")),
];

/// A color written as `#rrggbb` or `#rrggbbaa`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct Hex(pub Rgba);

impl TryFrom<String> for Hex {
    type Error = String;

    fn try_from(value: String) -> Result<Self, String> {
        parse_hex(&value)
            .map(Hex)
            .ok_or_else(|| format!("`{value}` is not a color (use #rrggbb or #rrggbbaa)"))
    }
}

pub fn parse_hex(text: &str) -> Option<Rgba> {
    let hex = text.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16)
            .ok()
            .map(|b| f32::from(b) / 255.0)
    };
    Some(Rgba {
        r: byte(0)?,
        g: byte(2)?,
        b: byte(4)?,
        a: if hex.len() == 8 { byte(6)? } else { 1.0 },
    })
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
    /// The darkest level: the dimmed window behind dialogs.
    pub deep: Hex,
    /// The bottom of the sidebar (voice and user panels).
    pub footer: Hex,
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
    /// Things that stand on their own: buttons, inputs, menus, tooltips.
    pub radius: u8,
    /// Rows in lists (sidebar, menus, settings navigation), which sit right against each other.
    pub row_radius: u8,
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
        overlay_keys(&mut merged, &user, "", &mut warnings);
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
            surface.footer,
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
            shape.row_radius,
            shape.radius_large,
            shape.indicator_width,
        );
    }

    /// The [`DERIVED`] sections as the older keys imply them (how the app looked before they
    /// existed).
    fn derived(&self) -> Theme {
        let none = Hex(TRANSPARENT);
        let accent = self.buttons.primary.background.0;
        let mut d = self.clone();
        d.surface = Surface {
            deep: Hex(mix(self.sidebar.background.0, BLACK, 0.12)),
            footer: self.sidebar.background,
            main: self.general.background,
            sidebar: self.sidebar.background,
            raised: self.text.code_bg,
            overlay: self.sidebar.background,
            hover: self.sidebar.hover,
        };
        d.accent = Accent {
            base: Hex(accent),
            highlight: self.buttons.primary.background_hover,
            muted: Hex(mix(accent, self.general.background.0, 0.35)),
            glow: Hex(alpha(accent, 0.15)),
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
            connected_bg: Hex(alpha(self.voice.speaking_ring.0, 0.12)),
            connected_hover: Hex(alpha(self.voice.speaking_ring.0, 0.2)),
        };
        d.shape = Shape {
            radius: 6,
            row_radius: 6,
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
        clamp(&mut self.shape.row_radius, Shape::MAX_RADIUS, "row_radius");
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
fn overlay_keys(
    base: &mut toml::Table,
    user: &toml::Table,
    path: &str,
    warnings: &mut Vec<String>,
) {
    for (key, value) in user {
        let key_path = join(path, key);
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(u)) => {
                overlay_keys(b, u, &key_path, warnings);
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

const TRANSPARENT: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};
const BLACK: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// `a` blended towards `b` by `t` (0–1).
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// `color` with its opacity scaled.
pub fn alpha(color: Rgba, scale: f32) -> Rgba {
    Rgba {
        a: color.a * scale,
        ..color
    }
}

// ---- what the views use -------------------------------------------------------------------------

/// The family everything is drawn with unless `font.family` says otherwise.
pub const FONT: &str = "JetBrainsMono Nerd Font";
/// `font.size` and the text size drawn differ by this: its default of 13 is the iced client's, and
/// this client draws that at 16 px, close to Discord's.
const FONT_SIZE_OFFSET: f32 = 3.0;
pub const MIN_SCALE: f64 = 0.75;
pub const MAX_SCALE: f64 = 2.0;

/// The smallest window that still fits the rail, a narrow sidebar and a usable chat with its
/// header buttons.
pub const WINDOW_MIN_W: f32 = 720.0;
pub const WINDOW_MIN_H: f32 = 480.0;
/// The icon rail on the far left.
pub const RAIL_WIDTH: f32 = 72.0;
pub const RAIL_ICON: f32 = 48.0;
pub const ROW_HEIGHT: f32 = 34.0;
pub const ROW_ICON: f32 = 20.0;
/// The space name over its channels, and the header over the main panel next to it.
pub const SIDEBAR_HEADER: f32 = 48.0;
pub const AVATAR: f32 = 40.0;
pub const AVATAR_SMALL: f32 = 32.0;

/// The look in use: there is one window, drawn on one thread.
struct Active {
    theme: Theme,
    scale: f32,
    text: f32,
    family: SharedString,
    mono: SharedString,
}

thread_local! {
    static ACTIVE: RefCell<Active> = RefCell::new(Active {
        theme: Theme::dark(),
        scale: 1.0,
        text: 16.0,
        family: FONT.into(),
        mono: FONT.into(),
    });
}

/// Makes `theme`, the fonts and the UI scale the ones everything draws with.
pub fn set_active(theme: Theme, font: &hermes_client_core::config::Font) {
    let family = |name: &str| -> SharedString {
        if name.trim().is_empty() {
            FONT.into()
        } else {
            name.trim().to_owned().into()
        }
    };
    ACTIVE.with(|active| {
        *active.borrow_mut() = Active {
            theme,
            scale: font.ui_scale.clamp(MIN_SCALE, MAX_SCALE) as f32,
            text: f32::from(font.size.clamp(8, 32)) + FONT_SIZE_OFFSET,
            family: family(&font.family),
            mono: family(&font.mono_family),
        };
    });
}

fn with<T>(f: impl FnOnce(&Active) -> T) -> T {
    ACTIVE.with(|active| f(&active.borrow()))
}

/// A length, following the UI scale. Everything in this crate uses this instead of gpui's `px`.
pub fn px(value: f32) -> Pixels {
    gpui_kit::px(value * with(|a| a.scale))
}

/// The UI scale.
pub fn scale() -> f32 {
    with(|a| a.scale)
}

pub fn font() -> SharedString {
    with(|a| a.family.clone())
}

pub fn mono_font() -> SharedString {
    with(|a| a.mono.clone())
}

/// The sizes of text.
pub fn text_size() -> Pixels {
    with(|a| gpui_kit::px(a.text * a.scale))
}
pub fn small_size() -> Pixels {
    with(|a| gpui_kit::px((a.text - 2.0).max(8.0) * a.scale))
}
pub fn tiny_size() -> Pixels {
    with(|a| gpui_kit::px((a.text - 4.0).max(8.0) * a.scale))
}

macro_rules! colors {
    ($($name:ident = |$t:ident| $value:expr;)*) => {
        $(pub fn $name() -> Rgba {
            with(|a| {
                let $t = &a.theme;
                $value
            })
        })*
    };
}

colors! {
    background = |t| t.surface.main.0;
    sidebar = |t| t.surface.sidebar.0;
    rail = |t| t.surface.deep.0;
    footer = |t| t.surface.footer.0;
    raised = |t| t.surface.raised.0;
    overlay = |t| t.surface.overlay.0;
    hover = |t| t.surface.hover.0;
    hover_soft = |t| alpha(t.surface.hover.0, 0.5);
    selected = |t| t.sidebar.selected.0;
    border = |t| t.general.border.0;
    text = |t| t.text.primary.0;
    text_secondary = |t| t.text.secondary.0;
    text_tertiary = |t| t.text.tertiary.0;
    accent = |t| t.accent.base.0;
    accent_soft = |t| t.accent.glow.0;
    on_accent = |t| t.buttons.primary.text.0;
    speaking = |t| t.voice.speaking_ring.0;
    muted_voice = |t| t.voice.muted.0;
    success_soft = |t| alpha(t.text.success.0, 0.25);
    danger = |t| t.text.error.0;
    warning = |t| t.text.warning.0;
    link = |t| t.text.link.0;
    code_background = |t| t.text.code_bg.0;
    mention_soft = |t| t.text.mention_bg.0;
    role_default = |t| t.roles.default_color.0;
    row_selected_bar = |t| t.channel.selected_border.0;
    row_selected_bg = |t| t.channel.selected_bg.0;
    row_selected_hover = |t| t.channel.selected_hover.0;
    row_unread_bar = |t| t.channel.unread_border.0;
    row_unread_bg = |t| t.channel.unread_bg.0;
    row_unread_hover = |t| t.channel.unread_hover.0;
    row_mention_bar = |t| t.channel.mention_border.0;
    row_mention_bg = |t| t.channel.mention_bg.0;
    row_mention_hover = |t| t.channel.mention_hover.0;
    row_connected_bar = |t| t.channel.connected_border.0;
    row_connected_bg = |t| t.channel.connected_bg.0;
    row_connected_hover = |t| t.channel.connected_hover.0;
    scrim = |t| alpha(t.surface.deep.0, 0.7);
    scrim_strong = |t| alpha(t.surface.deep.0, 0.9);
    app_icon_background = |_t| gpui_kit::rgb(0x46424e);
    app_icon_feather = |_t| gpui_kit::rgb(0xe4e4ea);
}

/// The corner radii and the width of a row's state bar.
pub fn row_radius() -> Pixels {
    with(|a| gpui_kit::px(f32::from(a.theme.shape.row_radius) * a.scale))
}
pub fn indicator_width() -> Pixels {
    with(|a| gpui_kit::px(f32::from(a.theme.shape.indicator_width) * a.scale))
}
pub fn radius_large() -> Pixels {
    with(|a| gpui_kit::px(f32::from(a.theme.shape.radius_large) * a.scale))
}

/// A colour given as `0xRRGGBB` (role colours).
pub fn rgb_u32(color: u32) -> Rgba {
    gpui_kit::rgb(color)
}

/// A presence dot.
pub fn status(status: Status) -> Rgba {
    with(|a| match status {
        Status::Online => a.theme.presence.online.0,
        Status::Idle => a.theme.presence.idle.0,
        Status::Dnd => a.theme.presence.dnd.0,
        Status::Offline => a.theme.presence.offline.0,
    })
}

/// Colours the kit's own widgets (buttons, inputs, menus, sliders) from the active theme.
pub fn apply_to_kit(cx: &mut gpui_kit::App) {
    use gpui_kit::component::Theme as KitTheme;
    let (primary, primary_hover, primary_text) = with(|a| {
        (
            a.theme.buttons.primary.background.0,
            a.theme.buttons.primary.background_hover.0,
            a.theme.buttons.primary.text.0,
        )
    });
    let (secondary, secondary_hover, secondary_text) = with(|a| {
        (
            a.theme.buttons.secondary.background.0,
            a.theme.buttons.secondary.background_hover.0,
            a.theme.buttons.secondary.text.0,
        )
    });
    let (danger_bg, danger_hover, danger_text) = with(|a| {
        (
            a.theme.buttons.danger.background.0,
            a.theme.buttons.danger.background_hover.0,
            a.theme.buttons.danger.text.0,
        )
    });
    let theme = KitTheme::global_mut(cx);
    theme.font_family = font();
    theme.font_size = text_size();
    theme.mono_font_family = mono_font();
    theme.background = background().into();
    theme.foreground = text().into();
    theme.primary = primary.into();
    theme.primary_hover = primary_hover.into();
    theme.primary_active = primary_hover.into();
    theme.primary_foreground = primary_text.into();
    theme.secondary = secondary.into();
    theme.secondary_hover = secondary_hover.into();
    theme.secondary_active = secondary_hover.into();
    theme.secondary_foreground = secondary_text.into();
    // Buttons read these token copies, not the plain colours below.
    let token = |color: Rgba| gpui_kit::Hsla::from(color).into();
    theme.tokens.button = token(secondary);
    theme.tokens.button_hover = token(secondary_hover);
    theme.tokens.button_active = token(secondary_hover);
    theme.tokens.button_secondary = token(secondary);
    theme.tokens.button_secondary_hover = token(secondary_hover);
    theme.tokens.button_secondary_active = token(secondary_hover);
    theme.tokens.slider_bar = token(accent());
    theme.tokens.slider_thumb = token(text());
    theme.button_foreground = secondary_text.into();
    theme.button_secondary_foreground = secondary_text.into();
    theme.tokens.button_primary = token(primary);
    theme.tokens.button_primary_hover = token(primary_hover);
    theme.tokens.button_primary_active = token(primary_hover);
    theme.tokens.button_danger = token(danger_bg);
    theme.tokens.button_danger_hover = token(danger_hover);
    theme.tokens.button_danger_active = token(danger_hover);
    theme.button_primary = primary.into();
    theme.button_primary_hover = primary_hover.into();
    theme.button_primary_active = primary_hover.into();
    theme.button_primary_foreground = primary_text.into();
    theme.button_danger = danger_bg.into();
    theme.button_danger_hover = danger_hover.into();
    theme.button_danger_active = danger_hover.into();
    theme.button_danger_foreground = danger_text.into();
    theme.danger = danger_bg.into();
    theme.danger_hover = danger_hover.into();
    theme.danger_active = danger_hover.into();
    theme.danger_foreground = danger_text.into();
    theme.muted = raised().into();
    theme.muted_foreground = text_tertiary().into();
    theme.accent = hover().into();
    theme.accent_foreground = text().into();
    theme.border = border().into();
    theme.input = border().into();
    theme.ring = with(|a| a.theme.accent.focus.0).into();
    theme.selection = accent_soft().into();
    theme.popover = overlay().into();
    theme.popover_foreground = text().into();
    theme.caret = text().into();
    KitTheme::sync_base(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hermes-gpui-{}-{name}", std::process::id()));
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
        assert_eq!(parse_hex("#ff8000"), Some(gpui_kit::rgb(0xff8000)));
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
        assert_eq!(loaded.theme.text.primary.0, gpui_kit::rgb(0xff0000));
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
        // The themes from before these sections write out exactly what derivation gives.
        for (name, text) in BUILTIN.iter().filter(|(name, _)| *name != "neko") {
            let loaded = Theme::parse(name, &old_format(text)).unwrap();
            assert!(loaded.warnings.is_empty(), "{name}: {:?}", loaded.warnings);
            let (old, new) = (loaded.theme, Theme::builtin(name).unwrap());
            // Compared as 8-bit values: the files round what derivation computes.
            assert_eq!(
                format!("{:?}", rgba8(&old)),
                format!("{:?}", rgba8(&new)),
                "{name}"
            );
        }

        let loaded = Theme::parse("mine", "[sidebar]\nbackground = \"#ff0000\"\n").unwrap();
        assert_eq!(loaded.theme.surface.sidebar.0, gpui_kit::rgb(0xff0000));
        assert_eq!(loaded.theme.surface.overlay.0, gpui_kit::rgb(0xff0000));
        let loaded = Theme::parse(
            "mine",
            "[sidebar]\nbackground = \"#ff0000\"\n[surface]\nsidebar = \"#00ff00\"\n",
        )
        .unwrap();
        assert_eq!(loaded.theme.surface.sidebar.0, gpui_kit::rgb(0x00ff00));
    }

    /// Every color of the new sections as 8-bit RGBA, for comparing derived values.
    fn rgba8(theme: &Theme) -> Vec<[u8; 4]> {
        let s = &theme.surface;
        let a = &theme.accent;
        let c = &theme.channel;
        [
            s.deep,
            s.footer,
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
        .map(|h| [h.0.r, h.0.g, h.0.b, h.0.a].map(|v| (v * 255.0).round() as u8))
        .chain(std::iter::once([
            theme.shape.radius,
            theme.shape.radius_large,
            theme.shape.indicator_width,
            theme.shape.row_radius,
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
        assert_eq!(light.theme.general.background.0, gpui_kit::rgb(0x123456));

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
            ["dark", "light", "high-contrast", "neko", "broken"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
