//! The settings page, shown over the whole window: its tabs in sections (You, App, Voice &
//! video, Advanced), a search over every setting, and long groups that fold. Every change is
//! written to `config.toml`.

use std::cell::{Cell, RefCell};

use gpui_kit::AnimationExt as _;
use gpui_kit::component::IndexPath;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SelectEvent, SelectItem, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div};
use hermes_client_core::audio::{AudioDevices, Sound};
use hermes_client_core::config::{self, ClientConfig};
use hermes_client_core::proto::api::{SessionInfo, UpdatePrivacyRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::DmFrom;
use hermes_client_core::{InstanceId, time};

use crate::assets::{self, icon};
use crate::keybinds::{ACTIONS, KeyAction, Keybinds};
use crate::results::Remote;
use crate::screen::capture::{FRAME_RATES, PREFERENCES, Preference, ShareResolution};
use crate::shell::Shell;
use crate::theme::{self, Theme, px};

/// The level meter and the voice activity slider share this range (dBFS).
pub const METER_MIN: f32 = -70.0;
/// UI scales offered as one-click choices.
const SCALE_PRESETS: [f64; 5] = [0.9, 1.0, 1.1, 1.25, 1.5];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    Account,
    Privacy,
    #[default]
    Appearance,
    Chat,
    Notifications,
    Window,
    Keybinds,
    Voice,
    ScreenShare,
    Diagnostics,
    Admin,
    About,
}

/// A tab in the navigation: the tab, its icon and its name.
type NavTab = (Tab, &'static str, &'static str);

/// The navigation: sections of tabs. About stands apart at the bottom.
const SECTIONS: [(&str, &[NavTab]); 4] = [
    (
        "You",
        &[
            (Tab::Account, assets::ACCOUNT, "Account"),
            (Tab::Privacy, assets::LOCK, "Privacy & safety"),
        ],
    ),
    (
        "App",
        &[
            (Tab::Appearance, assets::PALETTE, "Appearance"),
            (Tab::Chat, assets::CHAT, "Chat"),
            (Tab::Notifications, assets::BELL, "Notifications"),
            (Tab::Window, assets::COG, "Window & startup"),
            (Tab::Keybinds, assets::KEYBOARD, "Keybinds"),
        ],
    ),
    (
        "Voice & video",
        &[
            (Tab::Voice, assets::MIC, "Voice & Audio"),
            (Tab::ScreenShare, assets::MONITOR_SHARE, "Screen sharing"),
        ],
    ),
    (
        "Advanced",
        &[
            (Tab::Diagnostics, assets::PULSE, "Diagnostics"),
            (Tab::Admin, assets::SERVER, "Administration"),
        ],
    ),
];
const ABOUT: NavTab = (Tab::About, assets::INFO, "About");

/// Every tab in the navigation's order, with its name.
fn all_tabs() -> impl Iterator<Item = (Tab, &'static str)> {
    SECTIONS
        .iter()
        .flat_map(|(_, tabs)| tabs.iter())
        .chain(std::iter::once(&ABOUT))
        .map(|(tab, _, name)| (*tab, *name))
}

/// The keybind groups, in the order shown. Pane keys come with split panes.
const KEY_GROUPS: [(&str, &[KeyAction]); 3] = [
    (
        "Navigation",
        &[
            KeyAction::CommandBar,
            KeyAction::Search,
            KeyAction::NextUnread,
            KeyAction::PrevUnread,
            KeyAction::NextChannel,
            KeyAction::PrevChannel,
            KeyAction::MarkRead,
            KeyAction::ToggleMemberList,
            KeyAction::ToggleSidebar,
        ],
    ),
    ("Voice", &[KeyAction::ToggleMute, KeyAction::ToggleDeafen]),
    (
        "App",
        &[
            KeyAction::Settings,
            KeyAction::ZoomIn,
            KeyAction::ZoomOut,
            KeyAction::ZoomReset,
        ],
    ),
];

/// What a slider of the page changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SliderKind {
    FontSize,
    UiScale,
    GroupMinutes,
    InputVolume,
    OutputVolume,
    Vad,
    VadHold,
    AgcTarget,
    Ducking,
    ReleaseDelay,
    SoundsVolume,
    MediaVolume,
    ScrollSpeed,
}

/// How often the level meter can redraw the window at most. Every frame redraws the whole window,
/// so this is not the display's rate: drawing at 144 Hz all the time cost about 10 % CPU and GPU.
const METER_FPS: f32 = 60.0;
/// How often the Voice tab lists the audio devices again while it shows.
const DEVICE_POLL: std::time::Duration = std::time::Duration::from_secs(3);
/// The bar has to move this far for a redraw (under a pixel on a setting row), so a settled
/// meter costs nothing.
const METER_STEP_DB: f32 = 0.1;

