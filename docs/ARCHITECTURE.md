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
   `fleet-ui-kit` component API are the seams everything else is written against; changing one
   is a deliberate, workspace-wide change, not a local edit.

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
  fleet-core      domain types, ids + validation, config/state schemas + defaults, agents/ (event model + reducer), pure helpers   (no I/O)
  fleet-proto     client<->daemon wire protocol: Request/Response/Event, Snapshot, Job, agent threads, terminal Frame/Cell, codec
  fleet-git       git plumbing over the real `git` binary: model, read, mutation, rebase, parse, watch
  fleet-term      pty (portable-pty) + VtEngine trait + GhosttyEngine (libghostty-vt) + terminal host thread (daemon side)
  fleet-daemon    bin `fleetd`: adapters (shell/git/gh/files/process/logs), stores (config/state+lock), services, jobs, socket server
  fleet-client    async client: connect/spawn daemon, api/, event stream, terminal attach, watches   (tokio)
  fleet-ui-kit    design system: tokens, theme, icons (Lucide SVG via AssetSource), components   (no domain deps)
  fleet-cli       clap parser, JSON envelopes, human output; uses fleet-client
  fleet-app       bin `fleet`: GPUI app (shell, state mirror, bridge, keymap, screens, dialogs, views, terminal) + CLI entry
  fleet-lazygit   bin `fleet-lazygit` and the `fleet://lazygit` native tab: git UI over fleet-git and fleet-ui-kit
```

Dependency direction: `core <- proto <- {term, client, cli} <- {daemon, app}`; `ui-kit` depends
only on gpui; `lazygit` depends on `git` and `ui-kit`, never on `core` or `proto`.

Inside the two GPUI crates the module layout follows responsibility, not screen count. `fleet-app`
splits `shell/`, `state/`, `bridge/`, `presentation/`,
`screens/{hub,workspace,agent_thread,agent_popup,jobs}/`,
`dialogs/`, `views/`, `terminal/` and `watches/`; each of those roots holds the type and its
composition, with preparation, actions, lifecycle and tests in siblings. `fleet-lazygit` splits
`root/`, `state/`, `panels/`, `views/`, `bridge/` and `drive/` the same way.

The load-bearing choices behind this map — the GPUI pin, terminal emulation, the in-house design
system, the native git UI and the diff pipeline — are recorded in `docs/decisions/`.

## Daemon (`fleetd`)

- **Runtime**: tokio multi-thread. One `Server` task accepts connections on
  `$FLEET_HOME/fleetd.sock`; each connection is an actor that decodes requests, dispatches to
  services, and forwards subscribed events. Length-prefixed (u32 BE) JSON frames (`fleet-proto`).
- **Stores**: `ConfigStore` (deep-merged defaults, atomic write), `StateStore` (Zod-equivalent
  validation with serde, `state.json.lock` cross-process lock, transaction API, broken-state
  quarantine) — semantics exactly as in the inventory. Writes retain their serialization guard
  through blocking completion even if the async caller is cancelled, and invalid observations are
  re-read under the cross-process lock before quarantine.
- **Adapters** (traits + real impls + fakes for tests): `Shell`, `Git`, `Github`, `Files`
  (clonefile/`cp -Rc`, atomic rename, trash), `Process` (`ps`, `lsof`, liveness), `Clock`, `Logs`.
  Exact git/gh command lines are those in the inventory §7.
- **Services**: `Contexts`, `Repos` (clone jobs, discovery cache), `Worktrees` (creation,
  publication, recovery, trash, hooks) with the prepared-copy `Pool`, `Inspect`, `Prune`,
  `Github` (PR tabs, caches, TTLs), `Sessions` (registry, lifecycle, host bridge, observations),
  `Hosts`, `Sleep`, `Watches` and `WatchDiscovery`, `AgentActivity`, `Agents` (the native agent
  session manager, below), `Awaited`, `Doctor`,
  `Import`, `Update`. `services/composition.rs` wires them, `dispatch.rs` routes requests,
  `snapshots.rs` builds the broadcast snapshot, and `maintenance.rs` owns the periodic sweeps.
  Revision-keyed caches in `services/cache.rs` let unchanged inventories be reused instead of
  rebuilt per request. Cache expiry inspects entry types and removes only the file identity it
  observed, so a concurrent refresh or temporary/non-directory entry is never unlinked as stale.
- **Jobs**: `JobManager` runs every long operation as a `Job` (id, kind, target, status,
  last progress line, log path, timestamps, cancellable). Per-repo mutexes, pool concurrency 2,
  GitHub concurrency 4, cancellation tokens. Job state is broadcast as events; logs go to
  `logs/jobs/<id>.log`. Jobs are never attached to a client connection. Repo-scoped admission is
  typed and atomic with repository deletion; cancellation does not report completion until tracked
  rollback/worker cleanup finishes. Clone launch ownership is persisted beside the job log, so a
  daemon restart can recover a child launched before its in-memory PID was published.
- **Sessions replace tmux**. `Session { id, kind: Worktree(id) | Agent(claude|opencode),
  cwd, terminals: Vec<Terminal>, active_terminal }`. `Terminal { id, name, command, cwd,
  shell_pid, foreground_command, status, title, keep_alive labels, kind: Pty | Native }`. A
  terminal is one PTY running the login shell with the configured command typed + Enter (so the
  shell survives the command, like tmux). Session naming and the default `nvim | cc | lg`
  layout follow the inventory. Multiple clients may attach to a terminal; the daemon keeps the
  VT state and sends a full frame on attach and dirty-row diffs afterwards; the most recent
  attach's size wins. Attach is asynchronous under one absolute five-second deadline; requests
  already expired when serviced cannot resize, and post-deadline results cannot publish a frame or
  increment attachment membership. Sleep applies the
  swarm policy (keep-alive rules, `:qa` handshake, port detection), but sends editor shutdown input
  only when the candidate process group is the terminal's foreground group; "close window" = kill
  that terminal. PTYs do not survive a daemon restart (like a tmux server).
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

The native Git pane still executes mutations locally rather than as daemon jobs. Safety-sensitive
operations carry the identity the user reviewed: partial staging carries the displayed diff
preimage, stash drop carries the stash OID rather than only a mutable index, and continuation
distinguishes revert from merge/rebase. The backend revalidates those identities immediately
before mutation; merged status comes from commit reachability.

## Native agent sessions

A Claude Code or OpenCode session is a **thread**, and a thread is daemon state exactly like a
terminal: the app never owns one. `docs/NATIVE-AGENTS.md` is the specification and ADR 0008
records why the load-bearing choices are what they are; this is the map.

```
fleet-core::agents        ids (ThreadId/TurnId/ItemId/GateId/Seq), AgentEvent, items, gates,
                          state enums, ThreadProjection (the reducer) and AgentThreadSummary
