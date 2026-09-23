//! `Sidebar`, `SidebarSection` and `NavItem` — a left column of clickable rows on the `chrome`
//! ground: labelled sections, a control at its foot, an edge the pointer can drag.
//!
//! Use a [`NavItem`] for a row of a sidebar — a scope to pick, a place to go, a live thing one
//! click away. Use a [`super::Row`] for a row of a content list, which has columns, a header and
//! a pane around it.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, ElementId, Empty, MouseButton, Pixels, SharedString, Window, div,
    prelude::*, relative,
};

use super::{list_view::ListPointer, tooltip::Tooltip, tooltip::WithTooltip};
use crate::{
    focus::FocusRing, harness::HarnessTargetExt, icons::IconSize, text::Text, theme::ActiveTheme,
    tone::Tone,
};

/// The hover group of one item, which reveals its hover action in place of its trailing slot.
const NAV_GROUP: &str = "nav-item";

/// `(new width, window, cx)`: what a drag on the sidebar's edge reports.
type ResizeHandler = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;

/// A left column: sections of [`NavItem`]s, a footer, and an optional draggable edge.
///
/// Expanded it is `width` wide (`metrics.sidebar_w` unless the caller passes the width the user
/// dragged it to, always within `sidebar_min_w`–`sidebar_max_w`); collapsed it is
/// `metrics.sidebar_collapsed_w` of icons, its section titles hidden and its items showing their
/// labels as tooltips. The caller owns the width and the collapsed flag.
#[derive(IntoElement)]
pub struct Sidebar {
    id: ElementId,
    width: Option<Pixels>,
    collapsed: bool,
    focused: bool,
    sections: Vec<SidebarSection>,
    footer: Option<AnyElement>,
    on_resize: Option<ResizeHandler>,
    handle_target: Option<SharedString>,
}

impl Sidebar {
    /// A sidebar identified by `id`, stable across frames.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            width: None,
            collapsed: false,
            focused: false,
            sections: Vec::new(),
            footer: None,
            on_resize: None,
            handle_target: None,
        }
    }

    /// The expanded width. Clamped to `sidebar_min_w`–`sidebar_max_w`; `metrics.sidebar_w`
    /// when unset.
    pub fn width(mut self, width: Option<Pixels>) -> Self {
        self.width = width;
        self
    }

    /// Draw the icon column instead of the full sidebar.
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    /// The sidebar owns the keyboard: draw the pane focus ring.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Append a section. Sections stack from the top; the footer stays at the bottom.
    pub fn section(mut self, section: SidebarSection) -> Self {
        self.sections.push(section);
        self
    }

    /// The control pinned to the bottom: normally the collapse [`super::IconButton`].
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }

    /// Make the trailing edge draggable. `handler` receives the new width, already clamped,
    /// on every pointer move of the drag. A collapsed sidebar has no draggable edge.
    pub fn on_resize(mut self, handler: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_resize = Some(Rc::new(handler));
        self
    }

    /// Record the draggable edge as a harness target under `name`.
    pub fn handle_target(mut self, name: impl Into<SharedString>) -> Self {
        self.handle_target = Some(name.into());
        self
    }
}

/// The value a drag of a sidebar's edge carries: which sidebar it resizes.
#[derive(Clone)]
struct DraggedEdge {
    sidebar: ElementId,
}

/// gpui draws a view under the pointer while something is dragged; an edge has nothing to
/// show, so this `Render` paints nothing and holds only the identity the move listener checks.
impl Render for DraggedEdge {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl RenderOnce for Sidebar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let metrics = &theme.metrics;
        let (min, max) = (metrics.sidebar_min_w, metrics.sidebar_max_w);
        let collapsed = self.collapsed;
        let width = if collapsed {
            metrics.sidebar_collapsed_w
        } else {
            self.width.unwrap_or(metrics.sidebar_w).clamp(min, max)
        };

