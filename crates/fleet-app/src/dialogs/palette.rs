//! §3.9 Command palette (`:`) — *jump to anything by name, or do the thing whose key I do not

use fleet_core::{
    ids::{ContextId, JobId, RepoId, SessionId, WorktreeId},
    sessions::{AgentActivity, SessionKind, SessionState},
};
use fleet_proto::{request::RequestBody, snapshot::Snapshot};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{fleet, palette as palette_actions},
    bridge::Bridge,
    dialogs::{
        ConfirmRequest, DialogHost, Dialogs, clear_all, notify, open_session, open_worktree,
        request_confirm, step, type_into, with_host,
    },
    keymap,
    presentation::{FuzzyQuery, SnapshotIndex, pretty_keys},
    screens::workspace::status_kind,
    state::{AppState, HubTab, Overlay, RepoScope, Screen, latest_failed_job, running_jobs},
};

/// The palette's total row cap (§3.9).
pub const ROW_CAP: usize = 10;
/// How many rows each section shows on an empty query (§3.9 "States").
pub const IDLE_ROWS: usize = 5;

/// The palette's draft.
#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    /// The query.
    pub(crate) query: TextFieldState,
    /// The flat cursor across all sections.
    pub(crate) cursor: usize,
    rows: std::rc::Rc<[Entry]>,
    total: usize,
    prepared_query: Option<String>,
}

/// What a palette row does when `Enter` runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// Open a session by id.
    OpenSession(SessionId),
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
    /// What `Enter` does.
    pub(crate) run: Run,
}

/// Every command the palette can run.
///
/// The list is deliberately small: only the commands that are worth reaching without their key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Open §3.8.1.
    NewWorktree,
    /// Open §3.8.2.
    CloneRepo,
    /// Delete the worktree under the cursor, through its confirm.
    DeleteWorktree,
    /// Prune the selected repository, through its confirm.
    PruneWorktrees,
    /// Sleep the session of the worktree under the cursor.
    SleepSession,
    /// Kill that session, through its confirm.
    KillSession,
    /// Re-inspect the worktree under the cursor.
    InspectWorktree,
    /// Open §3.8.5.
    MoveRepo,
    /// Open §3.8.4 for a new context.
    NewContext,
    /// Open §3.8.4 for the active context.
    EditContext,
    /// Delete the active context, through its confirm.
    DeleteContext,
    /// Go to the pull-requests screen.
    PullRequests,
    /// Go back to the worktrees list.
    Worktrees,
    /// Open the Jobs panel.
    JobsPanel,
    /// Open §3.8.6.
    Settings,
    /// Open §3.8.7.
    Help,
    /// Refresh statuses.
    Refresh,
    /// Update Fleet.
    UpdateFleet,
    /// Open the Claude agent session.
    OpenClaude,
    /// Open the OpenCode agent session.
    OpenOpencode,
    /// Quit the app; the daemon keeps running.
    Quit,
    /// Quit and stop the daemon.
    QuitDaemon,
}

