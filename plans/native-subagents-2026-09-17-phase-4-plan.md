# Native subagents, phase 4: app rows, child tabs and attach — Plan
> Tracker: ./native-subagents-2026-09-17-phase-4-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Show delegations in the app without adding a pane, a sidebar or automatic tabs. The caller's
transcript gets one live `DelegationRow` per child (status word, status mark, the child's latest
headline, elapsed time, `⏎ attach`) and a `DelegationResultCard` for the delivered report. A child
thread stays out of the tab strip until a human attaches it from the row or the card; once attached it
is an ordinary agent tab titled `↳ codex — <title>` with a pinned metadata segment naming its caller,
`^s u` to go up, and `^s x` to detach rather than close. A child that needs a human paints its
caller's tab, so the existing toast and sound fire with no new notification code. The GUI harness
proves every one of those, which first requires teaching the scripted provider to run a shell
command, because that is the only way a scripted caller can spawn a real delegation.

## Sizing call

**Phased, phase 4 of 7.** See ./native-subagents-2026-09-17-roadmap.md. This phase is the largest
app change of the initiative and its own pull request. It depends on phase 3's daemon behaviour and
on nothing later. It is not split further because the row, the attach model and the child chrome
are one user-visible feature that the harness scenarios exercise together.

## Repository context

- Rust workspace; this phase touches `fleet-ui-kit`, `fleet-app`, `fleet-harness`, `scenarios/`
  and `docs/`. No daemon change.
- Lint: `make lint`. Test: `make test`. Harness: `make harness` (whole corpus, virtual lane) and
  `make harness-one SCENARIO=scenarios/agents/<name>.scenario`. Any screen change is verified by
  `make harness`, not `make test` alone.
- Kit rows: `crates/fleet-ui-kit/src/components/agent/rows.rs` (`TranscriptRowKind` at line 410
  with eighteen variants; `SubagentRow` at 426 is the closest sibling), `rows/render.rs`,
  `rows/message.rs`, `rows/work.rs`, `rows/chrome.rs`; the fold the tool rows use lives with
  `tool_row.rs`. `MetadataSegment` in `metadata_row.rs:28` (`text`, `width`, `collapsible`).
  Galleries: `examples/gallery_agent.rs`, `examples/gallery_terminal.rs`,
  `examples/support/agent_rows.rs`. The kit test `components/agent/tests.rs` (`every_row_kind`
  at line 27, `every_row_kind_draws`, `every_component_state_draws`) fails when a variant is not
  drawn.
- App agent state: `crates/fleet-app/src/state/agents.rs` (`closed` set at line 72,
  `of_worktree` at 98, `close` at 116, `attention` at 156, `counts` at 175, `is_working` at 309)
  and `state/agents/tests.rs`. Strip: `crates/fleet-app/src/views/workspace_tabs.rs` and the kit's
  `terminal_tab_strip.rs`. Attention marks: `state/notifications.rs`, `shell/chrome.rs`.
- Transcript projection: `crates/fleet-app/src/screens/agent_thread/rows/{item.rs, turn.rs,
  live.rs, group.rs, fold.rs, mod.rs}` memoised behind a `RowsKey`; tests in
  `screens/agent_thread/tests/{rows.rs, fixtures.rs, composer.rs}`; `presentation.rs` for the
  header word and metadata row.
- Actions and keys: `crates/fleet-app/src/actions.rs`, `keymap.rs` (`Workspace > Prefix` rows from
  line 568 are repeated inside every agent sub-mode; `Agent > AgentNativeScroll > AgentRow` rows at
  715 to 719 bind `enter`, `u`, `o`, `y`, `d`; `x` is free there). Confirm dialog:
  `dialogs/confirm.rs`.
- Bridge and events: `crates/fleet-app/src/bridge/`, `state/agents.rs` handles the
  `AgentSummary` family; `DelegationChanged` joins it (phase 2).
