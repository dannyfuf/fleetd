//! The visual test bench for the **data display** group of `fleet-ui-kit`.
//!
//! `ListView`, `Row`, `ColumnLadder`, `StatusGlyph`, `Chip`, `Badge`, `StatusDot`,
//! `KeepAliveChips`, `DegradedChip`, `PrBadge`, `AgeLabel`, `FreshnessStamp`, `FactRow`,
//! `FactList`, `KeyValueList`, `SectionHeader`, `EmptyState`, `SkeletonRows`, `KeyHint`,
//! `DoctorTable`, `Divider` and `Spinner` — every one of them in every state it can be in,
//! in both themes.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_data
//! ```
//!
//! | key | does |
//! | --- | --- |
//! | `t` | toggle light / dark |
//! | `j` `k` `gg` `G` `ctrl-d` `ctrl-u` | drive the live list's cursor (the real bindings) |
//! | `w` | cycle the worktrees pane width: 138 → 93 → 70 ch, so the column ladder moves |
//! | `s` | cycle the live list between rows, cold load and empty |
//! | `q` / `cmd-q` | quit |

use fleet_ui_kit::KitAssets;
use fleet_ui_kit::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, Menu, MenuItem,
    Pixels, SharedString, TitlebarOptions, UniformListScrollHandle, Window, WindowBounds,
    WindowOptions, actions, div, px, size,
};

actions!(
    gallery_data,
    [ToggleTheme, CycleWidth, CycleListState, Quit]
);

/// The three pane widths §2.9 names: default, detail panel open, and narrow.
const PANE_WIDTHS: [f32; 3] = [138.0, 93.0, 70.0];

/// The rows of the live list, so the cursor has something to move over.
const LIVE_ROWS: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListState {
    Rows,
    Loading,
    Empty,
}

impl ListState {
    fn next(self) -> Self {
        match self {
            ListState::Rows => ListState::Loading,
            ListState::Loading => ListState::Empty,
            ListState::Empty => ListState::Rows,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ListState::Rows => "rows",
            ListState::Loading => "cold load",
            ListState::Empty => "empty",
        }
    }
}

struct DataGallery {
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    cursor: ListCursor,
    width_ix: usize,
    list_state: ListState,
}

impl DataGallery {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            cursor: ListCursor::new(LIVE_ROWS).page(6),
            width_ix: 0,
            list_state: ListState::Rows,
        }
    }

    fn pane_ch(&self) -> f32 {
        PANE_WIDTHS[self.width_ix]
    }

    /// Every list action lands here: move the cursor, then reveal it with the scrolloff.
    /// This is the shape `fleet-app` uses — the element never sees a key.
    fn motion(&mut self, motion: ListMotion, cx: &mut Context<Self>) {
        let moving_down = self.cursor.motion(motion);
        ListView::reveal(&self.scroll, &self.cursor, moving_down);
        cx.notify();
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        cx.notify();
    }

    fn cycle_width(&mut self, _: &CycleWidth, _window: &mut Window, cx: &mut Context<Self>) {
        self.width_ix = (self.width_ix + 1) % PANE_WIDTHS.len();
        cx.notify();
    }

    fn cycle_list_state(
        &mut self,
        _: &CycleListState,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.list_state = self.list_state.next();
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }
}

impl Focusable for DataGallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------- layout helpers

fn section(title: &str, t: &Theme, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(t.space.md)
        .pb(t.space.xl)
        .child(SectionHeader::new(title.to_string()))
        .child(Divider::horizontal())
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(t.space.md)
                .pt(t.space.xs)
                .children(children),
        )
        .into_any_element()
}

fn labeled(label: &str, t: &Theme, child: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .items_start()
        .w_full()
        .gap(t.space.md)
        .child(Text::hint(label.to_string()).faint().w(px(190.0)))
        .child(div().flex().flex_1().min_w_0().items_center().child(child))
        .into_any_element()
}

fn strip(t: &Theme, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(t.space.md)
        .children(children)
        .into_any_element()
}