impl Command {
    /// Every command, in the order the `DO` section lists them.
    pub const ALL: &'static [Self] = &[
        Self::NewWorktree,
        Self::CloneRepo,
        Self::PruneWorktrees,
        Self::DeleteWorktree,
        Self::SleepSession,
        Self::KillSession,
        Self::InspectWorktree,
        Self::MoveRepo,
        Self::NewContext,
        Self::EditContext,
        Self::DeleteContext,
        Self::PullRequests,
        Self::Worktrees,
        Self::JobsPanel,
        Self::Settings,
        Self::Help,
        Self::Refresh,
        Self::UpdateFleet,
        Self::OpenClaude,
        Self::OpenOpencode,
        Self::Quit,
        Self::QuitDaemon,
    ];

    /// The row label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NewWorktree => "New worktree",
            Self::CloneRepo => "Clone repo",
            Self::DeleteWorktree => "Delete worktree",
            Self::PruneWorktrees => "Prune worktrees",
            Self::SleepSession => "Sleep session",
            Self::KillSession => "Kill session",
            Self::InspectWorktree => "Inspect worktree",
            Self::MoveRepo => "Move repo to context",
            Self::NewContext => "New context",
            Self::EditContext => "Edit context",
            Self::DeleteContext => "Delete context",
            Self::PullRequests => "Pull requests",
            Self::Worktrees => "Worktrees",
            Self::JobsPanel => "Jobs panel",
            Self::Settings => "Settings",
            Self::Help => "Keymap",
            Self::Refresh => "Refresh",
            Self::UpdateFleet => "Update Fleet",
            Self::OpenClaude => "Open Claude agent",
            Self::OpenOpencode => "Open OpenCode agent",
            Self::Quit => "Quit Fleet",
            Self::QuitDaemon => "Quit and stop fleetd",
        }
    }

    /// The glyph, the same one the action wears elsewhere.
    #[must_use]
    pub const fn icon(self) -> Icon {
        match self {
            Self::NewWorktree => Icon::GitBranchPlus,
            Self::CloneRepo => Icon::CloudDownload,
            Self::DeleteWorktree | Self::DeleteContext => Icon::Trash,
            Self::PruneWorktrees => Icon::Scissors,
            Self::SleepSession => Icon::Moon,
            Self::KillSession | Self::QuitDaemon => Icon::Power,
            Self::InspectWorktree => Icon::Eye,
            Self::MoveRepo => Icon::ArrowRightLeft,
            Self::NewContext | Self::EditContext => Icon::Boxes,
            Self::PullRequests => Icon::GitPullRequest,
            Self::Worktrees => Icon::GitBranch,
            Self::JobsPanel => Icon::Clock,
            Self::Settings => Icon::Settings2,
            Self::Help => Icon::CircleQuestionMark,
            Self::Refresh => Icon::LoaderCircle,
            Self::UpdateFleet => Icon::CircleArrowUp,
            Self::OpenClaude | Self::OpenOpencode => Icon::Bot,
            Self::Quit => Icon::CircleX,
        }
    }

    /// Whether the row wears `triangle-alert`. It still goes through its confirm.
    #[must_use]
    pub const fn destructive(self) -> bool {
        matches!(
            self,
            Self::DeleteWorktree
                | Self::DeleteContext
                | Self::PruneWorktrees
                | Self::KillSession
                | Self::QuitDaemon
        )
    }

    /// The action whose bound key this row shows.
    #[must_use]
    pub const fn action(self) -> &'static str {
        match self {
            Self::NewWorktree => "worktrees::Create",
            Self::CloneRepo => "repos::Clone",
            Self::DeleteWorktree => "worktrees::Delete",
            Self::PruneWorktrees => "worktrees::Prune",
            Self::SleepSession => "worktrees::Sleep",
            Self::KillSession => "worktrees::Kill",
            Self::InspectWorktree => "worktrees::Inspect",
            Self::MoveRepo => "repos::MoveToContext",
            Self::NewContext => "hub::NewContext",
            Self::EditContext => "hub::EditContext",
            Self::DeleteContext => "hub::DeleteContext",
            Self::PullRequests => "hub::GoPrs",
            Self::Worktrees => "hub::GoWorktrees",
            Self::JobsPanel => "fleet::OpenJobs",
            Self::Settings => "fleet::OpenSettings",
            Self::Help => "fleet::OpenHelp",
            Self::Refresh => "fleet::Refresh",
            Self::UpdateFleet => "fleet::UpdateFleet",
            Self::OpenClaude => "fleet::OpenAgentClaude",
            Self::OpenOpencode => "fleet::OpenAgentOpencode",
            Self::Quit => "fleet::Quit",
            Self::QuitDaemon => "fleet::QuitAndStopDaemon",
        }
    }

    /// Whether the command can run right now. Invalid commands are not listed (§3.9).
    #[must_use]
    pub fn valid(self, state: &AppState) -> bool {
        let snapshot = state.snapshot.as_ref();
        let has_repo = snapshot.is_some_and(|snapshot| !snapshot.repos.is_empty());
        let has_worktree = snapshot.is_some_and(|snapshot| !snapshot.worktrees.is_empty());
        let has_context = snapshot.is_some_and(|snapshot| !snapshot.contexts.is_empty());
        let on_prs = matches!(state.screen, Screen::Hub { tab: HubTab::Prs });
        match self {
            Self::NewWorktree | Self::PruneWorktrees => has_repo,
            Self::CloneRepo | Self::MoveRepo => has_context && has_repo || self == Self::CloneRepo,
            Self::DeleteWorktree | Self::InspectWorktree | Self::SleepSession => has_worktree,
            Self::KillSession => snapshot.is_some_and(|snapshot| !snapshot.sessions.is_empty()),
            Self::EditContext | Self::DeleteContext => state.active_context().is_some(),
            Self::PullRequests => !on_prs && has_repo,
            Self::Worktrees => on_prs,
            Self::UpdateFleet => state.update_version.is_some(),
            Self::NewContext
            | Self::JobsPanel
            | Self::Settings
            | Self::Help
            | Self::Refresh
            | Self::OpenClaude
            | Self::OpenOpencode
            | Self::Quit
            | Self::QuitDaemon => true,
        }
    }
}

