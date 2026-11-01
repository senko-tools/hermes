//! The Account, Chat, Notifications, Keybinds and About tabs of the Settings buffer (PLAN.md
//! section 15.1).

use hermes_client_core::InstanceId;
use hermes_client_core::config::ClientConfig;
use hermes_client_core::proto::api::SessionInfo;
use hermes_client_core::proto::models::CurrentUser;
use hermes_client_core::time;
use iced::widget::{Space, button, checkbox, column, pick_list, row, slider, text_input};
use iced::{Alignment, Element, Fill};

use super::settings::{Message, setting};
use super::space_settings::Remote;
use crate::font::Fonts;
use crate::keybinds::{ACTIONS, Keybinds};
use crate::theme::{Theme, style};
use crate::widget::{label, small, strong};

/// What the Account tab shows.
pub struct AccountView<'a> {
    pub instances: Vec<InstanceId>,
    pub selected: Option<InstanceId>,
    pub me: Option<CurrentUser>,
    pub sessions: &'a Remote<Vec<SessionInfo>>,
    pub utc_offset: i64,
}

pub fn account<'a>(
    view: &AccountView<'a>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let mut col = column![strong("Account", fonts)].spacing(14).max_width(720);
    if view.instances.is_empty() {
        return col
            .push(
                label(
                    "Add an instance first: your account lives on a server.",
                    fonts,
                )
                .color(theme.text.secondary.0),
            )
            .into();
    }
    if view.instances.len() > 1 {
        col = col.push(setting(
            "Instance",
            "Each server has its own account.",
            pick_list(
                view.instances.clone(),
                view.selected.clone(),
                Message::AccountInstance,
            )
            .font(fonts.ui)
            .text_size(fonts.size)
            .width(260),
            fonts,
            theme,
        ));
    }
    let Some(me) = &view.me else {
        return col
            .push(label("Not signed in to this instance.", fonts).color(theme.text.secondary.0))
            .into();
    };
    col = col
        .push(setting(
            "Profile",
            &format!(
                "{} (@{}){}",
                me.user.name(),
                me.user.username,
                me.user
                    .status_text
                    .as_deref()
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default()
            ),
            button(label("Edit profile…", fonts))
                .style(move |_, s| style::primary(theme, s))
                .on_press(Message::EditProfile),
            fonts,
            theme,
        ))
        .push(setting(
            "Avatar",
            if me.user.avatar.is_some() {
                "Shown next to your messages."
            } else {
                "No picture yet; your initials are shown."
            },
            row![
                button(label(
                    if me.user.avatar.is_some() {
                        "Change…"
                    } else {
                        "Upload…"
                    },
                    fonts
                ))
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::PickAvatar),
                button(label("Remove", fonts))
                    .style(move |_, s| style::secondary(theme, s))
                    .on_press_maybe(me.user.avatar.is_some().then_some(Message::RemoveAvatar)),
            ]
            .spacing(8),
            fonts,
            theme,
        ))
        .push(setting(
            "Password",
            "Changing it signs out your other devices.",
            button(label("Change password…", fonts))
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::ChangePassword),
            fonts,
            theme,
        ));
    if me.instance_admin {
        col = col.push(
            small(
                "You are an admin of this instance. Manage users with the hermes-server user \
                 commands, or from the API (/admin/users).",
                fonts,
            )
            .color(theme.text.tertiary.0),
        );
    }

    let mut sessions = column![
        row![
            strong("Signed-in devices", fonts).width(Fill),
            button(small("Refresh", fonts))
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::LoadSessions),
            button(small("Sign out all others", fonts))
                .style(move |_, s| style::danger(theme, s))
                .on_press_maybe(
                    view.sessions
                        .ready()
                        .is_some_and(|l| l.iter().any(|s| !s.current))
                        .then_some(Message::RevokeOthers)
                ),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    ]
    .spacing(8);
    match view.sessions {
        Remote::Idle | Remote::Loading => {
            sessions = sessions.push(small("Loading…", fonts).color(theme.text.tertiary.0));
        }
        Remote::Failed(err) => {
            sessions = sessions
                .push(small(format!("Could not load: {err}"), fonts).color(theme.text.error.0));
        }
        Remote::Ready(list) => {
            for s in list {
                let seen = time::format(s.last_seen_at, "%Y-%m-%d %H:%M", view.utc_offset);
                sessions = sessions.push(
                    row![
                        column![
                            label(
                                if s.current {
                                    format!("{} (this device)", s.device_name)
                                } else {
                                    s.device_name.clone()
                                },
                                fonts
                            ),
                            small(format!("Last active {seen}"), fonts)
                                .color(theme.text.tertiary.0),
                        ]
                        .spacing(2)
                        .width(Fill),
                        button(small("Sign out", fonts))
                            .style(move |_, st| style::secondary(theme, st))
                            .on_press_maybe((!s.current).then_some(Message::RevokeSession(s.id))),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                );
            }
        }
    }
    col.push(sessions).into()
}

