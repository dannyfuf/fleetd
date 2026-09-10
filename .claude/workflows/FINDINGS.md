# Quality pass — working notes

Temporary. Tracks the chore/cleaning quality pass. Deleted together with `quality-pass.js` and
`quality-fix.js` when the pass lands.

## Status

- [x] Audit run: 13 area auditors, each loading the project skill that governs it
- [x] Adversarial verification: 85 of ~100 findings survived (73 confirmed, 12 plausible)
- [ ] Wave 1 (daemon core) landed
- [ ] Wave 2 (git, lazygit, app) landed
- [ ] Wave 3 (ui-kit, palette) landed
- [ ] `make lint` + `make test` green
- [ ] Scaffolding removed

## Findings (85)

### agents

- **high** `agents-plan-gate-rewrites-settled-turn-footer` — `settle_blocked_time` charges gate wait to a turn whose `ended` footer is already frozen, so answering OpenCode's plan gate retroactively shrinks the completed turn's duration, to zero when the user reads for longer than the turn ran.
  <br>`crates/fleet-core/src/agents/projection.rs:845` · CONFIRMED · trivial
- **high** `agents-pending-claude-inputs-head-of-line` — `pending_claude_inputs` is only drained while the front entry's turn matches the turn that just started, so one entry whose `TurnStarted` never lands blocks the deque forever: every later user prompt is accepted and sent to Claude but never recorded in the transcript, and the deque grows without bound.
  <br>`crates/fleet-daemon/src/services/agents/manager.rs:784` · PLAUSIBLE · medium
- **high** `agents-claude-stop-select-race-wedges-turn` — When the child exits from the stdin close before the supervisor polls the queued `Stop` command, `select!` takes the `child.wait()` branch, drops the `done` oneshot, and `RunningProcess::stop` returns `ProviderError::Exited`, so `AgentSessionManager::stop` reports a conflict and skips every settlement it owes — leaving an in-flight turn `Running` forever.
  <br>`crates/fleet-daemon/src/services/agents/providers/claude/process.rs:352` · CONFIRMED · small
- **medium** `agents-projection-cloned-per-event` — Every applied event deep-clones the entire `ThreadProjection` — items, turns, notices, checkpoints, gates and all their `String`/`Value` payloads — only so `update_record` can read `projection.title`.
  <br>`crates/fleet-daemon/src/services/agents/manager.rs:1028` · CONFIRMED · trivial
- **medium** `agents-opencode-set-mode-skips-effective-mode` — `OpenCodeProvider::set_mode` records the requested mode verbatim, skipping the `effective_mode` downgrade `start` applies, so the metadata row can advertise a permission promise the server will never keep.
  <br>`crates/fleet-daemon/src/services/agents/providers/opencode/mod.rs:386` · CONFIRMED · small
- **medium** `agents-create-rollback-leaks-provider` — The `create` rollback discards the result of `provider.stop()`, so a provider that fails to stop after a failed index write leaves an orphaned `claude`/`opencode serve` child with no thread record, no log line and no way to reach it again.
  <br>`crates/fleet-daemon/src/services/agents/manager.rs:277` · CONFIRMED · trivial

### app-async

- **high** `app-async-attach-retry-sticky-error-never-cleared` — A failed terminal attach writes a `job: None` sticky error that nothing ever clears, so a single transient attach failure leaves "could not attach terminal: …; retrying" pinned in the status bar for the rest of the session even after the retry succeeds.
  <br>`crates/fleet-app/src/terminal/surface.rs:613` · PLAUSIBLE · small
- **high** `app-async-diff-cache-poisoned-by-cancelled-syntax` — `reload` publishes the prepared `DiffModel` into the process-wide LRU cache before the syntax pass runs, so any cancelled or superseded pass permanently caches an un-highlighted model that every later view adopts without ever re-running the syntax job.
  <br>`crates/fleet-lazygit/src/diff_view.rs:444` · CONFIRMED · small
- **medium** `app-async-agent-file-latch-outlives-view` — The `@`-completion file-listing latch `agent_files` is keyed by thread and never pruned, while the view it feeds (`agent_views`) is pruned whenever a thread leaves the snapshot, so a rebuilt agent tab is offered no file listing for the rest of the session.
  <br>`crates/fleet-app/src/screens/workspace/agent.rs:284` · CONFIRMED · trivial
