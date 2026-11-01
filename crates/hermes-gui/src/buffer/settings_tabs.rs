//! The Account, Chat, Notifications, Keybinds and About tabs of the Settings buffer (PLAN.md
//! section 15.1).

use hermes_client_core::InstanceId;
use hermes_client_core::config::ClientConfig;
use hermes_client_core::proto::api::SessionInfo;
use hermes_client_core::proto::models::{CurrentUser, PrivacySettings};
use hermes_client_core::time;
use iced::widget::{column, container, row, slider, text_input};
use iced::{Alignment, Element, Fill};

use super::page::{dropdown, group, group_with, page, setting, toggle};
use super::settings::{DmFrom, Message};
use super::space_settings::Remote;
use crate::font::Fonts;
use crate::keybinds::{ACTIONS, Action, Keybinds};
use crate::theme::{Theme, style};
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, heading, icon, label};

/// What the Account tab shows.
pub struct AccountView<'a> {
    pub instances: Vec<InstanceId>,
    pub selected: Option<InstanceId>,
    pub me: Option<CurrentUser>,
    /// My privacy settings on the selected instance.
    pub privacy: PrivacySettings,
    pub sessions: &'a Remote<Vec<SessionInfo>>,
    pub utc_offset: i64,
}

pub fn account<'a>(
    view: &AccountView<'a>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let note = |text: &str| -> Element<'a, Message> {
        label(text.to_owned(), fonts)
            .color(theme.text.secondary.0)
            .into()
    };
    if view.instances.is_empty() {
        return page(
            "Account",
            None,
            vec![note(
                "Add an instance first: your account lives on a server.",
            )],
            fonts,
            theme,
        );
    }
    let mut account_rows = Vec::new();
    if view.instances.len() > 1 {
        account_rows.push(setting(
            "Instance",
            "Each server has its own account.",
            dropdown(
                view.instances.clone(),
                view.selected.clone(),
                Message::AccountInstance,
                fonts,
                theme,
            ),
            fonts,
            theme,
        ));
    }
    let Some(me) = &view.me else {
        account_rows.push(note("Not signed in to this instance."));
        return page(
            "Account",
            None,
            vec![group("Account", account_rows, fonts, theme)],
            fonts,
            theme,
        );
    };
    let mut profile_hint = format!(
        "{} (@{}){}",
        me.user.name(),
        me.user.username,
        me.user
            .status_text
            .as_deref()
            .map(|s| format!(" · {s}"))
            .unwrap_or_default()
    );
    if me.instance_admin {
        profile_hint.push_str(
            ". You are an admin of this instance: manage users with the hermes-server user \
             commands or the API (/admin/users).",
        );
    }
    account_rows.push(setting(
        "Profile",
        &profile_hint,
        text_button("Edit profile…", ButtonKind::Primary, fonts, theme)
            .on_press(Message::EditProfile),
        fonts,
        theme,
    ));
    account_rows.push(setting(
        "Avatar",
        if me.user.avatar.is_some() {
            "Shown next to your messages."
        } else {
            "No picture yet; your initials are shown."
        },
        row![
            text_button(
                if me.user.avatar.is_some() {
                    "Change…"
                } else {
                    "Upload…"
                },
                ButtonKind::Secondary,
                fonts,
                theme
            )
            .on_press(Message::PickAvatar),
            text_button("Remove", ButtonKind::Secondary, fonts, theme)
                .on_press_maybe(me.user.avatar.is_some().then_some(Message::RemoveAvatar)),
        ]
        .spacing(style::GAP_S),
        fonts,
        theme,
    ));
    account_rows.push(setting(
        "Password",
        "Changing it signs out your other devices.",
        text_button("Change password…", ButtonKind::Secondary, fonts, theme)
            .on_press(Message::ChangePassword),
        fonts,
        theme,
    ));

    let privacy = view.privacy;
    let privacy_rows = vec![
        setting(
            "Direct messages from",
            "Who can start a conversation with you on this instance. Conversations you already \
             have stay open.",
            dropdown(
                &DmFrom::ALL[..],
                Some(DmFrom(privacy.dm_from)),
                Message::DmFrom,
                fonts,
                theme,
            ),
            fonts,
            theme,
        ),
        toggle(
            "Message requests",
            "Messages from everyone else wait in Requests under Messages: you are notified \
             once per request, and they stay quiet until you accept. Off: they cannot message \
             you at all.",
            privacy.message_requests,
            Message::AllowRequests,
            fonts,
            theme,
        ),
    ];

    let mut device_rows = Vec::new();
    match view.sessions {
        Remote::Idle | Remote::Loading => {
            device_rows.push(
                caption("Loading…", fonts)
                    .color(theme.text.tertiary.0)
                    .into(),
            );
        }
        Remote::Failed(err) => {
            device_rows.push(
                caption(format!("Could not load: {err}"), fonts)
                    .color(theme.text.error.0)
                    .into(),
            );
        }
        Remote::Ready(list) => {
            for s in list {
                let seen = time::format(s.last_seen_at, "%Y-%m-%d %H:%M", view.utc_offset);
                let control: Element<'a, Message> = if s.current {
                    caption("This device", fonts)
                        .color(theme.text.tertiary.0)
                        .into()
                } else {
                    text_button("Sign out", ButtonKind::Secondary, fonts, theme)
                        .on_press(Message::RevokeSession(s.id))
                        .into()
                };
                device_rows.push(setting(
                    &s.device_name,
                    &format!("Last active {seen}"),
                    control,
                    fonts,
                    theme,
                ));
            }
        }
    }
    let devices = group_with(
        "Signed-in devices",
        vec![
            text_button("Refresh", ButtonKind::Secondary, fonts, theme)
                .on_press(Message::LoadSessions)
                .into(),
            text_button("Sign out all others", ButtonKind::Danger, fonts, theme)
                .on_press_maybe(
                    view.sessions
                        .ready()
                        .is_some_and(|l| l.iter().any(|s| !s.current))
                        .then_some(Message::RevokeOthers),
                )
                .into(),
        ],
        device_rows,
        fonts,
        theme,
    );
    page(
        "Account",
        None,
        vec![
            group("Account", account_rows, fonts, theme),
            group("Privacy", privacy_rows, fonts, theme),
            devices,
        ],
        fonts,
        theme,
    )
}

