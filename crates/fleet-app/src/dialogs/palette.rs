//! §3.9 Command palette (`:`) — *jump to anything by name, or do the thing whose key I do not

#[cfg(test)]
use fleet_core::agents::Seq;
use fleet_core::{
    agents::{AgentThreadSummary, Attention, AttentionKind, ThreadId},
    ids::{CardId, ContextId, JobId, RepoId, SessionId, WorktreeId},
    sessions::{AgentActivity, SessionKind, SessionState},
};
use fleet_proto::{request::RequestBody, snapshot::Snapshot};
use fleet_ui_kit::{
    Icon, IconSize, InputMode, Spinner, StatusDot, TextInput, TextInputEvent, Tone, prelude::*,
};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    action_catalogue::{self, ActionInfo, Place},
    actions::{board, card_detail, fleet, palette as palette_actions},
    bridge::Bridge,
    dialogs::{
        ConfirmRequest, DialogHost, Dialogs, SessionTransport, notify, open_agent_session,
        open_agent_thread, open_worktree, request_confirm, step, with_host,
    },
    keymap,
    presentation::{FuzzyQuery, SnapshotIndex, pretty_keys, selected_worktree_id},
    screens::agent_thread::presentation::tab_title,
    screens::workspace::status_kind,
    state::{
        AppState, Cursors, HubPane, HubTab, Overlay, RepoScope, Screen, latest_failed_job,
        running_jobs,
    },
};

/// The palette's total row cap (§3.9).
pub const ROW_CAP: usize = 10;
/// How many rows each section shows on an empty query (§3.9 "States").
pub const IDLE_ROWS: usize = 5;
const ATTENTION_MARK_SIZE: f32 = 16.0;

/// The palette's draft.
#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    /// The query, mirrored from the live editor the host owns.
    pub(crate) query: String,
    /// The flat cursor across all sections.
    pub(crate) cursor: usize,
    rows: std::rc::Rc<[Entry]>,
    total: usize,
    /// Every input [`candidates`] read to build [`Self::rows`], so a notification that changed
    /// none of them does not rebuild them.
    prepared: Option<PreparedKey>,
}

/// Every input the prepared rows are derived from, as revisions and cheap values.
///
/// `refresh` runs on every `AppState` notification while the palette is open (`host::watch`),
/// and `candidates` indexes the whole snapshot and allocates a label and a detail per row, so
/// the rebuild is keyed the way the Hub and the board key their projections
/// (`docs/APP-CONTRACTS.md`, "render prepares nothing").
///
/// The palette is modal, so only the daemon-driven halves of this key can move while it is
/// open; the rest is here because a key that omits an input the rows are derived from is a
/// palette that keeps drawing rows the state no longer has.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedKey {
    /// `AppState::snapshot_revision`: sessions, worktrees, repos, jobs and contexts.
    snapshot: u64,
    /// Native-thread summary and installation-local cursor generations.
    agent_summaries: u64,
    agent_seen: u64,
    /// The local tab-set choice is independent of the daemon snapshot.
    attached: u64,
    /// `BoardState::revision`: a card edit never lands through a snapshot.
    board: u64,
    /// The board's selection and filter, which decide which card the `Board:` rows act on.
    board_focus: crate::state::BoardFocus,
    board_filter: String,
    /// `AppState::link_generation` and the link itself, which gate every connected command.
    connection: u64,
    connected: bool,
    /// What the `DO` rows are judged against besides the snapshot.
    screen: Screen,
    hub_pane: HubPane,
    pr_tab: fleet_core::github::PrTab,
    scope: RepoScope,
    cursors: Cursors,
    /// Whether `Update Fleet` has a version to offer.
    update: bool,
    /// The typed query, and the dialog the palette replaced.
    query: String,
    behind: Option<Dialogs>,
    detail_card: Option<CardId>,
}

impl PreparedKey {
    fn new(
        state: &AppState,
        query: String,
        behind: Option<Dialogs>,
        detail_card: Option<CardId>,
    ) -> Self {
        Self {
            snapshot: state.snapshot_revision,
            agent_summaries: state.agents.summaries_revision(),
            agent_seen: state.agents.seen_revision(),
            attached: state.agents.attached_revision(),
            board: state.board.revision,
            board_focus: state.board.focus,
            board_filter: state.board.filter.clone(),
            connection: state.link_generation,
            connected: state.daemon.is_connected(),
            screen: state.screen.clone(),
            hub_pane: state.hub_pane,
            pr_tab: state.pr_tab,
            scope: state.scope.clone(),
            cursors: state.cursors.clone(),
            update: state.update_version.is_some(),
            query,
            behind,
            detail_card,
        }
    }
}

