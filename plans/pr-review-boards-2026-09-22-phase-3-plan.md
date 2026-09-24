# PR review boards, phase 3: scheduled agent tasks — Plan
> Tracker: ./pr-review-boards-2026-09-22-phase-3-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.
> For this initiative the tracker's truth is the worktree board `dannyfuf/fleetd#feat-board-workflow-schedule-custom-task` (prefix FEA).

## Summary

A **schedule** is a prompt that belongs to one board. The daemon runs it with a coding agent every N
minutes, or once at a set time. The agent runs headless: `claude -p` or `codex exec`, in a
per-schedule scratch directory, with the user's normal configuration. That means every MCP server,
skill and CLI tool the user has installed is available with no setup in Fleet. The daemon adds a
fixed footer telling the agent how to record what it found on the board
(`fleet board --board <id> card new … --pr …`, idempotent since phase 2). Every run is a daemon
**job**: it is visible in the jobs panel, cancellable, and has a log file, a cost and a one-line
summary. All shapes are in `./pr-review-boards-2026-09-22-contracts.md` C6 to C8.

## Sizing call

Phased: phase 3 of 4. It adds a new core module, a new store, a new daemon service with one loop, a
headless runner, one wire family and one CLI command group. That is one feature and one pull
request, but the largest phase, so its cards are small.

## Repository context

- **Periodic loops:**
  - `crates/fleet-daemon/src/services/maintenance.rs`: `start_periodic_tasks` around 78-111, which spawns about ten loops into `PeriodicTasks`, each taking a `CancellationToken`. `run_pr_cache_expiry` around 497 is a small `sleep_until` loop and the best template. `RepeatedFailure` around 187 rate-limits a warning that keeps recurring.
  - `crates/fleet-daemon/src/main.rs` around 134-166 creates the shutdown token and joins the loops (2 s timeout each).
- **Clock:** `crates/fleet-daemon/src/adapters/clock.rs` (`Clock::now() -> DateTime<Utc>`, injectable for tests). Timers use `tokio::time` and the wall clock is read through `Clock`. `chrono` is already a workspace dependency, and there is no cron crate (none is needed).
- **Stores:** `crates/fleet-daemon/src/stores/{board.rs,config.rs,state.rs}`. `state.rs` is the template: versioned JSON, `Files::atomic_write_text`, quarantine of an unreadable file to `*.broken-<epoch>`. Paths are on `FleetHome` in `crates/fleet-core/src/paths.rs`.
- **Jobs:** `crates/fleet-daemon/src/jobs/manager.rs`, where `submit(kind, target, title, cancellable, retryable, operation)` (around 193) returns a `JobId` and runs `operation(JobCtx)`. `JobKind` is in `crates/fleet-proto/src/job.rs`.
- **Subprocesses:** `crates/fleet-daemon/src/adapters/shell.rs`. `Shell::run_streaming(command, cancel, on_line)` runs a child, streams its lines and honours cancellation; `ShellCommand { program, args, cwd, env, timeout }`. Tests use a fake `Shell`; grep `impl Shell for` under `crates/fleet-daemon` for it.
- **The agent's environment and binaries:**
  - `crates/fleet-daemon/src/agents/harness/process.rs::login_environment(cwd)` (around 195) gives the user's login-shell environment.
  - `crates/fleet-daemon/src/services/agents/delegation/run.rs::resolve_fleet_program` (around 86-108) finds the `fleet` binary next to `fleetd` for the footer and `PATH`.
  - Provider binaries come from `Config.agent_binaries` (`crates/fleet-core/src/config.rs` around 188).
- **Headless CLIs, as verified on this machine:**
  - Claude: `claude -p --output-format stream-json --verbose --permission-mode <mode> [--model M] [--effort E] "<prompt>"`. The last stream line is a `{"type":"result", …}` object carrying `result`, `is_error` and `total_cost_usd`.
  - Codex: `codex exec --json --skip-git-repo-check [--dangerously-bypass-approvals-and-sandbox | -s <sandbox>] [-m M] [-c model_reasoning_effort="E"] -o <last-message-file> "<prompt>"`.
  - The implementer must re-check both `--help` outputs before coding, because flags drift between releases.
