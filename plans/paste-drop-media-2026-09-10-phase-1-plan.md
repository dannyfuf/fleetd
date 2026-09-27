# Phase 1 — Media staging and the terminal gesture — Plan

> Tracker: ./paste-drop-media-2026-09-10-phase-1-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.
>
> **Revised 2026-09-24** against `main` at `fa31272`. Every `path:line` below was re-verified at
> that commit. If you start later than that, expect line numbers to drift, but not the shapes.
> Same day, second revision: **chunked, resumable upload** replaced the single-frame 10 MiB
> request, so dropping onto a remote machine feels the same as dropping onto a local one (see
> *Remote experience*).

## Summary

Pasting an image into a Fleet terminal does nothing, and dragging a file onto the window does
nothing. Pasting a copied *file* inserts the wrong thing: GPUI's `ClipboardItem::text()`
flattens `ExternalPaths` by writing every path back to back, unquoted and with no separator (pinned
`gpui/src/platform.rs:2403-2417`).

This phase makes all of these gestures right: pasting an image, pasting a copied file, and dropping
files or folders. When the payload is not plain text, Fleet puts the bytes on disk **on the machine
that owns the target terminal** and types the resulting absolute path into it, shell-quoted. When
the file is already on that machine, the original path is inserted and nothing is copied.

The machine part is the point. Fleet runs one `fleetd` per machine and the local daemon federates
remote ones (`docs/REMOTE-MACHINES.md`). A terminal shown in the app may be a PTY on a Tailscale
devbox. A path on the Mac is worthless to an agent reading it there.

This phase also puts the whole wire surface on the wire, including the `Thread` anchor that phase
2's composer attachments need, so the later phases add no protocol.

## Remote experience

The goal: dropping a file onto a remote terminal feels like dropping it onto a local one. Concretely:

- **Any reasonable size works.** Files move in 128 KiB chunks, with a sanity limit of 1 GiB per
  drop, not 10 MiB.
- **The host never drops because of an upload,** and typing into its terminals stays responsive
  during one. No frame is larger than ~171 KiB. At most 4 chunks (≈ 683 KiB on the wire) are in
  flight per upload, which bounds the extra wait in front of a keystroke to about 0.55 s on a
  10 Mbit/s uplink, and less on anything faster.
- **Progress is visible and cancellable:** "Copying design.pdf to devbox… 40%".
- **A link that blips does not lose the drop.** An upload started while the host is reconnecting
  waits for it. An upload interrupted mid-way resumes from what the remote already has, because
  chunks are idempotent and the remote keeps a half-finished upload alive for a while.
- **Folders work.** A dropped folder is recreated under `~/Downloads/fleet/` on the remote, and
  its path is inserted.
- **The result is verified.** Each file is SHA-256-checked on the remote before its path is
  handed back.

## Sizing call

**Phased**: phase 1 of 3; see ./paste-drop-media-2026-09-10-roadmap.md.

This is a vertical slice across `fleet-proto`, `fleet-daemon`, `fleet-client`, `fleet-app` and
`docs/`, and it carries the decisions that are expensive to revisit: the wire shape, the chunking
and resume model, the capability string, the routing rule, both staging directories, the limits
and the sweep policy. Chunked upload makes it roughly a third bigger than the single-frame design.
It stays one phase, because shipping the single-frame version first would put a request on the wire
that has to be kept compatible forever and that can take a remote link down.

## Repository context

Verified at `fa31272`.

- **Workspace:** 12 crates (`fleet-drive` and `fleet-harness` joined since the first draft),
  edition 2024, resolver 3, toolchain 1.97.1, GPUI from Zed tag `v1.18.1` (`Cargo.toml:2-20,61-66`).
- **Make targets:**
  - `make lint` = fmt-check plus full clippy (`Makefile:137-146`).
  - `make test` builds `fleet-daemon`, `fleet-app` and `fleet-harness`, then runs
    `cargo test --workspace` (`Makefile:84-89`).
  - `make check` (`Makefile:81-82`).
  - `make restart` after any daemon change.
  - `make harness` for any change a human would see (`CLAUDE.md`).
- **Layering:** `core <- proto <- {term, client, cli} <- {daemon, app}`. `ui-kit` depends only on
  `gpui` (`docs/ARCHITECTURE.md:54-55`).
- **Platforms:** macOS, plus Linux on both Wayland and X11. Both Linux backends are product
  backends, not a convenience (`docs/DEVELOPMENT.md` §GPUI platform backends). The pinned GPUI's
  Wayland clipboard handles `ClipboardEntry::Image`
  (`gpui_linux/src/linux/wayland/clipboard.rs`). T01's source audit also found that X11 tries PNG,
  JPEG, WebP, GIF, SVG, BMP, TIFF, ICO, and PNM before text; image paste is source-supported on
  both Linux backends, with the real-machine result retained in the manual matrix.
- **Wire:** length-prefixed JSON, `PROTOCOL_VERSION = 8` (`crates/fleet-proto/src/lib.rs:4`),
  `MAX_FRAME_SIZE = 16 MiB` (`codec.rs:10-11`). The handshake matches the version exactly
  (`crates/fleet-daemon/src/server/connection.rs:639-687`), so a version bump locks out every
  un-upgraded remote. Optional behaviour is advertised as a capability instead.
- **Capabilities:**
  - Constants live in `crates/fleet-proto/src/lib.rs:6-93` and `response.rs:53-83`.
  - They are advertised from `server/connection.rs:965-990` (production) and `:1075-1099` (test
    vector).
  - A byte-exact assertion at `:1104-1127` pins all 18 strings.
  - Remote hellos carry `RemoteHello.capabilities` (`crates/fleet-daemon/src/machines/link.rs:48-55`).
- **Goldens:** `assert_frame` now lives in `crates/fleet-proto/tests/support/mod.rs:14-41`.
  `tests/compatibility.rs` imports it. The testing skill requires one integration-test binary per
  crate (`.claude/skills/rust-gpui-testing/SKILL.md:264-279`), so add to the existing binary rather
  than a new `tests/*.rs`.
