//! A small dialog with a few fields (PLAN.md section 15.4): create space, create channel, edit
//! channel, ban, nickname, invite, profile. What happens on submit depends on its [`Purpose`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, SharedString, Subscription, Window, div};
use hermes_client_core::proto::api::{
    BanRequest, CreateChannelRequest, CreateInviteRequest, OverrideRequest, UpdateChannelRequest,
    UpdateMemberRequest,
};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::{CoreError, InstanceId};

use crate::modal::{dialog, error_line, heading, paragraph};
use crate::shell::Shell;
use crate::theme::{self, px};

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

/// What the form is for, with everything the submit needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    CreateSpace(InstanceId),
    /// A text or voice channel, at the top level or in a category.
    CreateChannel {
        inst: InstanceId,
        space: Id,
        parent: Option<Id>,
    },
    CreateCategory {
        inst: InstanceId,
        space: Id,
    },
    EditChannel {
        inst: InstanceId,
        channel: Id,
    },
    Ban {
        inst: InstanceId,
        space: Id,
        user: Id,
    },
    Nickname {
        inst: InstanceId,
        space: Id,
        user: Id,
    },
    CreateInvite {
        inst: InstanceId,
        space: Id,
    },
    EditProfile(InstanceId),
    ChangePassword(InstanceId),
}

pub enum Field {
    Text {
        label: String,
        hint: String,
        input: Entity<InputState>,
    },
    Choice {
        label: String,
        hint: String,
        options: Vec<String>,
        selected: String,
    },
    Check {
        label: String,
        hint: String,
        value: bool,
    },
}

pub struct Form {
    pub purpose: Purpose,
    pub title: String,
    pub text: String,
    pub fields: Vec<Field>,
    pub submit: &'static str,
    /// The submit button is red (ban).
    pub danger: bool,
    pub busy: bool,
    pub error: Option<String>,
    _inputs: Vec<Subscription>,
}

impl Form {
    pub fn new(purpose: Purpose, title: impl Into<String>, submit: &'static str) -> Form {
        Form {
            purpose,
            title: title.into(),
            text: String::new(),
            fields: Vec::new(),
            submit,
            danger: false,
            busy: false,
            error: None,
            _inputs: Vec::new(),
        }
    }

    pub fn text(mut self, text: impl Into<String>) -> Form {
        self.text = text.into();
        self
    }

    pub fn danger(mut self) -> Form {
        self.danger = true;
        self
    }

    /// The trimmed value of text field `i` (empty if it is not one).
    fn value(&self, i: usize, cx: &Context<Shell>) -> String {
        self.raw(i, cx).trim().to_owned()
    }

    /// The value of text field `i` as typed (passwords).
    fn raw(&self, i: usize, cx: &Context<Shell>) -> String {
        match self.fields.get(i) {
            Some(Field::Text { input, .. }) => input.read(cx).value().to_string(),
            _ => String::new(),
        }
    }

    /// The selected option of choice field `i`.
    fn selected(&self, i: usize) -> &str {
        match self.fields.get(i) {
            Some(Field::Choice { selected, .. }) => selected,
            _ => "",
        }
    }

    fn checked(&self, i: usize) -> bool {
        matches!(self.fields.get(i), Some(Field::Check { value: true, .. }))
    }

    fn required(&self, i: usize, what: &str, cx: &Context<Shell>) -> Result<String, String> {
        let value = self.value(i, cx);
        if value.is_empty() {
            Err(format!("Please enter {what}."))
        } else {
            Ok(value)
        }
    }
}

/// Builds the fields of a form: text fields need the window.
struct Fields<'a, 'b> {
    form: Form,
    window: &'a mut Window,
    cx: &'a mut Context<'b, Shell>,
}

impl Fields<'_, '_> {
    fn text(self, label: &str, placeholder: &str, value: impl Into<String>) -> Self {
        self.input(label, "", placeholder, value.into(), false)
    }

    fn text_hint(
        self,
        label: &str,
        hint: &str,
        placeholder: &str,
        value: impl Into<String>,
    ) -> Self {
        self.input(label, hint, placeholder, value.into(), false)
    }

    fn password(self, label: &str) -> Self {
        self.input(label, "", "", String::new(), true)
    }

