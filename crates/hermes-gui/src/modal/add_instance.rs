//! Add instance / sign in / create account (PLAN.md section 15.4): address or invite link →
//! probe → certificate prompt if needed → instance preview with Sign in and Create account tabs.
//!
//! The network steps are plain async functions ([`probe`], [`preview`], [`submit`]) that
//! `update` runs as tasks, so tests can call them directly.

use hermes_client_core::address::{AddTarget, HostPort, InviteLink};
use hermes_client_core::proto::api::RegistrationMode;
use hermes_client_core::proto::models::Invite;
use hermes_client_core::{CertStatus, Core, CoreError, InstanceId, Probe};
use iced::widget::{button, column, container, row, space, text_input};
use iced::{Alignment, Element, Fill, Task};

use crate::font::Fonts;
use crate::modal::fingerprint;
use crate::theme::{Theme, style};
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, heading, label, strong};

pub const ADDRESS_INPUT: &str = "add-instance-address";
pub const USERNAME_INPUT: &str = "add-instance-username";
pub const PASSWORD_INPUT: &str = "add-instance-password";

#[derive(Debug, Clone, PartialEq)]
pub struct AddInstance {
    pub step: Step,
    pub busy: bool,
    pub error: Option<String>,
    /// Filled into the username field (signing in again).
    username_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Address {
        input: String,
    },
    /// The certificate is unknown (TOFU) or changed.
    Fingerprint {
        probe: Box<Probe>,
        invite: Option<String>,
    },
    Account(Box<Account>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub host: HostPort,
    pub name: String,
    pub description: String,
    pub registration: RegistrationMode,
    pub invite: Option<String>,
    pub preview: Option<Invite>,
    pub tab: Tab,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    SignIn,
    Register,
}

/// What a probe found.
#[derive(Debug, Clone, PartialEq)]
pub struct Probed {
    pub probe: Box<Probe>,
    pub invite: Option<String>,
    pub preview: Option<Invite>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Address(String),
    SubmitAddress,
    Probed(Result<Probed, String>),
    Trust,
    Preview(Option<Invite>),
    Tab(Tab),
    Username(String),
    Password(String),
    Submit,
    Done(Result<InstanceId, String>),
    Back,
    Cancel,
}

/// What the app should do after an update.
pub enum Action {
    None,
    Run(Task<Message>),
    Close,
    SignedIn(InstanceId),
}

impl AddInstance {
    pub fn new(input: String) -> AddInstance {
        AddInstance {
            step: Step::Address { input },
            busy: false,
            error: None,
            username_hint: None,
        }
    }

    /// Signing in again to a known instance: probes it right away (see [`Message::SubmitAddress`]).
    pub fn sign_in(host: &HostPort, username: &str) -> AddInstance {
        AddInstance {
            username_hint: Some(username.to_owned()),
            ..AddInstance::new(host.display())
        }
    }

    pub fn update(&mut self, message: Message, core: &Core) -> Action {
        match message {
            Message::Address(text) => {
                if let Step::Address { input } = &mut self.step {
                    *input = text;
                }
            }
            Message::SubmitAddress => {
                let Step::Address { input } = &self.step else {
                    return Action::None;
                };
                if self.busy || input.trim().is_empty() {
                    return Action::None;
                }
                self.busy = true;
                self.error = None;
                let (core, input) = (core.clone(), input.clone());
                return Action::Run(Task::perform(probe(core, input), Message::Probed));
            }
            Message::Probed(result) => {
                self.busy = false;
                match result {
                    Ok(probed) => self.on_probed(probed),
                    Err(err) => self.error = Some(err),
                }
            }
            Message::Trust => {
                let Step::Fingerprint { probe, invite } = &self.step else {
                    return Action::None;
                };
                if let Err(err) = core.trust_fingerprint(&probe.host, probe.fingerprint) {
                    self.error = Some(err.to_string());
                    return Action::None;
                }
                let task = invite.clone().map(|code| {
                    Task::perform(
                        preview(core.clone(), probe.host.clone(), code),
                        Message::Preview,
                    )
                });
                let (probe, invite) = (probe.clone(), invite.clone());
                self.show_account(&probe, invite, None);
                if let Some(task) = task {
                    return Action::Run(task);
                }
            }
            Message::Preview(invite) => {
                if let Step::Account(account) = &mut self.step {
                    account.preview = invite;
                }
            }
            Message::Tab(tab) => {
                if let Step::Account(account) = &mut self.step
                    && (tab == Tab::SignIn || account.can_register())
                {
                    account.tab = tab;
                    self.error = None;
                }
            }
            Message::Username(text) => {
                if let Step::Account(account) = &mut self.step {
                    account.username = text;
                }
            }
            Message::Password(text) => {
                if let Step::Account(account) = &mut self.step {
                    account.password = text;
                }
            }
            Message::Submit => {
                let Step::Account(account) = &self.step else {
                    return Action::None;
                };
                if self.busy {
                    return Action::None;
                }
                if let Err(err) = account.validate() {
                    self.error = Some(err.to_owned());
                    return Action::None;
                }
                self.busy = true;
                self.error = None;
                return Action::Run(Task::perform(
                    submit(core.clone(), (**account).clone()),
                    Message::Done,
                ));
            }
            Message::Done(result) => {
                self.busy = false;
                match result {
                    Ok(inst) => return Action::SignedIn(inst),
                    Err(err) => self.error = Some(err),
                }
            }
            Message::Back => {
                let input = match &self.step {
                    Step::Address { .. } => return Action::None,
                    Step::Fingerprint { probe, .. } => probe.host.display(),
                    Step::Account(account) => account.host.display(),
                };
                *self = AddInstance::new(input);
            }
            Message::Cancel => return Action::Close,
        }
        Action::None
    }

