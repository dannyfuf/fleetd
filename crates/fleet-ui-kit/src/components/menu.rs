//! `Menu` — a floating list of verbs: the ⋯ row menu, the right-click menu, the `+` new-tab
//! menu, the context switcher and the option list of a [`Dropdown`].
//!
//! A menu is opened by one of three wrappers and never placed by hand: [`PopoverMenu`] under a
//! trigger (normally a [`super::Button`] or [`super::IconButton`]), [`ContextMenu`] at the
//! pointer on a right-click, and [`Dropdown`], a field-looking trigger that opens a list of
//! options with a check on the chosen one. Each wrapper builds the menu fresh every time it
//! opens, so the items always reflect the current state.
//!
//! **Items are actions.** A [`MenuItem`] normally carries a gpui `Action`: activating it
//! dispatches that action to the element that had focus before the menu opened, exactly as its
//! key would, and the item shows that key as a [`Kbd`] chip resolved from the live keymap in the
//! same context. An item whose action nothing on that element's dispatch path handles is **left
//! out** rather than greyed (DESIGN-SYSTEM §4). [`MenuItem::on_select`] is for the rare entry
//! that is not an action, such as a dropdown option.
//!
//! **Keyboard.** An open menu owns the focus. `↑`/`↓` (or `ctrl-p`/`ctrl-n`) move the highlight,
//! `⏎` activates, `esc` closes; the keys are the [`menu_actions`] bound against
//! [`MENU_KEY_CONTEXT`], which `fleet-app`'s key table binds and [`menu_key_bindings`] binds for
//! galleries and tests. There is no `j`/`k`: a menu may grow a filter field (§4). On close, the
//! focus goes back to where it was.
//!
//! **Pointer.** Hover moves the highlight, a click activates, a click anywhere outside closes.
//!
//! This is the fourth entity in the kit, and it earns it (§6): a menu owns a `FocusHandle` so it
//! can take the keyboard while it is open, and a highlight that must survive frames.

mod anchor;
mod dropdown;
#[cfg(test)]
mod tests;

use std::rc::Rc;

use gpui::{
    Action, App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, Global, KeyBinding,
    MouseMoveEvent, Role, SharedString, Toggled, WeakFocusHandle, Window, div, prelude::*,
};

pub use anchor::{ContextMenu, MenuAnchor, PopoverMenu};
pub use dropdown::Dropdown;

use super::kbd::{Kbd, KbdSize};
use crate::{
    harness::HarnessTargetExt as _,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
};

/// The key context an open [`Menu`] publishes. `fleet-app`'s key table binds [`menu_actions`]
/// against it; the word is unique across the app's contexts.
pub const MENU_KEY_CONTEXT: &str = "FleetMenu";

/// The harness target part every activatable item paints under: `menu.item[N]`, numbered over
/// the visible items in order, skipping separators and headers.
pub const MENU_ITEM_TARGET: &str = "menu.item";

/// The four verbs an open menu understands.
pub mod menu_actions {
    gpui::actions!(
        fleet_menu,
        [
            /// `↓` / `ctrl-n`: highlight the next item.
            SelectNext,
            /// `↑` / `ctrl-p`: highlight the previous item.
            SelectPrevious,
            /// `⏎`: activate the highlighted item.
            Confirm,
            /// `esc`: close the menu and give the focus back.
            Cancel,
        ]
    );
}

use menu_actions::{Cancel, Confirm, SelectNext, SelectPrevious};

