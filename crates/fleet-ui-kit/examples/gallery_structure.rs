//! Visual bench for the **structure** group of `fleet-ui-kit`.
//!
//! Every structural component, in every state it can be in, in both themes:
//! `AppFrame`, `SplitLayout`, `ContextBar`, `StatusBar`, `Pane`, `PaneHeader`, `Sheet`,
//! `Dialog`, `Overlay`, `ToastStack`, `Veil`, `Banner`, `ModeWord` and `DaemonDot`.
//!
//! The floating layers are wired as **live layers of this window**, not as pictures of
//! themselves, because their whole contract is where they sit relative to the chrome and to
//! each other: press `d`, `p`, `j` and `o` together and the paint order of
//! [`fleet_ui_kit::OverlayLayer`] is on screen — sheet under palette under dialog under toasts.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_structure
//! ```
//!
//! | Key | Toggles |
//! | --- | --- |
//! | `t` | light / dark theme |
//! | `b` | the §3.12 case-C banner |
//! | `d` | a dialog · `e` its error line |
//! | `p` | the palette overlay |
//! | `j` | the Jobs sheet · `x` its 640 px expanded width |
//! | `o` | the toast stack |
//! | `v` | the veil over the terminal demo |
//! | `f` | which demo pane owns the focus ring |
//! | `q` | quit |

use fleet_ui_kit::KitAssets;
use fleet_ui_kit::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, Menu, MenuItem,
    Pixels, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, px, size,
};

actions!(
    gallery_structure,
    [
        ToggleTheme,
        ToggleBanner,
        ToggleDialog,
        ToggleDialogError,
        TogglePalette,
        ToggleSheet,
        ToggleSheetExpanded,
        ToggleToasts,
        ToggleVeil,
        CycleFocus,
        Quit,
    ]
);

/// The verbatim §3.12 [D-17] sentence. A warm "reconnected" banner is the single most
/// damaging false reassurance in the app, so the copy is pinned here rather than improvised.
const RECONNECT_TEXT: &str =
    "fleetd restarted. Terminal sessions did not survive; worktrees, jobs and state are intact.";

struct StructureGallery {
    focus_handle: FocusHandle,
    banner: bool,
    dialog: bool,
    dialog_error: bool,
    palette: bool,
    sheet: bool,
    sheet_expanded: bool,
    toasts: bool,
    veil: bool,
    focused_pane: usize,
}

impl StructureGallery {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            banner: false,
            dialog: false,
            dialog_error: false,
            palette: false,
            sheet: false,
            sheet_expanded: false,
            toasts: false,
            veil: true,
            focused_pane: 0,
        }
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        cx.notify();
    }

    fn toggle_banner(&mut self, _: &ToggleBanner, _: &mut Window, cx: &mut Context<Self>) {
        self.banner = !self.banner;
        cx.notify();
    }

    fn toggle_dialog(&mut self, _: &ToggleDialog, _: &mut Window, cx: &mut Context<Self>) {
        self.dialog = !self.dialog;
        cx.notify();
    }

    fn toggle_dialog_error(
        &mut self,
        _: &ToggleDialogError,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dialog_error = !self.dialog_error;
        self.dialog = true;
        cx.notify();
    }

    fn toggle_palette(&mut self, _: &TogglePalette, _: &mut Window, cx: &mut Context<Self>) {
        self.palette = !self.palette;
        cx.notify();
    }

    fn toggle_sheet(&mut self, _: &ToggleSheet, _: &mut Window, cx: &mut Context<Self>) {
        self.sheet = !self.sheet;
        cx.notify();
    }

    fn toggle_sheet_expanded(
        &mut self,
        _: &ToggleSheetExpanded,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sheet_expanded = !self.sheet_expanded;
        self.sheet = true;
        cx.notify();
    }

    fn toggle_toasts(&mut self, _: &ToggleToasts, _: &mut Window, cx: &mut Context<Self>) {
        self.toasts = !self.toasts;
        cx.notify();
    }

    fn toggle_veil(&mut self, _: &ToggleVeil, _: &mut Window, cx: &mut Context<Self>) {
        self.veil = !self.veil;
        cx.notify();
    }

    fn cycle_focus(&mut self, _: &CycleFocus, _: &mut Window, cx: &mut Context<Self>) {
        self.focused_pane = (self.focused_pane + 1) % 3;
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }
}

impl Focusable for StructureGallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ------------------------------------------------------------------ layout helpers

