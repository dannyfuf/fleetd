# gpui-styling — pattern catalog

Zed citations are relative to the Zed checkout at tag **v1.18.1** (commit `bebe92f`) and written
`zed/crates/<crate>/src/<file>.rs:<line>`. fleetd citations are relative to the fleetd worktree
root. Every path here was checked with `sed`/`rg` before it was written down.

Governing docs: `docs/DESIGN-SYSTEM.md` (authoritative for `crates/fleet-ui-kit`,
`docs/README.md:9`), `docs/UX-SPEC.md` §2.4/§2.5/§9, `docs/APP-CONTRACTS.md:101` ("Render prepares
nothing" — reading the theme in `render` is fine, doing work there is not).

---

## P1 — Theme access {#p1-theme-access}

`Theme` is a GPUI `Global`; `ActiveTheme` is the extension trait on `App`. Because `Context<T>`
derefs to `App`, `cx.theme()` works in `Render::render` and `RenderOnce::render` alike.

```rust
// crates/fleet-ui-kit/src/theme/theme.rs:68, 229-241
impl Global for Theme {}

pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}
```

The rule is written next to the struct (`theme.rs:43`): *"Components must never hold a `Theme`
across frames; read it from `cx` each render."* This is exactly Zed's idiom
(`zed/crates/theme/src/theme.rs:146-149` — `ActiveTheme` returning `&Arc<Theme>`, implemented for
`App`), except fleetd stores the `Theme` by value rather than behind an `Arc`.

Lifecycle: `Theme::init(ThemeMode::Dark, cx)` once before any component renders
(`theme.rs:160-163`, called from `crates/fleet-app/src/shell/root/bootstrap.rs`),
`Theme::change(mode, cx)` / `Theme::toggle(cx)` to switch (`theme.rs:165-193`). `set_mode` swaps
only `colors`, `terminal` and `elevation`, preserving typography, geometry and motion overrides
(`theme.rs:174-180`) — and that is unit-tested (`theme.rs:244+`).

**Deviation from Zed worth knowing.** Zed splits colour (`theme`) from fonts/sizes/density
(`theme::theme_settings(cx)`, `zed/crates/theme/src/theme_settings_provider.rs`) so the `theme`
and `ui` crates need not depend on the settings stack. fleetd has no settings stack for
appearance: `Theme` carries `text`, `space`, `radii`, `elevation`, `motion`, `metrics`,
`font_ui`, `font_mono` in the same struct (`theme.rs:44-67`). Simpler; do not split it without a
reason.

**Not implemented in fleetd:** system appearance. Zed reads `cx.window_appearance()` into a
`SystemAppearance` global (`zed/crates/theme/src/theme.rs:159,189-193`); fleetd has 0
`window_appearance` reads and defaults to `ThemeMode::Dark`.

---

## P2 — Semantic colour: `Tone` {#p2-tone}

`Tone` is fleetd's answer to Zed's `ui::Color`: a nine-variant enum that resolves to an `Hsla`
at render time, so a component API never carries a colour.

```rust
// crates/fleet-ui-kit/src/tone.rs:35-62 (trimmed)
pub fn color(self, theme: &Theme) -> Hsla {
    let c = &theme.colors;
    match self {
        Tone::Default => c.text,
        Tone::Secondary => c.text_secondary,
        Tone::Muted => c.text_muted,
        Tone::Accent => c.accent,
        Tone::Success => c.success,
        Tone::Warning => c.warning,
        Tone::Danger => c.danger,
        Tone::Info => c.info,
        Tone::Inverse => c.text_inverse,
    }
}

/// A low-alpha fill of the same hue, for chip and badge backgrounds.
pub fn fill(self, theme: &Theme) -> Hsla {
    match self {
        Tone::Default | Tone::Secondary | Tone::Muted | Tone::Inverse => {
            theme.colors.text.opacity(theme.metrics.neutral_fill_opacity)
        }
        other => other.color(theme).opacity(theme.metrics.semantic_fill_opacity),
    }
}
```

`ColorTokens` (`theme/tokens.rs:34-83`) is 24 roles, not Zed's ~200: three surfaces (`bg`,
`surface`, `elevated`, `overlay`), two row states (`row_selected`, `row_hover`), four text
contrasts, four semantics (`accent`, `success`, `warning`, `danger`) plus `info`, three border
roles, and the terminal/diff extras. Zed's `element_*` / `ghost_element_*` state families do not
exist here because Fleet has no buttons (`docs/DESIGN-SYSTEM.md:39`).

