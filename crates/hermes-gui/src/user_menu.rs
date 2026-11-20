//! What a person's right-click menu does beyond the plain actions: their profile card, how I hear
//! them in voice, and the voice moderation (server mute and deafen, moving and disconnecting).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window,
    div,
};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::api::{UpdateMeRequest, VoiceStatePatchRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{ChannelKind, PresenceStatus, Status};
use hermes_client_core::view;

use crate::menu::{Line, run_line};
use crate::menu_entries::{Action, Target};
use crate::shell::Shell;
use crate::theme::{self, px};

/// An open profile card: whose, in which server (`None` in DMs), and where.
pub struct ProfileCard {
    pub inst: InstanceId,
    pub space: Option<Id>,
    pub user: Id,
    pub at: crate::modal::PopupFrom,
    /// When they were last online, once the server has said.
    pub last_online: Option<i64>,
    /// Their "About me", once the profile is fetched.
    pub bio: Option<String>,
    /// My own card, open for editing: the banner, picture, name, status and "About me" change
    /// where they are.
    pub editing: bool,
    /// The text being edited, in place of what it shows.
    pub field: Option<FieldEdit>,
}

/// What text of my profile can be edited on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileField {
    Name,
    Status,
    Bio,
}

impl ProfileField {
    /// The longest it may be, in characters (the server's limits).
    fn max_chars(self) -> usize {
        match self {
            ProfileField::Name => 32,
            ProfileField::Status => 128,
            ProfileField::Bio => 190,
        }
    }
}

/// A text of my profile being edited on the card.
pub struct FieldEdit {
    pub field: ProfileField,
    pub input: Entity<InputState>,
    _events: Subscription,
}

/// The width of a profile card, and of my own while it is edited.
const CARD_WIDTH: f32 = 300.;
const EDIT_WIDTH: f32 = 360.;

impl Shell {
    /// A left click on someone (a voice row, a member, a name or picture in the chat): their
    /// profile, beside what was clicked. Their menu stays on the right click.
    pub fn open_profile(
        &mut self,
        target: &Target,
        position: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Target::Member(inst, space, user) = target.clone() else {
            return;
        };
        if self.dismissed_by_this_press(&crate::modal::PopupKey::Profile(inst.clone(), user)) {
            return;
        }
        let mut at = self.popup_from(position);
        // Clicked inside a bordered bar (my own user panel): the card goes beside the whole
        // bar, not over its other buttons.
        if let Some(bar) = self.bar_around(at.trigger) {
            at.trigger = bar;
        }
        self.show_profile(inst, space, user, at, cx);
    }

    fn show_profile(
        &mut self,
        inst: InstanceId,
        space: Option<Id>,
        user: Id,
        at: crate::modal::PopupFrom,
        cx: &mut Context<Self>,
    ) {
        self.cancel_closing("profile-card-in");
        self.profile = Some(ProfileCard {
            inst,
            space,
            user,
            at,
            last_online: None,
            bio: None,
            editing: false,
            field: None,
        });
        self.fetch_last_online(cx);
        cx.notify();
    }

    /// My own profile card on `inst`, open for editing, beside `at`. It shows at once from what
    /// the store has; the "About me" comes in the background.
    pub fn open_my_profile(
        &mut self,
        inst: InstanceId,
        at: crate::modal::PopupFrom,
        cx: &mut Context<Self>,
    ) {
        let me = self.core.store().instance(&inst).and_then(|s| s.me_id());
        let Some(me) = me else { return };
        self.show_profile(inst, None, me, at, cx);
        if let Some(card) = &mut self.profile {
            card.editing = true;
        }
    }

