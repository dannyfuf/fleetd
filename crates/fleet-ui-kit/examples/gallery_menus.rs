//! The visual and behavioural test bench for the kit's **menus**.
//!
//! `Menu` · `MenuItem` · `PopoverMenu` · `ContextMenu` · `Dropdown`.
//!
//! Every item here is wired the way a screen wires one: `.action(..)`, with the chip resolved
//! from the live keymap this example binds, dispatched to the page when chosen. The status bar
//! names the last action that ran, so choosing an item and pressing its key read the same. The
//! "Archive" item's action is bound to a key but handled by nothing on the page, so every menu
//! leaves it out.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_menus
//! ```
//!
//! | Key | What |
//! | --- | --- |
//! | `ctrl-t` | toggle light / dark |
//! | `↓` / `ctrl-n` · `↑` / `ctrl-p` · `⏎` · `esc` | inside an open menu |
//! | `o` · `r` · `ctrl-s c` · `ctrl-s a` · `shift-d` · `b` | the demo actions |
//! | `ctrl-q` / `cmd-q` | quit |

pub mod support;
const LAYOUT: support::layout::GalleryLayout = support::layout::GalleryLayout {
    label_width: 170.0,
    column: false,
    divided: false,
    compact: true,
};
use std::{cell::Cell, rc::Rc};

use fleet_ui_kit::prelude::*;
use gpui::{Action, Entity, FocusHandle, Focusable, KeyBinding, actions};
use support::layout::strip;

actions!(
    gallery_menus,
    [
        ToggleTheme,
        Quit,
        Open,
        Rename,
        NewTerminal,
        NewAgent,
        Delete,
        Board,
        Archive,
    ]
);

const CONTEXT: &str = "GalleryMenus";

const ACCESS: [&str; 3] = ["Read only", "Ask each time", "Full access"];

struct MenusGallery {
    focus_handle: FocusHandle,
    /// The last action that ran, by key or by menu.
    last: SharedString,
    /// The dropdown's chosen option, shared with the option handlers.
    access: Rc<Cell<usize>>,
    /// An always-open menu, drawn in place to show every item state at once.
    specimen: Entity<Menu>,
}

impl MenusGallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Built before the first frame, when there is no dispatch tree to check actions
        // against, so its items carry explicit chips and handlers rather than actions.
        let chip = |keys: &str| Kbd::parse(keys).unwrap_or_else(|error| panic!("{error}"));
        let specimen = Menu::build(window, cx, |menu, _, _| {
            menu.header("Worktree")
                .item(
                    MenuItem::new("Open")
                        .icon(Icon::SquareTerminal)
                        .kbd(chip("o"))
                        .on_select(|_, _| {}),
                )
                .item(MenuItem::new("Rename").kbd(chip("r")).on_select(|_, _| {}))
                .item(
                    MenuItem::new("Chosen option")
                        .checked(true)
                        .on_select(|_, _| {}),
                )
                .separator()
                .item(
                    MenuItem::new("Delete worktree")
                        .icon(Icon::Trash2)
                        .kbd(chip("shift-d"))
                        .destructive(true)
                        .on_select(|_, _| {}),
                )
        });
        Self {
            focus_handle: cx.focus_handle(),
            last: "nothing yet".into(),
            access: Rc::new(Cell::new(1)),
            specimen,
        }
    }

    fn ran(&mut self, action: &dyn Action, cx: &mut Context<Self>) {
        self.last = action.name().into();
        cx.notify();
    }
}

impl Focusable for MenusGallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn row_item(label: &'static str, icon: Icon, action: Box<dyn Action>) -> MenuItem {
    MenuItem::new(label).icon(icon).action(action)
}

/// A worktree row's verbs: the ⋯ menu and the right-click menu show the same list.
fn row_menu(menu: Menu) -> Menu {
    menu.item(row_item("Open", Icon::SquareTerminal, Box::new(Open)))
        .item(MenuItem::new("Rename").action(Box::new(Rename)))
        .item(MenuItem::new("Archive").action(Box::new(Archive)))
        .separator()
        .item(
            MenuItem::new("Delete worktree")
                .icon(Icon::Trash2)
                .action(Box::new(Delete))
                .destructive(true),
        )
}

/// The `+` new-tab menu of the Workspace mockup.
fn new_tab_menu(menu: Menu) -> Menu {
    menu.header("New tab")
        .item(row_item(
            "Terminal",
            Icon::SquareTerminal,
            Box::new(NewTerminal),
        ))
        .item(row_item(
            "Claude thread",
            Icon::Sparkles,
            Box::new(NewAgent),
        ))
        .item(MenuItem::new("Lazygit").icon(Icon::GitBranch))
        .separator()
        .item(MenuItem::new("Board").action(Box::new(Board)))
}

