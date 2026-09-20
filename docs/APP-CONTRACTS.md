# `fleet-app` — internal contracts

**Status: authoritative for the app's internal seams.** `docs/UX-SPEC.md` decides what a screen
shows, `docs/KEYMAP.md` decides which key does it, `docs/DESIGN-SYSTEM.md` + `fleet-ui-kit`
decide what it is drawn with. This document decides **how the parts of `fleet-app` plug into
each other**: what the shell owns, what a screen owns, and what crosses between them.

These seams are stable on purpose: changing a signature here changes every screen, so change it
deliberately and update this document in the same pass.

The daemon wire protocol is version 8. Hello includes a defaultable `HelloClient` (`app`, `cli`,
or `proxy`, plus an optional forwarding host id); its response envelope includes a stable daemon
id, optional build commit, and capabilities including `remote-machines`. Host snapshots include
provider, remote version, link state, resolved address, and optional agent-binary availability.
Remote link and terminal-reattach changes arrive as additive events. Worktree paths carry an
optional owning host, and PR worktree creation carries optional placement.

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
| `screens/board.rs` | BOARD §8: the board tab, its loading states and action extension points |
| `screens/workspace/` | §3.6 |
| `screens/agent_thread/` | §3.6.0, one native agent thread: transcript rows, decisions, the docked composer, the completion pickers |
| `screens/agent_popup/` | §3.6.1, the floating agent PTY surface and its terminal attachment |
| `screens/jobs/` | §3.7 |
| `dialogs/` | §3.8–§3.10, plus the `ActiveDialog` entity the shell mounts |
| `terminal/` | the painted cell grid, its geometry, selection and prepared presentation |
| `views/` | domain views composed from the kit: rows, lists, the detail panel, the watch pane |
| `watches/` | the read-only subagent output mirror and its catch-up cursors |

Each of those roots holds the type and its composition only. Preparation, lifecycle, actions and
tests live in sibling modules — `screens/hub/{projection,cache,navigation,actions,composition}`,
`screens/workspace/{model,lifecycle,terminal,native,agent,chrome,actions}` and so on.
The agent thread follows the same split:
`screens/agent_thread/{rows/,decisions,composer,presentation,picker,actions}` prepare,
`state/agents.rs` holds the per-worktree tab order and the client mirror, and the view itself
issues no I/O — it emits `AgentThreadEvent`, which `screens/workspace/agent.rs` relays as
`BridgeCommand`s. Three of those events are **not** fire-and-forget, because their answer is
state the surface reads rather than a mutation to forget: `LoadOlder` prepends a page,
`RefreshCheckpoints` decides whether `[u]` is drawn at all, and `AccountLogin` carries the
sign-in URL the workspace opens in a browser. `rows/` is the flat row projection
(`NATIVE-AGENTS.md` §5): a turn is an
emergent run of rows, never a container, and every row is memoised behind a revision key so a
stream chunk never re-runs grouping, folding or summarization.

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
        board: &mut BoardScreen,          // the shell's one board screen, lent for the frame
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;
}

// screens/board.rs — same frozen constructor/render signature as HubScreen, minus the loan:
// `Shell` owns the one BoardScreen and passes it to whichever surface draws the board this
// frame (the Hub's `Board` tab, or a Workspace's `fleet://board` tab). Its root tracks the
// `focus` it is handed, so the surface's own root must not track it a second time.
impl BoardScreen { pub fn new(cx: &mut App) -> Self; pub fn render(/* … */) -> AnyElement; }

// screens/jobs.rs — same shape, rendered into the overlay layer
impl JobsPanel { pub fn new(cx: &mut App) -> Self; pub fn render(/* … */) -> AnyElement; }

// screens/workspace.rs and screens/agent_popup.rs — two phases
impl WorkspaceScreen {
    /// Attachment, requests, focus and teardown. Called before the frame is composed.
    pub(crate) fn synchronize(&mut self, state: &Entity<AppState>, bridge: &Bridge,
                              window: &mut Window, cx: &mut App);
    /// Composes what `synchronize` prepared. Issues no request, reconciles no resource.
    /// Takes the same lent `board: &mut BoardScreen` first argument as `HubScreen::render`,
    /// for the frames where the session's active tab is the `fleet://board` one.
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
                   EditHooks, Settings, RenameTerminal, Help, Quit, QuitDaemon,
                   CardDetail, CardCreate, CardPicker, BoardSettings }