- **Wire and client:** as in phase 2, plus the event enum `crates/fleet-proto/src/event.rs` (`EventKind` around 20, `Event` around 74). The router forwards some events (`crates/fleet-daemon/src/services/router/mod.rs` around 508); schedules are local-only and are not forwarded.
- **Verification:** `make lint`, `make test`, `make restart` before manual runs.
- **Skills to load first:**
  - `rust-async-background-work` (the loop, the runner);
  - `rust-ipc-protocol` (wire);
  - `rust-workspace-architecture` (the new module, store and ADR);
  - `rust-gpui-testing` (tests with a paused clock);
  - `zed-quality-review` before done.

## Assumptions

- Headless runs are enough for fetching. A scheduled run needs no live transcript tab, only a log and a summary.
- The user's `claude` / `codex` installs are logged in. A run that fails because they are not is a `Failed` run with the CLI's error in its summary.
- One scheduler per daemon. Schedules never run on a remote host.

## Out of scope

- Cron expressions, time zones and active hours: interval and one-shot only (follow-up).
- Schedules not attached to a board (follow-up).
- Any app surface (phase 4).

## Affected areas

- `crates/fleet-core/src/{schedule.rs (new),ids.rs,paths.rs,lib.rs}`
- `crates/fleet-daemon/src/stores/schedules.rs` (new), `stores/mod.rs`
- `crates/fleet-daemon/src/services/schedules/{mod.rs,runner.rs,tick.rs}` (new), `services/composition.rs`, `services/maintenance.rs`, `services/dispatch.rs`, `services/boards/lifecycle.rs` (board delete cascade), `server/connection.rs`
- `crates/fleet-proto/src/{request.rs,response.rs,event.rs,job.rs}`, `crates/fleet-proto/tests/compatibility.rs`
- `crates/fleet-client/src/connection.rs` and a schedules methods file
- `crates/fleet-cli/src/{args.rs,commands/schedules.rs (new),commands.rs,envelope.rs,human.rs}`
- `docs/ARCHITECTURE.md`, `docs/BOARD.md` (new §12), `docs/decisions/0024-scheduled-agent-tasks.md`, `docs/README.md`, `.claude/skills/fleet-board-planning/SKILL.md`

## Tasks