- **Existing paste path:** `route_paste` at `crates/fleet-app/src/terminal/surface.rs:956-970`
  reads `item.text()` and delivers `PendingInput::Paste` (`surface.rs:75-94`, converted to
  `RequestBody::PasteTerminal` at `:81-85`). Callers:
  - `screens/workspace/terminal.rs:285-302` (route at `:296`), action at
    `screens/workspace/actions.rs:504-508`;
  - `screens/agent_popup/terminal.rs:423-436`, action in `screens/agent_popup/actions.rs:11-16`.
- **Delivery gate:** `TerminalSurface::send_or_queue` (`surface.rs:308-342`) is `#[must_use]`.
  The bounded queue rejects the newest event. Queued input is dropped when the terminal or the link
  generation changes (`screens/workspace/lifecycle.rs:80-100`, clear at `:95-96`).
- **Drag-and-drop exists now, but only for board cards.** `CardDrag` with `on_drag`,
  `on_drag_move::<CardDrag>` and `on_drop::<CardDrag>` (`views/board_screen.rs:645-690,752-784`,
  payload in `views/board_screen/drag.rs:21-83`). Kit drop visuals are in
  `fleet-ui-kit/src/components/drop_slot.rs`. **No `ExternalPaths` or `can_drop` use exists.**
  Follow the board's typed-payload and hover-state pattern. GPUI APIs are in the pinned checkout:
  `ClipboardEntry` at `gpui/src/platform.rs:2349-2355`, `Image` at `:2560-2566`, `ExternalPaths` at
  `gpui/src/interactive.rs:683-690`, and `on_drag_move`/`on_drop`/`can_drop` at
  `gpui/src/elements/div.rs:338-350,548-563`.
- **Drop anchors:**
  - workspace: root `div()` at `screens/workspace.rs:209`, inside `render_prepared` (`:122`),
    before `with_keys` (`:228`);
  - agent popup: `card` at `screens/agent_popup.rs:162`, inside `render_prepared` (`:139`),
    before the overlay (`:174`).
- **Routing:**
  - `PasteTerminal` classifies via `host_or_local(resolver.host_of_terminal(..))` at
    `services/router/classify.rs:86-101`. The second, explicit match lists `PasteTerminal` at
    `:432`.
  - Terminal ids are rewritten for the remote in `services/router/translate.rs:109-125`.
  - Agent requests route by `resolver.host_of_thread` (`services/router/agents.rs:340-361`,
    resolver in `router/mod.rs:44-53`). A `ThreadId` is a global UUID and is **not** rewritten
    (`translate.rs:22-52`).
  - `ResponseBody::Path { path, host }` is at `fleet-proto/src/response.rs:427-434`.
    `WorktreePath` returns it (`dispatch.rs:563-566`), and remote ownership is stamped in
    `translate.rs:277`.
- **Deadlines and ordering:**
  - `request_timeout` is at `crates/fleet-client/src/connection.rs:680-761`, now with many
    documented exemptions.
  - The PTY ordering predicates are at `server/connection.rs:759-784`.
  - Agent mutations have their own per-thread predicate (`fleet-proto/src/request.rs:1152-1180`).
- **Daemon file IO:**
  - The `Files` port (`adapters/files.rs:83-132`) has `atomic_write_text` (`:91-92`, real impl
    `:361-392`) and **no bytes method**.
  - `ConfigStore::save` shows the `spawn_blocking` idiom (`stores/config.rs:110-122`).
  - Direct `tokio::fs` writes also exist now (`adapters/git.rs:157-165`, `services/sessions/holder.rs:262-265`).
    The port is still the right home, because it is what the fake tests against.
- **Periodic tasks:** `start_periodic_tasks` at `services/maintenance.rs:76-112` spawns 11 tasks
  (`:87-109`). Add one more the same way.
- **Error kinds:** `NotFound`, `Validation`, `Fs`, `Remote` and `Unsupported` all exist (`fleet-proto/src/error.rs:8-32`).
- **Paths:**
  - `FleetHome::agents_attachments_path()` resolves `$FLEET_HOME/agents/attachments`
    (`crates/fleet-core/src/paths.rs:61-75`).
  - Claude's `--add-dir` expects one *leaf* directory per thread, so siblings are not granted
    (`crates/fleet-daemon/src/agents/claude/argv.rs:23-33`).
  - `$HOME` is read with `std::env::var_os("HOME")`. `dirs`/`home` are only transitive and must
    not become direct dependencies.
- **Dependencies:** `base64` (`Cargo.toml:55`), `sha2` (`:82`), `shell-words` (`:83`) and `uuid`
  with `v4`/`serde` (`:98`) are all in `[workspace.dependencies]` already. None of them needs a new
  root entry; crates opt in with `foo.workspace = true`. No new third-party dependency is needed.
- **The remote link is one framed stream, written one frame at a time.** Each remote host has one
  link actor over one `Framed` transport (`crates/fleet-daemon/src/machines/link.rs:46`). Any
  single write that exceeds `WRITE_BUDGET` (10 s, `:39-44`, enforced at `:341` and `:503`) turns
  the link `Down`, failing every pending request and disconnecting every terminal and thread on
  that host until backoff reconnects. A remote daemon stops reading once it holds
  `MAX_PENDING_REQUESTS = 64` (`server/connection.rs:41`). **This is why this phase sends files
  in small chunks and never as one large frame**: a 13 MiB frame on a 10 Mbit/s upload takes
  ~11 s and would take the host down. Even on a fast link, it would queue every keystroke to that
  host behind it.
- **An unreachable host** returns `ErrorKind::Remote` ("host … is unreachable",
  `services/router/mod.rs:718`). That kind is broad, so the app decides whether to wait for a
  reconnect from the host's observed `LinkState` (`fleet-proto/src/snapshot.rs:56,75`, updated by
  the link event at `event.rs:205`), not from the error. `ResponseBody::Ack` exists for unit
  replies (`response.rs:398`).
