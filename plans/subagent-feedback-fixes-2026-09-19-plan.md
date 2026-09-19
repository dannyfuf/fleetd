# Native subagent feedback fixes (batch 1) — Plan

> Tracker: ./subagent-feedback-fixes-2026-09-19-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Fleet's native subagent feature (`fleet subagent run|complete|wait|status|list|cancel` and
`fleet agent list|tail`) shipped in the `plans/native-subagents-2026-09-17-*` batch. An
orchestrator agent then drove it end to end for a full session and reported eight pain points.
This plan fixes five of them: the `fleet` CLI not being reachable from a child process, `subagent
wait` printing a terminal-shaped message on timeout while being hard-capped at 540 seconds, no
way to pass reasoning effort to a child, a same-worktree warning that fires even when the caller
asked for that worktree by name, and `fleet agent tail` having no way to print a snapshot and
exit. The other three reported items are deliberately deferred and live in `SESSION_TODO.md` at
the repo root.

None of this changes a pixel. It is CLI surface, daemon child-environment construction, and the
delegation contract text that children and callers read.

## Sizing call

**Standard.** Five independent, small changes to one subsystem (native subagents) across a CLI
crate, a client crate, a proto crate and the daemon. Each item is tens of lines. There is no
migration, no intermediate state that must ship on its own, and no sequencing risk beyond one
additive wire field that has to land before the two consumers of it. I considered phasing and
rejected it: a phase boundary would buy nothing except two more files to keep in sync. One plan,
one tracker, five tasks.

The interesting constraint is not size, it is **concurrency**. These tasks are executed by
separate agents at the same time in one shared worktree, so the binding limit is file ownership,
not scope. See *Concurrency and file ownership* below; it is the reason the tasks are cut the way
they are rather than one-per-reported-item.

## Repository context

- **Project type:** Rust, one Cargo workspace, edition 2024 (`rust-toolchain.toml`,
  `rustfmt.toml`). Members under `crates/`: `fleet-app`, `fleet-cli`, `fleet-client`,
  `fleet-core`, `fleet-daemon`, `fleet-drive`, `fleet-git`, `fleet-harness`, `fleet-lazygit`,
  `fleet-proto`, `fleet-term`, `fleet-ui-kit`. Binaries: `fleet` (GPUI macOS app + CLI) and
  `fleetd` (daemon).
- **Lint:** `make lint` = `cargo fmt --all -- --check` then
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **Test:** `make test` builds `fleet-daemon`, `fleet-app` and `fleet-harness` first, then runs
  `cargo test --workspace` with `FLEET_DAEMON`, `FLEET_APP` and `FLEET_HARNESS_BIN` pointed at
  `target/debug/`.
- **Type-check:** there is no separate type-checker; `cargo check --workspace --all-targets` is
  exposed as `make check` and is subsumed by `make lint`.
- **GUI:** `make harness` / `make harness-headless` drive the real app. **Not needed for this
  batch** — nothing here renders. Say so in the tracker rather than skipping it silently.
- **Daemon:** `make restart` rebuilds and restarts the running `fleetd`. Required after any
  change under `crates/fleet-daemon/`, so T02 and T04 both end with it.
- **No install target.** The `Makefile` has `build`, `run`, `restart`, `doctor`, `lint`, `test`,
  `harness*`, `bootstrap`, `prune`, `fresh`, `clean` — and no `install`. `make build` puts
  `fleet` and `fleetd` side by side in `target/$(PROFILE)/`, which is what makes the T01/T02
  approach work locally with no Makefile change. This is recorded under *Assumptions*.
- **Docs are authoritative** (`CLAUDE.md`, `docs/README.md` assigns each file a domain). A
  behaviour change and its doc change belong in the same commit. This batch has one deliberate,
  named exception — see *Concurrency and file ownership*.
- **Skills** live in `.claude/skills/`. Every task below names the one to load before editing.
- **Prior art, do not modify:** `plans/native-subagents-2026-09-17-*` (roadmap, seven phase plans
  and trackers, plus a contracts file). They describe the feature this plan repairs.

## Assumptions

- **`fleet` and `fleetd` are siblings in every supported install.** Verified for the local dev
  path (`make build` → `target/debug/`). The remote path installs only `fleetd`
  (`crates/fleet-daemon/src/services/bootstrap.rs`, `$HOME/.local/bin`), which is exactly the
  case the daemon-side fallback in T02 exists to cover. No `Makefile` change is planned because
  there is no install target to change; if one is added later it must keep the two binaries
  together.
- **`--effort` without `--model` is allowed.** The child keeps the provider's default model and
  gets the requested effort. `create_with` in
  `crates/fleet-daemon/src/services/agents/manager/commands.rs` already fills `effort` from the
  per-provider defaults only when the selection carries none, so an explicit effort survives. T03
  documents this explicitly rather than leaving it to be discovered.
- **`--effort` is a free string, not a clap enum.** The legal ladder is per provider and per
  model and is published by the harness (`docs/NATIVE-AGENTS.md` §"Reasoning effort"), so Fleet
  never hardcodes one. A bad value is the provider's error to report, not clap's.
