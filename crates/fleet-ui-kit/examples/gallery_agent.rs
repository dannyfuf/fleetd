//! The visual test bench for the **native-agent** group of `fleet-ui-kit`.
//!
//! `docs/DESIGN-SYSTEM.md` §8.3: *if a state is not in a gallery, it is not implemented.* Every
//! state of every component in §6.6 has a panel here — the six `ToolRow` states, all eighteen
//! transcript rows, the three `DecisionDock` occupants and their queue and in-flight states,
//! the `MetadataRow` collapse ladder, the live `MultilineInput` with its trigger reports, and
//! the streaming `Markdown` invariants.
//!
//! The transcript, the composer and the dock are *live*: a real
//! [`fleet_ui_kit::TranscriptList`] entity over GPUI `list`, a real
//! [`fleet_ui_kit::MultilineInput`], and a real drawer attached to the composer's top edge, so
//! scrolling, the jump-to-latest chip, the seam and the key routing behave here exactly as they
//! do in an agent tab.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_agent
//! ```
//!
//! | Key | What |
//! | --- | --- |
//! | `ctrl-t` | toggle light / dark |
//! | `ctrl-e` | expand / collapse every expandable row |
//! | `ctrl-d` | cycle the drawer: approval → question → wizard → plan → none |
//! | `ctrl-s` | toggle scroll mode (freezes the tail) |
//! | `ctrl-g` | jump to latest |
//! | `ctrl-r` | toggle the streaming caret and the shimmer |
//! | `ctrl-x` | toggle the empty transcript |
//! | `ctrl-w` | cycle the metadata strip's available width |
//! | `ctrl-y` | toggle the drawer's `answering…` state |
//! | `ctrl-q` / `cmd-q` | quit |

pub mod support;
const LAYOUT: support::layout::GalleryLayout = support::layout::GalleryLayout {
    label_width: 150.0,
    column: false,
    divided: false,
    compact: true,
};
use std::time::Instant;

use fleet_ui_kit::prelude::*;
use fleet_ui_kit::theme::ch;
use gpui::{AnyElement, App, Context, Entity, FocusHandle, Focusable, KeyBinding, Window, actions};

actions!(
    gallery_agent,
    [
        ToggleTheme,
        Quit,
        ToggleExpand,
        CycleDecision,
        ToggleScroll,
        JumpToLatest,
        ToggleStreaming,
        ToggleEmpty,
        CycleWidth,
        ToggleAnswering,
    ]
);

/// The widths `ctrl-w` walks, so the metadata strip's collapse ladder is visible.
const WIDTHS: [f32; 4] = [520.0, 300.0, 200.0, 120.0];

// ---------------------------------------------------------------------------------------------
// The drawer
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drawer {
    Approval,
    ApprovalNoEdit,
    Question,
    Wizard,
    Plan,
    None,
}

impl Drawer {
    fn next(self) -> Self {
        match self {
            Drawer::Approval => Drawer::ApprovalNoEdit,
            Drawer::ApprovalNoEdit => Drawer::Question,
            Drawer::Question => Drawer::Wizard,
            Drawer::Wizard => Drawer::Plan,
            Drawer::Plan => Drawer::None,
            Drawer::None => Drawer::Approval,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Drawer::Approval => "approval",
            Drawer::ApprovalNoEdit => "approval · no [e], with a diff",
            Drawer::Question => "question",
            Drawer::Wizard => "wizard 2/3",
            Drawer::Plan => "plan ready",
            Drawer::None => "closed",
        }
    }