impl Dialogs {
    pub const fn context_name(&self) -> &'static str;   // the `Dialog > <name>` word
    fn render(/* state, bridge, focus, window, cx */) -> AnyElement;   // called by ActiveDialog
}
```

**Render prepares nothing.** A render function composes already-prepared data. Filesystem access,
request initiation, expensive projection and focus or lifecycle reconciliation belong in
`synchronize`, in an observation, or in a background task — never in `render`. Drafts are entities
released with the window; there is no global draft store.

The harness snapshot obeys this rule like any other projection. `AppState::harness_projection`
builds the versioned `UiSnapshot` that `docs/TESTING-HARNESS.md` §3 freezes, memoised behind a key
naming every input it reads, and its only callers are the harness socket's `dump`, `await` and
`assert` commands in `crates/fleet-app/src/drive.rs`. No render path may reach it, and the
per-element target recorder in `fleet_ui_kit::harness` is one branch on a thread-local flag that is
false in every launch that did not ask for harness mode.

Rules that come with those signatures:

1. **The returned root element must call `.track_focus(focus)`.** That is what puts your
   `on_action` listeners on the key-dispatch path: gpui dispatches an action from the focused
   node upwards, so a listener *below* the focus node never fires. The shell focuses the handle
   it passed you and never touches focus again, except that a native agent tab consumes one
   activation intent after its composer has appeared in a painted frame.
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
* the one-shot prefix: entering it, and leaving it on the very next key — including the `^s`
  chord of a native agent tab, which the shell's keystroke interceptor takes whole;
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
| Workspace, `fleet://board` tab | `Fleet > Workspace > Native > Board` — the board is Fleet-drawn, so the word is Fleet's own and every `Hub > Board` row is repeated on it |
| Workspace, native agent tab | `Fleet > Agent > AgentIdle` \| `AgentWorking` \| `AgentNativeScroll`, or `Fleet > Agent > AgentDecision > AgentPermission` \| `AgentQuestion` \| `AgentPlan` while a gate is open |
| Floating agent terminal | `Fleet > Agent > Terminal` \| `Prefix` \| `Scroll` |
| Filter / Palette / Jobs | `Fleet > Filter` (`> BoardFilter` on the board) \| `Palette` \| `Jobs`, then the focused editor's `FleetTextInput` context for the first two |
| Dialog browsing | `Fleet > Dialog > CardDetail` \| `BoardSettings` \| `CardPicker` \| `Settings` \| `Create` (or the dialog's other stable name) |
| Dialog text editing | `Fleet > Dialog > CardDetailEditing` \| `BoardSettingsEditing` \| `SettingsEditing` \| `CreateEditing`, then the focused component's `FleetTextInput` context |
| Daemon banner showing (§3.12 C) | the base chain **plus** `Daemon > Banner`, innermost — unless a live editor owns the keyboard on that base chain, when the word is absent |
| fleetd will not start (§3.12 B) | `Fleet > Daemon > Down` |
| First run (§3.13) | `Fleet > FirstRun` |

Two consequences worth knowing:

* A deeper context wins, so `Hub > Prs`'s `l` (next PR tab) beats `Hub`'s `l` (next pane).
* **`Overlay::Filter` mounts no overlay layer.** §3.10 replaces the pane header in place, so the
  Hub's filter editor is part of the Hub body and the `Filter` key context wraps that body
  rather than a layer above it — the arrangement the board filter already had. The shell's
  focus reconciliation hands the keyboard to that editor, and the container keys the editor does
  not own (`ctrl-n` / `ctrl-p`, `Enter`) are listeners on the Hub body; `Esc` is the shell's,
  and it names `body_focus` explicitly because the editor is *inside* that handle's subtree.
  "Is that editor mounted" and "does it own the keyboard" are the same question, so the Hub's
  composition and the shell's focus reconciliation both call `AppState::hub_filter_owns_keys()`
  rather than each testing their own fields; two predicates that drift apart mount an editor
  nothing focuses, or focus one nothing mounted, and either way the keyboard dies.
* **Except for the card picker, a browsing word never remains in the chain while that dialog is
  editing text.** Bare-letter and caret-collision rows live only on `CardDetail`, `BoardSettings`,
  `Settings` and `Create`; their `*Editing` partners carry only container actions such as confirm, cancel,
  field/list navigation, save and palette entry. `DialogHost` owns the real draft predicate and
  mirrors only its derived word into `AppState`, so `context_chain()` remains authoritative:
  card detail uses `edit.is_some()`, board settings uses whether its focused row has a text
  buffer, settings uses `editing.is_some()`, and create-worktree uses `field == Branch`. A
  dialog with more than one editor also names the one that owns the keyboard — new card,
  new/edit context, repository hooks — and that marker is a **mirror** of focus, not a second
  opinion about it: each such dialog subscribes to `TextInputEvent::Focused` and writes the
  field a click landed on, because a pointer focuses an editor without asking the dialog and the
  next reconciliation would otherwise pull the caret back. A live
  input then appends `FleetTextInput`; its
  `mode` attribute is `single_line` or `multiline`, and only the latter satisfies
  `FleetTextInput && mode == multiline` for `Enter`. `CardPicker` is the documented exception: its
  query is a filter that never contains a space, so it keeps the browsing word and `space` toggles
  the highlighted card. The rule is now complete: the `Dialog` container binds no editing key of
  its own, so there is no legacy editor row left for a dialog to fall back on.
* Inside a native agent tab, `^s` never reaches gpui's two-key matcher. The shell's keystroke
  interceptor consumes it, resolves the second key against the *live* chain through
  `keymap::chord_action_for_chain`, and consumes that key too — running its row, or toasting
  `^s <key> is not bound here`. gpui replays the keystrokes of a sequence that matched nothing as
  *input*, which over a text composer typed a stray character; the rows stay in `keymap::table()`
  because that is what the Help overlay and this contract are read from.
* gpui's `>` is a **subsequence** test over the rendered chain, not a parent test. An embedded
  view therefore may not reuse any context word this table uses: `crates/fleet-lazygit` renames
  its overlay words to `LgDialog` / `LgConfirm` / `LgHelp` for exactly that reason, and the
  `the_embedded_pane_shares_no_context_word_with_the_app` test in `keymap.rs` keeps it true.
  The same test file checks that no gpui action name is registered twice: actions are
  process-wide (`namespace::Name` via `inventory`) and `App::load_actions` panics on a
  duplicate, so the pane's colliding namespaces are `lg_confirm` and `lg_help`.
* While the §3.12 C banner is undismissed it is the **innermost** context, so `r`, `l` and
  `Esc` belong to it, exactly as `KEYMAP.md` says. `Esc` dismisses the banner and hands those
  keys straight back. The banner is a container with two bare letters, so the ownership rule
  above applies to it as it does to a dialog's browsing word: `context_chain()` does not append
  `Daemon > Banner` while a live editor owns the keyboard on that base chain, which today means
  §3.10's board filter and §12's agent composer — every other editor sits under an overlay that
  publishes its own chain and never reaches the append. The whole word leaves, `Esc` with it;
  the banner stays dismissible from every surface that is not typing.

The floating agent is persistent state beside the ordinary `overlay` slot, not another member of
that mutually-exclusive enum. When it is topmost, `Agent` owns focus and the Hub/Workspace remains
mounted underneath. Help and `Quit` / `QuitDaemon` may occupy the one ordinary overlay slot above
it; closing that dialog returns focus to `Agent`. Palette, Settings, Filter, and Jobs have no
binding in `Agent`, so they cannot create a second competing topmost surface. `ctrl-q` resolves to
`agent::Hide` in this context; `ctrl-shift-q` remains the global stop-confirm action. The binding
lives on the popup's own children (`Agent > Terminal`, `Agent > Prefix`), never on `Agent`
itself: gpui matches `>` as a subsequence, so a binding on the root would also match a native
agent tab's `Agent > AgentIdle` — where the popup is not mounted, nothing handles the action and
the global quit would be shadowed by a dead key.

The native agent tab reuses the same `Agent` root word but is **not** the popup: it is the
Workspace's selected tab, so the Workspace stays mounted and only the chain's second word
changes. `AppState::agent_context_chain()` derives it from daemon state, not from the view —
the thread's newest open gate picks `AgentDecision > AgentPermission` \| `AgentQuestion` \|
`AgentPlan`, and otherwise a running session, a running turn or live background work picks
`AgentWorking` over `AgentIdle`; a frozen transcript tail (`ctrl-s [`) takes precedence over all
of them and picks `AgentNativeScroll`, which is also what the status bar's `SCROLL` word is read
from. Deriving it from the projection is what makes the card own the
keyboard in the *same frame* the gate appears, instead of one frame later. `Agent > AgentRow`
is bound, listed in Help and handled by the workspace (`ExpandRow`, `Revert`, `OpenInEditor`
act on the newest expandable item), but nothing calls `TranscriptList::focus_row` yet, so no
chain contains that word and those three keys cannot fire — row focus is the follow-up in
`NATIVE-AGENTS.md` §10.

Agent-tab activation records one composer-focus intent in `AgentThreads`. The Workspace consumes
it once and uses `Window::on_next_frame` so a newly constructed `AgentThreadView` has mounted its
input before `focus_composer` runs. Re-selecting an existing tab follows the same path; scroll mode
and a decision-owned keyboard veto the request. Already-mounted tabs may restore their composer
immediately after a popup or overlay releases focus, but ordinary synchronization never schedules
another deferred focus, so transcript reading is not pulled back to the composer.

The floating popup mounts its focus-tracked overlay shell before `Model::build` can resolve a
daemon session. The attaching shell keeps the `Agent > Terminal` / `Agent > Prefix` action path
live, including hide and provider switching, while drawing `attaching…`; model readiness changes
the content of that shell, not whether the keyboard can reach it.

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
    if let Ok(Ok(ResponseBody::Path { path, host })) = reply.recv().await {
        state.update(cx, |state, cx| { /* … */ cx.notify(); });
    }
})
.detach();
```

`BridgeEvent::EffectiveConfig` hydrates the app-facing terminal and notification settings,
`jobs.warnBeforeQuit`, and the PR-cache TTL on every connection/reconnection and config response.
Consumers use those effective values rather than reconstructing persisted defaults.

PTY agent activity and attention are separate contracts. Output-byte heuristics update only
`AgentActivity::{Working, Idle}` and its glyph; heuristic idle never creates a toast or sound.
`fleet agent-status` supplies the optional shared `AttentionKind` on
`SetAgentActivity`, `AgentActivityChanged`, `Terminal.agent_attention`, and
`WorktreeWindowStatus.agent_attention`. The app
patches or snapshots that same field, shows the terminal tab's existing amber `attention` mark,
and notifies once per session edge into a new `Some(kind)`. Connection seeding establishes a
baseline without replaying historical attention. Explicit `working`, terminal input, terminal
close, or real non-echo output clears terminal attention.

Terminals attach with plain requests (`AttachTerminal`, `ResizeTerminal`, `TerminalKey`,
`ScrollTerminal`, `DetachTerminal`); frames arrive as ordinary events and the shell has already
applied them to `AppState::grids` before your screen renders. `fleet-client` re-attaches every
terminal after a reconnect on its own. Daemon and host share one absolute five-second attach
deadline. App surfaces await the correlated acknowledgement with a bounded deadline; a refusal or
timeout clears their optimistic attachment and generation, reports a sticky error naming the
terminal, and schedules reconciliation again; the next successful attach of that same terminal,
by the attempt that still owns the surface, clears that error. A request already expired when the owner services it cannot resize the PTY,
and no post-deadline result can publish an attachment frame or claim an attachment. Before the
first valid frame, each app surface retains an ordered prefix of at most
1,024 key/paste events and 1 MiB; it rejects the newest input after either bound and flushes the
retained prefix only to the same live terminal.

**Never block the foreground thread on the daemon.** Everything above is non-blocking by
construction; there is no synchronous path and there must not be one.

### Native agent threads

Agent traffic does not go through `RequestBody` at the call site. `BridgeCommand` mirrors the
eleven screen-issued agent requests — `AgentThreadList`, `AgentThreadCreate { worktree, provider, model, mode,
resume_cursor, title }`, `AgentThreadOpen { thread, from_seq }`, `AgentThreadClose`, `AgentSend
{ thread, input }`, `AgentInterrupt`, `AgentRespond { thread, gate, answer }`, `AgentSetMode`,
`AgentSetModel`, `AgentMarkSeen { thread, seq }`, `AgentStop` — and converts losslessly with
`From<BridgeCommand> for RequestBody`, so a view can name an intent without depending on the
wire enum:

```rust
bridge.send_agent(BridgeCommand::AgentSend { thread, input });          // fire and forget
let reply = bridge.request_agent(BridgeCommand::AgentThreadOpen { thread, from_seq });
```

Connection bootstrap has one additional typed read that no screen issues: `AgentSeenCursors`.
When `agent.seen` was negotiated, the bridge fetches that installation's cursor census after
Hello and publishes it as `BridgeEvent::AgentSeenCursors` before the first snapshot. Because
`fleet-client` can reconnect beneath the bridge, the two-second health pass refreshes the census
once when the client's connection generation changes and republishes only when the census changed.
`AgentThreads` is seeded before it re-reports any newer local
overrides. Keeping this read out of shared summary broadcasts preserves O(1) event fan-out.

`AgentThreadView` sends nothing itself. Every mutation leaves it as an `AgentThreadEvent`
(`Command(BridgeCommand)`, `OpenInEditor(String)`, `Notice(SharedString)`), and
`screens/workspace/agent.rs` is the one place that relays those onto the bridge, opens an
editor, or pushes a toast. The view therefore renders purely from state it already holds, which
is the same render discipline §2 imposes on a screen.

Two events arrive back: `BridgeEvent::Agent { thread, event }` carries one `SeqEvent` for a
thread the window has opened, and `BridgeEvent::AgentSummary(summary)` carries the tab and
counter state for every thread, opened or not. The shell has already applied both to
`AppState::agents` before your screen renders.

Applying an agent event also produces `fleet_core::agents::Applied`: `Text { item, stream,
appended }` names the exact UTF-8 byte suffix appended to an existing open item;
`Structural` covers every other change. The client exposes it in `MirrorOutcome::Applied`, and
`AgentThreads` retains the last accepted description per opened thread. A shell batch containing
only `Text` outcomes records scoped agent-text damage and updates only the active
`AgentThreadView`; it neither calls `cx.notify()` on `AppState`, evaluates attention edges, nor
runs `synchronize_surfaces`. Any summary, structural outcome, gap, rejection, or unrelated event
keeps the ordinary global notification path.

### IPC and CLI compatibility

Daemon IPC is version **8**. Rust field names are shown below; serde renders them as camelCase on
the wire. The mandatory first request is `Hello { protocol, client: HelloClient }`, where
`client.kind` is `app`, `cli`, or `proxy`, `client.host_id` optionally identifies the forwarding
daemon, and `client.client_id` is the optional stable per-install UUID stored at
`FleetHome::client_id_path()`. The entire `client` value defaults to
an app client. `HelloResponse` flattens the ordinary correlated `Response` and adds
`capabilities: Vec<String>`, `daemon_id: String`, and optional `build_commit`; a federating daemon
advertises `remote-machines`. Proxy links require protocol lockstep before any request is routed.

`Subscribe { events }` is additive: the daemon unions the named kinds with the ones the
connection already receives, and `Unsubscribe` is the only way to clear the set. `fleet-client`
replays that union on every reconnect, so a connection delivers the same events before and after
one. Every connection subscribed to `DaemonShuttingDown` receives it before its socket closes,
whether the stop came from a `DaemonShutdown` request or from a signal; the connection that
asked reads the outcome from its own `ShuttingDown` response instead. A close with no notice
means the daemon died. The notice is always about the daemon the connection is attached to: a
federating daemon drops a remote peer's `DaemonShuttingDown` at the link boundary instead of
republishing it, because a remote stop is a change to that host's link and reaches the app as
`HostLinkChanged { link: down }`.

`Snapshot.hosts` contains `HostStatus { id, provider, version, link, address, agent_binaries,
reachable, checked_at, error }`. `link` is `connecting`, `ready`, `down`, or `legacy`, and
`agent_binaries` is the optional `{ claude, opencode }` result of host diagnostics. Two additive
events keep the app's remote state live: `HostLinkChanged { host, link, version, error }` changes
availability, and `TerminalReattach { terminal }` tells an attached terminal surface to attach
again after remote-link recovery.

Remote placement is additive to existing requests and responses. `CreateWorktreeFromPr` carries an
optional `host`; `BootstrapHost { host, git_ref }` starts remote installation; and
`ResponseBody::Path { path, host }` returns the owning host for a remote `WorktreePath`. The app
must treat that path as display data: only a path whose host is absent may become a local
`PathBuf` or enter an embedded native-Git operation.

Bulk worktree operations have explicit per-item outcomes so a down host cannot erase successful
local or other-host results:

| Request | Response | Per-item contract |
| --- | --- | --- |
| `DeleteWorktrees { ids }` | `WorktreesDeleted(Vec<WorktreeDeleteResult>)` | `{ worktree_id, ok, reason?, trash_entry? }` for every requested worktree |
| `InspectWorktrees { ids, repo, fetch }` | `Inspections(Vec<WorktreeInspection>)` | one inspection per selected worktree, with its own warnings/error |
| `PruneWorktrees { dry_run, fetch, kill_sessions, repo, ids }` | `Pruned(PruneResult)` | `deleted`/would-delete ids plus `skipped` entries carrying each reason and safety facts |

Mixed-host dismiss, sleep, and kill routing follows the same rule: the router partitions by owner,
runs host parts independently, and preserves an outcome for each requested item instead of failing
the whole request on the first unreachable host.

The version-6 board and native-agent families remain valid in v8, and both `Snapshot.boards` and
`Snapshot.agent_threads` stay `#[serde(default)]`. `PruneWorktrees.ids` also remains defaulted and
omitted when `None`, preserving its legacy request shape; `None` retains repo/all-worktree
discovery, `Some(ids)` is the reviewed allowlist, and an empty explicit list deletes nothing. A
daemon that honors exact IDs advertises `prune.reviewed_ids`; the client refuses a reviewed prune
with update/restart guidance when that capability is absent.

