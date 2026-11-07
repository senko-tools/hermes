//! The building blocks of the settings pages (user settings and space settings), so every page
//! lines up the same way: a heading, titled groups, and rows of a name with a hint on the left and
//! the control in a fixed column on the right.

use std::borrow::Borrow;

use iced::widget::{PickList, button, column, container, pick_list, row, rule, toggler};
use iced::{Alignment, Element, Fill};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{caption, heading, icon, label, section_title};

/// Widest a page's content gets.
pub const MAX_WIDTH: f32 = 880.0;

/// A page: its heading, an optional line under it, then its groups.
pub fn page<'a, M: 'a>(
    title: &str,
    intro: Option<&str>,
    groups: Vec<Element<'a, M>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    let mut top = column![heading(title.to_owned(), fonts)].spacing(style::GAP_XS);
    if let Some(intro) = intro {
        top = top.push(caption(intro.to_owned(), fonts).color(theme.text.tertiary.0));
    }
    column![top]
        .extend(groups)
        .spacing(style::GAP_L)
        .max_width(MAX_WIDTH)
        .into()
}

/// A titled group of rows.
pub fn group<'a, M: 'a>(
    title: &str,
    rows: Vec<Element<'a, M>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    group_with(title, Vec::new(), rows, fonts, theme)
}

/// A titled group with buttons on the right of its title.
pub fn group_with<'a, M: 'a>(
    title: &str,
    actions: Vec<Element<'a, M>>,
    rows: Vec<Element<'a, M>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    let mut title = row![
        section_title(title, fonts)
            .color(theme.text.tertiary.0)
            .width(Fill)
    ]
    .spacing(style::GAP_S)
    .align_y(Alignment::Center);
    if !actions.is_empty() {
        title = title.extend(actions).height(style::CONTROL_H);
    }
    column![container(title).padding(iced::Padding::ZERO.top(style::GAP))]
        .extend(rows)
        .spacing(style::GAP)
        .into()
}

/// A setting: a hairline, then its name and hint on the left and `control` in the control
/// column, left-aligned there so every control on every page starts at the same place.
pub fn setting<'a, M: 'a>(
    name: &str,
    hint: &str,
    control: impl Into<Element<'a, M>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    let mut text = column![label(name.to_owned(), fonts).font(fonts.medium())].spacing(2);
    if !hint.is_empty() {
        text = text.push(caption(hint.to_owned(), fonts).color(theme.text.tertiary.0));
    }
    column![
        rule::horizontal(1).style(move |_| style::rule(theme)),
        row![
            text.width(Fill),
            container(control).width(style::CONTROL_COL)
        ]
        .spacing(style::GAP_L)
        .align_y(Alignment::Center),
    ]
    .spacing(style::GAP)
    .into()
}

/// An on/off setting: the switch sits at the right edge of the control column, so all of them
/// form one column.
pub fn toggle<'a, M: Clone + 'a>(
    name: &str,
    hint: &str,
    on: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    toggle_maybe(name, hint, on, Some(on_toggle), fonts, theme)
}

/// A [`toggle`] that is disabled without `on_toggle`.
pub fn toggle_maybe<'a, M: Clone + 'a, F: Fn(bool) -> M + 'a>(
    name: &str,
    hint: &str,
    on: bool,
    on_toggle: Option<F>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    setting(
        name,
        hint,
        container(
            toggler(on)
                .size(style::TOGGLER)
                .style(move |_, st| style::toggler(theme, st))
                .on_toggle_maybe(on_toggle),
        )
        .width(Fill)
        .align_right(Fill),
        fonts,
        theme,
    )
}

/// A drop-down filling the control column.
pub fn dropdown<'a, T, L, V, M>(
    options: L,
    selected: Option<V>,
    on_select: impl Fn(T) -> M + 'a,
    fonts: &Fonts,
    theme: &'a Theme,
) -> PickList<'a, T, L, V, M>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    M: Clone + 'a,
{
    pick_list(options, selected, on_select)
        .padding(style::INPUT_PAD)
        .style(move |_, st| style::pick_list(theme, st))
        .menu_style(move |_| style::menu(theme))
        .font(fonts.ui)
        .text_size(fonts.size)
        .width(Fill)
}

/// An entry of a settings page's navigation.
pub fn nav_item<'a, M: Clone + 'a>(
    glyph: crate::widget::icon::Glyph,
    name: &str,
    selected: bool,
    on_press: M,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    button(
        container(
            row![
                icon::icon(glyph, style::ROW_ICON).color(if selected {
                    theme.accent.base.0
                } else {
                    theme.icons.default.0
                }),
                label(name.to_owned(), fonts)
                    .color(if selected {
                        theme.text.primary.0
                    } else {
                        theme.text.secondary.0
                    })
                    .wrapping(iced::widget::text::Wrapping::None),
            ]
            .spacing(style::GAP)
            .align_y(Alignment::Center),
        )
        .center_y(Fill),
    )
    .height(style::ROW_H)
    .padding([0.0, style::GAP])
    .width(Fill)
    .style(move |_, s| style::sidebar_item(theme, s, selected))
    .on_press(on_press)
    .into()
}

/// The title over a page's navigation.
pub fn nav_title<'a, M: 'a>(title: &str, fonts: &Fonts, theme: &'a Theme) -> Element<'a, M> {
    container(section_title(title, fonts).color(theme.text.tertiary.0))
        .padding([style::GAP_XS, style::GAP])
        .into()
}
