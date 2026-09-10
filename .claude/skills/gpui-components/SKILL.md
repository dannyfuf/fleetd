---
name: gpui-components
description: How to design, name and compose GPUI components in fleetd — choosing RenderOnce vs Render vs raw Element, builder-API and argument conventions, trait vocabulary, named slots, FluentBuilder composition, list primitives, fleet-ui-kit file/export conventions, and the gallery-as-acceptance-test rule. Load it before adding or changing anything under `crates/fleet-ui-kit/src/components/`, before writing a `render`/`render_*` function or a `Props` struct in `crates/fleet-app/src/views|screens|dialogs`, when deciding whether a view fragment should become a kit component or stay a free function, when adding a list/row/slot/builder method, or when a review asks "is this the right component shape?". Do not load it for theme tokens, colors, spacing or elevation — that is `gpui-styling`.
---

# GPUI components in fleetd

Component design and composition for `fleet-ui-kit` and `fleet-app`: which rendering tier a
thing belongs to, what its constructor and builders look like, how it takes children, and how
it proves it works. Patterns verified against Zed v1.18.1, the GPUI tag fleetd depends on
(`Cargo.toml`, ADR `docs/decisions/0001-gpui-and-toolchain.md`).

`docs/DESIGN-SYSTEM.md` is authoritative for this area — §6 is the component catalog, §7 lists
what is deliberately absent, §8 is the change protocol. Code and doc move in the same commit.

## When to use

- Adding or changing a component under `crates/fleet-ui-kit/src/components/`.
- Writing a `render`/`render_*` free function or a `Props` struct in `crates/fleet-app/src/`.
- Deciding: free `fn render` in the app, or a `#[derive(IntoElement)]` struct in the kit?
- Deciding: stateless builder, `Entity` + `Render`, or a hand-written `impl Element`?
- Adding a list, a row, a slot, a builder method, or an `ElementId` to an interactive thing.
- Reviewing a UI PR ("is this the right component shape?").

## When not to

- Theme tokens, colors, `Tone`, `TextRole`, spacing, radii, elevation, motion, icons → **gpui-styling**.
- Entities, subscriptions, `Task` ownership, `WeakEntity`, globals → **gpui-state-and-memory**.
- Actions, keymaps, focus routing, modal/dialog hosting, notifications → **gpui-app-shell**.

## Rules

1. **`#[derive(IntoElement)]` + `impl RenderOnce` is the default tier.** A component is a
   struct of settings whose `render` consumes `self`; no entity, no `cx.notify()` bookkeeping,
   the caller owns the truth and passes it down each frame (Zed `.rules:105`). fleetd already
   does this: 73 `RenderOnce` impls in `fleet-ui-kit` against 3 production `Render`
   (`crates/fleet-ui-kit/src/components/badge.rs:24,62` is the model). Keep doing it.
2. **`impl Render` on an `Entity` only when state must outlive the frame.** Focus handle, caret
   or IME, a text buffer, a measured-height cache, subscriptions. fleetd's three production
   exceptions are documented (`docs/DESIGN-SYSTEM.md:1057-1061`, §6.6): `TextInput`
   (`crates/fleet-ui-kit/src/components/text_field/input.rs:210`), `MultilineInput`
   (`multiline_input/input.rs:557`), `TranscriptList` (`agent/transcript_list.rs:620`). A new
   one needs a sentence in its module doc saying which of those reasons applies.
3. **Hand-write `impl Element` only for custom layout or paint, and say why in a comment.**
   Repo-wide fleetd has two, both text inputs (`text_field/element.rs:33`,
   `multiline_input/element.rs:39`); Zed has 3 in all of `crates/ui`. Anything else is a
   `RenderOnce` or a `canvas()`.
4. **`new()` takes identity and required arguments only; every other knob is `mut self -> Self`.**
   Optional fields are `Option<T>` initialised to `None` in `new` and set by a same-named
   builder. `crates/fleet-ui-kit/src/components/badge.rs:34-59` and
   `list_view.rs:290-308` are the house examples.