- **medium** `app-async-mutation-backpressure-stalls-bridge-control-loop` — The bridge command loop `await`s the send into the request lane from inside its `tokio::select!` arm, so a backed-up mutation lane parks the whole loop — health probes, `Reconnect` and `Shutdown` all stop being served — instead of shedding load the way `Bridge::send` already does.
  <br>`crates/fleet-app/src/bridge/runtime.rs:103` · PLAUSIBLE · medium
- **medium** `app-async-lazygit-drain-loop-never-yields` — Lazygit's bridge drain loop batches but never yields, so an always-ready receiver keeps the GPUI foreground executor inside the loop and starves rendering — the exact hazard fleet-app's identical loop guards against with a 1 ms timer.
  <br>`crates/fleet-lazygit/src/root/events.rs:9` · CONFIRMED · trivial
- **medium** `app-async-before-timeout-duplicated` — `before_timeout` — the crate's only request-deadline primitive — exists as two byte-identical private copies in unrelated modules, so the timeout semantics every bounded bridge request depends on can drift between the terminal and the dialogs.
  <br>`crates/fleet-app/src/terminal/surface.rs:629` · CONFIRMED · trivial
- **low** `app-async-pr-refresh-task-holds-strong-owner` — `schedule_pr_refresh` stores a task holding a strong `Entity<HubState>` *on that same HubState*, a self-sustaining cycle for the whole PR TTL — while `schedule_inspection`, seventy lines below in the same file and doing the same job, downgrades both entities.
  <br>`crates/fleet-app/src/screens/hub/cache.rs:455` · CONFIRMED · trivial

### app-render-perf

- **high** `app-render-perf-board-model-derived-in-render` — The whole board model — grouping, filtering, per-field `to_lowercase()`, per-column sorting, the orphan scan — is recomputed inside `board_screen::render`, so every frame pays O(cards × statuses) plus one heap allocation per card per searchable field.
  <br>`crates/fleet-app/src/views/board_screen.rs:88` · CONFIRMED · large
- **high** `app-render-perf-kanban-column-unvirtualized` — `KanbanColumn` renders every tile it is given into a plain `overflow_y_scroll` div, and `board_screen::columns` builds an `AnyElement` for every card of every column, so element construction cost scales with the board rather than with the viewport.
  <br>`crates/fleet-ui-kit/src/components/kanban_column.rs:153` · CONFIRMED · large
- **high** `app-render-perf-palette-rebuilds-on-every-notify` — While the palette overlay is open, every single `AppState` notify rebuilds every palette candidate, because the observation calls `palette::refresh` unconditionally instead of going through the `prepared_query` guard the keystroke path uses.
  <br>`crates/fleet-app/src/dialogs/palette.rs:1078` · PLAUSIBLE · medium
- **high** `app-render-perf-markdown-parsed-and-highlighted-per-frame` — `MarkdownText::render` re-parses its source markdown on every frame, and the transcript's code blocks re-run the whole highlighting lexer and re-allocate their text on every frame, defeating gpui's two-frame `LineLayoutCache`.
  <br>`crates/fleet-ui-kit/src/components/markdown/render.rs:177` · CONFIRMED · medium
- **medium** `app-render-perf-card-picker-derives-candidates-in-render` — The card-property picker derives its whole candidate list inside `render` — walking every card, cloning assignees, sorting and deduping — and the filter allocates a fresh lowercased query `String` for every option it tests.
  <br>`crates/fleet-app/src/dialogs/card_picker/schema.rs:171` · CONFIRMED · small
- **medium** `app-render-perf-agent-thread-notifies-from-render` — `AgentThreadView::render` mutates the composer entity mid-render, and that mutation calls `cx.notify()` on the input, scheduling an extra frame every time the composer's focus-ring state changes.
  <br>`crates/fleet-app/src/screens/agent_thread/mod.rs:1192` · CONFIRMED · small
- **medium** `app-render-perf-diff-shapes-every-row-on-the-foreground-thread` — `diff_list` shapes every payload row of the diff model on the foreground thread the first time a model is drawn, through a throwaway `WindowTextSystem` whose shaping cache is discarded — a synchronous hitch proportional to the whole diff, inside the render path.
  <br>`crates/fleet-lazygit/src/views/diff.rs:489` · CONFIRMED · medium

