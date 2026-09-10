---
name: gpui-app-shell
description: The app-shell layer of fleetd — actions, key table and key contexts, focus handles and restoration, dialogs and the palette, list navigation, toasts and the sticky error, window and menu setup. Load it before touching crates/fleet-app/src/{actions.rs,keymap.rs}, anything under dialogs/ or shell/root/, docs/KEYMAP.md, or whenever you add a keybinding, a dialog, a fuzzy list, a toast, an overlay, a focus handle, or a second window. It carries Zed's ManagedView/PickerDelegate/ModalLayer designs adapted to fleetd's single-Entity<AppState> architecture, and the incremental Dialog-trait and Picker-delegate extractions the audit asks for.
---

# App shell: actions, keymap, focus, dialogs, pickers, feedback

fleetd is a keyboard-first app: almost every interaction is an action resolved through a key
context, run by a listener on a focus-tracking element, drawn by a dialog or a list. This skill
is the contract for that layer. Patterns are verified against Zed v1.18.1, the GPUI tag fleetd
depends on (`Cargo.toml`) — but fleetd depends only on `gpui` + `gpui_platform`, never on Zed's
`workspace`, `ui`, `picker`, `menu` or `fuzzy`, so every Zed pattern here is a *design to
reimplement*, not a crate to import.

