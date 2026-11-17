//! Right-click menus that open on mouse up. Every click in this app acts on release, and the
//! kit's `context_menu` opens on press, so it is not used.

use std::rc::Rc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, DismissEvent, Div, Entity, Focusable as _, MouseButton, MouseUpEvent,
    Pixels, Point, SharedString, Stateful, Subscription, WeakEntity, Window, anchored, deferred,
    div,
};
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;

use crate::menu_entries::{self, Entries, Entry, Grouped, Target};
use crate::selection::FilePlace;
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
            .child(self.popup_trigger())
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

    /// A child that notes where its parent is painted, so a popup opened from it can hug it.
    /// It draws nothing; absolute children are placed against their parent in gpui.
    pub fn popup_trigger(&self) -> AnyElement {
        let triggers = self.popup_triggers.clone();
        gpui_kit::canvas(
            move |bounds, _, _| triggers.borrow_mut().push(bounds),
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    /// `element` (a button that opens a popup) with a [`Shell::popup_trigger`].
    pub fn hug_target(&self, element: AnyElement) -> AnyElement {
        div()
            .flex()
            .flex_shrink_0()
            .child(element)
            .child(self.popup_trigger())
            .into_any_element()
    }

    /// `at` and the most specific thing under it that a popup can hug.
    pub fn popup_from(&self, at: Point<Pixels>) -> crate::modal::PopupFrom {
        crate::modal::PopupFrom::find(at, &self.popup_triggers.borrow())
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
        self.menu_from = self.popup_from(position);
        let shell = cx.entity().downgrade();
        let view = build_menu(shell, lines, window, cx);
        view.focus_handle(cx).focus(window, cx);
        let dismiss = cx.subscribe_in(&view, window, |this, _, _: &DismissEvent, _, cx| {
            this.close_later("menu", |s| s.menu = None, cx);
            cx.notify();
        });
        self.cancel_closing("menu");
        self.menu = Some(OpenMenu {
            view,
            position,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    /// Gives a file its own menu instead of its message's. While the mouse is over it, its
    /// message is not highlighted (`hovered_file`).
    pub fn with_file_menu(
        &self,
        element: Stateful<Div>,
        inst: &str,
        place: FilePlace,
        attachment: Id,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let target = Target::File(inst.to_owned(), place.channel, place.message, attachment);
        let message = place.message;
        // On a wrapper: gpui allows one hover listener per element, and a playing video's
        // surface has its own (its controls fade with the pointer).
        let element = div()
            .id(SharedString::from(format!("file-{attachment}-hover")))
            .child(element)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered_file = Some(message);
                } else if this.hovered_file == Some(message) {
                    this.hovered_file = None;
                }
                cx.notify();
            }));
        self.with_own_menu(element, target, cx)
    }

    /// [`Shell::with_menu`] for something inside an element with a menu of its own. gpui calls
    /// an element's listeners last added first, so what stops the event is added before the menu.
    pub fn with_own_menu(
        &self,
        element: Stateful<Div>,
        target: Target,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let element = element
            .on_mouse_down(MouseButton::Right, |_, _, cx: &mut App| {
                cx.stop_propagation()
            })
            .on_mouse_up(MouseButton::Right, |_, _, cx: &mut App| {
                cx.stop_propagation()
            });
        self.with_menu(element, target, cx)
    }

    /// The menu of a file: opening it, downloading it, copying its name.
    fn file_lines(&self, inst: &str, channel: Id, message: Id, file: Id) -> Vec<Line> {
        let attachment = {
            let store = self.core.store();
            store.instance(inst).and_then(|s| {
                s.window(channel)?
                    .get(message)?
                    .attachments
                    .iter()
                    .find(|a| a.id == file)
                    .cloned()
            })
        };
        let Some(a) = attachment else {
            return Vec::new();
        };
        let inst = inst.to_owned();
        let mut lines = Vec::new();
        if crate::channel::is_image(&a) {
            let (open_inst, open) = (inst.clone(), a.clone());
            lines.push(run_line("Open", move |this, _, cx| {
                this.open_image(open_inst.clone(), open.clone(), cx);
            }));
        } else if crate::media::is_video(&a.content_type, &a.filename)
            || crate::media::is_sound(&a.content_type, &a.filename)
        {
            let (play_inst, play) = (inst.clone(), a.clone());
            let label = if self.media_playing(&inst, a.id) {
                "Pause"
            } else {
                "Play"
            };
            lines.push(run_line(label, move |this, _, cx| {
                this.media_toggle(play_inst.clone(), play.clone(), cx);
            }));
            if self.media_can_go_full(&inst, a.id) {
                let full_inst = inst.clone();
                let id = a.id;
                lines.push(run_line("Full view", move |this, _, cx| {
                    this.media_full(&(full_inst.clone(), id), true, cx);
                }));
            }
        }
        let (save_inst, save) = (inst, a.clone());
        lines.push(run_line("Download", move |this, _, cx| {
            this.download(save_inst.clone(), save.clone(), cx);
        }));
        let name = a.filename;
        lines.push(run_line("Copy file name", move |this, _, cx| {
            this.copy(name.clone(), "File name", cx);
        }));
        lines
    }

    fn menu_lines(&self, target: &Target) -> Vec<Line> {
        if let Target::File(inst, channel, message, file) = target {
            return self.file_lines(inst, *channel, *message, *file);
        }
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
        // Taken now: the left press on the entry clears the selection.
        if matches!(target, Target::Message(..))
            && let Some(text) = self.selected_text()
        {
            lines.push(run_line("Copy selection", move |this, _, cx| {
                this.copy(text.clone(), "Text", cx);
            }));
            lines.push(Line::Separator);
        }
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
        use gpui_kit::AnimationExt as _;
        let menu = self.menu.as_ref()?;
        // Each menu fades in as it drops a little away from the pointer.
        let leaving = crate::modal::closing("menu");
        let key = SharedString::from(format!(
            "menu-{}-{}-{}",
            if leaving { "out" } else { "in" },
            menu.position.x.as_f32(),
            menu.position.y.as_f32()
        ));
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(crate::theme::px(8.))
                    .child(
                        div().relative().child(menu.view.clone()).with_animation(
                            key,
                            gpui_kit::Animation::new(crate::theme::fade())
                                .with_easing(gpui_kit::ease_out_quint()),
                            move |d, t| {
                                let t = if leaving { 1. - t } else { t };
                                d.opacity(t).top(crate::theme::px(-4. * (1. - t)))
                            },
                        ),
                    ),
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
    // A mute that ends says when.
    let label: SharedString = match entry.until {
        Some(until) => {
            let offset = hermes_client_core::time::local_offset_secs();
            let today = hermes_client_core::time::day(hermes_client_core::now_ms(), offset);
            let pattern = if hermes_client_core::time::day(until, offset) == today {
                "%H:%M"
            } else {
                "%d.%m. %H:%M"
            };
            format!(
                "{label} (muted until {})",
                hermes_client_core::time::format(until, pattern, offset)
            )
            .into()
        }
        None => label.into(),
    };
    Line::Item {
        label,
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
                        this.close_later("menu", |s| s.menu = None, cx);
                        run(this, window, cx);
                        cx.notify();
                    });
                }))
            }
        };
    }
    menu
}