The equivalent Zed policy, verbatim (`zed/crates/ui/src/styles/color.rs:33-37`):

> "It is highly, HIGHLY recommended not to use this! Using this color means detaching it from any
> semantic meaning across themes."

**Deriving instead of adding.** Zed chains `.opacity(f)` on a token 112 times. fleetd does the
same but names the factor: eleven `*_opacity` fields on `Metrics` (`theme/tokens.rs:521-543`).
`Hsla::opacity`, `fade_out`, `alpha`, `blend`, `grayscale` all exist in gpui v1.18.1
(`zed/crates/gpui/src/color.rs`). If a derived value appears in three or more places, promote it
to a named metric rather than repeating the literal.

**Where literal `Hsla` is legitimate:** inside `theme/tokens.rs` (the `c()`/`ca()` hex helpers,
`tokens.rs:8-18`) and `theme/palette.rs`. Nowhere else — currently zero violations.

---

## P3 — Units: pixels, `ch`, and why not rems {#p3-units}

### Zed's system

`Styled` is macro-generated (`zed/crates/gpui/src/styled.rs:22-34`, abridged):

```rust
pub trait Styled: Sized {
    fn style(&mut self) -> &mut StyleRefinement;
    gpui_macros::style_helpers!();            // w_*, h_*, size_*, min/max_*, gap_*
    gpui_macros::margin_style_methods!();
    gpui_macros::padding_style_methods!();
    gpui_macros::position_style_methods!();
    gpui_macros::border_style_methods!();
    gpui_macros::box_shadow_style_methods!();
    // … plus visibility_style_methods!, overflow_style_methods!, cursor_style_methods!
}
```

The suffix tables decide the unit. Box metrics are **rems**
(`zed/crates/gpui_macros/src/styles.rs:935-941`: `0p5 => rems(0.125)`, `1 => rems(0.25)`),
borders are **px** (`:1336-1340`: `1 => px(1.)`). A rem is not 16 px — it is whatever
`window.set_rem_size(ui_font_size)` last set, i.e. the user's UI font size
(`zed/crates/theme_settings/src/settings.rs:587-596`).

Zed layers `DynamicSpacing::BaseNN` on top for density-aware container padding
(`zed/crates/ui/src/styles/spacing.rs:29-52`), whose module comment says: *"Do not use this
[`ui_density`] to calculate spacing values. Always use [DynamicSpacing] for spacing values."*

### fleetd's system

Every token is `Pixels`. There is no rem basis, no `set_rem_size`, no `UiDensity`. Verified:
0 `rems(`, 0 `rems_from_px`, 0 `set_rem_size`, 0 `DynamicSpacing` in the workspace.

```rust
// crates/fleet-ui-kit/src/theme/tokens.rs:20-30
/// The base unit of the whole system. Every spacing and size token is a multiple of it.
pub const BASE_UNIT: f32 = 4.0;

/// Width of one monospace cell at the data type size, in logical pixels (`1 ch`).
pub const CH: f32 = 7.5;

/// Convert a `ch` budget (the unit the UX spec measures column ladders in) to pixels.
#[inline]
pub fn ch(n: f32) -> Pixels {
    px(n * CH)
}
```

Four scales, all `Pixels`:

| Scale | Values | Source | Use |
| --- | --- | --- | --- |
| `Spacing` | `xxs 2 · xs 4 · sm 8 · md 12 · lg 16 · xl 24 · xxl 32` | `tokens.rs:258-287` | gaps, padding |
| `Radii` | `none 0 · xs 3 · sm 4 · md 6 · lg 12 · full 9999` | `tokens.rs:289-317` | corners |
| `Metrics` | ~60 named fields (`row_h`, `dialog_w`, `hairline`, `chip_h`, …) | `tokens.rs:414-543` | fixed geometry from the UX spec |
| `ch(n)` | multiples of 7.5 px | `tokens.rs:23-30` | column ladders |

`md` (12 px) is the list-column gap and row padding; `lg` (16 px) is pane and dialog padding;
`xxs` exists only inside a chip (`docs/DESIGN-SYSTEM.md:163-166`).