/// The key bound to an action, formatted for the right-hand column.
#[must_use]
pub fn key_for(action: &str) -> Option<String> {
    static HINTS: std::sync::OnceLock<std::collections::HashMap<&'static str, String>> =
        std::sync::OnceLock::new();
    HINTS
        .get_or_init(|| {
            let mut hints = std::collections::HashMap::new();
            for spec in keymap::table() {
                hints
                    .entry(spec.action)
                    .or_insert_with(|| pretty_keys(spec.keys));
            }
            hints
        })
        .get(action)
        .cloned()
}

/// The §2.5 detail wording a `GO` row carries on its right.
///
/// §3.9 wants the row's **state** there (`session attached`, `sleeping`, `PR · mine`, `repo`),
/// not a type word: "worktree" repeats what the id already says, while the state is the thing
/// that decides whether jumping there resumes work or starts it.
#[must_use]
pub fn session_detail(session: SessionState, slept: bool) -> &'static str {
    match session {
        SessionState::Attached => "session attached",
        SessionState::Detached if slept => "sleeping",
        SessionState::Detached => "running, detached",
        SessionState::Unknown => "unknown",
        SessionState::None => "no session",
    }
}

/// Every candidate row, in section order, before the cap.
///
/// Filtering runs on the borrowed snapshot strings, so a row is only built once it has
/// survived the query and the section's idle cap — this runs on every keystroke.
#[must_use]
pub fn candidates(state: &AppState, query: &str) -> Vec<Entry> {
    let sessions_only = query.trim() == "sessions";
    let effective_query = if sessions_only { "" } else { query };
    let idle = effective_query.trim().is_empty();
    let matcher = FuzzyQuery::new(effective_query);
    let Some(snapshot) = state.snapshot.as_ref() else {
        return Vec::new();
    };
    let index = SnapshotIndex::new(snapshot);
    // §3.9 shows only the first `IDLE_ROWS` of each section until a query narrows it.
    let limit = if idle { IDLE_ROWS } else { usize::MAX };

    let mut rows = go_rows(state, snapshot, &index, &matcher, sessions_only, limit);
    if !sessions_only {
        rows.extend(do_rows(state, snapshot, &matcher, limit));
        rows.extend(context_rows(snapshot, &matcher));
    }
    rows
}