fn framed(t: &Theme, width: Option<Pixels>, height: Pixels, child: impl IntoElement) -> AnyElement {
    div()
        .relative()
        .h(height)
        .when_some(width, |el, width| el.w(width).flex_none())
        .when(width.is_none(), |el| el.w_full())
        .rounded(t.radii.sm)
        .bg(t.colors.surface)
        .border_1()
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

// ---------------------------------------------------------------- sample data

fn keep_alive() -> Vec<KeepAliveLabel> {
    vec![
        KeepAliveLabel::with_icon("claude", Icon::Bot),
        KeepAliveLabel::with_icon(":3000", Icon::Server),
        KeepAliveLabel::with_icon("nvim", Icon::FilePen),
        KeepAliveLabel::new("vitest"),
    ]
}

const STATUS_KINDS: [StatusKind; 10] = [
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

const PR_STATES: [PrBadgeState; 7] = [
    PrBadgeState::Draft,
    PrBadgeState::CiFail,
    PrBadgeState::Changes,
    PrBadgeState::CiPending,
    PrBadgeState::Approved,
    PrBadgeState::Review,
    PrBadgeState::Merged,
];

// ---------------------------------------------------------------- sections

fn glyph_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let glyphs: Vec<AnyElement> = STATUS_KINDS
        .iter()
        .enumerate()
        .map(|(ix, kind)| {
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .w(px(210.0))
                .child(StatusGlyph::new(*kind).id(("glyph", ix)))
                .child(Text::ui(kind.detail_word()).muted())
                .into_any_element()
        })
        .collect();

    let sizes = strip(
        &t,
        vec![
            StatusGlyph::new(StatusKind::Attached)
                .size(IconSize::Large)
                .id("size-lg")
                .into_any_element(),
            StatusGlyph::new(StatusKind::Attached)
                .size(IconSize::Medium)
                .id("size-md")
                .into_any_element(),
            StatusGlyph::new(StatusKind::Attached)
                .size(IconSize::Small)
                .id("size-sm")
                .into_any_element(),
        ],
    );

    let sentences = strip(
        &t,
        vec![
            Text::ui(StatusKind::Unknown.detail_sentence(Some("host devbox offline")))
                .muted()
                .into_any_element(),
            Text::ui(StatusKind::Sleeping.detail_sentence(Some("kept cc (claude)")))
                .muted()
                .into_any_element(),
            Text::ui(StatusKind::Attached.detail_sentence(None))
                .muted()
                .into_any_element(),
        ],
    );

    section(
        "status glyphs (§2.5 vocabulary)",
        &t,
        vec![
            labeled("every kind", &t, strip(&t, glyphs)),
            labeled("sizes 16 / 14 / 12", &t, sizes),
            labeled("detail sentences", &t, sentences),
            labeled(
                "blank cell vs. no session",
                &t,
                strip(
                    &t,
                    vec![
                        Text::hint("blank = column n/a").faint().into_any_element(),
                        div().w(px(16.0)).into_any_element(),
                        Text::hint("dim dot = no session")
                            .faint()
                            .into_any_element(),
                        StatusGlyph::new(StatusKind::NoSession)
                            .id("no-session")
                            .into_any_element(),
                        Text::hint("frozen pane forces:").faint().into_any_element(),
                        StatusGlyph::new(StatusKind::frozen())
                            .id("frozen")
                            .into_any_element(),
                    ],
                ),
            ),
        ],
    )
}

fn marks_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();

    let chips = strip(
        &t,
        vec![
            Chip::counter(Icon::LoaderCircle, 2)
                .tone(Tone::Warning)
                .spinning(true)
                .id("chip-jobs")
                .into_any_element(),
            Chip::counter(Icon::CircleDot, 3)
                .tone(Tone::Success)
                .into_any_element(),
            Chip::counter(Icon::Moon, 5)
                .tone(Tone::Secondary)
                .into_any_element(),
            Chip::counter(Icon::CircleQuestionMark, 1)
                .tone(Tone::Warning)
                .into_any_element(),
            Chip::labeled(Icon::Cloud, "devbox").into_any_element(),
            Chip::labeled(Icon::CloudOff, "devbox")
                .tone(Tone::Warning)
                .filled(true)
                .into_any_element(),
            Chip::labeled(Icon::TriangleAlert, "1 failed")
                .tone(Tone::Danger)
                .filled(true)
                .into_any_element(),
        ],
    );

    let suppressed = strip(
        &t,
        vec![
            Text::hint("counter(flag, 0) renders:")
                .faint()
                .into_any_element(),
            Chip::counter(Icon::Flag, 0).into_any_element(),
            Text::hint("nothing · zero_suppress(false):")
                .faint()
                .into_any_element(),
            Chip::counter(Icon::Flag, 0)
                .zero_suppress(false)
                .into_any_element(),
        ],
    );

    let badges = strip(
        &t,
        vec![
            Badge::new("default").into_any_element(),
            Badge::new("current")
                .style(BadgeStyle::Outlined)
                .into_any_element(),
            Badge::new("ready")
                .tone(Tone::Success)
                .style(BadgeStyle::Filled)
                .into_any_element(),
            Badge::new("stopped")
                .tone(Tone::Danger)
                .style(BadgeStyle::Filled)
                .into_any_element(),
            Badge::new("draft").tone(Tone::Muted).into_any_element(),
        ],
    );

    let dots = strip(
        &t,
        vec![
            StatusDot::new(Tone::Success).into_any_element(),
            StatusDot::new(Tone::Warning).into_any_element(),
            StatusDot::new(Tone::Danger).into_any_element(),
            StatusDot::new(Tone::Muted).opacity(0.4).into_any_element(),
            StatusDot::small(Tone::Warning).into_any_element(),
            StatusDot::small(Tone::Secondary).into_any_element(),
        ],
    );

    let pr_badges: Vec<AnyElement> = PR_STATES
        .iter()
        .map(|state| PrBadge::new(412, *state).into_any_element())
        .collect();

    let pr_variants = strip(
        &t,
        vec![
            PrBadge::new(412, PrBadgeState::CiFail)
                .stale(true)
                .into_any_element(),
            PrBadge::state_only(PrBadgeState::Approved).into_any_element(),
        ],
    );

    let keep_alive_ladder: Vec<AnyElement> = [18.0f32, 14.0, 10.0]
        .iter()
        .map(|budget| {
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .child(Text::hint(format!("{budget:.0}ch")).faint())
                .child(KeepAliveChips::new(keep_alive()).width_ch(*budget))
                .into_any_element()
        })
        .collect();

    section(
        "chips, badges and dots",
        &t,
        vec![
            labeled("chips", &t, chips),
            labeled("zero suppression", &t, suppressed),
            labeled("badges", &t, badges),
            labeled("status dots 8 / 6 px", &t, dots),
            labeled("pr badges", &t, strip(&t, pr_badges)),
            labeled("pr badge · stale, state only", &t, pr_variants),
            labeled("keep-alive ladder", &t, strip(&t, keep_alive_ladder)),
            labeled(
                "keep-alive · kind icons",
                &t,
                KeepAliveChips::new(keep_alive())
                    .show_kind_icons(true)
                    .max_visible(3),
            ),
            labeled(
                "degraded (outranks keep-alive)",
                &t,
                DegradedChip::hooks_failed().hint("J", "for log"),
            ),
            labeled(
                "spinners",
                &t,
                strip(
                    &t,
                    vec![
                        Spinner::new("spin-lg").into_any_element(),
                        Spinner::new("spin-md")
                            .size(IconSize::Medium)
                            .tone(Tone::Secondary)
                            .into_any_element(),
                        SpinnerWithLabel::new("spin-label", "waking payroll\u{2026}")
                            .into_any_element(),
                    ],
                ),
            ),
        ],
    )
}

