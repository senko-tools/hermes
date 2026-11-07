//! Add instance / sign in / create account (PLAN.md section 15.4): address or invite link →
//! probe → certificate prompt if needed → instance preview with Sign in and Create account tabs.
//!
//! The network steps are plain async functions ([`probe`], [`preview`], [`submit`]), so tests can
//! call them directly.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, SharedString, Subscription, Window, div};
use hermes_client_core::address::{AddTarget, HostPort, InviteLink};
use hermes_client_core::proto::api::RegistrationMode;
use hermes_client_core::proto::models::Invite;
use hermes_client_core::{CertStatus, Core, CoreError, InstanceId, Probe};

use crate::modal::{dialog, error_line, heading, paragraph};
use crate::shell::Shell;
use crate::theme::{self, px};

pub struct AddInstance {
    pub step: Step,
    pub busy: bool,
    pub error: Option<String>,
    /// Filled into the username field (signing in again).
    username_hint: Option<String>,
    address: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    _inputs: Vec<Subscription>,
}

pub enum Step {
    Address,
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
}

impl Shell {
    /// Starts the add-instance flow with an invite link or address (command line, a forwarded
    /// `hermes://` link) and goes on with it.
    pub fn open_link(&mut self, link: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_add_instance(link, window, cx);
        self.submit_address(cx);
    }