        let body = div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .py(theme.space.md)
            .px(if collapsed {
                theme.space.xs
            } else {
                theme.space.sm
            })
            .gap(theme.space.lg)
            .children(
                self.sections
                    .into_iter()
                    .map(|section| section.collapsed(collapsed)),
            )
            .child(div().flex_1())
            .children(self.footer.map(|footer| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .when(collapsed, |el| el.justify_center())
                    .child(footer)
            }));

        let resize = self.on_resize.filter(|_| !collapsed);
        let handle = resize.is_some().then(|| {
            let edge = DraggedEdge {
                sidebar: self.id.clone(),
            };
            let handle = div()
                .id("sidebar-resize")
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(metrics.resize_handle_w)
                .cursor_col_resize()
                .on_drag(edge, |edge, _offset, _window, cx| cx.new(|_| edge.clone()));
            match self.handle_target {
                Some(name) => handle.harness_target(name).into_any_element(),
                None => handle.into_any_element(),
            }
        });

        let sidebar = self.id.clone();
        div()
            .id(self.id)
            .relative()
            .flex_none()
            .h_full()
            .w(width)
            .bg(theme.colors.chrome)
            .border_r(metrics.hairline)
            .border_color(theme.colors.border)
            .child(FocusRing::pane(self.focused).content(body))
            .when_some(resize, |el, on_resize| {
                el.on_drag_move::<DraggedEdge>(move |event, window, cx| {
                    if event.drag(cx).sidebar != sidebar {
                        return;
                    }
                    let width = (event.event.position.x - event.bounds.left()).clamp(min, max);
                    on_resize(width, window, cx);
                })
            })
            .children(handle)
    }
}

/// One labelled group of a [`Sidebar`]: a sentence-case title with an optional control at its
/// end (`Repositories  +`), over its items.
#[derive(IntoElement)]
pub struct SidebarSection {
    title: SharedString,
    action: Option<AnyElement>,
    header: Option<AnyElement>,
    body: Option<AnyElement>,
    body_height: Option<Pixels>,
    collapsed: bool,
}

impl SidebarSection {
    /// A section titled `title`, e.g. `Repositories`.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            action: None,
            header: None,
            body: None,
            body_height: None,
            collapsed: false,
        }
    }

    /// The control at the end of the title row: normally a compact [`super::IconButton`].
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// Replace the title row, for a live editor that takes its place (the filter bar).
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// The items: a column of [`NavItem`]s, or a list of them.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// The natural height of a body that cannot measure itself — a virtualized list's rows
    /// times their height. The section takes that much, and shrinks below it (its list then
    /// scrolls) when the sidebar is shorter than its sections.
    pub fn body_height(mut self, height: Pixels) -> Self {
        self.body_height = Some(height);
        self
    }

    /// Hide the title row: the sidebar is its icon column. Set by the [`Sidebar`].
    fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }
}

impl RenderOnce for SidebarSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let header = (!self.collapsed).then(|| {
            self.header.unwrap_or_else(|| {
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(theme.metrics.button_h_compact)
                    .pl(theme.space.sm)
                    .child(Text::sentence_label(self.title))
                    .children(self.action)
                    .into_any_element()
            })
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.space.xxs)
            .map(|el| match self.body_height {
                Some(_) => el.min_h_0(),
                None => el.flex_none(),
            })
            .children(header.map(|header| div().flex_none().child(header)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .when_some(self.body_height, |el, height| el.h(height).min_h_0())
                    .children(self.body),
            )
    }
}

/// One sidebar row: a leading icon or dot, a label, and trailing facts (a chip, a count).
///
/// It is `metrics.row_h` tall whatever it carries, so a sidebar list can be a uniform
/// [`super::ListView`]; a running job shows as a thin bar along the item's foot rather than a
/// second line. A hover action (the `⋯` menu trigger) takes the trailing slot's place while the
/// pointer is on the item. Collapsed, the item is its leading element and its label becomes a
/// tooltip.
#[derive(IntoElement)]
pub struct NavItem {
    id: ElementId,
    label: SharedString,
    tone: Option<Tone>,
    leading: Option<AnyElement>,
    trailing: Vec<AnyElement>,
    hover_action: Option<AnyElement>,
    progress: Option<u8>,
    selected: bool,
    cursor: bool,
    collapsed: bool,
    pointer: Option<(ListPointer, usize)>,
}