fn time_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let ages = strip(
        &t,
        vec![
            AgeLabel::from_secs(42).into_any_element(),
            AgeLabel::from_secs(600).into_any_element(),
            AgeLabel::from_secs(7_200).into_any_element(),
            AgeLabel::from_secs(432_000).into_any_element(),
            AgeLabel::from_secs(1_814_400).into_any_element(),
            AgeLabel::from_secs(63_072_000).into_any_element(),
            AgeLabel::none().into_any_element(),
            AgeLabel::from_secs(7_200).mono(true).into_any_element(),
        ],
    );

    let stamps = div()
        .flex()
        .flex_col()
        .gap(t.space.xs)
        .child(FreshnessStamp::new("checked", 8))
        .child(FreshnessStamp::new("checked", 240))
        .child(FreshnessStamp::new("checked", 1_800).action("I", "re-check"))
        .child(
            FreshnessStamp::new("fetched", 240)
                .refreshing("stamp-refreshing")
                .action("r", "refresh"),
        )
        .child(FreshnessStamp::new("inspected", 60).error("gh: HTTP 502 upstream connect error"));

    let derived = strip(
        &t,
        vec![
            PrBadge::new(412, PrBadgeState::Approved)
                .stale(Freshness::Fresh.derived_opacity(t.metrics.stale_opacity) < 1.0)
                .into_any_element(),
            PrBadge::new(408, PrBadgeState::Approved)
                .stale(Freshness::Stale.derived_opacity(t.metrics.stale_opacity) < 1.0)
                .into_any_element(),
            Text::hint("fresh · stale (55 %)")
                .faint()
                .into_any_element(),
        ],
    );

    section(
        "ages and freshness (§2.6)",
        &t,
        vec![
            labeled("age labels", &t, ages),
            labeled("freshness ladder", &t, stamps),
            labeled("derived marks", &t, derived),
        ],
    )
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
        Fact::risk("session attached \u{b7} claude, :3000 running"),
        Fact::unknown("unique commit count unavailable (gh unavailable)"),
        Fact::safe("PR #412 open (not merged)"),
    ]);
    let loading = FactList::from_facts([Fact::safe("clean")]).loading(true);

    let key_line = |list: &FactList| {
        Text::hint(format!(
            "confirm key {} \u{b7} enter {} \u{b7} {} risk(s) \u{b7} {} unknown",
            list.confirm_key().label(),
            if list.confirm_key().accepts_enter() {
                "accepted"
            } else {
                "refused"
            },
            list.risk_count(),
            list.unknown_count(),
        ))
        .faint()
    };

    let kv = KeyValueList::titled("safety")
        .trailing(FreshnessStamp::new("checked", 14).action("I", "re-check"))
        .row("dirty", FactValue::known("12 files"))
        .row("ahead / behind", FactValue::known("\u{21e1}3 \u{21e3}0"))
        .row("unique commits", FactValue::Null)
        .row("published", FactValue::known("no"))
        .mono_row(
            "path",
            FactValue::known("~/.fleet/worktrees/buk/payroll/feat"),
        )
        .row("warning", FactValue::warning("gh unavailable"));

    let kv_refreshing = KeyValueList::titled("safety")
        .trailing(
            FreshnessStamp::new("checked", 240)
                .refreshing("kv-refreshing")
                .action("I", "re-check"),
        )
        .row("dirty", FactValue::known("12 files"))
        .row("unique commits", FactValue::Null)
        .refreshing(true);

    let doctor = DoctorTable::new([
        DoctorRow::new("git", DoctorStatus::Ok, "git version 2.49.0"),
        DoctorRow::new(
            "gh auth",
            DoctorStatus::Fail,
            "gh: not logged in to github.com",
        ),
        DoctorRow::new("copy-on-write", DoctorStatus::Ok, "cp -c (APFS clonefile)"),
        DoctorRow::new(
            "host devbox",
            DoctorStatus::Warn,
            "ssh: slow handshake (2.1s)",
        ),
    ]);

    section(
        "facts, tables and hints",
        &t,
        vec![
            labeled("fact rows", &t, kv),
            labeled("fact rows \u{b7} re-inspecting", &t, kv_refreshing),
            labeled("fact list \u{b7} compact", &t, compact.clone()),
            labeled("", &t, key_line(&compact)),
            labeled("fact list \u{b7} expanded", &t, expanded.clone()),
            labeled("", &t, key_line(&expanded)),
            labeled("fact list \u{b7} loading", &t, loading.clone()),
            labeled("", &t, key_line(&loading)),
            labeled("doctor table", &t, doctor),
            labeled(
                "key hints",
                &t,
                KeyHintRow::new()
                    .key("\u{23ce}", "open")
                    .key("^s x", "close")
                    .hint(KeyHint::labeled("Y", "delete").key_tone(Tone::Default))
                    .key("esc", "cancel"),
            ),
            labeled(
                "section header",
                &t,
                div().w_full().child(
                    SectionHeader::new("session")
                        .trailing(StatusGlyph::new(StatusKind::Attached).id("section-glyph")),
                ),
            ),
            labeled(
                "dividers",
                &t,
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .h(px(24.0))
                    .child(div().flex_1().child(Divider::horizontal()))
                    .child(Divider::vertical())
                    .child(div().flex_1().child(Divider::horizontal())),
            ),
        ],
    )
}

