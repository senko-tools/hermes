//! The small dialogs (see `modal::form`): what each one asks and what its submit does.

use hermes_client_core::proto::api::{
    BanRequest, CreateChannelRequest, CreateInviteRequest, OverrideRequest, UpdateChannelRequest,
    UpdateMemberRequest,
};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::{CoreError, InstanceId};
use iced::Task;
use iced::widget::operation;

use super::App;
use crate::message::Message;
use crate::modal::form::{self, Field, Form, Purpose};

const SLOWMODE: [(&str, u32); 8] = [
    ("Off", 0),
    ("5 seconds", 5),
    ("10 seconds", 10),
    ("30 seconds", 30),
    ("1 minute", 60),
    ("5 minutes", 300),
    ("15 minutes", 900),
    ("1 hour", 3600),
];

const BAN_DELETE: [(&str, u32); 4] = [
    ("Don't delete any", 0),
    ("From the last hour", 1),
    ("From the last 24 hours", 24),
    ("From the last 7 days", 168),
];

const INVITE_EXPIRY: [(&str, u64); 5] = [
    ("30 minutes", 30 * 60),
    ("1 day", 24 * 60 * 60),
    ("7 days", 7 * 24 * 60 * 60),
    ("30 days", 30 * 24 * 60 * 60),
    ("Never", 0),
];

const INVITE_USES: [(&str, u32); 5] = [
    ("No limit", 0),
    ("1 use", 1),
    ("5 uses", 5),
    ("10 uses", 10),
    ("25 uses", 25),
];

