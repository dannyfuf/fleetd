//! The visual test bench for `fleet-ui-kit`.
//!
//! Every component, in every state, in both themes. `t` toggles light/dark, `q` / `cmd-q`
//! quits. This example is the acceptance test for a component change: if a state is not
//! visible here, it is not implemented.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example kit_gallery
//! ```

use fleet_ui_kit::prelude::*;
use fleet_ui_kit::KitAssets;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, Menu, MenuItem,
    SharedString, TitlebarOptions, UniformListScrollHandle, Window, WindowBounds, WindowOptions,
    actions, div, px, size,
};

actions!(kit_gallery, [ToggleTheme, Quit]);

struct Gallery {
    focus_handle: FocusHandle,
    list_scroll: UniformListScrollHandle,
    cursor: usize,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            list_scroll: UniformListScrollHandle::new(),
            cursor: 1,
        }
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
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

// ---------------------------------------------------------------- layout helpers

fn section(title: &str, t: &Theme, children: Vec<AnyElement>) -> AnyElement {
    let theme = t;
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.md)
        .pb(theme.space.xl)
        .child(SectionHeader::new(title.to_string()))
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(theme.space.md)
                .children(children),
        )
        .into_any_element()
}

fn labeled(label: &str, t: &Theme, child: impl IntoElement) -> AnyElement {
    let theme = t;
    div()
        .flex()
        .items_start()
        .w_full()
        .gap(theme.space.md)
        .child(Text::hint(label.to_string()).faint().w(px(150.0)))
        .child(div().flex().flex_1().min_w_0().items_center().child(child))
        .into_any_element()
}

fn strip(t: &Theme, children: Vec<AnyElement>) -> AnyElement {
    let theme = t;
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme.space.md)
        .children(children)
        .into_any_element()
}

fn box_of(t: &Theme, height: gpui::Pixels, child: impl IntoElement) -> AnyElement {
    let theme = t;
    div()
        .relative()
        .w_full()
        .h(height)
        .rounded(theme.radii.sm)
        .bg(theme.colors.surface)
        .border_1()
        .border_color(theme.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

// ---------------------------------------------------------------- sections

fn colors_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let swatch = |name: &'static str, color: gpui::Hsla| {
        div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .w(px(104.0))
            .child(
                div()
                    .h(px(28.0))
                    .w_full()
                    .rounded(t.radii.sm)
                    .bg(color)
                    .border_1()
                    .border_color(t.colors.border),
            )
            .child(Text::hint(name).faint())
            .into_any_element()
    };
    let c = &t.colors;
    let roles = vec![
        swatch("bg", c.bg),
        swatch("surface", c.surface),
        swatch("elevated", c.elevated),
        swatch("row_selected", c.row_selected),
        swatch("border", c.border),
        swatch("text", c.text),
        swatch("text_secondary", c.text_secondary),
        swatch("text_muted", c.text_muted),
        swatch("accent", c.accent),
        swatch("success", c.success),
        swatch("warning", c.warning),
        swatch("danger", c.danger),
        swatch("info", c.info),
        swatch("focus_ring", c.focus_ring),
        swatch("selection", c.selection),
        swatch("skeleton", c.skeleton),
    ];
    let ansi: Vec<AnyElement> = (0u8..16)
        .map(|ix| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(t.space.xxs)
                .w(px(40.0))
                .child(
                    div()
                        .size(px(24.0))
                        .rounded(t.radii.xs)
                        .bg(t.terminal.color(ix))
                        .border_1()
                        .border_color(t.colors.border),
                )
                .child(Text::hint(ix.to_string()).faint())
                .into_any_element()
        })
        .collect();

    let children = vec![
        labeled("color roles", &t, strip(&t, roles)),
        labeled("terminal ansi", &t, strip(&t, ansi)),
        labeled(
            "terminal default",
            &t,
            strip(&t,
                vec![
                    swatch("fg", t.terminal.foreground),
                    swatch("bg", t.terminal.background),
                    swatch("cursor", t.terminal.cursor),
                ],
            ),
        ),
    ];
    section("colors", &t, children)
}

fn type_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let children = vec![
        labeled("ui 13/18", &t, Text::ui("feat/payroll-fix — the branch you think in")),
        labeled("ui_strong 13/18", &t, Text::ui_strong("Fix RUT validation on payroll import")),
        labeled("title 15/20", &t, Text::title("New worktree")),
        labeled("data 12.5/18", &t, Text::data("~/.fleet/worktrees/buk/payroll/feat-payroll-fix")),
        labeled("data_small 11.5/16", &t, Text::data_small("Receiving objects: 40% (81/202)")),
        labeled("label 11/14", &t, Text::label("worktrees")),
        labeled("hint mono 11/14", &t, Text::hint("⏎ open · esc cancel")),
        labeled(
            "truncate head/middle/tail",
            &t,
            strip(&t,
                vec![
                    Text::data(truncate("dannyfuf/fleetd", 10, Truncate::Head)).into_any_element(),
                    Text::data(truncate("feat/payroll-fix", 10, Truncate::Middle)).into_any_element(),
                    Text::data(truncate("Fix RUT validation", 10, Truncate::Tail)).into_any_element(),
                ],
            ),
        ),
    ];
    section("type", &t, children)
}