/// Where the level meter's bar goes next: it follows the level quickly upwards and falls slowly, as
/// meters do, in dB and `seconds` after it was last drawn.
pub fn meter_step(shown: f32, level: f32, seconds: f32) -> f32 {
    const RISE: f32 = 0.03;
    const FALL: f32 = 0.25;
    let time = if level > shown { RISE } else { FALL };
    shown + (level - shown) * (1.0 - (-seconds / time).exp())
}

#[cfg(test)]
#[test]
fn the_meter_rises_quickly_and_falls_slowly() {
    let frame = 1.0 / 60.0;
    let rise = meter_step(-60.0, -20.0, frame) + 60.0;
    let fall = -20.0 - meter_step(-20.0, -60.0, frame);
    assert!(rise > 4.0 * fall, "rise {rise}, fall {fall}");
    assert!((meter_step(-30.0, -30.0, frame) + 30.0).abs() < 1e-6);
    assert!((meter_step(-60.0, -20.0, 10.0) + 20.0).abs() < 0.01);
}

/// A choice of the audio device dropdowns; `value` is what the config stores.
#[derive(Clone)]
pub(crate) struct DeviceChoice {
    pub(crate) value: String,
    pub(crate) name: SharedString,
}

/// A switch row for a `bool` in the config, naming the field once so what the switch shows and
/// what it changes cannot differ: `flag!(self, cx, "Name", "Hint", buffer.compact)`.
macro_rules! flag {
    ($shell:expr, $cx:expr, $name:expr, $hint:expr, $($field:ident).+) => {
        $shell.flag(
            $name,
            $hint,
            $shell.config.$($field).+,
            stringify!($($field).+),
            |c: &mut ClientConfig, on: bool| c.$($field).+ = on,
            $cx,
        )
    };
}

// After `flag!`: a macro is only seen by what comes after it.
mod about_tab;
mod account_tab;
mod appearance_tab;
mod chat_tab;
mod notifications_tab;
mod page;
mod privacy_tab;
mod render;
mod search;
mod security_tab;
mod voice_tab;
mod window_tab;

impl SelectItem for DeviceChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }
}

/// The installed fonts, with the one in use even if it is not installed any more.
fn font_choices(names: &[String], current: &str) -> (Vec<DeviceChoice>, usize) {
    let mut all: Vec<String> = names.to_vec();
    if !all.iter().any(|n| n == current) {
        all.insert(0, current.to_owned());
    }
    let selected = all.iter().position(|n| n == current).unwrap_or(0);
    let choices = all
        .into_iter()
        .map(|name| DeviceChoice {
            name: SharedString::from(name.clone()),
            value: name,
        })
        .collect();
    (choices, selected)
}

/// The choices for the device saved as `current`: the system default, what is plugged in, and the
/// saved device even when it is not there, so the dropdown says what is really set.
fn device_choices(names: &[String], current: &str) -> (Vec<DeviceChoice>, usize) {
    let current = if current.is_empty() {
        "default"
    } else {
        current
    };
    let mut choices = vec![DeviceChoice {
        value: "default".to_owned(),
        name: "System default".into(),
    }];
    choices.extend(names.iter().map(|name| DeviceChoice {
        value: name.clone(),
        name: name.clone().into(),
    }));
    if !choices.iter().any(|c| c.value == current) {
        choices.push(DeviceChoice {
            value: current.to_owned(),
            name: format!("{current} (not connected)").into(),
        });
    }
    let selected = choices
        .iter()
        .position(|c| c.value == current)
        .unwrap_or_default();
    (choices, selected)
}

