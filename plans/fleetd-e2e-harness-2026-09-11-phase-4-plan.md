# Fleet e2e harness — Phase 4: agents, fixtures and fault injection — Plan
> Tracker: ./fleetd-e2e-harness-2026-09-11-phase-4-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

By the end of Phase 3 the harness can drive and interrogate any surface it can reach — but most of
Fleet's interesting surfaces are unreachable from an empty temporary home. There are no repos, no
worktrees, no pull requests, no board, no jobs; the native-agent thread needs a Claude or Codex
binary, a network and a token; and the daemon-down, reconnect and sticky-error screens only appear
when something breaks.

This phase makes those states reachable on demand and deterministically. The harness gains a fixture
builder that seeds a hermetic home with real git repos and worktrees; a scripted agent binary that
speaks the Claude and Codex wire protocols from a transcript file, so approvals, gates, streaming
and errors can be rehearsed without a vendor CLI or a token; and fault-injection controls that kill,
stall or restart the daemon so the app's degraded surfaces can be photographed and asserted on.

## Sizing call

**Phased**, phase 4 of 5 — see [the roadmap](./fleetd-e2e-harness-2026-09-11-roadmap.md). The largest
phase after Phase 1: it touches `fleet-daemon`'s agent harness, adds a second binary, and builds a
fixture library. It stays one phase because the fixture, the agent and the fault controls all serve
the same goal — making states reachable — and share the same seams with the runner.

## Repository context

- **Project type / commands:** as Phase 1 — `make lint`, `make test`, `make restart` after daemon
  changes.
- **A scripted mock peer already exists.** `crates/fleet-daemon/src/agents/harness/mockpeer.rs` is a
  `/bin/sh` script that stands in for a vendor CLI, chosen over Python or Node because `fleetd`
  already depends on a POSIX shell. Its own comment states the policy this phase builds on: "The
  default `make test` run spawns neither `claude` nor `codex` — every other agent test drives a
  scripted `/bin/sh` mock peer — because a test suite that needs two vendor CLIs installed is a test
  suite that silently skips on the machine that most needs it."
- **Real-binary tests are opt-in** behind `--features real-agents`
  (`crates/fleet-daemon/tests/agents_real_binaries.rs`) and deliberately send no prompt, so they cost
  no tokens. The harness follows the same split.
- **`fleet-daemon` already has a `test-support` feature** (`crates/fleet-daemon/Cargo.toml`) used by
  its own integration tests; that is the door the harness uses rather than a new one.
- **Fakes are an established pattern**: fake `gh` in `crates/fleet-app/tests/common/mod.rs`, fake
  `acli` for Jira (`adapters/board/jira/acli.rs`), fake process/git/github adapters throughout
  `fleet-daemon/src/adapters/`.
- **Authoritative docs for this phase:** `docs/NATIVE-AGENTS.md` (adapters, event model, thread
  state, decision surfaces, the agent tab), `docs/decisions/0010-native-agents.md`,
  `docs/decisions/0013-sqlite-agent-transcripts.md`, `docs/decisions/0014-drop-opencode-add-codex.md`,
  `docs/research/harness-protocols.md` and `docs/research/harness-codex-app-server.md` (the wire
  references), and `docs/REMOTE-MACHINES.md` for the link states the app can show.
- **Known gaps that the harness should be able to demonstrate** are listed in `TODO.md` — the Codex
  approval that shows paths instead of a diff, the empty `^s e` menu on Codex, the amber dot
  returning after restart. These make good first scenarios because their fixed state is verifiable.

## Assumptions

- Scripted agents imitate the wire protocol, not the vendor's behaviour. The harness proves Fleet
  reacts correctly to a given sequence of protocol messages; it does not prove Claude or Codex emits
  that sequence. The opt-in `real-agents` tests remain the check on that.
- Fixtures build real git repositories with `git`, not fabricated `.git` directories. Fleet's git
  layer is real and deserves real input.
- Fault injection belongs to the runner and to `fleetd`'s `test-support` surface, never to the app's
  socket. The app must learn about a dead daemon the way it does in production.

## Out of scope

- Remote machines over real SSH. `docs/REMOTE-MACHINES.md` describes a federation the harness could
  eventually exercise with a loopback link, and `crates/fleet-daemon/tests/remote_*.rs` already does
  some of it; wiring it into the GUI harness is a follow-up, not this phase.
- Jira and GitHub network behaviour beyond the existing fake `gh` and fake `acli`.
- Time travel inside the daemon. The app-side clock control in P4-T06 covers toast dwell and
  debounce; making `fleetd` believe in a fake clock is a much larger change with no harness payoff
  yet.

## Affected areas