- **App state and errors:**
  - Toasts: `AppState::toast` (`state/notifications.rs:331-334`), `toast_short` (`:388-395`),
    `apply_toast_event` (`:397-405`, `ToastLevel::Error` writes the sticky error).
  - `show_sticky_error` is a private helper in `screens/workspace/actions.rs:118-123`. Use
    `apply_toast_event` from shared code.
- **Terminal → host in the app:** `AppState::snapshot` (`state.rs:135-136`) →
  `Snapshot::sessions` (`fleet-proto/src/snapshot.rs:127-128`) → the `Session` whose `terminals`
  hold the id → `Session::host` (`fleet-core/src/sessions.rs:34-45`). No app code reads
  `Session::host`, and `AppState::host_of_terminal` does not exist. Do not use `worktree_of`
  (`screens/workspace/model.rs:282-292`); it resolves only the displayed session.
- **Spawn rule (`CLAUDE.md`):** a fallible task ends in `.detach_and_log_err(cx)` or is stored.
  Two shapes are in use:
  - a `Task<Result<_>>` with `detach_and_log_err` (`screens/jobs/actions.rs:119-138`);
  - an infallible closure that turns every failure into handled state and returns `()`, then
    `.detach()` (`screens/board/lifecycle.rs:139-154`).
  This flow must *show* its errors to the user, not just log them, so use the second shape. A
  `.detach()` on a closure that drops an error is never acceptable.
- **OSC 52 (unrelated but adjacent):** terminal programs can now *write* the user clipboard
  (`shell/root/clipboard.rs:9-50`, `Event::TerminalClipboard`). It does not touch the paste path.
  Do not route through it.
- **Help:** terminal clipboard guidance is data in `dialogs/help/guides.rs:338-360`. The old
  `TERMINAL_CLIPBOARD` constant is gone.
- **Harness limits:** `clipboard set/get` fails in every lane on purpose, and `drag` cannot inject
  an OS file drag (`docs/TESTING-HARNESS.md:70-80`). No scenario can prove this gesture. Run
  `make harness` for non-regression only.
- **ADRs:** the highest is 0025 (`docs/README.md:30-56`). Use **0026**.
- **Skills to load:**
  - `rust-ipc-protocol` for proto, server loop and router;
  - `rust-workspace-architecture` for dependencies, modules, errors and logs;
  - `rust-async-background-work` for the executor read, `spawn_blocking` and the spawn shape;
  - `gpui-app-shell` for the drop targets;
  - `gpui-styling` for the drop affordance;
  - `rust-gpui-testing` for tests;
  - `zed-quality-review` before done.

## Assumptions

- `~/Downloads/fleet/` on the target machine is the staging directory for `Local`, `Terminal` and
  `Host` anchors. `Thread` anchors stage into that thread's leaf directory under
  `$FLEET_HOME/agents/attachments/` (see T03).
- **Limits** (constants in `fleet_proto::media`; tune them in one place):
  - `CHUNK_BYTES = 128 KiB` decoded, which is ≈ 171 KiB of base64 per frame;
  - `UPLOAD_WINDOW = 4` chunks in flight per upload;
  - `MAX_UPLOAD_BYTES = 1 GiB` total per drop;
  - `MAX_UPLOAD_FILES = 10 000` entries per folder drop;
  - `UPLOAD_IDLE_EXPIRY = 3 min` on the daemon.
  The keystroke-latency bound in *Remote experience* follows from chunk size × window. Changing
  either constant changes that promise, so the doc comment carries the arithmetic.
- A multi-file drop stages each item as its own upload, concurrently, but shares **one** window
  budget per host, so two big files do not double the keystroke delay. All paths are inserted
  space-separated, in drop order, once every upload has finished. If one fails, the others are
  still inserted, and the failure is reported by name.
- Symlinks inside a dropped folder are skipped, not followed, and the toast says how many. Special
  files (sockets, fifos, devices) are skipped the same way.
- Inserted text is bare paths, quoted with `shell_words::quote` only when needed. No trailing
  newline and no agent-specific syntax.

## Out of scope

- The composer's attachment chips, the store writer and the provider mapping (phase 2). Dialog
  `TextInput`s (phase 3).
- Pasting *out* of Fleet (remote → Mac).
- Resuming an upload across an **app** restart. A link blip or a local-daemon reconnect is in
  scope; quitting the app mid-upload cancels it.
- Compression, deduplication, previews, and a configurable staging directory.
- Extending the frozen harness contract to inject images or file drops (a follow-up that needs its
  own ADR).

## Affected areas

**`crates/fleet-proto`**
- `Cargo.toml` — `base64.workspace = true`, `uuid.workspace = true` if not already present
- `src/lib.rs` — `MEDIA_STAGE_CAPABILITY`, `pub mod media`
- `src/request.rs` — `RequestBody::StageMedia`, `MediaAnchor`, `StageOp`, `StageEntry`, `UploadId`
- existing `ResponseBody::Ack` and `ResponseBody::Path` are reused; no `response.rs` change
- `src/media.rs` (new) — limits, `encode`, `decode`
- `tests/compatibility.rs` — goldens

**`crates/fleet-daemon`**
- `src/adapters/files.rs` — the part-file operations, real and fake
- `src/services/media.rs` (new) and, if it passes ~900 lines, `src/services/media/` submodules for
  the upload table, the sanitiser and the sweep
- `src/services/mod.rs`, `src/services/composition.rs`, `src/services/dispatch.rs`
- `src/services/router/classify.rs`, `src/services/router/translate.rs`
- `src/services/maintenance.rs`
- `src/server/connection.rs` — capability vectors, golden, ordering comment

**`crates/fleet-client`**
- `src/connection.rs` — `request_timeout` arms

