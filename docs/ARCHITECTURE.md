# Fleet — Architecture

Fleet is the native successor of `swarm` (a tmux-based TUI that manages copy-on-write
development worktrees, GitHub PRs, and per-worktree tmux sessions with a fixed 3-window
layout). Fleet keeps every swarm behavior (see `docs/SWARM-INVENTORY.md`, the authoritative
1:1 feature inventory) but replaces tmux and the terminal TUI with a **daemon + native GPUI app**
that owns everything: worktrees, GitHub state, background jobs, terminal sessions and rendering.

Guiding rules:

1. **Nothing the user started is ever tied to a UI surface.** Jobs (clone, pool build, hooks,
   prune, fetch) and terminal sessions live in the daemon `fleetd`. Closing a dialog, the
   workspace, or the whole app never cancels or blocks them. Only an explicit cancel does.
2. **Keyboard first, nvim-inspired.** Every screen is fully operable with the keymap in
   `docs/KEYMAP.md`. The mouse is optional sugar.
3. **Clean and minimal.** Each view shows exactly what the user needs to decide the next
   action, nothing more. Icons (Lucide) are used to replace words, not to decorate.
4. **Contracts before code.** `fleet-core`, `fleet-proto`, the `VtEngine` trait and the
   `fleet-ui-kit` component API are frozen before parallel implementation starts.

## Processes and binaries

| Binary | Crate | Role |
| --- | --- | --- |
| `fleetd` | `fleet-daemon` | Long-lived daemon. Owns config/state, filesystem layout, jobs, GitHub cache, sessions + PTYs + VT emulation, sleep policy. Listens on a Unix socket. |
| `fleet` | `fleet-app` | GPUI app when run without a subcommand; CLI (`fleet create …`, `fleet list --json`, same JSON envelopes as swarm with `protocol: 1`) when run with one. Both talk to `fleetd` through `fleet-client`, auto-spawning it if the socket is dead. |

`FLEET_HOME` (default `~/.fleet`) mirrors `~/.swarm`: `config.json`, `state.json` (+ lock),
`repos/`, `worktrees/`, `cache/`, `logs/` (+ `logs/jobs/<job-id>.log`), `trash/`, plus
`fleetd.sock` and `fleetd.pid`. Config and state schemas are the swarm schemas (version 1),
so `fleet import --from-swarm` can copy `~/.swarm/{config,state}.json` verbatim.

## Crate map (Cargo workspace, Rust 1.97.1, edition 2024)

```
crates/
  fleet-core      domain types, ids + validation, config/state schemas + defaults, pure helpers   (no I/O)
  fleet-proto     client<->daemon wire protocol: Request/Response/Event, Snapshot, Job, terminal Frame/Cell, codec
  fleet-term      Pty (portable-pty) + VtEngine trait + GhosttyEngine (libghostty-vt) + TerminalHost thread (daemon side)
  fleet-daemon    bin `fleetd`: adapters (shell/git/gh/files/process), stores (config/state+lock), services, jobs, socket server
  fleet-client    async client: connect/spawn daemon, request/response, event stream, terminal attach   (tokio)
  fleet-ui-kit    design system: tokens, theme, icons (Lucide SVG via AssetSource), reusable components   (no domain deps)
  fleet-cli       clap parser, JSON envelopes, human output; uses fleet-client
  fleet-app       bin `fleet`: GPUI app (state mirror, keymap/modes, views, terminal element) + CLI entry
```

Dependency direction: `core <- proto <- {term, client, cli} <- {daemon, app}`; `ui-kit` depends only on gpui.

Toolchain decisions (verified on this machine, see `docs/research/`):

- gpui comes from the Zed monorepo git tag **`v1.18.1`** (`gpui` + `gpui_platform` with
  `font-kit`), Rust **1.97.1** pinned in `rust-toolchain.toml`. crates.io `gpui 0.2.2` is
  11 months stale and is not used.
