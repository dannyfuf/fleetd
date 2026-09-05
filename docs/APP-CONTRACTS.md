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
| `screens/workspace.rs` | §3.6 | workspace agent |
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

// screens/workspace.rs — same shape
impl WorkspaceScreen { pub fn new(cx: &mut App) -> Self; pub fn render(/* … */) -> AnyElement; }

// screens/jobs.rs — same shape, rendered into the overlay layer
impl JobsPanel { pub fn new(cx: &mut App) -> Self; pub fn render(/* … */) -> AnyElement; }

// dialogs/mod.rs
pub enum Dialogs { CreateWorktree, CloneRepo, Confirm, NewContext, EditContext,
                   AssignRepo, Settings, Help, Quit, QuitDaemon }
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
| Workspace, PTY tab | `Fleet > Workspace > Terminal` \| `Prefix` \| `Scroll` |
| Workspace, `fleet://` tab | `Fleet > Workspace > Native`, then the embedded view's own chain (`> Lazygit > Panels > Files`, …) |
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
| `screen`, `hub_pane`, `pr_tab`, `scope`, `cursors` | where the cursor is, per list |
| `terminal_mode`, `overlay`, `mode()` | the mode word and the key context |
| `filter` | query + whether the input still owns the keyboard |
| `session_mru`, `terminal_mru` | `ctrl-s w` and `ctrl-s Tab` are `Mru::alternate()` |
| `toasts`, `sticky_error` | §2.7 and §1.8; errors are sticky, never toasts |
| `daemon: DaemonLink` | §3.12; `refuses_mutations()` and `drops_terminal_keys()` are the two questions a screen asks |

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
`WatchStatus::{Running, Exited { code, signal }}`, `WatchStream::{Stdout, Stderr}`,
and immutable `WatchChunk { seq, stream, text }`. Watch metadata includes parent
session/terminal, label, argv, optional cwd/PID, and RFC3339 `startedAt`.
Watches are runtime-only and do not survive a daemon restart.

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
clear prior IDs before rebuilding from ListWatches.

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
ID after a daemon restart. A connection lease owns each StartWatch; dropping its socket marks unfinished
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
No wire types or daemon lifecycle behavior changed for the pane.