fn section(title: &str, t: &Theme, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(t.space.md)
        .pb(t.space.xl)
        .child(SectionHeader::new(title.to_string()))
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(t.space.md)
                .children(children),
        )
        .into_any_element()
}

/// A caption above a specimen, so a state can be named instead of guessed at.
fn specimen(label: &str, t: &Theme, child: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(t.space.xs)
        .child(Text::hint(label.to_string()).faint())
        .child(child)
        .into_any_element()
}

/// A bordered stage a chrome component can be shown at its real height inside.
fn stage(t: &Theme, height: Pixels, child: impl IntoElement) -> AnyElement {
    div()
        .relative()
        .flex()
        .flex_col()
        .w_full()
        .h(height)
        .rounded(t.radii.sm)
        .bg(t.colors.bg)
        .border_1()
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

/// Filler that stands in for a real list body without dragging the data-display group in.
fn filler(t: &Theme, lines: usize) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .p(t.space.lg)
        .gap(t.space.sm)
        .children((0..lines).map(|ix| {
            div()
                .h(px(8.0))
                .w(gpui::relative(if ix % 3 == 0 { 0.8 } else { 0.55 }))
                .rounded(t.radii.xs)
                .bg(t.colors.skeleton)
        }))
        .into_any_element()
}

/// A stand-in terminal surface: mono rows on the terminal background, so the veil has a live
/// surface to sit on top of.
fn fake_terminal(t: &Theme) -> AnyElement {
    let lines = [
        "$ cargo run -p fleet-ui-kit --example gallery_structure",
        "   Compiling fleet-ui-kit v0.1.0",
        "    Finished dev profile in 4.12s",
        "$ ",
    ];
    div()
        .flex()
        .flex_col()
        .size_full()
        .p(t.space.sm)
        .bg(t.terminal.background)
        .children(
            lines
                .into_iter()
                .map(|line| Text::data(line).color(t.terminal.foreground)),
        )
        .into_any_element()
}

// ------------------------------------------------------------------ sections

fn app_frame_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let mini = |banner: bool| {
        let frame = AppFrame::new()
            .context_bar(
                ContextBar::new([ContextTab::new("buk", 1), ContextTab::new("personal", 2)])
                    .active(0)
                    .leading_inset(t.space.md)
                    .chip(Chip::counter(Icon::CircleDot, 3))
                    .daemon(if banner {
                        DaemonState::Lost
                    } else {
                        DaemonState::Healthy
                    }),
            )
            .body(filler(&t, 6))
            .status_bar(
                StatusBar::new()
                    .breadcrumb("buk › payroll › feat/payroll-fix")
                    .mode(Mode::Normal),
            );
        if banner {
            frame.banner(
                Banner::warning("fleetd stopped")
                    .icon(Icon::Dot)
                    .countdown("reconnecting in 3s")
                    .hints(
                        KeyHintRow::new()
                            .key("r", "reconnect")
                            .key("esc", "dismiss"),
                    ),
            )
        } else {
            frame
        }
    };

    section(
        "AppFrame — 36 context bar · 28 banner · flex body · 26 status bar",
        &t,
        vec![
            specimen(
                "hub: context bar 36 + body + status bar 26",
                &t,
                stage(&t, px(220.0), mini(false)),
            ),
            specimen(
                "with the §3.12 case-C banner: 28 px, pushes the body down, never floats over it",
                &t,
                stage(&t, px(220.0), mini(true)),
            ),
        ],
    )
}