- **`docs/SWARM-INVENTORY.md` is not touched.** It was named as an authoritative doc for this
  work, but it contains no reference to subagents, delegations or the `fleet subagent` verb group
  — it is the Swarm compatibility baseline. Nothing in this batch has a Swarm counterpart to
  deviate from. If a reviewer disagrees, the change belongs in its Fleet-deviations preamble.
- **`StartRequest` is daemon-internal.** It lives in `crates/fleet-core/src/agents/provider.rs`
  and appears nowhere in `crates/fleet-proto/`, so T02 can extend it without touching a wire
  golden. Only the `DelegationRun` request variant in T01 has goldens.
- **Line numbers drift.** Every location below was re-verified on 2026-09-19 against the
  `fix/native-subagents` branch and is given as a symbol name plus an approximate line. Trust the
  symbol.

## Out of scope

Tracked in `SESSION_TODO.md`, not here:

- Cost and token visibility in `fleet agent list` / `fleet subagent status`.
- A repeatable `--env KEY=VALUE` on `fleet subagent run`.
- Changing the end-of-turn result-delivery policy (ADR 0017), including any `wait --follow`.
- `fleet subagent run --json` echoing the whole brief back in its envelope.

Also out of scope: any GUI change, any change to `fleet agent new`, paginated event history in
the protocol, and touching `plans/native-subagents-2026-09-17-*`.

## Affected areas

Single repository (`fleetd`, this worktree). Grouped by crate.

**`crates/fleet-proto`** — `src/request.rs` (`RequestBody::DelegationRun`, ~315),
`src/request/tests.rs`, `tests/agent_compatibility.rs` (wire goldens at ~787 and ~857, the
`delegation_run` JSON literals at ~799 and ~869).

**`crates/fleet-client`** — `src/api/agents.rs` (`DelegationRunRequest` ~58, `delegation_run`
~102). `src/connection.rs` already grants `timeout_ms + 15 s` for a wait and imposes no cap; it
should need no change, but confirm.

**`crates/fleet-cli`** — `src/args.rs` (`SubagentRunArgs` ~390, `SubagentWaitArgs` ~444,
`AgentTailArgs` ~547), `src/commands/subagents.rs` (`run` ~92, `wait` ~187, `model_selection`
~265), `src/commands/agents.rs` (`tail` ~119, `tail_to` ~128, `print_tail` ~207), `src/human.rs`
(`delivered_message` ~213), `src/commands/tests.rs` (1889 lines, the crate's only command-level
test module).

**`crates/fleet-core`** — `src/agents/provider.rs` (`StartRequest` ~65).

**`crates/fleet-daemon`** — `src/services/agents/delegation/run.rs` (`extra_env` ~146, the
warning at ~268), `src/services/agents/delegation/footer.rs` (`FOOTER_TEMPLATE` ~9,
`SAME_WORKTREE_WARNING` ~24, `footer` ~29, and its verbatim-text tests),
`src/services/agents/delegation/tests/run.rs`, `src/services/agents/manager/commands.rs`
(`CreateOptions` ~27), `src/agents/harness/process.rs` (`resolve_program` ~176,
`login_environment` ~195, `filter_environment` ~226, `spawn_child` ~289),
`src/agents/claude/mod.rs` (`environment` ~113), `src/agents/codex/mod.rs` (`environment` ~129),
`src/services/doctor.rs` (`check_subagent_fleet` / `subagent_fleet_check` ~454),
`tests/doctor_checks.rs`.

**Docs** — `docs/NATIVE-AGENTS.md` (§15 delegation: ~1775 wait cap, ~1790 footer text, ~1812
same-worktree warning; plus the effort table at ~1008), `docs/decisions/0017-native-subagents.md`
(~127 worktree default, ~147 the 540 s cap), `docs/research/agents-contracts.md` (~778 the
`fleet agent` verb list, ~785 the `run` flag list, ~793 the `wait` cap),
`docs/DEVELOPMENT.md` (~87 the `make doctor` expectation), `README.md` (~101 the
`fleet agent tail` row and the surrounding CLI table).

**Root** — `SESSION_TODO.md`.

## Concurrency and file ownership

These five tasks are executed by separate agents **at the same time in one shared worktree**.
There is no branch per task and no merge step, so two agents editing one file will clobber each
other. Ownership below is therefore exclusive and absolute: **if a file is not listed under your
task's "Owns", do not open it for writing.**

Five reported items map onto four shared files, so a naive one-task-per-item cut is impossible:

| Contested file | Wanted by items | Resolution |
| --- | --- | --- |
| `crates/fleet-cli/src/args.rs` | 2 (wait cap), 3 (`--effort`), 5 (tail flags) | **Merged.** All three flag changes live in T03. Splitting a clap field's declaration from its use would leave the declaring task with a dead field and `-D warnings` failing. |
| `crates/fleet-cli/src/commands/subagents.rs` | 1 (send `current_exe`), 2 (wait message), 3 (effort) | **Ordered.** T01 owns it first and commits; T03 owns it afterwards. T01's edit is small and lands early. |
| `crates/fleet-cli/src/commands/tests.rs` | 1, 2, 3, 5 | Same ordering as above: T01, then T03. |
| `crates/fleet-daemon/src/services/agents/delegation/run.rs` and `footer.rs` | 1 (daemon half), 4 (warning) | **Merged.** Items 1-daemon and 4 are one task, T02. Both are a few lines in the same two files, and both change the contract text a child or caller reads. |
| `docs/NATIVE-AGENTS.md` | 1, 2, 3, 4, 5 | **Merged into T02, which writes the §15 changes for the whole batch from this plan.** See the deviation note below. |
| `docs/research/agents-contracts.md` | 1, 2, 3, 5 | Owned by T03 (it is the CLI-surface contract). T01 adds nothing to it; the `fleet_path` field is an internal request field, and T02 documents its behaviour in NATIVE-AGENTS. |