5. **Use borrow-friendly argument types at the API boundary.** Text is
   `impl Into<SharedString>`, ids are `impl Into<ElementId>`, element slots are
   `impl IntoElement` stored as `Option<AnyElement>`, handlers are `impl Fn(..) + 'static`
   boxed as `Box<dyn Fn>` (use `Rc`/`Arc` only when the handler is genuinely cloned into more
   than one element, and say so). Never `String`, never a concrete closure type.
6. **Anything that owns interactivity, scroll or hover state takes an `ElementId` and threads
   it into every `.id()` it creates.** List rows key with the `("prefix", index)` tuple.
   fleetd is partial here: only ~4 kit constructors take an id first
   (`list_view.rs:294`, `spinner.rs:22,71`, `kanban_column.rs:45`); several add it later as
   `.id(..)` (`chip.rs:101`, `row.rs:158`, `terminal_grid.rs:85`). New interactive components
   take it in `new`.
7. **Name a concept once, in a trait, not once per component.** fleetd has zero shared
   component traits today — 5 ad-hoc `pub fn disabled(` and one `pub fn on_click(` across the
   kit. Before inventing a method name, check whether the kit already spells that idea; when
   you add the second spelling of one, lift it into `crates/fleet-ui-kit/src/traits/` and
   re-export from `prelude` (`crates/fleet-ui-kit/src/lib.rs:62`). Zed's whole vocabulary is
   six tiny traits. Note: at v1.18.1 the trait is `Toggleable` with a tri-state `ToggleState`;
   `Selectable` does not exist.
8. **Conditionals go through `FluentBuilder`, never an `if` that returns `AnyElement`.**
   `.when` / `.when_some` / `.when_else` / `.map` (`zed/crates/gpui/src/util.rs:10-61`,
   Zed `.rules:109`). `Option<T>` is `IntoIterator`, so an optional child is `.children(opt)` —
   `.when_some(x, |t, x| t.child(x))` is redundant. fleetd already does this
   (`badge.rs:74-81`); keep it.
9. **Children arrive through named slots, not a `Vec<AnyElement>` parameter.** fleetd's
   convention is `header`/`body`/`footer` builders taking `impl IntoElement`
   (`crates/fleet-ui-kit/src/components/pane.rs:74-89`, `dialog.rs:92-104`). Named slots beat
   `ParentElement` here because the kit's surfaces have fixed regions with different rules;
   only reach for `ParentElement` delegation when a container genuinely accepts a homogeneous
   run of children.
10. **Promote a view fragment to a kit component when it is reused across screens or takes
    more than ~4 parameters.** Free `fn render(props: &XProps, .., cx) -> AnyElement` is the
    documented `fleet-app` shape (`docs/APP-CONTRACTS.md:50-72`,
    `crates/fleet-app/src/views/board_screen.rs:72-78`) and is fine for one-off screen
    composition — Zed itself has 854 `render_*` helpers. The `Props` struct is the local
    convention for "everything one frame needs, borrowed"; when the same struct is borrowed by
    two screens, it wants to be a component.
11. **Pick the list primitive deliberately and let the caller own the scroll handle.**
    Uniform-height rows → `ListView` / `LogView`, which wrap `uniform_list`
    (`crates/fleet-ui-kit/src/components/list_view.rs:385-393`). `uniform_list` measures the
    first row and lays out the rest from it (`zed/crates/gpui/src/elements/uniform_list.rs:1-5`),
    so heterogeneous heights must not be forced into it — flag them and reach for
    `list(ListState)` (`zed/crates/gpui/src/elements/list.rs:1-8`), which fleetd does not use yet.
    A small fixed set is plain `div().children(..)`. Terminal rows use neither (ADR 0002).
12. **Affordances are `KeyHint`s. Do not add buttons or tooltips.**
    `docs/DESIGN-SYSTEM.md:39` ("There are no buttons anywhere in Fleet") and `:1232-1233`
    ("Tooltips. Every affordance already states its key"). Zed's "nearly all interactable
    elements should have a tooltip" rule (`zed/crates/ui/src/components/button/button_like.rs:35-39`)
    is explicitly rejected here. The fleetd analogue: a new affordance ships with a visible
    `KeyHint` (`crates/fleet-ui-kit/src/components/key_hint.rs:22-49`) obeying
    `docs/DESIGN-SYSTEM.md` §4 (lowercase safe / uppercase stronger; prefixed `^s x` inside the
    Workspace). And a `gpui::Role` with no accessible name is worse than no role —
    `crates/fleet-ui-kit/src/components/control.rs:32` sets `Role::Button` with no name; do not
    add more of those.
