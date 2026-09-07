# `fleet-app` — internal contracts

**Status: authoritative for the app's internal seams.** `docs/UX-SPEC.md` decides what a screen
shows, `docs/KEYMAP.md` decides which key does it, `docs/DESIGN-SYSTEM.md` + `fleet-ui-kit`
decide what it is drawn with. This document decides **how the parts of `fleet-app` plug into
each other**, so the Hub, the Workspace, the Jobs panel and the dialogs can be implemented in
parallel without touching the shell.

Everything below is frozen. A change to a signature here is a cross-agent change: raise it as an
integration request instead of editing another agent's file.

---

## 1. Module map and ownership

| Module | Owns | Implemented by |
| --- | --- | --- |
| `shell/` | window, `AppFrame`, routing, chrome, focus, key contexts, quit flow, daemon surfaces | app-shell |
| `state.rs` | `AppState`, the snapshot mirror, mirror grids, modes, cursors, MRU, toasts | app-shell |
| `bridge.rs` | the tokio thread and the daemon channels | app-shell |
| `actions.rs` / `keymap.rs` | one action per `KEYMAP.md` row and the binding table | app-shell |
| `screens/hub.rs` | §3.1–§3.5 | hub agent |
| `screens/board.rs` | BOARD §8 board tab, loading and action extension points | contracts-app / app stage |
| `screens/workspace.rs` | §3.6 | workspace agent |
| `screens/agent_popup.rs` | §3.6.1 floating agent surface, coordinated terminal attachment | app-shell |
| `screens/jobs.rs` | §3.7 | jobs agent |
| `dialogs/` | §3.8, §3.9, §3.10 | dialogs agent |
| `terminal_element.rs` | the painted cell grid | workspace agent |
| `views/` | shared domain views composed from the kit (rows, detail panel, …) | whoever needs one first |

The placeholder files under `screens/` are meant to be **replaced wholesale**. Keep the
constructor, the render signature and the focus rule; everything else is yours.

---

## 2. The extension points

```rust
// screens/hub.rs
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

// screens/board.rs — same frozen constructor/render signature as HubScreen
impl BoardScreen { pub fn new(cx: &mut App) -> Self; pub fn render(/* … */) -> AnyElement; }

// screens/workspace.rs — same shape
impl WorkspaceScreen { pub fn new(cx: &mut App) -> Self; pub fn render(/* … */) -> AnyElement; }

// screens/jobs.rs — same shape, rendered into the overlay layer
impl JobsPanel { pub fn new(cx: &mut App) -> Self; pub fn render(/* … */) -> AnyElement; }

// dialogs/mod.rs
pub enum Dialogs { CreateWorktree, CloneRepo, Confirm, NewContext, EditContext,
                   AssignRepo, Settings, Help, Quit, QuitDaemon,
                   CardDetail, CardCreate, CardPicker, BoardSettings }
impl Dialogs {
    pub const fn context_name(&self) -> &'static str;   // the `Dialog > <name>` word
    pub const fn title(&self) -> &'static str;
    pub fn render(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;
}
```

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
5. `Dialogs` variants may grow payloads (`Confirm(ConfirmState)`, …). The variant **names** and
   the strings from `context_name()` are frozen, because `keymap.rs` binds against them.

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
| Hub, board tab (independent of repo pane selection) | `Fleet > Hub > Board` |
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

`bridge.send` is the right call for every mutation: the daemon re-broadcasts its snapshot, the
shell applies it, and your screen re-renders. Use `bridge.request` only when you need the
answer itself (a path to copy, a doctor table, a PR slice):

```rust
let reply = bridge.request(RequestBody::WorktreePath { id });
cx.spawn(async move |_, cx| {
    if let Ok(Ok(ResponseBody::Path(path))) = reply.recv().await {
        state.update(cx, |state, cx| { /* … */ cx.notify(); });
    }
})
.detach();
```

Terminals attach with plain requests (`AttachTerminal`, `ResizeTerminal`, `TerminalKey`,
`ScrollTerminal`, `DetachTerminal`); frames arrive as ordinary events and the shell has already
applied them to `AppState::grids` before your screen renders. `fleet-client` re-attaches every
terminal after a reconnect on its own.

**Never block the foreground thread on the daemon.** Everything above is non-blocking by
construction; there is no synchronous path and there must not be one.

---

## 5. The state mirror

`AppState` (`state.rs`) is the single source of truth on the client. The parts a screen touches:

