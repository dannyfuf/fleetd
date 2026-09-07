# `fleet-app` — internal contracts

**Status: authoritative for the app's internal seams.** `docs/UX-SPEC.md` decides what a screen
shows, `docs/KEYMAP.md` decides which key does it, `docs/DESIGN-SYSTEM.md` + `fleet-ui-kit`
decide what it is drawn with. This document decides **how the parts of `fleet-app` plug into
each other**: what the shell owns, what a screen owns, and what crosses between them.

These seams are stable on purpose: changing a signature here changes every screen, so change it
deliberately and update this document in the same pass.

---

## 1. Module map

| Module | Owns |
| --- | --- |
| `shell/` | the window, `AppFrame`, routing, chrome, focus, key contexts, the quit flow, the daemon surfaces |
| `state.rs` + `state/` | `AppState`: the snapshot mirror, mirror grids, modes, cursors, MRU, toasts, notifications |
| `bridge.rs` + `bridge/` | the tokio thread, the daemon channels, and request dispatch |
| `actions.rs` / `keymap.rs` | one action per `KEYMAP.md` row, and the binding table as data |
| `presentation/` | shared projection and formatting used by more than one screen |
| `screens/hub/` | §3.1–§3.5 |
| `screens/workspace/` | §3.6 |
| `screens/agent_popup/` | §3.6.1, the floating agent surface and its terminal attachment |
| `screens/jobs/` | §3.7 |
| `dialogs/` | §3.8–§3.10, plus the `ActiveDialog` entity the shell mounts |
| `terminal/` | the painted cell grid, its geometry, selection and prepared presentation |
| `views/` | domain views composed from the kit: rows, lists, the detail panel, the watch pane |
| `watches/` | the read-only subagent output mirror and its catch-up cursors |

Each of those roots holds the type and its composition only. Preparation, lifecycle, actions and
tests live in sibling modules — `screens/hub/{projection,cache,navigation,actions,composition}`,
`screens/workspace/{model,lifecycle,terminal,native,chrome,actions}` and so on.

---

## 2. The extension points

The shell mounts the Hub and the Jobs panel as plain screens, drives the Workspace and the agent
popup in two phases, and mounts dialogs as an entity.

```rust
// screens/hub.rs — prepares inside render through its own cached projection
impl HubScreen {
    pub fn new(cx: &mut App) -> Self;
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;
}

// screens/jobs.rs — same shape, rendered into the overlay layer
impl JobsPanel { pub fn new(cx: &mut App) -> Self; pub fn render(/* … */) -> AnyElement; }

// screens/workspace.rs and screens/agent_popup.rs — two phases
impl WorkspaceScreen {
    /// Attachment, requests, focus and teardown. Called before the frame is composed.
    pub(crate) fn synchronize(&mut self, state: &Entity<AppState>, bridge: &Bridge,
                              window: &mut Window, cx: &mut App);
    /// Composes what `synchronize` prepared. Issues no request, reconciles no resource.
    pub(crate) fn render_prepared(/* same arguments as `render` */) -> AnyElement;
}

// dialogs/host.rs — the shell mounts one entity, not a render call
pub struct ActiveDialog;                       // observes `AppState.overlay` and the draft host
impl ActiveDialog {
    pub fn new(state: Entity<AppState>, bridge: Bridge, focus: FocusHandle,
               cx: &mut Context<Self>) -> Self;
}

// dialogs/mod.rs
pub enum Dialogs { CreateWorktree, CloneRepo, Confirm, NewContext, EditContext, AssignRepo,
                   EditHooks, Settings, RenameTerminal, Help, Quit, QuitDaemon }
impl Dialogs {
    pub const fn context_name(&self) -> &'static str;   // the `Dialog > <name>` word
    fn render(/* state, bridge, focus, window, cx */) -> AnyElement;   // called by ActiveDialog
}
```

**Render prepares nothing.** A render function composes already-prepared data. Filesystem access,
request initiation, expensive projection and focus or lifecycle reconciliation belong in
`synchronize`, in an observation, or in a background task — never in `render`. Drafts are entities
released with the window; there is no global draft store.

Rules that come with those signatures:

1. **The returned root element must call `.track_focus(focus)`.** That is what puts your
   `on_action` listeners on the key-dispatch path: gpui dispatches an action from the focused
   node upwards, so a listener *below* the focus node never fires. The shell focuses the handle
   it passed you and never touches focus again.