13. **A component is not done until it is documented and previewed.** The doc comment says what
    it is *and when to use the sibling instead* (`badge.rs:1-5` is the model). Then, in the same
    commit: a panel in the matching `crates/fleet-ui-kit/examples/gallery_*.rs` showing **every**
    state, and a §6 entry in `docs/DESIGN-SYSTEM.md` — the change protocol is spelled out at
    `docs/DESIGN-SYSTEM.md:1242-1255`, and "if a state is not in a gallery, it is not
    implemented" (`:1251-1252`).
14. **One concept per file, explicit re-exports, no new `mod.rs`.** A file may hold a small
    family; a family that grows gets a directory. fleetd's explicit
    `pub use badge::{Badge, BadgeStyle};` form (`crates/fleet-ui-kit/src/components/mod.rs:81`)
    is better than Zed's glob — keep it. fleetd has 31 `mod.rs` files against Zed's ban
    (`.rules:14`); do not add the 32nd, and rename the one you are already editing.

## Core patterns

Full catalog with Zed citations: `references/patterns.md`.

### 1. The default component ([patterns.md#p1](references/patterns.md#p1-stateless-component))

```rust
/// `StaleStamp` — how old a mirrored snapshot is. Not a `FreshnessStamp`: no refresh spinner.
#[derive(IntoElement)]
pub struct StaleStamp {
    age: SharedString,
    tone: Tone,
    frozen: bool,
}

impl StaleStamp {
    /// A stamp for an age already formatted by the caller.
    pub fn new(age: impl Into<SharedString>) -> Self {
        Self { age: age.into(), tone: Tone::Muted, frozen: false }
    }

    /// Mark the snapshot as frozen rather than merely old.
    pub fn frozen(mut self, frozen: bool) -> Self {
        self.frozen = frozen;
        self
    }
}
```

### 2. Named slots ([patterns.md#p2](references/patterns.md#p2-named-slots))

```rust
impl Pane {
    /// The 30 px header, normally a [`super::PaneHeader`].
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }
}

// caller
Pane::new().header(PaneHeader::new("Worktrees")).body(list).focused(true)
```

### 3. FluentBuilder composition ([patterns.md#p3](references/patterns.md#p3-fluentbuilder))

```rust
impl RenderOnce for StaleStamp {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .when(self.frozen, |el| el.opacity(theme.metrics.dimmed_opacity))
            .child(Text::hint(self.age).color(self.tone.color(theme)))
            .children(self.badge)          // Option<Badge>: renders only when Some
    }
}
```

`.children(opt)` replaces `.when_some(opt, |el, x| el.child(x))`. Use `.map(|el| ..)` with an
explicit `if` only when the two branches produce different element types.

### 4. When a component earns an entity ([patterns.md#p4](references/patterns.md#p4-stateful))

```rust
// Only because it owns a caret and an IME preedit that must survive frames.
impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .child(TextInputElement::new(cx.entity().clone()))
    }
}
```

Every state change inside such a component calls `cx.notify()`. Lifetimes, weak handles and
task retention are **gpui-state-and-memory**'s subject.

### 5. Lists ([patterns.md#p5](references/patterns.md#p5-lists))

```rust
ListView::new("worktrees", rows.len(), move |ix, is_cursor, window, cx| {
    worktree_row(&rows[ix], is_cursor, window, cx).into_any_element()
})
.cursor(cursor)
.scroll(scroll.clone())     // the caller owns the handle, so scroll survives re-render
```

`ListView` passes `(index, is_cursor)` so a row never compares indices itself
(`crates/fleet-ui-kit/src/components/list_view.rs:290-296`). Uniform heights only.

### 6. A view fragment stays a free function ([patterns.md#p6](references/patterns.md#p6-fragments))

