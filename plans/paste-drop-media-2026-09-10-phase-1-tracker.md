# Phase 1 — Media staging and the terminal gesture — Tracker

> Plan: ./paste-drop-media-2026-09-10-phase-1-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

Repo-specific, on top of the above:
- Load the skill `CLAUDE.md` names for the area before editing it, and `zed-quality-review` before calling the phase done.
- Commit as `<area>: <imperative lowercase summary>` — `proto`, `daemon`, `client`, `app`, `docs`. One logical change per commit; the doc update rides with the code it describes.
- Run `make restart` after any daemon change, or the running `fleetd` will not match your build and manual checks will lie to you.

## Kickoff
- [x] The branch is on current `main` (re-grounded at `fa31272` on 2026-09-24; if `main` has moved again, rebase and spot-check the plan's `path:line` references first). — verified: `HEAD`, `main`, and their merge-base are all `fa31272`
- [x] I have read the plan end to end. — verified: integration pass re-read the current plan end to end before editing
- [x] I have read `./paste-drop-media-2026-09-10-roadmap.md` and understand what phases 2 and 3 expect from this one: all four `MediaAnchor` arms, the `Thread` leaf directory, and a `media::stage` that takes a continuation and a `force_copy` flag. — verified: integration pass checked all three seams against code
- [x] Green verification on the integrated tree: `make lint`, `make test`, `make check`, `make harness`. — verified: all four commands passed on 2026-09-25; the virtual harness passed all 104 scenarios
- [x] I am ready to start. — verified: staged task reports and ownership handoff reviewed

## Tasks
- [x] P1-T01 — Confirm what the clipboard and a file drop actually give GPUI, on both platforms — verified: pinned Zed `v1.18.1` source audit plus live Wayland text, PNG, and URI-list probes; throwaway example deleted
- [x] P1-T02 — Put the chunked `StageMedia` on the wire — verified: strict proto clippy, 94 proto tests, and workspace all-targets check pass
- [x] P1-T03 — Receive uploads on disk in the daemon — verified: full tests cover symlinked roots, confined publication, bounded naming, per-entry expiry cleanup, and real/fake Files parity
- [x] P1-T04 — Route every op to the anchor's host — verified: router/dispatch/app regressions cover all ops, stable host resolution, unknown-terminal refusal, and host-stamped paths
- [x] P1-T05 — Deadlines, ordering, and keeping the link healthy — verified: timeout regression and paused-clock 50 MiB/two-upload/10 Mbit/s fairness regression pass; link stays ready
- [x] P1-T06 — Advertise and gate `media.stage` — verified: exact capability assertion and old-host refusal/continued-endpoint regression pass
- [x] P1-T07 — Sweep staged downloads without touching anything Fleet did not write — verified: deterministic inspection/removal failures continue to later candidates; final lint and workspace tests pass
- [x] P1-T08 — Read the clipboard and dropped paths in the app — verified: 11 media regressions, strict fleet-app clippy, and the final fleet-app suite (1,247 unit + 8 integration tests) pass
- [x] P1-T09 — The upload driver: windowed, resumable, cancellable, with one continuation — verified: deterministic regressions cover stable handles/outcomes, 12-worker queuing, link races, expiry restart, progress and cancellation; full app suite passes
- [x] P1-T10 — Wire paste and drop into the two terminal surfaces — verified: popup refusal, pinned button-only Cancel, terminal-close copy and both surfaces are covered; manual matrix intentionally unfilled
- [x] P1-T11 — Move the docs with the code — verified: architecture, remote-machine, UX, design-system and ADR contracts match the final implementation; final lint and workspace tests pass

## Manual verification matrix (P1-T10)

This is the phase's real acceptance test; the harness cannot drive clipboards or OS file drags.
Every blank result cell is intentionally left for a human with the named desktop and remote host;
the integration pass does not convert source inspection or deterministic unit coverage into a
manual pass.

| Case | macOS | Linux (Wayland) | Linux (X11) | Notes |
| --- | --- | --- | --- | --- |
| Screenshot → paste into a **local** terminal | | | | |
| File copied in the file manager → paste into a **local** terminal | | | | macOS: original quoted path, no copy; Wayland/X11: GPUI text is preserved, not reinterpreted |
| Two files copied → paste | | | | macOS: two quoted paths; Wayland/X11: offered text/URI list remains byte-identical text |
| File drag → drop on a **local** terminal | | | | |
| Screenshot → paste into a terminal on a **Tailscale host** | | | | file in `~/Downloads/fleet/` there? inserted path is the remote one? |
| Plain text → paste into any terminal | | | | must be byte-identical to today |
| Folder drag → drop on a **local** terminal | | | | original folder path, no copy |
| Folder drag → drop on a **remote** terminal | | | | recreated under `~/Downloads/fleet/<stamp>-<name>/` there; symlinks skipped and counted |
| **200 MB file** → drop on a remote terminal, typing in another terminal on the same host meanwhile | | | | typing stays responsive; link never goes `Down`; progress toast advances |
| Kill the link mid-upload (e.g. `tailscale down` briefly, or stop the remote `fleetd connect`) | | | | toast says waiting; upload resumes and completes; file verifies |
| Drop onto a remote host that is currently reconnecting | | | | waits, then completes; or fails after 30 s naming the host |
| Cancel a remote upload from the toast | | | | `.part` gone on the remote |
| Close the terminal during a long upload | | | | toast tells where the file landed |
| Staged/remote drop over 1 GiB | | | | refused before any bytes move, message names the limit |
| Host running an older `fleetd` | | | | `Unsupported`, message names the host |
| Screenshot paste on **X11** | n/a | n/a | | source-supported per T01; manual result still required |

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-10 — Plan written. Key decisions taken up front, so they do not get re-litigated mid-phase:
  - **No `PROTOCOL_VERSION` bump.** The handshake matches the version literally, so a bump locks out every un-upgraded remote daemon. Optional behaviour is advertised as `MEDIA_STAGE_CAPABILITY = "media.stage"` instead (`rust-ipc-protocol` rule 8).
  - **All `MediaAnchor` arms ship in this phase**, so later phases add no wire surface.
  - **No chunking (superseded by the second revision below).** Frames are length-prefixed JSON capped at 16 MiB; base64 costs 4/3.
  - **`StageMedia` is deliberately not on the ordered terminal path.** Serialising a large write inline would stall keystrokes.
  - **Staging directory is `~/Downloads/fleet/` on the target machine**, falling back to `$FLEET_HOME/media`.
- 2026-09-24 — Re-grounded against `main` at `fa31272` (365 commits later; nothing had been implemented). Changes:
  - **`PROTOCOL_VERSION` is 8 now**, not 7. It still must not be bumped.
  - **`MediaAnchor::Thread { thread }` added.** Phase 2 now builds real composer attachments (user's call, 2026-09-24), which must live in the thread's own leaf directory `$FLEET_HOME/agents/attachments/<thread>/` on its owner host. That directory is not swept by T07.
  - **Cap raised from 8 to 10 MiB** to equal the per-image attachment limit already decided in `TODO.md` §5. 10 MiB × 4/3 ≈ 13.3 MiB still fits the 16 MiB frame.
  - **ADR number is 0026**; 0013 is now the SQLite transcripts ADR.
  - **The spawn guidance was wrong and is corrected.** The earlier "use `.detach()`, `detach_and_log_err` has zero uses" note contradicted `CLAUDE.md`. The rule: fallible → `detach_and_log_err` or stored; a closure that turns every failure into user-visible state and returns `()` may `.detach()` (`screens/board/lifecycle.rs:139-154`).
  - **Linux is a first-class platform.** T01 and the manual matrix cover Wayland and X11; pinned GPUI source confirms X11 image paste support.
  - **File paste is not "silently dropped" today; it is wrong.** `ClipboardItem::text()` flattens `ExternalPaths` with no separator or quoting. T08/T10 fix it for terminals; phase 3 fixes it for `TextInput`.
  - **Manual GUI checks no longer use `FLEET_DRIVE`** (ADR 0007 superseded by ADR 0016 `fleet-harness`). The harness cannot inject clipboards or OS drags, so the matrix above is manual.

- 2026-09-24 (second revision) — **Chunked, resumable upload replaces the single 10 MiB frame**, at the user's request for a seamless remote experience. Why: each remote link is one framed stream written one frame at a time, and any write over `WRITE_BUDGET` (10 s) marks the host `Down`. A 13 MiB frame on a 10 Mbit/s uplink takes ~11 s, so it would disconnect every terminal on that host, and even on fast links it queues keystrokes behind it. Decisions:
  - **One `StageMedia { anchor, upload, op }` variant**, with `op = Begin | Chunk | Finish | Cancel`. Every op carries its anchor, so routing stays stateless. `UploadId` is app-generated.
  - **128 KiB chunks, window of 4 per host, 1 GiB and 10 000 entries per drop, 3 min idle expiry.** Keystroke delay is bounded at ~0.55 s on a 10 Mbit/s uplink and proven by the T05 throttled-link test. The 10 MiB cap from the first revision is gone.
  - **Resumable across link blips**, because chunks are idempotent and the remote keeps the upload alive. Not resumable across an app restart.
  - **The app waits on a down link for up to 30 s** before starting, keyed on the observed `LinkState`, not on the broad `ErrorKind::Remote`.
  - **Folders are sent as a manifest of files**, not as tar: no new dependency, and every relative component is sanitised on the daemon. Symlinks and special files are skipped.
  - **SHA-256 per file** is checked on `Finish` (`sha2` is already a workspace dependency).
  - Upload tasks are **stored** in an app-side registry (so cancel = drop), which satisfies the `CLAUDE.md` spawn rule.

- 2026-09-25 — Integration and adversarial review closed the phase's remaining seams:
  - The local client rechecks `media.stage` after reconnect, matching the router's remote gate.
  - The receiver serializes allocation, caps each connection at 16 active uploads, creates every
    staging component without following symlinks, and retains a completed `Finish` result until
    idle expiry so a lost response is replayable.
  - Multi-path preparation and worker failures are aggregated in source order; successful siblings
    still insert, every failed item is named, and an empty top-level directory counts toward the
    10,000-entry staging limit.
  - The terminal drop veil wraps content only during an active drag and preserves the popup's
    explicit height. The harness tab-switching scenario now awaits the complete event-backed
    selection state instead of asserting while `settling_mutations` is nonzero.
  - The manual matrix remains deliberately blank for a human with real macOS, Wayland, X11, and
    remote machines; automated/source verification is not recorded as a manual pass.

- 2026-09-25 — Review remediation tightened the completed phase:
  - The app resolves remote terminal/thread anchors to a stable host before `Begin`; every daemon
    rejects terminal anchors it does not own, so mapping loss can never stage on the local machine.
  - A shared protocol cap keeps 12 app workers below the receiver's 16-upload owner ceiling.
    `stage` returns stable per-item ids, exposes progress/cancel, and reports one result per item.
  - Upload toasts are pinned against transient eviction, destructive activation is button-only,
    and dismiss explicitly cancels. Link transport failures wait for recovery without requiring
    the mirrored Down event to win the race.
  - Downloads and attachment roots are canonicalized once; ordinary `Files::rename` keeps its
    historical symlink semantics while media publication uses the confined `rename_part` path.
  - Harness cannot inject clipboard images or OS file drags by frozen contract, so no synthetic
    media gesture scenario was added; the existing 104-scenario corpus remains the non-regression gate.

- 2026-09-25 — Re-verification of the 31 review findings: 29 were already fixed by the remediation
  above; the two file-size findings were closed by mechanical splits (`media/upload/{driver,prepare}.rs`,
  `adapters/files/{fd,tests}.rs`). verified: `make lint` green; `make test` green except the
  `sessions_lifecycle` isolated children, which `env_clear()` into `/tmp` and hit the host's
  `/tmp` disk quota (os error 122) — environmental, not code.

### P1-T01 findings
(Record, for each platform and source, exactly which `ClipboardEntry` variants arrive and in what order. Everything downstream is built on this.)

| Source | macOS | Linux / Wayland | Linux / X11 |
| --- | --- | --- | --- |
| Screenshot to clipboard | **source:** one `Image`; PNG wins when present, otherwise the first supported image UTI (including TIFF) | **observed:** one `Image(Png)` from `grim - \| wl-copy --type image/png` | **source:** one `Image`; all supported image MIME atoms are tried before text |
| File copied in file manager (one) | **source:** `ExternalPaths([path])`, then `String(path)` | **source:** one `String` when an accepted text alias is offered, otherwise `None`; never `ExternalPaths` | **source:** `String` if an accepted text target is offered, otherwise `None`; never `ExternalPaths` |
| Files copied in file manager (two) | **source:** `ExternalPaths([path1, path2])`, then one newline-separated `String`, in source order | **observed/source:** `String("file:///tmp/fleet-one.png\nfile:///tmp/fleet-folder/\n")` from URI-list with text aliases; otherwise `None`; never `ExternalPaths` | **source:** one text `String` if offered, otherwise `None`; never `ExternalPaths` |
| Image copied from a browser | **source:** `String` wins if the browser offers UTF-8 plain text; otherwise one `Image` in `ImageFormat` priority | **source:** `String` wins if offered; otherwise one `Image` in `ImageFormat` priority | **source:** one `Image` wins if offered; otherwise `String` |
| Plain text | **source:** one `String` | **observed:** one `String`, byte content preserved for LF-only text | **source:** one `String` |
| File-manager drag of a file (`on_drop`) | **source:** `ExternalPaths([file])` | **source:** `ExternalPaths([file])` parsed from `text/uri-list` | **source:** `ExternalPaths([file])` parsed from XDND URI-list data |
| File-manager drag of a folder (`on_drop`) | **source:** `ExternalPaths([folder])`; directories are not filtered | **source:** `ExternalPaths([folder])`; directories are not filtered | **source:** `ExternalPaths([folder])`; directories are not filtered |

Pinned-source evidence: `gpui_macos/src/pasteboard.rs` reads file paths before strings before
images; its file-copy tests pin entry order. `gpui_macos/src/window.rs` passes every filename from
the drag pasteboard through. `gpui_linux/src/linux/wayland/clipboard.rs` reads accepted text before
images and has no clipboard URI-list-to-`ExternalPaths` path; Wayland drops parse `text/uri-list`
in `wayland/client.rs`. X11 `clipboard.rs::get_any` explicitly registers every supported image
atom before its text atoms, while XDND paths are parsed in `x11/client.rs`.

Deviation found and reconciled by T01: the roadmap/plan claim that pinned X11 had no image handling
was stale and is now corrected. At checkout `bebe92f` (Zed tag `v1.18.1`),
`x11/clipboard.rs::get_any` supports PNG, JPEG, WebP, GIF, SVG, BMP, TIFF, ICO, and PNM and prefers
them over text. X11 image paste is therefore not an upstream follow-up on source evidence. macOS
screenshot support is present, so T01 is not blocked.

### P1-T09 note — where the upload registry lives
(Record: a field on `AppState`, or its own entity — and why, per `gpui-state-and-memory`. Also how cancel is exposed: toast action or palette command.)

- 2026-09-24 — `UploadRegistry` is a plain field on `AppState`: it is app-wide orchestration
  state with no independent rendering or lifecycle identity, so a separate GPUI entity would add
  indirection without an ownership benefit. It retains every upload task and the shared per-host
  semaphore windows. Each progress toast carries `ToastTarget::MediaUpload`; its `Cancel` action
  drops the retained task and sends a best-effort protocol `Cancel`.

### P1-T10 note — manual verification

- 2026-09-24 — The matrix is intentionally unfilled. This agent cannot inject real OS clipboard
  contents or file-manager drags on macOS, Wayland, and X11. Deterministic GPUI coverage verifies
  local copied-path quoting without staging, byte-identical plain-text fallback, matching popup
  delivery, and the terminal-closed completion toast; the platform and remote gesture rows remain
  manual acceptance work.

### P1-T03 note — thread leaf directory
(Record the exact leaf spelling chosen — e.g. `$FLEET_HOME/agents/attachments/<thread-uuid>/` — phase 2's store writer and GC build on it.)

- 2026-09-24 — The exact leaf is `$FLEET_HOME/agents/attachments/<thread-uuid>/`, with `<thread-uuid>` spelled by `ThreadId`'s canonical UUID display.

### P1-T09 note — host resolution
The app can resolve a terminal's host, but no accessor exists. Path at `fa31272`:
`AppState::snapshot` (`state.rs:135-136`) → `Snapshot::sessions` (`fleet-proto/src/snapshot.rs:127-128`)
→ the `Session` whose `terminals` contain the id → `Session::host` (`fleet-core/src/sessions.rs:34-45`).
P1-T09 adds `AppState::host_of_terminal`. Do **not** route through `worktree_of`
(`screens/workspace/model.rs:282-292`) — it only resolves the displayed workspace session.

- 2026-09-24 — Implemented `AppState::terminal_host` in `state/snapshot.rs`, matching the
  current `AppState` organization, by distinguishing an unknown terminal from a known local
  session and a known remote host. Direct host anchors carry their host,
  and thread anchors resolve through the agent summary's host. The upload driver reads
  `AppState::host_link_state` and is woken by both snapshots and `HostLinkChanged`.

### Anchors confirmed 2026-09-24 — do not "fix" these toward generic advice
- **Errors reach the user** via `AppState::apply_toast_event(ToastLevel::Error, …)` (`state/notifications.rs:397-405`), which writes the sticky error. `show_sticky_error` (`screens/workspace/actions.rs:118-123`) is a private workspace helper, not shared API.
- **Daemon file writes go through the `Files` port.** The implementation added exclusive/no-follow private-directory and part-file operations, offset writes, SHA-256, safe part removal, and confined `rename_part`; ordinary callers retain `rename`. No `atomic_write_bytes` operation was needed.
- **`$HOME` is read with `std::env::var_os("HOME")`.** `dirs`, `home` and `directories` must not become direct dependencies.
- **Drop anchors**: `screens/workspace.rs:209` (in `render_prepared` at `:122`, before `with_keys` at `:228`) and `screens/agent_popup.rs:162` (in `render_prepared` at `:139`, before the overlay at `:174`). Hover-state pattern: `views/board_screen/drag.rs`.
- **Sweep hook**: one more `tokio::spawn` in `start_periodic_tasks`' `handles` (`services/maintenance.rs:87-109`). `SWEEP_DAYS` in the Makefile is about build artifacts — unrelated.
- **Goldens**: `assert_frame` is in `crates/fleet-proto/tests/support/mod.rs:14-41`; stay inside the crate's one integration-test binary.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Resuming an upload across an app restart (would need the registry persisted and a remote `Status` op).
- Harness support for image clipboards and OS file drags — needs an ADR, since `docs/TESTING-HARNESS.md` is frozen.
- X11 real-machine image paste remains a human matrix row; T01 resolved the suspected upstream gap by finding pinned GPUI image support.