/// The controls of the settings page that hold state of their own.
pub struct SettingsPage {
    pub tab: Tab,
    pub account_inst: Option<InstanceId>,
    pub sessions: Remote<Vec<SessionInfo>>,
    /// Counts the session list loads; only the newest one's answer is shown.
    sessions_load: u64,
    /// The action waiting for a key press, by its config name.
    pub capturing_key: Option<&'static str>,
    /// Whether this account has two-factor login, once known.
    two_factor: Option<bool>,
    admin_users: Remote<Vec<hermes_client_core::proto::api::AdminUser>>,
    admin_log: Remote<Vec<hermes_client_core::proto::api::AdminLogEntry>>,
    admin_settings: Option<hermes_client_core::proto::api::AdminSettings>,
    /// The audio device dropdowns, once the Voice & Audio tab has listed the devices.
    device_selects: Option<DeviceSelects>,
    /// The audio devices are being listed (a click or the timer then does not list them again).
    devices_listing: bool,
    /// Counts the times the Voice tab opened; a refresh timer of an older opening stops.
    device_poll: u64,
    /// The screen dropdown, once the screens have been listed.
    screen_select: Option<(Entity<SelectState<Vec<DeviceChoice>>>, Subscription)>,
    pub mic_test: bool,
    /// Held while the Voice tab and its level meter show.
    fine_timers: Option<crate::desktop::window::FineTimers>,
    font: Entity<SelectState<Vec<DeviceChoice>>>,
    mono_font: Entity<SelectState<Vec<DeviceChoice>>>,
    /// The date formats, today's date in each, and Custom ([`CUSTOM_DATE`]).
    date_select: Entity<SelectState<Vec<DeviceChoice>>>,
    time_format: Entity<InputState>,
    date_format: Entity<InputState>,
    /// Custom was chosen for the time or date: its pattern field shows.
    custom_time: bool,
    custom_date: bool,
    /// Searching every setting by name and hint.
    search: Entity<InputState>,
    ptt_key: Entity<InputState>,
    sliders: Vec<(SliderKind, Entity<SliderState>)>,
    _events: Vec<Subscription>,
}

struct DeviceSelects {
    input: Entity<SelectState<Vec<DeviceChoice>>>,
    output: Entity<SelectState<Vec<DeviceChoice>>>,
    /// What they were made from: a listing that finds the same changes nothing.
    listed: AudioDevices,
    _events: [Subscription; 2],
}

/// The date dropdown's value for Custom: no pattern is empty.
const CUSTOM_DATE: &str = "";

/// The date dropdown's choices for `current`, and which one holds.
fn date_choices(current: &str, utc_offset: i64) -> (Vec<DeviceChoice>, usize) {
    let mut choices: Vec<DeviceChoice> = DATE_FORMATS
        .iter()
        .map(|f| DeviceChoice {
            value: (*f).to_owned(),
            name: time::format(hermes_client_core::now_ms(), f, utc_offset).into(),
        })
        .collect();
    choices.push(DeviceChoice {
        value: CUSTOM_DATE.to_owned(),
        name: "Custom…".into(),
    });
    let selected = DATE_FORMATS
        .iter()
        .position(|f| *f == current)
        .unwrap_or(DATE_FORMATS.len());
    (choices, selected)
}

/// The date formats offered on the day dividers.
const DATE_FORMATS: [&str; 5] = [
    "%B %e, %Y",
    "%e %B %Y",
    "%A, %e %B %Y",
    "%Y-%m-%d",
    "%d.%m.%Y",
];

/// The two settings that are a time pattern: message times and the day lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Time,
    Date,
}

/// The time patterns offered as chips.
const TIME_FORMATS: [(&str, &str); 2] = [("%H:%M", "24 hour"), ("%I:%M %p", "12 hour")];