2. **Read and write `AppState` through the entity.** `state.read(cx)` for reads,
   `state.update(cx, |state, cx| { …; cx.notify(); })` for writes. The shell observes the
   entity, so a `cx.notify()` inside that closure repaints the whole frame.
3. **The daemon is only reachable through `Bridge`** (§4). Clone it into your listeners.
4. **Do not draw the context bar, the status bar, the banner or the toasts.** The shell owns all
   four, on every screen, at the same pixel positions (§2.1).
5. The `Dialogs` variant **names** and the strings from `context_name()` are stable, because
   `keymap.rs` binds against them. A dialog's own data lives in its draft entity, not in the
   variant: `Confirm` reads the pending `ConfirmRequest` that `dialogs::request_confirm` staged.

### What the shell already handles for you

Do not re-implement these; they arrive as `AppState` changes:

* opening and closing every overlay, and the mode word that follows it;
* `Esc` (`fleet::Cancel`, `dialog::Cancel`, `filter::Escape`) including the two-stage filter
  escape, and `q` closing the Jobs panel;
* the whole quit flow, `ctrl-q` and `ctrl-shift-q`, including `W` never-warn;
* the daemon surfaces of §3.12 and the `Daemon > Down` / `Daemon > Banner` keys;
* the one-shot prefix: entering it, and leaving it on the very next key;
* pane focus (`h`/`l`/`Tab`), screen switching (`p`, `gw`, `gp`), `i`, `H`, scroll-mode entry
  and exit.

If your dialog needs `Esc` for something else — §3.8.2's "Esc cancels only the search request,
never a started clone" — handle `dialog::Cancel` yourself and call `cx.stop_propagation()`.

### What you publish back into `AppState`

Three fields exist purely so a screen can feed the shell's chrome:

| Field | Set by | Used by |
| --- | --- | --- |
| `breadcrumb_row: Option<String>` | the focused list | the status-bar breadcrumb (§2.2) |
| `review_pr_count: usize` | the PR screen | the `flag` chip (§2.3) |
| `update_version: Option<String>` | whoever learns about an update | the `↑version` chip (§2.3, D-2) |

---

## 3. Key contexts

`AppState::context_chain()` returns the nested contexts of the focused element, outermost
first, and the shell renders one `div().key_context(_)` per entry above your element. The root
is always `Fleet`.

| Situation | Chain |
| --- | --- |
| Hub, repos rail focused | `Fleet > Hub > Repos` |
| Hub, worktrees list focused | `Fleet > Hub > Worktrees` |
| Hub, PR screen focused | `Fleet > Hub > Prs` |
| Workspace, PTY tab | `Fleet > Workspace > Terminal` \| `Prefix` \| `Scroll` |
| Workspace, `fleet://` tab | `Fleet > Workspace > Native`, then the embedded view's own chain (`> Lazygit > Panels > Files`, …) |
| Floating agent terminal | `Fleet > Agent > Terminal` \| `Prefix` \| `Scroll` |
| Filter / Palette / Jobs | `Fleet > Filter` \| `Palette` \| `Jobs` |
| Any dialog | `Fleet > Dialog > <name>` |
| Daemon banner showing (§3.12 C) | the base chain **plus** `Daemon > Banner`, innermost |
| fleetd will not start (§3.12 B) | `Fleet > Daemon > Down` |
| First run (§3.13) | `Fleet > FirstRun` |

Two consequences worth knowing:

* A deeper context wins, so `Hub > Prs`'s `l` (next PR tab) beats `Hub`'s `l` (next pane).
* gpui's `>` is a **subsequence** test over the rendered chain, not a parent test. An embedded
  view therefore may not reuse any context word this table uses: `crates/fleet-lazygit` renames
  its overlay words to `LgDialog` / `LgConfirm` / `LgHelp` for exactly that reason, and the
  `the_embedded_pane_shares_no_context_word_with_the_app` test in `keymap.rs` keeps it true.
  The same test file checks that no gpui action name is registered twice: actions are
  process-wide (`namespace::Name` via `inventory`) and `App::load_actions` panics on a
  duplicate, so the pane's colliding namespaces are `lg_confirm` and `lg_help`.
* While the §3.12 C banner is undismissed it is the **innermost** context, so `r`, `l` and
  `Esc` belong to it, exactly as `KEYMAP.md` says. `Esc` dismisses the banner and hands those
  keys straight back.