- `crates/fleet-daemon/src/agents/harness/mockpeer.rs` and its `test-support` exposure.
- `crates/fleet-harness/src/fixture/` (new) — repos, worktrees, contexts, config, fake tools.
- `crates/fleet-harness/src/agent/` (new) — the scripted agent binary and its transcript format.
- `crates/fleet-harness/src/fault.rs` (new) — daemon kill/stall/restart, job failure.
- `docs/TESTING-HARNESS.md`, `docs/NATIVE-AGENTS.md`.

## Tasks

### P4-T01 — Promote the mock peer to `test-support`
- **Intent:** one scripted peer implementation, usable by the daemon's tests and by the harness.
- **Touches:** `crates/fleet-daemon/src/agents/harness/mockpeer.rs`, `crates/fleet-daemon/Cargo.toml`.
- **Steps:**
  - Load `rust-workspace-architecture` (feature and module rules) and `rust-gpui-testing` (fake
    conventions) first.
  - Move the mock peer behind the existing `test-support` feature instead of `#[cfg(test)]`, keeping
    its `/bin/sh` implementation and its existing callers green.
  - Keep its API narrow: build a peer from a transcript, return the command line to launch it.
- **Verification:** `make lint`; `make test`; the daemon's existing agent tests
  (`services/agents/manager/tests/`, `agents/claude/tests/`, `agents/codex/tests.rs`) still pass
  unchanged.
- **Done when:** the harness can construct a mock peer without duplicating it.

### P4-T02 — The scripted agent binary and its transcript format
- **Intent:** rehearse a whole agent conversation — streaming, tool calls, approvals, errors —
  deterministically and for free.
- **Touches:** `crates/fleet-harness/src/agent/`.
- **Steps:**
  - Define a transcript file format (one step per line or a small JSON document) covering: assistant
    text deltas with configurable pacing, tool calls, file-change items with real diffs, permission
    and approval requests, reasoning-effort and model listings, errors, and a terminal exit.
  - Implement `fleet-harness agent --provider <claude|codex> --transcript <file>` as the binary a
    seeded config points `AgentKind::executable()` at, speaking the wire protocols described in
    `docs/research/harness-protocols.md` and `docs/research/harness-codex-app-server.md`.
  - Reuse the mock peer from P4-T01 wherever it already covers a handshake, rather than
    reimplementing it.
  - Ship three starter transcripts: a clean two-turn conversation; one that requests an edit
    approval with a diff; one that errors mid-stream.
- **Verification:** `make lint`; `make test`; a harness scenario opens an agent tab against the
  scripted binary, awaits the streamed text in the snapshot, approves an edit, and asserts the
  thread reaches its completed state — with no vendor CLI installed and no network.
- **Done when:** the agent surfaces in `docs/NATIVE-AGENTS.md` §6 are reachable from a transcript.

### P4-T03 — The fixture builder
- **Intent:** start a scenario in the state it is about, not at an empty first-run screen.
- **Touches:** `crates/fleet-harness/src/fixture/`.
- **Steps:**
  - Build a hermetic `FLEET_HOME` from a declarative fixture description: N repos (real `git init`,
    real commits, branches, dirty files where asked), worktrees, contexts, a seeded `config.json`,
    and agent commands pointed at the scripted binary from P4-T02.
  - Put the existing fake `gh` on `PATH`, plus a fake `acli` for board scenarios, both answering from
    fixture data so PR and board screens have content.
  - Provide a small set of named presets (`empty`, `one-repo`, `busy`, `board`, `agents`) so
    scenarios declare `fixture: busy` on their first line rather than repeating setup.
  - Seeding goes through the daemon's own API wherever one exists, so the fixture cannot drift from
    what the daemon would have written itself.
- **Verification:** `make lint`; `make test`; each preset boots and a `dump` shows the expected repos,
  worktrees and rows; two runs of the same preset produce identical dumps except for ids and times.
- **Done when:** a scenario can declare the world it needs in one line.

### P4-T04 — Fault injection
- **Intent:** photograph and assert the degraded surfaces, which are exactly the ones nobody tests.
- **Touches:** `crates/fleet-harness/src/fault.rs`.
- **Steps:**
  - Runner-side commands: `daemon kill` (SIGKILL, for `Daemon > Down`), `daemon stop`/`daemon cont`
    (SIGSTOP/SIGCONT, for a stalled daemon and the reconnect banner), `daemon restart`, and
    `socket remove` for a vanished socket file.
  - Each is a scenario line that returns only once the *app* has observed the consequence — assert
    on the snapshot's connection state, do not sleep.
  - Verify the recovery path too: after `daemon restart` the app must return to a working state, and
    the scenario asserts it.
- **Verification:** `make lint`; `make test`; scenarios that reach `Daemon > Down`, the reconnect
  banner and the recovered state, each with a screenshot; `docs/KEYMAP.md`'s claim that `ctrl-q`
  still works on the down screen is checked by one of them.