/// One worktree row built the way `fleet-app` must build it: the ladder resolves the columns,
/// `RowColumn::resolved` fills them, and the row never decides a width itself.
fn worktree_row(t: &Theme, ix: usize, pane_ch: f32, cursor: bool) -> AnyElement {
    let ladder = ColumnLadder::worktrees_in_scope(false);
    let branches = [
        "feat/payroll-fix",
        "fix/rut-validator",
        "spike/gpui-vt",
        "chore/deps",
        "api-poc",
        "new-slug",
    ];
    let kinds = [
        StatusKind::Attached,
        StatusKind::Sleeping,
        StatusKind::NoSession,
        StatusKind::Degraded,
        StatusKind::Unknown,
        StatusKind::JobRunning,
    ];
    let branch = branches[ix % branches.len()];
    let kind = kinds[ix % kinds.len()];

    let mut row = Row::with_id(("wt", ix))
        .leading(StatusGlyph::new(kind).id(("wt-glyph", ix)))
        .selected(cursor)
        .cursor(cursor);

    for column in ladder.resolve(pane_ch) {
        let element: AnyElement = match column.key.as_ref() {
            "branch" => div()
                .flex()
                .items_center()
                .gap(t.space.xs)
                .min_w_0()
                .child(Text::data(branch).ellipsize())
                // The dirty mark rides with the branch instead of buying a column (§3.3).
                .when(ix.is_multiple_of(2), |el| {
                    el.child(
                        Icon::FilePen
                            .el()
                            .size(IconSize::Small)
                            .color(t.colors.warning),
                    )
                })
                .when(ix % 3 == 2, |el| {
                    el.child(Chip::labeled(Icon::Cloud, "devbox"))
                })
                .into_any_element(),
            "repo" => Text::ui(truncate("buk/payroll", 14, Truncate::Head))
                .muted()
                .into_any_element(),
            "keepalive" => {
                let budget = KeepAliveChips::from_pane_ch(pane_ch);
                if kind == StatusKind::Degraded {
                    DegradedChip::hooks_failed().into_any_element()
                } else if ix.is_multiple_of(2) {
                    KeepAliveChips::new(keep_alive())
                        .width_ch(budget)
                        .into_any_element()
                } else {
                    div().into_any_element()
                }
            }
            "pr" => {
                if ix % 3 == 2 {
                    div().into_any_element()
                } else {
                    PrBadge::new(400 + ix as u64, PR_STATES[ix % PR_STATES.len()])
                        .stale(ix % 4 == 3)
                        .into_any_element()
                }
            }
            "age" => AgeLabel::from_secs(3_600 * (ix as i64 + 1)).into_any_element(),
            _ => div().into_any_element(),
        };
        row = row.column(RowColumn::resolved(&column, element));
    }
    row.into_any_element()
}