- Harness: `crates/fleet-harness/src/agent/{launcher.rs, transcript.rs, claude.rs, codex.rs}`
  (scripted provider), `fixture/{seed.rs, tools.rs, plan.rs}` (the `agents` preset installs the
  scripted provider under the vendor's name), `crates/fleet-app/src/state/harness/projection.rs`
  (`UiSnapshot` version 1: `agents.threads[]`, `tabs`, `lists.tabs`), `crates/fleet-app/src/drive/`.
  `docs/TESTING-HARNESS.md` is frozen; §3 and §5 allow additive optional data; §2 (the line
  grammar) and existing target names are untouched.
- Skills: `gpui-components` (rows, segment), `gpui-styling` (tones, icons, spinner),
  `gpui-state-and-memory` (new state, subscriptions), `gpui-performance` (row memo, headline
  updates), `gpui-app-shell` (actions, keys, confirm dialog), `rust-gpui-testing` and
  `docs/TESTING-HARNESS.md` (scenarios), `zed-quality-review` at the end.
- Docs: `docs/UX-SPEC.md` §3.6, §3.6.0, §9 component inventory; `docs/KEYMAP.md` "Native agent
  thread" and "Workspace (Native mode)"; `docs/NATIVE-AGENTS.md` §2, §11, §12, §13, §15;
  `docs/TESTING-HARNESS.md` §3, §5; `scenarios/agents/README.md`.

## Assumptions

- **Scripted shell step.** The transcript gains an additive step
  `{"type":"shell","command":"<sh -c string>","name":"Bash"}` that the scripted provider runs
  with its own process environment (so `FLEET_SESSION`, `FLEET_DELEGATION` and the token are
  visible), presents to Fleet as an ordinary tool call named `name` with the command's stdout as
  its output, and waits for it to exit before the next step. §5's "optional data may be added"
  rule covers it; the frozen line grammar of §2 does not change.
- **Role-based transcript.** The `agents` preset's launcher chooses the transcript by role: when
  `FLEET_DELEGATION` is set in its environment it plays `subagent-child.json`, else the existing
  starter. Selection is by environment, not by provider, so a Claude caller can spawn a Codex child
  and both are scripted.
- **`fleet` on the child's `PATH`.** The preset's tool shim directory (where the vendor-named
  scripted provider is installed) also gets a `fleet` shim pointing at `FLEET_APP`, which is the
  same binary that serves the CLI.
- **Snapshot additions** are optional fields under existing objects: `agents.threads[].parent`,
  `agents.threads[].attached`, and a new `agents.delegations[]` list of `{id, status, caller,
  child, delivery, headline}`. Tab rows gain `child: bool`. No existing name changes.
- **Delegation state in the app** is a map keyed by `DelegationId`, seeded with `DelegationList`
  after every `Hello` when the daemon advertises `agent.delegation`, and updated by
  `DelegationChanged`. Against a daemon without the capability the map stays empty and every
  delegation-specific surface is absent, which is what an older daemon deserves.
- **Attention bubbling** is derived in `attention()` from the child summaries, memoised per
  summary revision, never stored. `counts()` counts children under the caller's word.
- **Status tones** reuse the tab strip's: gray for working, amber for needs-you, the existing
  success and failure tones for done and failed. No new token.

## Out of scope

- The agents picker and `^s d`. Phase 5.
- Cross-worktree attach through a session switch. Phase 5.
- Any daemon behaviour, including cancel propagation.

## Affected areas

- `crates/fleet-ui-kit/src/components/agent/{rows.rs, rows/render.rs, rows/delegation.rs (new),
  metadata_row.rs, tests.rs}`, `crates/fleet-ui-kit/examples/{gallery_agent.rs,
  gallery_terminal.rs, support/agent_rows.rs}`, `crates/fleet-ui-kit/src/components/terminal_tab_strip.rs`.
- `crates/fleet-app/src/state/agents.rs`, `state/agents/tests.rs`, `state/notifications.rs`,
  `bridge/`, `views/workspace_tabs.rs`, `screens/agent_thread/{rows/item.rs, rows/mod.rs,
  presentation.rs, tests/}`, `actions.rs`, `keymap.rs`, `dialogs/confirm.rs`,
  `state/harness/projection.rs`, `shell/chrome.rs`.
- `crates/fleet-harness/src/agent/{transcript.rs, launcher.rs, claude.rs, codex.rs, tests.rs}`,
  `fixture/{tools.rs, seed.rs}`, transcript JSON files under the fixture's directory.
- `scenarios/agents/subagent-*.scenario`, `scenarios/agents/README.md`.
- `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/NATIVE-AGENTS.md`, `docs/TESTING-HARNESS.md`.

## Tasks

### P4-T01 — Teach the harness to spawn a real delegation from a scripted caller
- **Intent:** Make a subagent scenario possible before any UI is written, within the frozen
  contract's additive rules.
- **Touches:** `crates/fleet-harness/src/agent/{transcript.rs, claude.rs, codex.rs, launcher.rs,
  tests.rs}`, `fixture/{tools.rs, seed.rs}`, new transcripts `subagent-caller.json` and
  `subagent-child.json`, `docs/TESTING-HARNESS.md` §5, `crates/fleet-app/src/state/harness/projection.rs`,
  `docs/TESTING-HARNESS.md` §3.
- **Steps:**
  - Add the `shell` step to the transcript model; both players run it via `sh -c` with the
    inherited environment, stream stdout as the tool output, and continue on exit. Unit-test the
    step in `agent/tests.rs` with a command that echoes an environment variable.
  - Launcher: select `subagent-child.json` when `FLEET_DELEGATION` is present. Install a `fleet`
    shim beside the vendor shims in `fixture/tools.rs`.
  - `subagent-caller.json`: one text step, a `shell` step that writes a brief to a temp file and
    runs `fleet subagent run --provider codex --expect "the path" --brief-file <file>`, a text step
    "delegated, waiting", `end_turn completed`; then a second turn (the delivered result) with one
    text step and `end_turn`. `subagent-child.json`: one text step, a `shell` step that writes a
    report and runs `fleet subagent complete --result-file <file>`, `end_turn completed`. A second
    child transcript, `subagent-child-blocked.json`, opens a question gate instead, for the
    blocked scenario; select it by a second environment variable the scenario's brief cannot set,
    so use a different **provider** for that one (Claude child asks, Codex child completes) and
    document the pairing in the fixture's README.
  - Snapshot: add the optional fields listed in Assumptions; keep every existing field and name.
  - `docs/TESTING-HARNESS.md`: §5 gains the `shell` step and the role rule; §3 gains the new
    optional fields. Nothing in §2 changes.
- **Verification:** `cargo test -p fleet-harness`, `cargo test -p fleet-app harness`, then a
  throwaway scenario that runs the caller transcript and asserts
  `agents.delegations[0].status == succeeded` before any UI exists.
- **Done when:** The throwaway scenario passes headless and virtual, proving a scripted caller can
  spawn and finish a real delegation through the private daemon.

### P4-T02 — `DelegationRow` and `DelegationResultCard` in the kit
- **Intent:** Add the two row kinds with every state drawn in the gallery.
- **Touches:** `crates/fleet-ui-kit/src/components/agent/{rows.rs, rows/render.rs,
  rows/delegation.rs, tests.rs}`, `examples/gallery_agent.rs`, `examples/support/agent_rows.rs`.
- **Steps:**
  - `DelegationRow { provider_glyph, title, status: DelegationRowStatus, headline, elapsed,
    hint }` where the status enum is the kit's own (`Starting, Working, Blocked, Done, Incomplete,
    Failed, Cancelled`) and takes no domain type. Slots: leading `↳` plus provider glyph; title;
    status word; one status mark (gray spinner, amber dot, `circle-check`, `circle-x`); second line
    headline; trailing elapsed and `⏎ attach`. Never grouped, never folded while live: add it to
    the fold exemption table beside `Subagent`.
  - `DelegationResultCard { header, body: Markdown, expanded }`: header
    `↳ codex finished · done · 14m 02s · 6 files`; the body collapsed to eight lines with the same
    fold affordance the tool rows use; the same `attach` hint.
  - `MetadataSegment` gains `target: Option<SharedString>` (an opaque id the app maps to a thread)
    so a segment can be a jump; render with the link tone and the focus ring the kit already has.
  - Gallery: every `DelegationRowStatus`, both card states, and a metadata row with a target
    segment. Extend `every_row_kind` and `every_component_state_draws`.
- **Verification:** `cargo test -p fleet-ui-kit`, `cargo run -p fleet-ui-kit --example gallery_agent`
  looked at once, `make lint`.
- **Done when:** The kit test draws every new state and the gallery shows them.

### P4-T03 — Delegation state, the `attached` set and `reopen` in the app
- **Intent:** Hold the daemon's delegations and the per-window attach choice without touching the
  caller's projection.
- **Touches:** `crates/fleet-app/src/state/agents.rs`, `state/agents/tests.rs`, `bridge/`,
  `state/harness/projection.rs`.
- **Steps:**
  - `delegations: HashMap<DelegationId, Delegation>` with `by_caller(thread)` and
    `by_child(thread)` accessors, a `delegations_revision: u64` bumped on every change for the
    row memo, seeded from `DelegationList` after `Hello` when the capability is advertised and
    updated from `DelegationChanged`.
  - `attached: HashSet<ThreadId>`; `attach(thread)` (removes from `closed` too),
    `detach(thread)`, `reopen(thread)`. `of_worktree` returns a thread with `summary.parent` set
    only when attached; a thread without a parent keeps today's rule. Order: callers in daemon
    order, each followed by its attached children in creation order.
  - `cx.notify` once per event batch, not per delegation; one `Subscription` field.
  - Tests: a child is absent from `of_worktree` until attached; detach hides it; `reopen` clears
    `closed`; order is caller then children; the revision bumps on `DelegationChanged`.
- **Verification:** `cargo test -p fleet-app state::agents`, `make lint`.
- **Done when:** The five tests pass and `of_worktree` is the only place that decides strip
  membership.

### P4-T04 — Project the delegation row and the result card into the transcript
- **Intent:** Replace phase 2's two fallbacks with the real rows, joined to the delegation record.
- **Touches:** `crates/fleet-app/src/screens/agent_thread/rows/{item.rs, mod.rs, fold.rs}`,
  `screens/agent_thread/tests/{rows.rs, fixtures.rs}`, `actions.rs`, `keymap.rs`,
  `dialogs/confirm.rs`.
- **Steps:**
  - `ItemKind::Delegation` joins `delegations.get(id)`: status word and mark from
    `Delegation.status`, headline, elapsed from `created` to `finished` or now, title from the
    child summary's title or the brief's first line. Include `delegations_revision` in `RowsKey` so
    a `DelegationChanged` rewrites one row and re-runs no grouping; the caller's projection is
    untouched.
  - `UserMessage { origin: Delegation { id } }` becomes `DelegationResultCard` with the header
    from the record and the body from the message text, collapsed, expanded per the existing
    per-row expand state.
  - Row keys on `Agent > AgentNativeScroll > AgentRow`: `enter` on a delegation row or card →
    `native_agent::AttachChild` (attach and select); `x` → `native_agent::CancelDelegation` behind
    the confirm dialog, sending `DelegationCancel`; `y` on a delegation row copies the id (extend
    `CopyRow`). `ExpandRow` stays on `enter` for every other row; disambiguate in the handler by
    row kind.
  - Tests: row projection for each status; the card collapses at eight lines; `enter` on the row
    attaches; `x` opens the confirm and then sends the request; a `DelegationChanged` changes one
    row and leaves the `RowsKey` of every other row equal.
- **Verification:** `cargo test -p fleet-app agent_thread`, `make lint`.
- **Done when:** The two phase-2 fallbacks are deleted and every test above passes.

### P4-T05 — The child tab: title, caller segment, placeholder, `^s u`, `^s x` detaches
- **Intent:** Make a child an ordinary tab with exactly one piece of child-specific chrome.
- **Touches:** `crates/fleet-app/src/views/workspace_tabs.rs`,
  `crates/fleet-ui-kit/src/components/terminal_tab_strip.rs`,
  `screens/agent_thread/presentation.rs`, `screens/agent_thread/tests/composer.rs`, `actions.rs`,
  `keymap.rs`, the `prefix::CloseTerminal` handler.
- **Steps:**
  - Strip title `↳ <provider> — <title>` for a thread with a parent; the arrow is the only marker.
  - Metadata row: a pinned, non-collapsible leading segment `for [3] claude — design` with the
    caller's strip index (or `·` when the caller is not attached) and title, `target` = the caller
    thread; activating it selects the caller, attaching first if hidden.
  - Composer placeholder on a child: `Steering a subagent of [3]. It reports to its caller when it
    finishes.`
  - `^s u` → `prefix::UpToCaller`, bound in `Workspace > Prefix` and repeated in every agent
    sub-mode like the other prefix rows; on a non-child it is a no-op with the unbound-chord
    toast.
  - `^s x` on a child calls `detach`, not `close`; the thread keeps running. On a caller it closes
    as today.
  - Tests: title format; segment content with an attached and a hidden caller; `^s u` selects
    the caller; `^s x` on a child leaves the summary present and the tab gone.
- **Verification:** `cargo test -p fleet-app`, `make lint`.
- **Done when:** All four tests pass and no other child-specific chrome exists.

### P4-T06 — Attention bubbles up from children to the caller
- **Intent:** Paint the caller's tab and count children under its word, with no new notification
  code.
- **Touches:** `crates/fleet-app/src/state/agents.rs` (`attention`, `counts`, `is_working`),
  `state/agents/tests.rs`, `state/notifications.rs` (tests only), `shell/chrome.rs` if the
  header word needs the derived value.
- **Steps:**
  - `attention(caller)` = max(own, max over children of `needs_you`, then `working`), where a
    child's contribution is its own `attention_for` against this client's cursor. Memoise per
    summaries revision. A delivered result makes the caller `working` then `finished` through its
    own turn, as today.
  - `counts()` folds each child under its caller's word; `is_working(caller)` is true while any
    child is live.
  - Tests: a blocked child makes the caller `needs_you` and survives selecting the caller; a
    working child makes it `working`; a finished child contributes nothing; the notification edge
    test fires the existing toast on the caller when a child blocks.
- **Verification:** `cargo test -p fleet-app state`, `make lint`.
- **Done when:** The four tests pass and `notifications.rs` has no delegation-specific code.

### P4-T07 — Harness scenarios for the row, attach, detach, blocked child and `^s u`
- **Intent:** Verify every screen change with the real GUI, per the repo rule.
- **Touches:** `scenarios/agents/subagent-attach-from-row.scenario`,
  `subagent-detach-and-reattach.scenario`, `subagent-blocked-child-paints-caller.scenario`,
  `subagent-up-to-caller.scenario`, `subagent-result-card.scenario`, `scenarios/agents/README.md`,
  `scenarios/baselines/` for any new `shot`.
- **Steps:**
  - Each scenario: `fixture: agents`, open a workspace, start the caller (`^s a` or `^s A` per
    the transcript pairing), type a prompt, await `agents.delegations[0].status == working`, then
    the behaviour under test. Use `await` on snapshot fields, never `wait`.
  - Attach from row: `key enter` on the delegation row, assert the strip gained a `child: true`
    tab titled with `↳`, and `focused == agents.composer`.
  - Detach and re-attach: `^s x` on the child, assert the tab is gone and
    `agents.threads[n].attached == false`, `enter` on the row again, assert it is back.
  - Blocked child: the Claude child asks a question; assert the caller's tab mark is `needs_you`
    while the caller is selected and the row reads `blocked`.
  - `^s u`: from the child tab, assert the caller is selected.
  - Result card: await `agents.delegations[0].delivery == delivered`, assert the caller's last
    row is the card, `enter` expands it.
  - Add each to the README table with its document reference.
- **Verification:** `make harness-one SCENARIO=...` per file, then `make harness`.
- **Done when:** All five pass in the virtual lane and the ones without `shot` pass headless.

### P4-T08 — Update the four authoritative documents for the surfaces added
- **Intent:** Keep `docs/` authoritative for the row, the card, the child tab and the keys.
- **Touches:** `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/NATIVE-AGENTS.md`.
- **Steps:**
  - `UX-SPEC.md` §3.6 strip rows gain the child row and its order rule; §3.6.0 gains the
    delegation row, the result card, the caller segment and the placeholder; §9 component inventory
    gains `DelegationRow`, `DelegationResultCard` and the segment target; the intentionally-omitted
    list keeps the sidebar and the inspector.
  - `KEYMAP.md` "Native agent thread" gains `^s u`, and `⏎`, `x`, `y` on a delegation row or card;
    "Workspace (Native mode)" gains `^s x` detaches a child.
  - `NATIVE-AGENTS.md` §2 gains the sentence "a child thread is in the strip only when attached";
    §11 lists the kit additions; §12 the keys; §13 row 9 "UI: rows and attach done, picker owed";
    §15 gains the UI subsection with the attention rule table.
- **Verification:** `git diff docs/` beside the scenarios of P4-T07.
- **Done when:** Every scenario assertion has a sentence in one of the three documents.

## Verification

```sh
make lint
make test
make harness
```

`make harness` is required: this phase changes screens, the keymap and a dialog.

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean.
- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] `make test` passes, including the kit gallery test.
- [ ] `make harness` passes with the five new scenarios.
- [ ] `docs/TESTING-HARNESS.md` changed only under its additive rules; §2 is byte-identical.
- [ ] `UX-SPEC.md`, `KEYMAP.md`, `NATIVE-AGENTS.md` describe what runs.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **The scripted `shell` step is new harness surface.** Keep it minimal (`sh -c`, inherited
  environment, stdout as output) and unit-tested; do not add options the scenarios do not need.
- **Row memo regressions.** A `DelegationChanged` must not rebuild the caller's rows. The
  `RowsKey` equality test in P4-T04 is the guard; run it under `gpui-performance` review.
- **`enter` is already `ExpandRow` on `AgentRow`.** Disambiguating by row kind in one handler is
  simpler than a new key context; if the handler grows past a screen, split a `DelegationRow`
  context instead and record it in `KEYMAP.md`.
- **Rollback.** Revert the phase PR; the app returns to phase 2's plain fallbacks, which still read
  correctly against a phase-3 daemon.