fn icons_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let icons: Vec<AnyElement> = Icon::ALL
        .iter()
        .map(|icon| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(t.space.xxs)
                .w(px(88.0))
                .child(icon.el().size(IconSize::Large).color(t.colors.text))
                .child(Text::hint(icon.name()).faint())
                .into_any_element()
        })
        .collect();
    let sizes = strip(&t,
        vec![
            Icon::Moon.el().size(IconSize::Small).into_any_element(),
            Icon::Moon.el().size(IconSize::Medium).into_any_element(),
            Icon::Moon.el().size(IconSize::Large).into_any_element(),
            Spinner::new("gallery-spinner").into_any_element(),
        ],
    );
    let children = vec![
        labeled("sizes 12 / 14 / 16 + spinner", &t, sizes),
        strip(&t, icons),
    ];
    section("icons (lucide, stroke 1.5)", &t, children)
}

fn glyphs_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let kinds = [
        StatusKind::Attached,
        StatusKind::DetachedAwake,
        StatusKind::Sleeping,
        StatusKind::NoSession,
        StatusKind::Unknown,
        StatusKind::Degraded,
        StatusKind::JobRunning,
        StatusKind::Cloning,
        StatusKind::CloneFailed,
        StatusKind::HostUnreachable,
    ];
    let glyphs: Vec<AnyElement> = kinds
        .iter()
        .enumerate()
        .map(|(ix, kind)| {
            div()
                .flex()
                .items_center()
                .gap(t.space.xs)
                .w(px(190.0))
                .child(StatusGlyph::new(*kind).id(("glyph", ix)))
                .child(Text::ui(kind.detail_word()).muted())
                .into_any_element()
        })
        .collect();

    let pr_states = [
        PrBadgeState::Draft,
        PrBadgeState::CiFail,
        PrBadgeState::Changes,
        PrBadgeState::CiPending,
        PrBadgeState::Approved,
        PrBadgeState::Review,
        PrBadgeState::Merged,
    ];
    let badges: Vec<AnyElement> = pr_states
        .iter()
        .map(|state| PrBadge::new(412, *state).into_any_element())
        .collect();

    let children = vec![
        labeled("status glyphs", &t, strip(&t, glyphs)),
        labeled("pr badges", &t, strip(&t, badges)),
        labeled(
            "pr badge, stale (>10 min)",
            &t,
            PrBadge::new(412, PrBadgeState::CiFail).stale(true),
        ),
        labeled(
            "chips",
            &t,
            strip(&t,
                vec![
                    Chip::counter(Icon::LoaderCircle, 2)
                        .tone(Tone::Warning)
                        .spinning(true)
                        .id("chip-jobs")
                        .into_any_element(),
                    Chip::counter(Icon::CircleDot, 3).tone(Tone::Success).into_any_element(),
                    Chip::counter(Icon::Moon, 5).tone(Tone::Secondary).into_any_element(),
                    Chip::counter(Icon::CircleQuestionMark, 1).tone(Tone::Warning).into_any_element(),
                    Chip::counter(Icon::Flag, 4).tone(Tone::Secondary).into_any_element(),
                    Chip::counter(Icon::CircleArrowUp, 0).into_any_element(),
                    Chip::labeled(Icon::Cloud, "devbox").filled(true).into_any_element(),
                    Chip::labeled(Icon::TriangleAlert, "1 failed")
                        .tone(Tone::Danger)
                        .filled(true)
                        .into_any_element(),
                ],
            ),
        ),
        labeled(
            "badges",
            &t,
            strip(&t,
                vec![
                    Badge::new("default").into_any_element(),
                    Badge::new("current").style(BadgeStyle::Outlined).into_any_element(),
                    Badge::new("ready").tone(Tone::Success).style(BadgeStyle::Filled).into_any_element(),
                    Badge::new("danger").tone(Tone::Danger).style(BadgeStyle::Filled).into_any_element(),
                ],
            ),
        ),
        labeled(
            "status dots",
            &t,
            strip(&t,
                vec![
                    StatusDot::new(Tone::Success).into_any_element(),
                    StatusDot::new(Tone::Warning).into_any_element(),
                    StatusDot::new(Tone::Danger).into_any_element(),
                    StatusDot::small(Tone::Warning).into_any_element(),
                ],
            ),
        ),
        labeled(
            "daemon dot",
            &t,
            strip(&t,
                vec![
                    DaemonDot::new(DaemonState::Healthy).into_any_element(),
                    DaemonDot::new(DaemonState::Degraded)
                        .label("reconnecting")
                        .into_any_element(),
                    DaemonDot::new(DaemonState::Lost)
                        .label("fleetd stopped")
                        .into_any_element(),
                ],
            ),
        ),
        labeled(
            "keep-alive / degraded",
            &t,
            strip(&t,
                vec![
                    KeepAliveChips::new([
                        KeepAliveLabel::with_icon("claude", Icon::Bot),
                        KeepAliveLabel::with_icon(":3000", Icon::Server),
                        KeepAliveLabel::with_icon("nvim", Icon::FilePen),
                        KeepAliveLabel::new("vitest"),
                    ])
                    .width_ch(18.0)
                    .into_any_element(),
                    DegradedChip::hooks_failed().hint("J", "for log").into_any_element(),
                ],
            ),
        ),
        labeled(
            "age labels",
            &t,
            strip(&t,
                vec![
                    AgeLabel::from_secs(42).into_any_element(),
                    AgeLabel::from_secs(7_200).into_any_element(),
                    AgeLabel::from_secs(432_000).into_any_element(),
                    AgeLabel::none().into_any_element(),
                ],
            ),
        ),
        labeled(
            "freshness ladder",
            &t,
            strip(&t,
                vec![
                    FreshnessStamp::new("checked", 8).into_any_element(),
                    FreshnessStamp::new("checked", 240).into_any_element(),
                    FreshnessStamp::new("checked", 1_800)
                        .action("I", "re-check")
                        .into_any_element(),
                    FreshnessStamp::new("inspected", 60)
                        .error("gh: HTTP 502")
                        .into_any_element(),
                ],
            ),
        ),
        labeled(
            "mode words",
            &t,
            strip(&t,
                vec![
                    ModeWord::new(Mode::Normal).into_any_element(),
                    ModeWord::new(Mode::Terminal).into_any_element(),
                    ModeWord::new(Mode::Prefix).into_any_element(),
                    ModeWord::new(Mode::Scroll).into_any_element(),
                    ModeWord::new(Mode::Filter).into_any_element(),
                    ModeWord::new(Mode::Palette).into_any_element(),
                    ModeWord::new(Mode::Dialog).into_any_element(),
                    ModeWord::new(Mode::Jobs).into_any_element(),
                ],
            ),
        ),
        labeled(
            "key hints",
            &t,
            KeyHintRow::new()
                .key("⏎", "open")
                .key("^s x", "close")
                .key("esc", "cancel"),
        ),
        labeled("divider", &t, div().w_full().child(Divider::horizontal())),
    ];
    section("vocabulary", &t, children)
}