### app-shell

- **medium** `app-shell-palette-cursor-not-clamped-on-refresh` — `palette::refresh` replaces `host.palette.rows` without clamping `host.palette.cursor`, and it is re-run on every `AppState` notification while the palette is open, so a shrinking snapshot leaves the cursor past the end and `Enter` becomes a silent no-op.
  <br>`crates/fleet-app/src/dialogs/palette.rs:1086` · CONFIRMED · trivial
- **medium** `app-shell-doctor-context-undocumented-and-retry-dead` — The three `Daemon > Doctor` bindings exist in the key table but have no rows in `docs/KEYMAP.md`, and because `Daemon > Doctor` replaces `Daemon > Down` in the rendered context chain, case B's documented `r` (retry starting fleetd) silently stops working while the doctor report is on screen.
  <br>`crates/fleet-app/src/keymap.rs:592` · CONFIRMED · small
- **medium** `app-shell-agent-thread-focus-not-restored-on-dismiss` — `focus_surface` hands responsibility for `FocusTarget::AgentThread` to the thread view without a guard, but `AgentThreadView::focus_composer` declines when the thread's host is unreachable, so dismissing an overlay over an unreachable native agent tab leaves focus on the now-unmounted `overlay_focus` and no surface owns the keyboard.
  <br>`crates/fleet-app/src/shell/root/focus.rs:427` · CONFIRMED · small
- **medium** `app-shell-filter-accept-is-tested-through-a-copy` — `filter::accept` (the `Enter` handler wired into the Filter overlay) is untested; the two `#[gpui::test]`s in the file exercise `accept_for_test`, a `#[cfg(test)]`-only reimplementation whose behaviour already differs from production, so a regression in the real path passes CI.
  <br>`crates/fleet-app/src/dialogs/filter.rs:193` · CONFIRMED · small
- **low** `app-shell-palette-recomputes-every-candidate-per-notification` — While the palette is open, every `AppState` notification synchronously rebuilds the whole candidate list on the foreground thread — sorting sessions, building a `SnapshotIndex` and allocating a `String` per row — even when nothing the palette shows has changed.
  <br>`crates/fleet-app/src/dialogs/host.rs:287` · PLAUSIBLE · medium

### app-state

- **high** `app-state-unprimed-mirror-never-recovers` — A mirror created by a diff frame is never primed and never desynced, so the Shell never sends `RequestFullFrame` for it and the pane stays on "attaching" forever.
  <br>`crates/fleet-app/src/shell/root/events.rs:53` · CONFIRMED · trivial
- **medium** `app-state-reconnect-dropped-while-link-alive` — `retry_daemon` unconditionally enters `DaemonLink::Starting` (the full-window cold-start splash) while the bridge runtime silently discards `Command::Reconnect` whenever it still holds a link, so pressing `r` on the "fleetd stopped" banner blanks the whole UI and removes the banner's own recovery keys until an unrelated health check fires.
  <br>`crates/fleet-app/src/bridge/runtime.rs:109` · CONFIRMED · small
- **medium** `app-state-agent-view-notify-fans-out-to-appstate` — Every `AgentThreadView` notification is relayed into an unconditional `AppState` notify, which re-runs every `AppState` observer — including `Shell::synchronize_surfaces` — so each streamed agent delta costs two full model-wide reconciliation passes instead of one.
  <br>`crates/fleet-app/src/screens/workspace/agent.rs:187` · PLAUSIBLE · medium

### core-git-term

- **medium** `core-git-term-remote-head-symref-unused` — `remote_branches` filters the remote's symbolic HEAD by testing `name.ends_with("/HEAD")`, but `%(refname:short)` never produces that suffix for `refs/remotes/<remote>/HEAD`, so a remote whose name contains a slash lists its symbolic HEAD as a phantom branch.
  <br>`crates/fleet-git/src/parse/refs.rs:41` · CONFIRMED · small
- **medium** `core-git-term-network-command-60s-timeout` — Every git invocation, including `fetch`/`pull`/`push`, is killed after a single hard-coded 60 s deadline; there is no per-kind timeout and no way for a caller to raise it.
  <br>`crates/fleet-git/src/command.rs:301` · CONFIRMED · small
