//! "New message": start a DM or group DM with people I share a space with (PLAN.md section
//! 15.1).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, Subscription, Window, div, px};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::store::InstanceStore;

use crate::modal::{dialog, error_line, heading, paragraph};
use crate::shell::Shell;

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
                let rest = core.rest(&target).map_err(|e| e.to_string())?;
                rest.create_dm(ids)
                    .await
                    .map(|c| c.id)
                    .map_err(|e| e.to_string())
            },
            move |this, result, window, cx| match result {
                Ok(channel) => {
                    this.new_dm = None;
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
                    "Usernames of people you share a space with, separated by commas.",
                ))
                .child(Input::new(&state.input).large())
                .children(state.error.clone().map(error_line))
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
                                    this.new_dm = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("new-dm-go")
                                .cursor_pointer()
                                .large()
                                .primary()
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
/// users the store knows. I am left out.
pub fn resolve(store: &InstanceStore, input: &str) -> Result<Vec<Id>, String> {
    let me = store.me_id();
    let mut ids = Vec::new();
    let mut unknown = Vec::new();
    for name in input.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let name = name.trim_start_matches('@');
        let mut matches = store.users.values().filter(|u| {
            Some(u.id) != me
                && (u.username.eq_ignore_ascii_case(name)
                    || u.display_name
                        .as_deref()
                        .is_some_and(|d| d.eq_ignore_ascii_case(name)))
        });
        // A username match wins over a display name.
        let found = store
            .users
            .values()
            .find(|u| Some(u.id) != me && u.username.eq_ignore_ascii_case(name))
            .or_else(|| matches.next());
        match found {
            Some(user) if !ids.contains(&user.id) => ids.push(user.id),
            Some(_) => {}
            None => unknown.push(name.to_owned()),
        }
    }
    if !unknown.is_empty() {
        return Err(format!("Not found: {}", unknown.join(", ")));
    }
    if ids.is_empty() {
        return Err("Enter at least one username.".into());
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

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
}