    fn decision(self, answering: bool) -> Option<Decision> {
        let decision = match self {
            Drawer::None => return None,
            Drawer::Approval => Decision::new(
                "gate-approval",
                "claude wants to run a command",
                DecisionKind::Approval(
                    ApprovalRequest::new("Bash", "git push --force origin main")
                        .rationale("the branch diverged after the rebase")
                        .caution("this command came from fetched web content")
                        .allows_edit(true),
                ),
            )
            .queued(0, 3),
            // Codex accepts no amended invocation, so `[e]` is unbound *and* absent; the diff
            // is joined by item id and drawn by the owner (here, a stand-in).
            Drawer::ApprovalNoEdit => Decision::new(
                "gate-approval-codex",
                "codex wants to change a file",
                DecisionKind::Approval(
                    ApprovalRequest::new("apply_patch", "crates/fleet-core/src/payroll.rs")
                        .allows_edit(false),
                ),
            ),
            Drawer::Question => Decision::new(
                "gate-question",
                "claude has a question",
                DecisionKind::Question(
                    QuestionSet::new(vec![
                        DecisionQuestion::new(
                            "package manager",
                            "which package manager should the project use?",
                        )
                        .options(vec![
                            QuestionOption::new("pnpm").description("fast, disk-efficient"),
                            QuestionOption::new("npm").description("the default"),
                            QuestionOption::new("yarn"),
                        ])
                        .multi_select(true)
                        .allow_other(true),
                    ])
                    .selected(vec![vec![0]]),
                ),
            ),
            Drawer::Wizard => Decision::new(
                "gate-wizard",
                "claude has questions",
                DecisionKind::Question(
                    QuestionSet::new(vec![
                        DecisionQuestion::new("scope", "which files?")
                            .options(vec![QuestionOption::new("payroll")]),
                        DecisionQuestion::new("rounding", "which rounding should the fix use?")
                            .options(vec![
                                QuestionOption::new("half-up").description("like the spec"),
                                QuestionOption::new("banker's"),
                            ]),
                        DecisionQuestion::new("tests", "add a regression test?")
                            .options(vec![QuestionOption::new("yes"), QuestionOption::new("no")]),
                    ])
                    .cursor(1),
                ),
            ),
            Drawer::Plan => Decision::new(
                "gate-plan",
                "plan ready",
                DecisionKind::PlanReady {
                    title: "fix the payroll rounding".into(),
                    markdown: None,
                },
            ),
        };
        Some(decision.answering(answering))
    }
}

// ---------------------------------------------------------------------------------------------
// The gallery
// ---------------------------------------------------------------------------------------------

struct AgentGallery {
    focus_handle: FocusHandle,
    transcript: Entity<TranscriptList>,
    composer: Entity<MultilineInput>,
    /// The composer's two non-editing states: disabled while an approval owns the keys, and
    /// dimmed while it still holds the focus handle but is not where the user is.
    read_only: Entity<MultilineInput>,
    dimmed: Entity<MultilineInput>,
    /// The per-width memo the composer's metadata strip reads. It outlives the frame on
    /// purpose: that is the whole point of `MetadataFit`.
    fit: MetadataFit,
    width: usize,
    expanded: bool,
    drawer: Drawer,
    answering: bool,
    scroll_mode: bool,
    streaming: bool,
    empty: bool,
    started_at: Instant,
    trigger: Option<SharedString>,
}

