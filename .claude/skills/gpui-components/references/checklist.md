# Review checklist — GPUI component design (fleetd)

Standalone. Scope: component *shape* and composition in `crates/fleet-ui-kit/src/components/`
and the render layer of `crates/fleet-app/src/{views,screens,dialogs}`. Token/colour/spacing
questions belong to the **gpui-styling** checklist; entity lifetime and task retention to
**gpui-state-and-memory**; actions, focus and key contexts to **gpui-app-shell**.

Answer each **yes** or **N/A**. A **no** is a change request.

## Tier choice

1. **Is every stateless thing a `#[derive(IntoElement)]` + `impl RenderOnce`, not an `Entity`?**
   Why: an entity per component costs a view allocation and notify bookkeeping, and makes cursor
   stability under background updates unreasonable (`docs/DESIGN-SYSTEM.md:350-355`).
   Fix: delete the entity; pass cursor/query/focus down each frame.
2. **Does every new `impl Render` name the state that must survive the frame?**
   Why: the kit's three exceptions (caret/IME, measured row heights) are documented per
   component; an undocumented fourth is indistinguishable from a mistake.
   Fix: one sentence in the module doc, or convert to `RenderOnce`.
3. **Does every hand-written `impl Element` carry a comment justifying custom layout or paint?**
   Why: fleetd has exactly two (`text_field/element.rs:33`, `multiline_input/element.rs:39`);
   a third without a reason is almost always a `RenderOnce` or a `canvas()`.
   Fix: add the reason, or drop down a tier.
4. **Does every state change inside a stateful component call `cx.notify()`?**
   Why: without it the view silently stops repainting (Zed `.rules:127`).
   Fix: add `cx.notify()` in the mutating path, not in `render`.

## API shape

5. **Does `new()` take identity plus required arguments only?**
   Why: optional positional parameters make call sites unreadable and every new knob a breaking
   change (`crates/fleet-ui-kit/src/components/badge.rs:34-59`).
   Fix: move the extras to `mut self -> Self` builders defaulting to `None`.
6. **Is text `impl Into<SharedString>`, are ids `impl Into<ElementId>`, are slots
   `impl IntoElement`, are handlers `impl Fn(..) + 'static`?**
   Why: `String` allocates per frame; a concrete closure type leaks into the public API.
   Fix: change the parameter type; store slots as `Option<AnyElement>`.
7. **Are handlers `Box<dyn Fn>` unless the component genuinely clones them into more than one
   element (then `Rc`/`Arc`, with a reason)?**
   Why: `Rc`/`Arc` by default hides whether the handler is shared; Zed's `ListItem` is `Box` for
   `on_click` and `Arc` for `on_toggle` precisely because only the latter is re-used
   (`zed/crates/ui/src/components/list/list_item.rs:48-50`).
   Fix: downgrade to `Box`, or add the comment.
8. **Does every component that owns interactivity, scroll or hover state take an `ElementId`,
   and thread it into every `.id()` it creates?**
   Why: gpui keys per-frame state by id, and an element with no id never enters the
   accessibility tree (`zed/crates/gpui/src/element.rs:106-111`).
   Fix: `new(id: impl Into<ElementId>, ..)`; rows key with `("prefix", index)`.
9. **Does a new method name duplicate a concept the kit already spells differently?**
   Why: the kit is a vocabulary; a second word for "disabled" costs every future reader.
   Fix: reuse the existing name, or lift it into `crates/fleet-ui-kit/src/traits/` and
   re-export from `prelude` (`crates/fleet-ui-kit/src/lib.rs:62`).
10. **Do container components take children through named slots rather than a
    `Vec<AnyElement>` parameter?**
    Why: `Vec<AnyElement>` forces the caller to erase every child and erases which region it
    belongs to (`crates/fleet-ui-kit/src/components/pane.rs:74-89`).
    Fix: `header`/`body`/`footer`-style builders taking `impl IntoElement`.

## Composition

11. **Are all conditionals `.when` / `.when_some` / `.when_else` / `.map`?**
    Why: an `if` returning `AnyElement` erases types, allocates and hides the tree's shape
    (`zed/crates/gpui/src/util.rs:10-61`, Zed `.rules:109`).
    Fix: rewrite as a FluentBuilder chain.
12. **Do optional children go through `.children(opt)` rather than `.when_some(..).child(..)`?**
    Why: `Option<T>` is already `IntoIterator`; the longer form is noise.
    Fix: `.children(opt)`.
13. **Does `into_any_element()` appear only where branches genuinely differ in type?**
    Why: each call boxes; a chain that never branches should stay concretely typed.
    Fix: return `impl IntoElement`.