| Field | Meaning |
| --- | --- |
| `snapshot: Option<Snapshot>` | the daemon's authoritative state; `None` until the first one lands |
| `snapshot_at` / `snapshot_age(now)` | what the `stale · <age>` stamp ages (§1.3) |
| `grids: HashMap<TerminalId, MirrorGrid>` | one mirror grid per terminal, diffs already applied |
| `board: BoardState` | active context’s `BoardView`, loading/error, `BoardFocus { column, row }`, filter and optional `GroupBy` |
| `board_stale: bool` | authoritative refresh pending; lives outside the frozen `BoardState` fields |
| `board_backends: Vec<BackendDescriptor>` | the daemon's backend registry, fetched once per connection; the header label and the settings dialog's rows are drawn from it |
| `screen`, `hub_pane`, `pr_tab`, `scope`, `cursors` | where the cursor is, per list |
| `terminal_mode`, `agent_popup`, `overlay`, `mode()` | the base Workspace mode, floating-agent mode, top overlay, and resulting mode word/key context |
| `filter` | query + whether the input still owns the keyboard |
| `session_mru`, `terminal_mru` | `ctrl-s w` and `ctrl-s Tab` are `Mru::alternate()` |
| `toasts`, `sticky_error` | §2.7 and §1.8; errors are sticky, never toasts |
| `daemon: DaemonLink` | §3.12; `refuses_mutations()` and `drops_terminal_keys()` are the two questions a screen asks |

`agent_popup: Option<AgentPopupState>` is screen-independent. Its `Terminal` / `Prefix` / `Scroll`
submodes reuse the existing `TERMINAL` / `^S` / `SCROLL` status words; it does not add a ninth mode
word. An ordinary dialog above it temporarily shows `DIALOG`, then reveals the popup's prior word.

Pure helpers worth reusing rather than re-deriving: `move_cursor`, `clamp_cursor`, `half_page`,
`push_toast`, `expire_toasts`, `latest_failed_job`, `running_jobs`, `parse_percent`,
`chip_counts`, `breadcrumb`, `filter_escape`, `reconnect_backoff`.

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
silent: the key is consumed and nothing else happens. This is what made `J` / `q` / `Esc`
unable to close the Jobs panel — the panel reset its own state and never handed `jobs::Close`
back to `Shell::close_jobs`. Any comment that says "the shell owns …" is a promise that the
listener calls `cx.propagate()`.

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
operations signal a child. Closing a running pane in phase 2 should hide it
locally; `DismissWatch` is only for completed watches.

AppendWatchOutput and FinishWatch require the starting connection (otherwise
`Conflict`), preventing reconnected wrappers from modifying an unrelated reused
ID after a daemon restart. They always return `Conflict` for a `discovered` watch.
A connection lease owns each StartWatch; dropping its socket marks unfinished
watches Exited with `code: None, signal: Some(9)` (an interruption marker, not a
claim that the child received SIGKILL). The wrapper uses a private PID-preserving
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

Each scan uses one process snapshot. Environment reads are limited to candidates and
cached by PID. Ownership order is valid Fleet env tags, then descent from a session
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

## Board app extension points (BOARD §8)

`HubTab::Board` is selected by `board::GoBoard` (`g b`). `HubScreen` owns
`screens::board::BoardScreen` and renders it with the frozen signature. The Hub's
screen strip includes `Board`, the active context's summary `open_count`, and a
conflict dot when `conflict_count > 0`. `BoardScreen` owns one horizontal
`ScrollHandle` for the columns and one per column for its cards, and reveals the
focused column and card when their selection changes. Only the inner Board root tracks the
Hub focus handle on that tab. `views::board_screen` holds the pure model
(filter predicate, visible slice of a column, priority and category mappings, header
facts) and the rendering; `views::board_card_detail` holds the property-row model and
the detail panes. Placement and content decisions are in `UX-SPEC.md` § Board.

`BoardState` has `view: Option<BoardView>`, `loading: bool`,
`error: Option<String>`, `focus: BoardFocus`, `filter: String`,
`filter_editing: bool` and `group_secondary: Option<GroupBy>`.
`filter_editing` records whether the board filter owns text input. It is what
`AppState::board_filter_owns_keys` reads, and while it is set `context_chain()`
returns `["Filter", "BoardFilter"]` (and `mode()` returns `Mode::Filter`) instead of
`["Hub", "Board"]`, so the board's bare letters type instead of firing.
`AppState::board_filter_escape()` is the §3.10 two-stage `Esc` for it; base Cancel
calls its second stage. `Filter > BoardFilter` adds left/right and ctrl-b/ctrl-f
column navigation to the inherited Filter editing bindings.
`clamp_board_focus` is `pub(crate)` and clamps against the **filtered** column, so the
selection can never point at a hidden card. `BoardFocus` has `column: usize` and `row: usize`;
`GroupBy` is `Priority | Assignee | Labels`. `AppState::board() -> Option<&BoardView>`
returns the current view. Reducers are `apply_board_view(BoardView)`,
`apply_card(Card)`, and `clear_board()`. Card upserts reject another board, sort by status and position, and clamp
focus. Unknown-status responses request a full refresh instead of inserting an invisible card; board views reject another active context. Clear resets the draft and
invalidates pending responses. `apply_daemon_event(Event, Instant)` handles
`Event::BoardChanged` by setting `board_stale` only for the displayed board.