    /// Starts editing `field` of my profile on the open card: an input in its place, saved on
    /// Enter or when it loses focus.
    fn edit_profile_field(
        &mut self,
        field: ProfileField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_profile_field(cx);
        let Some(card) = &self.profile else { return };
        let current = {
            let store = self.core.store();
            let me = store
                .instance(&card.inst)
                .and_then(|s| s.me.as_ref().map(|m| m.user.clone()));
            match field {
                ProfileField::Name => me.and_then(|u| u.display_name),
                ProfileField::Status => me.and_then(|u| u.status_text),
                ProfileField::Bio => card.bio.clone(),
            }
            .unwrap_or_default()
        };
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(current, window, cx);
            state
        });
        let events = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.save_profile_field(cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        if let Some(card) = &mut self.profile {
            card.field = Some(FieldEdit {
                field,
                input,
                _events: events,
            });
        }
        cx.notify();
    }

    /// Saves the text being edited on my card, when it changed: one request for that field.
    fn save_profile_field(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &mut self.profile else {
            return;
        };
        let Some(edit) = card.field.take() else {
            return;
        };
        cx.notify();
        let text = edit.input.read(cx).value().trim().to_owned();
        let inst = card.inst.clone();
        if text.chars().count() > edit.field.max_chars() {
            self.error(
                format!(
                    "that is too long: at most {} characters",
                    edit.field.max_chars()
                ),
                cx,
            );
            return;
        }
        let value = (!text.is_empty()).then_some(text);
        let current = {
            let store = self.core.store();
            let me = store
                .instance(&inst)
                .and_then(|s| s.me.as_ref().map(|m| m.user.clone()));
            match edit.field {
                ProfileField::Name => me.and_then(|u| u.display_name),
                ProfileField::Status => me.and_then(|u| u.status_text),
                ProfileField::Bio => card.bio.clone(),
            }
            .filter(|t| !t.is_empty())
        };
        if value == current {
            return;
        }
        let mut req = UpdateMeRequest::default();
        match edit.field {
            ProfileField::Name => req.display_name = Some(value),
            ProfileField::Status => req.status_text = Some(value),
            ProfileField::Bio => {
                // The card shows it at once; the server's answer does not carry it.
                card.bio = value.clone();
                req.bio = Some(value);
            }
        }
        self.call_status(
            &inst,
            "Profile saved",
            move |r| async move { r.update_me(&req).await },
            cx,
        );
    }

    /// The person's entries of a member menu. `false` if `action` is not one of them.
    pub fn run_member_action(
        &mut self,
        target: &Target,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Target::Member(inst, space, user) = target.clone() else {
            return false;
        };
        let voice = |muted: Option<bool>, deafened: Option<bool>| VoiceStatePatchRequest {
            server_mute: muted,
            server_deaf: deafened,
            channel_id: None,
        };
        match action {
            Action::Mention => {
                let username = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.user(user).map(|u| u.username.clone()));
                let open = self.open_channel().filter(|(i, _)| *i == inst);
                match (username, open) {
                    (Some(name), Some(key)) => {
                        self.insert_text(&key, &format!("@{name} "), window, cx);
                    }
                    _ => self.info("Open a channel first to mention someone", cx),
                }
            }
            Action::Profile => {
                let at = self.menu_from;
                self.show_profile(inst, space, user, at, cx);
            }
            Action::Volume => {
                let at = self.menu_from;
                self.open_volume_menu(inst, user, at, window, cx);
            }
            Action::MuteForMe | Action::UnmuteForMe => {
                self.core
                    .set_user_muted(&inst, user, action == Action::MuteForMe);
                cx.notify();
            }
            Action::ServerMute => self.patch_member_voice(&inst, user, voice(Some(true), None), cx),
            Action::ServerUnmute => {
                self.patch_member_voice(&inst, user, voice(Some(false), None), cx);
            }
            Action::ServerDeafen => {
                self.patch_member_voice(&inst, user, voice(None, Some(true)), cx);
            }
            Action::ServerUndeafen => {
                self.patch_member_voice(&inst, user, voice(None, Some(false)), cx);
            }
            Action::DisconnectVoice => {
                let req = VoiceStatePatchRequest {
                    channel_id: Some(None),
                    ..VoiceStatePatchRequest::default()
                };
                self.patch_member_voice(&inst, user, req, cx);
            }
            _ => return false,
        }
        true
    }

    /// Changes someone's voice state in the server of the voice channel they are in.
    fn patch_member_voice(
        &mut self,
        inst: &InstanceId,
        user: Id,
        req: VoiceStatePatchRequest,
        cx: &mut Context<Self>,
    ) {
        let space = {
            let store = self.core.store();
            store.instance(inst).and_then(|s| {
                let state = s.voice_states.get(&user)?;
                s.space_of(state.channel_id)
            })
        };
        let Some(space) = space else {
            self.error("They are not in a voice channel any more.", cx);
            return;
        };
        let ok = match (req.server_mute, req.server_deaf, req.channel_id) {
            (Some(true), ..) => "Muted for everyone",
            (Some(false), ..) => "Unmuted",
            (_, Some(true), _) => "Deafened",
            (_, Some(false), _) => "Undeafened",
            (_, _, Some(None)) => "Disconnected",
            _ => "Moved",
        };
        self.call_status(
            inst,
            ok,
            move |r| async move { r.update_voice_state(space, user, &req).await },
            cx,
        );
    }

    /// The "Move to" submenu of a member menu: the other voice channels of their server.
    pub(crate) fn move_lines(&self, target: &Target) -> Vec<Line> {
        let Target::Member(inst, _, user) = target else {
            return Vec::new();
        };
        let user = *user;
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return Vec::new();
        };
        let Some(current) = s.voice_states.get(&user).map(|v| v.channel_id) else {
            return Vec::new();
        };
        let Some(space) = s.space_of(current) else {
            return Vec::new();
        };
        view::channel_tree(s, space)
            .into_iter()
            .filter(|c| c.kind == ChannelKind::Voice && c.id != current)
            .map(|c| {
                let (inst, channel) = (inst.clone(), c.id);
                run_line(c.name.clone(), move |this, _, cx| {
                    let req = VoiceStatePatchRequest {
                        channel_id: Some(Some(channel)),
                        ..VoiceStatePatchRequest::default()
                    };
                    this.patch_member_voice(&inst, user, req, cx);
                })
            })
            .collect()
    }

    /// Asks the server when the person on the open profile card was last online.
    fn fetch_last_online(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &self.profile else {
            return;
        };
        let (inst, user) = (card.inst.clone(), card.user);
        let Ok(rest) = self.core.rest(&inst) else {
            return;
        };
        self.run(
            cx,
            async move { rest.user(user).await },
            move |this, result, _, cx| match result {
                Ok(profile) => {
                    if let Some(card) = this.profile.as_mut().filter(|c| c.user == user) {
                        card.last_online = profile.last_online_at;
                        card.bio = profile.bio;
                        cx.notify();
                    }
                }
                Err(err) => tracing::debug!("could not fetch {user}'s profile: {err}"),
            },
        );
    }

    /// The profile card: a band in their top role's colour, their picture over it, names and
    /// status, and when they joined this server, where they talk, and their roles.
    pub fn render_profile(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = self.profile.as_ref()?;
        // A picture being cropped for it: the card waits under the crop dialog.
        if self.crop.is_some() {
            return None;
        }
        let (inst, user) = (card.inst.clone(), card.user);
        let store = self.core.store();
        let s = store.instance(&inst)?;
        let found = s.user(user);
        let name = s.display_name(card.space, user);
        let handle = found.map(|u| u.handle());
        let status = found
            .and_then(|u| u.status_text.clone())
            .filter(|t| !t.is_empty());
        let mine = s.me_id() == Some(user);
        let editing = mine && card.editing;
        let with_bio = s.has_feature(hermes_client_core::proto::compat::features::BIO);
        let width = if editing { EDIT_WIDTH } else { CARD_WIDTH };
        let editing_field = card.field.as_ref().map(|f| (f.field, f.input.clone()));
        let member = card
            .space
            .and_then(|space| s.spaces.get(&space))
            .and_then(|state| Some((state, state.members.get(&user)?)));
        let joined = member.map(|(_, m)| long_date(m.joined_at, self.utc_offset));
        let roles: Vec<(String, Option<gpui_kit::Rgba>)> = member
            .map(|(state, member)| {
                let mut roles: Vec<_> = state
                    .roles
                    .iter()
                    .filter(|r| member.role_ids.contains(&r.id))
                    .collect();
                roles.sort_by_key(|r| std::cmp::Reverse(r.position));
                roles
                    .into_iter()
                    .map(|r| (r.name.clone(), r.color.map(theme::rgb_u32)))
                    .collect()
            })
            .unwrap_or_default();
        let band = roles
            .iter()
            .find_map(|(_, color)| *color)
            .unwrap_or_else(theme::accent);
        let looks_online = matches!(s.status(user), Status::Online | Status::Dnd);
        let last_online = if looks_online {
            Some("Online now".to_owned())
        } else {
            card.last_online
                .map(|at| last_online_text(hermes_client_core::now_ms(), at, self.utc_offset))
        };
        let in_voice = s.voice_states.get(&user).and_then(|v| {
            let channel = s.channel(v.channel_id)?;
            let place = s
                .space_of(v.channel_id)
                .and_then(|space| s.spaces.get(&space))
                .map(|st| st.space.name.clone());
            let name = channel.name.clone().unwrap_or_default();
            Some(match place {
                Some(place) => format!("{name} in {place}"),
                None => name,
            })
        });
        let (has_avatar, has_banner) =
            found.map_or((false, false), |u| (u.avatar.is_some(), u.banner.is_some()));
        let avatar = self.avatar_with_status(&inst, found, &name, 80., s.status(user));
        let banner = self.banner_picture(&inst, found);
        drop(store);

        let section = |title: &'static str| {
            div()
                .text_size(theme::caption_size())
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text_tertiary())
                .child(title.to_uppercase())
        };
        let handle_row = handle.map(|h| {
            let copied = h.clone();
            div()
                .id("profile-handle")
                .flex()
                .items_center()
                .gap(px(4.))
                .cursor_pointer()
                .group("profile-handle")
                .text_color(theme::text_tertiary())
                .hover(|d| d.text_color(theme::accent()))
                .child(h)
                .child(
                    crate::assets::icon(crate::assets::COPY, 12., theme::text_tertiary())
                        .group_hover("profile-handle", |s| s.text_color(theme::accent())),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.copy(copied.clone(), "Name", cx);
                }))
        });
        let mut details: Vec<AnyElement> = Vec::new();
        if editing && with_bio {
            let bio = card.bio.clone().filter(|b| !b.is_empty());
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section("About me"))
                    .child(self.profile_text(
                        ProfileField::Bio,
                        editing_field.clone(),
                        bio.map(|b| div().whitespace_normal().child(b).into_any_element()),
                        "About me",
                        cx,
                    ))
                    .into_any_element(),
            );
        } else if let Some(bio) = card.bio.clone() {
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section("About me"))
                    .child(div().whitespace_normal().child(bio))
                    .into_any_element(),
            );
        }
        if let Some(joined) = joined {
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section("Member since"))
                    .child(joined)
                    .into_any_element(),
            );
        }
        if let Some(note) = self.core.note(&card.inst, card.user) {
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section("Your note"))
                    .child(note)
                    .into_any_element(),
            );
        }
        if let Some(last_online) = last_online {
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section("Last online"))
                    .child(last_online)
                    .into_any_element(),
            );
        }
        if let Some(place) = in_voice.clone() {
            let speaking = self.core.is_speaking(&inst, user);
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section("In voice"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            // The speaker swells while they talk.
                            .child(
                                crate::assets::icon(crate::assets::VOLUME, 14., theme::speaking())
                                    .with_spring(
                                        "profile-speaking",
                                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                                            gpui_kit::SpringConfig::new(420., 24., 1.),
                                        ))
                                        .to(speaking),
                                        |d, phase| {
                                            let pop = 1. + 0.2 * phase.0.clamp(0., 1.2);
                                            d.with_transformation(gpui_kit::Transformation::scale(
                                                gpui_kit::size(pop, pop),
                                            ))
                                        },
                                    ),
                            )
                            .child(div().min_w_0().truncate().child(place)),
                    )
                    .into_any_element(),
            );
        }
        if !roles.is_empty() {
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(section("Roles"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .children(roles.into_iter().map(|(role, color)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(theme::radius_small())
                                    .bg(theme::raised())
                                    .text_size(theme::caption_size())
                                    .child(
                                        div()
                                            .size(px(8.))
                                            .rounded(theme::radius_small())
                                            .bg(color.unwrap_or_else(theme::text_secondary)),
                                    )
                                    .child(role)
                            })),
                    )
                    .into_any_element(),
            );
        }

        let message_inst = inst.clone();
        let volume_inst = inst.clone();
        let status_row = mine.then(|| {
            let current = self.core.presence();
            div().flex().flex_wrap().gap(px(4.)).children(
                [
                    (PresenceStatus::Online, Status::Online, "Online"),
                    (PresenceStatus::Idle, Status::Idle, "Idle"),
                    (PresenceStatus::Dnd, Status::Dnd, "Do Not Disturb"),
                    (PresenceStatus::Invisible, Status::Offline, "Invisible"),
                ]
                .into_iter()
                .enumerate()
                .map(|(n, (status, dot, label))| {
                    let chosen = current == status;
                    div()
                        .id(("my-status", n))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(theme::radius_small())
                        .cursor_pointer()
                        .text_color(if chosen {
                            theme::text()
                        } else {
                            theme::text_secondary()
                        })
                        .when(chosen, |d| d.bg(theme::selected()))
                        .when(!chosen, |d| d.hover(|d| d.bg(theme::hover())))
                        .child(div().size(px(8.)).rounded_full().bg(theme::status(dot)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.core.set_presence(status);
                            cx.notify();
                        }))
                }),
            )
        });
        // My card: a pencil in the corner opens it for editing, a tick closes that again.
        let edit_toggle = mine.then(|| {
            div()
                .id("profile-edit-toggle")
                .absolute()
                .top(px(8.))
                .right(px(8.))
                .p(px(6.))
                .rounded_full()
                .cursor_pointer()
                .bg(theme::alpha(theme::sidebar(), 0.75))
                .hover(|d| d.bg(theme::sidebar()))
                .child(crate::assets::icon(
                    if editing {
                        crate::assets::CHECK
                    } else {
                        crate::assets::PENCIL
                    },
                    14.,
                    theme::text(),
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.save_profile_field(cx);
                    if let Some(card) = &mut this.profile {
                        card.editing = !card.editing;
                    }
                    cx.notify();
                }))
        });
        let buttons = (!mine).then(|| {
            div()
                .flex()
                .gap(px(8.))
                .child(
                    div().flex_1().child(
                        Button::new("profile-message")
                            .cursor_pointer()
                            .primary()
                            .w_full()
                            .label(SharedString::from("Message"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_later("profile-card-in", |s| s.profile = None, cx);
                                this.run_action(
                                    Target::Member(message_inst.clone(), None, user),
                                    Action::SendMessage,
                                    window,
                                    cx,
                                );
                            })),
                    ),
                )
                .when(in_voice.is_some(), |d| {
                    d.child(
                        Button::new("profile-volume")
                            .cursor_pointer()
                            .label(SharedString::from("Volume\u{2026}"))
                            .on_click(cx.listener(
                                move |this, event: &gpui_kit::ClickEvent, window, cx| {
                                    this.close_later("profile-card-in", |s| s.profile = None, cx);
                                    let at = this.popup_from(event.position());
                                    this.open_volume_menu(
                                        volume_inst.clone(),
                                        user,
                                        at,
                                        window,
                                        cx,
                                    );
                                },
                            )),
                    )
                })
        });

        // Editing: the banner and the picture are pickers, a dimmed layer and a pencil over them
        // under the pointer.
        let banner_height = if banner.is_some() {
            (width - 2.) * 0.4
        } else if editing {
            88.
        } else {
            64.
        };
        let edit_inst = inst.clone();
        let banner_box = div()
            .id("profile-banner-box")
            .relative()
            .h(px(banner_height))
            .rounded_t(px(9.))
            .overflow_hidden()
            // Not under a banner: each layer fades on its own, so a band there would show
            // through the banner while the card goes.
            .when(banner.is_none(), |d| {
                d.bg(theme::mix(theme::sidebar(), band, 0.75))
            })
            .children(banner.map(|picture| {
                gpui_kit::img(picture)
                    .size_full()
                    .rounded_t(px(9.))
                    .object_fit(gpui_kit::ObjectFit::Cover)
                    .id("profile-banner")
            }))
            .when(editing, |d| {
                d.cursor_pointer()
                    .group("profile-banner-edit")
                    .child(picker_layer("profile-banner-edit"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pick_picture(crate::crop::CropFor::Banner(edit_inst.clone()), cx);
                    }))
                    .when(has_banner, |d| {
                        d.child(self.remove_picture("profile-banner-edit", true, &inst, cx))
                    })
            });
        let avatar_inst = inst.clone();
        let avatar_box = div()
            .id("profile-avatar-box")
            .relative()
            .group("profile-avatar-edit")
            .child(avatar)
            .when(editing, |d| {
                d.child(
                    div()
                        .id("profile-avatar-pick")
                        .absolute()
                        .inset_0()
                        .rounded_full()
                        .overflow_hidden()
                        .cursor_pointer()
                        .child(picker_layer("profile-avatar-edit"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_picture(
                                crate::crop::CropFor::Avatar(avatar_inst.clone()),
                                cx,
                            );
                        })),
                )
                .when(has_avatar, |d| {
                    d.child(self.remove_picture("profile-avatar-edit", false, &inst, cx))
                })
            });
        let name_line = div()
            .text_size(theme::heading_size())
            .font_weight(FontWeight::BOLD)
            .child(name.clone());
        let name_line = if editing {
            self.profile_text(
                ProfileField::Name,
                editing_field.clone(),
                Some(name_line.into_any_element()),
                "Display name",
                cx,
            )
        } else {
            name_line.into_any_element()
        };
        let status_bubble = |text: String| {
            div()
                .px(px(10.))
                .py(px(6.))
                .rounded(theme::row_radius())
                .bg(theme::raised())
                .text_color(theme::text_secondary())
                .child(text)
                .into_any_element()
        };
        let status = if editing {
            Some(self.profile_text(
                ProfileField::Status,
                editing_field,
                status.map(status_bubble),
                "Status",
                cx,
            ))
        } else {
            status.map(status_bubble)
        };

        Some(crate::modal::popup(
            "profile-card-in",
            card.at,
            width,
            self.viewport,
            div()
                .id("profile-card")
                .occlude()
                .relative()
                .flex()
                .flex_col()
                .w(px(width))
                .overflow_hidden()
                .rounded(theme::radius_large())
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .font_family(theme::font())
                .text_color(theme::text())
                .text_size(theme::secondary_size())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if let Some(card) = &this.profile
                        && !crate::modal::closing("profile-card-in")
                    {
                        let key = crate::modal::PopupKey::Profile(card.inst.clone(), card.user);
                        this.note_dismissed(key);
                    }
                    this.close_later("profile-card-in", |s| s.profile = None, cx);
                    cx.notify();
                }))
                // gpui clips to the card's box, not its rounded corners: the band rounds its own
                // (the card's radius less its border). A banner is 5:2 across the card; without
                // one, a shorter band of colour.
                .child(banner_box)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .px(px(16.))
                        .pb(px(16.))
                        // The picture sits over the band's lower edge, in a ring of the card's
                        // colour.
                        .child(
                            div()
                                .mt(px(-44.))
                                .p(px(4.))
                                .w(px(88.))
                                .rounded(px(44.))
                                .bg(theme::sidebar())
                                .child(avatar_box),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(name_line)
                                .children(handle_row),
                        )
                        .children(status)
                        .when(!details.is_empty(), |d| {
                            d.child(div().h(px(1.)).bg(theme::border()))
                                .children(details)
                        })
                        .children(buttons)
                        .children(status_row),
                )
                .children(edit_toggle),
        ))
    }

    /// A text of my profile on the card while it is edited: what it shows (or `empty`, dimmed),
    /// tinted under the pointer with a pencil beside it, and an input in its place once
    /// clicked.
    fn profile_text(
        &self,
        field: ProfileField,
        editing: Option<(ProfileField, Entity<InputState>)>,
        shown: Option<AnyElement>,
        empty: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some((_, input)) = editing.filter(|(f, _)| *f == field) {
            return Input::new(&input).into_any_element();
        }
        let group = SharedString::from(format!("profile-text-{field:?}"));
        div()
            .id(group.clone())
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(6.))
            .mx(px(-6.))
            .px(px(6.))
            .py(px(2.))
            .rounded(theme::radius_small())
            .cursor_text()
            .hover(|d| d.bg(theme::hover()))
            .child(div().flex_1().min_w_0().child(shown.unwrap_or_else(|| {
                div()
                    .text_color(theme::text_tertiary())
                    .child(empty)
                    .into_any_element()
            })))
            .child(
                crate::assets::icon(crate::assets::PENCIL, 12., theme::text_tertiary())
                    .group_hover(group, |s| s.text_color(theme::accent())),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_profile_field(field, window, cx);
            }))
            .into_any_element()
    }
}

