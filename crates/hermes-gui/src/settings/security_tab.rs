//! Security, your data, and Administration.

use super::*;

impl Shell {
    pub(super) fn is_instance_admin(&self) -> bool {
        let store = self.core.store();
        self.account_instance()
            .and_then(|inst| store.instance(&inst).and_then(|s| s.me.clone()))
            .is_some_and(|me| me.instance_admin)
    }

    pub(super) fn security_group(
        &self,
        page: &SettingsPage,
        inst: &InstanceId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = page.two_factor == Some(true);
        let (form_inst, export_inst, delete_inst) = (inst.clone(), inst.clone(), inst.clone());
        group(
            "Security and your data",
            vec![
                setting(
                    "Two-factor login",
                    if on {
                        "On: signing in also needs a code."
                    } else {
                        "Signing in can also need a code from an authenticator app."
                    },
                    Button::new("two-factor")
                        .cursor_pointer()
                        .label(if on { "Turn off" } else { "Turn on" })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if on {
                                this.open_form_two_factor_disable(form_inst.clone(), window, cx);
                            } else {
                                this.open_form_two_factor_begin(form_inst.clone(), window, cx);
                            }
                        })),
                ),
                setting(
                    "Connect when Hermes starts",
                    "",
                    Switch::new("autoconnect")
                        .cursor_pointer()
                        .checked(
                            self.config
                                .instances
                                .iter()
                                .find(|e| e.id == *inst)
                                .is_none_or(|e| e.autoconnect),
                        )
                        .on_click(cx.listener({
                            let inst = inst.clone();
                            move |this, on: &bool, _, cx| {
                                if let Err(err) = this.core.set_autoconnect(&inst, *on) {
                                    this.error(err.to_string(), cx);
                                }
                                this.apply_config(cx);
                            }
                        })),
                ),
                setting(
                    "Your data",
                    "A file with your profile, servers and messages as the server holds them.",
                    Button::new("export-account")
                        .cursor_pointer()
                        .label("Export…")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.export_account(export_inst.clone(), cx);
                        })),
                ),
                setting(
                    "Delete account",
                    "This cannot be undone.",
                    Button::new("delete-account")
                        .cursor_pointer()
                        .danger()
                        .label("Delete…")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_form_delete_account(delete_inst.clone(), window, cx);
                        })),
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
        let chips = [
            (Mode::Open, "Open"),
            (Mode::InviteOnly, "Invite only"),
            (Mode::Closed, "Closed"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (mode, name))| {
            let inst = inst.clone();
            crate::ui::chip(self, ("registration", n), name, current == Some(mode), cx).on_click(
                cx.listener(move |this, _, _, cx| {
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
                }),
            )
        });
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
                    users.push(setting(
                        &entry.user.handle(),
                        flags,
                        wrap()
                            .child(
                                Button::new(SharedString::from(format!("admin-disable-{id}")))
                                    .cursor_pointer()
                                    .label(if disabled { "Enable" } else { "Disable" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.admin_patch(
                                            disable_inst.clone(),
                                            id,
                                            Some(!disabled),
                                            None,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("admin-admin-{id}")))
                                    .cursor_pointer()
                                    .label(if admin { "Remove admin" } else { "Make admin" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.admin_patch(
                                            admin_inst.clone(),
                                            id,
                                            None,
                                            Some(!admin),
                                            cx,
                                        );
                                    })),
                            ),
                    ));
                }
            }
        }
        let mut log: Vec<AnyElement> = Vec::new();
        match &page.admin_log {
            Remote::Idle | Remote::Loading => log.push(theme::skeleton("log", 5)),
            Remote::Failed(err) => log.push(muted(err.clone())),
            Remote::Ready(list) if list.is_empty() => log.push(muted("Nothing yet.")),
            Remote::Ready(list) => {
                for entry in list {
                    log.push(muted(format!(
                        "{} · {} · {}",
                        time::format(entry.created_at, "%Y-%m-%d %H:%M", self.utc_offset),
                        entry.action,
                        entry.details
                    )));
                }
            }
        }
        page_body(
            "Instance",
            None,
            vec![
                group(
                    "Who can make an account",
                    vec![
                        setting(
                            "Registration",
                            "Takes effect at once, and over the config file.",
                            wrap().children(chips),
                        ),
                        setting(
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