impl Shell {
    /// "Time" or "Date on day lines": preset chips and Custom, which shows the pattern field
    /// under them. A preset also goes into the field, so the two never disagree (#11).
    fn pattern_row(
        &self,
        page: &SettingsPage,
        which: Pattern,
        hint: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let b = &self.config.buffer;
        let (name, current, input, custom_chosen) = match which {
            Pattern::Time => (
                "Time",
                &b.timestamp_format,
                &page.time_format,
                page.custom_time,
            ),
            Pattern::Date => (
                "Date on day lines",
                &b.date_format,
                &page.date_format,
                page.custom_date,
            ),
        };
        let presets: Vec<(String, String)> = match which {
            Pattern::Time => TIME_FORMATS
                .iter()
                .map(|(f, n)| ((*f).to_owned(), (*n).to_owned()))
                .collect(),
            Pattern::Date => DATE_FORMATS
                .iter()
                .map(|f| {
                    let shown = time::format(hermes_client_core::now_ms(), f, self.utc_offset);
                    ((*f).to_owned(), shown)
                })
                .collect(),
        };
        let custom = custom_chosen || !presets.iter().any(|(f, _)| f == current);
        let id = match which {
            Pattern::Time => "time-format",
            Pattern::Date => "date-format",
        };
        // Picking a preset puts it in the field too; Custom opens the field under them.
        let formats: Vec<String> = presets.iter().map(|(f, _)| f.clone()).collect();
        let chosen = if custom {
            Some(presets.len())
        } else {
            presets.iter().position(|(f, _)| f == current)
        };
        let pick =
            move |this: &mut Shell, n: usize, window: &mut Window, cx: &mut Context<Shell>| {
                match formats.get(n) {
                    Some(format) => this.set_pattern(which, format.clone(), false, window, cx),
                    None => {
                        if let Some(page) = this.settings.as_mut() {
                            match which {
                                Pattern::Time => page.custom_time = true,
                                Pattern::Date => page.custom_date = true,
                            }
                        }
                        cx.notify();
                    }
                }
            };
        // Times are short: side by side. Dates are long: one under the other.
        let picker: AnyElement = match which {
            Pattern::Time => crate::ui::segmented(
                self,
                id,
                presets
                    .into_iter()
                    .map(|(_, shown)| crate::ui::Segment::new(shown))
                    .chain([crate::ui::Segment::new("Custom")])
                    .collect(),
                chosen,
                true,
                pick,
                cx,
            )
            .into_any_element(),
            // Dates are long: a dropdown (made in `open_settings`), not a row of them.
            Pattern::Date => crate::ui::dropdown(&page.date_select, crate::ui::DropWidth::Medium)
                .into_any_element(),
        };
        let field = custom.then(|| {
            let help = match which {
                Pattern::Time => "%H hours, %M minutes, %I and %p for 12 hours. Enter saves.",
                Pattern::Date => "%e day, %B month, %A weekday, %Y year. Enter saves.",
            };
            crate::channel::grow_in(
                &self.grow_heights,
                SharedString::from(format!("{id}-custom")),
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .pt(px(8.))
                            .w(px(300.))
                            .child(Input::new(input))
                            .child(muted(help)),
                    )
                    .into_any_element(),
            )
        });
        setting(
            name,
            hint,
            div()
                .flex()
                .flex_col()
                .items_end()
                .child(picker)
                .children(field),
        )
    }

    /// Saves a time or date pattern and shows it in the pattern field; `custom` keeps the
    /// field open (it came from the field itself).
    fn set_pattern(
        &mut self,
        which: Pattern,
        format: String,
        custom: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = self.settings.as_mut() {
            let input = match which {
                Pattern::Time => {
                    page.custom_time = custom;
                    page.time_format.clone()
                }
                Pattern::Date => {
                    page.custom_date = custom;
                    page.date_format.clone()
                }
            };
            if !custom {
                let shown = format.clone();
                input.update(cx, |input, cx| input.set_value(shown, window, cx));
            }
        }
        self.set_config(
            |c| match which {
                Pattern::Time => c.buffer.timestamp_format = format,
                Pattern::Date => c.buffer.date_format = format,
            },
            cx,
        );
    }
}

/// A tab's name in the navigation.
fn tab_name(tab: Tab) -> &'static str {
    all_tabs()
        .find(|(t, _)| *t == tab)
        .map_or("", |(_, name)| name)
}

/// A setting as the search finds it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Found {
    pub group: String,
    /// The group's place among the page's groups, for scrolling to it.
    pub index: usize,
    pub name: String,
    pub hint: String,
    /// The row sits in a group's "More" part, which opens under this key.
    pub more: Option<&'static str>,
    /// The row to light when it is a part of another row (a cell of a table of switches).
    pub lit: Option<String>,
}

/// What [`setting`] and [`group_with`] noted while a tab was built for the search.
#[derive(Default)]
struct Recording {
    /// Rows of the group being built (its rows come before its title).
    rows: Vec<Noted>,
    groups: usize,
    found: Vec<Found>,
}

/// A row noted for the search, before its group's title is known.
struct Noted {
    name: String,
    hint: String,
    more: Option<&'static str>,
    lit: Option<String>,
}

impl Recording {
    fn row(&mut self, name: &str, hint: &str, lit: Option<&str>) {
        self.rows.push(Noted {
            name: name.to_owned(),
            hint: hint.to_owned(),
            more: IN_MORE.with(Cell::get),
            lit: lit.map(str::to_owned),
        });
    }

    fn group(&mut self, title: &str) {
        let index = self.groups;
        self.groups += 1;
        for row in self.rows.drain(..) {
            self.found.push(Found {
                group: title.to_owned(),
                index,
                name: row.name,
                hint: row.hint,
                more: row.more,
                lit: row.lit,
            });
        }
    }
}

thread_local! {
    /// Set while the search notes every tab's rows.
    static RECORDING: RefCell<Option<Recording>> = const { RefCell::new(None) };
    /// The row the search went to, lit for a moment.
    static LIT: RefCell<Option<(String, std::time::Instant)>> = const { RefCell::new(None) };
    /// The settings page's scroll; the search moves it to a group.
    static PAGE_SCROLL: gpui_kit::ScrollHandle = gpui_kit::ScrollHandle::new();
    /// The group a page scrolls to when it is next drawn.
    static SCROLL_TO: Cell<Option<usize>> = const { Cell::new(None) };
    /// The "More" part whose rows are being built, for the search.
    static IN_MORE: Cell<Option<&'static str>> = const { Cell::new(None) };
}