impl AgentGallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let transcript = cx.new(TranscriptList::new);
        let composer = cx.new(|cx| {
            MultilineInput::new(
                cx,
                "message claude… (@ files · $ skills · / commands)".into(),
            )
        });
        cx.subscribe(&composer, |this, _, event: &MultilineInputEvent, cx| {
            if let MultilineInputEvent::Trigger(trigger) = event {
                this.trigger = Some(SharedString::from(format!(
                    "trigger {} at {}",
                    trigger.symbol, trigger.at
                )));
                cx.notify();
            }
        })
        .detach();
        let read_only = cx.new(|cx| {
            let mut input = MultilineInput::new(cx, "git push --force origin main".into());
            input.set_read_only(true, cx);
            input
        });
        let dimmed = cx.new(|cx| {
            let mut input = MultilineInput::new(cx, "message claude…".into());
            input.set_text("a draft nobody is typing into", cx);
            input.set_focus_visible(false, cx);
            input
        });
        let this = Self {
            focus_handle: cx.focus_handle(),
            transcript,
            composer,
            read_only,
            dimmed,
            fit: MetadataFit::new(),
            width: 0,
            expanded: false,
            drawer: Drawer::Approval,
            answering: false,
            scroll_mode: false,
            streaming: true,
            empty: false,
            started_at: Instant::now(),
            trigger: None,
        };
        this.publish(cx);
        this
    }

    fn publish(&self, cx: &mut Context<Self>) {
        let rows = self.rows();
        self.transcript
            .update(cx, |transcript, cx| transcript.set_rows(rows, cx));
    }

    /// Every row kind the transcript can draw, in the order a real turn emits them.
    fn rows(&self) -> Vec<TranscriptRow> {
        support::agent_rows::sample_thread(support::agent_rows::Thread {
            expanded: self.expanded,
            streaming: self.streaming,
            empty: self.empty,
            started_at: self.started_at,
        })
    }

    // -- actions ------------------------------------------------------------------------------

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        cx.notify();
    }

    fn toggle_expand(&mut self, _: &ToggleExpand, _window: &mut Window, cx: &mut Context<Self>) {
        self.expanded = !self.expanded;
        self.publish(cx);
        cx.notify();
    }

    fn cycle_decision(&mut self, _: &CycleDecision, _window: &mut Window, cx: &mut Context<Self>) {
        self.drawer = self.drawer.next();
        cx.notify();
    }

    fn toggle_answering(
        &mut self,
        _: &ToggleAnswering,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.answering = !self.answering;
        cx.notify();
    }

    fn toggle_scroll(&mut self, _: &ToggleScroll, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_mode = !self.scroll_mode;
        let enabled = self.scroll_mode;
        self.transcript
            .update(cx, |transcript, cx| transcript.scroll_mode(enabled, cx));
        cx.notify();
    }

    fn toggle_streaming(
        &mut self,
        _: &ToggleStreaming,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.streaming = !self.streaming;
        self.publish(cx);
        cx.notify();
    }

    fn toggle_empty(&mut self, _: &ToggleEmpty, _window: &mut Window, cx: &mut Context<Self>) {
        self.empty = !self.empty;
        self.publish(cx);
        cx.notify();
    }

    fn cycle_width(&mut self, _: &CycleWidth, _window: &mut Window, cx: &mut Context<Self>) {
        self.width = (self.width + 1) % WIDTHS.len();
        cx.notify();
    }

    fn jump_to_latest(&mut self, _: &JumpToLatest, _window: &mut Window, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |transcript, cx| transcript.scroll_to_latest(cx));
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }
}

impl Focusable for AgentGallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Every `ToolRow` state, plus the two geometry rules: the chevron is `invisible` rather than
/// absent when a row cannot expand, and an expanded row keeps the same 30 px line.
fn tool_states(cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let row = |key: usize,
               label: &'static str,
               state: ToolRowState,
               icon: Icon,
               summary: &'static str,
               result: Option<&'static str>,
               body: bool| {
        let mut row = ToolRow::new(label, "bash", summary).icon(icon).state(state);
        if let Some(result) = result {
            row = row.result(result);
        }
        if body {
            row = row.body("the first meaningful line of output");
        }
        LAYOUT.labeled(
            label,
            &theme,
            div()
                .w_full()
                .child(ToolRowElement::new(row, key).into_any_element()),
        )
    };
    let children = vec![
        row(
            0,
            "running",
            ToolRowState::Running,
            Icon::Terminal,
            "cargo build",
            None,
            false,
        ),
        row(
            1,
            "done",
            ToolRowState::Done,
            Icon::Terminal,
            "cargo test",
            Some("exit 0 · 1.2s"),
            true,
        ),
        row(
            2,
            "failed",
            ToolRowState::Failed,
            Icon::Terminal,
            "cargo clippy --workspace --all-targets --all-features -D warnings",
            Some("exit 101"),
            false,
        ),
        row(
            3,
            "denied",
            ToolRowState::Denied,
            Icon::Terminal,
            "rm -rf target",
            None,
            false,
        ),
        row(
            4,
            "stopped",
            ToolRowState::Stopped,
            Icon::Terminal,
            "cargo build --release",
            None,
            false,
        ),
        row(
            5,
            "severe",
            ToolRowState::Severe,
            Icon::TriangleAlert,
            "the session process died before the turn settled",
            None,
            true,
        ),
        LAYOUT.labeled(
            "expanded",
            &theme,
            div().w_full().child(
                ToolRowElement::new(
                    ToolRow::new("t-x", "edit", "crates/fleet-core/src/payroll.rs")
                        .icon(Icon::FilePen)
                        .state(ToolRowState::Done)
                        .result(format_file_delta(14, 3))
                        .body("crates/fleet-core/src/payroll.rs\ncrates/fleet-core/src/tax.rs")
                        .expanded(true),
                    6,
                )
                .into_any_element(),
            ),
        ),
    ];
    LAYOUT.section("tool rows · five states plus severe", &theme, children)
}