fn facts_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let compact = FactList::from_facts([
        Fact::safe("clean"),
        Fact::safe("merged into origin/main"),
        Fact::safe("no session"),
    ]);
    let expanded = FactList::from_facts([
        Fact::risk("12 uncommitted files"),
        Fact::risk("3 commits not on origin/main"),
        Fact::risk("session attached · claude, :3000 running"),
        Fact::unknown("unique commit count unavailable (gh unavailable)"),
        Fact::safe("PR #412 open (not merged)"),
    ]);
    let children = vec![
        labeled(
            "key/value list",
            &t,
            KeyValueList::titled("safety")
                .trailing(FreshnessStamp::new("checked", 14).action("I", "re-check"))
                .row("dirty", FactValue::known("12 files"))
                .row("ahead / behind", FactValue::known("⇡3 ⇣0"))
                .row("unique commits", FactValue::Null)
                .row("published", FactValue::known("no"))
                .mono_row("path", FactValue::known("~/.fleet/worktrees/buk/payroll/feat"))
                .row("warning", FactValue::warning("gh unavailable")),
        ),
        labeled("fact list · compact (y)", &t, compact),
        labeled("fact list · expanded (Y)", &t, expanded),
        labeled(
            "doctor table",
            &t,
            DoctorTable::new([
                DoctorRow::new("git", DoctorStatus::Ok, "git version 2.49.0"),
                DoctorRow::new("gh auth", DoctorStatus::Fail, "gh: not logged in to github.com"),
                DoctorRow::new("copy-on-write", DoctorStatus::Ok, "cp -c (APFS clonefile)"),
                DoctorRow::new("host devbox", DoctorStatus::Warn, "ssh: slow handshake (2.1s)"),
            ]),
        ),
        labeled("empty state", &t, box_of(&t, px(72.0), EmptyState::new("No worktrees yet.").action("n  create one"))),
        labeled("skeleton rows", &t, box_of(&t, px(120.0), SkeletonRows::new(4))),
    ];
    section("facts and tables", &t, children)
}