/// What a palette row does when `Enter` runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// Select one native thread, attaching or reopening its tab first when needed.
    OpenAgentThread(ThreadId),
    /// Wake and open a fixed agent session.
    OpenSession {
        session: SessionId,
        agent: fleet_core::config::Agent,
    },
    /// Open (or create) a worktree's session.
    OpenWorktree(WorktreeId),
    /// Scope the Hub to a repository.
    SelectRepo(RepoId),
    /// Switch the active context.
    SwitchContext(ContextId),
    /// Run a command.
    Command(Command),
    /// Cancel a background job.
    CancelJob(JobId),
}

/// One palette row, already resolved against the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Which section it belongs to.
    pub(crate) section: PaletteSectionKind,
    /// The row label, which is also what the query matches against.
    pub(crate) label: String,
    /// The muted right-hand description.
    pub(crate) detail: Option<String>,
    /// The muted second line.
    pub(crate) secondary: Option<String>,
    /// The right-aligned row verb.
    pub(crate) trailing: Option<String>,
    /// The bound key, right-aligned.
    pub(crate) key: Option<String>,
    /// Whether the row is prefixed with `triangle-alert`.
    pub(crate) destructive: bool,
    /// The glyph, when it is not derived from a session state.
    pub(crate) icon: Icon,
    /// The §2.5 glyph this row wears, for `GO` rows.
    ///
    /// It is a resolved [`StatusKind`] and not a raw [`SessionState`] on purpose: `detached`
    /// alone cannot tell `circle` from `moon`, and §5 invariant 1 requires the palette to
    /// draw exactly the glyph the Hub draws for the same worktree.
    pub(crate) status: Option<StatusKind>,
    /// The native-thread attention mark, when this is an `AGENTS` row.
    pub(crate) attention: Option<Attention>,
    /// What `Enter` does.
    pub(crate) run: Run,
}

mod command;
mod rows;
#[cfg(test)]
mod tests;

pub use command::Command;
use command::{CardContext, card_context};
use rows::candidates;

/// Renders the palette overlay (§3.9): the palette card inside the kit's top-anchored
/// [`fleet_ui_kit::Overlay`].
pub(super) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (query, cursor, rows, total, input) = {
        let host = host.read(cx);
        (
            host.palette.query.clone(),
            host.palette.cursor,
            host.palette.rows.clone(),
            host.palette.total,
            host.palette_input.clone(),
        )
    };
    let Some(input) = input else {
        // `seed` runs before the first paint of an open palette; without its editor there is
        // nothing to draw and nothing to type into.
        return div().track_focus(focus).size_full().into_any_element();
    };

    let windowed = is_session_switcher(&query) || is_agents_picker(&query);
    let (visible_rows, visible_cursor) = visible_rows(&rows, cursor, windowed);
    let mut card = fleet_ui_kit::Palette::new(input)
        .cursor(visible_cursor)
        .cap(ROW_CAP)
        .total(total)
        .empty(format!("Nothing matches \"{query}\"."));
    for kind in [
        PaletteSectionKind::Go,
        PaletteSectionKind::Do,
        PaletteSectionKind::Context,
        PaletteSectionKind::Agents,
    ] {
        let section: Vec<PaletteRow> = visible_rows
            .iter()
            .filter(|entry| entry.section == kind)
            .enumerate()
            .map(|(index, entry)| {
                let mut row = PaletteRow::new(entry.label.clone())
                    .destructive(entry.destructive)
                    .icon(entry.icon);
                if let Some(status) = entry.status {
                    row = row.leading(StatusGlyph::new(status).id(gpui::SharedString::from(
                        format!("palette-glyph-{}", entry.label),
                    )));
                } else if let Some(attention) = entry.attention {
                    row = row.leading(attention_mark(index, attention));
                }
                if let Some(detail) = entry.detail.clone() {
                    row = row.detail(detail);
                }
                if let Some(secondary) = entry.secondary.clone() {
                    row = row.secondary(secondary);
                }
                if let Some(trailing) = entry.trailing.clone() {
                    row = row.trailing(trailing);
                }
                if let Some(key) = entry.key.clone() {
                    row = row.key(key);
                }
                row
            })
            .collect();
        if !section.is_empty() {
            card = card.section(PaletteSection::new(kind, section));
        }
    }

    let (top, width) = {
        let theme = cx.theme();
        (theme.metrics.palette_top, theme.metrics.palette_w)
    };
    let run_state = state.clone();
    let run_bridge = bridge.clone();

    div()
        .track_focus(focus)
        .size_full()
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::CursorDown, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::CursorUp, _window, cx| move_cursor(&state, -1, cx)
        })
        .on_action(move |_: &palette_actions::Run, window, cx| {
            run_selected(&run_state, &run_bridge, window, cx);
        })
        .child(
            fleet_ui_kit::Overlay::new()
                .top(top)
                .width(width)
                .scrim(true)
                .dismiss_action(Box::new(palette_actions::Close))
                .content(card),
        )
        .into_any_element()
}

