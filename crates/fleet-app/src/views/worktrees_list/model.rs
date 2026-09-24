//! The worktrees list's view model: one [`WorktreeRow`] per worktree, every word the list and
//! the detail panel draw already built.
//!
//! Built in the Hub's projection (an update path, memoised per revision), never in `render`:
//! the renderer only lays out what is here. Ages are kept as seconds and aged by the caller's
//! offset, so a clock tick re-labels them without rebuilding the model.

use std::{collections::HashMap, path::Path};

use fleet_core::{
    ids::{JobId, RepoId, WorktreeId},
    inspection::WorktreeInspection,
    model::Worktree,
    sessions::{AgentActivity, SessionState, WorktreeStatus},
};
use fleet_proto::job::{JobKind, JobRecord, JobStatus};
use fleet_proto::snapshot::LinkState;
use fleet_ui_kit::{Freshness, Icon, PrBadgeState, StatusKind, Tone};
use gpui::SharedString;

use crate::{
    presentation::{age_secs, contains_folded, inspection_badge, session_glyph, tilde},
    views::detail::{Inspected, resolved_worktree_status},
};

/// A pull request the Hub's PR cache knows more about than the inspection does: its title and
/// its full review/CI state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPr {
    /// The state the chip shows.
    pub state: PrBadgeState,
    /// The PR's title, for the detail panel's link.
    pub title: SharedString,
}

/// The session in words, for the list's session column and the detail panel's card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWords {
    /// `claude working · 2 tabs`, `1 terminal`, `Sleeping`, `No session`.
    pub text: SharedString,
    /// The dot's tone. `None` draws no dot: nothing is running.
    pub dot: Option<Tone>,
    /// Whether the words themselves are quiet (`No session`) rather than a fact about a
    /// running thing.
    pub quiet: bool,
}

/// The icon that leads a row's name: health first, like the glyph it replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameIcon {
    /// The glyph.
    pub icon: Icon,
    /// Its tone.
    pub tone: Tone,
    /// Whether it spins (a job owns the row).
    pub spins: bool,
}

/// What the detail panel's *Git* section says about the worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitFacts {
    /// No inspection yet: `not checked`, with the Inspect button.
    NotChecked,
    /// The first inspection is in flight.
    Checking,
    /// The inspection failed; the daemon's words, verbatim.
    Failed(SharedString),
    /// The inspected facts.
    Known(Box<KnownGit>),
}

/// The inspected facts, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownGit {
    /// `Clean` or `12 files`.
    pub changes: SharedString,
    /// Whether the worktree has uncommitted changes.
    pub dirty: bool,
    /// `vs origin/main`.
    pub versus: SharedString,
    /// `2 ahead · 0 behind`, absent when git could not say.
    pub ahead_behind: Option<SharedString>,
    /// `Yes, origin/spike` or `No`.
    pub published: SharedString,
    /// The PR, when one matches the branch.
    pub pr: Option<DetailPr>,
    /// The daemon's warnings, verbatim.
    pub warnings: Vec<SharedString>,
    /// A refresh is in flight: the values dim, they never blank.
    pub refreshing: bool,
    /// Seconds since the inspection ran, when its stamp parses.
    pub checked_age: Option<i64>,
}

/// The pull request line of the detail panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailPr {
    /// `#4 Ship the settings dialog`, or `#4` when the title is not known.
    pub link: SharedString,
    /// The chip.
    pub state: Option<PrBadgeState>,
}

/// Everything the worktree detail panel draws, built with the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDetail {
    /// `acme/web · from origin/main · on this Mac`.
    pub subtitle: SharedString,
    /// The session card's state line: `claude is working`, `Attached`, `No session`.
    pub session_state: SharedString,
    /// The glyph beside it.
    pub session_kind: StatusKind,
    /// Seconds since the state last changed, when the daemon says.
    pub session_age: Option<i64>,
    /// `2 tabs: zsh, claude — kept by fleetd`, absent with no terminals.
    pub session_tabs: Option<SharedString>,
    /// The *Git* section.
    pub git: GitFacts,
    /// The `$HOME`-collapsed path.
    pub path: SharedString,
    /// Seconds since the worktree was created.
    pub created_age: Option<i64>,
}