`Metrics::hairline` is fleetd's equivalent of Zed's `border_1 = px(1.)` — a border is a hairline,
not a scaled value. Zed documents the same exception for platform chrome
(`zed/crates/ui/src/utils/constants.rs:5-9`): *"Use pixels here instead of a rem-based size
because the macOS traffic lights are a static size, and don't scale with the rest of the UI."*

**The judgment call.** fleetd's whole layout is a monospace grid measured in `ch`; a rem system
whose basis is the *UI* font would not scale the grid coherently. Keep pixels. Should Fleet ever
expose a scale setting, add one `f32` multiplier applied inside `TypeScale::default`,
`Spacing::default` and `Metrics::default` — one edit, no call-site churn.

---

## P4 — Flex, shrinking and truncation {#p4-flex-and-truncation}

### The flexbox trap

`flex_1()` sets `flex_basis: relative(0.)` but a flex item's `min-width` still defaults to
`auto`, so the child refuses to shrink below its content and nothing ever ellipsises. The fix is
`min_w_0()`. Zed's banner is the reference (`zed/crates/ui/src/components/banner.rs:105-116`):

```rust
let icon_and_child = h_flex()
    .items_start()
    .min_w_0()
    .flex_1()
    .gap_1p5()
    .child(
        h_flex()
            .h(window.line_height())
            .flex_shrink_0()
            .child(Icon::new(icon).size(IconSize::XSmall).color(icon_color)),
    )
    .child(div().min_w_0().flex_1().children(self.children));
```

fleetd's `Row` encodes the same rule per column (`crates/fleet-ui-kit/src/components/row.rs:279-292`):

```rust
div()
    .flex()
    .items_center()
    .overflow_hidden()
    .min_w_0()
    .when(column.flex, |el| el.flex_1())
    .when_some(column.width, |el, width| el.w(width).flex_none())
    .when_some(column.min_width, |el, min_width| el.min_w(min_width))
    .map(|el| match column.align {
        ColumnAlign::Left => el.justify_start(),
        ColumnAlign::Center => el.justify_center(),
        ColumnAlign::Right => el.justify_end(),
    })
    .child(column.element)
```

Rule of thumb: **`min_w_0()` (+ `flex_1()`) on the shrinkable column, `flex_none()` on the fixed
slot, a budget on the text.** 68 `min_w_0()` sites in fleetd.

### Two truncations

Zed's `truncate()` is a composite (`zed/crates/gpui/src/styled.rs:139-141`):

```rust
fn truncate(mut self) -> Self {
    self.overflow_hidden().whitespace_nowrap().text_ellipsis()
}
```

fleetd deliberately does not use it (0 `.truncate()`, 4 `text_ellipsis`). It truncates in
**display columns**, because every column budget in the UX spec is a `ch` number
(`crates/fleet-ui-kit/src/truncate.rs:3-6`):

```rust
// crates/fleet-ui-kit/src/truncate.rs:12-31
pub enum Truncate {
    Head,    // …/payroll        — keep the end (owner/name columns)
    Middle,  // feat/pay…-fix    — keep both ends (branches, paths)
    Tail,    // Fix RUT valid…   — keep the start (PR titles)  [default]
}

pub const ELLIPSIS: char = '\u{2026}';

pub fn truncate(text: &str, budget: usize, mode: Truncate) -> SharedString { … }
```

Grapheme clusters are never split (`unicode_segmentation` + `unicode_width`), and
`truncate_shared` retains the original `SharedString` storage when the text already fits
(`truncate.rs:34-36`) — an allocation win worth preserving.

Choosing between them:

| Situation | Tool |
| --- | --- |
| The UX spec names a `ch` budget for the column | `Text::…truncate_at(n, Truncate::Middle)` |
| A flex column with no fixed budget | `Text::…ellipsize()` (`text.rs:170-174`) |
| A non-`Text` container that must not push the window wider | `.overflow_hidden()` |

### `h_flex` / `v_flex`

Zed collapses the flex triplets (`zed/crates/ui/src/traits/styled_ext.rs:33-42`):

