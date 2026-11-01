//! The Account, Chat, Notifications and About settings tabs: profile, avatar, password,
//! sessions, plain config flags and debug info.

use hermes_client_core::InstanceId;
use hermes_client_core::proto::api::{ChangePasswordRequest, UpdateMeRequest};
use iced::Task;

use super::App;
use crate::buffer::settings::{Message as S, Tab};
use crate::buffer::space_settings::Remote;
use crate::message::Message;
use crate::modal::form::{Field, Form, Purpose};

impl App {
    /// The instance the Account tab shows.
    pub(super) fn account_instance(&self) -> Option<InstanceId> {
        let ids = self.core.instance_ids();
        self.account_inst
            .clone()
            .filter(|i| ids.contains(i))
            .or_else(|| ids.first().cloned())
    }

    /// Handles the messages of the newer tabs; gives the others back.
    pub(super) fn on_account(&mut self, message: S) -> Result<Task<Message>, S> {
        let task = match message {
            S::Tab(Tab::Account) => {
                self.settings_tab = Tab::Account;
                self.load_sessions()
            }
            S::AccountInstance(inst) => {
                self.account_inst = Some(inst);
                self.load_sessions()
            }
            S::LoadSessions => self.load_sessions(),
            S::Sessions(result) => {
                self.sessions = match result {
                    Ok(list) => Remote::Ready(list),
                    Err(err) => Remote::Failed(err),
                };
                Task::none()
            }
            S::RevokeSession(id) => match self.account_instance() {
                Some(inst) => self.call(
                    &inst,
                    move |r| async move { r.delete_session(id).await },
                    |r| match r {
                        Ok(()) => Message::Settings(S::LoadSessions),
                        Err(err) => Message::Done(Err(format!("could not sign out: {err}"))),
                    },
                ),
                None => Task::none(),
            },
            S::RevokeOthers => {
                let others: Vec<_> = self
                    .sessions
                    .ready()
                    .map(|l| l.iter().filter(|s| !s.current).map(|s| s.id).collect())
                    .unwrap_or_default();
                match self.account_instance() {
                    Some(inst) => self.call(
                        &inst,
                        move |r| async move {
                            for id in others {
                                r.delete_session(id).await?;
                            }
                            Ok(())
                        },
                        |r| match r {
                            Ok(()) => Message::Settings(S::LoadSessions),
                            Err(err) => Message::Done(Err(format!("could not sign out: {err}"))),
                        },
                    ),
                    None => Task::none(),
                }
            }
            S::EditProfile => match self.account_instance() {
                Some(inst) => self.open_form_profile(inst),
                None => Task::none(),
            },
            S::ChangePassword => match self.account_instance() {
                Some(inst) => {
                    self.form = Some(
                        Form::new(Purpose::ChangePassword(inst), "Change password", "Change")
                            .text("Your other devices are signed out afterwards.")
                            .field(Field::password("Current password"))
                            .field(Field::password("New password"))
                            .field(Field::password("New password again")),
                    );
                    iced::widget::operation::focus(iced::widget::Id::new(
                        crate::modal::form::FIRST_INPUT,
                    ))
                }
                None => Task::none(),
            },
            S::PickAvatar => match self.account_instance() {
                Some(inst) => {
                    let core = self.core.clone();
                    Task::perform(
                        async move {
                            let Some(file) = rfd::AsyncFileDialog::new()
                                .set_title("Choose a picture")
                                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
                                .pick_file()
                                .await
                            else {
                                return Ok(None);
                            };
                            let avatar = core.upload(&inst, file.path()).await?;
                            core.rest(&inst)?
                                .update_me(&UpdateMeRequest {
                                    avatar_id: Some(Some(avatar.id)),
                                    ..UpdateMeRequest::default()
                                })
                                .await?;
                            Ok::<_, hermes_client_core::CoreError>(Some(()))
                        },
                        |r| match r {
                            Ok(Some(())) => Message::Done(Ok("Avatar changed".into())),
                            Ok(None) => Message::Nothing,
                            Err(err) => Message::Done(Err(format!("could not change it: {err}"))),
                        },
                    )
                }
                None => Task::none(),
            },
            S::RemoveAvatar => match self.account_instance() {
                Some(inst) => self.call_status(&inst, "Avatar removed", |r| async move {
                    r.update_me(&UpdateMeRequest {
                        avatar_id: Some(None),
                        ..UpdateMeRequest::default()
                    })
                    .await
                }),
                None => Task::none(),
            },
            S::Flag(key, on) => {
                self.persist(key, if on { "true" } else { "false" });
                Task::none()
            }
            S::GroupMinutes(minutes) => {
                self.config.buffer.group_minutes = minutes;
                Task::none()
            }
            S::SaveGroupMinutes => {
                let value = self.config.buffer.group_minutes.to_string();
                self.persist("buffer.group_minutes", &value);
                Task::none()
            }
            S::TimestampFormat(text) => {
                self.config.buffer.timestamp_format = text;
                Task::none()
            }
            S::SaveTimestampFormat => {
                let value =
                    toml::Value::String(self.config.buffer.timestamp_format.clone()).to_string();
                self.persist("buffer.timestamp_format", &value);
                self.info("Time format saved".into());
                Task::none()
            }
            S::CopyDebugInfo => {
                let text = self.debug_info();
                self.update(Message::Copy(text, "Debug info"))
            }
            S::OpenLogs => {
                if let Some(paths) = self.core.paths() {
                    let dir = paths.dir.join("logs");
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(err) = crate::open::path(&dir) {
                        self.error(format!("could not open {}: {err}", dir.display()));
                    }
                }
                Task::none()
            }
            S::OpenProjectPage => {
                if let Err(err) = crate::open::url(env!("CARGO_PKG_REPOSITORY")) {
                    self.error(format!("could not open the browser: {err}"));
                }
                Task::none()
            }
            other => return Err(other),
        };
        Ok(task)
    }

