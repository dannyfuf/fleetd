//! The visual test bench for the **native-agent** group of `fleet-ui-kit`.
//!
//! `ToolRow` · `DecisionCard` · `TranscriptList` and every row the transcript can draw:
//! user block, assistant text, thinking, tool rows with nested children, worked fold, turn
//! footer, decision cards, error cards, checkpoint lines, queued messages and the empty state.
//!
//! The transcript is *live*: it is a real [`fleet_ui_kit::TranscriptList`] entity over GPUI
//! `list`, so scrolling, the jump-to-latest affordance and the scroll thumb behave here exactly
//! as they do in an agent tab.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_agent
//! ```
//!
//! | Key | What |
//! | --- | --- |
//! | `ctrl-t` | toggle light / dark |
//! | `e` | expand / collapse every expandable row |
//! | `d` | cycle the decision card: permission → question → plan |
//! | `s` | toggle scroll mode (freezes the tail) |
//! | `g` | jump to latest |
//! | `t` | toggle the streaming caret after the last assistant paragraph |
//! | `x` | toggle the empty transcript |
//! | `ctrl-q` / `cmd-q` | quit |

pub mod support;
const LAYOUT: support::layout::GalleryLayout = support::layout::GalleryLayout {
    label_width: 150.0,
    column: false,
    divided: false,
    compact: true,
};
use fleet_ui_kit::prelude::*;
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
        ToggleEmpty
    ]
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Decision {
    Permission,
    Question,
    Plan,
}

impl Decision {
    fn next(self) -> Self {
        match self {
            Decision::Permission => Decision::Question,
            Decision::Question => Decision::Plan,
            Decision::Plan => Decision::Permission,
        }
    }

    fn card(self) -> DecisionCard {
        match self {
            Decision::Permission => DecisionCard::new(
                "gate-permission",
                "claude wants to run a command",
                DecisionCardKind::Permission {
                    tool: "bash".into(),
                    payload: "cargo test -p fleet-core --all-features".into(),
                    rationale: Some("verify the reducer before touching the daemon".into()),
                },
            )
            .actions(permission_actions(
                DecisionAction::AllowSession,
                "allow for this session",
            )),
            Decision::Question => DecisionCard::new(
                "gate-question",
                "claude has a question",
                DecisionCardKind::Question {
                    questions: vec![DecisionQuestion {
                        header: "scope".into(),
                        text: "which rounding should the payroll fix use?".into(),
                        options: vec![
                            "half-up, like the spec".into(),
                            "banker's rounding".into(),
                            "keep the current behaviour".into(),
                        ],
                        multi_select: false,
                        allow_other: true,
                    }],
                },
            )
            .actions(question_actions(3, false)),
            Decision::Plan => DecisionCard::new(
                "gate-plan",
                "claude proposes a plan",
                DecisionCardKind::Plan {
                    markdown: "## plan\n\nadd the reducer test, then the projection".into(),
                    steps: vec![
                        "read the current reducer".into(),
                        "add a failing test for the rounding".into(),
                        "fix the projection and re-run".into(),
                    ],
                },
            )
            .actions(plan_actions()),
        }
    }
}

struct AgentGallery {
    focus_handle: FocusHandle,
    transcript: Entity<TranscriptList>,
    expanded: bool,
    decision: Decision,
    scroll_mode: bool,
    /// Whether the transcript draws the streaming caret (§6.6).
    streaming: bool,
    /// Whether the transcript is showing its empty state instead of the sample turn.
    empty: bool,
}