fn recording() -> bool {
    RECORDING.with(|r| r.borrow().is_some())
}

/// How long a row found by the search stays lit.
const LIT_FOR: std::time::Duration = std::time::Duration::from_millis(1800);

/// Starts a page at its top (a new tab does not keep the last one's scroll).
pub(crate) fn reset_page_scroll() {
    PAGE_SCROLL.with(|h| h.set_offset(gpui_kit::point(px(0.), px(0.))));
}

/// A section's caption in the navigation.
fn nav_section(title: &str) -> AnyElement {
    div()
        .flex_shrink_0()
        .px(px(8.))
        .pt(px(12.))
        .pb(px(4.))
        .text_size(theme::caption_size())
        .text_color(theme::text_tertiary())
        .child(title.to_uppercase())
        .into_any_element()
}

// ---- parts, also used by the space settings ---------------------------------------------------

/// Chips side by side, wrapping; they line up on the right, where a setting's control goes.
pub fn wrap() -> gpui_kit::Div {
    div().flex().flex_wrap().justify_end().w_full().gap(px(4.))
}

pub fn muted(text: impl Into<SharedString>) -> AnyElement {
    div()
        .text_size(theme::secondary_size())
        .text_color(theme::text_tertiary())
        .child(text.into())
        .into_any_element()
}

/// A page over the whole window: navigation, content and a way back.
pub fn frame(
    title: String,
    nav: Vec<AnyElement>,
    content: AnyElement,
    close: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id("page")
        .flex()
        .flex_1()
        .min_h_0()
        .size_full()
        .child(
            div()
                .id("page-nav")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .gap(px(2.))
                .w(px(240.))
                .h_full()
                .p(px(12.))
                .overflow_y_scroll()
                .bg(theme::sidebar())
                // The whole line goes back, not just the arrow.
                .child(
                    div()
                        .id("page-close")
                        .group("page-close")
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.))
                        .h(px(30.))
                        .mb(px(8.))
                        .px(px(6.))
                        .cursor_pointer()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .hover(|d| d.text_color(theme::accent()))
                        .child(
                            icon(assets::ARROW_LEFT, 18., theme::text_tertiary())
                                .group_hover("page-close", |s| s.text_color(theme::accent())),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .on_click(close),
                )
                .children(nav),
        )
        .child(div().flex_1().min_w_0().h_full().child(content))
        .into_any_element()
}

/// A tab's content: title, optional text and groups, scrolling.
pub fn page_body(title: &str, text: Option<&str>, groups: Vec<AnyElement>) -> AnyElement {
    page_body_with(title, None, text, groups)
}

/// [`page_body`] with buttons on the right of the title ("Refresh", "Create invite…").
///
/// Every part sits in one column at most [`PAGE_W`] wide, centered in the space beside the
/// navigation; a narrow window gets what there is. Each part gets its own box (not one wrapper)
/// so the search can still scroll to the n-th group.
pub fn page_body_with(
    title: &str,
    actions: Option<AnyElement>,
    text: Option<&str>,
    groups: Vec<AnyElement>,
) -> AnyElement {
    let scroll = PAGE_SCROLL.with(|h| h.clone());
    if !recording()
        && let Some(index) = SCROLL_TO.with(Cell::take)
    {
        // The title and the text come before the groups.
        scroll.scroll_to_item(index + 1 + usize::from(text.is_some()));
    }
    div()
        .id("page-body")
        .track_scroll(&scroll)
        .flex()
        .flex_col()
        // Sections stand further apart than the rows inside them.
        .gap(px(SECTION_GAP))
        .size_full()
        .p(px(32.))
        .overflow_y_scroll()
        .child(page_part(title_bar(
            div()
                .text_size(theme::heading_size())
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(title.to_owned()))
                .into_any_element(),
            actions,
        )))
        .children(text.map(|t| page_part(muted(SharedString::from(t.to_owned())))))
        .children(groups.into_iter().map(page_part))
        .into_any_element()
}

/// How far apart a page's sections stand.
pub const SECTION_GAP: f32 = 32.;

/// A page's (or an item's) title with its buttons on the right. The title keeps its own width
/// and only truncates once the buttons need the room: a wide box of buttons must never squeeze
/// it to a letter per line.
pub fn title_bar(title: AnyElement, actions: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(crate::ui::control_h())
        .child(div().flex_1().min_w(px(120.)).truncate().child(title))
        .children(actions.map(|a| {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .child(a)
        }))
        .into_any_element()
}

/// How wide a page's column may grow: wide enough to use the window, narrow enough that a
/// row's name and its control do not drift apart.
pub const PAGE_W: f32 = 960.;

