//! The visual and behavioural test bench for the **controls** group of `fleet-ui-kit`.
//!
//! `Kbd` · `Button` · `IconButton` · `Tooltip`.
//!
//! Every button here is wired the way a screen wires one: `.action(..)`, with the chip resolved
//! from the live keymap this example binds. Click a button or press its key and the status bar
//! names the action that ran — the two paths are the same dispatch. An action this example does
//! not bind (`Archive`) shows no chip. Rest the pointer on an icon button for its tooltip.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_buttons
//! ```
//!
//! | Key | What |
//! | --- | --- |
//! | `ctrl-t` | toggle light / dark |
//! | `enter` · `escape` · `shift-y` · `ctrl-s a` · `g b` · `r` · `ctrl-f` · `ctrl-k` | the demo actions |
//! | `ctrl-q` / `cmd-q` | quit |

pub mod support;
const LAYOUT: support::layout::GalleryLayout = support::layout::GalleryLayout {
    label_width: 170.0,
    column: false,
    divided: false,
    compact: true,
};
use fleet_ui_kit::prelude::*;
use gpui::{Action, AnyElement, App, Context, FocusHandle, Focusable, KeyBinding, Window, actions};
use support::layout::strip;

actions!(
    gallery_buttons,
    [
        ToggleTheme,
        Quit,
        Create,
        Cancel,
        DeleteAnyway,
        CloseTerminal,
        GoToBoard,
        Refresh,
        Filter,
        Commands,
        Archive,
    ]
);

const CONTEXT: &str = "GalleryButtons";

struct ButtonsGallery {
    focus_handle: FocusHandle,
    /// The last action that ran, by key or by click.
    last: SharedString,
    /// The one toggle on the page.
    pinned: bool,
}

impl ButtonsGallery {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            last: "nothing yet".into(),
            pinned: true,
        }
    }

    fn ran(&mut self, action: &dyn Action, cx: &mut Context<Self>) {
        self.last = action.name().into();
        cx.notify();
    }
}

impl Focusable for ButtonsGallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn kbd(source: &str) -> Kbd {
    Kbd::parse(source).unwrap_or_else(|error| panic!("{source:?}: {error}"))
}

const STYLES: [(&str, ButtonStyle); 4] = [
    ("primary", ButtonStyle::Primary),
    ("secondary", ButtonStyle::Secondary),
    ("ghost", ButtonStyle::Ghost),
    ("danger", ButtonStyle::Danger),
];

fn styles_section(cx: &App) -> AnyElement {
    let t = cx.theme();
    let mut children = Vec::new();
    for (size_name, size) in [("", ButtonSize::Default), (" compact", ButtonSize::Compact)] {
        for (name, style) in STYLES {
            let id = |slot: &'static str| SharedString::from(format!("{name}{size_name}-{slot}"));
            let action: Box<dyn Action> = match style {
                ButtonStyle::Primary => Box::new(Create),
                ButtonStyle::Secondary => Box::new(Cancel),
                ButtonStyle::Ghost => Box::new(GoToBoard),
                ButtonStyle::Danger => Box::new(DeleteAnyway),
            };
            let label = match style {
                ButtonStyle::Primary => "Create worktree",
                ButtonStyle::Secondary => "Cancel",
                ButtonStyle::Ghost => "Board",
                ButtonStyle::Danger => "Delete anyway",
            };
            let icon = match style {
                ButtonStyle::Primary => Icon::Plus,
                ButtonStyle::Secondary => Icon::X,
                ButtonStyle::Ghost => Icon::GitBranch,
                ButtonStyle::Danger => Icon::Trash2,
            };
            let base = |slot| Button::new(id(slot), label).style(style).size(size);
            children.push(LAYOUT.labeled(
                &format!("{name}{size_name}"),
                t,
                strip(
                    t,
                    vec![
                        base("plain").into_any_element(),
                        base("icon").icon(icon).into_any_element(),
                        base("kbd").action(action.boxed_clone()).into_any_element(),
                        base("both")
                            .icon(icon)
                            .action(action.boxed_clone())
                            .into_any_element(),
                        base("disabled")
                            .icon(icon)
                            .action(action.boxed_clone())
                            .disabled(true)
                            .into_any_element(),
                    ],
                ),
            ));
        }
    }
    LAYOUT.section("button · style × size", t, children)
}