impl AgentGallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let transcript = cx.new(TranscriptList::new);
        let this = Self {
            focus_handle: cx.focus_handle(),
            transcript,
            expanded: false,
            decision: Decision::Permission,
            scroll_mode: false,
            streaming: false,
            empty: false,
        };
        this.publish(cx);
        this
    }

    fn publish(&self, cx: &mut Context<Self>) {
        let rows = self.rows();
        self.transcript
            .update(cx, |transcript, cx| transcript.set_rows(rows, cx));
    }

    fn rows(&self) -> Vec<TranscriptRow> {
        // DESIGN-SYSTEM §8.3: if a state is not in a gallery, it is not implemented. The
        // empty transcript is the whole surface, not one row among others.
        if self.empty {
            return vec![TranscriptRow::EmptyState {
                message: "New claude session \u{b7} feat/payroll-fix".into(),
            }];
        }
        let expanded = self.expanded;
        vec![
            TranscriptRow::CheckpointLine {
                text: format_resumed(7_200_000),
            },
            TranscriptRow::UserBlock {
                text: "fix the payroll rounding and add a test".into(),
                attachments: vec!["payroll.rs".into(), "spec.md".into()],
            },
            TranscriptRow::Thinking {
                text: SharedString::new_static("the spec says half-up; the reducer truncates"),
                duration_ms: 6_000,
                expanded,
            },
            TranscriptRow::AssistantText {
                markdown: parse_markdown(
                    "Reading the reducer first, then the projection it feeds.",
                ),
            },
            TranscriptRow::ToolRow {
                row: ToolRow::new("t-read", "read", "crates/fleet-core/src/payroll.rs")
                    .state(ToolRowState::Done)
                    .result("120 lines")
                    .output("pub fn round(cents: i64) -> i64 { cents / 100 }")
                    .expanded(expanded),
                children: Vec::new(),
            },
            TranscriptRow::ToolRow {
                row: ToolRow::new("t-agent", "agent", "explore · find every caller")
                    .state(ToolRowState::Done)
                    .result("8 tools · 24s")
                    .expanded(expanded),
                children: vec![
                    TranscriptRow::ToolRow {
                        row: ToolRow::new("t-agent-1", "grep", "round(")
                            .state(ToolRowState::Done)
                            .result("12 hits"),
                        children: Vec::new(),
                    },
                    TranscriptRow::ToolRow {
                        row: ToolRow::new("t-agent-2", "read", "crates/fleet-core/src/tax.rs")
                            .state(ToolRowState::Denied)
                            .result("denied"),
                        children: Vec::new(),
                    },
                ],
            },
            TranscriptRow::ToolRow {
                row: ToolRow::new("t-edit", "edit", "crates/fleet-core/src/payroll.rs")
                    .state(ToolRowState::Done)
                    .result("+14 −3 · [⏎] diff")
                    .diff("@@ -1,3 +1,3 @@\n-cents / 100\n+cents.div_euclid(100)")
                    .expanded(expanded),
                children: Vec::new(),
            },
            TranscriptRow::ToolRow {
                row: ToolRow::new("t-bash", "bash", "cargo test -p fleet-core")
                    .state(ToolRowState::Error)
                    .result("exit 101 · 4.2s"),
                children: Vec::new(),
            },
            TranscriptRow::ToolRow {
                row: ToolRow::new("t-run", "bash", "cargo test -p fleet-core --all-features"),
                children: Vec::new(),
            },
            TranscriptRow::WorkedFold {
                text: format_worked(12_000, 3),
                expanded,
            },
            TranscriptRow::ErrorCard {
                message: "API error 529 — overloaded".into(),
                retrying: true,
            },
            TranscriptRow::TurnFooter {
                text: format_turn_footer(48_000, 12_400, 2, 36, 3),
            },
            TranscriptRow::CheckpointLine {
                text: format_compacted(84_000, Some(12_000)),
            },
            TranscriptRow::Notice {
                text: "Stop hook error occurred \u{b7} ctrl+o to see".into(),
            },
            TranscriptRow::QueuedMessage {
                text: "also update the CHANGELOG".into(),
            },
            TranscriptRow::DecisionCard(self.decision.card()),
        ]
    }

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
        self.decision = self.decision.next();
        self.publish(cx);
        cx.notify();
    }

    fn toggle_scroll(&mut self, _: &ToggleScroll, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_mode = !self.scroll_mode;
        let enabled = self.scroll_mode;
        self.transcript
            .update(cx, |transcript, cx| transcript.scroll_mode(enabled, cx));
        cx.notify();
    }

    /// The streaming caret §6.6 draws after the last assistant paragraph.
    fn toggle_streaming(
        &mut self,
        _: &ToggleStreaming,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.streaming = !self.streaming;
        let streaming = self.streaming;
        self.transcript
            .update(cx, |transcript, cx| transcript.set_streaming(streaming, cx));
        cx.notify();
    }

    fn toggle_empty(&mut self, _: &ToggleEmpty, _window: &mut Window, cx: &mut Context<Self>) {
        self.empty = !self.empty;
        self.publish(cx);
        cx.notify();
    }

    fn jump_to_latest(&mut self, _: &JumpToLatest, _window: &mut Window, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |transcript, cx| transcript.scroll_to_bottom(cx));
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