/// One row of the worktrees list, fully resolved from the snapshot and the inspection cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    /// The worktree this row stands for.
    pub id: WorktreeId,
    /// Its repository, for the `owner/name` column and for `x` prune.
    pub repo: RepoId,
    /// The §2.5 health-first status, which the detail panel and the rail agree on.
    pub glyph: StatusKind,
    /// The icon that leads the name.
    pub name_icon: NameIcon,
    /// `Worktree.branch` — the only string the user thinks in.
    pub branch: SharedString,
    /// `↑2`, when the branch is ahead of its base.
    pub ahead: Option<SharedString>,
    /// Whether the inspection says the worktree has uncommitted changes.
    pub dirty: bool,
    /// The remote host, absent for the 95 % local case.
    pub host: Option<SharedString>,
    /// Whether that host's last probe failed.
    pub host_unreachable: bool,
    /// The machine provider backing that host (`tailscale`, `command`, `legacy`).
    pub host_provider: Option<SharedString>,
    /// The daemon-link state of that host, absent while no status has arrived.
    pub host_link: Option<LinkState>,
    /// `owner/name`, shown in `All` scope or in a wide pane.
    pub repo_label: SharedString,
    /// Keep-alive labels of the running terminals (§4 sleep policy); the filter matches them.
    pub keep_alive: Vec<SharedString>,
    /// The session column, in words.
    pub session: SessionWords,
    /// Post-create hooks failed: the `Setup hook failed` chip and its `View log` button.
    pub degraded: bool,
    /// The failed hooks job, which `View log` opens in Jobs.
    pub hook_job: Option<JobId>,
    /// A job phase, which replaces the session words *and* the age column.
    pub phase: Option<SharedString>,
    /// The row is being deleted: dimmed to 40 %, non-selectable.
    pub deleting: bool,
    /// The inspection errored; the age column gains an amber `triangle-alert`.
    pub inspect_error: bool,
    /// `#n` plus the badge state, when a PR matches the branch.
    pub pr: Option<(u64, PrBadgeState)>,
    /// Whether the derived marks are older than 10 minutes (§2.6).
    pub stale_marks: bool,
    inspected_age: Option<i64>,
    /// Age of `lastOpenedAt ?? createdAt`, in seconds.
    pub age: Option<i64>,
    /// Whether the row needs the user: hooks failed, host offline or an inspection error.
    pub needs_attention: bool,
    /// The detail panel's words.
    pub detail: WorktreeDetail,
}

impl WorktreeRow {
    /// Whether the derived marks are stale once the model has aged by `age_offset` seconds.
    #[must_use]
    pub fn marks_stale(&self, age_offset: i64) -> bool {
        self.inspected_age
            .map(|age| Freshness::from_secs(age.saturating_add(age_offset).max(0)))
            .is_some_and(|freshness| freshness == Freshness::Stale)
    }
}

/// The phase word a running job on this row shows instead of percentages (§3.3).
#[must_use]
fn job_phase(job: &JobRecord) -> SharedString {
    if let Some(progress) = job.progress.as_deref().filter(|line| !line.is_empty()) {
        return SharedString::from(progress.to_owned());
    }
    SharedString::new_static(match job.kind {
        JobKind::CreateWorktree => "copying files\u{2026}",
        JobKind::DeleteWorktree => "deleting",
        JobKind::PostCreateHooks => "running hooks\u{2026}",
        JobKind::Prune => "pruning\u{2026}",
        JobKind::Inspect => "checking\u{2026}",
        _ => "working\u{2026}",
    })
}

