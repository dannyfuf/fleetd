//! The visual test bench for the **jobs and terminal** half of `fleet-ui-kit`.
//!
//! `TerminalGrid`, `TerminalTabStrip`, `ScrollPill`, `ScrollbackBadge`, `PrefixHint`,
//! `ExitStrip`, `JobRow`, `JobTicker`, `StickyErrorSlot` and `LogView`, each in every state
//! the design system names, in both themes.
//!
//! The first section is **live**: a fake VT frame generator ticks at 60 Hz and the grid is
//! notified only when the frame it would paint actually changed — which is the redraw contract
//! `TerminalGrid` is built for, demonstrated rather than asserted. The frame counter in the
//! header counts changed generated frames; it does not claim to measure actual paints.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_terminal
//! ```
//!
//! | key | |
//! | --- | --- |
//! | `t` | light / dark |
//! | `a` | alt-screen (suppresses the scroll overlays) |
//! | `s` | Scroll mode (`ScrollPill`) / scrolled-back (`ScrollbackBadge`) / live |
//! | `v` | selection |
//! | `u` | focus the terminal / unfocus it (hollow cursor) |
//! | `c` | cursor shape |
//! | `p` | prefix hint |
//! | `L` | focus the log (then `f`, `j`, `k`, `G`); `esc` returns |
//! | `q` | quit |

use std::sync::Arc;

mod live_terminal;
pub mod support;
use live_terminal::{LiveTerminal, frames::*};

const LAYOUT: support::layout::GalleryLayout = support::layout::GalleryLayout {
    label_width: 168.0,
    column: false,
    divided: false,
    compact: false,
};
use fleet_ui_kit::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, KeyBinding, SharedString,
    UniformListScrollHandle, Window, actions, div, px,
};

actions!(
    gallery_terminal,
    [
        ToggleTheme,
        ToggleAltScreen,
        CycleScroll,
        ToggleSelection,
        ToggleFocus,
        CycleCursor,
        TogglePrefix,
        FocusLog,
        FocusRoot,
        Quit,
    ]
);

struct Gallery {
    focus_handle: FocusHandle,
    log_focus: FocusHandle,
    log_scroll: UniformListScrollHandle,

    live: Entity<LiveTerminal>,

