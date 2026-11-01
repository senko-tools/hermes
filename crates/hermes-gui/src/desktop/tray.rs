//! The tray icon (PLAN.md section 15.7): show/hide, mute, deafen, quit, and a badge dot while
//! something needs attention. The icon is drawn in code from the feather of the header (a mask
//! rendered from the bundled Nerd Font's `md-feather`, `assets/icons/feather-mask.png`).

use std::sync::{Mutex, OnceLock};

use futures_util::Stream;
use tokio::sync::mpsc;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub const ICON_SIZE: u32 = 32;
/// The dark theme's sidebar and accent: a gold feather on a charcoal square.
const BACKGROUND: [u8; 3] = [0x17, 0x16, 0x1c];
const ACCENT: [u8; 3] = [0xe5, 0xa5, 0x50];
const FEATHER_MASK: &[u8] = include_bytes!("../../assets/icons/feather-mask.png");
const BADGE: [u8; 3] = [0xed, 0x42, 0x45];

/// What the tray asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleWindow,
    ToggleMute,
    ToggleDeafen,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    badge: bool,
    mute: CheckMenuItem,
    deafen: CheckMenuItem,
}

/// Menu item IDs.
const SHOW: &str = "show";
const MUTE: &str = "mute";
const DEAFEN: &str = "deafen";
const QUIT: &str = "quit";

/// Tray and menu events, forwarded into a channel the subscription reads once.
static EVENTS: OnceLock<Mutex<Option<mpsc::UnboundedReceiver<TrayAction>>>> = OnceLock::new();

impl Tray {
    /// Creates the tray icon. Must run on the thread with the event loop (the main thread).
    pub fn new() -> Result<Tray, String> {
        let menu = Menu::new();
        let mute = CheckMenuItem::with_id(MUTE, "Mute", true, false, None);
        let deafen = CheckMenuItem::with_id(DEAFEN, "Deafen", true, false, None);
        menu.append_items(&[
            &MenuItem::with_id(SHOW, "Show / hide Hermes", true, None),
            &PredefinedMenuItem::separator(),
            &mute,
            &deafen,
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(QUIT, "Quit Hermes", true, None),
        ])
        .map_err(|e| e.to_string())?;

        let (tx, rx) = mpsc::unbounded_channel();
        let _ = EVENTS.set(Mutex::new(Some(rx)));
        let menu_tx = tx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == MenuId::new(SHOW) {
                Some(TrayAction::ToggleWindow)
            } else if event.id == MenuId::new(MUTE) {
                Some(TrayAction::ToggleMute)
            } else if event.id == MenuId::new(DEAFEN) {
                Some(TrayAction::ToggleDeafen)
            } else if event.id == MenuId::new(QUIT) {
                Some(TrayAction::Quit)
            } else {
                None
            };
            if let Some(action) = action {
                let _ = menu_tx.send(action);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = tx.send(TrayAction::ToggleWindow);
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Hermes")
            .with_icon(tray_icon(false)?)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Tray {
            icon,
            badge: false,
            mute,
            deafen,
        })
    }

    /// The check marks of Mute and Deafen.
    pub fn set_voice(&mut self, muted: bool, deafened: bool) {
        self.mute.set_checked(muted || deafened);
        self.deafen.set_checked(deafened);
    }

    /// Shows or hides the badge dot (only touches the icon when it changes).
    pub fn set_badge(&mut self, badge: bool) {
        if badge == self.badge {
            return;
        }
        self.badge = badge;
        match tray_icon(badge) {
            Ok(icon) => {
                if let Err(err) = self.icon.set_icon(Some(icon)) {
                    tracing::warn!("tray icon not updated: {err}");
                }
            }
            Err(err) => tracing::warn!("tray icon: {err}"),
        }
        let tooltip = if badge {
            "Hermes · unread mentions or messages"
        } else {
            "Hermes"
        };
        let _ = self.icon.set_tooltip(Some(tooltip));
    }
}