fn split_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let rail = |label: &str| {
        Pane::fixed(t.metrics.rail_w)
            .raised(true)
            .header(PaneHeader::new(label.to_string()).total(6))
            .body(filler(&t, 4))
    };
    section(
        "SplitLayout — fix the rail and the detail panel, never the list",
        &t,
        vec![
            specimen(
                "horizontal · leading fixed 240 (rail) · trailing flexes",
                &t,
                stage(
                    &t,
                    px(160.0),
                    SplitLayout::horizontal()
                        .leading_size(t.metrics.rail_w)
                        .leading(rail("repos"))
                        .trailing(
                            Pane::new()
                                .header(PaneHeader::new("worktrees").scope("payroll").total(12))
                                .body(filler(&t, 4)),
                        ),
                ),
            ),
            specimen(
                "nested: rail | (list | 340 detail) — the rail does not move when the panel opens",
                &t,
                stage(
                    &t,
                    px(160.0),
                    SplitLayout::horizontal()
                        .leading_size(t.metrics.rail_w)
                        .leading(rail("repos"))
                        .trailing(
                            SplitLayout::horizontal()
                                .trailing_size(t.metrics.detail_w)
                                .leading(
                                    Pane::new()
                                        .header(PaneHeader::new("worktrees").total(12))
                                        .body(filler(&t, 4)),
                                )
                                .trailing(
                                    Pane::new()
                                        .raised(true)
                                        .header(PaneHeader::new("detail"))
                                        .body(filler(&t, 3)),
                                ),
                        ),
                ),
            ),
            specimen(
                "vertical · no divider · one side empty (the hairline zero-suppresses)",
                &t,
                stage(
                    &t,
                    px(120.0),
                    SplitLayout::vertical()
                        .leading_size(px(48.0))
                        .leading(filler(&t, 2))
                        .trailing(filler(&t, 3))
                        .divider(false),
                ),
            ),
        ],
    )
}

fn context_bar_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let bar_stage = |bar: ContextBar| stage(&t, t.metrics.context_bar_h, bar);
    let tabs = || {
        [
            ContextTab::new("buk", 1),
            ContextTab::new("personal", 2),
            ContextTab::new("oss", 3),
        ]
    };
    let chips = |bar: ContextBar| {
        bar.chip(
            Chip::counter(Icon::LoaderCircle, 2)
                .tone(Tone::Warning)
                .spinning(true)
                .id("cb-jobs"),
        )
        .chip(Chip::counter(Icon::CircleDot, 3).tone(Tone::Success))
        .chip(Chip::counter(Icon::Moon, 5))
        // Zero-suppressed: passed on every frame, rendered only when it has something.
        .chip(Chip::counter(Icon::Flag, 0))
        .chip(Chip::counter(Icon::CircleQuestionMark, 1).tone(Tone::Warning))
        .chip(Chip::labeled(Icon::CircleArrowUp, "0.2.0"))
    };
    section(
        "ContextBar — 84 px inset · numbered tabs · 2 px accent underline · chips · daemon dot",
        &t,
        vec![
            specimen(
                "default · tab 1 active · every §2.3 chip passed, including the zero one",
                &t,
                bar_stage(chips(ContextBar::new(tabs()).active(0))),
            ),
            specimen(
                "third tab active · +3 overflow for contexts past nine",
                &t,
                bar_stage(ContextBar::new(tabs()).active(2).overflow(3)),
            ),
            specimen(
                "daemon degraded · the dot grows a labelled amber pill",
                &t,
                bar_stage(
                    ContextBar::new(tabs())
                        .active(0)
                        .daemon(DaemonState::Degraded)
                        .daemon_label("fleetd slow"),
                ),
            ),
            specimen(
                "daemon lost · red pill, and the label is mandatory reading",
                &t,
                bar_stage(
                    ContextBar::new(tabs())
                        .active(0)
                        .daemon(DaemonState::Lost)
                        .daemon_label("fleetd stopped"),
                ),
            ),
            specimen(
                "empty · the fact, then the key that fixes it (§3.13)",
                &t,
                bar_stage(
                    ContextBar::new([]).empty("No contexts yet.", "N  create your first context"),
                ),
            ),
            specimen(
                "12 px inset · the platform with no traffic lights",
                &t,
                bar_stage(ContextBar::new(tabs()).active(1).leading_inset(t.space.md)),
            ),
        ],
    )
}

