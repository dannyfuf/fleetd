# App-shell review checklist (gpui-app-shell)

Standalone: usable without loading `SKILL.md`. Apply to any diff touching
`crates/fleet-app/src/actions.rs`, `crates/fleet-app/src/keymap.rs`,
`crates/fleet-app/src/dialogs/**`, `crates/fleet-app/src/shell/**`, toasts / the sticky error,
window or menu setup, or `docs/KEYMAP.md`.

Answer each **yes / no / n-a**. A "no" is a finding: report it as
`<file>:<line> — <question> — <fix>`.

---

## Actions

1. **Is every new action declared with `actions!(namespace, [..])` (or `#[derive(Action)]` when
   it carries data) in `crates/fleet-app/src/actions.rs`?**
   Why: `actions.rs` is the app's one dependency-free action module; declaring elsewhere creates
   cycles and hides the verb from `keymap.rs`. Fix: move the declaration into the namespace that
   owns the surface.

2. **Does every new action carry a `///` doc comment naming its keystroke, written for a user?**
   Why: GPUI displays action doc comments to users, and `actions.rs:3` states "exactly one action
   per row of `docs/KEYMAP.md`". Fix: add `/// \`y\` — copy the highlighted worktree's path.`

3. **Is the namespace the one that matches the surface, with no duplicate `namespace::Name`
   anywhere in the process (including `fleet-lazygit`)?**
   Why: action names are process-wide via `inventory`; registering two actions with the same name panics during `App` creation.
   Fix: rename, or use the `lg_*` convention for embedded-pane namespaces
   (`docs/APP-CONTRACTS.md:174-182`). The `keymap.rs` test suite covers this — run `make test`.

4. **Does a *renamed* action carry `#[action(deprecated_aliases = ["old::Name"])]`?**
   Why: an in-flight keymap or script referencing the old name breaks silently. Fix: add the
   attribute (0 uses in the repo today — this is a new convention).

5. **Is the new verb actually user-addressable, rather than an internal message?**
   Why: actions are the user surface; internal signalling belongs in `AppState` + `cx.notify()`.
   Fix: drop the action and mutate state directly.

## Keymap and contexts

6. **Is every new binding a `key_table!` row in `keymap.rs:142-605`, with an explicit context?**
   Why: a binding declared anywhere else escapes `table()`, so the Help overlay and palette hints
   will not show it. Fix: add the row; do not call `cx.bind_keys` elsewhere.

7. **Is the context word globally unique across `fleet-app` and `fleet-lazygit`?**
   Why: gpui's `>` is a *subsequence* test, not a parent test, so a reused word makes the binding
   fire inside surfaces it does not belong to. Fix: pick a new word; the
   `the_embedded_pane_shares_no_context_word_with_the_app` test enforces it.

8. **Is the predicate as deep as the surface allows, and is there no context-less binding?**
   Why: a binding with **no** context is treated as the *deepest* context and shadows everything
   (`zed/crates/gpui/src/keymap.rs:154-156`). Fix: name the deepest context that is always present
   for that surface.

9. **Was `docs/KEYMAP.md` changed in the same commit?**
   Why: `docs/KEYMAP.md:12-13` — "This file is the single source of truth for keys … where the two
   disagree, this file wins." Fix: add or update the row.

10. **If the code deliberately deviates from `docs/KEYMAP.md`, is the deviation written down?**
    Why: three such arbitrations already live at `docs/APP-CONTRACTS.md:225-236` and
    `keymap.rs:28-31`; a fourth silent one is a bug. Fix: record it in both places.

11. **Does the change avoid `KeyBinding::new`?**
    Why: it `.unwrap()`s the context-predicate parse. Fix: use
    `KeyBindingContextPredicate::parse` + `KeyBinding::load(.., &DummyKeyboardMapper)` and log the
    error, as `keymap.rs:121-136` already does.

12. **Does anything that displays a shortcut render it from `keymap::table()` (or
    `palette::key_for`) rather than a hardcoded string?**
    Why: hardcoded hints drift from the table. Fix: look the key up.

## Handlers and dispatch

13. **Is every handler `.on_action(..)` on an element, with `cx.on_action` still at zero in
    production code?**
    Why: `cx.on_action` fires with nothing focused, so two surfaces sharing a verb fight. Fix:
    move it onto the element that calls `.track_focus(focus)`.

14. **Is the listener on the element that tracks the focus handle (not a descendant, not a
    sibling)?**
    Why: gpui dispatches from the focused node upward; a listener below the focus node never
    fires (`docs/APP-CONTRACTS.md:107-112`). Fix: hang it on the root returned by
    `dialogs::root(focus)` / the screen's root.