Pong response envelopes likewise have an optional `daemon` object containing `pid` and `bootId`.
`bootId` is stable for one fleetd process and changes across starts, including PID reuse. New
clients retain it while delivering the existing unit `Pong` body to callers; older clients ignore
the additive envelope member. App reconnect identity probes use this Pong metadata and never load
a fallback snapshot. IPC v8 otherwise does not include daemon Git-mutation jobs, arbitrary
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
| `board: BoardState` | active context’s `BoardView`, loading/error, `BoardFocus { column, row }`, filter and optional `GroupBy` |
| `board_stale: bool` | authoritative refresh pending; lives outside the frozen `BoardState` fields |
| `board_backends: Vec<BackendDescriptor>` | the daemon's backend registry, fetched once per connection; the header label and the settings dialog's rows are drawn from it |
| `screen`, `hub_pane`, `pr_tab`, `scope`, `cursors` | where the cursor is, per list |
| `terminal_mode`, `agent_popup`, `overlay`, `mode()` | the base Workspace mode, floating-agent mode, top overlay, and resulting mode word/key context |
| `filter` | query + whether the input still owns the keyboard |
| `session_mru`, `terminal_mru` | `ctrl-s w` and `ctrl-s Tab` are `Mru::alternate()` |
| `toasts`, `sticky_error` | §2.7 and §1.8; errors are sticky, never toasts |
| `agents: AgentThreads` | the native-agent mirror: daemon summaries, opened `ThreadProjection`s, the last reducer `Applied` description per opened thread, the per-worktree selected tab, seen cursors and pending resyncs |
| `last_agent_activity` / `last_agent_attention` | PTY status-only glyph baseline and semantic hook-attention edge baseline; reconnect seeding is silent |
| `watches: Watches` | the read-only subagent mirror and its per-session pane state |
| `daemon: DaemonLink` | §3.12; `refuses_mutations()` and `drops_terminal_keys()` are the two questions a screen asks |

