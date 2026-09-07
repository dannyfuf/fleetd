use super::*;

/// The decisive facts of §3.8.3, already ordered and classified.
#[derive(Debug, Default)]
pub struct Facts {
    /// The kit list, which owns the ordering and the escalation key.
    pub(crate) list: FactList,
    /// Whether any risk fact is true.
    pub(crate) risky: bool,
    /// The age of the facts in seconds, for the mandatory freshness stamp.
    pub(crate) age_secs: Option<i64>,
}

/// The ref the daemon actually compared against.
///
/// `WorktreeInspection::target_branch` is the bare branch name; the daemon merges against
/// `origin/<branch>` (`inspect.rs`). §3.8.3 quotes the full ref — `✓ merged into origin/main` —
/// because "merged into main" is ambiguous when a local `main` has drifted from the remote.
#[must_use]
pub fn target_ref(target_branch: &str) -> String {
    let branch = target_branch.trim();
    if branch.is_empty() {
        return "the base ref".to_owned();
    }
    if branch.starts_with("origin/") {
        return branch.to_owned();
    }
    format!("origin/{branch}")
}

/// Turns one inspection into the fact list §3.8.3 draws.
#[must_use]
pub fn worktree_facts(inspection: Option<&WorktreeInspection>, loading: bool, now: i64) -> Facts {
    let Some(inspection) = inspection else {
        return Facts {
            list: FactList::new().loading(true),
            risky: false,
            age_secs: None,
        };
    };
    let mut list = FactList::new().loading(loading);
    let mut risky = false;

    if inspection.dirty {
        risky = true;
        list = list.fact(Fact::risk(match inspection.dirty_files {
            Some(count) => format!("{count} uncommitted files"),
            None => "uncommitted changes".to_owned(),
        }));
    } else {
        list = list.fact(Fact::safe("clean"));
    }

    match inspection.unique_commits {
        Some(0) => list = list.fact(Fact::safe("no commits of its own")),
        Some(count) => {
            risky = true;
            list = list.fact(Fact::risk(format!(
                "{count} commits not on {}",
                target_ref(&inspection.target_branch)
            )));
        }
        None => {
            list = list.fact(Fact::unknown("unique commit count unavailable"));
        }
    }

    if inspection.merged || inspection.merged_into_target {
        list = list.fact(Fact::safe(format!(
            "merged into {}",
            target_ref(&inspection.target_branch)
        )));
    } else if let Some(pull_request) = inspection.pr.as_ref() {
        let label = match pull_request.state {
            InspectionPrState::Open => {
                format!("PR #{} open (not merged)", pull_request.number)
            }
            InspectionPrState::Merged => format!("PR #{} merged", pull_request.number),
            InspectionPrState::Closed => {
                risky = true;
                format!("PR #{} closed without merging", pull_request.number)
            }
        };
        list = list.fact(if matches!(pull_request.state, InspectionPrState::Closed) {
            Fact::risk(label)
        } else {
            Fact::safe(label)
        });
    } else if !inspection.published {
        risky = true;
        list = list.fact(Fact::risk("never pushed"));
    }

    match inspection.session {
        SessionState::None => list = list.fact(Fact::safe("no session")),
        // §2.5 gives one wording per state and §5 invariant 1 makes it the *same* wording on
        // every screen: a detached session must not be called attached here, because this is
        // the one screen where a wrong session fact changes a destructive decision.
        SessionState::Detached | SessionState::Attached => {
            risky = true;
            let running = if inspection.running.is_empty() {
                String::new()
            } else {
                format!(" \u{00b7} {} running", inspection.running.join(", "))
            };
            let state = match inspection.session {
                SessionState::Attached => "attached",
                _ => "running, detached",
            };
            list = list.fact(Fact::risk(format!("session {state}{running}")));
        }
        SessionState::Unknown => list = list.fact(Fact::unknown("session state unknown")),
    }

    for warning in &inspection.warnings {
        list = list.fact(Fact::unknown(warning.clone()));
    }
    if let Some(error) = inspection.error.as_ref() {
        list = list.fact(Fact::unknown(error.clone()));
    }

    Facts {
        list,
        risky,
        age_secs: age_secs(&inspection.inspected_at, now),
    }
}

/// `K` on a worktree: the facts are what the session is running (§3.8.3).
pub(super) fn kill_facts(terminals: usize, running: &[String], unsaved: bool) -> Facts {
    let mut list = FactList::new().fact(Fact::risk(format!("{terminals} terminals")));
    if unsaved {
        list = list.fact(Fact::risk("an editor has unsaved changes"));
    }
    for label in running {
        list = list.fact(Fact::risk(format!("{label} running")));
    }
    Facts {
        list,
        risky: true,
        age_secs: None,
    }
}

/// `ctrl-s x`: one fact, the command that dies with the terminal.
pub(super) fn close_terminal_facts(running: Option<&str>) -> Facts {
    match running {
        Some(command) => Facts {
            list: FactList::new().fact(Fact::risk(format!("{command} is running in it"))),
            risky: true,
            age_secs: None,
        },
        None => Facts {
            list: FactList::new().fact(Fact::safe("nothing is running in it")),
            risky: false,
            age_secs: None,
        },
    }
}