fn status_bar_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let bar_stage = |bar: StatusBar| stage(&t, t.metrics.status_bar_h, bar);
    section(
        "StatusBar — breadcrumb · mode word (84 px, fixed) · ticker · sticky error",
        &t,
        vec![
            specimen(
                "hub, idle",
                &t,
                bar_stage(
                    StatusBar::new()
                        .breadcrumb("buk › payroll › feat/payroll-fix")
                        .mode(Mode::Normal),
                ),
            ),
            specimen(
                "with the job ticker",
                &t,
                bar_stage(
                    StatusBar::new()
                        .breadcrumb("buk › payroll › feat/payroll-fix")
                        .mode(Mode::Normal)
                        .ticker(JobTicker::new("clone", "nixos").percent(40).extra(1)),
                ),
            ),
            specimen(
                "with a sticky error · the error replaces the ticker, it never joins it",
                &t,
                bar_stage(
                    StatusBar::new()
                        .breadcrumb("buk › payroll › feat/payroll-fix")
                        .mode(Mode::Normal)
                        .ticker(JobTicker::new("clone", "nixos").percent(40))
                        .error(StickyErrorSlot::new("prs failed: gh HTTP 502").key("!")),
                ),
            ),
            specimen(
                "workspace · breadcrumb is the session name, mode is TERMINAL, daemon dot trails",
                &t,
                bar_stage(
                    StatusBar::new()
                        .breadcrumb("buk › payroll › feat/payroll-fix › nvim")
                        .breadcrumb_ch(48)
                        .mode(Mode::Terminal)
                        .trailing(DaemonDot::new(DaemonState::Healthy)),
                ),
            ),
            specimen(
                "long breadcrumb, middle-truncated at a ch budget",
                &t,
                bar_stage(
                    StatusBar::new()
                        .breadcrumb(
                            "buk › dannyfuf/fleetd-experiments › spike/gpui-vt-mirror-grid › nvim",
                        )
                        .breadcrumb_ch(40)
                        .mode(Mode::Scroll),
                ),
            ),
        ],
    )
}

fn pane_section(cx: &mut App, focused_pane: usize) -> AnyElement {
    let t = cx.theme().clone();
    section(
        "Pane / PaneHeader — the focus ring, the scroll thumb and the header in place",
        &t,
        vec![
            specimen(
                "three panes, one focused — press f to move the ring (only one pane is ever focused)",
                &t,
                stage(
                    &t,
                    px(150.0),
                    div()
                        .flex()
                        .size_full()
                        .child(
                            Pane::fixed(px(180.0))
                                .raised(true)
                                .border(PaneBorder::Right)
                                .focused(focused_pane == 0)
                                .header(PaneHeader::new("repos").total(6))
                                .body(filler(&t, 4)),
                        )
                        .child(
                            Pane::new()
                                .focused(focused_pane == 1)
                                .header(
                                    PaneHeader::new("worktrees")
                                        .scope("payroll")
                                        .total(12)
                                        .range(1, 8),
                                )
                                .body(filler(&t, 4))
                                .scroll_thumb(0.0, 0.66),
                        )
                        .child(
                            Pane::fixed(px(200.0))
                                .raised(true)
                                .border(PaneBorder::Left)
                                .focused(focused_pane == 2)
                                .header(PaneHeader::new("detail"))
                                .body(filler(&t, 3)),
                        ),
                ),
            ),
            specimen(
                "scroll thumb: at the top, halfway, and suppressed when the content fits",
                &t,
                stage(
                    &t,
                    px(120.0),
                    div()
                        .flex()
                        .size_full()
                        .child(
                            Pane::new()
                                .border(PaneBorder::Right)
                                .header(PaneHeader::new("top").total(40).range(1, 8))
                                .body(filler(&t, 3))
                                .scroll_thumb(0.0, 0.2),
                        )
                        .child(
                            Pane::new()
                                .border(PaneBorder::Right)
                                .header(PaneHeader::new("middle").total(40).range(17, 24))
                                .body(filler(&t, 3))
                                .scroll_thumb(0.4, 0.2),
                        )
                        .child(
                            Pane::new()
                                .header(PaneHeader::new("fits").total(3).range(1, 3))
                                .body(filler(&t, 3))
                                .scroll_thumb(0.0, 1.0),
                        ),
                ),
            ),
            specimen(
                "with a pinned footer (the Jobs panel's key rows)",
                &t,
                stage(
                    &t,
                    px(140.0),
                    Pane::new()
                        .raised(true)
                        .header(PaneHeader::new("jobs").total(4))
                        .body(filler(&t, 3))
                        .footer(
                            div()
                                .flex()
                                .items_center()
                                .h(px(24.0))
                                .px(t.space.lg)
                                .child(
                                    KeyHintRow::new()
                                        .key("⏎", "log")
                                        .key("c", "cancel")
                                        .key("D", "dismiss"),
                                ),
                        ),
                ),
            ),
            specimen(
                "PaneHeader states: normal · filtering in place · filter retained · stale · no match",
                &t,
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(t.space.sm)
                    .child(stage(
                        &t,
                        t.metrics.pane_header_h,
                        PaneHeader::new("worktrees")
                            .scope("payroll")
                            .total(12)
                            .range(1, 8),
                    ))
                    .child(stage(
                        &t,
                        t.metrics.pane_header_h,
                        PaneHeader::new("worktrees")
                            .shown(2)
                            .total(12)
                            .filter(FilterBar::new("rut", 2, 12).focused(true)),
                    ))
                    .child(stage(
                        &t,
                        t.metrics.pane_header_h,
                        PaneHeader::new("worktrees")
                            .scope("payroll")
                            .filter_chip("rut")
                            .shown(2)
                            .total(12)
                            .range(1, 2),
                    ))
                    .child(stage(
                        &t,
                        t.metrics.pane_header_h,
                        PaneHeader::new("worktrees")
                            .scope("payroll")
                            .total(12)
                            .range(1, 8)
                            .stale("2m"),
                    ))
                    .child(stage(
                        &t,
                        t.metrics.pane_header_h,
                        PaneHeader::new("worktrees")
                            .scope("payroll")
                            .shown(0)
                            .total(12),
                    )),
            ),
        ],
    )
}

