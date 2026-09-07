use super::Shell;
use gpui::{Context, Task};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Last completed filesystem observation, refreshed only while the first-run card — the sole
/// consumer — is the body. Render reads these values and never probes.
#[derive(Default, PartialEq, Eq)]
pub(super) struct LocalFiles {
    pub(super) fleet_state_exists: bool,
    pub(super) swarm_state_exists: bool,
}

impl Shell {
    pub(super) fn spawn_file_observer(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |shell, cx| {
            let Ok((home, user_home)) = shell.update(cx, |shell, cx| {
                (shell.state.read(cx).home.clone(), shell.user_home.clone())
            }) else {
                return;
            };
            loop {
                let Ok((started_at, first_run)) = shell.update(cx, |shell, cx| {
                    let state = shell.state.read(cx);
                    (
                        state
                            .snapshot
                            .as_ref()
                            .map(|snapshot| snapshot.daemon.started_at.clone()),
                        state.is_first_run(),
                    )
                }) else {
                    return;
                };
                let probe_home = home.clone();
                let probe_user_home = user_home.clone();
                let probe_started_at = started_at.clone();
                let (files, outdated) = cx
                    .background_executor()
                    .spawn(async move {
                        // Only the first-run card reads these two, so nothing else pays for them.
                        let files = first_run.then(|| LocalFiles {
                            fleet_state_exists: probe_home.join("state.json").exists(),
                            swarm_state_exists: crate::views::first_run::has_swarm_state(
                                probe_user_home.as_deref(),
                            ),
                        });
                        let outdated = probe_started_at
                            .as_deref()
                            .is_some_and(daemon_binary_is_newer);
                        (files, outdated)
                    })
                    .await;
                if shell
                    .update(cx, |shell, cx| {
                        if let Some(files) = files
                            && shell.local_files != files
                        {
                            shell.local_files = files;
                            cx.notify();
                        }
                        shell.state.update(cx, |state, cx| {
                            // A reconnect can replace the process while the metadata read is in flight.
                            if state
                                .snapshot
                                .as_ref()
                                .map(|snapshot| snapshot.daemon.started_at.as_str())
                                == started_at.as_deref()
                                && state.daemon_outdated != outdated
                            {
                                state.daemon_outdated = outdated;
                                cx.notify();
                            }
                        });
                        tracing::trace!("completed background shell filesystem observation");
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
            }
        })
    }
}

/// Whether the fleetd binary the app would spawn was written after this daemon started.
#[must_use]
fn daemon_binary_is_newer(started_at: &str) -> bool {
    let Ok(path) = fleet_client::resolve_daemon_path() else {
        return false;
    };
    let Ok(modified) = std::fs::metadata(path).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    modified_after_start(modified, started_at)
}

fn modified_after_start(modified: SystemTime, started_at: &str) -> bool {
    let Ok(started) = chrono::DateTime::parse_from_rfc3339(started_at) else {
        return false;
    };
    let Ok(modified) = modified.duration_since(UNIX_EPOCH) else {
        return false;
    };
    let started_millis = started.timestamp_millis();
    started_millis >= 0 && modified.as_millis() > started_millis as u128
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_mtime_is_compared_to_the_observed_process_start() {
        let modified = UNIX_EPOCH + Duration::from_secs(10);
        assert!(modified_after_start(modified, "1970-01-01T00:00:05Z"));
        assert!(!modified_after_start(modified, "1970-01-01T00:00:15Z"));
        assert!(!modified_after_start(modified, "not-a-date"));
    }
}