### P3-T01 — Add the schedule model, its validation and its fire-time rule to fleet-core
- **Intent:** `fleet_core::schedule` holds every type, limit, sentence, template and the pure `next_run_at` rule of contracts C7, fully unit-tested.
- **Touches:** `crates/fleet-core/src/schedule.rs` (new), `crates/fleet-core/src/lib.rs`, `crates/fleet-core/src/ids.rs`, `crates/fleet-core/src/paths.rs`, `crates/fleet-core/src/schedule/tests.rs` (new), `docs/decisions/0024-scheduled-agent-tasks.md` (new), `docs/README.md`, `docs/BOARD.md` (new §12 "Schedules").
- **Steps:**
  - Load `rust-workspace-architecture` and read how `fleet_core::board` is laid out: a module file, submodules, a `tests.rs`. Copy that layout.
  - In `ids.rs`, add `string_id!(ScheduleId, "schedule", validate_slug);` and `pub fn new_schedule_id() -> ScheduleId`, producing `sch-` + 8 lowercase hex characters from a UUID. Look at how `new_card_id` is generated and mirror it.
  - In `paths.rs`, add `schedules_path()` (`<home>/schedules.json`), `schedule_dir(id)` (`<home>/schedules/<id>`), `schedule_work_dir(id)` (`…/work`) and `schedule_log_path(id, started_at)` (`…/logs/<YYYYMMDDTHHMMSSZ>.log`). Copy the doc-comment style of `config_path`.
  - In `schedule.rs`, add every C7 type with the stated serde attributes. Each field gets a one-line `///` doc.
    - `ScheduleAgent` implements `Default` by hand: Claude, `None`, `None`, `FullAccess`.
    - `AgentKind` and `PermissionMode` come from `fleet_core::agents`.
  - Add `ScheduleError` using `thiserror`, the way `BoardError` does. It needs `Invalid { field: String, reason: String }` and `NotFound(String)`.
  - `pub fn validate_schedule(schedule: &Schedule) -> Result<(), ScheduleError>`: every C7 refusal except `board_id`, which the daemon checks. For `mode`, use `AgentKind`'s supported-modes helper (research: `crates/fleet-core/src/agents/state.rs` around 24-41) and print the mode and provider with their existing `Display` / `executable()` spellings.
  - `pub fn apply_draft(draft: ScheduleDraft, id: ScheduleId, now: &str) -> Schedule` and `pub fn apply_patch(schedule: &mut Schedule, patch: SchedulePatch, now: &str)`. Defaults: `enabled = true`, `timeout_minutes = SCHEDULE_DEFAULT_TIMEOUT_MINUTES`, `agent = ScheduleAgent::default()`.
  - `pub fn next_run_at(schedule, now) -> Option<DateTime<Utc>>` exactly as C7 says. Parse stored RFC 3339 strings with `DateTime::parse_from_rfc3339`; treat an unparsable stored time as "never ran".
  - `pub fn push_run(schedule: &mut Schedule, run: ScheduleRun)`: append, then drop the oldest past `MAX_RUNS_PER_SCHEDULE`. Return the dropped runs' `log_path`s so the daemon can delete those files.
  - `pub fn render_prompt(schedule: &Schedule, fleet: &str, now: &str) -> String`:
    - substitute `{board}`, `{last_run_at}` and `{now}` in the user's prompt;
    - append two newlines and `SCHEDULE_FOOTER_TEMPLATE` with `{name}`, `{board}`, `{fleet}` and `{last_run_at}` substituted.
    - Put the footer text in the source exactly as C7 prints it, with the same line breaks.
  - `pub fn summary_line(final_message: &str) -> Option<String>`: the last line that starts with `SUMMARY:`, prefix and whitespace trimmed, cut to `SCHEDULE_SUMMARY_MAX_CHARS` at a char boundary.
  - Add `STARTER_PROMPT_GITHUB_REVIEWS` exactly as in C7.
  - Tests in `schedule/tests.rs`:
    - one per C7 refusal;
    - `next_run_at`:
      - a never-run every-schedule is due now;
      - a run 10 minutes ago on `every 15` is due in 5;
      - a daemon that was down for an hour catches up once (due now, not four times);
      - once-in-the-future;
      - once-already-run is `None`;
      - disabled is `None`;
    - `push_run` caps at 20 and returns the dropped log paths;
    - `render_prompt` substitutes every placeholder and ends with the footer;
    - `summary_line`: finds the last one, handles none, and cuts a long one at a UTF-8 boundary (use a string with multibyte characters);
    - the draft defaults.
  - Write ADR 0024, "Scheduled agent tasks run headless as daemon jobs", in the house shape. Decisions:
    - a schedule is board-owned (its output is cards, and its settings live with the board);
    - it runs headless as a job, not as a native agent thread. Reasons: a thread needs a worktree and a strip tab, and nobody steers a fetch. The job gives cancel, log, retry and the jobs panel for free;
    - interval or once, no cron in v1;
    - one catch-up run after downtime, never a burst;
    - an overlapping fire is recorded as `Skipped`;
    - the default mode is full access, and why;
    - the footer contract.

    Add the row to `docs/README.md`.
  - In BOARD.md, add "§12 Schedules": the model fence copied from C7, the rules, and a pointer to ADR 0024. Later cards add the daemon and CLI parts.
- **Verification:** `cargo test -p fleet-core schedule`, `make lint`.
- **Done when:** the module compiles, every listed test passes, and ADR 0024 is indexed.