**`crates/fleet-app`**
- `Cargo.toml` — `shell-words`, `sha2`, `uuid` via `.workspace = true`
- `src/media.rs` (new; split into `media/` submodules if the upload driver grows), `src/lib.rs`
- `src/bridge/requests.rs` — `Bridge::stage_media`
- `src/state/snapshot.rs` — `AppState::host_of_terminal` and host link-state accessors, matching
  the current snapshot-owned `AppState` organization
- the workspace and agent-popup paste actions and screen roots listed above
- `src/dialogs/help/guides.rs` — clipboard guide rows

**`docs/`**
- `REMOTE-MACHINES.md` §3, `ARCHITECTURE.md` (service list, protocol compatibility at `:542`),
  `UX-SPEC.md`, `KEYMAP.md`
- `decisions/0026-media-staging.md` (new), and its `README.md` row

## Tasks

### P1-T01 — Confirm what the clipboard and a file drop actually give GPUI, on both platforms

**Intent:** before any wire work, find out which payloads really arrive as
`ClipboardEntry::Image` / `ExternalPaths` on macOS and on Linux.

**Touches:** a throwaway example under `crates/fleet-app/examples/`. It must compile under
`--all-targets`, so either keep it tidy or delete it when the task closes, and record which.

**Steps:**
- Write a tiny GPUI example that dumps `cx.read_from_clipboard()`'s entries: the variant, plus
  `ImageFormat` and byte length for `Image`, and the paths for `ExternalPaths`. Attach a temporary
  `on_drop::<ExternalPaths>` to its root and log drops.
- On **macOS**, record:
  1. a `cmd-shift-ctrl-4` screenshot,
  2. a Finder `cmd-c` of one file, and of two,
  3. an image copied from a browser,
  4. plain text,
  5. a Finder drag of a file, and of a **folder**.
- On **Linux / Wayland** (Hyprland here), record: a `grim -g "$(slurp)" - | wl-copy` screenshot, a
  file copied in a file manager, an image copied from a browser, plain text, and a file-manager
  drag of a file and of a folder.
- On **Linux / X11**, or XWayland if no X11 session is available, record at least the screenshot
  and the plain-text case. The pinned source audit found image atoms ahead of text atoms; retain a
  real-machine matrix row because source support does not prove compositor interoperability.
- If a macOS screenshot yields no `Image` entry, stop and re-plan. That feature would need a
  pasteboard read GPUI does not expose.

**Verification:** the example runs on each platform, and the results table in the tracker is
filled. `make lint` clean.

**Done when:** the tracker states, per platform and per source, exactly which variants arrive and
in what order, and whether a dropped folder arrives as its own path.

### P1-T02 — Put the chunked `StageMedia` on the wire

**Intent:** one additive request that carries a whole upload lifecycle, a four-arm anchor, the
limits and the capability, with byte-exact goldens and no version bump.

**Touches:** `crates/fleet-proto/{Cargo.toml, src/lib.rs, src/request.rs, src/media.rs, tests/compatibility.rs}`.

**Steps:**
- Load `rust-ipc-protocol`. Do **not** bump `PROTOCOL_VERSION` (it stays 8).
- **One request variant, so that routing is one arm and stays stateless:**

  ```rust
  StageMedia { anchor: MediaAnchor, upload: UploadId, op: StageOp }
  ```

  Every op carries the anchor. The router therefore needs no upload → host table, and a local
  daemon restart mid-upload loses nothing that routing depends on.
  - `UploadId` is a `uuid` newtype generated by the **app**, so a retried `Begin` is idempotent.
  - `MediaAnchor` is `#[serde(tag = "type", rename_all = "snake_case")]`, with the arms `Local`,
    `Terminal { terminal: TerminalId }`, `Host { host: HostId }` and
    `Thread { thread: ThreadId }`.
  - `StageOp`:
    - `Begin { entry: StageEntry }` → `Ack`. Idempotent for the same `upload` and the same entry.
    - `Chunk { file: u32, offset: u64, data: String }` → `Ack`. `data` is base64 of at most
      `CHUNK_BYTES`. The `offset` must be a multiple of `CHUNK_BYTES`. Idempotent: re-sending an
      already written chunk is a success.
    - `Finish { sha256: Vec<String> }` → `ResponseBody::Path { path, host }`. It carries one hex
      digest per file, in manifest order.
    - `Cancel` → `Ack`. Idempotent, and a success for an unknown upload.
  - `StageEntry`:
    - `File { name: String, size: u64 }`;
    - `Directory { name: String, files: Vec<StagedFile>, dirs: Vec<String> }`, where
      `StagedFile { relative: String, size: u64 }`. Relative paths use `/` and are validated on the
      daemon, never trusted. `dirs` recreates empty directories.
  - The daemon distinguishes "I do not know this upload" (it expired, or the remote restarted) as
    `ErrorKind::NotFound`, which already exists (`fleet-proto/src/error.rs:9-10`). The app keys
    its restart-from-zero on that kind.
- `base64.workspace = true` (and `uuid.workspace = true`, if not already present) in
  `fleet-proto/Cargo.toml`.
- In `lib.rs`, a documented `pub const MEDIA_STAGE_CAPABILITY: &str = "media.stage";`.
- New `src/media.rs` holds `CHUNK_BYTES`, `UPLOAD_WINDOW`, `MAX_UPLOAD_BYTES`,
  `MAX_UPLOAD_FILES`, `UPLOAD_IDLE_EXPIRY`, `encode` and `decode`. `decode` rejects data longer
  than one chunk by its *encoded* length, before allocating. Doc-comment the arithmetic:
  - frame size: 128 KiB × 4/3 ≈ 171 KiB, far under `MAX_FRAME_SIZE`;
  - write time: far under `WRITE_BUDGET` on any link fast enough to use;
  - keystroke delay: at most `UPLOAD_WINDOW` × frame bytes on the host's link.
