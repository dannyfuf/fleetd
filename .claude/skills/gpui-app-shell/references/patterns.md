# App-shell patterns: Zed v1.18.1 → fleetd

Every Zed citation is `zed/crates/<crate>/src/<file>.rs:<line>` at tag **v1.18.1**
(`/Users/danny/.swarm/repos/zed-industries/zed`). Every fleetd path is relative to the worktree
root and was verified with `rg`/`ls`. fleetd depends on `gpui` + `gpui_platform` only — none of
Zed's `workspace`, `ui`, `picker`, `menu`, `fuzzy` crates are importable, so each pattern below
is a design to reimplement.

Contents: [boot](#boot) · [actions](#actions) · [handlers](#handlers) ·
[keymap](#keymap) · [contexts](#contexts) · [focus](#focus) · [navigation](#navigation) ·
[dialogs](#dialogs) · [overlays](#overlays) · [pickers](#pickers) ·
[discoverability](#discoverability) · [feedback](#feedback) · [windows](#windows) ·
[menus](#menus) · [settings](#settings)

---

## boot

**Zed.** 90 crates expose `pub fn init(cx: &mut App)`; `main.rs` calls them in one flat,
explicitly ordered block inside `app.run(move |cx| {` (`zed/crates/zed/src/main.rs:485`).

```rust
// zed/crates/theme_selector/src/theme_selector.rs:31
pub fn init(cx: &mut App) {
    cx.on_action(|action: &zed_actions::theme_selector::Toggle, cx| {
        let action = action.clone();
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            toggle_theme_selector(workspace, &action, window, cx);
        });
    });
}
```

Hard constraints readable from Zed's order: settings before any settings observer;
`AppState::set_global` before every `*::init(&app_state, cx)`; theme before the first
`cx.theme()` read; menus before the first window.

**fleetd — matches.** `crates/fleet-app/src/shell/root/bootstrap.rs:43-89`:
`gpui_platform::application().with_assets(KitAssets).run(..)` → `Theme::init(ThemeMode::Dark, cx)`
`:46` → `keymap::init(cx)` `:47` → `fleet_lazygit::keymap::init(cx)` `:52` → `cx.set_menus` `:53`
→ `cx.on_window_closed` `:59` → `cx.open_window` `:80` → `window.activate_window()` +
`cx.activate(true)` `:87-88`. `init_tracing()` (`:27-34`) runs *before* `application()`.

**Do.** Add a new subsystem's `init(cx)` to that block, at the position its constraints require.
**Don't.** Add an `init` that registers nothing app-level, or hide subsystem setup in
`Shell::new` where the ordering is invisible. `Application::new()` has **0 uses** in v1.18.1 —
the constructors are `with_platform` (`zed/crates/gpui/src/app.rs:177`) and
`new_inaccessible` (`:194`), and `.run(f)` is `:233`.

---

## actions

**Zed rule** (`/.rules`, §GPUI/Actions, verbatim): *"Actions with no data are defined with the
`actions!(some_namespace, [SomeAction, AnotherAction])` macro call. Otherwise the `Action` derive
macro is used. Doc comments on actions are displayed to the user."*

```rust
// zed/crates/zed_actions/src/lib.rs:15
/// Opens a URL in the system's default web browser.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
pub struct OpenBrowser { pub url: Arc<str> }
```

`#[action(..)]` options (`zed/crates/gpui/src/action.rs:72-89`): `namespace = ns`,
`name = "Other"`, `no_json`, `no_register`, `deprecated_aliases = ["old::Name"]`,
`deprecated = "msg"`. Two actions with the same name panic at `App` creation (`action.rs:52`).
Counts: 184 `actions!(`, 260 `#[action(`, **0** real `impl_actions!`.

**fleetd — matches, strongly.** `crates/fleet-app/src/actions.rs` (800 lines) is one module per
key context — `fleet`, `hub`, `repos`, `worktrees`, `prs`, `board`, `prefix`, `scroll`, `filter`,
`palette`, `jobs`, `dialog`, `confirm`, `settings`, `card_detail`, `agent`, `native_agent`, … —
each `actions!` variant carrying a `///` comment that names its keystroke:

```rust
// crates/fleet-app/src/actions.rs:8-16 (trimmed)
pub mod fleet {
    use gpui::actions;
    actions!(fleet, [
        /// `ctrl-q` — quit the app; the daemon keeps running.
        Quit,
        /// `:` — open the command palette.
        OpenPalette,
    ]);
}
```

The invariant is stated in the file header (`actions.rs:3`): **one action per row of
`docs/KEYMAP.md`**.

**Gap.** `deprecated_aliases` has 0 uses. On any rename, keep the old name reachable:
`#[action(deprecated_aliases = ["worktrees::Copy"])]`.

**Don't.** Use an action for an internal message between your own code paths — that is
`cx.emit`/state. Actions are the *user-addressable* surface.

### The dependency-free actions module

`zed/crates/menu/src/menu.rs:1-37` is the entire `menu` crate (eleven actions, abridged here):

```rust
use gpui::actions;

// If the zed binary doesn't use anything in this crate, it will be optimized away
// and the actions won't initialize. So we just provide an empty initialization function
// to be called from main.
pub fn init() {}

actions!(menu, [
    /// Cancels the current menu operation.
    Cancel,
    /// Confirms the selected menu item.
    Confirm,
    /// Selects the next item in the menu.
    SelectNext,
]);
```

Three reasons: (a) it breaks dependency cycles; (b) the empty `init()` is load-bearing —
registration happens in a ctor before `main`, and an unreferenced crate is optimized away,
silently losing its actions (rust-lang/rust#47384); (c) one vocabulary means the keymap binds
`down`/`ctrl-n`/`tab` → `menu::SelectNext` **once** for every list-like surface.

**fleetd — matches.** `actions.rs` is dependency-free, and `fleet-lazygit/src/actions.rs` uses
non-colliding namespaces (`lg_confirm`, `lg_help`) because gpui action names are process-wide and
registering two actions with the same name panics during `App` creation (`docs/APP-CONTRACTS.md:174-182`). Single binary, so
ctor elision does not bite today — do not delete the pattern on that basis if fleetd ever splits.

---

## handlers

Zed's three tiers:

| tier | API | Zed uses | when |
| --- | --- | ---: | --- |
| element | `.on_action(cx.listener(Self::foo))` | 1023 | anything scoped to a view; fires only on the focus path |
| shell indirection | `workspace.register_action::<A>(cb)` | ~340 | a downstream module attaching to a shell it can't depend on |
| app-global | `cx.on_action(\|a: &A, cx\| ..)` | 50 | quit/hide/about — must work with no window focused |

```rust
// zed/crates/zed/src/zed.rs:194
pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(quit);
```

**Dispatching in code** (`/.rules`): `window.dispatch_action(SomeAction.boxed_clone(), cx)`
(`zed/crates/gpui/src/window.rs:2187`, 326 uses), `cx.dispatch_action`
(`zed/crates/gpui/src/app.rs:2460`, 359), `focus_handle.dispatch_action(&A, window, cx)`
(`window.rs:627`, 71 — act as if *that* element were focused).

**fleetd — matches on tier 1, diverges on the listener.** 349 `.on_action`, **0** `cx.on_action`
in production. `Shell::with_actions` (`crates/fleet-app/src/shell/root/actions.rs:218-249`)
carries 82 handlers, all `cx.listener`:

```rust
// crates/fleet-app/src/shell/root/actions.rs:238-241
.on_action(cx.listener(Self::quit))
.on_action(cx.listener(Self::quit_and_stop_daemon))
.on_action(cx.listener(Self::open_palette))
```

Screens and dialogs are plain structs, not entities, so they close over a cloned
`Entity<AppState>` instead — `crates/fleet-app/src/dialogs/palette.rs:998-1004`:

```rust
.on_action({
    let state = state.clone();
    move |_: &palette_actions::CursorDown, _window, cx| move_cursor(&state, 1, cx)
})
```

**[Deliberate deviation]** from Zed's `cx.listener` default; it is correct given the
single-entity architecture, and belongs in `docs/APP-CONTRACTS.md` as such.
Zed's `register_action` indirection buys nothing in a single-crate app — skip it.

### Propagation

```rust
// zed/crates/picker/src/picker.rs:1021-1025
// Propagate so `tab` retains its other meanings (e.g.
// `ConfirmCompletion`) in pickers without multi-select.
if !self.delegate.supports_multi_select() { cx.propagate(); return; }
```

fleetd states the same rule harder (`docs/APP-CONTRACTS.md:449-464`): gpui stops at the first
bubble listener, "there is no 'also run the outer handler' by default, and the failure is
silent". 8 `cx.propagate()` sites in `fleet-app`, each with a comment explaining the hand-off.

---

## keymap

**Zed.** Bindings are a JSON asset (1712 lines in `assets/keymaps/default-macos.json`), reloaded
idempotently — and the OS menu is rebuilt in the same function so accelerators re-resolve:

```rust
// zed/crates/zed/src/zed.rs:2398
fn reload_keymaps(cx: &mut App, mut user_key_bindings: Vec<KeyBinding>) {
    cx.clear_key_bindings();
    load_default_keymap(cx);
    for key_binding in &mut user_key_bindings { key_binding.set_meta(KeybindSource::User.meta()); }
    cx.bind_keys(filter_disabled_ai_bindings(user_key_bindings, cx));
    let menus = app_menus(cx);
    cx.set_menus(menus);
}
```

Predicate grammar — `zed/crates/gpui/src/keymap/context.rs:172`:
`Identifier`, `Equal("mode","full")`, `NotEqual`, `Descendant(a, b)` (`A > B`), `Not`, `And`,
`Or`. Keystroke syntax: space-separated sequences (`"cmd-k cmd-s"`), modifiers
`ctrl- cmd- alt- shift- fn- secondary-`.

**`KeyBinding::new` panics.** It `.unwrap()`s the context parse
(`zed/crates/gpui/src/keymap/binding.rs:33`); `KeyBinding::load(..)` (`:48`) returns a `Result`.

**fleetd — diverges deliberately, and does the safe thing.**
`crates/fleet-app/src/keymap.rs` (1 081 lines) is a `key_table!` macro (`:59-119`) over 386 rows
of `"keys", "Context" => action;` (`:142-605`), emitting three things from one source:

```rust
// crates/fleet-app/src/keymap.rs — the shape of every row
"y", "Hub > Worktrees" => worktrees::CopyPath;
```

- `bindings() -> Vec<KeyBinding>` (`:77`), registered by `init(cx)` (`:608-610`);
- `table() -> Vec<BindingSpec>` — `{keys, context, action}`, which the Help overlay and the
  palette hints render from, so keys and docs cannot drift
  (`docs/APP-CONTRACTS.md:437-443`); **this is better than Zed**, which splits JSON from
  generated docs;
- `action_for_keystroke(context, keystroke)` — the live one-shot `Workspace > Prefix` resolver,
  because the rendered context tree is a frame behind a state change (`:99-118`).

Parsing goes through `KeyBindingContextPredicate::parse` + `KeyBinding::load(.., &DummyKeyboardMapper)`
and **logs `tracing::error!` instead of panicking** on a bad built-in row (`:66-68`, `:121-136`).

**Costs of the divergence** (state them, don't fix them uninvited): no user remapping, and a
recompile per binding change. If remapping is ever wanted, move to a JSON asset with
`clear_key_bindings()` + `bind_keys()` and keep `table()` as the built-in layer.

**Doc gate.** `docs/KEYMAP.md:12-13`: *"This file is the single source of truth for keys …
where the two disagree, this file wins."* A keymap row and its KEYMAP.md row change together.
Deliberate arbitrations are recorded (`docs/APP-CONTRACTS.md:225-236`, `keymap.rs:28-31`):
`q` unbound in Palette (a binding would make the query untypable), `f` in Jobs, `q` closing Jobs.

---

## contexts

**Zed.** 172 `.key_context(` sites; a view builds its context in one function:

```rust
// zed/crates/editor/src/editor.rs:2672
pub fn key_context(&self, window: &mut Window, cx: &mut App) -> KeyContext {
    let mut key_context = KeyContext::new_with_defaults();   // pre-seeds os = macos|linux|windows
    key_context.add("Editor");
    if EditorSettings::jupyter_enabled(cx) { key_context.add("jupyter"); }
    key_context.set("mode", mode);
    if self.pending_rename.is_some() { key_context.add("renaming"); }
```

**The precedence trap**, verbatim from `zed/crates/gpui/src/keymap.rs:154-159`:
> Precedence is defined by the depth in the tree (matches on the Editor take precedence over
> matches on the Pane, then the Workspace, etc.). **Bindings with no context are treated as the
> same as the deepest context.** In the case of multiple bindings at the same depth, the ones
> added to the keymap later take precedence.

**fleetd — diverges, centralized in state.** The chain comes from `AppState::context_chain()`
and is rendered as one nested div per word:

```rust
// crates/fleet-app/src/shell/root/focus.rs:295-306 (trimmed)
pub(super) fn contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
    let mut element = child;
    for context in chain.iter().rev() {
        element = div().size_full().key_context(*context).child(element).into_any_element();
    }
    element
}
```

with `overlay_contexts` (`:311-323`) as the `.absolute().inset_0()` variant, so an overlay's
wrappers do not consume the frame's flex column. The root word is `ROOT_CONTEXT = "Fleet"`
(`keymap.rs:140`), applied at `shell/root/render.rs:43`. Chains look like
`Fleet > Hub > Worktrees`, `Fleet > Workspace > Prefix`, `Fleet > Dialog > Confirm`
(`docs/APP-CONTRACTS.md:155-170`).

**Consequence.** A view cannot add a *state flag* to its own context the way `Editor` does. If
one is needed, add an explicit hook (`fn key_context_flags(&self) -> &[&str]`) rather than
calling `.key_context` ad hoc inside a screen — two sites do that today
(`screens/jobs/presentation.rs`, `dialogs/rename_terminal.rs`) and they are the exception.

**Uniqueness is mandatory.** gpui's `>` is a subsequence test over the rendered chain, not a
parent test, so an embedded view may not reuse any app context word — `fleet-lazygit` renames its
overlay words to `LgDialog`/`LgConfirm`/`LgHelp`, and the
`the_embedded_pane_shares_no_context_word_with_the_app` test in `keymap.rs` keeps it true
(`docs/APP-CONTRACTS.md:174-182`).

**Debugging.** Zed ships `dev::OpenKeyContextView`
(`zed/crates/language_tools/src/key_context_view.rs:16`), ~200 lines over `cx.observe_keystrokes`
(`zed/crates/gpui/src/app.rs:2186`) + `cx.all_bindings_for_input` (`:2304`), rendering the live
context stack and every binding that could have matched. fleetd has 386 rows and no equivalent;
it would pay for itself.

---

## focus

```rust
// zed/crates/gpui/src/window.rs:700
pub trait Focusable: 'static { fn focus_handle(&self, cx: &App) -> FocusHandle; }
```

Surface actually used in Zed: `.track_focus(&h)` 200 · `h.focus(window, cx)` /
`window.focus(&h, cx)` 261 · `cx.focus_self(window)` 59 (`zed/crates/gpui/src/app/context.rs:752`,
defers to the next effect cycle) · `h.is_focused(window)` (`window.rs:605`) ·
`h.contains_focused(window, cx)` (`:611`) · `h.within_focused(window, cx)` (`:617`) ·
`window.on_focus_in(&h, cx, cb)` (`:4908`) · `window.on_focus_out(&h, cx, cb)` (`:4928`).
Tab order: `cx.focus_handle().tab_index(n).tab_stop(true)` + `window.focus_next(cx)` /
`focus_prev(cx)` (`window.rs:2093`, `:2104`).

**Never** focus during `render` or before the target is laid out — `cx.defer_in(window, ..)` /
`cx.focus_self(window)`.

**fleetd — three window-level handles, one `Focusable`.**

```rust
// crates/fleet-app/src/shell/root.rs:50-56 (trimmed)
/// Focused while the Hub or the Workspace owns the keyboard.
body_focus: FocusHandle,
/// Focused while a dialog, the palette, the filter or the jobs panel is open, so that an
/// overlay's key context really does shadow the screen behind it.
overlay_focus: FocusHandle,
/// Focused while the floating agent terminal is the topmost surface.
agent_focus: FocusHandle,
```

`impl Focusable for Shell` returns `body_focus` (`shell/root.rs:145-149`). Screens and dialogs
receive `&FocusHandle` and must track it — `dialogs::root(focus)` does it for every dialog:

```rust
// crates/fleet-app/src/dialogs/mod.rs:197-199
pub(crate) fn root(focus: &FocusHandle) -> Div {
    div().track_focus(focus).size_full()
}
```

The contract (`docs/APP-CONTRACTS.md:107-112`): the root element *must* call `.track_focus(focus)`
— "that is what puts your `on_action` listeners on the key-dispatch path" — and "the shell focuses
the handle it passed you and never touches focus again".

**Consequence.** Nothing inside a dialog is independently focusable, so `tab_index`/`tab_stop`
have 0 uses. A dialog with more than one input that needs real tab order should own its own
handles, keeping the shell handle as the region root.

**Focus application** is state-driven, outside render:

```rust
// crates/fleet-app/src/shell/root/focus.rs:420-431 (trimmed)
let wanted = match target {
    FocusTarget::Body => body,
    FocusTarget::Overlay => overlay,
    FocusTarget::Agent => agent,
    // Native panes and agent tabs restore their own descendant focus on activation.
    FocusTarget::Native | FocusTarget::AgentThread => return,
};
if !wanted.is_focused(window) { window.focus(wanted, cx); }
```

with `focus_target(state)` (`:382-411`) deriving the target from `AppState`. There is **no**
previous-focus stack: closing an overlay lands focus on Body/Agent by recomputation. That works
for three targets; if per-view handles land, adopt Zed's restore rule below.

### `FocusOwnerKeys` — fleetd-only, ahead of Zed, do not remove

```rust
// crates/fleet-app/src/shell/root/focus.rs:19-27 (trimmed)
pub(super) struct FocusOwnerKeys {
    pub(super) live_owner: Vec<&'static str>,
    pub(super) live_generation: u64,
    pub(super) replayed_generation: u64,
    pub(super) queued: VecDeque<KeyDownEvent>,
    pub(super) awaiting_capture: bool,
}
```

`sync_owner` bumps the generation when the authoritative owner changes (`:42-52`); `capture`
queues the complete event, evicting past `STALE_KEY_CAPACITY = 64` (`:13`, `:67-77`);
`finish_render` releases the queue only for the current generation (`:82-89`);
`rejects_pointer` drops mouse events resolved against a stale frame (`:91-97`). Installed by
`install_input_gates` (`:153-200`, `cx.observe` + `cx.intercept_keystrokes`), with the root
`capture_key_down` half at `shell/root/actions.rs:224-236` and the pointer gate deferred at
`POINTER_GATE_PRIORITY = usize::MAX` (`:16`, `:288-289`). Documented at
`docs/APP-CONTRACTS.md:210-222`.

**Coexistence rules for new code.**
1. A new authoritative focus owner must be derivable from `AppState` and appear in
   `focus_owner` (`:129`) and `focus_target` (`:382`). An owner the gate cannot see never bumps
   the generation, and its first keystroke can be lost.
2. Never call `window.focus(..)` from a listener. Change the state the gate reads; `focus_surface`
   applies it.
3. A new overlay painting above the interactive tree must be `deferred` with a priority *below*
   `POINTER_GATE_PRIORITY`, so the gate still sits last.
4. Do not add a second `intercept_keystrokes`, `capture_key_down`, or focus observer — extend the
   existing ones.
5. Removing or restructuring the gate is out of scope unless the task is explicitly about it.

---

## navigation

**Zed.** Every list-like surface handles the same `menu::` actions; the keymap binds them once
(`assets/keymaps/default-macos.json:6-27`). Handlers hang off the container:

```rust
// zed/crates/picker/src/render.rs:166
let menu = v_flex()
    .key_context(key_context)
    .on_action(cx.listener(Self::select_next))
    .on_action(cx.listener(Self::select_previous))
    .on_action(cx.listener(Self::select_first))
    .on_action(cx.listener(Self::select_last))
    .on_action(cx.listener(Self::cancel))
    .on_action(cx.listener(Self::confirm))
    .on_action(cx.listener(Self::secondary_confirm))
```

For non-uniform focusable rows Zed wraps them in `Navigable`
(`zed/crates/ui/src/components/navigable.rs:6`), holding `Vec<NavigableEntry>` of
`{focus_handle, scroll_anchor}` and mapping `SelectNext/Previous` onto them in declaration order.

**fleetd — missing, and the highest-leverage change in this area.** Navigation verbs are
re-declared per namespace: `palette::{CursorDown,CursorUp,Backspace,DeleteWord,Clear}`
(`crates/fleet-app/src/actions.rs:469-487`), `scroll::*` (`:394`), and separate cursor actions in
`card_picker`, `create_worktree`, `clone_repo`, `filter`. What *is* shared today is only
`dialogs::step` (`dialogs/mod.rs:30`, a clamped cursor step re-exported from `state`),
`FuzzyQuery`/`contains_folded` (`presentation/keys.rs:5-45`) and the render-only `FuzzyList`
(`crates/fleet-ui-kit/src/components/fuzzy_list.rs`).

**Proposal.** Add `actions::nav::{SelectNext, SelectPrevious, SelectFirst, SelectLast, Confirm,
Cancel}`, bind them once against a `List` context word, and have every new list handle those.
Honour `FuzzyList`'s documented split (`fuzzy_list.rs:3-7`): a list **under a text field** moves
with `ctrl-n`/`ctrl-p` or `↓`/`↑` and never with `j`/`k`; a dialog with no text field (Assign,
Settings, Confirm) does bind `j`/`k`. Do not retrofit existing surfaces opportunistically —
each migration is a KEYMAP.md change.

---

## dialogs

**Zed's contract.**

```rust
// zed/crates/gpui/src/window.rs:711-715
/// ManagedView is a view (like a Modal, Popover, Menu, etc.)
/// where the lifecycle of the view is handled by another view.
pub trait ManagedView: Focusable + EventEmitter<DismissEvent> + Render {}
impl<M: Focusable + EventEmitter<DismissEvent> + Render> ManagedView for M {}
```

```rust
// zed/crates/workspace/src/modal_layer.rs:49-56
pub trait ModalView: ManagedView {
    fn on_before_dismiss(&mut self, _window: &mut Window, _: &mut Context<Self>) -> DismissDecision {
        DismissDecision::Dismiss(true)
    }
```

Most impls are empty; the veto exists for modals that must not close mid-operation
(`zed/crates/remote_connection/src/remote_connection.rs:427`:
`DismissDecision::Dismiss(self.finished)`). Host state is `ActiveModal { modal,
_subscriptions: [Subscription; 2], previous_focus_handle: Option<FocusHandle>, focus_handle }`
(`modal_layer.rs:104-108`); `toggle_modal` (`:152-170`) *inherits* `previous_focus_handle` from an
outgoing modal so a chain still restores the original pre-modal focus.

Three behaviours worth copying verbatim:

```rust
// zed/crates/workspace/src/modal_layer.rs:185-201 (trimmed) — focus is granted deferred
let dismiss_subscription = modal.subscribe_dismiss(window, cx);
self.active_modal = Some(ActiveModal { modal, _subscriptions: [dismiss_subscription,
    cx.on_focus_out(&focus_handle, window, |this, _e, window, cx| {
        if this.dismiss_on_focus_lost { this.hide_modal(window, cx); }
    })], previous_focus_handle, focus_handle });
cx.defer_in(window, move |_, window, cx| { window.focus(&modal_focus_handle, cx); });
```
```rust
// zed/crates/workspace/src/modal_layer.rs:254-258 — restore only if we still hold focus
if let Some(previous_focus) = active_modal.previous_focus_handle
    && active_modal.focus_handle.contains_focused(window, cx)
{ previous_focus.focus(window, cx); }
```
```rust
// zed/crates/workspace/src/modal_layer.rs:298-327 (trimmed) — scrim vs card
div().absolute().size_full().inset_0().occlude()
    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.hide_modal(window, cx)))
    .child(v_flex().h(px(0.0)).top_20().items_center()      // zero height: centred, no layout impact
        .track_focus(&active_modal.focus_handle)
        .child(h_flex().occlude().child(active_modal.modal.view())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())))
```

`dismiss_on_focus_lost` is set only when `on_before_dismiss` returned `Dismiss(false)` — a modal
that vetoed an explicit close becomes click-outside-dismissable instead.

**fleetd — a single overlay slot, permanently mounted.** `AppState.overlay: Option<Overlay>`
(`Dialog(Dialogs) | Palette | Filter | Jobs`), rendered by `Shell::render_overlay`
(`crates/fleet-app/src/shell/root/render.rs:53-73`), with one `ActiveDialog` entity
(`crates/fleet-app/src/dialogs/host.rs:412-435`) that observes state + host and matches on the
current overlay. Drafts for **every** dialog live in one `DialogHost` struct
(`host.rs:17-57`) held in a `Global` registry keyed by the `EntityId` of the `AppState` entity,
released with the window (`host.rs:60-110`, `cx.observe_release`). Zero `DismissEvent`.

**Do not port `ModalLayer`.** One slot is simpler and already correct. Port the two ideas that
are missing: the dismissal veto, and a single owned close path.

### The incremental `Dialog` trait

The problem (audit §7.8): `Dialogs::` is matched at 140 sites; the enum fans out across
`context_name` (`dialogs/mod.rs:92-110`), `width` (`:114-137`), `render` (`:140-168`), `seed`
(`:172-193`), a `DialogHost` field, `actions.rs` and `keymap.rs`. Every dialog module
independently defines free `seed` (13), `render` (21), `submit` (5), `close` (3), `move_cursor`
(4) with near-identical signatures. **There is no `Dialog` trait.**

```rust
// crates/fleet-app/src/dialogs/dialog.rs (proposed). fleetd's own enum: Zed's
// `workspace::DismissDecision` (`modal_layer.rs:8-11`) is `Dismiss(bool) | Pending`.
pub(crate) enum DismissDecision { Dismiss, Keep }

pub(crate) trait Dialog: 'static {
    /// The key-context word `keymap.rs` binds against. Globally unique (APP-CONTRACTS §3).
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
    /// Reset the draft. Called by `dialogs::dismiss` after the veto passes.
    fn close(draft: &mut Self::Draft) { *draft = Self::Draft::default(); }
}
```

and one owned close path replacing the scattered `state.update(|app| app.close_overlay())`:

```rust
// crates/fleet-app/src/dialogs/mod.rs (proposed)
pub(crate) fn dismiss(state: &Entity<AppState>, cx: &mut App) -> bool {
    let Some(dialog) = read_host(state, cx, |host, _| host.open.clone()) else { return false };
    if dialog.on_before_dismiss(state, cx) == DismissDecision::Keep { return false }
    with_host(state, cx, |host| dialog.close(host));
    state.update(cx, |app, cx| { app.close_overlay(); cx.notify(); });
    true
}
```

**Why this shape and not Zed's.** `ManagedView` requires each modal to be an entity that emits
`DismissEvent`; fleetd's dialogs are free functions over one `Entity<AppState>` plus a draft in
`DialogHost`. The trait above keeps that: associated `Draft` instead of `self`, static methods
instead of `&mut self`, and no `EventEmitter`. The veto and the single close path — the two
behaviours the pattern actually buys — survive intact.

**Migration order.**
1. Land `Dialog` + a `dialog_table!` macro that expands one line per variant into the existing
   `context_name`/`width`/`render`/`seed` matches. Unmigrated variants keep their hand-written
   arms; the macro and the match coexist.
2. Move the smallest dialogs first: `rename_terminal`, `assign_repo`, `confirm`, `card_picker`,
   `context`. Keep each `Dialogs` variant name and `context_name()` string byte-identical —
   `keymap.rs` binds against them (`docs/APP-CONTRACTS.md:118-120`).
3. Add `dialogs::dismiss` and route `close_overlay` call sites through it as you touch them.
4. `create_worktree.rs` (1 750 lines) and `palette.rs` (2 046) go last, each split behind a
   picker delegate rather than moved wholesale.

Every step keeps `docs/APP-CONTRACTS.md` §3.8 accurate in the same commit.

---

## overlays

**Zed.** Trigger-anchored popovers are *not* modals:

```rust
// zed/crates/gpui/examples/popover.rs:57 (trimmed)
deferred(
    anchored().anchor(Anchor::TopLeft).snap_to_window_with_margin(px(8.))
        .child(popover().child("…").on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.secondary_open = false; cx.notify();
        }))),
).priority(2)
```

`deferred(child)` (`zed/crates/gpui/src/elements/deferred.rs:7`) + `.with_priority(n)` (`:25`)
paints above siblings; `anchored()` (`zed/crates/gpui/src/elements/anchored.rs:27`) +
`.snap_to_window_with_margin(..)` (`:74`) keeps it inside the window.

**fleetd — partial, correctly.** 5 `deferred(` uses
(`crates/fleet-ui-kit/src/components/{overlay.rs,dialog.rs,sheet.rs,toast_stack.rs}` plus the
pointer gate at `shell/root/focus.rs:289`) and **0** `anchored()`. No `ContextMenu`, no
`PopoverMenu`, no right-click menu, no `Tooltip` — consistent with the keyboard-only mandate.
Leave it that way. If a trigger-anchored popover is ever genuinely needed, use
`deferred(anchored().snap_to_window_with_margin(px(8.)))`, not a new overlay slot in `AppState`.

---

## pickers

**Zed.** `picker.rs` (1999) + `render.rs` (453) + `shape.rs` (678) implement *once*: the query
editor, uniform-list virtualization, selection, scrolling, every `menu::` action, multi-select,
preview, footer, and size persistence. A feature supplies only a delegate — 51 `impl
PickerDelegate for` across 48 files, a third of them under 500 lines.

```rust
// zed/crates/picker/src/picker.rs:164 — the required methods; the rest of the ~450-line trait is defaulted
pub trait PickerDelegate: Sized + 'static {
    type ListItem: IntoElement;
    fn name() -> &'static str;                       // serialization key
    fn match_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn set_selected_index(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Picker<Self>>);
    fn placeholder_text(&self, _w: &mut Window, _cx: &mut App) -> Arc<str>;
    fn update_matches(&mut self, query: String, w: &mut Window, cx: &mut Context<Picker<Self>>) -> Task<()>;
    fn confirm(&mut self, secondary: bool, w: &mut Window, cx: &mut Context<Picker<Self>>);
    fn dismissed(&mut self, w: &mut Window, cx: &mut Context<Picker<Self>>);
    fn render_match(&self, ix: usize, selected: bool, w: &mut Window, cx: &mut Context<Picker<Self>>)
        -> Option<Self::ListItem>;
}
```

Canonical `update_matches` — match in the background, apply on the foreground
(`zed/crates/theme_selector/src/theme_selector.rs:440`): build `StringMatchCandidate::new(id, text)`,
`match_strings(..).await` on the background executor, then `this.update(cx, ..)` to store
`StringMatch { candidate_id, string, positions, score }`. `finalize_update_matches`
(`picker.rs:237`) blocks up to 16 ms on in-flight matching so `enter` right after typing does the
right thing — *"This avoids a flash of an empty command-palette on cmd-shift-p."*

Cancel/confirm never index:

```rust
// zed/crates/picker/src/picker.rs:989 (trimmed)
pub fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
    if self.delegate.should_dismiss() {
        self.delegate.clear_selection(cx);
        self.delegate.dismissed(window, cx);
        cx.emit(DismissEvent);
    }
}
```

**fleetd — the largest gap.** `crates/fleet-app/src/dialogs/palette.rs` is 2 046 lines with no
delegate: `PaletteState { query, cursor, rows, total, prepared_query }` (`:30-40`), `Entry`
(`:63`), a ~60-variant `Command` enum (`:90`), `candidates(..) -> Vec<Entry>` (`:665`), three
section builders (`:698`, `:827`, `:897`), `render` (`:918`), `visible_rows` (`:1044`),
`seed`/`refresh`/`refresh_query` (`:1054`, `:1068`, `:1093`), `move_cursor` (`:1103`),
`run_selected` (`:1111`), `run_command` (`:1148`). Five other surfaces reimplement
type → filter → clamp → enter: `dialogs/create_worktree.rs` (1 750), `dialogs/clone_repo.rs`
(831), `views/prs_screen.rs` (697), `dialogs/filter.rs` (370),
`dialogs/card_picker/draft.rs` (202).

Matching is synchronous and unscored: `FuzzyQuery::matches`
(`crates/fleet-app/src/presentation/keys.rs:17-24`) is a subsequence walk, and `candidates()`
runs in the notify path (`dialogs/host.rs:129-133` → `palette::refresh_query`). `ROW_CAP = 10`,
`IDLE_ROWS = 5` (`palette.rs:25-27`); order is section order then insertion order. `fleet-app`
calls `uniform_list` **0** times — virtualization lives in `fleet_ui_kit::{ListView, LogView}`.

**Proposal — a `PickerDelegate` in `fleet-app`, not in `fleet-ui-kit`.**

```rust
// crates/fleet-app/src/dialogs/picker.rs (proposed)
pub(crate) trait PickerDelegate: 'static {
    fn match_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn set_selected_index(&mut self, ix: usize);
    fn placeholder_text(&self, cx: &App) -> SharedString;
    /// Match on `cx.background_executor()`, apply on the foreground. Retain the Task.
    fn update_matches(&mut self, query: String, state: &Entity<AppState>, cx: &mut App) -> Task<()>;
    fn confirm(&mut self, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App);
    fn dismissed(&mut self, cx: &mut App);
    /// `None` for an out-of-range index — never `rows[ix]`.
    fn render_match(&self, ix: usize, selected: bool, cx: &App) -> Option<FuzzyItem>;
}
```

**Why app-side.** `fleet-ui-kit`'s crate doc is explicit: *"No domain types. This crate depends on
`gpui` and nothing else"* (`crates/fleet-ui-kit/src/lib.rs:11-13`), and
`docs/DESIGN-SYSTEM.md:350-354`: *"None of them owns state: the view's entity owns the cursor, the
query, the focus and the timers, and passes them down each frame."* A `Picker` that owns selection
and an in-flight `Task` breaks both. Put the delegate and its driver in `fleet-app`, keep
selection in the `DialogHost` draft, and render through the existing stateless
`fleet_ui_kit::FuzzyList` / `Palette` components. Retain the matching task with
`dialogs::host::retain_task(state, cx, "picker", task)` (`host.rs:149-158`), which already exists
for exactly this.

**Ranking.** There is no fuzzy crate in `Cargo.toml` and no scoring. When the delegate lands, add
score + match `positions`; `FuzzyItem::matches` (`fuzzy_list.rs:9-11`) already accepts the hit
indices and renders them as a weight bump — `docs/DESIGN-SYSTEM.md` caps the treatment there
deliberately. Model the data shape on `StringMatchCandidate::new(id, text)` →
`StringMatch { candidate_id, string, positions, score }`; do **not** add a Zed crate dependency.

**Migration order.** `card_picker` (smallest) → `clone_repo` → `create_worktree`'s base-ref list →
`palette.rs` split into a `PaletteDelegate` plus the `Command` table. Target: no list surface over
~500 lines.

**When NOT to.** A fixed list of under ~10 entries with no query is not a picker.

---

## discoverability

**Zed.** 240 `Tooltip::text(`, 201 `Tooltip::for_action…`; nothing hardcodes a shortcut string.

```rust
// zed/crates/ui/src/components/keybinding.rs:63
pub fn for_action(action: &dyn Action, cx: &App) -> Self { Self::new(action, None, cx) }
// :68 — like for_action, but matched from a specific context
pub fn for_action_in(action: &dyn Action, focus: &FocusHandle, cx: &App) -> Self { … }
```

backed by `window.highest_precedence_binding_for_action_in(..)`
(`zed/crates/gpui/src/window.rs:5897`).

**fleetd — matches, and is ahead.** The Help overlay and palette hints render from
`keymap::table()`; `palette::key_for(action)` (`crates/fleet-app/src/dialogs/palette.rs:627`)
resolves an action name to its keystroke, and `presentation::keys::humanize` spells action names
(`presentation/keys.rs:2`, re-exported from `fleet_lazygit::keymap`). Trade-off: it cannot show a
user-remapped key, which fleetd does not have.

---

## feedback

```rust
// zed/crates/workspace/src/notifications.rs:41-46
pub enum NotificationId { Unique(TypeId), Composite(TypeId, ElementId), Named(SharedString) }
```
```rust
// zed/crates/workspace/src/notifications.rs:157 — showing one first dismisses the same id
self.dismiss_notification(id, cx);
self.notifications.push((id.clone(), build_notification(cx)));
```

so `unique::<MyError>()` collapses repeats and `composite::<T>(path)` gives one-per-subject. A
toast is a notification (`:182`) plus an autohide branch (`:196`). The error-path ergonomic:

```rust
// zed/crates/workspace/src/notifications.rs:1633
pub trait DetachAndPromptErr<R> {
    fn prompt_err(self, msg: &str, window: &Window, cx: &App,
        f: impl FnOnce(&anyhow::Error, &mut Window, &mut App) -> Option<String> + 'static) -> Task<Option<R>>;
    fn detach_and_prompt_err(self, msg: &str, window: &Window, cx: &App, f: …);
}
```

blanket-implemented for `Task<anyhow::Result<R>>`, so any fallible task becomes
`task.detach_and_prompt_err("Failed to save", window, cx, |_, _, _| None);`. Zed's split:
blocking/must-answer → `window.prompt(PromptLevel::{Info,Warning,Critical}, ..)`; informational →
notification or toast.

**fleetd — partial, and stricter where it matters.**

```rust
// crates/fleet-app/src/state/notifications.rs:19-30 (trimmed)
pub fn push_toast(toasts: &mut Vec<LiveToast>, toast: Toast, now: Instant, dwell: Duration) {
    let mut toast = toast;
    if toast.tone == Tone::Danger { toast.tone = Tone::Warning; }   // errors are never toasts
    if let Some(existing) = toasts.iter_mut()
        .find(|live| live.toast.text == toast.text && now - live.shown_at <= TOAST_COALESCE_WINDOW)
    { existing.toast.count += 1; existing.expires_at = now + dwell; return; }
```

with `MAX_TOASTS` eviction (`:36-38`), `expire_toasts` (`:42-46`), `StickyError { text, job,
retryable }` (`:49-57`), and `ToastStack::MAX = 3` in the kit. The policy is stricter than Zed's
and stays: `docs/UX-SPEC.md:222` — *"A toast is allowed only when there is no row and no pill that
already shows the outcome"* — and `:239-241` — *"and any error — errors are sticky, never
transient."*

**Two gaps.**
1. **Identity.** Coalescing on rendered text is defeated by any interpolated id. Add a `ToastId`
   (`Named(&'static str)` / `Composite(&'static str, String)`), dedupe on it first, keep the 1 s
   text window as fallback.
2. **Fallible tasks.** 33 `.detach()` in `fleet-app`, 0 `detach_and_log_err`. Add the
   extension-trait shape on `Task<anyhow::Result<T>>` that logs *and* writes
   `AppState.sticky_error` (`crates/fleet-app/src/shell/root.rs:36-43` already has the setter
   `record_request_failure`).

**Do not adopt `window.prompt`.** `docs/UX-SPEC.md:965-967`: *"No OK/Cancel button pair anywhere —
the hint row states the keys, and this is a keyboard app."* Confirmations are in-app dialogs
(`Dialogs::Confirm` + `ConfirmRequest`, staged through `dialogs::request_confirm`,
`crates/fleet-app/src/dialogs/host.rs:141`).

---

## windows

**Zed.** One `build_window_options` function, stored as a fn pointer on `AppState`
(`zed/crates/workspace/src/workspace.rs:1123`) so tests swap in `|_, _| Default::default()`.
Field list at `zed/crates/gpui/src/platform.rs:1830`; `Default` (`:2001`) is
`focus: true, show: true`, which Zed overrides to `false` so the window is populated before it
appears. Bounds are saved debounced 100 ms off `cx.observe_window_bounds` and restored via a
three-way ladder (env override → this workspace's saved `(display, bounds)` → a global default →
`None`). Multi-window: `cx.windows()` → `Vec<AnyWindowHandle>`, `handle.downcast::<Root>()`,
`WindowHandle::update(cx, |root, window, cx| ..)`; the activation idiom is the pair
`cx.activate(true); window.activate_window();`. Single instance on macOS is a **pre-`run`** guard
(`zed/crates/zed/src/main.rs:366-389`). `cx.on_app_quit` (`zed/crates/gpui/src/app.rs:2345`) is for
work that must finish before exit — *"It is not possible to cancel the quit event at this point"*
— so the user-facing confirm lives in the `Quit` action instead.

**fleetd — one window, options inline.**

```rust
// crates/fleet-app/src/shell/root/bootstrap.rs:66-79 (trimmed)
let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
let options = WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(bounds)),
    window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
    titlebar: Some(TitlebarOptions {
        title: Some("Fleet".into()), appears_transparent: true,
        traffic_light_position: Some(gpui::point(cx.theme().space.md, cx.theme().space.md)),
    }),
    ..Default::default()
};
```

`DEFAULT_SIZE (1280,800)` `:9`, `MIN_SIZE (900,560)` `:10`. `window_background` and `app_id` are
unset. `cx.on_window_closed(|cx, _| if cx.windows().is_empty() { cx.quit() })` (`:59-64`) matches
Zed exactly. Quit runs through the quit dialog and `Shell::quit`/`quit_and_stop_daemon`, which is
the right placement; `cx.on_app_quit` has 0 uses and is only needed if something must flush on an
OS-initiated quit.

**Gaps, in priority order.** (1) Extract `fn window_options(cx: &mut App) -> WindowOptions` before
a second window exists. (2) No single-instance guard — a second `fleet` silently opens a second
app against the same daemon; put the guard before `gpui_platform::application()`. (3) Bounds are
never persisted; `cx.observe_window_bounds` is already subscribed
(`crates/fleet-app/src/shell/root/focus.rs:340-344`) but only calls `synchronize_surfaces` +
`cx.notify()`. The daemon already owns a config channel to store `(display_uuid, bounds)` in.

**Note.** fleetd builds `gpui` with `default-features = false`, so titlebar/decoration behaviour
may differ from Zed's — verify visually before trusting a Zed titlebar snippet.

---

## menus

```rust
// zed/crates/gpui/src/platform/app_menu.rs:76
pub enum MenuItem {
    Separator, Submenu(Menu), SystemMenu(OsMenu),
    Action { name: SharedString, action: Box<dyn Action>, os_action: Option<OsAction>,
             checked: bool, disabled: bool },
}
```

Menus are **rebuilt** from `app_menus(cx)`, never mutated — at startup and again from
`reload_keymaps` so accelerators re-resolve (`zed/crates/zed/src/zed.rs:2407-2408`). macOS editing
verbs must use `os_action` so the OS wires the responder chain
(`zed/crates/zed/src/zed/app_menus.rs:148`):
`MenuItem::os_action("Undo", editor::actions::Undo, OsAction::Undo)`, and likewise Cut/Copy/Paste.
`OsAction = Cut|Copy|Paste|SelectAll|Undo|Redo` (`app_menu.rs:311`). A minimal complete example is
`zed/crates/gpui/examples/set_menus.rs:93`.

**fleetd — one menu, one item.**

```rust
// crates/fleet-app/src/shell/root/bootstrap.rs:53-58
cx.set_menus(vec![Menu {
    name: "Fleet".into(),
    items: vec![MenuItem::action("Quit", fleet::Quit)],
    disabled: false,
}]);
```

0 `os_action`, no Edit menu. ~40 lines would add: an App menu (About, Settings, Quit, Quit & Stop
Daemon), a View menu mirroring the overlay toggles, and an Edit menu built from
`MenuItem::os_action(..)` so macOS text conventions work in the text fields. Everything in the
menu must be an action that also works from the keymap and the palette.

---

## settings

**Zed.** `cx.observe_global::<SettingsStore>(..)` (`zed/crates/gpui/src/app.rs:2080`), 75 uses,
in two shapes: blunt (`cx.observe_global::<SettingsStore>(Self::settings_changed)`, held in
`_subscriptions`) and diffed:

```rust
// zed/crates/zed/src/zed/quick_action_bar.rs:57 (trimmed)
let mut was_agent_enabled = AgentSettings::get_global(cx).enabled(cx);
cx.observe_global::<SettingsStore>(move |_, cx| {
    let is = AgentSettings::get_global(cx).enabled(cx);
    if was_agent_enabled != is { was_agent_enabled = is; cx.notify(); }
});
```

The generalized helper's doc states the contract: *"only fires when the value actually changes.
The returned [`Subscription`] must be retained for the callback to keep firing."* A windowed
variant exists: `cx.observe_global_in::<G>(window, ..)`.

**fleetd — diverges, correctly.** There is no `SettingsStore`: config is daemon-owned and fetched
over IPC behind a `seq` generation guard
(`crates/fleet-app/src/dialogs/settings/persistence.rs`), which is the right analogue of Zed's
diffed observer. The observed global is `Theme`
(`crates/fleet-ui-kit/src/theme/theme.rs`, `impl Global for Theme`), watched in the shell:

```rust
// crates/fleet-app/src/shell/root/focus.rs:334-337 (trimmed)
cx.observe_global_in::<fleet_ui_kit::Theme>(window, |shell, window, cx| {
    shell.synchronize_surfaces(window, cx);
    cx.notify();
})
```

retained in `Shell._subscriptions`. **Don't** observe and unconditionally `cx.notify()` from a view
with a large tree; diff first.