/// An on/off `[section] key` setting.
fn flag<'a>(
    name: &str,
    hint: &str,
    on: bool,
    key: &'static str,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    toggle(name, hint, on, move |v| Message::Flag(key, v), fonts, theme)
}

pub fn chat<'a>(
    config: &'a ClientConfig,
    utc_offset: i64,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let b = &config.buffer;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let preview = time::format(now, &b.timestamp_format, utc_offset);
    page(
        "Chat",
        None,
        vec![
            group(
                "Messages",
                vec![
                    flag(
                        "Show avatars",
                        "",
                        b.show_avatars,
                        "buffer.show_avatars",
                        fonts,
                        theme,
                    ),
                    flag(
                        "Show image previews",
                        "Images in messages are shown, not just linked.",
                        b.image_previews,
                        "buffer.image_previews",
                        fonts,
                        theme,
                    ),
                    flag(
                        "Show who is typing",
                        "",
                        b.show_typing,
                        "buffer.show_typing",
                        fonts,
                        theme,
                    ),
                    flag(
                        "Enter sends",
                        "Shift+Enter starts a new line.",
                        b.enter_sends,
                        "buffer.enter_sends",
                        fonts,
                        theme,
                    ),
                ],
                fonts,
                theme,
            ),
            group(
                "Layout",
                vec![
                    setting(
                        "Group messages",
                        &format!(
                            "Messages by the same person within {} minutes share one name line.",
                            b.group_minutes
                        ),
                        slider(0..=30u32, b.group_minutes.min(30), Message::GroupMinutes)
                            .on_release(Message::SaveGroupMinutes),
                        fonts,
                        theme,
                    ),
                    setting(
                        "Time format",
                        &format!(
                            "strftime style: %H:%M is 18:05, %I:%M %p is 06:05 PM. Now: {preview}"
                        ),
                        text_input("%H:%M", &b.timestamp_format)
                            .padding(style::INPUT_PAD)
                            .style(move |_, st| style::input(theme, st))
                            .on_input(Message::TimestampFormat)
                            .on_submit(Message::SaveTimestampFormat)
                            .font(fonts.mono)
                            .size(fonts.size),
                        fonts,
                        theme,
                    ),
                ],
                fonts,
                theme,
            ),
        ],
        fonts,
        theme,
    )
}

pub fn notifications<'a>(
    config: &'a ClientConfig,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let n = &config.notifications;
    let g = &config.general;
    page(
        "Notifications",
        Some(
            "Which messages notify is set per space or channel: right-click it and pick \
             \"Notify: …\" or Mute. By default spaces notify on @mentions and DMs on every message.",
        ),
        vec![
            group(
                "Desktop notifications",
                vec![
                    flag(
                        "Show notifications",
                        "",
                        n.enabled,
                        "notifications.enabled",
                        fonts,
                        theme,
                    ),
                    flag(
                        "Show the message text",
                        "Off: only who wrote, and where.",
                        n.show_preview,
                        "notifications.show_preview",
                        fonts,
                        theme,
                    ),
                    flag(
                        "Play a sound",
                        "",
                        n.sound,
                        "notifications.sound",
                        fonts,
                        theme,
                    ),
                    flag(
                        "Flash the taskbar button",
                        "",
                        n.flash_taskbar,
                        "notifications.flash_taskbar",
                        fonts,
                        theme,
                    ),
                ],
                fonts,
                theme,
            ),
            group(
                "Window",
                vec![
                    flag(
                        "Close to the tray",
                        "Closing the window keeps Hermes running in the tray.",
                        g.close_to_tray,
                        "general.close_to_tray",
                        fonts,
                        theme,
                    ),
                    flag(
                        "Start minimized to the tray",
                        "",
                        g.start_minimized,
                        "general.start_minimized",
                        fonts,
                        theme,
                    ),
                    flag(
                        "Check for updates on start",
                        "",
                        g.check_for_updates,
                        "general.check_for_updates",
                        fonts,
                        theme,
                    ),
                ],
                fonts,
                theme,
            ),
        ],
        fonts,
        theme,
    )
}

