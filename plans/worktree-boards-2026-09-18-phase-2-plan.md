# Worktree-scoped boards — Phase 2: the `fleet://board` Workspace tab — Plan
> Tracker: ./worktree-boards-2026-09-18-phase-2-tracker.md
> Roadmap: ./worktree-boards-2026-09-18-roadmap.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Phase 1 made the daemon, protocol and CLI understand a board scoped to one worktree. This phase
gives it a place in the GPUI app: an opt-in, Fleet-drawn tab inside the worktree's Workspace,
reached with `ctrl-s b`, that shows that worktree's board with the same keys, dialogs and palette
rows the Hub board already has. The tab is a reserved `fleet://board` native command, the same
mechanism the `lg` git pane uses, so the daemon keeps owning the tab list and numbering. The
board screen becomes scope-aware (context or worktree) instead of context-only. A harness
scenario drives the tab end to end. No new card behaviour is added.

## Sizing call

**Phased.** This is phase 2 of 3 (see the roadmap) and runs last. It is GPUI-only plus docs and
harness, one focused stretch, and it depends on phase 1's wire contract being merged and on
phase 3's shared text input being in place (the board dialogs it reuses are migrated there). It is kept separate
from phase 1 because a second pane kind in the Workspace and a scope-aware `BoardState` are the
riskiest parts of the initiative and should not share a critical path with daemon and protocol
work.

## Repository context

- GPUI app in `crates/fleet-app` (GPUI from Zed tag `v1.18.1`). Lint `make lint`, tests `make
  test`, GUI harness `make harness` (required for anything a human sees), `make harness-one
  SCENARIO=<path>` for one scenario. Board harness fixture is `fixture: board`; list names
  `board`, `board.cards`, `tabs`; focus targets like `board.column[0].card[0]`.
- Screens: `state/navigation.rs` has `Screen::{Hub{tab}, Workspace{session}}`, `HubTab::Board`,
  `TerminalMode::{Terminal, Native, Prefix, Scroll}`; `AppState::active_terminal_is_native`
  decides the resting mode. Workspace code is `screens/workspace.rs` + `workspace/{actions,
  chrome, lifecycle, model, native, terminal, agent}.rs`. `native.rs::sync_panes` lazily builds
  one `Pane { view: Entity<Lazygit>, … }` per worktree and evicts it when the daemon stops
  listing the worktree; it keys panes by `WorktreeId` and assumes every native tab is lazygit.
  `actions.rs::active_worktree_id` gives the session's worktree.
- Board screen: `screens/board.rs` (`BoardScreen`, `ensure_current`, `send_card`),
  `views/board_*.rs`, `state/board.rs::BoardState { view, loading, error, focus, filter,
  filter_editing, group_secondary, revision }`; `apply_board_view` rejects a view whose
  `context_id` is not the active context. Dialogs `card_detail`, `card_create`, `card_picker`,
  `board_settings` read `state.board()` and do not care which surface opened them. Loader runs
  on tab entry, context change, reconnect, stale board. Frozen extension points are in
  `docs/APP-CONTRACTS.md` "Board app extension points".
- Keymap: `keymap.rs` binds `Hub > Board` rows; `Workspace > Prefix` rows are `ctrl-s <key>`
  (`c` new terminal, `b` is unbound today). A drift test keeps `docs/KEYMAP.md` and `keymap.rs`
  in agreement. Palette rows are `Command` variants in `dialogs/palette.rs` (`Board: …`).
- Native commands: `fleet_core::config::{NATIVE_SCHEME, NATIVE_LAZYGIT, NATIVE_COMMANDS,
  is_native_command}`; config validation rejects unknown `fleet://` commands; the daemon's
  `Sessions::new_terminal` routes `is_native_command` to `new_native_terminal` (no PTY).
  Proxied remote sessions degrade `fleet://lazygit` to a `lazygit` PTY; the app's settings
  schema (`dialogs/settings/schema.rs`) displays native commands specially.
- New tab request: `RequestBody::NewTerminal { session, name, command, cwd }`; the app already
  sends it (`dialogs/settings/persistence.rs`, `workspace/actions.rs::request_shell_tab`).
- Capability check on the live connection: `supports_capability("board.worktree")` (phase 1).
- Docs touched: `docs/UX-SPEC.md` §3.6 (Workspace, native tab rows) and §Board (placement),
  `docs/KEYMAP.md`, `docs/APP-CONTRACTS.md` (board extension points, key context table),
  `docs/ARCHITECTURE.md` (native tabs paragraph), `docs/SWARM-INVENTORY.md` (`windows` row),
  `docs/BOARD.md` §8, `docs/TESTING-HARNESS.md` (frozen; read before adding a fixture or target).