fn rows_section(cx: &mut App, cursor: usize, scroll: &UniformListScrollHandle) -> AnyElement {
    let t = cx.theme().clone();
    let sample_row = |glyph: StatusKind, branch: &'static str, ix: usize| {
        Row::with_id(("row", ix))
            .leading(StatusGlyph::new(glyph).id(("row-glyph", ix)))
            .column(RowColumn::flex(Text::ui(branch)))
            .column(RowColumn::fixed(
                fleet_ui_kit::theme::ch(14.0),
                Text::ui("buk/payroll").muted(),
            ))
            .column(RowColumn::fixed(
                fleet_ui_kit::theme::ch(15.0),
                PrBadge::new(412, PrBadgeState::CiFail),
            ))
            .column(
                RowColumn::fixed(fleet_ui_kit::theme::ch(7.0), AgeLabel::from_secs(7_200))
                    .align(ColumnAlign::Right),
            )
    };

    let states = box_of(&t,
        px(160.0),
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(sample_row(StatusKind::Attached, "feat/payroll-fix", 0))
            .child(
                sample_row(StatusKind::Sleeping, "fix/rut-validator", 1)
                    .selected(true)
                    .cursor(true),
            )
            .child(sample_row(StatusKind::NoSession, "spike/gpui-vt", 2).dimmed(true))
            .child(sample_row(StatusKind::Unknown, "api-poc", 3).disabled(true))
            .child(
                Row::with_id(("row", 4usize))
                    .height(t.metrics.job_row_h)
                    .leading(StatusGlyph::new(StatusKind::Cloning).id("row-glyph-4"))
                    .column(RowColumn::flex(Text::ui("nixos")))
                    .second_line(Text::data_small("Receiving objects: 40% (81/202)").faint()),
            ),
    );

    let list = box_of(&t,
        px(180.0),
        ListView::new("gallery-list", 24, move |ix, is_cursor, _window, _cx| {
            Row::with_id(("list-row", ix))
                .selected(is_cursor)
                .cursor(is_cursor)
                .leading(StatusGlyph::new(StatusKind::DetachedAwake).id(("list-glyph", ix)))
                .column(RowColumn::flex(Text::ui(format!("row {ix} — j/k moves the cursor"))))
                .column(
                    RowColumn::fixed(fleet_ui_kit::theme::ch(7.0), AgeLabel::from_secs(60 * ix as i64 + 30))
                        .align(ColumnAlign::Right),
                )
                .into_any_element()
        })
        .cursor(cursor)
        .track_scroll(scroll)
        .empty(EmptyState::new("Nothing matches.").action("esc  clear")),
    );

    let jobs = box_of(&t,
        px(320.0),
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                JobRow::new(JobStatus::Running, "clone", "nixos")
                    .id("job-0")
                    .elapsed("0:42")
                    .percent(40)
                    .progress("Receiving objects: 40% (81/202)")
                    .selected(true)
                    .cursor(true),
            )
            .child(
                JobRow::new(JobStatus::Running, "hooks", "buk/payroll#feat-rut")
                    .id("job-1")
                    .elapsed("0:08")
                    .progress("pnpm install (2/3)"),
            )
            .child(
                JobRow::new(JobStatus::Failed, "prs", "review")
                    .id("job-2")
                    .elapsed("1m")
                    .trailing_key("R"),
            )
            .child(JobRow::new(JobStatus::Done, "prune", "buk/www").id("job-3").elapsed("12s"))
            .child(JobRow::new(JobStatus::Cancelled, "fetch", "dannyfuf/fleetd").id("job-4"))
            .child(JobRow::new(JobStatus::Queued, "pool", "buk/payroll").id("job-5"))
            .child(
                JobRow::new(JobStatus::Cancelling, "delete", "buk/www#chore-deps")
                    .id("job-6")
                    .elapsed("0:03")
                    .progress("waiting for the worker to stop"),
            )
            // The quit-and-stop confirm (§3.8.9) labels every cancellable job.
            .child(
                JobRow::new(JobStatus::Running, "clone", "nixos")
                    .id("job-7")
                    .percent(40)
                    .retryable(true),
            )
            .child(
                JobRow::new(JobStatus::Running, "hooks", "payroll#feat-rut")
                    .id("job-8")
                    .retryable(false),
            ),
    );

    let children = vec![
        labeled("row states", &t, states),
        labeled("list view (virtualized)", &t, list),
        labeled("job rows", &t, jobs),
        labeled(
            "ticker / sticky error",
            &t,
            strip(&t,
                vec![
                    JobTicker::new("clone", "nixos").percent(40).extra(1).into_any_element(),
                    StickyErrorSlot::new("gh: HTTP 502 upstream connect error").into_any_element(),
                ],
            ),
        ),
        labeled(
            "column ladder (worktrees @ 138 / 93 / 60 ch)",
            &t,
            strip(&t,
                [138.0f32, 93.0, 60.0]
                    .iter()
                    .map(|w| {
                        let keys = ColumnLadder::worktrees()
                            .resolve(*w)
                            .iter()
                            .map(|c| c.key.to_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        Text::hint(format!("{w:.0}ch: {keys}")).faint().into_any_element()
                    })
                    .collect(),
            ),
        ),
    ];
    section("rows and lists", &t, children)
}