fn states_section(cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let row =
        |id: &'static str, state: ToolRowState, summary: &'static str, result: &'static str| {
            LAYOUT.labeled(
                id,
                &theme,
                div().w_full().child(tool_row(
                    &ToolRow::new(id, "bash", summary)
                        .state(state)
                        .result(result),
                    cx,
                )),
            )
        };
    let children = vec![
        row("running", ToolRowState::Running, "cargo build", ""),
        row("done", ToolRowState::Done, "cargo test", "exit 0 · 1.2s"),
        row("error", ToolRowState::Error, "cargo clippy", "exit 101"),
        row("denied", ToolRowState::Denied, "rm -rf target", "denied"),
        LAYOUT.labeled(
            "empty state",
            &theme,
            div().w_full().child(
                div()
                    .w_full()
                    .child(Text::ui_strong("New claude session · feat/payroll-fix"))
                    .child(
                        KeyHintRow::new()
                            .key("⏎", "send your first message")
                            .key("⇧⇥", "plan mode first")
                            .key("@", "mention a file")
                            .key("/", "commands"),
                    ),
            ),
        ),
    ];
    LAYOUT.section("tool rows", &theme, children)
}

impl Render for AgentGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mode = if self.scroll_mode { "scroll" } else { "live" };
        div()
            .key_context("AgentGallery")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::toggle_expand))
            .on_action(cx.listener(Self::cycle_decision))
            .on_action(cx.listener(Self::toggle_scroll))
            .on_action(cx.listener(Self::jump_to_latest))
            .on_action(cx.listener(Self::toggle_streaming))
            .on_action(cx.listener(Self::toggle_empty))
            .on_action(cx.listener(Self::quit))
            .size_full()
            .bg(theme.colors.bg)
            .flex()
            .flex_col()
            .gap(theme.space.lg)
            .p(theme.space.xl)
            .child(states_section(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.md)
                    .child(Text::label("transcript"))
                    .child(Text::hint(mode).muted())
                    .child(
                        KeyHintRow::new()
                            .key("e", "expand")
                            .key("d", "decision")
                            .key("s", "scroll mode")
                            .key("g", "jump to latest")
                            .key("t", "streaming")
                            .key("x", "empty"),
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
    }
}

fn main() {
    support::runtime::run(
        "fleet-ui-kit · agent gallery",
        (1100.0, 900.0),
        Quit,
        |cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-t", ToggleTheme, None),
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("e", ToggleExpand, Some("AgentGallery")),
                KeyBinding::new("d", CycleDecision, Some("AgentGallery")),
                KeyBinding::new("s", ToggleScroll, Some("AgentGallery")),
                KeyBinding::new("g", JumpToLatest, Some("AgentGallery")),
                KeyBinding::new("t", ToggleStreaming, Some("AgentGallery")),
                KeyBinding::new("x", ToggleEmpty, Some("AgentGallery")),
            ]);
        },
        AgentGallery::new,
    );
}