15. **Does a handler that does only part of the work end in `cx.propagate()`, with a comment
    saying who finishes it?**
    Why: gpui stops at the first bubble listener and "the failure is silent: the key is consumed
    and nothing else happens" (`docs/APP-CONTRACTS.md:456-464`). Fix: add the call and the comment.

16. **Does a handler that fully consumes a verb an outer listener also binds call
    `cx.stop_propagation()` explicitly?**
    Why: both directions must be stated, not inherited. Fix: add the call.

## Focus

17. **Does every new screen/dialog root call `.track_focus(focus)` on the handle it was given?**
    Why: it is what puts the listeners on the dispatch path (`docs/APP-CONTRACTS.md:107-112`).
    Fix: return `dialogs::root(focus)` (or `div().track_focus(focus)`) as the root.

18. **Is focus granted outside `render` — by changing the state `focus_target` reads, never by
    calling `window.focus(..)` from a listener or a render function?**
    Why: the handle is not in the focus tree until the element exists, and the generation gate
    owns the transition. Fix: mutate `AppState` and let `focus_surface`
    (`crates/fleet-app/src/shell/root/focus.rs:414-431`) apply it.

19. **If the change introduces a new authoritative keyboard owner, does it appear in
    `focus_owner` (`focus.rs:129`) and `focus_target` (`focus.rs:382-411`)?**
    Why: an owner `FocusOwnerKeys` cannot see never bumps the generation, and the first keystroke
    after the transition can be dropped. Fix: extend both functions.

20. **Does the change avoid adding a second `intercept_keystrokes`, `capture_key_down`, or
    focus observer, and avoid restructuring `FocusOwnerKeys`?**
    Why: the generation gate, the 64-entry FIFO and the `usize::MAX` pointer gate are bespoke,
    interdependent and documented (`docs/APP-CONTRACTS.md:210-222`). Fix: extend the existing
    gates at `focus.rs:153-200` and `shell/root/actions.rs:224-236`.

21. **Does a new deferred overlay use a priority *below* `POINTER_GATE_PRIORITY`
    (`focus.rs:16`)?**
    Why: the pointer gate must stay last so stale-frame mouse events are still dropped. Fix:
    lower the priority.

22. **Is the surface fully operable from the keyboard — next / previous / first / last, confirm,
    cancel all do something sensible, and nothing focusable is unreachable?**
    Why: fleetd is a keyboard app; a mouse-only affordance is a defect. Fix: bind the missing
    verbs.

## Dialogs and overlays

23. **Does a new dialog reuse the existing single overlay slot (`AppState.overlay`) rather than
    adding a parallel visibility flag?**
    Why: one slot is the invariant that makes key contexts and focus deterministic. Fix: add a
    `Dialogs` variant.

24. **Does the dialog close through one owned path, not by mutating `AppState.overlay` at each
    call site?**
    Why: every call site otherwise has to remember the veto, the draft reset and focus hand-back.
    Fix: route through `dialogs::dismiss(state, cx)`.

25. **If an operation must not be interrupted, does the dialog veto dismissal instead of leaving
    `Esc` unbound?**
    Why: an unbound key silently does nothing and the user cannot tell why; Zed's
    `on_before_dismiss -> DismissDecision` exists for this
    (`zed/crates/workspace/src/modal_layer.rs:49-56`). Fix: return `Keep` and show a toast saying
    why.

26. **Does the new `Dialogs` variant have a `context_name()` word bound in `keymap.rs`, a
    `width()`, a `seed` and a `render` arm, and a draft field on `DialogHost`?**
    Why: the enum fans out across those places (`dialogs/mod.rs:92`, `:114`, `:140`, `:172`;
    `dialogs/host.rs:17-57`) and a missing arm compiles but renders nothing. Fix: complete all
    five; the `every_dialog_has_a_key_context_word` test covers one of them.

27. **Do the dialog's frame, widths and footer follow `docs/UX-SPEC.md` §3.8 (no OK/Cancel button
    pair, `Esc` closes, key hints in the footer)?**
    Why: `docs/UX-SPEC.md:965-967` is explicit. Fix: use the shared width constants in
    `dialogs/mod.rs:40-47` and the kit's dialog frame.

28. **Does a trigger-anchored popover (if any) use
    `deferred(anchored().snap_to_window_with_margin(..))` rather than a new overlay slot?**
    Why: it must stay inside the window and above siblings without stealing the modal slot. Fix:
    use those two elements — but first confirm the popover is warranted at all (fleetd has 0
    `anchored()` uses, deliberately).

## Lists and pickers