/// The menu keys as gpui bindings against [`MENU_KEY_CONTEXT`], for a gallery or a test bench.
/// `fleet-app` binds the same keys through its own key table, which is what Help reads.
pub fn menu_key_bindings() -> Vec<KeyBinding> {
    let context = Some(MENU_KEY_CONTEXT);
    vec![
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("ctrl-n", SelectNext, context),
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("ctrl-p", SelectPrevious, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new("escape", Cancel, context),
    ]
}

/// The focus handles of every live menu, weakly: a menu's handle dies with the menu, and the
/// next build prunes it.
#[derive(Default)]
struct OpenMenus(Vec<WeakFocusHandle>);

impl Global for OpenMenus {}

/// Whether an open [`Menu`] holds the focus in `window`.
///
/// A menu is not part of any application focus model: it takes the focus when it opens and
/// hands it back when it closes. A shell that reconciles the focus from its own state asks this
/// first and leaves the focus alone while it is true, so a background update cannot pull the
/// keyboard out of an open menu.
pub fn menu_holds_focus(window: &Window, cx: &App) -> bool {
    cx.try_global::<OpenMenus>().is_some_and(|open| {
        open.0
            .iter()
            .filter_map(WeakFocusHandle::upgrade)
            .any(|focus| focus.is_focused(window))
    })
}

type SelectHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Builds a menu's entries each time it opens. Shared by the wrappers, which rebuild on open.
pub(crate) type MenuBuilder = Rc<dyn Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu>;

/// One activatable row of a [`Menu`]: `[icon] label [detail] [✓] [Kbd]`.
pub struct MenuItem {
    label: SharedString,
    detail: Option<SharedString>,
    icon: Option<Icon>,
    action: Option<Box<dyn Action>>,
    handler: Option<SelectHandler>,
    kbd: Option<Kbd>,
    destructive: bool,
    checked: Option<bool>,
}

impl MenuItem {
    /// An item reading `label`. Give it an [`Self::action`] or an [`Self::on_select`].
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            icon: None,
            action: None,
            handler: None,
            kbd: None,
            destructive: false,
            checked: None,
        }
    }

    /// Lead the label with a glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A muted word after the label that says what state the choice is in, such as a
    /// worktree switcher's `sleeping`. It never replaces the label, which is what the item is
    /// named by.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Dispatch `action` to the element focused before the menu opened, and show its live key.
    /// The item is left out when nothing on that element's path handles the action.
    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.action = Some(action);
        self
    }

    /// Run `handler` when the item is activated, after the menu has closed and the focus is
    /// back. With [`Self::action`] as well, the handler runs first.
    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.handler = Some(Rc::new(handler));
        self
    }

    /// Show this key instead of the one [`Self::action`] resolves: for an item whose key is
    /// not an action binding. Never a hand-typed string for a key the keymap owns.
    pub fn kbd(mut self, kbd: Kbd) -> Self {
        self.kbd = Some(kbd);
        self
    }

    /// Draw the icon and label in `danger`: an item that deletes or stops something.
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Make this a choice and say whether it is the chosen one: a checked item shows a `✓`.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// The label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    fn is_available(&self, origin: Option<&FocusHandle>, window: &Window, cx: &App) -> bool {
        let Some(action) = self.action.as_deref() else {
            return true;
        };
        match origin {
            Some(origin) => window.is_action_available_in(action, origin),
            None => window.is_action_available(action, cx),
        }
    }
}

/// One entry of a [`Menu`].
enum MenuEntry {
    Item(MenuItem),
    Separator,
    Header(SharedString),
}

/// A floating list of [`MenuItem`]s, with separators and group headers. See the module doc.
///
/// Build one with [`Menu::build`] from a wrapper's builder; the wrappers own its lifetime.
pub struct Menu {
    focus: FocusHandle,
    /// The element focused when the menu opened: actions dispatch to it, chips resolve in its
    /// context, and the focus returns to it on close.
    origin: Option<FocusHandle>,
    entries: Vec<MenuEntry>,
    /// Index into `entries` of the highlighted item; always an [`MenuEntry::Item`].
    selected: Option<usize>,
    dismissed: bool,
}

impl EventEmitter<DismissEvent> for Menu {}

