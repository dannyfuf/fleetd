# Worktree-scoped boards — Phase 1: model, daemon, protocol, client and CLI — Plan
> Tracker: ./worktree-boards-2026-09-18-phase-1-tracker.md
> Roadmap: ./worktree-boards-2026-09-18-roadmap.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Fleet has exactly one kanban board per context (`docs/BOARD.md`). This phase lets a board also
be scoped to one **worktree** (`fleet_core::ids::WorktreeId`, shaped `owner/name#slug`), created
only on demand, so the work inside a single worktree can be planned separately from the context
board. The daemon learns to create, find, list and cascade-delete such boards; the wire protocol
gains two additive requests and a capability string; the client gains two typed methods; and the
`fleet board …` CLI gains a `--worktree` selector that also resolves from the current worktree
terminal, so an agent running inside a worktree can drive its board without knowing any ids. No
card, status, sync or dialog behaviour changes.

## Sizing call

**Phased.** This is phase 1 of 2 (see the roadmap). It spans five crates and a wire change with
byte-exact goldens, so it is more than a week on its own, and it ends in a shippable state: the CLI
and daemon fully support worktree boards while the GPUI app keeps working untouched. Phase 2 adds
the app surface. Crushing both into one plan would put the GPUI pane work on the same critical
path as the daemon cascade and the protocol goldens.

## Repository context

- Rust Cargo workspace (`Cargo.toml` at the root, crates under `crates/`). GPUI app `fleet` is
  the `fleet-app` bin; the daemon is `fleetd`; the CLI is the `fleet-cli` lib compiled into the
  `fleet` binary. Rust edition uses `let … && let` chains (edition 2024).
- Lint: `make lint` (`cargo fmt --check` + `cargo clippy --workspace --all-targets --all-features
  -D warnings`). Tests: `make test` (builds `fleetd` first, then `cargo test --workspace`).
  Daemon restart after daemon changes: `make restart`. GUI harness: `make harness` (not needed in
  this phase; nothing a human sees changes).
- Board domain: `crates/fleet-core/src/board/{model,defaults,ops,sync,property}.rs`; ids in
  `crates/fleet-core/src/ids.rs` (`BoardId` is a slug `[a-z0-9][a-z0-9-]*`; `WorktreeId` is
  `owner/name#slug`, so a worktree id is **not** a valid board id). `fleet_core::slug::slugify`
  exists.
- Daemon board service: `crates/fleet-daemon/src/services/boards.rs` +
  `boards/{lifecycle,documents,cards,sync,worktree,tests}.rs`; store
  `crates/fleet-daemon/src/stores/board.rs` (one JSON doc per board under `$FLEET_HOME/boards/`).
  `documents.rs::context_board` finds a context's board by trying `BoardId == context id` first,
  then scanning every document for a matching `contextId`. `lifecycle.rs::list` and
  `summaries` skip boards whose context no longer exists. `Boards::new` receives
  `Arc<Worktrees>`; `Worktrees` is built before `Boards` in
  `crates/fleet-daemon/src/services/composition.rs`.
- Worktree deletion has one funnel: `Worktrees::delete_one` in
  `crates/fleet-daemon/src/services/worktrees/trash.rs`. It is reached from
  `dispatch.rs` (`DeleteWorktrees`, repository deletion via `delete_guarded`) and from the
  prune job through the `WorktreeDeleter` trait in `services/prune.rs`. Context deletion
  already cascades boards at `dispatch.rs` (`self.boards.delete_for_context`).
- Protocol: `crates/fleet-proto/src/{request,response,event,snapshot}.rs`,
  `PROTOCOL_VERSION = 8` in `lib.rs`, capability strings such as
  `SNAPSHOT_REVISION_CAPABILITY` published in `crates/fleet-daemon/src/server/connection.rs`
  (two `chain(std::iter::once(…))` sites). Goldens live in `crates/fleet-proto/tests/compatibility.rs`
  and `src/request/tests.rs`. Client capability check:
  `fleet_client::connection::…::supports_capability(&str)`.
