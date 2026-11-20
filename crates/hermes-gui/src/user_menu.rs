//! What a person's right-click menu does beyond the plain actions: their profile card, how I hear
//! them in voice, and the voice moderation (server mute and deafen, moving and disconnecting).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{RopeExt as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, App, Bounds, Context, Entity, FontWeight, Pixels, Point,
    SharedString, Subscription, Window, div,
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
    /// The text of my profile being edited where it shows, if any.
    pub field: Option<FieldEdit>,
    /// Where my banner, picture and presence dot are on screen, for their menus to open beside.
    parts: Rc<RefCell<HashMap<ProfilePart, Bounds<Pixels>>>>,
}

/// What on my own card opens a menu: the banner, the picture, the presence dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProfilePart {
    Banner,
    Avatar,
    Presence,
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

/// A text of my profile being edited on the card, in an input that takes its place.
pub struct FieldEdit {
    pub field: ProfileField,
    input: FieldInput,
    _events: Subscription,
}

/// One line (the name, the status), or lines that wrap and grow (the "About me").
#[derive(Clone)]
enum FieldInput {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

impl FieldInput {
    fn value(&self, cx: &App) -> String {
        match self {
            FieldInput::Line(input) => input.read(cx).value().to_string(),
            FieldInput::Area(input) => input.read(cx).value().to_string(),
        }
    }
}

/// The width of a profile card.
const CARD_WIDTH: f32 = 300.;
/// The size of the picture on it.
const CARD_AVATAR: f32 = 80.;

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
            field: None,
            parts: Default::default(),
        });
        self.fetch_last_online(cx);
        cx.notify();
    }

    /// My own profile card on `inst`, beside `at`. What on it is mine to change changes where
    /// it shows. It shows at once from what the store has; the "About me" comes in the
    /// background.
    pub fn open_my_profile(
        &mut self,
        inst: InstanceId,
        at: crate::modal::PopupFrom,
        cx: &mut Context<Self>,
    ) {
        let me = self.core.store().instance(&inst).and_then(|s| s.me_id());
        let Some(me) = me else { return };
        self.show_profile(inst, None, me, at, cx);
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
        let (current, username) = {
            let store = self.core.store();
            let me = store
                .instance(&card.inst)
                .and_then(|s| s.me.as_ref().map(|m| m.user.clone()));
            let username = me.as_ref().map(|u| u.username.clone()).unwrap_or_default();
            let current = match field {
                ProfileField::Name => me.and_then(|u| u.display_name),
                ProfileField::Status => me.and_then(|u| u.status_text),
                ProfileField::Bio => card.bio.clone(),
            }
            .unwrap_or_default();
            (current, username)
        };
        let (input, events) = if field == ProfileField::Bio {
            let input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .auto_grow(1, 8)
                    .submit_on_enter(true);
                state.set_value(current, window, cx);
                state
            });
            let events = cx.subscribe_in(&input, window, |this, _, event, _, cx| match event {
                InputEvent::PressEnter { shift: false, .. } | InputEvent::Blur => {
                    this.save_profile_field(cx);
                }
                _ => {}
            });
            // The caret at the end, where one goes on writing.
            input.update(cx, |input, cx| {
                let end = input.text().offset_to_position(input.value().len());
                input.set_cursor_position(end, window, cx);
            });
            (FieldInput::Area(input), events)
        } else {
            // An empty name falls back to the username: it shows dimmed while there is none.
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                if field == ProfileField::Name {
                    state = state.placeholder(username);
                }
                state.set_value(current, window, cx);
                state
            });
            let events = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.save_profile_field(cx);
                }
            });
            // The caret at the end, where one goes on writing.
            input.update(cx, |input, cx| {
                let end = input.text().offset_to_position(input.value().len());
                input.set_cursor_position(end, window, cx);
            });
            (FieldInput::Line(input), events)
        };
        if let Some(card) = &mut self.profile {
            card.field = Some(FieldEdit {
                field,
                input,
                _events: events,
            });
        }
        cx.notify();
    }

    /// Stops editing a text of my card without saving it (Escape). `false` if none was.
    pub fn cancel_profile_field(&mut self, cx: &mut Context<Self>) -> bool {
        let edited = self.profile.as_mut().and_then(|c| c.field.take()).is_some();
        if edited {
            cx.notify();
        }
        edited
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
        let text = edit.input.value(cx).trim().to_owned();
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

    /// Opens the menu of `part` of my card (clicked at `position`) beside it.
    fn open_part_menu(
        &mut self,
        part: ProfilePart,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_profile_field(cx);
        let Some(card) = &self.profile else { return };
        let inst = card.inst.clone();
        // The banner is as wide as the card: its menu opens at the pointer, not beside it.
        let trigger = card
            .parts
            .borrow()
            .get(&part)
            .copied()
            .filter(|_| part != ProfilePart::Banner)
            .unwrap_or_else(|| crate::modal::PopupFrom::point(position).trigger);
        let lines = match part {
            ProfilePart::Banner | ProfilePart::Avatar => {
                let banner = part == ProfilePart::Banner;
                let set = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.me.as_ref().map(|m| m.user.clone()))
                        .is_some_and(|u| {
                            if banner {
                                u.banner.is_some()
                            } else {
                                u.avatar.is_some()
                            }
                        })
                };
                picture_lines(inst, banner, set)
            }
            ProfilePart::Presence => presence_lines(self.core.presence()),
        };
        self.open_lines(
            crate::modal::PopupKey::ProfilePart(part),
            lines,
            position,
            trigger,
            window,
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
    /// status, and when they joined this server, where they talk, and their roles. On my own
    /// card the same things can be changed where they are: the banner, the picture and the
    /// presence dot open a menu, and the name, status and "About me" turn into inputs that look
    /// like them. Under the pointer only a small pencil says so.
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
        let with_bio = s.has_feature(hermes_client_core::proto::compat::features::BIO);
        let editing = card.field.as_ref().map(|f| (f.field, f.input.clone()));
        let editing_of = |field: ProfileField| {
            editing
                .clone()
                .filter(|(f, _)| *f == field)
                .map(|(_, input)| input)
        };
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
        let presence = s.status(user);
        let looks_online = matches!(presence, Status::Online | Status::Dnd);
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
        // My own picture carries a presence dot of its own, which opens the status menu.
        let avatar = if mine {
            self.avatar(&inst, found, &name, CARD_AVATAR)
        } else {
            self.avatar_with_status(&inst, found, &name, CARD_AVATAR, presence)
        };
        let banner = self.banner_picture(&inst, found);
        drop(store);

        let section = |title: &'static str| {
            div()
                .text_size(theme::caption_size())
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text_tertiary())
                .child(title.to_uppercase())
        };
        // Only the name and the copy icon copy it, not the empty rest of the line.
        let handle_row = handle.map(|h| {
            let copied = h.clone();
            div()
                .id("profile-handle")
                .self_start()
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
        let bio = card.bio.clone().filter(|b| !b.is_empty());
        if mine && with_bio {
            let about = |text: AnyElement| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section("About me"))
                    .child(text)
                    .into_any_element()
            };
            details.push(match (editing_of(ProfileField::Bio), bio) {
                (Some(input), _) => about(field_input(&input, theme::secondary_size(), false)),
                (None, Some(bio)) => about(self.editable_text(
                    ProfileField::Bio,
                    div().whitespace_normal().child(bio).into_any_element(),
                    true,
                    cx,
                )),
                (None, None) => self.add_link(ProfileField::Bio, "Add about me", cx),
            });
        } else if let Some(bio) = bio {
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

        // A banner is 5:2 across the card; without one, a shorter band of colour.
        let banner_height = if banner.is_some() {
            (CARD_WIDTH - 2.) * 0.4
        } else {
            64.
        };
        let parts = card.parts.clone();
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
            .when(mine, |d| {
                d.cursor_pointer()
                    .group("profile-banner-edit")
                    .child(note_part(parts.clone(), ProfilePart::Banner))
                    // A shade and a pencil in the corner, under the pointer only.
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .group_hover("profile-banner-edit", |d| d.bg(HOVER_SHADE)),
                    )
                    .child(
                        pencil_badge("profile-banner-edit")
                            .absolute()
                            .top(px(8.))
                            .right(px(8.)),
                    )
                    .on_click(
                        cx.listener(|this, event: &gpui_kit::ClickEvent, window, cx| {
                            this.open_part_menu(ProfilePart::Banner, event.position(), window, cx);
                        }),
                    )
            });
        // The picture alone, with no ground of its own: a transparent one shows the banner.
        let avatar_box = div()
            .id("profile-avatar-box")
            .relative()
            .size(px(CARD_AVATAR))
            .child(avatar)
            .when(mine, |d| {
                d.cursor_pointer()
                    .group("profile-avatar-edit")
                    .child(note_part(parts.clone(), ProfilePart::Avatar))
                    // On the picture's corner, half over the banner.
                    .child(
                        pencil_badge("profile-avatar-edit")
                            .absolute()
                            .top(px(0.))
                            .right(px(0.)),
                    )
                    .on_click(
                        cx.listener(|this, event: &gpui_kit::ClickEvent, window, cx| {
                            this.open_part_menu(ProfilePart::Avatar, event.position(), window, cx);
                        }),
                    )
            });
        // My presence dot: it darkens under the pointer, with a pencil in it, and opens the
        // status menu. It sits over the picture's corner but is not part of it.
        let presence_dot = mine.then(|| {
            let dot = crate::images::presence_dot_size(CARD_AVATAR);
            let group = "profile-presence";
            div()
                .id("profile-presence")
                .occlude()
                .absolute()
                .bottom(px(-1.))
                .right(px(-1.))
                .size(px(dot))
                .group(group)
                .cursor_pointer()
                .child(crate::images::presence_dot(
                    SharedString::from(format!("presence-{inst}-{user}-{presence:?}")),
                    presence,
                    dot,
                    theme::sidebar(),
                ))
                .child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left(px(2.))
                        .right(px(2.))
                        .bottom(px(2.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .group_hover(group, |d| {
                            d.bg(theme::mix(theme::status(presence), BLACK, 0.3))
                        })
                        .child(
                            crate::assets::icon(
                                crate::assets::PENCIL,
                                11.,
                                theme::alpha(WHITE, 0.),
                            )
                            .group_hover(group, |s| s.text_color(WHITE)),
                        ),
                )
                .child(note_part(parts.clone(), ProfilePart::Presence))
                .on_click(
                    cx.listener(|this, event: &gpui_kit::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_part_menu(ProfilePart::Presence, event.position(), window, cx);
                    }),
                )
        });
        let avatar_box = div()
            .relative()
            .size(px(CARD_AVATAR))
            .child(avatar_box)
            .children(presence_dot);

        // The name, one line tall whether it shows or is being changed. On my card, a link to
        // add a status sits at the end of it while I have none.
        let name_text = div()
            .text_size(theme::heading_size())
            .font_weight(FontWeight::BOLD)
            .child(name.clone())
            .into_any_element();
        let name_line = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .map(|d| match editing_of(ProfileField::Name) {
                Some(input) => d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::BOLD)
                        .child(field_input(&input, theme::heading_size(), true)),
                ),
                None if mine => d.child(div().flex_1().min_w_0().flex().child(self.editable_text(
                    ProfileField::Name,
                    name_text,
                    false,
                    cx,
                ))),
                None => d.child(div().flex_1().min_w_0().child(name_text)),
            })
            .when(
                mine && status.is_none() && editing_of(ProfileField::Status).is_none(),
                |d| d.child(self.add_link(ProfileField::Status, "Add status text", cx)),
            );
        let status_bubble = |content: AnyElement| {
            div()
                .px(px(10.))
                .py(px(6.))
                .rounded(theme::row_radius())
                .bg(theme::raised())
                .text_color(theme::text_secondary())
                .child(content)
                .into_any_element()
        };
        let status = match (editing_of(ProfileField::Status), status) {
            (Some(input), _) => Some(status_bubble(field_input(
                &input,
                theme::secondary_size(),
                false,
            ))),
            (None, Some(text)) if mine => Some(status_bubble(self.editable_text(
                ProfileField::Status,
                div().whitespace_normal().child(text).into_any_element(),
                true,
                cx,
            ))),
            (None, Some(text)) => Some(status_bubble(text.into_any_element())),
            (None, None) => None,
        };

        Some(crate::modal::popup(
            "profile-card-in",
            card.at,
            CARD_WIDTH,
            self.viewport,
            div()
                .id("profile-card")
                .occlude()
                .relative()
                .flex()
                .flex_col()
                .w(px(CARD_WIDTH))
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
                    // A menu of the card is open: that press closes the menu, not the card.
                    if this.menu.is_some() && !crate::modal::closing("menu") {
                        return;
                    }
                    this.save_profile_field(cx);
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
                // (the card's radius less its border).
                .child(banner_box)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .px(px(16.))
                        .pb(px(16.))
                        // The picture sits over the band's lower edge.
                        .child(div().mt(px(-44.)).p(px(4.)).self_start().child(avatar_box))
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
                        .children(buttons),
                ),
        ))
    }

    /// A text of my card as it shows (`shown`), with a pencil after it under the pointer; a
    /// click turns it into an input in its place. `fill`: it takes the whole width (text that
    /// wraps), else only its own.
    fn editable_text(
        &self,
        field: ProfileField,
        shown: AnyElement,
        fill: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = SharedString::from(format!("profile-text-{field:?}"));
        div()
            .id(group.clone())
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .when(fill, |d| d.flex_1())
            .cursor_text()
            .child(div().min_w_0().when(fill, |d| d.flex_1()).child(shown))
            .child(
                crate::assets::icon(
                    crate::assets::PENCIL,
                    12.,
                    theme::alpha(theme::text_tertiary(), 0.),
                )
                .group_hover(group, |s| s.text_color(theme::text_tertiary())),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_profile_field(field, window, cx);
            }))
            .into_any_element()
    }

    /// A plain text link (like a link on a web page) that starts writing `field` of my card.
    fn add_link(
        &self,
        field: ProfileField,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("profile-add-{field:?}")))
            .flex_shrink_0()
            .cursor_pointer()
            .text_size(theme::caption_size())
            .text_color(theme::accent())
            .hover(|d| d.underline())
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_profile_field(field, window, cx);
            }))
            .into_any_element()
    }
}