    fn on_probed(&mut self, probed: Probed) {
        let Probed {
            probe,
            invite,
            preview,
        } = probed;
        match probe.cert {
            CertStatus::Trusted | CertStatus::Pinned => self.show_account(&probe, invite, preview),
            CertStatus::Unknown | CertStatus::Changed { .. } => {
                self.step = Step::Fingerprint { probe, invite };
            }
        }
    }

    fn show_account(&mut self, probe: &Probe, invite: Option<String>, preview: Option<Invite>) {
        let mut account = Account {
            host: probe.host.clone(),
            name: probe.info.name.clone(),
            description: probe.info.description.clone(),
            registration: probe.info.registration_mode,
            tab: Tab::SignIn,
            invite,
            preview,
            username: self.username_hint.clone().unwrap_or_default(),
            password: String::new(),
        };
        // Invite links start on "Create account" (section 15.4).
        if account.invite.is_some() && self.username_hint.is_none() && account.can_register() {
            account.tab = Tab::Register;
        }
        self.step = Step::Account(Box::new(account));
        self.error = None;
    }

    /// The input to focus when the dialog shows this step.
    pub fn focus(&self) -> Option<&'static str> {
        match &self.step {
            Step::Address { .. } => Some(ADDRESS_INPUT),
            Step::Fingerprint { .. } => None,
            Step::Account(a) if a.username.is_empty() => Some(USERNAME_INPUT),
            Step::Account(_) => Some(PASSWORD_INPUT),
        }
    }

    pub fn view<'a>(&'a self, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
        let body: Element<'a, Message> = match &self.step {
            Step::Address { input } => column![
                heading("Add an instance", fonts),
                label("Paste an invite link or an address.", fonts),
                text_input("hermes.example.com or https://…/invite/…", input)
                    .padding(style::INPUT_PAD)
                    .style(move |_, st| style::input(theme, st))
                    .id(ADDRESS_INPUT)
                    .on_input(Message::Address)
                    .on_submit(Message::SubmitAddress)
                    .font(fonts.ui)
                    .size(fonts.size),
                buttons(
                    fonts,
                    theme,
                    None,
                    if self.busy {
                        "Connecting…"
                    } else {
                        "Continue"
                    },
                    (!self.busy).then_some(Message::SubmitAddress),
                ),
            ]
            .spacing(style::GAP)
            .into(),
            Step::Fingerprint { probe, .. } => fingerprint::view(
                probe.host.display(),
                probe.fingerprint,
                match probe.cert {
                    CertStatus::Changed { pinned } => Some(pinned),
                    _ => None,
                },
                fonts,
                theme,
                Message::Trust,
                Message::Back,
            ),
            Step::Account(account) => account.view(self.busy, fonts, theme),
        };
        let mut content = column![body].spacing(style::GAP);
        if let Some(error) = &self.error {
            content = content.push(label(error.as_str(), fonts).color(theme.text.error.0));
        }
        content.into()
    }
}