/// The `Markdown` streaming invariants, side by side: an open fence is code and uncoloured, the
/// same fence closed is coloured, and every decided block above it stayed where it was.
fn markdown_states(cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    const PREFIX: &str = "## rounding\n\nThe reducer truncates, the spec says half-up.\n\n\
                          - read the reducer\n- add a failing test\n\n> then fix the projection\n\n\
                          ```rust\nlet cents = cents.div_euclid(1";
    let closed = format!("{PREFIX}00);\n```");
    let children = vec![
        LAYOUT.labeled(
            "streaming prefix",
            &theme,
            div()
                .w_full()
                .child(markdown(&parse_markdown_document(PREFIX), cx)),
        ),
        LAYOUT.labeled(
            "closed fence",
            &theme,
            div()
                .w_full()
                .child(markdown(&parse_markdown_document(&closed), cx)),
        ),
    ];
    LAYOUT.section("markdown · a prefix parses safely", &theme, children)
}

/// The composer's states that are not "someone is typing": the gallery's live one covers empty,
/// typing, multi-line, IME and the trigger reports.
fn composer_states(gallery: &AgentGallery, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let children = vec![
        LAYOUT.labeled(
            "read-only",
            &theme,
            div().w_full().child(gallery.read_only.clone()),
        ),
        LAYOUT.labeled(
            "dimmed · a decision owns the keys",
            &theme,
            div().w_full().child(gallery.dimmed.clone()),
        ),
    ];
    LAYOUT.section("composer · non-editing states", &theme, children)
}