fn mode_and_daemon_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let words = Mode::ALL
        .iter()
        .map(|mode| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(t.space.xxs)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(t.metrics.status_bar_h)
                        .rounded(t.radii.sm)
                        .bg(t.colors.surface)
                        .child(ModeWord::new(*mode)),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

    let dots = DaemonState::ALL
        .iter()
        .flat_map(|state| {
            let bare = DaemonDot::new(*state);
            let labelled = DaemonDot::new(*state).label(match state {
                DaemonState::Healthy => "fleetd running",
                DaemonState::Degraded => "fleetd slow",
                DaemonState::Lost => "fleetd stopped",
            });
            [
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(Text::hint("default").faint())
                    .child(bare)
                    .into_any_element(),
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(Text::hint("labelled").faint())
                    .child(labelled)
                    .into_any_element(),
            ]
        })
        .collect::<Vec<_>>();

    section(
        "ModeWord · DaemonDot",
        &t,
        vec![
            specimen(
                "all eight modes at the fixed 84 px — only ^S is amber, because only it expires",
                &t,
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(t.space.sm)
                    .children(words),
            ),
            specimen(
                "healthy is a dot and nothing else; degraded and lost always read as words",
                &t,
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(t.space.lg)
                    .children(dots),
            ),
        ],
    )
}

fn banner_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let banner_stage = |banner: Banner| stage(&t, t.metrics.banner_h, banner);
    section(
        "Banner — §3.12 case C only",
        &t,
        vec![
            specimen(
                "daemon died while attached · the countdown lives in its own slot so the sentence never reflows",
                &t,
                banner_stage(
                    Banner::warning("fleetd stopped")
                        .icon(Icon::Dot)
                        .countdown("reconnecting in 3s")
                        .hints(
                            KeyHintRow::new()
                                .key("r", "reconnect now")
                                .key("l", "log")
                                .key("esc", "dismiss"),
                        ),
                ),
            ),
            specimen(
                "on reconnect · [D-17], verbatim: the sessions did not come back",
                &t,
                banner_stage(Banner::warning(RECONNECT_TEXT).icon(Icon::TriangleAlert)),
            ),
            specimen(
                "danger · the connection is gone and no retry is scheduled",
                &t,
                banner_stage(
                    Banner::danger("fleetd unreachable — the socket is gone")
                        .hints(KeyHintRow::new().key("r", "retry").key("D", "doctor")),
                ),
            ),
            specimen(
                "inside the Workspace every key carries its prefix (§3.6 [D-8])",
                &t,
                banner_stage(
                    Banner::warning("fleetd stopped")
                        .icon(Icon::Dot)
                        .countdown("reconnecting…")
                        .hints(
                            KeyHintRow::new()
                                .key("^s r", "reconnect")
                                .key("^s l", "log"),
                        ),
                ),
            ),
        ],
    )
}

