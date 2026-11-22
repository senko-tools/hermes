//! Security, your data, and Administration.

use super::*;
use crate::ui::{ButtonKind, Segment};

impl Shell {
    pub(super) fn is_instance_admin(&self) -> bool {
        let store = self.core.store();
        self.account_instance()
            .and_then(|inst| store.instance(&inst).and_then(|s| s.me.clone()))
            .is_some_and(|me| me.instance_admin)
    }

    /// Two-factor login and connecting on start, for the Login group of the Account tab.
    pub(super) fn security_rows(
        &self,
        page: &SettingsPage,
        inst: &InstanceId,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let on = page.two_factor == Some(true);
        let form_inst = inst.clone();
        vec![
            setting_fit(
                "Two-factor login",
                if on {
                    "On: signing in also needs a code."
                } else {
                    "Signing in can also need a code from an authenticator app."
                },
                crate::ui::button(
                    self,
                    "two-factor",
                    if on { "Turn off" } else { "Turn on" },
                    ButtonKind::Secondary,
                    true,
                    move |this, window, cx| {
                        if on {
                            this.open_form_two_factor_disable(form_inst.clone(), window, cx);
                        } else {
                            this.open_form_two_factor_begin(form_inst.clone(), window, cx);
                        }
                    },
                    cx,
                ),
            ),
            setting_fit("Connect when Hermes starts", "", {
                let on = self
                    .config
                    .instances
                    .iter()
                    .find(|e| e.id == *inst)
                    .is_none_or(|e| e.autoconnect);
                let inst = inst.clone();
                crate::ui::switch(
                    self,
                    "autoconnect",
                    on,
                    true,
                    move |this, _, cx| {
                        if let Err(err) = this.core.set_autoconnect(&inst, !on) {
                            this.error(err.to_string(), cx);
                        }
                        this.apply_config(cx);
                    },
                    cx,
                )
            }),
        ]
    }

    /// Taking your data with you, or deleting the account.
    pub(super) fn data_group(&self, inst: &InstanceId, cx: &mut Context<Self>) -> AnyElement {
        let (export_inst, delete_inst) = (inst.clone(), inst.clone());
        group(
            "Your data",
            vec![
                setting_fit(
                    "Export",
                    "A file with your profile, servers and messages as the server holds them.",
                    crate::ui::button(
                        self,
                        "export-account",
                        "Export…",
                        ButtonKind::Secondary,
                        true,
                        move |this, _, cx| this.export_account(export_inst.clone(), cx),
                        cx,
                    ),
                ),
                setting_fit(
                    "Delete account",
                    "This cannot be undone.",
                    crate::ui::button(
                        self,
                        "delete-account",
                        "Delete…",
                        ButtonKind::Danger,
                        true,
                        move |this, window, cx| {
                            this.open_form_delete_account(delete_inst.clone(), window, cx);
                        },
                        cx,
                    ),
                ),
            ],
        )
    }

    pub(super) fn export_account(&mut self, inst: InstanceId, cx: &mut Context<Self>) {
        let core = self.core.clone();
        self.run_status(cx, async move {
            let file = rfd::AsyncFileDialog::new()
                .set_file_name("hermes-export.json")
                .save_file()
                .await;
            let Some(file) = file else {
                return Ok("Export cancelled".to_owned());
            };
            let text = core.rest(&inst)?.export_account().await?;
            let path = file.path().to_path_buf();
            let written = path.clone();
            tokio::task::spawn_blocking(move || std::fs::write(written, text))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Ok(format!("Saved to {}", path.display()))
        });
    }