### P3-T02 — Persist schedules in a versioned daemon store
- **Intent:** `ScheduleStore` loads and saves `schedules.json` atomically, validates every schedule on load and save, and quarantines an unreadable file instead of crashing the daemon.
- **Touches:** `crates/fleet-daemon/src/stores/schedules.rs` (new), `crates/fleet-daemon/src/stores/mod.rs`, the store's tests (inline `mod tests` or `stores/schedules/tests.rs`, matching `state.rs`), `docs/BOARD.md` §12.
- **Steps:**
  - Read `stores/state.rs` fully and copy its structure: `Arc<dyn Files>`, a `tokio::sync::Mutex` gate, `spawn_blocking` for the file IO, `atomic_write_text`, and the quarantine path.
  - API:
    - `load() -> DaemonResult<SchedulesDocument>`: a missing file is an empty document at version 1;
    - `transaction<F>(f: F) -> DaemonResult<R>` where `F: FnOnce(&mut SchedulesDocument) -> DaemonResult<R>`: load, apply, validate every schedule, save, all under the gate.
  - Version: refuse a document whose version is not `SCHEDULES_DOCUMENT_VERSION`, with the same `Unsupported` style `stores/board.rs` uses for boards. Refuse, do not quarantine: a newer daemon's file must survive an older daemon.
  - An unparsable or invalid document is quarantined to `schedules.json.broken-<epoch>` and treated as empty, with one `tracing::warn!` carrying the path. Copy the state store's log line form.
  - Tests with the in-memory `Files` fake:
    - missing file is empty;
    - round trip;
    - an invalid schedule refuses the save and leaves the file unchanged;
    - a corrupt file is quarantined;
    - a future version is refused and left in place.
  - Docs: BOARD.md §12, a "Storage" paragraph (path, version, quarantine).
- **Verification:** `cargo test -p fleet-daemon schedules`, `make lint`.
- **Done when:** the five tests pass.

### P3-T03 — Build the headless runner for Claude and Codex
- **Intent:** `ScheduleRunner::run(schedule, prompt, cancel) -> RunResult` launches the provider CLI headless in the schedule's work directory, streams its output to the run's log file, enforces the timeout, and returns the outcome, the summary line and the cost.
- **Touches:** `crates/fleet-daemon/src/services/schedules/runner.rs` (new), `services/schedules/mod.rs` (new, `mod runner;`), runner tests, `docs/BOARD.md` §12.
- **Steps:**
  - Load `rust-async-background-work`. The runner is `async`, uses the `Shell` adapter (never `std::process` directly) so tests can fake it, and takes a `CancellationToken`.
  - `fn claude_argv(agent: &ScheduleAgent, prompt: &str) -> Vec<String>` →
    `["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", <wire>, ("--model", M)?, ("--effort", E)?, prompt]`.
    - For `<wire>`, reuse the Claude adapter's `permission_mode_to_wire` (`crates/fleet-daemon/src/agents/claude/argv.rs` around 43). Make it `pub(crate)` if needed; do not copy the table.
    - Put the prompt last, as one argument. It can contain anything; the shell adapter passes argv without a shell.
  - `fn codex_argv(agent, prompt, last_message: &Path) -> Vec<String>` →
    `["exec", "--json", "--skip-git-repo-check", <mode flags>, ("-m", M)?, ("-c", "model_reasoning_effort=\"E\"")?, "-o", <last_message>, prompt]`.
    - Mode flags: `FullAccess` → `--dangerously-bypass-approvals-and-sandbox`; `AcceptEdits` → `-s workspace-write`; `Ask` / `Plan` → `-s read-only`.
    - Codex has no prompt channel in exec mode, so `Ask` cannot prompt anyone. Document that on the function.
    - Re-verify each flag with `codex exec --help` before writing it.
  - Program: `Config.agent_binaries` for the provider (look at how the native harness picks the binary; grep `agent_binaries` in `crates/fleet-daemon/src/services/agents/providers`) or the bare `claude` / `codex`.
  - `ShellCommand`:
    - `cwd = schedule_work_dir(id)` (create it with `create_dir_all`);
    - `env = login_environment(cwd)` plus `PATH` with `resolve_fleet_program`'s directory prepended, `FLEET_BOARD=<board>` and `FLEET_SCHEDULE=<id>`;
    - strip the same variables the Claude and Codex adapters strip (`agents/harness/probe.rs` around 240);
    - `timeout = timeout_minutes`.
  - Streaming: open the log file (`schedule_log_path`) and append each line from `on_line`. Keep the last Claude `{"type":"result"}` object: parse each line with `serde_json::from_str::<serde_json::Value>` and ignore lines that are not JSON. For Codex, read the `-o` file after exit.
  - Result mapping:
    - exit 0 and the result not `is_error` → `Succeeded`;
    - a timeout → `TimedOut`;
    - cancelled → `Failed` with summary `canceled`;
    - anything else → `Failed`.
    - The summary is `summary_line(final_message)`, else for failures the last non-empty stderr/stdout line cut to 280 chars.
    - The cost is `total_cost_usd` for Claude and `None` for Codex (no cost in exec JSON).
  - Tests with the fake `Shell`:
    - the argv for each provider, mode, model and effort combination (table test);
    - the prompt is the last argument;
    - a Claude stream with a result line yields `Succeeded`, the summary and the cost;
    - `is_error: true` yields `Failed`;
    - a timeout yields `TimedOut`;
    - cancel yields `Failed`/`canceled`;
    - the log file receives every line;
    - `PATH` starts with the fleet directory, and `FLEET_BOARD` / `FLEET_SCHEDULE` are set.
  - Docs: BOARD.md §12, "How a schedule runs": both argv shapes, the environment, the log, and the outcome table.