fleet-daemon services/agents/
  manager.rs              AgentSessionManager: threads, lifecycle, sequencing, broadcast
  thread.rs               one live thread: runtime handles, serialized operation gate, coalescing
  store.rs                append-only event log per thread + the versioned thread index
  providers/claude/       Claude Code over bidirectional stream-json on stdio
  providers/opencode/     OpenCode over HTTP + SSE against one managed `opencode serve` per thread
fleet-client api/agents/  typed commands plus AgentMirror, which replays the same reducer
fleet-app  screens/agent_thread/   Entity<AgentThreadView> per open tab; state/agents.rs mirrors
```

**Ownership boundary.** Provider IO produces normalised `AgentEvent`s; a serialised reducer
stamps each with a per-thread `seq`, persists it, applies it to the projection, and broadcasts
it; views render projections. Provider wire types never leave `fleet-daemon`, and GPUI never
owns lifecycle truth. Because the reducer is `fleet_core::agents::ThreadProjection::apply`, the
daemon and every client run the *same* function over the same ordered events.

**Event flow.** `AgentThreadCreate` resolves the published `WorktreeId` to a trusted canonical
path through `Worktrees`, spawns the adapter named by `config.agentCommands`, and returns an
`AgentThreadSummary`. The adapter's `mpsc` receiver is drained by one task per thread: each
event is sequenced, appended to the log, reduced, and published as `Event::Agent { thread,
event }`, with `Event::AgentSummary` whenever the summary changes. `ContentDelta`s are coalesced
per item on a 16 ms tick before broadcast, so a fast model cannot schedule a render per token.
Turn completion is the provider's authoritative primitive only (§4 of `NATIVE-AGENTS.md`);
before a `TurnCompleted` is applied every open item of that turn is closed, so no finished turn
shows a spinner. Gates are independent of turns and close only on `GateResolved`.

**Attention** is derived by the reducer, not by any view: permission > question > plan >
finished > failed > working > unread > idle, carried in `AgentThreadSummary` so the tab badge,
the session header word and the context-bar counters cannot disagree. `Finished` is amber and
clears when the client reports `AgentMarkSeen { thread, seq }`; seen state is per client and
lives in the app.

**Persistence** is `$FLEET_HOME/agents/`: one append-only `<thread_id>/events.ndjson` written
before the event is broadcast, plus a versioned `index.json` (id, worktree, provider, title,
created, last activity, resume cursor, model, mode, last outcome) written with the `StateStore`
discipline — serialised mutation, atomic rename, quarantine to `index.json.broken-*` on
corruption. It is not part of `PersistedState` version 1; it has its own file and version.

**Restart.** On boot the manager loads the index and replays each thread's log. A thread the
log leaves `Starting`/`Running` is an orphan and is settled explicitly: with a resume cursor it
records `TurnAborted { ProviderExited }` and `SessionStateChanged(Stopped)` and is resumed
lazily the next time it is opened; without one it records `SessionExited { expected: false }`.
No PID is ever reattached, and every thread stays browsable read-only regardless. A log whose
tail the reducer refuses on replay is trimmed back to the last event that reduces
(`AgentStore::truncate_after`), so one bad record costs the tail rather than the thread.

**Clients** get `Snapshot.agent_threads` for tabs and counters, then `AgentThreadOpen { thread,
from_seq }` for one thread's `ThreadProjection` plus the events after that cursor, then the
event stream. A `seq` gap makes the mirror return `MirrorOutcome::Gap` and the app re-opens from
its last applied `seq`, mirroring terminal frame recovery rather than inventing a rule.
`fleet agent list|new|send|respond|interrupt|stop|tail` drives the same requests from the CLI,
which is how a thread is exercised without the app; `fleet agent terminal` is the unchanged PTY
popup path, kept under its own verb. The read-only verbs open with an explicit `Seq(0)` cursor, so
reading a thread never triggers the lazy resume a cursorless open means.

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
on the client's mirror grid. The wire protocol is version 5; `wrapped` preserves logical lines
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

PTY output, PTY writes, and host commands each have a 4 MiB byte budget; host events use two
16 MiB slots. The isolated writer thread prevents a blocked child from blocking the terminal
owner. Queue exhaustion is an explicit error, never silent loss. The host drains at most 256 KiB
or 2 ms of PTY output per iteration and batches at most 1,024 commands, coalescing adjacent
viewport moves with per-command boundary clamping. Key/resize and application-directed wheel
input preserve ordering by ending the current viewport batch. Viewport moves emit frames
immediately, bypassing the normal 16,667 µs output frame gate. A blocking command receive wakes
immediately for input, with a 4 ms timeout for PTY polling.

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
explicit return-to-bottom behavior.

## Protocol compatibility

Daemon IPC is version **5**. The bump is the native-agent surface: eleven `RequestBody` variants
(`AgentThreadList`, `AgentThreadCreate`, `AgentThreadOpen`, `AgentThreadClose`, `AgentSend`,
`AgentInterrupt`, `AgentRespond`, `AgentSetMode`, `AgentSetModel`, `AgentMarkSeen`,
`AgentStop`), their `ResponseBody` answers (`AgentThreads`, `AgentThreadCreated`,
`AgentThreadSnapshot`, `AgentAck`), and the `Agent` / `AgentSummary` events. `Snapshot`'s new
`agent_threads` is `#[serde(default)]`, so a version-4 snapshot payload still deserializes; the
requests are new names, which an older daemon rejects rather than misreads, and `Hello`
negotiates the version before any of them is sent.