impl Render for AgentGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mode = if self.scroll_mode { "scroll" } else { "live" };
        let available = px(WIDTHS[self.width]);
        let segments = vec![
            MetadataSegment::pinned("claude-opus-5"),
            MetadataSegment::new("high"),
            MetadataSegment::new("supervised"),
            MetadataSegment::new("build"),
        ];
        // The memo is keyed per width; the revision never changes here because the segment list
        // does not.
        // The overflow chip is an ellipsis glyph plus a count: three display columns.
        let fit = self
            .fit
            .fit(available, 0, &segments, theme.space.md, ch(3.0));
        let decision = self.drawer.decision(self.answering);

        div()
            .key_context("AgentGallery")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::toggle_expand))
            .on_action(cx.listener(Self::cycle_decision))
            .on_action(cx.listener(Self::toggle_answering))
            .on_action(cx.listener(Self::toggle_scroll))
            .on_action(cx.listener(Self::jump_to_latest))
            .on_action(cx.listener(Self::toggle_streaming))
            .on_action(cx.listener(Self::toggle_empty))
            .on_action(cx.listener(Self::cycle_width))
            .on_action(cx.listener(Self::quit))
            .size_full()
            .bg(theme.colors.bg)
            .flex()
            .gap(theme.space.xl)
            .p(theme.space.xl)
            .child(
                // The static half: states that do not need a live entity.
                div()
                    .id("agent-gallery-static")
                    .w(AGENT_CONTENT_W)
                    .flex_none()
                    .h_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(tool_states(cx))
                    .child(composer_states(self, cx))
                    .child(markdown_states(cx)),
            )
            .child(
                // The live half: the transcript, the drawer and the composer, in their real
                // stacking order so the attachment seam is visible.
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(theme.space.md)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(theme.space.md)
                            .child(Text::label("transcript"))
                            .child(Text::hint(mode).muted())
                            .child(Text::hint(self.drawer.label()).muted())
                            .child(
                                KeyHintRow::new()
                                    .key("^e", "expand")
                                    .key("^d", "drawer")
                                    .key("^y", "answering")
                                    .key("^s", "scroll")
                                    .key("^g", "latest")
                                    .key("^r", "stream")
                                    .key("^x", "empty")
                                    .key("^w", "width"),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .rounded(theme.radii.sm)
                            .border(theme.metrics.hairline)
                            .border_color(theme.colors.border)
                            .overflow_hidden()
                            .child(self.transcript.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(AGENT_CONTENT_W)
                            .flex()
                            .flex_col()
                            .children(decision.map(|decision| {
                                let patch = matches!(decision.kind, DecisionKind::Approval(_))
                                    && decision.id == "gate-approval-codex";
                                let dock = DecisionDock::new(decision)
                                    .on_action(|_action, _window, _cx| {});
                                // The diff slot: the real one is a `fleet_lazygit::DiffView`,
                                // which the kit cannot reach (ADR 0010), so the owner hands one
                                // in. This stand-in shows the bounded height it lands at.
                                if patch {
                                    dock.diff(
                                        div()
                                            .id("dock-diff")
                                            .w_full()
                                            .max_h(AGENT_BODY_MAX_H)
                                            .overflow_y_scroll()
                                            .rounded(theme.radii.sm)
                                            .bg(theme.colors.bg)
                                            .p(theme.space.sm)
                                            .child(Text::data_small(
                                                "@@ -1,3 +1,3 @@\n-cents / 100\n\
                                                 +cents.div_euclid(100)",
                                            )),
                                    )
                                } else {
                                    dock
                                }
                            }))
                            .child(
                                // A stand-in for the composer surface the app owns: the dock
                                // rounds only its top corners and masks the border they share,
                                // so the two read as one panel.
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap(theme.space.xs)
                                    .rounded_b(theme.radii.md)
                                    .border(theme.metrics.hairline)
                                    .border_color(theme.colors.border)
                                    .bg(theme.colors.surface)
                                    .px(theme.space.md)
                                    .py(theme.space.sm)
                                    .child(self.composer.clone())
                                    .child(MetadataRow::new(segments, fit).trailing(vec![
                                        MetadataSegment::new("34%"),
                                        MetadataSegment::new("$0.42"),
                                        MetadataSegment::new("48s"),
                                    ])),
                            )
                            .children(
                                self.trigger
                                    .clone()
                                    .map(|trigger| Text::hint(trigger).faint()),
                            ),
                    ),
            )
    }
}

fn main() {
    support::runtime::run(
        "fleet-ui-kit · agent gallery",
        (1400.0, 900.0),
        Quit,
        |cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-t", ToggleTheme, None),
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-e", ToggleExpand, Some("AgentGallery")),
                KeyBinding::new("ctrl-d", CycleDecision, Some("AgentGallery")),
                KeyBinding::new("ctrl-y", ToggleAnswering, Some("AgentGallery")),
                KeyBinding::new("ctrl-s", ToggleScroll, Some("AgentGallery")),
                KeyBinding::new("ctrl-g", JumpToLatest, Some("AgentGallery")),
                KeyBinding::new("ctrl-r", ToggleStreaming, Some("AgentGallery")),
                KeyBinding::new("ctrl-x", ToggleEmpty, Some("AgentGallery")),
                KeyBinding::new("ctrl-w", CycleWidth, Some("AgentGallery")),
            ]);
        },
        AgentGallery::new,
    );
}
