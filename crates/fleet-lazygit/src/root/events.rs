use super::*;

impl Lazygit {
    /// Drains the bridge's event stream into the state. Stored, never detached: dropping the task
    /// would cancel the future.
    pub(super) fn spawn_event_loop(bridge: &GitBridge, cx: &mut Context<Self>) -> Task<()> {
        let events = bridge.events();
        cx.spawn(async move |root, cx| {
            while let Ok(event) = events.recv().await {
                let mut batch = vec![event];
                for _ in 1..64 {
                    let Ok(event) = events.try_recv() else {
                        break;
                    };
                    batch.push(event);
                }
                if root
                    .update(cx, |root, cx| {
                        let mut dirty = false;
                        let now = Instant::now();
                        for event in batch {
                            dirty |= root.apply_event(event, now);
                        }
                        if dirty && root.active {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
    }

    /// Expires toasts and issues the periodic snapshot fallback.
    pub(super) fn spawn_ticker(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |root, cx| {
            loop {
                let timer = cx.background_executor().timer(TICK);
                timer.await;
                let updated = root.update(cx, |root, cx| {
                    let now = Instant::now();
                    let mut dirty = root.state.expire_toasts(now);
                    let interval = if root.window_active {
                        REFRESH_ACTIVE
                    } else {
                        REFRESH_IDLE
                    };
                    if root.active
                        && !root.state.refreshing
                        && root.state.fatal.is_none()
                        && now.duration_since(root.last_refresh) >= interval
                    {
                        root.last_refresh = now;
                        root.state.refreshing = true;
                        root.bridge.send(GitRequest::Snapshot);
                        dirty = true;
                    }
                    if dirty && root.active {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
        })
    }

    /// Applies one event and reports whether the frame it changed is on screen.
    pub(super) fn apply_event(&mut self, event: GitEvent, now: Instant) -> bool {
        let mut damaged = self.event_damage(&event);
        match event {
            GitEvent::ReadSuperseded => self.state.finish_read(),
            GitEvent::Opened { root } => {
                self.state.root = root;
            }
            GitEvent::OpenFailed { message } => {
                self.state.fatal = Some(message);
                self.state.refreshing = false;
            }
            GitEvent::Snapshot(snapshot) => {
                let previous = self.current_main_identity();
                self.last_refresh = now;
                let requests = self.state.apply_snapshot(snapshot);
                if self.active {
                    for request in requests {
                        self.send(request);
                    }
                }
                self.sync_main_len();
                self.reset_main_scroll_if_changed(&previous);
                if self.active && self.snapshot_dirty {
                    self.snapshot_dirty = false;
                    self.state.refreshing = true;
                    self.bridge.send(GitRequest::Snapshot);
                }
            }
            GitEvent::ReadFailed {
                label,
                identity,
                message,
            } => {
                // A read owns no in-progress guard and arms no escalation dialog.
                self.state.finish_read();
                if identity == crate::bridge::ReadIdentity::Snapshot {
                    self.state.refreshing = false;
                }
                let line = format!("{label}: {}", crate::state::error_line(&message));
                self.state.main.record_read_error(&identity, line.clone());
                self.last_error_label = None;
                self.state.last_error = Some(line);
            }
            GitEvent::Mutated { label, warning } => {
                self.state.finish_mutation();
                self.disarm_escalation(&label);
                self.finish_prompt_submission(&label, true);
                if self.last_error_label.as_deref() == Some(visible_label(&label)) {
                    self.state.last_error = None;
                    self.last_error_label = None;
                }
                if let Some(warning) = warning {
                    self.state.toast(
                        Toast::new(format!("{}: {warning}", visible_label(&label)))
                            .icon(Icon::TriangleAlert),
                        now,
                        TOAST_DWELL,
                    );
                }
            }
            GitEvent::Failed { label, message } => {
                self.state.finish_mutation();
                // A safe attempt that Git refused becomes the "are you sure?" dialog rather than
                // an error the user cannot act on.
                if let Some(escalation) = self.take_escalation(&label, &message) {
                    self.open_confirm(*escalation);
                    return true;
                }
                self.finish_prompt_submission(&label, false);
                self.last_error_label = Some(visible_label(&label).to_owned());
                self.state.last_error = Some(format!(
                    "{}: {}",
                    visible_label(&label),
                    crate::state::error_line(&message)
                ));
            }
            GitEvent::Command(event) => {
                if let Some(line) = command_line(&event) {
                    self.state.log_command(line);
                }
            }
            GitEvent::CommandsReseeded(records) => {
                self.state.command_log.clear();
                for record in records {
                    if let Some(line) = record_line(&record) {
                        self.state.log_command(line);
                    }
                }
            }
            GitEvent::WatcherFailed { message } => {
                self.last_error_label = None;
                self.state.last_error =
                    Some(format!("watcher: {}", crate::state::error_line(&message)));
            }
            GitEvent::Changed(change) => {
                tracing::debug!(
                    paths = change.paths.len(),
                    active = self.active,
                    "git watcher invalidation"
                );
                if self.active && !self.state.refreshing {
                    self.state.refreshing = true;
                    self.bridge.send(GitRequest::Snapshot);
                    // The status bar renders the in-flight hint from `refreshing`, so the frame
                    // this arm just changed has to be repainted.
                    damaged = true;
                } else {
                    self.snapshot_dirty = true;
                }
            }
            read => self.apply_read_result(read),
        }
        damaged
    }

    /// Whether the event changes something the current frame shows.
    ///
    /// Every read carries the request it answers, and the view may have moved on since: a result
    /// for content the main panel no longer shows is still applied, but repaints nothing.
    fn event_damage(&self, event: &GitEvent) -> bool {
        match event {
            GitEvent::ReadSuperseded | GitEvent::Changed(_) => false,
            GitEvent::Command(event) => {
                self.state.show_command_log && matches!(event.as_ref(), CommandEvent::Finished(_))
            }
            GitEvent::CommandsReseeded(_) => self.state.show_command_log,
            GitEvent::Snapshot(snapshot) => {
                self.state.snapshot.is_none() || snapshot.generation > self.state.epoch
            }
            GitEvent::FileDiff { path, .. } => {
                matches!(&self.state.main, MainContent::FileDiff { path: current, .. } if current == path)
            }
            GitEvent::CommitDiff { oid, .. } => {
                matches!(&self.state.main, MainContent::CommitDiff { oid: current, .. } | MainContent::CommitFiles { oid: current, .. } | MainContent::SubCommits { shown: Some(current), .. } if current == oid)
            }
            GitEvent::BranchDiff { name, .. } => {
                matches!(&self.state.main, MainContent::BranchDiff { name: current, .. } if current == name)
            }
            GitEvent::StashDiff { index, .. } => {
                matches!(&self.state.main, MainContent::StashDiff { index: current, .. } if current == index)
            }
            GitEvent::Conflict { path, .. } => {
                matches!(&self.state.main, MainContent::Conflict { path: current, .. } if current == path)
            }
            GitEvent::RefCommits { reference, .. } => {
                matches!(&self.state.main, MainContent::SubCommits { reference: current, .. } if current == reference)
            }
            GitEvent::CommitFileDiff { oid, path, .. } => {
                matches!(&self.state.main, MainContent::CommitFiles { oid: current, shown: Some(shown), .. } if current == oid && shown == path)
            }
            _ => true,
        }
    }

    /// Fills the main-panel slot a completed read belongs to, and drops it when the view has
    /// moved on to other content in the meantime.
    fn apply_read_result(&mut self, event: GitEvent) {
        self.state.finish_read();
        match event {
            GitEvent::FileDiff {
                path,
                unstaged,
                staged,
            } => {
                self.state
                    .main
                    .clear_read_error(&crate::bridge::ReadIdentity::FileDiff(path.clone()));
                if self.state.apply_file_diff(&path, unstaged, staged) {
                    self.retarget_staging_side();
                    self.sync_main_len();
                    // The diff usually arrives after `enter` opened staging mode, so this is
                    // where the cursor first lands on something selectable.
                    self.snap_to_change();
                }
            }
            GitEvent::CommitDiff {
                oid,
                diff,
                files: changed,
            } => {
                self.state
                    .main
                    .clear_read_error(&crate::bridge::ReadIdentity::CommitDiff(oid.clone()));
                self.apply_commit_diff(&oid, diff, changed);
            }
            GitEvent::BranchDiff { name, diff } => {
                self.state
                    .main
                    .clear_read_error(&crate::bridge::ReadIdentity::BranchDiff(name.clone()));
                if let MainContent::BranchDiff {
                    name: current,
                    diff: slot,
                    ..
                } = &mut self.state.main
                    && *current == name
                {
                    crate::state::keep_or_replace(slot, diff);
                    self.sync_main_len();
                }
            }
            GitEvent::StashDiff { index, diff } => {
                self.state
                    .main
                    .clear_read_error(&crate::bridge::ReadIdentity::StashDiff(index));
                if let MainContent::StashDiff {
                    index: current,
                    diff: slot,
                    ..
                } = &mut self.state.main
                    && *current == index
                {
                    *slot = Some(diff);
                    self.sync_main_len();
                }
            }
            GitEvent::Conflict { path, file } => {
                self.state
                    .main
                    .clear_read_error(&crate::bridge::ReadIdentity::Conflict(path.clone()));
                if let MainContent::Conflict {
                    path: current,
                    file: slot,
                    ..
                } = &mut self.state.main
                    && *current == path
                {
                    *slot = Some(file);
                }
            }
            GitEvent::RefCommits {
                reference,
                commits: loaded,
            } => {
                self.state
                    .main
                    .clear_read_error(&crate::bridge::ReadIdentity::RefCommits(reference.clone()));
                let mut reload = None;
                if let MainContent::SubCommits {
                    reference: current,
                    commits,
                    shown,
                    diff,
                    diff_error,
                    ..
                } = &mut self.state.main
                    && *current == reference
                {
                    let selected = commits
                        .get(self.state.cursors.main.index())
                        .map(|commit| commit.oid.clone());
                    *commits = loaded;
                    self.state.cursors.main.retain(commits.len(), |_| {
                        let selected = selected.as_ref()?;
                        commits.iter().position(|commit| &commit.oid == selected)
                    });
                    let retained = commits
                        .get(self.state.cursors.main.index())
                        .map(|commit| commit.oid.clone());
                    if selected != retained && shown.is_some() {
                        *shown = retained.clone();
                        *diff = None;
                        *diff_error = None;
                        reload = retained;
                    }
                }
                self.sync_main_len();
                if let Some(oid) = reload {
                    self.send(GitRequest::CommitDiff(oid));
                }
            }
            GitEvent::CommitFileDiff { oid, path, diff } => {
                self.state
                    .main
                    .clear_read_error(&crate::bridge::ReadIdentity::CommitFileDiff {
                        oid: oid.clone(),
                        path: path.clone(),
                    });
                if let MainContent::CommitFiles {
                    oid: current,
                    shown: Some(shown),
                    diff: slot,
                    ..
                } = &mut self.state.main
                    && *current == oid
                    && *shown == path
                {
                    *slot = Some(diff);
                }
            }
            _ => {}
        }
    }

    /// One commit read fills three different views: the commit's own patch, the commit-files
    /// drill-down (which needs the file list, and shows the whole patch on its header row) and a
    /// sub-commit's patch.
    fn apply_commit_diff(
        &mut self,
        oid: &fleet_git::ObjectId,
        diff: Arc<fleet_git::Diff>,
        changed: Arc<[fleet_git::CommitFile]>,
    ) {
        match &mut self.state.main {
            MainContent::CommitDiff {
                oid: current,
                diff: slot,
                files,
                ..
            } if current == oid => {
                crate::state::keep_or_replace(slot, diff);
                *files = changed;
                self.sync_main_len();
            }
            MainContent::CommitFiles {
                oid: current,
                files,
                whole,
                shown,
                diff: slot,
                ..
            } if current == oid => {
                *whole = Some(diff);
                *files = changed;
                if shown.is_none() {
                    *slot = whole.clone();
                }
                self.sync_main_len();
                self.request_main_selection();
            }
            MainContent::SubCommits {
                shown: Some(current),
                diff: slot,
                ..
            } if current == oid => {
                *slot = Some(diff);
            }
            _ => {}
        }
    }

    /// Forgets an armed escalation once its mutation succeeded.
    pub(super) fn disarm_escalation(&mut self, label: &str) {
        if self
            .state
            .escalation
            .as_ref()
            .is_some_and(|(armed, _)| armed == label)
        {
            self.state.escalation = None;
        }
    }

    /// Takes the escalation armed for `label`, if the failure that just arrived is its.
    pub(super) fn take_escalation(&mut self, label: &str, message: &str) -> Option<Box<Confirm>> {
        match &self.state.escalation {
            Some((armed, _)) if armed == label => {
                let confirm = self.state.escalation.take().map(|(_, confirm)| confirm);
                is_unmerged_delete_failure(message)
                    .then_some(confirm)
                    .flatten()
            }
            _ => None,
        }
    }

    pub(super) fn tracked_label(&mut self, label: &str) -> String {
        self.next_request_id = self.next_request_id.wrapping_add(1);
        format!("{label}\u{1f}{}", self.next_request_id)
    }

    fn finish_prompt_submission(&mut self, label: &str, succeeded: bool) {
        if self.pending_prompt.as_deref() != Some(label) {
            return;
        }
        self.pending_prompt = None;
        if succeeded && matches!(self.state.overlay(), Some(Overlay::Prompt(_))) {
            self.state.pop_overlay();
        }
    }

    /// Dispatches one request and books it.
    ///
    /// Mutations and reads are counted separately: `pending` is the guard `q` asks about, and a
    /// read that fails must never touch it.
    pub(super) fn send(&mut self, request: GitRequest) {
        if matches!(request, GitRequest::Mutate { .. }) {
            self.state.begin_mutation();
            self.state.refreshing = true;
        } else if !matches!(
            request,
            GitRequest::Snapshot | GitRequest::Shutdown | GitRequest::SetDiffContext(_)
        ) {
            self.state.begin_read();
        }
        self.bridge.send(request);
    }

    pub(super) fn mutate(&mut self, label: &str, mutation: Mutation) {
        self.send(GitRequest::Mutate {
            label: label.to_owned(),
            mutation: Box::new(mutation),
        });
    }

    pub(super) fn quit(&mut self, _: &global::Quit, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.mutation_in_flight() || self.state.operation() != OperationState::None {
            self.open_confirm(Confirm {
                title: "Quit".to_owned(),
                target: self.state.root.display().to_string(),
                facts: vec!["An operation is still in progress.".to_owned()],
                danger: false,
                outcome: ConfirmOutcome::Request(Box::new(GitRequest::Shutdown)),
            });
            cx.notify();
            return;
        }
        self.bridge.send(GitRequest::Shutdown);
        cx.emit(LazygitEvent::Quit);
    }

    pub(super) fn refresh(&mut self, _: &global::Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.state.refreshing = true;
        self.bridge.send(GitRequest::Snapshot);
        cx.notify();
    }
}

fn visible_label(label: &str) -> &str {
    label.split_once('\u{1f}').map_or(label, |(label, _)| label)
}

fn is_unmerged_delete_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("not fully merged") || message.contains("is not merged")
}

/// One command-log line, or `None` for events that add nothing.
fn command_line(event: &CommandEvent) -> Option<String> {
    match event {
        CommandEvent::Started(_) => None,
        CommandEvent::Finished(record) => record_line(record),
    }
}

fn record_line(record: &fleet_git::CommandRecord) -> Option<String> {
    let argv = record.display_argv.join(" ");
    let elapsed = record
        .elapsed
        .map(|elapsed| format!(" ({} ms)", elapsed.as_millis()))
        .unwrap_or_default();
    match &record.outcome {
        CommandOutcome::Running => None,
        CommandOutcome::Success { .. } => Some(format!("✓ {argv}{elapsed}")),
        CommandOutcome::Failed {
            status,
            stderr_preview,
            ..
        } => {
            let stderr = String::from_utf8_lossy(stderr_preview);
            let first = stderr.lines().next().unwrap_or_default();
            Some(format!(
                "✗ {argv}{elapsed} — exit {}{}",
                status.unwrap_or(-1),
                if first.is_empty() {
                    String::new()
                } else {
                    format!(": {first}")
                }
            ))
        }
        CommandOutcome::TimedOut => Some(format!("✗ {argv} — timed out")),
        CommandOutcome::Cancelled => Some(format!("✗ {argv} — cancelled")),
        CommandOutcome::OutputLimitExceeded => Some(format!("✗ {argv} — output limit exceeded")),
        CommandOutcome::SpawnFailed(message) => Some(format!("✗ {argv} — {message}")),
    }
}

#[cfg(test)]
mod command_log_tests {
    use std::time::SystemTime;

    use fleet_git::{CommandKind, CommandOutcome, CommandRecord};

    use super::record_line;

    fn record(outcome: CommandOutcome) -> CommandRecord {
        CommandRecord {
            id: 1,
            kind: CommandKind::Read,
            display_argv: vec!["git".to_owned(), "status".to_owned()],
            started_at: SystemTime::UNIX_EPOCH,
            elapsed: None,
            outcome,
        }
    }

    #[test]
    fn terminal_command_outcomes_are_visible() {
        assert_eq!(
            record_line(&record(CommandOutcome::Cancelled)).as_deref(),
            Some("✗ git status — cancelled")
        );
        assert_eq!(
            record_line(&record(CommandOutcome::OutputLimitExceeded)).as_deref(),
            Some("✗ git status — output limit exceeded")
        );
    }
}
