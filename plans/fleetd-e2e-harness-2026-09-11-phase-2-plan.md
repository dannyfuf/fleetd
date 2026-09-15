# Fleet e2e harness — Phase 2: assertions and synchronisation — Plan
> Tracker: ./fleetd-e2e-harness-2026-09-11-phase-2-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

After Phase 1 the harness can drive the real app and photograph it, but it still cannot tell you
what happened. The only way to check a result is to read a PNG, which is slow, unreliable for text,
and impossible to assert on automatically; and the only way to wait for something is `wait 500`,
which is flaky by construction.

This phase gives the app a voice. `fleet` learns to serialise a versioned `UiSnapshot` — which
screen, which mode and key-context stack, what has focus, which list rows exist and which one is
selected, what the dialogs and toasts say, what the terminal grid contains — and to answer three
new commands over the Phase 1 socket: `dump` (give me the snapshot), `await` (reply when this
predicate holds, or time out), and `assert` (fail the run if it does not hold now). After this
phase, screenshots are evidence for a human to look at; the snapshot is the oracle.

## Sizing call

**Phased**, phase 2 of 5 — see [the roadmap](./fleetd-e2e-harness-2026-09-11-roadmap.md). A week of
focused work: a new serialisable projection of `AppState` that must obey the repo's
"render prepares nothing" contract, a small predicate evaluator, condition-waiting plumbed through
the window, and the runner-side reporting. Not split further because `await` without a snapshot is
meaningless and a snapshot nobody waits on is a debugging toy.

## Repository context

- **Project type / commands:** as Phase 1 — Rust workspace, `make lint`, `make test`, `make restart`
  after daemon changes.
- **State lives in one place.** `crates/fleet-app/src/state.rs` plus `state/{navigation, snapshot,
  agents, board, connection, notifications, terminal}` hold the daemon mirror and all local
  interaction state, reduced without foreground I/O. `Mode`, `Screen`, `HubPane`, `HubTab`,
  `Overlay`, `Cursors`, `FilterState`, `AgentPopupState`, `StickyError`, `LiveToast` and `MirrorGrid`
  are already public types on that module — the snapshot is a projection of them, not new state.
- **The rule that governs this phase:** `docs/APP-CONTRACTS.md` and `CLAUDE.md` — "Render prepares
  nothing: no IO, no requests, no `cx.notify`, no heavy CPU inside `render`. Precompute in update
  paths and memoise per revision." The snapshot must therefore be built where state changes, not
  where it is drawn.
- **Key contexts are authoritative in `docs/KEYMAP.md`**, which lists the gpui key contexts
  (`Hub > Repos`, `Workspace > Prefix`, `Agent > AgentDecision > *`, `Dialog > <name>`, …). The
  snapshot must report the live context stack, because "which key works right now" is exactly what a
  keymap test needs to know.
- **Terminal mirror:** `state::MirrorGrid` holds the cells the app draws; `fleet-proto`'s `Cell`,
  `CursorState`, `ViewportInfo` describe them. Extracting text lines is a projection, not a new
  pipeline.
- **Existing tests to imitate:** `crates/fleet-app/tests/board_flow.rs` and `jobs_panel.rs` drive the
  real daemon; `crates/fleet-app/src/state/snapshot/tests.rs` covers the daemon-snapshot reducers.

## Assumptions

- The snapshot is for tests and for an agent reading a run; it is not a public API and not a
  persisted format. It carries a `version` integer so a mismatch between runner and app is an
  explicit error rather than a confusing failure, but it may change freely between versions.
- Predicates are small on purpose. A scenario asks "is the palette open?", not "run this query
  language". If a predicate needs a new operator, adding one is cheap; adding a DSL is not.
- Values the snapshot reports are what the user can see or act on. It does not expose internal
  entity ids except where a test needs a stable handle (list row ids, tab ids, thread ids).

## Out of scope