- Skills to load: `gpui-state-and-memory`, `gpui-components`, `gpui-app-shell`,
  `gpui-performance`, `rust-gpui-testing`, `rust-async-background-work`; `zed-quality-review`
  before done.

## Assumptions

- **One `BoardState`, with a scope.** The Hub and the Workspace are never visible together, so
  `BoardState` gains `scope: Option<BoardScope>` where `BoardScope = Context(ContextId) |
  Worktree(WorktreeId)`. Loading the Hub board sets `Context(active)`; activating the board tab
  sets `Worktree(session worktree)`. `apply_board_view` accepts a view only when it matches the
  current scope (`view.board.worktree_id` for a worktree scope, `context_id` and no worktree
  for a context scope). Switching surface clears and re-ensures; ensure reads are lock-free
  daemon-side, so this is cheap.
- **The tab is created on demand, not by config.** `ctrl-s b` selects the session's existing
  `fleet://board` terminal or sends `NewTerminal { name: "board", command: "fleet://board" }`
  and selects it. Because it is not in `windows[]`, sleeping the session closes it and waking
  does not restore it; `ctrl-s b` reopens it in one keystroke. A user who wants it permanent adds
  `{"name":"board","command":"fleet://board"}` to `windows[]` in `config.json`; that path exists
  already and needs only the reserved-command registration.
- **Remote worktrees keep the tab native.** `fleet://board` is daemon-data-driven, not
  process-backed, so the proxied-session degradation table must not map it to a program.
- **No new ui-kit components.** The pane reuses `screens::board` rendering and `views/board_*`;
  the tab strip already draws the native glyph for `Terminal.kind = Native`.
- **Key context chain** for the pane is `Fleet > Workspace > Native > Board`, and every
  `Hub > Board` row is repeated there with the same action, following the precedent that a
  native pane owns its bare keys. `Filter > BoardFilter` behaves as on the Hub.
- **No tab badge.** The Hub's Board tab shows an open count; the Workspace tab does not get one
  in this phase (no new features).
- **Key ownership follows phase 3.** `Workspace > Native > Board` binds bare letters and the
  board filter owns a text input, so while the filter input is focused the chain publishes
  `Filter > BoardFilter` and not the board word, exactly as the Hub board does today and as
  phase 3's rule requires. The pane adds no text input of its own.

## Out of scope

- A board tab in the default `windows[]` config.
- Showing both boards at once, or a scope switcher inside the Hub board.
- A worktree-board summary badge on the Workspace tab or in the Hub worktrees list.
- Any change to card dialogs, pickers, or the board settings dialog.
- Agent-thread tabs and the agent popup; they are unaffected.

## Affected areas

- `crates/fleet-core/src/config.rs` (reserved command), its tests.
- `crates/fleet-daemon` proxied-degradation site for `fleet://` commands (find via
  `NATIVE_LAZYGIT` uses in `services/` and `machines/`).
- `crates/fleet-app/src/state/board.rs`, `state/navigation.rs`, `screens/board.rs`,
  `screens/workspace.rs`, `workspace/{native,actions,model,chrome,lifecycle}.rs`,
  `shell/root/{board,actions,routing}.rs`, `actions.rs`, `keymap.rs`, `dialogs/palette.rs`,
  `dialogs/help.rs`, `dialogs/settings/schema.rs`, `drive.rs` if a target is added, tests.
- `scenarios/workspace/board-tab.scenario`, harness fixture in `crates/fleet-harness`.
- Docs listed in Repository context.

## Tasks

### P2-T01 — Reserve `fleet://board` as a native command
- **Intent:** Make `fleet://board` a valid, daemon-owned, PTY-less tab command everywhere the
  scheme is validated or degraded.
- **Touches:** `crates/fleet-core/src/config.rs`, the daemon degradation site,
  `crates/fleet-app/src/dialogs/settings/schema.rs`, `docs/ARCHITECTURE.md`,
  `docs/SWARM-INVENTORY.md`.
- **Steps:**
  - Add `NATIVE_BOARD = "fleet://board"` and put it in `NATIVE_COMMANDS`; extend the config
    validation test that currently names only `fleet://lazygit`.
  - Find where a proxied session maps `fleet://lazygit` to `lazygit` and make it leave
    `fleet://board` native, with a comment saying why; add a test.
  - Settings schema: display the new command the way `fleet://lazygit` is displayed.
  - ARCHITECTURE "Native tabs": there are now two reserved commands, one process-backed and one
    daemon-data-driven; SWARM-INVENTORY `windows` row: name both.
- **Verification:** `cargo test -p fleet-core config`; `cargo test -p fleet-daemon`; `make lint`.
- **Done when:** A `windows[]` entry or a `NewTerminal` with `fleet://board` is accepted, spawns
  no PTY, and stays native on a remote worktree.

### P2-T02 — Give `BoardState` a scope and make the loader scope-aware
- **Intent:** Let the single board mirror hold either the active context's board or a
  worktree's board, with the existing generation and staleness rules intact.