pub fn chat<'a>(config: &'a ClientConfig, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    let b = &config.buffer;
    let check = |on: bool, text: &'static str, key: &'static str| {
        checkbox(on)
            .label(text)
            .on_toggle(move |v| Message::Flag(key, v))
            .text_size(fonts.size)
            .font(fonts.ui)
    };
    column![
        strong("Chat", fonts),
        setting(
            "Messages",
            "",
            column![
                check(b.show_avatars, "Show avatars", "buffer.show_avatars"),
                check(
                    b.image_previews,
                    "Show image previews",
                    "buffer.image_previews"
                ),
                check(b.show_typing, "Show who is typing", "buffer.show_typing"),
                check(
                    b.enter_sends,
                    "Enter sends (Shift+Enter for a new line)",
                    "buffer.enter_sends"
                ),
            ]
            .spacing(6),
            fonts,
            theme,
        ),
        setting(
            "Group messages",
            &format!(
                "Messages by the same person within {} minutes share one name line.",
                b.group_minutes
            ),
            slider(0..=30u32, b.group_minutes.min(30), Message::GroupMinutes)
                .on_release(Message::SaveGroupMinutes)
                .width(220),
            fonts,
            theme,
        ),
        setting(
            "Time format",
            "strftime style: %H:%M is 18:05, %I:%M %p is 06:05 PM.",
            text_input("%H:%M", &b.timestamp_format)
                .on_input(Message::TimestampFormat)
                .on_submit(Message::SaveTimestampFormat)
                .font(fonts.mono)
                .size(fonts.size)
                .width(220),
            fonts,
            theme,
        ),
    ]
    .spacing(14)
    .max_width(720)
    .into()
}

pub fn notifications<'a>(
    config: &'a ClientConfig,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let n = &config.notifications;
    let g = &config.general;
    let check = |on: bool, text: &'static str, key: &'static str| {
        checkbox(on)
            .label(text)
            .on_toggle(move |v| Message::Flag(key, v))
            .text_size(fonts.size)
            .font(fonts.ui)
    };
    column![
        strong("Notifications", fonts),
        setting(
            "Desktop notifications",
            "Which messages notify is set per space or channel: right-click it and pick \
             \"Notify: …\" or Mute. By default spaces notify on @mentions and DMs on every message.",
            column![
                check(n.enabled, "Show notifications", "notifications.enabled"),
                check(
                    n.show_preview,
                    "Show the message text",
                    "notifications.show_preview"
                ),
                check(n.sound, "Play a sound", "notifications.sound"),
                check(
                    n.flash_taskbar,
                    "Flash the taskbar button",
                    "notifications.flash_taskbar"
                ),
            ]
            .spacing(6),
            fonts,
            theme,
        ),
        setting(
            "Window",
            "",
            column![
                check(
                    g.close_to_tray,
                    "Closing the window keeps Hermes running in the tray",
                    "general.close_to_tray"
                ),
                check(
                    g.start_minimized,
                    "Start minimized to the tray",
                    "general.start_minimized"
                ),
                check(
                    g.check_for_updates,
                    "Check for updates on start",
                    "general.check_for_updates"
                ),
            ]
            .spacing(6),
            fonts,
            theme,
        ),
    ]
    .spacing(14)
    .max_width(720)
    .into()
}

pub fn keybinds<'a>(
    config: &'a ClientConfig,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let mut list = column![
        strong("Keybinds", fonts),
        small(
            "Change them in the [keys] section of config.toml (Settings changes apply at once). \
             Examples: \"Ctrl+K\", \"Alt+Shift+Down\", \"Ctrl+\\\\\".",
            fonts
        )
        .color(theme.text.tertiary.0),
    ]
    .spacing(6)
    .max_width(720);
    for (action, _, _, title) in ACTIONS {
        let combo = Keybinds::combo_text(&config.keys, action).unwrap_or_else(|| "—".into());
        list = list.push(
            row![
                label(title, fonts).width(Fill),
                label(combo, fonts)
                    .font(fonts.mono)
                    .color(theme.text.secondary.0),
            ]
            .padding([2, 0]),
        );
    }
    list = list
        .push(Space::new().height(10))
        .push(strong("Everywhere", fonts))
        .push(
            small(
                "Right-click channels, spaces, messages and people for more actions. Double-click \
                 a voice channel to join it. Drag channels and roles to reorder them. Escape \
                 closes menus and dialogs. Ctrl+K finds anything.",
                fonts,
            )
            .color(theme.text.tertiary.0),
        );
    list.into()
}

pub fn about<'a>(fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    column![
        strong("About Hermes", fonts),
        label(format!("Version {}", env!("CARGO_PKG_VERSION")), fonts),
        small(
            "A self-hostable chat with spaces, text and voice channels, DMs and calls. Licensed \
             GPL-3.0-or-later; the server is AGPL-3.0-or-later.",
            fonts
        )
        .color(theme.text.tertiary.0),
        row![
            button(label("Copy debug info", fonts))
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::CopyDebugInfo),
            button(label("Open the logs folder", fonts))
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::OpenLogs),
            button(label("Project page", fonts))
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::OpenProjectPage),
        ]
        .spacing(8),
    ]
    .spacing(14)
    .max_width(720)
    .into()
}