`PruneWorktrees.ids` is the shipped additive field: it defaults to
absent and is omitted when `None`, preserving legacy request JSON; `Some(ids)` is the exact
reviewed allowlist for a commit, which the daemon may shrink after locked reinspection but never
expand. This is old-client/new-daemon compatible; `Hello` has no capability list, so exact-set
commits cannot safely target an older daemon that ignores the field. Daemon Git-mutation jobs,
terminal search/history/focus, cell hyperlinks/frame effects, and persisted `ui.presentation`
configuration remain outside the current protocol and config schemas. The separate
swarm-compatible CLI JSON envelope remains version 1.

## Client (`fleet` app)

- **State mirror**: the app holds the latest `Snapshot` (contexts, repos, worktrees, clone
  jobs, sessions, jobs) plus per-terminal mirror grids, all updated from the event stream on
  the gpui foreground executor. A tokio runtime on a background thread runs `fleet-client`;
  channels bridge into gpui.
- **Modes**: `Normal` (lists), `Terminal` (keys go to the PTY), `Native` (keys go to a
  Fleet-drawn tab), `Agent` (keys go to a native agent thread's composer), `Prefix` (one-shot
  after `ctrl-s` inside a terminal), `Scroll` (copy/scrollback mode), `Filter`, `Palette`,
  `Dialog`. Implemented as gpui key contexts + actions; see `docs/KEYMAP.md`.