```rust
// crates/fleet-app/src/views/<x>.rs — the documented fleet-app shape
pub(crate) fn render(props: &BoardProps<'_>, scroll: &ScrollHandle, cx: &App) -> AnyElement
```

Return `AnyElement` when branches differ in type, `impl IntoElement` otherwise. Cross a screen
boundary or a fifth parameter and it becomes a `#[derive(IntoElement)]` struct in the kit.

### 7. Lifting a repeated method into a trait ([patterns.md#p7](references/patterns.md#p7-traits))

```rust
// crates/fleet-ui-kit/src/traits/disableable.rs (proposed; the kit has no traits/ yet)
/// A component that can be shown as unavailable rather than removed.
pub trait Disableable {
    /// Render the component as unavailable.
    fn disabled(self, disabled: bool) -> Self;
}
```

Add the trait only on the second spelling of the same idea, and re-export it from `prelude`.

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
| --- | --- | --- |
| `Entity` + `Render` for a stateless fragment | Allocates a view, adds notify bookkeeping, breaks cursor stability under background updates (`docs/DESIGN-SYSTEM.md:350-354`) | `#[derive(IntoElement)]` + `RenderOnce` |
| `impl Element` for ordinary UI | You reimplement layout, hit-testing and accessibility by hand | `RenderOnce`, or `canvas()` when you really paint |
| `fn new(a, b, c, d, e, f)` | Call sites become positional puzzles; adding a knob breaks every caller | `new(id, required)` + `mut self -> Self` builders |
| `label: String` / `on_click: MyClosure` | Forces an allocation per frame and leaks the concrete type into the API | `impl Into<SharedString>`, `impl Fn(..) + 'static` |
| Interactive component with no `ElementId` | gpui cannot keep hover/scroll/focus state across frames, and it never enters the accessibility tree | `new(id: impl Into<ElementId>, ..)`, rows keyed `("prefix", ix)` |
| `if cond { a.into_any_element() } else { b.into_any_element() }` | Erases types, allocates, hides the shape of the tree | `.when` / `.when_else` / `.map` |
| `.when_some(opt, \|el, x\| el.child(x))` | Reinvents what `Option: IntoIterator` already does | `.children(opt)` |
| `fn new(children: Vec<AnyElement>)` | The caller must erase and box every child; regions lose their meaning | Named slots taking `impl IntoElement` |
| A component whose new state has no gallery panel | The gallery is the acceptance test; an unpreviewed state is unimplemented (`docs/DESIGN-SYSTEM.md:1251`) | Panel + §6 catalog entry in the same commit |
| Adding a tooltip or a button to the kit | Contradicts §7 of the design system; Fleet is keyboard-first | A `KeyHint` obeying §4 |
| A new `mod.rs` | Unnameable, ungreppable module path (Zed `.rules:14`) | `x.rs` beside `x/` |
| Ad-hoc method names for shared ideas (`is_off`, `inactive`, `greyed`) | The kit stops being a vocabulary | One trait in `traits/`, re-exported from `prelude` |

## fleetd-specific guidance

**Where things live.** `crates/fleet-ui-kit/src/components/` (95 files, ~70 exported types) is
the vocabulary; `crates/fleet-app/src/views|screens|dialogs` compose it and never style by hand
(`docs/DESIGN-SYSTEM.md:9-12`). The kit depends on `gpui` and nothing else — no `fleet-core`
type may enter a component signature (`crates/fleet-ui-kit/src/lib.rs:12-13`). Galleries live in
`crates/fleet-ui-kit/examples/gallery_{structure,data,input,terminal,agent,board}.rs`, with
`kit_gallery.rs` as the combined overview.

**Already at or above Zed's bar — keep doing it, do not "fix" it.** The `RenderOnce`-first kit;
explicit `pub use` instead of globs; `impl Into<SharedString>` arguments; named slots;
`FluentBuilder` conditionals; `#![warn(missing_docs)]` (`lib.rs:41`); `ListView` passing
`(ix, is_cursor)`, which is better than Zed's raw `uniform_list`; the compile-checked
`lucide_icons!` set; the `Tone`/`TextRole` narrowing over Zed's ~20-variant `Color`.

