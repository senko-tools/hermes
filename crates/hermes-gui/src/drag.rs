//! Drag and drop for lists (roles in the space settings, channels in the sidebar and the
//! settings). iced buttons take the mouse press, so rows only report the pointer entering and
//! leaving them; a window-wide listener reports the press and the release. A press over a row
//! picks it up, a release over another row drops it there (PLAN.md sections 15.1 and 15.4).

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Role,
    Channel,
}

/// A draggable row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Item {
    pub inst: InstanceId,
    pub space: Id,
    pub kind: Kind,
    pub id: Id,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drag {
    /// The row under the pointer.
    pub hover: Option<Item>,
    /// The row picked up by a press, while the button is held.
    pub picked: Option<Item>,
}

impl Drag {
    /// The pointer entered `item` (or left every row, `None`).
    pub fn hover(&mut self, item: Option<Item>) {
        self.hover = item;
    }

    /// The pointer left `item`; ignored if it already entered another row.
    pub fn leave(&mut self, item: &Item) {
        if self.hover.as_ref() == Some(item) {
            self.hover = None;
        }
    }

    pub fn press(&mut self) {
        self.picked = self.hover.clone();
    }

    /// The button was released: `(picked, target)` if a row was dropped on another row of the
    /// same list.
    pub fn release(&mut self) -> Option<(Item, Item)> {
        let picked = self.picked.take()?;
        let target = self.hover.clone()?;
        (picked != target
            && picked.kind == target.kind
            && picked.inst == target.inst
            && picked.space == target.space)
            .then_some((picked, target))
    }

    /// The row being dragged over another (for drawing the drop marker).
    pub fn target(&self) -> Option<&Item> {
        match (&self.picked, &self.hover) {
            (Some(p), Some(h)) if p != h && p.kind == h.kind => Some(h),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: Kind, id: u64) -> Item {
        Item {
            inst: "h".into(),
            space: Id(1),
            kind,
            id: Id(id),
        }
    }

    #[test]
    fn press_hover_release_drops() {
        let mut d = Drag::default();
        d.press();
        assert_eq!(d.release(), None, "a press outside any row picks nothing");

        d.hover(Some(item(Kind::Role, 2)));
        d.press();
        assert!(d.target().is_none());
        // Entering the next row can come before leaving the last one.
        d.hover(Some(item(Kind::Role, 3)));
        d.leave(&item(Kind::Role, 2));
        assert!(d.target().is_some());
        assert_eq!(
            d.release(),
            Some((item(Kind::Role, 2), item(Kind::Role, 3)))
        );
        assert_eq!(d.release(), None, "only once");

        // A click without moving drops nothing; different lists never mix.
        d.press();
        assert_eq!(d.release(), None);
        d.press();
        d.hover(Some(item(Kind::Channel, 4)));
        assert_eq!(d.release(), None);
    }
}