- Client API: `crates/fleet-client/src/api/boards.rs` (one typed method per request).
- CLI: `crates/fleet-cli/src/args.rs` (`BoardArgs` with global `--board`/`--context`/`--json`),
  `commands/board.rs` (`resolve_context`, `resolve_board`, card helpers), `commands/board/tests.rs`,
  `human.rs::boards` (summary table). `FLEET_SESSION` (a `SessionId`) is set inside every Fleet
  terminal; `Snapshot.sessions[].kind == SessionKind::Worktree(id)` maps it to a worktree.
- Existing daemon tests: `crates/fleet-daemon/tests/boards_service.rs`, `boards_jira.rs`.
- Docs are authoritative: `docs/BOARD.md` (board contract), `docs/README.md` (doc domains),
  `docs/decisions/` (ADRs 0001–0017; 0008 is the board model). Commit style
  `<area>: <imperative lowercase summary>`, one logical change per commit, doc rides with code.
- Skills to load before editing: `rust-workspace-architecture` (core/daemon/module changes,
  ADR), `rust-ipc-protocol` (proto/client/dispatch), `rust-gpui-testing` (any test),
  `rust-async-background-work` (daemon async), `zed-quality-review` before declaring done.

## Assumptions

- **Scope lives on the board.** `Board` gains `worktree_id: Option<WorktreeId>`
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`). `context_id` stays populated
  with the worktree's repository's context, so every existing context rule (listing, summaries,
  context-delete cascade, "repositories must share the board's context") keeps holding. No
  `BOARD_DOCUMENT_VERSION` bump: the field is additive with a default.
- **Exactly one board per worktree.** `EnsureWorktreeBoard` is get-or-create;
  `CreateWorktreeBoard` for a worktree that already has one is `BoardError::Duplicate`.
- **Board id derivation.** `defaults::worktree_board_id(worktree)` returns
  `wt-<slugify(owner)>-<slugify(name)>-<slugify(slug)>` truncated to the same length the slug
  validator accepts. Lookup never depends on the id: `Boards::worktree_board` tries the derived
  id as a fast path (accepting it only when the document's `worktreeId` matches) and falls back
  to scanning documents, exactly as `context_board` does. If the derived id is already taken by
  another board, creation appends `-2`, `-3`, … until free. No hashing, so no new dependency.
- **Defaults for a new worktree board.** Name = the worktree slug; prefix = first three
  alphanumerics of the slug uppercased, fallback `WT` (same rule as `default_prefix`);
  `default_repo_id = Some(worktree.repo_id)`; default statuses; local backend. Everything a
  context board can do afterwards (`board set`, `--backend jira`, `card worktree`) is left as is.
- **Cascade seam.** `Worktrees` cannot depend on `Boards` (`Boards` already holds
  `Arc<Worktrees>`), so `Worktrees` gets a late-bound observer (a small trait, e.g.
  `WorktreeCascade`, stored in a `OnceLock<Arc<dyn …>>` and set in `composition.rs` after
  `Boards` exists) invoked once after a worktree's trash move succeeds. A cascade error is
  logged with `tracing::warn!` and not propagated: the worktree is already gone. Alternative
  considered and rejected: calling `boards.delete_for_worktree` from the three call sites in
  `dispatch.rs` and a prune wrapper; it is three places to forget instead of one.
- **Capability, not a version bump.** The new requests are additive, so `PROTOCOL_VERSION`
  stays 8 and a `BOARD_WORKTREE_CAPABILITY = "board.worktree"` string is published by the daemon.
- **CLI default.** `fleet board` with no selector keeps meaning the active context's board.
  `--worktree` with no value means "the worktree of the terminal I am in" (`FLEET_SESSION`).
  This keeps existing scripts and humans unaffected.
- **`fleet board list`** continues to list every board of the daemon, now with a scope column;
  no new filter flag is added.

## Out of scope

- Any GPUI/app change (phase 2).
- A `fleet board delete` command (the `DeleteBoard` request exists but is not exposed today;
  the cascade covers cleanup). Record as a follow-up if wanted.
- New card fields, statuses, backends, sync behaviour, dialogs or keys.
- Multiple boards per worktree, or boards scoped to a session, a repository or a host.
- Migrating existing cards from a context board to a worktree board.
- Remote (proxied) daemons: the requests are ordinary board requests and are routed like the
  rest; nothing remote-specific is added or tested here.

## Affected areas

- `crates/fleet-core/src/board/model.rs`, `board/defaults.rs`, `board/ops.rs` (summarize),
  `board/tests` (wherever the module's tests live).
- `crates/fleet-daemon/src/services/boards.rs`, `boards/{lifecycle,documents}.rs`,
  `services/worktrees.rs` + `worktrees/trash.rs`, `services/composition.rs`,
  `services/dispatch.rs`, `crates/fleet-daemon/tests/boards_service.rs`.
- `crates/fleet-proto/src/{lib,request,response}.rs`, `src/request/tests.rs`,
  `tests/compatibility.rs`; `crates/fleet-daemon/src/server/connection.rs` (capability
  publication).
- `crates/fleet-client/src/api/boards.rs` (+ its round-trip test).
- `crates/fleet-cli/src/args.rs`, `commands/board.rs`, `commands/board/tests.rs`, `human.rs`.
- `docs/BOARD.md` §0, §1, §2, §4, §5, §6, §9; `README.md` board paragraph;
  `docs/decisions/0019-worktree-scoped-boards.md`; `docs/README.md` if it indexes ADRs.

## Tasks

### P1-T01 — Add the worktree scope to the core board model
- **Intent:** Teach `fleet_core::board` that a board may belong to a worktree, with defaults
  and an id derivation, without changing any existing behaviour.
- **Touches:** `crates/fleet-core/src/board/model.rs`, `board/defaults.rs`, `board/ops.rs`,
  core board tests, `docs/BOARD.md` §0 §2.
- **Steps:**
  - Add `worktree_id: Option<WorktreeId>` to `Board` and to `BoardSummary`, both serde-defaulted
    and skipped when `None`; make `ops::summarize` copy it across.
  - Add `defaults::worktree_board_id(worktree: &WorktreeId) -> BoardId` (slugified, `wt-`
    prefixed, truncated to a valid slug) and `defaults::new_worktree_board(context: &Context,
    worktree: &Worktree, now: &str) -> Board` per the Assumptions; keep `new_board` unchanged.
  - Unit tests: a `Board` JSON without `worktreeId` deserialises to `None`; a board with the
    field round-trips; id derivation for a representative worktree id and for one whose parts
    contain characters `slugify` must drop; the prefix fallback.
  - Update `docs/BOARD.md` §0 ("one board per context, optionally one more per worktree"), §2
    (struct fields, new `defaults` signatures).
- **Verification:** `cargo test -p fleet-core board`; `make lint`.
- **Done when:** The core crate compiles with the new fields and defaults, old documents still
  parse, and BOARD.md §2 matches the code.

### P1-T02 — Make board lookup and listing scope-aware in the daemon
- **Intent:** A context's board is the one with that `contextId` and no `worktreeId`; a
  worktree's board is found by `worktreeId`; orphaned worktree boards are skipped.
- **Touches:** `crates/fleet-daemon/src/services/boards/documents.rs`, `boards/lifecycle.rs`,
  `crates/fleet-daemon/tests/boards_service.rs`, `docs/BOARD.md` §4.
- **Steps:**
  - In `documents.rs::context_board`, accept a document only when `board.worktree_id.is_none()`
    in both the fast path and the scan.
  - Add `documents.rs::worktree_board(&WorktreeId) -> DaemonResult<Option<BoardId>>` with the
    derived-id fast path and the scan fallback, mirroring `context_board`'s comments.
  - In `lifecycle.rs::list` and `summaries`, skip a worktree board whose worktree is no longer
    in `State.worktrees`, exactly as boards of missing contexts are skipped.
  - Regression tests: a context with both a context board and a worktree board — `ensure(context)`
    returns the context board even when the context board's id is not the context id; `list`
    reports both with the right `worktreeId`; a worktree board whose worktree was removed from
    state is absent from `list` and `summaries`.
  - Update BOARD.md §4 prose ("Boards whose context no longer exists are skipped…" gains the
    worktree sentence; `context_board` rule stated).
- **Verification:** `cargo test -p fleet-daemon --test boards_service`; `make lint`.
- **Done when:** The Hub's `EnsureBoard` path can never return a worktree board and the new
  lookup is covered by tests.

### P1-T03 — Add ensure/create for worktree boards to the `Boards` service
- **Intent:** Provide `Boards::ensure_for_worktree` (get-or-create) and
  `Boards::create_for_worktree` (explicit, `Duplicate` if one exists).
- **Touches:** `crates/fleet-daemon/src/services/boards/lifecycle.rs`,
  `crates/fleet-daemon/tests/boards_service.rs`, `docs/BOARD.md` §4.
- **Steps:**
  - Resolve the worktree from `State.worktrees` (`NotFound` otherwise) and its context through
    `State.repos` (repo id → context id); build the board with `new_worktree_board`.
  - Follow `ensure`'s existing shape: lock-free read when the board exists, per-board gate,
    quarantine refusal, duplicate check, backend validate/normalize, `save` with
    `BoardChangeReason::Created`. Apply the `-2`, `-3` id suffix rule when the derived id is
    taken by a document whose `worktreeId` differs.
  - `create_for_worktree(worktree, name, prefix, backend)` mirrors `create` and refuses a second
    board for the same worktree with `BoardError::Duplicate(worktree id)`.
  - Tests: ensure twice returns the same board and creates one document; create after ensure is
    `Duplicate`; ensure on an unpublished worktree is `NotFound`; the created board carries
    `worktreeId`, `contextId`, `defaultRepoId` and the derived prefix; the id-collision suffix.
  - Update BOARD.md §4 with the two signatures and their doc comments.
- **Verification:** `cargo test -p fleet-daemon --test boards_service`; `make lint`.
- **Done when:** A worktree board can be created and re-read through the service with the
  documented semantics.

### P1-T04 — Cascade board deletion when a worktree is deleted or pruned
- **Intent:** Deleting a worktree by any path deletes its board so a recreated worktree with the
  same id cannot adopt stale cards.
- **Touches:** `crates/fleet-daemon/src/services/boards/lifecycle.rs`,
  `services/worktrees.rs`, `services/worktrees/trash.rs`, `services/composition.rs`,
  `crates/fleet-daemon/tests/boards_service.rs` (or a worktrees test file if one fits better),
  `docs/BOARD.md` §4.
- **Steps:**
  - Add `Boards::delete_for_worktree(&WorktreeId) -> DaemonResult<()>` shaped like
    `delete_for_context` (a document this build cannot read goes to trash with the worktree).
  - Add the late-bound observer on `Worktrees` (see Assumptions), invoked at the end of
    `delete_one` after the trash move succeeds; log and swallow its error with a comment saying
    why. Register `Boards` as that observer in `composition.rs` right after it is built.
  - Tests: `DeleteWorktrees` removes the worktree's board document and emits
    `BoardChanged { Deleted }`; the prune path (through `WorktreeDeleter`) does the same; a
    context board of the same context is untouched; deleting a worktree with no board is a no-op.
  - Update BOARD.md §4 (cascade sentence next to the context cascade one).
- **Verification:** `cargo test -p fleet-daemon`; `make lint`.
- **Done when:** No deletion path leaves a worktree board document behind, and the tests prove
  both the direct and the prune paths.

### P1-T05 — Add the wire requests, the capability string and the dispatch arms
- **Intent:** Expose worktree boards on the protocol additively, gated by a capability.
- **Touches:** `crates/fleet-proto/src/lib.rs`, `src/request.rs`, `src/request/tests.rs`,
  `tests/compatibility.rs`, `crates/fleet-daemon/src/services/dispatch.rs`,
  `crates/fleet-daemon/src/server/connection.rs`, `docs/BOARD.md` §5.
- **Steps:**
  - Add `RequestBody::EnsureWorktreeBoard { worktree_id }` → `ResponseBody::Board` and
    `RequestBody::CreateWorktreeBoard { worktree_id, name: Option<String>, prefix:
    Option<String>, backend: Option<BackendRef> }` → `ResponseBody::Board`, snake_case like their
    siblings.
  - Add `pub const BOARD_WORKTREE_CAPABILITY: &str = "board.worktree"` with a doc comment
    explaining why it is a capability and not a version bump; publish it at both
    `HelloResponse.capabilities` sites in `server/connection.rs`.
  - Dispatch arms calling `boards.ensure_for_worktree` / `create_for_worktree`.
  - Goldens: byte-exact JSON for both requests; a `Board` and a `BoardSummary` without
    `worktreeId` decode to `None`; a hello response listing the new capability. Follow the
    existing compatibility-test pattern in `tests/compatibility.rs`.
  - Update BOARD.md §5 (request table) and §1 (no crate placement change, but the capability
    row if §1 lists such things).
  - Run `make restart` so the running daemon serves the new requests.
- **Verification:** `cargo test -p fleet-proto`; `cargo test -p fleet-daemon`; `make lint`;
  `make restart`.
- **Done when:** Both requests round-trip through the daemon socket and the goldens are frozen.

### P1-T06 — Add the typed client methods
- **Intent:** Give `fleet_client::Client` `ensure_worktree_board` and `create_worktree_board`.
- **Touches:** `crates/fleet-client/src/api/boards.rs` and its round-trip test.
- **Steps:**
  - One method per request, matching the shape of `ensure_board` / `create_board`.
  - One socket round-trip test per method, following the crate's existing board test.
  - Update BOARD.md §6 client list.
- **Verification:** `cargo test -p fleet-client`; `make lint`.
- **Done when:** Both methods exist, are tested, and BOARD.md §6 names them.

### P1-T07 — Add the `--worktree` selector to `fleet board`
- **Intent:** Let a human or agent target a worktree's board explicitly or from the current
  worktree terminal, with every existing card/board subcommand working unchanged.
- **Touches:** `crates/fleet-cli/src/args.rs`, `commands/board.rs`, `commands/board/tests.rs`,
  `human.rs`, `docs/BOARD.md` §6, `README.md`.
- **Steps:**
  - In `BoardArgs`, add a global `--worktree[=<id>]` (`num_args(0..=1)`, `require_equals = true`, conflicts with
    `--board` and `--context`). Model it as an enum: explicit id, or "from the session".
  - In `resolve_board`, when the selector is present: if "from the session", read
    `FLEET_SESSION`, find that session in `Snapshot.sessions`, require
    `SessionKind::Worktree(id)`; otherwise use the explicit id. Then call
    `ensure_worktree_board`. Error texts: `no worktree session: pass
    --worktree=<owner/name#slug> or run inside a worktree terminal` and, when the daemon lacks the
    capability, `this daemon does not support worktree boards; run \`fleet daemon restart\``.
  - `fleet board create --worktree[=<id>]` calls `create_worktree_board` with the same
    `--name/--prefix/--backend/--setting` flags.
  - `human::boards` gains a `Scope` column (`context` or the worktree id); `board show`'s
    header line names the worktree when the board has one. JSON envelopes need no change
    beyond the new field flowing through.
  - Tests: arg parsing for bare and valued `--worktree` and the conflicts; resolution from
    `FLEET_SESSION` against a fake snapshot (worktree session, agent session, missing
    session); the capability refusal; the `Scope` column.
  - Update BOARD.md §6 (selector precedence: `--board` else `--worktree` else `--context`
    else active context) and README's board paragraph.