`agent_popup: Option<AgentPopupState>` is screen-independent. Its `Terminal` / `Prefix` / `Scroll`
submodes reuse the existing `TERMINAL` / `^S` / `SCROLL` status words; it does not add a ninth mode
word. An ordinary dialog above it temporarily shows `DIALOG`, then reveals the popup's prior word.

`agents: AgentThreads` is the one place agent state is read from, and everything derived from it
is a pure function so no two surfaces can disagree about a thread:

| Question | Answer |
| --- | --- |
| Which tabs does this worktree have? | `agents.of_worktree(&worktree)`, in daemon snapshot order |
| What mark does a tab carry? | `agents.attention(thread)` → `tab_badge`: spinner (`Working`) · amber dot (`NeedsYou`) · gray dot (`Unread`) · `exited <code>` (`Failed`) · nothing |
| What does the session header say? | the same attention → `header_word`: `working` · `needs you` · `failed` · `idle` |
| What do the context-bar chips count? | `agents.counts()` → `AgentCounts { needs_you, working, failed }`, including the thread on the current tab, each chip zero-suppressed |
| When does a notification fire? | `agents.attention_edges()` — one toast per *edge* into `NeedsYou`/`Failed`, so a thread that stays blocked does not re-notify |
| What has this installation shown? | `agents.seen(thread)`; selecting a tab sends monotonic `AgentMarkSeen`, which is what clears a `NeedsYou(Finished)`. Windows sharing one Fleet home share this cursor; a different installation does not |