/// One part of a page in the centered column.
pub fn page_part(part: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .w_full()
        .max_w(px(PAGE_W))
        .self_center()
        .child(part)
        .into_any_element()
}

/// Something at its own width, centered under a card ("Load older entries").
pub fn centered(part: impl IntoElement) -> AnyElement {
    div().flex().justify_center().child(part).into_any_element()
}

pub fn group(title: &str, rows: Vec<AnyElement>) -> AnyElement {
    section(title, "", rows)
}

/// A titled group of settings, with something (a button) on the right of its title.
pub fn group_with(title: &str, actions: AnyElement, rows: Vec<AnyElement>) -> AnyElement {
    section_with(title, "", Some(actions), rows)
}

/// A section: its name, and a short description under it, above its card of rows.
pub fn section(title: &str, description: &str, rows: Vec<AnyElement>) -> AnyElement {
    section_with(title, description, None, rows)
}

/// [`section`] with something (a button) on the right of its name.
pub fn section_with(
    title: &str,
    description: &str,
    actions: Option<AnyElement>,
    rows: Vec<AnyElement>,
) -> AnyElement {
    let header = section_header(title, description, actions);
    group_column(
        header,
        (!rows.is_empty()).then(|| group_card(rows).into_any_element()),
    )
}

/// A section's name and description, with its buttons on the right.
fn section_header(title: &str, description: &str, actions: Option<AnyElement>) -> gpui_kit::Div {
    RECORDING.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.group(title);
        }
    });
    div()
        .flex()
        .items_end()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(theme::body_size())
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(title.to_owned())),
                )
                .when(!description.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(theme::secondary_size())
                            .text_color(theme::text_tertiary())
                            .child(SharedString::from(description.to_owned())),
                    )
                }),
        )
        .children(actions.map(|a| div().flex_shrink_0().child(a)))
}

/// A group's rows on their card, a thin line between each two.
pub(crate) fn group_card(rows: Vec<AnyElement>) -> gpui_kit::Div {
    let count = rows.len();
    let mut children: Vec<AnyElement> = Vec::with_capacity(count * 2);
    for (n, row) in rows.into_iter().enumerate() {
        children.push(row);
        if n + 1 < count {
            children.push(
                div()
                    .h(px(1.))
                    .flex_shrink_0()
                    .bg(theme::border())
                    .into_any_element(),
            );
        }
    }
    div()
        .flex()
        .flex_col()
        .px(px(16.))
        .py(px(6.))
        .rounded(theme::radius_large())
        .border_1()
        .border_color(theme::border())
        .bg(theme::sidebar())
        .children(children)
}

/// The header over the card (no card while it has no rows).
fn group_column(header: gpui_kit::Div, card: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(10.))
        .w_full()
        .child(header)
        .children(card)
        .into_any_element()
}

/// How long a hint may be and still show in full under its setting; a longer one shows its
/// start and the rest in the (i) next to the name.
const HINT_CHARS: usize = 72;

/// Where a row's control goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// On the right, in a column at least 240 px wide (text fields and sliders without a width
    /// of their own).
    Column,
    /// On the right, as wide as the control is: switches, short choices, sized dropdowns.
    Fit,
    /// Under the name and hint, across the whole card: wide sliders, tables of switches.
    Below,
}

/// A row: the name and a one-line hint on the left, the control on the right. A hint too long
/// for one line puts all of it in the (i) beside the name.
pub fn setting(name: &str, hint: &str, control: impl IntoElement) -> AnyElement {
    setting_at(Place::Column, name, hint, control)
}

/// [`setting`] with the control on the right at its own width (#64): a switch, a few choices,
/// or a dropdown or slider given a width that fits what it holds ([`sized`]).
pub fn setting_fit(name: &str, hint: &str, control: impl IntoElement) -> AnyElement {
    setting_at(Place::Fit, name, hint, control)
}

/// [`setting`] with the control under the name and hint, as wide as the card (#64): a volume
/// slider, a table of switches.
pub fn setting_below(name: &str, hint: &str, control: impl IntoElement) -> AnyElement {
    setting_at(Place::Below, name, hint, control)
}

/// Two [`setting_below`] side by side on one row ("Input volume" and "Output volume"); a card
/// too narrow for both puts the second under the first.
pub fn setting_pair(first: AnyElement, second: AnyElement) -> AnyElement {
    let cell = |row: AnyElement| {
        div()
            .flex_grow_1()
            .flex_basis(px(0.))
            .min_w(px(PAIR_CELL_MIN_W))
            .child(row)
    };
    div()
        .flex()
        .flex_wrap()
        .gap_x(px(24.))
        .child(cell(first))
        .child(cell(second))
        .into_any_element()
}