**Named deviation from the same-commit doc rule.** `CLAUDE.md` requires a behaviour change and
its doc change to land in one commit. `docs/NATIVE-AGENTS.md` §15 is touched by four of the five
items, so honouring that literally would serialise the whole batch. Instead **T02 owns
`docs/NATIVE-AGENTS.md` and writes every §15 change in this batch, including the ones whose code
lives in T03**, working from this plan — which is the specification, so the doc does not need to
read T03's diff. T03 then re-reads §15 as its last step (T02 has committed by then and the file
is free) and fixes any place where the shipped behaviour and the doc disagree. This is a
deliberate, bounded exception; do not generalise it.

**Execution order.**

```
T01  (wire + caller side)            ── must commit first
  ├── T02  (daemon + NATIVE-AGENTS + ADR 0017)   ┐
  ├── T03  (all of fleet-cli + contracts + README) ├── parallel
  └── T04  (doctor + DEVELOPMENT.md)             ┘
        └── T05  (verify + SESSION_TODO)          ── last, after all three
```

T02, T03 and T04 all read `RequestBody::DelegationRun`'s new field or depend on the tree
compiling with it, so none of them may start before T01 has committed. T03 additionally waits for
T01 because it inherits `crates/fleet-cli/src/commands/subagents.rs`.

## Tasks

### T01 — Send the caller's own `fleet` executable path with `subagent run`

**Intent:** make the daemon able to learn where a wire-compatible `fleet` binary lives, by having
the `fleet` process that issued `subagent run` report its own absolute path.

**Skill to load:** `rust-ipc-protocol` (this changes `fleet-proto` and the `fleet-client` request
builder). Load `zed-quality-review` before declaring done.

**Owns:**
- `crates/fleet-proto/src/request.rs`
- `crates/fleet-proto/src/request/tests.rs`
- `crates/fleet-proto/tests/agent_compatibility.rs`
- `crates/fleet-client/src/api/agents.rs`
- `crates/fleet-cli/src/commands/subagents.rs` *(handed to T03 after this task commits)*
- `crates/fleet-cli/src/commands/tests.rs` *(handed to T03 after this task commits)*

**Steps:**
- Add an optional field to `RequestBody::DelegationRun` in `crates/fleet-proto/src/request.rs`
  (~315) carrying the caller's absolute `fleet` path as a string. Make it additive and
  backward-compatible the same way the variant's other optionals are:
  `#[serde(default, skip_serializing_if = "Option::is_none")]`. Place it after `title` and before
  `eager` so the serialized field order matches the declaration order the goldens already assume.
- Document on the field, in a doc comment, *why* it exists and what it is not: it is a hint about
  where a wire-compatible `fleet` lives on the **caller's** host, it is advisory, and the daemon
  must tolerate it being absent, stale, or pointing at a path that does not exist on the daemon's
  host.
- Add the matching field to `DelegationRunRequest` in `crates/fleet-client/src/api/agents.rs`
  (~58) and pass it through `delegation_run` (~102). Keep the client type a `PathBuf` and convert
  at the wire boundary, or keep it a `String` throughout — pick one and be consistent; do not
  convert twice.
- In `crates/fleet-cli/src/commands/subagents.rs`, in `run` (~92), resolve the path once:
  `std::env::current_exe()`, then canonicalise it. Both calls are fallible and **must not** be
  `unwrap`ped or `let _ =`'d. On failure, send `None` and carry on — a missing hint degrades to
  today's behaviour, it is not a reason to refuse a delegation. Do not log at error level for
  this.
- Decide and comment on whether a non-`fleet` file name is sent. The CLI is by definition the
  binary the orchestrator invoked, so send whatever it is; the daemon prepends the *directory*,
  not the file.
- Update the two `delegation_run` wire goldens in
  `crates/fleet-proto/tests/agent_compatibility.rs` (~787/~799 the fully-populated one, ~857/~869
  the minimal one). The fully-populated golden gains the new field with a realistic absolute
  path; the minimal one must stay byte-identical, which is the proof that the field is optional.
- Add a CLI-level test in `crates/fleet-cli/src/commands/tests.rs` covering the `run` path
  (see *Regression test* below).

**Regression test:** in `crates/fleet-proto/tests/agent_compatibility.rs`, a case asserting that
a `DelegationRun` **without** the new field round-trips to exactly the byte sequence it produced
before this change (the ~869 literal, unmodified), and a second case asserting the populated
variant serializes the new field. In `crates/fleet-cli/src/commands/tests.rs`, extend the
existing `DelegationRun` case near ~1138 so it asserts the request the CLI builds carries a
non-empty absolute path when `current_exe` resolves.

