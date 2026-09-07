//! §3.8.8 Quit (`ctrl-q`) and §3.8.9 Quit and stop the daemon (`ctrl-shift-q`).

use fleet_core::sessions::Session;
use fleet_proto::job::{JobRecord, JobStatus};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    dialogs::root,
    state::{AppState, running_jobs},
};

/// One line of the "keeps running" or "cancelled now" list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JobLine {
    /// `clone`, `hooks`, … — the job kind as the ticker words it.
    kind: String,
    /// What it is working on.
    target: String,
    /// The most recent progress line, when there is one.
    progress: Option<String>,
    /// Whether an explicit cancel can still be requested.
    can_cancel: bool,
    /// Whether the underlying detached process outlives fleetd.
    survives_shutdown: bool,
    /// Whether the operation can be started again after it stops.
    restartable: bool,
}

/// The job kind, in the short word the ticker and both quit dialogs use.
#[must_use]
pub(super) fn kind_word(job: &JobRecord) -> String {
    use fleet_proto::job::JobKind;
    match &job.kind {
        JobKind::PoolRefresh => "pool",
        JobKind::RepoDiscovery => "discovery",
        kind => crate::presentation::job_kind_label(kind),
    }
    .to_owned()
}

/// Every running job, as the lines both dialogs draw.
#[must_use]
fn job_lines(jobs: &[JobRecord]) -> Vec<JobLine> {
    running_jobs(jobs)
        .into_iter()
        .map(|job| {
            let can_cancel = job.cancellable && !matches!(job.status, JobStatus::Cancelling);
            let survives_shutdown = matches!(job.kind, fleet_proto::job::JobKind::PostCreateHooks)
                && !job.cancellable
                && matches!(job.status, JobStatus::Running);
            JobLine {
                kind: kind_word(job),
                target: if job.target.is_empty() {
                    job.title.clone()
                } else {
                    job.target.clone()
                },
                progress: job.progress.clone(),
                can_cancel,
                survives_shutdown,
                restartable: job.retryable,
            }
        })
        .collect()
}

/// How many terminals the given sessions own.
#[must_use]
fn terminal_count(sessions: &[Session]) -> usize {
    sessions.iter().map(|session| session.terminals.len()).sum()
}

fn restart_word(line: &JobLine) -> &'static str {
    if line.restartable {
        "restartable"
    } else {
        "not restartable"
    }
}