- **Done when:** the daemon-failure surfaces are as easy to reach as the hub.

### P4-T05 — Job, toast and PR injection
- **Intent:** make the notification surfaces deterministic.
- **Touches:** `crates/fleet-harness/src/fixture/`, `crates/fleet-harness/src/fault.rs`.
- **Steps:**
  - Drive real jobs that succeed, fail and run long, through the daemon's own job API, so the jobs
    panel, the toast stack, the coalescing rule and the sticky error all light up from real events.
  - Seed PR data through the fake `gh` so the PR screen and its badges have content.
- **Verification:** `make lint`; `make test`; a scenario asserts a failed job appears in the sticky
  error slot and that `!` focuses it, per `docs/KEYMAP.md` [A18].
- **Done when:** notification surfaces can be driven without waiting for real work.

### P4-T06 — App-side clock control
- **Intent:** stop scenarios waiting out real timers.
- **Touches:** `crates/fleet-app/src/drive.rs`, `crates/fleet-app/src/state/notifications.rs`.
- **Steps:**
  - Load `rust-async-background-work`; timers and debounce are its territory.
  - Add an `advance <ms>` command that moves the app's own time-dependent state forward — toast
    dwell and expiry, debounce windows — without sleeping, and document exactly what it does and does
    not affect (it does not move the daemon's clock).
  - Where the app reads wall-clock time directly in a way that blocks this, note it as a follow-up
    rather than refactoring broadly.
- **Verification:** `make lint`; `make test`; a scenario asserts a toast is present, advances past its
  dwell, and asserts it is gone — in milliseconds of wall time.
- **Done when:** timer-dependent behaviour is testable without real waiting.

### P4-T07 — Freeze the scenario grammar and document it
- **Intent:** Phase 5 writes dozens of scenarios; the vocabulary must stop moving first.
- **Touches:** `docs/TESTING-HARNESS.md`, `docs/NATIVE-AGENTS.md`.
- **Steps:**
  - Write the complete grammar into `docs/TESTING-HARNESS.md`: every command, every predicate
    operator, the fixture presets, the target naming scheme, the transcript format, and the run
    directory layout. Mark it as the frozen surface Phase 5 depends on.
  - Add a testing section to `docs/NATIVE-AGENTS.md` describing how an agent surface is exercised
    from a transcript, so the two documents agree about the same mechanism.
  - Load `zed-quality-review` before declaring the phase done.
- **Verification:** `make lint`; `make test`; every command in the documented grammar appears in at
  least one passing scenario.
- **Done when:** a stranger can write a scenario from the doc alone.

## Verification

```sh
make lint
make test
make restart
```

Acceptance run:

```sh
cargo build --workspace
./target/debug/fleet-harness run scenarios/agent-approval.scenario   # scripted Codex edit approval
./target/debug/fleet-harness run scenarios/daemon-down.scenario      # kill, banner, restart, recover
```

Both exit 0, with screenshots of the approval card and of the daemon-down screen, and with no vendor
CLI installed, no network access and no tokens spent.

## Definition of done

- [ ] Every task in the tracker is checked off, with its verification output recorded.
- [ ] `make lint` is clean.
- [ ] `make test` passes on a clean tree, with the daemon's existing agent tests unchanged.
- [ ] Both acceptance scenarios pass, and their screenshots have been looked at.
- [ ] No scenario requires `claude`, `codex`, network access or a token.
- [ ] `docs/TESTING-HARNESS.md` and `docs/NATIVE-AGENTS.md` match the code.
- [ ] The scenario grammar is declared frozen in the docs.
- [ ] The tracker reflects reality; follow-ups are recorded.

## Risks and rollback

- **The scripted agent drifts from the real protocol.** A transcript that no vendor would emit gives
  false confidence. Mitigated by generating starter transcripts from the wire references in
  `docs/research/`, and by keeping the opt-in `real-agents` handshake tests as the check on reality.
  When a real protocol changes, the transcripts are the first thing to update.
- **`test-support` leaks into a release build.** The feature already exists and is used this way;
  keep the mock peer behind it and never enable it by default. `rust-workspace-architecture` is the
  reviewer.
- **Fixtures become a second definition of Fleet's state.** Seed through the daemon's API wherever
  possible so there is one writer. Where a fixture must write a file directly, say so in a comment
  naming the schema it is imitating and the version constant that governs it.
- **Fault injection leaves a stray daemon.** `daemon stop` without a matching `cont` would leave a
  SIGSTOPped process. Teardown must `SIGCONT` then kill unconditionally, and the runner already owns
  teardown from Phase 1.
- **Clock control diverges from the daemon's clock.** Documented explicitly as app-side only; a
  scenario that needs daemon-side time control should wait on a real event instead.