**Verification:**
- `cargo test -p fleet-proto`
- `cargo test -p fleet-client`
- `cargo test -p fleet-cli`
- `cargo clippy -p fleet-proto -p fleet-client -p fleet-cli --all-targets --all-features -- -D warnings`
- `make lint`

**Docs updated in this commit:** none. The field is internal plumbing with no observable
behaviour until T02 consumes it; T02 documents the behaviour, T03 documents the CLI surface.
State this explicitly in the commit body so a reviewer does not think the doc rule was missed.

**Done when:** a `fleet subagent run` issued by a built `fleet` puts that binary's absolute path
on the wire, an older client omitting the field is still accepted byte-for-byte, and nothing
about the child's behaviour has changed yet.

---

### T02 — Put `fleet` on the child's PATH, print it in the footer, and stop warning on an explicit `--worktree`

**Intent:** fix reported items 1 (daemon half) and 4 — a child can run bare `fleet subagent
complete`, and a caller who named a worktree on purpose is not lectured about it.

**Skill to load:** `rust-workspace-architecture` (new helper, new log line, error handling) and
`rust-async-background-work` (this is in the spawn path). Load `zed-quality-review` before
declaring done.

**Owns:**
- `crates/fleet-core/src/agents/provider.rs`
- `crates/fleet-daemon/src/agents/harness/process.rs`
- `crates/fleet-daemon/src/agents/claude/mod.rs`
- `crates/fleet-daemon/src/agents/codex/mod.rs`
- `crates/fleet-daemon/src/services/agents/manager/commands.rs`
- `crates/fleet-daemon/src/services/agents/delegation/run.rs`
- `crates/fleet-daemon/src/services/agents/delegation/footer.rs`
- `crates/fleet-daemon/src/services/agents/delegation/tests/run.rs`
- `docs/NATIVE-AGENTS.md`
- `docs/decisions/0017-native-subagents.md`

**Starts after:** T01 has committed.

**Steps — PATH (item 1):**
- Add a `path_prepend: Option<PathBuf>` (name it as you like; be explicit) to `CreateOptions` in
  `crates/fleet-daemon/src/services/agents/manager/commands.rs` (~27) and to `StartRequest` in
  `crates/fleet-core/src/agents/provider.rs` (~65). `StartRequest` is serialized but appears
  nowhere in `fleet-proto`, so mark it `#[serde(default, skip_serializing_if = "Option::is_none")]`
  and move on; there is no golden to regenerate.
- Do **not** smuggle this through `extra_env`. `extra_env` becomes a whole-value override in both
  adapters, so putting a `PATH` there would replace the login shell's `PATH` rather than extend
  it. A separate field is the point.
- In `crates/fleet-daemon/src/services/agents/delegation/run.rs` (~146, where `FLEET_DELEGATION`
  and `FLEET_DELEGATION_TOKEN` are built), resolve the directory to inject, in this order:
  1. the parent directory of the `fleet_path` T01 put on the request, **if that directory exists
     on this daemon's host** — this is the same-host case and covers the reported symptom;
  2. otherwise a sibling `fleet` next to the daemon's own `std::env::current_exe()`, if that file
     exists. Note the gotcha: the daemon's `current_exe` is `fleetd`, so you want
     `parent.join("fleet")`, and you must check `is_file()` before using the parent directory.
     `resolve_daemon_path` in `crates/fleet-client/src/spawn.rs` (~182) is the precedent for this
     shape;
  3. otherwise `None` — inject nothing and let bare `fleet` resolve or not, as today.
- Add a small, tested helper in `crates/fleet-daemon/src/agents/harness/process.rs` that prepends
  a directory to a `PATH` value in an environment map, using `std::env::join_paths` /
  `split_paths` so separators are not hand-rolled. It must be idempotent: prepending a directory
  already first in `PATH` leaves the value unchanged.
- Call that helper from `ClaudeHarness::environment` (`crates/fleet-daemon/src/agents/claude/mod.rs`
  ~113) and `CodexHarness::environment` (`crates/fleet-daemon/src/agents/codex/mod.rs` ~129),
  **after** `process::filter_environment` returns, so the prepend applies to the final merged
  environment and the `filter_environment` signature stays stable.
- Log once, at debug or info, which directory was injected and which of the three branches chose
  it. A child that cannot report is the single worst failure mode this feature has; make it
  diagnosable from the daemon log.

**Steps — footer (item 1):**
- `FOOTER_TEMPLATE` in `crates/fleet-daemon/src/services/agents/delegation/footer.rs` (~9) hard-codes
  bare `fleet subagent complete` in three places. Parameterise the program token: `footer()` (~29)
  takes the resolved absolute path when one exists and the literal `fleet` when it does not.
  Keep the rest of the template verbatim — the file's own doc comment says every string here is
  contract text with a verbatim test, so honour that and update the tests to match rather than
  loosening them.
- `first_message` (~38) passes it through from `run.rs`.

**Steps — warning (item 4):**
- `RequestBody::DelegationRun` already distinguishes an explicit worktree from a default:
  `worktree: Option<WorktreeId>`, and `run.rs` (~125) does
  `request.worktree.clone().unwrap_or_else(|| caller_record.worktree.clone())`. **No proto change
  and no golden regeneration is needed for this item.** Capture `request.worktree.is_some()`
  before the `unwrap_or_else` consumes it.
