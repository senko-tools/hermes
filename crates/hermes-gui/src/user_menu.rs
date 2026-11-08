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

    /// The profile card: a band in their top role's colour, their picture over it, names and
    /// status, and when they joined this server, where they talk, and their roles.
    pub fn render_profile(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = self.profile.as_ref()?;
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
        if let Some(place) = in_voice.clone() {
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
                            .child(crate::assets::icon(
                                crate::assets::VOLUME,
                                14.,
                                theme::speaking(),
                            ))
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
                                    .rounded(px(4.))
                                    .bg(theme::raised())
                                    .text_size(theme::caption_size())
                                    .child(
                                        div()
                                            .size(px(8.))
                                            .rounded(px(4.))
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
                                this.profile = None;
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
                                    this.profile = None;
                                    this.open_volume_menu(
                                        volume_inst.clone(),
                                        user,
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                    )
                })
        });

        Some(crate::modal::popup(
            "profile-card-in",
            card.at,
            self.viewport,
            div()
                .id("profile-card")
                .occlude()
                .flex()
                .flex_col()
                .w(px(300.))
                .overflow_hidden()
                .rounded(px(10.))
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .font_family(theme::font())
                .text_color(theme::text())
                .text_size(theme::secondary_size())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.profile = None;
                    cx.notify();
                }))
                .child(
                    // gpui clips to the card's box, not its rounded corners: the band rounds its
                    // own (the card's radius less its border).
                    // A banner is 5:2 across the card; without one, a shorter band of colour.
                    div()
                        .h(px(if banner.is_some() { 119.2 } else { 64. }))
                        .rounded_t(px(9.))
                        .overflow_hidden()
                        .bg(theme::mix(theme::sidebar(), band, 0.75))
                        .children(banner.map(|picture| {
                            gpui_kit::img(picture)
                                .size_full()
                                .rounded_t(px(9.))
                                .object_fit(gpui_kit::ObjectFit::Cover)
                                .id("profile-banner")
                        })),
                )
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
                                .child(avatar),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(theme::heading_size())
                                        .font_weight(FontWeight::BOLD)
                                        .child(name),
                                )
                                .children(handle_row),
                        )
                        .children(status.map(|text| {
                            div()
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(6.))
                                .bg(theme::raised())
                                .text_color(theme::text_secondary())
                                .child(text)
                        }))
                        .when(!details.is_empty(), |d| {
                            d.child(div().h(px(1.)).bg(theme::border()))
                                .children(details)
                        })
                        .children(buttons),
                ),
        ))
    }
}

/// A date like `Mar 4, 2026`.
fn long_date(unix_ms: i64, offset_secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = hermes_client_core::time::format(unix_ms, "%Y %m %d", offset_secs);
    let mut parts = date.split(' ').map(|p| p.parse::<usize>().unwrap_or(1));
    let (y, m, d) = (
        parts.next().unwrap_or(1970),
        parts.next().unwrap_or(1),
        parts.next().unwrap_or(1),
    );
    format!("{} {d}, {y}", MONTHS[m.clamp(1, 12) - 1])
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_read_like_words() {
        // 2026-03-04 12:00 UTC.
        assert_eq!(super::long_date(1_772_625_600_000, 0), "Mar 4, 2026");
    }
}