- **Verification:** `cargo test -p fleet-daemon runner`, `make lint`.
- **Done when:** the tests pass, and a manual `claude -p` launch with the built argv (print it in a test with `--nocapture` and paste it into a shell) works on this machine.

### P3-T04 — Fire due schedules from a daemon loop and record every run
- **Intent:** a `Schedules` service owns the store and the runner. A loop in `start_periodic_tasks` sleeps until the next due schedule, submits one `ScheduledTask` job per due schedule, skips a schedule whose previous run is still live, records each run, and publishes `SchedulesChanged`.
- **Touches:** `crates/fleet-daemon/src/services/schedules/{mod.rs,tick.rs}`, `services/composition.rs`, `services/maintenance.rs`, `services/boards/lifecycle.rs` (board delete cascade), `crates/fleet-proto/src/{job.rs,event.rs}` (the `ScheduledTask` job kind and the `SchedulesChanged` event only; requests are P3-T05), tests, `docs/ARCHITECTURE.md`, `docs/BOARD.md` §12.
- **Steps:**
  - Add `JobKind::ScheduledTask` with a doc comment. Add `EventKind::SchedulesChanged` and `Event::SchedulesChanged { board_id }`. Find every exhaustive match on `JobKind` / `EventKind` with `cargo build --workspace` and handle the new arm: the app's job table slug is `sched` (7 characters or fewer, UX-SPEC §6 job table), and the router does not forward the event.
  - `Schedules` service (`mod.rs`):
    - holds `store`, `runner`, `jobs`, `clock`, `events`, the boards service (to check a board exists), and a `live: Mutex<BTreeMap<ScheduleId, JobId>>`;
    - add a `tokio::sync::Notify` named `changed` that every mutation calls, so the loop re-plans at once.
  - Methods used by P3-T05: `list(board: Option<&BoardId>)`, `create(draft)`, `update(id, patch)`, `delete(id)`, `run_now(id)`.
    - Each mutation goes through `store.transaction` and recomputes `next_run_at` with `fleet_core::schedule::next_run_at(…, clock.now())`.
    - `create` refuses an unknown board with the C7 `board_id` sentence.
    - Each mutation publishes `SchedulesChanged` through the bus, as `Boards::changed` does (`services/boards.rs` around 187), and wakes `changed`.
  - `fire(id)` (`tick.rs`):
    - If `live` holds the schedule, push a `Skipped` run (summary `the previous run was still going`) and return.
    - Otherwise, render the prompt; the fleet path comes from `resolve_fleet_program`.
    - Record a `ScheduleRun` with `started_at` now, `log_path` and no outcome, and save.
    - Submit a job: kind `ScheduledTask`, target the schedule id, title `Scheduled: {name}`, `cancellable = true`, `retryable = false`. The operation calls the runner with the job's cancellation token (see `JobCtx`), then writes the outcome, summary, cost and `ended_at` onto that same run (matched by `started_at`), deletes the log files `push_run` dropped, removes itself from `live`, recomputes `next_run_at`, saves and publishes.
    - A job that returns an error still records a `Failed` run.
  - Loop (`tick.rs::run_schedules(self, shutdown: CancellationToken)`), modelled on `run_pr_cache_expiry`:
    - load, find the earliest `next_run_at`, and `select!` on `shutdown.cancelled()`, `changed.notified()` and `sleep_until(min(earliest, now + 60 s))`;
    - on wake, fire every schedule whose `next_run_at <= clock.now()`;
    - the 60 s cap means a wall-clock jump or sleep is noticed within a minute;
    - use `RepeatedFailure` for a store that keeps failing.
  - Register the loop in `start_periodic_tasks` and construct the service in `composition.rs`, next to `Boards`.
  - Startup: a run recorded with no outcome belongs to a daemon that died mid-run. On service start, mark each such run `Failed` with summary `the daemon stopped during this run`.
  - Board delete cascade: when a board is deleted (`Boards::delete`, and `delete_for_context` through it), delete that board's schedules and their directories. Call from the dispatch site that deletes boards, so `Boards` does not depend on `Schedules`. Check `dispatch.rs` around 908 for the context cascade and add the call for both paths.
  - Tests, with `tokio::time::pause()` and a fake clock and runner (see `rust-gpui-testing` for tokio paused-clock tests):
    - a due schedule fires once;
    - a not-due schedule does not;
    - an overlapping fire is `Skipped`;
    - a disabled schedule never fires;
    - a once-schedule fires once and then has no `next_run_at`;
    - an update wakes the loop (fires without waiting 60 s);
    - an interrupted run is marked `Failed` on start;
    - deleting a board deletes its schedules;
    - shutdown stops the loop within the join timeout.
  - Docs:
    - `docs/ARCHITECTURE.md`: add the schedules loop to the list of periodic sweeps (around line 98) and the service to the services list;
    - BOARD.md §12: the firing rules.