- Goldens in `tests/compatibility.rs` via `assert_frame`: each op, each anchor arm, both entry
  kinds, and `ResponseBody::Path` with and without `host`. Write the fixtures by hand; do not copy
  the serializer's output.

**Verification:** `cargo test -p fleet-proto`; `make lint`.

**Done when:** every op, anchor and entry kind round-trips through byte-exact goldens,
`PROTOCOL_VERSION` is still 8, and the constants exist with their arithmetic.

### P1-T03 — Receive uploads on disk in the daemon

**Intent:** a daemon service that assembles chunked uploads safely into the right directory,
verifies them, and returns the absolute path.

**Touches:** `adapters/files.rs`, `services/media.rs` (new), `services/mod.rs`,
`services/composition.rs`, `services/dispatch.rs`.

**Steps:**
- Load `rust-workspace-architecture` and `rust-async-background-work`.
- **`Files` port additions** (real and fake), so the service is testable against the fake:
  - create a part file of a given size;
  - write bytes at an offset;
  - read back for hashing (or hash inside the port);
  - rename a part file or part directory onto its final name;
  - remove a part tree.
  All are synchronous. The service calls them through `spawn_blocking`, as `ConfigStore::save`
  does (`stores/config.rs:110-122`).
- **Directories.** `Media::new(home: &FleetHome)` resolves the downloads directory once and logs
  the choice with a qualified `tracing::info!`:
  1. `$HOME/Downloads/fleet`, when `$HOME` is set and `$HOME/Downloads` exists and is writable;
  2. otherwise `$FLEET_HOME/media`.
  It is created `0700`. **Thread** targets use `home.agents_attachments_path().join(<thread-id>)`,
  one leaf per thread, as Claude's `--add-dir` expects. Create the leaf `0700` on demand, after
  checking that the thread exists on this daemon. Record the leaf spelling in the tracker; phase 2
  builds on it.
- **The upload table** is in memory, keyed by `UploadId`. Each entry holds:
  - the target;
  - the sanitised final path;
  - the part path;
  - the manifest;
  - a received-chunk bitmap per file;
  - `last_activity`.
  Handle each op as follows:
  - **`Begin`:**
    - validate the entry: total size ≤ `MAX_UPLOAD_BYTES`, file count ≤ `MAX_UPLOAD_FILES`, and
      every relative path sanitised;
    - check free space where it is cheap to do so;
    - create `<stamp>-<name>.part` (a file) or `<stamp>-<name>.part/` (a directory tree);
    - a repeated `Begin` with the same id and entry returns `Ack` without re-creating anything.
  - **`Chunk`:**
    - bounds-check `file` and `offset`, and check the length (full chunks except the last);
    - write at the offset;
    - mark the bitmap;
    - an already-marked chunk is `Ack` without rewriting it.
  - **`Finish`:**
    - require every bitmap to be full;
    - SHA-256 each file and compare it with the digest sent;
    - rename the part onto the final name;
    - drop the entry;
    - return the absolute path;
    - a mismatch is `ErrorKind::Validation` naming the file, and it deletes the part.
  - **`Cancel`:** delete the part and drop the entry.
  - **Idle expiry:** a periodic check removes entries idle longer than `UPLOAD_IDLE_EXPIRY`, and
    their parts.
  - **Concurrency:** chunks for one upload may arrive concurrently from the unordered pool. Guard
    each entry so that bitmap updates and writes to one file do not race.
- **Sanitising** names and every relative component:
  - reject empty, `.`, `..` and absolute components;
  - strip control characters and separators within a component;
  - keep the extension;
  - truncate a component to 64 characters (stem-first for the top-level name);
  - prefix the top-level name with a UTC `%Y%m%d-%H%M%S-` stamp;
  - if a name sanitises to nothing, use `paste`.
  Two relative paths that sanitise to the same thing are a `Validation` error, not a silent
  overwrite.
- An IO failure is `ErrorKind::Fs` with context naming the path. Validation failures name the limit
  they hit.
- Dispatch arm for `RequestBody::StageMedia`, reached only after routing has decided this daemon
  owns the request. `Local` and `Terminal` target the downloads directory. `Thread` targets the
  leaf. `Host` has already been rewritten to `Local` by T04.
- Tests (adversarial, against both targets):
  - `../../etc/passwd`, `foo/bar.png`, a 300-character name, an empty name, only dots, a newline
    and a NUL in the top name and in relative components; nothing escapes;
  - out-of-order chunks; duplicate chunks; a `Finish` with a missing chunk; a wrong digest; a
    repeated `Begin`; `Cancel` of an unknown id;
  - expiry removes the part (paused clock);
  - a folder with nested empty dirs is recreated exactly.

**Verification:** `cargo test -p fleet-daemon media`; `make lint`; `make test`.

**Done when:** a local upload of a file and of a folder lands verified in the right directory, every
crafted name stays inside it, and a half-finished upload is removed on cancel or expiry.

### P1-T04 — Route every op to the anchor's host

**Intent:** the daemon that owns the anchor serves every op of the upload.

**Touches:** `services/router/classify.rs`, `services/router/translate.rs`.

**Steps:**
- `classify.rs`: one `StageMedia { anchor, .. }` arm, for every op:
  - `Local` → local;
  - `Terminal { terminal }` → `host_or_local(resolver.host_of_terminal(*terminal))`, as
    `PasteTerminal` does (`:86-101`);
  - `Host { host }` → that host;
  - `Thread { thread }` → `resolver.host_of_thread(thread)`, as `AgentSend` does
    (`router/agents.rs:340-361`).
  Also list `StageMedia` in the explicit second match (beside `PasteTerminal` at `:432`), so it is
  never defaulted.
- `translate.rs`, when forwarding:
  - `Terminal` → rewrite the id (`:109-125`);
  - `Host { host }` → `Local`;
  - `Thread` → unchanged, because `ThreadId` is global (`:22-52`);
  - `Local` → never forwarded;
  - `UploadId` → never rewritten, because the app generated it and it is globally unique.