impl NavItem {
    /// An item identified by `id` reading `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            tone: None,
            leading: None,
            trailing: Vec::new(),
            hover_action: None,
            progress: None,
            selected: false,
            cursor: false,
            collapsed: false,
            pointer: None,
        }
    }

    /// The label's tone. Secondary by default, and the default tone while selected.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    /// The leading icon, dot or glyph. The one thing a collapsed item shows.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// Append a trailing element: a chip, then a count.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing.push(trailing.into_any_element());
        self
    }

    /// The control shown in the trailing slot's place while the item is hovered: the `⋯`
    /// menu trigger. Every action behind it must also have a key and a right-click route.
    pub fn hover_action(mut self, action: impl IntoElement) -> Self {
        self.hover_action = Some(action.into_any_element());
        self
    }

    /// A running job's percent, drawn as a thin bar along the item's foot.
    pub fn progress(mut self, percent: Option<u8>) -> Self {
        self.progress = percent;
        self
    }

    /// Paint the selection background: the item under the list's cursor.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Draw the cursor bar: the sidebar owns the keyboard.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// Show only the leading element, with the label as a tooltip.
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    /// Route presses on this item, as row `ix`, through `pointer`: a click selects, a
    /// double-click opens, a right-click selects and shows the menu (UX-SPEC §5.1).
    pub fn pointer(mut self, pointer: &ListPointer, ix: usize) -> Self {
        self.pointer = (!pointer.is_empty()).then(|| (pointer.clone(), ix));
        self
    }
}

impl RenderOnce for NavItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.selected;
        let tone = self.tone.unwrap_or(if selected {
            Tone::Default
        } else {
            Tone::Secondary
        });
        let hover_bg = theme.colors.row_hover;
        let has_hover_action = self.hover_action.is_some();
        let slot = IconSize::Medium.px();

        let content = if self.collapsed {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .children(self.leading)
        } else {
            div()
                .flex()
                .items_center()
                .size_full()
                .gap(theme.space.sm)
                .px(theme.space.sm)
                .children(self.leading.map(|leading| {
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(slot)
                        .child(leading)
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(Text::ui(self.label.clone()).tone(tone).ellipsize()),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(theme.space.xs)
                        .when(has_hover_action, |el| {
                            el.group_hover(NAV_GROUP, |style| style.invisible())
                        })
                        .children(self.trailing),
                )
        };
        // Hidden, not removed: the trigger keeps its hit box off until the pointer is on the
        // item, and never pushes the label aside when it appears.
        let hover_action = self.hover_action.filter(|_| !self.collapsed).map(|action| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(theme.space.xxs)
                .flex()
                .items_center()
                .invisible()
                .group_hover(NAV_GROUP, |style| style.visible())
                .child(action)
        });
        let progress = self.progress.filter(|_| !self.collapsed).map(|percent| {
            div()
                .absolute()
                .bottom_0()
                .left(theme.space.sm)
                .right(theme.space.sm)
                .h(theme.metrics.progress_bar_h)
                .rounded(theme.radii.full)
                .bg(theme.colors.control)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(f32::from(percent.min(100)) / 100.0))
                        .bg(Tone::Info.color(theme)),
                )
        });
        let tooltip = self.collapsed.then(|| Tooltip::new(self.label.clone()));

        div()
            .id(self.id)
            .relative()
            .w_full()
            .h(theme.metrics.row_h)
            .rounded(theme.radii.control)
            .overflow_hidden()
            .group(NAV_GROUP)
            .when(selected, |el| el.bg(theme.colors.row_selected))
            // Hover is pointer feedback only: it never paints over the selection.
            .when(!selected, |el| el.hover(move |style| style.bg(hover_bg)))
            .when_some(self.pointer, |el, (pointer, ix)| {
                let secondary = pointer.clone();
                el.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        pointer.press(ix, event, window, cx)
                    })
                    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        secondary.press(ix, event, window, cx)
                    })
            })
            .when_some(tooltip, |el, tooltip| el.with_tooltip(tooltip, cx))
            .child(FocusRing::cursor_row(self.cursor).content(content))
            .children(hover_action)
            .children(progress)
    }
}