fn attention_mark(index: usize, attention: Attention) -> AnyElement {
    if !attention_has_visible_mark(attention) {
        return div()
            .w(px(ATTENTION_MARK_SIZE))
            .h(px(ATTENTION_MARK_SIZE))
            .into_any_element();
    }
    match attention {
        Attention::Working | Attention::Waiting => Spinner::new(("palette-agent", index))
            .size(IconSize::Small)
            .tone(Tone::Secondary)
            .into_any_element(),
        Attention::NeedsYou(_) => StatusDot::small(Tone::Warning).into_any_element(),
        Attention::Unread => StatusDot::small(Tone::Secondary).into_any_element(),
        Attention::Failed => Icon::CircleX
            .el()
            .size(IconSize::Small)
            .tone(Tone::Danger)
            .into_any_element(),
        Attention::Idle => div()
            .w(px(ATTENTION_MARK_SIZE))
            .h(px(ATTENTION_MARK_SIZE))
            .into_any_element(),
    }
}

const fn attention_has_visible_mark(attention: Attention) -> bool {
    !matches!(attention, Attention::Idle)
}

fn visible_rows(rows: &[Entry], cursor: usize, windowed: bool) -> (&[Entry], usize) {
    if !windowed || rows.len() <= ROW_CAP {
        return (rows, cursor);
    }
    let cursor = cursor.min(rows.len() - 1);
    let start = cursor.saturating_sub(ROW_CAP - 1).min(rows.len() - ROW_CAP);
    (&rows[start..start + ROW_CAP], cursor - start)
}

/// Builds the query editor and resets the draft when the palette opens.
///
/// The editor lives exactly as long as the palette: `host::close_with` drops it with the rest
/// of the drafts, so a reopened palette never inherits the last one's text, selection or undo
/// history.
pub(super) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let seed = state.update(cx, |app, _| app.palette_seed.take());
    if with_host(state, cx, |host| host.palette_open) {
        refresh(state, cx);
        return;
    }
    let query = seed.unwrap_or_default();
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        // §3.9's query row is the palette's own 44 px chrome, so the editor brings no box.
        input.set_embedded(true, cx);
        input.set_placeholder("go to, or do", cx);
        input.set_text(query.clone(), cx);
        input
    });
    // Weak, like every other editor subscription: the handle lives on `DialogHost`, which
    // `AppState` owns, so a strong capture here would be a cycle holding the app alive.
    let watched = state.downgrade();
    let subscription = cx.subscribe(&input, move |input, event: &TextInputEvent, cx| {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(watched) = watched.upgrade() else {
            return;
        };
        let query = input.read(cx).text().to_owned();
        with_host(&watched, cx, |host| {
            if host.palette.query == query {
                return;
            }
            host.palette.query = query;
            // A re-ranked palette must never keep a cursor past the end of its new rows.
            host.palette.cursor = 0;
        });
        notify(&watched, cx);
    });
    with_host(state, cx, |host| {
        host.palette = PaletteState {
            query,
            ..PaletteState::default()
        };
        host.palette_input = Some(input);
        host.palette_input_subscription = Some(subscription);
        host.palette_open = true;
    });
    refresh(state, cx);
}