The floating agent is persistent state beside the ordinary `overlay` slot, not another member of
that mutually-exclusive enum. When it is topmost, `Agent` owns focus and the Hub/Workspace remains
mounted underneath. Help and `Quit` / `QuitDaemon` may occupy the one ordinary overlay slot above
it; closing that dialog returns focus to `Agent`. Palette, Settings, Filter, and Jobs have no
binding in `Agent`, so they cannot create a second competing topmost surface. `ctrl-q` resolves to
`agent::Hide` in this context; `ctrl-shift-q` remains the global stop-confirm action.

Every focus-owner generation change also dirties the window. gpui synchronously draws a dirty
window before dispatching keyboard input, so the new context/focus tree is normally already live
when the next key resolves. A bounded FIFO remains as a safety net if a generation ever advances
without that draw: it retains the complete key-down event and replays it through gpui's normal
dispatch in order; a replay that changes owner pauses the remaining events behind the next draw.
Pointer input is never queued. Because gpui mouse dispatch hit-tests the last rendered frame, each
painted root gate captures that frame's focus-owner generation and drops mouse events once the live
generation advances past it. The gate also records whether its frame exposes the base screen, so
an old base frame cannot receive pointer input once the popup is live-open. It is painted after the
interactive tree: gpui's forward capture pass first clears pending clicks and pressed state, then
the gate stops propagation before reverse-order bubble handlers can resize, scroll, select, focus,
or send terminal input. Agent and Workspace terminal handlers additionally verify their exact live
terminal owner.

### Arbitrations against `KEYMAP.md`

* **`q` is not bound in Palette mode.** `KEYMAP.md` lists `Esc`, `q` for the palette, but gpui
  dispatches bindings *before* a text input sees the key, so binding `q` makes the query
  untypable. Only `Esc` closes the palette. Proposed amendment: drop `q` from that row.
* **`f` in the Jobs panel** is one binding (`jobs::CycleFilter`). `KEYMAP.md` gives it two
  meanings — cycle the filter in the list, toggle follow inside an expanded log — which one key
  in one context cannot dispatch twice; the jobs agent routes on whether a log is expanded.
* **`q` closes the Jobs panel**, from the global "`q` closes the topmost overlay" row, even
  though the §Jobs table lists only `J` and `Esc`.

---

## 4. The daemon bridge

```rust
let bridge: Bridge;                                  // cloneable, thread-safe
bridge.send(RequestBody::…);                         // fire and forget
let reply = bridge.request(RequestBody::…);          // async_channel::Receiver<Result<ResponseBody, ProtoError>>
bridge.reconnect();                                  // `r` on a daemon surface
bridge.shutdown();                                   // quitting
```

Use `bridge.send` for event-backed mutations that do not gate a local state change or navigation:
the daemon re-broadcasts its snapshot, the shell applies it, and the screen re-renders. Enqueue and
background delivery failures return as `BridgeEvent::MutationFailed` and occupy the sticky error
slot; a fire-and-forget call must never fail invisibly. Use `bridge.request` when the answer gates
local state/navigation or when you need the value itself (an acknowledgement, a path to copy, a
doctor table, a PR slice):

```rust
let reply = bridge.request(RequestBody::WorktreePath { id });
cx.spawn(async move |_, cx| {
    if let Ok(Ok(ResponseBody::Path(path))) = reply.recv().await {
        state.update(cx, |state, cx| { /* … */ cx.notify(); });
    }
})
.detach();
```

`BridgeEvent::EffectiveConfig` hydrates the app-facing terminal and notification settings,
`jobs.warnBeforeQuit`, and the PR-cache TTL on every connection/reconnection and config response.
Consumers use those effective values rather than reconstructing persisted defaults.

Terminals attach with plain requests (`AttachTerminal`, `ResizeTerminal`, `TerminalKey`,
`ScrollTerminal`, `DetachTerminal`); frames arrive as ordinary events and the shell has already
applied them to `AppState::grids` before your screen renders. `fleet-client` re-attaches every
terminal after a reconnect on its own. Daemon and host share one absolute five-second attach
deadline. App surfaces await the correlated acknowledgement with a bounded deadline; a refusal or
timeout clears their optimistic attachment and generation, reports a sticky error, and schedules
reconciliation again. A request already expired when the owner services it cannot resize the PTY,
and no post-deadline result can publish an attachment frame or claim an attachment. Before the
first valid frame, each app surface retains an ordered prefix of at most
1,024 key/paste events and 1 MiB; it rejects the newest input after either bound and flushes the
retained prefix only to the same live terminal.