fn detached_jobs_fact(lines: &[&JobLine]) -> String {
    format!(
        "{} job(s) keep running: {}",
        lines.len(),
        lines
            .iter()
            .map(|line| format!("{} ({})", line.kind, restart_word(line)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Renders the `ctrl-q` confirm: everything listed **survives** in fleetd.
pub(crate) fn render_quit(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.xs;
    let app = state.read(cx);
    let (lines, sessions, terminals) = app.snapshot.as_ref().map_or_else(
        || (Vec::new(), 0, 0),
        |snapshot| {
            (
                job_lines(&snapshot.jobs),
                snapshot.sessions.len(),
                terminal_count(&snapshot.sessions),
            )
        },
    );

    let mut list = FactList::new();
    for line in &lines {
        list = list.fact(Fact::safe(match &line.progress {
            Some(progress) => format!("{}  {}  {progress}", line.kind, line.target),
            None => format!("{}  {}", line.kind, line.target),
        }));
    }
    if sessions > 0 {
        list = list.fact(Fact::safe(format!(
            "{sessions} sessions \u{00b7} {terminals} terminals"
        )));
    }

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(Text::ui("These keep running in fleetd:").muted())
        .child(list)
        .child(Text::ui("They will be here when you come back.").muted());

    root(focus)
        .child(
            Dialog::new("Quit Fleet?")
                .icon(Icon::CircleQuestionMark)
                .width(super::Dialogs::Quit.width(cx))
                .body(body)
                .hint_row(
                    KeyHintRow::new()
                        .key("J", "jobs")
                        .key("W", "never warn again")
                        .key("n", "cancel"),
                )
                .primary("y  Quit"),
        )
        .into_any_element()
}

/// Renders the `ctrl-shift-q` confirm: everything listed **dies**, and `ctrl-q` is the way out.
pub(crate) fn render_quit_daemon(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.sm;
    let app = state.read(cx);
    let (lines, sessions) = app.snapshot.as_ref().map_or_else(
        || (Vec::new(), &[][..]),
        |snapshot| (job_lines(&snapshot.jobs), snapshot.sessions.as_slice()),
    );
    let (detached, stopping): (Vec<&JobLine>, Vec<&JobLine>) =
        lines.iter().partition(|line| line.survives_shutdown);

    let mut killed = FactList::new();
    for session in sessions {
        let running: Vec<&str> = session
            .terminals
            .iter()
            .filter(|terminal| !terminal.keep_alive.is_empty())
            .map(|terminal| terminal.name.as_str())
            .collect();
        let detail = if running.is_empty() {
            format!("{} terminals", session.terminals.len())
        } else {
            running.join(", ")
        };
        killed = killed.fact(Fact::risk(format!("{}   {detail}", session.id.as_str())));
    }

    let mut cancelled = FactList::new();
    for line in &stopping {
        let restart = restart_word(line);
        let action = if line.can_cancel {
            "cancelled"
        } else {
            "stops with fleetd"
        };
        cancelled = cancelled.fact(Fact::risk(match &line.progress {
            Some(progress) => format!(
                "{} {}   {progress}   ({action}; {restart})",
                line.kind, line.target
            ),
            None => format!("{} {}   ({action}; {restart})", line.kind, line.target),
        }));
    }

    let mut body = div().flex().flex_col().gap(gap);
    if !sessions.is_empty() {
        body = body.child(SectionHeader::new("KILLED NOW")).child(killed);
    }
    if !stopping.is_empty() {
        body = body
            .child(SectionHeader::new("CANCELLED NOW"))
            .child(cancelled);
    }
    if !detached.is_empty() {
        // §6: a detached post-create runner has no cancel token; the dialog says so rather
        // than pretending it can stop it.
        body = body.child(Text::ui(detached_jobs_fact(&detached)));
    }
    let body = body
        .child(Text::ui("Worktrees, repos and state on disk are untouched.").muted())
        .child(Text::ui("ctrl-q quits Fleet and leaves all of this running.").muted());

    root(focus)
        .child(
            Dialog::new("Stop fleetd and quit?")
                .icon(Icon::Power)
                .width(super::Dialogs::QuitDaemon.width(cx))
                .tone(Tone::Warning)
                .body(body)
                .hint_row(KeyHintRow::new().key("n", "cancel"))
                .primary("Y  Stop and quit"),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use fleet_core::ids::JobId;
    use fleet_proto::job::JobKind;

    use super::*;

    fn job(kind: JobKind, status: JobStatus, cancellable: bool) -> JobRecord {
        JobRecord {
            id: JobId::try_from("job-1").unwrap_or_else(|error| panic!("{error}")),
            kind,
            target: "buk/payroll".to_owned(),
            title: "Clone".to_owned(),
            status,
            progress: Some("40%".to_owned()),
            log_path: "/tmp/job.log".to_owned(),
            started_at: "2026-09-04T12:00:00Z".to_owned(),
            finished_at: None,
            cancellable,
            retryable: false,
        }
    }

    #[test]
    fn only_running_jobs_are_enumerated() {
        let jobs = vec![
            job(JobKind::Clone, JobStatus::Running, true),
            job(JobKind::Prune, JobStatus::Succeeded, false),
        ];
        let lines = job_lines(&jobs);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, "clone");
        assert!(lines[0].can_cancel);
    }

    #[test]
    fn a_job_already_cancelling_is_not_offered_as_cancellable() {
        let jobs = vec![job(JobKind::PostCreateHooks, JobStatus::Cancelling, true)];
        let lines = job_lines(&jobs);
        assert_eq!(lines[0].kind, "hooks");
        assert!(!lines[0].can_cancel);
    }

    #[test]
    fn shutdown_job_facts_use_independent_capabilities() {
        let mut cancelling = job(JobKind::Clone, JobStatus::Cancelling, true);
        cancelling.retryable = false;
        let mut detached = job(JobKind::PostCreateHooks, JobStatus::Running, false);
        detached.retryable = true;
        let lines = job_lines(&[cancelling, detached]);

        assert!(!lines[0].can_cancel);
        assert!(!lines[0].survives_shutdown);
        assert!(!lines[0].restartable);
        assert!(!lines[1].can_cancel);
        assert!(lines[1].survives_shutdown);
        assert!(lines[1].restartable);
        assert_eq!(
            detached_jobs_fact(&[&lines[1]]),
            "1 job(s) keep running: hooks (restartable)"
        );
    }

    #[test]
    fn every_job_kind_has_a_short_word() {
        for kind in [
            JobKind::Clone,
            JobKind::PoolBuild,
            JobKind::CreateWorktree,
            JobKind::Import,
            JobKind::Custom("doctor".to_owned()),
        ] {
            let word = kind_word(&job(kind, JobStatus::Running, true));
            assert!(!word.is_empty());
            assert!(!word.contains(' '));
        }
    }
}