pub(crate) fn owns_row(job: &JobRecord) -> bool {
    matches!(
        job.status,
        JobStatus::Running | JobStatus::Queued | JobStatus::Cancelling
    ) && matches!(
        job.kind,
        JobKind::CreateWorktree
            | JobKind::DeleteWorktree
            | JobKind::PostCreateHooks
            | JobKind::Prune
    )
}

pub(crate) fn job_targets_worktree(job: &JobRecord, worktree: &WorktreeId) -> bool {
    job.target.parse::<WorktreeId>().ok().as_ref() == Some(worktree)
        || job
            .target
            .rsplit_once(':')
            .filter(|(_, attempt)| !attempt.is_empty())
            .and_then(|(target, _)| target.parse::<WorktreeId>().ok())
            .as_ref()
            == Some(worktree)
}

/// Everything the model needs from the snapshot to build the rows.
pub struct RowInputs<'a> {
    /// The worktrees of the current scope, in the order the rows will appear.
    pub worktrees: Vec<&'a Worktree>,
    /// The client's inspection cache.
    pub inspections: &'a HashMap<WorktreeId, Inspected>,
    /// Pull requests the PR cache knows, keyed by repository and number.
    pub known_prs: &'a HashMap<(RepoId, u64), KnownPr>,
    /// `$HOME`, for tilde collapsing.
    pub home: Option<&'a Path>,
    /// The current epoch second, so the model stays pure.
    pub now: i64,
}

/// Builds one row per worktree, in the order the caller supplied them.
#[must_use]
pub fn build_rows(
    inputs: &RowInputs<'_>,
    index: &crate::presentation::SnapshotIndex<'_>,
) -> Vec<WorktreeRow> {
    inputs
        .worktrees
        .iter()
        .map(|worktree| build_row(worktree, inputs, index))
        .collect()
}

