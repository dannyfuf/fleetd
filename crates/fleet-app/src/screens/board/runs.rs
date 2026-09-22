//! The three run keys — `A` attach, `X` cancel, `>` run now (contracts §5.5).
//!
//! They are the only board keys that act on a card's *run* rather than its fields, and each of
//! them can be pressed on a card that has nothing for it to do. The resolution of "which card,
//! and what does it have" is therefore a pure function of the mirror ([`target`]), and the
//! refusal each key answers with is a sentence that function hands back — so the key, the
//! palette row and the test all read the same words.
//!
//! Nothing here starts, cancels or attaches anything by itself: `>` and `X` ask the daemon and
//! draw what comes back (`docs/APP-CONTRACTS.md`), and `A` selects a thread the daemon already
//! owns, exactly as a finished delegated child is selected.

use super::{actions::needs_named, *};
use fleet_core::agents::ThreadId;

/// What `A` answers on a card that has never run at all.
const NO_RUN: &str = "has no run";
/// What `A` answers when the card has run but no run of it reached a thread.
///
/// The CLI's `card attach` refuses in these same words (`commands/board.rs`): §5.5 asks the two
/// surfaces to say the same thing about the same card, and a start that never reached a provider
/// is still a run the card carries — the detail is where its failure is read — so it is not the
/// sentence a card that never ran gets.
const NO_THREAD: &str = "'s runs never reached a thread";
/// What `X` answers on a card whose newest run has already ended.
///
/// The daemon's own `NotFound` sentence, so the CLI and the app refuse in the same words.
const NO_LIVE_RUN: &str = "has no live run";
/// What `>` answers in a column that runs nothing.
const NO_ACTION: &str = "has no action";

/// What the run keys need to know about the card they act on, read once from the mirror.
///
/// Borrowed nothing: a key handler reads the state, decides, and then sends — and the send
/// needs the mirror again, so carrying ids and already-built sentences out of the read is what
/// keeps the borrow short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RunTarget {
    /// The card itself.
    card: CardId,
    /// Its display key, which every refusal names.
    key: String,
    /// The thread of the card's live run, else of its newest run that reached one.
    thread: Option<ThreadId>,
    /// Whether a run is live, or owed and waiting for a slot: what `X` can stop.
    live: bool,
    /// Whether the card carries any run row at all, which is what tells `A`'s two refusals apart.
    has_runs: bool,
    /// The column the card sits in.
    column: String,
    /// Whether that column has an `on_enter` action for `>` to ask for.
    has_action: bool,
}

impl RunTarget {
    /// The thread `A` attaches, or the sentence it refuses with.
    ///
    /// A run whose start failed never reached a thread ([`CardRun::failed_to_start`]), so there
    /// is nothing to open — but the card did run, and saying so is what sends the reader to the
    /// detail, where the failure itself is.
    fn to_attach(&self) -> Result<ThreadId, String> {
        self.thread.ok_or_else(|| {
            if self.has_runs {
                format!("{}{NO_THREAD}", self.key)
            } else {
                format!("{} {NO_RUN}", self.key)
            }
        })
    }

    /// The card `X` cancels, or the sentence it refuses with.
    fn to_cancel(&self) -> Result<CardId, String> {
        if self.live {
            Ok(self.card.clone())
        } else {
            Err(format!("{} {NO_LIVE_RUN}", self.key))
        }
    }

    /// The card `>` runs, or the sentence it refuses with.
    ///
    /// The column is what refuses, not the card: `>` in a column that runs nothing would start
    /// a run no automation asked for, and naming the column says where to go and change that.
    fn to_run(&self) -> Result<CardId, String> {
        if self.has_action {
            Ok(self.card.clone())
        } else {
            Err(format!("{} {NO_ACTION}", self.column))
        }
    }
}

