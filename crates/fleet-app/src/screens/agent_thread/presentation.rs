//! Copy, badges, metadata segments and key hints derived from agent state (§2, §3.3, §12).
//!
//! Everything here is a pure function of daemon state, so the tab strip, the session header, the
//! context bar and the status bar can never disagree about one thread. The drawer's own keys are
//! deliberately **not** here: §6.2 has the status bar mirror the open decision, and the decision
//! is the one that knows which scope `[a]` grants and how many options its question has.

use fleet_core::agents::{
    AgentKind, AgentThreadSummary, Attention, ModelSelection, OpenGate, PermissionMode,
    ThreadProjection,
};
use fleet_ui_kit::{KeyHintRow, MetadataSegment, format_cost, format_duration, format_token_count};
use gpui::SharedString;

use super::{
    composer::{ComposerMode, InteractionMode},
    decisions::{QuestionWizard, decision_for},
};

/// The mark a tab carries for a thread's attention (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabBadge {
    /// Gray spinner: the harness, the turn or a background task is alive.
    Spinner,
    /// Amber 6 px dot: a gate is open, or a turn finished unseen.
    NeedsYou,
    /// Gray dot: non-terminal output arrived since the thread was last viewed.
    Unread,
    /// `exited <code>` in red.
    Exited(Option<i32>),
    /// No mark.
    None,
}

/// The badge the §3.3 table assigns, with an exit code winning over a bare failure.
#[must_use]
pub(crate) const fn tab_badge(attention: Attention, exit_code: Option<i32>) -> TabBadge {
    match attention {
        Attention::NeedsYou(_) => TabBadge::NeedsYou,
        Attention::Failed => TabBadge::Exited(exit_code),
        // §3.3: a parked usage window is a gray spinner with a countdown, never an amber dot.
        // The thread is waiting on the harness, and nothing the user can do releases it.
        Attention::Working | Attention::Waiting => TabBadge::Spinner,
        Attention::Unread => TabBadge::Unread,
        Attention::Idle => TabBadge::None,
    }
}

/// The session header word: the same vocabulary the context bar counts (§3.3).
#[must_use]
pub(crate) const fn header_word(attention: Attention) -> &'static str {
    match attention {
        Attention::NeedsYou(_) => "needs you",
        Attention::Failed => "failed",
        Attention::Working => "working",
        Attention::Waiting => "waiting",
        Attention::Unread | Attention::Idle => "idle",
    }
}

/// `claude — rounding fix`, or `claude` while the thread has no harness title yet (§2).
#[must_use]
pub(crate) fn tab_title(summary: &AgentThreadSummary) -> String {
    let provider = summary.provider.executable();
    let title = summary.title.trim();
    if title.is_empty()
        || title.eq_ignore_ascii_case(provider)
        || title.eq_ignore_ascii_case(summary.provider.display_name())
    {
        return provider.to_owned();
    }
    format!("{provider} \u{2014} {title}")
}

/// The left half of the 22 px composer metadata row (§2, `spec-B` §B5.1).
///
/// **Every segment the harness reports is shown and no segment is invented**: a fresh Claude tab
/// that has not published an effort has three segments, not four. The model segment is pinned,
/// so a narrow pane truncates its name rather than losing which model is answering.
#[must_use]
pub(crate) fn metadata_segments(
    projection: &ThreadProjection,
    interaction: InteractionMode,
) -> Vec<MetadataSegment> {
    let mut segments = Vec::new();
    if let Some(ModelSelection { model, effort, .. }) = &projection.model {
        segments.push(MetadataSegment::pinned(SharedString::new(model.as_str())));
        if let Some(effort) = effort.as_deref().filter(|effort| !effort.is_empty()) {
            segments.push(MetadataSegment::new(SharedString::new(effort)));
        }
    }
    segments.push(MetadataSegment::new(SharedString::new_static(mode_label(
        projection.mode,
    ))));
    segments.push(MetadataSegment::new(SharedString::new_static(
        match interaction {
            InteractionMode::Build => "build",
            InteractionMode::Plan => "plan",
        },
    )));
    segments
}

/// The right half: `34% · $0.42 · 48m` (§2), turn-derived and empty until a turn has run.
///
/// A harness that reports no cost contributes **no segment** rather than a placeholder: §B5.1's
/// rule is that nothing is invented, and an em dash is an invention that reads as a number.
#[must_use]
pub(crate) fn trailing_segments(projection: &ThreadProjection) -> Vec<MetadataSegment> {
    let mut segments = Vec::new();
    if projection.context_pct > 0.0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a context percentage is rendered as a whole number"
        )]
        let pct = projection.context_pct.round().clamp(0.0, 100.0) as u32;
        segments.push(MetadataSegment::new(SharedString::from(format!("{pct}%"))));
    }
    if let Some(cost) = projection.cumulative_cost_usd {
        segments.push(MetadataSegment::new(format_cost(cost)));
    }
    if let Some(elapsed) = elapsed_ms(projection) {
        segments.push(MetadataSegment::new(format_duration(elapsed)));
    }
    if segments.is_empty() && projection.cumulative_usage.total_tokens > 0 {
        segments.push(MetadataSegment::new(SharedString::from(format!(
            "{} tokens",
            format_token_count(projection.cumulative_usage.total_tokens)
        ))));
    }
    segments
}