- Mouse targets (`targets` map) — Phase 3 adds them to this snapshot and bumps its version.
- Scripted agents and fault injection — Phase 4.
- Golden images and the corpus — Phase 5.
- Exposing the snapshot outside the harness (no `fleet dump-state` CLI command, no daemon RPC). The
  socket is the only consumer.

## Affected areas

- `crates/fleet-app/src/state.rs` and `state/` — the snapshot projection and its memoisation.
- `crates/fleet-app/src/state/harness.rs` (new) — `UiSnapshot` and its builder.
- `crates/fleet-app/src/drive.rs` — the three new commands and the waiter.
- `crates/fleet-drive/` — envelope additions, predicate parsing shared with the runner.
- `crates/fleet-harness/src/` — `await`/`assert` scenario lines, failure reporting, `report.md`.
- `docs/TESTING-HARNESS.md`, `docs/APP-CONTRACTS.md`.

## Tasks

### P2-T01 — Define `UiSnapshot`
- **Intent:** one serialisable answer to "what is on screen and what can I do to it?".
- **Touches:** `crates/fleet-app/src/state/harness.rs` (new), `crates/fleet-app/src/state.rs`.
- **Steps:**
  - Load `gpui-state-and-memory` before touching `AppState`.
  - Define `UiSnapshot` with `serde::Serialize` and a `version` field. Contents:
    - `screen`, `mode`, `hub_pane`, `hub_tab`, `overlay`, `key_contexts` (the live stack, as
      `docs/KEYMAP.md` names them), `focused` (a stable name for the focus owner).
    - `lists`: for each visible list, `{ name, rows: [{ id, label, badges, marks }], selected,
      filter }` — repos rail, worktrees, PRs, board columns, jobs, tabs.
    - `dialog`: `{ name, fields: [{ name, value, focused }], buttons, message }` when one is open.
    - `toasts`: `[{ level, text, count }]`; `sticky_error`: the failed job and its retry affordance.
    - `jobs`: running and recently finished, by id and status.
    - `agents`: popup state, open threads, per-thread `{ provider, state, unread, pending_gate }`.
    - `terminal`: from P2-T06.
    - `window`: logical bounds, scale factor, title, frame number.
  - Every enum serialises to the vocabulary the docs use, so a scenario reads like the keymap.
- **Verification:** `make lint`; `make test`; a unit test asserts the serialised form of a
  hand-built state, so the shape is pinned and a rename is a visible diff.
- **Done when:** `UiSnapshot` exists, serialises deterministically, and is covered by a shape test.

### P2-T02 — Build the snapshot in update paths and memoise it
- **Intent:** never violate the render contract, and never pay for the snapshot when nothing changed.
- **Touches:** `crates/fleet-app/src/state.rs`, `crates/fleet-app/src/state/harness.rs`.
- **Steps:**
  - Load `gpui-performance` — it owns memoised projections and the frame budget.
  - Give `AppState` a revision counter that already-existing update paths bump (or reuse one if the
    app has it); build the snapshot lazily on request and cache it against that revision.
  - Nothing in `render` may call the builder. Confirm by reading the call sites, not by assuming.
  - When harness mode is off, the builder is never called and the cache is never allocated.
- **Verification:** `make lint`; `make test`; a test asserts that two `dump`s with no intervening
  state change return the same revision and do not rebuild; `grep` the render paths for the builder
  and record that it is absent.
- **Done when:** the snapshot is always current, built off the render path, and free when unused.

### P2-T03 — `dump` command
- **Intent:** let a scenario capture the app's own account of a moment, next to the screenshot of it.
- **Touches:** `crates/fleet-app/src/drive.rs`, `crates/fleet-harness/src/`.
- **Steps:**
  - `dump <name>`: reply with the full snapshot; the runner writes `dumps/NNN-<name>.json` beside
    the shots, using the same sequence number so a shot and a dump of the same moment pair up.
  - The response also carries a short human summary line (screen, mode, focus, selected row) that
    the runner echoes to stdout, so a run is readable without opening files.
