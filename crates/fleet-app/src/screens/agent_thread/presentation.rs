//! Copy, badges, metadata segments and key hints derived from agent state (§2, §3.3, §12).
//!
//! Everything here is a pure function of daemon state, so the tab strip, the session header, the
//! context bar and the status bar can never disagree about one thread. The drawer's own keys are
//! deliberately **not** here: §6.2 has the status bar mirror the open decision, and the decision
//! is the one that knows which scope `[a]` grants and how many options its question has.

use fleet_core::{
    agents::{
        AccountStatus, AgentKind, AgentThreadSummary, Attention, ModelSelection, PermissionMode,
        ThreadProjection,
    },
    ids::CardId,
};
use fleet_ui_kit::{MetadataSegment, format_cost, format_duration, format_token_count};
use gpui::SharedString;

use super::composer::{ComposerMode, InteractionMode};

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

/// The provider-independent title a surface places after its own child/provider chrome.
///
/// Delegation creation stores the contract's complete default (`↳ provider — brief`) as the
/// thread title. Tabs, picker rows and delegation rows already render that chrome themselves, so
/// peel it once rather than producing `↳ codex — ↳ codex — brief` on the default path.
#[must_use]
pub(crate) fn title_subject(summary: &AgentThreadSummary) -> String {
    let provider = summary.provider.executable();
    let title = summary.title.trim();
    if summary.parent.is_some() {
        let prefix = format!("\u{21b3} {provider} \u{2014} ");
        if let Some(subject) = title.strip_prefix(&prefix) {
            return subject.trim().to_owned();
        }
    }
    title.to_owned()
}

/// `claude — rounding fix`, or `claude` while the thread has no harness title yet (§2).
fn plain_tab_title(summary: &AgentThreadSummary) -> String {
    let provider = summary.provider.executable();
    let title = title_subject(summary);
    if title.is_empty()
        || title.eq_ignore_ascii_case(provider)
        || title.eq_ignore_ascii_case(summary.provider.display_name())
    {
        return provider.to_owned();
    }
    format!("{provider} \u{2014} {title}")
}

/// `claude — rounding fix`, prefixed by `↳ ` when the thread is a delegated child (§7).
#[must_use]
pub(crate) fn tab_title(summary: &AgentThreadSummary) -> String {
    let title = plain_tab_title(summary);
    if summary.parent.is_some() {
        format!("\u{21b3} {title}")
    } else {
        title
    }
}

/// The child's pinned jump back to its caller.
///
/// `caller_index` is the one-based strip index. A hidden caller has no index and therefore uses
/// the contract's middle dot, while retaining its opaque thread target so activation can attach
/// it before selection.
#[must_use]
pub(crate) fn caller_metadata_segment(
    caller: &AgentThreadSummary,
    caller_index: Option<usize>,
) -> MetadataSegment {
    let position = caller_index.map_or_else(|| "\u{b7}".to_owned(), |index| format!("[{index}]"));
    MetadataSegment::pinned(SharedString::from(format!(
        "for {position} {}",
        plain_tab_title(caller)
    )))
    .target(caller.thread.to_string())
}

/// What a card-called run's thread works for, as the strip has it (contracts §5.5).
///
/// A card is a caller like a thread is: the run it started is an ordinary tab, and the one
/// thing that tab has to say that no other one does is which card, in which column, on which
/// board, is waiting for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CardCaller {
    /// The card itself, which is what the segment's target names.
    pub(crate) card: CardId,
    /// The card's display key, e.g. `FLT-12`.
    pub(crate) key: String,
    /// The column whose action started the run.
    pub(crate) column: String,
    /// The board the card lives on.
    pub(crate) board: String,
}

/// The card run's pinned `for FLT-12 · Ready · Fleet` segment (contracts §5.5).
///
/// Targeted like a thread caller's, under the [`CARD_TARGET_PREFIX`] scheme: clicking it makes
/// the same jump `^s u` makes — this worktree's board tab, with the card selected.
#[must_use]
pub(crate) fn card_metadata_segment(caller: &CardCaller) -> MetadataSegment {
    MetadataSegment::pinned(SharedString::from(format!(
        "for {} \u{b7} {} \u{b7} {}",
        caller.key, caller.column, caller.board
    )))
    .target(format!("{CARD_TARGET_PREFIX}{}", caller.card))
}

/// What marks a metadata target as a card rather than a thread id.
///
/// Thread targets are bare ids, so a prefix is what lets one `on_target` serve both without
/// either being able to be mistaken for the other.
pub(crate) const CARD_TARGET_PREFIX: &str = "card:";

