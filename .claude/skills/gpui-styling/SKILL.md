---
name: gpui-styling
description: How to style a GPUI element in fleetd — design tokens, theme colour access, the pixel/`ch` unit system, spacing and radii scales, the `Styled` builder and flex layout, elevation and surfaces, truncation, icons, focus/hover states, and animation. Load it before writing or reviewing any `div()` chain, before adding a colour, size, radius, shadow, font size, icon or duration, before touching `crates/fleet-ui-kit/src/theme/`, `text.rs`, `tone.rs`, `icons.rs` or `focus.rs`, and whenever a diff contains `px(`, `rgb(`, `hsla(`, `.text_size(`, `.bg(`, `.hover(` or `with_animation`. It is about *how a thing looks*; component structure (`RenderOnce`, builders, galleries) belongs to `gpui-components`.
---

# GPUI styling and theming in fleetd

Everything a Fleet surface draws comes from one `Theme` global read out of `cx` each frame.
This skill covers how to reach those tokens, how to compose a `div()` chain that respects
them, and which of Zed's styling idioms fleetd deliberately does *not* use.

Patterns verified against Zed **v1.18.1**, the GPUI tag fleetd depends on
(`Cargo.toml`, `docs/DESIGN-SYSTEM.md:16`). `docs/DESIGN-SYSTEM.md` is **authoritative for
`crates/fleet-ui-kit`** (`docs/README.md:9`); where this skill and that document disagree,
the document wins and this skill is the bug.

## When to use

- Writing or reviewing any `div()` / `svg()` styling chain in `fleet-ui-kit`, `fleet-app` or `fleet-lazygit`.
- Adding or changing a colour, spacing value, radius, shadow, font size, icon or duration.
- Touching `crates/fleet-ui-kit/src/theme/`, `tone.rs`, `text.rs`, `truncate.rs`, `focus.rs`, `icons.rs`.
- A layout misbehaves: text will not shrink, a row overflows, an overlay paints under something.
- Adding a surface (pane, dialog, sheet, overlay, toast) or an animation.
- Porting a Zed styling idiom (`h_flex`, `DynamicSpacing`, `rems`, `elevation_2`) into fleetd.

## When not to

- Designing a *component's* API — constructor, builders, gallery panel, `RenderOnce` vs `Render`: use `gpui-components`.
- Deciding what a screen shows, or which key does it: `docs/UX-SPEC.md` and `docs/KEYMAP.md` decide that.
- Fixing render-time CPU cost, memoisation or virtualization: use `gpui-performance`.

## Rules

**Read the theme from `cx` in `render`, never hold it.** `Theme` is a GPUI `Global`;
`cx.theme()` works on anything that derefs to `App`, so it is available in both
`Render::render` and `RenderOnce::render`. A cached `Theme` survives a `Theme::change`
and paints the old appearance. fleetd states this inline at
`crates/fleet-ui-kit/src/theme/theme.rs:43`, and already does it in all 276 `cx.theme()` sites across the three GPUI crates.