- **Verification:** `make lint`; `make test`; a scenario of `dump start`, `key ?`, `dump help` shows
  `overlay` changing from none to the help overlay in the two files.
- **Done when:** every `dump` line produces a JSON file and a one-line summary.

### P2-T04 — Predicate language and `await`
- **Intent:** replace sleeping with waiting for the thing you actually meant.
- **Touches:** `crates/fleet-drive/src/predicate.rs` (new), `crates/fleet-app/src/drive.rs`.
- **Steps:**
  - Grammar: `<path> <op> <value>`, where `path` is a dotted path into the snapshot
    (`overlay`, `lists.worktrees.selected.label`, `toasts[0].text`), and `op` is one of
    `==`, `!=`, `~=` (regex), `>`, `<`, `exists`, `absent`. Plus the bare predicate `idle`
    (see P2-T07). Conjunction with `&&` is enough; no precedence rules, no parentheses.
  - Parse in `fleet-drive` so the runner can reject a malformed predicate before the app sees it.
  - `await <predicate> [timeout-ms]` (default 5000): evaluate on every snapshot revision change;
    reply as soon as it holds. On timeout reply `ok:false` with the predicate, the elapsed time and
    the *last* snapshot, so the failure explains itself.
  - Implement the waiter with the app's existing observation machinery — no polling timer. Load
    `rust-async-background-work` for the cancellation and timeout shape.
- **Verification:** `make lint`; `make test`; unit tests for the parser (including malformed input)
  and an integration test where `await` returns in well under its timeout after a state change and
  reports the last snapshot when it times out.
- **Done when:** a scenario can replace every `wait` with an `await` and run faster and greener.

### P2-T05 — `assert`, and failure that explains itself
- **Intent:** a scenario that is wrong must fail loudly, once, with everything needed to diagnose it.
- **Touches:** `crates/fleet-app/src/drive.rs`, `crates/fleet-harness/src/`.
- **Steps:**
  - `assert <predicate>`: evaluate once against the current snapshot; on failure reply `ok:false`
    with the predicate, the actual value at that path, and the snapshot.
  - On any failed command the runner automatically captures a screenshot and a dump named
    `failure-<NNN>`, writes them into the run directory, and stops the scenario unless
    `--continue-on-failure` was passed.
  - Exit code and stdout name the failing line, the actual value, and the artifact paths.
- **Verification:** `make lint`; `make test`; a deliberately wrong scenario exits nonzero, prints the
  actual value, and leaves a failure screenshot and dump.
- **Done when:** a failure is diagnosable from the runner's stdout alone, with the artifacts as
  backup.

### P2-T06 — Terminal grid as text
- **Intent:** make PTY output assertable, since a large part of Fleet is a terminal.
- **Touches:** `crates/fleet-app/src/state/terminal.rs`, `crates/fleet-app/src/state/harness.rs`.
- **Steps:**
  - Project `MirrorGrid` into `terminal: { rows: [String], cursor: {row, col, shape}, viewport }`,
    trimming trailing blanks, preserving wide-character columns so alignment survives.
  - Keep it out of the default snapshot when no terminal is visible; it is the largest field.
  - A predicate like `terminal.rows[*] ~= "installed"` needs a wildcard — add exactly that one
    wildcard form to the predicate grammar, or expose `terminal.text` as the joined string and skip
    wildcards entirely. Prefer the latter unless a scenario needs per-row precision.
- **Verification:** `make lint`; `make test`; a scenario that opens a worktree session, types
  `echo harness-ok`, and awaits `terminal.text ~= "harness-ok"` passes without a sleep.
- **Done when:** terminal content is assertable as text.

### P2-T07 — The `idle` predicate
- **Intent:** one predicate that means "the app has finished reacting", so scenarios stop guessing.
- **Touches:** `crates/fleet-app/src/state/harness.rs`, `crates/fleet-app/src/bridge.rs`.
- **Steps:**
  - Define idle precisely and document it: no in-flight bridge requests, no running jobs, no pending
    frame request, no live toast timer, no debounce timer armed.
  - Expose the constituent counters in the snapshot too, so a timeout on `await idle` says *which*
    part is still busy.