- Terminal emulation is **`libghostty-vt`** (crates.io, safe wrapper over Ghostty's public
  lib-vt; builds Ghostty from vendored source with a pinned Zig). It runs **inside the daemon**.
  The `VtEngine` trait isolates it; `alacritty_terminal` is the documented fallback.
- The client never hosts a native NSView; it paints the cell grid itself with gpui primitives.

## Daemon (`fleetd`)

- **Runtime**: tokio multi-thread. One `Server` task accepts connections on
  `$FLEET_HOME/fleetd.sock`; each connection is an actor that decodes requests, dispatches to
  services, and forwards subscribed events. Length-prefixed (u32 BE) JSON frames (`fleet-proto`).
- **Stores**: `ConfigStore` (deep-merged defaults, atomic write), `StateStore` (Zod-equivalent
  validation with serde, `state.json.lock` cross-process lock, transaction API, broken-state
  quarantine) — semantics exactly as in the inventory.
- **Adapters** (traits + real impls + fakes for tests): `Shell`, `Git`, `Github`, `Files`
  (clonefile/`cp -Rc`, atomic rename, trash), `Process` (`ps`, `lsof`, liveness), `Clock`.
  Exact git/gh command lines are those in the inventory §7.
- **Services**: `Contexts`, `Repos` (clone jobs, discovery cache), `Worktrees` (prepared-copy
  pool, claim/create, publish-intent recovery, delete, inspect, prune), `Github` (PR tabs,
  caches, TTLs), `Sessions` (see below), `Sleep`, `Doctor`.
- **Jobs**: `JobManager` runs every long operation as a `Job` (id, kind, target, status,
  last progress line, log path, timestamps, cancellable). Per-repo mutexes, pool concurrency 2,
  GitHub concurrency 4, cancellation tokens. Job state is broadcast as events; logs go to
  `logs/jobs/<id>.log`. Jobs are never attached to a client connection.
- **Sessions replace tmux**. `Session { id, kind: Worktree(id) | Agent(claude|opencode),
  cwd, terminals: Vec<Terminal>, active_terminal }`. `Terminal { id, name, command, cwd,
  shell_pid, foreground_command, status, title, keep_alive labels, kind: Pty | Native }`. A
  terminal is one PTY running the login shell with the configured command typed + Enter (so the
  shell survives the command, like tmux). Session naming and the default `nvim | cc | lg`
  layout follow the inventory. Multiple clients may attach to a terminal; the daemon keeps the
  VT state and sends a full frame on attach and dirty-row diffs afterwards; the most recent
  attach's size wins. Sleep applies the swarm policy (keep-alive rules, `:qa` handshake, port
  detection); "close window" = kill that terminal. PTYs do not survive a daemon restart (like a
  tmux server).