Authoritative docs: `docs/KEYMAP.md` (source of truth for keys — "where the two disagree, this
file wins", `:12-13`), `docs/APP-CONTRACTS.md` §2-§6 (render/focus/action contracts),
`docs/UX-SPEC.md` §2.7 (toast law) and §3.8 (dialog frame). Code and doc move in one commit.

## When to use

- Adding, renaming or removing an action, a keybinding, or a key context.
- Adding a dialog, an overlay, or a palette command; changing dismissal behaviour.
- Adding a list that filters as you type, or making an existing one rank/score results.
- Anything touching `FocusHandle`, `.track_focus`, `key_context`, or `shell/root/focus.rs`.
- Adding a toast, a sticky error, an OS menu item, or a second window; reviewing a diff under
  `crates/fleet-app/src/{actions.rs,keymap.rs,dialogs,shell}`.

## When not to

- Component work inside `fleet-ui-kit` with no action/focus surface → `gpui-components`.
- Entity/subscription/Task lifetime questions → `gpui-state-and-memory`; tokens → `gpui-styling`.

## Rules

1. **Register new subsystems in the bootstrap init list, not in `Shell::new`.**
   `crates/fleet-app/src/shell/root/bootstrap.rs:43-89` is one flat, ordered block:
   `application().with_assets(KitAssets)` → `Theme::init` → `keymap::init(cx)` →
   `fleet_lazygit::keymap::init(cx)` → `cx.set_menus` → `cx.on_window_closed` → `open_window` →
   `window.activate_window()` + `cx.activate(true)`. Ordering constraints belong there, where
   they are readable — theme before the first `cx.theme()` read is already one. Zed does the
   same with 90 `pub fn init(cx: &mut App)` (`zed/crates/theme_selector/src/theme_selector.rs:31`).

2. **One action per row of `docs/KEYMAP.md`, declared with `actions!` and a `///` comment
   naming its keystroke** — the rule is written at `crates/fleet-app/src/actions.rs:3`, and the
   file already holds 25 namespaces with a doc comment on each; GPUI shows them to users. Use
   `#[derive(Action)]` only for a data-carrying action (`impl_actions!` does not exist at v1.18.1). A
   duplicate `namespace::Name` panics at `App` creation; `keymap.rs`'s tests guard it.

3. **Keep `actions.rs` dependency-free** — it imports `gpui::actions` and nothing from the app,
   which lets `keymap.rs`, the shell, screens and dialogs share a verb without a cycle. Zed's
   `zed_actions` (994 lines, 45 dependents) and 37-line `menu` crate exist for exactly this;
   `fleet-lazygit` follows it with non-colliding namespaces (`docs/APP-CONTRACTS.md:174-182`).

4. **Define list navigation once, not per surface.** The same verbs are re-declared per namespace
   today — `palette::{CursorDown,CursorUp}` (`actions.rs:475-479`), plus cursor actions in
   `card_picker`, `create_worktree`, `clone_repo`, `filter` and `scroll` (`:394`). Zed's whole
   `menu` crate is eleven actions (`zed/crates/menu/src/menu.rs:12-37`) bound once and honoured by
   every list. New surfaces use a shared `actions::nav` vocabulary (Core pattern 3); do not
   migrate existing ones opportunistically.

5. **Handlers live on the element that tracks focus, and say how they propagate.** `.on_action`
   fires only on the dispatch path from the focused node upward, and stops at the first bubble
   listener (`docs/APP-CONTRACTS.md:107-112`, `:449-455`). fleetd has 349 `.on_action` and
   **zero** `cx.on_action` in production — keep it that way; `cx.on_action` fires with nothing
   focused, so two surfaces with the same verb fight (Zed reserves it for quit/hide/about). A
   partial handler must end in `cx.propagate()` — "the failure is silent: the key is consumed and
   nothing else happens" (`:456-464`) — and a consuming one calls `cx.stop_propagation()`.

6. **Every binding is a `key_table!` row with an explicit, globally unique context.** Add rows to
   `crates/fleet-app/src/keymap.rs:142-605`; never call `KeyBinding::new` (it `.unwrap()`s the
   predicate parse) — fleetd already uses `KeyBindingContextPredicate::parse` +
   `KeyBinding::load(.., &DummyKeyboardMapper)` and logs instead of panicking (`:66-68`,
   `:121-136`). A binding with **no** context is treated as the *deepest* context and shadows
   everything (`zed/crates/gpui/src/keymap.rs:154-156`), and gpui's `>` is a subsequence test,
   not a parent test, so no context word may be reused (`docs/APP-CONTRACTS.md:174-182`).

7. **Change `docs/KEYMAP.md` in the same commit as a keymap row, and record deviations.**
   `keymap::table()` feeds both `cx.bind_keys` and the Help overlay and palette key hints, so
   keys and their documentation cannot drift (`docs/APP-CONTRACTS.md:437-443`) — fleetd is ahead
   of Zed here, which splits JSON keymap from generated docs. Intentional exceptions are written
   down (`docs/APP-CONTRACTS.md:225-236`, `keymap.rs:28-31`), never left silent.

8. **Every screen and dialog root calls `.track_focus(focus)`.** `dialogs::root(focus)`
   (`crates/fleet-app/src/dialogs/mod.rs:197-199`) exists so a dialog cannot forget. The shell
   owns the three region handles (`body_focus`, `overlay_focus`, `agent_focus`,
   `shell/root.rs:50-56`) and "focuses the handle it passed you and never touches focus again"
   (`docs/APP-CONTRACTS.md:109-112`). A view-owned `FocusHandle` is warranted only for a dialog
   with more than one independently focusable input.

9. **Never grant focus during `render`** — the handle is not in the focus tree until the element
   exists. Use `cx.defer_in(window, ..)` / `cx.focus_self(window)`, as Zed's `ModalLayer` does
   (`zed/crates/workspace/src/modal_layer.rs:199-201`). fleetd focuses from
   `shell/root/focus.rs:414-431`, guarded by `if !wanted.is_focused(window)`.

10. **A dialog closes through one owned path that can veto, never by mutating `AppState` at the
    call site.** Zed's contract is `ManagedView = Focusable + EventEmitter<DismissEvent> + Render`
    (`zed/crates/gpui/src/window.rs:711-715`) plus an `on_before_dismiss -> DismissDecision` veto
    (`zed/crates/workspace/src/modal_layer.rs:49-56`), so a modal mid-operation refuses `Esc`
    without unbinding it. fleetd has 0 `DismissEvent` and scatters `app.close_overlay()` across
    dialog modules; new code calls one `dialogs::dismiss(state, cx)` instead.

11. **Filter and rank off the main thread, apply from a `Task`.** `candidates()` runs
    synchronously in the notify path today (`dialogs/host.rs:129-133` → `palette::refresh_query`,
    `dialogs/palette.rs:1093`) and `FuzzyQuery::matches` (`presentation/keys.rs:17-24`) is an
    unscored subsequence walk. Any *new* list matches in `cx.background_executor().spawn` and
    returns a retained `Task<()>` — the shape of `PickerDelegate::update_matches`
    (`zed/crates/picker/src/picker.rs:226-232`). "Render prepares nothing" (APP-CONTRACTS:101).

12. **Never index a row list by the cursor.** Clamp against the row count and return `Option`,
    as Zed's picker does with `match_count()` + `render_match(ix, ..) -> Option<Self::ListItem>`.
    `dialogs::step` (`dialogs/mod.rs:30`) and `FuzzyList::next_cursor`/`prev_cursor`
    (`crates/fleet-ui-kit/src/components/fuzzy_list.rs:5-7`) are the existing clamped helpers.

13. **Dedupe transient feedback by identity, not by rendered text; errors are never toasts.**
    fleetd coalesces on identical *text* within 1 s (`state/notifications.rs:22-30`), which any
    interpolated id defeats; Zed keys on `NotificationId::{unique::<T>, composite::<T>, named}`
    (`zed/crates/workspace/src/notifications.rs:41-46`, dismissed before pushing at `:157`).
    `docs/UX-SPEC.md:221-241` is stricter and stays: a toast only when no row or pill already
    shows the outcome, and every error goes to the sticky slot (`shell/root.rs:36-43`).

14. **No fallible task ends in a bare `.detach()` or `let _ =`.** fleetd has 33 `.detach()` in
    `fleet-app` and 0 `detach_and_log_err`; route errors to the sticky-error slot with Zed's
    extension-trait shape (`DetachAndPromptErr`, `zed/crates/workspace/src/notifications.rs:1633`).
    Do **not** adopt `window.prompt` — `docs/UX-SPEC.md:965-967` forbids an OK/Cancel pair
    anywhere, so confirmations stay in-app dialogs.

15. **Do not restructure `shell/root/focus.rs`'s `FocusOwnerKeys`; make new code coexist with it.**
    The generation gate, 64-entry FIFO and `usize::MAX`-priority pointer gate (`focus.rs:14-97`,
    `:288-289`, `shell/root/actions.rs:224-236`) are bespoke, documented
    (`docs/APP-CONTRACTS.md:210-222`), and the highest-risk machinery in the app. Coexistence
    rules: fleetd-specific guidance below.

## Core patterns — full catalog in `references/patterns.md`

### 1 — Add an action, bind it, document it (three edits, one commit)

```rust
// crates/fleet-app/src/actions.rs — inside the namespace that owns the surface
actions!(worktrees, [
    /// `y` — copy the highlighted worktree's path.
    CopyPath,
]);

// crates/fleet-app/src/keymap.rs — one key_table! row, explicit context
"y", "Hub > Worktrees" => worktrees::CopyPath;
```
Then add the row to `docs/KEYMAP.md`. `keymap::table()` renders the Help overlay and the palette
hint from that same row, so there is no fourth place to update. `references/patterns.md#actions`.

### 2 — Handle it where the state is

Two shapes: inside `Shell`, `cx.listener` (82 uses, `shell/root/actions.rs:237-249`); inside a
screen or dialog — plain structs, not entities — a cloned `Entity<AppState>`:

```rust
// crates/fleet-app/src/dialogs/<dialog>.rs
dialogs::root(focus)                       // .track_focus(focus).size_full()
    .on_action({
        let state = state.clone();
        move |_: &worktrees::CopyPath, _window, cx| {
            state.update(cx, |state, cx| {
                state.copy_selected_path();
                cx.notify();
            });
        }
    })
```

This deviates from Zed's `cx.listener` default and is correct here — there is no per-view entity
to listen on (`references/patterns.md#handlers`).

### 3 — One navigation vocabulary for every list

```rust
// crates/fleet-app/src/actions.rs — new module, modelled on zed/crates/menu/src/menu.rs
pub mod nav {
    use gpui::actions;
    actions!(nav, [
        /// `ctrl-n` / `down` — move to the next row.
        SelectNext,
        /// `ctrl-p` / `up` — move to the previous row.
        SelectPrevious,
        /// `⏎` — run the selected row.
        Confirm,
        /// `Esc` — leave the list.
        Cancel,
    ]);
}
```
Bind them once against a `List` context word in `keymap.rs` and have each list's root handle
those instead of its own cursor actions. Respect `FuzzyList::binds_jk` (`fuzzy_list.rs:3-7`):
a list under a text field never binds `j`/`k`.

### 4 — A `Dialog` trait, extracted one variant at a time

`dialogs/mod.rs` fans a 16-variant enum across parallel matches — `context_name` (`:92`), `width`
(`:114`), `render` (`:140`), `seed` (`:172`) — plus a field per dialog on `DialogHost`
(`dialogs/host.rs:17-57`), so adding a dialog edits seven places (audit §7.8). Zed's analogue is
`ManagedView`/`ModalView`, adapted here with no new entity and no `EventEmitter`:

```rust
// crates/fleet-app/src/dialogs/dialog.rs (new). Not Zed's `workspace::DismissDecision`.
pub(crate) enum DismissDecision { Dismiss, Keep }

pub(crate) trait Dialog: 'static {
    /// The key-context word `keymap.rs` binds against. Globally unique (§ APP-CONTRACTS 174).
    const CONTEXT: &'static str;
    /// The draft this dialog owns inside `DialogHost`.
    type Draft: Default + 'static;
    fn draft(host: &mut DialogHost) -> &mut Self::Draft;
    fn width(cx: &App) -> Pixels;
    fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App);
    fn render(state: &Entity<AppState>, bridge: &Bridge, focus: &FocusHandle,
              host: &Entity<DialogHost>, window: &mut Window, cx: &mut App) -> AnyElement;
    /// Refuse `Esc` while an operation is in flight, instead of unbinding the key.
    fn on_before_dismiss(_draft: &Self::Draft) -> DismissDecision { DismissDecision::Dismiss }
}
```

The `Dialogs` enum stays — it is the keymap- and state-facing token — but its arms become one
generated table. Migration order and the `dismiss` veto path: `references/patterns.md#dialogs`.

### 5 — A picker delegate for any new fuzzy list

`dialogs/palette.rs` (2 046 lines) has no delegate, and `create_worktree.rs` (1 750),
`clone_repo.rs` (831), `filter.rs` (370), `card_picker/draft.rs` (202) and `views/prs_screen.rs`
(697) each reimplement type → filter → clamp → enter. Zed solves it once with `Picker<D>` + a
9-method `PickerDelegate` (`zed/crates/picker/src/picker.rs:164`; 48 delegates, half under 500
lines each):

```rust
// crates/fleet-app/src/dialogs/picker.rs (new) — app-side, so ui-kit stays domain-free
pub(crate) trait PickerDelegate: 'static {
    fn match_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn set_selected_index(&mut self, ix: usize);
    fn placeholder_text(&self, cx: &App) -> SharedString;
    /// Match on the background executor; apply on the foreground. Retain the Task.
    fn update_matches(&mut self, query: String, state: &Entity<AppState>, cx: &mut App) -> Task<()>;
    fn confirm(&mut self, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App);
    fn dismissed(&mut self, cx: &mut App);
    /// `None` for an out-of-range index — never `rows[ix]`.
    fn render_match(&self, ix: usize, selected: bool, cx: &App) -> Option<FuzzyItem>;
}
```

It renders through the existing `fleet_ui_kit::FuzzyList`, keeping selection state in the
`DialogHost` draft so the kit's "components own no state" contract stays intact
(`docs/DESIGN-SYSTEM.md:350-354`, `crates/fleet-ui-kit/src/lib.rs:11-13`);
`references/patterns.md#pickers`.

### 6 — Give transient feedback a stable identity

```rust
// crates/fleet-app/src/state/notifications.rs — add alongside the text coalescer
pub enum ToastId {
    Named(&'static str),            // one per call site, whatever it interpolates
    Composite(&'static str, String) // one per subject: ("clone", repo_id.to_string())
}
```
Dedupe on the id first, keeping the 1 s identical-text window as fallback. Errors never take this
path — they go to `AppState.sticky_error` (`docs/UX-SPEC.md:239-241`).

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
| --- | --- | --- |
| `cx.on_action` for a screen or dialog verb | Fires with nothing focused; two surfaces with the same verb fight | `.on_action` on the element that calls `.track_focus(focus)` |
| A `key_table!` row with a context that another surface also uses | gpui's `>` is a subsequence test, so the binding fires in the wrong place (`docs/APP-CONTRACTS.md:174-182`) | A globally unique context word; `keymap.rs`'s tests enforce it |
| Unbinding `Esc` so a dialog cannot close mid-operation | The key silently does nothing | `on_before_dismiss -> DismissDecision::Keep` plus a toast saying why |
| A dialog module mutating `AppState.overlay` to close itself | Every call site must then remember the veto, the draft reset and the focus hand-back | One `dialogs::dismiss(state, cx)` |
| Filtering a candidate list in `render` or the notify path | Breaks "render prepares nothing" and the frame budget as the list grows | `cx.background_executor().spawn` behind `update_matches -> Task<()>` |
| `rows[cursor]` / `matches[selected]` | Panics on a stale cursor after a background refresh | `match_count()` + `render_match(ix) -> Option<_>` |
| Coalescing toasts on rendered text | Any interpolated id or count defeats it | A `ToastId`, checked before the text window |
| A toast for something a row or pill already shows | Violates the §2.7 toast law | Let the row show it; sticky slot for errors |
| Hardcoding a shortcut string in a hint or menu label | Drifts from `keymap.rs` | Render it from `keymap::table()` / `palette::key_for` (`palette.rs:627`) |
| `.detach()` on a fallible task | The error never reaches the user | A logging/sticky-error helper on `Task<anyhow::Result<T>>` |

## fleetd-specific guidance

**Where things live** (under `crates/fleet-app/src/`). `actions.rs` (800 lines, 25 namespaces) ·
`keymap.rs` (1 081; `key_table!` `:59-119`, rows `:142-605`, `init` `:608`) · `dialogs/mod.rs`
(279; the central match) · `dialogs/host.rs` (438; `DialogHost` `:18`, `DialogRegistry` `:59-64`,
`SessionTransport` `:66-84`, `impl Render for ActiveDialog` `:413`) · `dialogs/palette.rs`
(2 046) · `shell/root/focus.rs` (551) · `shell/root/bootstrap.rs` (127).

**Coexisting with `FocusOwnerKeys` (`focus.rs:14-97`).** It bumps a generation whenever
`focus_owner(state)` (`:129`) changes, queues key-downs arriving before the matching frame is
painted, and drops pointer events resolved against a stale frame. For new code: (a) a new
*authoritative* focus owner must come from `AppState` and appear in `focus_owner`/`focus_target`
(`:382-411`) — one the gate cannot see never bumps the generation and can lose its first
keystroke; (b) never focus a handle from a listener; change the state the gate reads and let
`focus_surface` (`:413-431`) do it; (c) a new overlay must be `deferred` *below*
`POINTER_GATE_PRIORITY` (`:16`); (d) do not add a second `intercept_keystrokes`,
`capture_key_down` or focus observer — extend those at `:153-200` and
`shell/root/actions.rs:224-236`. "Simplifying" the gate is out of scope unless the task says so.

**Migrating dialogs to the trait, incrementally.** Land the trait plus a `dialog_table!` macro
that expands one line per variant into the four existing matches, then move variants smallest
first — `rename_terminal`, `assign_repo`, `confirm`, `card_picker`, `context` — leaving the rest
in their hand-written arms. Each migration keeps the `Dialogs` variant name and its
`context_name()` string byte-identical, because `keymap.rs` binds against them
(`docs/APP-CONTRACTS.md:118-120`). Add `dialogs::dismiss(state, cx)` in the same series, routing
`close_overlay` call sites through it as you touch them; `palette.rs` and `create_worktree.rs`
go last, each split behind Core pattern 5 rather than moved wholesale.

**Gaps worth closing when you are already in the file.** `#[action(deprecated_aliases = [..])]`
is unused (0 sites) — add it on any action rename so an in-flight keymap keeps working.
`bootstrap.rs:66-79` builds `WindowOptions` inline: extract `fn window_options(cx: &mut App) ->
WindowOptions` before adding a second window, and note bounds are never persisted
(`Bounds::centered`, `:66`) while `cx.observe_window_bounds` is wired for reflow only
(`focus.rs:340-344`). The OS menu is one item (`bootstrap.rs:54-58`, 0 `os_action`); a macOS Edit
menu from `MenuItem::os_action(.., OsAction::{Cut,Copy,Paste,SelectAll,Undo,Redo})` is ~40 lines.
There is no single-instance guard — a second `fleet` opens a second app against the same daemon,
and it belongs *before* `gpui_platform::application()` (Zed: `ensure_only_instance()`, `main.rs:384`).

**What not to change.** Do not port Zed's `ModalLayer`: fleetd's single
`AppState.overlay: Option<Overlay>` slot with one permanently mounted `ActiveDialog` is simpler
and already correct. Do not move the key table to JSON unless user remapping is actually wanted
(`table()` feeding both bindings and docs beats Zed). Do not add `anchored()` popovers or context
menus (0 uses, deliberate — this is a keyboard app), and do not adopt `window.prompt`.

**Keep doing.** One action per KEYMAP row with a user-facing doc comment; a key table that logs
instead of panicking on a bad built-in binding; `_subscriptions`/`_tasks` retention on `Shell`
(`shell/root.rs:71-72`); recording KEYMAP deviations in `docs/APP-CONTRACTS.md`.

## Review checklist — full list in `references/checklist.md`

- Does every new action have a `///` comment naming its keystroke, plus a `docs/KEYMAP.md` row
  changed in the same commit?
- Does every new `key_table!` row carry an explicit, globally unique context?
- Is each handler on an element that calls `.track_focus(focus)`, with `cx.on_action` still
  absent from production, and does a partial handler end in `cx.propagate()`?
- Is focus granted outside `render` (state change → `focus_surface`), and does a new dialog close
  through one dismissal path, with a veto rather than an unbound `Esc`?
- Does a new filtering list match on the background executor, apply from a retained `Task`, and
  read the selected row through a clamped `Option`-returning accessor rather than an index?
- Does new transient feedback have a stable identity and pass the §2.7 toast law, and does every
  new fallible task surface its error instead of `.detach()`?
- Does new focus-owner code appear in `focus_owner`/`focus_target` so the generation gate sees it?

## Related skills

- `gpui-state-and-memory` — entities, `WeakEntity`, subscriptions, `Task` ownership, globals.
- `gpui-components` — `RenderOnce` design, `FuzzyList`, list rendering, the gallery.
- `gpui-styling` — dialog frame tokens, elevation, icons, motion.
- `gpui-performance` — render discipline, virtualization, the frame budget behind rule 11.
- `rust-async-background-work` — background executor, `Bridge`, Task lifetimes.
- `rust-gpui-testing` — `#[gpui::test]` for key dispatch, focus and dialogs.
- `zed-quality-review` — loads this skill's `references/checklist.md`.