    fn load_sessions(&mut self) -> Task<Message> {
        let Some(inst) = self.account_instance() else {
            return Task::none();
        };
        self.sessions = Remote::Loading;
        self.call(
            &inst,
            |r| async move { r.sessions().await },
            |r| Message::Settings(S::Sessions(r)),
        )
    }

    /// Version, system and connection state, for bug reports (no tokens or message content).
    pub(super) fn debug_info(&self) -> String {
        let mut out = format!(
            "Hermes {} ({} {})\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        for inst in self.core.instance_ids() {
            out.push_str(&format!("instance {inst}: {:?}\n", self.core.status(&inst)));
        }
        out.push_str(&format!("voice: {:?}\n", self.core.voice_status()));
        out.push_str(&format!(
            "theme {}, font size {}, ui scale {}\n",
            self.config.general.theme, self.config.font.size, self.config.font.ui_scale
        ));
        if let Some(paths) = self.core.paths() {
            out.push_str(&format!("config dir: {}\n", paths.dir.display()));
        }
        out
    }

    /// The submit of the change-password form.
    pub(super) fn submit_password(
        &self,
        inst: InstanceId,
        current: String,
        new: String,
        again: String,
    ) -> Result<Task<Message>, String> {
        if current.is_empty() || new.is_empty() {
            return Err("Please fill in both passwords.".into());
        }
        if new != again {
            return Err("The new passwords are not the same.".into());
        }
        let core = self.core.clone();
        Ok(Task::perform(
            async move {
                core.rest(&inst)?
                    .change_password(&ChangePasswordRequest {
                        current_password: current,
                        new_password: new,
                    })
                    .await?;
                Ok::<_, hermes_client_core::CoreError>(())
            },
            |r| {
                Message::FormDone(
                    r.map(|()| "Password changed; other devices were signed out".into())
                        .map_err(|e| e.to_string()),
                )
            },
        ))
    }
}