- **medium** `core-git-term-reword-abort-error-swallowed` — The reword transaction's rollback (`git rebase --abort`) discards its `Result` entirely, so a failed abort leaves the repository stopped mid-rebase while the caller is told only about the original error.
  <br>`crates/fleet-git/src/rebase.rs:117` · CONFIRMED · trivial
- **low** `core-git-term-uncommented-fire-and-forget-sends` — Several fire-and-forget channel sends in fleet-git and fleet-term use `let _ =` without the comment the project rule requires, so a reader cannot tell a sanctioned drop from an unhandled error.
  <br>`crates/fleet-git/src/command_log.rs:88` · CONFIRMED · trivial
- **low** `core-git-term-bare-tracing-macros` — `fleet-term` imports `tracing::warn` and calls the bare `warn!` macro at 24 sites, against the workspace rule that log macros are always fully qualified.
  <br>`crates/fleet-term/src/ghostty.rs:25` · CONFIRMED · trivial

### daemon-async

- **high** `daemon-async-config-reread-per-request` — Every dispatched request — including every terminal keystroke, mouse and wheel event — re-reads and re-parses `config.json` from disk behind one global async mutex, because `dispatch_routed_with_owner` starts with an uncached `config.load().await`.
  <br>`crates/fleet-daemon/src/services/dispatch.rs:31` · CONFIRMED · small
- **high** `daemon-async-accept-error-kills-the-daemon` — Any error returned by `UnixListener::accept` — including the transient `ECONNABORTED`, `EMFILE` and `ENFILE` — breaks the accept loop, and `main` propagates that error out of `main`, terminating `fleetd` and every PTY it hosts.
  <br>`crates/fleet-daemon/src/server/listener.rs:190` · CONFIRMED · small
- **medium** `daemon-async-quiesce-waits-forever` — `quiesce_repo` cancels only cancellable jobs but then unconditionally awaits *every* active job with no timeout, so a `DeleteRepo` request blocks forever behind a non-cancellable job while holding the repository-deletion tombstone that rejects all other work for that repo.
  <br>`crates/fleet-daemon/src/jobs/manager.rs:675` · CONFIRMED · small
- **medium** `daemon-async-progress-io-under-global-lock` — `record_progress` and `append_log_line` perform `create_dir_all`, `OpenOptions::open` and buffered `writeln!` file IO while holding `JobManagerInner::state`, the one `std::sync::Mutex` that every other job operation (`list`, `record`, `cancel`, `submit`, `finish`, `prune`) also takes — all on tokio runtime threads.
  <br>`crates/fleet-daemon/src/jobs/manager/logs.rs:24` · PLAUSIBLE · medium
- **low** `daemon-async-shutdown-select-not-biased` — Every `tokio::select!` in the server with a `shutdown.cancelled()` branch omits `biased;`, so once shutdown is requested the control branch competes at random against permanently-ready data branches instead of winning deterministically.
  <br>`crates/fleet-daemon/src/server/connection.rs:142` · CONFIRMED · trivial
- **low** `daemon-async-lag-recovery-swallows-failures` — The broadcast-lag recovery path discards the result of every `RequestFullFrame` dispatch with `let _ignored`, while the disconnect-cleanup path a few lines away classifies the identical dispatch result and logs anything that is not `NotFound` — so a failed resync leaves the client's terminal grid permanently stale with no trace.
  <br>`crates/fleet-daemon/src/server/connection.rs:516` · CONFIRMED · trivial

### daemon-services

- **high** `daemon-services-startup-recovery-aborts-on-first-failure` — One unreadable or non-git worktree directory aborts the entire startup recovery scan, so every later repository's attempts are left unrecovered and post-create hook intents are never reconciled.
  <br>`crates/fleet-daemon/src/services/worktrees/recovery.rs:27` · CONFIRMED · small
- **high** `daemon-services-post-create-hooks-relaunched-after-crash` — The detached post-create runner's pid is persisted only after the child is spawned, so a daemon crash inside that window makes startup recovery launch the user's post-create hooks a second time, concurrently with the first still-running copy.
  <br>`crates/fleet-daemon/src/services/worktrees/post_create.rs:101` · CONFIRMED · small