    log_following: bool,
    log_top: usize,
    log_lines: Arc<[SharedString]>,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            log_focus: cx.focus_handle(),
            log_scroll: UniformListScrollHandle::new(),
            live: cx.new(LiveTerminal::new),
            log_following: true,
            log_top: 0,
            log_lines: sample_log().into(),
        }
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        self.live.update(cx, |live, cx| {
            live.refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    fn toggle_alt_screen(
        &mut self,
        _: &ToggleAltScreen,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.live.update(cx, |live, cx| {
            live.alt_screen = !live.alt_screen;
            live.refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    fn cycle_scroll(&mut self, _: &CycleScroll, _window: &mut Window, cx: &mut Context<Self>) {
        self.live.update(cx, |live, cx| {
            live.scroll = live.scroll.next();
            cx.notify();
        });
    }

    fn toggle_selection(
        &mut self,
        _: &ToggleSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.live.update(cx, |live, cx| {
            live.selection = !live.selection;
            cx.notify();
        });
    }

    fn toggle_focus(&mut self, _: &ToggleFocus, _window: &mut Window, cx: &mut Context<Self>) {
        self.live.update(cx, |live, cx| {
            live.focused = !live.focused;
            cx.notify();
        });
    }

    fn cycle_cursor(&mut self, _: &CycleCursor, _window: &mut Window, cx: &mut Context<Self>) {
        self.live.update(cx, |live, cx| {
            live.cursor_shape += 1;
            cx.notify();
        });
    }

    fn toggle_prefix(&mut self, _: &TogglePrefix, _window: &mut Window, cx: &mut Context<Self>) {
        self.live.update(cx, |live, cx| {
            live.prefix = !live.prefix;
            cx.notify();
        });
    }

    fn focus_log(&mut self, _: &FocusLog, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.log_focus, cx);
        cx.notify();
    }

    fn focus_root(&mut self, _: &FocusRoot, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }
}

impl Focusable for Gallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn sample_log() -> Vec<SharedString> {
    let mut lines = Vec::new();
    for ix in 0..48 {
        lines.push(SharedString::from(format!(
            "remote: Counting objects: {}% ({}/202), done.",
            (ix * 2).min(100),
            ix * 4
        )));
    }
    lines.push(SharedString::new_static(
        "fatal: unable to access 'https://github.com/…': HTTP 502",
    ));
    lines
}

/// A bordered, `relative` box: the overlays position themselves inside one of these.
fn stage(t: &Theme, height: gpui::Pixels, child: impl IntoElement) -> AnyElement {
    div()
        .relative()
        .w_full()
        .h(height)
        .rounded(t.radii.sm)
        .bg(t.terminal.background)
        .border(t.metrics.hairline)
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

fn attributes_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let row = |label: &'static str, sgr: Sgr| {
        row_of(
            &t,
            &[
                Span("  ", Sgr::default()),
                Span(label, sgr),
                Span("  the quick brown fox", sgr),
            ],
        )
    };
    let rows = vec![
        row("plain     ", Sgr::default()),
        row("bold      ", Sgr::default().bold()),
        row("dim       ", Sgr::default().dim()),
        row("italic    ", Sgr::default().italic()),
        row(
            "underline ",
            Sgr::default().underline(UnderlineStyle::Single),
        ),
        row(
            "double    ",
            Sgr::default().underline(UnderlineStyle::Double),
        ),
        row(
            "curly     ",
            Sgr::default()
                .underline(UnderlineStyle::Curly)
                .underline_color(1),
        ),
        row("strike    ", Sgr::default().strikethrough()),
        row("inverse   ", Sgr::default().inverse()),
        row("blink     ", Sgr::default().blink()),
        row("invisible ", Sgr::default().invisible().bg(4)),
        row("on bg     ", Sgr::fg(0).bg(3)),
        row("256 color ", Sgr::fg(13).bold()),
        wide_row(&t, "  幅の広い文字", Sgr::fg(6)),
    ];

    let palette = {
        let mut cells = Vec::new();
        for ix in 0..16 {
            for ch in " \u{2588}\u{2588} ".chars() {
                cells.push(Sgr::fg(ix).apply(GridCell::new(ch.to_string(), &t), &t));
            }
        }
        GridRow::new(cells)
    };

    let cursor_stage = |label: &'static str, shape: CursorShape, focused: bool| {
        let rows = vec![row_of(
            &t,
            &[
                Span("  ", Sgr::default()),
                Span("cursor here", Sgr::default()),
            ],
        )];
        div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .child(Text::hint(label).faint())
            .child(
                div()
                    .w(px(180.0))
                    .h(px(38.0))
                    .rounded(t.radii.sm)
                    .border(t.metrics.hairline)
                    .border_color(t.colors.border)
                    .overflow_hidden()
                    .child(
                        TerminalGrid::from_shared(rows)
                            .padding(t.space.xs)
                            .focused(focused)
                            .cursor(GridCursor {
                                row: 0,
                                col: 9,
                                visible: true,
                                shape,
                            }),
                    ),
            )
            .into_any_element()
    };

    let children = vec![
        LAYOUT.labeled(
            "attributes",
            &t,
            stage(
                &t,
                px(f32::from(t.text.data.line_height) * 15.0),
                TerminalGrid::from_shared(rows),
            ),
        ),
        LAYOUT.labeled(
            "ansi 0-15",
            &t,
            stage(&t, px(40.0), TerminalGrid::from_shared(vec![palette])),
        ),
        LAYOUT.labeled(
            "selection",
            &t,
            stage(
                &t,
                px(80.0),
                TerminalGrid::from_shared(vec![
                    row_of(&t, &[Span("  selected across", Sgr::default())]),
                    row_of(&t, &[Span("  three whole rows", Sgr::default())]),
                    row_of(&t, &[Span("  and part of this", Sgr::default())]),
                ])
                .selection(GridSelection::new(0, 2, 2, 8)),
            ),
        ),
        div()
            .flex()
            .flex_wrap()
            .gap(t.space.md)
            .children(vec![
                cursor_stage("block", CursorShape::Block, true),
                cursor_stage("bar", CursorShape::Bar, true),
                cursor_stage("underline", CursorShape::Underline, true),
                cursor_stage("hollow (unfocused)", CursorShape::Block, false),
            ])
            .into_any_element(),
    ];
    LAYOUT.section("terminal grid \u{b7} states", &t, children)
}

fn overlays_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let children = vec![
        LAYOUT.labeled(
            "scroll pill",
            &t,
            stage(&t, px(64.0), ScrollPill::new(412, 2000)),
        ),
        LAYOUT.labeled(
            "scroll pill \u{b7} selecting",
            &t,
            stage(&t, px(78.0), ScrollPill::new(412, 2000).selecting(true)),
        ),
        LAYOUT.labeled(
            "scroll pill \u{b7} alt-screen (suppressed)",
            &t,
            stage(&t, px(48.0), ScrollPill::new(412, 2000).alt_screen(true)),
        ),
        LAYOUT.labeled(
            "scrollback badge",
            &t,
            stage(&t, px(56.0), ScrollbackBadge::new(412, 2000)),
        ),
        LAYOUT.labeled(
            "scrollback badge \u{b7} live (suppressed)",
            &t,
            stage(&t, px(48.0), ScrollbackBadge::new(0, 2000)),
        ),
        LAYOUT.labeled(
            "prefix hint \u{b7} hidden",
            &t,
            stage(&t, px(48.0), PrefixHint::new(false)),
        ),
        LAYOUT.labeled(
            "prefix hint \u{b7} visible",
            &t,
            stage(&t, px(64.0), PrefixHint::new(true)),
        ),
        LAYOUT.labeled(
            "prefix hint \u{b7} custom keys",
            &t,
            stage(
                &t,
                px(64.0),
                PrefixHint::new(true).prefix("^A").hints(
                    KeyHintRow::new()
                        .key("s", "hub")
                        .key("]", "paste")
                        .key("z", "zoom"),
                ),
            ),
        ),
        LAYOUT.labeled("exit strip \u{b7} failure", &t, ExitStrip::new(1)),
        LAYOUT.labeled("exit strip \u{b7} clean", &t, ExitStrip::new(0)),
        LAYOUT.labeled("exit strip \u{b7} killed", &t, ExitStrip::new(None)),
        LAYOUT.labeled(
            "exit strip \u{b7} custom keys",
            &t,
            ExitStrip::new(127).hints(KeyHintRow::new().key("^s r", "restart")),
        ),
    ];
    LAYOUT.section("terminal overlays", &t, children)
}

