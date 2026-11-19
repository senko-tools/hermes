//! "New message": start a DM or group DM with people I share a space with (PLAN.md section
//! 15.1).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, Subscription, Window, div};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::store::InstanceStore;

use crate::modal::{dialog, error_line, heading, paragraph};
use crate::shell::Shell;
use crate::theme::px;

pub struct NewDm {
    pub inst: InstanceId,
    input: Entity<InputState>,
    pub error: Option<String>,
    pub busy: bool,
    _events: Subscription,
}

impl Shell {
    pub fn open_new_dm(&mut self, inst: InstanceId, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("alice, bob"));
        let events = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_new_dm(cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.cancel_closing("new-dm");
        self.new_dm = Some(NewDm {
            inst,
            input,
            error: None,
            busy: false,
            _events: events,
        });
        cx.notify();
    }

    fn submit_new_dm(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.new_dm else {
            return;
        };
        if dialog.busy {
            return;
        }
        let inst = dialog.inst.clone();
        let text = dialog.input.read(cx).value().to_string();
        let ids = match self.core.store().instance(&inst) {
            Some(store) => resolve(store, &text),
            None => Err("Not connected to this instance.".into()),
        };
        let ids = match ids {
            Ok(ids) => ids,
            Err(err) => {
                dialog.error = Some(err);
                cx.notify();
                return;
            }
        };
        dialog.busy = true;
        dialog.error = None;
        let core = self.core.clone();
        let target = inst.clone();
        self.run(
            cx,
            async move {
                let rest = core.rest(&target)?;
                rest.create_dm(ids)
                    .await
                    .map(|c| c.id)
                    .map_err(|e| e.to_string())
            },
            move |this, result, window, cx| match result {
                Ok(channel) => {
                    this.close_later("new-dm", |s| s.new_dm = None, cx);
                    this.select(inst, channel, window, cx);
                }
                Err(err) => {
                    if let Some(dialog) = &mut this.new_dm {
                        dialog.busy = false;
                        dialog.error = Some(err);
                    }
                }
            },
        );
        cx.notify();
    }

    pub fn render_new_dm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.new_dm.as_ref()?;
        Some(dialog(
            "new-dm",
            440.,
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(heading("New message"))
                .child(paragraph(
                    "Names of your friends and people you share a server with, separated by commas (name or name#1234).",
                ))
                .child(Input::new(&state.input).large())
                .children(state.error.clone().map(|e| error_line(self, "new-dm-error", e, cx)))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            Button::new("new-dm-cancel")
                                .cursor_pointer()
                                .large()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_later("new-dm", |s| s.new_dm = None, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("new-dm-go")
                                .cursor_pointer()
                                .large()
                                .primary()
                                .loading(state.busy)
                                .label(if state.busy { "Please wait…" } else { "Open" })
                                .disabled(state.busy)
                                .when(state.busy, |b| b.cursor_default())
                                .on_click(cx.listener(|this, _, _, cx| this.submit_new_dm(cx))),
                        ),
                ),
        ))
    }
}

/// Turns "alice, Bob" into user IDs, by username or display name (case-insensitive), among the
/// users the store knows. I am left out. A display name several people share is not guessed.
pub fn resolve(store: &InstanceStore, input: &str) -> Result<Vec<Id>, String> {
    let me = store.me_id();
    let mut ids = Vec::new();
    let mut unknown = Vec::new();
    let mut ambiguous = Vec::new();
    let others = || store.users.values().filter(move |u| Some(u.id) != me);
    for name in input.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let name = name.trim_start_matches('@');
        // Display names are any text, so they are compared in full Unicode lowercase.
        let lower = name.to_lowercase();
        let mut by_display = others().filter(|u| {
            u.display_name
                .as_deref()
                .is_some_and(|d| d.to_lowercase() == lower)
        });
        // A whole `name#1234` wins, then a username, then a display name.
        let found = others()
            .find(|u| u.handle().eq_ignore_ascii_case(name))
            .or_else(|| others().find(|u| u.username.eq_ignore_ascii_case(name)))
            .map(Ok)
            .or_else(|| match (by_display.next(), by_display.next()) {
                (Some(user), None) => Some(Ok(user)),
                (Some(_), Some(_)) => Some(Err(())),
                _ => None,
            });
        match found {
            Some(Ok(user)) if !ids.contains(&user.id) => ids.push(user.id),
            Some(Ok(_)) => {}
            Some(Err(())) => ambiguous.push(name.to_owned()),
            None => unknown.push(name.to_owned()),
        }
    }
    if !unknown.is_empty() {
        return Err(format!("Not found: {}", unknown.join(", ")));
    }
    if !ambiguous.is_empty() {
        return Err(format!(
            "More than one person is called {}: use their username.",
            ambiguous.join(", ")
        ));
    }
    if ids.is_empty() {
        return Err("Enter at least one username.".into());
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_client_core::testing::fixtures;

    #[test]
    fn names_resolve_to_known_users() {
        let store = fixtures::store();
        assert_eq!(resolve(&store, "carol"), Ok(vec![Id(4)]));
        assert_eq!(
            resolve(&store, " @Bob , bobby, CAROL "),
            Ok(vec![Id(3), Id(4)]),
            "username or display name, any case, no duplicates"
        );
        assert_eq!(
            resolve(&store, "carol, dave, me"),
            Err("Not found: dave, me".into()),
            "I am not a recipient"
        );
        assert!(resolve(&store, " , ").is_err());
    }

    #[test]
    fn display_names_compare_in_unicode_and_are_not_guessed() {
        let mut store = fixtures::store();
        store
            .users
            .insert(Id(5), fixtures::user(5, "asa", Some("Åsa")));
        assert_eq!(resolve(&store, "åsa"), Ok(vec![Id(5)]), "Unicode case");
        store
            .users
            .insert(Id(6), fixtures::user(6, "asa2", Some("ÅSA")));
        assert!(
            resolve(&store, "åsa").is_err_and(|e| e.contains("More than one")),
            "two people share the name"
        );
        assert_eq!(
            resolve(&store, "asa2"),
            Ok(vec![Id(6)]),
            "the username decides"
        );
    }
}