fn rows_section(cx: &mut App, pane_ch: f32) -> AnyElement {
    let t = cx.theme().clone();

    let states = framed(
        &t,
        None,
        t.metrics.row_h * 6.0 + t.metrics.job_row_h,
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(worktree_row(&t, 0, pane_ch, false))
            .child(worktree_row(&t, 1, pane_ch, true))
            .child(
                // Selected but not focused: the background stays, the cursor bar goes.
                Row::with_id("row-selected-unfocused")
                    .leading(StatusGlyph::new(StatusKind::DetachedAwake).id("row-sel"))
                    .column(RowColumn::flex(Text::data("selected, pane not focused")))
                    .selected(true),
            )
            .child(
                Row::with_id("row-dimmed")
                    .leading(StatusGlyph::new(StatusKind::JobRunning).id("row-dim"))
                    .column(RowColumn::flex(Text::data("deleting \u{2014} dimmed 40 %")))
                    .dimmed(true),
            )
            .child(
                Row::with_id("row-disabled")
                    .leading(StatusGlyph::new(StatusKind::Unknown).id("row-dis"))
                    .column(RowColumn::flex(Text::data("disabled \u{2014} no hover")))
                    .disabled(true),
            )
            .child(
                // No leading element at all: the blank cell of §2.5.
                Row::with_id("row-blank")
                    .column(RowColumn::flex(Text::data("no glyph column for this row"))),
            )
            .child(
                Row::with_id("row-two-line")
                    .leading(StatusGlyph::new(StatusKind::Cloning).id("row-clone"))
                    .column(RowColumn::flex(Text::data("nixos")))
                    .column(RowColumn::fixed_ch(7.0, AgeLabel::none()).align(ColumnAlign::Right))
                    .second_line(Text::data_small("Receiving objects: 40% (81/202)").faint()),
            ),
    );

    let ladder_keys: Vec<AnyElement> = PANE_WIDTHS
        .iter()
        .map(|w| {
            let keys = ColumnLadder::worktrees()
                .resolve(*w)
                .iter()
                .map(|c| c.key.to_string())
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
            Text::hint(format!("{w:.0}ch: {keys}"))
                .faint()
                .into_any_element()
        })
        .collect();

    let pr_ladder: Vec<AnyElement> = [(true, true), (false, false)]
        .iter()
        .map(|(review, multi)| {
            let keys = ColumnLadder::pull_requests_for(*review, *multi)
                .resolve(140.0)
                .iter()
                .map(|c| c.key.to_string())
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
            Text::hint(format!(
                "{} @140ch: {keys}",
                if *review { "REVIEW" } else { "MINE   " }
            ))
            .faint()
            .into_any_element()
        })
        .collect();

    section(
        "rows and the column ladder (§2.9)",
        &t,
        vec![
            labeled(
                "row states",
                &t,
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(t.space.xs)
                    .child(
                        Text::hint(format!(
                            "pane {pane_ch:.0} ch \u{b7} w cycles 138 / 93 / 70"
                        ))
                        .faint(),
                    )
                    .child(states),
            ),
            labeled(
                "worktrees ladder",
                &t,
                div().flex().flex_col().children(ladder_keys),
            ),
            labeled("pr ladder", &t, div().flex().flex_col().children(pr_ladder)),
        ],
    )
}