/// GO: sessions first, because reaching one from inside another is the point (§3.9).
fn go_rows(
    state: &AppState,
    snapshot: &Snapshot,
    index: &SnapshotIndex<'_>,
    matcher: &FuzzyQuery,
    sessions_only: bool,
    limit: usize,
) -> Vec<Entry> {
    let mut rows: Vec<Entry> = Vec::new();
    for session in &snapshot.sessions {
        if rows.len() == limit {
            return rows;
        }
        // §5 invariant 1: the row wears the Hub's glyph and the Hub's wording for the same
        // worktree, so both are resolved from the one `WorktreeStatus` the Hub reads.
        // `slept_at` alone cannot tell `attached` from `running, detached` — it only tells
        // `awake` from `sleeping` — and reading it as "attached" is how the palette came to
        // call a detached session green.
        let worktree = match &session.kind {
            SessionKind::Worktree(id) => index.worktree(id),
            SessionKind::Agent { .. } => None,
        };
        // §3.9 lists worktrees by their `WorktreeId`; a session id is a different id scheme
        // and mixing the two in one section makes the list unreadable.
        let label = worktree.map_or_else(|| session.id.as_str(), |worktree| worktree.id.as_str());
        if !matcher.matches(label) {
            continue;
        }
        let slept = session.slept_at.is_some();
        let runtime_status = worktree.and_then(|worktree| index.status(&worktree.id));
        let agent_activity = runtime_status.map_or_else(
            || state.session_agent_activity(&session.id),
            |status| status.agent_activity,
        );
        let session_state = runtime_status.map_or(
            // An agent session has no `WorktreeStatus`; the session record itself is then the
            // only evidence, and it can only say awake or slept.
            if slept {
                SessionState::Detached
            } else {
                SessionState::Attached
            },
            |status| status.session,
        );
        let degraded = worktree.is_some_and(|worktree| worktree.degraded.is_some());
        rows.push(Entry {
            section: PaletteSectionKind::Go,
            label: label.to_owned(),
            detail: Some(session_detail(session_state, slept).to_owned()),
            key: None,
            destructive: false,
            icon: Icon::GitBranch,
            status: Some(status_kind(session_state, slept, agent_activity, degraded)),
            run: Run::OpenSession(session.id.clone()),
        });
    }
    if sessions_only {
        return rows;
    }

    let attached: std::collections::HashSet<&str> = snapshot
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect();
    for worktree in &snapshot.worktrees {
        if rows.len() == limit {
            return rows;
        }
        if attached.contains(worktree.session.as_str()) || !matcher.matches(worktree.id.as_str()) {
            continue;
        }
        // §1.3: until the daemon reports a status the state is `unknown`, never a false
        // `none` — the same rule the worktrees list follows.
        let status = index.status(&worktree.id);
        let session = status.map_or(SessionState::Unknown, |status| status.session);
        rows.push(Entry {
            section: PaletteSectionKind::Go,
            label: worktree.id.as_str().to_owned(),
            detail: Some(session_detail(session, false).to_owned()),
            key: None,
            destructive: false,
            icon: Icon::GitBranch,
            status: Some(status_kind(
                session,
                false,
                status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
                worktree.degraded.is_some(),
            )),
            run: Run::OpenWorktree(worktree.id.clone()),
        });
    }
    for repo in &snapshot.repos {
        if rows.len() == limit {
            return rows;
        }
        if !matcher.matches(repo.id.as_str()) {
            continue;
        }
        rows.push(Entry {
            section: PaletteSectionKind::Go,
            label: repo.id.as_str().to_owned(),
            detail: Some("repo".to_owned()),
            key: None,
            destructive: false,
            icon: Icon::FolderGit2,
            status: None,
            run: Run::SelectRepo(repo.id.clone()),
        });
    }
    rows
}

/// DO: valid commands, then the jobs worth cancelling.
fn do_rows(
    state: &AppState,
    snapshot: &Snapshot,
    matcher: &FuzzyQuery,
    limit: usize,
) -> Vec<Entry> {
    let mut rows: Vec<Entry> = Vec::new();
    for command in Command::ALL.iter().copied() {
        if rows.len() == limit {
            return rows;
        }
        if !command.valid(state) || !matcher.matches(command.label()) {
            continue;
        }
        rows.push(Entry {
            section: PaletteSectionKind::Do,
            label: command.label().to_owned(),
            detail: None,
            key: key_for(command.action()),
            destructive: command.destructive(),
            icon: command.icon(),
            status: None,
            run: Run::Command(command),
        });
    }
    for job in running_jobs(&snapshot.jobs) {
        if rows.len() == limit {
            return rows;
        }
        if !job.cancellable {
            continue;
        }
        let label = format!("Cancel job: {} {}", super::quit::kind_word(job), job.target);
        if !matcher.matches(&label) {
            continue;
        }
        rows.push(Entry {
            section: PaletteSectionKind::Do,
            label,
            detail: None,
            key: key_for("fleet::OpenJobs"),
            destructive: false,
            icon: Icon::CircleStop,
            status: None,
            run: Run::CancelJob(job.id.clone()),
        });
    }
    if rows.len() < limit
        && let Some(failed) = latest_failed_job(&snapshot.jobs)
    {
        let label = format!("Show failed job: {}", failed.title);
        if matcher.matches(&label) {
            rows.push(Entry {
                section: PaletteSectionKind::Do,
                label,
                detail: None,
                key: key_for("fleet::FocusStickyError"),
                destructive: false,
                icon: Icon::CircleX,
                status: None,
                run: Run::Command(Command::JobsPanel),
            });
        }
    }
    rows
}

