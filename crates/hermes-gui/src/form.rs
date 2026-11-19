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

const BAN_LENGTH: [(&str, u32); 5] = [
    ("Until lifted", 0),
    ("1 hour", 1),
    ("1 day", 24),
    ("7 days", 168),
    ("30 days", 720),
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
        /// The roles of the "Allow …" checkboxes, in their order, as the form listed them.
        roles: Vec<Id>,
    },
    CreateCategory {
        inst: InstanceId,
        space: Id,
    },
    EditChannel {
        inst: InstanceId,
        channel: Id,
        /// The categories of the "Category" choice after "None", in their order, and the one
        /// the channel was in when the form opened.
        categories: Vec<Id>,
        parent: Option<Id>,
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
    /// `bio`: the form has an "About me" field (the server has the `bio` feature).
    EditProfile {
        inst: InstanceId,
        bio: bool,
    },
    ChangePassword(InstanceId),
    LoginCode {
        host: hermes_client_core::address::HostPort,
        username: String,
        password: String,
        invite: Option<String>,
    },
    QuickReactions,
    /// Shows a channel's messages from the start of a day.
    JumpToDate {
        inst: InstanceId,
        channel: Id,
    },
    QuietHours,
    Keywords,
    Note {
        inst: InstanceId,
        user: Id,
    },
    LocalName {
        inst: InstanceId,
        user: Id,
    },
    TwoFactorBegin(InstanceId),
    TwoFactorConfirm(InstanceId),
    TwoFactorDisable(InstanceId),
    DeleteAccount(InstanceId),
    Report {
        inst: InstanceId,
        channel: Id,
        message: Id,
        excerpt: String,
    },
    ChangeUsername(InstanceId),
    AddFriend,
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
        /// An index into `options` (two options may read the same, like two categories).
        selected: usize,
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
    /// My profile everywhere, or my nickname in a server: the form switches between the two.
    pub profile_switch: Option<ProfileSwitch>,
    _inputs: Vec<Subscription>,
}