    fn input(
        mut self,
        label: &str,
        hint: &str,
        placeholder: &str,
        value: String,
        secure: bool,
    ) -> Self {
        let placeholder = placeholder.to_owned();
        let input = self.cx.new(|cx| {
            let mut state = InputState::new(self.window, cx)
                .placeholder(placeholder)
                .masked(secure);
            state.set_value(value, self.window, cx);
            state
        });
        let subscription = self.cx.subscribe_in(
            &input,
            self.window,
            |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit_form(window, cx),
                InputEvent::Change => {
                    if let Some(form) = &mut this.form {
                        form.error = None;
                    }
                }
                _ => {}
            },
        );
        self.form._inputs.push(subscription);
        self.form.fields.push(Field::Text {
            label: label.to_owned(),
            hint: hint.to_owned(),
            input,
        });
        self
    }

    fn choice(mut self, label: &str, hint: &str, options: &[&str], selected: &str) -> Self {
        self.form.fields.push(Field::Choice {
            label: label.to_owned(),
            hint: hint.to_owned(),
            options: options.iter().map(|o| (*o).to_owned()).collect(),
            selected: selected.to_owned(),
        });
        self
    }

    fn check(mut self, label: &str, hint: &str, value: bool) -> Self {
        self.form.fields.push(Field::Check {
            label: label.to_owned(),
            hint: hint.to_owned(),
            value,
        });
        self
    }
}

impl Shell {
    fn fields<'a, 'b>(
        form: Form,
        window: &'a mut Window,
        cx: &'a mut Context<'b, Shell>,
    ) -> Fields<'a, 'b> {
        Fields { form, window, cx }
    }

    pub fn open_form_create_space(
        &mut self,
        inst: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(Purpose::CreateSpace(inst), "Create a space", "Create")
            .text("A space is a home for your group, with its own channels, roles and members.");
        self.form = None;
        let built = Self::fields(form, window, cx).text("Name", "e.g. Friends", "");
        self.finish(built);
    }