**Never block the foreground thread on the daemon.** Everything above is non-blocking by
construction; there is no synchronous path and there must not be one.

### IPC and CLI compatibility

Daemon IPC remains version 4. The bug-fix program added one request field:
`PruneWorktrees.ids: Option<Vec<WorktreeId>>`. It is defaulted and omitted when `None`, so an old
request still decodes and the current legacy call path emits the byte-identical request shape.
`None` retains repo/all-worktree discovery. The prune dialog sends `Some(ids)` only when committing,
with exactly the DELETE rows the user reviewed; the daemon locks and re-inspects those IDs and may
keep newly ineligible entries, but it never expands the set. An empty explicit list deletes nothing.

This guarantees old-client/new-daemon compatibility. The mandatory Hello response envelope has a
defaulted, omitted-when-empty `capabilities` array. A daemon that honors exact IDs advertises
`prune.reviewed_ids`; the client retains the latest negotiated set, and the app refuses a reviewed
prune with update/restart guidance when that capability is absent. The metadata is outside the
existing Hello body so older IPC-v4 decoders ignore it without changing the version.

Pong response envelopes likewise have an optional `daemon` object containing `pid` and `bootId`.
`bootId` is stable for one fleetd process and changes across starts, including PID reuse. New
clients retain it while delivering the existing unit `Pong` body to callers; older clients ignore
the additive envelope member. App reconnect identity probes use this Pong metadata and never load
a fallback snapshot. IPC v4 otherwise does not include daemon Git-mutation jobs, arbitrary
terminal-history reads, terminal search/focus requests, cell hyperlinks, or frame effects. Those
deferred surfaces require a separately negotiated additive contract before clients may send them.
The public JSON CLI is a separate, unchanged protocol-1 envelope.

---

## 5. The state mirror

`AppState` (`state.rs`, with its reducers in `state/{connection,navigation,notifications,snapshot,terminal}.rs`)
is the single source of truth on the client. The parts a screen touches:

| Field | Meaning |
| --- | --- |
| `snapshot: Option<Snapshot>` | the daemon's authoritative state; `None` until the first one lands |
| `snapshot_at` / `snapshot_age(now)` | what the `stale · <age>` stamp ages (§1.3) |
| `grids: HashMap<TerminalId, MirrorGrid>` | one mirror grid per terminal, diffs already applied |
| `displayed_hub: DisplayedHub` | stable IDs and rows from the Hub's current scoped/sorted/filtered projection |
| `screen`, `hub_pane`, `pr_tab`, `scope`, `cursors` | where the cursor is, per list |
| `terminal_mode`, `agent_popup`, `overlay`, `mode()` | the base Workspace mode, floating-agent mode, top overlay, and resulting mode word/key context |
| `filter` | query + whether the input still owns the keyboard |
| `session_mru`, `terminal_mru` | `ctrl-s w` and `ctrl-s Tab` are `Mru::alternate()` |
| `toasts`, `sticky_error` | §2.7 and §1.8; errors are sticky, never toasts |
| `watches: Watches` | the read-only subagent mirror and its per-session pane state |
| `daemon: DaemonLink` | §3.12; `refuses_mutations()` and `drops_terminal_keys()` are the two questions a screen asks |

`agent_popup: Option<AgentPopupState>` is screen-independent. Its `Terminal` / `Prefix` / `Scroll`
submodes reuse the existing `TERMINAL` / `^S` / `SCROLL` status words; it does not add a ninth mode
word. An ordinary dialog above it temporarily shows `DIALOG`, then reveals the popup's prior word.

Dialogs, the palette, and activation/destructive actions resolve their target from
`displayed_hub`, not by repeating filters against the raw snapshot. Cursor movement may clamp an
index, but acting always uses the stable identity of the row the user can currently see.

Pure helpers worth reusing rather than re-deriving, all re-exported from `state`:
`move_cursor`, `clamp_cursor`, `half_page` and `filter_escape` (`state/navigation.rs`);
`push_toast`, `expire_toasts`, `dwell_for`, `latest_failed_job` and `running_jobs`
(`state/notifications.rs`); `breadcrumb` and `ChipCounts` (`state/snapshot.rs`);
`reconnect_backoff` (`state/connection.rs`); `parse_percent` (`presentation/jobs.rs`).

