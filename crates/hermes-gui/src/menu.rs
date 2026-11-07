//! Right-click menus that open on mouse up. Every click in this app acts on release, and the
//! kit's `context_menu` opens on press, so it is not used.

use std::rc::Rc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, DismissEvent, Div, Entity, Focusable as _, MouseButton, MouseUpEvent,
    Pixels, Point, SharedString, Stateful, Subscription, WeakEntity, Window, anchored, deferred,
    div, px,
};
use hermes_client_core::gateway::ConnStatus;

use crate::menu_entries::{self, Entries, Entry, Grouped, Target};
use crate::shell::Shell;

pub struct OpenMenu {
    view: Entity<PopupMenu>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}

type RunLine = Rc<dyn Fn(&mut Shell, &mut Window, &mut Context<Shell>)>;

/// One entry of a built menu.
#[derive(Clone)]
pub(crate) enum Line {
    Item {
        label: SharedString,
        checked: bool,
        /// Drawn in red: it removes or ends something.
        danger: bool,
        run: RunLine,
    },
    Submenu {
        label: SharedString,
        lines: Vec<Line>,
    },
    Separator,
    /// The "Move to" submenu, filled in by the shell from the voice channels.
    Move,
}

impl Shell {
    /// Gives `element` the menu of `target`, opened when the right button is released over it
    /// after being pressed on it.
    pub fn with_menu(
        &self,
        element: Stateful<Div>,
        target: Target,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pressed = target.clone();
        element
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.right_pressed = Some(pressed.clone())),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    if this.right_pressed.as_ref() == Some(&target) {
                        this.open_menu(target.clone(), event.position, window, cx);
                    }
                }),
            )
    }

    /// The empty space of the rail or of Friends' sidebar: adding instances, and each instance's
    /// menu.
    pub fn with_instances_menu(
        &self,
        element: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.with_menu(element, Target::Instances, cx)
    }

    /// Opens the menu of `target` at `position`, as if it had been right-clicked there.
    pub fn open_menu(
        &mut self,
        target: Target,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lines = self.menu_lines(&target);
        if lines.is_empty() {
            return;
        }
        self.menu_at = position;
        let shell = cx.entity().downgrade();
        let view = build_menu(shell, lines, window, cx);
        view.focus_handle(cx).focus(window, cx);
        let dismiss = cx.subscribe_in(&view, window, |this, _, _: &DismissEvent, _, cx| {
            this.menu = None;
            cx.notify();
        });
        self.menu = Some(OpenMenu {
            view,
            position,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    fn menu_lines(&self, target: &Target) -> Vec<Line> {
        let status = target
            .inst()
            .map_or(ConnStatus::Connected, |inst| self.core.status(inst));
        let extra = menu_entries::Extra {
            presence: Some(self.core.presence()),
            locally_muted: match target {
                Target::Member(inst, _, user) => self.core.user_muted(inst, *user),
                _ => false,
            },
        };
        let entries = {
            let store = self.core.store();
            menu_entries::entries_with(
                target,
                &status,
                target.inst().and_then(|i| store.instance(i)),
                extra,
            )
        };
        let mut lines = Vec::new();
        // The user panel's menu is also a way to the instances, which the sidebar hides.
        if matches!(target, Target::Me(_) | Target::Instances) {
            lines.extend(self.instance_lines());
        }
        for line in entry_lines(target, &entries) {
            match line {
                Line::Move => lines.push(Line::Submenu {
                    label: menu_entries::Action::MoveVoice.label().into(),
                    lines: self.move_lines(target),
                }),
                line => lines.push(line),
            }
        }
        if lines.last().is_some_and(|l| matches!(l, Line::Separator)) {
            lines.pop();
        }
        lines
    }

    /// "Add an instance", and the actions of each saved instance.
    fn instance_lines(&self) -> Vec<Line> {
        let mut lines = vec![Line::Item {
            label: "Add an instance…".into(),
            checked: false,
            danger: false,
            run: Rc::new(|this, window, cx| this.open_add_instance(String::new(), window, cx)),
        }];
        for inst in self.core.instance_ids() {
            let status = self.core.status(&inst);
            let mut inside = Vec::new();
            if status == ConnStatus::LoggedOut {
                let sign_in = inst.clone();
                inside.push(Line::Item {
                    label: "Sign in…".into(),
                    checked: false,
                    danger: false,
                    run: Rc::new(move |this, window, cx| this.sign_in(&sign_in, window, cx)),
                });
            }
            let target = Target::Instance(inst.clone());
            let entries = {
                let store = self.core.store();
                menu_entries::entries_with(
                    &target,
                    &status,
                    store.instance(&inst),
                    menu_entries::Extra::default(),
                )
            };
            inside.extend(entry_lines(&target, &entries));
            lines.push(Line::Submenu {
                label: inst.into(),
                lines: inside,
            });
        }
        lines.push(Line::Separator);
        lines
    }

    pub fn render_menu(&self) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(menu.view.clone()),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }
}

/// The lines of `entries`, with neighbours of a group in a submenu.
fn entry_lines(target: &Target, entries: &Entries) -> Vec<Line> {
    // The channel area's few entries stay flat, and act on the server.
    if let Target::ChannelArea(inst, space) = target {
        let server = Target::Space(inst.clone(), *space);
        return entries
            .iter()
            .map(|entry| match entry {
                None => Line::Separator,
                Some(entry) => action_line(&server, *entry, entry.action.area_label()),
            })
            .collect();
    }
    menu_entries::grouped(entries)
        .into_iter()
        .map(|line| match line {
            Grouped::Separator => Line::Separator,
            Grouped::Entry(entry) => action_line(target, entry, entry.action.label()),
            Grouped::Submenu { label, entries } => Line::Submenu {
                label: label.into(),
                // Not grouped again: these are the members of the group.
                lines: entries
                    .iter()
                    .map(|entry| match entry {
                        None => Line::Separator,
                        Some(entry) => action_line(target, *entry, entry.action.short_label()),
                    })
                    .collect(),
            },
        })
        .collect()
}

/// A plain entry that does `run`, for menus the shell builds itself.
pub(crate) fn run_line(
    label: impl Into<SharedString>,
    run: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) -> Line {
    Line::Item {
        label: label.into(),
        checked: false,
        danger: false,
        run: Rc::new(run),
    }
}

fn action_line(target: &Target, entry: Entry, label: &'static str) -> Line {
    if entry.action == menu_entries::Action::MoveVoice {
        return Line::Move;
    }
    let (target, action) = (target.clone(), entry.action);
    Line::Item {
        label: label.into(),
        checked: entry.checked,
        danger: action.is_negative(),
        run: Rc::new(move |this, window, cx| {
            this.menu_action(target.clone(), action, window, cx);
        }),
    }
}

fn build_menu(
    shell: WeakEntity<Shell>,
    lines: Vec<Line>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PopupMenu> {
    PopupMenu::build(window, cx, move |menu, window, cx| {
        add_lines(menu, &shell, lines, window, cx)
    })
}

fn add_lines(
    mut menu: PopupMenu,
    shell: &WeakEntity<Shell>,
    lines: Vec<Line>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    for line in lines {
        menu = match line {
            Line::Separator => menu.separator(),
            // Replaced by its submenu before the menu is built.
            Line::Move => menu,
            Line::Submenu { label, lines } => {
                let shell = shell.clone();
                menu.submenu(label, window, cx, move |menu, window, cx| {
                    add_lines(menu, &shell, lines.clone(), window, cx)
                })
            }
            Line::Item {
                label,
                checked,
                danger,
                run,
            } => {
                let shell = shell.clone();
                // The kit's plain items cannot set a cursor, so the row brings its own.
                let row = PopupMenuItem::element(move |_, _| {
                    div()
                        .flex()
                        .flex_1()
                        .self_stretch()
                        .items_center()
                        .cursor_pointer()
                        .when(danger, |d| d.text_color(crate::theme::danger()))
                        .child(label.clone())
                });
                menu.item(row.checked(checked).on_click(move |_, window, cx| {
                    let _ = shell.update(cx, |this, cx| {
                        this.menu = None;
                        run(this, window, cx);
                        cx.notify();
                    });
                }))
            }
        };
    }
    menu
}