impl Focusable for Menu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Menu {
    /// Build a menu with `f`, remembering the focused element as the one it acts on. Items
    /// whose action that element cannot handle are dropped, separators and headers left with
    /// nothing to separate or head are dropped with them, and the checked item (else the first) is
    /// highlighted.
    ///
    /// Call it from an event — a click, a key — as the wrappers do. Checking an item's action
    /// needs the dispatch tree of a painted frame, and a window that has not painted yet has
    /// none: gpui asserts on it.
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Menu, &mut Window, &mut Context<Menu>) -> Menu,
    ) -> gpui::Entity<Menu> {
        let origin = window.focused(cx);
        let menu = cx.new(|cx| {
            let menu = Menu {
                focus: cx.focus_handle(),
                origin,
                entries: Vec::new(),
                selected: None,
                dismissed: false,
            };
            let mut menu = f(menu, window, cx);
            let origin = menu.origin.clone();
            let entries = std::mem::take(&mut menu.entries)
                .into_iter()
                .filter(|entry| match entry {
                    MenuEntry::Item(item) => item.is_available(origin.as_ref(), window, cx),
                    MenuEntry::Separator | MenuEntry::Header(_) => true,
                })
                .collect();
            menu.entries = tidy(entries);
            // A list of choices opens on the chosen one; a list of verbs on its first.
            let chosen = menu
                .items()
                .find(|(_, item)| item.checked == Some(true))
                .map(|(ix, _)| ix);
            menu.selected = chosen.or_else(|| menu.first_item());
            menu
        });
        let focus = menu.read(cx).focus.downgrade();
        let open = &mut cx.default_global::<OpenMenus>().0;
        open.retain(|menu| menu.upgrade().is_some());
        open.push(focus);
        menu
    }

    /// Append an item.
    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(MenuEntry::Item(item));
        self
    }

    /// Append a hairline between two groups of items.
    pub fn separator(mut self) -> Self {
        self.entries.push(MenuEntry::Separator);
        self
    }

    /// Append a group heading, in sentence case.
    pub fn header(mut self, label: impl Into<SharedString>) -> Self {
        self.entries.push(MenuEntry::Header(label.into()));
        self
    }

    /// Whether the menu has no item left to show; a wrapper does not open an empty menu.
    pub fn is_empty(&self) -> bool {
        self.first_item().is_none()
    }

    /// The labels of the visible items in order, the order `menu.item[N]` counts in.
    pub fn item_labels(&self) -> Vec<SharedString> {
        self.items().map(|(_, item)| item.label.clone()).collect()
    }

    /// The position among [`Self::item_labels`] of the highlighted item.
    pub fn highlighted(&self) -> Option<usize> {
        let selected = self.selected?;
        self.items().position(|(ix, _)| ix == selected)
    }

    fn items(&self) -> impl Iterator<Item = (usize, &MenuItem)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(ix, entry)| match entry {
                MenuEntry::Item(item) => Some((ix, item)),
                MenuEntry::Separator | MenuEntry::Header(_) => None,
            })
    }

    fn first_item(&self) -> Option<usize> {
        self.items().next().map(|(ix, _)| ix)
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let next = match self.selected {
            Some(current) => self.items().map(|(ix, _)| ix).find(|ix| *ix > current),
            None => self.first_item(),
        };
        // The highlight stops at the last item, as a Fleet list cursor does.
        if let Some(next) = next {
            self.highlight(next, cx);
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let previous = match self.selected {
            Some(current) => self
                .items()
                .map(|(ix, _)| ix)
                .take_while(|ix| *ix < current)
                .last(),
            None => self.first_item(),
        };
        if let Some(previous) = previous {
            self.highlight(previous, cx);
        }
    }

    fn highlight(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.selected != Some(ix) {
            self.selected = Some(ix);
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.activate(ix, window, cx);
        }
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(window, cx);
    }

    /// Close the menu: give the focus back to where it was, then tell the wrapper, which drops
    /// the menu on the next frame. Idempotent.
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismissed {
            return;
        }
        self.dismissed = true;
        if self.focus.contains_focused(window, cx) || self.focus.is_focused(window) {
            match &self.origin {
                Some(origin) => window.focus(origin, cx),
                None => window.blur(),
            }
        }
        cx.emit(DismissEvent);
    }

    /// Close the menu, then run the item: its handler, then its action, dispatched to the
    /// element the focus has just gone back to — the same dispatch its key makes.
    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(MenuEntry::Item(item)) = self.entries.get(ix) else {
            return;
        };
        let handler = item.handler.clone();
        let action = item.action.as_ref().map(|action| action.boxed_clone());
        self.dismiss(window, cx);
        if let Some(handler) = handler {
            handler(window, cx);
        }
        if let Some(action) = action {
            window.dispatch_action(action, cx);
        }
    }

    fn resolve_kbd(&self, item: &MenuItem, window: &Window, cx: &App) -> Option<Kbd> {
        item.kbd.clone().or_else(|| {
            let action = item.action.as_deref()?;
            match &self.origin {
                Some(origin) => Kbd::for_action_in(action, origin, window),
                None => Kbd::for_action(action, window, cx),
            }
        })
    }
}