/// How narrow one half of a [`setting_pair`] may get before they stack.
const PAIR_CELL_MIN_W: f32 = 240.;

/// The space above and under a row's content.
const ROW_PAD_Y: f32 = 12.;

/// Notes for the search a setting that is a part of the row named `row` (a cell of a table of
/// switches): finding it lights that row. Draws nothing.
pub fn note_part(name: &str, hint: &str, row: &str) {
    RECORDING.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.row(name, hint, Some(row));
        }
    });
}

/// A control `width` px wide, for [`setting_fit`]: sliders and dropdowns take their parent's
/// width.
pub fn sized(width: f32, control: impl IntoElement) -> AnyElement {
    div()
        .w(px(width))
        .max_w_full()
        .child(control)
        .into_any_element()
}

/// A row with its name and hint, a control on the right of them, and more across the card under
/// them: "Voice activity sensitivity" with Automatic gain beside it and the level bar under it.
pub fn setting_split(
    name: &str,
    hint: &str,
    control: impl IntoElement,
    below: impl IntoElement,
) -> AnyElement {
    setting_at_with(
        Place::Fit,
        name,
        hint,
        control,
        Some(below.into_any_element()),
    )
}

fn setting_at(place: Place, name: &str, hint: &str, control: impl IntoElement) -> AnyElement {
    setting_at_with(place, name, hint, control, None)
}

fn setting_at_with(
    place: Place,
    name: &str,
    hint: &str,
    control: impl IntoElement,
    below: Option<AnyElement>,
) -> AnyElement {
    RECORDING.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.row(name, hint, None);
        }
    });
    // Just found by the search: it glows and fades back.
    let lit = LIT.with(|l| {
        l.borrow()
            .as_ref()
            .is_some_and(|(found, at)| found == name && at.elapsed() < LIT_FOR)
    });
    let long = hint.chars().count() > HINT_CHARS;
    let full_hint = SharedString::from(hint.to_owned());
    let info = long.then(|| {
        let tip = full_hint.clone();
        div()
            .id(SharedString::from(format!("hint-{name}")))
            .flex_shrink_0()
            .child(icon(assets::INFO, 14., theme::text_tertiary()))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
    });
    let text = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(theme::body_size())
                .child(SharedString::from(name.to_owned()))
                .children(info),
        )
        .when(!hint.is_empty(), |d| {
            d.child(
                div()
                    .text_size(theme::secondary_size())
                    .text_color(theme::text_tertiary())
                    .truncate()
                    .child(full_hint),
            )
        });
    let row = match place {
        Place::Below => div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .py(px(ROW_PAD_Y))
            .child(text)
            .child(div().w_full().child(control)),
        Place::Column | Place::Fit => div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .when(below.is_none(), |d| d.py(px(ROW_PAD_Y)))
            .child(text.flex_1().min_w(px(220.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .flex_shrink_0()
                    // At least wide enough for a slider, which has no width of its own.
                    .when(place == Place::Column, |d| d.min_w(px(240.)))
                    .max_w_full()
                    .child(control),
            ),
    };
    let row = match below {
        Some(below) => div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .py(px(ROW_PAD_Y))
            .child(row)
            .child(div().w_full().child(below)),
        None => row,
    };
    if !lit {
        return row.into_any_element();
    }
    row.rounded(theme::radius_small())
        .with_animation(
            SharedString::from(format!("lit-{name}")),
            gpui_kit::Animation::new(LIT_FOR).with_easing(gpui_kit::ease_in_out),
            |d, t| d.bg(theme::alpha(theme::accent(), 0.22 * (1. - t))),
        )
        .into_any_element()
}

/// A rounded bar standing in for a line of text in a theme's miniature.
fn preview_bar(width: f32, height: f32, color: gpui_kit::Rgba) -> gpui_kit::Div {
    div()
        .flex_shrink_0()
        .w(px(width))
        .h(px(height))
        .rounded(px(height / 2.))
        .bg(color)
}

/// A sidebar row in a theme's miniature, with its state bar and background if it has them.
fn preview_row(
    state: Option<(gpui_kit::Rgba, gpui_kit::Rgba)>,
    indicator: f32,
    text: gpui_kit::Rgba,
    width: f32,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .w_full()
        .py(px(2.))
        .when_some(state, |d, (_, background)| d.bg(background))
        .child(
            div()
                .flex_shrink_0()
                .w(px(indicator))
                .h(px(10.))
                .when_some(state, |d, (bar, _)| d.bg(bar)),
        )
        .child(preview_bar(width, 4., text))
}

/// A line of chat in a theme's miniature.
fn preview_line(
    avatar: gpui_kit::Rgba,
    first: (f32, gpui_kit::Rgba),
    second: (f32, gpui_kit::Rgba),
) -> gpui_kit::Div {
    div()
        .flex()
        .gap(px(5.))
        .child(
            div()
                .flex_shrink_0()
                .size(px(10.))
                .rounded(px(5.))
                .bg(avatar),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(preview_bar(first.0, 4., first.1))
                .child(preview_bar(second.0, 4., second.1)),
        )
}

/// A theme drawn in miniature: sidebar with a selected and an unread row, chat lines and the input.
fn theme_preview(t: &Theme) -> gpui_kit::Div {
    let indicator = f32::from(t.shape.indicator_width).max(1.);
    let corner = f32::from(t.shape.radius).min(4.);
    let sidebar = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(4.))
        .w(px(52.))
        .h_full()
        .p(px(6.))
        .bg(t.surface.sidebar.0)
        .child(preview_bar(28., 5., t.accent.highlight.0))
        .child(preview_row(None, indicator, t.text.secondary.0, 22.))
        .child(preview_row(
            Some((t.channel.selected_border.0, t.channel.selected_bg.0)),
            indicator,
            t.text.primary.0,
            26.,
        ))
        .child(preview_row(
            Some((t.channel.unread_border.0, t.channel.unread_bg.0)),
            indicator,
            t.text.primary.0,
            18.,
        ))
        .child(preview_row(None, indicator, t.text.tertiary.0, 24.));
    let chat = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .gap(px(6.))
        .p(px(8.))
        .bg(t.surface.main.0)
        .child(preview_line(
            t.accent.muted.0,
            (40., t.text.primary.0),
            (60., t.text.secondary.0),
        ))
        .child(preview_line(
            t.presence.online.0,
            (30., t.text.primary.0),
            (48., t.text.secondary.0),
        ))
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .gap(px(4.))
                .child(
                    div()
                        .flex_1()
                        .h(px(12.))
                        .rounded(px(corner))
                        .bg(t.surface.raised.0),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(18.))
                        .h(px(12.))
                        .rounded(px(corner))
                        .bg(t.accent.base.0),
                ),
        );
    div().flex().size_full().child(sidebar).child(chat)
}

