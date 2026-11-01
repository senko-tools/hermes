//! Small open animations: menus drop in, dialogs and the settings page slide in over a fading
//! backdrop, notifications rise from the bottom. Each is a `bool` [`Animation`] that goes to
//! `true` when its overlay appears; closing is immediate (and resets it for the next time).

use iced::Animation;
use iced::animation::Easing;
use iced::time::{Duration, Instant};

const MENU_MS: u64 = 140;
const DIALOG_MS: u64 = 160;
const PAGE_MS: u64 = 180;
const TOAST_MS: u64 = 220;

fn fresh(ms: u64) -> Animation<bool> {
    Animation::new(false)
        .duration(Duration::from_millis(ms))
        .easing(Easing::EaseOutCubic)
}

#[derive(Debug, Clone)]
pub struct Anims {
    pub menu: Animation<bool>,
    pub dialog: Animation<bool>,
    pub page: Animation<bool>,
    pub toast: Animation<bool>,
}

impl Default for Anims {
    fn default() -> Self {
        Anims {
            menu: fresh(MENU_MS),
            dialog: fresh(DIALOG_MS),
            page: fresh(PAGE_MS),
            toast: fresh(TOAST_MS),
        }
    }
}

/// What is open, to start the animation of whatever just appeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Open {
    pub menu: bool,
    pub dialog: bool,
    pub page: bool,
    pub toast: bool,
}

impl Anims {
    /// Starts the animation of everything open in `now` but not in `before`; what closed is
    /// reset so it animates again next time.
    pub fn sync(&mut self, before: Open, now: Open, at: Instant) {
        let slots = [
            (&mut self.menu, before.menu, now.menu, MENU_MS),
            (&mut self.dialog, before.dialog, now.dialog, DIALOG_MS),
            (&mut self.page, before.page, now.page, PAGE_MS),
            (&mut self.toast, before.toast, now.toast, TOAST_MS),
        ];
        for (anim, was, is, ms) in slots {
            match (was, is) {
                (false, true) => anim.go_mut(true, at),
                (true, false) => *anim = fresh(ms),
                _ => {}
            }
        }
    }

    pub fn animating(&self, at: Instant) -> bool {
        [&self.menu, &self.dialog, &self.page, &self.toast]
            .iter()
            .any(|a| a.is_animating(at))
    }
}

/// How far `anim` is: 0 just opened, 1 settled.
pub fn progress(anim: &Animation<bool>, at: Instant) -> f32 {
    anim.interpolate(0.0, 1.0, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_animates_and_closing_resets() {
        let t0 = Instant::now();
        let mut anims = Anims::default();
        let open = Open {
            menu: true,
            ..Open::default()
        };
        anims.sync(Open::default(), open, t0);
        assert!(anims.animating(t0 + Duration::from_millis(10)));
        assert!(progress(&anims.menu, t0) < 0.01);
        let settled = t0 + Duration::from_millis(500);
        assert!(!anims.animating(settled));
        assert!((progress(&anims.menu, settled) - 1.0).abs() < 1e-3);
        anims.sync(open, Open::default(), settled);
        assert!(progress(&anims.menu, settled) < 0.01);
    }
}