**Concrete gaps in this area, in priority order.**
1. *No in-file previews.* 7,749 lines of `examples/gallery_*.rs` sit apart from the components
   they exercise, so a new variant can ship without a preview and the diff will not show it.
   Zed keeps `preview()` in the component's own file via `RegisterComponent`
   (`zed/crates/component/src/component.rs:25-61`). Incremental port: first move
   `GalleryLayout::section`/`::labeled`/`strip`
   (`crates/fleet-ui-kit/examples/support/layout.rs:11-75`) out of `examples/` into the crate so
   a component *can* build its own preview; adopt a registry later, and only if `inventory`
   builds on the pinned toolchain — a hand-written `register_all(cx)` is a zero-dependency
   substitute.
2. *No trait vocabulary.* `crates/fleet-ui-kit/src/traits/` does not exist; the only kit traits
   are `ActiveTheme` and `FuzzyCursorSource`. Add traits one at a time, when a second component
   needs the same verb — not as a big-bang port of Zed's six.
3. *`ElementId` not taken first.* Audit only the interactive components you touch (rule 6).
4. *Roles without names.* `control.rs:32` sets `Role::Button` and the kit has zero accessible
   names. When you touch such a component, give it one.
5. *31 `mod.rs` files*, including `crates/fleet-ui-kit/src/components/mod.rs` and
   `crates/fleet-app/src/views/mod.rs`. Rename opportunistically, in the commit that already
   touches the module; never as a standalone churn PR.
6. *App-side render cost.* `crates/fleet-app/src/views/board_screen.rs:88` recomputes
   `grouped_cards(..)` every frame while only the Hub memoises
   (`crates/fleet-app/src/screens/hub/projection.rs:4-32`). Component shape will not fix that —
   see **gpui-performance**.

**Migration posture.** fleetd's one-`Entity<AppState>` architecture with free `render` functions
is deliberate and coherent (`docs/APP-CONTRACTS.md` §2). Do not propose replacing it with Zed's
many-entities model. Apply these patterns to the code you are already changing; a new stateful
sub-view, dialog body or measured list may become its own entity, and existing free functions
stay free functions until they hit rule 10.

**Verification.** `cargo check -p fleet-ui-kit --examples` and
`cargo run -p fleet-ui-kit --example kit_gallery` are the stated acceptance gate
(`docs/DESIGN-SYSTEM.md:1253-1254`), then `make lint` and `make test`. Commit as
`ui-kit: <imperative lowercase summary>` (or `app:` for view changes).

## Review checklist

Full list: `references/checklist.md`.

1. Is anything stateless implemented as an `Entity` + `Render`?
2. Does every new `impl Render` / `impl Element` carry a comment naming the state or paint it needs?
3. Does `new()` take identity plus required args only, with everything else a `mut self -> Self` builder?
4. Are text/id/slot/handler parameters the borrow-friendly forms, not `String` or `Vec<AnyElement>`?
5. Does every interactive component take an `ElementId`, with rows keyed `("prefix", ix)`?
6. Are all conditionals `.when`/`.when_some`/`.when_else`/`.map`, and optional children `.children(opt)`?
7. Does a new method name duplicate a concept the kit already spells differently?
8. Are the list heights actually uniform, and does the caller own the scroll handle?
9. Does the new affordance have a visible `KeyHint` (and no tooltip, no button)?
10. Gallery panel for **every** new state, §6 catalog entry, and a doc comment naming the sibling
    to use instead — all in this commit? And no new `mod.rs`?

## Related skills

- **gpui-styling** — `Theme`, `Tone`, `TextRole`, spacing, radii, elevation, icons, motion; the
  layout helpers (`h_flex`/`v_flex`-style `StyledExt`) the kit currently lacks.
- **gpui-state-and-memory** — entities, `WeakEntity`, subscriptions, `Task` retention, globals.
- **gpui-app-shell** — actions, key contexts, focus, dialogs, overlays, notifications.
- **gpui-performance** — render-time allocation, memoisation, virtualisation.
- **rust-gpui-testing** — `#[gpui::test]` for components, `TestAppContext`.
- **rust-workspace-architecture** — crate layering, file size, `missing_docs`, module naming.
- **zed-quality-review** — aggregates this skill's `references/checklist.md`.
