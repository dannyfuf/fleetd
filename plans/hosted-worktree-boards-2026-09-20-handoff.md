# Handoff: worktree boards for hosted worktrees live on the wrong daemon

## The symptom

On the Mac, the Fleet app connects to the dev-box as a remote host. Opening the board tab for
worktree `dannyfuf/fleetd#feat-workflows-v1` shows an empty board. On the dev-box,
`fleet board --worktree=dannyfuf/fleetd#feat-workflows-v1 show` prints eleven cards
(FEA-1..FEA-11). Both are "the same" board, `wt-dannyfuf-fleetd-feat-workflows-v1`, and they are
two different files on two different machines.

## Root cause (verified on feat/workflows-v1 at 1fee043)

1. The router never forwards a board request to a host. Every board and card variant
   (`GetBoard`, `EnsureBoard`, `EnsureWorktreeBoard`, `CreateBoard`, `CreateWorktreeBoard`,
   `CreateCard`, `UpdateCard`, `MoveCard`, `DeleteCard`, `AddCardComment`, `ListBoards`,
   `SyncBoard`, `ResolveCardConflict`, `DescribeBoardBackend`, `ListBoardBackends`,
   `CreateWorktreeFromCard`) sits in the local arm of
   `crates/fleet-daemon/src/services/router/classify.rs` (around lines 105-125, and the
   mirror list around 316-330). `docs/REMOTE-MACHINES.md` §6 states the rule: "Config, context,
   repo, board, Hello, Ping, host, Doctor, update, and daemon lifecycle commands are local
   orchestration."
2. `EnsureWorktreeBoard { worktree_id }` therefore runs on the laptop daemon, which derives
   `worktree_board_id(worktree)` = `wt-<owner>-<repo>-<slug>` (`docs/BOARD.md` line ~262) and
   creates an empty local document for a worktree it only mirrors. The laptop already has
   that stale empty file in its `~/.fleet/boards/`.
3. `Event::BoardChanged` is subscribed from the host link (`crates/fleet-daemon/src/machines/link.rs`
   `all_event_kinds`) but the router drops it: `router/agents.rs` ~451 and
   `router/translate.rs` ~325 treat it as nothing to register or translate, so even if the
   laptop had the right board it would not learn of changes made on the dev-box.
4. The `IdMaps` resolver already knows which host owns a worktree:
   `router/ids.rs::host_of_worktree` (line ~251), populated from the host snapshot. Terminals,
   sessions, jobs and agent threads use it (`classify.rs` ~90-105). Boards do not.
5. Worktree boards live in the daemon's `boards/<board-id>.json`
   (`crates/fleet-daemon/src/stores/board.rs`: `list`, `load`, `save`, `delete`,
   `delete_with_worktree`, `restore_with_worktree`). Document shape: `{"version":1,"board":{"id",
   "contextId","worktreeId","name","prefix","nextNumber","backend","statuses",...},"cards":[...]}`.

