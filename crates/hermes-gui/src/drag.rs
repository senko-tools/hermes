//! What can be dragged: roles in the space settings and channels in the sidebar and the settings
//! (PLAN.md sections 15.1 and 15.4). gpui does the dragging; a row carries an [`Item`].

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
pub use hermes_client_core::view::Place;

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

impl Item {
    /// Whether this row can be dropped on `target`: another row of the same list.
    pub fn can_drop_on(&self, target: &Item) -> bool {
        self != target
            && self.kind == target.kind
            && self.inst == target.inst
            && self.space == target.space
    }
}

/// What a channel list row is, for where a drop on it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Channel,
    /// A category; `open` when its channels show under it.
    Category {
        open: bool,
    },
}

/// A row of a channel list as it is on screen, for [`slot_for`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowBox {
    pub id: Id,
    pub shape: Shape,
    /// A channel inside a category.
    pub nested: bool,
    pub top: f32,
    pub bottom: f32,
}

/// Where a dragged channel would land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    /// At `place` of that row.
    At(Id, Place),
    /// Last at the top level, after everything.
    End,
}

/// Over a category, below this share of its height a channel goes into it (above, before it).
const INTO_FROM: f32 = 0.35;
/// Further than this under the last row is the end of the list.
const END_GAP: f32 = 8.;

/// Where a channel dropped at height `y` lands, from the rows of its list (in any order). The
/// whole list takes drops, gaps and margins too, so there is always an answer (unless the list
/// is empty):
/// - a channel over a category's lower part goes into it (first);
/// - otherwise the nearest row edge: above a row, or below a channel row (a category has no
///   "below": dropped on, a channel goes into it). Edges of a nested row mean inside its
///   category, others the top level, so the bottom of a category's last channel and the top of
///   the next category (10 px further) are two places;
/// - a category only goes above top-level rows, or to the end;
/// - under the last row: the end.
pub fn slot_for(rows: &[RowBox], y: f32, dragging_category: bool) -> Option<Slot> {
    let last_bottom = rows.iter().map(|r| r.bottom).reduce(f32::max)?;
    if y > last_bottom + END_GAP {
        return Some(Slot::End);
    }
    let category = |r: &RowBox| matches!(r.shape, Shape::Category { .. });
    if !dragging_category
        && let Some(r) = rows
            .iter()
            .find(|r| category(r) && y >= r.top + (r.bottom - r.top) * INTO_FROM && y <= r.bottom)
    {
        return Some(Slot::At(r.id, Place::Into));
    }
    let mut rows: Vec<&RowBox> = rows
        .iter()
        .filter(|r| !dragging_category || !r.nested)
        .collect();
    rows.sort_by(|a, b| a.top.total_cmp(&b.top));
    let edges = rows.iter().flat_map(|r| {
        let after = (!category(r) && !dragging_category)
            .then_some((r.bottom, Slot::At(r.id, Place::After)));
        std::iter::once((r.top, Slot::At(r.id, Place::Before))).chain(after)
    });
    // The end sits a little under the last row, so the last row's own lower edge comes first.
    edges
        .chain(std::iter::once((last_bottom + END_GAP / 2., Slot::End)))
        .map(|(at, slot)| ((y - at).abs(), slot))
        // The first of equally near edges: `min_by` keeps the last one.
        .fold(None, |best: Option<(f32, Slot)>, (d, slot)| match best {
            Some((bd, _)) if bd <= d => best,
            _ => Some((d, slot)),
        })
        .map(|(_, slot)| slot)
}

/// The little label following the pointer while dragging.
pub struct Ghost(pub String);