```rust
/// Horizontally stacks elements. Sets `flex()`, `flex_row()`, `items_center()`
fn h_flex(self) -> Self { self.flex().flex_row().items_center() }
/// Vertically stacks elements. Sets `flex()`, `flex_col()`
fn v_flex(self) -> Self { self.flex().flex_col() }
```

blanket-implemented over `Styled`, with `#[track_caller]` free-function wrappers
(`zed/crates/ui/src/components/stack.rs`). 1 128 + 752 uses repo-wide.

**fleetd has neither** (0 uses); `.flex()` + `.flex_col()`/`.items_center()` is spelled out.
Adding them to `fleet-ui-kit` is cheap and would make row/column intent readable. Two cautions:
`h_flex()` forces `items_center`, so a row whose children top-align needs `.items_start()`
(Zed itself does this at `banner.rs:105-106`); and migration should be opportunistic, not a sweep.

---

## P5 — Typography {#p5-typography}

### fleetd: roles, not sizes

```rust
// crates/fleet-ui-kit/src/text.rs:94-126 — constructors, one per role
Text::ui(s)          // system 13/18 regular — body text, values, row content
Text::ui_strong(s)   // system 13/18 medium  — active tab, row title
Text::title(s)       // system 15/20 medium  — dialog and detail titles
Text::data(s)        // mono 12.5/18         — branch, path, sha, head ref
Text::data_small(s)  // mono 11.5/16         — job progress sub-line, log tail
Text::label(s)       // system 11/14 medium, UPPERCASED — pane/section labels
Text::hint(s)        // mono 11/14           — key hints
```

Builders: `.tone(Tone)`, `.muted()`, `.faint()`, `.color(Hsla)`, `.opacity(f)`, `.weight(w)`,
`.truncate_at(n, mode)`, `.ellipsize()`, `.w(px)`, `.w_ch(f)`, `.flex_none()`
(`text.rs:129-192`).

`Text` applies the role's casing itself — `resolved_text_in` uppercases when
`TypeStyle::uppercase` is set, *before* applying the `ch` budget, and a test pins the interaction
(`text.rs:199-214`, `text.rs:265-269`: `Text::label("straße").truncate_at(6, Tail)` → `"STRAS…"`).
Callers must **not** pre-uppercase.

For a container that styles many children at once:

```rust
// crates/fleet-ui-kit/src/text.rs:219-229
pub fn styled_with<E: Styled>(element: E, style: TypeStyle, theme: &Theme) -> E {
    let family = match style.font {
        FontRole::Ui => theme.font_ui.clone(),
        FontRole::Mono => theme.font_mono.clone(),
    };
    element
        .font_family(family)
        .text_size(style.size)
        .line_height(style.line_height)
        .font_weight(style.weight)
}
```

Only 5 `.text_size(` exist in the whole repo, all in root/frame code.

### Two deviations from Zed, both correct

1. **Fixed line height in pixels, not a ratio.** `TypeStyle::line_height: Pixels` with the doc
   *"Fixed line height in logical pixels. Never relative."* (`theme/tokens.rs:158-163`). Zed uses
   `line_height(relative(1.))` for UI labels so it tracks the font size
   (`zed/crates/ui/src/components/label/label_like.rs`). Fleet's grid is fixed, so a fixed line
   height is the point.
2. **`TypeStyle::tracking` is recorded but inert.** The spec asks for `.06em` on the label role;
   gpui v1.18.1's `Styled` exposes no letter-spacing setter, so `Text` cannot apply it
   (`docs/DESIGN-SYSTEM.md:157-159`). Verified against the v1.18.1 checkout. Keep the field as
   spec documentation, or drop it — but do not pretend it works.

### The mono probe (no Zed equivalent)

`SF Mono` is not part of a stock macOS install, and gpui answers a missing family with a
proportional fallback, which would silently break every `ch` measurement.
`Theme::resolve_mono_family` walks `Theme::MONO_STACK` (`SF Mono → SFMono-Regular → Menlo →
Monaco → DejaVu Sans Mono → Liberation Mono → Courier New`) and keeps the first family whose
`i`, `M` and `W` share one advance (`theme/theme.rs:109-157`, `MONO_PROBE_EPSILON = px(0.01)`).
Do not bypass it by hard-coding a family.

---

## P6 — Elevation and surfaces {#p6-elevation}

### Zed