fn triggers_section(cx: &App) -> AnyElement {
    let t = cx.theme();
    let children = vec![
        LAYOUT.labeled(
            "⋯ row menu (bottom right)",
            t,
            div()
                .flex()
                .items_center()
                .justify_between()
                .w(px(420.0))
                .h(t.metrics.row_h_comfortable)
                .px(t.space.md)
                .rounded(t.radii.card)
                .bg(t.colors.surface_raised)
                .border(t.metrics.hairline)
                .border_color(t.colors.border)
                .child(Text::ui_strong("feature/menus"))
                .child(
                    PopoverMenu::new("row-more")
                        .anchor(MenuAnchor::BottomRight)
                        .trigger_with(|open, _, _| {
                            IconButton::new("row-more-trigger", Icon::Ellipsis, "More actions")
                                .size(ButtonSize::Compact)
                                .selected(open)
                        })
                        .menu(|menu, _, _| row_menu(menu)),
                ),
        ),
        LAYOUT.labeled(
            "+ new tab (bottom left)",
            t,
            PopoverMenu::new("new-tab")
                .trigger_with(|open, _, _| {
                    Button::new("new-tab-trigger", "New tab")
                        .icon(Icon::Plus)
                        .style(ButtonStyle::Ghost)
                        .selected(open)
                })
                .menu(|menu, _, _| new_tab_menu(menu)),
        ),
        LAYOUT.labeled(
            "right-click area",
            t,
            ContextMenu::new(
                "context-area",
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(420.0))
                    .h(px(96.0))
                    .rounded(t.radii.card)
                    .border(t.metrics.hairline)
                    .border_color(t.colors.border_strong)
                    .child(Text::caption("Right-click anywhere in here")),
            )
            .menu(|menu, _, _| row_menu(menu)),
        ),
    ];
    LAYOUT.section("menu · triggers", t, children)
}

fn dropdown_section(access: &Rc<Cell<usize>>, cx: &App) -> AnyElement {
    let t = cx.theme();
    let dropdown = |id: &'static str, access: Rc<Cell<usize>>| {
        Dropdown::new(id, ACCESS[access.get()]).menu(move |menu, _, _| {
            ACCESS.iter().enumerate().fold(menu, |menu, (ix, label)| {
                let access = access.clone();
                menu.item(MenuItem::new(*label).checked(access.get() == ix).on_select(
                    move |window, _| {
                        access.set(ix);
                        window.refresh();
                    },
                ))
            })
        })
    };
    let children = vec![
        LAYOUT.labeled(
            "field",
            t,
            div()
                .w(px(260.0))
                .child(dropdown("access", access.clone()).label("Default access")),
        ),
        LAYOUT.labeled(
            "full width, no label",
            t,
            div()
                .w(px(420.0))
                .child(dropdown("access-wide", access.clone()).full_width()),
        ),
    ];
    LAYOUT.section("dropdown · a check on the chosen option", t, children)
}

fn specimen_section(specimen: &Entity<Menu>, cx: &App) -> AnyElement {
    let t = cx.theme();
    let children = vec![LAYOUT.labeled(
        "header · icon · key · check · separator · destructive",
        t,
        strip(t, vec![specimen.clone().into_any_element()]),
    )];
    LAYOUT.section("menu · every item state", t, children)
}

impl Render for MenusGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pad = cx.theme().space.xl;
        let sections = vec![
            triggers_section(cx),
            dropdown_section(&self.access, cx),
            specimen_section(&self.specimen, cx),
        ];
        AppFrame::new()
            .body(
                div()
                    .id("menus-scroll")
                    .track_focus(&self.focus_handle)
                    .key_context(CONTEXT)
                    .on_action(cx.listener(|_, _: &ToggleTheme, _, cx| {
                        Theme::toggle(cx);
                        cx.notify();
                    }))
                    .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
                    .on_action(cx.listener(|g, a: &Open, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &Rename, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &NewTerminal, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &NewAgent, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &Delete, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &Board, _, cx| g.ran(a, cx)))
                    .size_full()
                    .overflow_y_scroll()
                    .p(pad)
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .status_bar(StatusBar::new().breadcrumb(SharedString::from(format!(
                "fleet-ui-kit · menus · last action: {} · access: {}",
                self.last,
                ACCESS[self.access.get()]
            ))))
    }
}

fn main() {
    support::runtime::run_with_window(
        "fleet-ui-kit · menus",
        (1280.0, 860.0),
        Quit,
        |cx| {
            let context = Some(CONTEXT);
            cx.bind_keys(menu_key_bindings());
            cx.bind_keys([
                KeyBinding::new("ctrl-t", ToggleTheme, context),
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("o", Open, context),
                KeyBinding::new("r", Rename, context),
                KeyBinding::new("ctrl-s c", NewTerminal, context),
                KeyBinding::new("ctrl-s a", NewAgent, context),
                KeyBinding::new("shift-d", Delete, context),
                KeyBinding::new("b", Board, context),
                KeyBinding::new("x", Archive, context),
            ]);
        },
        MenusGallery::new,
    );
}