**Pick a colour through `Tone`, never a literal and never a neighbouring role.**
`Tone::color(theme)` and `Tone::fill(theme)` (`crates/fleet-ui-kit/src/tone.rs:35-62`) are the
component-facing indirection, exactly like Zed's `ui::Color`
(`zed/crates/ui/src/styles/color.rs:33-37`: "It is highly, HIGHLY recommended not to use
this!"). fleetd has **zero** `rgb(0x`/`hsla(` outside `theme/` — keep it at zero.

**Derive a variant with `.opacity(theme.metrics.<named>_opacity)`; do not add a near-duplicate token.**
`Metrics` carries eleven named opacities (`theme/tokens.rs:521-543`: `veil_opacity`,
`dimmed_opacity`, `neutral_fill_opacity`, …) and `Tone::fill` is built from two of them. A raw
`.opacity(0.4)` in a component is an unnamed token.

**Size from tokens: `theme.space.*`, `theme.metrics.*`, `theme.radii.*`, `ch()`.**
Spacing is the 4 px scale (`tokens.rs:258-287`), fixed geometry is a `Metrics` field with a doc
comment naming its UX-spec clause (`tokens.rs:414-543`), column widths are `ch` because every
ladder in the UX spec is stated in `ch` (`docs/DESIGN-SYSTEM.md:154-155`). A bare
`px(<literal>)` in a view or a component is the single most mechanically checkable defect here.

**fleetd measures in pixels; Zed measures in rems. That is deliberate — do not "fix" it.**
Zed's generated box scale is rems and only borders/shadows are px
(`zed/crates/gpui_macros/src/styles.rs:935-941` vs `:1336-1340`) because a rem tracks the user's
UI font size (`zed/crates/theme_settings/src/settings.rs:587-596`). Fleet exposes no UI-scale
setting: 0 `rems(`, 0 `set_rem_size` in the workspace, and every token is `Pixels`. Do not port
`rems`, `rems_from_px` or `DynamicSpacing`. If Fleet ever gains a scale setting, the cheap path
is one multiplier inside the `TypeScale`/`Spacing`/`Metrics` constructors, not a rems migration.

**Text is a role, not a size.** `Text::ui/ui_strong/title/data/data_small/label/hint`
(`crates/fleet-ui-kit/src/text.rs:94-126`) applies family, size, line height, weight, casing and
tone in one place. Views never call `.text_size`, `.font_family` or `.line_height` (`text.rs:3-5`).
The escape hatch for a composite container is `text::styled_with(el, style, theme)` (`text.rs:219`).

**Set font, size, colour and line height once at the root and let children inherit.**
`AppFrame` does it at `crates/fleet-ui-kit/src/components/app_frame.rs:112-122` — the same shape
as Zed's window root (`zed/crates/workspace/src/workspace.rs:9134-9143`). A screen that re-sets
the base font is fighting the frame.

**Truncate at a `ch` budget; use pixel ellipsis only when the spec names no budget.**
`Text::truncate_at(n, Truncate::{Head,Middle,Tail})` shortens in display columns without
splitting graphemes (`truncate.rs:29-31`); `Text::ellipsize()` is the flex-column fallback
(`text.rs:170-174`). fleetd has 0 `.truncate()` and only 4 `text_ellipsis` on purpose — the
`ch` budget is the primary tool (`truncate.rs:3-6`).

**A shrinkable flex child needs `min_w_0()`; a fixed slot needs `flex_none()`.**
A flex item's `min-width` defaults to `auto`, so without `min_w_0()` it never shrinks and no
ellipsis ever appears — Zed encodes the same idiom at `zed/crates/ui/src/components/banner.rs:105-116`.
fleetd's canonical row does it at `crates/fleet-ui-kit/src/components/row.rs:279-286`; 68 `min_w_0()` sites.

**Elevation is bg + radius + hairline + shadow, applied together.** The four surfaces are
`Pane`, `Dialog`, `Sheet`, `Overlay` and there is deliberately no fifth
(`docs/DESIGN-SYSTEM.md:1233-1235`). Levels 0/1 carry a hairline instead of a shadow; shadows come
only from `theme.dialog_shadow()` / `theme.sheet_shadow()` (`theme/theme.rs:212-220`). Never
hand-assemble a surface in `fleet-app`.

**There is no z-index in GPUI.** Layering is paint order, `absolute()` inside `relative()`, and
`deferred(child).with_priority(n)`; an overlay that must swallow clicks calls `.occlude()`.
fleetd uses 5 `deferred(` and 7 `occlude()` (`components/dialog.rs:194,203,218`,
`components/overlay.rs:120,127,142`, `shell/root/focus.rs:289`) and 0 `anchored(`.

**Blue means focus or cursor, and only `FocusRing` may draw it.** Two affordances exist: a 2 px
inset pane ring and a 2 px leading cursor bar (`docs/DESIGN-SYSTEM.md:230-238`,
`crates/fleet-ui-kit/src/focus.rs:1-5`). No component paints `colors.accent` as its own border.

**Hover is pointer feedback only — it never encodes state.** Fleet is keyboard-first ("There are
no buttons anywhere in Fleet", `docs/DESIGN-SYSTEM.md:39`), so hover never paints over
`row_selected` (`components/row.rs:254-257`). Zed's `group_hover`/`visible_on_hover` reveal idiom
has no place here; fleetd has 0 uses and needs none.

**Animate through `with_animation` with a `theme.motion.*` duration.** `AnimationExt` honours
`App::reduce_motion` for free (`zed/crates/gpui/src/elements/animation.rs:76-81`), so never
hand-roll a frame timer. Repeated animated items must pass an explicit `ElementId`
(`crates/fleet-ui-kit/src/icons.rs:242-246`). Do not add a `motion.*` token without a consumer.

**Tokens, components and docs move in the same commit.** A new token goes in `theme/tokens.rs`
**and** §2 of `docs/DESIGN-SYSTEM.md`; a new icon means an SVG at `stroke-width` 1.5, a
`lucide_icons!` variant and a §5.1 entry; a new state needs a gallery panel — "If a state is not
in a gallery, it is not implemented" (`docs/DESIGN-SYSTEM.md:1242-1254`).

## Core patterns

Full catalog with Zed provenance: `references/patterns.md`.

### Read tokens, pick a tone

```rust
use fleet_ui_kit::prelude::*;

impl RenderOnce for DegradedChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();                       // never stored on self
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .px(theme.space.sm)
            .h(theme.metrics.chip_h)
            .rounded(theme.radii.full)
            .bg(Tone::Warning.fill(theme))            // derived, not a new token
            .child(Icon::TriangleAlert.el().size(IconSize::Small).tone(Tone::Warning))
            .child(Text::label("degraded").tone(Tone::Warning))
    }
}
```

See `references/patterns.md#p1-theme-access` and `#p2-tone`.

### A row: fixed slot, shrinkable column, budgeted text

```rust
let theme = cx.theme();
div()
    .flex()
    .items_center()
    .h(theme.metrics.row_h)
    .gap(theme.space.md)
    .px(theme.space.md)
    .child(div().flex_none().w(ch(2.0)).child(StatusGlyph::new(kind)))  // fixed glyph column
    .child(
        div()
            .flex_1()
            .min_w_0()                                                  // or it never shrinks
            .overflow_hidden()
            .child(Text::data(branch).truncate_at(28, Truncate::Middle)),
    )
    .child(Text::ui(age).tone(Tone::Secondary).flex_none())
```

`references/patterns.md#p4-flex-and-truncation`.

### Conditional style stays in the chain

```rust
div()
    .flex()
    .when(selected, |el| el.bg(theme.colors.row_selected))
    .when(hoverable, |el| el.hover(move |s| s.bg(hover_bg)))            // pointer feedback only
    .when_some(width, |el, w| el.w(w).flex_none())
    .map(|el| match align {
        ColumnAlign::Left => el.justify_start(),
        ColumnAlign::Center => el.justify_center(),
        ColumnAlign::Right => el.justify_end(),
    })
```

Adapted from `crates/fleet-ui-kit/src/components/row.rs:279-292`; `.when` cannot change the
element *type*, so genuinely different branches build separately and `into_any_element()`.

### An elevated surface

```rust
div()
    .flex()
    .flex_col()
    .w(theme.metrics.dialog_w)
    .max_h_full()
    .min_h_0()
    .rounded(theme.radii.lg)
    .bg(theme.colors.elevated)
    .border(theme.metrics.hairline)
    .border_color(theme.colors.border_strong)
    .shadow(theme.dialog_shadow())
    .overflow_hidden()
    .occlude()                                        // swallow clicks meant for the scrim
```

This is `components/dialog.rs:208-218` verbatim in shape; `overlay.rs:132-142` and
`toast_stack.rs:246-255` repeat it. Compose the existing surface component instead of
retyping the recipe — see `references/patterns.md#p6-elevation` for the helper this asks for.

### Focus, not accent

```rust
FocusRing::pane(pane_focused).content(
    div().flex().flex_col().size_full().bg(theme.colors.surface).children(rows),
)
```

`crates/fleet-ui-kit/src/focus.rs:29-51`. A cursor row uses `FocusRing::cursor_row(active)`.

### An animated glyph

```rust
let duration = Duration::from_millis(theme.motion.spinner);
svg()
    .flex_none()                                      // an icon never stretches
    .size(IconSize::Small.px())
    .path(Icon::LoaderCircle.path())
    .text_color(Tone::Secondary.color(theme))         // monochrome SVG inherits currentColor
    .with_animation(id, gpui::Animation::new(duration).repeat(), |svg, delta| {
        svg.with_transformation(Transformation::rotate(percentage(delta)))
    })
```

`crates/fleet-ui-kit/src/icons.rs:256-269`. Prefer `Icon::X.el().spinning(true).id(..)`.

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
| --- | --- | --- |
| `rgb(0x…)` / `hsla(…)` in a component | Detaches the value from light/dark and from every theme; fleetd is currently at zero outside `theme/` | `Tone::*` or a named `theme.colors.*` role |
| `px(12.0)` inline in a view or component | An unnamed token; drifts from the ladder the spec pins | `theme.space.md`, a `Metrics` field, or `ch(n)` |
| `.opacity(0.4)` with a bare literal | Same value ends up spelled three ways | `theme.metrics.dimmed_opacity` (or a new named metric) |
| Porting `rems()` / `DynamicSpacing` from Zed | fleetd has no rem basis and no `UiDensity`; a mixed system scales half the UI | Keep `Pixels`; if scaling is ever needed, add one multiplier in the token constructors |
| `.text_size(..)` / `.font_family(..)` in a view | Breaks §2.5 "identical on every screen"; the same branch name renders two heights | `Text::data(..)` etc., or `text::styled_with` for a container |
| `flex_1()` without `min_w_0()` | The child never shrinks, so nothing ever truncates and the row overflows | `.flex_1().min_w_0()` on the shrinkable column |
| Hand-assembling a dialog/sheet surface in `fleet-app` | Four copies of the recipe drift apart; `fleet-app` gains styling it must not have | Compose `Dialog` / `Sheet` / `Overlay` / `Pane` from the kit |
| Reaching for a z-index | GPUI has none | Paint order, `absolute()` in `relative()`, `deferred().with_priority(n)`, `.occlude()` |
| Drawing `colors.accent` as a component border | Blue then means three things and focus stops reading as focus | Wrap in `FocusRing::pane` / `FocusRing::cursor_row` |
| Hover that paints over `row_selected` | Pointer feedback erases state the keyboard set | Gate it: `hoverable && !disabled && !selected` |
| A hand-rolled timer for a visual effect | Ignores `App::reduce_motion`, an accessibility regression | `with_animation` + `theme.motion.*` |
| A call-site-derived animation id in a repeated row | All rows share one animation state | `Icon::…el().spinning(true).id(scoped_id)` |
| A `motion.*` / colour token with no consumer | Erodes "the crate is the machine-readable half of the contract" | Implement the consumer or delete the token |

## fleetd-specific guidance

**Where things live.** `crates/fleet-ui-kit/src/theme/tokens.rs` holds every raw value
(`ColorTokens`, `TypeScale`, `Spacing`, `Radii`, `Elevation`, `Motion`, `Metrics`, plus `CH`/`ch()`
at `:23-30`); `theme/theme.rs` assembles them into the `Theme` global and exposes `ActiveTheme`
(`:68`, `:229-241`); `theme/palette.rs` is the terminal palette. `tone.rs`, `text.rs`,
`truncate.rs`, `focus.rs`, `icons.rs` are the five styling primitives every component uses.
`components/` holds 70 modules; `fleet-app` composes them and adds no styling of its own.

**Keep doing (already at or above Zed's bar — do not preach these).** Zero colour literals
outside `theme/`; the `ActiveTheme` accessor read fresh from `cx` each render (the `Global` is
still `impl Global for Theme` on the public type — wrapping it in a private newtype is
`gpui-state-and-memory`'s item, to do the next time `theme.rs` is touched); a semantic
`Tone` enum instead of `Hsla` in component APIs; a root that sets font/size/colour once; 68
`min_w_0()`; `svg().flex_none()` on every icon; the one animation going through
`with_animation`, so `reduce_motion` is honoured by construction; the mono-family probe in
`Theme::resolve_mono_family` (`theme/theme.rs:134-157`), which has no Zed equivalent.

**Gaps worth closing, incrementally, when you are already in the file:**

- **No elevation helper.** The bg + radius + hairline + shadow recipe is retyped at
  `components/dialog.rs:212-216`, `components/overlay.rs:136-140`,
  `components/toast_stack.rs:250-254` and `components/sheet.rs:98-101`. Zed collapses this into
  `StyledExt::elevation_2/3` (`zed/crates/ui/src/traits/styled_ext.rs:49-93`). A fleetd
  `sheet_surface(self, cx)` / `dialog_surface(self, cx)` extension trait would stop the four
  surfaces drifting. Add it when you next touch two of them, not as a standalone refactor.
- **Ad-hoc hover in `fleet-app`.** `views/watch_pane.rs:82` hand-rolls
  `.cursor_pointer().hover(|s| s.bg(theme.colors.row_hover))` — styling that
  `docs/DESIGN-SYSTEM.md:9-12` says must not exist in `fleet-app`. The affordance belongs in a
  kit component.
- **No `h_flex()`/`v_flex()`.** 0 uses; `.flex().flex_col()` / `.flex().items_center()` is spelled
  out everywhere. Zed's helpers are three lines
  (`zed/crates/ui/src/traits/styled_ext.rs:33-42`), blanket-implemented over `Styled`. Worth
  adding to `fleet-ui-kit`; migrate opportunistically, never in a sweep.
- **No system-appearance follow.** 0 `window_appearance` reads; mode is explicit only
  (`Theme::init/change/toggle`). A fresh install cannot match the OS.
- **No contrast tooling.** No WCAG helper and no ratio shown in the gallery, while the two-mode
  palette (`theme/tokens.rs:87-144`) is exactly where a regression will land. Zed's
  `calculate_contrast_ratio` is ~30 lines (`zed/crates/ui/src/utils/color_contrast.rs:10`).

**Divergences that are correct — document, don't remove.** Pixels instead of rems; fixed
`line_height` in px rather than a ratio (`tokens.rs:158-163`); `ch`-budget truncation instead of
`truncate()`; `TypeStyle::tracking` recorded but inert because gpui 1.18.1's `Styled` exposes no
letter-spacing setter (`docs/DESIGN-SYSTEM.md:157-159` — verified: no such method at v1.18.1).

**Verification.** `cargo check -p fleet-ui-kit --examples` and
`cargo run -p fleet-ui-kit --example kit_gallery` (`t` toggles the theme) are the stated
acceptance gate (`docs/DESIGN-SYSTEM.md:1253-1254`); then `make lint` and `make test`.
Commit as `ui-kit: <imperative lowercase summary>` (or `app:`), with the doc change in the same commit.

## Review checklist

Full list: `references/checklist.md`.

1. Any `rgb(`/`rgba(`/`hsla(` outside `crates/fleet-ui-kit/src/theme/`?
2. Any `px(<literal>)` or bare `.opacity(<literal>)` in a component or view, rather than a `theme.*` token?
3. Any `rems(`, `rems_from_px`, `DynamicSpacing` ported in from Zed?
4. Any `.text_size` / `.font_family` / `.line_height` outside `text.rs` and the root frame?
5. Does every shrinkable flex child carry `min_w_0()`, and every fixed slot `flex_none()`?
6. Is overflowing text budgeted with `truncate_at(n, mode)` or `ellipsize()`?
7. Is a new surface composed from `Pane`/`Dialog`/`Sheet`/`Overlay`, not hand-assembled?
8. Does anything draw `colors.accent` itself instead of wrapping in `FocusRing`?
9. Does hover stay pointer-only, gated on `!disabled && !selected`?
10. Does every animation go through `with_animation`, with a `theme.motion.*` duration and an explicit `ElementId` when repeated?
11. Did a new token/icon/state land in `docs/DESIGN-SYSTEM.md` and a gallery in the same commit?

## Related skills

- `gpui-components` — component API, `RenderOnce` vs `Render`, builders, lists, gallery conventions.
- `gpui-app-shell` — focus handles, key contexts, dialogs, overlays and the modal host.
- `gpui-performance` — render-time cost, memoisation, allocation in styling chains.
- `gpui-state-and-memory` — entities, `cx.notify()`, subscriptions behind a theme change.
- `zed-quality-review` — aggregate review pass that loads this skill's checklist.
