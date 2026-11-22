//! The Encryption tab of a server's settings: who hands a channel's keys to those who lack them,
//! whether new members can read the past, for the whole server and per channel, and who is waiting
//! to be let in. The server's rules are the owner's; a channel's are the owner's or an admin's.

use crate::ui::ButtonKind;
use std::collections::HashMap;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, div};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::e2ee::{KeyHandover, KeyPolicy, KeyPolicyOverride, KeyRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::view;

use crate::results::Remote;
use crate::settings::{muted, page_body, section, setting_below, setting_fit};
use crate::shell::Shell;
use crate::space_settings::SpacePage;
use crate::theme::{self, px};

/// What the Encryption tab shows.
#[derive(Default)]
pub struct KeySettings {
    /// The server's rules.
    pub policy: Remote<KeyPolicy>,
    /// What each channel changes of them.
    pub channels: HashMap<Id, KeyPolicyOverride>,
}

/// The message channels of a server, in the order they are listed.
fn message_channels(s: &InstanceStore, space: Id) -> Vec<(Id, String)> {
    let mut channels: Vec<_> = s
        .spaces
        .get(&space)
        .map(|st| {
            st.channels
                .values()
                .filter(|c| c.kind.has_messages())
                .map(|c| (c.position, c.id, c.name.clone().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    channels.sort_by_key(|(position, id, _)| (*position, *id));
    channels
        .into_iter()
        .map(|(_, id, name)| (id, name))
        .collect()
}

/// Two or three choices, side by side. Without `run`, they only show which one holds.
fn choices<T: Copy + PartialEq + 'static>(
    shell: &Shell,
    id: &str,
    options: &[(&'static str, T)],
    current: T,
    enabled: bool,
    cx: &mut Context<Shell>,
    run: impl Fn(&mut Shell, T, &mut Context<Shell>) + Clone + 'static,
) -> AnyElement {
    let values: Vec<T> = options.iter().map(|(_, value)| *value).collect();
    crate::ui::segmented(
        shell,
        id.to_owned(),
        options
            .iter()
            .map(|(label, _)| crate::ui::Segment::new(*label))
            .collect(),
        values.iter().position(|v| *v == current),
        enabled,
        move |this, n, _, cx| run(this, values[n], cx),
        cx,
    )
    .into_any_element()
}

impl Shell {
    /// Fetches the rules of the server and of each of its channels.
    pub(crate) fn load_key_settings(&mut self, key: (InstanceId, Id), cx: &mut Context<Self>) {
        let (inst, space) = key.clone();
        let channels: Vec<Id> = {
            let store = self.core.store();
            store.instance(&inst).map_or_else(Vec::new, |s| {
                message_channels(s, space)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            })
        };
        if let Some(page) = self.space_pages.get_mut(&key) {
            page.keys.policy = Remote::Loading;
        }
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let policy = core.space_key_policy(&inst, space).await?;
                let mut overrides = HashMap::new();
                for channel in channels {
                    let own = core.channel_key_policy(&inst, channel).await?;
                    overrides.insert(channel, own);
                }
                Ok::<_, String>((policy, overrides))
            },
            move |this, result, _, cx| {
                if let Some(page) = this.space_pages.get_mut(&key) {
                    match result {
                        Ok((policy, channels)) => {
                            page.keys.policy = Remote::Ready(policy);
                            page.keys.channels = channels;
                        }
                        Err(err) => page.keys.policy = Remote::Failed(err),
                    }
                }
                cx.notify();
            },
        );
    }

    fn set_server_key_rules(
        &mut self,
        key: (InstanceId, Id),
        policy: KeyPolicy,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = self.space_pages.get_mut(&key) {
            page.keys.policy = Remote::Ready(policy);
        }
        let core = self.core.clone();
        let (inst, space) = key;
        self.run_status(cx, async move {
            core.set_space_key_policy(&inst, space, policy).await?;
            Ok("Encryption rules saved".to_owned())
        });
        cx.notify();
    }

    fn set_channel_key_rules(
        &mut self,
        key: (InstanceId, Id),
        channel: Id,
        policy: KeyPolicyOverride,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = self.space_pages.get_mut(&key) {
            page.keys.channels.insert(channel, policy);
        }
        let core = self.core.clone();
        let inst = key.0;
        self.run_status(cx, async move {
            core.set_channel_key_policy(&inst, channel, policy).await?;
            Ok("Encryption rules saved".to_owned())
        });
        cx.notify();
    }

    fn approve_key_request(
        &mut self,
        inst: InstanceId,
        request: KeyRequest,
        cx: &mut Context<Self>,
    ) {
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.approve_key_request(&inst, &request)
                    .await
                    .map_err(|e| e.to_string())
            },
            |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("could not hand over the keys: {err}"), cx);
                }
                cx.notify();
            },
        );
    }

    /// The Encryption tab.
    pub(crate) fn tab_encryption(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let intro = "Messages, files and reactions in this server are end-to-end encrypted. The \
                     server stores them but cannot read them.";
        let (inst, space) = (page.inst.clone(), page.space);
        let Remote::Ready(policy) = page.keys.policy else {
            let body = match &page.keys.policy {
                Remote::Failed(err) => muted(format!("Could not load the rules: {err}")),
                _ => theme::skeleton("encryption", 4),
            };
            return page_body("Encryption", Some(intro), vec![body]);
        };
        let key = (inst.clone(), space);
        let owner = s.spaces.get(&space).map(|st| st.space.owner_id) == s.me_id();
        let admin = owner || view::can_in_space(s, space, Permissions::ADMINISTRATOR);
        let mut groups = Vec::new();

        // Those waiting to be let in come first: they are waiting on someone.
        if view::can_in_space(s, space, Permissions::MANAGE_SPACE) {
            let waiting: Vec<KeyRequest> = self
                .core
                .key_requests(&inst)
                .into_iter()
                .filter(|r| s.space_of(r.channel_id) == Some(space))
                .collect();
            if !waiting.is_empty() {
                let rows = waiting
                    .into_iter()
                    .enumerate()
                    .map(|(n, request)| self.waiting_row(n, &inst, space, s, request, cx))
                    .collect();
                groups.push(section(
                    "Waiting for keys",
                    "New devices that asked to read a channel.",
                    rows,
                ));
            }
        }

        let handover_key = key.clone();
        let handover = if owner {
            crate::ui::radio_list(
                self,
                "server-handover",
                HANDOVERS
                    .iter()
                    .map(|(_, name, hint)| {
                        (SharedString::from(*name), Some(SharedString::from(*hint)))
                    })
                    .collect(),
                HANDOVERS.iter().position(|(h, ..)| *h == policy.handover),
                move |this, n, _, cx| {
                    let policy = KeyPolicy {
                        handover: HANDOVERS[n].0,
                        ..policy
                    };
                    this.set_server_key_rules(handover_key.clone(), policy, cx);
                },
                cx,
            )
            .into_any_element()
        } else {
            // Only the owner changes it: say which one holds, and what it means.
            let (_, name, hint) = HANDOVERS
                .iter()
                .find(|(h, ..)| *h == policy.handover)
                .copied()
                .unwrap_or(HANDOVERS[0]);
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(theme::body_size()).child(name))
                .child(muted(hint))
                .into_any_element()
        };
        let history_key = key.clone();
        let read_history = policy.new_members_read_history;
        groups.push(section(
            "This server",
            if owner {
                "Changing these never takes back keys someone already has."
            } else {
                "Only the owner can change these."
            },
            vec![
                setting_below("Who shares a channel's keys", "", handover),
                setting_fit(
                    "New members can read earlier messages",
                    "Off: each new member starts a new key and gets only that one.",
                    crate::ui::switch(
                        self,
                        "server-history",
                        read_history,
                        owner,
                        move |this, _, cx| {
                            let policy = KeyPolicy {
                                new_members_read_history: !read_history,
                                ..policy
                            };
                            this.set_server_key_rules(history_key.clone(), policy, cx);
                        },
                        cx,
                    ),
                ),
            ],
        ));

        // One line per channel, in columns, as the notification table has them.
        let channels = message_channels(s, space);
        let mut rows = vec![channel_rules_header()];
        for (channel, name) in &channels {
            rows.push(self.channel_rules_row(
                &key,
                *channel,
                name,
                page.keys.channels.get(channel).copied().unwrap_or_default(),
                admin,
                cx,
            ));
        }
        if channels.is_empty() {
            rows = vec![
                div()
                    .py(px(12.))
                    .child(muted("No channels yet."))
                    .into_any_element(),
            ];
        }
        groups.push(section(
            "Channels",
            if admin {
                "A channel can differ from the server. Server: it follows the rule above."
            } else {
                "A channel can differ from the server. Only the owner and administrators can \
                 change this."
            },
            rows,
        ));
        page_body("Encryption", Some(intro), groups)
    }

    fn waiting_row(
        &self,
        n: usize,
        inst: &InstanceId,
        space: Id,
        s: &InstanceStore,
        request: KeyRequest,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = s.display_name(Some(space), request.user_id);
        let channel = s
            .channel(request.channel_id)
            .and_then(|c| c.name.clone())
            .unwrap_or_default();
        let (approve_inst, approve_request) = (inst.clone(), request.clone());
        let dismiss_inst = inst.clone();
        setting_fit(
            &name,
            &format!("Wants to read #{channel} on a new device"),
            div()
                .flex()
                .gap(px(8.))
                .child(crate::ui::button(
                    self,
                    format!("key-dismiss-{n}"),
                    "Not now",
                    ButtonKind::Secondary,
                    true,
                    move |this, _, cx| {
                        this.core.dismiss_key_request(&dismiss_inst, &request);
                        cx.notify();
                    },
                    cx,
                ))
                .child(crate::ui::button(
                    self,
                    format!("key-approve-{n}"),
                    "Approve",
                    ButtonKind::Primary,
                    true,
                    move |this, _, cx| {
                        this.approve_key_request(approve_inst.clone(), approve_request.clone(), cx);
                    },
                    cx,
                )),
        )
    }

    fn channel_rules_row(
        &self,
        key: &(InstanceId, Id),
        channel: Id,
        name: &str,
        own: KeyPolicyOverride,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let handover_options = [
            ("Server", None),
            ("Automatic", Some(KeyHandover::Automatic)),
            ("Approved", Some(KeyHandover::Approved)),
        ];
        let history_options = [("Server", None), ("Yes", Some(true)), ("No", Some(false))];
        let (handover_key, history_key) = (key.clone(), key.clone());
        table_line(
            div()
                .truncate()
                .text_size(theme::body_size())
                .child(format!("#{name}"))
                .into_any_element(),
            choices(
                self,
                &format!("channel-handover-{channel}"),
                &handover_options,
                own.handover,
                enabled,
                cx,
                move |this, handover, cx| {
                    let policy = KeyPolicyOverride { handover, ..own };
                    this.set_channel_key_rules(handover_key.clone(), channel, policy, cx);
                },
            ),
            choices(
                self,
                &format!("channel-history-{channel}"),
                &history_options,
                own.new_members_read_history,
                enabled,
                cx,
                move |this, read, cx| {
                    let policy = KeyPolicyOverride {
                        new_members_read_history: read,
                        ..own
                    };
                    this.set_channel_key_rules(history_key.clone(), channel, policy, cx);
                },
            ),
        )
    }
}