`ElevationIndex` at v1.18.1 has **five** variants — `Background`, `Surface`, `EditorSurface`,
`ElevatedSurface`, `ModalSurface` (`zed/crates/ui/src/styles/elevation.rs:14-26`). There is **no**
`Wash` variant at this tag. It answers three questions: `shadow(cx)`, `bg(cx)`,
`on_elevation_bg(cx)`. `StyledExt` wraps it so bg, radius, border and shadow are set together
(`zed/crates/ui/src/traits/styled_ext.rs:49-93`): `elevation_1` (Surface: title bar, panel, tab
bar), `elevation_2` (ElevatedSurface: notifications, palettes), `elevation_3` (ModalSurface:
dialogs), each with a `_borderless` twin.

`elevation_3` carries a behavioural contract (`styled_ext.rs`, doc comment): a modal surface must
dismiss or prompt on any outside interaction; if it does not, it belongs at the elevated-surface
layer.

### fleetd

Four surfaces, no fifth (`docs/DESIGN-SYSTEM.md:1233-1235`): `Pane`, `Dialog`, `Sheet`,
`Overlay`. `Elevation` is only a pair of shadow recipes (`theme/tokens.rs:334-339`:
`sheet`, `dialog`), turned into `Vec<BoxShadow>` by:

```rust
// crates/fleet-ui-kit/src/theme/theme.rs:202-220
pub fn shadow(&self, token: ShadowToken) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: token.color,
        offset: point(px(0.0), token.y),
        blur_radius: token.blur,
        spread_radius: token.spread,
        inset: false,
    }]
}
pub fn dialog_shadow(&self) -> Vec<BoxShadow> { self.shadow(self.elevation.dialog) }
pub fn sheet_shadow(&self) -> Vec<BoxShadow> { self.shadow(self.elevation.sheet) }
```

Levels 0 and 1 are flat and carry a hairline instead of a shadow (`docs/DESIGN-SYSTEM.md` §2.6).

**The gap.** There is no helper that applies bg + radius + border + shadow together, so the
recipe is retyped in four places:

| Surface | Site | Radius | Shadow |
| --- | --- | --- | --- |
| `Dialog` | `components/dialog.rs:212-216` | `radii.lg` | `dialog_shadow()` |
| `Overlay` | `components/overlay.rs:136-140` | `radii.lg` | `dialog_shadow()` |
| `Toast` | `components/toast_stack.rs:250-254` | `radii.md` | `sheet_shadow()` |
| `Sheet` | `components/sheet.rs:98-101` | — (`border_l` only) | `sheet_shadow()` |

A `StyledExt`-style extension trait in `fleet-ui-kit` (`fn dialog_surface(self, cx)`,
`fn sheet_surface(self, cx)`) would stop the four drifting. Add it when a change already touches
two of them.

### No z-index

`grep` for `z_index` in `zed/crates/gpui/src/style.rs` and `styled.rs` returns nothing.
Layering is:

1. child order (later children paint on top);
2. `absolute()` / `inset_0` inside a `relative()` parent;
3. `deferred(child).with_priority(n)` — higher priority draws on top
   (`zed/crates/gpui/src/elements/deferred.rs:21-28`);
4. `.occlude()` to stop mouse events reaching what is painted underneath
   (`zed/crates/gpui/src/elements/div.rs:1194`).

fleetd: 5 `deferred(` and 7 `occlude()`, all on surfaces —
`components/dialog.rs:194,203,218`, `components/overlay.rs:120,127,142`,
`components/sheet.rs:91,103`, `components/toast_stack.rs:227,255`,
`components/veil.rs:76`, `shell/root/focus.rs:289` (a pointer gate with an explicit priority
constant). 0 `anchored(` — Fleet has no popovers.

---

## P7 — Conditional style: `FluentBuilder` {#p7-fluent}

`map`, `when`, `when_else`, `when_some`, `when_none` on every element
(`zed/crates/gpui/src/util.rs:11-53`). Zed's own `.rules` file states it:

> "If some attributes or children of an element tree are conditional,
> `.when(condition, |this| ...)` can be used to run the closure only when `condition` is true.
> Similarly, `.when_some(option, |this, value| ...)` runs the closure when the `Option` has a value."

fleetd uses 148 `.when(`, 20 `.when_some(`, and `map` for the multi-way form (see the `Row`
column snippet in P4).