The **daemon's** `Attention` is authoritative — it is derived by the same `fleet-core` reducer
every client replays, so a listed thread and an opened one cannot rank differently. The app
applies exactly one local override: the two attentions defined against a seen cursor
(`NeedsYou(Finished)` and `Unread`) drop to `Idle` as soon as this process's cursor reaches
`last_seq`, so a tab the user is reading never keeps an amber dot while the durable daemon echo
is in flight. On connect and reconnect, the `agent.seen` census seeds that override before the
workspace evaluates marks. A `MirrorOutcome::Gap` from `apply_event` marks the thread for resync instead of
applying a hole, and the workspace re-opens it from its last applied `seq`.

A clean daemon shutdown appends a synthetic stopped-session event. It changes lifecycle state but
is not user transcript output, so the store advances only installation cursors already caught up
to the preceding sequence; a reconnect alone cannot manufacture an unread mark.

The mirror projection remains authoritative during streamed text. The active thread view keeps a
presentation copy and, for `Applied::Text`, borrows the mirror, copies only `text[appended]`, and
queues that suffix in its reveal buffer. Structural damage flushes the queue and replaces the
presentation projection wholesale. This is why a text-only batch need not increment any
`UiSnapshot` update-path revision: harness mode has reduced motion enabled and therefore flushes
the same suffixes synchronously, while `await idle` continues to depend on bridge request/settle
counters rather than `AppState` observer notifications.

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