- **Verification:** `cargo test -p fleet-cli`; `make lint`; manual: inside a Fleet worktree
  terminal, `fleet board --worktree show`, `fleet board --worktree card new "try"`,
  `fleet board --worktree card move <key> in-progress`, `fleet board list`.
- **Done when:** An agent inside a worktree terminal can CRUD and move cards on that worktree's
  board with `fleet board --worktree …` and never needs an id.

### P1-T08 — Record the decision and finish the docs
- **Intent:** Write the ADR and reconcile every doc this phase touched.
- **Touches:** `docs/decisions/0019-worktree-scoped-boards.md`, `docs/BOARD.md` (§9 tests
  paragraph, §Decision records), `docs/README.md` if it indexes ADRs, `docs/ARCHITECTURE.md`
  if it describes boards.
- **Steps:**
  - ADR 0019: context, decision (scope field on `Board`, one board per worktree, derived id
    with lookup by field, late-bound cascade observer, capability instead of version bump),
    alternatives rejected, consequences (phase 2's single `BoardState` assumption).
  - BOARD.md §9: add what the new tests hold per layer. Link the ADR from BOARD.md's
    "Decision records" section.
  - Run the `zed-quality-review` skill over the phase's diff and fix what it finds.
- **Verification:** `make lint`; `make test`; a read-through of `docs/BOARD.md` against the code
  for every signature this phase added.
- **Done when:** A reader of `docs/BOARD.md` and ADR 0019 can explain worktree boards without
  reading this plan.

## Verification

Run from the repository root:

```sh
make lint
make test
make restart        # after any daemon change, so the running fleetd matches the build
```

Targeted while iterating: `cargo test -p fleet-core board`, `cargo test -p fleet-daemon --test
boards_service`, `cargo test -p fleet-proto`, `cargo test -p fleet-client`,
`cargo test -p fleet-cli`.

## Definition of done

- [x] Every P1 task is `[x]` in the tracker and the tracker matches the code.
- [x] `make lint` is clean.
- [x] `make test` passes (this repo has no separate type-check step; clippy `-D warnings` is it).
- [x] `make restart` has been run and the manual CLI check in P1-T07 was done against it.
- [x] `docs/BOARD.md`, `README.md` and ADR 0019 agree with the code.
- [x] No `unwrap`, `todo!`, `dbg!`, `TODO`, bare `.detach()` or `let _ =` on a fallible call
      in the diff.
- [x] Follow-ups (e.g. `fleet board delete`) are captured in the tracker.

## Risks and rollback

- **Context board mis-lookup** (a worktree board shown as the context board): covered by
  P1-T02's regression test; if it slips, the fix is one predicate in `context_board`.
- **Cascade never fires** (observer not registered): P1-T04's tests run through the real
  composition; rollback is removing the observer, which leaves orphan documents that `list`
  and `summaries` already skip.
- **Id collision on derivation**: the suffix rule handles it; lookup never trusts the id.
- **Older daemon with a newer CLI**: the capability check yields an actionable error instead
  of an opaque `unknown request`.
- **Rollback of the whole phase:** revert the commits; existing board documents are unaffected
  because the only persisted change is an optional field older builds ignore. Any worktree board
  documents created would then appear as extra boards of their context in `fleet board list`
  on the old build; delete them from `$FLEET_HOME/boards/wt-*.json` if that matters.