impl Account {
    /// Whether "Create account" can work: open registration, or any invite (the server
    /// decides whether a `closed` instance takes it; its setup invite always works).
    pub fn can_register(&self) -> bool {
        self.registration == RegistrationMode::Open || self.invite.is_some()
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.username.trim().is_empty() || self.password.is_empty() {
            return Err("Enter a username and a password.");
        }
        if self.tab == Tab::Register {
            let name = self.username.trim().to_lowercase();
            let valid = (2..=32).contains(&name.len())
                && name.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.'
                });
            if !valid {
                return Err("Usernames are 2–32 characters: letters, digits, _ and .");
            }
        }
        Ok(())
    }

    fn view<'a>(&'a self, busy: bool, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
        let mut info = column![heading(self.name.as_str(), fonts)].spacing(style::GAP_XS);
        if !self.description.is_empty() {
            info = info.push(label(self.description.as_str(), fonts).color(theme.text.secondary.0));
        }
        info = info.push(caption(self.host.display(), fonts).color(theme.text.tertiary.0));
        if let Some(space) = self.preview.as_ref().and_then(|p| p.space.as_ref()) {
            let members = if space.member_count == 1 {
                "1 member".to_owned()
            } else {
                format!("{} members", space.member_count)
            };
            info = info.push(
                container(
                    row![
                        caption("You're invited to", fonts),
                        strong(space.name.as_str(), fonts),
                        caption(members, fonts).color(theme.text.secondary.0),
                    ]
                    .spacing(style::GAP_S)
                    .align_y(Alignment::Center),
                )
                .padding([style::GAP_S, 0.0]),
            );
        }

        let tab = |name: &'a str, which: Tab, enabled: bool| {
            let selected = self.tab == which;
            button(label(name, fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::tab(theme, s, selected))
                .on_press_maybe(enabled.then_some(Message::Tab(which)))
        };
        let tabs = row![
            tab("Sign in", Tab::SignIn, true),
            tab("Create account", Tab::Register, self.can_register()),
        ]
        .spacing(style::GAP_S);

        let mut form = column![
            info,
            tabs,
            text_input("Username", &self.username)
                .padding(style::INPUT_PAD)
                .style(move |_, st| style::input(theme, st))
                .id(USERNAME_INPUT)
                .on_input(Message::Username)
                .on_submit(Message::Submit)
                .font(fonts.ui)
                .size(fonts.size),
            text_input("Password", &self.password)
                .padding(style::INPUT_PAD)
                .style(move |_, st| style::input(theme, st))
                .id(PASSWORD_INPUT)
                .secure(true)
                .on_input(Message::Password)
                .on_submit(Message::Submit)
                .font(fonts.ui)
                .size(fonts.size),
        ]
        .spacing(style::GAP);
        if !self.can_register() {
            form = form.push(
                caption(
                    "This instance only accepts new accounts with an invite link.",
                    fonts,
                )
                .color(theme.text.tertiary.0),
            );
        }
        let action = match (busy, self.tab) {
            (true, _) => "Please wait…",
            (false, Tab::SignIn) => "Sign in",
            (false, Tab::Register) => "Create account",
        };
        form.push(buttons(
            fonts,
            theme,
            Some(Message::Back),
            action,
            (!busy).then_some(Message::Submit),
        ))
        .into()
    }
}

fn buttons<'a>(
    fonts: &Fonts,
    theme: &'a Theme,
    back: Option<Message>,
    action: &'a str,
    on_action: Option<Message>,
) -> Element<'a, Message> {
    let mut row = row![]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center)
        .width(Fill);
    if let Some(back) = back {
        row = row.push(text_button("Back", ButtonKind::Secondary, fonts, theme).on_press(back));
    }
    row.push(space::horizontal())
        .push(text_button("Cancel", ButtonKind::Secondary, fonts, theme).on_press(Message::Cancel))
        .push(text_button(action, ButtonKind::Primary, fonts, theme).on_press_maybe(on_action))
        .into()
}

/// Parses `input`, trusts an invite's `#fp=` (section 13), probes the server and, for a trusted
/// server, loads the invite preview.
pub async fn probe(core: Core, input: String) -> Result<Probed, String> {
    let target = AddTarget::parse(&input).map_err(|e| e.to_string())?;
    let (host, invite, fingerprint) = match target {
        AddTarget::Address(host) => (host, None, None),
        AddTarget::Invite(InviteLink {
            host,
            code,
            fingerprint,
        }) => (host, Some(code), fingerprint),
    };
    if let Some(fp) = fingerprint {
        core.trust_fingerprint(&host, fp)
            .map_err(|e| e.to_string())?;
    }
    let probe = core
        .probe_instance(&host)
        .await
        .map_err(|e| format!("{}: {e}", host.display()))?;
    let preview = match (&probe.cert, &invite) {
        (CertStatus::Trusted | CertStatus::Pinned, Some(code)) => {
            preview(core, host, code.clone()).await
        }
        _ => None,
    };
    Ok(Probed {
        probe: Box::new(probe),
        invite,
        preview,
    })
}

/// The invite preview; `None` if it cannot be loaded (the invite may still work for joining).
pub async fn preview(core: Core, host: HostPort, code: String) -> Option<Invite> {
    match core.invite_preview(&host, &code).await {
        Ok(invite) => Some(invite),
        Err(err) => {
            tracing::warn!("invite preview: {err}");
            None
        }
    }
}

/// Signs in or creates the account. Signing in with an invite also joins its space.
pub async fn submit(core: Core, account: Account) -> Result<InstanceId, String> {
    let username = account.username.trim();
    let result = match account.tab {
        Tab::Register => {
            core.register(
                &account.host,
                username,
                &account.password,
                account.invite.as_deref(),
            )
            .await
        }
        Tab::SignIn => match core.login(&account.host, username, &account.password).await {
            Ok(inst) => match &account.invite {
                // Already a member, or the invite expired: signing in still worked.
                Some(code) => match core.join_invite(&inst, code).await {
                    Ok(_) | Err(CoreError::Rest(_)) => Ok(inst),
                    Err(e) => Err(e),
                },
                None => Ok(inst),
            },
            Err(e) => Err(e),
        },
    };
    result.map_err(|e| e.to_string())
}