    pub(super) fn load_admin(&mut self, cx: &mut Context<Self>) {
        let Some(inst) = self.account_instance() else {
            return;
        };
        if let Some(page) = &mut self.settings {
            page.admin_users = Remote::Loading;
            page.admin_log = Remote::Loading;
        }
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let rest = core.rest(&inst)?;
                let users = rest.admin_users(None, 200).await?;
                let log = rest.admin_log().await?;
                let settings = rest.admin_settings().await?;
                Ok::<_, String>((users, log, settings))
            },
            |this, result, _, cx| {
                if let Some(page) = &mut this.settings {
                    match result {
                        Ok((users, log, settings)) => {
                            page.admin_users = Remote::Ready(users);
                            page.admin_log = Remote::Ready(log);
                            page.admin_settings = Some(settings);
                        }
                        Err(err) => {
                            page.admin_users = Remote::Failed(err.clone());
                            page.admin_log = Remote::Failed(err);
                        }
                    }
                }
                cx.notify();
            },
        );
    }

    pub(super) fn tab_admin(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        use hermes_client_core::proto::api::{AdminSettings, RegistrationMode as Mode};
        let Some(inst) = self.account_instance() else {
            return muted("Add an account first.");
        };
        let current = page.admin_settings.map(|s| s.registration_mode);
        const MODES: [(Mode, &str); 3] = [
            (Mode::Open, "Open"),
            (Mode::InviteOnly, "Invite only"),
            (Mode::Closed, "Closed"),
        ];
        let registration = {
            let inst = inst.clone();
            crate::ui::segmented(
                self,
                "registration",
                MODES.iter().map(|(_, name)| Segment::new(*name)).collect(),
                current.and_then(|c| MODES.iter().position(|(m, _)| *m == c)),
                true,
                move |this, n, _, cx| {
                    let mode = MODES[n].0;
                    let core = this.core.clone();
                    let inst = inst.clone();
                    this.run(
                        cx,
                        async move {
                            core.rest(&inst)?
                                .set_admin_settings(&AdminSettings {
                                    registration_mode: mode,
                                })
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |this, result, _, cx| match result {
                            Ok(_) => this.load_admin(cx),
                            Err(err) => this.error(err, cx),
                        },
                    );
                },
                cx,
            )
        };
        let mut users: Vec<AnyElement> = Vec::new();
        match &page.admin_users {
            Remote::Idle | Remote::Loading => users.push(theme::skeleton("users", 4)),
            Remote::Failed(err) => users.push(muted(err.clone())),
            Remote::Ready(list) => {
                for entry in list {
                    let id = entry.user.id;
                    let flags = match (entry.instance_admin, entry.disabled) {
                        (true, _) => "admin",
                        (_, true) => "disabled",
                        _ => "",
                    };
                    let disabled = entry.disabled;
                    let admin = entry.instance_admin;
                    let (disable_inst, admin_inst) = (inst.clone(), inst.clone());
                    users.push(setting_fit(
                        &entry.user.handle(),
                        flags,
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(crate::ui::button(
                                self,
                                format!("admin-admin-{id}"),
                                if admin { "Remove admin" } else { "Make admin" },
                                ButtonKind::Secondary,
                                true,
                                move |this, _, cx| {
                                    this.admin_patch(
                                        admin_inst.clone(),
                                        id,
                                        None,
                                        Some(!admin),
                                        cx,
                                    );
                                },
                                cx,
                            ))
                            .child(crate::ui::button(
                                self,
                                format!("admin-disable-{id}"),
                                if disabled { "Enable" } else { "Disable" },
                                if disabled {
                                    ButtonKind::Secondary
                                } else {
                                    ButtonKind::Danger
                                },
                                true,
                                move |this, _, cx| {
                                    this.admin_patch(
                                        disable_inst.clone(),
                                        id,
                                        Some(!disabled),
                                        None,
                                        cx,
                                    );
                                },
                                cx,
                            )),
                    ));
                }
            }
        }
        let mut log: Vec<AnyElement> = Vec::new();
        match &page.admin_log {
            Remote::Idle | Remote::Loading => log.push(theme::skeleton("log", 5)),
            Remote::Failed(err) => log.push(muted(err.clone())),
            Remote::Ready(list) if list.is_empty() => {
                log.push(
                    div()
                        .py(px(12.))
                        .child(muted("Nothing yet."))
                        .into_any_element(),
                );
            }
            Remote::Ready(list) => {
                for entry in list {
                    log.push(log_entry(
                        entry.action.clone(),
                        entry.details.clone(),
                        time::format(entry.created_at, "%Y-%m-%d %H:%M", self.utc_offset),
                    ));
                }
            }
        }
        page_body(
            "Administration",
            None,
            vec![
                group(
                    "Who can make an account",
                    vec![
                        setting_fit(
                            "Registration",
                            "Takes effect at once, and over the config file.",
                            registration,
                        ),
                        setting_fit(
                            "Channels must be encrypted",
                            "Set in the server's config file.",
                            muted(
                                if self
                                    .core
                                    .store()
                                    .instance(&inst)
                                    .and_then(|s| {
                                        s.instance.as_ref().map(|i| i.limits.channels_encrypted)
                                    })
                                    .unwrap_or(false)
                                {
                                    "Yes"
                                } else {
                                    "No"
                                },
                            ),
                        ),
                    ],
                ),
                group("Accounts", users),
                group("Log", log),
            ],
        )
    }

    pub(super) fn admin_patch(
        &mut self,
        inst: InstanceId,
        user: Id,
        disabled: Option<bool>,
        instance_admin: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.rest(&inst)?
                    .admin_update_user(
                        user,
                        &hermes_client_core::proto::api::AdminUserPatch {
                            disabled,
                            instance_admin,
                            tagless: None,
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())
            },
            |this, result, _, cx| match result {
                Ok(()) => this.load_admin(cx),
                Err(err) => this.error(err, cx),
            },
        );
    }
}