/// A card for a theme: its miniature (or that its file did not load) and its name. The theme in use
/// is outlined in the accent colour.
fn theme_card(
    n: usize,
    name: &str,
    loaded: &Result<Theme, String>,
    chosen: bool,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    const WIDTH: f32 = 160.;
    const PREVIEW_HEIGHT: f32 = 96.;
    let picture = match loaded {
        Ok(t) => theme_preview(t),
        Err(_) => div()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .text_size(theme::caption_size())
            .text_color(theme::danger())
            .child("not loaded"),
    };
    div()
        .id(("theme-card", n))
        .group("theme-card")
        .flex()
        .flex_col()
        .gap(px(6.))
        .w(px(WIDTH))
        .cursor_pointer()
        .child(
            div()
                .relative()
                .w_full()
                .h(px(PREVIEW_HEIGHT))
                .overflow_hidden()
                .rounded(theme::row_radius())
                .child(picture)
                // The ring lies over the preview, so choosing never shrinks it; it thickens and
                // takes the accent on a spring.
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(theme::row_radius())
                        .when(!chosen, |d| {
                            d.group_hover("theme-card", |d| d.border_color(theme::text_tertiary()))
                        })
                        .with_spring(
                            ("theme-card-ring", n),
                            gpui_kit::SpringAnimation::new(crate::theme::paced(
                                crate::hover::HOVER_SPRING,
                            ))
                            .to(chosen),
                            |d, phase| {
                                let t = phase.0.clamp(0., 1.);
                                theme::border_width(d, px(1. + t)).border_color(theme::mix(
                                    theme::border(),
                                    theme::accent(),
                                    t,
                                ))
                            },
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .text_color(if chosen {
                    theme::text()
                } else {
                    theme::text_secondary()
                })
                .child(name.to_owned())
                .when(chosen, |d| {
                    d.child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::accent())
                            .child("in use"),
                    )
                }),
        )
}

/// `Ctrl+Shift+M` as separate key caps.
fn key_caps(combo: &str) -> AnyElement {
    if combo.is_empty() {
        return muted("Not bound");
    }
    // A combo ending in `+` (Ctrl++) has `+` as its key.
    let mut keys: Vec<String> = combo.split('+').map(str::to_owned).collect();
    if combo.ends_with('+') {
        keys.retain(|k| !k.is_empty());
        keys.push("+".into());
    }
    div()
        .flex()
        .gap(px(4.))
        .children(keys.into_iter().map(|key| {
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded(theme::radius_small())
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised())
                .text_size(theme::caption_size())
                .child(key)
        }))
        .into_any_element()
}