/// How tall a line of text is, for its font size (gpui's default line height).
const LINE: f32 = std::f32::consts::GOLDEN_RATIO;
/// The space a textarea of the smallest size keeps at its sides.
const AREA_INSET: f32 = 4.;
const BLACK: gpui_kit::Rgba = gpui_kit::Rgba {
    r: 0.,
    g: 0.,
    b: 0.,
    a: 1.,
};
const WHITE: gpui_kit::Rgba = gpui_kit::Rgba {
    r: 1.,
    g: 1.,
    b: 1.,
    a: 1.,
};
/// What a picture of my card darkens by under the pointer.
const HOVER_SHADE: gpui_kit::Rgba = gpui_kit::Rgba {
    r: 0.,
    g: 0.,
    b: 0.,
    a: 0.12,
};

/// An input in place of a text of my card, without a box of its own, in the text's size and
/// as tall as its lines: what is being changed stays where it was and looks the same.
fn field_input(input: &FieldInput, size: Pixels, bold: bool) -> AnyElement {
    let line = gpui_kit::relative(LINE);
    match input {
        FieldInput::Line(input) => Styled::h(Input::new(input).appearance(false), size * LINE)
            .px_0()
            .py_0()
            .text_size(size)
            .line_height(line)
            .when(bold, |i| i.font_weight(FontWeight::BOLD))
            .into_any_element(),
        FieldInput::Area(input) => div()
            .mx(px(-AREA_INSET))
            .child(
                Textarea::new(input)
                    .xsmall()
                    .appearance(false)
                    .text_size(size)
                    .line_height(line),
            )
            .into_any_element(),
    }
}