pub(super) fn refresh(state: &Entity<AppState>, cx: &mut App) {
    // The backdrop and the card the detail is holding decide which `Board:` and `Card detail:`
    // rows exist at all, so they are read with the query the rows are prepared from.
    let (query, behind, detail_card) = with_host(state, cx, |host| {
        (
            host.palette.query.clone(),
            host.behind_palette.clone(),
            host.card_detail.card_id.clone(),
        )
    });
    let key = PreparedKey::new(state.read(cx), query, behind, detail_card);
    // This runs on every `AppState` notification while the palette is open, and building the
    // candidates indexes the whole snapshot: a notification that moved none of the inputs the
    // rows came from does no work at all.
    if with_host(state, cx, |host| {
        host.palette.prepared.as_ref() == Some(&key)
    }) {
        return;
    }
    let rows = candidates(
        state.read(cx),
        &key.query,
        key.behind.clone(),
        key.detail_card.as_ref(),
    );
    let total = rows.len();
    let cap = if is_session_switcher(&key.query) || is_agents_picker(&key.query) {
        usize::MAX
    } else {
        ROW_CAP
    };
    let rows: Vec<Entry> = rows.into_iter().take(cap).collect();
    with_host(state, cx, |host| {
        // A revision moves for changes the palette does not list, so rows that came out the
        // same keep the `Rc` the card already drew instead of a fresh one.
        if host.palette.rows.as_ref() != rows.as_slice() {
            host.palette.rows = rows.into();
        }
        host.palette.total = total;
        host.palette.prepared = Some(key);
        // A snapshot can lose rows under an open palette; an unclamped cursor would point past
        // the end and make `Enter` a silent no-op (§3.9).
        host.palette.cursor = step(host.palette.cursor, 0, host.palette.rows.len());
    });
}

pub(super) fn refresh_query(state: &Entity<AppState>, cx: &mut App) {
    // Every draft edit reaches here through `dialogs::notify`; `refresh` itself is what decides
    // whether the typed character changed anything the rows are derived from.
    if with_host(state, cx, |host| host.palette_open) {
        refresh(state, cx);
    }
}

fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
        host.palette.cursor = step(host.palette.cursor, delta, host.palette.rows.len());
    });
    notify(state, cx);
}

/// `Enter`: close the palette, then do what the row says.
fn run_selected<T: SessionTransport>(
    state: &Entity<AppState>,
    transport: &T,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(entry) = with_host(state, cx, |host| {
        host.palette.rows.get(host.palette.cursor).cloned()
    }) else {
        return;
    };
    // Read before closing: the palette replaced whatever dialog was open, and closing it is
    // what makes the host forget which one that was.
    let behind = super::with_host(state, cx, |host| host.behind_palette.clone());
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
    match entry.run {
        Run::OpenAgentThread(thread) => open_agent_thread(thread, state, transport, cx),
        Run::OpenSession { agent, .. } => open_agent_session(agent, state, transport, cx),
        Run::OpenWorktree(id) => open_worktree(id, state, transport, cx),
        Run::SelectRepo(repo) => {
            state.update(cx, |app, cx| {
                app.scope = RepoScope::Repo(repo);
                app.screen = Screen::hub();
                cx.notify();
            });
        }
        Run::SwitchContext(context) => {
            transport.send(RequestBody::SetActiveContext { id: Some(context) });
        }
        Run::CancelJob(job) => transport.send(RequestBody::CancelJob { job }),
        Run::Command(command) => run_command(command, behind, state, transport, window, cx),
    }
}