/// The keybind groups, in the order shown.
const KEY_GROUPS: [(&str, &[Action]); 4] = [
    (
        "Navigation",
        &[
            Action::CommandBar,
            Action::Search,
            Action::NextUnread,
            Action::PrevUnread,
            Action::NextChannel,
            Action::PrevChannel,
            Action::MarkRead,
        ],
    ),
    (
        "Panes",
        &[
            Action::FocusNextPane,
            Action::FocusPrevPane,
            Action::SplitRight,
            Action::SplitDown,
            Action::ClosePane,
            Action::MaximizePane,
            Action::ToggleMemberList,
            Action::ToggleSidebar,
        ],
    ),
    ("Voice", &[Action::ToggleMute, Action::ToggleDeafen]),
    (
        "App",
        &[
            Action::Settings,
            Action::ZoomIn,
            Action::ZoomOut,
            Action::ZoomReset,
        ],
    ),
];

/// `Ctrl+Shift+M` as separate key caps.
fn key_caps<'a>(combo: &str, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    if combo.is_empty() {
        return caption("Not bound", fonts)
            .color(theme.text.tertiary.0)
            .into();
    }
    // A combo ending in `+` (Ctrl++) has `+` as its key.
    let mut keys: Vec<String> = combo.split('+').map(str::to_owned).collect();
    if combo.ends_with('+') {
        keys.retain(|k| !k.is_empty());
        keys.push("+".into());
    }
    row(keys.into_iter().map(|key| {
        container(
            caption(key, fonts)
                .font(fonts.mono)
                .color(theme.text.primary.0),
        )
        .padding([2.0, style::GAP_S])
        .style(move |_| container::Style {
            background: Some(theme.surface.raised.0.into()),
            border: iced::Border {
                color: theme.general.border.0,
                width: 1.0,
                radius: style::radius_small(theme).into(),
            },
            ..container::Style::default()
        })
        .into()
    }))
    .spacing(style::GAP_XS)
    .align_y(Alignment::Center)
    .into()
}

pub fn keybinds<'a>(
    config: &'a ClientConfig,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let mut groups: Vec<Element<'a, Message>> = KEY_GROUPS
        .iter()
        .map(|(title, actions)| {
            let rows = actions
                .iter()
                .filter_map(|action| ACTIONS.iter().find(|(a, ..)| a == action))
                .map(|(action, _, _, title)| {
                    let combo = Keybinds::combo_text(&config.keys, *action).unwrap_or_default();
                    setting(title, "", key_caps(&combo, fonts, theme), fonts, theme)
                })
                .collect();
            group(title, rows, fonts, theme)
        })
        .collect();
    groups.push(group(
        "Everywhere",
        vec![
            caption(
                "Right-click channels, spaces, messages and people for more actions. Double-click \
                 a voice channel to join it. Drag channels and roles to reorder them. Escape \
                 closes menus and dialogs. Ctrl+K finds anything.",
                fonts,
            )
            .color(theme.text.tertiary.0)
            .into(),
        ],
        fonts,
        theme,
    ));
    page(
        "Keybinds",
        Some(
            "Change them in the [keys] section of config.toml; changes apply at once. \
             Examples: \"Ctrl+K\", \"Alt+Shift+Down\", \"Ctrl+\\\\\".",
        ),
        groups,
        fonts,
        theme,
    )
}

pub fn about<'a>(fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    let badge = container(icon::icon(icon::FEATHER, 28.0).color(theme.accent.base.0))
        .center_x(56)
        .center_y(56)
        .style(move |_| container::Style {
            background: Some(theme.accent.glow.0.into()),
            border: iced::Border {
                radius: style::radius_large(theme).into(),
                ..iced::Border::default()
            },
            ..container::Style::default()
        });
    column![
        row![
            badge,
            column![
                heading("Hermes", fonts),
                caption(format!("Version {}", env!("CARGO_PKG_VERSION")), fonts)
                    .color(theme.text.tertiary.0),
            ]
            .spacing(style::GAP_XS),
        ]
        .spacing(style::GAP_L)
        .align_y(Alignment::Center),
        label(
            "A self-hostable chat with spaces, text and voice channels, DMs and calls. Licensed \
             GPL-3.0-or-later; the server is AGPL-3.0-or-later.",
            fonts
        )
        .color(theme.text.secondary.0),
        row![
            text_button("Copy debug info", ButtonKind::Secondary, fonts, theme)
                .on_press(Message::CopyDebugInfo),
            text_button("Open the logs folder", ButtonKind::Secondary, fonts, theme)
                .on_press(Message::OpenLogs),
            text_button("Project page", ButtonKind::Secondary, fonts, theme)
                .on_press(Message::OpenProjectPage),
        ]
        .spacing(style::GAP_S),
    ]
    .spacing(style::GAP_L)
    .width(Fill)
    .max_width(super::page::MAX_WIDTH)
    .into()
}