**When not to.** `.when` cannot early-return and cannot change the element *type*. When the two
branches are genuinely different elements, build them separately and `into_any_element()` them —
which is why `-> AnyElement` appears 97 times in `fleet-app`.

---

## P8 — Interaction states {#p8-interaction}

### Zed's reference implementation

`ButtonLike::render` (`zed/crates/ui/src/components/button/button_like.rs:805-831`) resolves five
states through `ButtonStyle::{enabled, hovered, active, focused, disabled}`, each returning
`{ background, border_color, label_color, icon_color }`. Two points that carry over:

- **Disabled is a token set, not an opacity hack** — `element_disabled`, `border_disabled`,
  `Color::Disabled`.
- **Hover on a filled surface fades the existing background** (`filled_background.opacity(0.5)`;
  `Hsla::fade_out` takes `&mut self` and does not chain) rather than picking a new colour.

Group-scoped reveal (`zed/crates/ui/src/traits/visible_on_hover.rs:12-17`):

```rust
impl<E: InteractiveElement + Styled> VisibleOnHover for E {
    fn visible_on_hover(self, group_name: impl Into<SharedString>) -> Self {
        self.invisible().group_hover(group_name, |style| style.visible())
    }
}
```

`.invisible()` keeps layout; `.hidden()` sets `display: none`.

### fleetd

Fleet is keyboard-first — "There are no buttons anywhere in Fleet. Affordances are `KeyHint`s"
(`docs/DESIGN-SYSTEM.md:39`) — so the state vocabulary is different and much smaller.

**Focus and selection** (`docs/DESIGN-SYSTEM.md:230-242`): exactly two blue affordances, both
drawn by `FocusRing` and by nothing else (`crates/fleet-ui-kit/src/focus.rs:1-5`):

| Affordance | Rendering | Entry point |
| --- | --- | --- |
| Focused pane | 2 px inset ring in `focus_ring` | `FocusRing::pane(focused)` (`focus.rs:29-35`) |
| Cursor row | 2 px leading bar in `cursor_bar` | `FocusRing::cursor_row(active)` (`focus.rs:38-44`) |

Selection is a **background** (`row_selected`), not a border, and it is a separate flag from
cursor: a list can show a selected row while focus lives in another pane, in which case the row
keeps `row_selected` and loses the bar.

**Hover** is pointer feedback only and never expresses state
(`crates/fleet-ui-kit/src/components/row.rs:255-257`):

```rust
// Hover is pointer feedback only (§3): it never expresses state, and it never paints
// over the selection background of the row the cursor is already on.
let hoverable = self.hoverable && !self.disabled && !selected;
```

Nine `.hover(` sites exist workspace-wide. Seven are in the kit and in `fleet-lazygit`;
**two are ad-hoc styling inside `fleet-app`** (`crates/fleet-app/src/views/watch_pane.rs:82,123`),
which `docs/DESIGN-SYSTEM.md:9-12` forbids. That affordance belongs in a kit component.

`group_hover` / `visible_on_hover` have 0 uses and are not needed: nothing in Fleet is revealed
by hovering, because every affordance already states its key.

**There is deliberately no disabled visual style** (`docs/DESIGN-SYSTEM.md:1238-1240`): "you
cannot edit this here" is said by the *absence* of an input box, and an unavailable command is
not listed at all. `Row::disabled(true)` lowers opacity and removes hover; it does not recolour.

---

## P9 — Icons {#p9-icons}

### The closed set

```rust
// crates/fleet-ui-kit/src/icons.rs:19-30 (trimmed)
macro_rules! lucide_icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        /// Every Lucide glyph Fleet is allowed to draw.
        ///
        /// The set is closed on purpose: §1 of the UX spec makes each glyph a word, so adding one
        /// is a design decision, not an implementation detail.
        pub enum Icon { $($variant,)* }
```

The macro derives the variant, its path and its embedded bytes together, so `include_bytes!`
guarantees *variant ⇒ file exists* at compile time. Zed reaches the same guarantee with two
runtime tests, `test_all_icons_exist` and `test_no_dangling_icons`
(`zed/crates/icons/src/icons.rs:325,338`). fleetd has the first direction only — nothing catches
an orphan SVG in `crates/fleet-ui-kit/assets/icons/` with no variant. A directory-walking test
would close it cheaply.

