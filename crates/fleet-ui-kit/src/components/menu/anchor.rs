//! The two ways a [`Menu`] is opened in place: [`PopoverMenu`] under a trigger and
//! [`ContextMenu`] at the pointer.
//!
//! Both are `RenderOnce` wrappers whose open menu lives in gpui element state keyed by the
//! wrapper's id (the shape of Zed's `PopoverMenu` and `right_click_menu`), so a screen does not
//! have to own a field per menu: the menu exists only while it is open, and it goes away with
//! the wrapper if the wrapper stops being rendered. The menu paints through `anchored()` inside
//! `deferred()`, at [`OverlayLayer::Menu`], so it floats above its siblings and is kept inside
//! the window.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AnyElement, App, Context, DismissEvent, ElementId, Entity, MouseButton, MouseDownEvent, Pixels,
    Point, Subscription, Window, anchored, deferred, div, prelude::*,
};

use super::{Menu, MenuBuilder};
use crate::{components::OverlayLayer, theme::ActiveTheme};

/// Which corner of the trigger a [`PopoverMenu`] hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MenuAnchor {
    /// The menu's left edge under the trigger's left edge. The default.
    #[default]
    BottomLeft,
    /// The menu's right edge under the trigger's right edge: a trigger at the right of a row or
    /// a header.
    BottomRight,
}

/// The one menu a wrapper has open, and where.
struct OpenMenu {
    menu: Entity<Menu>,
    /// Window position for a [`ContextMenu`]; unused under a trigger.
    position: Point<Pixels>,
    /// Dropped with the menu: clears this slot when the menu dismisses itself.
    _dismissed: Subscription,
}

#[derive(Default)]
struct SlotState {
    open: Option<OpenMenu>,
    /// Set when a left press lands on the trigger of an open popover: that press has already
    /// closed the menu (as a click outside it), so the click it completes must not reopen it.
    swallow_click: bool,
}

type Slot = Rc<RefCell<SlotState>>;

/// The slot for the wrapper with `id`, carried across frames in gpui element state.
fn slot(id: &ElementId, window: &mut Window) -> Slot {
    window.with_global_id(id.clone(), |global_id, window| {
        window.with_element_state::<Slot, _>(global_id, |slot, _| {
            let slot = slot.unwrap_or_default();
            (slot.clone(), slot)
        })
    })
}

/// Build the menu, focus it, and park it in `slot`. An empty menu (every item left out) does
/// not open.
fn show(
    slot: &Slot,
    builder: &MenuBuilder,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let menu = Menu::build(window, cx, |menu, window, cx| builder(menu, window, cx));
    if menu.read(cx).is_empty() {
        return;
    }
    let weak_slot = Rc::downgrade(slot);
    let menu_id = menu.entity_id();
    let dismissed = window.subscribe(&menu, cx, move |_, _: &DismissEvent, window, _| {
        if let Some(slot) = weak_slot.upgrade() {
            let mut slot = slot.borrow_mut();
            // A newer menu may already sit in the slot (a second right-click); leave it.
            if slot
                .open
                .as_ref()
                .is_some_and(|open| open.menu.entity_id() == menu_id)
            {
                slot.open = None;
            }
        }
        window.refresh();
    });
    let focus = menu.read(cx).focus.clone();
    window.focus(&focus, cx);
    // The menu enters the dispatch tree only when the next frame paints it, so for that one
    // frame an ancestor's `contains_focused` is false and a surface that reconciles its focus
    // (the app shell does) may take it back. Claim it again once the menu is on screen.
    let weak_menu = menu.downgrade();
    window.on_next_frame(move |window, cx| {
        if weak_menu.upgrade().is_some() && !focus.contains_focused(window, cx) {
            window.focus(&focus, cx);
        }
    });
    slot.borrow_mut().open = Some(OpenMenu {
        menu,
        position,
        _dismissed: dismissed,
    });
    window.refresh();
}

/// The menu floating layer: `anchored()` inside `deferred()`, snapped inside the window.
fn floating(
    menu: Entity<Menu>,
    corner: gpui::Anchor,
    position: Option<Point<Pixels>>,
    offset: Pixels,
    cx: &App,
) -> AnyElement {
    let margin = cx.theme().space.sm;
    let layer = anchored().anchor(corner).snap_to_window_with_margin(margin);
    let layer = match position {
        Some(position) => layer.position(position),
        None => layer,
    };
    deferred(layer.child(div().mt(offset).child(menu)))
        .with_priority(OverlayLayer::Menu.priority())
        .into_any_element()
}

type TriggerBuilder = Box<dyn FnOnce(bool, &mut Window, &mut App) -> AnyElement>;

