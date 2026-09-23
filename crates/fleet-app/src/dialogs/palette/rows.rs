//! The candidate rows: every object and command the palette can list, resolved against the
//! snapshot.

use super::*;

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
pub fn candidates(
    state: &AppState,
    query: &str,
    behind: Option<Dialogs>,
    detail_card: Option<&CardId>,
) -> Vec<Entry> {
    if let Some(filter) = agents_picker_filter(query) {
        return agents_rows(state, &FuzzyQuery::new(filter), usize::MAX);
    }
    let sessions_only = is_session_switcher(query);
    let effective_query = if sessions_only { "" } else { query };
    let idle = effective_query.trim().is_empty();
    let matcher = FuzzyQuery::new(effective_query);
    let Some(snapshot) = state.snapshot.as_ref() else {
        return Vec::new();
    };
    let index = SnapshotIndex::new(snapshot);
    // §3.9 shows only the first `IDLE_ROWS` of each section until a query narrows it.
    let limit = if sessions_only {
        usize::MAX
    } else if idle {
        IDLE_ROWS
    } else {
        usize::MAX
    };

    let mut rows = go_rows(state, snapshot, &index, &matcher, sessions_only, limit);
    if !sessions_only {
        let card = card_context(state, behind, detail_card);
        rows.extend(do_rows(state, snapshot, &matcher, card, limit));
        rows.extend(context_rows(snapshot, &matcher));
    }
    rows
}

/// Native threads reachable from the selected worktree, in caller/child order.
pub(super) fn agents_rows(state: &AppState, matcher: &FuzzyQuery, limit: usize) -> Vec<Entry> {
    let current = selected_worktree_id(state);
    let summaries = state.agents.summaries();
    let callers: Vec<_> = summaries
        .iter()
        .filter(|summary| {
            summary.parent.is_none()
                && current
                    .as_ref()
                    .is_none_or(|worktree| &summary.worktree == worktree)
        })
        .collect();
    let caller_ids: std::collections::HashSet<_> =
        callers.iter().map(|summary| summary.thread).collect();
    let children: Vec<_> = summaries
        .iter()
        .filter(|summary| {
            summary
                .parent
                .is_some_and(|parent| caller_ids.contains(&parent))
        })
        .collect();

    callers
        .into_iter()
        .chain(children.iter().copied().filter(|summary| {
            current
                .as_ref()
                .is_none_or(|worktree| &summary.worktree == worktree)
        }))
        .chain(children.iter().copied().filter(|summary| {
            current
                .as_ref()
                .is_some_and(|worktree| &summary.worktree != worktree)
        }))
        .filter(|summary| agent_matches(summary, matcher))
        .take(limit)
        .map(|summary| agent_entry(state, summary, current.as_ref()))
        .collect()
}

pub(super) fn agent_matches(summary: &AgentThreadSummary, matcher: &FuzzyQuery) -> bool {
    matcher.matches(&summary.title)
        || matcher.matches(summary.provider.executable())
        || matcher.matches(summary.provider.display_name())
}

pub(super) fn agent_entry(
    state: &AppState,
    summary: &AgentThreadSummary,
    current: Option<&WorktreeId>,
) -> Entry {
    let child = summary.parent.is_some();
    let other_worktree = child && current.is_some_and(|worktree| worktree != &summary.worktree);
    let mut label = tab_title(summary);
    if other_worktree {
        label.push_str(" \u{b7} ");
        label.push_str(summary.worktree.as_str());
    }
    let attached = state.agents.is_attached(summary.thread);
    let attention = state.agents.attention(summary.thread);
    Entry {
        section: PaletteSectionKind::Agents,
        label,
        detail: None,
        secondary: Some(agent_detail(summary, attention)),
        trailing: Some(if attached || !child { "go" } else { "attach" }.to_owned()),
        key: Some(
            agent_strip_index(state, summary)
                .map_or_else(|| "\u{b7}".to_owned(), |index| index.to_string()),
        ),
        destructive: false,
        icon: Icon::Bot,
        status: None,
        attention: Some(attention),
        run: Run::OpenAgentThread(summary.thread),
    }
}

pub(super) fn agent_strip_index(state: &AppState, summary: &AgentThreadSummary) -> Option<usize> {
    if !state.agents.is_attached(summary.thread) {
        return None;
    }
    let session = state.snapshot.as_ref()?.sessions.iter().find(|session| {
        matches!(&session.kind, SessionKind::Worktree(worktree) if worktree == &summary.worktree)
    })?;
    let offset = state.agents.strip_offset(summary.thread)?;
    Some(session.terminals.len() + offset + 1)
}