- **Native tabs (`kind: Native`)**. A `windows[].command` may be a reserved `fleet://` command
  instead of a program. The daemon still owns the tab — same id counter, same position in
  `terminals`, same `active_terminal` and `SelectTerminal` — but spawns no PTY: `shell_pid` is
  `None`, there is no `TerminalHost`, and every process-shaped request (`AttachTerminal`,
  `TerminalKey`, `ResizeTerminal`, `ScrollTerminal`, `WheelTerminal`, paste, restart) answers `NotFound` /
  `Conflict` rather than reaching one. The **client** draws the tab. Keeping it in the session
  list is what keeps `ctrl-s <n>` numbering stable. Sleep sees a tab with no process, so it is
  always idle: it closes with the other idle tabs and `EnsureSession` puts it back on wake. A
  worktree on a remote host degrades the command back to the program it stands for
  (`fleet://lazygit` → `lazygit`), because the client-side implementation runs `git` locally.
  The only reserved command today is `fleet://lazygit`, drawn by `crates/fleet-lazygit`
  embedded in `fleet-app` (see that crate's README, "Embedding").

## Terminal pipeline

```
pty (portable-pty) --bytes--> VtEngine (libghostty-vt) --dirty rows--> FrameUpdate (proto) --socket--> client mirror grid --paint--> gpui
client key event --proto--> daemon key encoder (terminal modes aware) --bytes--> pty
```

`FrameUpdate { terminal_id, seq, cols, rows, full: bool, shift: Option<i32>, rows: [RowUpdate{index, cells, wrapped}],
cursor{row,col,visible,shape}, viewport{scrollback_len, offset, history_epoch},
modes{alt_screen, mouse, bracketed_paste}, title }`.
`Cell { text (grapheme), fg, bg, attrs bitflags, width }`.
Colors are `Default | Palette(u8) | Rgb`; the client resolves palette colors from the theme.
Scrollback is viewed by asking the daemon to move the viewport offset. Selection/copy happens
on the client's mirror grid. The wire protocol is version 3; `wrapped` preserves logical lines
during copy, while `history_epoch` invalidates bounded-history indexes when Ghostty's tracked oldest
row is discarded, history shrinks, or a column change reflows it, without treating viewport
movement as eviction. Off-screen
copy caches only visible rows covered by an active selection and caps them at 5,000.

Terminal history uses `terminal.scrollbackBytes` (Rust `terminal.scrollback_bytes`), a
**byte budget passed directly to `TerminalOptions.max_scrollback`**, defaulting to
1,073,741,824 bytes (1 GiB) per terminal, allocated as history grows. Config validates a
positive budget; it applies to newly spawned/restarted terminals. Ghostty retains history
in pages, so retained row count depends on width and contents, and tiny budgets have page
granularity. The low-level engine accepts zero to disable history; alternate screens never
have history. No unlimited setting is supported. After 250 ms without PTY output or commands,
the owner thread runs one bounded incremental compression step at most every 16 ms. Completed
passes are skipped until Ghostty's compression activity token changes; synchronous full-history
compression is never used.

`WheelTerminal { terminal, wheel: { steps, col, row, mods } }` carries whole rows:
positive steps move down toward live output, negative steps move up. Wheel, viewport, key,
raw input and paste requests share ordered dispatch. The app only enqueues wheel requests;
it never waits for acknowledgements. The host decides using **live emulator modes**:

| State | Wheel behavior |
| --- | --- |
| Primary screen, any tracking state, no Shift | Move Fleet's viewport by `steps` rows |
| Primary screen, Shift held, tracking enabled | Encode mouse wheel reports (Four/Five) and write to PTY |
| Alternate screen, tracking enabled | Encode mouse wheel reports and write to PTY |
| Alternate screen, tracking disabled, DECSET 1007 enabled | Encode Up/Down keys per step, honoring DECCKM |
| Alternate screen, tracking disabled, DECSET 1007 disabled | Drop |

PTY-directed wheel output is capped at four times the current row count, up to 1,024 steps.
Primary-screen Shift without tracking also moves Fleet's viewport. All mouse/key sequences
come from Ghostty encoders. Wheel pseudo-buttons never become held buttons.

The app accumulates pixel deltas divided by measured cell height; line deltas use
`terminal.scrollLinesPerStep` (default 3, range 1..=50). It truncates toward zero and retains the
fractional remainder. GPUI positive content movement becomes negative steps. Changing the
active terminal (including no terminal), the terminal under the pointer, ending or cancelling a gesture resets the remainder; momentum
events otherwise pass through unchanged. Cell coordinates use measured inner painter bounds
with grid padding already removed. Settings load on connection/reconnection and config responses.

The PTY reader uses an unbounded queue so large writes into echoing children cannot deadlock.
The host drains at most 256 KiB or 2 ms of PTY output per iteration and batches at most 1,024
commands, coalescing adjacent viewport moves with per-command boundary clamping. Key/resize
and application-directed wheel input preserve ordering by ending the current viewport batch.
Viewport moves emit frames immediately, bypassing the normal 16,667 µs output frame gate.
A blocking command receive wakes immediately for input, with a 4 ms timeout for PTY polling.

Small viewport-only moves use `FrameUpdate.shift`: positive shifts move existing mirror rows
up, negative shifts move them down, and wrap flags rotate with their rows. Only newly exposed
rows are replaced. Absolute selection anchors keep following the same text as the viewport moves;
an epoch change clears selections and their caches. Epoch changes force full frames, and clients
reject shifts across epochs and request full recovery. Output, resize,
large moves and explicit full-frame requests use ordinary row/full frames. Broadcast lag
requests full frames for all attached terminals; forward sequence gaps freeze the last valid
mirror and reuse the client's full-frame recovery path. Stale frames are rejected. Sessions
retains the next sequence for each terminal across restarts, including attachment frames.

`ScrollOrKeyTerminal { terminal, scroll, key }` shares the ordered input path. The host
coalesces the scroll on the primary screen and forwards the key through normal input
handling on the alternate screen, using live modes for the four viewport shortcuts.

At the bottom, output follows live. While scrolled up, Ghostty preserves the history anchor.
Real keys, raw input and paste atomically return to bottom on the host before writing to the
PTY. Wheel input and copy-mode navigation preserve the viewport; copy-mode exit retains its
explicit return-to-bottom behavior. Wire protocol version is 3; the separate swarm-compatible
CLI JSON envelope remains version 1.

## Client (`fleet` app)

- **State mirror**: the app holds the latest `Snapshot` (contexts, repos, worktrees, clone
  jobs, sessions, jobs) plus per-terminal mirror grids, all updated from the event stream on
  the gpui foreground executor. A tokio runtime on a background thread runs `fleet-client`;
  channels bridge into gpui.
- **Modes**: `Normal` (lists), `Terminal` (keys go to the PTY), `Prefix` (one-shot after
  `ctrl-s` inside a terminal), `Scroll` (copy/scrollback mode), `Filter`, `Palette`, `Dialog`.
  Implemented as gpui key contexts + actions; see `docs/KEYMAP.md`.
- **Screens**: `Hub` (contexts / repos / worktrees or PRs / detail / status bar), `Workspace`
  (session terminals with a tab strip and a compact session header), `Jobs` panel (overlay),
  dialogs (Create, Clone, Confirm, Context, Assign, Settings, Help), Palette, Filter.
  See `docs/UX-SPEC.md` for the per-view content and placement decisions.
- **Design system**: `fleet-ui-kit` — see `docs/DESIGN-SYSTEM.md`. Views compose only kit
  components; no ad-hoc styling in `fleet-app`.

## Testing

Ports/adapters with fakes as in swarm §8: adapter tests assert exact argv; service tests
assert domain results and side-effect order; core helpers are pure-tested; proto has
round-trip tests; `fleet-term` has an engine test (bytes in → cells out); `fleet-ui-kit`
components are exercised by a `kit-gallery` example binary.

## Cooperative subagent watches

`fleet-core::watches` holds read-only child metadata and bounded sequenced output.
The daemon `Watches` service indexes watches by ID, session, and terminal; it
coalesces output events, retains completed results for 30 minutes, and removes
watches on terminal/session removal. A connection lease marks unfinished watches
interrupted on disconnect. No watch operation owns or kills the child process.

```text
piped child stdout/stderr -> fleet exec tee -> original stdout/stderr (raw bytes)
                                         -> AppendWatchOutput -> bounded watch registry -> WatchOutput events / TailWatch -> phase 2 Workspace pane
```

Optional PATH shims wrap only piped `codex` / `claude` invocations inside Fleet.
PTY login shells receive `FLEET_SESSION` (session ID), `FLEET_TERMINAL` (human
terminal name for compatibility), and `FLEET_TERMINAL_ID` (the registered numeric
terminal ID). Shims gate on `FLEET_TERMINAL_ID`; `fleet exec --watch` uses that ID
to associate the watch with its terminal, never the name.
`fleet exec` connects without autostart and falls back transparently when the
daemon is unavailable or watch eligibility fails. `FLEET_DEBUG=1` reports a
one-line reason for each passthrough decision; the default stays silent.
The private launcher preserves the child's PID while
waiting for its watch ID; it then execs the target with inherited stdin and piped
stdout/stderr. `FLEET_WATCH` prevents nested watches. See
`APP-CONTRACTS.md` for wire types, recovery, retention, and ownership semantics.
