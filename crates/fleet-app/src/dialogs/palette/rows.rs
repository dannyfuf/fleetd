//! The candidate rows: every object and command the palette can list, resolved against the
//! snapshot, and the ranking that turns a query into one ordered list of sections.

use super::*;

/// The keystrokes bound to an action, for a row's key chips.
///
/// Read from the key table itself, so a chip is never typed by hand. The first row that binds
/// the action wins; a row bound in a one-shot prefix context (`Workspace > Prefix`) is spelled
/// with the `ctrl-s` that enters it, because that is what a person types.
#[must_use]
pub fn keys_for(action: &str) -> Option<Vec<Keystroke>> {
    static KEYS: std::sync::OnceLock<std::collections::HashMap<&'static str, Vec<Keystroke>>> =
        std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let mut keys = std::collections::HashMap::new();
        for spec in keymap::table() {
            if keys.contains_key(spec.action) {
                continue;
            }
            let prefix = spec
                .context
                .ends_with("Prefix")
                .then_some("ctrl-s")
                .into_iter();
            let strokes = prefix
                .chain(spec.keys.split_whitespace())
                .map(Keystroke::parse)
                .collect::<Result<Vec<_>, _>>();
            match strokes {
                Ok(strokes) => {
                    keys.insert(spec.action, strokes);
                }
                Err(error) => {
                    tracing::error!(keys = spec.keys, %error, "unparsable key table row");
                }
            }
        }
        keys
    })
    .get(action)
    .cloned()
}

/// The §2.5 detail wording a `Go to` row carries on its right.
///
/// §3.9 wants the row's **state** there (`session attached`, `sleeping`, `repo`), not a type
/// word: "worktree" repeats what the id already says, while the state is the thing that decides
/// whether jumping there resumes work or starts it.
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

/// The session switcher's rows (`^s W`), most recently used first. The title bar's worktree
/// switcher lists the same rows, so the two can never disagree about order or state.
#[must_use]
pub(crate) fn session_rows(state: &AppState) -> Vec<Entry> {
    candidates(state, "sessions", None, None, &[])
}

/// Every row the palette lists for `query`, already ranked, sectioned and capped.
///
/// Runs in the update path (`refresh`), never in `render`, and only when an input of
/// [`PreparedKey`] moved.
#[must_use]
pub fn candidates(
    state: &AppState,
    query: &str,
    behind: Option<Dialogs>,
    detail_card: Option<&CardId>,
    prs: &[PalettePr],
) -> Vec<Entry> {
    let parsed = parse_query(query);
    let needle = FuzzyQuery::new(parsed.needle);
    let card = card_context(state, behind, detail_card);
    let snapshot = state.snapshot.as_ref();

    if parsed.scope == Scope::All && needle.is_empty() {
        return idle_rows(state, card);
    }

    let mut pool: Vec<Entry> = Vec::new();
    let wants = |scope: Scope| parsed.scope == Scope::All || parsed.scope == scope;
    if wants(Scope::GoTo)
        && let Some(snapshot) = snapshot
    {
        let index = SnapshotIndex::new(snapshot);
        pool.extend(go_rows(state, snapshot, &index, parsed.sessions_only));
        if !parsed.sessions_only {
            pool.extend(context_rows(snapshot));
        }
    }
    if wants(Scope::Commands)
        && let Some(snapshot) = snapshot
    {
        pool.extend(do_rows(state, snapshot, card));
    }
    if wants(Scope::Cards) {
        pool.extend(card_rows(state));
    }
    if parsed.scope == Scope::All {
        pool.extend(pr_rows(prs));
    }
    if wants(Scope::Agents) {
        pool.extend(agents_rows(state));
    }

    let mut rows = if needle.is_empty() {
        pool
    } else {
        rank(pool, &needle, here(state, card))
    };
    rows.truncate(RESULT_CAP);
    rows
}

/// The empty query: where you were lately, then what is worth doing here.
fn idle_rows(state: &AppState, card: CardContext) -> Vec<Entry> {
    let mut rows: Vec<Entry> = Vec::new();
    if let Some(snapshot) = state.snapshot.as_ref() {
        let index = SnapshotIndex::new(snapshot);
        rows.extend(
            go_rows(state, snapshot, &index, false)
                .into_iter()
                .take(RECENT_ROWS)
                .map(|entry| Entry {
                    section: Section::Recent,
                    ..entry
                }),
        );
    }
    if state.snapshot.is_some() {
        let mut suggested: Vec<(u8, usize, Command)> = Command::ALL
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(order, command)| {
                let rank = command.info().rank?;
                command
                    .valid_with(state, card)
                    .then_some((rank, order, command))
            })
            .collect();
        suggested.sort_unstable_by_key(|(rank, order, _)| (*rank, *order));
        rows.extend(
            suggested
                .into_iter()
                .take(SUGGESTED_ROWS)
                .map(|(_, _, command)| Entry {
                    section: Section::Suggested,
                    ..command_entry(command)
                }),
        );
    }
    rows
}