/// Drop separators and headers that no longer separate or head anything: a header with no
/// item under it before the next break, a separator at either end, and a run of separators down
/// to one. Leaving items out is what makes this necessary.
fn tidy(entries: Vec<MenuEntry>) -> Vec<MenuEntry> {
    // `heads_items[i]`: an item follows entry `i` before the next separator or header.
    let mut heads_items = vec![false; entries.len()];
    let mut item_ahead = false;
    for (ix, entry) in entries.iter().enumerate().rev() {
        heads_items[ix] = item_ahead;
        item_ahead = match entry {
            MenuEntry::Item(_) => true,
            MenuEntry::Separator | MenuEntry::Header(_) => false,
        };
    }
    let mut kept = Vec::with_capacity(entries.len());
    let mut separator_due = false;
    for (ix, entry) in entries.into_iter().enumerate() {
        let keep = match &entry {
            MenuEntry::Item(_) => true,
            MenuEntry::Header(_) => heads_items[ix],
            MenuEntry::Separator => {
                separator_due = true;
                false
            }
        };
        if keep {
            if std::mem::take(&mut separator_due) && !kept.is_empty() {
                kept.push(MenuEntry::Separator);
            }
            kept.push(entry);
        }
    }
    kept
}

impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let colors = &theme.colors;
        let mut visible = 0usize;
        let mut children = Vec::with_capacity(self.entries.len());
        for (ix, entry) in self.entries.iter().enumerate() {
            let child = match entry {
                MenuEntry::Separator => div()
                    .h(theme.metrics.hairline)
                    .mx(theme.space.sm)
                    .my(theme.space.xs)
                    .bg(colors.border)
                    .into_any_element(),
                MenuEntry::Header(label) => div()
                    .flex()
                    .items_end()
                    .h(theme.metrics.section_header_h)
                    .px(theme.space.sm)
                    .child(Text::sentence_label(label.clone()).muted())
                    .into_any_element(),
                MenuEntry::Item(item) => {
                    let n = visible;
                    visible += 1;
                    let selected = self.selected == Some(ix);
                    let fg = if item.destructive {
                        colors.danger
                    } else if selected {
                        colors.text
                    } else {
                        colors.text_secondary
                    };
                    let kbd = self
                        .resolve_kbd(item, window, cx)
                        .map(|kbd| kbd.size(KbdSize::Small));
                    let check = item
                        .checked
                        .filter(|checked| *checked)
                        .map(|_| Icon::Check.el().size(IconSize::Medium).color(fg));
                    div()
                        .id(("menu-item", ix))
                        .flex()
                        .items_center()
                        .gap(theme.space.sm)
                        .h(theme.metrics.row_h)
                        .px(theme.space.sm)
                        .rounded(theme.radii.control)
                        .cursor_pointer()
                        .when(selected, |el| el.bg(colors.control_hover))
                        .role(if item.checked.is_some() {
                            Role::MenuItemRadio
                        } else {
                            Role::MenuItem
                        })
                        .aria_label(item.label.clone())
                        .aria_selected(selected)
                        .when_some(item.checked, |el, checked| {
                            el.aria_toggled(if checked {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                        })
                        .when_some(kbd.as_ref().and_then(Kbd::aria_shortcut), |el, keys| {
                            el.aria_keyshortcuts(keys)
                        })
                        .on_mouse_move(cx.listener(move |menu, _: &MouseMoveEvent, _, cx| {
                            menu.highlight(ix, cx);
                        }))
                        .on_click(cx.listener(move |menu, _, window, cx| {
                            menu.activate(ix, window, cx);
                        }))
                        .children(
                            item.icon
                                .map(|icon| icon.el().size(IconSize::Medium).color(fg)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Text::ui(item.label.clone()).color(fg).ellipsize()),
                        )
                        .children(
                            item.detail
                                .clone()
                                .map(|detail| Text::ui(detail).muted().flex_none()),
                        )
                        .children(check)
                        .children(kbd)
                        .harness_target_indexed(MENU_ITEM_TARGET, n)
                        .into_any_element()
                }
            };
            children.push(child);
        }
        div()
            .id("fleet-menu")
            .debug_selector(|| "fleet-menu".to_owned())
            .key_context(MENU_KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down_out(cx.listener(|menu, _, window, cx| menu.dismiss(window, cx)))
            .occlude()
            .role(Role::Menu)
            .flex()
            .flex_col()
            .min_w(theme.metrics.menu_min_w)
            .p(theme.space.xs)
            .rounded(theme.radii.popover)
            .bg(colors.elevated)
            .border(theme.metrics.hairline)
            .border_color(colors.border_strong)
            .shadow(theme.popover_shadow())
            .children(children)
    }
}