    pub fn open_add_instance(
        &mut self,
        input: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let address = cx.new(|cx| {
            let mut state =
                InputState::new(window, cx).placeholder("hermes.example.com or https://…/invite/…");
            state.set_value(input, window, cx);
            state
        });
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let inputs = vec![
            cx.subscribe_in(&address, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_address(cx);
                }
            }),
            cx.subscribe_in(&username, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_account(cx);
                }
            }),
            cx.subscribe_in(&password, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_account(cx);
                }
            }),
        ];
        address.update(cx, |input, cx| input.focus(window, cx));
        self.add_instance = Some(AddInstance {
            step: Step::Address,
            busy: false,
            error: None,
            username_hint: None,
            address,
            username,
            password,
            _inputs: inputs,
        });
        cx.notify();
    }

    /// Signs in again to an instance that is logged out: probes it right away.
    pub fn sign_in(&mut self, inst: &InstanceId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.core.config().instance(inst).cloned() else {
            return;
        };
        self.open_add_instance(entry.url.display(), window, cx);
        if let Some(dialog) = &mut self.add_instance {
            dialog.username_hint = Some(entry.username);
        }
        self.submit_address(cx);
    }

    fn submit_address(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.add_instance else {
            return;
        };
        let input = dialog.address.read(cx).value().to_string();
        if dialog.busy || input.trim().is_empty() || !matches!(dialog.step, Step::Address) {
            return;
        }
        dialog.busy = true;
        dialog.error = None;
        let core = self.core.clone();
        self.run(cx, probe(core, input), |this, result, window, cx| {
            let Some(dialog) = &mut this.add_instance else {
                return;
            };
            dialog.busy = false;
            match result {
                Ok(probed) => {
                    let Probed {
                        probe,
                        invite,
                        preview,
                    } = probed;
                    match probe.cert {
                        CertStatus::Trusted | CertStatus::Pinned => {
                            this.show_account(&probe, invite, preview, window, cx);
                        }
                        CertStatus::Unknown | CertStatus::Changed { .. } => {
                            dialog.step = Step::Fingerprint { probe, invite };
                        }
                    }
                }
                Err(err) => dialog.error = Some(err),
            }
        });
        cx.notify();
    }

    /// Goes to the sign-in step, with the keyboard on the first empty field.
    fn show_account(
        &mut self,
        probe: &Probe,
        invite: Option<String>,
        preview: Option<Invite>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = &mut self.add_instance else {
            return;
        };
        let hint = dialog.username_hint.clone();
        let mut account = Account {
            host: probe.host.clone(),
            name: probe.info.name.clone(),
            description: probe.info.description.clone(),
            registration: probe.info.registration_mode,
            tab: Tab::SignIn,
            invite,
            preview,
            username: String::new(),
            password: String::new(),
        };
        // Invite links start on "Create account" (section 15.4).
        if account.invite.is_some() && hint.is_none() && account.can_register() {
            account.tab = Tab::Register;
        }
        dialog.step = Step::Account(Box::new(account));
        dialog.error = None;
        let (username, password) = (dialog.username.clone(), dialog.password.clone());
        let focus = match hint {
            Some(name) => {
                username.update(cx, |input, cx| input.set_value(name, window, cx));
                password
            }
            None => username,
        };
        focus.update(cx, |input, cx| input.focus(window, cx));
    }

    fn trust_certificate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.add_instance else {
            return;
        };
        let Step::Fingerprint { probe, invite } = &dialog.step else {
            return;
        };
        let (probe, invite) = (probe.clone(), invite.clone());
        if let Err(err) = self.core.trust_fingerprint(&probe.host, probe.fingerprint) {
            dialog.error = Some(err.to_string());
            return;
        }
        if let Some(code) = invite.clone() {
            let core = self.core.clone();
            let host = probe.host.clone();
            self.run(cx, preview(core, host, code), |this, invite, _, _| {
                if let Some(AddInstance {
                    step: Step::Account(account),
                    ..
                }) = &mut this.add_instance
                {
                    account.preview = invite;
                }
            });
        }
        self.show_account(&probe, invite, None, window, cx);
        cx.notify();
    }

    fn submit_account(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.add_instance else {
            return;
        };
        let Step::Account(account) = &mut dialog.step else {
            return;
        };
        if dialog.busy {
            return;
        }
        account.username = dialog.username.read(cx).value().to_string();
        account.password = dialog.password.read(cx).value().to_string();
        if let Err(err) = account.validate() {
            dialog.error = Some(err.to_owned());
            cx.notify();
            return;
        }
        let account = (**account).clone();
        dialog.busy = true;
        dialog.error = None;
        let core = self.core.clone();
        self.run(cx, submit(core, account), |this, result, _, cx| {
            let Some(dialog) = &mut this.add_instance else {
                return;
            };
            dialog.busy = false;
            match result {
                Ok(inst) => {
                    this.add_instance = None;
                    this.info(format!("Signed in to {inst}"), cx);
                }
                Err(err) => dialog.error = Some(err),
            }
        });
        cx.notify();
    }

    fn add_instance_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &self.add_instance else {
            return;
        };
        let input = match &dialog.step {
            Step::Address => return,
            Step::Fingerprint { probe, .. } => probe.host.display(),
            Step::Account(account) => account.host.display(),
        };
        self.open_add_instance(input, window, cx);
    }

    pub fn render_add_instance(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog_state = self.add_instance.as_ref()?;
        let busy = dialog_state.busy;
        let cancel = Button::new("add-cancel")
            .cursor_pointer()
            .large()
            .label("Cancel")
            .on_click(cx.listener(|this, _, _, cx| {
                this.add_instance = None;
                cx.notify();
            }));
        let back = Button::new("add-back")
            .cursor_pointer()
            .large()
            .label("Back")
            .on_click(cx.listener(|this, _, window, cx| this.add_instance_back(window, cx)));
        let mut body: Vec<AnyElement> = Vec::new();
        let buttons = div().flex().items_center().gap(px(8.));
        let spacer = || div().flex_1();
        match &dialog_state.step {
            Step::Address => {
                body.push(heading("Add an instance").into_any_element());
                body.push(paragraph("Paste an invite link or an address.").into_any_element());
                body.push(Input::new(&dialog_state.address).large().into_any_element());
                body.extend(
                    dialog_state
                        .error
                        .clone()
                        .map(|e| error_line(e).into_any_element()),
                );
                let go = Button::new("add-go")
                    .cursor_pointer()
                    .large()
                    .primary()
                    .label(if busy { "Connecting…" } else { "Continue" })
                    .disabled(busy)
                    .when(busy, |b| b.cursor_default())
                    .on_click(cx.listener(|this, _, _, cx| this.submit_address(cx)));
                body.push(
                    buttons
                        .child(spacer())
                        .child(cancel)
                        .child(go)
                        .into_any_element(),
                );
            }
            Step::Fingerprint { probe, .. } => {
                let changed = match probe.cert {
                    CertStatus::Changed { pinned } => Some(pinned),
                    _ => None,
                };
                body.push(
                    heading(if changed.is_some() {
                        "The certificate changed"
                    } else {
                        "Check this certificate"
                    })
                    .into_any_element(),
                );
                body.push(
                    paragraph(match changed {
                        Some(_) => format!(
                            "{} shows a different certificate than last time. If its owner \
                             re-generated it, trust the new one. Otherwise someone may be \
                             intercepting the connection.",
                            probe.host.display()
                        ),
                        None => format!(
                            "{} uses a self-signed certificate. Compare this fingerprint with \
                             what the server's owner gave you.",
                            probe.host.display()
                        ),
                    })
                    .into_any_element(),
                );
                if let Some(pinned) = changed {
                    body.push(fingerprint_line("Was", pinned.colon_hex()));
                    body.push(fingerprint_line("Now", probe.fingerprint.colon_hex()));
                } else {
                    body.push(fingerprint_line("SHA-256", probe.fingerprint.colon_hex()));
                }
                body.extend(
                    dialog_state
                        .error
                        .clone()
                        .map(|e| error_line(e).into_any_element()),
                );
                let trust = Button::new("add-trust")
                    .cursor_pointer()
                    .large()
                    .label(if changed.is_some() {
                        "Trust the new one"
                    } else {
                        "Trust it"
                    })
                    .when(changed.is_some(), |b| b.danger())
                    .when(changed.is_none(), |b| b.primary())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.trust_certificate(window, cx)),
                    );
                body.push(
                    buttons
                        .child(back)
                        .child(spacer())
                        .child(cancel)
                        .child(trust)
                        .into_any_element(),
                );
            }
            Step::Account(account) => {
                body.push(heading(account.name.clone()).into_any_element());
                if !account.description.is_empty() {
                    body.push(paragraph(account.description.clone()).into_any_element());
                }
                body.push(
                    div()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .child(account.host.display())
                        .into_any_element(),
                );
                if let Some(space) = account.preview.as_ref().and_then(|p| p.space.as_ref()) {
                    let members = if space.member_count == 1 {
                        "1 member".to_owned()
                    } else {
                        format!("{} members", space.member_count)
                    };
                    body.push(
                        paragraph(format!("You're invited to {} ({members})", space.name))
                            .into_any_element(),
                    );
                }
                let tab = account.tab;
                let can_register = account.can_register();
                body.push(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(tab_button("Sign in", Tab::SignIn, tab, true, cx))
                        .child(tab_button(
                            "Create account",
                            Tab::Register,
                            tab,
                            can_register,
                            cx,
                        ))
                        .into_any_element(),
                );
                body.push(
                    Input::new(&dialog_state.username)
                        .large()
                        .into_any_element(),
                );
                body.push(
                    Input::new(&dialog_state.password)
                        .large()
                        .into_any_element(),
                );
                if !can_register {
                    body.push(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child("This instance only accepts new accounts with an invite link.")
                            .into_any_element(),
                    );
                }
                body.extend(
                    dialog_state
                        .error
                        .clone()
                        .map(|e| error_line(e).into_any_element()),
                );
                let go = Button::new("add-submit")
                    .cursor_pointer()
                    .large()
                    .primary()
                    .label(match (busy, tab) {
                        (true, _) => "Please wait…",
                        (false, Tab::SignIn) => "Sign in",
                        (false, Tab::Register) => "Create account",
                    })
                    .disabled(busy)
                    .when(busy, |b| b.cursor_default())
                    .on_click(cx.listener(|this, _, _, cx| this.submit_account(cx)));
                body.push(
                    buttons
                        .child(back)
                        .child(spacer())
                        .child(cancel)
                        .child(go)
                        .into_any_element(),
                );
            }
        }
        Some(dialog(
            "add-instance",
            480.,
            div().flex().flex_col().gap(px(14.)).children(body),
        ))
    }
}

