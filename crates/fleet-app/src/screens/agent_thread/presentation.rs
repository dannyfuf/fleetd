//! Copy, badges and key hints derived from agent state (`docs/NATIVE-AGENTS.md` §2, §3.3, §9).
//!
//! Everything here is a pure function of daemon state, so the tab strip, the session header, the
//! context bar and the status bar can never disagree about one thread.

use fleet_core::agents::{
    AgentKind, AgentThreadSummary, Attention, ModelSelection, OpenGate, PermissionMode,
    ThreadProjection,
};
use fleet_ui_kit::{KeyHintRow, decision_key_hints};

use super::rows::{count_label, duration_label};

/// The mark a tab carries for a thread's attention (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabBadge {
    /// Gray spinner: the provider, the turn or a background task is alive.
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
        Attention::Working => TabBadge::Spinner,
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
        Attention::Unread | Attention::Idle => "idle",
    }
}

/// `claude — rounding fix`, or `claude` while the thread has no provider title yet (§2).
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

/// The left half of the 22 px composer metadata row (§2).
#[must_use]
pub(crate) fn metadata_left(projection: &ThreadProjection) -> String {
    let mut parts = vec![agent_label(projection.provider, projection.mode).to_owned()];
    if let Some(ModelSelection { model, effort, .. }) = &projection.model {
        parts.push(model.clone());
        if let Some(effort) = effort {
            parts.push(effort.clone());
        }
    }
    parts.push(mode_label(projection.mode).to_owned());
    parts.join(" \u{b7} ")
}

/// The right half: `context 34% · $0.42 · 48m` (§2), zero-suppressed.
///
/// §2 fixes the shape for *both* providers, so a provider that reports no cost keeps the slot
/// and fills it with an em dash rather than shifting the row to two elements: Claude reading
/// `context 3% · $0.87 · 3m` beside OpenCode reading `context 2% · 3.4s` made the same row look
/// like two different rows.
#[must_use]
pub(crate) fn metadata_right(projection: &ThreadProjection) -> String {
    let mut parts = Vec::new();
    if projection.context_pct > 0.0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a context percentage is rendered as a whole number"
        )]
        let pct = projection.context_pct.round().clamp(0.0, 100.0) as u32;
        parts.push(format!("context {pct}%"));
    }
    let cost = projection
        .cumulative_cost_usd
        .map(|cost| format!("${cost:.2}"));
    let elapsed = elapsed_ms(projection).map(duration_label);
    if cost.is_some() || !parts.is_empty() || elapsed.is_some() {
        parts.push(cost.unwrap_or_else(|| COST_UNREPORTED.to_owned()));
    }
    parts.extend(elapsed);
    if parts.is_empty() && projection.cumulative_usage.total_tokens > 0 {
        parts.push(format!(
            "{} tokens",
            count_label(projection.cumulative_usage.total_tokens)
        ));
    }
    parts.join(" \u{b7} ")
}

/// The placeholder that holds the cost slot open for a provider that reports no cost.
const COST_UNREPORTED: &str = "\u{2014}";

/// How long this thread has been alive, from its first turn to its last applied event.
fn elapsed_ms(projection: &ThreadProjection) -> Option<u64> {
    let started = projection.turns.first()?.started_at;
    let last = projection.last_activity?;
    u64::try_from(last.signed_duration_since(started).num_milliseconds()).ok()
}

/// `agent mode` for Claude; OpenCode names its agent, which is what the design's copy shows.
const fn agent_label(provider: AgentKind, mode: PermissionMode) -> &'static str {
    match (provider, mode) {
        (AgentKind::Claude, _) => "agent mode",
        (AgentKind::OpenCode, PermissionMode::Plan) => "plan agent",
        (AgentKind::OpenCode, _) => "build agent",
    }
}

/// The permission policy spelled out, never abbreviated to a flag name.
const fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "asks before edits",
        PermissionMode::AcceptEdits => "accepts edits",
        PermissionMode::Plan => "plans before editing",
        PermissionMode::FullAccess => "full access",
    }
}

/// The placeholder of the docked composer (§2).
#[must_use]
pub(crate) fn composer_placeholder(provider: AgentKind) -> String {
    format!(
        "Message {}\u{2026} (@ file \u{b7} / command)",
        provider.executable()
    )
}

/// The status-bar hints for the composer's key set (§9), as data.
///
/// The table is returned rather than rendered so the status bar and the tests agree on which
/// set is live; only the last step turns it into a [`KeyHintRow`]. A card's keys are *not* in
/// here: §2 has the status bar mirror the open card, and the card is the one that knows which
/// scope `a` grants and how many options its question has — [`decision_hints`] reads them off it.
///
/// The argument is §3.3's `Working` predicate — the very one `agent_context_chain` picks the
/// key context from — and deliberately not [`Attention`]: a plan gate on a running turn reads
/// `NeedsYou(Plan)` while `Agent > AgentWorking` still owns the keys, so an attention-driven
/// row advertised `⇧⇥ plan mode · / commands · ^s m model`, none of which is bound there
/// (DESIGN-SYSTEM §4: an invalid command is not listed).
#[must_use]
pub(crate) const fn key_hint_set(working: bool) -> &'static [(&'static str, &'static str)] {
    if working {
        &[
            ("esc", "stop"),
            ("\u{23ce}", "queue"),
            ("^s [", "scroll"),
            ("^s F", "terminal"),
        ]
    } else {
        // §9's Idle row, trimmed to what fits: `^s [` and `^s F` are the two commands with
        // no other surface, so they stay and the pickers' own triggers make way.
        &[
            ("\u{23ce}", "send"),
            ("\u{21e7}\u{21e5}", "plan mode"),
            ("/", "commands"),
            ("^s m", "model"),
            ("^s [", "scroll"),
            ("^s F", "terminal"),
        ]
    }
}

/// The same set, rendered for the status bar.
#[must_use]
pub(crate) fn key_hints(working: bool) -> KeyHintRow {
    key_hint_set(working)
        .iter()
        .fold(KeyHintRow::new(), |row, (keys, label)| {
            row.key(*keys, *label)
        })
}

/// The open card's own keys, which the status bar mirrors verbatim (§2).
///
/// Reading them off the card is what keeps the two surfaces from disagreeing about the scope
/// `a` grants — "allow for this session" on Claude, "allow for this directory" on OpenCode —
/// or about how many options a question offers. `cursor` is the question the card's keys
/// currently address: on a 2-4 question gate the advertised `1–N` range and `space` move with
/// it, so a fixed question 0 would put a different key set on the two surfaces.
#[must_use]
pub(crate) fn decision_hints(gate: &OpenGate, provider: AgentKind, cursor: usize) -> KeyHintRow {
    let questions = match &gate.kind {
        fleet_core::agents::GateKind::Question { questions } => questions.len(),
        _ => 0,
    };
    decision_key_hints(&super::decisions::card(
        gate,
        provider,
        &super::decisions::QuestionSelection::at(questions, cursor),
        false,
    ))
}