- The `Path` from `Finish` comes back stamped with the host, as `WorktreePath` is
  (`translate.rs:277`).
- Tests with `FakeMachine` / `FakeRemote` (`crates/fleet-daemon/src/testing/machines.rs:33-203`):
  - every op of a remote `Terminal` upload arrives with the translated terminal id and the same
    upload id;
  - a remote `Thread` upload arrives unchanged;
  - `Finish` returns a host-stamped `Path`.

**Verification:** `cargo test -p fleet-daemon router`; `make test`.

**Done when:** remote `Terminal`, `Host` and `Thread` uploads are served remotely end to end and
come back labelled with their host.

### P1-T05 — Deadlines, ordering, and keeping the link healthy

**Intent:** an upload never stalls keystrokes noticeably and never trips `WRITE_BUDGET`, and that is
proven rather than assumed.

**Touches:** `crates/fleet-client/src/connection.rs:680-761`, `server/connection.rs:759-784`,
`fleet-proto/src/request.rs:1152-1180`, a daemon router test.

**Steps:**
- `request_timeout`: give each op its own documented arm:
  - `Chunk` 30 s;
  - `Begin` 30 s;
  - `Finish` 120 s, because it hashes up to 1 GiB;
  - `Cancel` the default.
  Write the reasoning in a comment in the style of its neighbours.
- **`StageMedia` is not ordered.** Say so, with the reason, beside the PTY ordering predicates and
  beside the per-thread agent-mutation predicate. It does not write to a PTY and changes no
  thread state.
- **Prove fairness against a slow link.** Use a `FakeRemote` whose transport is throttled (for
  example 10 Mbit/s, on a paused clock). While a 50 MiB upload runs:
  - a `PasteTerminal` to the same host completes within the bound stated in `media.rs`;
  - no single frame write exceeds a small fraction of `WRITE_BUDGET`;
  - the link never goes `Down`.
  If the throttled fake does not exist, build it in `testing/machines.rs` as part of this task.
- The window is per **host**, not per upload (see Assumptions). The app enforces it in T09. This
  task's test covers two concurrent uploads to the same host.

**Verification:** `cargo test -p fleet-client`; `cargo test -p fleet-daemon`; `make lint`.

**Done when:** the timeouts and ordering comments exist, and the throttled-link test passes with the
link staying up.

### P1-T06 — Advertise and gate `media.stage`

**Intent:** an old remote `fleetd` produces a clear message, not a protocol error.

**Touches:** `server/connection.rs:965-990`, `:1075-1099`, `:1104-1127`, and the router's remote
forward.

**Steps:**
- Add `MEDIA_STAGE_CAPABILITY` to both vectors and update the byte-exact assertion. The list is
  compared exactly, so both vectors keep the same order.
- Before forwarding any `StageMedia` op to a remote, check that host's
  `RemoteHello.capabilities`. If the capability is missing, return `ErrorKind::Unsupported` with a
  sentence that names the host and says its `fleetd` needs updating. Keep the connection open.
- Test with a `FakeRemote` whose hello lacks the capability: `Begin` comes back `Unsupported`, and
  the daemon stays up.

**Verification:** `cargo test -p fleet-daemon`; `make test`.

**Done when:** a fresh daemon advertises `media.stage`, and staging to a host without it returns
`Unsupported` naming the host.

### P1-T07 — Sweep staged downloads without touching anything Fleet did not write

**Intent:** `~/Downloads/fleet/` does not grow forever, and the sweep can never delete a user's
own file.

**Touches:** `services/media.rs`, `services/maintenance.rs`.

**Steps:**
- `Media::sweep(older_than) -> DaemonResult<usize>` may delete an entry only if **all** of these
  hold:
  1. it sits directly in the resolved downloads directory;
  2. its name matches `^\d{8}-\d{6}-`;
  3. it is a regular file, or a directory whose whole tree contains only regular files and
     directories. A tree containing a symlink or a special file is left alone. Removal never
     follows a symlink.
  Leftover `*.part` entries with the stamp that are not in the live upload table are removed after
  1 day, whatever their age rule. Write all of this as the function's doc comment.
- Test it with: a user file, a user directory, a symlink pointing outside, a stamped directory
  containing a symlink (kept), a real staged file, a real staged folder, and an orphaned `.part`.
- The sweep **never** walks `$FLEET_HOME/agents/attachments`. Phase 2's store GC owns those. Add a
  test that a stamped file in a thread leaf survives.
- Add one more `tokio::spawn` in `start_periodic_tasks`' `handles` (`maintenance.rs:87-109`), taking
  the same `CancellationToken`. Retention is 7 days, as a named constant in `media.rs`. The upload
  idle-expiry check can ride the same task on a shorter tick. `SWEEP_DAYS` in the `Makefile` is
  unrelated.
- Handle every fallible call: `?` with context, or `if let Err` with a qualified `tracing::warn!`.
  Drive the test with a paused tokio clock.

**Verification:** `cargo test -p fleet-daemon media`; `make test`.

**Done when:** stale staged files, folders and orphaned parts go; user files, directories, symlinks
and thread attachments provably stay.

### P1-T08 — Read the clipboard and dropped paths in the app

**Intent:** one place that turns a clipboard or a drop into a typed `Attachment`, and that fixes
the flattened-paths paste.

**Touches:** `crates/fleet-app/src/media.rs` (new), `src/lib.rs`, `Cargo.toml`.