- **Verification:** `make lint`; `make test`; a test where a long-running job keeps `idle` false and
  its completion flips it, with the timeout message naming the job.
- **Done when:** `await idle` is reliable enough that scenarios use it between steps by default.

### P2-T08 — Runner report
- **Intent:** one file an agent can read to know what the run did.
- **Touches:** `crates/fleet-harness/src/report.rs` (new).
- **Steps:**
  - Write `report.md` in the run directory: the scenario, the lane, per-line outcome and duration,
    the one-line summary from each `dump`, inline links to each screenshot, and the failure block if
    there was one.
  - Keep it small enough to read in full; the JSONL stays the complete record.
- **Verification:** `make lint`; `make test`; the report of a passing and of a failing run both read
  correctly to someone who did not watch the run.
- **Done when:** reading `report.md` is enough to know what happened.

### P2-T09 — Documentation
- **Intent:** keep `docs/` authoritative as the harness grows.
- **Touches:** `docs/TESTING-HARNESS.md`, `docs/APP-CONTRACTS.md`.
- **Steps:**
  - Document the snapshot contract in `docs/TESTING-HARNESS.md`: every field, its vocabulary, the
    version policy, and the rule that it is built in update paths and memoised.
  - Add a line to `docs/APP-CONTRACTS.md` noting that the harness snapshot is a projection subject to
    the same "render prepares nothing" rule, so a future contributor does not move it into `render`.
  - Load `zed-quality-review` before declaring the phase done.
- **Verification:** `make lint`; `make test`; every predicate path in the docs is one a real dump
  actually contains (check against a captured dump).
- **Done when:** the docs describe the snapshot that exists.

## Verification

```sh
make lint
make test
make restart   # if anything daemon-side changed
```

Acceptance run, from a clean tree:

```sh
cargo build --workspace
cat > /tmp/assert.scenario <<'SCENARIO'
await idle
assert screen == Hub
key ?
await overlay == Help
shot help
dump help
key escape
await overlay absent
quit
SCENARIO
./target/debug/fleet-harness run /tmp/assert.scenario
```

It must exit 0 with no `wait` line in the scenario, and leave a paired `shots/…-help.png` and
`dumps/…-help.json`. Changing `assert screen == Hub` to a wrong value must exit nonzero, print the
actual value, and leave a failure screenshot and dump.

## Definition of done

- [ ] Every task in the tracker is checked off, with its verification output recorded.
- [ ] `make lint` is clean.
- [ ] `make test` passes on a clean tree.
- [ ] Both acceptance runs (passing and deliberately failing) behave as described.
- [ ] No snapshot construction happens in any `render` path — checked by reading the call sites.
- [ ] `docs/TESTING-HARNESS.md` and `docs/APP-CONTRACTS.md` match the code, updated in the same
      commits.
- [ ] The tracker reflects reality, including deviations from this plan.
- [ ] Follow-ups are recorded in the tracker.

## Risks and rollback

- **The snapshot becomes a second source of truth.** It must remain a pure projection: if a field
  cannot be derived from existing state, that is a signal the state is missing something, not a
  reason to store it twice. Rollback: the builder is one module; deleting it removes the risk.
- **Memoisation goes stale.** A snapshot that misses an update makes `await` hang until timeout —
  annoying, not dangerous, and the timeout message shows the stale revision. Prefer rebuilding on any
  state mutation over clever invalidation.
- **The predicate language grows a grammar.** Resist. If scenarios need computation, they need a
  Rust test instead. Cap it at the operators listed.
- **Terminal text in every snapshot bloats dumps.** Gate it on a visible terminal and, if it still
  hurts, on an explicit `dump --terminal` flag.
- **`idle` is wrong in a way that hides a bug.** Anything that makes `idle` true while work is
  pending will make scenarios flaky in a way that looks like an app bug. Its constituent counters are
  in the snapshot precisely so a flake can be traced to the right half.