impl Shell {
    /// A small cross in the corner of my banner (`banner`) or picture while the pointer is on
    /// it: removes it.
    fn remove_picture(
        &self,
        group: &'static str,
        banner: bool,
        inst: &InstanceId,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let inst = inst.clone();
        div()
            .id(if banner {
                "profile-banner-remove"
            } else {
                "profile-avatar-remove"
            })
            .absolute()
            .top(px(if banner { 8. } else { 0. }))
            .left(px(if banner { 8. } else { 0. }))
            .p(px(4.))
            .rounded_full()
            .cursor_pointer()
            .bg(theme::alpha(theme::sidebar(), 0.))
            .group_hover(group, |d| d.bg(theme::sidebar()))
            .hover(|d| d.bg(theme::danger()))
            .child(
                crate::assets::icon(crate::assets::CLOSE, 12., theme::alpha(theme::text(), 0.))
                    .group_hover(group, |s| s.text_color(theme::text())),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let req = if banner {
                    UpdateMeRequest {
                        banner_id: Some(None),
                        ..UpdateMeRequest::default()
                    }
                } else {
                    UpdateMeRequest {
                        avatar_id: Some(None),
                        ..UpdateMeRequest::default()
                    }
                };
                this.call_status(
                    &inst,
                    if banner {
                        "Banner removed"
                    } else {
                        "Avatar removed"
                    },
                    move |r| async move { r.update_me(&req).await },
                    cx,
                );
            }))
    }
}