/// Runs one command. Destructive rows open their confirm rather than acting (§3.9).
fn run_command<T: SessionTransport>(
    command: Command,
    behind: Option<Dialogs>,
    state: &Entity<AppState>,
    transport: &T,
    window: &mut Window,
    cx: &mut App,
) {
    let open = |dialog: Dialogs, cx: &mut App| {
        if dialog == Dialogs::CardDetail {
            // Re-seeding a detail the palette was opened over throws away the edit the command
            // is about to act on, which is every text row's `ctrl-s`.
            if behind.as_ref() != Some(&Dialogs::CardDetail) {
                super::card_detail::seed(state, cx);
            }
            super::with_host(state, cx, |host| host.open = Some(Dialogs::CardDetail));
        }
        state.update(cx, |app, cx| {
            app.open_overlay(Overlay::Dialog(dialog));
            cx.notify();
        });
    };
    match command {
        Command::BoardGoBoard => window.dispatch_action(Box::new(board::GoBoard), cx),
        Command::BoardPrevColumn => window.dispatch_action(Box::new(board::PrevColumn), cx),
        Command::BoardNextColumn => window.dispatch_action(Box::new(board::NextColumn), cx),
        Command::BoardNextCard => window.dispatch_action(Box::new(board::NextCard), cx),
        Command::BoardPrevCard => window.dispatch_action(Box::new(board::PrevCard), cx),
        Command::BoardOpenCard => window.dispatch_action(Box::new(board::OpenCard), cx),
        Command::BoardNewCard => window.dispatch_action(Box::new(board::NewCard), cx),
        Command::BoardPickStatus => window.dispatch_action(Box::new(board::PickStatus), cx),
        Command::BoardPickPriority => window.dispatch_action(Box::new(board::PickPriority), cx),
        Command::BoardPickAssignee => window.dispatch_action(Box::new(board::PickAssignee), cx),
        Command::BoardPickLabels => window.dispatch_action(Box::new(board::PickLabels), cx),
        Command::BoardPickEstimate => window.dispatch_action(Box::new(board::PickEstimate), cx),
        Command::BoardMovePrevColumn => window.dispatch_action(Box::new(board::MovePrevColumn), cx),
        Command::BoardMoveNextColumn => window.dispatch_action(Box::new(board::MoveNextColumn), cx),
        Command::BoardCreateWorktree => window.dispatch_action(Box::new(board::CreateWorktree), cx),
        Command::BoardOpenWorktree => window.dispatch_action(Box::new(board::OpenWorktree), cx),
        Command::BoardSync => window.dispatch_action(Box::new(board::Sync), cx),
        Command::BoardFullSync => window.dispatch_action(Box::new(board::FullSync), cx),
        Command::BoardOpenRemote => window.dispatch_action(Box::new(board::OpenRemote), cx),
        Command::BoardDeleteCard => window.dispatch_action(Box::new(board::DeleteCard), cx),
        Command::BoardSettings => window.dispatch_action(Box::new(board::Settings), cx),
        Command::BoardReload => window.dispatch_action(Box::new(board::Reload), cx),
        Command::BoardFilter => window.dispatch_action(Box::new(board::Filter), cx),
        // The keys' own handlers, never a copy: every refusal sentence and every confirm the
        // key raises is raised here too (P9-T04).
        Command::BoardAttachRun => window.dispatch_action(Box::new(board::AttachRun), cx),
        Command::BoardCancelRun => window.dispatch_action(Box::new(board::CancelRun), cx),
        Command::BoardRunNow => window.dispatch_action(Box::new(board::RunNow), cx),
        Command::BoardPickBlockedBy => window.dispatch_action(Box::new(board::PickBlockedBy), cx),
        Command::BoardPickAgent => window.dispatch_action(Box::new(board::PickAgent), cx),
        Command::BoardColumns => window.dispatch_action(Box::new(board::Columns), cx),
        Command::CardDetailClose => {
            // Like every other row here: the dialog comes back, and `Close` then does to it
            // exactly what `Esc` would — cancel the open edit, or close the dialog.
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::Close), cx);
        }
        Command::CardDetailEditTitle => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::EditTitle), cx);
        }
        Command::CardDetailEditDescription => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::EditDescription), cx);
        }
        Command::CardDetailAddComment => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::AddComment), cx);
        }
        Command::CardDetailNextProperty => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::NextProperty), cx);
        }
        Command::CardDetailPrevProperty => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::PrevProperty), cx);
        }
        Command::CardDetailEditProperty => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::EditProperty), cx);
        }
        Command::CardDetailCreateWorktree => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::CreateWorktree), cx);
        }
        Command::CardDetailOpenRemote => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::OpenRemote), cx);
        }
        Command::CardDetailKeepLocal => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::KeepLocal), cx);
        }
        Command::CardDetailTakeRemote => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::TakeRemote), cx);
        }
        Command::CardDetailSave => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::Save), cx);
        }

        Command::WorkspaceOpenBoard => {
            window.dispatch_action(Box::new(crate::actions::prefix::OpenBoard), cx);
        }
        Command::NewWorktree => open(Dialogs::CreateWorktree, cx),
        Command::CloneRepo => open(Dialogs::CloneRepo, cx),
        Command::MoveRepo => open(Dialogs::AssignRepo, cx),
        Command::NewContext => open(Dialogs::NewContext, cx),
        Command::EditContext => open(Dialogs::EditContext, cx),
        Command::Settings => open(Dialogs::Settings, cx),
        Command::Help => open(Dialogs::Help, cx),
        Command::JobsPanel => {
            state.update(cx, |app, cx| {
                app.open_overlay(Overlay::Jobs);
                cx.notify();
            });
        }
        Command::DeleteWorktree => {
            if let Some(id) = cursor_worktree(state, cx) {
                request_confirm(cx, ConfirmRequest::DeleteWorktree { id });
                open(Dialogs::Confirm, cx);
            }
        }
        Command::PruneWorktrees => {
            if let Some(repo) = crate::dialogs::focused_repo(state.read(cx)) {
                request_confirm(cx, ConfirmRequest::Prune { repo });
                open(Dialogs::Confirm, cx);
            }
        }
        Command::KillSession => {
            let request = kill_request(state.read(cx));
            if let Some(request) = request {
                request_confirm(cx, request);
                open(Dialogs::Confirm, cx);
            }
        }
        Command::DeleteContext => {
            let request = state.read(cx).snapshot.as_ref().and_then(|snapshot| {
                let id = state.read(cx).active_context()?.clone();
                let context = snapshot.contexts.iter().find(|entry| entry.id == id)?;
                let repos: Vec<_> = snapshot
                    .repos
                    .iter()
                    .filter(|repo| repo.context_id == id)
                    .collect();
                let worktrees = snapshot
                    .worktrees
                    .iter()
                    .filter(|worktree| repos.iter().any(|repo| repo.id == worktree.repo_id))
                    .count();
                Some(ConfirmRequest::DeleteContext {
                    context: id.clone(),
                    name: context.name.clone(),
                    repos: repos.len(),
                    worktrees,
                    sessions: snapshot.sessions.len(),
                })
            });
            if let Some(request) = request {
                request_confirm(cx, request);
                open(Dialogs::Confirm, cx);
            }
        }
        Command::SleepSession => {
            if let Some(id) = cursor_worktree(state, cx) {
                transport.send(RequestBody::SleepWorktree { id });
            }
        }
        Command::InspectWorktree => {
            if let Some(id) = cursor_worktree(state, cx) {
                transport.send(RequestBody::InspectWorktrees {
                    ids: vec![id],
                    repo: None,
                    fetch: true,
                });
            }
        }
        Command::PullRequests => {
            state.update(cx, |app, cx| {
                app.screen = Screen::Hub { tab: HubTab::Prs };
                cx.notify();
            });
        }
        Command::Worktrees => {
            state.update(cx, |app, cx| {
                app.screen = Screen::hub();
                cx.notify();
            });
        }
        Command::Refresh => transport.send(RequestBody::RefreshStatuses { repo: None }),
        Command::UpdateFleet => transport.send(RequestBody::Update),
        Command::OpenClaude => window.dispatch_action(Box::new(fleet::OpenAgentClaude), cx),
        Command::OpenCodex => window.dispatch_action(Box::new(fleet::OpenAgentCodex), cx),
        // The shell owns the whole quit flow, and its listeners sit on the window root, which
        // is an ancestor of this overlay — so dispatching reaches them.
        Command::Quit => window.dispatch_action(Box::new(fleet::Quit), cx),
        Command::QuitDaemon => window.dispatch_action(Box::new(fleet::QuitAndStopDaemon), cx),
    }
}