/// A trigger that opens a [`Menu`] below itself on click.
///
/// The trigger is normally a [`super::super::Button`] or [`super::super::IconButton`] with **no**
/// action of its own: the popover owns the click. Clicking the trigger again, clicking
/// anywhere outside the menu, `esc`, or activating an item closes it.
#[derive(IntoElement)]
pub struct PopoverMenu {
    id: ElementId,
    trigger: Option<TriggerBuilder>,
    menu: Option<MenuBuilder>,
    anchor: MenuAnchor,
    full_width: bool,
}

impl PopoverMenu {
    /// A popover identified by `id`, which must be stable across frames: the open menu is kept
    /// under it.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            trigger: None,
            menu: None,
            anchor: MenuAnchor::default(),
            full_width: false,
        }
    }

    /// The element that opens the menu.
    pub fn trigger(mut self, trigger: impl IntoElement + 'static) -> Self {
        self.trigger = Some(Box::new(move |_, _, _| trigger.into_any_element()));
        self
    }

    /// The trigger, built knowing whether the menu is open: an `IconButton` passes it to
    /// `.selected(open)` so an open menu's trigger stays pressed.
    pub fn trigger_with<E: IntoElement>(
        mut self,
        trigger: impl FnOnce(bool, &mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.trigger = Some(Box::new(move |open, window, cx| {
            trigger(open, window, cx).into_any_element()
        }));
        self
    }

    /// Build the menu's entries. Runs each time the menu opens.
    pub fn menu(
        mut self,
        builder: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.menu = Some(Rc::new(builder));
        self
    }

    /// Stretch the trigger across the container, for a full-width field.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// [`Self::menu`] with a builder already shared, for the kit's own wrappers.
    pub(super) fn menu_builder(mut self, builder: MenuBuilder) -> Self {
        self.menu = Some(builder);
        self
    }

    /// Hang the menu from another corner of the trigger.
    pub fn anchor(mut self, anchor: MenuAnchor) -> Self {
        self.anchor = anchor;
        self
    }
}

impl RenderOnce for PopoverMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let slot = slot(&self.id, window);
        let open = slot.borrow().open.as_ref().map(|open| open.menu.clone());
        let trigger = self
            .trigger
            .map(|trigger| trigger(open.is_some(), window, cx));
        let offset = cx.theme().space.xs;
        let right = self.anchor == MenuAnchor::BottomRight;
        let corner = if right {
            gpui::Anchor::TopRight
        } else {
            gpui::Anchor::TopLeft
        };
        let menu = open.map(|menu| {
            // A zero-size box pinned to the trigger's bottom corner: `anchored()` with no
            // explicit position hangs the menu from where layout put it.
            div()
                .absolute()
                .top_full()
                .map(|el| if right { el.right_0() } else { el.left_0() })
                .child(floating(menu, corner, None, offset, cx))
        });
        let builder = self.menu;
        div()
            .id(self.id)
            .relative()
            .map(|el| {
                if self.full_width {
                    el.w_full()
                } else {
                    el.flex_none()
                }
            })
            .aria_expanded(menu.is_some())
            .capture_any_mouse_down({
                let slot = slot.clone();
                move |event: &MouseDownEvent, _, _| {
                    let mut slot = slot.borrow_mut();
                    slot.swallow_click = event.button == MouseButton::Left && slot.open.is_some();
                }
            })
            .on_click(move |_, window, cx| {
                if std::mem::take(&mut slot.borrow_mut().swallow_click) {
                    return;
                }
                if let Some(builder) = &builder {
                    show(&slot, builder, Point::default(), window, cx);
                }
            })
            .children(trigger)
            .children(menu)
    }
}

/// A region that opens a [`Menu`] at the pointer on a right-click.
///
/// Wrap the row, card or pane that the menu is about. The same verbs belong in a visible
/// control too (a ⋯ [`PopoverMenu`] on the row, the detail panel): a right-click menu is never
/// the only way to an action (ADR 0023).
#[derive(IntoElement)]
pub struct ContextMenu {
    id: ElementId,
    child: Option<AnyElement>,
    menu: Option<MenuBuilder>,
}

impl ContextMenu {
    /// A right-click region identified by `id`, stable across frames. A list keys it by row:
    /// `("worktree-menu", ix)`.
    pub fn new(id: impl Into<ElementId>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            child: Some(child.into_any_element()),
            menu: None,
        }
    }

    /// Build the menu's entries. Runs each time the menu opens.
    pub fn menu(
        mut self,
        builder: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.menu = Some(Rc::new(builder));
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let slot = slot(&self.id, window);
        let menu = slot.borrow().open.as_ref().map(|open| {
            floating(
                open.menu.clone(),
                gpui::Anchor::TopLeft,
                Some(open.position),
                Pixels::ZERO,
                cx,
            )
        });
        let builder = self.menu;
        div()
            .id(self.id)
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                let Some(builder) = &builder else {
                    return;
                };
                cx.stop_propagation();
                window.prevent_default();
                show(&slot, builder, event.position, window, cx);
            })
            .children(self.child)
            .children(menu)
    }
}