/// Scores every row against the query, drops the misses, and orders what is left.
///
/// Rows keep their section; inside it they sort by score, and the sections sort by their best
/// row, so the top of the list is always the single best match and `Enter` runs it. A command
/// bound to a place in `here` — one that works on what the screen is showing — earns a bonus
/// that settles a near tie between matches of the same kind but never lifts a weaker kind of
/// match over a stronger one. Ties keep the order the rows were built in, which is the order
/// each section means something in (MRU sessions first, catalogue order for commands).
fn rank(pool: Vec<Entry>, needle: &FuzzyQuery, here: &[Place]) -> Vec<Entry> {
    /// A hit on the searchable extras only (a provider name, a repo) ranks under any label hit.
    const EXTRA_PENALTY: i32 = 500;
    /// Well under the 1000 between a prefix, a word-boundary and a mid-word hit, and well over
    /// the length and offset terms that separate two hits of the same kind.
    const HERE_BONUS: i32 = 200;
    /// A section, its best score, and its scored rows.
    type Group = (Section, i32, Vec<(i32, Entry)>);
    let mut groups: Vec<Group> = Vec::new();
    for mut entry in pool {
        let hit = match needle.score(&entry.label) {
            Some(hit) => Some((hit.score, hit.indices)),
            None => entry
                .search
                .as_deref()
                .and_then(|extra| needle.score(extra))
                .map(|hit| (hit.score - EXTRA_PENALTY, Vec::new())),
        };
        let Some((score, indices)) = hit else {
            continue;
        };
        let score = match entry.run {
            Run::Command(command) if here.contains(&command.info().place) => score + HERE_BONUS,
            _ => score,
        };
        entry.matches = indices;
        match groups
            .iter_mut()
            .find(|(section, _, _)| *section == entry.section)
        {
            Some((_, best, rows)) => {
                *best = (*best).max(score);
                rows.push((score, entry));
            }
            None => groups.push((entry.section, score, vec![(score, entry)])),
        }
    }
    for (_, _, rows) in &mut groups {
        rows.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    }
    groups.sort_by_key(|(_, best, _)| std::cmp::Reverse(*best));
    groups
        .into_iter()
        .flat_map(|(_, _, rows)| rows.into_iter().map(|(_, entry)| entry))
        .collect()
}