fn veil_section(cx: &mut App, veiled: bool) -> AnyElement {
    let t = cx.theme().clone();
    section(
        "Veil — 55 % over terminal grids only, and keys are dropped, not buffered",
        &t,
        vec![specimen(
            if veiled {
                "active (press v) · the grid is veiled; a list beside it stays at 100 % and stays navigable"
            } else {
                "inactive (press v) · zero cost when the daemon is healthy"
            },
            &t,
            stage(
                &t,
                px(120.0),
                div()
                    .flex()
                    .size_full()
                    .child(
                        div().flex().w(px(260.0)).flex_none().h_full().child(
                            Pane::new()
                                .header(PaneHeader::new("worktrees").total(12).stale("2m"))
                                .body(filler(&t, 3)),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(Veil::new(veiled).child(fake_terminal(&t))),
                    ),
            ),
        )],
    )
}

fn sheet_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    section(
        "Sheet — right-docked, non-blocking, 440 / 640",
        &t,
        vec![
            specimen(
                "440 px over a list that stays fully readable — that is why it is not a modal",
                &t,
                stage(
                    &t,
                    px(200.0),
                    div()
                        .relative()
                        .size_full()
                        .child(
                            Pane::new()
                                .header(PaneHeader::new("worktrees").total(12).range(1, 8))
                                .body(filler(&t, 6)),
                        )
                        .child(
                            Sheet::new(true)
                                .header(sheet_header(&t))
                                .body(filler(&t, 4))
                                .footer(sheet_footer(&t)),
                        ),
                ),
            ),
            specimen(
                "640 px expanded, for the inline log tail",
                &t,
                stage(
                    &t,
                    px(200.0),
                    div()
                        .relative()
                        .size_full()
                        .child(
                            Pane::new()
                                .header(PaneHeader::new("worktrees").total(12).range(1, 8))
                                .body(filler(&t, 6)),
                        )
                        .child(
                            Sheet::new(true)
                                .expanded(true)
                                .header(sheet_header(&t))
                                .body(filler(&t, 5))
                                .footer(sheet_footer(&t)),
                        ),
                ),
            ),
            specimen(
                "closed · renders nothing at all",
                &t,
                stage(
                    &t,
                    px(80.0),
                    div()
                        .relative()
                        .size_full()
                        .child(filler(&t, 2))
                        .child(Sheet::new(false).body(filler(&t, 2))),
                ),
            ),
        ],
    )
}

fn sheet_header(t: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(t.metrics.pane_header_h)
                .px(t.space.lg)
                .child(Text::label("jobs"))
                .child(Text::label("⟳2 running · ✕1 failed · ✓5 done").faint()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(t.metrics.strip_h)
                .px(t.space.lg)
                .child(Text::data_small("~/.fleet/logs/jobs/j-8f3c.log"))
                .child(KeyHint::labeled("y", "copy")),
        )
        .into_any_element()
}

fn sheet_footer(t: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .h(px(24.0))
        .px(t.space.lg)
        .child(
            KeyHintRow::new()
                .key("⏎", "log")
                .key("c", "cancel")
                .key("R", "retry")
                .key("esc", "close"),
        )
        .into_any_element()
}

fn dialog_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    section(
        "Dialog — scrim + card + 44 header + 44 footer, and never a button pair",
        &t,
        vec![
            specimen(
                "compact confirm, 480 px · the hint row is the affordance",
                &t,
                stage(
                    &t,
                    px(230.0),
                    div().relative().size_full().child(filler(&t, 5)).child(
                        Dialog::new("Delete feat/payroll-fix?")
                            .icon(Icon::Trash)
                            .width(px(480.0))
                            .body(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(t.space.xs)
                                    .child(Text::ui("✓ no uncommitted changes"))
                                    .child(Text::ui("The worktree and its branch are removed. This cannot be undone.").muted()),
                            )
                            .hints(KeyHintRow::new().key("n / esc", "cancel").key("I", "re-check"))
                            .primary("y  Delete"),
                    ),
                ),
            ),
            specimen(
                "expanded confirm, 560 px, amber tone · error line pinned above the hints",
                &t,
                stage(
                    &t,
                    px(250.0),
                    div().relative().size_full().child(filler(&t, 5)).child(
                        Dialog::new("Delete feat/rut-validator?")
                            .icon(Icon::TriangleAlert)
                            .subtitle("· buk/payroll")
                            .tone(Tone::Warning)
                            .body(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(t.space.xs)
                                    .child(Text::ui("⚠ 3 uncommitted files"))
                                    .child(Text::ui("⚠ branch is not merged"))
                                    .child(Text::ui("checked 14s ago · I re-check").muted()),
                            )
                            .error("git: unable to remove worktree — it is locked")
                            .hints(KeyHintRow::new().key("n / esc", "cancel"))
                            .primary("Y  Delete"),
                    ),
                ),
            ),
        ],
    )
}