`MirrorGrid::apply` already ignores a diff that arrives before the first full frame or out of
sequence, and resizes on a full frame; paint from `lines`, `cursor`, `viewport` and `modes` and
nothing else.

---

## 6. Keys, actions and the help overlay

`keymap::table()` returns the whole binding table as data — `{ keys, context, action }` — in
registration order. The help overlay (§3.8.7) and the palette's right-aligned key hints must
render from it rather than restating keys, so a binding and its documentation cannot drift.

`actions.rs` has one module per namespace (`hub`, `repos`, `worktrees`, `prs`, `prefix`,
`scroll`, `filter`, `palette`, `jobs`, `dialog`, `confirm`, `settings`, …). Listen for the ones
your screen owns; the shell already owns the ones listed in §2.

### Actions stop at the first listener

gpui dispatches an action up the focus chain and, **in the bubble phase, stops at the first
listener that handles it** (`window.rs`: `cx.propagate_event = false; // Actions stop
propagation by default during the bubble phase`). `Interactivity::on_action` only ever fires on
Bubble, and a screen or dialog tracks the focus handle, so *its* listener is always the
innermost one and always runs first.

A listener that does part of the work and expects the shell to do the rest must therefore end
with `cx.propagate()`. There is no "also run the outer handler" by default, and the failure is
silent: the key is consumed and nothing else happens. Any comment that says "the shell owns …"
is a promise that the listener calls `cx.propagate()` — a panel that resets its own state and
keeps `jobs::Close` can never be closed.

The mirror-image rule holds too: a listener that fully handles an action and must stop an
outer one from *also* acting calls `cx.stop_propagation()` explicitly, so both directions are
stated rather than inherited.

Two behaviours the Workspace must implement itself, because only it can:

* forward a key to the PTY **only** while `terminal_mode == TerminalMode::Terminal` — a key
  typed in `Prefix` or `Scroll` must never reach the shell's PTY;
* **drop** keys, never buffer them, while `drops_terminal_keys()` is true (§3.12 C). The shell
  already paints the 55 % veil over whatever the Workspace returns.

## Subagent watches

PTY login shells receive `FLEET_SESSION` (session ID), `FLEET_TERMINAL` (human
terminal name, retained for compatibility), and `FLEET_TERMINAL_ID` (numeric
daemon-local ID matching the terminal registry). `fleet exec --watch` requires
a valid `FLEET_SESSION` and parses `FLEET_TERMINAL_ID` for `StartWatch.terminal`.
Missing/invalid IDs, absent `--watch`, an existing `FLEET_WATCH`, or daemon
connection failure cause transparent passthrough. `FLEET_DEBUG=1` enables a
one-line reason for every passthrough decision; default stderr remains unchanged.
Shims require `FLEET_TERMINAL_ID`, so older name-only daemon environments stay
transparent.

`fleet-core::watches` defines `WatchId` (daemon-local numeric ID), `Watch`,
`WatchSource::{Cooperative, Discovered}`,
`WatchStatus::{Running, Exited { code, signal }}`, `WatchStream::{Stdout, Stderr}`,
and immutable `WatchChunk { seq, stream, text }`. Watch metadata includes parent
session/terminal, label, argv, optional cwd/PID, RFC3339 `startedAt`, `source`, and
optional `logFile`. Struct fields are camelCase on the wire. `source` and `logFile`
have serde defaults (`cooperative` and null), so new clients accept older daemon data.
Watch IDs and retained buffers are runtime-only; live discovered watches are rebuilt
by the next scan after a daemon restart.

| Request | Response |
| --- | --- |
| `StartWatch { terminal, label, command, cwd, pid }` | `WatchStarted(WatchId)`; validates terminal and derives session |
| `AppendWatchOutput { watch, stream, text }` | `Ack`; completed watches reject output |
| `FinishWatch { watch, code, signal }` | `Ack`; first completion wins |
| `ListWatches { session }` | `Watches(Vec<Watch>)` in registration order |
| `TailWatch { watch, from_seq }` | `WatchTail { watch: Watch, chunks, first_retained_seq, next_seq }` |
| `DismissWatch { watch }` | `Ack`; Running returns `Conflict`; missing IDs return `NotFound` |