fn list_section(cx: &mut App, gallery: &DataGallery) -> AnyElement {
    let t = cx.theme().clone();
    let cursor = gallery.cursor.index();
    let state = gallery.list_state;
    let item_count = match state {
        ListState::Rows => LIVE_ROWS,
        _ => 0,
    };

    let list = ListView::new(
        "data-gallery-list",
        item_count,
        move |ix, is_cursor, _w, _cx| {
            Row::with_id(("live", ix))
                .selected(is_cursor)
                .cursor(is_cursor)
                .leading(
                    StatusGlyph::new(STATUS_KINDS[ix % STATUS_KINDS.len()]).id(("live-glyph", ix)),
                )
                .column(RowColumn::flex(Text::data(format!(
                    "row {ix:02} \u{2014} j/k, gg/G, ctrl-d/ctrl-u"
                ))))
                .column(
                    RowColumn::fixed_ch(7.0, AgeLabel::from_secs(60 * ix as i64 + 30))
                        .align(ColumnAlign::Right),
                )
                .into_any_element()
        },
    )
    .cursor(cursor)
    .row_height(t.metrics.row_h)
    .track_scroll(&gallery.scroll)
    .loading(state == ListState::Loading)
    .empty(EmptyState::new("Nothing matches \"rut\".").action("esc  clear"));

    let empties = strip(
        &t,
        vec![
            framed(
                &t,
                Some(px(280.0)),
                px(90.0),
                EmptyState::new("No worktrees yet.").action("n  create one"),
            ),
            framed(
                &t,
                Some(px(280.0)),
                px(90.0),
                EmptyState::new("No PRs waiting for your review in buk.").action("r  refresh"),
            ),
        ],
    );

    section(
        "list view (virtualized, scrolloff 2)",
        &t,
        vec![
            labeled(
                "live list",
                &t,
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(t.space.xs)
                    .child(
                        Text::hint(format!(
                            "state {} (s) \u{b7} cursor {}/{} \u{b7} page {} \u{b7} scrolloff {}",
                            state.label(),
                            cursor + 1,
                            gallery.cursor.len(),
                            gallery.cursor.page_rows(),
                            gallery.cursor.scrolloff_rows(),
                        ))
                        .faint(),
                    )
                    .child(framed(&t, None, px(240.0), list)),
            ),
            labeled(
                "skeleton rows",
                &t,
                framed(&t, None, px(120.0), SkeletonRows::new(4)),
            ),
            labeled("empty states", &t, empties),
        ],
    )
}