pub(super) fn agent_detail(summary: &AgentThreadSummary, attention: Attention) -> String {
    let word = match attention {
        Attention::NeedsYou(AttentionKind::Permission) => "blocked \u{b7} permission",
        Attention::NeedsYou(AttentionKind::Question) => "blocked \u{b7} question",
        Attention::NeedsYou(AttentionKind::Plan) => "blocked \u{b7} plan",
        Attention::NeedsYou(AttentionKind::Finished) => "done",
        Attention::Working => "working",
        Attention::Waiting => "waiting",
        Attention::Failed => "failed",
        Attention::Unread => "unread",
        Attention::Idle => "idle",
    };
    if matches!(
        attention,
        Attention::NeedsYou(
            AttentionKind::Permission | AttentionKind::Question | AttentionKind::Plan
        )
    ) {
        return word.to_owned();
    }
    let age = summary.last_activity.map_or_else(
        || "\u{2013}".to_owned(),
        |last| {
            let seconds = chrono::Utc::now()
                .signed_duration_since(last)
                .num_seconds()
                .max(0);
            fleet_ui_kit::format_age(seconds)
        },
    );
    format!("{word} \u{b7} {age}")
}

/// GO: sessions first, because reaching one from inside another is the point (§3.9).
pub(super) fn go_rows(
    state: &AppState,
    snapshot: &Snapshot,
    index: &SnapshotIndex<'_>,
    matcher: &FuzzyQuery,
    sessions_only: bool,
    limit: usize,
) -> Vec<Entry> {
    let mut rows: Vec<Entry> = Vec::new();
    // The rank is read once per session, so it is resolved through a map built once rather
    // than by rescanning the MRU inside the sort key — this list is rebuilt on every
    // `AppState` notification while the palette is open.
    let ranks: std::collections::HashMap<&SessionId, usize> = state
        .session_mru
        .entries()
        .iter()
        .enumerate()
        .map(|(rank, id)| (id, rank))
        .collect();
    let mut sessions: Vec<_> = snapshot.sessions.iter().collect();
    sessions.sort_by_key(|session| ranks.get(&session.id).copied().unwrap_or(usize::MAX));
    for session in sessions {
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
            secondary: None,
            trailing: None,
            key: None,
            destructive: false,
            icon: Icon::GitBranch,
            status: Some(status_kind(session_state, slept, agent_activity, degraded)),
            attention: None,
            run: match (&session.kind, worktree) {
                (SessionKind::Agent(agent), _) => Run::OpenSession {
                    session: session.id.clone(),
                    agent: *agent,
                },
                (SessionKind::Worktree(id), _) => Run::OpenWorktree(id.clone()),
            },
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
            secondary: None,
            trailing: None,
            key: None,
            destructive: false,
            icon: Icon::GitBranch,
            status: Some(status_kind(
                session,
                false,
                status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
                worktree.degraded.is_some(),
            )),
            attention: None,
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
            secondary: None,
            trailing: None,
            key: None,
            destructive: false,
            icon: Icon::FolderGit2,
            status: None,
            attention: None,
            run: Run::SelectRepo(repo.id.clone()),
        });
    }
    rows
}

/// DO: valid commands, then the jobs worth cancelling.
pub(super) fn do_rows(
    state: &AppState,
    snapshot: &Snapshot,
    matcher: &FuzzyQuery,
    card: CardContext,
    limit: usize,
) -> Vec<Entry> {
    let mut rows: Vec<Entry> = Vec::new();
    for command in Command::ALL.iter().copied() {
        if rows.len() == limit {
            return rows;
        }
        if !command.valid_with(state, card) || !matcher.matches(command.label()) {
            continue;
        }
        let info = command.info();
        rows.push(Entry {
            section: PaletteSectionKind::Do,
            label: info.label.to_owned(),
            // The place tells apart the rows a board and an open card both offer ("Open the
            // card's issue in the browser"), which carry the same label on purpose.
            detail: (info.place != Place::Everywhere).then(|| info.place.title().to_owned()),
            secondary: None,
            trailing: None,
            key: key_for(command.action()),
            destructive: command.destructive(),
            icon: command.icon(),
            status: None,
            attention: None,
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
        let label = format!(
            "Cancel job: {} {}",
            super::super::quit::kind_word(job),
            job.target
        );
        if !matcher.matches(&label) {
            continue;
        }
        rows.push(Entry {
            section: PaletteSectionKind::Do,
            label,
            detail: None,
            secondary: None,
            trailing: None,
            key: key_for("fleet::OpenJobs"),
            destructive: false,
            icon: Icon::CircleStop,
            status: None,
            attention: None,
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
                secondary: None,
                trailing: None,
                key: key_for("fleet::FocusStickyError"),
                destructive: false,
                icon: Icon::CircleX,
                status: None,
                attention: None,
                run: Run::Command(Command::JobsPanel),
            });
        }
    }
    rows
}

/// CONTEXT: the digit that switches to it is the key hint, so the position is the one before
/// filtering.
pub(super) fn context_rows(snapshot: &Snapshot, matcher: &FuzzyQuery) -> Vec<Entry> {
    snapshot
        .contexts
        .iter()
        .enumerate()
        .filter(|(_, context)| matcher.matches(&context.name))
        .map(|(index, context)| Entry {
            section: PaletteSectionKind::Context,
            label: context.name.clone(),
            detail: None,
            secondary: None,
            trailing: None,
            key: (index < 9).then(|| (index + 1).to_string()),
            destructive: false,
            icon: Icon::Boxes,
            status: None,
            attention: None,
            run: Run::SwitchContext(context.id.clone()),
        })
        .collect()
}