fn tabs_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let children = vec![
        LAYOUT.labeled(
            "default \u{b7} three terminals",
            &t,
            TerminalTabStrip::new([
                TerminalTab::new(1, "nvim"),
                TerminalTab::new(2, "cc"),
                TerminalTab::new(3, "lg"),
            ])
            .id("tabs-default"),
        ),
        LAYOUT.labeled(
            "every mark",
            &t,
            TerminalTabStrip::new([
                TerminalTab::new(1, "nvim").keep_alive(Icon::FilePen),
                TerminalTab::new(2, "cc")
                    .activity(true)
                    .keep_alive(Icon::Bot),
                TerminalTab::new(3, "server").keep_alive(Icon::Server),
                TerminalTab::new(4, "test").exited(1),
                TerminalTab::new(5, "build").exited(None),
                TerminalTab::new(6, "a-very-long-terminal-name").activity(true),
            ])
            .id("tabs-marks")
            .active(2),
        ),
        LAYOUT.labeled(
            "waking \u{b7} every PTY still spawning",
            &t,
            TerminalTabStrip::new([
                TerminalTab::new(1, "nvim").starting(true),
                TerminalTab::new(2, "cc").starting(true),
                TerminalTab::new(3, "lg").starting(true),
            ])
            .id("tabs-waking"),
        ),
        LAYOUT.labeled(
            "no new-tab affordance",
            &t,
            TerminalTabStrip::new([TerminalTab::new(1, "shell")])
                .id("tabs-no-plus")
                .show_plus(false),
        ),
    ];
    LAYOUT.section("terminal tab strip", &t, children)
}