- **Touches:** `crates/fleet-app/src/state/board.rs`, `screens/board.rs` (`ensure_current`,
  `finish_board_load`), `shell/root/events.rs` or wherever `BoardChanged` is applied, tests,
  `docs/APP-CONTRACTS.md` board extension points, `docs/BOARD.md` §8.
- **Steps:**
  - Add `BoardScope` and `BoardState.scope`; `clear_board` resets it; a scope change clears the
    view, bumps `revision`, and invalidates pending responses like a context switch does.
  - `apply_board_view` guards on scope match instead of only the active context;
    `apply_card` and `board_stale` keep keying on the displayed board id.
  - `ensure_current` sends `EnsureBoard(context)` or `EnsureWorktreeBoard(worktree)` from the
    scope, with the same one-in-flight-per-generation rule. Triggers gain "board tab activated"
    and "Workspace session changed while the board tab is active".
  - Refuse to enter a worktree scope when `supports_capability("board.worktree")` is false;
    surface the toast text from the roadmap.
  - Reducer tests: context view rejected under a worktree scope and vice versa; A → B → A scope
    switches reject the stale response; stale flag only for the displayed board.
  - Update APP-CONTRACTS (fields, loader triggers) and BOARD.md §8.
- **Verification:** `cargo test -p fleet-app state::board`; `make lint`.
- **Done when:** The board mirror can be pointed at a worktree board and never applies a view
  from the other scope.

### P2-T03 — Add `ctrl-s b` to open or select the board tab
- **Intent:** One prefix key that creates the `fleet://board` tab if absent and selects it.
- **Touches:** `crates/fleet-app/src/actions.rs`, `keymap.rs`, `shell/root/actions.rs`,
  `screens/workspace/actions.rs`, `dialogs/palette.rs`, `dialogs/help.rs`, `docs/KEYMAP.md`,
  `docs/UX-SPEC.md` §3.6 keyboard line.
- **Steps:**
  - Add `prefix::OpenBoard`; bind `"b", "Workspace > Prefix"` and add the KEYMAP row (the drift
    test will fail until both exist).
  - Handler: if the active session is `SessionKind::Worktree`, look for a terminal whose
    command is `NATIVE_BOARD` and `SelectTerminal` it; otherwise send `NewTerminal` with name
    `board`, command `NATIVE_BOARD`, cwd the session cwd, then select the reply's terminal the
    way `request_shell_tab` does. On an agent session, toast that boards belong to worktrees.
    Capability-gate as in P2-T02.
  - Palette row `Workspace: Open board tab` and the help overlay entry.
- **Verification:** `cargo test -p fleet-app keymap`; `make lint`; manual: open a worktree
  session, `ctrl-s b` twice creates one tab and selects it both times.
- **Done when:** `ctrl-s b` is idempotent and documented in KEYMAP and the palette.

### P2-T04 — Render the board pane inside the Workspace
- **Intent:** Draw the scoped board screen in the terminal area when the active native tab's
  command is `fleet://board`, with its own key context.
- **Touches:** `crates/fleet-app/src/screens/workspace/native.rs`, `workspace/model.rs`,
  `workspace.rs`, `state/navigation.rs` (context chain), `keymap.rs`, `screens/board.rs`,
  `shell/root/board.rs`, tests, `docs/KEYMAP.md`, `docs/APP-CONTRACTS.md` key context table.
- **Steps:**
  - Extend the workspace `Model` so a native tab carries which reserved command it is; key panes
    by `(WorktreeId, kind)` or hold an enum `Pane::{Lazygit(..), Board(..)}`. Keep lazygit's
    lazy creation and eviction rules untouched.
  - The board pane hosts the board screen (reuse `BoardScreen`/`views::board_screen`; do not
    fork the rendering). Activation sets `BoardScope::Worktree` and triggers the loader;
    deactivation leaves the pane idle. Follow `gpui-state-and-memory` for entity ownership and
    `gpui-performance` for memoising behind `BoardState.revision`.
  - Publish the chain `Fleet > Workspace > Native > Board` (and `Filter > BoardFilter` while
    filtering) and repeat every `Hub > Board` row for `Workspace > Native > Board` in
    `keymap.rs` and KEYMAP.md. `ctrl-s` stays reserved for the prefix.
  - Board actions that assume the Hub (`board::GoBoard`, `board::OpenWorktree`) must behave
    sensibly from the pane: `GoBoard` goes to the Hub board as today; `OpenWorktree` on a card
    linked to the current worktree is a no-op with a toast.
  - Dialogs opened from the pane shadow the Workspace as they shadow the Hub; verify
    `card_detail`, `card_create`, `card_picker`, `board_settings` need no change because they
    read `state.board()`.
  - Tests: pane kind selection from the terminal command; key context chain on the pane; the
    keymap drift test.