- At `run.rs` ~268, emit `SAME_WORKTREE_WARNING` only when the resolved worktree equals the
  caller's **and** the caller did not pass `--worktree`. An explicit `--worktree` that happens to
  name the caller's own worktree is a deliberate choice — orchestrators with disjoint file
  ownership do exactly this — and gets no warning.
- Reword `SAME_WORKTREE_WARNING` (~24) so it names that pattern briefly: it should still say the
  child edits the caller's worktree and still offer both remedies (end your turn, or pass
  `--worktree`), and should make clear the warning is about the *implicit default*. Keep it one
  line; it is appended to human output and carried in the JSON `warning` field.

**Steps — docs:**
- `docs/NATIVE-AGENTS.md` §15: update the footer text block (~1790) to show the resolved-path
  form and say when the bare name is used instead; rewrite the same-worktree warning section
  (~1812) to state the explicit-`--worktree` exemption and quote the new wording; add a short
  paragraph, authoritative, on how `fleet` reaches the child's `PATH` and the three-step
  resolution order including the remote-host fallback.
- **Also in `docs/NATIVE-AGENTS.md`, write the §15 changes whose code lands in T03** (see
  *Concurrency and file ownership*): the `wait` line at ~1775 must stop saying Fleet "refuses a
  larger timeout" and must instead state the 540 s default, the absence of an upper bound, the
  distinct non-terminal message on timeout, exit code 2, and that the report body is returned on
  success in both human and JSON output; the effort table row at ~1008 and the §15 `run` copy
  must mention `--effort` and that it is accepted without `--model`; and §15 must state
  `fleet agent tail`'s `--no-follow` and `--last <N>` behaviour.
- `docs/decisions/0017-native-subagents.md`: at ~127 record that an explicit `--worktree` is
  treated as intent and suppresses the warning; at ~147 replace the "capped at 540" decision with
  the new one — 540 stays the default because it matches the Claude Code shell-tool ceiling, but
  Fleet no longer imposes a ceiling of its own, and the caller's own tool timeout may still kill
  a longer wait. Write it as an amendment in the ADR's own voice, do not delete the original
  reasoning.

**Regression tests:**
- In `crates/fleet-daemon/src/agents/harness/process.rs`'s test module (~449): the prepend helper
  puts the directory first, preserves the rest of `PATH` in order, is idempotent, and handles a
  missing `PATH` key.
- In `crates/fleet-daemon/src/services/agents/delegation/footer.rs`'s test module (~102): the
  footer renders the absolute path verbatim when one is given and the bare `fleet` when it is
  not, asserted against the full template both ways.
- In `crates/fleet-daemon/src/services/agents/delegation/tests/run.rs`: **three** cases — an
  implicit same-worktree run returns the warning; a run passing `--worktree` naming the caller's
  own worktree returns **no** warning; a run passing a different worktree returns no warning.
- In the same file: a case asserting the child's `CreateOptions` carries the expected
  `path_prepend` derived from the request's `fleet_path`, and one asserting it is `None` (or the
  daemon-sibling fallback) when the request's path does not exist.

**Verification:**
- `cargo test -p fleet-core -p fleet-daemon`
- `cargo test -p fleet-daemon delegation`
- `cargo clippy -p fleet-core -p fleet-daemon --all-targets --all-features -- -D warnings`
- `make lint`
- `make restart` (daemon code changed), then `make doctor` and read the `subagent fleet CLI` line.

**Done when:** a child spawned by `fleet subagent run` can execute bare `fleet subagent complete`
with no absolute path in its brief; the footer it reads names a path that actually works; and a
caller passing `--worktree` gets no warning while a caller relying on the default still does.

---

### T03 — CLI: uncap `wait` and report a running timeout honestly, add `--effort`, add `tail --no-follow` and `--last`

**Intent:** fix reported items 2, 3 and 5, all of which are `fleet-cli` surface.

**Skill to load:** `rust-workspace-architecture` (CLI args, error types, output shape) and
`rust-gpui-testing` (for the test work). Load `zed-quality-review` before declaring done.

**Owns:**
- `crates/fleet-cli/src/args.rs`
- `crates/fleet-cli/src/commands/subagents.rs` *(inherited from T01)*
- `crates/fleet-cli/src/commands/agents.rs`
- `crates/fleet-cli/src/human.rs`
- `crates/fleet-cli/src/commands/tests.rs` *(inherited from T01)*
- `docs/research/agents-contracts.md`
- `README.md`

**Starts after:** T01 has committed. Its final doc cross-check step additionally waits for T02.

**Steps — item 2, `subagent wait`:**
- In `crates/fleet-cli/src/args.rs`, `SubagentWaitArgs` (~444): keep `default_value_t = 540`,
  drop `value_parser = clap::value_parser!(u64).range(..=540)`. The cap exists only in clap — the
  proto carries an unrestricted `u64` and `crates/fleet-client/src/connection.rs` (~672) already
  grants `timeout_ms + 15 s`, so nothing below the CLI enforces it and nothing below needs to
  change. Confirm that while you are here.
