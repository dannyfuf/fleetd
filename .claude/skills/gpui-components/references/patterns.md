# gpui-components — pattern catalog

Zed citations are `zed/crates/<crate>/src/<file>.rs:<line>` at tag **v1.18.1**
(`/Users/danny/.swarm/repos/zed-industries/zed`, `git describe --tags` → `v1.18.1`).
fleetd citations are relative to `/Users/danny/.swarm/worktrees/dannyfuf/fleetd/chore-skills`.

Contents: [P1](#p1-stateless-component) · [P2](#p2-named-slots) · [P3](#p3-fluentbuilder) ·
[P4](#p4-stateful) · [P5](#p5-lists) · [P6](#p6-fragments) · [P7](#p7-traits) ·
[P8](#p8-base-delegation) · [P9](#p9-preview) · [P10](#p10-files) · [P11](#p11-ids) ·
[P12](#p12-text) · [P13](#p13-affordances)

---

## P1 — Stateless component <a id="p1-stateless-component"></a>

`#[derive(IntoElement)]` + `impl RenderOnce`. `render` takes ownership of `self` and receives
`&mut App`, not `&mut Context<Self>` (Zed `.rules:105`). This is tier 1 of three:

| Tier | Trait | `render` receives | For | Zed count |
| --- | --- | --- | --- | ---: |
| Component | `RenderOnce` + `#[derive(IntoElement)]` | `self`, `&mut Window`, `&mut App` | Builders that exist only to become elements | 121 |
| View | `Render` on `T`, held as `Entity<T>` | `&mut self`, `&mut Window`, `&mut Context<Self>` | Owns state / focus / subscriptions | 397 |
| Element | `Element` (`request_layout`/`prepaint`/`paint`) | frame callbacks | Custom layout or paint | 34 (24 in `gpui`) |

Zed's `crates/ui` is almost purely tier 1 (70 `RenderOnce` : 5 `Render`); feature crates are
almost purely tier 2. That split *is* the design-system boundary.

```rust
// zed/crates/ui/src/components/disclosure.rs:8-35 (trimmed)
#[derive(IntoElement, RegisterComponent)]
pub struct Disclosure {
    id: ElementId,
    is_open: bool,
    disabled: bool,
    visible_on_hover: Option<SharedString>,
    tooltip: Option<Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>>,
    on_toggle_expanded: Option<Arc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl Disclosure {
    pub fn new(id: impl Into<ElementId>, is_open: bool) -> Self { /* all-None defaults */ }
}
```

**fleetd**: `crates/fleet-ui-kit/src/components/badge.rs:24-59` is the same shape without the
id (a badge is not interactive). 72 `#[derive(IntoElement)]` / 73 `impl RenderOnce` in the kit.
`docs/DESIGN-SYSTEM.md:350-355` states the contract: "Every component is a `RenderOnce` +
`IntoElement` struct with a `new`-style constructor and chained builder methods. None of them
owns state."

Glossary (`zed/docs/src/development/glossary.md:55-71`): "`Component`: A builder which can be
rendered turning it into an `Element`." "`View`: An `Entity` which can produce an `Element`
through its implementation of `Render`."

---

## P2 — Named slots <a id="p2-named-slots"></a>

Two shapes exist. Zed's optional-slot form accepts `impl Into<Option<E>>` so a caller can pass
`None` or an element interchangeably, and erases eagerly:

```rust
// zed/crates/ui/src/components/tab.rs:69-77
pub fn start_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
    self.start_slot = element.into().map(IntoElement::into_any_element);
    self
}
pub fn end_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
    self.end_slot = element.into().map(IntoElement::into_any_element);
    self
}
```

fleetd's form is simpler and is the established convention for its four surfaces
(`Pane`, `Dialog`, `Sheet`, `Overlay`) — the slot is either set or absent, never `None`-passed:

```rust
// crates/fleet-ui-kit/src/components/pane.rs:74-89 (trimmed)
pub fn header(mut self, header: impl IntoElement) -> Self {
    self.header = Some(header.into_any_element());
    self
}
pub fn body(mut self, body: impl IntoElement) -> Self { /* … */ }
pub fn footer(mut self, footer: impl IntoElement) -> Self { /* … */ }
```

Keep fleetd's form. Reach for `impl Into<Option<E>>` only when a call site actually needs to
pass a computed `Option` without a `.when_some`. `crates/fleet-ui-kit/src/components/dialog.rs:92-104`
shows the same slots plus `hints(impl IntoElement)` / `hint_row(KeyHintRow)`.

**When `ParentElement` instead.** Zed containers that take a homogeneous run of children
implement `ParentElement` by extending a `SmallVec<[AnyElement; 2]>`
(`zed/crates/ui/src/components/tab.rs:40, 103-107`). fleetd has zero `ParentElement` impls and does
not need them for fixed-region surfaces; a future `Row`-of-N or list container is the case that
would justify one.

---

## P3 — FluentBuilder <a id="p3-fluentbuilder"></a>

```rust
// zed/crates/gpui/src/util.rs:10-61
pub trait FluentBuilder {
    fn map<U>(self, f: impl FnOnce(Self) -> U) -> U;
    fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self;
    fn when_else(self, condition: bool,
                 then: impl FnOnce(Self) -> Self, else_fn: impl FnOnce(Self) -> Self) -> Self;
    fn when_some<T>(self, option: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self;
    fn when_none<T>(self, option: &Option<T>, then: impl FnOnce(Self) -> Self) -> Self;
}
```

Zed `.rules:109` documents `.when` / `.when_some` as *the* way to express a conditional
attribute or child. Zed has 1176 `.when(` and 546 `.when_some(` call sites.

```rust
// crates/fleet-ui-kit/src/components/badge.rs:74-81
.when(self.style != BadgeStyle::Bare, |el| {
    el.h(theme.metrics.chip_h).px(theme.space.xs)
})
.when(self.style == BadgeStyle::Filled, |el| el.bg(fill))
.when(self.style == BadgeStyle::Outlined, |el| {
    el.border(theme.metrics.hairline).border_color(theme.colors.border)
})
```

`.map` with an explicit `if` is the escape hatch when branches produce different types —
`crates/fleet-ui-kit/examples/support/layout.rs:52-60` uses it exactly that way.

**Redundancy to avoid.** `Option<T>` implements `IntoIterator`, so
`.when_some(opt, |el, x| el.child(x))` is just `.children(opt)`.

---

## P4 — Stateful component <a id="p4-stateful"></a>

Reach for an `Entity` + `impl Render` only when something must survive across frames: a focus
handle, a caret/IME, a scroll or measurement cache, subscriptions, an `EventEmitter`.

```rust
impl Render for GitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("git_panel")
            .key_context(self.dispatch_context(window, cx))
            .track_focus(&self.focus_handle)
            .when(has_write_access, |this| {
                this.on_action(cx.listener(Self::toggle_staged_for_selected))
            })
    }
}
```
`zed/crates/git_ui/src/git_panel.rs:8480-8513`

In Zed's own `crates/ui`, only `Tooltip`, `LinkPreview` and `ContextMenu` qualify — all three
need a focus handle or must be held alive by a popover.

**fleetd**: three production `Render` components in the kit, and the reason is documented per
component (`docs/DESIGN-SYSTEM.md:1056-1061` for the native-agent group):

| Component | File | Why it owns state |
| --- | --- | --- |
| `TextInput` | `crates/fleet-ui-kit/src/components/text_field/input.rs:210` | caret + IME preedit |
| `MultilineInput` | `crates/fleet-ui-kit/src/components/multiline_input/input.rs:557` | caret + wrapped-line cache |
| `TranscriptList` | `crates/fleet-ui-kit/src/components/agent/transcript_list.rs:620` | measured row heights |

`fleet-app` has five (`shell/root/render.rs:11`, `dialogs/host.rs:413`, `shell/chrome.rs:242`,
`screens/agent_thread/mod.rs:1147`, `views/doctor_view/retained.rs:135`). Every state change
inside one calls `cx.notify()` (Zed `.rules:127`). Ownership, weak handles and task retention
belong to **gpui-state-and-memory**.

---

## P5 — Lists <a id="p5-lists"></a>

```rust
// zed/crates/gpui/src/elements/uniform_list.rs:22-29
pub fn uniform_list<R: IntoElement>(
    id: impl Into<ElementId>,
    item_count: usize,
    f: impl 'static + Fn(Range<usize>, &mut Window, &mut App) -> Vec<R>,
) -> UniformList

// zed/crates/gpui/src/elements/list.rs:24-27
pub fn list(
    state: ListState,
    render_item: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
) -> List
```

The module docs state the constraint outright: `uniform_list` "measures the first element and
then lays out all remaining elements in a line based on that measurement […] only works for
elements with uniform height" (`uniform_list.rs:1-5`); `list` is "for a large number of
differently sized elements" and requires `ListState::splice`/`reset` when a height changes
(`list.rs:1-8`). Zed usage: 79 `uniform_list` sites vs 26 `list(ListState)`.

**fleetd** wraps `uniform_list` once, in the kit, and the app never calls it directly (0 sites
in `fleet-app`):

```rust
// crates/fleet-ui-kit/src/components/list_view.rs:294-298
pub fn new(
    id: impl Into<ElementId>,
    item_count: usize,
    render_row: impl Fn(usize, bool, &mut Window, &mut App) -> AnyElement + 'static,
) -> Self
```

```rust
// crates/fleet-ui-kit/src/components/list_view.rs:385-395 (trimmed)
let list = uniform_list(self.id, self.item_count, move |range, window, cx| {
    range.map(|ix| render_row(ix, cursor == Some(ix), window, cx)).collect::<Vec<_>>()
})
.size_full();
match self.scroll {
    Some(handle) => list.track_scroll(&handle).into_any_element(),
    None => list.into_any_element(),
}
```

The `(ix, is_cursor)` pair is a genuine improvement over Zed's raw range closure: a row never
compares indices itself. `LogView` (`crates/fleet-ui-kit/src/components/log_view.rs:57`) is the
second wrapper. The caller owns the `UniformListScrollHandle`
(`crates/fleet-app/src/views/repos_rail.rs:277-281`), which is what keeps scroll position
stable across re-renders.

**Not for terminal rows.** ADR `docs/decisions/0002-terminal-emulation.md` forbids
`uniform_list`/`list` for the grid; it is a `canvas()` with `shape_line(.., force_width)`
(`crates/fleet-ui-kit/src/components/terminal_grid/painter.rs`).

**Gap.** `list(ListState)` is unused in fleetd. The first genuinely variable-height list (a
wrapped-text transcript, a diff with folded hunks) must not be forced into `uniform_list`;
either fix the row height or use `ListState`.

---

## P6 — View fragments <a id="p6-fragments"></a>

Zed has 854 `fn render_*` helpers repo-wide. Dominant return type is `impl IntoElement` (75 of
the explicit-signature ones), then `AnyElement` (36), `Div` (21), `Option<AnyElement>` (20) —
the last fed straight into `.children(opt)`.

fleetd's analogue is a module of free functions plus a borrowed `Props` struct, and it is
documented, not accidental (`docs/APP-CONTRACTS.md:50-72`):

```rust
// crates/fleet-app/src/views/board_screen.rs:72-78
pub(crate) fn render(
    props: &BoardProps<'_>,
    board_scroll: &ScrollHandle,
    column_scrolls: &[ScrollHandle],
    on_click: impl Fn(BoardClick, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement
```

Seven `Props` structs exist (`BoardProps`, `ListProps`, `RailProps`, `PrProps` ×2,
`WorktreeProps`, `RepoProps`); `&Entity<AppState>` appears in 326 signatures. `fleet-app` has
exactly one `#[derive(IntoElement)]` (`crates/fleet-app/src/views/workspace_header.rs:24`).

**Promotion rule.** Zed promotes a fragment to a component even inside a feature crate once it
has real parameters — `zed/crates/git_ui/src/git_panel.rs:8821` declares
`#[derive(IntoElement, RegisterComponent)] pub struct PanelRepoFooter` in the middle of the
panel file. In fleetd the promotion target is `fleet-ui-kit`, and it triggers on: reuse across
screens, more than ~4 parameters, or a fragment that has grown its own states. Remember the kit
takes no domain types (`crates/fleet-ui-kit/src/lib.rs:12-13`) — the app converts first.

---

## P7 — Trait vocabulary <a id="p7-traits"></a>

Zed's entire component contract is six tiny traits in `zed/crates/ui/src/traits/`:

```rust
// zed/crates/ui/src/traits/clickable.rs:4-8
pub trait Clickable {
    fn on_click(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;
    fn cursor_style(self, cursor_style: CursorStyle) -> Self;
}

// zed/crates/ui/src/traits/toggleable.rs:5-20
pub trait Toggleable { fn toggle_state(self, selected: bool) -> Self; }
pub enum ToggleState { #[default] Unselected, Indeterminate, Selected }
```

Plus `Disableable`, `FixedWidth`, `VisibleOnHover`, `Transformable`, and the blanket `StyledExt`.
Component-family traits (`ButtonCommon`, `SelectableButton`, `LabelCommon`) live beside their
family. Everything is re-exported from `zed/crates/ui/src/prelude.rs:1` — "The prelude of this
crate. When building UI in Zed you almost always want to import this."

**Naming warning.** At v1.18.1 there is no `Selectable` trait; it is `Toggleable` with the
tri-state `ToggleState`. Do not cite `Selectable`.

**fleetd**: no `traits/` directory. The kit's only traits are `ActiveTheme`
(`crates/fleet-ui-kit/src/theme/theme.rs`) and `FuzzyCursorSource`. Five ad-hoc
`pub fn disabled(` and one `pub fn on_click(` exist across 95 component files. The cheapest
consistency win in the kit is to add one trait at the moment a second component needs the same
verb, and export it from `crates/fleet-ui-kit/src/lib.rs:62` (`pub mod prelude`). Do not
big-bang port Zed's six.

A generic-over-`InteractiveElement` helper already hints at the idea:

```rust
// crates/fleet-ui-kit/src/components/control.rs:27-34
pub(super) fn on_activate<E: InteractiveElement + StatefulInteractiveElement + Styled>(
    element: E,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> E {
    element.role(gpui::Role::Button).cursor_pointer()
        .on_click(move |_, window, cx| activate(window, cx))
}
```

---

## P8 — Base delegation

When a component adds behaviour on top of an interactive base, Zed holds the base and forwards
the standard traits instead of re-implementing interactivity:

```rust
// zed/crates/ui/src/components/tab.rs:32-42, 88-94, 109-111 (trimmed)
#[derive(IntoElement, RegisterComponent)]
pub struct Tab { div: Stateful<Div>, selected: bool, start_slot: Option<AnyElement>, /* … */ }

impl InteractiveElement for Tab {
    fn interactivity(&mut self) -> &mut gpui::Interactivity { self.div.interactivity() }
}
impl StatefulInteractiveElement for Tab {}
impl RenderOnce for Tab {
    #[allow(refining_impl_trait)]
    fn render(self, _: &mut Window, cx: &mut App) -> Stateful<Div> { /* … */ }
}
```

`Button` does it one level up: it wraps `base: ButtonLike` and every trait method mutates
`self.base` (`zed/crates/ui/src/components/button/button.rs:104`), so there is exactly one
implementation of "how a button looks when disabled"
(`zed/crates/ui/src/components/button/button_like.rs`).

The `#[allow(refining_impl_trait)]` idiom — returning the concrete `Stateful<Div>` from
`RenderOnce::render` — lets the caller keep chaining on the result.

**fleetd**: zero delegation impls, and mostly it does not need them: its containers use named
slots (P2), and it has no button family. Adopt this only if a kit component starts re-exposing
`.on_click` / `.hover` / `.w_full` by hand. A leaf that never takes children or clicks should
just `impl RenderOnce` and skip the boilerplate — Zed's `Divider` only implements `Styled`
(`zed/crates/ui/src/components/divider.rs:131-135`).

---

## P9 — Previews live with the component

```rust
// zed/crates/ui/src/components/button/button.rs:515-545 (trimmed)
impl Component for Button {
    fn scope() -> ComponentScope { ComponentScope::Input }
    fn sort_name() -> &'static str { "ButtonA" }
    fn description() -> &'static str { "A button triggers an event or action." }
    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        v_flex().gap_6().children(vec![
            example_group_with_title("Button Styles", vec![
                single_example("Default", Button::new("default", "Default").into_any_element()),
            ]),
        ]).into_any_element()
    }
}
```

Registration is compile-time through `inventory`, walked once at startup:

```rust
// zed/crates/component/src/component.rs:25-29, 47-61 (trimmed)
pub fn init() { for f in inventory::iter::<ComponentFn>() { (f.0)(); } }
pub fn register_component<T: Component>() {
    let metadata = ComponentMetadata { id: T::id(), description: …, preview: T::preview,
                                       scope: T::scope(), sort_name: …, status: T::status() };
}
```

70 components are registered; `ComponentStatus` marks lifecycle
(`WorkInProgress`/`EngineeringReady`/`Live`/`Deprecated`) and `ComponentScope` is the gallery
taxonomy. The point is that a new variant with no preview is visible **in the same diff**.

**fleetd** has the same promise but not the same enforcement. `docs/DESIGN-SYSTEM.md:1251-1252`
— "If a state is not in a gallery, it is not implemented" — is backed by seven hand-written
gallery files (7,749 lines) in `crates/fleet-ui-kit/examples/`, separate from the components:

| Gallery | Group |
| --- | --- |
| `examples/gallery_structure.rs` | `AppFrame`, `SplitLayout`, `Pane`, `Sheet`, `Dialog`, `Overlay`, `Banner`, … |
| `examples/gallery_data.rs` | rows, lists, facts, badges, chips |
| `examples/gallery_input.rs` | text fields, selects, toggles, palette |
| `examples/gallery_terminal.rs` | grid, tab strip, log view |
| `examples/gallery_agent.rs` | transcript, decision cards, tool rows |
| `examples/gallery_board.rs` | kanban columns, card tiles |
| `examples/kit_gallery.rs` | combined overview |

A panel is a plain `fn <name>_section(cx: &mut App) -> AnyElement` built from
`GalleryLayout::section` / `::labeled` / `strip`
(`crates/fleet-ui-kit/examples/support/layout.rs:11-75`), e.g.
`gallery_structure.rs:498` (`fn pane_section`).

**Incremental port**, in this order:
1. Move `GalleryLayout` and `strip` from `examples/support/` into the crate (behind a
   `pub mod preview` or a `preview` feature) so a component *can* build its own preview.
2. Add a `Component`-like trait with `description()` + `preview()` and have each component
   implement it beside its struct.
3. Only then decide registration: `inventory` (unverified on the pinned toolchain) or a
   hand-written `register_all(cx)` listing components — zero new dependencies.

Until that lands, the change protocol at `docs/DESIGN-SYSTEM.md:1242-1255` is the enforcement:
gallery panel + §6 catalog entry in the same commit.

---

## P10 — Files and exports

Zed: 75 files under `crates/ui/src/components/` (~25.5k lines). A file holds one *concept*,
which may be a small family (`toggle.rs` holds `Checkbox`, `Switch`, `SwitchField`); a family
that grows becomes a directory (`button/` → `button.rs`, `button_like.rs`, `icon_button.rs`, …).
Two hard rules from `zed/.rules`:

- `:5` — "Prefer implementing functionality in existing files unless it is a new logical
  component. Avoid creating many small files."
- `:14` — "Never create files with `mod.rs` paths - prefer `src/some_module.rs` instead of
  `src/some_module/mod.rs`."

`components.rs` is a flat `mod x;` block followed by a matching `pub use x::*;` block.

**fleetd** matches the one-concept-per-file layout (95 files, `agent/`, `markdown/`,
`multiline_input/`, `text_field/`, `text_area/`, `terminal_grid/` as directories) and is
*better* on exports: explicit `pub use badge::{Badge, BadgeStyle};`
(`crates/fleet-ui-kit/src/components/mod.rs:78-162`) makes the public surface reviewable in a
way Zed's globs do not. Keep the explicit form.

It diverges on the `mod.rs` ban: 31 `mod.rs` files exist, including
`crates/fleet-ui-kit/src/components/mod.rs`, `crates/fleet-ui-kit/src/theme/mod.rs`,
`crates/fleet-ui-kit/src/components/agent/mod.rs`, `crates/fleet-app/src/views/mod.rs`,
`crates/fleet-app/src/screens/mod.rs`, `crates/fleet-app/src/dialogs/mod.rs`. Renaming is
mechanical (`git mv components/mod.rs components.rs`) and makes the module greppable — do it in
the commit that already touches the module, never as standalone churn.

Free constructor functions beside the type are Zed's shorthand when the type is usually built
inline (`pub fn checkbox(id, toggle_state) -> Checkbox`,
`zed/crates/ui/src/components/toggle.rs:15-23`). fleetd does not use this; not needed.

---

## P11 — Element ids

`ElementId` has `From` impls for `usize`, `i32`, `SharedString`, `String`, `&'static str`,
`Uuid`, `(&'static str, usize|u64|u32|EntityId)`, `(SharedString, usize)` and `[u8; 20]`
(`zed/crates/gpui/src/window.rs:6685-6781`). The `("prefix", index)` tuple is *the* way to key
list rows; `ElementId::named_usize` is the explicit form.

An element also needs a non-`None` id to appear in the accessibility tree
(`zed/crates/gpui/src/element.rs:106-111`).

**fleetd**: 102 `ElementId` mentions but only four constructors take one first —
`crates/fleet-ui-kit/src/components/list_view.rs:294`,
`spinner.rs:22`, `spinner.rs:71`, `kanban_column.rs:45`. Six more accept it as a late
`.id(..)` builder (`chip.rs:101`, `row.rs:158`, `job_row.rs:124`, `status_glyph.rs:168`,
`terminal_grid.rs:85`, `terminal_tab_strip.rs:111,237`, `sticky_error_slot.rs:67`,
`freshness_stamp.rs:129`, `icons.rs:243`). Identity-first is the better default for anything
that owns interactivity, scroll or hover state, because the id becomes non-optional at the type
level. Audit the interactive components you touch; do not sweep.

---

## P12 — Text

Zed's `Label` family shares `LabelCommon`: `size`, `weight`, `line_height_style`, `color`,
`strikethrough`, `italic`, `underline`, `alpha`, `truncate`, `single_line`
(`zed/crates/ui/src/components/label/label_like.rs:34-64`). One in-code rule worth porting:

> "Buttons with static labels should _never_ be truncated, ensure this is only used when the
> label is dynamic and may overflow." — `zed/crates/ui/src/components/button/button.rs:230-231`

Generalised for fleetd: truncate only what the *data* can make long, never a fixed caption.
`InteractiveText` exists in `gpui` but has zero application call sites at v1.18.1 — do not
teach it.

**fleetd** narrows type to seven fixed roles with fixed line heights
(`crates/fleet-ui-kit/src/text.rs:15-33`: `Ui`, `UiStrong`, `Title`, `Data`, `DataSmall`,
`Label`, `Hint`) and colour to nine `Tone` variants
(`crates/fleet-ui-kit/src/tone.rs:10-33`) against Zed's ~20-variant `Color`. That narrowing is
deliberate and documented (`docs/DESIGN-SYSTEM.md` §1.4, §2.3) — the details belong to
**gpui-styling**. Truncation helpers live in `crates/fleet-ui-kit/src/truncate.rs`.

---

## P13 — Affordances: `KeyHint`, not tooltips

This is where fleetd deliberately departs from Zed, and the departure is written down.

Zed: `ButtonCommon::tooltip`'s own doc says "Nearly all interactable elements should have a
tooltip. Some example exceptions might a scroll bar, or a slider."
(`zed/crates/ui/src/components/button/button_like.rs:35-39`), and `Button::render` back-fills
`aria_label` from the visible label (`zed/crates/ui/src/components/button/button.rs:428-441`).

fleetd: `docs/DESIGN-SYSTEM.md:39` — "**Everything is a key.** There are no buttons anywhere in
Fleet. Affordances are `KeyHint`s." — and `:1232-1233` lists Buttons and Tooltips under "What is
deliberately not in the kit" ("Every affordance already states its key"). The repo has zero
`.tooltip(` call sites. **Do not port Zed's tooltip rule.**

The fleetd obligations instead (`docs/DESIGN-SYSTEM.md` §4, lines 262-278):

```rust
// crates/fleet-ui-kit/src/components/key_hint.rs:22-49, 76-92
KeyHint::labeled("^s r", "rename")            // prefixed inside the Workspace
KeyHintRow::new().key("j/k", "move").key("⏎", "open")
```

- Lowercase key = safe, uppercase = stronger variant; never hand-write that decision —
  `FactList::confirm_key()` returns it.
- Inside the Workspace every hint carries its `^s` prefix, because bare keys go to the PTY.
- A list under a text field moves with `ctrl-n`/`ctrl-p`, never `j`/`k`; `FuzzyList::binds_jk()`
  states which case a list is in.
- An invalid command is **not listed**, never greyed.

**Accessibility.** fleetd has 46 `Role::` sites and zero `aria_label`. A role with no
accessible name is worse than no role: `crates/fleet-ui-kit/src/components/control.rs:32` sets
`Role::Button` on every activatable settings control with nothing to announce. When you touch
such a component, give it a name.