    /// `Fields::open` needs the entity, which is being updated; this sets it directly.
    fn finish(&mut self, fields: Fields<'_, '_>) {
        let Fields { form, window, cx } = fields;
        if let Some(Field::Text { input, .. }) =
            form.fields.iter().find(|f| matches!(f, Field::Text { .. }))
        {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        self.form = Some(form);
        cx.notify();
    }

    pub fn open_form_create_channel(
        &mut self,
        inst: InstanceId,
        space: Id,
        parent: Option<Id>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let form = Form::new(
            Purpose::CreateChannel {
                inst,
                space,
                parent,
            },
            "Create a channel",
            "Create",
        );
        let mut fields = Self::fields(form, window, cx)
            .text("Name", "e.g. general", "")
            .choice(
                "Kind",
                "Voice channels also have a text chat.",
                &["Text", "Voice"],
                "Text",
            )
            .check(
                "Private channel",
                "Only the roles you tick below (and administrators) can see it.",
                false,
            );
        for role in roles {
            fields = fields.check(&format!("Allow {role}"), "", false);
        }
        self.finish(fields);
    }

    pub fn open_form_create_category(
        &mut self,
        inst: InstanceId,
        space: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(
            Purpose::CreateCategory { inst, space },
            "Create a category",
            "Create",
        )
        .text("Categories group channels in the sidebar. Drag channels onto it afterwards.");
        let fields = Self::fields(form, window, cx).text("Name", "e.g. Games", "");
        self.finish(fields);
    }

    pub fn open_form_edit_channel(
        &mut self,
        inst: InstanceId,
        channel: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let c = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.channel(channel).cloned())
        };
        let Some(c) = c else { return };
        let title = match c.kind {
            ChannelKind::Category => "Edit category",
            ChannelKind::GroupDm => "Rename group",
            _ => "Edit channel",
        };
        let form = Form::new(Purpose::EditChannel { inst, channel }, title, "Save");
        let mut fields =
            Self::fields(form, window, cx).text("Name", "", c.name.clone().unwrap_or_default());
        if matches!(c.kind, ChannelKind::Text | ChannelKind::Voice) {
            let slow = SLOWMODE
                .iter()
                .find(|(_, v)| *v == c.slowmode_secs)
                .map_or("Off", |(l, _)| *l);
            fields = fields
                .text(
                    "Topic",
                    "What is this channel about?",
                    c.topic.clone().unwrap_or_default(),
                )
                .choice(
                    "Slow mode",
                    "How long members wait between messages.",
                    &labels(&SLOWMODE),
                    slow,
                );
        }
        if c.kind == ChannelKind::Voice {
            fields = fields
                .text_hint(
                    "User limit",
                    "People with Move members can always join.",
                    "0 = no limit",
                    c.user_limit.unwrap_or(0).to_string(),
                )
                .text(
                    "Bitrate (kbit/s)",
                    "64",
                    c.bitrate
                        .map_or_else(|| "64".to_owned(), |b| (b / 1000).to_string()),
                );
        }
        self.finish(fields);
    }

    pub fn open_form_ban(
        &mut self,
        inst: InstanceId,
        space: Id,
        user: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = {
            let store = self.core.store();
            store
                .instance(&inst)
                .map(|s| s.display_name(Some(space), user))
                .unwrap_or_default()
        };
        let form = Form::new(
            Purpose::Ban { inst, space, user },
            format!("Ban {name}?"),
            "Ban",
        )
        .text("They are removed from the space and cannot rejoin until unbanned.")
        .danger();
        let fields = Self::fields(form, window, cx)
            .text("Reason (optional)", "Shown in the ban list", "")
            .choice(
                "Delete their recent messages",
                "",
                &labels(&BAN_DELETE),
                BAN_DELETE[0].0,
            );
        self.finish(fields);
    }

    pub fn open_form_nickname(
        &mut self,
        inst: InstanceId,
        space: Id,
        user: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.member(space, user))
                .and_then(|m| m.nickname.clone())
                .unwrap_or_default()
        };
        let form = Form::new(
            Purpose::Nickname { inst, space, user },
            "Change nickname",
            "Save",
        )
        .text("A nickname is only used in this space. Leave it empty to remove it.");
        let fields = Self::fields(form, window, cx).text("Nickname", "", current);
        self.finish(fields);
    }

    pub fn open_form_invite(
        &mut self,
        inst: InstanceId,
        space: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(
            Purpose::CreateInvite { inst, space },
            "Create an invite link",
            "Create and copy",
        );
        let fields = Self::fields(form, window, cx)
            .choice("Expires after", "", &labels(&INVITE_EXPIRY), "7 days")
            .choice("Max uses", "", &labels(&INVITE_USES), "No limit");
        self.finish(fields);
    }

    pub fn open_form_profile(
        &mut self,
        inst: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = {
            let store = self.core.store();
            store.instance(&inst).and_then(|s| s.me.clone())
        };
        let Some(me) = me else { return };
        let form = Form::new(Purpose::EditProfile(inst), "Edit profile", "Save")
            .text(format!("Signed in as @{}.", me.user.username));
        let fields = Self::fields(form, window, cx)
            .text_hint(
                "Display name",
                "Leave empty to use your username.",
                "Shown instead of your username",
                me.user.display_name.clone().unwrap_or_default(),
            )
            .text(
                "Status",
                "e.g. Working on a project",
                me.user.status_text.clone().unwrap_or_default(),
            );
        self.finish(fields);
    }

    pub fn open_form_password(
        &mut self,
        inst: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(Purpose::ChangePassword(inst), "Change password", "Change")
            .text("Your other devices are signed out afterwards.");
        let fields = Self::fields(form, window, cx)
            .password("Current password")
            .password("New password")
            .password("New password again");
        self.finish(fields);
    }

    fn form_done(&mut self, result: Result<String, String>, cx: &mut Context<Self>) {
        match result {
            Ok(text) => {
                self.form = None;
                self.info(text, cx);
            }
            Err(err) => {
                if let Some(form) = &mut self.form {
                    form.busy = false;
                    form.error = Some(err);
                } else {
                    self.error(err, cx);
                }
            }
        }
        cx.notify();
    }

    pub fn submit_form(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.form.take() else { return };
        if form.busy {
            self.form = Some(form);
            return;
        }
        let result = self.start_submit(&form, cx);
        self.form = Some(form);
        let form = self.form.as_mut().expect("just put back");
        match result {
            Ok(()) => form.busy = true,
            Err(problem) => form.error = Some(problem),
        }
        cx.notify();
    }

    /// Starts the request of a submitted form, or says what to fix first.
    fn start_submit(&self, form: &Form, cx: &mut Context<Self>) -> Result<(), String> {
        let core = self.core.clone();
        match form.purpose.clone() {
            Purpose::CreateSpace(inst) => {
                let name = form.required(0, "a name", cx)?;
                self.form_call(cx, async move {
                    let space = core.rest(&inst)?.create_space(&name).await?;
                    Ok(format!("Created {}", space.space.name))
                });
            }
            Purpose::CreateCategory { inst, space } => {
                let name = form.required(0, "a name", cx)?;
                self.form_call(cx, async move {
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
                });
            }
            Purpose::CreateChannel {
                inst,
                space,
                parent,
            } => {
                let voice = form.selected(1) == "Voice";
                let name = channel_name(&form.required(0, "a name", cx)?, voice);
                let kind = if voice {
                    ChannelKind::Voice
                } else {
                    ChannelKind::Text
                };
                let private = form.checked(2);
                // The roles ticked for a private channel, by the order the form listed them.
                let allowed: Vec<Id> = {
                    let store = self.core.store();
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
                self.form_call(cx, async move {
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
                });
            }
            Purpose::EditChannel { inst, channel } => {
                let kind = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.channel(channel))
                        .map(|c| c.kind)
                };
                let kind = kind.ok_or("the channel is gone")?;
                let name = form.required(0, "a name", cx)?;
                let mut req = UpdateChannelRequest {
                    name: Some(if kind.is_private() || kind == ChannelKind::Category {
                        name
                    } else {
                        channel_name(&name, kind == ChannelKind::Voice)
                    }),
                    ..UpdateChannelRequest::default()
                };
                if matches!(kind, ChannelKind::Text | ChannelKind::Voice) {
                    let topic = form.value(1, cx);
                    req.topic = Some((!topic.is_empty()).then_some(topic));
                    req.slowmode_secs = lookup(&SLOWMODE, form.selected(2));
                }
                if kind == ChannelKind::Voice {
                    let limit: u32 = form
                        .value(3, cx)
                        .parse()
                        .map_err(|_| "the user limit must be a number (0 = no limit)")?;
                    req.user_limit = Some((limit > 0).then_some(limit));
                    let kbit: u32 = form
                        .value(4, cx)
                        .parse()
                        .map_err(|_| "the bitrate must be a number, like 64")?;
                    if !(8..=510).contains(&kbit) {
                        return Err("the bitrate must be between 8 and 510 kbit/s".into());
                    }
                    req.bitrate = Some(kbit * 1000);
                }
                self.form_call(cx, async move {
                    core.rest(&inst)?.update_channel(channel, &req).await?;
                    Ok("Saved".to_owned())
                });
            }
            Purpose::Ban { inst, space, user } => {
                let reason = form.value(0, cx);
                let req = BanRequest {
                    reason: (!reason.is_empty()).then_some(reason),
                    delete_messages_hours: lookup(&BAN_DELETE, form.selected(1)).filter(|h| *h > 0),
                };
                self.form_call(cx, async move {
                    core.rest(&inst)?.ban(space, user, &req).await?;
                    Ok("Banned".to_owned())
                });
            }
            Purpose::Nickname { inst, space, user } => {
                let nick = form.value(0, cx);
                let me = self.core.store().instance(&inst).and_then(|s| s.me_id()) == Some(user);
                let req = UpdateMemberRequest {
                    nickname: Some((!nick.is_empty()).then_some(nick)),
                    role_ids: None,
                };
                self.form_call(cx, async move {
                    core.rest(&inst)?
                        .update_member(space, (!me).then_some(user), &req)
                        .await?;
                    Ok("Nickname changed".to_owned())
                });
            }
            Purpose::CreateInvite { inst, space } => {
                let req = CreateInviteRequest {
                    expires_in_secs: lookup(&INVITE_EXPIRY, form.selected(0)),
                    max_uses: lookup(&INVITE_USES, form.selected(1)).filter(|n| *n > 0),
                };
                let target = inst.clone();
                self.run(
                    cx,
                    async move {
                        let invite = core.rest(&target)?.create_invite(space, &req).await?;
                        Ok::<_, CoreError>(core.invite_link(&target, &invite.code))
                    },
                    move |this, result, _, cx| match result {
                        Ok(Some(link)) => {
                            this.form = None;
                            this.copy(link, "Invite link", cx);
                            this.invite_created(inst, space, cx);
                        }
                        Ok(None) => this.form_done(Err("no such instance".into()), cx),
                        Err(err) => this.form_done(Err(err.to_string()), cx),
                    },
                );
            }
            Purpose::ChangePassword(inst) => {
                let (current, new, again) = (form.raw(0, cx), form.raw(1, cx), form.raw(2, cx));
                if current.is_empty() || new.is_empty() {
                    return Err("Please fill in both passwords.".into());
                }
                if new != again {
                    return Err("The new passwords are not the same.".into());
                }
                self.form_call(cx, async move {
                    core.rest(&inst)?
                        .change_password(&hermes_client_core::proto::api::ChangePasswordRequest {
                            current_password: current,
                            new_password: new,
                        })
                        .await?;
                    Ok("Password changed; other devices were signed out".to_owned())
                });
            }
            Purpose::EditProfile(inst) => {
                let (display, status) = (form.value(0, cx), form.value(1, cx));
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
                self.form_call(cx, async move {
                    core.rest(&inst)?.update_me(&req).await?;
                    Ok("Profile saved".to_owned())
                });
            }
        }
        Ok(())
    }

    fn form_call(
        &self,
        cx: &mut Context<Self>,
        work: impl std::future::Future<Output = Result<String, CoreError>> + Send + 'static,
    ) {
        self.run(cx, work, |this, result, _, cx| {
            this.form_done(result.map_err(|e| e.to_string()), cx);
        });
    }

    pub fn render_form(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.form.as_ref()?;
        let mut content: Vec<AnyElement> = vec![heading(form.title.clone()).into_any_element()];
        if !form.text.is_empty() {
            content.push(paragraph(form.text.clone()).into_any_element());
        }
        for (i, field) in form.fields.iter().enumerate() {
            content.push(render_field(i, field, cx));
        }
        if let Some(error) = &form.error {
            content.push(error_line(error.clone()).into_any_element());
        }
        let cancel = Button::new("form-cancel")
            .cursor_pointer()
            .large()
            .label("Cancel")
            .on_click(cx.listener(|this, _, _, cx| {
                this.form = None;
                cx.notify();
            }));
        let label = if form.busy {
            "Please wait…"
        } else {
            form.submit
        };
        let submit = Button::new("form-submit")
            .cursor_pointer()
            .large()
            .label(label)
            .disabled(form.busy)
            .when(form.busy, |b| b.cursor_default())
            .when(form.danger, |b| b.danger())
            .when(!form.danger, |b| b.primary())
            .on_click(cx.listener(|this, _, window, cx| this.submit_form(window, cx)));
        content.push(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(cancel)
                .child(submit)
                .into_any_element(),
        );
        Some(dialog(
            "form",
            480.,
            div().flex().flex_col().gap(px(14.)).children(content),
        ))
    }
}