## Board app extension points (BOARD §8)

`HubTab::Board` is selected by `board::GoBoard` (`g b`). `Shell` owns the one
`screens::board::BoardScreen` and lends it for the frame to whichever surface is drawing the
board — `HubScreen::render` on the board tab, `WorkspaceScreen::render_prepared` on a
`fleet://board` tab — which is what keeps one filter editor, one set of column lists and one
projection cache behind the one `BoardState`. The board pane is not an entity and is not keyed
by worktree: the tab is a place to draw the shared mirror, not a thing to build and evict. The
Hub's screen strip includes `Board`, the active context's summary `open_count`, and a
conflict dot when `conflict_count > 0`. `BoardScreen` owns one horizontal
`ScrollHandle` for the columns and one per column for its cards, and reveals the
focused column and card when their selection changes. Only the inner Board root tracks the
body focus handle on either surface — the Hub's screen root and the Workspace's both skip
`track_focus` while the board is drawing, because two dispatch nodes for one focus id is one
node too many. `views::board_screen` holds the pure model
(filter predicate, visible slice of a column, priority and category mappings, header
facts) and the rendering; `views::board_card_detail` holds the property-row model and
the detail panes. Placement and content decisions are in `UX-SPEC.md` § Board.

`BoardState` has `scope: Option<BoardScope>`, `view: Option<BoardView>`, `loading: bool`,
`error: Option<String>`, `focus: BoardFocus`, `filter: String`,
`filter_editing: bool` and `group_secondary: Option<GroupBy>`.
`BoardScope` is `Context(ContextId) | Worktree(WorktreeId)`: one mirror serves both the Hub
tab and the Workspace's board pane, because the two are never visible at once. `None` resolves
to the active context — the Hub's board — and the first load records that resolution.
`filter_editing` records whether the board filter owns text input. It is what
`AppState::board_filter_owns_keys` reads on both surfaces — `Screen::Hub { tab: Board }` or
`AppState::board_pane_is_active()`, which is a Workspace whose active tab is a native terminal
whose command is `fleet://board` — and while it is set `context_chain()`
returns `["Filter", "BoardFilter"]` (and `mode()` returns `Mode::Filter`) instead of
`["Hub", "Board"]` or `["Workspace", "Native", "Board"]`, so the board's bare letters type
instead of firing.
`AppState::board_filter_escape()` is the §3.10 two-stage `Esc` for it; base Cancel
calls its second stage. The focused board `TextInput` adds `FleetTextInput` beneath
`Filter > BoardFilter`; its `Changed` event mirrors text into `BoardState.filter`.
`Tab` / `Shift-Tab` move columns, while left/right and ctrl-b/ctrl-f belong to the deeper input
context and move its caret.
`clamp_board_focus` is `pub(crate)` and clamps against the **filtered** column, so the
selection can never point at a hidden card. `BoardFocus` has `column: usize` and `row: usize`;
`GroupBy` is `Priority | Assignee | Labels`. `AppState::board() -> Option<&BoardView>`
returns the current view. Reducers are `apply_board_view(BoardView)`,
`apply_card(Card)`, `clear_board()`, `enter_context_board_scope() -> bool` and
`enter_worktree_board_scope(WorktreeId, Instant) -> bool`; both scope reducers answer whether
the mirror moved, so an observation that runs on every notify only notifies when it did. Card
upserts reject another board, sort by status and position, and clamp
focus. Unknown-status responses request a full refresh instead of inserting an invisible card;
board views are admitted by the scope alone — a worktree's board must carry that worktree, a
context's board that context and **no** worktree — so the app never derives a board id from
either. Clear resets the draft and
invalidates pending responses; a scope change does the same through that one generation
counter, so a reply from the scope just left can never land. Clear also resets the scope to
`None`, which is the Hub's board: a reconnect, a link change or a context switch takes a
worktree scope with it, and the surface that wanted one enters it again.
`enter_worktree_board_scope` refuses, toasts `WORKTREE_BOARDS_UNSUPPORTED`
(§2.7, 3.2 s, `info`) and leaves the scope and the shown board untouched when the
connected daemon does not advertise `board.worktree`.
`apply_daemon_event(Event, Instant)` handles
`Event::BoardChanged` by setting `board_stale` only for the displayed board.