impl gpui_kit::Render for Ghost {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use crate::theme;
        use gpui_kit::AnimationExt as _;
        use gpui_kit::prelude::*;
        // A lifted card: raised, outlined and shadowed, a little see-through so the place it
        // goes shows under it. It pops up from the row as the drag starts.
        gpui_kit::div()
            .flex()
            .items_center()
            .gap(theme::px(6.))
            .px(theme::px(10.))
            .py(theme::px(5.))
            .rounded(theme::radius_small())
            .bg(theme::raised())
            .border_1()
            .border_color(theme::accent())
            .shadow_md()
            .text_color(theme::text())
            .text_size(theme::secondary_size())
            .font_family(theme::font())
            .child(crate::assets::icon(
                crate::assets::DRAG,
                14.,
                theme::accent(),
            ))
            .child(self.0.clone())
            .with_animation(
                "ghost-lift",
                gpui_kit::Animation::new(theme::fade()).with_easing(gpui_kit::ease_out_quint()),
                |d, t| d.opacity(0.6 + 0.32 * t),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sidebar-like list: rows 30 px high, 2 px apart, categories 10 px further down.
    ///
    /// general 0..30, Games 42..70, chess 72..102, Lounge 104..134, Music (closed) 146..174.
    fn list() -> Vec<RowBox> {
        let row = |id, shape, nested, top: f32| RowBox {
            id: Id(id),
            shape,
            nested,
            top,
            bottom: top + if shape == Shape::Channel { 30. } else { 28. },
        };
        vec![
            row(1, Shape::Channel, false, 0.),
            row(2, Shape::Category { open: true }, false, 42.),
            row(3, Shape::Channel, true, 72.),
            row(4, Shape::Channel, true, 104.),
            row(5, Shape::Category { open: false }, false, 146.),
        ]
    }

    fn at(id: u64, place: Place) -> Option<Slot> {
        Some(Slot::At(Id(id), place))
    }

    #[test]
    fn a_channel_lands_at_the_nearest_edge_or_into_a_category() {
        let rows = list();
        let slot = |y| slot_for(&rows, y, false);
        // First place, from anywhere above the first row.
        assert_eq!(slot(-6.), at(1, Place::Before));
        assert_eq!(slot(10.), at(1, Place::Before));
        assert_eq!(slot(20.), at(1, Place::After));
        // The margin above a category and its top: before it, at the top level.
        assert_eq!(slot(40.), at(2, Place::Before));
        assert_eq!(slot(48.), at(2, Place::Before));
        // Its lower part: into it. Never "after" a category.
        assert_eq!(slot(60.), at(2, Place::Into));
        assert_eq!(slot(70.), at(2, Place::Into));
        assert_eq!(slot(160.), at(5, Place::Into));
        assert_eq!(slot(174.), at(5, Place::Into));
        // Between channels of a category, and last in it.
        assert_eq!(slot(80.), at(3, Place::Before));
        assert_eq!(slot(103.), at(3, Place::After));
        assert_eq!(slot(130.), at(4, Place::After));
        // Out of it, before the next category.
        assert_eq!(slot(143.), at(5, Place::Before));
        // Under the last row: the end, even right under a category.
        assert_eq!(slot(177.), Some(Slot::End));
        assert_eq!(slot(400.), Some(Slot::End));
    }

    #[test]
    fn a_category_goes_before_top_level_rows_or_to_the_end() {
        let rows = list();
        let slot = |y| slot_for(&rows, y, true);
        assert_eq!(slot(-6.), at(1, Place::Before));
        assert_eq!(slot(25.), at(2, Place::Before));
        assert_eq!(slot(55.), at(2, Place::Before), "no 'into' for a category");
        // Over another category's channels: the nearest top-level edge.
        assert_eq!(slot(120.), at(5, Place::Before));
        assert_eq!(slot(170.), Some(Slot::End));
        assert_eq!(slot(400.), Some(Slot::End));
    }

    #[test]
    fn the_first_row_being_a_category_still_has_a_first_place() {
        let mut rows = list();
        rows.remove(0);
        assert_eq!(slot_for(&rows, 0., false), at(2, Place::Before));
        assert_eq!(slot_for(&rows, 30., false), at(2, Place::Before));
        assert_eq!(slot_for(&rows, 0., true), at(2, Place::Before));
    }

    #[test]
    fn gaps_and_unlisted_rows_still_land_somewhere() {
        let mut rows = list();
        // A voice channel with people under it (not listed): a 60 px gap after chess.
        rows[3].top += 60.;
        rows[3].bottom += 60.;
        for y in (-20..300).map(|y| y as f32) {
            assert!(slot_for(&rows, y, false).is_some(), "{y}");
            assert!(slot_for(&rows, y, true).is_some(), "{y}");
        }
        assert_eq!(slot_for(&[], 0., false), None);
    }
}