/// The composer prompt of a card run: steering it, not owning what it answers.
///
/// A card run reports to its card, and the card's column is what moves it — so the sentence
/// says where the reply goes, exactly as a subagent's says it reports to its caller.
pub(crate) const CARD_COMPOSER_PLACEHOLDER: &str =
    "Steering a card run. Its report moves the card when it finishes.";

/// The ordinary composer prompt of a delegated child.
#[must_use]
pub(crate) fn child_composer_placeholder(caller_index: Option<usize>) -> String {
    let caller = caller_index.map_or_else(|| "·".to_owned(), |index| format!("[{index}]"));
    format!("Steering a subagent of {caller}. It reports to its caller when it finishes.")
}

/// What the composer's settings strip shows (§2, `spec-B` §B5.1): the model chip, the access
/// chip, Build or Plan, the context meter and the remaining facts.
///
/// **Every value the harness reports is shown and nothing is invented**: a tab that has
/// published no model has no model chip text of its own, and a model with no effort reads as
/// the model alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ComposerControls {
    /// `gpt-5 · high`: the model the next send carries and its effort, when one is known.
    pub(crate) model: Option<SharedString>,
    /// The access ladder a Build send carries, spelled out (`asks before edits`).
    pub(crate) access: SharedString,
    /// Build or Plan, as the next send carries it.
    pub(crate) interaction: InteractionMode,
    /// How much of the window is used: the meter's fill and its `34%`.
    pub(crate) context: Option<(u8, SharedString)>,
    /// The rest of the right half: `$0.42 · 48m`, or nothing until a turn has run.
    pub(crate) facts: Option<SharedString>,
}

/// The composer's settings, from the projection and the draft the next send will carry.
#[must_use]
pub(crate) fn composer_controls(
    projection: &ThreadProjection,
    model: Option<&ModelSelection>,
    access: PermissionMode,
    interaction: InteractionMode,
) -> ComposerControls {
    let model = model.map(|ModelSelection { model, effort, .. }| {
        match effort.as_deref().filter(|effort| !effort.is_empty()) {
            Some(effort) => SharedString::from(format!("{model} \u{b7} {effort}")),
            None => SharedString::new(model.as_str()),
        }
    });
    let context =
        context_percent(projection).map(|pct| (pct, SharedString::from(format!("{pct}%"))));
    let facts = trailing_segments(projection)
        .into_iter()
        .skip(usize::from(context.is_some()))
        .map(|segment| segment.text.to_string())
        .collect::<Vec<_>>();
    ComposerControls {
        model,
        access: SharedString::new_static(mode_label(access)),
        interaction,
        context,
        facts: (!facts.is_empty()).then(|| SharedString::from(facts.join(" \u{b7} "))),
    }
}

/// The context window used, as a whole percentage, once the harness has reported one.
fn context_percent(projection: &ThreadProjection) -> Option<u8> {
    (projection.context_pct > 0.0).then(|| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a context percentage is rendered as a whole number"
        )]
        let pct = projection.context_pct.round().clamp(0.0, 100.0) as u8;
        pct
    })
}

/// The right half: `34% · $0.42 · 48m` (§2), turn-derived and empty until a turn has run.
///
/// A harness that reports no cost contributes **no segment** rather than a placeholder: §B5.1's
/// rule is that nothing is invented, and an em dash is an invention that reads as a number.
#[must_use]
pub(crate) fn trailing_segments(projection: &ThreadProjection) -> Vec<MetadataSegment> {
    let mut segments = Vec::new();
    if let Some(pct) = context_percent(projection) {
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
    // The account goes last, so the row collapses it before it collapses the context meter: which
    // account is answering matters less per-turn than how much of the window is left. A harness
    // that reports none contributes no segment at all (§B5.1).
    if let Some(account) = account_segment(projection) {
        segments.push(MetadataSegment::new(account));
    }
    segments
}

/// `signed out`, or the account's own shortest label, or nothing at all.
fn account_segment(projection: &ThreadProjection) -> Option<SharedString> {
    match projection.account.as_ref()? {
        AccountStatus::SignedOut => Some(SharedString::new_static("signed out")),
        AccountStatus::SignedIn(info) => info.label().map(SharedString::new),
    }
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
        PermissionMode::Auto => "auto-approves safe actions",
        PermissionMode::DontAsk => "denies unlisted tools",
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
            "Message {}\u{2026} @ files \u{b7} $ skills \u{b7} / commands",
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