fn structure_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let pane = box_of(&t,
        px(150.0),
        SplitLayout::horizontal()
            .leading_size(px(200.0))
            .leading(
                Pane::fixed(px(200.0))
                    .border(PaneBorder::None)
                    .header(PaneHeader::new("repos").total(6))
                    .body(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                Row::new()
                                    .leading(StatusGlyph::new(StatusKind::Attached).id("rail-0"))
                                    .column(RowColumn::flex(Text::ui("All")))
                                    .column(RowColumn::auto(Text::ui("27").muted()))
                                    .selected(true)
                                    .cursor(true),
                            )
                            .child(
                                Row::new()
                                    .leading(StatusGlyph::new(StatusKind::Sleeping).id("rail-1"))
                                    .column(RowColumn::flex(Text::ui("payroll")))
                                    .column(RowColumn::auto(Text::ui("8").muted())),
                            ),
                    ),
            )
            .trailing(
                Pane::new()
                    .focused(true)
                    .header(
                        PaneHeader::new("worktrees")
                            .scope("payroll")
                            .shown(8)
                            .total(12)
                            .range(1, 8),
                    )
                    .scroll_thumb(0.0, 0.6)
                    .body(
                        div().flex().flex_col().child(
                            Row::new()
                                .leading(StatusGlyph::new(StatusKind::Attached).id("wt-0"))
                                .column(RowColumn::flex(Text::ui("feat/payroll-fix")))
                                .selected(true)
                                .cursor(true),
                        ),
                    ),
            ),
    );

    let filtered = box_of(&t,
        t.metrics.pane_header_h,
        PaneHeader::new("worktrees").filter(FilterBar::new("rut", 2, 12)),
    );
    let retained = box_of(&t,
        t.metrics.pane_header_h,
        PaneHeader::new("worktrees")
            .scope("payroll")
            .filter_chip("rut")
            .shown(2)
            .total(12)
            .range(1, 2),
    );
    let stale = box_of(&t,
        t.metrics.pane_header_h,
        PaneHeader::new("worktrees").scope("payroll").total(12).stale("2m"),
    );

    let bars = box_of(&t,
        t.metrics.context_bar_h,
        ContextBar::new([
            ContextTab::new("buk", 1),
            ContextTab::new("personal", 2),
            ContextTab::new("oss", 3),
        ])
        .active(0)
        .overflow(3)
        .chip(Chip::counter(Icon::LoaderCircle, 2).tone(Tone::Warning).spinning(true).id("cb-jobs"))
        .chip(Chip::counter(Icon::CircleDot, 3).tone(Tone::Success))
        .chip(Chip::counter(Icon::Moon, 5).tone(Tone::Secondary))
        .chip(Chip::counter(Icon::CircleQuestionMark, 1).tone(Tone::Warning))
        .chip(Chip::counter(Icon::Flag, 4).tone(Tone::Secondary))
        .daemon(DaemonState::Healthy),
    );

    let status = box_of(&t,
        t.metrics.status_bar_h,
        StatusBar::new()
            .breadcrumb("buk › payroll › feat/payroll-fix")
            .mode(Mode::Normal)
            .ticker(JobTicker::new("clone", "nixos").percent(40).extra(1)),
    );
    let status_error = box_of(&t,
        t.metrics.status_bar_h,
        StatusBar::new()
            .breadcrumb("buk › payroll › feat/payroll-fix")
            .mode(Mode::Terminal)
            .ticker(JobTicker::new("clone", "nixos"))
            .error(StickyErrorSlot::new("clone failed: gh: HTTP 502")),
    );

    let children = vec![
        labeled("context bar", &t, bars),
        labeled("status bar", &t, status),
        labeled("status bar · error", &t, status_error),
        labeled("panes + split", &t, pane),
        labeled("pane header · filtering", &t, filtered),
        labeled("pane header · retained", &t, retained),
        labeled("pane header · stale", &t, stale),
        labeled(
            "banner",
            &t,
            box_of(&t,
                t.metrics.banner_h,
                Banner::danger("fleetd stopped")
                    .countdown("reconnecting in 3s")
                    .hints(KeyHintRow::new().key("r", "reconnect").key("l", "log")),
            ),
        ),
    ];
    section("structure", &t, children)
}