29. **Does a new filtering list run its matching on `cx.background_executor()` and apply results
    from a retained `Task`?**
    Why: `docs/APP-CONTRACTS.md:101-103` — "Render prepares nothing" — and synchronous matching in
    the notify path degrades as the list grows. Fix: an `update_matches(..) -> Task<()>`, retained
    with `dialogs::host::retain_task`.

30. **Is the selected row read through a clamped accessor returning `Option`, never `rows[cursor]`
    or `matches[selected]`?**
    Why: a background refresh can shrink the list under a stale cursor. Fix: clamp with
    `dialogs::step` / `FuzzyList::next_cursor` and return `Option`.

31. **Does the list reuse the shared navigation vocabulary rather than declaring its own cursor
    actions — and if it cannot, does the PR say why?**
    Why: per-surface nav actions multiply keymap rows and diverge in behaviour. Fix: use
    `actions::nav::*` bound against the `List` context.

32. **Does it respect `FuzzyList::binds_jk` (a list under a text field moves with `ctrl-n`/`ctrl-p`
    or arrows, never `j`/`k`)?**
    Why: otherwise the query becomes untypable — the same trap that unbound `q` in the palette.
    Fix: bind the right pair for the case.

33. **Does the query field keep focus while the arrows move the selection?**
    Why: typing must never be interrupted by navigation. Fix: keep focus on the dialog root and
    route keys to the draft.

34. **Is the new list under ~500 lines, or does it reuse an existing delegate/driver?**
    Why: Zed keeps half of 48 picker delegates under 500 lines; `palette.rs` at 2 046 is the
    counter-example this repo already pays for. Fix: split the candidate table from the driver.

## Feedback and errors

35. **Does new transient feedback have a stable identity (a `ToastId`-style key), not just
    matching text?**
    Why: text coalescing (`state/notifications.rs:22-30`) is defeated by any interpolated value.
    Fix: dedupe on an explicit id first.

36. **Does the toast pass the §2.7 law — no row and no pill already shows the outcome?**
    Why: `docs/UX-SPEC.md:222`. Fix: delete the toast and let the row show it.

37. **Do errors go to the sticky slot rather than a toast?**
    Why: `docs/UX-SPEC.md:239-241` — "any error — errors are sticky, never transient"; `push_toast`
    even downgrades `Tone::Danger` to `Warning` so a caller cannot smuggle one in. Fix: set
    `AppState.sticky_error`.

38. **Does every fallible task surface its error instead of ending in `.detach()` or `let _ =`?**
    Why: a dropped `Result` means the user sees nothing happen. Fix: route through a
    logging/sticky-error helper on `Task<anyhow::Result<T>>`.

39. **Does the change avoid `window.prompt` / `PromptLevel`?**
    Why: `docs/UX-SPEC.md:965-967` forbids an OK/Cancel pair anywhere. Fix: use `Dialogs::Confirm`
    with `dialogs::request_confirm` (`dialogs/host.rs:141`).

## Shell, windows and menus

40. **Does a new subsystem expose `init(cx)` and sit at the right position in
    `shell/root/bootstrap.rs:43-89`?**
    Why: ordering constraints must be readable in one list. Fix: move it there out of
    `Shell::new`.

41. **Do `WindowOptions` come from one place, and does a second window (if added) reuse it?**
    Why: the literal is inline at `bootstrap.rs:66-79` today; duplicating it makes the two windows
    diverge. Fix: extract `fn window_options(cx: &mut App) -> WindowOptions`.

42. **Is anything added to the OS menu also reachable from the keymap and the palette?**
    Why: a menu-only verb is invisible in a keyboard app. Fix: add the action + binding, and use
    `MenuItem::os_action(..)` for macOS editing verbs so the responder chain works.

43. **Is work that must finish before exit on `cx.on_app_quit` and retained in `_subscriptions`,
    with the user-facing confirmation left in the quit action?**
    Why: `cx.on_app_quit` cannot cancel the quit; the confirm belongs in the action, as fleetd
    already does. Fix: split the two.

44. **Are new subscriptions and tasks retained in `Shell._subscriptions` / `_tasks`
    (`shell/root.rs:71-72`) or a `DialogHost` task slot?**
    Why: a dropped `Subscription` stops firing and a dropped `Task` is cancelled. Fix: store it.

## Docs

45. **Were `docs/KEYMAP.md`, `docs/APP-CONTRACTS.md` and/or `docs/UX-SPEC.md` updated in the same
    commit as the contract they describe?**
    Why: `docs/README.md` assigns each document authority over a domain, and a change that
    contradicts a doc is a bug in one of the two. Fix: update both.

## Mechanical gates

```sh
make lint   # fmt-check + clippy -D warnings
make test   # builds fleetd first; runs the keymap/dialog/context tests
```

Both must pass before the change is done. Commit style: `app: <imperative lowercase summary>`.