- **high** `daemon-services-remote-frame-pump-never-rebound` — `RemoteTerminalFrames` keys its frame pump by host only, never records which endpoint it is bound to, and never removes the entry when the pump task exits, so after a host's endpoint is replaced no new pump is ever started and remote terminals stop receiving frames permanently.
  <br>`crates/fleet-daemon/src/services/router/sessions.rs:62` · CONFIRMED · small
- **medium** `daemon-services-post-create-waits-on-recycled-pid` — After a daemon restart the post-create reconciliation job busy-waits on a raw pid from the previous boot with no start-time check, no timeout and no cancellation, so a recycled pid pins the job — and its dedupe key — forever.
  <br>`crates/fleet-daemon/src/services/worktrees/post_create.rs:111` · CONFIRMED · medium
- **medium** `daemon-services-failed-remote-detach-leaks-attachment` — `forward` only runs the detach bookkeeping when `attaching != response.is_ok()`, so a `DetachTerminal` that the remote rejects leaves the local attachment count above zero forever — the frames pump is never torn down and the terminal is falsely restored on reconnect.
  <br>`crates/fleet-daemon/src/services/router/mod.rs:171` · CONFIRMED · trivial
- **medium** `daemon-services-discovery-retained-map-unbounded` — `DiscoveryState::retained` accumulates one entry per finished discovered process and is only ever pruned when the same pid is reused, so it grows without bound and is rescanned in full for every candidate on every discovery pass.
  <br>`crates/fleet-daemon/src/services/watch_discovery.rs:45` · CONFIRMED · small
- **medium** `daemon-services-terminal-kill-error-swallowed` — `close_terminal_if_present` and `kill_if_present` discard the result of `TerminalHost::kill()` with an uncommented `let _ =`, and because the host-event forwarder thread holds its own `Arc<TerminalHost>`, a failed kill leaks the PTY child, the owner thread and the forwarder thread permanently.
  <br>`crates/fleet-daemon/src/services/sessions/registry.rs:239` · CONFIRMED · small
- **low** `daemon-services-orphan-worktree-when-card-archived-mid-clone` — If a card is archived while its worktree is being cloned, `create_and_link_worktree` returns the generic archived-card refusal and drops the worktree it just created on the floor, unlike the adjacent branch that explicitly names the orphan.
  <br>`crates/fleet-daemon/src/services/boards/worktree.rs:196` · PLAUSIBLE · trivial

### ipc

- **high** `ipc-endpoint-registry-race` — `Machines::endpoint` is a check-then-act across two separate lock acquisitions, so concurrent callers each build and `connect()` a distinct `RemoteLink` for the same host; the loser is overwritten in the map but its actor and SSH child are never closed.
  <br>`crates/fleet-daemon/src/machines/registry.rs:107` · CONFIRMED · trivial
- **high** `ipc-link-write-unbounded` — `connected_loop` awaits `transport.send(encoded)` with no deadline and outside any `select!`, so a full SSH pipe wedges the link actor permanently — `RemoteLink::close()` awaits that task and never returns.
  <br>`crates/fleet-daemon/src/machines/link.rs:476` · CONFIRMED · small
- **medium** `ipc-shutdown-event-unobservable` — `Listener::run` publishes `Event::DaemonShuttingDown` only after its loop breaks on the shutdown token, but every `Connection::run` breaks on that same token first, so no client ever receives the event on a SIGTERM/Ctrl-C shutdown.
  <br>`crates/fleet-daemon/src/server/listener.rs:212` · CONFIRMED · small
- **medium** `ipc-disconnect-detach-unbounded` — The disconnect cleanup loop dispatches one `DetachTerminal` per attached terminal with no deadline; for a remote terminal that call goes over `RemoteEndpoint::request`, which has no deadline either, so a client disconnect can hold its `MAX_CONNECTIONS` admission permit forever.
  <br>`crates/fleet-daemon/src/server/connection.rs:343` · CONFIRMED · small
- **medium** `ipc-subscribe-extend-vs-replace` — The daemon *adds* to a connection's subscription set on `Subscribe`, while the client API documents and models it as a *replace*, so the two sides disagree about what a connection is subscribed to and a reconnect silently changes the delivered event set.
  <br>`crates/fleet-daemon/src/server/connection.rs:172` · PLAUSIBLE · trivial
