# Fleet — Architecture

Fleet is the native successor of `swarm` (a tmux-based TUI that manages copy-on-write
development worktrees, GitHub PRs, and per-worktree tmux sessions with a fixed 3-window
layout). Fleet keeps the swarm compatibility baseline (see `docs/SWARM-INVENTORY.md`, including
its explicit Fleet deviations) but replaces tmux and the terminal TUI with a **daemon + native
GPUI app** that owns everything: worktrees, GitHub state, background jobs, terminal sessions and
rendering.

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
| `fleetd` | `fleet-daemon` | Long-lived daemon. Owns config/state, filesystem layout, jobs, GitHub cache, sessions + VT emulation, sleep policy. Listens on a Unix socket. |
| `fleetd pty-hold` | `fleet-daemon` | One detached holder process per terminal, started by `fleetd` and hidden from `--help`. Owns that terminal's PTY and child, listens on a per-terminal Unix socket, and outlives the daemon. |
| `fleet` | `fleet-app` | GPUI app when run without a subcommand; CLI (`fleet create …`, `fleet list --json`, same JSON envelopes as swarm with `protocol: 1`) when run with one. Both talk to `fleetd` through `fleet-client`, auto-spawning it if the socket is dead. |

`FLEET_HOME` (default `~/.fleet`) mirrors `~/.swarm`: `config.json`, `state.json` (+ lock),
`repos/`, `worktrees/`, `cache/`, `logs/` (+ `logs/jobs/<job-id>.log` and `logs/pty-hold.log`),
`trash/`, `pty/` (one `<terminal-id>.sock` and `<terminal-id>.json` per live terminal, see
"Detached PTY holders"), plus `fleetd.sock`, `fleetd.pid`, and the stable `daemon-id`. Config and state remain version 1 and
`fleet import --from-swarm` can copy compatible data. Fleet extends host configuration with tagged
machine providers while retaining the old `{ssh, swarmCommand}` entry as probe-only input.

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
  Exact git/gh command lines are those in the inventory §7. `Process` resolves its
  listening-port source once per daemon: `lsof` when it is on `PATH`, otherwise Linux
  `/proc/net/tcp{,6}` joined to `/proc/<pid>/fd` socket inodes, otherwise no ports at all with
  one warning naming the install command. Ports are observed only when an enabled keep-alive
  rule matches on them, and each pid's descriptor walk stops once that pid accounts for every
  listening inode. A source that reports no ports is not an error:
  observation refresh and sleep continue on the `ps` half, only port keep-alive rules go
  unmatched, and a periodic failure is warned about at most once an hour.
- **Machines**: `machines/` owns `MachineProvider`, provider construction, the Tailscale/OpenSSH
  transport, the advanced command transport, legacy probing, and one lazy `RemoteLink` endpoint
  per federated host. `fleetd connect` bridges stdin/stdout to the remote daemon's Unix socket;
  neither daemon listens on a tailnet TCP port.
