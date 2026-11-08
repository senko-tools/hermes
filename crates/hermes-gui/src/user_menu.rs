//! What a person's right-click menu does beyond the plain actions: their profile card, how I hear
//! them in voice, and the voice moderation (server mute and deafen, moving and disconnecting).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, Pixels, Point, SharedString, Window, div};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::api::VoiceStatePatchRequest;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
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
    pub at: Point<Pixels>,
}

impl Shell {
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
            Action::Profile => {
                self.profile = Some(ProfileCard {
                    inst,
                    space,
                    user,
                    at: self.menu_at,
                });
                cx.notify();
            }
            Action::Volume => {
                let at = self.menu_at;
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

    /// The profile card: picture, names, status and their roles in this server.
    pub fn render_profile(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = self.profile.as_ref()?;
        let (inst, user) = (card.inst.clone(), card.user);
        let store = self.core.store();
        let s = store.instance(&inst)?;
        let found = s.user(user);
        let name = s.display_name(card.space, user);
        let handle = found.map(|u| u.handle());
        let status = found.and_then(|u| u.status_text.clone());
        let mine = s.me_id() == Some(user);
        let roles: Vec<(String, gpui_kit::Rgba)> = card
            .space
            .and_then(|space| s.spaces.get(&space))
            .and_then(|state| {
                let member = state.members.get(&user)?;
                let mut roles: Vec<_> = state
                    .roles
                    .iter()
                    .filter(|r| member.role_ids.contains(&r.id))
                    .collect();
                roles.sort_by_key(|r| std::cmp::Reverse(r.position));
                Some(
                    roles
                        .into_iter()
                        .map(|r| {
                            let color = r.color.map_or(theme::text_secondary(), theme::rgb_u32);
                            (r.name.clone(), color)
                        })
                        .collect(),
                )
            })
            .unwrap_or_default();
        let avatar = self.avatar_with_status(&inst, found, &name, 64., s.status(user));
        drop(store);
        let message_inst = inst.clone();
        Some(crate::modal::popup(
            "profile-card-in",
            card.at,
            self.viewport,
            div()
                .id("profile-card")
                .occlude()
                .flex()
                .flex_col()
                .gap(px(10.))
                .w(px(280.))
                .p(px(14.))
                .rounded(px(8.))
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .font_family(theme::font())
                .text_color(theme::text())
                .text_size(theme::secondary_size())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.profile = None;
                    cx.notify();
                }))
                .child(avatar)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(theme::heading_size())
                                .font_weight(FontWeight::MEDIUM)
                                .child(name),
                        )
                        .children(
                            handle.map(|h| div().text_color(theme::text_tertiary()).child(h)),
                        ),
                )
                .children(
                    status
                        .filter(|t| !t.is_empty())
                        .map(|text| div().text_color(theme::text_secondary()).child(text)),
                )
                .when(!roles.is_empty(), |d| {
                    d.child(
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
                                    .rounded(px(4.))
                                    .bg(theme::raised())
                                    .text_size(theme::caption_size())
                                    .child(div().size(px(8.)).rounded(px(4.)).bg(color))
                                    .child(role)
                            })),
                    )
                })
                .when(!mine, |d| {
                    d.child(
                        Button::new("profile-message")
                            .cursor_pointer()
                            .primary()
                            .label(SharedString::from("Message"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.profile = None;
                                this.run_action(
                                    Target::Member(message_inst.clone(), None, user),
                                    Action::SendMessage,
                                    window,
                                    cx,
                                );
                            })),
                    )
                }),
        ))
    }
}