- **low** `ipc-nudge-doc-drift` — The authoritative `RemoteEndpoint` trait listing in docs/REMOTE-MACHINES.md § 5 omits `nudge_reconnect`, a trait method the bootstrap flow depends on, and § 5's backoff paragraph never mentions the nudge or the retained permit.
  <br>`docs/REMOTE-MACHINES.md:95` · CONFIRMED · trivial
- **low** `ipc-nudge-connected-test-vacuous` — `nudge_on_a_connected_link_does_not_disturb_it` never connects the link, so it asserts nothing about the connected case; meanwhile the trait's own doc comment claims a nudge is a no-op on a connected link, which `RemoteLink::nudge_reconnect` deliberately is not.
  <br>`crates/fleet-daemon/src/machines/link.rs:960` · CONFIRMED · small

### tests

- **high** `tests-fake-shell-streaming-diverges` — `FakeShell::run_streaming` returns the rule's `stdout`/`stderr` inside the `ShellResult` and ignores `command.timeout`, while `RealShell::run_streaming` always returns empty `stdout`/`stderr` and can return `DaemonError::Timeout`; service tests therefore assert output that production never produces.
  <br>`crates/fleet-daemon/src/testing/fakes.rs:115` · CONFIRMED · medium
- **high** `tests-pool-freshness-window-untested` — The hot-pool staleness decision reads `Utc::now()` directly instead of the injectable `Clock` adapter, and no test in the workspace ever exercises the `age >= config.hot_freshness_ms` branch — the default 60 s window makes it unreachable in a test.
  <br>`crates/fleet-daemon/tests/pool_lifecycle.rs:83` · CONFIRMED · small
- **high** `tests-reexec-harness-passes-on-zero-tests` — `isolated_test` re-executes the test binary with `--exact <name>` and only checks the child's exit status; libtest exits 0 when the filter matches nothing, so any drift between the hand-written name literal and the function name silently turns ten tests into no-ops that still report as passing.
  <br>`crates/fleet-daemon/tests/sessions_lifecycle.rs:795` · CONFIRMED · trivial
- **medium** `tests-tautological-write-budget-assert` — `nearly_expired_command_retains_connection_write_budget` builds no command and asserts a relation between two compile-time constants, so it can never fail and never touches the code path its name describes.
  <br>`crates/fleet-client/src/connection.rs:1065` · CONFIRMED · small
- **medium** `tests-connection-resize-coalescing-untested` — The reconnect queue's two most delicate branches — dropping a superseded `ResizeTerminal` for the same terminal, and expiring queued commands with the "timed out while reconnecting" message — have no test at all, although both are plain functions in the same module as `mod tests`.
  <br>`crates/fleet-client/src/connection.rs:652` · CONFIRMED · small
- **medium** `tests-link-nudge-connected-misnamed` — `nudge_on_a_connected_link_does_not_disturb_it` never connects the link; its own comment says "Not connected yet", so the invariant in its name — a nudge on an established link must not tear it down — is untested while the name advertises that it is covered.
  <br>`crates/fleet-daemon/src/machines/link.rs:959` · CONFIRMED · medium
- **medium** `tests-daemon-shutdown-event-assertion-optional` — `a_shutdown_reads_as_a_lost_daemon_not_as_a_crash` makes its `DaemonShuttingDown` observation optional — if the event never arrives it falls through to a ping-failure loop that passes — so no test in the workspace verifies that the daemon actually publishes `Event::DaemonShuttingDown`.
  <br>`crates/fleet-daemon/tests/server_events.rs:214` · CONFIRMED · medium
- **medium** `tests-agent-manager-wallclock-polling` — The agent-manager suite drives every asynchronous assertion through a wall-clock `settle` loop (5 s deadline, 5 ms real sleeps) under an unpaused `#[tokio::test]`, although the crate already dev-depends on `tokio` with `test-util`.
  <br>`crates/fleet-daemon/src/services/agents/manager/tests.rs:363` · PLAUSIBLE · medium
- **low** `tests-assert-frame-no-track-caller` — `assert_frame`, the golden-frame helper called ~30 times from a single test function, has no `#[track_caller]`, so any golden mismatch reports line 30 or 37 of the helper instead of the wire message that broke.
  <br>`crates/fleet-proto/tests/compatibility.rs:23` · CONFIRMED · trivial