- Rewrite the `--timeout` doc comment (it is the help text) to say: the default is 540 seconds,
  chosen to sit under the Claude Code shell-tool ceiling; larger values are accepted but the
  caller's own tool timeout may still kill the wait; a timeout exits 2 and a terminal record
  exits 0. Also state on the `wait` subcommand itself that **the child's report body is returned
  on success** — it already is, in both human output and the JSON envelope's `result.text`, and
  the orchestrator did not discover it.
- In `crates/fleet-cli/src/commands/subagents.rs`, `wait` (~187): `human::delivered_message` is
  the *terminal* template and is what produces the nonsense `finished: running`. Branch on
  `delegation.status.is_terminal()` — which the function already computes — and on the
  non-terminal path print a distinct single line naming the delegation, its current status and
  the elapsed wait, e.g. `[fleet subagent <id> still running after <n>s]`. Exit code stays 2.
- **JSON output is unchanged.** The `--json` branch already emits the envelope regardless of
  status; do not add a field, do not change a word. Add a test that pins this.
- If a shared formatting helper is the tidy way to build that line, it belongs in
  `crates/fleet-cli/src/human.rs` next to `delivered_message` (~213), which you own.

**Steps — item 3, `subagent run --effort`:**
- Add `--effort <EFFORT>` to `SubagentRunArgs` in `crates/fleet-cli/src/args.rs` (~390) as an
  `Option<String>`. No `value_enum`: the ladder is provider- and model-specific and published by
  the harness catalogue (`docs/NATIVE-AGENTS.md` ~994–1011). Say so in the help text and say that
  it may be passed without `--model`.