Request discriminants and fields use snake_case, like other requests. Watch and
catch-up struct fields use camelCase on the wire. Each event has its own
`EventKind`: `WatchStarted(Watch)`, `WatchOutput { watch: WatchId, chunks }`,
`WatchExited(Watch)`, `WatchDismissed(WatchId)`. These events are global, like
`SessionChanged`; clients filter using watch metadata. No terminal attachment is
needed. `Client::connect` subscribes to them by default. Explicit subscriptions
should include all four kinds. `Client::events()` provides the receiver, and
`fleet-client::watches` adds typed `start_watch`, `append_watch_output`,
`finish_watch`, `list_watches`, `tail_watch`, and `dismiss_watch` methods.

Subscribe/create the receiver before listing and tailing. Sequence numbers start
at zero and `from_seq` is inclusive; `None` returns all retained chunks. Merge
catch-up and queued events by sequence, ignoring duplicates. Resume from
`next_seq`. On sequence gaps, receiver lag, or reconnect, list/tail again. If the
requested cursor precedes `first_retained_seq`, show that older output was
trimmed. Metadata and output in a tail response are atomic. After daemon restart,
clear prior IDs before rebuilding from ListWatches; live matching processes receive
new IDs on the next discovery scan.

Each watch retains at most 1 MiB of UTF-8 text and 20,000 chunks, dropping whole
oldest chunks (including a chunk larger than the cap). This keeps worst-case JSON
escaping below the 16 MiB frame limit. Pending events reuse that bounded history.
Output events coalesce on a 50 ms daemon tick; finish flushes pending output before
`WatchExited`. Finished watches expire 30 minutes after completion. Dismissal,
TTL, terminal close, and session kill emit `WatchDismissed`; none of the watch
operations signal a child. Closing a running pane hides it locally; `DismissWatch`
is only for completed watches.

AppendWatchOutput and FinishWatch require the starting connection (otherwise
`Conflict`), preventing reconnected wrappers from modifying an unrelated reused
ID after a daemon restart. They always return `Conflict` for a `discovered` watch.
A connection lease owns each StartWatch; dropping its socket marks unfinished
watches Exited with `code: None, signal: None` because the daemon does not know the
child's exit cause. The wrapper uses a private PID-preserving
launcher handshake so StartWatch has the child PID and the target starts with
`FLEET_WATCH`. It forwards SIGINT/SIGTERM/SIGHUP and preserves normal/signalled
exit codes. Reporting uses a bounded 128-chunk queue and batches up to 256 KiB
per 50 ms, coalescing each stream separately (cross-stream interleaving is not
preserved). Original bytes are written unchanged and flushed before enqueueing.
A full queue applies backpressure to subsequent reads instead of silently losing
burst output. A two-second RPC timeout closes the copy queue so daemon failure
cannot strand the tee threads. Child completion is still attempted when output
reporting failed; disconnect interruption remains authoritative if already set.

### Discovered lifecycle and configuration

`discoveredWatches.enabled` defaults to `true`; `intervalMs` defaults to 2000 and is
clamped to at least 500 ms by the loop. `processes` is an ordered list of
`{ id, pattern, enabled }` rules. Invalid regexes are skipped. Defaults are:

| id | pattern |
| --- | --- |
| `codex-companion` | `codex-companion\.mjs task-worker` |
| `codex` | `(^|/)codex( |$)` |
| `claude` | `(^|/)claude( |$)` |
| `opencode` | `(^|/)opencode( |$)` |

Each scan uses one process snapshot. Environment reads are limited to candidates and successful
reads are cached by `(PID, process start identity)`; failures remain retryable, and PID reuse
cannot inherit a prior process's environment. Ownership order is valid Fleet env tags, then descent from a session
terminal's shell PID. A valid session without a valid terminal selects the terminal
whose launch command matches the configured agent command, then the first terminal.
Any process launched directly by a terminal's PTY shell (the terminal's own foreground
program, such as `cc` resolving to `claude`), shell wrappers, nested matching
children, and these helpers are hidden: `app-server-broker.mjs`, `codex app-server`,
`codex-code-mode-host`, `unified-computer-use.*launch.mjs`, and
`codex-companion.mjs status`. PID dedupe includes both sources.