### ui-kit-components

- **high** `ui-kit-components-question-digit-range-clamped` — `question_actions` clamps the advertised digit range to `1–4` while the card renders and honours up to `options.len() + allow_other` numbered rows, so on a four-option question that also offers free text the fifth row ("Something else…") is drawn as `5` but is advertised nowhere and bound nowhere.
  <br>`crates/fleet-ui-kit/src/components/agent/decision_card.rs:173` · CONFIRMED · medium
- **medium** `ui-kit-components-tab-strip-doc-drift` — The §6 `TerminalTabStrip` entry documents an API signature the code does not have (`agent_status(StatusKind)` vs `agent_status(TerminalAgentState)`), omits the `attention` and `kind` builders entirely, and states a dot rule the code deliberately contradicts.
  <br>`docs/DESIGN-SYSTEM.md:973` · CONFIRMED · small
- **medium** `ui-kit-components-tab-strip-gallery-gap` — Four shipped `TerminalTab` visual states — `attention`, `unread`, `agent_status` (working and finished) and `kind(TerminalTabKind::Native)` — appear in no gallery panel, including the one whose label claims to show "every mark".
  <br>`crates/fleet-ui-kit/examples/gallery_terminal.rs:408` · CONFIRMED · small
- **medium** `ui-kit-components-kanban-tiles-eager` — `KanbanColumn` takes its rows as an already-built `Vec<AnyElement>`, so every card in every column is constructed, boxed and laid out on every frame regardless of the viewport — the column API makes virtualization structurally impossible, unlike `ListView`, which takes `(id, count, render_row)`.
  <br>`crates/fleet-ui-kit/src/components/kanban_column.rs:95` · PLAUSIBLE · large
- **low** `ui-kit-components-cycler-tabs-gallery-gap` — Two production-used visual variants have no gallery panel: `Cycler::off_grid(true)` (both arrows stay live for a persisted value outside the configured steps) and `SegmentedTabs::underlined(false)` (selected background instead of the accent underline).
  <br>`crates/fleet-ui-kit/examples/gallery_input.rs:725` · CONFIRMED · small
- **low** `ui-kit-components-kv-trailing-slot-dropped` — `KeyValueList::trailing` is silently discarded when no `title` is set: the trailing element is only ever consumed inside `title.map(..)`, so `KeyValueList::new().trailing(stamp)` renders nothing at all.
  <br>`crates/fleet-ui-kit/src/components/key_value_list.rs:96` · CONFIRMED · trivial
- **low** `ui-kit-components-role-without-accessible-name` — `control::on_activate` stamps `gpui::Role::Button` on every element it makes clickable without ever setting an accessible name, so the whole terminal tab strip and the new-tab affordance enter the accessibility tree as anonymous buttons.
  <br>`crates/fleet-ui-kit/src/components/control.rs:32` · CONFIRMED · small

### ui-kit-styling

- **medium** `ui-kit-styling-hover-paints-over-selection` — The watch tab strip applies its hover background unconditionally, so pointing at the already-selected watch tab repaints `row_selected` with `row_hover` and erases the selection the keyboard set.
  <br>`crates/fleet-app/src/views/watch_pane.rs:123` · CONFIRMED · trivial
- **medium** `ui-kit-styling-app-hand-assembled-elevated-surface` — The mention-picker popup is a fifth elevated surface hand-assembled inside `fleet-app`, and it uses a literal `border_1()` plus the wrong hairline role (`colors.border` instead of `colors.border_strong`), so its outline is nearly invisible against `colors.elevated` in dark mode.
  <br>`crates/fleet-app/src/screens/agent_thread/mod.rs:1297` · CONFIRMED · trivial
- **medium** `ui-kit-styling-composer-opacity-derived-by-arithmetic` — The dimmed composer expresses the spec's 60 % opacity as `1.0 - theme.metrics.dimmed_opacity`, coupling it to an unrelated token, while the identically specified 60 % elsewhere uses the named `refreshing_opacity`.
  <br>`crates/fleet-app/src/screens/agent_thread/mod.rs:1161` · PLAUSIBLE · trivial