### Three sizes, all pixels

```rust
// crates/fleet-ui-kit/src/icons.rs:136-157
pub enum IconSize {
    Small,           // 12 px: status bar, inline marks inside a row, exited-tab cross
    Medium,          // 14 px: inside chips, pane headers, filter bar
    #[default] Large // 16 px: the default glyph column of every list, dialog headers
}
```

Zed's `IconSize` is the same idea in rems via `rems_from_px` (`Indicator` 10, `XSmall` 12,
`Small` 14, `Medium` 16 default, `XLarge` 48, plus `Custom(Rems)` —
`zed/crates/ui/src/components/icon.rs:54-67`), and pairs each size with a `DynamicSpacing`
padding so an icon button's hit area is density-aware. fleetd needs neither, having no icon
buttons and no density.

### Rendering

```rust
// crates/fleet-ui-kit/src/icons.rs:256-269
let base = svg()
    .flex_none()
    .size(self.size.px())
    .path(self.icon.path())
    .text_color(color);

if self.spinning {
    let duration = Duration::from_millis(theme.motion.spinner);
    base.with_animation(
        self.id,
        gpui::Animation::new(duration).repeat(),
        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
    )
    .into_any_element()
} else {
    base.into_any_element()
}
```

Three invariants in five lines, identical to Zed's (`zed/crates/ui/src/components/icon.rs:218-224`):
**a monochrome SVG is coloured with `text_color`, never `bg`; it is always explicitly `.size(..)`;
it is always `.flex_none()` so it never stretches in a flex row.**

### Assets

gpui allows exactly one `AssetSource` per application, so `fleet-ui-kit` exposes its bytes rather
than registering its own: `KitAssets` for an app that has no other assets, `kit_asset(path)` for
one that delegates from its own source (`crates/fleet-ui-kit/src/assets.rs`). A missing asset must
return `Ok(None)`, never `Err`. Zed's equivalent (`zed/crates/assets/src/assets.rs`) exists purely
so the `RustEmbed` macro is not re-expanded on every rebuild.

**Adding a glyph** (`docs/DESIGN-SYSTEM.md:1245-1247`): download the SVG into
`crates/fleet-ui-kit/assets/icons/`, rewrite its `stroke-width` from 2 to 1.5, add the variant to
`lucide_icons!` in `src/icons.rs`, add it to §5.1 of `docs/DESIGN-SYSTEM.md`. All in one commit.

---

## P10 — Animation {#p10-animation}

`gpui::Animation` (`zed/crates/gpui/src/elements/animation.rs:15-71`) offers `duration`,
`oneshot`, `synced`, `easing`, `max_fps`, with builders `.repeat()`, `.repeat_synced()`,
`.with_easing(f)`, `.with_max_fps(n)` and easing functions `linear`, `quadratic`, `ease_in_out`,
`ease_out_quint`, `bounce`, `pulsating_between(min, max)`.

The accessibility contract is what makes `with_animation` mandatory
(`zed/crates/gpui/src/elements/animation.rs:76-81`):

> "Animations rendered through this trait automatically respect `App::reduce_motion`: when it is
> set, the element is rendered in a static state (the end state for oneshot animations, the start
> state for repeating ones) and no animation frames are scheduled."

So never hand-roll a frame timer for a visual effect.

**Element ids.** Zed's `with_rotate_animation` — in the **`ui`** crate, not gpui
(`zed/crates/ui/src/traits/animation_ext.rs:15`, trait `DefaultAnimations`) — derives its id from
`ElementId::CodeLocation(*std::panic::Location::caller())` and warns in its own doc comment:
*"If this is not sufficient to identify your state (e.g. you're rendering a list item), you can
provide a custom ElementID using the `use_keyed_rotate_animation` method."* (the method is really
`with_keyed_rotate_animation`, `:26`). fleetd exposes exactly that escape hatch
(`crates/fleet-ui-kit/src/icons.rs:242-246`):

```rust
/// Override the call-site id. Repeated spinning glyphs need one id per item.
pub fn id(mut self, id: impl Into<ElementId>) -> Self { self.id = id.into(); self }
```