**Steps:**
- New module `media.rs`:
  - `pub(crate) enum Attachment { Paths(Vec<PathBuf>), Blob { name: String, format: ImageFormat, bytes: Vec<u8> } }`.
  - `from_clipboard(item: &ClipboardItem) -> Option<Attachment>`: prefer `ExternalPaths`, then
    `Image`. Return `None` for a text-only item, so text paste stays untouched. Taking the item
    rather than `cx` lets phases 2 and 3 reuse it on a kit event's payload.
  - `from_external_paths(&ExternalPaths) -> Attachment`.
  - `manifest(path) -> Result<StageEntry, …>`, run on the background executor. It stats a file, or
    walks a folder without following symlinks, collects relative paths, sizes and empty dirs,
    counts what it skipped, and enforces `MAX_UPLOAD_BYTES` / `MAX_UPLOAD_FILES` **before any byte
    is sent**.
  - `insert_text(&[PathBuf]) -> String`: `shell_words::quote` each path and join with single
    spaces.
  - Blob name: stem `clipboard`, extension from `ImageFormat`. Do not sniff bytes.
- Add `shell-words`, `sha2` and `uuid` with `.workspace = true` to `fleet-app`.
- Match variants exactly as T01 recorded.
- Unit-test `insert_text` (a space, a quote, a plain path that must come out bare, several paths)
  and `manifest` (nested folder, empty dir, symlink skipped, over-limit refused).

**Verification:** `cargo test -p fleet-app media`; `make lint`.

**Done when:** `from_clipboard` is right for each T01 source, returns `None` for plain text, and
`manifest` refuses an over-limit folder before any request.

### P1-T09 — The upload driver: windowed, resumable, cancellable, with one continuation

**Intent:** one app-side flow that stages anything to any anchor, survives link blips, reports
progress, and hands the final path to a caller-supplied continuation. Phases 2 and 3 reuse it
unchanged.

**Touches:** `crates/fleet-app/src/media.rs` (or `media/upload.rs`), `src/bridge/requests.rs`,
`src/state/snapshot.rs`.

**Steps:**
- Load `rust-async-background-work` and `gpui-state-and-memory`.
- `Bridge::stage_media(anchor, upload, op)` in `bridge/requests.rs` encodes chunk bytes with
  `fleet_proto::media::encode` and returns the reply receiver. Screens never build a bare
  `RequestBody` and never touch base64.
- Add `AppState::host_of_terminal(&self, TerminalId) -> Option<&HostId>` over the snapshot sessions
  (see Repository context). Add a small accessor for a host's current `LinkState`.
- **An upload registry in app state** (a small model owned by `AppState` or its own entity; choose
  per `gpui-state-and-memory` and record it). For each live upload it holds:
  - the upload id and anchor;
  - its target host;
  - its progress;
  - its state: `Waiting | Sending | Finishing | Failed`;
  - the stored `Task` driving it.
  Storing the task satisfies the `CLAUDE.md` spawn rule, and dropping it cancels the upload. The
  registry also owns the **per-host window**: one semaphore of `UPLOAD_WINDOW` permits per host,
  shared by every upload to it.
- `media::stage(anchor, attachment, force_copy: bool, on_path: impl FnOnce(Vec<PathBuf>, &mut App) + 'static, cx)`.
  The exact signature is yours, but it must take the continuation. Do not hard-code
  `PendingInput::Paste`. The flow:
  1. **Local short-circuit:** for a local target with `Paths` and `force_copy == false`, call
     `on_path` with the original paths. No request is made. Phase 2 passes `force_copy = true`.
  2. Build the manifests (T08) and refuse over-limit drops up front, with a message naming the
     limit.
  3. **Wait for the link.** If the target host's `LinkState` is not up, set the toast to "Waiting
     for <host> to reconnect…" and wait for the link event, for up to 30 s. After that, fail with
     a message naming the host.
  4. `Begin`, then stream chunks under the host window, reading them on the background executor and
     hashing each file incrementally as it streams. Update the progress toast at most a few times
     a second ("Copying design.pdf to devbox… 40%"). A progress update is a state update plus a
     notify from the update path; never compute progress in `render`.
  5. **On a failed chunk while the host link is down:** go to `Waiting`, wait for the link (for up
     to `UPLOAD_IDLE_EXPIRY` minus a margin), then resend every chunk not yet acknowledged.
     Chunks are idempotent, so resending is always safe.
  6. **On `NotFound` from the remote** (the upload expired or the remote restarted): restart once
     from `Begin` with a fresh id, then fail.
  7. `Finish` with the digests. On `Path`, call `on_path`. On any terminal error, surface it through
     `apply_toast_event(ToastLevel::Error, …)`. `Unsupported` already reads as a sentence, so pass
     it through.
  8. **Cancel** is available from the progress toast (use the toast action mechanism,
     `ToastTarget`, at `state/notifications.rs:59`), or from a palette command if a toast cannot
     carry it. Cancel drops the task and sends a best-effort `Cancel`. The send may fail: log it at
     `debug`, because the remote's expiry cleans up anyway.
- Nothing here runs inside `render`.
- Tests (`#[gpui::test]`, `run_until_parked`, and a fake bridge that answers ops):
  - local short-circuit (no requests);
  - a multi-chunk file (the window is never exceeded, and progress advances);
  - a folder;
  - the link goes down mid-upload and comes back (only unacknowledged chunks are resent, and the
    upload completes);
  - `NotFound` triggers one restart;
  - cancel (the task is dropped and `Cancel` is sent);
  - over-limit refusal before any request.

**Verification:** `cargo test -p fleet-app media`; `make test`.

**Done when:** every test above passes, and the continuation receives the remote path(s).

### P1-T10 — Wire paste and drop into the two terminal surfaces

**Intent:** the gesture works in the app.

**Touches:** `screens/workspace/{actions.rs:504-508, terminal.rs:285-302}`, `screens/workspace.rs`,
`screens/agent_popup/{actions.rs, terminal.rs:423-436}`, `screens/agent_popup.rs`,
`dialogs/help/guides.rs:338-360`.

