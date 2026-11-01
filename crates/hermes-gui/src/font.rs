//! Fonts (PLAN.md section 15.5, default changed in docs/DECISIONS.md): JetBrainsMono Nerd Font
//! (the default for text and code, and the icon font), Inter and JetBrains Mono are bundled; any
//! other `font.family` / `font.mono_family` is looked up among the system fonts by name.

use std::collections::BTreeSet;
use std::sync::Mutex;

use hermes_client_core::config;
use iced::Font;
use iced::font::Weight;

pub const NERD_FONT: &str = "JetBrainsMono Nerd Font";
pub const INTER: &str = "Inter";
pub const JETBRAINS_MONO: &str = "JetBrains Mono";

/// The bundled families, offered first in the font settings.
pub const CHOICES: [&str; 3] = [NERD_FONT, INTER, JETBRAINS_MONO];

/// The bundled font files, loaded at startup.
pub const BUNDLED: [&[u8]; 10] = [
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Medium.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Bold.ttf"),
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Italic.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
];

/// Icons (`widget::icon`) always come from the bundled Nerd Font, whatever `font.family` is.
pub const ICONS: Font = Font::with_name(NERD_FONT);

pub const MIN_SIZE: u16 = 8;
pub const MAX_SIZE: u16 = 32;
pub const MIN_SCALE: f64 = 0.75;
pub const MAX_SCALE: f64 = 2.0;

/// The fonts and text size the views use, from the `[font]` config.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fonts {
    pub ui: Font,
    pub mono: Font,
    /// Base text size in logical pixels.
    pub size: f32,
    /// `font.ui_scale`, on top of the OS DPI.
    pub scale: f32,
}

impl Fonts {
    pub fn from_config(font: &config::Font) -> Fonts {
        Fonts {
            ui: Font::with_name(family_name(&font.family, NERD_FONT)),
            mono: Font::with_name(family_name(&font.mono_family, NERD_FONT)),
            size: f32::from(font.size.clamp(MIN_SIZE, MAX_SIZE)),
            scale: if font.ui_scale.is_finite() {
                font.ui_scale.clamp(MIN_SCALE, MAX_SCALE) as f32
            } else {
                1.0
            },
        }
    }

    pub fn bold(&self) -> Font {
        Font {
            weight: Weight::Bold,
            ..self.ui
        }
    }

    pub fn medium(&self) -> Font {
        Font {
            weight: Weight::Medium,
            ..self.ui
        }
    }

    // The type scale: every text in the app uses one of these sizes (see `widget`).

    /// Captions: timestamps, hints, section titles, badges.
    pub fn caption(&self) -> f32 {
        (self.size - 2.0).max(f32::from(MIN_SIZE))
    }

    /// Titles: the pane header, the space name, the sidebar brand.
    pub fn title(&self) -> f32 {
        self.size + 2.0
    }

    /// Page headings.
    pub fn heading(&self) -> f32 {
        self.size + 5.0
    }

    /// Welcome and empty screens.
    pub fn display(&self) -> f32 {
        self.size + 11.0
    }
}

/// `iced::Font` needs a `&'static str`; each distinct family name is leaked once.
fn family_name(name: &str, default: &'static str) -> &'static str {
    static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let name = name.trim();
    if name.is_empty() || name == default {
        return default;
    }
    let mut names = NAMES.lock().expect("font names lock");
    if let Some(known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::font::Family;

    #[test]
    fn defaults_and_limits() {
        let fonts = Fonts::from_config(&config::Font::default());
        assert_eq!(fonts.ui.family, Family::Name(NERD_FONT));
        assert_eq!(fonts.mono.family, Family::Name(NERD_FONT));
        assert_eq!(fonts.size, 13.0);
        assert_eq!(fonts.scale, 1.0);
        assert_eq!(
            [
                fonts.caption(),
                fonts.title(),
                fonts.heading(),
                fonts.display()
            ],
            [11.0, 15.0, 18.0, 24.0]
        );

        let fonts = Fonts::from_config(&config::Font {
            family: "  ".into(),
            mono_family: "Consolas".into(),
            size: 200,
            ui_scale: 0.1,
        });
        assert_eq!(fonts.ui.family, Family::Name(NERD_FONT));
        assert_eq!(fonts.mono.family, Family::Name("Consolas"));
        assert_eq!(fonts.size, f32::from(MAX_SIZE));
        assert_eq!(fonts.scale, MIN_SCALE as f32);

        let tiny = Fonts::from_config(&config::Font {
            size: 8,
            ..config::Font::default()
        });
        assert_eq!(tiny.caption(), f32::from(MIN_SIZE));
    }

    #[test]
    fn family_names_are_leaked_once() {
        let a = family_name("Segoe UI", INTER);
        let b = family_name("Segoe UI", INTER);
        assert!(std::ptr::eq(a, b));
    }
}