There are **no new `BridgeEvent` variants**: like PR/worktree response consumers,
`screens::board::ensure_current` awaits the receiver returned by
`Bridge::request(RequestBody::EnsureBoard { context_id })`. It applies
`ResponseBody::Board` via `finish_board_load` and `apply_board_view`. Card
request consumers apply `ResponseBody::Card` through `apply_card`.
The board loader runs on tab entry, active-context change, reconnect, or a stale
board's next render. Only one request is in flight per generation. Context switches
(including A → B → A) and link changes reject old responses. Errors remain visible
in state until reload; an event arriving during a refresh schedules one more load.

The payload-free `Dialogs` variants and `context_name()` values are `CardDetail`,
`CardCreate`, `CardPicker`, and `BoardSettings`. Their titles are `Card detail`,
`New card`, `Card property`, and `Board settings`. Each module exposes `render`
with the ordinary dialog signature; each returned root tracks focus and closes on
Escape. `DialogHost` owns these public fields:

| Field | Type | Initial draft |
| --- | --- | --- |
| `card_detail` | `card_detail::CardDetailState` | `card_id`, `property_row`, `area: TextAreaState`, `edit: Option<CardEdit>`, `revision`, `saving: Option<u64>` and `error` |
| `card_create` | `card_create::CardCreateState` | `board_id`, `draft: CardDraft`, plus `field`, `title_caret` (chars), `description_area: TextAreaState` and `error` |
| `card_picker` | `card_picker::CardPickerState` | `kind`, `card_id`, `query`, `cursor`, plus `caret`, `selected: Vec<String>`, `then_worktree`, `then_detail` and `error` |
| `board_settings` | `board_settings::BoardSettingsState` | `board_id`, `name`, `prefix`, `default_repo_id`, `start_on_worktree`, `push_new_cards`, `conflict_policy`, `backend_kind`, `original_kind`, `original_settings`, `rows: Vec<BackendRow>`, plus `row`, `caret` and `error` |

`DialogHost.behind_palette` names the dialog the open palette replaced — the palette does not
stack on a dialog, and a `Card detail:` palette row reopens that dialog instead of reseeding it
over the text the user already typed. `:` is therefore bound in `Dialog > CardDetail` as well as
in `Hub`: without a way in from the detail, `Card detail: Close` and `Card detail: Save text edit`
are rows no state could ever list and the whole `behind_palette` path is unreachable. The added fields are all local editing state; the BOARD §8
fields keep their names and meanings. `CardDetailState` holds **one** buffer for the three text surfaces
(title, description, comment), because at most one of them is ever open.
`Dialogs::CardDetail.width()` is 880 px — it is a two-pane surface, not a form — and
the other three board dialogs are 560 px.

`card_picker::PickerKind` is `Status | Priority | Assignee | Labels | Estimate |
DueDate | Repo | Property(String)`. Set `host.card_picker.kind` before opening
`CardPicker`; seeding preserves it and resets the selection/query for the target card.
Opening a picker from detail carries its card ID, independently of the board cursor.
Applying or cancelling returns to the existing detail draft without reseeding it.
Other dialog openings seed fresh drafts.

Every BOARD §8 command has a palette `Command` variant, label and action dispatch.
The exact action names are listed in `KEYMAP.md`; the namespaces are `board` and
`card_detail`. `Shell::with_actions` registers every action. Board handlers call
the corresponding snake_case free function in `screens::board`; detail handlers
call the matching free function in `dialogs::card_detail`. Every one of them is
implemented; a dialog's own `on_action` handler shadows the shell's, because gpui
stops action propagation by default in the bubble phase.

Three additions outside the skeleton's list:

* `actions::board::CreateAndOpen` (`ctrl-enter` in `Dialog > CardCreate`) creates the
  card and opens its detail. It has no palette command: it only means anything inside
  that dialog.