fn states_section(pinned: bool, cx: &mut Context<ButtonsGallery>) -> AnyElement {
    let t = cx.theme().clone();
    let toggle = cx.listener(|gallery, _: &gpui::ClickEvent, _, cx| {
        gallery.pinned = !gallery.pinned;
        cx.notify();
    });
    let children = vec![
        LAYOUT.labeled(
            "selected (toggle)",
            &t,
            strip(
                &t,
                vec![
                    Button::new("pin", "Pinned")
                        .icon(Icon::Flag)
                        .selected(pinned)
                        .on_click(toggle)
                        .into_any_element(),
                    Button::new("ghost-on", "Mine")
                        .style(ButtonStyle::Ghost)
                        .selected(true)
                        .into_any_element(),
                    Button::new("ghost-off", "Everyone")
                        .style(ButtonStyle::Ghost)
                        .selected(false)
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "sequence chip",
            &t,
            strip(
                &t,
                vec![
                    Button::new("close-terminal", "Close terminal")
                        .action(Box::new(CloseTerminal))
                        .into_any_element(),
                    Button::new("explicit", "Explicit kbd")
                        .kbd(kbd("ctrl-s x"))
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "unbound action",
            &t,
            Button::new("archive", "Archive")
                .action(Box::new(Archive))
                .tooltip("Archive has no key here, so it shows no chip"),
        ),
        LAYOUT.labeled(
            "with tooltip",
            &t,
            Button::new("tooltip", "Refresh")
                .icon(Icon::RefreshCw)
                .action(Box::new(Refresh))
                .tooltip("Fetch every remote now"),
        ),
        LAYOUT.labeled(
            "full width",
            &t,
            div().w(px(360.0)).child(
                Button::new("full", "Create worktree")
                    .style(ButtonStyle::Primary)
                    .action(Box::new(Create))
                    .full_width(),
            ),
        ),
    ];
    LAYOUT.section("button · states", &t, children)
}

fn icon_buttons_section(cx: &App) -> AnyElement {
    let t = cx.theme();
    let row = |size: ButtonSize, prefix: &'static str| {
        strip(
            t,
            vec![
                IconButton::new((prefix, 0usize), Icon::RefreshCw, "Refresh")
                    .size(size)
                    .action(Box::new(Refresh))
                    .into_any_element(),
                IconButton::new((prefix, 1usize), Icon::Search, "Filter")
                    .size(size)
                    .action(Box::new(Filter))
                    .into_any_element(),
                IconButton::new((prefix, 2usize), Icon::Command, "Commands")
                    .size(size)
                    .style(ButtonStyle::Secondary)
                    .action(Box::new(Commands))
                    .into_any_element(),
                IconButton::new((prefix, 3usize), Icon::Flag, "Pinned")
                    .size(size)
                    .selected(true)
                    .into_any_element(),
                IconButton::new((prefix, 4usize), Icon::Paperclip, "Archive (unbound)")
                    .size(size)
                    .action(Box::new(Archive))
                    .into_any_element(),
                IconButton::new((prefix, 5usize), Icon::Trash2, "Delete (not yet)")
                    .size(size)
                    .disabled(true)
                    .into_any_element(),
            ],
        )
    };
    let children = vec![
        LAYOUT.labeled("default · hover me", t, row(ButtonSize::Default, "icon")),
        LAYOUT.labeled("compact", t, row(ButtonSize::Compact, "icon-compact")),
    ];
    LAYOUT.section("icon button · tooltip carries label and key", t, children)
}

fn kbd_section(cx: &App) -> AnyElement {
    let t = cx.theme();
    let on = |bg: gpui::Hsla, chip: Kbd| {
        div()
            .p(t.space.sm)
            .rounded(t.radii.control)
            .bg(bg)
            .child(chip)
            .into_any_element()
    };
    let children = vec![
        LAYOUT.labeled(
            "chord · sequence",
            t,
            strip(
                t,
                [
                    "ctrl-s",
                    "ctrl-s a",
                    "g b",
                    "shift-y",
                    "cmd-k",
                    "alt-shift-f5",
                ]
                .into_iter()
                .map(|keys| kbd(keys).into_any_element())
                .collect(),
            ),
        ),
        LAYOUT.labeled(
            "named keys",
            t,
            strip(
                t,
                [
                    "enter",
                    "escape",
                    "tab",
                    "space",
                    "backspace",
                    "up",
                    "down",
                    "left",
                    "right",
                ]
                .into_iter()
                .map(|keys| kbd(keys).into_any_element())
                .collect(),
            ),
        ),
        LAYOUT.labeled(
            "small",
            t,
            strip(
                t,
                vec![
                    kbd("ctrl-s a").size(KbdSize::Small).into_any_element(),
                    kbd("escape").size(KbdSize::Small).into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "on accent · on danger",
            t,
            strip(
                t,
                vec![
                    on(t.colors.accent_fill, kbd("enter").tone(KbdTone::OnAccent)),
                    on(t.colors.danger, kbd("shift-y").tone(KbdTone::OnDanger)),
                ],
            ),
        ),
        LAYOUT.labeled(
            "warning (a held prefix)",
            t,
            strip(
                t,
                vec![kbd("ctrl-s").tone(KbdTone::Warning).into_any_element()],
            ),
        ),
        LAYOUT.labeled(
            "tooltip (static)",
            t,
            strip(
                t,
                vec![
                    Tooltip::new("Refresh").kbd(kbd("r")).into_any_element(),
                    Tooltip::new("A tooltip with no key").into_any_element(),
                ],
            ),
        ),
    ];
    LAYOUT.section("kbd · tooltip", t, children)
}

impl Render for ButtonsGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pad = cx.theme().space.xl;
        let sections = vec![
            styles_section(cx),
            states_section(self.pinned, cx),
            icon_buttons_section(cx),
            kbd_section(cx),
        ];
        AppFrame::new()
            .body(
                div()
                    .id("buttons-scroll")
                    .track_focus(&self.focus_handle)
                    .key_context(CONTEXT)
                    .on_action(cx.listener(|_, _: &ToggleTheme, _, cx| {
                        Theme::toggle(cx);
                        cx.notify();
                    }))
                    .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
                    .on_action(cx.listener(|g, a: &Create, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &Cancel, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &DeleteAnyway, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &CloseTerminal, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &GoToBoard, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &Refresh, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &Filter, _, cx| g.ran(a, cx)))
                    .on_action(cx.listener(|g, a: &Commands, _, cx| g.ran(a, cx)))
                    .size_full()
                    .overflow_y_scroll()
                    .p(pad)
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .status_bar(StatusBar::new().breadcrumb(SharedString::from(format!(
                "fleet-ui-kit · controls · last action: {}",
                self.last
            ))))
    }
}

fn main() {
    support::runtime::run(
        "fleet-ui-kit · controls",
        (1280.0, 860.0),
        Quit,
        |cx| {
            let context = Some(CONTEXT);
            cx.bind_keys([
                KeyBinding::new("ctrl-t", ToggleTheme, context),
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("enter", Create, context),
                KeyBinding::new("escape", Cancel, context),
                KeyBinding::new("shift-y", DeleteAnyway, context),
                KeyBinding::new("ctrl-s a", CloseTerminal, context),
                KeyBinding::new("g b", GoToBoard, context),
                KeyBinding::new("r", Refresh, context),
                KeyBinding::new("ctrl-f", Filter, context),
                KeyBinding::new("ctrl-k", Commands, context),
            ]);
        },
        ButtonsGallery::new,
    );
}