fn layers_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    section(
        "Floating layers — live on this window",
        &t,
        vec![specimen(
            "the paint order is OverlayLayer: sheet (100) → palette (200) → dialog (300) → toasts (400)",
            &t,
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(t.space.sm)
                .p(t.space.lg)
                .rounded(t.radii.sm)
                .bg(t.colors.surface)
                .border_1()
                .border_color(t.colors.border)
                .child(
                    KeyHintRow::new()
                        .key("j", "jobs sheet")
                        .key("x", "expand it to 640"),
                )
                .child(
                    KeyHintRow::new()
                        .key("p", "palette overlay (no scrim: a jump, not a decision)"),
                )
                .child(
                    KeyHintRow::new()
                        .key("d", "dialog (scrim: it ghosts the base screen)")
                        .key("e", "its error line"),
                )
                .child(KeyHintRow::new().key("o", "toast stack, including a coalesced ×3"))
                .child(
                    KeyHintRow::new()
                        .key("b", "banner")
                        .key("v", "veil")
                        .key("f", "focus ring")
                        .key("t", "theme"),
                ),
        )],
    )
}

// ------------------------------------------------------------------ live layers

fn palette_card(t: &Theme) -> AnyElement {
    let row = |label: &str, detail: &str, key: &str| {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(t.metrics.palette_row_h)
            .px(t.space.lg)
            .child(Text::ui(label.to_string()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .child(Text::ui(detail.to_string()).muted())
                    .child(Text::hint(key.to_string()).faint()),
            )
            .into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .h(t.metrics.dialog_header_h)
                .px(t.space.lg)
                .gap(t.space.sm)
                .border_b(px(1.0))
                .border_color(t.colors.border)
                .child(Text::hint(":"))
                .child(Text::ui("pay fix")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .h(t.metrics.section_header_h)
                .px(t.space.lg)
                .child(Text::label("go")),
        )
        .child(row("payroll#feat-payroll-fix", "session attached", ""))
        .child(row("payroll#fix-rut-validator", "sleeping", ""))
        .child(
            div()
                .flex()
                .items_center()
                .h(t.metrics.section_header_h)
                .px(t.space.lg)
                .child(Text::label("do")),
        )
        .child(row("Prune worktrees · buk/payroll", "", "x"))
        .child(
            div()
                .flex()
                .items_center()
                .h(px(24.0))
                .px(t.space.lg)
                .border_t(px(1.0))
                .border_color(t.colors.border)
                .child(
                    KeyHintRow::new()
                        .key("9 of 63 · ⏎", "run")
                        .key("esc", "cancel"),
                ),
        )
        .into_any_element()
}

fn live_toasts() -> Vec<Toast> {
    let mut toasts = Vec::new();
    ToastStack::push_at(
        &mut toasts,
        Toast::new("Slept · kept cc (claude)").icon(Icon::Moon),
        ToastStack::MAX,
        0,
    );
    ToastStack::push_at(
        &mut toasts,
        Toast::new("Cloned buk/ledger · ⏎ opens").icon(Icon::CircleCheck),
        ToastStack::MAX,
        10,
    );
    // Three identical clipboard acknowledgements inside the one-second window collapse into
    // a single `×3` toast — the §2.7 coalescing law, exercised rather than described.
    for at in [20, 300, 700] {
        ToastStack::push_at(
            &mut toasts,
            Toast::new("Path copied")
                .icon(Icon::ClipboardCheck)
                .short()
                .raised_at(at),
            ToastStack::MAX,
            at,
        );
    }
    toasts
}

impl Render for StructureGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let focused_pane = self.focused_pane;
        let veiled = self.veil;

        let sections = vec![
            app_frame_section(cx),
            split_section(cx),
            context_bar_section(cx),
            status_bar_section(cx),
            pane_section(cx, focused_pane),
            mode_and_daemon_section(cx),
            banner_section(cx),
            veil_section(cx, veiled),
            sheet_section(cx),
            dialog_section(cx),
            layers_section(cx),
        ];

        let mut frame = AppFrame::new()
            .context_bar(
                ContextBar::new([ContextTab::new("structure", 1)])
                    .active(0)
                    .chip(Chip::labeled(
                        if t.mode.is_dark() {
                            Icon::Moon
                        } else {
                            Icon::CircleArrowUp
                        },
                        if t.mode.is_dark() { "dark" } else { "light" },
                    ))
                    .daemon(if self.banner {
                        DaemonState::Lost
                    } else {
                        DaemonState::Healthy
                    })
                    .daemon_label("fleetd stopped"),
            )
            .body(
                div()
                    .id("structure-gallery-scroll")
                    .track_focus(&self.focus_handle)
                    .key_context("StructureGallery")
                    .on_action(cx.listener(Self::toggle_theme))
                    .on_action(cx.listener(Self::toggle_banner))
                    .on_action(cx.listener(Self::toggle_dialog))
                    .on_action(cx.listener(Self::toggle_dialog_error))
                    .on_action(cx.listener(Self::toggle_palette))
                    .on_action(cx.listener(Self::toggle_sheet))
                    .on_action(cx.listener(Self::toggle_sheet_expanded))
                    .on_action(cx.listener(Self::toggle_toasts))
                    .on_action(cx.listener(Self::toggle_veil))
                    .on_action(cx.listener(Self::cycle_focus))
                    .on_action(cx.listener(Self::quit))
                    .size_full()
                    .overflow_y_scroll()
                    .p(t.space.xl)
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .status_bar(
                StatusBar::new()
                    .breadcrumb("fleet-ui-kit › structure")
                    .mode(if self.dialog {
                        Mode::Dialog
                    } else if self.palette {
                        Mode::Palette
                    } else if self.sheet {
                        Mode::Jobs
                    } else {
                        Mode::Normal
                    })
                    .ticker(
                        KeyHintRow::new()
                            .key("t", "theme")
                            .key("b d e p j x o v f", "layers")
                            .key("q", "quit"),
                    ),
            );

        if self.banner {
            frame = frame.banner(
                Banner::warning("fleetd stopped")
                    .icon(Icon::Dot)
                    .countdown("reconnecting in 3s")
                    .hints(
                        KeyHintRow::new()
                            .key("r", "reconnect now")
                            .key("l", "log")
                            .key("esc", "dismiss"),
                    ),
            );
        }
        if self.sheet {
            frame = frame.body_overlay(
                Sheet::new(true)
                    .expanded(self.sheet_expanded)
                    .header(sheet_header(&t))
                    .body(filler(&t, 6))
                    .footer(sheet_footer(&t)),
            );
        }
        if self.toasts {
            frame = frame.body_overlay(ToastStack::new(live_toasts()));
        }
        if self.palette {
            frame = frame.overlay(Overlay::new().child(palette_card(&t)));
        }
        if self.dialog {
            let mut dialog = Dialog::new("New worktree")
                .icon(Icon::GitBranchPlus)
                .subtitle("· buk/payroll")
                .body(
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.sm)
                        .child(
                            TextField::new("feat/rut-validator")
                                .label("Branch")
                                .caret(18)
                                .focused(true),
                        )
                        .child(Text::ui("~/.fleet/worktrees/payroll/feat-rut-validator").muted()),
                )
                .hints(
                    KeyHintRow::new()
                        .key("esc", "cancel")
                        .key("^n / ^p", "base"),
                )
                .primary("⏎  Create");
            if self.dialog_error {
                dialog = dialog.error("branch feat/rut-validator already exists in buk/payroll");
            }
            frame = frame.overlay(dialog);
        }

        frame
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(|cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            cx.bind_keys([
                KeyBinding::new("t", ToggleTheme, None),
                KeyBinding::new("b", ToggleBanner, None),
                KeyBinding::new("d", ToggleDialog, None),
                KeyBinding::new("e", ToggleDialogError, None),
                KeyBinding::new("p", TogglePalette, None),
                KeyBinding::new("j", ToggleSheet, None),
                KeyBinding::new("x", ToggleSheetExpanded, None),
                KeyBinding::new("o", ToggleToasts, None),
                KeyBinding::new("v", ToggleVeil, None),
                KeyBinding::new("f", CycleFocus, None),
                KeyBinding::new("q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
            ]);
            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            cx.set_menus(vec![Menu {
                name: "fleet-ui-kit".into(),
                items: vec![MenuItem::action("Quit", Quit)],
                disabled: false,
            }]);
            cx.on_window_closed(|cx: &mut App, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1280.0), px(860.0)), cx);
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("fleet-ui-kit · structure".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_window, cx| {
                    let view: Entity<StructureGallery> = cx.new(StructureGallery::new);
                    view
                },
            );

            match opened {
                Ok(window) => {
                    window
                        .update(cx, |view, window, cx| {
                            window.focus(&view.focus_handle(cx), cx);
                        })
                        .ok();
                    cx.activate(true);
                }
                Err(error) => {
                    eprintln!("failed to open the structure gallery window: {error}");
                    cx.quit();
                }
            }
        });
}