- In `crates/fleet-cli/src/commands/subagents.rs`, `model_selection` (~265) currently takes only
  the model string and hardcodes `effort: None`. Rework it to take both, and handle the four
  combinations: neither (send no `ModelSelection`), model only (today's behaviour), both, and
  **effort only** — which must still produce a `ModelSelection`, with the model left as the
  provider default. Check what `ModelSelection.model: String`
  (`crates/fleet-core/src/agents/state.rs` ~201) means when no model was named and pick the
  representation that `create_with`
  (`crates/fleet-daemon/src/services/agents/manager/commands.rs` ~160) already handles correctly;
  it fills `effort` from defaults only when the selection carries none, so an explicit effort
  survives either way. If effort-only cannot be expressed without a model, say so in the tracker
  and reject `--effort` without `--model` with a clear validation error instead of silently
  dropping it — but try the former first.
- Apply the same empty-string validation the existing `model_selection` applies to the model.
- No proto, client or store change: `ModelSelection.effort` is already on the wire and already
  present in the populated golden at `crates/fleet-proto/tests/agent_compatibility.rs` ~799.
  Confirm that and note it in the commit body.

**Steps — item 5, `fleet agent tail`:**
- Add `--no-follow` and `--last <N>` to `AgentTailArgs` in `crates/fleet-cli/src/args.rs` (~547).
- In `crates/fleet-cli/src/commands/agents.rs`, `tail_to` (~128): today, without `--replay`, the
  retained events are applied through `advance` and only future events print — so a `timeout`-
  killed tail with no new events prints nothing at all, which is the reported symptom.
  `--no-follow` prints the retained snapshot and exits 0 without entering the follow loop. It
  **implies** replay; do not make the user pass both, and make the implication explicit in help
  text rather than silent.
- `--last <N>` limits the replayed snapshot to the last N events. It is a **client-side** trim of
  what the cursored open already returned — do not add paginated history to the protocol. Decide
  and document whether `--last` also applies when following (recommendation: it trims the replay
  only, and everything live still prints) and whether `--last` implies replay (recommendation:
  yes, same as `--no-follow`; a `--last` that printed nothing would repeat the bug).
- Leave the existing cursored open (`Some(Seq(0))`) alone — the comment at ~136 explains that a
  tail must never trigger the §6 lazy resume, and that is still true.
- Output stays one JSON `SeqEvent` per line, flushed per line, with the existing `BrokenPipe`
  handling in `print_tail` (~207) untouched.

**Steps — docs:**
- `docs/research/agents-contracts.md`: at ~778 update the `fleet agent` verb list to
  `tail <THREAD> [--replay] [--no-follow] [--last N]`; at ~785 add `[--effort E]` to the `run`
  flag list and state the no-`--model` rule; at ~793 replace "caps the flag at 540" with the new
  contract (540 default, no upper bound, timeout exits 2 with a distinct non-terminal message,
  terminal exits 0, the report body is returned on success).
- `README.md` (~101): update the `fleet agent tail` row for the two new flags, and the
  `fleet subagent` rows for `--effort` and the `wait` timeout wording. Keep the table's existing
  column shape and voice.
- **Last step, after T02 has committed:** re-read `docs/NATIVE-AGENTS.md` §15 and confirm the
  paragraphs T02 wrote for items 2, 3 and 5 match what you actually shipped. If they diverge, fix
  the doc — the file is yours to edit at that point. Record in the tracker whether it needed a
  fix.

**Regression tests:** all in `crates/fleet-cli/src/commands/tests.rs`.
- `wait` with a non-terminal delegation produces the new `still running` line, not a line
  containing `finished:`, and exits 2.
- `wait` with a terminal delegation still produces the exact `delivered_message` text and exits 0.
- `wait --json` produces identical bytes for a non-terminal delegation before and after this
  change — pin the literal.
- Parsing `--timeout 3600` succeeds (it is the regression that proves the cap is gone) and
  `--timeout 0` still behaves as today.
- `--effort high` alone, `--model X --effort high`, and `--model X` alone each produce the
  expected `ModelSelection` on the built request; `--effort ""` is a validation error.
- `tail --no-follow` on a thread with retained events and no live events prints those events and
  exits 0 — this is the exact reported failure and the case that must not regress.
- `tail --no-follow --last 2` prints exactly the last two retained events.

**Verification:**
- `cargo test -p fleet-cli`
- `cargo clippy -p fleet-cli --all-targets --all-features -- -D warnings`
- `make lint`
- Manual: `fleet subagent wait <live-id> --timeout 5` shows the running line and exits 2;
  `fleet agent tail <thread> --no-follow --last 20` prints and returns.

**Done when:** `wait` can be given any timeout and never claims a running child finished,
`--effort` reaches the provider, and `tail --no-follow` prints something useful and exits.

---

### T04 — Report the directory `fleet doctor` would inject for subagents

**Intent:** `check_subagent_fleet` only answers "does bare `fleet` resolve through the login
shell". After T02 the answer can be yes *because Fleet injected a directory*, and doctor should
say which one — otherwise the one diagnostic aimed at this failure mode is now misleading.

**Skill to load:** `rust-workspace-architecture`. Load `zed-quality-review` before declaring done.

**Owns:**
- `crates/fleet-daemon/src/services/doctor.rs`
- `crates/fleet-daemon/tests/doctor_checks.rs`
- `docs/DEVELOPMENT.md`

**Starts after:** T01 has committed. Deliberately does **not** depend on T02's helper — see below.

**Steps:**
- `subagent_fleet_check` (~460) currently reports `Ok` with the resolved path or `Fail` with
  advice to put `fleet` on `PATH`. Extend it so it also names the directory this daemon would
  prepend for a child, computed daemon-side only: a sibling `fleet` next to the daemon's own
  `std::env::current_exe()` (remember `current_exe` is `fleetd`, so join `"fleet"` onto the parent
  and check `is_file()`).
- Do **not** call into T02's process-level helper, and do not try to report the caller-supplied
  path: doctor has no delegation in flight and no caller, so it can only speak about the
  daemon-side fallback. Keeping the two independent is what lets T02 and T04 run in parallel.
  State this limit in the check's detail text or a code comment so nobody later mistakes it for a
  complete answer.
- Rework the failure copy: with the fallback in place, a hard `Fail` is only correct when bare
  `fleet` does not resolve **and** there is no sibling to inject. When a sibling exists but bare
  `fleet` does not resolve through the login shell, that is a pass with an explanation, not a
  failure — a child will still be able to report.
- Keep the check name `subagent fleet CLI` stable; `docs/DEVELOPMENT.md` (~87) tells developers to
  look for it by name and `crates/fleet-daemon/tests/doctor_checks.rs` asserts on it.
- Update `docs/DEVELOPMENT.md` (~87) to describe the new output, including the injected-directory
  line and the fact that a child inherits it automatically.

**Regression test:** in `crates/fleet-daemon/tests/doctor_checks.rs`, cases for the three shapes —
bare `fleet` resolves (Ok, names the resolved path); bare `fleet` does not resolve but a sibling
exists (Ok or Warn, names the injectable directory, does **not** tell the user to fix their PATH);
neither (Fail, keeps the actionable advice). The middle case is the new behaviour and the one that
must not regress.

**Verification:**
- `cargo test -p fleet-daemon doctor`
- `cargo clippy -p fleet-daemon --all-targets --all-features -- -D warnings`
- `make lint`
- `make restart` then `make doctor`, and read the `subagent fleet CLI` line.

**Done when:** `make doctor` tells a developer not just whether a child can find `fleet` but
which directory makes that true.

---

### T05 — Verify the batch end to end and record what is still deferred

**Intent:** prove the five fixes hold together in a real orchestrator run, and leave
`SESSION_TODO.md` honest.

**Skill to load:** `zed-quality-review`, then `rust-gpui-testing` if a gap needs a new test.

**Owns:**
- `SESSION_TODO.md`

**Starts after:** T02, T03 and T04 have all committed.

**Steps:**
- Run the full gate on a clean tree: `make lint`, then `make test`, then `make restart`.
- `make harness` is **not** required — nothing in this batch renders. Write that sentence in the
  tracker under Notes rather than leaving the omission unexplained.
- Read the final diff across all four preceding tasks as one change. Specifically check the three
  seams a concurrent execution is most likely to have broken: (a) the `fleet_path` field is
  produced by T01 and consumed by T02 with no leftover dead code; (b) `docs/NATIVE-AGENTS.md`
  §15 matches what T03 shipped for items 2, 3 and 5; (c) no two tasks left conflicting copy in
  `docs/research/agents-contracts.md` and `docs/NATIVE-AGENTS.md` about the `wait` timeout.
- Run the field check `SESSION_TODO.md` already asks for: an orchestrator with four concurrent
  children in one worktree, confirming that no brief needs an absolute `fleet` path, that one
  `wait` per child is enough, that an explicit `--worktree` produces no warning, and that
  `fleet agent tail --no-follow --last 20` is usable for peeking at a child.
- Update `SESSION_TODO.md`: tick the "Verify after the first batch lands" box or record exactly
  what failed, and confirm the four deferred items are still stated correctly (cost visibility,
  repeatable `--env`, end-of-turn delivery policy, and the `run --json` brief echo).
- Anything the field check surfaces that is not in this plan goes under Follow-ups in the tracker
  and, if it deserves to outlive this batch, into `SESSION_TODO.md`. Do not fix it here.

**Regression test:** none new of its own; this task's product is the green gate plus the recorded
field-check result. If the field check finds a behaviour no test covers, add that test to
whichever crate owns the behaviour and say so in the tracker.

**Verification:**
- `make lint`
- `make test`
- `make restart`
- `make doctor`

**Done when:** the workspace is green, a live four-child orchestrator run shows all five symptoms
gone, and `SESSION_TODO.md` reflects reality.

## Verification

Project-wide, run from the repo root:

```sh
make lint      # cargo fmt --all -- --check + cargo clippy --workspace --all-targets --all-features -- -D warnings
make test      # builds fleetd/fleet/fleet-harness, then cargo test --workspace
make restart   # rebuild and restart the running fleetd (required: daemon code changed)
make doctor    # environment diagnostics; check the `subagent fleet CLI` line
```

`make harness` and `make harness-headless` are **not** required for this batch — no screen,
dialog, keymap or token changes. Record that decision rather than silently skipping it.

Per-crate, while iterating:

```sh
cargo test -p fleet-proto
cargo test -p fleet-client
cargo test -p fleet-cli
cargo test -p fleet-core -p fleet-daemon
cargo test -p fleet-daemon delegation
cargo test -p fleet-daemon doctor
cargo clippy -p <crate> --all-targets --all-features -- -D warnings
```

## Definition of done

- [ ] Every task T01–T05 is ticked in the tracker, with its verification output or a one-line
      "verified: <how>" pasted beside it.
- [ ] `make lint` is clean (fmt and clippy, `-D warnings`, whole workspace).
- [ ] `make test` passes on the whole workspace.
- [ ] `make restart` has been run and the live `fleetd` matches the build.
- [ ] `make doctor` shows a passing `subagent fleet CLI` check naming a real directory.
- [ ] No `unwrap`, `todo!`, `unimplemented!`, `dbg!` or `TODO` was added to production code, and
      no fallible call was discarded with `let _ =`.
- [ ] `docs/NATIVE-AGENTS.md`, `docs/decisions/0017-native-subagents.md`,
      `docs/research/agents-contracts.md`, `docs/DEVELOPMENT.md` and `README.md` describe the
      shipped behaviour, with no leftover claim that `wait` is capped at 540.
- [ ] Commits follow `<area>: <imperative lowercase summary>` with areas drawn from `proto`,
      `client`, `cli`, `core`, `daemon`, `docs`.
- [ ] The tracker reflects reality, including anything that was skipped and why.
- [ ] Follow-ups discovered mid-flight are captured in the tracker and, where they outlive this
      batch, in `SESSION_TODO.md`.

## Risks and rollback

- **Concurrent agents clobbering a shared file.** The highest-probability failure in this batch,
  and the reason for the ownership table. If it happens, the symptom is a task's change silently
  vanishing rather than a conflict marker. Mitigation: each task greps for its own change before
  ticking its box. Rollback: re-apply the lost edit from the task's own commit.
- **PATH injection changes behaviour for non-delegated threads.** The prepend must apply only
  when `path_prepend` is set, which only delegation sets. If a normal `fleet agent new` thread's
  environment changes, that is a bug. T02's tests must include the `None` case.
- **Injecting a path from another host.** A caller on machine A sending its `current_exe` to a
  daemon on machine B would inject a directory that does not exist there, or worse, one that
  exists and holds an unrelated binary. T02's step 1 requires the directory to exist on the
  daemon host before using it; the fallback covers the remote case, where `bootstrap.rs` installs
  only `fleetd`.
- **Footer text is contract text.** `footer.rs` documents that every string there is asserted
  verbatim. Changing the template risks breaking a child's parsing or a scenario's expectation.
  Mitigation: change the program token only, keep the tests verbatim rather than loosening them.
- **Removing the `wait` cap lets a caller hang past its own tool timeout.** That is the caller's
  timeout to manage and the reason the 540 default stays; the help text and the ADR amendment
  must both say so. Rollback: restore the `range(..=540)` parser, a one-line revert.
- **`--effort` with no `--model`.** If `ModelSelection` cannot represent "default model, explicit
  effort", T03 falls back to rejecting the combination with a clear error. That is a documented
  degradation, not a silent drop. Decide it in T03 and record it in the tracker.
- **Rollback overall:** every task is one commit against `fix/native-subagents` with no schema
  migration and no store change. Any single item can be reverted independently, with one
  exception: reverting T01 requires reverting T02 first, since T02 reads the field T01 adds.