There are **no new `BridgeEvent` variants**: like PR/worktree response consumers,
`screens::board::ensure_current` awaits the receiver returned by
`Bridge::request`, sending `RequestBody::EnsureBoard { context_id }` or
`RequestBody::EnsureWorktreeBoard { worktree_id }` — whichever the scope names. It applies
`ResponseBody::Board` via `finish_board_load(&BoardScope, generation, result)` and
`apply_board_view`. Card
request consumers apply `ResponseBody::Card` through `apply_card`.
The board loader runs on tab entry, active-context change, reconnect, a stale
board's next render, a Workspace board-tab activation, and a Workspace session change while
that tab is active; `screens::board::{enter_context_scope, enter_worktree_scope}` are the two
triggers that point the mirror and load it, the second answering `false` when the daemon
refuses.

`WorkspaceScreen::sync_board_scope` is where the pane's half of that runs, from
`WorkspaceScreen::synchronize` and therefore on the update path, never from a paint. It enters
the worktree scope whenever the session's active tab is the `fleet://board` one — whichever key
or click selected it — and `release_board_scope` hands the mirror back to
`enter_context_scope` as soon as it is not, including when the Workspace itself goes away, so
the Hub never inherits a worktree scope. Both are idempotent against a claim the screen keeps:
`BoardClaim::Drawing { worktree, generation }` records `AppState::board_generation()` alongside
the worktree, so a `clear_board` makes the claim stale and the scope is entered again, while a
daemon that refused once is not asked again on every notify. `BoardClaim::Requested` is the
other half: `ctrl-s b` points the mirror before the tab exists so its load is in flight by the
time the pane first paints, and the frames until fleetd lists and selects that tab still show
the previous one — releasing there would cancel the load the keystroke started. Selecting any
other tab drops the pending claim, as does leaving the Workspace.

`prefix::OpenBoard` (`ctrl-s b`, `Workspace > Prefix`) is the Workspace's way in: on a
session with no worktree it toasts `boards belong to worktrees` and stops, otherwise it enters
the worktree scope and then selects the session's `fleet://board` terminal — or asks for one
with `NewTerminal { name: "board", command: "fleet://board", cwd }` and selects the reply, the
same path `ctrl-s c` takes. The tab is created on demand and never written to `windows[]`, so
pressing the key twice is one tab, selected twice. Its listener is the shell root's, because the
palette's `Workspace: Open board tab` row dispatches the same action from a sibling branch of
the element tree.
Only one request is in flight per generation. Context switches
(including A → B → A), scope switches and link changes reject old responses. Errors remain
visible in state until reload; an event arriving during a refresh schedules one more load.

The payload-free `Dialogs` variants and `context_name()` values are `CardDetail`,
`CardCreate`, `CardPicker`, and `BoardSettings`. Their titles are `Card detail`,
`New card`, `Card property`, and `Board settings`. Each module exposes `render`
with the ordinary dialog signature; each returned root tracks focus and closes on
Escape. `DialogHost` owns these public fields:

