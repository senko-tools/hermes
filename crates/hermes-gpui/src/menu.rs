//! Right-click menus that open on mouse up. Every click in this app acts on release, and the
//! kit's `context_menu` opens on press, so it is not used.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, DismissEvent, Div, Entity, Focusable as _, MouseButton, MouseUpEvent,
    Pixels, Point, Stateful, Subscription, Window, anchored, deferred, px,
};

use crate::shell::Shell;
use crate::sidebar::open_add_instance;

/// What was right-clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuTarget {
    /// The empty space of the rail or the sidebar.
    EmptySpace,
}

pub struct OpenMenu {
    view: Entity<PopupMenu>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}

impl Shell {
    /// Gives `element` the menu of `target`, opened when the right button is released over it
    /// after being pressed on it.
    pub fn with_menu(
        &self,
        element: Stateful<Div>,
        target: MenuTarget,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        element
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.right_pressed = Some(target)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    if this.right_pressed == Some(target) {
                        this.open_menu(target, event.position, window, cx);
                    }
                }),
            )
    }

    fn open_menu(
        &mut self,
        target: MenuTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = PopupMenu::build(window, cx, move |menu, _, _| match target {
            MenuTarget::EmptySpace => menu.item(
                PopupMenuItem::new("Add an instance...")
                    .on_click(|_, window, cx| open_add_instance(window, cx)),
            ),
        });
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