- **Verification:** `cargo test -p fleet-daemon schedules`, `make lint`, `make test`.
- **Done when:** every test passes and `make restart` starts a daemon that logs no schedule warning with an empty store.

### P3-T05 — Put schedules on the wire and in the client
- **Intent:** the five schedule requests, the two responses and the `schedules` capability exist with goldens, dispatch to the `Schedules` service, and are gated in the client.
- **Touches:** `crates/fleet-proto/src/{request.rs,response.rs}`, `crates/fleet-proto/tests/compatibility.rs`, `crates/fleet-daemon/src/services/dispatch.rs`, `crates/fleet-daemon/src/server/connection.rs` (Hello and event forwarding to clients), `crates/fleet-daemon/src/server/connection/events.rs`, `crates/fleet-client/src/connection.rs` and a new client methods file (mirror the board methods), `docs/BOARD.md` §12.
- **Steps:**
  - Add `SCHEDULES_CAPABILITY`, the five request variants and `ResponseBody::Schedules` / `ResponseBody::Schedule` exactly as C6. For the delete answer, use whatever `DeleteBoard` answers today.
  - Goldens: one per request and one per response, hand-written, with `"type":"create_schedule"` and camelCase fields. Include a cadence of each kind.
  - Dispatch each request to the service from P3-T04.
  - Map `SchedulesChanged` to its `EventKind` in `server/connection/events.rs` so subscribed clients receive it. Follow how `BoardChanged` is mapped around line 75.
  - Advertise `SCHEDULES_CAPABILITY` in the Hello list.
  - Client: `list_schedules`, `create_schedule`, `update_schedule`, `delete_schedule` and `run_schedule_now`, each gated on the capability in `required_capability`.
  - Tests: the goldens; a dispatch test per request against a daemon test fixture; a client test that an old daemon's missing capability gives the restart error.
  - Docs: the request table in BOARD.md §12, in the same shape as §5.
- **Verification:** `cargo test -p fleet-proto`, `cargo test -p fleet-daemon dispatch`, `cargo test -p fleet-client`, `make lint`.
- **Done when:** the goldens and the dispatch tests pass.

