//! Account: the accounts and the Account tab.

use super::*;
use crate::ui::ButtonKind;

impl Shell {
    /// Every account (an instance signed in to), with signing in and out, going offline and
    /// removing it, and adding another: these left the menus (#17).
    pub(super) fn accounts_group(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::menu_entries::{Action as MenuAction, Target};
        use hermes_client_core::gateway::ConnStatus;
        let mut rows: Vec<AnyElement> = Vec::new();
        for inst in self.core.instance_ids() {
            let status = self.core.status(&inst);
            let username = self
                .config
                .instances
                .iter()
                .find(|e| e.id == inst)
                .map(|e| e.username.clone())
                .unwrap_or_default();
            let hint = match &status {
                ConnStatus::Connected => format!("Signed in as @{username}"),
                ConnStatus::Connecting | ConnStatus::Reconnecting { .. } => {
                    "Connecting…".to_owned()
                }
                ConnStatus::Offline => "Offline".to_owned(),
                ConnStatus::LoggedOut => "Signed out".to_owned(),
                _ => "Not connected".to_owned(),
            };
            let button = |label: &'static str, action: Option<MenuAction>, danger: bool| {
                let inst = inst.clone();
                crate::ui::button(
                    self,
                    format!("account-{inst}-{label}"),
                    label,
                    if danger {
                        ButtonKind::Danger
                    } else {
                        ButtonKind::Secondary
                    },
                    true,
                    move |this, window, cx| match action {
                        Some(action) => {
                            this.menu_action(Target::Instance(inst.clone()), action, window, cx)
                        }
                        None => this.sign_in(&inst, window, cx),
                    },
                    cx,
                )
            };
            let mut buttons = div().flex().flex_wrap().justify_end().gap(px(6.));
            buttons = match status {
                ConnStatus::LoggedOut => buttons.child(button("Sign in", None, false)),
                ConnStatus::Offline => {
                    buttons.child(button("Go online", Some(MenuAction::GoOnline), false))
                }
                _ => buttons
                    .child(button("Reconnect", Some(MenuAction::Reconnect), false))
                    .child(button("Go offline", Some(MenuAction::GoOffline), false)),
            };
            if status != ConnStatus::LoggedOut {
                buttons = buttons.child(button("Sign out", Some(MenuAction::LogOut), false));
            }
            buttons = buttons.child(button("Remove", Some(MenuAction::RemoveInstance), true));
            rows.push(setting(&inst, &hint, buttons));
        }
        rows.push(setting(
            "Another account",
            "Sign in with a Hermes address, or paste an invite link.",
            crate::ui::button(
                self,
                "accounts-add",
                "Add an account",
                ButtonKind::Secondary,
                true,
                |this, window, cx| this.open_add_instance(String::new(), window, cx),
                cx,
            ),
        ));
        group("Accounts", rows)
    }

    pub(super) fn tab_account(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let instances = self.core.instance_ids();
        if instances.is_empty() {
            return page_body("Account", None, vec![self.accounts_group(cx)]);
        }
        let selected = self.account_instance();
        let me = {
            let store = self.core.store();
            let s = selected.as_ref().and_then(|i| store.instance(i));
            s.and_then(|s| s.me.clone())
        };
        let mut account_rows = Vec::new();
        if instances.len() > 1 {
            // A few, each with who is signed in there: a radio list.
            let options = instances
                .iter()
                .map(|inst| {
                    let username = self
                        .config
                        .instances
                        .iter()
                        .find(|e| &e.id == inst)
                        .map(|e| SharedString::from(format!("@{}", e.username)));
                    (SharedString::from(inst.to_string()), username)
                })
                .collect();
            let chosen = instances.iter().position(|i| Some(i) == selected.as_ref());
            let pick = instances.clone();
            account_rows.push(setting_below(
                "Account",
                "Which account these settings are for.",
                crate::ui::radio_list(
                    self,
                    "account-inst",
                    options,
                    chosen,
                    move |this, n, _, cx| {
                        if let Some(page) = &mut this.settings {
                            page.account_inst = pick.get(n).cloned();
                        }
                        this.load_sessions(cx);
                    },
                    cx,
                ),
            ));
        }
        let Some(me) = me else {
            account_rows.push(muted("Not signed in."));
            return page_body(
                "Account",
                None,
                vec![group("Account", account_rows), self.accounts_group(cx)],
            );
        };
        let inst = selected.clone().unwrap_or_default();
        let mut hint = format!(
            "{} (@{}){}",
            me.user.name(),
            me.user.username,
            me.user
                .status_text
                .as_deref()
                .map(|s| format!(" · {s}"))
                .unwrap_or_default()
        );
        if me.instance_admin {
            hint.push_str(" · administrator");
        }
        let username_inst = inst.clone();
        let copy_handle = me.user.handle();
        account_rows.push(setting(
            "Username",
            &format!("You are {copy_handle}. Friends add you by this name."),
            wrap()
                .child(crate::ui::button(
                    self,
                    "account-username-copy",
                    self.copy_label("account-username-copy", "Copy"),
                    ButtonKind::Secondary,
                    true,
                    move |this, _, cx| {
                        this.copy_from(
                            "account-username-copy",
                            copy_handle.clone(),
                            "Username",
                            cx,
                        );
                    },
                    cx,
                ))
                .child(crate::ui::button(
                    self,
                    "account-username",
                    "Change username…",
                    ButtonKind::Secondary,
                    true,
                    move |this, window, cx| {
                        this.open_form_username(username_inst.clone(), window, cx);
                    },
                    cx,
                )),
        ));
        let profile_inst = inst.clone();
        account_rows.push(setting(
            "Profile",
            &hint,
            crate::ui::button(
                self,
                "account-profile",
                "Edit profile…",
                ButtonKind::Primary,
                true,
                move |this, window, cx| {
                    let at = this.popup_from(window.mouse_position());
                    this.open_my_profile(profile_inst.clone(), at, cx);
                },
                cx,
            ),
        ));
        let password_inst = inst.clone();
        account_rows.push(setting(
            "Password",
            "Changing it signs out your other devices.",
            crate::ui::button(
                self,
                "account-password",
                "Change password…",
                ButtonKind::Secondary,
                true,
                move |this, window, cx| this.open_form_password(password_inst.clone(), window, cx),
                cx,
            ),
        ));

        let mut device_rows: Vec<AnyElement> = Vec::new();
        match &page.sessions {
            Remote::Idle | Remote::Loading => device_rows.push(theme::skeleton("devices", 3)),
            Remote::Failed(err) => device_rows.push(
                div()
                    .text_size(theme::secondary_size())
                    .text_color(theme::danger())
                    .child(format!("Could not load: {err}"))
                    .into_any_element(),
            ),
            Remote::Ready(list) => {
                for s in list {
                    let seen = time::format(s.last_seen_at, "%Y-%m-%d %H:%M", self.utc_offset);
                    let id = s.id;
                    let control: AnyElement = if s.current {
                        muted("This device")
                    } else {
                        crate::ui::button(
                            self,
                            format!("session-{id}"),
                            "Sign out",
                            ButtonKind::Secondary,
                            true,
                            move |this, _, cx| this.revoke_sessions(vec![id], cx),
                            cx,
                        )
                        .into_any_element()
                    };
                    device_rows.push(setting(
                        &s.device_name,
                        &format!("Last active {seen}"),
                        control,
                    ));
                }
            }
        }
        let others: Vec<Id> = page
            .sessions
            .ready()
            .map(|l| l.iter().filter(|s| !s.current).map(|s| s.id).collect())
            .unwrap_or_default();
        // In the group's header, at its own width: not a bar across the card.
        let sign_out_others = if others.is_empty() {
            div().into_any_element()
        } else {
            crate::ui::button(
                self,
                "sessions-others",
                "Sign out all others",
                ButtonKind::Danger,
                true,
                move |this, _, cx| this.revoke_sessions(others.clone(), cx),
                cx,
            )
            .into_any_element()
        };
        page_body(
            "Account",
            None,
            vec![
                group("Account", account_rows),
                group_with("Signed-in devices", sign_out_others, device_rows),
                self.security_group(page, &inst, cx),
                self.accounts_group(cx),
            ],
        )
    }
}