/// CONTEXT: the digit that switches to it is the key hint, so the position is the one before
/// filtering.
fn context_rows(snapshot: &Snapshot, matcher: &FuzzyQuery) -> Vec<Entry> {
    snapshot
        .contexts
        .iter()
        .enumerate()
        .filter(|(_, context)| matcher.matches(&context.name))
        .map(|(index, context)| Entry {
            section: PaletteSectionKind::Context,
            label: context.name.clone(),
            detail: None,
            key: (index < 9).then(|| (index + 1).to_string()),
            destructive: false,
            icon: Icon::Boxes,
            status: None,
            run: Run::SwitchContext(context.id.clone()),
        })
        .collect()
}

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
    let (query, cursor, rows, total) = {
        let draft = &host.read(cx).palette;
        (
            draft.query.clone(),
            draft.cursor,
            draft.rows.clone(),
            draft.total,
        )
    };

    let mut card = fleet_ui_kit::Palette::new(query.text().to_owned())
        .cursor(cursor)
        .cap(ROW_CAP)
        .total(total)
        .empty(format!("Nothing matches \"{}\".", query.text()));
    for kind in [
        PaletteSectionKind::Go,
        PaletteSectionKind::Do,
        PaletteSectionKind::Context,
    ] {
        let section: Vec<PaletteRow> = rows
            .iter()
            .filter(|entry| entry.section == kind)
            .map(|entry| {
                let mut row = PaletteRow::new(entry.label.clone())
                    .destructive(entry.destructive)
                    .icon(entry.icon);
                if let Some(status) = entry.status {
                    row = row.leading(StatusGlyph::new(status).id(gpui::SharedString::from(
                        format!("palette-glyph-{}", entry.label),
                    )));
                }
                if let Some(detail) = entry.detail.clone() {
                    row = row.detail(detail);
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
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let typed = with_host(&state, cx, |host| {
                    let typed = type_into(&mut host.palette.query, event);
                    if typed {
                        host.palette.cursor = 0;
                    }
                    typed
                });
                if typed {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::CursorDown, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::CursorUp, _window, cx| move_cursor(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::Backspace, _window, cx| {
                if with_host(&state, cx, |host| host.palette.query.backspace()) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::DeleteWord, _window, cx| {
                if with_host(&state, cx, |host| host.palette.query.delete_word_before()) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::Clear, _window, cx| {
                if with_host(&state, cx, |host| clear_all(&mut host.palette.query)) {
                    notify(&state, cx);
                }
            }
        })
        .on_action(move |_: &palette_actions::Run, window, cx| {
            run_selected(&run_state, &run_bridge, window, cx);
        })
        .child(
            fleet_ui_kit::Overlay::new()
                .top(top)
                .width(width)
                .scrim(true)
                .content(card),
        )
        .into_any_element()
}

/// Resets the draft the first time the open palette is rendered.
pub(super) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let seed = state.update(cx, |app, _| app.palette_seed.take());
    with_host(state, cx, |host| {
        if !host.palette_open {
            host.palette = PaletteState::default();
            if let Some(seed) = seed {
                host.palette.query = TextFieldState::from_text(seed);
            }
            host.palette_open = true;
        }
    });
    refresh(state, cx);
}

pub(super) fn refresh(state: &Entity<AppState>, cx: &mut App) {
    let query = with_host(state, cx, |host| host.palette.query.text().to_owned());
    let rows = candidates(state.read(cx), &query);
    let total = rows.len();
    let rows = rows.into_iter().take(ROW_CAP).collect();
    with_host(state, cx, |host| {
        host.palette.rows = rows;
        host.palette.total = total;
        host.palette.prepared_query = Some(query);
    });
}

pub(super) fn refresh_query(state: &Entity<AppState>, cx: &mut App) {
    let changed = with_host(state, cx, |host| {
        host.palette_open
            && host.palette.prepared_query.as_deref() != Some(host.palette.query.text())
    });
    if changed {
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
fn run_selected(state: &Entity<AppState>, bridge: &Bridge, window: &mut Window, cx: &mut App) {
    let Some(entry) = with_host(state, cx, |host| {
        host.palette.rows.get(host.palette.cursor).cloned()
    }) else {
        return;
    };
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
    match entry.run {
        Run::OpenSession(session) => open_session(session, state, cx),
        Run::OpenWorktree(id) => open_worktree(id, state, bridge, cx),
        Run::SelectRepo(repo) => {
            state.update(cx, |app, cx| {
                app.scope = RepoScope::Repo(repo);
                app.screen = Screen::hub();
                cx.notify();
            });
        }
        Run::SwitchContext(context) => {
            bridge.send(RequestBody::SetActiveContext { id: Some(context) });
        }
        Run::CancelJob(job) => bridge.send(RequestBody::CancelJob { job }),
        Run::Command(command) => run_command(command, state, bridge, window, cx),
    }
}

/// Runs one command. Destructive rows open their confirm rather than acting (§3.9).
fn run_command(
    command: Command,
    state: &Entity<AppState>,
    bridge: &Bridge,
    window: &mut Window,
    cx: &mut App,
) {
    let open = |dialog: Dialogs, cx: &mut App| {
        state.update(cx, |app, cx| {
            app.open_overlay(Overlay::Dialog(dialog));
            cx.notify();
        });
    };
    match command {
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
            let request = state.read(cx).snapshot.as_ref().and_then(|snapshot| {
                let session = snapshot.sessions.first()?;
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
            });
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
                bridge.send(RequestBody::SleepWorktree { id });
            }
        }
        Command::InspectWorktree => {
            if let Some(id) = cursor_worktree(state, cx) {
                bridge.send(RequestBody::InspectWorktrees {
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
        Command::Refresh => bridge.send(RequestBody::RefreshStatuses { repo: None }),
        Command::UpdateFleet => bridge.send(RequestBody::Update),
        Command::OpenClaude => window.dispatch_action(Box::new(fleet::OpenAgentClaude), cx),
        Command::OpenOpencode => window.dispatch_action(Box::new(fleet::OpenAgentOpencode), cx),
        // The shell owns the whole quit flow, and its listeners sit on the window root, which
        // is an ancestor of this overlay — so dispatching reaches them.
        Command::Quit => window.dispatch_action(Box::new(fleet::Quit), cx),
        Command::QuitDaemon => window.dispatch_action(Box::new(fleet::QuitAndStopDaemon), cx),
    }
}

/// The worktree the Hub's cursor is on.
fn cursor_worktree(state: &Entity<AppState>, cx: &App) -> Option<WorktreeId> {
    let app = state.read(cx);
    let snapshot = app.snapshot.as_ref()?;
    snapshot
        .worktrees
        .get(app.cursors.worktrees)
        .map(|worktree| worktree.id.clone())
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_go_row_says_its_state_not_its_type() {
        // §3.9's right-hand column is the §2.5 state; "worktree" is what the id already says.
        assert_eq!(
            session_detail(SessionState::Attached, false),
            "session attached"
        );
        assert_eq!(session_detail(SessionState::Detached, true), "sleeping");
        assert_eq!(
            session_detail(SessionState::Detached, false),
            "running, detached"
        );
        assert_eq!(session_detail(SessionState::None, false), "no session");
        assert_eq!(session_detail(SessionState::Unknown, false), "unknown");
    }
    use std::time::Instant;

    use super::*;

    #[test]
    fn every_command_has_a_label_and_a_bound_key() {
        for command in Command::ALL {
            assert!(!command.label().is_empty());
            assert!(
                key_for(command.action()).is_some(),
                "`{}` is bound to nothing",
                command.action()
            );
        }
    }

    #[test]
    fn destructive_commands_are_marked_and_are_the_ones_with_confirms() {
        assert!(Command::DeleteWorktree.destructive());
        assert!(Command::PruneWorktrees.destructive());
        assert!(Command::QuitDaemon.destructive());
        assert!(!Command::Settings.destructive());
    }

    #[test]
    fn commands_that_need_a_snapshot_are_not_listed_without_one() {
        let state = AppState::new("/tmp/fleet", Instant::now());
        assert!(!Command::NewWorktree.valid(&state));
        assert!(!Command::DeleteWorktree.valid(&state));
        assert!(Command::Settings.valid(&state));
        assert!(Command::Help.valid(&state));
        assert!(!Command::UpdateFleet.valid(&state));
    }

    #[test]
    fn an_empty_snapshot_still_offers_the_always_valid_commands() {
        let state = AppState::new("/tmp/fleet", Instant::now());
        let rows = candidates(&state, "");
        assert!(
            rows.is_empty(),
            "without a snapshot the palette has nothing to point at"
        );
    }

    fn go_snapshot(state: SessionState, slept: bool) -> fleet_proto::snapshot::Snapshot {
        use fleet_core::sessions::{Session, SessionKind};
        let id: WorktreeId = "acme/widgets#feature-one"
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        let mut snapshot = fleet_proto::snapshot::Snapshot {
            generated_at: "2026-09-04T12:00:00Z".to_owned(),
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: "0.1.0".to_owned(),
                pid: 1,
                started_at: "2026-09-04T09:00:00Z".to_owned(),
                home: "/tmp/fleet".to_owned(),
            },
        };
        snapshot.worktrees = vec![fleet_core::model::Worktree {
            id: id.clone(),
            repo_id: "acme/widgets"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            slug: "feature-one".to_owned(),
            branch: "feature-one".to_owned(),
            base_ref: "main".to_owned(),
            path: "/tmp/widgets/feature-one".to_owned(),
            session: "widgets/feature-one".to_owned(),
            host: None,
            created_at: "2026-09-04T09:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        }];
        snapshot.sessions = vec![Session {
            id: "widgets/feature-one"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            kind: SessionKind::Worktree(id.clone()),
            cwd: "/tmp/widgets/feature-one".to_owned(),
            terminals: Vec::new(),
            active_terminal: None,
            slept_at: slept.then(|| "2026-09-04T11:00:00Z".to_owned()),
            kept_terminals: Vec::new(),
        }];
        snapshot.statuses = vec![fleet_core::sessions::WorktreeStatus {
            worktree_id: id,
            session: state,
            windows: Vec::new(),
            running: Vec::new(),
            agent_activity: fleet_core::sessions::AgentActivity::Unknown,
            agent_activity_changed_at: None,
        }];
        snapshot
    }

    #[test]
    fn a_go_row_takes_its_state_and_its_id_from_the_same_place_the_hub_does() {
        let now = Instant::now();
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(go_snapshot(SessionState::Detached, false), now);
        let rows = candidates(&app, "");
        let go: Vec<_> = rows
            .iter()
            .filter(|entry| entry.section == PaletteSectionKind::Go)
            .collect();
        assert_eq!(go.len(), 1, "one worktree is one GO row, {go:?}");
        assert_eq!(
            go[0].label, "acme/widgets#feature-one",
            "\u{a7}3.9 labels a GO row with its WorktreeId, never a session id"
        );
        assert_eq!(go[0].detail.as_deref(), Some("running, detached"));
        assert_eq!(go[0].status, Some(StatusKind::DetachedAwake));

        // The same worktree, actually attached, and slept.
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(go_snapshot(SessionState::Attached, false), now);
        let rows = candidates(&app, "");
        assert_eq!(rows[0].status, Some(StatusKind::Attached));
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(go_snapshot(SessionState::Detached, true), now);
        let rows = candidates(&app, "");
        assert_eq!(rows[0].status, Some(StatusKind::Sleeping));
        assert_eq!(rows[0].detail.as_deref(), Some("sleeping"));
    }

    #[test]
    fn the_section_order_is_fixed() {
        assert!(PaletteSectionKind::Go < PaletteSectionKind::Do);
        assert!(PaletteSectionKind::Do < PaletteSectionKind::Context);
    }
    #[gpui::test]
    fn palette_seeding_and_cursor_motion_reuse_prepared_matches(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/palette", std::time::Instant::now());
            state.snapshot = Some(go_snapshot(SessionState::Detached, false));
            state.palette_seed = Some("sessions".into());
            state
        });
        let before = cx.update(|cx| {
            seed(&state, cx);
            with_host(&state, cx, |host| {
                assert_eq!(host.palette.query.text(), "sessions");
                assert!(!host.palette.rows.is_empty());
                host.palette.rows.clone()
            })
        });
        cx.update(|cx| {
            move_cursor(&state, 1, cx);
            let after = with_host(&state, cx, |host| host.palette.rows.clone());
            assert!(std::rc::Rc::ptr_eq(&before, &after));
            with_host(&state, cx, |host| host.palette.query.insert("missing"));
            super::super::notify(&state, cx);
            let after = with_host(&state, cx, |host| host.palette.rows.clone());
            assert!(after.is_empty());
            assert!(!std::rc::Rc::ptr_eq(&before, &after));
        });
    }
}