### P3-T06 — Add the `fleet schedule` command group
- **Intent:** a user or agent lists, creates, edits, deletes, runs and inspects schedules from a terminal, as contracts C8 describes, with human and `--json` output.
- **Touches:** `crates/fleet-cli/src/args.rs`, `crates/fleet-cli/src/commands.rs`, `crates/fleet-cli/src/commands/schedules.rs` (new), `crates/fleet-cli/src/envelope.rs`, `crates/fleet-cli/src/human.rs`, CLI tests, `docs/BOARD.md` §12.
- **Steps:**
  - Model the args on `BoardArgs` + `BoardCommand`. Reuse the board selector flags by extracting the selector group into a shared struct if that is clean; otherwise duplicate the four flags and share only the resolution function. Board resolution must be the same code path as `fleet board`.
  - Verbs and flags exactly as C8.
    - `--prompt`, `--prompt-file` and `--starter github-reviews` are mutually exclusive, and one is required on `new`. `--starter github-reviews` uses `STARTER_PROMPT_GITHUB_REVIEWS`.
    - `--every` and `--once` are mutually exclusive, and one is required on `new`.
    - `edit` takes every field as optional and refuses an empty patch, exactly as `card edit` does (`nothing to change`; copy that sentence).
  - Human output:
    - `list`: one row per schedule, `id  name  cadence  next  last outcome  last summary`, with `every 15m` / `once 2026-09-23 09:00` cadence cells, `—` for empty cells, and `human::columns` for padding;
    - `show`: fact lines plus the last five runs;
    - `runs`: every run with its log path, so a user can `less` it.
  - JSON: `ScheduleEnvelope { protocol: 1, schedule }` and `SchedulesEnvelope { protocol: 1, schedules }`.
  - `run <id>` prints `Started {job id}` and returns immediately. Add `--wait`, which waits for the job with the existing job-wait client call if there is one (grep `wait_job` / `JobWait`), and otherwise polls `show` once a second until the run has an outcome. Document which it is.
  - Tests: argument conflicts; the empty-patch refusal; the human rows for each cadence; the envelopes.
  - Docs: the CLI fence in BOARD.md §12, with one worked example of creating the GitHub review schedule on a Reviews board.
- **Verification:** `cargo test -p fleet-cli`, `make lint`, `make restart`, then:
  ```
  fleet schedule --reviews new --name "GitHub reviews" --starter github-reviews --every 15
  fleet schedule --reviews run <id> --wait
  fleet board --reviews show
  ```
  Cards appear for the PRs you have been asked to review.
- **Done when:** the tests pass and the manual run creates real cards.

### P3-T07 — Teach agents to use schedules and pin the flow with a scripted smoke
- **Intent:** the planning skill documents schedules, and `make smoke-reviews` also proves that a schedule run creates a card through the footer contract.
- **Touches:** `.claude/skills/fleet-board-planning/SKILL.md`, `.claude/skills/fleet-board-planning/references/commands.md`, `scripts/reviews-smoke.sh`, `docs/DEVELOPMENT.md`.
- **Steps:**
  - Extend `scripts/reviews-smoke.sh`:
    - install a fake `claude` script on `PATH` that ignores its arguments, runs `fleet board --board "$FLEET_BOARD" card new "Scheduled PR" --pr <owner>/<name>#2 --label github`, then prints a Claude-shaped `{"type":"result","result":"SUMMARY: 1 created, 0 existing, 0 reopened","is_error":false,"total_cost_usd":0}` line;
    - create a schedule with `--every 5`, `fleet schedule run --wait` it, and assert the card exists, the run `Succeeded`, and its summary is the fake's line.
  - In the skill:
    - add a "Scheduling" section: when a schedule is the right tool (recurring intake from a source Fleet cannot see), `fleet schedule new`, the footer contract, the 5-minute minimum, and that runs are jobs with logs;
    - add the verbs to `references/commands.md`.
- **Verification:** `make smoke-reviews`, `make lint`.
- **Done when:** the smoke passes with the schedule step.

## Verification

- `make lint`
- `make test`
- `make smoke-reviews`
- `make restart`, then the manual run in P3-T06.

## Definition of done

- [ ] Every phase-3 card is in Done on the FEA board.
- [ ] `make lint` clean and `make test` passing.
- [ ] The smoke passes, including the schedule step.
- [ ] ADR 0024, `docs/BOARD.md` §12 and `docs/ARCHITECTURE.md` describe what the code does.
- [ ] `schedules` is advertised only by the build that serves every request.
- [ ] Follow-ups (cron / active hours, board-less schedules) captured as Backlog cards.

## Risks and rollback

- **Runaway cost:** mitigated by the 5-minute minimum, skip-on-overlap, the timeout, and cost recorded per run. Disable a schedule with `fleet schedule edit <id> --disabled`.
- **The headless CLI's flags change:** the argv builders are unit-tested and verified against `--help`; a failure shows up as a `Failed` run with the CLI's own message.
- **Rollback:** reverting P3-T05 un-advertises the feature. The store file is inert without it.