This matters beyond display. The board-workflows feature (plans/board-workflows-2026-09-20-*.md on
feat/workflows-v1, design doc https://claude.ai/code/artifact/083bbcb1-091c-4360-ac31-5e1ba8548ddc)
turns a card move into a delegation on the daemon that owns the worktree. That daemon is always the
dev-box in this setup, so a laptop app that cannot reach the dev-box's board can never see or drive
a run. The roadmap's "feature is local-daemon only" decision assumed app and worktree share a daemon;
amend it to "the daemon that owns the worktree, reached through the router".

## What to build

Route a hosted worktree's board to the daemon that owns the worktree. Context boards stay local.

1. **Classify.** `EnsureWorktreeBoard` and `CreateWorktreeBoard` route by
   `host_or_local(resolver.host_of_worktree(&worktree_id))`. The `RequestBody` match in
   `Router::route` is exhaustive on purpose; keep it that way.
2. **Board ownership.** Add a `boards: HashMap<BoardId, HostId>` map to `IdMaps` with
   `register_board` / `host_of_board`, cleared with the host's other ids on a Down transition
   (see `ClearedIds` in `ids.rs` ~230-247) and rebuilt from the host snapshot on Ready. Register
   when a hosted `EnsureWorktreeBoard`/`CreateWorktreeBoard` response comes back, when
   `ListBoards` is merged, and from a forwarded `BoardChanged`. Then `GetBoard`, `CreateCard`,
   `UpdateCard`, `MoveCard`, `DeleteCard`, `AddCardComment`, `SyncBoard`,
   `ResolveCardConflict`, `DescribeBoardBackend`, `CreateWorktreeFromCard` route by
   `host_of_board`. Board ids are already globally unique (derived from the worktree id), so no
   bijective remap is needed; they pass through like worktree ids and thread UUIDs.
3. **ListBoards.** Merge local and host results like agent/list/inspect do; a hosted worktree
   board must not appear twice.
4. **Events.** Forward `BoardChanged` from a host endpoint to local subscribers (register the
   board id, no translation needed). The app's board mirror and the CLI `--json` consumers
   then see dev-box changes live.
5. **Capability.** The laptop daemon must refuse `EnsureWorktreeBoard` for a hosted worktree whose
   host does not advertise `board.worktree` (`crates/fleet-proto/src/response.rs`
   `BOARD_WORKTREE_CAPABILITY`, advertised in `server/connection.rs` ~1052) with the existing
   sentence "this daemon does not support worktree boards; run `fleet daemon restart`".
6. **Docs in the same commit.** `docs/REMOTE-MACHINES.md` §6 (routing table), `docs/BOARD.md`
   §0 and the CLI paragraph (~line 670), and `docs/ARCHITECTURE.md` if it lists the routing
   classes. Add an ADR under `docs/decisions/` only if the numbering convention there asks for one.

## Recovering the cards that already exist

The user must get back the eleven cards already on the dev-box, not just future ones.

- **The host copy is authoritative.** The dev-box file is
  `/home/df/.fleet/boards/wt-dannyfuf-fleetd-feat-workflows-v1.json` (`nextNumber` is 12,
  cards FEA-1..FEA-11, labels core/daemon/cli/app). After routing lands, opening the board on the
  laptop must read that file through the host and show all eleven cards, with no migration step.
- **The laptop's stale local file must not shadow it.** On host Ready, or on the first routed
  `EnsureWorktreeBoard`, a local `boards/<id>.json` whose `worktreeId` resolves to a hosted
  worktree is moved to the daemon's trash (reuse the `delete_with_worktree` / trash convention in
  `stores/board.rs`), with one `tracing` line naming the file. Never silently merge: if the
  stale local copy has cards (the user may have created some from the app while the bug was
  live), log a warning that names the trashed path and the card count so they can be re-created
  by hand. No `let _ =`, no `unwrap`.
- **Stopgap until the fix lands, if needed today:** copy the dev-box file to the laptop's
  `~/.fleet/boards/` under the same name and restart the laptop daemon. The app will show the
  cards, but the two files diverge from the first edit, and the laptop copy is the one the fix
  will trash. Prefer waiting for the fix.

## Verification

- Router unit tests in `crates/fleet-daemon/src/services/router/` covering classify and translate
  for every board variant, hosted and local, plus the Down-clears-ownership path.
- A `tests/` integration test: two private daemons, one configured as the other's host, a
  worktree owned by the host, cards created on the host with `fleet board`; the laptop-side
  `fleet board --worktree=<id> show` prints them, `card move` from the laptop changes the host
  file, and a pre-existing stale laptop file with the same id ends up in trash with a warning.
  `crates/fleet-daemon/tests/remote_create.rs` and `boards_service.rs` show the fixtures.
- `make lint`, `make test`, and `make harness` if any app path changes.

## Skills to load first

`rust-ipc-protocol` (router, capability, reconnect), `rust-workspace-architecture` (logging,
error policy, docs-with-code), `rust-gpui-testing` (two-daemon fixture), and `zed-quality-review`
before calling it done. Commit as `daemon: route hosted worktree boards to their owner`.

## Related cards on the dev-box board `dannyfuf/fleetd#feat-workflows-v1`

FEA-11 (this issue, Backlog, high) and FEA-10 (bare `--worktree` from a native thread, Backlog,
medium). Move FEA-11 as the work really progresses; from a native thread pass
`--worktree=dannyfuf/fleetd#feat-workflows-v1` with the `=`. `fleet` is on PATH on the dev-box via
`~/.local/bin/fleet` → the feat-workflows-v1 debug build.