fn fingerprint_line(name: &'static str, value: String) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .text_size(theme::caption_size())
        .child(div().text_color(theme::text_tertiary()).child(name))
        .child(SharedString::from(value))
        .into_any_element()
}

fn tab_button(
    name: &'static str,
    which: Tab,
    current: Tab,
    enabled: bool,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let selected = which == current;
    div()
        .id(name)
        .px(px(12.))
        .py(px(6.))
        .rounded(px(4.))
        .text_size(theme::secondary_size())
        .when(selected, |d| d.bg(theme::selected()))
        .when(!selected && enabled, |d| {
            d.cursor_pointer().hover(|d| d.bg(theme::hover()))
        })
        .when(!enabled, |d| d.text_color(theme::text_tertiary()))
        .child(name)
        .when(enabled, |d| {
            d.on_click(cx.listener(move |this, _, _, cx| {
                if let Some(AddInstance {
                    step: Step::Account(account),
                    error,
                    ..
                }) = &mut this.add_instance
                {
                    account.tab = which;
                    *error = None;
                }
                cx.notify();
            }))
        })
        .into_any_element()
}

/// Parses `input`, trusts an invite's `#fp=` for a new host (section 13), probes the server
/// and, for a trusted server, loads the invite preview.
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
        core.trust_invite_fingerprint(&host, fp)
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