- **Services**: `Contexts`, `Boards` (documents, cards, remote sync), `Repos` (clone jobs,
  discovery cache), `Worktrees` (creation, publication, recovery, trash, hooks) with the
  prepared-copy `Pool`, `Inspect`, `Prune`,
  `Github` (PR tabs, caches, TTLs), `Sessions` (registry, lifecycle, host bridge, observations),
  `Hosts`, `Sleep`, `Watches` and `WatchDiscovery`, `AgentActivity`, `Agents` (the native agent
  session manager, below), `Router`, `Mirror`, `Bootstrap`, `Awaited`, `Doctor`, `Import`,
  `Update`. `services/composition.rs` wires them, `dispatch.rs` enters the router,
  `snapshots.rs` merges local state with mirrored host fragments, and `maintenance.rs` owns the
  periodic sweeps.
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
- **Sessions replace tmux**. `Session { id, kind: Worktree(id) | Agent(claude|codex),
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
  that terminal. Sleep degrades rather than fails when ports cannot be observed.
  **Terminals survive a daemon restart**: every PTY lives in a detached holder process and is
  reattached by the next daemon (below).
- **Native tabs (`kind: Native`)**. A `windows[].command` may be a reserved `fleet://` command
  instead of a program. The daemon still owns the tab — same id counter, same position in
  `terminals`, same `active_terminal` and `SelectTerminal` — but spawns no PTY: `shell_pid` is
  `None`, there is no `TerminalHost`, and every process-shaped request (`AttachTerminal`,
  `TerminalKey`, `ResizeTerminal`, `ScrollTerminal`, `WheelTerminal`, paste, restart) answers `NotFound` /
  `Conflict` rather than reaching one. The **client** draws the tab. Keeping it in the session
  list is what keeps `ctrl-s <n>` numbering stable. Sleep sees a tab with no process, so it is
  always idle: it closes with the other idle tabs and `EnsureSession` puts it back on wake. A
  proxied session degrades a process-backed native command to the program it stands for
  (`fleet://lazygit` → `lazygit`), because the embedded implementation would run `git` on the
  client machine. Structured native-agent tabs are explicitly exempt: clients draw them from
  routed protocol events, so their provider still runs on the worktree's owning daemon. The only
  reserved process-backed command today is `fleet://lazygit`, drawn by `crates/fleet-lazygit`
  embedded in `fleet-app` (see that crate's README, "Embedding").

The native Git pane still executes mutations locally rather than as daemon jobs. Safety-sensitive
operations carry the identity the user reviewed: partial staging carries the displayed diff
preimage, stash drop carries the stash OID rather than only a mutable index, and continuation
distinguishes revert from merge/rebase. The backend revalidates those identities immediately
before mutation; merged status comes from commit reachability.

## Terminal agent status and attention

PTY output counters are sampled every 500 ms. `AgentActivityTracker` turns real non-echo output
into `Working` and 2.5 seconds of silence into `Idle`; those heuristic states drive glyphs only.
They never authorize a toast or sound. `fleet agent-status` is the authoritative terminal-attention
path: `working` clears attention, while `permission`, `question`, `plan`, and `finished` set the
shared `fleet_core::agents::AttentionKind` and report `Idle`. The optional attention travels on the
request, transition event, and per-terminal snapshot status so event loss and snapshot recovery use
one field. It persists through silence and echo, and clears on explicit working, user input, PTY
close, or byte-advancing output beyond the echo window. The app renders the native `NeedsYou` amber
tab mark and notifies once per session edge into semantic attention; reconnect seeding is silent.

## Remote machines and daemon federation

Every machine runs its own `fleetd` and owns the files, Git operations, PTYs, process observation,
agent processes, and persistence located there. The app and CLI connect only to the local Unix
socket. For each tagged host in config, `Machines` builds a provider; the Tailscale provider
resolves the node through `tailscale status --json`, then uses non-interactive OpenSSH to run
`fleetd connect --home <fleetHome>`. A `RemoteLink` performs Hello as `ClientKind::Proxy`,
correlates forwarded requests and responses, and rebroadcasts remote events. Legacy
`{ssh, swarmCommand}` hosts have `Legacy` link state and can only be probed.

`Router` classifies every `RequestBody` as local orchestration, one owning host, a host-partitioned
fanout, or explicitly unsupported. Worktree operations resolve ownership from local state and the
in-memory `Mirror`; session, terminal, job, and agent-thread requests resolve through id ownership
tables. Before forwarding, the router converts local ids to that daemon's ids and clears explicit
placement such as `host`, so the remote daemon executes its ordinary local service path. On the
way back it reverses the translation. Worktree ids and thread UUIDs pass through but register an
owner; session ids become `<host>/<remote-session>`; terminal and job ids use bijective mappings
allocated from shared local counters.

A remote `EnsureSession` therefore follows this complete path:

1. The app sends `EnsureSession` to its local `fleetd`, exactly as for a local worktree.
2. `dispatch` asks `Router` to resolve the `WorktreeId`; `Mirror` identifies the owning host.
3. The router rewrites ids and placement and calls that host's ready `RemoteEndpoint`.
4. The remote daemon dispatches the request locally, creates or repairs the session and terminals,
   and applies proxied degradation (`fleet://lazygit` becomes a remote `lazygit` PTY).
5. The response, terminal frames, session changes, and agent events return over `RemoteLink`.
   The local router prefixes/remaps their ids and broadcasts them to the original client.

Bulk requests are partitioned by owning host and executed concurrently. Their merged response
keeps one outcome per requested item, including an `Unreachable` reason for items on a down host;
one host failure never erases successful results from another host.

`Mirror` retains one authoritative in-memory snapshot fragment per remote daemon and never
persists those records as local worktrees. Local records win for local state; each remote fragment
is the only source of truth for its host. When a link becomes `Down`, the fragment stays visible
but stale/offline, new routed requests fail fast with `Unreachable`, terminal mappings are cleared,
and clients receive terminal-ended behavior. When it becomes `Ready`, Fleet refreshes the remote
snapshot, rebuilds mappings, resubscribes to events, and emits `TerminalReattach` for attachments
that should recover.

## Native agent sessions

A Claude Code or Codex session is a **thread**, and a thread is daemon state exactly like a
terminal: the app never owns one. `docs/NATIVE-AGENTS.md` is the specification and ADR 0010
records why the load-bearing choices are what they are; this is the map.

```
fleet-core::agents        ids (ThreadId/TurnId/ItemId/GateId/Seq), AgentEvent, items, gates,
                          state enums, ThreadProjection (the reducer) and AgentThreadSummary
fleet-daemon agents/           the harness layer: one live process per thread (NATIVE-AGENTS §3.1)
  harness/                the Harness trait, spawn, the probe cache, NDJSON framing, the
                          SIGTERM->SIGKILL ladder, and SchemaFingerprint (never a payload)
  claude/                 Claude Code over bidirectional stream-json on stdio
  codex/                  Codex over the app-server protocol; wire/ and methods.rs are generated
                          from the installed binary's own schema by scripts/generate-codex-wire.py
fleet-daemon services/agents/
  manager.rs              AgentSessionManager: threads, lifecycle, sequencing, broadcast
  thread.rs               one live thread: runtime handles, serialized operation gate, coalescing
  store/                  one SQLite database: the event log and every read model (ADR 0013);
                          mirror.rs owns the owner_host columns of a mirrored remote thread
  manager/window.rs       the windowed open, its page cursor and the resume admission ladder
  manager/bodies.rs       what a window cuts to fit its budget, and AgentItemBody reading it back
  manager/controls.rs     one runtime control: what the process takes now, what needs a restart
  manager/checkpoints.rs  the two capture call sites, and the rule that a failure never refuses
  manager/mirror.rs       the durable read-through mirror of the threads other hosts own
  providers/mod.rs        the one bridge between the Harness trait and the manager's verbs
fleet-daemon server/connection/events.rs
                          which events one connection may see: its own subscriptions, and the
                          capability filter that keeps an undecodable variant off its wire
fleet-daemon services/checkpoints/
                          Fleet-owned turn checkpoints: git refs under refs/fleet/checkpoints/
                          and the revert that restores files from one, no database and no harness
fleet-client api/agents/  typed commands plus AgentMirror, which replays the same reducer
fleet-app  screens/agent_thread/   Entity<AgentThreadView> per open tab, owning the transcript
                          list and the composer; rows/ is the flat row projection behind a
                          revision key; state/agents.rs mirrors summaries, windows and cursors
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
before a `TurnSettled` is applied every open item of that turn is closed, so no finished turn
shows a spinner. Gates are independent of turns and close only on `GateResolved`.

For a remote worktree, the local router sends `AgentThreadCreate` to the owning daemon before the
local agent manager can resolve a path. The returned thread UUID is registered to that host;
open/send/respond/interrupt/mode/model/seen/stop requests use that registration, while list fans
out and merges summaries — and an unreachable host's summaries come from the local mirror rather
than disappearing. Agent and summary events cross the link unchanged except for any
embedded session or terminal ids. The harness process and its stdio remain entirely on the
remote daemon; no provider stream is tunneled over the tailnet.

**Checkpoints** are a separate service on purpose: `services/checkpoints/` takes no database
handle, no event bus and no harness, because a checkpoint is a git ref and a revert is a file
operation. It records the worktree before a turn and a file before its first edit under
`refs/fleet/checkpoints/<thread>/`, restores from one without touching `HEAD`, the user's index or
the conversation, and garbage collects per thread. The ref namespace is the whole store — the
`checkpoints` **table** belongs to the projector and records compaction boundaries
(`NATIVE-AGENTS.md` §5, §8).

**Attention** is derived by the reducer, not by any view: permission > question > plan >
finished > failed > working > unread > idle, carried in `AgentThreadSummary` so the tab badge,
the session header word and the context-bar counters cannot disagree. `Finished` is amber and
clears when the client reports `AgentMarkSeen { thread, seq }`; seen state is per client and
lives in the app.

**Persistence** is `$FLEET_HOME/agents/state.sqlite`: one append-only `agent_events` log written,
projected and committed in a single transaction before the event is broadcast, plus the read
models derived from it — `threads`, `turns`, `items`, `gates`, `checkpoints`, `sessions` (ADR 0013,
`NATIVE-AGENTS.md` §8). It is not part of `PersistedState` version 1; it has its own forward-only
migration ledger. A remote thread's harness and its own database stay on the owning daemon, and
the local daemon keeps a **durable read-through mirror** of what it has read: the same tables with
one nullable `threads.owner_host` set, so an open runs the same SQL whether the thread is local or
remote. The mirror is a cache and never a replica — only the owner's link may append to its
sequence, no harness is started for it, every mutation routes upstream, and only the owner's
`AgentSynchronized` means live (`NATIVE-AGENTS.md` §9.3).

**Restart.** On boot the manager loads the index and replays each thread's log. A thread the
log leaves `Starting`/`Running` is an orphan and is settled explicitly: with a resume cursor it
records `TurnAborted { ProviderExited }` and `SessionStateChanged(Stopped)` and is resumed
lazily the next time it is opened; without one it records `SessionExited { expected: false }`.
No PID is ever reattached, and every thread stays browsable read-only regardless. A log whose
tail the reducer refuses on replay is trimmed back to the last event that reduces
(`AgentStore::truncate_after`), so one bad record costs the tail rather than the thread.

**Clients** get `Snapshot.agent_threads` for tabs and counters, then `AgentThreadOpen` for one
thread and then the event stream. An open that carries a window field — and only to a daemon
advertising `agent.window` — answers `AgentThreadWindow`: a bounded slice of turns, every open
gate whatever its turn, a keyset page cursor, and either the replay of `(after_seq, head]` or, past
the admission ladder's 1 000 events and 8 MiB, a window instead of that replay. An open without
one answers the unchanged `AgentThreadSnapshot`, or a typed refusal naming the capability when the
whole projection would not survive a frame. A `seq` gap makes the mirror return `MirrorOutcome::Gap` and the app re-opens from
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
on the client's mirror grid. The wire protocol is version 7; `wrapped` preserves logical lines
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
owner. Queue exhaustion is an explicit error, never silent loss. A kill is the one command
admitted regardless of the command budget: it is not user input, and refusing it because queued
writes filled the budget would strand the child process and the threads that own it. The host
drains at most 256 KiB or 2 ms of PTY output per iteration and batches at most 1,024 commands,
coalescing adjacent viewport moves with per-command boundary clamping. Key/resize and
application-directed wheel input preserve ordering by ending the current viewport batch. Viewport
moves emit frames immediately, bypassing the normal 16,667 µs output frame gate. A blocking command receive wakes
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

### Detached PTY holders

`fleetd` never owns a terminal's child process. Creating a terminal starts one
`fleetd pty-hold` process, `setsid`-ed before `exec` so it leaves the daemon's session and process
group; the daemon's SIGTERM, a `ctrl-c` in its terminal and its own exit therefore never reach it.
The holder owns the PTY and the login shell and nothing else — no VT engine, no scrollback, no
services — and it starts before any tokio runtime does. Its stdin is `/dev/null` and its stdout and
stderr append to `logs/pty-hold.log`, which is **not** rotated: unlike `fleetd.log` it carries only
lifecycle lines, a few per terminal, and never terminal contents. Why holders rather than tmux, and
what the failure modes cost, are in `docs/decisions/0015-detached-pty-holders.md`.

```
shell <--pty--> holder process --socket--> daemon VtEngine --> FrameUpdate --> client
```

The daemon writes `<fleet-home>/pty/<terminal-id>.json` for each holder: schema version, terminal
and session ids, the session's kind and cwd, the terminal's name, command and cwd, the holder pid,
the login-shell pid, the socket path, and the creation time — everything needed to rebuild the
registry record. A rename rewrites it; a resize does not, because window size is not recorded at
all (the holder reports its own, below). The socket name is
`<terminal-id>-<per-spawn-nonce>.sock`: terminal identifiers are reused by `restart_terminal`, so a
name per terminal would let a lingering predecessor unlink its successor's socket, and a
predictable name under a shared directory is one a local user can pre-create. Its directory is
`<fleet-home>/pty/`, created `0700`, with the socket itself `0600`; a Fleet home deep enough that a
socket path would exceed the portable `sun_path` budget (`MAX_SOCKET_PATH_BYTES`, 100 bytes, under
macOS's 104 and Linux's 108) puts sockets in a private `0700` directory in the temporary directory
instead, and a holder refuses to bind unless that directory is owned by this user with that mode.
Because the socket is therefore not always beside the record, the record stores its path and
nothing recomputes it (`fleet_core::paths::pty_socket_path`).

The wire format is `tag: u8 | length: u32 BE | payload`, with payloads capped at `MAX_FRAME_BYTES`
(8 MiB) so a corrupt length header cannot make either peer allocate without bound. Daemon to
holder: `Input(bytes)`, `Resize{cols,rows}`, `Kill`, `Detach`. Holder to daemon:
`Hello{version,child_pid,cols,rows}` — always the first frame of a connection — then
`Output(bytes)`, and finally `Exited{code}`. The two peers are different `fleetd` builds by design,
because a holder started before an upgrade is still running the old binary when the new daemon
reattaches: `HOLDER_PROTOCOL_VERSION` is the first payload byte of every greeting, a mismatch is
reported and that holder is stopped rather than silently mis-parsed, and the byte layout of every
frame is pinned by a golden test in `fleet-term`.

`Hello`'s size is the kernel's current window size and is **authoritative**: the child has been
resized since the terminal was created, and the replay tail was produced for the grid it is living
at now, so the reattaching daemon builds its emulator at that size rather than at any recorded one.
One daemon connection at a time; a new connection replaces the old one and shuts it down. Each
holder keeps a bounded **1 MiB replay buffer** of raw child output. On attach it sends `Hello`,
then the whole buffer as output, then live output, all under one lock, so a reattaching daemon sees
the tail and the live stream in order with nothing lost or duplicated; then it signals the PTY's
foreground process group as a resize would, because a full-screen TUI repaints on `SIGWINCH` and on
nothing else, and re-applying an unchanged size raises none. The buffer holds bytes, not parsed
state: a truncated escape sequence costs at most the first sequence of a replay, and the emulator
resynchronises at the next one. Writes to a connection are never timed out — a daemon behind on
reads is applying backpressure exactly as a directly owned PTY does, and mistaking that for a dead
peer would end a terminal the user is still using.

**On startup** the daemon binds its socket, then adopts before it serves anyone: a client's
connection waits in the backlog while the holders come back, so the first snapshot anyone sees
already lists the surviving sessions and no client waits on a socket that does not exist yet.
Holders are attached concurrently under one overall deadline, then inserted in terminal-id order so
tab numbering does not depend on which answered first. Terminals keep their original identifiers,
which is what lets the app's tabs reconnect without noticing; the shared terminal-id allocator is
advanced past every adopted id; and a reattach never retypes the terminal's configured command,
because that shell already ran it.

The failure paths all turn on one rule: **a live holder's socket is never unlinked**, because it is
the only way back to the user's shell. A record whose holder pid is dead is deleted with its
socket. A record this build cannot read is resolved back to its socket through the terminal
identifier in the socket's name, and that holder is stopped before the record goes. A holder that
answers with an incompatible protocol version is stopped deliberately. A holder that is alive but
does not answer in time keeps everything it has, and the next daemon start tries again. Creating a
terminal whose record cannot be written stops the holder and fails the open, rather than leaving a
shell nothing will ever adopt. What is left over — a record naming a dead holder, or a socket with
no record — is reported by `fleet doctor`'s **pty holders** check.

**On shutdown** the holders keep running. Releasing a terminal host detaches rather than kills, so
a plain SIGTERM, an unclean daemon exit and `DaemonShutdown { stop_sessions: false }` (the
`fleet daemon restart` path) all leave every child alive.
`DaemonShutdown { stop_sessions: true }` — `ctrl-shift-q` — sends `Kill` to every holder and waits
up to two seconds for them to go; any still running is then killed outright and its record removed,
so the next daemon cannot reattach the sessions the user asked to destroy. A holder exits when its
child does: it reports `Exited`, then removes its record and unlinks its socket only if that path
still names the socket it bound. A child that exits *unasked* while no daemon is connected leaves
the holder holding its socket open for up to 30 s, so a daemon restarting at that instant still
learns the exit status; the wait ends the moment the report is delivered, and never happens at all
after a `Kill`.

What does **not** survive is the daemon's own emulator state. Frame sequences restart at one and
each grid is repainted from the replay buffer, so the first screen after a restart can be shorter
than the scrollback that preceded it. The client clears its mirror grids on a restarted reconnect
and rebuilds them from the attachment frame (`docs/UX-SPEC.md` §3.12 [D-17]).

The daemon's singleton guard is part of the same contract. `SingletonGuard` holds an exclusive
`flock` on `fleetd.pid`; its `Drop` removes the pid and socket files only while the pid file still
records **this** process, because a daemon that died without unwinding leaves its successor reusing
the same inode. Acquisition reclaims a lock only when the recorded pid is dead *and* the socket
answers nobody, after a short retry covering the window in which a dying daemon's descriptor is
still closing.

Holders are per daemon, not per machine: a remote host's terminals are held by processes on that
host and survive restarts of *its* `fleetd` (`docs/REMOTE-MACHINES.md`). They do not survive a
machine reboot — nothing does — and the next daemon clears their records as stale.

## Protocol compatibility

Daemon IPC is version **7**. `Hello { protocol, client }` identifies app, CLI, or proxy peers;
the flattened Hello response adds the stable daemon id, optional build commit, and capabilities.
`HelloClient` carries the peer's own `capabilities`, defaulted and omitted when empty, so an
older client that names nothing is treated as supporting nothing optional. Federation adds
`HostStatus` provider/version/link/address/agent-binary fields (`AgentBinaries` gained a defaulted
`codex`),
`HostLinkChanged` and `TerminalReattach` events, optional host placement on PR creation, host
ownership on `WorktreePath`, and `BootstrapHost`. These fields and events are additive, but remote
daemon links require the same protocol version so routing never crosses incompatible builds.

The native-agent family grew a **bounded** read without a version bump, gated on six capability
strings: `agent.window` (windowed transcript reads and backwards pagination), `agent.sync_marker`
(an explicit catch-up completion event), `agent.resync` (a live-budget overflow reported as a
resumable cursor rather than a dropped subscriber), `agent.item_body` (a stored item body served in
256 KiB ranges), `agent.checkpoints` (Fleet-owned worktree checkpoints and the revert that restores
one), and `agent.codex` (the Codex app-server harness). `fleet_proto::AGENT_CAPABILITIES` is the
whole slice and both sides publish it: the daemon in `HelloResponse.capabilities`, the client and
the federation proxy in `HelloClient.capabilities`. Negotiation runs both ways because `Event` is
adjacently tagged — a variant a peer has no arm for fails the decode of its whole frame, not just
that event — so the three agent stream-control events are sent only to a connection that named the
capability defining them, and `Event::Unknown` is never re-broadcast. `AgentThreadOpen` gained
`after_seq`, `turn_limit`, `before_cursor` and `request_sync_marker`, all defaulted and omitted when
absent; a daemon answers the bounded `AgentThreadWindow` exactly when one of them is present and the
unbounded `AgentThreadSnapshot` otherwise, so no peer receives a payload shape it cannot decode.
`AgentThreadSnapshot` keeps its meaning and is refused above half the frame ceiling with
`ErrorKind::Unsupported` naming `agent.window` — past that ceiling the frame is undecodable rather
than truncated, which made a long thread permanently unopenable. Every window response is asserted
against `WINDOW_MAX_WIRE_BYTES` (2 MiB) and **narrowed** when it does not fit — the largest item
bodies are cut to their head and named in `TranscriptWindow.elided` for the client to re-read with
`AgentItemBody`, and only then are the oldest turns dropped. `Event::AgentWindow`, `Event::AgentSynchronized` and
`Event::AgentResync` belong to the existing `Agent` event family, so `EventKind` and every client
subscription are unchanged, and `Event` itself decodes an unknown family to `Event::Unknown`
instead of dropping the frame. A client must never send a window field to a daemon that did not
advertise the capability, and capabilities reset on disconnect.

The board and native-agent request names introduced by version 6 remain unchanged. `Snapshot`'s
`agent_threads` and `boards` are both `#[serde(default)]`, so an older snapshot payload still
deserializes; new request names are rejected rather than misread. `PruneWorktrees.ids` also remains
defaulted and omitted when `None`; `Some(ids)` is the exact reviewed allowlist, which locked
reinspection may shrink but never expand. Exact-set support is advertised as
`prune.reviewed_ids` in Hello capabilities.

Delete, inspect, and prune responses preserve per-worktree results across host fanout:
`WorktreesDeleted(Vec<WorktreeDeleteResult>)`, `Inspections(Vec<WorktreeInspection>)`, and
`Pruned(PruneResult)`. Mixed-host dismiss, sleep, and kill operations follow the same per-item
rule. Daemon Git-mutation jobs, terminal search/history/focus, cell hyperlinks/frame effects, and
persisted `ui.presentation` configuration remain outside the current protocol and config schemas.
The separate swarm-compatible CLI JSON envelope remains version 1.

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

## Boards

The daemon `Boards` service owns one versioned board document per context. It applies
`fleet-core::board` operations, validates the resulting cards, atomically saves the
whole document, updates its card-to-board index, and publishes `BoardChanged` plus a
snapshot refresh request. The index is rebuilt lazily from disk after restart. A
shared mutation gate serializes edits, worktree linking, and sync jobs so remote I/O
cannot overwrite an intervening local edit. Snapshot reads skip damaged documents
and boards whose contexts have disappeared; full views clear missing worktree links
in memory without changing their stored history.

`BoardBackends` resolves the `BoardBackend` adapter by `BackendRef.kind`. Each adapter
validates its own settings, describes statuses and properties, and maps pull/push
traffic into the core schema. Each also reports a human `label` and a `settings_schema`,
which `ListBoardBackends` publishes as `BackendDescriptor`s so the CLI and app render
backend settings generically. The production registry contains `LocalBackend`, whose
capabilities are all disabled, and `JiraBackend`. Local sync returns Unsupported
without scheduling work. Remote sync is a detached, retryable `board.sync` job with
progress logs: describe, adopt schema, pull, reconcile, push, apply acknowledgements,
save. These jobs are not cancellable because a backend push may already have remote
side effects. Schema and reconciled cards are checkpointed together before push; failures
preserve those checkpoints, record `sync.last_error`, and emit `SyncFailed`.
Successful jobs store sync timestamps/cursors and emit `Synced`. The daemon assigns
UUIDs to newly imported cards, maps old local statuses after initial schema adoption,
and resolves remote parent keys and missing labels when accepting conflicts.

`JiraBackend` (`adapters/board/jira/`) mirrors one Jira project through the Atlassian
CLI only — every call is an `acli` invocation through the `Shell` adapter, with no HTTP
client and no API token (`docs/BOARD-JIRA.md`). What that CLI can do shapes the backend:
`search` returns seven fields, so a pull searches for keys and then issues one
`workitem view` per key under a concurrency semaphore; `edit` writes only summary,
description, labels and assignee, so priority, estimate, due date and parent are
declared read-only in the `BackendSchema` and `fleet-core` refuses local edits to them;
transitions move by status *name*, so statuses are keyed by name and a Jira status the
board has never seen becomes a new column mid-sync. There is no cursor pagination, so
incremental pulls are JQL `updated >= "-Nm"` over a watermark plus an overlap, and every
Nth pull is full to catch deletions and filter exits. Descriptions and comment bodies are
Atlassian Document Format, converted both ways by a pure `adf` module. `acli`
invocations carry a 90 s timeout and retry rate-limited or transient failures with
backoff; a display name is resolved to an account id through an in-memory `UserCache`
refilled by every pull. Backend-only fields ride in `Card.properties` under `jira.*`.

Card worktrees use `Worktrees::create`, including repository hooks and the existing
prepared-copy pipeline. Repository selection is explicit argument, card repository,
then board default. Slug and branch come from `ops::worktree_slug`; an existing
repository/slug pair is reused. Linking records activity and, when configured, moves
Backlog/Unstarted cards to the first Started status. The entire create-and-link transaction
runs independently of the requesting connection; archived cards are rejected before creating worktrees.
Mutation responses normalize pruned worktree links in the same way as full board views.

Incremental pulls missing dirty-card baselines trigger a cursor-free pull before push planning.
Rejected imports retain the previous cursor. Removed backend properties and values incompatible
with a changed backend schema are removed atomically with schema adoption and reconciliation;
local property schemas and values survive remote updates. Changing a linked board's backend **kind**
is rejected; changing the settings of the same kind is allowed and clears only the sync cursor, so the
status map and the backend's read-only list keep describing the remote the board still points at. Versionless push acknowledgements require an authoritative post-push
pull under the mutation gate. If that fails, the persisted acknowledgement remains recoverable,
and further edits wait for sync to restore its baseline.

On disk, `$FLEET_HOME/boards/<board-id>.json` contains a `BoardDocument` with version 1,
board metadata, and cards. `BoardStore` validates complete documents and writes via
`Files::atomic_write_text`. Malformed documents are renamed beside the original as
`<board-id>.json.broken-<uuid>`; deletion moves the document to
`$FLEET_HOME/trash/board-<board-id>-<uuid>.json` for recovery.