/// Which of my profiles a form edits, and the server the other one is for.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileSwitch {
    pub inst: InstanceId,
    pub space: Id,
    /// The server's name, for the switch.
    pub space_name: String,
    /// The form edits my profile everywhere (else my nickname here).
    pub everywhere: bool,
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
            profile_switch: None,
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
            Some(Field::Choice {
                options, selected, ..
            }) => options.get(*selected).map_or("", String::as_str),
            _ => "",
        }
    }

    /// Which option of choice field `i` is selected.
    fn selected_index(&self, i: usize) -> Option<usize> {
        match self.fields.get(i) {
            Some(Field::Choice { selected, .. }) => Some(*selected),
            _ => None,
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

    fn choice(self, label: &str, hint: &str, options: &[&str], selected: &str) -> Self {
        let index = options.iter().position(|o| *o == selected).unwrap_or(0);
        self.choice_at(label, hint, options, index)
    }

    fn choice_at(mut self, label: &str, hint: &str, options: &[&str], selected: usize) -> Self {
        self.form.fields.push(Field::Choice {
            label: label.to_owned(),
            hint: hint.to_owned(),
            options: options.iter().map(|o| (*o).to_owned()).collect(),
            selected,
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

/// The edit form's choice for a channel outside every category.
const NO_CATEGORY: &str = "None";

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
        let form = Form::new(Purpose::CreateSpace(inst), "Create a server", "Create")
            .text("A server is a home for your group, with its own channels, roles and members.");
        self.close_later("form", |s| s.form = None, cx);
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
        self.cancel_closing("form");
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
        let roles: Vec<(Id, String)> = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.spaces.get(&space))
                .map(|st| {
                    st.roles
                        .iter()
                        .filter(|r| !r.is_everyone())
                        .map(|r| (r.id, r.name.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let form = Form::new(
            Purpose::CreateChannel {
                inst,
                space,
                parent,
                roles: roles.iter().map(|(id, _)| *id).collect(),
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
        for (_, role) in roles {
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

    /// The categories of the space `channel` is in: `(id, name)`, in their order.
    fn categories_of(&self, inst: &str, channel: Id) -> Vec<(Id, String)> {
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return Vec::new();
        };
        let Some(space) = s.space_of(channel) else {
            return Vec::new();
        };
        hermes_client_core::view::channel_tree(s, space)
            .into_iter()
            .filter(|c| c.kind == ChannelKind::Category)
            .map(|c| (c.id, c.name))
            .collect()
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
        let categories = if matches!(c.kind, ChannelKind::Text | ChannelKind::Voice) {
            self.categories_of(&inst, channel)
        } else {
            Vec::new()
        };
        let form = Form::new(
            Purpose::EditChannel {
                inst: inst.clone(),
                channel,
                categories: categories.iter().map(|(id, _)| *id).collect(),
                // As the choice shows it: a category I cannot see shows as "None", and leaving
                // that as it is must not take the channel out of it.
                parent: c
                    .parent_id
                    .filter(|p| categories.iter().any(|(id, _)| id == p)),
            },
            title,
            "Save",
        );
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
        if matches!(c.kind, ChannelKind::Text | ChannelKind::Voice) {
            // Option 0 is "None", then the categories.
            let current = categories
                .iter()
                .position(|(id, _)| Some(*id) == c.parent_id)
                .map_or(0, |i| i + 1);
            let mut options = vec![NO_CATEGORY];
            options.extend(categories.iter().map(|(_, name)| name.as_str()));
            fields = fields.choice_at("Category", "The group it shows under.", &options, current);
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
        .text("They are removed from the server and cannot rejoin until unbanned.")
        .danger();
        let fields = Self::fields(form, window, cx)
            .text("Reason (optional)", "Shown in the ban list", "")
            .choice(
                "Delete their recent messages",
                "",
                &labels(&BAN_DELETE),
                BAN_DELETE[0].0,
            )
            .choice("How long", "", &labels(&BAN_LENGTH), BAN_LENGTH[0].0);
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
        let mine = self.core.store().instance(&inst).and_then(|s| s.me_id()) == Some(user);
        let switch = mine
            .then(|| self.profile_switch(&inst, Some(space), false))
            .flatten();
        let mut form = Form::new(
            Purpose::Nickname { inst, space, user },
            if switch.is_some() {
                "Edit profile"
            } else {
                "Change nickname"
            },
            "Save",
        )
        .text("A nickname is only used in this server. Leave it empty to remove it.");
        form.profile_switch = switch;
        let fields = Self::fields(form, window, cx).text("Nickname", "", current);
        self.finish(fields);
    }

    pub fn open_form_jump_to_date(
        &mut self,
        inst: InstanceId,
        channel: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let today = hermes_client_core::time::format(
            hermes_client_core::now_ms(),
            "%Y-%m-%d",
            self.utc_offset,
        );
        let form = Form::new(
            Purpose::JumpToDate { inst, channel },
            "Jump to a date",
            "Go",
        )
        .text("The channel shows its messages from the start of that day.");
        let fields = Self::fields(form, window, cx).text_hint(
            "Date",
            "Year-month-day.",
            "2026-11-19",
            today,
        );
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

    /// The switch between my profile and my nickname in `space`, if I may set one there.
    fn profile_switch(
        &self,
        inst: &InstanceId,
        space: Option<Id>,
        everywhere: bool,
    ) -> Option<ProfileSwitch> {
        let space = space?;
        let store = self.core.store();
        let s = store.instance(inst)?;
        if !hermes_client_core::view::can_in_space(
            s,
            space,
            hermes_client_core::proto::permissions::Permissions::CHANGE_NICKNAME,
        ) {
            return None;
        }
        Some(ProfileSwitch {
            inst: inst.clone(),
            space,
            space_name: s.spaces.get(&space)?.space.name.clone(),
            everywhere,
        })
    }

    pub fn open_form_profile(
        &mut self,
        inst: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_form_profile_in(inst, None, window, cx);
    }

    /// My profile, with a switch to my nickname in `space` when there is one.
    pub fn open_form_profile_in(
        &mut self,
        inst: InstanceId,
        space: Option<Id>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = {
            let store = self.core.store();
            store.instance(&inst).and_then(|s| s.me.clone())
        };
        let Some(me) = me else { return };
        let with_bio = self
            .core
            .store()
            .instance(&inst)
            .is_some_and(|s| s.has_feature(hermes_client_core::proto::compat::features::BIO));
        if !with_bio {
            self.show_profile_form(inst, space, me.user, None, window, cx);
            return;
        }
        // The "About me" is only on the full profile: fetched first.
        let Ok(rest) = self.core.rest(&inst) else {
            return;
        };
        let id = me.user.id;
        self.run(
            cx,
            async move { rest.user(id).await },
            move |this, result, window, cx| {
                let bio = result.ok().and_then(|p| p.bio).unwrap_or_default();
                this.show_profile_form(inst.clone(), space, me.user.clone(), Some(bio), window, cx);
            },
        );
    }

    fn show_profile_form(
        &mut self,
        inst: InstanceId,
        space: Option<Id>,
        user: hermes_client_core::proto::models::User,
        bio: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let switch = self.profile_switch(&inst, space, true);
        let purpose = Purpose::EditProfile {
            inst,
            bio: bio.is_some(),
        };
        let mut form = Form::new(purpose, "Edit profile", "Save")
            .text(format!("Signed in as @{}.", user.username));
        form.profile_switch = switch;
        let mut fields = Self::fields(form, window, cx)
            .text_hint(
                "Display name",
                "Leave empty to use your username.",
                "Shown instead of your username",
                user.display_name.clone().unwrap_or_default(),
            )
            .text(
                "Status",
                "e.g. Working on a project",
                user.status_text.clone().unwrap_or_default(),
            );
        if let Some(bio) = bio {
            fields = fields.text_hint(
                "About me",
                "Shown on your profile, up to 190 characters.",
                "A few words about you",
                bio,
            );
        }
        self.finish(fields);
    }

    pub fn open_form_username(
        &mut self,
        inst: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.me.as_ref().map(|m| m.user.username.clone()))
                .unwrap_or_default()
        };
        let form = Form::new(Purpose::ChangeUsername(inst), "Change username", "Change").text(
            "Your display name stays as it is. The # number stays too, unless someone with the \
                 new name already has it.",
        );
        let fields = Self::fields(form, window, cx)
            .text("New username", "letters a-z, digits, _ and .", current)
            .password("Current password");
        self.finish(fields);
    }

    /// Asks for `name#1234`; with several instances, also which of them.
    pub fn open_form_add_friend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let instances = self.core.instance_ids();
        if instances.is_empty() {
            self.error("Add an instance first", cx);
            return;
        }
        let form = Form::new(Purpose::AddFriend, "Add a friend", "Send request")
            .text("Ask them for their whole name: it is on their user panel, like name#1234.");
        let mut fields = Self::fields(form, window, cx).text("Name", "name#1234", "");
        if instances.len() > 1 {
            let options: Vec<&str> = instances.iter().map(String::as_str).collect();
            fields = fields.choice("Instance", "", &options, &instances[0]);
        }
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

    pub fn open_form_quick_reactions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.config.buffer.quick_reactions.clone();
        let form = Form::new(Purpose::QuickReactions, "Quick reactions", "Save")
            .text("The reactions offered first, separated by spaces.");
        let fields = Self::fields(form, window, cx).text("Reactions", "", current);
        self.finish(fields);
    }

    pub fn open_form_quiet_hours(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (from, to) = (
            self.config.notifications.quiet_from.clone(),
            self.config.notifications.quiet_to.clone(),
        );
        let form = Form::new(Purpose::QuietHours, "Quiet hours", "Save")
            .text("No notifications between these times, like 22:00 and 07:00. Empty: none.");
        let fields = Self::fields(form, window, cx)
            .text("From", "22:00", from)
            .text("Until", "07:00", to);
        self.finish(fields);
    }

    pub fn open_form_keywords(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.config.notifications.keywords.clone();
        let form = Form::new(Purpose::Keywords, "Words that notify me", "Save")
            .text("Messages with these words notify like a mention. Separate them with spaces.");
        let fields = Self::fields(form, window, cx).text("Words", "", current);
        self.finish(fields);
    }

    pub fn open_form_note(
        &mut self,
        inst: InstanceId,
        user: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.core.note(&inst, user).unwrap_or_default();
        let form = Form::new(Purpose::Note { inst, user }, "Personal note", "Save")
            .text("Only you see this, and only on this device.");
        let fields = Self::fields(form, window, cx).text("Note", "", current);
        self.finish(fields);
    }

    pub fn open_form_local_name(
        &mut self,
        inst: InstanceId,
        user: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.core.local_name(&inst, user).unwrap_or_default();
        let form = Form::new(
            Purpose::LocalName { inst, user },
            "Name them for me",
            "Save",
        )
        .text("Shown instead of their name, only to you. Empty goes back to theirs.");
        let fields = Self::fields(form, window, cx).text("Name", "", current);
        self.finish(fields);
    }

    pub fn open_form_login_code(
        &mut self,
        host: hermes_client_core::address::HostPort,
        username: String,
        password: String,
        invite: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let purpose = Purpose::LoginCode {
            host,
            username,
            password,
            invite,
        };
        let form = Form::new(purpose, "Two-factor login", "Sign in")
            .text("Enter the six digits your authenticator shows for this account.");
        let fields = Self::fields(form, window, cx).text("Code", "123456", "");
        self.finish(fields);
    }

    pub fn open_form_two_factor_begin(
        &mut self,
        inst: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(
            Purpose::TwoFactorBegin(inst),
            "Two-factor login",
            "Continue",
        )
        .text("Signing in will also need a code from an authenticator app.");
        let fields = Self::fields(form, window, cx).password("Your password");
        self.finish(fields);
    }

    fn open_form_two_factor_confirm(
        &mut self,
        inst: InstanceId,
        secret: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(
            Purpose::TwoFactorConfirm(inst),
            "Add it to your authenticator",
            "Turn on",
        )
        .text(format!(
            "Add an account by hand with this key, then enter the code it shows. If you lose \
                 the authenticator, an administrator has to turn this off for you.\n\n{secret}"
        ));
        let fields = Self::fields(form, window, cx).text("Code", "123456", "");
        self.finish(fields);
    }

    pub fn open_form_two_factor_disable(
        &mut self,
        inst: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(
            Purpose::TwoFactorDisable(inst),
            "Turn off two-factor login",
            "Turn off",
        )
        .danger();
        let fields = Self::fields(form, window, cx)
            .password("Your password")
            .text("Code from your authenticator", "123456", "");
        self.finish(fields);
    }

    pub fn open_form_delete_account(
        &mut self,
        inst: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(
            Purpose::DeleteAccount(inst),
            "Delete your account?",
            "Delete account",
        )
        .text(
            "This cannot be undone. Your name, picture and devices go, and you leave every \
                 server. Servers you own have to be given away or deleted first.",
        )
        .danger();
        let fields = Self::fields(form, window, cx)
            .password("Your password")
            .check("Also delete all my messages", "", false);
        self.finish(fields);
    }

    pub fn open_form_report(
        &mut self,
        inst: InstanceId,
        channel: Id,
        message: Id,
        excerpt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = Form::new(
            Purpose::Report {
                inst,
                channel,
                message,
                excerpt,
            },
            "Report this message",
            "Report",
        )
        .text("The moderators see your reason and the words of this message, even if it is encrypted.");
        let fields = Self::fields(form, window, cx).text("What is wrong with it?", "", "");
        self.finish(fields);
    }

    fn form_done(&mut self, result: Result<String, String>, cx: &mut Context<Self>) {
        match result {
            Ok(text) => {
                self.close_later("form", |s| s.form = None, cx);
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
            self.cancel_closing("form");
            self.form = Some(form);
            return;
        }
        let result = self.start_submit(&form, cx);
        self.cancel_closing("form");
        self.form = Some(form);
        let form = self.form.as_mut().expect("just put back");
        match result {
            Ok(()) => form.busy = true,
            Err(problem) => form.error = Some(problem),
        }
        cx.notify();
    }

    /// Starts the request of a submitted form, or says what to fix first.
    fn start_submit(&mut self, form: &Form, cx: &mut Context<Self>) -> Result<(), String> {
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
                roles,
            } => {
                let voice = form.selected(1) == "Voice";
                let name = channel_name(&form.required(0, "a name", cx)?, voice);
                let kind = if voice {
                    ChannelKind::Voice
                } else {
                    ChannelKind::Text
                };
                let private = form.checked(2);
                // The roles ticked for a private channel: the checkboxes are the roles as the
                // form listed them, which roles added, removed or moved since cannot shift.
                let allowed = ticked_roles(form, &roles);
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
            Purpose::EditChannel {
                inst,
                channel,
                categories,
                parent,
            } => {
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
                if matches!(kind, ChannelKind::Text | ChannelKind::Voice) {
                    let at = if kind == ChannelKind::Voice { 5 } else { 3 };
                    // Sent only when it changed, so saving the topic does not move a channel
                    // someone else moved meanwhile.
                    let chosen = chosen_category(form.selected_index(at), &categories);
                    if chosen != parent {
                        let gone = chosen.is_some_and(|id| {
                            !self
                                .categories_of(&inst, channel)
                                .iter()
                                .any(|(c, _)| *c == id)
                        });
                        if gone {
                            return Err("That category was deleted: pick another.".into());
                        }
                        req.parent_id = Some(chosen);
                    }
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
                    duration_hours: lookup(&BAN_LENGTH, form.selected(2)).filter(|h| *h > 0),
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
                            this.close_later("form", |s| s.form = None, cx);
                            this.copy(link, "Invite link", cx);
                            this.invite_created(inst, space, cx);
                        }
                        Ok(None) => this.form_done(Err("no such instance".into()), cx),
                        Err(err) => this.form_done(Err(err.to_string()), cx),
                    },
                );
            }
            Purpose::LoginCode {
                host,
                username,
                password,
                invite,
            } => {
                let code = form.required(0, "the code", cx)?;
                self.form_call(cx, async move {
                    let inst = core
                        .login_with_code(&host, &username, &password, Some(&code))
                        .await?;
                    if let Some(invite) = invite {
                        let _ = core.join_invite(&inst, &invite).await;
                    }
                    Ok(format!("Signed in to {inst}"))
                });
            }
            Purpose::JumpToDate { inst, channel } => {
                let text = form.value(0, cx);
                let Some((start, _)) = hermes_client_core::time::parse_day(&text, self.utc_offset)
                else {
                    return Err("Write the date as year-month-day, like 2026-11-19.".into());
                };
                self.jump_to_time(inst.clone(), channel, start, cx);
                let done = format!("Messages from {}", text.trim());
                self.form_call(cx, async move { Ok(done) });
            }
            Purpose::QuickReactions => {
                let value = form.value(0, cx);
                self.set_config(|c| c.buffer.quick_reactions = value, cx);
                self.form_call(cx, async { Ok("Saved".to_owned()) });
            }
            Purpose::QuietHours => {
                let (from, to) = (form.value(0, cx), form.value(1, cx));
                self.set_config(
                    |c| {
                        c.notifications.quiet_from = from;
                        c.notifications.quiet_to = to;
                    },
                    cx,
                );
                self.form_call(cx, async { Ok("Saved".to_owned()) });
            }
            Purpose::Keywords => {
                let value = form.value(0, cx);
                self.set_config(|c| c.notifications.keywords = value, cx);
                self.form_call(cx, async { Ok("Saved".to_owned()) });
            }
            Purpose::Note { inst, user } => {
                let text = form.value(0, cx);
                self.core.set_note(&inst, user, &text);
                cx.notify();
                self.form_call(cx, async { Ok("Saved".to_owned()) });
            }
            Purpose::LocalName { inst, user } => {
                let name = form.value(0, cx);
                self.core.set_local_name(&inst, user, &name);
                cx.notify();
                self.form_call(cx, async { Ok("Saved".to_owned()) });
            }
            Purpose::TwoFactorBegin(inst) => {
                let password = form.raw(0, cx);
                if password.is_empty() {
                    return Err("Your password is needed.".into());
                }
                let next = inst.clone();
                self.run(
                    cx,
                    async move {
                        core.rest(&inst)?
                            .two_factor_begin(password)
                            .await
                            .map_err(CoreError::from)
                    },
                    move |this, result: Result<_, CoreError>, window, cx| match result {
                        Ok(begun) => {
                            this.close_later("form", |s| s.form = None, cx);
                            this.open_form_two_factor_confirm(next, begun.secret, window, cx);
                        }
                        Err(err) => this.form_done(Err(err.to_string()), cx),
                    },
                );
            }
            Purpose::TwoFactorConfirm(inst) => {
                let code = form.required(0, "the code", cx)?;
                self.form_call(cx, async move {
                    core.rest(&inst)?.two_factor_confirm(code).await?;
                    Ok("Two-factor login is on".to_owned())
                });
            }
            Purpose::TwoFactorDisable(inst) => {
                let password = form.raw(0, cx);
                let code = form.required(1, "the code", cx)?;
                self.form_call(cx, async move {
                    core.rest(&inst)?.two_factor_disable(password, code).await?;
                    Ok("Two-factor login is off".to_owned())
                });
            }
            Purpose::DeleteAccount(inst) => {
                let password = form.raw(0, cx);
                if password.is_empty() {
                    return Err("Your password is needed.".into());
                }
                let delete_messages = form.checked(1);
                self.form_call(cx, async move {
                    core.rest(&inst)?
                        .delete_account(password, delete_messages)
                        .await?;
                    core.remove_instance(&inst).await?;
                    Ok("Your account has been deleted".to_owned())
                });
            }
            Purpose::Report {
                inst,
                channel,
                message,
                excerpt,
            } => {
                let reason = form.required(0, "a reason", cx)?;
                self.form_call(cx, async move {
                    core.rest(&inst)?
                        .report(channel, message, reason, excerpt)
                        .await?;
                    Ok("Reported to the moderators".to_owned())
                });
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
            Purpose::ChangeUsername(inst) => {
                let username = form.required(0, "a username", cx)?;
                let current_password = form.raw(1, cx);
                if current_password.is_empty() {
                    return Err("Your password is needed to change the username.".into());
                }
                self.form_call(cx, async move {
                    let me = core
                        .rest(&inst)?
                        .change_username(&hermes_client_core::proto::api::ChangeUsernameRequest {
                            username,
                            current_password,
                        })
                        .await?;
                    Ok(format!("You are now {}", me.user.handle()))
                });
            }
            Purpose::AddFriend => {
                let handle = form.required(0, "a name", cx)?;
                let inst = match form.selected(1) {
                    "" => self
                        .core
                        .instance_ids()
                        .first()
                        .cloned()
                        .unwrap_or_default(),
                    chosen => chosen.to_owned(),
                };
                self.form_call(cx, async move {
                    let added = core.rest(&inst)?.add_friend(&handle).await?;
                    Ok(match added.kind {
                        hermes_client_core::proto::models::RelationshipKind::Friend => {
                            format!("You and {} are friends", added.user.name())
                        }
                        _ => format!("Friend request sent to {}", added.user.handle()),
                    })
                });
            }
            Purpose::EditProfile { inst, bio } => {
                let (display, status) = (form.value(0, cx), form.value(1, cx));
                let about = bio.then(|| form.value(2, cx));
                if about.as_ref().is_some_and(|b| b.chars().count() > 190) {
                    return Err("About me can be at most 190 characters".into());
                }
                if display.chars().count() > 32 {
                    return Err("the display name can be at most 32 characters".into());
                }
                if status.chars().count() > 128 {
                    return Err("the status can be at most 128 characters".into());
                }
                let req = hermes_client_core::proto::api::UpdateMeRequest {
                    display_name: Some((!display.is_empty()).then_some(display)),
                    status_text: Some((!status.is_empty()).then_some(status)),
                    bio: about.map(|b| (!b.is_empty()).then_some(b)),
                    ..Default::default()
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
        if let Some(switch) = &form.profile_switch {
            content.push(profile_switch_chips(self, switch, cx));
        }
        if !form.text.is_empty() {
            content.push(paragraph(form.text.clone()).into_any_element());
        }
        for (i, field) in form.fields.iter().enumerate() {
            content.push(render_field(self, i, field, cx));
        }
        if let Some(error) = &form.error {
            content.push(error_line(error.clone()).into_any_element());
        }
        let cancel = Button::new("form-cancel")
            .cursor_pointer()
            .large()
            .label("Cancel")
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_later("form", |s| s.form = None, cx);
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
            .loading(form.busy)
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

/// "Everywhere" and "In <server>": which of my profiles the form edits; the other one replaces
/// the form.
fn profile_switch_chips(
    shell: &Shell,
    switch: &ProfileSwitch,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let choices = [
        (true, "Everywhere".to_owned()),
        (false, format!("In {}", switch.space_name)),
    ];
    div()
        .flex()
        .gap(px(6.))
        .children(
            choices
                .into_iter()
                .enumerate()
                .map(|(n, (everywhere, name))| {
                    let (inst, space) = (switch.inst.clone(), switch.space);
                    crate::ui::chip(
                        shell,
                        ("profile-switch", n),
                        name,
                        switch.everywhere == everywhere,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let current = this
                            .form
                            .as_ref()
                            .and_then(|f| f.profile_switch.as_ref())
                            .map(|s| s.everywhere);
                        if current == Some(everywhere) {
                            return;
                        }
                        let me = this.core.store().instance(&inst).and_then(|s| s.me_id());
                        if everywhere {
                            this.open_form_profile_in(inst.clone(), Some(space), window, cx);
                        } else if let Some(me) = me {
                            this.open_form_nickname(inst.clone(), space, me, window, cx);
                        }
                    }))
                }),
        )
        .into_any_element()
}

fn render_field(shell: &Shell, i: usize, field: &Field, cx: &mut Context<Shell>) -> AnyElement {
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
                let chosen = n == *selected;
                crate::ui::chip(
                    shell,
                    SharedString::from(format!("form-{i}-{n}")),
                    option.clone(),
                    chosen,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(Field::Choice { selected, .. }) =
                        this.form.as_mut().and_then(|f| f.fields.get_mut(i))
                    {
                        *selected = n;
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

/// The roles whose "Allow …" checkbox is ticked: checkbox `n` (field `3 + n`) is `roles[n]`.
fn ticked_roles(form: &Form, roles: &[Id]) -> Vec<Id> {
    roles
        .iter()
        .enumerate()
        .filter(|(n, _)| form.checked(3 + n))
        .map(|(_, id)| *id)
        .collect()
}

/// The category picked in the edit form: option 0 is "None", option `n` is `categories[n - 1]`.
fn chosen_category(selected: Option<usize>, categories: &[Id]) -> Option<Id> {
    selected
        .and_then(|n| n.checked_sub(1))
        .and_then(|n| categories.get(n))
        .copied()
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

    #[test]
    fn choices_and_checks_map_back_to_ids() {
        let mut form = Form::new(Purpose::AddFriend, "t", "ok");
        for n in 0..5 {
            form.fields.push(Field::Check {
                label: format!("{n}"),
                hint: String::new(),
                value: n == 3 || n == 4,
            });
        }
        assert_eq!(ticked_roles(&form, &[Id(7), Id(8)]), [Id(7), Id(8)]);
        assert_eq!(ticked_roles(&form, &[Id(7)]), [Id(7)], "only listed roles");
        form.fields.push(Field::Choice {
            label: String::new(),
            hint: String::new(),
            // Two categories may have the same name.
            options: vec!["None".into(), "Games".into(), "Games".into()],
            selected: 2,
        });
        let categories = [Id(20), Id(21)];
        assert_eq!(form.selected(5), "Games");
        assert_eq!(
            chosen_category(form.selected_index(5), &categories),
            Some(Id(21))
        );
        assert_eq!(chosen_category(Some(0), &categories), None, "None");
        assert_eq!(chosen_category(Some(9), &categories), None);
    }
}