/// How long this thread has been alive, from its first turn to its last applied event.
fn elapsed_ms(projection: &ThreadProjection) -> Option<u64> {
    let started = projection.turns.first()?.started_at;
    let last = projection.last_activity?;
    u64::try_from(last.signed_duration_since(started).num_milliseconds()).ok()
}

/// The permission policy spelled out, never abbreviated to a flag name.
#[must_use]
pub(crate) const fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "asks before edits",
        PermissionMode::AcceptEdits => "accepts edits",
        PermissionMode::Plan => "plans before editing",
        PermissionMode::FullAccess => "full access",
    }
}

/// The placeholder of the docked composer, per mode (`spec-B` §B5.2).
#[must_use]
pub(crate) fn composer_placeholder(
    mode: ComposerMode,
    provider: AgentKind,
    payload: Option<&str>,
    choice_only: bool,
) -> String {
    match mode {
        // Under an approval the placeholder *is* the payload under review, because the editor is
        // disabled and the field has nothing else to say.
        ComposerMode::Approval(_) => payload.unwrap_or("").to_owned(),
        ComposerMode::Question(_) if choice_only => "choose an option above".to_owned(),
        ComposerMode::Question(_) => {
            "type your own answer, or leave blank to use the selection".to_owned()
        }
        ComposerMode::PlanFollowUp(_) => {
            "add feedback to refine, or leave blank to implement".to_owned()
        }
        ComposerMode::Normal => format!(
            "message {}\u{2026} (@ files \u{b7} $ skills \u{b7} / commands)",
            provider.executable()
        ),
    }
}

/// The empty thread's invitation: the harness, and the three triggers.
#[must_use]
pub(crate) fn empty_invitation(provider: AgentKind, worktree: &str) -> String {
    format!(
        "new {} thread \u{b7} {worktree}\nask anything \u{b7} @ files \u{b7} $ skills \u{b7} / commands",
        provider.executable()
    )
}

/// The placeholder of a composer whose machine is out of reach.
#[must_use]
pub(crate) fn unreachable_placeholder(host: &str) -> String {
    format!("{host} is unreachable \u{2014} nothing can be sent yet")
}

/// The metadata row's right half while the thread's machine is out of reach.
#[must_use]
pub(crate) fn unreachable_hint(host: &str) -> String {
    format!("{host} unreachable \u{b7} the thread resumes when the link is back")
}

/// The toast a `⏎` earns while the machine is out of reach, naming the draft's fate.
#[must_use]
pub(crate) fn unreachable_notice(host: &str) -> String {
    format!("{host} is unreachable \u{b7} your message is kept in the composer")
}

/// The status-bar hints for the composer's key set (§12), as data.
///
/// The argument is §3.3's `Working` predicate — the very one the key context is picked from —
/// and deliberately not [`Attention`]: a plan gate on a running turn reads `NeedsYou(Plan)`
/// while `Agent > AgentWorking` still owns the keys, so an attention-driven row would advertise
/// commands nothing is bound to (DESIGN-SYSTEM §4: an invalid command is not listed).
#[must_use]
pub(crate) const fn key_hint_set(
    working: bool,
    scrolling: bool,
) -> &'static [(&'static str, &'static str)] {
    if scrolling {
        return &[
            ("j/k", "row"),
            ("\u{23ce}", "expand"),
            ("d", "diff"),
            ("gg/G", "ends"),
            ("q", "leave"),
        ];
    }
    if working {
        &[
            ("esc", "interrupt"),
            ("\u{23ce}", "steer"),
            ("^s [", "scroll"),
            ("^s F", "terminal"),
        ]
    } else {
        &[
            ("\u{23ce}", "send"),
            ("\u{21e7}\u{21e5}", "plan mode"),
            ("^s m", "model"),
            ("^s t", "access"),
            ("^s [", "scroll"),
            ("^s F", "terminal"),
        ]
    }
}

/// The same set, rendered for the status bar.
#[must_use]
pub(crate) fn key_hints(working: bool, scrolling: bool) -> KeyHintRow {
    key_hint_set(working, scrolling)
        .iter()
        .fold(KeyHintRow::new(), |row, (keys, label)| {
            row.key(*keys, *label)
        })
}

/// The open decision's own keys, which the status bar mirrors verbatim (§6.2).
///
/// Reading them off the decision is what keeps the two surfaces from disagreeing about the scope
/// `[a]` grants or about how many options a question offers. `cursor` is the question the keys
/// currently address: on a multi-question request the advertised `1–N` range and `space` move
/// with it, so a fixed question 0 would put a different key set on the two surfaces.
#[must_use]
pub(crate) fn decision_hints(gate: &OpenGate, provider: AgentKind, cursor: usize) -> KeyHintRow {
    let questions = match &gate.kind {
        fleet_core::agents::GateKind::Question { questions } => questions.len(),
        _ => 0,
    };
    decision_for(gate, provider, &QuestionWizard::at(questions, cursor)).key_hints()
}