Discovered watches have no owner connection. A dead PID exits with null code and
signal. Companion `done`/`completed`/`succeeded` maps to code 0 and `failed`/`error`
maps to code 1. The companion job path uses `CLAUDE_PLUGIN_DATA/state` when set;
otherwise it prefers the worker's `TMPDIR/codex-companion`, then the daemon temp dir.
The JSON `logFile` wins over the computed jobs path. The daemon polls logs every 500 ms,
starts at the last 64 KiB, appends new bytes as stdout, and restarts at byte zero after
truncation or file replacement. Generic discoveries emit one output-not-captured line.
All watches retain the same 30-minute completion TTL and dismissal rules. Discovery is
strictly observational and never signals a process.

The fallback state directory is
`<temp>/codex-companion/<slug>-<hash>/jobs`: `slug` is the workspace basename with each
run of non-`[a-zA-Z0-9._-]` characters replaced by `-`, and `hash` is the first 16 hex
digits of SHA-256 over the real workspace path. Each job uses `<job-id>.json` and
`<job-id>.log` beneath that directory.

### Read-only watch CLI

`fleet watch list [--session <id>] [--json]` calls `ListWatches`. An explicit session
wins over `FLEET_SESSION`; if neither is available, validation fails before daemon
autostart with `fleet watch list requires --session <id> or FLEET_SESSION`.
Human output is one tab-separated row per watch: numeric id, source, single-line label,
status (`running`, `exited N`, or `interrupted` when no exit code is available),
RFC3339 start time, and numeric terminal id. Empty lists produce no human rows.
JSON uses the CLI protocol-1 envelope `{"protocol":1,"watches":[...]}` with the
existing serialized `Watch` metadata. Errors use the standard CLI error envelope.

`fleet watch tail <id> [--follow]` initially calls `TailWatch` with no cursor and
prints retained chunks as text, forwarding stderr chunks to stderr. With `--follow`,
it polls every 250 ms from the previous response's `next_seq`, printing the final
snapshot's output before stopping on `Exited` (including interruptions). Without
`--follow`, it prints one snapshot. It needs no session environment variable and
returns CLI success on completed reads, independently of the watched child's exit
code. Output is bounded retained display text; evicted chunks cannot be recovered.
No protocol or daemon lifecycle changes are needed for these commands.

### Workspace mirror and recovery

`fleet-app::watches::Watches` owns metadata, stream-tagged lines, independent stdout
and stderr partials, the next inclusive sequence cursor, trimming state, and
per-session selection/visibility. The shell drains pending ListWatches/TailWatch
work through the ordinary Bridge. Lists reconcile only watches known when the
request was sent, preserving concurrent starts. Dismissal tombstones and link
generation checks prevent delayed responses from reviving removed or prior-daemon
IDs. One tail per watch is in flight; live chunks and catch-up results merge by
sequence and a remaining gap requests another tail.

Workspace entry/session changes and reconnect list then tail every retained watch
with `from_seq: None`. A detected output gap tails from the next expected cursor.
Failed list or tail requests remain unsynchronized and make four total attempts with 1 s, 2 s,
and 4 s backoff between retries. Invalidation, daemon reconnect, and session entry reset this
bounded retry sequence. A failed completed-watch tail preserves its exact cursor and reports a
sticky error rather than declaring success.
Receiver lag also invalidates the list. Tail retention gaps discard incomplete
partial lines before resuming; local line/text retention also sets the trimmed
indicator. `LogView::line_tones` uses existing semantic tones for stderr.

Elapsed time starts from `Watch.started_at` and freezes when this client first
observes exit. The wire contract has no completion timestamp, so a watch first
loaded after it has finished uses an estimated duration through discovery time;
a client that observed the exit retains its frozen duration across reconnect.
The pane consumes the shared `source` marker and discovered lifecycle without adding
another request or key binding.

`^s N` / `^s P` select in session-local registration order (monotonic watch IDs),
wrap, show a hidden pane, and leave prefix mode without changing the active terminal.
Empty sessions toast `no subagent watches`; a sole watch remains selected silently.
The help overlay derives these entries from the authoritative keymap.

Watch tabs and close controls use stateful GPUI `on_click` listeners. GPUI 0.2.2
(Zed v1.18.1) delivers `on_mouse_up_out` during capture and clicks during bubble.
The terminal's outside-release handler must stop propagation only when ending an
active terminal drag; idle or already-completed selections must let sibling watch
clicks reach bubble. The handler-path unit test covers this ownership decision,
including retaining completed selections for copying. There is no GPUI test-app
harness in this crate; actual pointer delivery still needs a host GUI smoke test.