* `Dialog > CardPicker` binds `space` to `settings::Toggle` (multi-select) and
  `Dialog > BoardSettings` binds `j`/`k`/`h`/`l`/`space` to the `settings::*` actions,
  reusing §3.8.6's row model — including its rule that a bare letter types when a text
  row owns the keyboard. Everything else these dialogs answer is inherited from the
  generic `Dialog` context.
* `ConfirmRequest::DeleteCard { card, key, title }` routes `d` on the board through
  §3.8.3, like every other destructive key. A card with a `remote` link never reaches the
  dialog: the daemon refuses that deletion (the sync would file the issue again as a new
  card), so `d` answers "Mirrored card — delete it in the backend" instead of confirming a
  consequence the system does not deliver.

Property mutations go through `screens::board::send_card`, which applies the returned
`Card` with `apply_card`, moves the focus onto it, and puts a refusal in the sticky
error slot. Nothing on the board is optimistic.

### The backend registry, generically (BOARD-JIRA §6)

`fleet-app` names no backend. `AppState` caches the daemon's registry in
`board_backends: Vec<BackendDescriptor>`, fetched **once per connection** by
`screens::board::ensure_backends` — `begin_backends_load()` marks the request as issued when
it goes out, not when it answers, because the board re-renders every frame and a flag cleared
by a failure would ask sixty times a second. `clear_board()` clears the flag (a reconnect may
land on a different fleetd) but keeps the descriptors, so the header's label never flickers.
`backend_label(kind)` falls back to the raw kind, which is what an older daemon leaves behind.

* **Header** — `views::board_screen::HeaderFacts::of(view, backend_label, now)` puts the label
  in the backend chip; `BoardProps.backend_label` carries it in.
* **Settings dialog** — the `Backend` row cycles `backend_kinds(state, current)` (the registry,
  plus the board's own kind when the registry does not know it), and every row below it is one
  `settings_schema` entry of the selected descriptor. The row model is pure and unit tested:
  `backend_rows(schema, settings)` → `Vec<BackendRow>`, `rows_to_settings(base, rows)` →
  settings JSON (keeping keys the schema never names, removing the ones a row emptied, writing
  numbers as numbers), and `rows_error(rows)` for the required and numeric rules.
  `PropertyKind` picks the control: `Bool` → `Toggle`, `Select` → `Cycler` over the schema's
  options, `Number` → `NumberField` that takes digits only (so `h`/`l` keep stepping it),
  everything else → `TextField`; `MultiSelect` is typed comma-separated. `PropertySchema` has
  no `required` flag, so a name ending in `fleet_core::board::REQUIRED_MARKER` (`(required)`,
  re-exported as `board_settings::REQUIRED_MARKER`) is the signal; the marker is stripped from
  the label and shown as `∗`. It lives in the core because the daemon reads it the same way: a
  required row names the remote itself, and `BoardService::update` refuses to change one on a
  board whose cards are already linked, exactly as it refuses a kind change.
  `h` on an empty `Number` row is a no-op — stepping down from unset would write `0`, a value the
  daemon is not using and that `backend_element` refuses to draw. Save sends
  `BoardPatch { backend: Some(BackendRef { kind, settings }), … }`; changing kind starts from
  empty settings and returning to the board's own kind restores them, mirroring the daemon's
  rule. A refusal keeps the dialog open with the daemon's message verbatim.
* **Read-only fields** — `AppState::readonly_fields()` / `is_readonly_field(field)` read
  `board.sync.readonly_fields`, and are empty on a local board exactly as
  `fleet_core::board::ops` reads them. `PickerKind::card_field()` maps a picker to the field
  name that list uses. `screens::board::readonly_message(state, kind)` builds
  `"<field> is read-only on <backend label> boards"`; the board's pickers and `[` / `]` show it
  as a toast (`Icon::Lock`), and the card detail writes it to its own error line, because the
  dialog's scrim covers the toast stack. `views::board_card_detail::PropertyRow.locked` draws
  the row in `Tone::Secondary` with a trailing lock glyph while keeping its picker target — a
  row that silently did nothing would look like a broken key.
* **`x`** — `board::OpenRemote` and `card_detail::OpenRemote` open `card.remote.url` with
  `cx.open_url`. `screens::board::remote_url(card)` is the single source of "is there an
  address here"; it also gates the two palette rows through `CardContext.remote`.

Detail text saves keep the editor until a matching successful reply. Revision guards prevent
older replies from clearing newer drafts; failures retain text and display the daemon error.
Both detail and CardCreate description editors retain `TextAreaState` across keystrokes.
In CardCreate, Enter in the description inserts a newline, Tab indents, Shift-Tab returns
to the title, and Ctrl-Enter creates and opens the card.