/// Native threads reachable from the selected worktree, in caller/child order.
pub(super) fn agents_rows(state: &AppState) -> Vec<Entry> {
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
        .map(|summary| agent_entry(state, summary, current.as_ref()))
        .collect()
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
        label,
        search: Some(format!(
            "{} {}",
            summary.provider.executable(),
            summary.provider.display_name()
        )),
        secondary: Some(agent_detail(summary, attention)),
        trailing: Some(if attached || !child { "go" } else { "attach" }.to_owned()),
        // The strip index is a real key: `^s <n>` selects that tab.
        key: agent_strip_index(state, summary)
            .and_then(|index| keys_for(&format!("native_agent::SelectTab{index}"))),
        icon: Icon::Bot,
        attention: Some(attention),
        ..Entry::new(
            Section::Agents,
            String::new(),
            Run::OpenAgentThread(summary.thread),
        )
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

/// Go to: sessions first, most recently used first, because reaching one from inside another
/// is the point (§3.9); then worktrees without a session, then repos.
pub(super) fn go_rows(
    state: &AppState,
    snapshot: &Snapshot,
    index: &SnapshotIndex<'_>,
    sessions_only: bool,
) -> Vec<Entry> {
    let mut rows: Vec<Entry> = Vec::new();
    // The rank is read once per session, so it is resolved through a map built once rather
    // than by rescanning the MRU inside the sort key.
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
        // §5 invariant 1: the row wears the Hub's glyph and the Hub's wording for the same
        // worktree, so both are resolved from the one `WorktreeStatus` the Hub reads.
        // `slept_at` alone cannot tell `attached` from `running, detached` — it only tells
        // `awake` from `sleeping`.
        let worktree = match &session.kind {
            SessionKind::Worktree(id) => index.worktree(id),
            SessionKind::Agent { .. } => None,
        };
        // §3.9 lists worktrees by their `WorktreeId`; a session id is a different id scheme
        // and mixing the two in one section makes the list unreadable.
        let label = worktree.map_or_else(|| session.id.as_str(), |worktree| worktree.id.as_str());
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
        let run = match (&session.kind, worktree) {
            (SessionKind::Agent(agent), _) => Run::OpenSession {
                session: session.id.clone(),
                agent: *agent,
            },
            (SessionKind::Worktree(id), _) => Run::OpenWorktree(id.clone()),
        };
        rows.push(Entry {
            detail: Some(session_detail(session_state, slept).to_owned()),
            icon: Icon::GitBranch,
            status: Some(status_kind(session_state, slept, agent_activity, degraded)),
            ..Entry::new(Section::GoTo, label.to_owned(), run)
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
        if attached.contains(worktree.session.as_str()) {
            continue;
        }
        // §1.3: until the daemon reports a status the state is `unknown`, never a false
        // `none` — the same rule the worktrees list follows.
        let status = index.status(&worktree.id);
        let session = status.map_or(SessionState::Unknown, |status| status.session);
        rows.push(Entry {
            detail: Some(session_detail(session, false).to_owned()),
            icon: Icon::GitBranch,
            status: Some(status_kind(
                session,
                false,
                status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
                worktree.degraded.is_some(),
            )),
            ..Entry::new(
                Section::GoTo,
                worktree.id.as_str().to_owned(),
                Run::OpenWorktree(worktree.id.clone()),
            )
        });
    }
    for repo in &snapshot.repos {
        rows.push(Entry {
            detail: Some("repo".to_owned()),
            icon: Icon::FolderGit2,
            ..Entry::new(
                Section::GoTo,
                repo.id.as_str().to_owned(),
                Run::SelectRepo(repo.id.clone()),
            )
        });
    }
    rows
}

/// One command's row: its catalogue label, where it acts (or that it asks first), its key.
pub(super) fn command_entry(command: Command) -> Entry {
    let info = command.info();
    let destructive = command.destructive();
    Entry {
        // The place tells apart the rows a board and an open card both offer ("Open the
        // card's issue in the browser"), which carry the same label on purpose. A destructive
        // command says instead that it will ask first: that is what a person needs to know
        // before pressing `Enter` on it.
        detail: if destructive {
            Some(ASKS_FIRST.to_owned())
        } else {
            (info.place != Place::Everywhere).then(|| info.place.title().to_owned())
        },
        key: keys_for(command.action()),
        destructive,
        icon: command.icon(),
        ..Entry::new(
            Section::Commands,
            info.label.to_owned(),
            Run::Command(command),
        )
    }
}

/// Commands: the valid ones, then the jobs worth cancelling.
pub(super) fn do_rows(state: &AppState, snapshot: &Snapshot, card: CardContext) -> Vec<Entry> {
    let mut rows: Vec<Entry> = Command::ALL
        .iter()
        .copied()
        .filter(|command| command.valid_with(state, card))
        .map(command_entry)
        .collect();
    for job in running_jobs(&snapshot.jobs) {
        if !job.cancellable {
            continue;
        }
        rows.push(Entry {
            key: keys_for("fleet::OpenJobs"),
            icon: Icon::CircleStop,
            ..Entry::new(
                Section::Commands,
                format!(
                    "Cancel job: {} {}",
                    super::super::quit::kind_word(job),
                    job.target
                ),
                Run::CancelJob(job.id.clone()),
            )
        });
    }
    if let Some(failed) = latest_failed_job(&snapshot.jobs) {
        rows.push(Entry {
            key: keys_for("fleet::FocusStickyError"),
            icon: Icon::CircleX,
            ..Entry::new(
                Section::Commands,
                format!("Show failed job: {}", failed.title),
                Run::Command(Command::JobsPanel),
            )
        });
    }
    rows
}

/// Contexts, under Go to: the digit that switches to one is its key, so the position is the
/// one before filtering.
pub(super) fn context_rows(snapshot: &Snapshot) -> Vec<Entry> {
    snapshot
        .contexts
        .iter()
        .enumerate()
        .map(|(index, context)| Entry {
            detail: Some("context".to_owned()),
            key: (index < 9)
                .then(|| keys_for(&format!("hub::SelectContext{}", index + 1)))
                .flatten(),
            icon: Icon::Boxes,
            ..Entry::new(
                Section::GoTo,
                context.name.clone(),
                Run::SwitchContext(context.id.clone()),
            )
        })
        .collect()
}

/// Cards of the board the mirror holds, keyed the way the board shows them (`FLT-7`), with
/// their column as a status word.
pub(super) fn card_rows(state: &AppState) -> Vec<Entry> {
    let Some(view) = state.board() else {
        return Vec::new();
    };
    view.cards
        .iter()
        .map(|card| {
            let status = view
                .board
                .statuses
                .iter()
                .find(|status| status.id == card.status_id)
                .map(|status| status.name.clone());
            Entry {
                badge: status,
                icon: Icon::SquareCheck,
                ..Entry::new(
                    Section::Cards,
                    format!("{} {}", card.display_key(&view.board), card.title),
                    Run::OpenCard(card.id.clone()),
                )
            }
        })
        .collect()
}

/// Pull requests the PR screen has already loaded, both tabs.
pub(super) fn pr_rows(prs: &[PalettePr]) -> Vec<Entry> {
    prs.iter()
        .map(|pr| {
            let tab = match pr.tab {
                fleet_core::github::PrTab::Mine => "mine",
                fleet_core::github::PrTab::Review => "review",
            };
            Entry {
                detail: Some(format!("{} \u{b7} {tab}", pr.repo.as_str())),
                search: Some(pr.repo.as_str().to_owned()),
                icon: Icon::GitPullRequest,
                ..Entry::new(
                    Section::PullRequests,
                    format!("#{} {}", pr.number, pr.title),
                    Run::GoToPr {
                        tab: pr.tab,
                        repo: pr.repo.clone(),
                        number: pr.number,
                        local: pr.local.clone(),
                    },
                )
            }
        })
        .collect()
}