impl Render for DataGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let pane_ch = self.pane_ch();
        let dark = theme.mode.is_dark();

        let sections = vec![
            glyph_section(cx),
            marks_section(cx),
            time_section(cx),
            facts_section(cx),
            rows_section(cx, pane_ch),
            list_section(cx, self),
        ];

        AppFrame::new()
            .context_bar(
                ContextBar::new([ContextTab::new("data display", 1)])
                    .leading_inset(px(84.0))
                    .chip(Chip::labeled(
                        if dark {
                            Icon::Moon
                        } else {
                            Icon::CircleArrowUp
                        },
                        if dark { "dark" } else { "light" },
                    ))
                    .daemon(DaemonState::Healthy),
            )
            .body(
                div()
                    .id("data-gallery-scroll")
                    .track_focus(&self.focus_handle)
                    .key_context("DataGallery")
                    .on_action(cx.listener(Self::toggle_theme))
                    .on_action(cx.listener(Self::cycle_width))
                    .on_action(cx.listener(Self::cycle_list_state))
                    .on_action(cx.listener(Self::quit))
                    .on_action(
                        cx.listener(|this, _: &ListDown, _w, cx| this.motion(ListMotion::Down, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ListUp, _w, cx| this.motion(ListMotion::Up, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ListFirst, _w, cx| {
                            this.motion(ListMotion::First, cx)
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &ListLast, _w, cx| this.motion(ListMotion::Last, cx)),
                    )
                    .on_action(cx.listener(|this, _: &ListPageDown, _w, cx| {
                        this.motion(ListMotion::PageDown, cx)
                    }))
                    .on_action(cx.listener(|this, _: &ListPageUp, _w, cx| {
                        this.motion(ListMotion::PageUp, cx)
                    }))
                    .size_full()
                    .overflow_y_scroll()
                    .p(theme.space.xl)
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .status_bar(
                StatusBar::new()
                    .breadcrumb("fleet-ui-kit \u{b7} data display")
                    .mode(Mode::Normal)
                    .ticker(
                        KeyHintRow::new()
                            .key("t", "theme")
                            .key("w", "pane width")
                            .key("s", "list state")
                            .key("j/k gg G ^d ^u", "cursor")
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
            let mut bindings = vec![
                KeyBinding::new("t", ToggleTheme, None),
                KeyBinding::new("w", CycleWidth, None),
                KeyBinding::new("s", CycleListState, None),
                KeyBinding::new("q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
            ];
            // The real list bindings, straight from the kit: j / k / gg / G / ctrl-d / ctrl-u.
            bindings.extend(list_key_bindings(None));
            cx.bind_keys(bindings);
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
            let title: SharedString = "fleet-ui-kit \u{b7} data display".into();
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some(title),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |_window, cx| {
                        let view: Entity<DataGallery> = cx.new(DataGallery::new);
                        view
                    },
                )
                .expect("failed to open the data-display gallery window");

            window
                .update(cx, |view, window, cx| {
                    window.focus(&view.focus_handle(cx), cx);
                })
                .ok();

            cx.activate(true);
        });
}