fn build_row(
    worktree: &Worktree,
    inputs: &RowInputs<'_>,
    index: &crate::presentation::SnapshotIndex<'_>,
) -> WorktreeRow {
    let status = index.status(&worktree.id);
    let host_status = worktree.host.as_ref().and_then(|host| index.host(host));
    let host_link = host_status.map(|host| host.link);
    // A probe failure and a dropped daemon link are the same thing to a row: the remote's
    // state is not knowable, so the glyph must fall back to `unknown`. A legacy entry has no
    // link by design and is never called offline for it.
    let unreachable =
        host_status.is_some_and(|host| !host.reachable) || host_link == Some(LinkState::Down);
    let jobs = index.jobs_for_target(worktree.id.as_str());
    let job = jobs
        .iter()
        .copied()
        .find(|job| owns_row(job) && job_targets_worktree(job, &worktree.id));
    let sessions = index.sessions_for_worktree(&worktree.id);
    let slept = sessions.iter().any(|session| session.slept_at.is_some());
    let inspected = inputs.inspections.get(&worktree.id);
    let inspection = inspected.and_then(|slot| slot.data.as_ref());
    let inspected_age = inspection.and_then(|data| age_secs(&data.inspected_at, inputs.now));
    let stale_marks = inspected_age
        .map(Freshness::from_secs)
        .is_some_and(|freshness| freshness == Freshness::Stale);
    let errored = inspected.is_some_and(|slot| slot.error.is_some())
        || inspection.is_some_and(|data| data.error.is_some());
    let degraded = worktree.degraded.is_some();
    let glyph = resolved_worktree_status(status, slept, degraded, unreachable, job.is_some());
    let session_kind = session_glyph(
        status.map_or(SessionState::Unknown, |status| status.session),
        slept,
        status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
    );
    // A mark derived from an errored inspection is not drawn at all (§2.6).
    let marks = inspection.filter(|_| !errored);
    let pr = marks.and_then(|data| data.pr.as_ref()).and_then(|pr| {
        let known = inputs.known_prs.get(&(worktree.repo_id.clone(), pr.number));
        known
            .map(|known| known.state)
            .or_else(|| inspection_badge(pr.state))
            .map(|state| (pr.number, state))
    });
    let hook_job = worktree.degraded.as_ref().and_then(|degraded| {
        jobs.iter()
            .rev()
            .find(|job| {
                matches!(job.kind, JobKind::PostCreateHooks)
                    && matches!(job.status, JobStatus::Failed { .. })
            })
            .or_else(|| jobs.iter().find(|job| job.log_path == degraded.log_path))
            .map(|job| job.id.clone())
    });
    let host = worktree
        .host
        .as_ref()
        .map(|host| SharedString::from(host.to_string()));

    WorktreeRow {
        id: worktree.id.clone(),
        repo: worktree.repo_id.clone(),
        glyph,
        name_icon: name_icon(glyph, session_kind),
        branch: SharedString::from(worktree.branch.clone()),
        ahead: marks
            .and_then(|data| data.ahead)
            .filter(|ahead| *ahead > 0)
            .map(|ahead| SharedString::from(format!("\u{2191}{ahead}"))),
        dirty: marks.is_some_and(|data| data.dirty),
        host: host.clone(),
        host_unreachable: unreachable,
        host_provider: host_status
            .map(|host| host.provider.clone())
            .filter(|provider| !provider.is_empty())
            .map(SharedString::from),
        host_link,
        repo_label: SharedString::from(worktree.repo_id.to_string()),
        keep_alive: status
            .map(|status| {
                status
                    .running
                    .iter()
                    .map(|label| SharedString::from(label.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        session: session_words(session_kind, status, unreachable),
        degraded,
        hook_job,
        phase: job.map(job_phase),
        deleting: job.is_some_and(|job| matches!(job.kind, JobKind::DeleteWorktree)),
        inspect_error: errored,
        pr,
        stale_marks,
        inspected_age,
        age: worktree
            .last_opened_at
            .as_deref()
            .or(Some(worktree.created_at.as_str()))
            .and_then(|iso| age_secs(iso, inputs.now)),
        needs_attention: degraded || unreachable || errored,
        detail: WorktreeDetail {
            subtitle: SharedString::from(format!(
                "{} \u{b7} from {} \u{b7} {}",
                worktree.repo_id,
                worktree.base_ref,
                host.as_ref()
                    .map_or_else(|| "on this Mac".to_owned(), |host| format!("@{host}")),
            )),
            session_state: session_state_words(session_kind, status),
            session_kind,
            session_age: status
                .and_then(|status| status.agent_activity_changed_at.as_deref())
                .or_else(|| {
                    sessions
                        .iter()
                        .find_map(|session| session.slept_at.as_deref())
                })
                .and_then(|iso| age_secs(iso, inputs.now)),
            session_tabs: session_tabs(status),
            git: git_facts(inspected, pr, inputs, worktree, inputs.now),
            path: SharedString::from(tilde(&worktree.path, inputs.home).into_owned()),
            created_age: age_secs(&worktree.created_at, inputs.now),
        },
    }
}

/// The name's icon: a failure or a job takes it over, otherwise the branch, blue while a session
/// is alive.
fn name_icon(glyph: StatusKind, session: StatusKind) -> NameIcon {
    match glyph {
        StatusKind::Degraded | StatusKind::HostUnreachable | StatusKind::JobRunning => NameIcon {
            icon: glyph.icon(),
            tone: glyph.tone(),
            spins: glyph.spins(),
        },
        _ => NameIcon {
            icon: Icon::GitBranch,
            tone: if session_is_live(session) {
                Tone::Accent
            } else {
                Tone::Muted
            },
            spins: false,
        },
    }
}

fn session_is_live(kind: StatusKind) -> bool {
    matches!(
        kind,
        StatusKind::Attached
            | StatusKind::DetachedAwake
            | StatusKind::AgentWorking
            | StatusKind::AgentFinished
    )
}

/// `1 tab`, `2 tabs`.
fn tabs(count: usize) -> String {
    if count == 1 {
        "1 tab".to_owned()
    } else {
        format!("{count} tabs")
    }
}

/// The agent a status names: the one working, else the first recognised one.
fn agent_name(status: Option<&WorktreeStatus>, activity: AgentActivity) -> String {
    status
        .and_then(|status| {
            status
                .windows
                .iter()
                .find(|window| window.agent.is_some() && window.agent_activity == activity)
                .or_else(|| status.windows.iter().find(|window| window.agent.is_some()))
                .and_then(|window| window.agent.clone())
        })
        .unwrap_or_else(|| "agent".to_owned())
}

/// The session column: what is running, in words (§3.3).
fn session_words(
    kind: StatusKind,
    status: Option<&WorktreeStatus>,
    unreachable: bool,
) -> SessionWords {
    if unreachable {
        return SessionWords {
            text: SharedString::new_static("Host offline"),
            dot: Some(Tone::Warning),
            quiet: false,
        };
    }
    let windows = status.map_or(0, |status| status.windows.len());
    let (text, dot, quiet) = match kind {
        StatusKind::AgentWorking => (
            format!(
                "{} working \u{b7} {}",
                agent_name(status, AgentActivity::Working),
                tabs(windows)
            ),
            Some(Tone::Success),
            false,
        ),
        StatusKind::AgentFinished => (
            format!(
                "{} waiting \u{b7} {}",
                agent_name(status, AgentActivity::Idle),
                tabs(windows)
            ),
            Some(Tone::Warning),
            false,
        ),
        StatusKind::Attached | StatusKind::DetachedAwake => (
            if windows == 1 {
                "1 terminal".to_owned()
            } else {
                format!("{windows} terminals")
            },
            Some(Tone::Muted),
            false,
        ),
        StatusKind::Sleeping => ("Sleeping".to_owned(), None, true),
        StatusKind::NoSession => ("No session".to_owned(), None, true),
        _ => ("Not known yet".to_owned(), None, true),
    };
    SessionWords {
        text: SharedString::from(text),
        dot,
        quiet,
    }
}

/// The session card's state line.
fn session_state_words(kind: StatusKind, status: Option<&WorktreeStatus>) -> SharedString {
    match kind {
        StatusKind::AgentWorking => SharedString::from(format!(
            "{} is working",
            agent_name(status, AgentActivity::Working)
        )),
        StatusKind::AgentFinished => SharedString::from(format!(
            "{} is waiting for you",
            agent_name(status, AgentActivity::Idle)
        )),
        StatusKind::Attached => SharedString::new_static("Open in a window"),
        StatusKind::DetachedAwake => SharedString::new_static("Running in the background"),
        StatusKind::Sleeping => SharedString::new_static("Sleeping"),
        StatusKind::NoSession => SharedString::new_static("No session"),
        _ => SharedString::new_static("Not known yet"),
    }
}

/// `2 tabs: zsh, claude — kept by fleetd`.
fn session_tabs(status: Option<&WorktreeStatus>) -> Option<SharedString> {
    let status = status.filter(|status| !status.windows.is_empty())?;
    let names = status
        .windows
        .iter()
        .map(|window| window.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(SharedString::from(format!(
        "{}: {names} \u{2014} kept by fleetd",
        tabs(status.windows.len())
    )))
}

fn git_facts(
    inspected: Option<&Inspected>,
    pr: Option<(u64, PrBadgeState)>,
    inputs: &RowInputs<'_>,
    worktree: &Worktree,
    now: i64,
) -> GitFacts {
    let Some(inspected) = inspected else {
        return GitFacts::NotChecked;
    };
    if let Some(error) = inspected.failure() {
        return GitFacts::Failed(SharedString::from(error.to_owned()));
    }
    let Some(data) = inspected.data.as_ref() else {
        return GitFacts::Checking;
    };
    GitFacts::Known(Box::new(known_git(
        data,
        inspected.loading,
        pr,
        inputs,
        worktree,
        now,
    )))
}

fn known_git(
    data: &WorktreeInspection,
    refreshing: bool,
    pr: Option<(u64, PrBadgeState)>,
    inputs: &RowInputs<'_>,
    worktree: &Worktree,
    now: i64,
) -> KnownGit {
    let changes = match (data.dirty, data.dirty_files) {
        (true, Some(1)) => "1 file".to_owned(),
        (true, Some(count)) => format!("{count} files"),
        (true, None) => "Uncommitted changes".to_owned(),
        (false, _) => "Clean".to_owned(),
    };
    let published = match (data.published, data.upstream.as_deref()) {
        (true, Some(upstream)) => format!("Yes, {upstream}"),
        (true, None) => "Yes".to_owned(),
        (false, _) if data.upstream_gone => "No, the remote branch is gone".to_owned(),
        (false, _) => "No".to_owned(),
    };
    let pr = data.pr.as_ref().map(|found| {
        let known = inputs
            .known_prs
            .get(&(worktree.repo_id.clone(), found.number));
        DetailPr {
            link: SharedString::from(match known {
                Some(known) => format!("#{} {}", found.number, known.title),
                None => format!("#{}", found.number),
            }),
            state: pr.map(|(_, state)| state),
        }
    });
    KnownGit {
        changes: SharedString::from(changes),
        dirty: data.dirty,
        versus: SharedString::from(format!("vs {}", data.base_ref)),
        ahead_behind: data.ahead.zip(data.behind).map(|(ahead, behind)| {
            SharedString::from(format!("{ahead} ahead \u{b7} {behind} behind"))
        }),
        published: SharedString::from(published),
        pr,
        warnings: data
            .warnings
            .iter()
            .map(|warning| SharedString::from(warning.clone()))
            .collect(),
        refreshing,
        checked_age: age_secs(&data.inspected_at, now),
    }
}

/// Sorts by `lastOpenedAt` desc, then `createdAt` desc (§3.3), so `Enter` alone is often the
/// whole task.
///
/// ISO-8601 UTC strings order lexicographically, which is why no parsing is needed here.
pub fn sort_rows(worktrees: &mut [&Worktree]) {
    worktrees.sort_by(|left, right| {
        right
            .last_opened_at
            .cmp(&left.last_opened_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.branch.cmp(&right.branch))
    });
}

/// Whether a row survives the filter query (§3.10): branch, repo, host or keep-alive label.
#[must_use]
pub fn matches(row: &WorktreeRow, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let needle = query.to_lowercase();
    contains_folded(&row.branch, &needle)
        || contains_folded(&row.repo_label, &needle)
        || row
            .host
            .as_ref()
            .is_some_and(|host| contains_folded(host, &needle))
        || row
            .keep_alive
            .iter()
            .any(|label| contains_folded(label, &needle))
}

/// The page subtitle: `4 across 2 repositories · 1 needs attention`, or `3 in acme/api` when one
/// repository is selected. `rows` is the scope before the filter.
#[must_use]
pub fn summary(rows: &[WorktreeRow], repo: Option<&RepoId>) -> SharedString {
    if rows.is_empty() {
        return SharedString::new_static("No worktrees yet");
    }
    let count = rows.len();
    let lead = match repo {
        Some(repo) => format!("{count} in {repo}"),
        None => {
            let mut repos: Vec<&RepoId> = rows.iter().map(|row| &row.repo).collect();
            repos.sort();
            repos.dedup();
            match repos.as_slice() {
                [only] => format!("{count} in {only}"),
                many => format!("{count} across {} repositories", many.len()),
            }
        }
    };
    let attention = rows.iter().filter(|row| row.needs_attention).count();
    if attention == 0 {
        SharedString::from(lead)
    } else {
        let verb = if attention == 1 { "needs" } else { "need" };
        SharedString::from(format!("{lead} \u{b7} {attention} {verb} attention"))
    }
}