fn terminal_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let mut rows = Vec::new();
    for (ix, line) in [
        "\u{276f} claude",
        "\u{23fa} Reading src/payroll/rounding.rb\u{2026}",
        "  \u{b7} 128 lines \u{b7} 3 matches",
    ]
    .iter()
    .enumerate()
    {
        let mut cells = Vec::new();
        for ch in line.chars() {
            let mut cell = GridCell::new(ch.to_string(), &t);
            if ix == 0 {
                cell.fg = t.terminal.ansi[2];
                cell.bold = true;
            } else if ix == 2 {
                cell.fg = t.terminal.ansi[8];
            }
            cells.push(cell);
        }
        rows.push(GridRow::new(cells));
    }

    let grid = box_of(
        &t,
        px(96.0),
        TerminalGrid::new(rows)
            .cursor(GridCursor {
                row: 1,
                col: 34,
                visible: true,
                shape: CursorShape::Block,
            })
            .selection(GridSelection::new(2, 2, 2, 18))
            .scrollback(412, 2000),
    );

    // Every cell attribute, so a regression in the proto -> kit conversion is visible here.
    let attr_cell = |text: &str, f: fn(GridCell) -> GridCell| f(GridCell::new(text.to_string(), &t));
    let attrs_row = GridRow::new([
        attr_cell("bold ", |c| c.bold(true)),
        attr_cell("dim ", |c| c.dim(true)),
        attr_cell("italic ", |c| c.italic(true)),
        attr_cell("under ", |c| c.underline(UnderlineStyle::Single)),
        attr_cell("double ", |c| c.underline(UnderlineStyle::Double)),
        attr_cell("curly ", |c| c.underline(UnderlineStyle::Curly)),
        attr_cell("strike ", |c| c.strikethrough(true)),
        attr_cell("blink ", |c| c.blink(true)),
        attr_cell("hidden ", |c| c.invisible(true)),
    ]);
    let attrs_row_2 = GridRow::new([
        attr_cell("inverse", |c| c.inverse(true)),
        GridCell::new(" ", &t),
        GridCell::new("err", &t)
            .underline(UnderlineStyle::Curly)
            .underline_color(t.colors.danger),
        GridCell::new(" ", &t),
        GridCell::new("\u{5e83}", &t).width(CellWidth::Wide),
        GridCell::new("", &t).width(CellWidth::Spacer),
        GridCell::new("\u{3044}", &t).width(CellWidth::Wide),
        GridCell::new("", &t).width(CellWidth::Spacer),
        GridCell::new(" wide + spacer", &t),
    ]);
    let attrs = box_of(
        &t,
        px(56.0),
        TerminalGrid::new([attrs_row, attrs_row_2]),
    );

    let unfocused = box_of(
        &t,
        px(38.0),
        TerminalGrid::new([GridRow::new(
            "hollow cursor \u{2014} terminal not focused"
                .chars()
                .map(|c| GridCell::new(c.to_string(), &t)),
        )])
        .focused(false)
        .cursor(GridCursor {
            row: 0,
            col: 14,
            visible: true,
            shape: CursorShape::Block,
        }),
    );

    let cursor_shapes = box_of(
        &t,
        px(38.0),
        div()
            .flex()
            .size_full()
            .bg(t.terminal.background)
            .children([CursorShape::Block, CursorShape::Bar, CursorShape::Underline].map(
                |shape| {
                    div().w(px(120.0)).h_full().child(
                        TerminalGrid::new([GridRow::new(
                            "  shape".chars().map(|c| GridCell::new(c.to_string(), &t)),
                        )])
                        .cursor(GridCursor {
                            row: 0,
                            col: 0,
                            visible: true,
                            shape,
                        }),
                    )
                },
            )),
    );

    let strip_el = box_of(
        &t,
        t.metrics.pane_header_h,
        TerminalTabStrip::new([
            TerminalTab::new(1, "nvim").keep_alive(Icon::FilePen),
            TerminalTab::new(2, "cc").keep_alive(Icon::Bot).activity(true),
            TerminalTab::new(3, "lg"),
            TerminalTab::new(4, "test").exited(1),
            TerminalTab::new(5, "server").exited(None),
            TerminalTab::new(6, "waking").starting(true),
        ])
        .active(1),
    );

    let overlays = box_of(
        &t,
        px(120.0),
        div()
            .relative()
            .size_full()
            .bg(t.terminal.background)
            .child(ScrollPill::new(412, 2000).selecting(true))
            .child(
                PrefixHint::new(true).hints(
                    KeyHintRow::new()
                        .key("s", "hub")
                        .key("1-9", "tab")
                        .key("c", "new")
                        .key("x", "close"),
                ),
            ),
    );

    let badge = box_of(
        &t,
        px(46.0),
        div()
            .relative()
            .size_full()
            .bg(t.terminal.background)
            .child(ScrollbackBadge::new(412, 2000)),
    );

    let veiled = box_of(
        &t,
        px(72.0),
        Veil::new(true).child(
            div()
                .size_full()
                .bg(t.terminal.background)
                .p(t.space.sm)
                .child(Text::data("keys typed here are dropped, not buffered")),
        ),
    );

    let children = vec![
        labeled("terminal grid", &t, grid),
        labeled("cell attributes", &t, attrs),
        labeled("cursor shapes", &t, cursor_shapes),
        labeled("unfocused cursor", &t, unfocused),
        labeled("tab strip", &t, strip_el),
        labeled("scroll pill + prefix hint", &t, overlays),
        labeled("scrollback badge", &t, badge),
        labeled(
            "vt modes",
            &t,
            strip(
                &t,
                vec![
                    TerminalModes::new([
                        TerminalMode::AltScreen,
                        TerminalMode::MouseReporting,
                        TerminalMode::BracketedPaste,
                    ])
                    .into_any_element(),
                    TerminalModes::new([TerminalMode::ApplicationCursor])
                        .glyphs_only()
                        .into_any_element(),
                ],
            ),
        ),
        labeled(
            "exit strip",
            &t,
            box_of(&t, t.metrics.strip_h, ExitStrip::new(1)),
        ),
        labeled(
            "exit strip (signal, no code)",
            &t,
            box_of(&t, t.metrics.strip_h, ExitStrip::new(None)),
        ),
        labeled("veil (daemon lost)", &t, veiled),
        labeled(
            "log view",
            &t,
            box_of(
                &t,
                px(96.0),
                LogView::new(
                    "gallery-log",
                    (0..40).map(|i| SharedString::from(format!("[{i:03}] remote: Counting objects\u{2026}"))),
                ),
            ),
        ),
        labeled(
            "daemon splash (cold start)",
            &t,
            box_of(
                &t,
                px(120.0),
                DaemonSplash::starting("Starting fleetd\u{2026}").detail("~/.fleet/fleetd.sock"),
            ),
        ),
        labeled(
            "daemon splash (will not start)",
            &t,
            box_of(
                &t,
                px(200.0),
                DaemonSplash::failed("fleetd could not start.")
                    .detail("The socket ~/.fleet/fleetd.sock is stale.")
                    .log_lines([
                        SharedString::from("ERROR listen: address already in use"),
                        SharedString::from("ERROR socket owner pid 4211 is gone"),
                        SharedString::from("ERROR giving up after 3 attempts"),
                    ])
                    .hints(
                        KeyHintRow::new()
                            .key("r", "retry")
                            .key("L", "open log")
                            .key("D", "doctor")
                            .key("ctrl-q", "quit"),
                    ),
            ),
        ),
    ];
    section("terminal", &t, children)
}