| Field | Type | Initial draft |
| --- | --- | --- |
| `card_detail` + `card_detail_input` | `card_detail::CardDetailState` + `Option<Entity<TextInput>>` | `card_id`, `property_row`, `edit: Option<CardEdit>`, revision, saving and error; one input is created at edit start and dropped at save/cancel |
| `card_create` + `card_create_title` / `card_create_description` | `card_create::CardCreateState` + two `Option<Entity<TextInput>>` fields | board id, focused field, save generation and error; submit reads both live inputs |
| `card_picker` + `card_picker_input` | `card_picker::CardPickerState` + `Option<Entity<TextInput>>` | kind, card id, row cursor, selected values, return flags and error; `Changed` prepares filtered rows |
| `board_settings` + `board_settings_input` | `board_settings::BoardSettingsState` + `Option<Entity<TextInput>>` | serializable board values, backend rows, focused row and error; a text-row input is materialized on focus and mirrors through `Changed` |

The §3.8 dialogs own their editors the same way. Each is created by that dialog's `seed` and
dropped by `close_with`, and every one of them is reported by `dialogs::focused_input`, so the
shell's focus reconciliation hands the keyboard to whichever editor the draft says owns it:

| Field | Type | What the editor owns |
| --- | --- | --- |
| `create` + `create_branch` | `create_worktree::CreateState` + `Option<Entity<TextInput>>` | the branch text; `CreateState.branch` is its `String` mirror, and `Changed` republishes the validation message and the worktree-id preview through `set_invalid` / `set_preview`. It owns the keyboard only while `field == Branch`, which is what leaves `←` / `→` to the host cycler while browsing |
| `clone` + `clone_query` | `clone_repo::CloneState` + `Option<Entity<TextInput>>` | the search query; `Changed` mirrors it into `CloneState.query` and re-arms the 150 ms debounce, and the leading glyph swaps between `search` and `loader-circle` in the same update paths |
| `context` + `context_name` / `context_owners` | `context::ContextState` + two `Option<Entity<TextInput>>` fields | the display name and the comma-separated owners; `Changed` mirrors both and republishes the collision message or the id preview on the name editor |
| `edit_hooks` + `hook_inputs` | `edit_hooks::EditHooksState` + `Vec<Entity<TextInput>>` | one editor per command row, prepare commands first and post-create after them, split by `prepare_len`. Each list always ends in a blank row; typing into that row appends the next one and renumbers the labels below it |
| `rename_terminal` + `rename_input` | `rename_terminal::RenameState` + `Option<Entity<TextInput>>` | the terminal name; the draft keeps only the target terminal, the refusal and the in-flight flag |
| `settings` + `settings_input` | `settings::SettingsState` + `Option<Entity<TextInput>>` | the row `Enter` opened; `SettingsState.editing` is its `String` mirror and the `SettingsEditing` predicate, `Changed` commits through `commit_value`, and a number row filters to ASCII digits |

`DialogHost.palette + palette_input` is the §3.9 query: `PaletteState.query` is a `String`
mirrored from a live single-line `TextInput` created when the palette opens and dropped with the
rest of the drafts when it closes, and its `Changed` event re-ranks the `GO` / `DO` / `CONTEXT`
rows and returns the flat cursor to the top. `dialogs::focused_input` reports it, so the shell's
focus reconciliation treats the palette exactly like a migrated dialog.

`DialogHost.behind_palette` names the dialog the open palette replaced — the palette does not
stack on a dialog, and a `Card detail:` palette row reopens that dialog instead of reseeding it
over the text the user already typed. `:` is therefore bound in `Dialog > CardDetail` as well as
in `Hub`: without a way in from the detail, `Card detail: Close` and `Card detail: Save text edit`
are rows no state could ever list and the whole `behind_palette` path is unreachable. The added fields are all local editing state; the BOARD §8
fields keep their names and meanings. `DialogHost.card_detail_input` is the **one** live editor
used by the three text surfaces (title, description, comment), because at most one is open.
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
* `Dialog > CardPicker` always binds `space` to `settings::Toggle` (multi-select); its
  always-focused query is a filter that intentionally never contains a space. Browsing
  `Dialog > BoardSettings` binds `j`/`k`/`h`/`l`/`space` to the `settings::*` actions, and a
  focused text or number row publishes `Dialog > BoardSettingsEditing`. Everything else these
  dialogs answer is inherited from the generic `Dialog` context.
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
  `PropertyKind` picks the browsing control: `Bool` → `Toggle`, `Select` → `Cycler` over the
  schema's options, `Number` → `NumberField`, everything else → a read-only `FactRow` (an empty
  value reads `—`, never a blank box). A focused free-text or number row materializes a
  single-line `TextInput` (numbers filter to ASCII digits), and `MultiSelect` is typed
  comma-separated. `PropertySchema` has
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
`DialogHost` retains the detail editor and both CardCreate inputs across keystrokes; none of
those dialogs owns a parallel string/caret buffer. In CardCreate, Enter in the description
inserts a newline, Tab and Shift-Tab switch fields, and Ctrl-Enter creates and opens the card.
In a detail description or comment, Tab inserts a hard tab through `TextInput::insert`.