14. **Is the fragment still in the right place — free `fn render` vs kit component?**
    Why: reuse across screens or more than ~4 parameters is the promotion threshold
    (`docs/APP-CONTRACTS.md:50-72`).
    Fix: promote to `#[derive(IntoElement)]` in `fleet-ui-kit`, converting domain types to
    `SharedString`/scalars at the boundary (`crates/fleet-ui-kit/src/lib.rs:12-13`).
15. **Does the component take any `fleet-core` / `fleet-proto` type?**
    Why: the kit depends on `gpui` and nothing else; that boundary is a crate boundary, not a
    convention.
    Fix: convert in the app, pass `SharedString`, scalars and closures.

## Lists

16. **Are the rows actually uniform height?**
    Why: `uniform_list` measures the first row and lays out the rest from it
    (`zed/crates/gpui/src/elements/uniform_list.rs:1-5`); a variable row silently misaligns.
    Fix: fix the row height, or flag the case for `list(ListState)` — unused in fleetd so far.
17. **Does the caller own the scroll handle and pass it in?**
    Why: a handle created inside `render` resets scroll position on every frame
    (`crates/fleet-ui-kit/src/components/list_view.rs:385-395`).
    Fix: hold `UniformListScrollHandle` in the screen struct and pass it to `.scroll(..)`.
18. **Is a terminal grid being fed through `uniform_list`/`list`?**
    Why: ADR `docs/decisions/0002-terminal-emulation.md` forbids it; the grid is a `canvas()`
    with `shape_line(.., force_width)`.
    Fix: use `TerminalGrid`.

## Keyboard and accessibility (fleetd-specific)

19. **Does every new affordance have a visible `KeyHint`?**
    Why: "There are no buttons anywhere in Fleet. Affordances are `KeyHint`s"
    (`docs/DESIGN-SYSTEM.md:39`).
    Fix: add a `KeyHint`/`KeyHintRow` (`crates/fleet-ui-kit/src/components/key_hint.rs:22-49`).
20. **Does the hint obey §4 — lowercase safe / uppercase stronger, `^s`-prefixed inside the
    Workspace, `ctrl-n`/`ctrl-p` under a text field, invalid commands unlisted?**
    Why: `docs/DESIGN-SYSTEM.md:262-278`; bare keys inside the Workspace leak into the PTY.
    Fix: prefix the hint; use `FactList::confirm_key()` / `FuzzyList::binds_jk()` instead of
    hand-deciding.
21. **Was a tooltip or a button added to the kit?**
    Why: both are on the "deliberately not in the kit" list (`docs/DESIGN-SYSTEM.md:1232-1233`).
    Fix: remove; express the affordance as a key.
22. **Does anything that sets a `gpui::Role` also set an accessible name?**
    Why: a role with nothing to announce is worse than no role;
    `crates/fleet-ui-kit/src/components/control.rs:32` is the existing offender.
    Fix: add the name while you are in the file.

## Files, docs and preview

23. **Does the component's doc comment say what it is *and* when to use the sibling instead?**
    Why: that sentence is what stops the kit growing a fifth near-duplicate;
    `crates/fleet-ui-kit/src/components/badge.rs:1-5` is the model.
    Fix: add it — `#![warn(missing_docs)]` catches absence, not vagueness.
24. **Does every new visual state appear in the matching `examples/gallery_*.rs` panel?**
    Why: "If a state is not in a gallery, it is not implemented"
    (`docs/DESIGN-SYSTEM.md:1251-1252`).
    Fix: add the panel via `GalleryLayout::section`/`::labeled`
    (`crates/fleet-ui-kit/examples/support/layout.rs:11-75`).
25. **Is there a `docs/DESIGN-SYSTEM.md` §6 catalog entry, in this commit?**
    Why: the change protocol requires code and doc to move together
    (`docs/DESIGN-SYSTEM.md:1242-1255`).
    Fix: add the entry; pick the §6.x subsection matching the gallery group.
26. **Is the file one concept, exported explicitly, with no new `mod.rs`?**
    Why: explicit `pub use` keeps the public surface reviewable
    (`crates/fleet-ui-kit/src/components/mod.rs:78-162`); `mod.rs` paths are ungreppable
    (Zed `.rules:14`).
    Fix: `x.rs` beside `x/`; add the `pub use` line; rename a `mod.rs` you were already editing.
27. **Did the acceptance gate run?**
    Why: `cargo check -p fleet-ui-kit --examples` and
    `cargo run -p fleet-ui-kit --example kit_gallery` are the stated gate
    (`docs/DESIGN-SYSTEM.md:1253-1254`), then `make lint` and `make test`.
    Fix: run them; a gallery that no longer compiles is a broken acceptance test.