fn input_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let children = vec![
        labeled(
            "text field",
            &t,
            div()
                .flex()
                .flex_col()
                .w(px(360.0))
                .child(TextField::new("feat/rut-validator").label("branch").focused(true).preview("→ buk/payroll#feat-rut-validator")),
        ),
        labeled(
            "text field · invalid",
            &t,
            div()
                .flex()
                .flex_col()
                .w(px(360.0))
                .child(TextField::new("feat/../rut").label("branch").invalid("branch cannot contain \"..\"")),
        ),
        labeled(
            "text field · placeholder",
            &t,
            div()
                .flex()
                .flex_col()
                .w(px(360.0))
                .child(TextField::new("").placeholder("Type to search GitHub repos in buk's owners.").icon(Icon::Search)),
        ),
        labeled("cycler", &t, Cycler::labeled("host", "local").has_prev(false)),
        labeled(
            "toggles",
            &t,
            div()
                .flex()
                .flex_col()
                .w(px(360.0))
                .child(Toggle::labeled("Sleep on switch", true).focused(true))
                .child(Toggle::labeled("Warn before quitting", false))
                .child(Toggle::labeled("Claude keep-alive", true).detail("matching 2 processes now").disabled(true)),
        ),
        labeled(
            "number fields",
            &t,
            div()
                .flex()
                .flex_col()
                .w(px(360.0))
                .child(NumberField::labeled("grace", 2000).unit("ms").min(0).focused(true))
                .child(NumberField::labeled("local status refresh", 200).unit("ms").min(500)),
        ),
        labeled("segmented tabs", &t, SegmentedTabs::new([SegmentedTab::new("mine", 7), SegmentedTab::new("review", 4).loading(true)]).active(0)),
        labeled(
            "select + fuzzy list",
            &t,
            div().w(px(420.0)).child(
                Select::new("origin/main")
                    .label("base")
                    .open(true)
                    .focused(true)
                    .options(
                        FuzzyList::new([
                            FuzzyItem::new("origin/main").trailing("default"),
                            FuzzyItem::new("origin/release-2026"),
                            FuzzyItem::new("pull/412/head").trailing("previous base"),
                        ])
                        .cursor(0)
                        .cap(6),
                    ),
            ),
        ),
        labeled(
            "fuzzy list · two-line",
            &t,
            div().w(px(420.0)).child(
                FuzzyList::new([
                    FuzzyItem::new("bukhr/payroll")
                        .secondary("Nómina y remuneraciones")
                        .trailing("2d")
                        .leading(Icon::Lock.el().size(IconSize::Medium)),
                    FuzzyItem::new("acme/payrolls")
                        .trailing("3w")
                        .leading(Icon::Globe.el().size(IconSize::Medium)),
                ])
                .cursor(0),
            ),
        ),
    ];
    section("input", &t, children)
}