/// A small round pencil that shows over a picture of my card while the pointer is on `group`.
fn pencil_badge(group: &'static str) -> gpui_kit::Div {
    div()
        .size(px(22.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::alpha(theme::sidebar(), 0.))
        .group_hover(group, |d| d.bg(theme::sidebar()))
        .child(
            crate::assets::icon(crate::assets::PENCIL, 12., theme::alpha(theme::text(), 0.))
                .group_hover(group, |s| s.text_color(theme::text())),
        )
}

/// Notes where its parent (a part of my card) is painted, for its menu to open beside it.
fn note_part(
    parts: Rc<RefCell<HashMap<ProfilePart, Bounds<Pixels>>>>,
    part: ProfilePart,
) -> AnyElement {
    gpui_kit::canvas(
        move |bounds, _, _| {
            parts.borrow_mut().insert(part, bounds);
        },
        |_, (), _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// The menu of my banner (`banner`) or picture: change it, or remove it when one is `set`.
fn picture_lines(inst: InstanceId, banner: bool, set: bool) -> Vec<Line> {
    let pick = inst.clone();
    let mut lines = vec![run_line("Change image", move |this, _, cx| {
        let purpose = if banner {
            crate::crop::CropFor::Banner(pick.clone())
        } else {
            crate::crop::CropFor::Avatar(pick.clone())
        };
        this.pick_picture(purpose, cx);
    })];
    if set {
        lines.push(Line::Item {
            label: "Remove image".into(),
            checked: false,
            danger: true,
            dot: None,
            run: Rc::new(move |this, _, cx| {
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
            }),
        });
    }
    lines
}

/// The menu of my presence dot: each status with its colour, the current one ticked.
fn presence_lines(current: PresenceStatus) -> Vec<Line> {
    [
        (PresenceStatus::Online, Status::Online, "Online"),
        (PresenceStatus::Idle, Status::Idle, "Idle"),
        (PresenceStatus::Dnd, Status::Dnd, "Do Not Disturb"),
        (PresenceStatus::Invisible, Status::Offline, "Invisible"),
    ]
    .into_iter()
    .map(|(status, dot, label)| Line::Item {
        label: label.into(),
        checked: current == status,
        danger: false,
        dot: Some(theme::status(dot)),
        run: Rc::new(move |this, _, cx| {
            this.core.set_presence(status);
            cx.notify();
        }),
    })
    .collect()
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