**Steps:**
- Load `gpui-app-shell` and `gpui-styling`.
- In both paste paths, try `media::from_clipboard(&item)` first. On `Some`, call `media::stage` with
  `MediaAnchor::Terminal` and `force_copy = false`. Its continuation delivers
  `PendingInput::Paste(insert_text(..))` through the existing `send_or_queue` gate and routes a
  rejection through `surface::report_input_delivery`, as today's paste does. On `None`, fall
  through to `route_paste` unchanged. `route_paste` itself does not change.
- The continuation re-checks that the terminal still exists before delivering. If the user closed
  it during a long upload, say where the file landed (as a toast with the path) instead of
  dropping it silently.
- Add `on_drop::<ExternalPaths>` to each root at the anchors listed above, calling the same thing,
  and `cx.stop_propagation()`. Follow the board's DnD pattern for the hover state
  (`views/board_screen/drag.rs`). Handlers only *install* closures inside `render`.
- Add a drop-target affordance on hover that names the destination: "Drop to copy to devbox" for a
  remote terminal, and "Drop to paste path" for a local one. The label is prepared in the update
  path, not in `render`. Every colour, radius and duration comes from theme tokens. Reuse
  `DropSlot` visuals if they fit, and do not force them if they don't.
- Update the terminal clipboard rows in `dialogs/help/guides.rs` so they say that paste also
  handles images and files, and that a drop works, including folders and remote terminals.

**Verification:**
- `make lint`, `make test`, and `make harness` (non-regression; the harness cannot drive this
  gesture).
- `make restart`, then fill the manual matrix in the tracker on macOS and on Linux, including the
  remote rows.

**Done when:** every matrix row passes, or has a recorded platform limitation from T01.

### P1-T11 — Move the docs with the code

**Intent:** `docs/` is authoritative.

**Touches:** `docs/REMOTE-MACHINES.md`, `docs/ARCHITECTURE.md`, `docs/UX-SPEC.md`,
`docs/KEYMAP.md`, `docs/decisions/0026-media-staging.md` (new), `docs/README.md`.

**Steps:**
- `REMOTE-MACHINES.md` §3: `StageMedia` and its ops, all four anchors, stateless routing by anchor,
  translation rules, the capability, the chunk/window/limit constants and the keystroke-latency
  bound, resume and expiry, and why `PROTOCOL_VERSION` stays 8.
- `ARCHITECTURE.md`: add the media service to the daemon service list, and the capability to
  §Protocol compatibility (`:542`).
- `UX-SPEC.md`: the terminal gesture, the drop affordance copy, the progress / waiting /
  cancel toasts, and the over-limit, outdated-remote and host-gone copy.
- `KEYMAP.md`: paste now also handles images and files.
- ADR 0026: adopted chunked, resumable staging on the owning host over the federated link, with
  two target directories. Rejected:
  - a single large frame, because it can trip `WRITE_BUDGET` and take the host down;
  - a shared mount;
  - out-of-band `scp` or `rsync`, which needs separate credentials and bypasses the link;
  - tar streaming for folders, which needs a new dependency and makes a tar entry trusted input;
  - inserting the local path.
  State the chunk × window arithmetic as a constraint on future code. Add the `docs/README.md`
  row.
- Each doc edit rides with the code commit it describes.

**Verification:** `make lint`; re-read each edited section against the code.

**Done when:** no edited doc contradicts the implementation, and the ADR is indexed.

## Verification

```sh
make lint      # fmt --check + clippy --workspace --all-targets --all-features -D warnings
make test      # builds fleetd, fleet and fleet-harness, then cargo test --workspace
make check     # cargo check --workspace --all-targets
make harness   # visible change: non-regression only, the gesture itself is not scriptable
make restart   # after any daemon change, before manual checks
```

## Definition of done

- [ ] Every P1 task is ticked in the tracker, with verification output beside it.
- [ ] `make lint`, `make test`, `make check` and `make harness` pass.
- [ ] The manual matrix is filled for macOS and Linux, including every remote row.
- [ ] The throttled-link test (T05) passes: the link stays up, and a keystroke-class request
      completes within the documented bound during a large upload.
- [ ] `PROTOCOL_VERSION` is still 8, and a fresh daemon advertises `media.stage`.
- [ ] No `unwrap`/`todo!`/`unimplemented!`/`dbg!`/`TODO` in production code, and no silent
      `let _ =`. Every upload task is stored in the registry. Any other spawned task reports its
      own failures before `.detach()` or ends in `detach_and_log_err`.
- [ ] Docs match the code, ADR 0026 is written and indexed, and each doc edit rode with its code.
- [ ] `zed-quality-review` has been run over the diff.
- [ ] The tracker reflects reality, and follow-ups are listed.

## Risks and rollback

- **The clipboard does not carry what we expect.** T01 found source support on macOS, Wayland, and
  X11, plus live Wayland image/text probes. Real macOS and X11 gestures remain in the manual
  matrix because source support does not prove the desktop integration.
- **Upload traffic starves interactive traffic.** Bounded by the per-host window and proven by the
  T05 throttled test. If real links disagree, lower `CHUNK_BYTES` or `UPLOAD_WINDOW`; both are
  one-line constants. Never raise `WRITE_BUDGET` to make uploads fit.
- **The daemon's memory under many uploads.** The upload table holds bitmaps and handles, not
  bytes. Chunks are written straight to disk. Cap the number of concurrent uploads per connection
  (e.g. 16) with a `Validation` error past it.
- **Disk filling up on the remote.** `Begin` checks free space where it is cheap, and a write
  failure mid-upload is `Fs` with the path, then cleaned up by `Cancel` or expiry.
- **Writing into `~/Downloads`.** Mitigated by the T07 invariant, which now covers folders and part
  files. If it is ever weakened, `$FLEET_HOME/media` should become the default.
- **Path escape from a crafted name or relative path.** Mitigated by the T03 sanitiser applied to
  every component, and tested against both target directories.
- **Rollback.** The wire change is additive and capability-gated. Reverting the app commits
  (T08–T10) restores today's behaviour, including the flattened-paths paste, and leaves the daemon
  side harmlessly in place.