- **low** `ui-kit-styling-sheet-has-no-radius` — `Sheet` is the only one of the three surfaces §2.5 assigns `radii.md` that applies no corner radius at all, so the design doc and the crate disagree about the sheet's corners.
  <br>`docs/DESIGN-SYSTEM.md:172` · CONFIRMED · trivial
- **low** `ui-kit-styling-metrics-doc-incomplete` — §2.8 presents itself as the full `Metrics` inventory and the full opacity ladder, but 19 of the 63 `Metrics` fields and 2 of the 11 named opacities appear nowhere in the design document.
  <br>`docs/DESIGN-SYSTEM.md:206` · CONFIRMED · small
- **low** `ui-kit-styling-dead-motion-tokens` — Three of the four animations §2.7 declares — the toast slide, the sheet slide and the value-swap highlight — have no implementation: their duration tokens are read nowhere in the workspace, so the document asserts behaviour the app never performs.
  <br>`crates/fleet-ui-kit/src/theme/tokens.rs:382` · CONFIRMED · small
- **low** `ui-kit-styling-inline-dialog-heights` — Three dialogs hard-code their height as an inline `px()` literal on the same builder call whose width correctly goes through the named-const `Dialogs::width(cx)` ladder, so half of each dialog's geometry is a token and half is an anonymous number.
  <br>`crates/fleet-app/src/dialogs/mod.rs:47` · CONFIRMED · small

### workspace-hygiene

- **high** `workspace-hygiene-daemon-id-expect-panics` — `fleetd` startup `expect()`s that the contents of the on-disk `$FLEET_HOME/daemon-id` file parse as a `HostId`, so any file the daemon did not write itself turns daemon startup into a panic instead of a handled error.
  <br>`crates/fleet-daemon/src/services/composition.rs:153` · CONFIRMED · small
- **medium** `workspace-hygiene-daemon-id-write-swallowed` — The daemon's persistent identity is written with `let _ = std::fs::write(...)`, so a failed write silently yields a brand-new identity on every restart while the doc comment promises a stable one.
  <br>`crates/fleet-daemon/src/services/mod.rs:161` · CONFIRMED · trivial
- **medium** `workspace-hygiene-daemon-ignores-rust-log` — `fleetd`'s subscriber is built with no `EnvFilter`, so it is hard-capped at INFO: `RUST_LOG` does nothing for the daemon and all 16 of its `tracing::debug!`/`trace!` sites can never emit — including the one whose comment says the redacted argv "stays in the debug log".
  <br>`crates/fleet-daemon/src/main.rs:57` · CONFIRMED · trivial
- **medium** `workspace-hygiene-rebase-abort-swallowed` — The reword transaction's rollback discards the result of `git rebase --abort`, so a failed abort leaves the repository stuck mid-rebase and the user sees only the original error, with nothing logged about the failed cleanup.
  <br>`crates/fleet-git/src/rebase.rs:117` · CONFIRMED · small
- **medium** `workspace-hygiene-no-layering-or-manifest-test` — The dependency direction and the manifest conventions the whole workspace relies on are stated only in prose; no test enforces them, so the first violating edge or version literal lands green.
  <br>`docs/ARCHITECTURE.md:52` · CONFIRMED · small
- **low** `workspace-hygiene-router-dead-code-suppression` — `Router::with_ids` opens with two no-op statements that exist only to reference two symbols so the dead-code lint stays quiet; one of them keeps a genuinely unused wrapper function alive and both mislead the next reader into thinking the constructor uses them.
  <br>`crates/fleet-daemon/src/services/router/mod.rs:87` · CONFIRMED · trivial
- **low** `workspace-hygiene-adr-0011-duplicate-number` — Two decision records claim number 0011 and one of them is absent from the index, so "ADR 0011" is ambiguous and the twelfth decision is invisible from docs/README.md.
  <br>`docs/decisions/0011-terminal-agent-attention.md:1` · CONFIRMED · trivial
- **low** `workspace-hygiene-make-run-doc-drift` — docs/DEVELOPMENT.md tells the reader that `make run` does not restart the daemon and that restarting is explicit, but the Makefile makes `run` depend on `restart`, so following the doc's PTY-preservation advice loses the sessions it promises to keep.
  <br>`docs/DEVELOPMENT.md:44` · CONFIRMED · trivial