/// The tray events, taken once by the subscription.
pub fn events() -> impl Stream<Item = TrayAction> {
    let rx = EVENTS
        .get()
        .and_then(|m| m.lock().expect("tray events lock").take());
    futures_util::stream::unfold(rx, |rx| async move {
        let mut rx = rx?;
        let action = rx.recv().await?;
        Some((action, Some(rx)))
    })
}

fn tray_icon(badge: bool) -> Result<tray_icon::Icon, String> {
    tray_icon::Icon::from_rgba(icon_rgba(ICON_SIZE, badge), ICON_SIZE, ICON_SIZE)
        .map_err(|e| e.to_string())
}

/// The feather's coverage (0–255) at `size`, scaled down from the bundled mask.
fn feather(size: u32) -> image::GrayImage {
    let mask = image::load_from_memory(FEATHER_MASK)
        .expect("the bundled feather mask is a PNG")
        .to_luma_alpha8();
    let alpha = image::GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        image::Luma([mask.get_pixel(x, y)[1]])
    });
    image::imageops::resize(&alpha, size, size, image::imageops::FilterType::Lanczos3)
}

/// The app icon as RGBA pixels: a charcoal rounded square with the gold feather, and a red dot
/// in the top right corner when `badge`.
pub fn icon_rgba(size: u32, badge: bool) -> Vec<u8> {
    let s = size as f32;
    let radius = s * 0.22;
    let feather = feather(size);
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = rounded_square(fx, fy, s, radius);
            let mut px = [0u8; 4];
            if inside {
                let t = f32::from(feather.get_pixel(x, y)[0]) / 255.0;
                let blend = |i: usize| {
                    (f32::from(BACKGROUND[i])
                        + (f32::from(ACCENT[i]) - f32::from(BACKGROUND[i])) * t)
                        .round() as u8
                };
                px = [blend(0), blend(1), blend(2), 255];
            }
            if badge {
                let (cx, cy, r) = (s * 0.78, s * 0.22, s * 0.2);
                let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                if d <= r + 1.0 {
                    px = if d <= r {
                        [BADGE[0], BADGE[1], BADGE[2], 255]
                    } else {
                        [255, 255, 255, 255]
                    };
                }
            }
            let i = ((y * size + x) * 4) as usize;
            out[i..i + 4].copy_from_slice(&px);
        }
    }
    out
}

fn rounded_square(x: f32, y: f32, s: f32, r: f32) -> bool {
    let cx = x.clamp(r, s - r);
    let cy = y.clamp(r, s - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_have_the_right_size_and_the_badge_shows() {
        let plain = icon_rgba(32, false);
        let badged = icon_rgba(32, true);
        assert_eq!(plain.len(), 32 * 32 * 4);
        assert_ne!(plain, badged);
        // Corners are transparent, the middle is not, and the feather is gold.
        assert_eq!(plain[3], 0);
        let mid = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(plain[mid + 3], 255);
        assert!(
            plain.chunks(4).any(|px| px[0] > 0xc0 && px[2] < 0x90),
            "no feather pixels"
        );
        // The badge is red in the top right.
        let dot = ((7 * 32 + 25) * 4) as usize;
        assert_eq!(&badged[dot..dot + 3], &BADGE);
    }

    #[test]
    fn the_installer_version_is_the_crate_version() {
        let config = include_str!("../../Packager.toml");
        let line = format!("version = \"{}\"", env!("CARGO_PKG_VERSION"));
        assert!(
            config.lines().any(|l| l == line),
            "set {line} in crates/hermes-gui/Packager.toml"
        );
    }

    /// Writes `assets/icons/icon-<size>.png`, from which `assets/icons/hermes.ico` (installer,
    /// shortcuts) is made: `magick icon-16.png … icon-256.png hermes.ico`. Run by hand with
    /// `cargo test -p hermes-gui write_icon_files -- --ignored` after changing the icon.
    #[test]
    #[ignore]
    fn write_icon_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
        std::fs::create_dir_all(&dir).unwrap();
        for size in [16, 24, 32, 48, 64, 128, 256, 512] {
            let rgba = icon_rgba(size, false);
            image::RgbaImage::from_raw(size, size, rgba)
                .unwrap()
                .save(dir.join(format!("icon-{size}.png")))
                .unwrap();
        }
    }
}