fn jobs_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let rows = div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(t.radii.sm)
        .bg(t.colors.surface)
        .border(t.metrics.hairline)
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(
            JobRow::new(JobStatus::Running, "clone", "nixos")
                .id("job-running")
                .elapsed("0:42")
                .percent(40)
                .progress("Receiving objects: 40% (81/202)")
                .selected(true)
                .cursor(true),
        )
        .child(
            JobRow::new(JobStatus::Running, "hooks", "buk/payroll#feat-rut")
                .id("job-hooks")
                .elapsed("0:08")
                .progress("pnpm install (2/3)"),
        )
        .child(
            JobRow::new(JobStatus::Cancelling, "pool", "dannyfuf/fleetd")
                .id("job-cancelling")
                .elapsed("1:03")
                .progress("waiting for the current copy to finish"),
        )
        .child(JobRow::new(JobStatus::Queued, "prune", "buk/www").id("job-queued"))
        .child(
            JobRow::new(JobStatus::Failed, "prs", "review")
                .id("job-failed")
                .elapsed("1m")
                .trailing_key("R")
                .progress("gh: HTTP 502 upstream connect error"),
        )
        .child(
            JobRow::new(JobStatus::Done, "prune", "buk/www")
                .id("job-done")
                .elapsed("12s"),
        )
        .child(JobRow::new(JobStatus::Cancelled, "fetch", "dannyfuf/fleetd").id("job-cancelled"));

    let confirm_rows = div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(t.radii.sm)
        .bg(t.colors.surface)
        .border(t.metrics.hairline)
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(
            JobRow::new(JobStatus::Running, "clone", "nixos")
                .id("confirm-1")
                .retryable(true),
        )
        .child(
            JobRow::new(JobStatus::Running, "hooks", "buk/payroll")
                .id("confirm-2")
                .retryable(false),
        );

    let children = vec![
        LAYOUT.labeled("every status", &t, rows),
        LAYOUT.labeled("quit-and-stop confirm", &t, confirm_rows),
        LAYOUT.labeled(
            "ticker",
            &t,
            JobTicker::new("clone", "nixos").id("ticker-1"),
        ),
        LAYOUT.labeled(
            "ticker \u{b7} percent + elapsed + others",
            &t,
            JobTicker::new("clone", "nixos")
                .id("ticker-2")
                .elapsed("0:42")
                .percent(40)
                .extra(2),
        ),
        LAYOUT.labeled(
            "sticky error",
            &t,
            StickyErrorSlot::new("gh: HTTP 502 upstream connect error").id("err-1"),
        ),
        LAYOUT.labeled(
            "sticky error \u{b7} repeated, prefixed key",
            &t,
            StickyErrorSlot::new("gh: HTTP 502 upstream connect error")
                .id("err-2")
                .count(7)
                .key("^s !"),
        ),
    ];
    LAYOUT.section("jobs", &t, children)
}

impl Gallery {
    fn log_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let weak = cx.weak_entity();
        let log = LogView::from_shared("log", self.log_lines.clone())
            .following(self.log_following)
            .top(self.log_top)
            .track_scroll(&self.log_scroll)
            .focus(&self.log_focus)
            .on_command(move |command, _window, cx| {
                weak.update(cx, |this, cx| {
                    match command {
                        LogCommand::ToggleFollow => this.log_following = !this.log_following,
                        LogCommand::Follow => {
                            this.log_following = true;
                            this.log_top = this.log_lines.len().saturating_sub(1);
                        }
                        LogCommand::ScrollTo(top) => {
                            this.log_following = false;
                            this.log_top = top;
                        }
                    }
                    cx.notify();
                })
                .ok();
            });