- **Verification:** `cargo test -p fleet-app`; `make lint`; manual: every key in the
  `Hub > Board` table works in the pane; `ctrl-s 1` leaves it; `ctrl-s x` closes it.
- **Done when:** A worktree's board is fully operable from its Workspace tab with the Hub board's
  keys.

### P2-T05 — Drive the tab with a harness scenario
- **Intent:** Prove the tab end to end in the real GUI.
- **Touches:** `crates/fleet-harness` fixture, `scenarios/workspace/board-tab.scenario`,
  `crates/fleet-app/src/drive.rs` only if a target or list is missing,
  `docs/TESTING-HARNESS.md` only if the grammar or a name must change (read it first; it is
  frozen).
- **Steps:**
  - Fixture: extend `board` or add `board-worktree` with a published worktree that owns a board
    holding at least two cards in two columns; document the fixture name in TESTING-HARNESS.
  - Scenario: open the worktree session, `ctrl-s b`, await `lists.tabs` contains `board`,
    assert `focused == "board.column[0].card[0]"`, `]` moves a card and the column badges
    change, `shot`, `ctrl-s s` back to Hub, `g b`, assert the Hub board is the **context**
    board (different card set) — the cross-phase regression from the roadmap.
  - Run `make harness-one SCENARIO=scenarios/workspace/board-tab.scenario` then `make harness`.
- **Verification:** `make harness` green with the new scenario in the report.
- **Done when:** The scenario passes in the virtual lane and the run directory holds its shot.

### P2-T06 — Reconcile the UX, keymap and contract docs
- **Intent:** Make the docs authoritative for the new surface.
- **Touches:** `docs/UX-SPEC.md` §3.6 (native tab rows, states table line "currently `lg`",
  keyboard line) and §Board "Placement"; `docs/KEYMAP.md` (already touched by P2-T03/T04,
  final pass); `docs/APP-CONTRACTS.md` (board extension points, key context table row for the
  pane); `docs/BOARD.md` §8 and §9; ADR 0018 consequences paragraph if phase 2 changed an
  assumption.
- **Steps:**
  - UX-SPEC: the board tab is a native tab like `lg`; placement paragraph gains "or, inside a
    worktree session, the `fleet://board` tab showing `EnsureWorktreeBoard(worktree)`"; state the
    sleep/wake behaviour plainly.
  - APP-CONTRACTS: `BoardState.scope`, loader triggers, the new key context row, the
    `prefix::OpenBoard` action.
  - BOARD.md §8 (state, bridge, keymap rows for the new context) and §9 (app tests, harness).
  - Run the `zed-quality-review` skill over the phase's diff and fix what it finds.
- **Verification:** `make lint`; `make test`; `make harness`; read-through of each doc section
  against the code.
- **Done when:** A reader of UX-SPEC, KEYMAP and APP-CONTRACTS can describe the board tab
  without this plan.

## Verification

Run from the repository root:

```sh
make lint
make test
make harness        # required: this phase changes a screen, the keymap and a tab
```

Targeted while iterating: `cargo test -p fleet-app`, `cargo test -p fleet-core config`,
`make harness-one SCENARIO=scenarios/workspace/board-tab.scenario`.

## Definition of done

- [x] Every P2 task is `[x]` in the tracker and the tracker matches the code.
- [x] `make lint` is clean.
- [x] `make test` passes (clippy `-D warnings` is this repo's type gate).
- [x] `make harness` passes with `board-tab.scenario` in the report.
- [x] UX-SPEC, KEYMAP, APP-CONTRACTS, ARCHITECTURE, SWARM-INVENTORY and BOARD.md §8 agree with
      the code.
- [x] No IO, requests or `cx.notify` inside `render`; no bare `.detach()`; no `unwrap`.
- [x] Follow-ups captured in the tracker.

## Risks and rollback

- **Second pane kind destabilises lazygit.** Keep lazygit's creation/eviction code path
  byte-for-byte where possible and gate the new kind on the terminal command; the existing
  workspace tests plus `make harness` are the net.
- **Scope-aware `BoardState` breaks the Hub board.** P2-T02's reducer tests and the existing
  `scenarios/board/*` harness scenarios cover the Hub; the roadmap's cross-check in P2-T05
  covers the mix.
- **Key context drift.** The keymap drift test fails the build if `keymap.rs` and KEYMAP.md
  disagree; keep them in one commit.
- **Sleep/wake surprise.** Stated in UX-SPEC; if users dislike it, the follow-up is adding the
  tab to the default `windows[]`, which is a config decision, not code.
- **Rollback:** revert the phase 2 commits. Phase 1 stays useful on its own; a
  `fleet://board` entry someone wrote into `windows[]` would then fail config validation, so
  mention that in the revert commit.