/// Over a picture that can be changed: dimmed, with a pencil, while the pointer is on it.
fn picker_layer(group: &'static str) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .group_hover(group, |d| d.bg(gpui_kit::rgba(0x0000_0059)))
        .child(
            crate::assets::icon(crate::assets::PENCIL, 18., gpui_kit::rgba(0xffff_ff00))
                .group_hover(group, |s| s.text_color(gpui_kit::rgba(0xffff_ffff))),
        )
}

/// A date like `Mar 4, 2026`.
/// How long ago someone was last online; past a week, the date.
fn last_online_text(now: i64, at: i64, offset_secs: i64) -> String {
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let ago = (now - at).max(0);
    let count = |n: i64, unit: &str| {
        let plural = if n == 1 { "" } else { "s" };
        format!("{n} {unit}{plural} ago")
    };
    match ago {
        _ if ago < MINUTE => "Just now".to_owned(),
        _ if ago < HOUR => count(ago / MINUTE, "minute"),
        _ if ago < DAY => count(ago / HOUR, "hour"),
        _ if ago < 7 * DAY => count(ago / DAY, "day"),
        _ => long_date(at, offset_secs),
    }
}

fn long_date(unix_ms: i64, offset_secs: i64) -> String {
    hermes_client_core::time::format(unix_ms, "%b %e, %Y", offset_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_online_reads_as_time_ago_then_a_date() {
        const MINUTE: i64 = 60_000;
        let now = 100 * 24 * 60 * MINUTE;
        let ago = |ms| last_online_text(now, now - ms, 0);
        assert_eq!(ago(30_000), "Just now");
        assert_eq!(ago(MINUTE), "1 minute ago");
        assert_eq!(ago(59 * MINUTE), "59 minutes ago");
        assert_eq!(ago(2 * 60 * MINUTE), "2 hours ago");
        assert_eq!(ago(3 * 24 * 60 * MINUTE), "3 days ago");
        assert_eq!(ago(8 * 24 * 60 * MINUTE), "Apr 3, 1970");
        assert_eq!(last_online_text(now, now + 5, 0), "Just now");
    }

    #[test]
    fn dates_read_like_words() {
        // 2026-03-04 12:00 UTC.
        assert_eq!(super::long_date(1_772_625_600_000, 0), "Mar 4, 2026");
    }
}