- **Screens**: `Hub` (contexts / repos / worktrees or PRs / detail / status bar), `Workspace`
  (session terminals with a tab strip, a compact session header and the subagent watch pane),
  the native agent tab, the floating agent popup, the `Jobs` panel, the dialogs, Palette and
  Filter. See `docs/UX-SPEC.md` for the per-view content and placement decisions.
- **Agent threads**: `AppState::agents` mirrors the daemon's summaries, the projections of the
  threads this window opened, and the local-only cursors (which tab is selected per worktree,
  which `seq` has been shown). The tab strip, the session header word, the context-bar counters
  and the attention notifications all read that one mirror; see `docs/APP-CONTRACTS.md`.
- **Render discipline**: a screen prepares in `synchronize` — attachment, requests, focus and
  resource reconciliation — and `render_prepared` only composes what is already prepared. Render
  performs no filesystem access and starts no request. See `docs/APP-CONTRACTS.md`.
- **Design system**: `fleet-ui-kit` — see `docs/DESIGN-SYSTEM.md`. Views compose only kit
  components; no ad-hoc styling in `fleet-app`.

## Testing

Ports/adapters with fakes as in swarm §8: adapter tests assert exact argv; service tests
assert domain results and side-effect order; core helpers are pure-tested; proto has
round-trip tests; `fleet-term` has an engine test (bytes in → cells out); `fleet-ui-kit`
components are exercised by a `kit-gallery` example binary. The agent adapters and the reducer
are tested by replaying the recorded harness captures (`docs/research/fixtures/agents/`, with the
subset the adapters replay under `crates/fleet-daemon/tests/fixtures/agents/`) into projections
and asserting the resulting thread state and attention — no live provider, no window.

## Cooperative and discovered subagent watches

`fleet-core::watches` holds read-only child metadata, its `cooperative` or
`discovered` source, an optional discovered log path, and bounded sequenced output.
The daemon `Watches` service indexes watches by ID, session, and terminal; it
coalesces output events, retains completed results for 30 minutes, and removes
watches on terminal/session removal. A connection lease marks unfinished cooperative
watches interrupted on disconnect. Discovered watches have no lease. No watch
operation owns, signals, or kills the observed process.

```text
piped child stdout/stderr -> fleet exec tee -> original stdout/stderr (raw bytes)
                                         -> AppendWatchOutput -> bounded watch registry -> WatchOutput events / TailWatch -> Workspace watch pane

one ps snapshot / 2 s -> candidate regexes -> Fleet env or PTY ancestry -> discovered watch
companion job JSON/log / 500 ms -----------------------------> metadata, exit, output
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

The `WatchDiscovery` loop takes one process-table snapshot per scan. It reads
environments only for matching candidates and caches successful reads by PID plus kernel process
start identity; failed reads retry, and a reused PID gets a fresh environment. Ownership prefers
`FLEET_SESSION` and a valid `FLEET_TERMINAL_ID`, then falls back to ancestry beneath a
terminal login shell. Session-only tags select the configured agent terminal, then the
first terminal. Helper processes and each terminal's own foreground program (a direct child of its
PTY shell) are excluded, and a matching tree collapses to its topmost eligible process. Existing cooperative or
discovered PIDs win deduplication.

Dropping a cooperative watch's owner connection marks it exited with unknown cause
(`code: None, signal: None`); it does not invent SIGKILL. Discovered watches likewise report an
unknown cause when liveness disappears without companion completion metadata.

Codex companion workers are correlated with their per-workspace job JSON. Their log is
tailed into stdout chunks, beginning with its last 64 KiB after discovery or daemon
restart. Generic discoveries carry an informational line because their output is not
captured. Liveness and companion terminal states only change watch metadata; discovery
never obtains a process-control handle.