fn labels<T>(table: &[(&'static str, T)]) -> Vec<&'static str> {
    table.iter().map(|(l, _)| *l).collect()
}

fn lookup<T: Copy>(table: &[(&'static str, T)], label: &str) -> Option<T> {
    table.iter().find(|(l, _)| *l == label).map(|(_, v)| *v)
}

impl App {
    fn open_form(&mut self, form: Form) -> Task<Message> {
        self.form = Some(form);
        operation::focus(iced::widget::Id::new(form::FIRST_INPUT))
    }

    pub(super) fn open_form_create_space(&mut self, inst: InstanceId) -> Task<Message> {
        self.open_form(
            Form::new(Purpose::CreateSpace(inst), "Create a space", "Create")
                .text("A space is a home for your group, with its own channels, roles and members.")
                .field(Field::text("Name", "e.g. Friends", "")),
        )
    }

    pub(super) fn open_form_create_channel(
        &mut self,
        inst: InstanceId,
        space: Id,
        parent: Option<Id>,
    ) -> Task<Message> {
        let roles: Vec<String> = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.spaces.get(&space))
                .map(|st| {
                    st.roles
                        .iter()
                        .filter(|r| !r.is_everyone())
                        .map(|r| r.name.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut form = Form::new(
            Purpose::CreateChannel {
                inst,
                space,
                parent,
            },
            "Create a channel",
            "Create",
        )
        .field(Field::text("Name", "e.g. general", ""))
        .field(
            Field::choice("Kind", &["Text", "Voice"], "Text")
                .hint("Voice channels also have a text chat."),
        )
        .field(
            Field::check("Private channel", false)
                .hint("Only the roles you tick below (and administrators) can see it."),
        );
        for role in roles {
            form = form.field(Field::check(format!("Allow {role}"), false));
        }
        self.open_form(form)
    }

    pub(super) fn open_form_create_category(
        &mut self,
        inst: InstanceId,
        space: Id,
    ) -> Task<Message> {
        self.open_form(
            Form::new(
                Purpose::CreateCategory { inst, space },
                "Create a category",
                "Create",
            )
            .text("Categories group channels in the sidebar. Drag channels onto it afterwards.")
            .field(Field::text("Name", "e.g. Games", "")),
        )
    }

    pub(super) fn open_form_edit_channel(
        &mut self,
        inst: InstanceId,
        channel: Id,
    ) -> Task<Message> {
        let c = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.channel(channel).cloned())
        };
        let Some(c) = c else {
            return Task::none();
        };
        let name = c.name.clone().unwrap_or_default();
        let title = match c.kind {
            ChannelKind::Category => "Edit category",
            ChannelKind::GroupDm => "Rename group",
            _ => "Edit channel",
        };
        let mut form = Form::new(Purpose::EditChannel { inst, channel }, title, "Save")
            .field(Field::text("Name", "", name));
        if matches!(c.kind, ChannelKind::Text | ChannelKind::Voice) {
            form = form.field(Field::text(
                "Topic",
                "What is this channel about?",
                c.topic.clone().unwrap_or_default(),
            ));
            let slow = SLOWMODE
                .iter()
                .find(|(_, v)| *v == c.slowmode_secs)
                .map_or("Off", |(l, _)| *l);
            form = form.field(
                Field::choice("Slow mode", &labels(&SLOWMODE), slow)
                    .hint("How long members wait between messages."),
            );
        }
        if c.kind == ChannelKind::Voice {
            form = form.field(
                Field::text(
                    "User limit",
                    "0 = no limit",
                    c.user_limit.unwrap_or(0).to_string(),
                )
                .hint("People with Move members can always join."),
            );
            form = form.field(Field::text(
                "Bitrate (kbit/s)",
                "64",
                c.bitrate
                    .map_or_else(|| "64".to_owned(), |b| (b / 1000).to_string()),
            ));
        }
        self.open_form(form)
    }

    pub(super) fn open_form_ban(&mut self, inst: InstanceId, space: Id, user: Id) -> Task<Message> {
        let name = {
            let store = self.core.store();
            store
                .instance(&inst)
                .map(|s| s.display_name(Some(space), user))
                .unwrap_or_default()
        };
        self.open_form(
            Form::new(
                Purpose::Ban { inst, space, user },
                format!("Ban {name}?"),
                "Ban",
            )
            .text("They are removed from the space and cannot rejoin until unbanned.")
            .field(Field::text(
                "Reason (optional)",
                "Shown in the ban list",
                "",
            ))
            .field(Field::choice(
                "Delete their recent messages",
                &labels(&BAN_DELETE),
                BAN_DELETE[0].0,
            ))
            .danger(),
        )
    }

    pub(super) fn open_form_nickname(
        &mut self,
        inst: InstanceId,
        space: Id,
        user: Id,
    ) -> Task<Message> {
        let current = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.member(space, user))
                .and_then(|m| m.nickname.clone())
                .unwrap_or_default()
        };
        self.open_form(
            Form::new(
                Purpose::Nickname { inst, space, user },
                "Change nickname",
                "Save",
            )
            .text("A nickname is only used in this space. Leave it empty to remove it.")
            .field(Field::text("Nickname", "", current)),
        )
    }

    pub(super) fn open_form_invite(&mut self, inst: InstanceId, space: Id) -> Task<Message> {
        self.open_form(
            Form::new(
                Purpose::CreateInvite { inst, space },
                "Create an invite link",
                "Create and copy",
            )
            .field(Field::choice(
                "Expires after",
                &labels(&INVITE_EXPIRY),
                "7 days",
            ))
            .field(Field::choice("Max uses", &labels(&INVITE_USES), "No limit")),
        )
    }

    pub(super) fn open_form_profile(&mut self, inst: InstanceId) -> Task<Message> {
        let me = {
            let store = self.core.store();
            store.instance(&inst).and_then(|s| s.me.clone())
        };
        let Some(me) = me else {
            return Task::none();
        };
        self.open_form(
            Form::new(Purpose::EditProfile(inst), "Edit profile", "Save")
                .text(format!("Signed in as @{}.", me.user.username))
                .field(
                    Field::text(
                        "Display name",
                        "Shown instead of your username",
                        me.user.display_name.clone().unwrap_or_default(),
                    )
                    .hint("Leave empty to use your username."),
                )
                .field(Field::text(
                    "Status",
                    "e.g. Working on a project",
                    me.user.status_text.clone().unwrap_or_default(),
                )),
        )
    }

    pub(super) fn on_form(&mut self, message: form::Message) -> Task<Message> {
        if message == form::Message::Cancel {
            self.form = None;
            return Task::none();
        }
        let Some(form) = &mut self.form else {
            return Task::none();
        };
        if !form.update(message) {
            return Task::none();
        }
        match submit(self, self.form.clone().expect("checked above")) {
            Ok(task) => {
                if let Some(form) = &mut self.form {
                    form.busy = true;
                }
                task
            }
            Err(problem) => {
                if let Some(form) = &mut self.form {
                    form.error = Some(problem);
                }
                Task::none()
            }
        }
    }

    pub(super) fn on_form_done(&mut self, result: Result<String, String>) {
        match result {
            Ok(text) => {
                self.form = None;
                self.info(text);
            }
            Err(err) => {
                if let Some(form) = &mut self.form {
                    form.busy = false;
                    form.error = Some(err);
                } else {
                    self.error(err);
                }
            }
        }
    }
}

/// The request for a submitted form, or what to fix first.
fn submit(app: &App, form: Form) -> Result<Task<Message>, String> {
    let core = app.core.clone();
    let done = |r: Result<String, CoreError>| Message::FormDone(r.map_err(|e| e.to_string()));
    match form.purpose.clone() {
        Purpose::CreateSpace(inst) => {
            let name = required(&form, 0, "a name")?;
            Ok(Task::perform(
                async move {
                    let space = core.rest(&inst)?.create_space(&name).await?;
                    Ok(format!("Created {}", space.space.name))
                },
                done,
            ))
        }
        Purpose::CreateCategory { inst, space } => {
            let name = required(&form, 0, "a name")?;
            Ok(Task::perform(
                async move {
                    core.rest(&inst)?
                        .create_channel(
                            space,
                            &CreateChannelRequest {
                                kind: ChannelKind::Category,
                                name: name.clone(),
                                parent_id: None,
                                topic: None,
                                bitrate: None,
                                user_limit: None,
                            },
                        )
                        .await?;
                    Ok(format!("Created category {name}"))
                },
                done,
            ))
        }
        Purpose::CreateChannel {
            inst,
            space,
            parent,
        } => {
            let name = channel_name(&required(&form, 0, "a name")?, form.selected(1) == "Voice");
            let kind = if form.selected(1) == "Voice" {
                ChannelKind::Voice
            } else {
                ChannelKind::Text
            };
            let private = form.checked(2);
            // The roles ticked for a private channel, by the order the form listed them.
            let allowed: Vec<Id> = {
                let store = app.core.store();
                store
                    .instance(&inst)
                    .and_then(|s| s.spaces.get(&space))
                    .map(|st| {
                        st.roles
                            .iter()
                            .filter(|r| !r.is_everyone())
                            .enumerate()
                            .filter(|(i, _)| form.checked(3 + i))
                            .map(|(_, r)| r.id)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            Ok(Task::perform(
                async move {
                    let rest = core.rest(&inst)?;
                    let channel = rest
                        .create_channel(
                            space,
                            &CreateChannelRequest {
                                kind,
                                name: name.clone(),
                                parent_id: parent,
                                topic: None,
                                bitrate: None,
                                user_limit: None,
                            },
                        )
                        .await?;
                    if private {
                        // @everyone has the space's ID.
                        rest.set_override(
                            channel.id,
                            space,
                            &OverrideRequest {
                                allow: Permissions::empty(),
                                deny: Permissions::VIEW_CHANNEL,
                            },
                        )
                        .await?;
                        for role in allowed {
                            rest.set_override(
                                channel.id,
                                role,
                                &OverrideRequest {
                                    allow: Permissions::VIEW_CHANNEL,
                                    deny: Permissions::empty(),
                                },
                            )
                            .await?;
                        }
                    }
                    Ok(format!(
                        "Created {}{name}",
                        if kind == ChannelKind::Voice { "" } else { "#" }
                    ))
                },
                done,
            ))
        }
        Purpose::EditChannel { inst, channel } => {
            let kind = {
                let store = app.core.store();
                store
                    .instance(&inst)
                    .and_then(|s| s.channel(channel))
                    .map(|c| c.kind)
            };
            let kind = kind.ok_or("the channel is gone")?;
            let name = required(&form, 0, "a name")?;
            let mut req = UpdateChannelRequest {
                name: Some(if kind.is_private() || kind == ChannelKind::Category {
                    name
                } else {
                    channel_name(&name, kind == ChannelKind::Voice)
                }),
                ..UpdateChannelRequest::default()
            };
            if matches!(kind, ChannelKind::Text | ChannelKind::Voice) {
                let topic = form.value(1).to_owned();
                req.topic = Some((!topic.is_empty()).then_some(topic));
                req.slowmode_secs = lookup(&SLOWMODE, form.selected(2));
            }
            if kind == ChannelKind::Voice {
                let limit: u32 = form
                    .value(3)
                    .parse()
                    .map_err(|_| "the user limit must be a number (0 = no limit)")?;
                req.user_limit = Some((limit > 0).then_some(limit));
                let kbit: u32 = form
                    .value(4)
                    .parse()
                    .map_err(|_| "the bitrate must be a number, like 64")?;
                if !(8..=510).contains(&kbit) {
                    return Err("the bitrate must be between 8 and 510 kbit/s".into());
                }
                req.bitrate = Some(kbit * 1000);
            }
            Ok(Task::perform(
                async move {
                    core.rest(&inst)?.update_channel(channel, &req).await?;
                    Ok("Saved".to_owned())
                },
                done,
            ))
        }
        Purpose::Ban { inst, space, user } => {
            let reason = form.value(0).to_owned();
            let req = BanRequest {
                reason: (!reason.is_empty()).then_some(reason),
                delete_messages_hours: lookup(&BAN_DELETE, form.selected(1)).filter(|h| *h > 0),
            };
            Ok(Task::perform(
                async move {
                    core.rest(&inst)?.ban(space, user, &req).await?;
                    Ok("Banned".to_owned())
                },
                done,
            ))
        }
        Purpose::Nickname { inst, space, user } => {
            let nick = form.value(0).to_owned();
            let me = app.core.store().instance(&inst).and_then(|s| s.me_id()) == Some(user);
            let req = UpdateMemberRequest {
                nickname: Some((!nick.is_empty()).then_some(nick)),
                role_ids: None,
            };
            Ok(Task::perform(
                async move {
                    core.rest(&inst)?
                        .update_member(space, (!me).then_some(user), &req)
                        .await?;
                    Ok("Nickname changed".to_owned())
                },
                done,
            ))
        }
        Purpose::CreateInvite { inst, space } => {
            let req = CreateInviteRequest {
                expires_in_secs: lookup(&INVITE_EXPIRY, form.selected(0)),
                max_uses: lookup(&INVITE_USES, form.selected(1)).filter(|n| *n > 0),
            };
            let inst2 = inst.clone();
            Ok(Task::perform(
                async move {
                    let invite = core.rest(&inst2)?.create_invite(space, &req).await?;
                    Ok::<_, CoreError>(core.invite_link(&inst2, &invite.code))
                },
                move |r| match r {
                    Ok(Some(link)) => Message::InviteCreated(inst.clone(), space, link),
                    Ok(None) => Message::FormDone(Err("no such instance".into())),
                    Err(err) => Message::FormDone(Err(err.to_string())),
                },
            ))
        }
        Purpose::ChangePassword(inst) => app.submit_password(
            inst,
            form.raw(0).to_owned(),
            form.raw(1).to_owned(),
            form.raw(2).to_owned(),
        ),
        Purpose::EditProfile(inst) => {
            let display = form.value(0).to_owned();
            let status = form.value(1).to_owned();
            if display.chars().count() > 32 {
                return Err("the display name can be at most 32 characters".into());
            }
            if status.chars().count() > 128 {
                return Err("the status can be at most 128 characters".into());
            }
            let req = hermes_client_core::proto::api::UpdateMeRequest {
                display_name: Some((!display.is_empty()).then_some(display)),
                status_text: Some((!status.is_empty()).then_some(status)),
                avatar_id: None,
            };
            Ok(Task::perform(
                async move {
                    core.rest(&inst)?.update_me(&req).await?;
                    Ok("Profile saved".to_owned())
                },
                done,
            ))
        }
    }
}

fn required(form: &Form, i: usize, what: &str) -> Result<String, String> {
    let value = form.value(i);
    if value.is_empty() {
        Err(format!("Please enter {what}."))
    } else {
        Ok(value.to_owned())
    }
}

/// Text channel names are lowercase with dashes (`My Room` → `my-room`); voice channel names
/// keep their spelling.
pub fn channel_name(name: &str, voice: bool) -> String {
    if voice {
        return name.trim().to_owned();
    }
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_whitespace() || c == '-' {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_channel_names_are_slugged() {
        assert_eq!(channel_name("My  Cool Room", false), "my-cool-room");
        assert_eq!(channel_name(" -memes- ", false), "memes");
        assert_eq!(channel_name("Game Night", true), "Game Night");
        assert_eq!(lookup(&SLOWMODE, "1 minute"), Some(60));
        assert_eq!(lookup(&SLOWMODE, "nope"), None);
    }
}