/// The card the run keys act on, and what it has: the detail's own card while the dialog is up,
/// else the board's cursor.
///
/// The same rule the pickers follow ([`super::actions::picker_target`]): a refresh that lands
/// while the detail is open can move another card under the cursor, and a key pressed on the
/// dialog has to keep acting on the card the dialog is showing.
#[must_use]
pub(super) fn target(
    app: &AppState,
    from_detail: bool,
    detail: Option<&CardId>,
) -> Option<RunTarget> {
    let selected = selected_card(app).map(|card| card.id.clone());
    let id = super::actions::picker_target(selected, from_detail, detail)?;
    let view = app.board()?;
    let card = view.cards.iter().find(|card| card.id == id)?;
    let status = view
        .board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id);
    // The live run wins over the newest one: a card that started again is read where it is
    // working, not where it last stopped.
    let run = card
        .runs
        .iter()
        .rev()
        .find(|run| run.is_live())
        .or_else(|| card.runs.last());
    Some(RunTarget {
        card: card.id.clone(),
        key: card.display_key(&view.board),
        thread: run.and_then(|run| run.thread_id),
        // The same mirror-first rule the tile mark and the move confirm read
        // (`actions::has_live_or_pending_run`): a card whose child the mirror holds live has a
        // run to stop even before its own `runs` row carries it, so `X` and the face cannot
        // disagree about one card.
        live: super::actions::has_live_or_pending_run(app, card),
        has_runs: !card.runs.is_empty(),
        column: status.map_or_else(
            || card.status_id.as_str().to_owned(),
            |status| status.name.clone(),
        ),
        has_action: status.is_some_and(|status| {
            status
                .automation
                .as_ref()
                .is_some_and(|automation| automation.on_enter.is_some())
        }),
    })
}

/// `A` — the card's run as an ordinary agent tab (contracts §5.5).
///
/// The thread is one the daemon already owns and already lists, so this attaches and selects it
/// exactly as `^s u` and a delegation row do; the transcript, the composer and the tab strip
/// are the ordinary ones. Opened from the detail, the dialog closes first: the tab it opens
/// would otherwise be drawn behind a dialog that still owns the keyboard.
pub(crate) fn attach_run(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some(target) = resolve(state, cx) else {
        return;
    };
    let thread = match target.to_attach() {
        Ok(thread) => thread,
        Err(message) => {
            needs_named(state, message, cx);
            return;
        }
    };
    if state.read(cx).overlay == Some(Overlay::Dialog(Dialogs::CardDetail)) {
        dialogs::card_detail::close(state, bridge, cx);
    }
    let reopen = bridge.clone();
    // The workspace's own focus reconciliation moves the keyboard onto the tab this selects,
    // through the one-shot composer focus `activate` sets.
    crate::screens::workspace::reopen_agent_tab(
        state,
        thread,
        move |command| reopen.send_agent(command),
        cx,
    );
}

/// `X` — cancel the card's live run, or drop the slot it waits for (contracts §5.5).
pub(crate) fn cancel_run(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let Some(target) = resolve(state, cx) else {
        return;
    };
    let card = match target.to_cancel() {
        Ok(card) => card,
        Err(message) => {
            needs_named(state, message, cx);
            return;
        }
    };
    // The key owns which card and whether it may at all; P9-T03's confirm owns what exactly
    // stops and whether the user said yes, and it is what sends `CardRunCancel`.
    super::actions::ask_cancel_run(state, &card, cx);
}

/// `>` — ask the column to run its action on this card now (contracts §5.5).
///
/// Re-running a card that has already run is the same request: the daemon owns the throttle,
/// the slot limit and the refusal, and the app only says what it wants and draws the answer.
pub(crate) fn run_now(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let Some(target) = resolve(state, cx) else {
        return;
    };
    let card_id = match target.to_run() {
        Ok(card) => card,
        Err(message) => {
            needs_named(state, message, cx);
            return;
        }
    };
    // A refusal belongs where the user is reading the card: on the detail's error line while
    // the dialog is up, in the sticky slot when the board itself asked.
    let refusal = if state.read(cx).overlay == Some(Overlay::Dialog(Dialogs::CardDetail)) {
        Refusal::CardDetail(card_id.clone())
    } else {
        Refusal::Sticky
    };
    send_card_reporting(
        state,
        bridge,
        RequestBody::CardRunStart { card_id },
        refusal,
        cx,
    );
}

/// Reads the target out of the mirror, saying so when no card is focused at all.
fn resolve(state: &Entity<AppState>, cx: &mut App) -> Option<RunTarget> {
    let from_detail = state.read(cx).overlay == Some(Overlay::Dialog(Dialogs::CardDetail));
    let detail = dialogs::with_host(state, cx, |host| host.card_detail.card_id.clone());
    let resolved = target(state.read(cx), from_detail, detail.as_ref());
    if resolved.is_none() {
        needs_named(state, super::actions::NO_CARD.to_owned(), cx);
    }
    resolved
}

#[cfg(test)]
mod tests;