        let children = vec![
            LAYOUT.labeled(
                "log \u{b7} press L to focus, then f / j / k / G",
                &t,
                div()
                    .w_full()
                    .h(px(180.0))
                    .rounded(t.radii.sm)
                    .bg(t.colors.surface)
                    .border(t.metrics.hairline)
                    .border_color(t.colors.border)
                    .overflow_hidden()
                    .child(log),
            ),
            LAYOUT.labeled(
                "log \u{b7} empty",
                &t,
                div()
                    .w_full()
                    .h(px(72.0))
                    .rounded(t.radii.sm)
                    .bg(t.colors.surface)
                    .border(t.metrics.hairline)
                    .border_color(t.colors.border)
                    .child(LogView::from_shared("log-empty", Vec::new())),
            ),
        ];
        LAYOUT.section("log view", &t, children)
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = cx.theme().mode;
        let pad = cx.theme().space.xl;

        let sections = vec![
            self.live.clone().into_any_element(),
            attributes_section(cx),
            overlays_section(cx),
            tabs_section(cx),
            jobs_section(cx),
            self.log_section(cx),
        ];

        AppFrame::new()
            .context_bar(
                ContextBar::new([ContextTab::new("jobs + terminal gallery", 1)])
                    .leading_inset(px(84.0))
                    .chip(Chip::labeled(
                        if mode.is_dark() {
                            Icon::Moon
                        } else {
                            Icon::CircleArrowUp
                        },
                        if mode.is_dark() { "dark" } else { "light" },
                    ))
                    .daemon(DaemonState::Healthy),
            )
            .body(
                div()
                    .id("gallery-terminal-scroll")
                    .track_focus(&self.focus_handle)
                    .key_context("GalleryTerminal")
                    .on_action(cx.listener(Self::toggle_theme))
                    .on_action(cx.listener(Self::toggle_alt_screen))
                    .on_action(cx.listener(Self::cycle_scroll))
                    .on_action(cx.listener(Self::toggle_selection))
                    .on_action(cx.listener(Self::toggle_focus))
                    .on_action(cx.listener(Self::cycle_cursor))
                    .on_action(cx.listener(Self::toggle_prefix))
                    .on_action(cx.listener(Self::focus_log))
                    .on_action(cx.listener(Self::focus_root))
                    .on_action(cx.listener(Self::quit))
                    .size_full()
                    .overflow_y_scroll()
                    .p(pad)
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .status_bar(
                StatusBar::new()
                    .breadcrumb("fleet-ui-kit \u{b7} jobs and terminal")
                    .mode(Mode::Terminal)
                    .ticker(
                        KeyHintRow::new()
                            .key("t", "theme")
                            .key("a", "alt")
                            .key("s", "scroll")
                            .key("v", "select")
                            .key("u", "focus")
                            .key("c", "cursor")
                            .key("p", "prefix")
                            .key("L", "log")
                            .key("q", "quit"),
                    ),
            )
    }
}

fn main() {
    support::runtime::run(
        "fleet-ui-kit · jobs + terminal",
        (1180.0, 860.0),
        Quit,
        |cx| {
            cx.bind_keys([
                KeyBinding::new("t", ToggleTheme, None),
                KeyBinding::new("a", ToggleAltScreen, None),
                KeyBinding::new("s", CycleScroll, None),
                KeyBinding::new("v", ToggleSelection, None),
                KeyBinding::new("u", ToggleFocus, None),
                KeyBinding::new("c", CycleCursor, None),
                KeyBinding::new("p", TogglePrefix, None),
                KeyBinding::new("shift-l", FocusLog, None),
                KeyBinding::new("escape", FocusRoot, None),
                KeyBinding::new("q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
            ]);
        },
        Gallery::new,
    );
}