fn render_field(i: usize, field: &Field, cx: &mut Context<Shell>) -> AnyElement {
    let hint_line = |hint: &str| {
        (!hint.is_empty()).then(|| {
            div()
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child(SharedString::from(hint.to_owned()))
        })
    };
    let name_line = |name: &str| {
        div()
            .text_size(theme::secondary_size())
            .text_color(theme::text_secondary())
            .child(SharedString::from(name.to_owned()))
    };
    let column = div().flex().flex_col().gap(px(6.));
    match field {
        Field::Text { label, hint, input } => column
            .child(name_line(label))
            .child(Input::new(input).large())
            .children(hint_line(hint))
            .into_any_element(),
        Field::Choice {
            label,
            hint,
            options,
            selected,
        } => {
            let chips = options.iter().enumerate().map(|(n, option)| {
                let chosen = option == selected;
                let value = option.clone();
                div()
                    .id(SharedString::from(format!("form-{i}-{n}")))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(theme::secondary_size())
                    .when(chosen, |d| {
                        d.bg(theme::accent()).text_color(theme::on_accent())
                    })
                    .when(!chosen, |d| {
                        d.bg(theme::selected()).hover(|d| d.bg(theme::hover()))
                    })
                    .child(SharedString::from(option.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(Field::Choice { selected, .. }) =
                            this.form.as_mut().and_then(|f| f.fields.get_mut(i))
                        {
                            selected.clone_from(&value);
                        }
                        cx.notify();
                    }))
            });
            column
                .child(name_line(label))
                .child(div().flex().flex_wrap().gap(px(6.)).children(chips))
                .children(hint_line(hint))
                .into_any_element()
        }
        Field::Check { label, hint, value } => column
            .child(
                Checkbox::new(SharedString::from(format!("form-check-{i}")))
                    .cursor_pointer()
                    .label(SharedString::from(label.clone()))
                    .checked(*value)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if let Some(Field::Check { value, .. }) =
                            this.form.as_mut().and_then(|f| f.fields.get_mut(i))
                        {
                            *value = *checked;
                        }
                        cx.notify();
                    })),
            )
            .children(hint_line(hint))
            .into_any_element(),
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
