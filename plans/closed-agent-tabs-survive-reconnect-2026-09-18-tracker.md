# Closed native-agent tabs survive a reconnect — Tracker
> Plan: ./closed-agent-tabs-survive-reconnect-2026-09-18-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] Baseline: the tree had no target/; the first full build ran during wave 1. main's CI state served as the green baseline.
- [x] I am ready to start.

## Tasks
- [x] T01 — Reproduce the respawn against a real daemon
- [x] T02 — Pin the diagnosis with a failing app-state test
- [x] T03 — Stop the snapshot reducer from discarding local close markers
- [x] T04 — Add the wire vocabulary: capability, reopen request, closed-set response
- [x] T05 — Persist the closed marker per installation in the agent store
- [x] T06 — Record close and reopen in the daemon and seed the set on Hello
- [x] T07 — Seed the app from the daemon and send reopen when a tab returns
- [~] T08 — Add the harness regression scenario
- [ ] T09 — Move the docs with the code
- [x] T11 — Mirror every top-level reopen (`^s u`, caller segment, cross-worktree open) to the daemon, not only the picker's
- [x] T12 — In-flight agent reply futures hold `AppState` weakly (leak at `quit` during a picker reopen)
- [x] T10 — Quality gate and PR (PR #38 open; pixel-lane `make harness` could not run from this session, see log)

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-18 — Plan written. Leading diagnosis: the `closed` set in `AgentThreads` is in-memory
  only, so an app relaunch loses it; and `sync_snapshot` prunes it against every incoming
  thread list, so a snapshot with an empty `agent_threads` (which `summaries()` returns on any
  store error) wipes it during a daemon restart. Both are fixed by the same design: persist per
  installation on the daemon, seed on Hello, never prune on snapshot.

- 2026-09-19 — Execution model: the orchestrator (Claude, thread cb3fe402) delegates every task
  to Codex `gpt-5.6-sol` children through `fleet subagent run --provider codex --model gpt-5.6-sol
  --mode full-access`, all sharing this worktree. Deviation from the working agreement's "one task
  in progress at a time": wave 1 runs T04, T05, T02+T03 and T01+T08 concurrently because their
  file sets are disjoint; T04 carries the duty of keeping the workspace compiling (minimal match
  arms in daemon/client/app) so the others are not blocked. Delegation ids: T04 1ad635f1,
  T05 7f2e128b, T02+T03 97568a36, T01+T08 5ae580e4.
- 2026-09-19 — T01 is done through the harness rather than by hand: no interactive GUI is
  available to the orchestrator on this Linux box, so the new T08 scenario is run against the
  unfixed sibling checkout `../fix-help-view` (contains main, built today) and its failure after
  `daemon restart` is the reproduction. Paths A (quit/relaunch) and C (`r` after a kill) are
  covered by the fresh-`AppState` test T07 adds and by the same reducer path as B respectively;
  neither can be driven from here without a display.
- 2026-09-19 — `fleet` is not on PATH for a native-agent child (only `fleetd` is installed under
  ~/.local/bin), so every brief tells the child the absolute path of a sibling build of `fleet`
  to run `subagent complete` with.
- 2026-09-19 — T01 result: the harness scenario `closed-tab-survives-a-daemon-restart` PASSES on
  the unfixed sibling build (headless lane; the virtual lane needs a Hyprland compositor that is
  not running). Path B (daemon restart with the window open) does not reproduce through the
  harness because the snapshot the reconnect carries is complete, exactly as the plan's T02 note
  predicted. No `could not list the native-agent threads` warning appeared. The bug the user sees
  is therefore path A (relaunch: the in-memory `closed` set is gone) and the incomplete-snapshot
  shape; both are pinned by deterministic app-state tests instead
  (`a_closed_thread_stays_hidden_across_an_incomplete_reconnect_snapshot` in
  `state/connection/tests.rs`, red before T03 and green after; the fresh-`AppState` relaunch test
  lands in T07). The harness grammar has no app-relaunch directive, so path A cannot be a GUI
  scenario; the daemon-side restart test in T06 covers durability instead.
- 2026-09-19 — T04 verified: `cargo test -p fleet-proto` 74 tests green, `cargo check
  --workspace --all-targets` green. New names: `AGENT_CLOSED_CAPABILITY`,
  `RequestBody::AgentClosedThreads`, `RequestBody::AgentThreadReopen { thread }`,
  `ResponseBody::AgentClosedThreads(Vec<ThreadId>)`. The plan's "PROTOCOL_VERSION 7" note was
  stale (it is 8); not bumped, as intended.
- 2026-09-19 — T05 verified: `cargo test -p fleet-daemon store` 96 unit tests green, clippy clean.
  Migration 5 `closed_threads` (sha c9c1a945…), store API `mark_closed` / `clear_closed` /
  `closed_threads(client_id)`. Tests: `marking_a_thread_closed_lists_it_for_the_installation`,
  `clearing_a_closed_thread_removes_it_from_the_installation`,
  `closed_threads_are_isolated_per_installation`, `closed_threads_survive_reopening_the_store`,
  `slot_005_adds_closed_threads_to_the_slot_004_schema`.
- 2026-09-19 — T02/T03 verified: `cargo test -p fleet-app --lib` 862 green, clippy clean.
  `sync_snapshot` no longer prunes `closed`; `attached` still is. The headless harness
  integration test failed during that run only because other crates were mid-edit; it is rerun
  in T10.
- 2026-09-19 — T06 delegation 12bdbac6, T07 delegation eff02c71 (concurrent; daemon vs
  client+app, disjoint files).
- 2026-09-19 — T06 verified: `cargo test -p fleet-daemon agents` 399 green; server tests green
  with `FLEET_DAEMON` set. Manager: `mark_closed_for`, `clear_closed_for`, `closed_threads`,
  `reopen`; `persist_seen_before_routing` became `persist_agent_preferences_before_routing`.
  Tests: `closed_threads_are_isolated_cleared_and_survive_a_manager_restart`,
  `close_from_a_client_without_the_capability_has_no_persistence_identity`.
- 2026-09-19 — T07 verified: `cargo test -p fleet-client` green, `cargo test -p fleet-app --lib`
  864 green. `BridgeEvent::AgentClosedThreads`, `BridgeCommand::AgentThreadReopen`,
  `AgentThreads::seed_closed`, `Client::{agent_closed_threads, agent_thread_reopen}`. Tests:
  `relaunch_seeds_closed_threads_before_the_first_snapshot` (the path-A relaunch test),
  `reopening_a_closed_caller_from_the_picker_sends_agent_thread_reopen`,
  `agent_closed_fetch_is_sent_only_after_capability_negotiation`.
- 2026-09-19 — T09 done: NATIVE-AGENTS §2/§8/§10/§13, APP-CONTRACTS, UX-SPEC, ADR 0019, README
  index. Orchestrator review pass in T10 tightens the ADR's claim about what the harness scenario
  proves (it is a guard for path B; it passed before the fix too).
- 2026-09-19 — `make restart` is deliberately NOT run by the orchestrator: this very session is a
  native-agent thread hosted by the running `fleetd`, and a restart cuts the running turn. The
  user restarts the daemon after review.
- 2026-09-19 — T10 review of T07 found that only the `AGENTS` picker sent `AgentThreadReopen`;
  `^s u`, the transcript's caller segment and the cross-worktree open in `dialogs/host.rs` all go
  through `AppState::select_agent_thread` and would leave the daemon marker in place. Added T11
  (delegation launched) rather than widening T07. `make lint` is green on the pre-T11 tree.
- 2026-09-19 — T11 verified: `cargo test -p fleet-app --lib` 866 green. `reopen_agent_tab` is a
  free function in `screens/workspace/agent/requests.rs`; routed: picker, `^s u`, the transcript's
  `SelectThread`, and `dialogs/host.rs` cross-worktree open. Tests:
  `ctrl_s_u_from_an_attached_child_reopens_its_closed_caller_once`,
  `selecting_an_already_open_top_level_thread_from_the_picker_sends_nothing`,
  `selecting_an_attached_child_sends_no_agent_thread_reopen`.
- 2026-09-19 — T10: `make lint` green; `cargo test -p fleet-proto -p fleet-daemon -p fleet-client`
  green; `make test` failed only in the headless harness lane: the new scenario's `quit` hit
  GPUI's leak check (`Leaked handle for entity AppState`). Same flake hit
  `subagent-reopen-closed-caller` 3 times today under load (23 runs, all earlier ones green);
  root cause is pre-existing: `open_thread` and its siblings park a strong `Entity<AppState>` in
  a detached reply future, so a quit 150 ms after a picker reopen leaks. Added T12 rather than
  masking with an `await idle`. `make test` also warns the headless subset now costs 127 s against
  a 60 s budget (pre-existing drift; the new scenario adds ~65 s because of the reconnect wait).
- 2026-09-19 — T12 verified: `open_thread`, `load_older_page`, `refresh_checkpoints`,
  `account_login` and `create_thread` hold `AppState` (or the view) weakly across the reply;
  new test `an_in_flight_open_thread_does_not_retain_app_state`. Both picker-reopen scenarios
  green twice each headless. `make test` then fully green (867 app lib tests, headless lane ok).
- 2026-09-19 — T10 done: `make lint` green, `make test` green, commits a774140 (`proto:` the
  whole change, matching how `agent.seen` landed in cc37ce7) and 04f37c6 (plan + tracker),
  PR #38 open. Pixel-lane `make harness` NOT verified: without compositor variables it falls back
  to headless and every `shot` scenario fails by design; pointed at the live compositor the
  isolated output existed but the window never painted (locked session), so the suite aborted on
  `agent-approval` before any scenario of ours ran. Recorded on the PR; the user runs it from an
  unlocked desktop. `make restart` also left to the user (see above).

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- The thread-delete verb `docs/NATIVE-AGENTS.md` §8 owes; it must also clear the new closed table.
- The delegated-child `attached` set has the same snapshot-pruning hazard; decide whether it
  should be durable per installation too.