fn overlays_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let dialog = box_of(&t,
        px(300.0),
        Dialog::new("New worktree")
            .subtitle("· buk/payroll")
            .icon(Icon::GitBranchPlus)
            .width(px(460.0))
            .body(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(TextField::new("feat/rut-validator").label("branch").focused(true).preview("→ buk/payroll#feat-rut-validator"))
                    .child(Text::hint("⚡ prepared copy ready — create takes ~2 s").faint()),
            )
            .hints(KeyHintRow::new().key("⇥", "field").key("esc", "cancel"))
            .primary("⏎ Create"),
    );

    let confirm_compact = box_of(&t,
        px(220.0),
        ConfirmDialog::new(
            "Delete buk/payroll#fix-rut-validator?",
            FactList::from_facts([
                Fact::safe("clean"),
                Fact::safe("merged into origin/main"),
                Fact::safe("no session"),
            ]),
        )
        .stamp(FreshnessStamp::new("checked", 8).action("I", "re-check"))
        .consequence("Moves the copy to trash, then removes it in the background."),
    );

    let confirm_expanded = box_of(&t,
        px(320.0),
        ConfirmDialog::new(
            "Delete worktree",
            FactList::from_facts([
                Fact::risk("12 uncommitted files"),
                Fact::risk("3 commits not on origin/main"),
                Fact::unknown("unique commit count unavailable (gh unavailable)"),
                Fact::safe("PR #412 open (not merged)"),
            ]),
        )
        .target("buk/payroll#feat-payroll-fix")
        .stamp(FreshnessStamp::new("checked", 180).action("I", "re-check"))
        .consequence("Deleting kills the session and moves the copy to trash; commits that exist only here are lost.")
        .hints(KeyHintRow::new().key("I", "re-check")),
    );

    let palette = box_of(&t,
        px(320.0),
        Overlay::new().top(px(12.0)).width(px(560.0)).child(
            Palette::new("pay fix")
                .total(63)
                .section(PaletteSection::new(
                    PaletteSectionKind::Go,
                    [
                        PaletteRow::new("payroll#feat-payroll-fix")
                            .leading(StatusGlyph::new(StatusKind::Attached).id("pal-0"))
                            .detail("session attached"),
                        PaletteRow::new("payroll#fix-rut-validator")
                            .leading(StatusGlyph::new(StatusKind::Sleeping).id("pal-1"))
                            .detail("sleeping"),
                    ],
                ))
                .section(PaletteSection::new(
                    PaletteSectionKind::Do,
                    [
                        PaletteRow::new("Prune worktrees · buk/payroll")
                            .icon(Icon::Scissors)
                            .key("x")
                            .destructive(true),
                        PaletteRow::new("Clone repo").icon(Icon::CloudDownload).key("n"),
                    ],
                ))
                .section(PaletteSection::new(
                    PaletteSectionKind::Context,
                    [PaletteRow::new("personal").icon(Icon::Boxes).key("2")],
                )),
        ),
    );

    let sheet = box_of(&t,
        px(260.0),
        Sheet::new(true)
            .header(
                div()
                    .flex()
                    .flex_col()
                    .p(px(12.0))
                    .child(SectionHeader::new("jobs").trailing(Text::hint("⟳2 running · ✕1 failed").faint()))
                    .child(Text::data_small("~/.fleet/logs/jobs/j-8f3c.log").faint()),
            )
            .body(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        JobRow::new(JobStatus::Running, "clone", "nixos")
                            .id("sheet-job-0")
                            .elapsed("0:42")
                            .percent(40)
                            .progress("Receiving objects: 40% (81/202)")
                            .selected(true)
                            .cursor(true),
                    )
                    .child(JobRow::new(JobStatus::Failed, "prs", "review").id("sheet-job-1").trailing_key("R")),
            )
            .footer(
                div().p(px(12.0)).child(
                    KeyHintRow::new()
                        .key("⏎", "log")
                        .key("c", "cancel")
                        .key("R", "retry")
                        .key("esc", "close"),
                ),
            ),
    );

    let toasts = box_of(&t,
        px(160.0),
        ToastStack::new([
            Toast::new("Path copied").icon(Icon::ClipboardCheck).short(),
            Toast::new("Slept · kept cc (claude)").icon(Icon::Moon),
            Toast {
                count: 2,
                ..Toast::new("Already running").icon(Icon::Info).short()
            },
        ]),
    );

    let children = vec![
        labeled("dialog", &t, dialog),
        labeled("confirm · compact", &t, confirm_compact),
        labeled("confirm · expanded", &t, confirm_expanded),
        labeled("palette (overlay)", &t, palette),
        labeled("sheet (jobs)", &t, sheet),
        labeled("toast stack", &t, toasts),
    ];
    section("overlays", &t, children)
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = cx.theme().mode;
        let pad = cx.theme().space.xl;
        let cursor = self.cursor;
        let scroll = self.list_scroll.clone();

        let sections = vec![
            colors_section(cx),
            type_section(cx),
            icons_section(cx),
            glyphs_section(cx),
            facts_section(cx),
            rows_section(cx, cursor, &scroll),
            structure_section(cx),
            terminal_section(cx),
            input_section(cx),
            overlays_section(cx),
        ];

        AppFrame::new()
            .context_bar(
                ContextBar::new([ContextTab::new("fleet-ui-kit gallery", 1)])
                    .leading_inset(px(84.0))
                    .chip(Chip::labeled(
                        if mode.is_dark() { Icon::Moon } else { Icon::CircleArrowUp },
                        if mode.is_dark() { "dark" } else { "light" },
                    ))
                    .daemon(DaemonState::Healthy),
            )
            .body(
                div()
                    .id("gallery-scroll")
                    .track_focus(&self.focus_handle)
                    .key_context("Gallery")
                    .on_action(cx.listener(Self::toggle_theme))
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
                    .breadcrumb("fleet-ui-kit · every component, every state")
                    .mode(Mode::Normal)
                    .ticker(
                        KeyHintRow::new()
                            .key("t", "toggle theme")
                            .key("q", "quit"),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(|cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            cx.bind_keys([
                KeyBinding::new("t", ToggleTheme, None),
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

            let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("fleet-ui-kit gallery".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |_window, cx| {
                        let view: Entity<Gallery> = cx.new(Gallery::new);
                        view
                    },
                )
                .expect("failed to open the gallery window");

            window
                .update(cx, |view, window, cx| {
                    window.focus(&view.focus_handle(cx), cx);
                })
                .ok();

            cx.activate(true);
        });
}