/// The worktree the Hub's cursor is on.
fn cursor_worktree(state: &Entity<AppState>, cx: &App) -> Option<WorktreeId> {
    selected_worktree_id(state.read(cx))
}

fn selected_worktree(state: &AppState) -> Option<&fleet_core::model::Worktree> {
    let selected = selected_worktree_id(state)?;
    state
        .snapshot
        .as_ref()?
        .worktrees
        .iter()
        .find(|worktree| worktree.id == selected)
}

fn target_session(state: &AppState) -> Option<&fleet_core::sessions::Session> {
    if let Some(session) = state.active_session() {
        return Some(session);
    }
    let worktree = selected_worktree(state)?;
    state.snapshot.as_ref()?.sessions.iter().find(|session| {
        session.id.as_str() == worktree.session
            || matches!(&session.kind, SessionKind::Worktree(id) if id == &worktree.id)
    })
}

fn kill_request(state: &AppState) -> Option<ConfirmRequest> {
    let session = target_session(state)?;
    Some(ConfirmRequest::KillSession {
        session: session.id.clone(),
        terminals: session.terminals.len(),
        running: session
            .terminals
            .iter()
            .flat_map(|terminal| terminal.keep_alive.clone())
            .collect(),
        unsaved: false,
    })
}

fn is_session_switcher(query: &str) -> bool {
    query.trim() == "sessions"
}

fn is_agents_picker(query: &str) -> bool {
    agents_picker_filter(query).is_some()
}

fn agents_picker_filter(query: &str) -> Option<&str> {
    let query = query.trim();
    let rest = query.strip_prefix("agents")?;
    (rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace)).then(|| rest.trim())
}