/// The server's handover rules: the rule, its name and what it means.
const HANDOVERS: [(KeyHandover, &str, &str); 2] = [
    (
        KeyHandover::Automatic,
        "Automatic",
        "Any member's Hermes shares them by itself. Simple, but a server that lies about who \
         is a member could get them.",
    ),
    (
        KeyHandover::Approved,
        "Approved",
        "Someone with Manage Server lets each new device in, which stops that too.",
    ),
];

/// How wide the channel table's columns of choices are.
const HANDOVER_COL_W: f32 = 290.;
const HISTORY_COL_W: f32 = 200.;

/// A line of the channel table: the channel, then its two choices in their columns.
fn table_line(name: AnyElement, handover: AnyElement, history: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .py(px(8.))
        .child(div().flex_1().min_w(px(100.)).child(name))
        .child(div().w(px(HANDOVER_COL_W)).flex_shrink_0().child(handover))
        .child(div().w(px(HISTORY_COL_W)).flex_shrink_0().child(history))
        .into_any_element()
}

/// The channel table's column names.
fn channel_rules_header() -> AnyElement {
    let caption = |text: &'static str| {
        div()
            .text_size(theme::secondary_size())
            .text_color(theme::text_secondary())
            .child(text)
            .into_any_element()
    };
    table_line(
        caption("Channel"),
        caption("Who shares keys"),
        caption("Earlier messages"),
    )
}
