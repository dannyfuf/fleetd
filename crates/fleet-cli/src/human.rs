//! Human-readable command output formatting.

use fleet_core::{
    inspection::WorktreeInspection,
    sessions::{SessionState, WorktreeStatus},
};
use fleet_proto::response::{
    DoctorCheck, DoctorStatus, PruneResult, SleepResult, WorktreeDeleteResult,
};

/// Formats the compact list summary.
#[must_use]
pub fn list(repo_count: usize, worktree_count: usize) -> String {
    format!("{repo_count} repos, {worktree_count} worktrees")
}

/// Formats one row per watch: ID, source, label, status, start time, and terminal ID.
#[must_use]
pub fn watches(watches: &[fleet_core::watches::Watch]) -> String {
    use fleet_core::watches::WatchStatus;
    watches
        .iter()
        .map(|watch| {
            let status = match watch.status {
                WatchStatus::Running => "running".to_owned(),
                WatchStatus::Exited {
                    code: Some(code), ..
                } => format!("exited {code}"),
                WatchStatus::Exited { .. } => "interrupted".to_owned(),
            };
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                watch.id,
                watch.source,
                crate::envelope::single_line(&watch.label),
                status,
                watch.started_at,
                watch.terminal
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats independent deletion results, one worktree per line.
#[must_use]
pub fn delete(results: &[WorktreeDeleteResult]) -> String {
    results
        .iter()
        .map(|result| {
            if result.ok {
                format!("Deleted {}", result.worktree_id)
            } else {
                format!(
                    "Failed {}: {}",
                    result.worktree_id,
                    result.reason.as_deref().unwrap_or("unknown error")
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats inspection facts as a readable per-worktree report.
#[must_use]
pub fn inspect(inspections: &[WorktreeInspection]) -> String {
    inspections
        .iter()
        .map(|inspection| {
            let state = if inspection.error.is_some() {
                "error"
            } else if inspection.dirty {
                "dirty"
            } else if inspection.merged {
                "merged"
            } else {
                "active"
            };
            let mut line = format!("{} {} {}", inspection.worktree_id, inspection.branch, state);
            if let Some(error) = &inspection.error {
                line.push_str(": ");
                line.push_str(error);
            }
            if !inspection.warnings.is_empty() {
                line.push_str(" [");
                line.push_str(&inspection.warnings.join(", "));
                line.push(']');
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats safe-prune results.
#[must_use]
pub fn prune(result: &PruneResult) -> String {
    let deleted = result.deleted.iter().map(|id| {
        if result.dry_run {
            format!("Would delete {id}")
        } else {
            format!("Deleted {id}")
        }
    });
    let skipped = result
        .skipped
        .iter()
        .map(|entry| format!("Skipped {}: {}", entry.worktree_id, entry.reason));
    deleted.chain(skipped).collect::<Vec<_>>().join("\n")
}

/// Formats worktree status in swarm's `<id> <session>` line form.
#[must_use]
pub fn status(statuses: &[WorktreeStatus]) -> String {
    statuses
        .iter()
        .map(|status| {
            let session = match status.session {
                SessionState::None => "none",
                SessionState::Detached => "detached",
                SessionState::Attached => "attached",
                SessionState::Unknown => "unknown",
            };
            format!("{} {session}", status.worktree_id)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats a sleep-policy report.
#[must_use]
pub fn sleep(result: &SleepResult) -> String {
    let kept = result
        .kept
        .iter()
        .map(|entry| format!("Kept {}: {}", entry.window, entry.reason));
    let closed = result
        .closed
        .iter()
        .map(|window| format!("Closed {window}"));
    let mut lines = kept.chain(closed).collect::<Vec<_>>();
    if result.session_killed {
        lines.push("Session killed".to_owned());
    }
    if lines.is_empty() {
        lines.push("Nothing to sleep".to_owned());
    }
    lines.join("\n")
}

/// Formats diagnostics as a plain fixed-column report.
#[must_use]
pub fn doctor(checks: &[DoctorCheck]) -> String {
    let mut lines = vec!["CHECK STATUS DETAIL".to_owned()];
    lines.extend(checks.iter().map(|check| {
        format!(
            "{} {} {}",
            check.check,
            match check.status {
                DoctorStatus::Ok => "ok",
                DoctorStatus::Warn => "warn",
                DoctorStatus::Fail => "fail",
            },
            check.detail
        )
    }));
    lines.join("\n")
}