**Fleet's motion budget.** Seven declared durations (`theme/tokens.rs:381-396`): `toast` 140 ms,
`sheet` 160 ms, `highlight` 120 ms, `prefix_hint_delay` 400 ms, `spinner` 1000 ms,
`toast_short` 1600 ms, `toast_normal` 3200 ms. The comment above them says "Fleet animates four
things and nothing else."

Consumers, verified: `spinner` (`icons.rs:263`), `prefix_hint_delay`
(`components/prefix_hint.rs:9`, `crates/fleet-app/src/terminal/surface.rs:790`), `toast_short` /
`toast_normal` (`components/toast_stack.rs:52-53`, `crates/fleet-app/src/state/notifications.rs:175-176`).

**`motion.toast`, `motion.sheet` and `motion.highlight` have no consumer.** Only one
`with_animation` exists in the whole workspace (the spinner). Either implement those three with
`with_animation` — which buys `reduce_motion` for free — or delete the tokens. A token with no
consumer contradicts `docs/DESIGN-SYSTEM.md:3-4` ("the crate is the machine-readable half of the
contract").

---

## P11 — Root-level inheritance {#p11-root}

Set family, size, colour and line height once at the window root and let everything inherit.
Zed (`zed/crates/workspace/src/workspace.rs:9134-9143`):

```rust
let ui_font = theme_settings::setup_ui_font(window, cx);   // also calls window.set_rem_size(..)
let colors = cx.theme().colors();
div()
    .relative()
    .size_full()
    .flex()
    .flex_col()
    .font(ui_font)
    .text_color(colors.text)
    .overflow_hidden()
```

fleetd, same shape without the rem step (`crates/fleet-ui-kit/src/components/app_frame.rs:112-122`):

```rust
let theme = cx.theme();
div()
    .relative()
    .flex()
    .flex_col()
    .size_full()
    .overflow_hidden()
    .bg(theme.colors.bg)
    .text_color(theme.colors.text)
    .font_family(theme.font_ui.clone())
    .text_size(theme.text.ui.size)
    .line_height(theme.text.ui.line_height)
```

`AppFrame` also owns the fixed chrome bands (`context_bar_h`, `banner_h`, `status_bar_h`), each
`flex_none()` with `overflow_hidden()`. A screen that re-sets the base font, colour or size is
fighting the frame; if a subtree genuinely needs a different basis, give it a `TextRole` or wrap
it in `text::styled_with`.

---

## P12 — Contrast tooling {#p12-contrast}

Zed ships `calculate_contrast_ratio(fg, bg) -> f32` (WCAG 2.0,
`zed/crates/ui/src/utils/color_contrast.rs:10`) and an APCA variant
(`zed/crates/ui/src/utils/apca_contrast.rs`), and displays the number per token pair in its theme
preview (`zed/crates/workspace/src/theme_preview.rs`). That is how accessibility becomes
reviewable instead of aspirational.

fleetd has neither, while carrying a hand-tuned two-mode palette
(`crates/fleet-ui-kit/src/theme/tokens.rs:87-144`) that is exactly where a contrast regression
will land. Porting the ~30-line WCAG function and showing the ratio in `kit_gallery` is the
cheapest available guard.

---

## Zed idioms that do NOT apply to fleetd

| Zed idiom | Why it does not apply here |
| --- | --- |
| `rems()` / `rems_from_px()` / `WithRemSize` | No rem basis; fleetd never calls `set_rem_size` and exposes no UI-scale setting |
| `DynamicSpacing::BaseNN` / `UiDensity` | No density setting; `Spacing` is a fixed 7-step px scale |
| `ui::Color::Custom` | fleetd's `Tone` has no custom variant, by design |
| `element_*` vs `ghost_element_*` token families | No buttons, so no chip-versus-surface control distinction |
| `visible_on_hover` / `group_hover` | Nothing in Fleet is revealed by hovering |
| `Label::truncate()` / `text_ellipsis_start` | Truncation is a `ch` budget with `Truncate::{Head,Middle,Tail}` |
| `anchored()` popovers | Fleet has no popovers; overlays are `deferred()` surfaces |
| `elevation_1/2/3` | Four named surface components instead of a generic elevation ladder |
| `ThemeRegistry` / extension themes / JSON theme schema | Two modes, compiled in; no user themes |
| `line_height(relative(1.))` | Fixed pixel line heights, because the grid is fixed |
