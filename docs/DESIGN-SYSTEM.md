# Fleet — design system

**Status: authoritative for `crates/fleet-ui-kit`.** This document and the crate are one
contract: the crate is the machine-readable half, this file is the half that says *why* and
*when*. It implements `docs/UX-SPEC.md` §2.4 (tokens), §2.5 (glyph vocabulary) and §9
(component inventory); where the UX spec and this document disagree, the UX spec wins and this
file is the bug.

Views in `fleet-app` compose **only** what `fleet-ui-kit` exports. There is no ad-hoc styling in
`fleet-app`: no literal color, no literal font size, no literal duration. If a view needs
something the kit does not have, the kit gains a component — that is the mechanism that keeps
`docs/UX-SPEC.md` §5's cross-screen invariants true by construction instead of by review.

- Crate: `crates/fleet-ui-kit` (depends on `gpui` only — no `fleet-core`, no `fleet-proto`).
- Visual test bench: `cargo run -p fleet-ui-kit --example kit_gallery` (`t` toggles the theme).
- gpui: Zed monorepo tag **v1.18.1** (`gpui` + `gpui_platform` with `font-kit`), Rust 1.97.1.

---

## 1. Principles that constrain the system

1. **One glyph beats one word; one word beats one sentence.** Any fact with ≤ 5 values is a
   Lucide glyph in a fixed column. The icon set is therefore *closed*: adding a variant to
   `Icon` is a design decision, documented in §6.
2. **Zero-suppression.** A count of 0, an empty label, "no error", "not remote" — none of them
   render. `Chip` suppresses `Some(0)` by default. The one exception is §1.3 of the UX spec:
   absence of *knowledge* always renders.
3. **Absence of information never renders as good news.** `StatusKind::Unknown` is an amber
   `circle-help`; `StatusKind::NoSession` is a `dot` at 30 % opacity; a truly blank cell means
   "this column does not apply". `FactValue::Null` renders `—`, **never `0`**.
4. **Four colors, all semantic, never decorative.** `success`, `warning`, `danger`, and
   `accent`. `accent` is the cursor, the focus ring and the fill of a surface's one primary
   button, and nothing else; `danger` also fills the strong form of a destructive button. Draft,
   muted, disabled and "not applicable" lower contrast; they never add a hue. This is why
   components take a [`Tone`], not a color.
5. **Errors are sticky; successes are transient.** `StickyErrorSlot` persists until dismissed;
   `Toast` decays. `Toast` never carries `Tone::Danger`.
6. **The state is visible where it matters.** There is no mode word. A state that changes what
   keys do is shown by the surface that has it: the scroll pill, the filter bar, the open overlay,
   the focused pane's ring, the ⌃S command menu. The status bar shows where you are and what is
   running, never which key context is active.
7. **Every action has a key and a control.** An action valid on a surface is reachable there by a
   visible control (a button, a menu item, a clickable row or chip) that shows its key as a `Kbd`
   chip from the live keymap. Controls are not focusable: the keyboard path is the key, and
   `Tab`, `j`/`k` and pane focus keep their meaning. Nothing lives only behind hover, and no key
   is taken away (ADR 0023).

Until a surface is rebuilt under §1.6 and §1.7, its §6 entry describes what ships; the entry is
rewritten in the commit that rebuilds it (ADR 0023, *Consequences*).

---

## 2. Tokens

Rust: `fleet_ui_kit::theme` — `ColorTokens`, `TerminalPalette`, `TypeScale`, `Spacing`, `Radii`,
`Elevation`, `Motion`, `Metrics`, assembled into `Theme` and installed as a gpui `Global`.

```rust
Theme::init(ThemeMode::Dark, cx);          // once, before any component renders
Theme::change(ThemeMode::Light, cx);       // explicit switch
Theme::toggle(cx);                         // returns the new mode

let theme = cx.theme();                    // ActiveTheme, on anything that derefs to App
theme.colors.warning;  theme.space.md;  theme.metrics.row_h;  theme.motion.spinner;
```

`ActiveTheme` is implemented for `App`; `Context<T>` derefs to `App`, so `cx.theme()` works in
both `Render::render` and `RenderOnce::render`.

### 2.1 Color roles

`chrome` / `bg` / `surface` / `surface_raised` / `elevated` are the grounds, darkest to highest,
and `overlay` is the scrim between the base screen and whatever floats over it; there is no
other ground. `row_hover` and `control_hover` exist only for the pointer and never express state.

| Role | Dark | Light | Use |
| --- | --- | --- | --- |
| `bg` | `#111317` | `#FBFBFC` | app ground: the content area |
| `chrome` | `#0E0F12` | `#F3F4F6` | window chrome: title bar, sidebar, status bar |
| `surface` | `#16181D` | `#FFFFFF` | rails, panes, scroll pill |
| `surface_raised` | `#1A1C22` | `#FFFFFF` | cards (board tiles, hub cards, grouped settings), always with a `border` hairline |
| `elevated` | `#1B1E24` | `#FFFFFF` | dialogs, sheets, toasts, palette, menus, popovers, tooltips |
| `overlay` | `rgba(5,6,8,.55)` | `rgba(0,0,0,.30)` | scrim behind a dialog, sheet or popover (see §2.6) |
| `row_selected` | `#1E2430` | `#EDF2FB` | cursor row |
| `row_hover` | `#191C22` | `#F3F4F6` | pointer hover on a row only |
| `text` | `#E6E8EB` | `#16181D` | branch, title, value |
| `text_secondary` | `#A3A9B5` | `#4B5563` | repo, host, age, counts, captions |
| `text_muted` | `#808794` | `#636A77` | draft, disabled, key hints, labels, the null dash |
| `text_inverse` | `#0E1013` | `#FBFBFC` | text on a semantic fill |
| `accent` | `#58A6FF` | `#0969DA` | cursor, focus and links: the one blue |
| `accent_fill` | `#58A6FF` | `#0969DA` | fill of a surface's one primary button |
| `accent_fill_hover` | `#79B8FF` | `#0858C0` | pointer hover on `accent_fill` |
| `accent_fill_active` | `#4493F8` | `#0A4A9E` | `accent_fill` while pressed |
| `accent_fill_text` | `#0B0E14` | `#FFFFFF` | label and key chip on `accent_fill` |
| `accent_subtle` | `rgba(88,166,255,.14)` | `rgba(9,105,218,.12)` | wash behind a blue chip or an informational callout |
| `control` | `#1A1D23` | `#FFFFFF` | resting fill of a secondary button or a control |
| `control_hover` | `#22262D` | `#F3F4F6` | pointer hover on a control, ghost button or icon button |
| `control_active` | `#2A2E36` | `#E8EAEE` | a control, ghost button or icon button while pressed |
| `control_border` | `#2C3039` | `#D3D6DC` | hairline around a resting control |
| `kbd_bg` | `#23262D` | `#F3F4F6` | key chip (`Kbd`) fill |
| `kbd_border` | `#30343D` | `#D3D6DC` | key chip (`Kbd`) outline |
| `success` | `#3FB950` | `#1A7F37` | attached, pass, approved, done |
| `warning` | `#D29922` | `#9A6700` | needs a person: running, pending, dirty, unknown, degraded |
| `danger` | `#F85149` | `#CF222E` | failed, changes requested, destructive; the fill of a danger button, under `text_inverse` |
| `danger_fill_hover` | `#FF6A63` | `#B31D28` | pointer hover on a danger button |
| `danger_fill_active` | `#EA4A42` | `#9A1822` | a danger button while pressed |
| `info` | `#58A6FF` | `#0969DA` | neutral information (same hue as accent; never state) |
| `border` | `#22262E` | `#E3E5E9` | 1 px hairlines |
| `border_strong` | `#2C313A` | `#D3D6DC` | hairline on top of `elevated` |
| `focus_ring` | `#58A6FF` | `#0969DA` | 2 px inset ring on the focused pane |
| `cursor_bar` | `#58A6FF` | `#0969DA` | 2 px left bar on the cursor row |
| `selection` | `rgba(88,166,255,.28)` | `rgba(9,105,218,.20)` | text selection, terminal + inputs |
| `scroll_thumb` | `#2C313A` | `#D3D6DC` | 3 px pane-edge thumb |
| `skeleton` | `#1E222A` | `#EEF0F3` | cold-load placeholder rows |
| `diff_added` | `rgba(63,185,80,.14)` | `rgba(26,127,55,.14)` | added-line wash inside reusable diff views |
| `diff_removed` | `rgba(248,81,73,.14)` | `rgba(207,34,46,.14)` | removed-line wash inside reusable diff views |

`accent` stays the single blue: `accent_fill`, `accent_subtle`, `focus_ring`, `cursor_bar` and
`info` are all that hue, never a second one. Amber (`warning`) keeps meaning "needs a person".

**Contrast.** `text`, `text_secondary` and `text_muted` each meet WCAG AA (4.5:1) on every
ground from `chrome` to `elevated` in both modes; a button's label meets it on its fill at rest,
hovered and pressed (`accent_fill_text` on the three `accent_fill` roles, `text_inverse` on
`danger` and its two fill roles, `text` on `control_active`). `theme::contrast_ratio(fg, bg)` computes the ratio; a unit test in
`theme/contrast.rs` holds the palette to it, and the gallery's colour page prints each ratio.

Semantic tones are selected through `Tone`, never by reaching for the field:
`Tone::{Default, Secondary, Muted, Accent, Success, Warning, Danger, Info, Inverse}`.
`Tone::fill(theme)` gives the low-alpha version used behind a filled chip, badge or banner
(14 % of the hue; 8 % of `text` for the neutral tones).

### 2.2 Terminal palette

`Cell` colors arrive as `Default | Palette(u8) | Rgb`. `Default` resolves to
`terminal.foreground` / `terminal.background`; `Palette(n)` resolves through
`TerminalPalette::color(n)`, which covers all 256 indices (0–15 from the table, 16–231 from the
6×6×6 cube, 232–255 from the 24-step grayscale ramp).

| # | Name | Dark | Light |
| --- | --- | --- | --- |
| 0 | black | `#0E1013` | `#24292F` |
| 1 | red | `#F85149` | `#CF222E` |
| 2 | green | `#3FB950` | `#1A7F37` |
| 3 | yellow | `#D29922` | `#9A6700` |
| 4 | blue | `#58A6FF` | `#0969DA` |
| 5 | magenta | `#BC8CFF` | `#8250DF` |
| 6 | cyan | `#39C5CF` | `#1B7C83` |
| 7 | white | `#B1BAC4` | `#6E7781` |
| 8 | bright black | `#5A6069` | `#57606A` |
| 9 | bright red | `#FF7B72` | `#A40E26` |
| 10 | bright green | `#56D364` | `#116329` |
| 11 | bright yellow | `#E3B341` | `#7D4E00` |
| 12 | bright blue | `#79C0FF` | `#0550AE` |
| 13 | bright magenta | `#D2A8FF` | `#6639BA` |
| 14 | bright cyan | `#56D4DD` | `#3192AA` |
| 15 | bright white | `#F0F6FC` | `#8C959F` |
| — | default fg | `#E6E8EB` | `#16181D` |
| — | default bg | `#0E1013` | `#FBFBFC` |
| — | cursor | `#58A6FF` | `#0969DA` |
| — | selection | `rgba(88,166,255,.28)` | `rgba(9,105,218,.20)` |

### 2.3 Type scale

Two families. The UI face is `.SystemUIFont`, which CoreText resolves to SF Pro with no
registration; the data face is `SF Mono`.

`SF Mono` is **not** part of a stock macOS install — it arrives with Xcode or the SF font
download — and gpui answers a missing family with its own fallback stack, which ends at the
proportional Helvetica/Arial. Left alone that degrades silently and catastrophically: every
terminal grid, branch, path and sha stops landing on the cell grid, because `CellMetrics`
derives the column width from the advance of a proportional `M`.

`Theme::init` therefore resolves the data face at startup from `Theme::MONO_STACK`
(`SF Mono → SFMono-Regular → Menlo → Monaco → DejaVu Sans Mono → Liberation Mono →
Courier New`), keeping the first candidate whose `i`, `M` and `W` share one advance. A family
that is absent resolves to the UI sans and fails that probe, so the stack really is walked
rather than trusted. `Theme::with_mono_family` overrides the result.

| Role | Family | Size / line height | Weight | Case | Used for |
| --- | --- | --- | --- | --- | --- |
| `page_title` | system | 20 / 26 | 600 | as written | the one heading of a screen |
| `section_title` | system | 16 / 22 | 600 | as written | a group heading inside a page or a sheet |
| `ui` | system | 13 / 18 | 400 | as written | body text, values, row content (the design's `body` role) |
| `ui_strong` | system | 13 / 18 | 500 | as written | active tab, row title, primary action |
| `title` | system | 15 / 20 | 500 | as written | dialog and detail-panel titles |
| `caption` | system | 12 / 16 | 400 | as written | supporting text under a title or beside a value |
| `sentence_label` | system | 11 / 14 | 600 | **as written** (sentence case) | field labels, sidebar and card group headings |
| `data` | mono | 12.5 / 18 | 400 | as written | branch, path, sha, target, head ref |
| `data_small` | mono | 11.5 / 16 | 400 | as written | job progress sub-line, log tail |
| `label` | system | 11 / 14 | 500 | **UPPERCASED** | legacy micro-header: pane and section labels, counts, the Git UI's status word |
| `hint` | mono | 11 / 14 | 400 | as written | key hints |

`Text::page_title`, `section_title`, `caption` and `sentence_label` are the constructors, with
`TextRole` variants of the same names. `ui` is the body role; there is no second spelling.

**The uppercase micro-header is retiring.** `label` keeps its uppercase until every screen that
uses it is redesigned; a redesigned surface uses `sentence_label`, with the string written in
sentence case (`Base branch`, not `BASE BRANCH`) and no tracking. When the last `label` call
site is gone, the role is removed.

One mono cell is **7.5 × 18 px**; `1 ch = 7.5 px` (`theme::CH`, `theme::ch(n)`). Every column
budget in the UX spec is stated in `ch` and resolved with `ch()` or `ColumnLadder`.

`TypeStyle::tracking` records the spec's `.06em` on the legacy `label` role, but gpui 1.18.1's
`Styled` exposes no letter-spacing setter, so `Text` cannot apply it yet. That is the one token in
this document that is data rather than behaviour. Every other role, `sentence_label` included,
has no tracking.

### 2.4 Spacing (4 px base)

`xxs 2` · `xs 4` · `sm 8` · `md 12` · `lg 16` · `xl 24` · `xxl 32`.

`md` (12 px) is the gap between list columns and the horizontal padding of a row; `lg` (16 px)
is pane padding and dialog padding. `xxs` exists only inside a chip.

### 2.5 Radii

The design's radii: `control 7` · `card 10` · `popover 12` · `dialog 12` · `pill 9999`.
The older scale they sit beside: `none 0` · `xs 3` · `sm 4` · `md 6` · `lg 12` · `full 9999`.

Buttons, inputs, nav rows and segmented controls are `control`. Cards (`surface_raised`) are
`card`. Menus, popovers and tooltips are `popover`; dialogs and the palette are `dialog`. Chips,
status pills and avatars are `pill`. Rows, panes and the terminal grid are square (`none`), and the
docked `Sheet` is square because it is flush to the window edge.

The older names stay because existing components use them: `xs` for inline marks, `sm` for badges
and in-dialog lists, `md` for toasts and the scroll pill. `lg` and `full` are the same values as
`dialog` and `pill`; a redesigned component uses the design name.

### 2.6 Elevation

Five levels, and only three of them have a shadow.

| Level | Surface | Treatment |
| --- | --- | --- |
| 0 | `chrome`, `bg` | nothing |
| 1 | `surface`, `surface_raised` | 1 px `border` hairline, no shadow |
| 2 | `elevated` | `sheet`: `0 8px 24px rgba(0,0,0,.35)` dark / `.12` light — sheets, toasts |
| 3 | `elevated` | `dialog`: `0 16px 48px rgba(0,0,0,.45)` dark / `.16` light — dialogs, palette |
| 4 | `elevated` | `popover`: `0 24px 64px rgba(0,0,0,.55)` dark / `.20` light — menus, popovers, tooltips, the Agent popup window |

`theme.sheet_shadow()`, `theme.dialog_shadow()` and `theme.popover_shadow()` return the
`Vec<BoxShadow>`.

One more shadow is not a level: `lift`, `0 4px 12px rgba(0,0,0,.35)` dark / `.12` light, returned
by `theme.lift_shadow()`. It is the **hover** state of a level-1 tile a pointer can pick up — a
board `CardTile` — drawn together with a `border_strong` hairline, and it goes away with the
pointer. Nothing rests at it.

**The scrim does not blur.** The design blurs the window behind a dialog or sheet, but gpui 1.18.1
has no in-window backdrop filter (its only blur is the OS window-background material). The
`overlay` scrim is instead flat and darker than it would be with a blur: `rgba(5,6,8,.55)` in
dark, `rgba(0,0,0,.30)` in light, so the base screen recedes by darkening alone.

### 2.7 Motion

Fleet animates the spinner and progressively reveals bursty native-agent prose; nothing else
blinks or re-announces itself. Reduced motion disables both continuous effects: the spinner holds
its static state and prose flushes immediately. The other entries below are dwell and delay
durations, not animations: they time how long something waits or stays.

| Token | ms | What |
| --- | --- | --- |
| `prefix_hint_delay` | 400 | how long a held `^S` waits before showing what comes next (the prefix menu) |
| `tooltip_delay` | 500 | how long the pointer rests on a control before its tooltip appears |
| `spinner` | 1000 | one turn of `loader-circle` |
| `jump_chip_delay` | 150 | how long the transcript's jump-to-latest chip waits before appearing |
| `working_tick` | 1000 | how often the working row's elapsed label re-reads the clock |
| `reveal_tick_ms` | 16 | cadence of UTF-8-safe native-agent prose reveal |
| `reveal_horizon_ms` | 200 | maximum time one received prose burst may trail the authoritative projection |
| `toast_short` | 1600 | dwell for instant acknowledgements |
| `toast_normal` | 3200 | dwell for everything else the toast law allows |

### 2.8 Metrics

`Metrics` holds the pixel constants the UX spec pins down, so no component hard-codes one. The
list below is the complete inventory, in px unless marked `ch`; a unit test in
`theme/tokens/tests.rs` fails when it and `crates/fleet-ui-kit/src/theme/tokens.rs` disagree.

| Group | Tokens |
| --- | --- |
| Window chrome | `title_bar_h 44` · `status_bar_h 28` · `traffic_light_inset 84` · `command_field_w 340` · `filter_field_w 220` · `monogram_size 18` · `mode_word_w 84` · `banner_h 28` · `strip_h 22` |
| Layout columns | `sidebar_w 232` · `rail_w 240` · `detail_w 344` · `detail_overlay_w 320` · `sheet_w 440` · `sheet_expanded_w 640` · `sheet_w_detail 736` |
| Rows and headers | `row_h 30` · `row_h_comfortable 44` · `pane_header_h 30` · `section_header_h 20` · `palette_row_h 34` · `palette_tile 22` · `job_row_h 44` · `progress_bar_h 4` |
| Controls | `button_h 30` · `button_h_compact 26` · `kbd_h 18` · `kbd_h_small 16` · `chip_h 22` · `tile_chip_h 18` · `avatar_size 20` · `text_field_h 36` · `field_status_h 18` · `number_field_w 96` · `segment_h 24` · `switch_w 34` · `switch_h 20` · `checkbox_size 16` |
| Dialogs and floating layers | `dialog_w 560` · `confirm_compact_w 480` · `dialog_header_h 44` · `dialog_footer_h 44` · `palette_w 640` · `palette_top 120` · `prefix_menu_w 900` · `palette_input_h 44` · `overlay_help_w 640` · `toast_w 320` · `toast_inset 12` · `scroll_pill_w 176` · `menu_min_w 240` · `alert_tile 34` |
| Lines and marks | `hairline 1` · `focus_ring_w 2` · `scroll_thumb_w 3` · `dot_size 8` · `dot_size_small 6` |
| Terminal | `cell_w 7.5` · `cell_h 18` · `terminal_tab_min_w 84` · `terminal_tab_max_w 200` · `tab_strip_h 40` · `terminal_tab_h 34` · `tab_close_size 18` |
| Detail and doctor columns | `fact_label_w 104` · `doctor_check_w 120` · `doctor_status_w 64` |
| Git UI | `status_pane_h 62` · `stash_pane_h 92` · `editor_box_h 160` · `diff_row_h 18` · `diff_caret_h 14` · `diff_scrollbar_w 5` · `diff_thumb_min_h 24` · `diff_horizontal_step 4ch` |

`row_h` (30) is the dense row, for menus, pickers and terminal-side lists; `row_h_comfortable`
(44) is the hub-list row. `title_bar_h` is the one top row of every window; `command_field_w` is
the palette field centred in it and `monogram_size` the context switcher's letter tile. `filter_field_w` is the `FilterField` in a page header's toolbar.
`mode_word_w` is the embedded Git UI's status word only — Fleet's own chrome draws no mode word.
`sidebar_w` is the redesigned sidebar; `rail_w` stays while the repos rail is on screen. `kbd_h_small` is the
key chip inside a compact button or a menu item. `segment_h` plus a `SegmentedControl`'s `xxs`
inset and hairline on each side is exactly `row_h`, so the control sits in a settings row or a pane
header without growing it; a `Switch` knob is `switch_h` less an `xxs` inset on each side.

The same token set owns the opacity ladder, and *that* list is complete — derive a variant
with one of these rather than adding a near-duplicate token or a bare float:
`veil 0.55` · `dimmed 0.40` · `refreshing 0.60` · `stale 0.55` · `terminal_blink 0.70` ·
`banner_border 0.35` · `error_hover 0.22` · `neutral_fill 0.08` · `semantic_fill 0.14` ·
`skeleton 0.30` · `no_session 0.30`.

The native-agent canvas has its own constants, and they live in `components::agent::metrics`
rather than in `Metrics`: `AGENT_CONTENT_W 760` · `AGENT_TOOL_KIND_W 60` · `AGENT_CARET_H 17` ·
`AGENT_BODY_MAX_H 240` · `AGENT_WELL_MAX_H 66` · `AGENT_LIST_OVERDRAW 256` ·
`AGENT_SCROLLBAR_INSET 3` · `AGENT_FOLLOW_REARM_PX 40` · `AGENT_USER_MAX_W 456` ·
`AGENT_PREVIEW_MAX_H 144` · `AGENT_PLAN_PREVIEW_H 180` · `AGENT_CONTEXT_METER_W 36`. `Metrics` is the density ladder a theme
may restate; these are fixed product decisions from `NATIVE-AGENTS.md` §5 that no theme may
move, which is exactly why they are constants and not tokens. They are still exported from
`fleet_ui_kit`, so no app-side copy of `760` exists.

---

## 3. Focus and selection conventions

There are exactly **two** blue affordances, and no component may draw the accent color as a
border on its own — it wraps itself in a `FocusRing`.

| Affordance | Rendering | Component |
| --- | --- | --- |
| Focused pane | 2 px inset ring in `focus_ring` | `Pane::focused(true)` → `FocusRing::pane` |
| Cursor row | 2 px bar in `cursor_bar` on the leading edge | `Row::cursor(true)` → `FocusRing::cursor_row` |

Selection is a **background**, not a border: `Row::selected(true)` paints `row_selected`.
Cursor and selection are separate flags because a list can show a selected row while focus lives
in another pane; in that case the row keeps `row_selected` and loses the bar.

Row states, and what each means:

| State | Builder | Rendering | Meaning |
| --- | --- | --- | --- |
| default | — | `bg` | — |
| selected | `.selected(true)` | `row_selected` | the list's current item |
| cursor | `.cursor(true)` | 2 px accent bar | this pane has focus |
| hover | automatic on every row, id or not; off with `.hoverable(false)` | `row_hover` | pointer only, never state |
| hover actions | `.hover_actions(..)` / `RowColumn::hover_only()` | revealed while hovered or selected, width always reserved | pointer twins of row keys, all also in the row menu |
| dimmed | `.dimmed(true)` | 40 % opacity | a row being deleted |
| disabled | `.disabled(true)` | 40 % opacity, no hover | not selectable |
| loading | `SkeletonRows` | 30 % placeholder | cold load only |
| error | leading `StatusGlyph`, not a row color | — | state is a glyph, never a row tint |

The detail panel is **never** in the focus cycle: `j`/`k` always move the list cursor and the
panel mirrors it.

---

## 4. Keyboard hint conventions

- **A hint lives inside the control that performs the action**: in the button, at the right of
  the menu item, in the tooltip of an icon-only button, on the palette or Help row. It is a `Kbd`
  chip resolved from the live keymap for the focused context and never a typed string; an
  action with no binding there shows no chip. Modifier glyphs follow the platform.
- **No footer legends.** A `KeyHintRow` is allowed only on a surface that has no controls to
  carry its keys: the terminal `ExitStrip` and the `ScrollPill`.
- **Lowercase = safe, uppercase = stronger variant.** `FactList::confirm_key()` returns
  `ConfirmKey::Lower` (`y`, and `Enter` is accepted), drawn as the primary button, or
  `ConfirmKey::Upper` (`Y`, and `Enter` is **not** accepted), drawn as a `danger` button. Never
  hand-write that decision.
- **Inside the Workspace every chip carries its prefix.** Terminal mode sends all keys to the
  PTY except `ctrl-s`, so a bare-key chip is forbidden there: `⌃S` `x`, never `x`. The chip gets
  this from the binding itself. The one exception is the ⌃S command menu, which shows only the
  second key because the prefix is already held.
- A list **under a text field** moves with `ctrl-n`/`ctrl-p` or `↓`/`↑` and never `j`/`k`; a
  dialog with no text field does bind `j`/`k`. `FuzzyList::binds_jk()` states which case a list
  is in.
- **An action invalid here is hidden, never greyed**: a greyed row costs a `j`. A control is
  disabled only when it will become valid on this surface without leaving it, such as Save
  before anything changed.

---

## 5. Iconography

Lucide, `stroke-width` rewritten from 2 to **1.5**, embedded from
`crates/fleet-ui-kit/assets/icons/`. Sizes: **16 px** in a list's glyph column and a dialog
header, **14 px** inside chips and pane headers, **12 px** in the status bar and for inline
marks (`IconSize::{Large, Medium, Small}`).

```rust
Icon::Moon.el().size(IconSize::Medium).color(theme.colors.text_secondary)
Icon::LoaderCircle.el().spinning(true).id("jobs-chip")     // one turn per `motion.spinner`
```

gpui allows exactly **one** `AssetSource` per app, so the kit does not register one. Either use
`KitAssets` directly, or delegate from the app's own source:

```rust
gpui_platform::application().with_assets(fleet_ui_kit::KitAssets).run(..)
// or, inside your own AssetSource::load:
if let Some(bytes) = fleet_ui_kit::kit_asset(path) { return Ok(Some(Cow::Borrowed(bytes))); }
```

An absent asset must return `Ok(None)`, never `Err`: `svg()` logs nothing, so an erroring source
turns an invisible icon into an invisible crash.

### 5.1 The closed icon set (82 glyphs)

| Purpose | Icons |
| --- | --- |
| Session state | `circle-dot` `circle` `moon` `dot` `circle-question-mark` |
| Jobs | `loader-circle` `clock` `circle-stop` `circle-slash` `circle-check` `circle-x` `activity` `copy-plus` `refresh-cw` `search-check` `import` `trash-2` |
| Warnings | `triangle-alert` `message-square-warning` `info` |
| Git / PR | `git-branch` `git-branch-plus` `git-merge` `git-fork` `git-pull-request` `git-pull-request-draft` `git-commit-horizontal` `file-diff` `file-pen` `folder-git-2` `eye` |
| Remote | `cloud` `cloud-off` `cloud-upload` `cloud-download` `globe` `lock` `unplug` `server` |
| Keep-alive | `zap` `bot` `sparkles` `server` `file-pen` |
| Terminal | `terminal` `square-terminal` `square-kanban` `chevrons-up` `command` `maximize-2` `plus` |
| Dialogs | `trash` `scissors` `power` `x` `boxes` `arrow-right-left` `settings-2` `hourglass` |
| Chrome | `flag` `circle-arrow-up` `circle-arrow-down` `search` `clipboard-check` `delete` `ellipsis` `check` `minus` `chevron-left` `chevron-right` `chevron-down` `sailboat` |
| Native agent | `brain` `wrench` `square-pen` `paperclip` `minimize-2` `undo-2` `copy` `external-link` `square` `square-check` `shield` `list-checks` |

Two Lucide renames the UX spec predates: `circle-help` is now **`circle-question-mark`**, and
`arrow-up-circle` is now **`circle-arrow-up`**. The kit keeps both `trash` and `trash-2`, since
the former labels destructive dialogs while the latter distinguishes permanent job cleanup.

### 5.2 Status glyph vocabulary (`StatusKind`)

`StatusGlyph` is the single source of truth: the shape a user learns in the worktrees list is
the same shape in the palette and in a confirm.

| `StatusKind` | Icon | Tone | Opacity | Detail word |
| --- | --- | --- | --- | --- |
| `Attached` | `circle-dot` | success | 1.0 | `attached` |
| `DetachedAwake` | `circle` | default | 1.0 | `running, detached` |
| `Sleeping` | `moon` | secondary | 1.0 | `sleeping` |
| `NoSession` | `dot` | muted | **0.30** | `no session` |
| `Unknown` | `circle-question-mark` | **warning** | 1.0 | `unknown` |
| `AgentWorking` | `loader-circle` (spins) | warning | 1.0 | `Agent working` |
| `AgentFinished` | `circle-check` | success | 1.0 | `Agent finished — waiting for you` |
| `Degraded` | `triangle-alert` | warning | 1.0 | `post-create hooks failed` |
| `JobRunning` | `loader-circle` (spins) | warning | 1.0 | `job running` |
| `Cloning` | `loader-circle` (spins) | warning | 1.0 | `cloning…` |
| `CloneFailed` | `circle-x` | danger | 1.0 | `clone failed` |
| `HostUnreachable` | `cloud-off` | warning | 1.0 | `host unreachable` |

`NoSession` is a dim dot, **not** a blank cell; a blank cell means "this column does not apply to
this row". An unreachable host forces the session to `Unknown`, never to `NoSession`.

**Board run marks (`RunMark`, §6.7).** A card whose column runs an action borrows this same
vocabulary rather than inventing one: these are the glyphs above, with the tones above, in the
board's own five states (`BOARD.md` §11.8, `UX-SPEC.md` §Board). They add **no token and no
glyph** — `⊘` is the only new character, and it is text in the data face, not an icon.

| `RunMark` | Glyph | Tone | Harness word |
| --- | --- | --- | --- |
| `Pending` | `Spinner` (`loader-circle`, spins) | secondary | `pending` |
| `Stalled` | `StatusDot::small` | **warning** | `stalled` |
| `Working` | `Spinner` (`loader-circle`, spins) | secondary | `working` |
| `NeedsYou` | `StatusDot::small` | **warning** | `needs you` |
| `Succeeded` | `check` | muted | `done` |
| — (blocked count) | `⊘ n`, `Text::data_small` | muted, or **warning** when a blocker is canceled, archived or gone | `blocked:n` |

`Stalled` and `NeedsYou` are deliberately the same amber dot: both mean "this card wants you", and
a tile is not the surface that explains which — the card detail's run row is. The spinner is
secondary rather than the `AgentWorking` amber above because a run in progress is *progress*, and
§1 reserves amber for what needs a person (`NATIVE-AGENTS.md` §2: gray is everything else,
including progress). The card detail draws the same three glyphs from its own table; the two must
stay identical.

---

## 6. Component catalog

Almost every component is a `RenderOnce` + `IntoElement` struct with a `new`-style constructor
and chained builder methods, and owns no state: the view's entity owns the cursor, the query,
the focus and the timers, and passes them down each frame. That is deliberate — `RenderOnce`
cannot hold state, and the alternative (an entity per component) would make cursor stability
under background updates impossible to reason about.

There are exactly four exceptions, and they are gpui **entities**:
`TextInput` (§6.4), which owns a caret, a selection, an undo history, a painted-layout cache and
an IME session (ADR 0020), and `TranscriptList` and `MultilineInput` (§6.6), which own measured
row geometry, a scroll machine, and — for the composer — the `TextInput` it wraps; and `Menu`
(§6.8), which owns a `FocusHandle` and a highlight while it is open. The surface holds the first
three; a menu is held by the wrapper that opened it (`PopoverMenu`, `ContextMenu`, `Dropdown`) in
gpui element state keyed by the wrapper's id, so a screen keeps no field per menu. Nothing else
in the kit implements `Render` except the view behind `Tooltip` (§6.8), which holds no state at
all: gpui's tooltip slot takes an `AnyView`, so the tooltip is built as a throwaway entity each
time it opens.

Legend for the "states" rows: **default · focused · selected · disabled · loading · error**.
A component that cannot be in a state says so rather than pretending.

**Harness targets.** `fleet_ui_kit::harness` records where a named element painted, so an
end-to-end scenario can click a row by name. `HarnessTargetExt` is blanket-implemented for every
`IntoElement`, which is how `fleet-app` names the surfaces it builds itself; the whole cost with
harness mode off is one flag read and one branch. Four components take typed items rather than
elements, so their names live here instead of at the call site: `ToastStack` records
`toasts.toast[N]`, `Palette` records `palette.input` and `palette.row[N]`,
`TerminalTabStrip` records `tabs.tab[N]` and — past `TerminalTabStrip::agents_from` —
`agents.tabs.tab[N]`, the decision drawer records the approval controls, and an open `Menu`
records `menu.item[N]` over its visible items, separators and headers skipped. Two more take the
name from the caller, because the same component appears under different surfaces:
`SegmentedTabs::harness_tabs` and `FuzzyList::harness_rows`. A name is never invented here;
`docs/TESTING-HARNESS.md` §3 is authoritative for the vocabulary.

The wrapper's contract is a frame stamp, not a cache: a recorded rectangle carries the frame it
painted on, and the snapshot reports it beside `window.frame`. A scenario that clicks a target
whose stamp is older than the current frame is clicking a rectangle that is no longer there, so
the driver refuses it and names both frame numbers instead of dispatching at stale coordinates.
That is why a component records on every paint and never keeps last frame's bounds.

### 6.1 Foundation

#### `Theme` / tokens
**Purpose.** One resolved token set per appearance, installed as a gpui `Global`.
**API.** `Theme::{dark, light, for_mode, init, change, toggle, is_dark}`,
`Theme::{resolve_mono_family, with_mono_family, shadow, dialog_shadow, sheet_shadow,
popover_shadow}`, `ThemeMode::{toggled, is_dark}`,
`trait ActiveTheme { fn theme(&self) -> &Theme }`, and
`theme::contrast_ratio(fg, bg) -> f32` with `theme::CONTRAST_AA` (4.5) for §2.1's contrast rule.
**Usage rule.** Read the theme **inside** `render`, never cache it in a struct: the mode can
change between frames. Clone it (`cx.theme().clone()`) only when a `'static` closure needs it.

#### `Icon` / `IconElement`
**Purpose.** One Lucide glyph, tinted by a token.
**Anatomy.** `svg()` with a size, a text color, and optionally a rotation animation.
**API.** `Icon::<Variant>.el() -> IconElement`, then `.size(IconSize) .color(Hsla) .opacity(f32)
.spinning(bool) .id(ElementId)`. Also `Icon::{name, path, bytes, from_path, ALL}`.
**States.** default · spinning · dimmed. Icons are never focusable or disabled on their own.
**Usage rule.** Never `svg()` directly: an `svg()` with no text color renders nothing and logs
nothing. Use `Spinner` rather than `.spinning(true)` when the glyph *is* the spinner.

#### `Text`
**Purpose.** A run of text in one of the eleven type roles (§2.3).
**API.** `Text::{page_title, section_title, ui, ui_strong, title, caption, sentence_label, data,
data_small, label, hint}(impl Into<SharedString>)`,
then `.tone(Tone) .muted() .faint() .color(Hsla) .opacity(f32) .weight(FontWeight)
.truncate_at(usize, Truncate) .ellipsize() .w(Pixels) .w_ch(f32) .flex_none()`. `Text::resolved_text()`
returns the string after the `ch` budget, for tests.
**Usage rule.** `truncate_at` when the spec names a `ch` budget; `ellipsize` when the column is
flex. A redesigned surface uses `sentence_label` with the string in sentence case; the legacy
`label` uppercases for you — do not pre-uppercase the string.

#### `Truncate`
**Purpose.** Head / middle / tail ellipsis at a `char` budget.
**API.** `truncate(&str, budget: usize, Truncate) -> SharedString`, `Truncate::{Head, Middle, Tail}`.
**Usage rule.** `Head` for `owner/name` (the name matters), `Middle` for branches and paths
(both ends matter), `Tail` for PR titles (the start matters).

#### `FocusRing`
**Purpose.** The only two blue affordances.
**API.** `FocusRing::pane(bool).child(..)`, `FocusRing::cursor_row(bool).child(..)`.
**Usage rule.** Do not call it directly in a view — `Pane` and `Row` already wrap themselves.

#### `Tone`
**Purpose.** Semantic color selection.
**API.** `Tone::color(&Theme) -> Hsla`, `Tone::fill(&Theme) -> Hsla`.
**Usage rule.** A component takes a `Tone`. It takes an `Hsla` only when the color is already
resolved from the terminal palette.

### 6.2 Structure

#### `AppFrame`
**Purpose.** Title bar (44) + optional banner (28) + flexible body + status bar (28), plus the
overlay layer.
**API.** `AppFrame::new().title_bar(..).banner(..).body(..).body_overlay(..).status_bar(..)
.overlay(..)`.
**Variants.** Hub (body = panes) · Workspace (body = terminal) · first run (body = one card).
**Usage rule.** `overlay` is the window-wide layer for dialogs and the palette; `body_overlay`
is the band between the bars for `Sheet` and `ToastStack`. Putting a sheet or toast in `overlay`
covers the status bar. Floating children never reflow what the user was looking at. The
Workspace keeps both bars at the same pixel positions as the Hub — same chrome, same saccade.

#### `OverlayLayer`
**Purpose.** The shared deferred-paint ordering for floating surfaces.
**API.** `OverlayLayer::{Sheet, Anchored, Dialog, Toast, Menu}.priority()` resolves to
`Sheet(100) → Anchored(200) → Dialog(300) → Toast(400) → Menu(500)`.
**Usage rule.** Never pass a literal deferred priority. This order encodes the Jobs sheet,
anchored palette, blocking dialogs, transient acknowledgements, and the open menu the pointer is
on — which closes on the next click anywhere, so nothing may slide over it. A menu opened inside
a dialog or sheet is a nested `deferred` and paints after that surface regardless of the number.

#### `SplitLayout`
**Purpose.** A fixed region, a hairline, and a flexible region.
**API.** `SplitLayout::{horizontal, vertical}().leading(..).trailing(..).leading_size(Pixels)
.trailing_size(Pixels).divider(bool)`.
**Usage rule.** The Hub is two nested splits: `rail | (list | detail)`. Fix the **rail** and the
**detail panel**, never the list — that is what makes "the rail never moves when `i` opens the
detail panel" a property of the layout instead of a convention.

#### `TitleBar`
**Purpose.** The one 44 px row at the top of every window: where you are, a way to search or run
anything, and what needs you. It is the unified macOS titlebar.
**Anatomy.** `[traffic-light inset][leading …][CommandField, centred in the window][… trailing]`.
In the Hub, leading is the context switcher (a `SwitcherButton` with a monogram, opening a
`PopoverMenu`) and the section nav (a `SegmentedControl` with counts); in the Workspace it is the
breadcrumb `← Worktrees / repo / worktree`. Trailing is the status cluster — `StatusButton`s for
what needs you, jobs, an update and an unhealthy daemon, each only while non-zero — then Help and
Settings as `IconButton`s.
**API.** `TitleBar::new().leading_inset(Pixels).leading(..).center(..).trailing(..)`; `leading`
and `trailing` append, left to right.
**States.** Hub · Workspace · first run (empty: there is nowhere to go yet) · busy (status buttons
present) · daemon unhealthy (amber `Reconnecting…` / red `fleetd down` pill).
**Usage rule.** Once per window, through `AppFrame::title_bar`; the frame owns the height. The two
side regions split the width either side of the field equally and clip, so a narrow window loses
the ends of its labels, never the field. A floating window of its own (the agent popup) draws a
header, not a second title bar. The default inset is the `md` gutter; pass
`metrics.traffic_light_inset` on macOS.

#### `CommandField`
**Purpose.** A button drawn as a search field — magnifier, `Search or run a command`, key chip —
that opens the palette.
**API.** `CommandField::new(id, placeholder).action(Box<dyn Action>).kbd(Kbd)`.
**Usage rule.** It is not an input: the click dispatches the palette's open action and the typing
happens in the palette. `command_field_w` wide, `button_h` tall. The chip resolves from the live
keymap like every `Button`'s.

#### `StatusBar`
**Purpose.** Where you are and what is running: daemon · breadcrumb · job ticker or sticky error ·
trailing buttons.
**API.** `StatusBar::new().daemon(DaemonState, Option<word>).breadcrumb(..).breadcrumb_ch(usize)
.ticker(..).error(..).trailing(..)`; `trailing` appends.
**Variants.** Hub (`Shortcuts ?`) · Workspace (`Fleet commands ⌃S`, `Shortcuts ⌃S ?`) · with ticker ·
with error (the error **replaces** the ticker) · daemon unhealthy (`fleetd unreachable` in the
daemon's tone).
**Usage rule.** There is no mode word (ADR 0023): a state that changes what keys do is shown on the
surface that has it — the scroll pill, the filter bar, the open overlay, the focused pane, the ⌃S
command menu. A healthy daemon is a green dot and `fleetd`, nothing more.

#### `Pane`
**Purpose.** A bordered region with a header slot, a body slot, a scroll thumb and the focus ring.
**API.** `Pane::{new, fixed(Pixels)}().header(..).body(..).footer(..).focused(bool)
.width(Pixels).border(PaneBorder).raised(bool).scroll_thumb(offset: f32, visible: f32)`.
**States.** default · focused (2 px ring) · scrolled (3 px thumb).
**Usage rule.** `Pane::fixed` for the 240 px rail and the 344 px detail panel; `Pane::new` for
the list, which must flex. Only one pane in a screen is `focused` at a time.

#### `PaneHeader`
**Purpose.** Label · scope · `shown/total` · visible range · stale stamp.
**API.** `PaneHeader::new("worktrees").scope(..).shown(usize).total(usize).range(first, last)
.stale(age).filter_chip(query).filter(impl IntoElement).trailing(..)`.
**Variants.** normal · filtering (`.filter(FilterBar::..)` replaces the left side **in place**) ·
filter retained (`.filter_chip("rut")`) · stale (`· stale · 2m`, amber).
**Usage rule.** Never swap the header element for a filter bar — pass the filter bar *through*
the header, or the row shifts by a pixel and the illusion of "the list did not move" breaks.

#### `PageHeader`
**Purpose.** The top of a Hub page: an H1 in `page_title`, one summary line under it, and the
page's toolbar right-aligned to the title's baseline.
**API.** `PageHeader::new(title).badge(impl IntoElement).subtitle(text).stale(age)
.fact(impl IntoElement).action(impl IntoElement)`; `fact` and `action` append. The badge sits after
the title on its line (a board's prefix); facts follow the subtitle on the summary line (the
board's clickable `1 needs you`, its dirty and conflict counters, a spinner).
**Variants.** normal · stale (`· stale · 2m`, amber, after the subtitle) · with badge and facts
(the board header).
**Usage rule.** The subtitle is a sentence the view model built in its update path
(`4 across 2 repositories · 1 needs attention`); the header never counts anything. The toolbar
holds the page's own controls — a `FilterField`, secondary `Button`s, at most one primary
`Button` — each showing its key. Not a `PaneHeader` (the dense 30 px pane label row) and not a
`SectionHeader` (a block inside a panel).

#### `FilterField`
**Purpose.** A page or board header's filter box: magnifier, the query or a placeholder, and the
key that opens it; while editing, the live editor and `shown/total`. The one filter box of the
redesigned surfaces — use it for the Worktrees page and the board header alike.
**API.** `FilterField::new(id, placeholder)` then `.query(text)` (the retained value, shown idle)
`.editor(Entity<TextInput>)` (the surface is editing: draw the editor) `.counts(shown, total)`
`.action(Box<dyn Action>)` (a click dispatches it, and its live key is the chip) `.on_click(handler)`
(a click runs it; for a filter that is not an action) `.kbd(Kbd)` (chip override)
`.on_clear(handler)` (a `✕` while a query is set, idle or editing) `.width(Pixels)` (default
`filter_field_w`); `.is_editing()`, `.count_tone()`.
**States.** idle (placeholder, key chip) · retained (the query in body text) · clearable (`✕`) ·
editing (the embedded editor, a `focus_ring` border, `shown/total`) · no match (the count turns
amber).
**Usage rule.** One box, two faces, so opening the filter moves nothing. The field takes no focus
itself: the surface owns the editor (built with `TextInput::set_embedded`), focuses it when its
filter mode opens, and owns the keys — as with `FilterBar`, which stays the in-place form for a
dense pane header. `button_h` tall.

#### `Sheet`
**Purpose.** A docked panel (the Jobs panel; the board card detail).
**API.** `Sheet::new(open: bool).expanded(bool).width(Pixels).side(SheetSide).scrim(bool)
.dismiss_action(Box<dyn Action>).on_dismiss(handler).header(..).body(..).footer(..)`;
`.resolved_width(&Theme)`.
**Variants.** 440 px · 640 px expanded for an inline log · `sheet_w_detail` 736 px for a full
detail · `SheetSide::Right` (default) or `Left` · dismissable (close ✕ at the top right of the
header, painted `sheet.close`; a click in the band outside the panel closes) · `scrim(true)`
darkens that band with `overlay`.
**Usage rule.** Use a `Sheet`, not a `Dialog`, whenever the content is *about* the rows behind
it. A centered modal would hide exactly what the jobs refer to. Leave the scrim off for such a
sheet so the rows stay readable; turn it on for a sheet that is a detail of its own. Pass
`dismiss_action` the action `esc` runs on the sheet, so the ✕, the click outside and the key are
one path.

#### `Dialog`
**Purpose.** The shared modal frame: scrim + card + 44 px header + 44 px footer.
**Anatomy.** header = icon + title + subtitle — or, with `header_fill`, one control in their place
that takes the free width (Help's search field) — then `header_actions` (a search field, a
segmented control) and the close ✕ at the right; body = padded, or edge to edge with
`flush_body(true)` for a dialog that lays out its own panes (Help's sidebar and content);
footer = `footer_start` (a link-like control such as "Open config.json", after any legacy
hints) on the left, the `actions` buttons right-aligned —
secondary first, the one primary last, each with its key chip.
**Alert form.** `.alert()` draws the header as an alert (a confirm): the icon in an `alert_tile`
square filled with the `tone`'s fill, the title beside it and the subtitle on its own line under
the title, with no rule and no ✕ — the footer's `Cancel` and a click outside close it — and the
body indented to the title's edge.
**API.** `Dialog::new(title).alert().icon(Icon).subtitle(..).width(Pixels).height(Pixels).tone(Tone)
.dismiss_action(Box<dyn Action>).on_dismiss(handler).header_fill(..).header_actions(..)
.flush_body(bool).body(..)
.footer_start(..).actions(Vec<Button>).error(..).warning(..)`. Deprecated while the dialogs
migrate: `.hints(..)`, `.hint_row(KeyHintRow)` and `.primary("⏎ Create")`, the bold label drawn
when there are no `actions`.
**Dismissal.** Either dismiss builder draws the ✕ and arms the scrim: a click outside the card
runs it, a click inside never does. `dismiss_action` is the normal form — pass the action `esc`
runs in that dialog; the pointer dispatches it to the focused element as the key would, and the
✕'s tooltip shows that key from the live keymap. `fleet-app` takes it from
`Dialogs::dismiss_action()`, which a test holds to the keymap's `escape` rows.
**Harness.** The ✕ paints `dialog.close`; footer buttons paint `dialog.button[N]`, `0` leftmost.
An alert has no ✕, so it paints no `dialog.close`.
**Widths.** 460 context/assign · 480 compact confirm · 520 quit · 560 create/clone/expanded
confirm · 720 settings/prune · 880 card detail · 1040 help (clamped to the window).
**States.** default · error (red footer line, dialog stays open).
**Keyboard.** `Esc` closes. Buttons are not focusable (ADR 0023): each shows the key that is
its keyboard path.
**Usage rule.** `Dialog` always ghosts the base screen. `Overlay` can do so when configured with
`scrim(true)` and `OverlayLayer::Dialog`, as the floating Agent terminal does. Use `ConfirmDialog`
for anything destructive so the `y`/`Y` escalation is computed, not typed.

#### `Overlay`
**Purpose.** A centered floating layer used by the top-anchored palette and the Agent popup.
**API.** `Overlay::new().top(Pixels).width(Pixels).scrim(bool).layer(OverlayLayer)
.popover_elevation(bool).dismiss_action(Box<dyn Action>).on_dismiss(handler)
.dismiss_on_scrim_click(bool).content(..)`.
**Variants.** dialog elevation (default: `radii.dialog`, level-3 shadow) · popover elevation
(`radii.popover`, level-4 shadow).
**Dismissal.** A scrimmed overlay with a dismiss closes on a scrim click unless
`dismiss_on_scrim_click(false)` (default `true`); it draws no ✕ of its own.
**Usage rule.** Default `top` is 120 px — the thinking position, not screen center. Leave
`scrim` off and use `OverlayLayer::Anchored` for the palette: it is a jump, not a decision. The
Agent popup is a modal floating surface, so it opts into the scrim and `OverlayLayer::Dialog`,
and into the popover elevation so the floating terminal reads as a window above the app. It sets
no dismiss: its card hides it on a scrim press itself, as `ctrl-q` does. The palette closes on a
scrim click through `palette::Close`.

#### `Toast` / `ToastStack`
**Purpose.** Bottom-right transient acknowledgements, max 3.
**API.** `Toast::new(text).icon(Icon).tone(Tone).short().raised_at(ms)`;
`ToastStack::new(toasts).max(usize).bottom_inset(Pixels)`;
`ToastStack::{MAX, is_visible}`, `COALESCE_WINDOW_MS`, and
`ToastStack::{push, push_at}` apply the coalescing law.
**Variants.** 1.6 s short · 3.2 s normal · coalesced (`×n`).
**Usage rule — the toast law.** *A toast is allowed only when there is no row and no pill that
already shows the outcome.* Never toast: job started, job succeeded with its row on screen,
worktree created, PR refreshed, context switched, session opened, settings saved, update
available, **and any error** — errors are sticky. `Toast::tone` refuses `Tone::Danger` to enforce
that rule.

### 6.3 Data display

#### `ListView` / `ListCursor`
**Purpose.** A virtualized list of fixed-height rows with a stable cursor.
**API.**
```rust
ListView::new(id, item_count, |ix, is_cursor, window, cx| -> AnyElement { .. })
    .cursor(usize).row_height(Pixels).track_scroll(&UniformListScrollHandle).empty(..)
    .loading(bool).skeleton_rows(usize)
ListView::reveal(&handle, &cursor, moving_down);           // call from the action, not render

ListCursor::new(len).scrolloff(2).page(10)
    .down() .up() .first() .last() .page_down() .page_up() .set(ix) .set_len(len)
    .motion(ListMotion) .set_page_from_visible(usize)
    .retain(new_len, |previous| Option<usize>)             // keep the cursor on its item
    .scroll_target(moving_down) -> usize
ListCursor::{scrolloff_rows, page_rows}; list_key_bindings(Option<&str>)
ListMotion::{Down, Up, First, Last, PageDown, PageUp}

// pointer (ADR 0023, UX-SPEC §5.1): the list routes every press through one ListPointer
ListView::new(..).on_select(|ix, window, cx| ..).on_open(|ix, window, cx| ..)
    .on_menu(|ix, position, window, cx| ..)     // or .pointer(ListPointer)
ListPointer::new().on_select(..).on_open(..).on_menu(..)
    .attach(ix, Row) -> Row                     // rows not inside a ListView
    .press(ix, &MouseDownEvent, window, cx)
RowPress::classify(MouseButton, click_count) -> Option<RowPress::{Select, Open, Menu}>
```
**States.** default · empty (`EmptyState`) · loading (`SkeletonRows` in the body instead).
**Keyboard.** `j`/`k`, `gg`/`G`, `ctrl-d`/`ctrl-u` — the view binds them and calls `ListCursor`.
**Pointer.** A primary press selects the row (`on_select`, which does what `j`/`k` do minus the
motion); the second press of a double-click selects and then opens it (`on_open`, the twin of
`⏎`); a right click selects and then asks for the row's menu at the pointer (`on_menu`). Every
press selects first, so `on_open` and `on_menu` act on the cursor row exactly as their keys do.
The list wraps each row only when a handler is set; a keyboard-only list keeps its element tree.
**Usage rule — cursor stability.** Background events (status polls, PR fetches, job completions)
must **never** re-sort, re-scroll or re-focus. Adopt new data with `ListCursor::retain`, and
re-sort only on an explicit user action (`r`, filter change, repo change, screen change).
Use `ListView` for lists; use `TerminalGrid` for a terminal — `uniform_list` assumes one element
per item, which is far too much overhead per terminal row.

#### `Row` / `RowColumn`
**Purpose.** One list row: leading glyph slot, flex content, trailing columns.
**API.** `Row::{new, with_id}().leading(..).leading_width(Pixels).column(RowColumn).columns(..).second_line(..)
.details(..).height(Pixels).comfortable().selected(bool).cursor(bool).dimmed(bool).disabled(bool)
.hoverable(bool).hover_actions(..).on_click(..).on_double_click(..).on_secondary_click(..)`;
`RowColumn::{fixed(Pixels, ..), fixed_ch(f32, ..), flex(..), auto(..), resolved(..)}
.align(ColumnAlign).min_width(Pixels).min_width_ch(f32).hover_only()`. The leading slot is
2 ch wide; `leading_width` widens it for an element that is not a glyph (the palette's
`palette_tile`), and every row of one list passes the same width.
**Variants.** 30 px one-line · 44 px two-line (`second_line`) · 44 px comfortable
(`comfortable()`, `row_h_comfortable`: the Hub lists; primary cell `Text::ui_strong`, secondary
cells `Text::ui(..).muted()`) · 34 px palette row · grown (`details`: the line keeps its
height and a block — a job's progress bar, inline error and buttons — sits under it, starting at
the first text column; the row is then of variable height, so its list must be a measured
`gpui::list`, never `ListView`).
**States.** the table in §3.
**Pointer.** Handlers take `(&MouseDownEvent, &mut Window, &mut App)`, so a view can pass a
`cx.listener(..)`. They fire on the **press**, as native lists select, and need no id: `on_click`
on a single press, `on_double_click` on the second press of a pair (click count ≥ 2; without it
a double-click is two selects), `on_secondary_click` on a right click. Any handler implies the
pointer cursor; a disabled row takes none. A row with indices wires all three through
`ListPointer::attach`. `hover_actions` is the trailing slot for the mockup's `Open ⏎` and `⋯`:
drawn while the row is hovered or selected, its width reserved either way so revealing it never
reflows the row. In a list with a `ListHeader`, make it a ladder column instead —
`RowColumn::resolved(actions, ..).hover_only()` — so the header reserves the same width.
**Usage rule.** A row that changes state changes its **glyph** in place; it does not change its
background color and it does not move. Leave `leading` unset when the column does not apply —
that is the blank cell of §2.5.

#### `ListHeader`
**Purpose.** The column heads above a laddered list: one line of sentence-case `caption`
labels over a hairline. Not a `SectionHeader`, which titles a group of rows.
**API.** `ListHeader::new().reserve_leading(bool).column(&ResolvedColumn, label)`.
**Usage rule.** Build it from the **same** `ColumnLadder::resolve(pane_ch)` the rows use, and
match the rows' `reserve_leading`; it renders through `Row` itself, so padding, gap and column
boxes cannot drift. A column with no head (the hover-only actions column) gets an empty label
and still reserves its width.

#### `ColumnLadder`
**Purpose.** Resolve a `ch`-based responsive column set for the current pane width.
**API.** `ColumnLadder::{new, worktrees}()`, `.column(ColumnSpec)`,
`.resolve(pane_ch) -> Vec<ResolvedColumn>`, `.shows(key, pane_ch) -> bool`,
`.width_ch(key, pane_ch)`, `.specs()`, `ColumnLadder::pane_ch(width, cx)`,
`ColumnLadder::{worktrees_in_scope, pull_requests_for}`;
`ColumnSpec::{fixed(key, ch), flex(key, min_ch), ladder(key, &[(pane_ch, ch)])}
.align(..).shown_from(pane_ch).forced(bool)`.
**Usage rule.** Measure the **pane**, not the window. `ColumnLadder::worktrees()` and
`::pull_requests_for()` are the §2.9 ladders verbatim, including the two-step author breakpoint
(12 ch at 70 ch, 16 ch at 130 ch). The worktrees ladder is keyed `branch` (Name, flex 24) ·
`repo` (12, from 110 ch or forced in `All`) · `session` (18 / 14 / 0 ch at 100 / 72 ch) · `pr`
(15, from 60 ch) · `age` (5, from 52 ch) · `actions` (13, always, filled `hover_only`).

#### `StatusGlyph`
**Purpose.** The §2.5 vocabulary, in one place. See §5.2 for the table.
**API.** `StatusGlyph::new(StatusKind).size(IconSize).id(ElementId)`;
`StatusKind::{icon, tone, opacity, spins, frozen, detail_word, detail_sentence}`.
**Usage rule.** Never assemble a session glyph from an `Icon` and a color: a second call site is
how `unknown` starts rendering like `none`, which is the exact live defect §1.3 exists to close.
Spinning kinds need `.id(..)`.

#### `Chip`
**Purpose.** The 22 px pill of the row chrome.
**API.** `Chip::{new, counter(Icon, usize), labeled(Icon, text)}().icon(..).text(..).count(..)
.tone(Tone).color(Hsla).filled(bool).spinning(bool).id(..).zero_suppress(bool)`;
`Chip::is_visible()`.
**States.** default · filled · spinning · suppressed (renders nothing).
**Usage rule.** Chip = icon + count/word in the *chrome*. For a fixed vocabulary word inside a
row column, use `Badge`. For a liveness dot with no glyph, use `StatusDot`.

#### `Badge`
**Purpose.** A short word carrying a tone, inside a row or a list.
**API.** `Badge::new(text).tone(Tone).style(BadgeStyle::{Bare, Filled, Outlined}).color(Hsla)`.
**Usage rule.** `Bare` by default — a short word does not need a box. Reserve `Filled` for a
word that must survive on a selected row.

#### `StatusDot`
**Purpose.** A filled dot with no glyph: daemon liveness (8 px) and tab activity (6 px).
**API.** `StatusDot::{new(Tone), small(Tone)}().size(Pixels).color(Hsla).opacity(f32)`.
**Usage rule.** Only two dots exist in Fleet. Anything else that expresses state is a
`StatusGlyph`, because a shape carries more than a color.

#### `KeepAliveChips`
**Purpose.** `⚡ claude, :3000` — why `sleep` will refuse to close windows, and what `K` kills.
**API.** `KeepAliveChips::new([KeepAliveLabel::with_icon("claude", Icon::Bot), ..])
.max_visible(3).width_ch(f32).show_bolt(bool).show_kind_icons(bool)`;
`KeepAliveChips::from_pane_ch(pane_ch) -> f32` resolves the 18 / 14 / 10 / 0 ch ladder;
`.is_visible()`, `.resolved_text()` for tests.
**Usage rule.** `DegradedChip` **outranks** this in the same row slot.

#### `DegradedChip`
**Purpose.** `⚠ hooks failed` with the key that opens the log.
**API.** `DegradedChip::{hooks_failed, new(text)}().hint(key, label)`.
**Usage rule.** A worktree that looks ready but whose post-create hooks failed is a trap; this
chip persists until the hooks job succeeds or the fact is dismissed from the detail panel.

#### `PrBadge`
**Purpose.** `#n` + one icon + one word (≤ 8 characters), collapsing three GitHub fields.
**API.** `PrBadge::{new(number, PrBadgeState), state_only(PrBadgeState)}().stale(bool).chip()`;
`PrBadgeState::{icon, tone, word, chip_word, chip_tone}`.
**Variants.** `Draft` faint · `CI fail` red · `Changes` amber · `CI ···` amber · `Approved`
green · `Review` secondary · `Merged` green. **Chip** (`.chip()`): a tinted pill reading `#n` and
the chip word — `Draft` · `CI failing` · `Needs changes` · `CI running` · `Approved` ·
`In review` (accent) · `Merged` — for a comfortable Hub row or a detail panel.
**Usage rule.** The caller resolves the priority (draft → ci_fail → changes → ci_pending →
approved → review, merged overrides all) so the kit stays free of GitHub types. `.stale(true)`
is the §2.6 rendering for a mark derived from a fact older than 10 minutes.

#### `AgeLabel`
**Purpose.** A relative time in exactly one unit.
**API.** `AgeLabel::{from_secs(i64), none(), text(..)}().tone(Tone).mono(bool)`;
`format_age(i64) -> String`.
**Usage rule.** `AgeLabel::none()` renders `–`; a nullable *fact* uses `FactValue::Null` (`—`).
Never two units, never an absolute timestamp in a row.

#### `FreshnessStamp`
**Purpose.** `checked 14s ago · I re-check`, with the §2.6 contrast ladder.
**API.** `FreshnessStamp::new(verb, age_secs).action(key, label).trailing(element).error(message)
.refreshing(bool)`; `trailing` puts a control after a `·` — the confirm's `Re-check` button, where
a pointer-first surface drops the `action` key hint;
`Freshness::{from_secs, tone, derived_opacity, draws_derived_mark}`.
**Variants.** ≤ 60 s normal · ≤ 10 min secondary · > 10 min amber (derived marks drop to 55 %) ·
errored red with the message verbatim.
**Usage rule.** Mandatory on every facts confirm and on the detail panel's SAFETY header. A fact
without an age is not a fact.

#### `FactRow` / `FactValue` / `KeyValueList`
**Purpose.** `label  value` with the null rule, and the block that holds them.
**API.** `FactRow::{new(label, FactValue), warning(message)}().label_width(Pixels).mono(bool)
.refreshing(bool)`;
`FactValue::{known, warning, from_option, Null}`;
`KeyValueList::{new, titled(title), titled_with_trailing(title, trailing)}().row(label, value).mono_row(label, value)
.label_width(Pixels).refreshing(bool)`.
**Usage rule.** Use `FactValue::from_option` for every nullable inspection fact. Warnings are
rendered **verbatim** — swarm's soft-warning strings are greppable diagnostics and must never be
paraphrased.

#### `Fact` / `FactList`
**Purpose.** Risks first, safe facts after, and the decision of which confirm to show.
**API.** `Fact::{safe, risk, unknown}(text).strong(lead)` — `strong` sets the leading words in
bold (the `section_title` weight), the number a reader scans for (`**3 uncommitted files**`,
`**2 commits** not on origin/main`), and is ignored unless the text starts with `lead`. A risk
reads in `text` beside an amber `triangle-alert`; a safe fact in `text_secondary` beside a green
`check`;
`FactList::{new, from_facts}().fact(Fact).loading(bool)`,
`.ordered()`, `.is_compact()`, `.risk_count()`, `.unknown_count()`,
`.confirm_key() -> ConfirmKey`.
**Usage rule.** Never hand-write the `y` vs `Y` decision: `confirm_key()` returns `Upper` when
any decisive fact is unknown or the facts are still loading, and `ConfirmKey::accepts_enter()`
says whether `Enter` also confirms.

#### `CopyField`
**Purpose.** A read-only value in a field box with the button that copies it: the detail
panel's worktree path.
**API.** `CopyField::new(value).button(impl IntoElement)`.
**Usage rule.** Mono, ellipsized, not focusable and not an input. The button is the caller's
`IconButton` carrying the copy action, so its tooltip shows the key (`y`). A value that needs no
action is a `FactRow`; one the user edits is a `TextInput`.

#### `InfoCard`
**Purpose.** A raised, rounded box grouping a few lines about one thing inside a detail panel:
the worktree panel's *Session* card.
**API.** `InfoCard::new().title(text).trailing(impl IntoElement).line(impl IntoElement)`;
`line` appends.
**Usage rule.** `surface_raised`, a hairline, `radii.card`, no shadow and no actions of its own —
the panel's action row owns those. Not a `Callout` (a consequence inside a dialog) and not a
`KeyValueList` (flat facts); leave facts flat when the panel already reads as sections.

#### `SectionHeader`
**Purpose.** A 20 px label row with an optional right-aligned stamp or action.
**API.** `SectionHeader::new(label).trailing(..)`.

#### `EmptyState`
**Purpose.** Exactly two centered lines: the fact, then the key — or, on a redesigned page, the
fact and the one button that fixes it.
**API.** `EmptyState::new(fact).action(key_line).button(impl IntoElement)`.
**Variants.** key line · button (`No worktrees yet` over a primary `New worktree  n`; the button
replaces the key line, so the key is taught where it is used).
**Usage rule.** Rendered **in the affected pane only**, never full-screen, so neighbouring panes
stay usable. Copy is swarm's, verbatim.

#### `SkeletonRows`
**Purpose.** 30 % placeholder rows.
**API.** `SkeletonRows::new(count).row_height(Pixels)`.
**Usage rule.** Cold load **only** — used exactly once, for a cold PR fetch. Everything else
renders from `state.json` immediately; a skeleton where cached truth exists is a lie.

#### `KeyHint` / `KeyHintRow`
**Purpose.** A key and its label as text, for the two surfaces with no control to carry a key
(§4). Everywhere else a control shows a `Kbd` (§6.8); existing hint rows move over as their
surfaces are rebuilt.
**API.** `KeyHint::{new(keys), labeled(keys, label)}().label(..).tone(Tone).key_tone(Tone)`;
`KeyHintRow::new().hint(KeyHint).key(keys, label).merge(other)`.
**Usage rule.** See §4 — prefix every Workspace hint.

#### `DoctorTable`
**Purpose.** `CHECK STATUS DETAIL`, red on fail.
**API.** `DoctorTable::new([DoctorRow::new(check, DoctorStatus::{Ok, Warn, Fail}, detail)])
.check_width(Pixels).status_width(Pixels)`; `DoctorStatus::icon()`.
**Usage rule.** `ok` renders in the **secondary** tone, not green: zero-suppression at the color
level. Good news does not get a hue.

#### `Divider`
**Purpose.** A 1 px hairline.
**API.** `Divider::{horizontal, vertical}().inset(bool)`.

#### `Spinner`
**Purpose.** `loader-circle` turning once per second — the only looping animation in Fleet.
**API.** `Spinner::new(id).size(IconSize).tone(Tone).color(Hsla)`;
`SpinnerWithLabel::new(id, label)`.
**Usage rule.** The id must be stable across frames or the animation restarts every frame.
Amber by default, because "in flight" is amber everywhere.

### 6.4 Input

Inputs are live entities owned by the surface. The caller holds an `Entity<TextInput>`, focuses
its `FocusHandle`, reads `text()`, and handles no editing keys around it. There is exactly one
input component (ADR 0020); a value that cannot be edited is not an input at all but a read-only
`FactRow` or a `Label`, with no box — and so is a resting row of a list where only the row under
the cursor is edited at a time (Settings, Board settings). `MarkdownText` is grouped here because it is the read half
of the description surface a multi-line `TextInput` edits.

#### `TextInput`
**Purpose.** Fleet's live IME-safe editor for single-line values and logical multi-line bodies.
It owns the buffer, selection, undo history, clipboard bridge, focus, input handler, layout
cache and scrolling.
**Anatomy.** optional label · 36 px single-line box or `min_rows`–`max_rows` multi-line box ·
optional leading icon · placeholder/value · selection · caret · marked-text underline. A
single-line input reserves an 18 px status slot containing either preview or validation, in the
`Caption` role (the UI face: a hint is prose, not code); a
multi-line input has no status slot. Multi-line values and placeholders soft-wrap at the box
width, and `min_rows` / `max_rows` count visual rows. Single-line mode never wraps and scrolls
horizontally instead. Both use the border ladder danger → focus → rest.
**API.** `TextInput::new(InputMode, cx)`, `text()`, `set_text(text, cx)`, `clear(cx)`,
`insert(text, cx)` (filtered user-style insertion that replaces the selection in one undo step),
`select_all(cx)`, `move_to_end(cx)`, `set_placeholder(value, cx)`, `set_label(option, cx)`,
`set_icon(option, cx)`, `set_mono(bool, cx)`, `set_preview(option, cx)`,
`set_hide_status_line(bool, cx)`, `set_embedded(bool, cx)` (draw the editing surface alone, for
a surface that already owns the frame around it — the filter bar's 30 px header row and the
palette's 44 px query row), `set_read_only(bool, cx)`, `set_invalid(option, cx)`,
`set_filter(option, cx)`, `set_enter_inserts_newline(bool, cx)`, `is_empty()`, `is_composing()`, `is_read_only()`, `is_invalid()`,
`has_selection()`, `focus_handle()`, `focus(window, cx)`, `mode()`, `buffer()`, and
`submit(cx)`, plus `move_vertical(down, select, cx) -> bool` for owners that route a claimed
vertical key back into the editor. `InputMode::{SingleLine, Multiline { min_rows, max_rows }}` selects behavior.
The filter is `Option<fn(char) -> bool>` and applies only to user insertion and paste.
**Events.** `TextInputEvent::{Changed, Submitted, Focused, Blurred}`. `Submitted` is emitted only
when a single-line owner explicitly calls `submit`; the input does not consume `Enter` itself.
`Focused` and `Blurred` report the editor's own focus handle, including the focus a click on the
value takes for itself. `Focused` also fires on the input's first paint when its handle is already
focused, so a surface that focuses an editor in the same frame it creates it still learns of it —
the focus listeners only join the focus tree once the element has painted. A surface with more than one editor must mirror `Focused` into whatever
marker it uses to remember which editor owns the keyboard, or a click and that marker disagree and
the next focus reconciliation moves the caret back to the marked field.
**Actions.** `text_input::{MoveLeft, MoveRight, MoveWordLeft, MoveWordRight, MoveToLineStart,
MoveToLineEnd, MoveToRowStart, MoveToRowEnd, MoveUp, MoveDown, MoveToStart, MoveToEnd, SelectLeft, SelectRight,
SelectWordLeft, SelectWordRight, SelectToLineStart, SelectToLineEnd, SelectToRowStart,
SelectToRowEnd, SelectUp, SelectDown,
SelectToStart, SelectToEnd, SelectAll, Backspace, Delete, DeleteWordBackward,
DeleteWordForward, DeleteToLineStart, DeleteToLineEnd, Newline, Copy, Cut, Paste, Undo, Redo}`.
**Key context.** `TEXT_INPUT_KEY_CONTEXT` is `FleetTextInput`; its `mode` attribute is
`single_line` or `multiline`, and `enter` is `newline` (the default) or `owner`.
**Bindings.** `text_input::default_bindings()` is the whole `FleetTextInput` table as
`Vec<KeyBinding>`, for an app with no key table of its own: the galleries, `fleet-lazygit`
standalone and the kit's tests all bind it. `fleet-app` states the same rows inside its
`key_table!`, because that macro also feeds the Help overlay and the documentation-drift test,
and a test there asserts the two agree.
**States.** single-line empty with placeholder · filled · focused with caret · selection ·
marked IME text · invalid with message · read-only · numeric-filtered · label with leading
icon · derived preview replaced by a validation message in the same slot; multi-line at minimum
rows · grown to maximum rows with scroll and a multi-line selection · mono · invalid; embedded
inside a `FilterBar` header row and inside the palette's query row. These are the states in
`examples/gallery_input.rs`, `examples/gallery_board.rs` and `examples/kit_gallery.rs`; a state
absent there is not implemented.
**Usage rule.** Never decode or forward editing keys around a `TextInput`; bind the exported
actions and let the entity own editing. Single-line mode propagates `Enter`, `Tab`, `Shift-Tab`,
`Up` and `Down`; multi-line plain `Newline` is bound under
`FleetTextInput && mode == multiline && enter == newline`, while `Shift-Enter` is bound under
`FleetTextInput && mode == multiline`. Visual `Up`/`Down` propagates at the first/last visual
row (selection-extending motion stops there), allowing an owner to bind history. A dialog's bare-letter bindings must live under a
context word that is absent while the input is focused. Read-only inputs still support motion,
selection and copy. Surface code may call `submit` after handling its own single-line submit
action. The single-line status slot is always present and the validation message **replaces** the
preview in it, so a failing branch name causes **zero layout shift**; fail before a job starts,
with the exact failing rule. `set_hide_status_line(true)` is only for a surface that can carry
neither, because it states its rule elsewhere: a settings row inside `NumberField`'s chrome, the
rename-terminal dialog, the board filter, a `lazygit` prompt.

#### `MarkdownText` / `parse_markdown`
**Purpose.** Read mode for a card description, a comment and any other stored markdown — the
read half of the surface a multi-line `TextInput` edits, which is why it sits in this group.
**API.** `MarkdownText::new(source).muted(bool)`; `parse_markdown(&str) -> Vec<MdBlock>`;
`MdBlock::{Heading{level,text}, Paragraph(Vec<MdSpan>), List{ordered,items}, Code(String)}`;
`MdSpan::{Text, Code, Bold, Link}` with `.text()`; `MAX_HEADING_LEVEL` = 3 and
`LIST_MARKER_CH` = 3 (the list gutter, in `ch`).
Ordered item spans retain the authored numeric marker, which rendering extracts without renumbering.
**Subset.** `#` `##` `###` headings (`Title` / `UiStrong` / `Label` of the type scale) ·
paragraphs with blank-line breaks · `-` `*` `1.` lists · ``` fenced code (data face, sunken
`bg`, hairline) · `` `inline code` `` (data face on a low-alpha fill) · `**bold**` · bare
`http(s)://` URLs (accent). **Anything else is text.**
**Usage rule.** No markdown crate — `docs/BOARD.md` §1 forbids one, and the parser is pure and
unit-tested instead. It never guesses: an unclosed `**` renders as two asterisks, and `####`
is a paragraph. Inline flow is one `gpui::StyledText` with byte-range highlights, so a
paragraph wraps like text rather than like a flex row.

#### `FuzzyList` / `FuzzyItem`
**Purpose.** A scrolling list of already-ranked results.
**API.** `FuzzyItem::new(primary).detail(..).secondary(..).trailing(..).leading(..)
.disabled(bool).key(..).kbd(Kbd).matches(..).destructive(bool).badge(..).checked(bool)
.heading(..)`; `kbd` is the row's own key as chips resolved from the keymap (`key` stays for a
value that is not a keystroke); `heading` opens a section — a sentence-case label drawn above
the row inside the same list item, so the cursor, `reveal` and the harness numbering keep
counting rows; `badge` is a
neutral `Badge` naming what the row is among its siblings (`default`, `previous base`), and
`checked` ends the row with an accent `✓` — the chosen value of a list that is a choice rather
than a launcher (the create dialog's base refs);
`FuzzyList::new(id, items).cursor(usize).cap(usize).visible_rows(usize).track_scroll(&ScrollHandle)
.on_click(|ix, window, cx| ..).under_text_field(bool).empty(..).row_height(Pixels)
.leading_width(Pixels).harness_rows(part, first)`; `leading_width` widens every row's leading
slot for a leading element wider than a glyph (the palette's icon tile); `.binds_jk()`, `.shown()`,
`FuzzyList::{next_cursor, prev_cursor, reveal}`.
**Pointer.** Rows hover; a press on an enabled row runs it (`on_click` receives the index in
`items` and does what `⏎` does there — a picker has no separate select). `visible_rows(n)` makes
the list exactly `n` rows tall and scrolls the rest by wheel; the caller keeps the cursor in view
with `FuzzyList::reveal(&handle, cursor)` from the action that moved it, never from render.
**Variants.** one-line (no `secondary`) · two-line. An item with an empty description collapses
to one line — zero-suppression. `detail` is a muted qualifier drawn **on the same line**, after
the primary label: use it when the qualifier is part of the row's identity (§3.8.5's context
owners), and `secondary` only for a description that earns a second line.
**Caps.** Unbounded by default. A cap is a product decision where a predictable `Enter` beats
completeness: 8 (Clone results) · 50 (Create base list, 6 visible). To bound the
*height* and keep every result reachable, use `visible_rows` instead.
**Keyboard.** `ctrl-n`/`ctrl-p` or `↓`/`↑` under a text field; `j`/`k` too when there is none.
**Usage rule.** Matching, ranking and the 150 ms debounce belong to the caller — they need the
domain's fields. Where a surface keeps a cap, the footer says `9 of 63`.

#### `FilterBar`
**Purpose.** Narrow a list without moving it.
**API.** `FilterBar::new(input: Entity<TextInput>, shown, total)`; `.query_slot()`,
`.is_empty_result()`, `.count_tone()`. The owner builds the editor **embedded**
(`set_embedded(true, cx)`) so it fits the 30 px header row, and sets its placeholder.
**States.** typing (caret, `esc` hint) · with a query, a compact clear ✕ at the end of the field
(painted `filter.clear`) that empties the editor — the same edit `ctrl-u` makes, so the owner hears
an ordinary change and the input keeps the keyboard · empty (no ✕: nothing to clear) · no match
(`shown/total` turns amber). "Exited but
retained" is a state of `PaneHeader::filter_chip`, not of this component: the bar is drawn only
while the editor owns the keyboard.
**Keyboard.** editing is the `FleetTextInput` table; `ctrl-n`/`↓` and `ctrl-p`/`↑` move the
**list** cursor while still typing · `Enter` opens the selected row · first `Esc` leaves the
input keeping the filter · second `Esc` clears it. `Esc` **never** quits the app.
**Usage rule.** Pass it into `PaneHeader::query_slot`, so it replaces the header in the same 30 px
row. A hidden active filter is the classic "where did my rows go" bug, so always show the
retained chip afterwards.

#### `Cycler`
**Purpose.** A closed choice in a settings row. The keyboard is the cycler's; the drawing picks
the form that reads at a glance.
**Anatomy.** The `row_h` cursor band (`control::cursor_row`): the label in `Ui`, then the control
at the row's end. Given its `options`, a set of up to `SEGMENTED_MAX` (4) options of at most
`SEGMENTED_MAX_CHARS` (32) characters together draws as a `SegmentedControl` with the value
raised; any other set, as a compact `Dropdown`. The label keeps its width and the control takes
the rest of the row, so a long value never pushes the setting's name out. A cycler whose
caller lists no options, or whose value is off the configured steps (none of the segments), draws
the compact dropdown field alone, stating the value.
**API.** `Cycler::{new(value), labeled(label, value)}().options(iter).on_select(Fn(ix, window,
app)).unavailable(indices).harness_segments(part).harness(options, dropdown).id(..).has_prev(bool).has_next(bool)
.focused(bool).disabled(bool).label_width(Pixels).off_grid(bool)`; `unavailable` dims those
options and refuses a click on them (a dropdown leaves them out of its list) while the keys still
reach them, so the surface states why next to the control; `harness` names the segments `<options>[N]` and the dropdown field `<dropdown>` for the harness target recorder (Settings names its cursor row's control); `.is_visible()`, `.form() -> CyclerForm::{Segmented(ix), Dropdown(listed)}`.
**Keyboard.** `←`/`→` (`h`/`l`), bound by the surface. **Pointer.** Only with `on_select`: a click
on a segment, or on a dropdown option, calls it with the option's index; point it at the update the
keys make. Without it the control is drawn only and takes no click.
**Usage rule.** Zero-suppress the whole control when the set has one member (the host cycler is
hidden when no hosts are configured).

#### `Toggle`
**Purpose.** A boolean settings row: the label, an optional `Caption` detail, and a `Switch` at
the row's end.
**API.** `Toggle::{new(checked), labeled(label, checked)}().detail(..).focused(bool).disabled(bool)
.label_width(Pixels).id(..).on_toggle(Fn(bool, window, app)).harness_switch(name)`; `.is_checked()`.
`harness_switch` names the switch itself, so a scenario clicks the control and not the row.
**Keyboard.** `Space`, bound by the surface; the row carries the cursor band. **Pointer.** Only
with `on_toggle`: a click on the switch asks for the other value.

#### `ValueField`
**Purpose.** A text setting in a settings list: the label, then the value in a field box that takes
the rest of the row. The box states the value; it is not an editor.
**API.** `ValueField::new(id, value).label(..).placeholder(..).mono(bool).focused(bool)
.label_width(Pixels).editor(Entity<TextInput>).on_click(Fn(window, app))`; `.shown()`,
`.is_editing()`.
**States.** default · focused (the cursor row, a stronger border) · placeholder (an empty value
drawn faint as what empty means, `Harness default`) · editing (the embedded editor inside the same
box, focus-ring border).
**Keyboard.** None of its own: the surface opens the row's live `TextInput` on `Enter` and hands it
back through `editor`. **Pointer.** With `on_click`, a click on the box does what `Enter` does.
**Usage rule.** Hand it an **embedded** editor (`TextInput::set_embedded`): the box is the chrome,
so opening and closing the row never moves it or changes its height. Use `NumberField` for an
integer and a bare `TextInput` for a field that is always editable.

#### `NumberField`
**Purpose.** An integer with a unit suffix and a clamp.
**API.** `NumberField::{new(value), labeled(label, value)}().unit(..).range(min, max).min(i64)
.focused(bool).invalid(message).label_width(Pixels).editor(Entity<TextInput>).end_aligned(bool)`;
`.clamp(i64)`, `.is_in_range()`, `.range_message()`, `.message()`, `.is_editing()`.
**States.** default · focused · invalid (out of range, red border) · editing · end-aligned (the box
at the row's end, where a `Cycler`'s or a `Toggle`'s control sits, and the label at full contrast,
for a list that mixes them — Settings).
**Usage rule.** The clamp is part of the contract: §3.8.6 states minimums, and an out-of-range
value must be refused at the field, not at save time. `editor` hands the field the live
`TextInput` the row is being typed into: the field draws that editor where the number would be
and keeps its own label, unit and message around it, so opening and closing a row never moves it.
While an editor is present the derived range message is suppressed — the number behind it is the
last committed one, and the editor states its own rule.

#### `SegmentedTabs`
**Purpose.** Underlined sub-tabs with counts inside a pane (PR `MINE 7` / `REVIEW 4`). A parent
navigation level, such as the Hub's screens, is a `SegmentedControl`.
**API.** `SegmentedTabs::new([SegmentedTab::new("mine", 7), SegmentedTab::bare("help")
.loading(bool)]).active(usize).harness_tabs(part).on_select(Fn(index, window, app))`;
`SegmentedTab::count_text()`, `SegmentedTabs::{len, is_empty, next_index, prev_index}`.
**Keyboard.** `Tab`/`S-Tab`/`h`/`l`. `on_select` sends a click through the same actions.
**Usage rule.** A tab is **not** a chip: `Some(0)` renders `0`, because an empty tab must still
say it is empty. `loading(true)` shows `…` while a refresh is in flight and keeps the cached
rows at full opacity.

#### `Select`
**Purpose.** A closed-list chooser that opens a `FuzzyList`.
**API.** `Select::new(value).label(..).placeholder(..).open(bool).focused(bool).options(..)
.disabled(bool).invalid(..).hint(..)`.
**Usage rule.** `Cycler` for 2–5 options, `Select` for a closed list, `FuzzyList` under a
`TextInput` for a searchable set.

#### `ConfirmDialog`
**Purpose.** Show exactly what will be lost, in facts, with their age — and how dangerous the
action is, in the colour of its button rather than the case of its key.
**Anatomy.** The `Dialog` alert form: the icon tile (neutral when compact, amber when expanded),
the title question, the target as the subtitle (`acme/api · ~/worktrees/acme/api/hotfix`), then
the facts, the stamp with its `Re-check` link button, and the consequence sentence. compact 480 px
(every fact known and safe, on one line) · expanded 560 px (one line per fact, risks first).
Footer: `Cancel` and the action button. Where `y` confirms, the action is a **primary** button
(`Delete`, chip `y`; `Enter` also confirms); where `Y` is required, a **danger** button (`Delete
anyway`, or a caller's stronger verb such as `Delete repository`, chip `⇧Y`; `Enter` does not
confirm).
**API.** `ConfirmDialog::new(title, FactList).target(..).consequence(..).stamp(FreshnessStamp)
.recheck_action(Box<dyn Action>).icon(Icon).accept_actions(lower, strong).accept_disabled(bool)
.action_label(..).strong_label(..).width(Pixels).body(..).footer_start(..).error(..)
.force_confirm_key(ConfirmKey).dismiss_action(..).on_dismiss(..)`; `.is_compact()`,
`.confirm_key()`, `.button_label()`, `.resolved_width(&Theme)`. Every button dispatches the
action its key does and shows that key from the live keymap: `Cancel` the dismiss action, the
action button `lower` or `strong` as the key rule asks, `Re-check` the recheck action. Without
`accept_actions` the footer states the key as a bold label only.
**States.** compact · expanded (primary) · expanded with an unknown fact (danger) · facts loading
(`checking…`, the action button disabled) · error line · multi-target body (prune: `Delete` and
`Keep` sections, a `Show kept` toggle button in `footer_start`).
**Keyboard.** `y`/`Y`/`Enter` confirm · `n`/`Esc`/`q` cancel · `I` re-check (delete) · `s` toggle
the KEEP list (prune). Nothing else is bound, so muscle memory cannot misfire.
**Usage rule.** No "don't ask again" checkbox, no second confirmation step, no countdown, no
typed-name confirmation, no disabled-button delay. The **compact form** is the real answer to
confirm fatigue. Users confirm the *consequence sentence*, so state it in plain future tense
and name exactly what is lost (`The 2 unpushed commits exist only here and will be lost.`).

#### `Palette`
**Purpose.** Jump to anything by name, or do the thing whose key you do not remember.
**Anatomy.** 640 px card at y = 120 · `palette_input_h` query row: `search` icon, the query in
the 15 px `title` size at regular weight, a scope chip (`All`, `Commands`, `Worktrees`, `Cards`,
`Agents`) and, while the query is empty, the prefix legend `type > commands · @ worktrees ·
# cards` · one `FuzzyList` of 34 px rows, `visible_rows` tall, scrolling past it, with sentence
case section headings riding above each section's first row · a footer naming the selected
row on the left and `Run ⏎` on the right.
**Row.** A `palette_tile` icon tile (`control` fill, `text_secondary` glyph; a custom leading
element such as a `StatusGlyph` sits in the same tile) · the label with its matched characters
bumped in weight · an optional same-line `qualifier` · a `badge` (a card's column) · a muted
right-hand `detail` (the place a command acts in, a worktree's state, `asks first`) or
`trailing` verb · the row's own key as a `Kbd`. A destructive row's tile takes the danger wash
and its glyph the danger colour.
**API.** `Palette::new(input: Entity<TextInput>).section(PaletteSection::new(title, rows))
.cursor(usize).visible_rows(usize).track_scroll(&ScrollHandle).scope(..).prefix_hint([(prefix,
meaning)]).selected_label(..).run_action(Box<dyn Action>).empty(..).on_click(|flat_ix, window,
cx| ..)`; `.flat_len()`; `Palette::reveal(&handle, cursor)`;
`PaletteSection::{new, len, is_empty}`;
`PaletteRow::new(label).icon(Icon).leading(..).detail(..).qualifier(..).secondary(..)
.trailing(..).badge(..).kbd(Kbd).matches(..).destructive(bool)`.
The owner builds the query editor **embedded** (`set_embedded(true, cx)`) and sets its
placeholder; the `Run` chip resolves from `run_action`'s live binding.
**Usage rule.** The owner ranks and the palette draws: sections render in the order they are
added, rows in the order given, so the first row is the best match and the flat cursor starts
there. A press on a row runs it with the same flat index the cursor uses. A command that is
invalid here is **not listed**, never greyed. Destructive commands still route through their
confirm.

### 6.5 Jobs and terminal

#### `JobRow`
**Purpose.** One background job written as a sentence — `glyph · lead subject · elapsed` —
with only what its state earns underneath: a progress bar and the last stdout line while it runs,
the error inline (and the buttons that act on it) once it failed, nothing once it is done.
**API.** `JobRow::new(ElementId, JobStatus, lead).subject(..).elapsed(..).percent(u8)
.progress(..).error(..).error_detail(..).actions(..).hover_action(..).retryable(bool)
.selected(bool).cursor(bool).pointer(&ListPointer, ix)`;
`JobStatus::{Queued, Running, Cancelling, Cancelled, Done, Failed}` with `.icon()`, `.tone()`
and `.is_finished()`. Built on `Row` (`details`), so hover, selection, the cursor bar and the
click / double-click / right-click contract are `Row`'s.
**Variants.** running (30 px line + `progress_bar_h` bar with its percent + last output line;
no bar without a parseable percent) · failed (line + danger-wash error block, `error_detail` as
its quieter second line, + `actions`) · finished (one 30 px line, sentence stepped down to
`text_secondary`, `elapsed` reads `2s · 1m ago`) · queued / cancelling.
**Usage rule.** `lead` is the verb phrase in the interface face ("Clone", "Run hooks for",
"Inspect worktrees"); `subject` is the real domain id in mono (`RepoId` / `WorktreeId`) —
swarm's footer showed `hot-copy:<repo>`, which matched no row anywhere in the app. `hover_action`
is the one pointer control of a live row (Cancel `c`); it is also drawn on the selected row, so
its key chip is visible wherever the key acts. Every button in `actions` or `hover_action` is
also reachable by its key and from the row's context menu. Failed jobs are **never**
auto-dismissed.
**Usage rule (`retryable`).** Renders §3.8.9's `(restartable)` / `(not restartable)` label in
the quit-and-stop confirm. Leave it **unset** when retryability is unknown: a job that claims
either is worse than one that says nothing. `JobStatus::Cancelling` mirrors
`fleet_proto::job::JobStatus::Cancelling`: cancellation was requested and shutdown is still in
progress, which is not the same row as `Cancelled`.

#### `JobTicker`
**Purpose.** The newest running job as one status-bar line.
**API.** `JobTicker::new(kind, target).id(ElementId).percent(u8).elapsed(..).extra(usize)`.
**Usage rule.** `extra` is zero-suppressed. The ticker is hidden while a `StickyErrorSlot` is
present.

#### `StickyErrorSlot`
**Purpose.** Red, addressable with `!`, persists until dismissed.
**API.** `StickyErrorSlot::new(text).id(ElementId).key(..).count(usize).on_activate(Fn(..))`.
**Usage rule.** Errors go here, **never** into a toast. It owns the last failed job and holds
the red jobs chip until the Jobs panel has been opened.

#### `LogView`
**Purpose.** The tail of `logs/jobs/<id>.log`.
**API.** `LogView::from_shared(id, lines).shared_line_tones(..).following(bool)
.track_scroll(&handle).top(usize).focus(&FocusHandle).on_command(Fn(LogCommand, ..))
.empty(..).show_badge(bool)`;
`.line_count()`, `LOG_TAIL_LINES = 200`, and
`LogCommand::{ToggleFollow, Follow, ScrollTo(usize)}`.
**Keyboard.** `f` toggles follow · `j`/`k` scroll · `G` re-enables follow · `Esc` collapses the
sheet back to 440 px.
**Usage rule.** Tail the file and batch at 16 ms in the caller; hand the component the lines.

#### `TerminalGrid`
**Purpose.** Paint the mirror cell grid.
**API.** `TerminalGrid::from_shared(rows).cache(&TerminalGridCache).id(ElementId)
.cursor(GridCursor).selection(GridSelection)
.focused(bool).padding(Pixels).scrollback(offset, len).scroll_pill(ScrollPill)
.modes([TerminalMode]).frame_size(cols, rows).dimmed(bool)
.on_resize(Fn(cols, rows, &mut Window, &mut App))`; `.row_count()`, `.column_count()`.
`CellMetrics { width, height }` with `CellMetrics::measure(&Theme, &Window, &App)` and
`.fit(Size<Pixels>) -> (cols, rows)`.
`GridRow::new(cells)` with `.columns()`; `GridCell::new(text, &theme)` and the builders
`.fg .bg .bold .dim .italic .underline(UnderlineStyle) .underline_color(Hsla) .strikethrough
.inverse .blink .invisible .width(CellWidth)`, plus `.resolve(&theme) -> (fg, Option<bg>)` and
`.same_style(&other)`.
`CursorShape::{Block, Bar, Underline, Hollow}`,
`UnderlineStyle::{None, Single, Double, Curly}` with `.is_some()`,
`CellWidth::{Narrow, Wide, Spacer}` with `.columns() -> 1 | 2 | 0`,
`GridSelection::new(r, c, r, c)` with `.normalized()` and `.span_in_row(row, row_columns)`.
**The attribute set is complete on purpose.** The cell model carries all ten VT flags plus the
underline style and color, because `INVERSE` and `DIM` are **not** cosmetic: lazygit, nvim
status lines and `fzf` draw their selection with reverse video and dim, so a reduced cell model
visibly corrupts exactly the apps the default `nvim | cc | lg` layout runs. `dim` is 55 %
foreground opacity, `blink` is 70 % (the kit runs no frame timer for it), `invisible` drops the
glyph and keeps the background, and `inverse` swaps fg/bg — all inside `GridCell::resolve`, so
`fleet-app` never resolves reverse video itself.
**Mappings that must not be re-guessed.** `proto::CellWidth::Spacer → CellWidth::Spacer →
**0** columns` (the continuation cell after a wide grapheme). `CursorShape::Hollow` exists
**only** in the kit — the client derives it from focus, and it must never be added to the wire
enum.
**Usage rule.** The kit defines its **own** cell model rather than depending on `fleet-proto`;
the app converts `proto::Cell` on the way in, resolving `Palette(u8)` through
`TerminalPalette::color` and `Default` through the palette's `foreground`/`background`.
An unfocused terminal draws a hollow cursor. The selection is painted as a
`terminal.selection` quad per row span, behind the text. `.scrollback(offset, len)` paints a
`ScrollbackBadge` in the top-right corner when `offset > 0`. The only things ever drawn **over**
the cells are the two scroll overlays and the ⌃S command menu (§3.6); `.modes(..)` feeds the
alt-screen suppression and paints nothing — Fleet draws no mode badges (UX-SPEC §3.6).

#### `TerminalModes`
**Purpose.** Zero-suppressed badges for the VT modes a `FrameUpdate` reports.
**API.** `TerminalModes::new([TerminalMode]).glyphs_only()`; `.is_visible() .is_alt_screen()`;
`TerminalMode::{AltScreen, MouseReporting, BracketedPaste, ApplicationCursor}` with
`.label() .icon() .tone()`.
**Usage rule.** These modes are the only explanation for the keymap appearing to lie: in
alt-screen there is no scrollback (`ctrl-s [` refuses), with mouse reporting on the app owns
drag-select, and paste (`cmd-v` or `ctrl-s ]`) follows the program's bracketed-paste mode. A plain shell shows
no badge, so the row costs nothing in the common case. `alt` is the only amber one, because it
is the only one that changes what a documented key does. **The app no longer draws this row**
(UX-SPEC §3.6: only the alternate screen changes a Fleet key, and `ctrl-s [` already says so);
the component stays for the gallery. Never draw it over the grid, whose cells are live output that
a badge would hide (zsh with `zle` sets bracketed paste and application cursor keys, so those two
badges are on in every plain shell).

#### `ScrollbackBadge`
**Purpose.** `↥ <offset>/<len>` in the grid's top-right corner.
**API.** `ScrollbackBadge::new(offset, len).alt_screen(bool)`; `.is_visible()`.
**Usage rule.** `ScrollPill` is the *mode* affordance and exists only while Scroll mode is
active; this badge is the *state* affordance. A viewport scrolled up with the wheel is not in
Scroll mode and would otherwise look exactly like a live one — which is how "my agent stopped
printing" bug reports are born. Zero-suppressed at `offset == 0` and in alt-screen.

#### `TerminalTabStrip`
**Purpose.** The Workspace's tabs, 84–200 px each on a 40 px `chrome` strip: what each tab is,
its state, and the pointer twins of the `^s` tab keys.
**API.** `TerminalTabStrip::new([TerminalTab::new(1, "nvim").kind(TerminalTabKind).icon(Icon)
.kbd(Option<Kbd>).activity(bool).unread(bool).attention(bool).starting(bool).keep_alive(Icon)
.agent_status(TerminalAgentState).exited(impl Into<Option<i32>>)]).id(ElementId).active(usize)
.agents_from(usize).show_plus(bool).on_select(Fn(position, ..)).on_close(Fn(position, ..))
.close_kbd(Option<Kbd>).tab_menu(Fn(position, Menu, ..) -> Menu).new_menu(Fn(Menu, ..) -> Menu)
.on_new(Fn(..)).trailing(impl IntoElement)`.
**Anatomy.** Per tab: kind glyph (`terminal` for a PTY, `git-branch` for a native pane, or the
caller's `icon`: `square-kanban` for the board, `bot` / `sparkles` for a Claude / Codex thread),
the name, at most one state mark, then the `✕`. After the tabs, the `+` (a ghost `IconButton`
opening `new_menu`, or running `on_new` when there is no menu); at the right end, the `trailing`
controls. The index is not painted: it is in the tab's tooltip, `Tab 2` with the tab's `kbd`.
**States.** active (the content ground, a hairline on three sides and none underneath, so it
joins the content below; `ui_strong`) · inactive (`text_secondary`, `row_hover` on hover) ·
unread (6 px blue dot, for `activity` or `unread`) · attention (amber `needs you` chip, which
survives selection) · starting (`loader-circle`, also a working native agent) · agent working
(`loader-circle`) · agent finished (`circle-check`) · keep-alive glyph · exited (faint name and
`exited 1` in red, or `killed` when a signal left no code) · close `✕` (always on the active tab,
on hover elsewhere; hidden, not removed, so hovering never reflows the strip).
`TerminalTabKind::{Pty, Native}`, `TerminalAgentState::{Working, Finished}`.
**Pointer.** A click selects (`on_select`). The `✕` and a middle-click close (`on_close`). A
right-click selects the tab, then opens `tab_menu` at the pointer. Every one of them is the twin of
a key the caller wires (`^s 1`–`9`, `^s x`, the menu's own keys), so each is optional and the
strip still draws with none.
**Usage rule (`starting`).** §3.6's "Waking a slept session" rebuilds the strip and spawns one
PTY per tab; without a per-tab spinner the strip claims six live terminals that do not exist
yet. **Usage rule (`exited`).** `.exited(1)` and `.exited(None)` both compile: exit codes are
`Option<i32>` end-to-end (`fleet-core::TerminalStatus`, `Event::TerminalExited`), because a
`SIGKILL` from `^s x` produces none.
**Usage rule (marks).** A tab draws **at most one** mark, and *needs you* wins: amber means the
tab is waiting on the user, blue only that content arrived while they were elsewhere. An exited
tab draws neither, and the active tab draws no dot — only the *needs you* chip survives
selection (`NATIVE-AGENTS.md` §3.3). Agent activity appears only on terminals with a recognized
agent. **Harness.** `tabs.tab[N]` / `agents.tabs.tab[N]`, their `.close`, and `tabs.new`.

#### `ScrollPill`
**Purpose.** `SCROLL <offset>/<len>` while in scroll mode.
**API.** `ScrollPill::new(offset, len).selecting(bool).alt_screen(bool)`; `.is_visible()`.
**Usage rule.** **Suppressed in alt-screen** — when an alt-screen app is running, `ctrl-s [`
shows the 1.6 s toast `no scrollback in alt-screen` instead. Top-right inside the terminal area,
because during scroll the eyes are on content and the top right never covers the prompt.

#### `PrefixMenu`
**Purpose.** The ⌃S command menu: every command the held prefix reaches, grouped into columns,
each a clickable row led by its second key.
**Anatomy.** A floating `elevated` panel, `radii.popover`, `border_strong` hairline, popover
shadow, `md` padding, at most `prefix_menu_w` wide, centred `md` above the bottom of its
(`relative`) container. Header: the prefix as a `Kbd` in `KbdTone::Warning`, the title in
`UiStrong`, the caller's note, and the caller's close control at the right edge. Below it, equal
columns `md` apart, each headed in `SentenceLabel` (muted); a row is `button_h_compact` tall,
`radii.control`, the `Small` key chip (or a `first`–`last` pair for a numbered range) then the
label in `Ui`, ellipsized; `control_hover` / `control_active` under the pointer.
**API.** `PrefixMenu::new(id, title).prefix(Kbd).note(impl IntoElement).close(impl IntoElement)
.column(PrefixMenuColumn).harness(&'static str)`;
`PrefixMenuColumn::new(title).item(impl IntoElement)`;
`PrefixMenuItem::new(id, Kbd, label).range_end(Kbd).on_click(Fn(&ClickEvent, ..))`.
**States.** hidden (the caller draws nothing) · shown · row hovered · row pressed · range row.
**Usage rule.** The delay is the caller's timer (`theme.motion.prefix_hint_delay`): the expert
types the second key first and never sees the menu; the returning user gets it exactly when they
hesitate. The panel occludes what is under it and swallows a mouse press, so a click never starts
a terminal selection, but it never takes focus: the held prefix keeps the keyboard. A row shows
only the key *after* the prefix. `fleet-app` builds the rows from the action catalogue in the update
pass after the delay runs out (`views/prefix_menu.rs`), never in render, so a fast second key
never pays for them. Use a `Menu` for a list of commands opened
by a click.

#### `ExitStrip`
**Purpose.** `⚠ process exited (<code>)` and the prefixed recovery keys.
**API.** `ExitStrip::new(impl Into<Option<i32>>).hints(KeyHintRow)`.
**Usage rule.** Defaults to `^s r restart · ^s x close · ^s c new`. Fleet must not silently
swallow a crashed dev server. `ExitStrip::new(None)` reads `process exited (killed)`: a
signal-killed process has no exit code and the strip must not invent `128 + signo`.

#### `ModeWord`
**Purpose.** The fixed 84 px status word of the embedded Git UI (`fleet-lazygit`): the key owner
(`NORMAL`, `STAGING`, `DIALOG`) and the repository operation (`REBASING`, `MERGING`).
**API.** `ModeWord::word(text).tone(Tone)`.
**Usage rule.** Only the Git UI draws it, because it mirrors lazygit's status line. Fleet's own
chrome has no mode word (ADR 0023); do not add one to the status bar or the title bar.

#### `Banner`
**Purpose.** A 28 px full-width strip with a countdown and recovery keys.
**API.** `Banner::{warning, danger}(text).icon(Icon).countdown(..).hints(KeyHintRow)`.
**Usage rule.** After a daemon restart the banner must state what became of the terminals, from
the first snapshot rather than from an assumption: *"fleetd restarted. `<n>` terminals were
reattached; worktrees, jobs and state are intact."*, or *"fleetd restarted. No terminals survived;
worktrees, jobs and state are intact."* A banner that leaves the user guessing whether the agent in
each tab is still running — or that promises a reattach that did not happen — is the one thing this
surface exists to prevent.

#### `DaemonSplash`
**Purpose.** The two **full-window** daemon surfaces of §3.12, cases A and B.
**API.** `DaemonSplash::{starting, failed}(title).detail(..).log_lines(..).hints(KeyHintRow)`;
`DaemonSplashKind::{Starting, Failed}`.
**Usage rule.** `Banner` covers case C only, because that one is a 28 px strip under the
title bar. Cases A and B are chrome-less full-window surfaces and neither fits `EmptyState`,
which is two lines and pane-scoped: A needs a spinner plus the socket path after 3 s, B needs a
mono tail of `~/.fleet/logs/fleetd.log`. The keys here are **bare** (`r`, `L`, `D`, `ctrl-q`) —
the D-8 prefix rule applies over a terminal grid, and there is no terminal on this screen.

#### `DaemonDot`
**Purpose.** 8 px liveness dot that grows a word when degraded.
**API.** `DaemonDot::new(DaemonState::{Healthy, Degraded, Lost}).label(..)`;
`.resolved_label()`. `DaemonState::{ALL, word, is_labelled}`.
**Usage rule.** Healthy is a dot and nothing else — good news must not cost pixels. Degraded and
lost grow a labelled pill, because bad news must be readable.

#### `Veil`
**Purpose.** A 55 % scrim over **terminal grids only**, while the daemon is gone.
**API.** `Veil::new(active).opacity(f32).content(..)`; `.drops_keys()`. Its default opacity comes
from `Theme::metrics.veil_opacity`.
**Usage rule.** Lists stay at 100 % and stay navigable — they are true, just frozen; only a live
surface is veiled. Keys typed into a veiled grid are **dropped, not buffered**; the component
renders the scrim and `drops_keys()` states the contract the caller must honour.

### 6.6 Native agent transcript

Two of these are among §6's stateful exceptions: a transcript and a composer cannot be
`RenderOnce`, because the list caches measured row heights and the scroll machine, and the
composer wraps the `TextInput` entity that owns the caret, the selection and the IME session. They are gpui **entities** that emit
events and never act on a thread; the screen that owns them decides what an event means.
`components::agent::metrics` holds their fixed dimensions (§2.8); `components::agent::format`
holds their copy — `format_duration`, `format_token_count`, `format_file_delta`,
`format_files_changed`, `format_cost`, `turn_footer_segments`, `format_worked`,
`format_stopped_after`, `format_thought`, `format_working`, `format_exit`, `format_counter`,
`format_retrying`, `format_compacted`, `format_resumed`, and `MINUS`, the U+2212 the design uses
for a removed-line count — and `components::agent::group` holds the group summarizer
(`ToolGroupCounts` + `format_group_summary`: `read 3 files and ran 2 commands`, with named MCP
servers hoisted to the front). Keycaps are never inside those strings: a control carries its key
as a `Kbd` chip the owner resolves from the live keymap (ADR 0023).

#### `TranscriptList`
**Purpose.** The bottom-anchored, variable-height conversation.
**API.** Entity. `TranscriptList::new(&mut Context<Self>)`, `set_rows(Vec<TranscriptRow>, cx)`,
`patch_row(usize, TranscriptRow, cx)`, `set_thread(Vec<TranscriptRow>, cx)`,
`set_row_body(RowBodyRenderer, cx)`, `set_row_action_kbd(RowActionKbd, cx)` (the owner's lookup
for the row verbs' chips); the scroll machine —
`scroll_to_latest(cx)`, `scroll_to_end(cx)`, `anchor_new_turn(cx)`, `release_anchor(cx)`,
`gesture(Gesture, cx) -> bool`, `scroll_mode(bool, cx)`, `scroll_rows(f32, cx)`,
`scroll_viewports(f32, cx)`, `scroll_to_top(cx)`; the row focus — `focus_row(Option<usize>, cx)`,
`move_row_focus(isize, cx)`, `focused_row()`; readers `rows()`, `scroll_state()`,
`is_following()`, `is_scroll_mode()`, `shows_jump_to_latest()`, `focus_handle()`,
`event_for_key(&str) -> Option<TranscriptEvent>`. Free helpers: `diff_rows` (the `RowSplice`
`set_rows` applies), `scroll_thumb`, and the pure scroll machine `FollowState` / `ScrollMode` /
`Gesture` / `breaks_follow` / `is_at_end`.
**Rows.** `TranscriptRow { id: TranscriptRowId, kind: TranscriptRowKind, attached }` — flat by
construction: a turn is a *run* of rows, never a container, because nested containers make
variable-height virtualization and scroll anchoring unsolvable. The twenty kinds are `User`,
`Assistant`, `AssistantMeta`, `Reasoning`, `Work`, `WorkLive`, `WorkGroup`, `Subagent`,
`Delegation`, `DelegationResult`, `Diff`, `TurnFold`, `TurnFooter`, `Plan`, `Gate`, `Checkpoint`,
`Notice`, `Error`, `Working`, `Empty`.
`Notice` is a harness's own user-facing message — a config warning, a deprecation — which is not
an error and must not be drawn as one; `Error` is the **severe** tier only (a runtime error or a
broken side effect), because a nonzero command exit is carried by the failing `Work` row.
**Identity.** `TranscriptRowId::LiveActivity` is shared by `WorkLive`, a streaming `Reasoning`
and `Working`, so *thinking → tool A running → tool A done* is **one row changing its label**,
not three mounts; `TranscriptRowId::Item` is shared by a `Work` row, the `Diff` under it and a
`Subagent`, so an update merging forward never remounts.
**States.** following the tail · anchoring the first turn · free scrolling · scroll mode (tail
frozen, a row focused) · streaming (caret and shimmer) · empty.
The floating `jump to latest` chip is offset from the bottom by one tokenized chip height plus
standard spacing, reserving the newest turn footer and its `[u] revert turn` hint.
**Events.** `TranscriptEvent::{Toggle, RowAction, ReachedOldest}`. A row verb pressed with the
pointer — a tool row's hover button or menu item, a turn footer's button — emits the same
`RowAction { row, action }` its key emits on the focused row, and a click on an expandable line or
a delegation row emits the same `Toggle` as `⏎`, so the owner has one path for both. `ReachedOldest` fires when the
reader comes within `OLDEST_PREFETCH_ROWS` of the top of the rows in hand, **once per row set**:
the transcript cannot know whether older history exists — that is the owner's page cursor — so it
reports the gesture and nothing else, and the owner's answer is itself a new row set, which is
what re-arms it. A splice that touches index 0 is the re-arm signal, so a page prepended above the
reader asks again and a fully loaded thread asks nothing.
**Usage rule.** GPUI `list`, **not** `uniform_list` — an assistant paragraph, a 30 px tool row
and an inline diff are not one height. ADR 0005's uniform-row rule still governs the diff, which
stays uniform *inside* its row. `set_rows` and `diff_rows` are structural only. Streaming calls
`patch_row`, which replaces one row and calls `ListState::remeasure_items` rather than `splice`,
so a growing scroll-top row keeps its exact in-row offset. `ListState::reset` is called from
`set_thread` alone — never from `render`, where it would discard every measured height. Parsed
Markdown and collection payloads are shared behind `Rc`, while long text remains `SharedString`,
so cloning a `TranscriptRow` does not clone transcript bodies.
**Usage rule (scroll callback).** GPUI calls the callback while the list's `RefCell` is mutably
borrowed. The callback uses only `ListScrollEvent::{visible_range,count,is_scrolled,
is_following_tail}`; strict end geometry and the scroll thumb are recomputed in a deferred entity
update after that borrow ends.
**Usage rule (follow).** A gesture may break follow **only when it can actually move the
viewport away from the live edge**, because follow gates the list's own auto-pin: a spurious
break produces no scroll event, never re-arms, and streaming silently stops following. The
re-arm band is `AGENT_FOLLOW_REARM_PX`, and the list's own `is_at_end` flag is a fallback, never
a short-circuit. Follow is armed at a *generation* that every manual navigation bumps, which is
what makes a stale async callback harmless with no cancellation token.
**Usage rule (nothing ticks but the clock).** The `Working` row carries `started_at`, never an
elapsed figure; the list owns a 1 Hz task that writes one `SharedString`. The jump-to-latest chip
is debounced on **show** only (`motion.jump_chip_delay`) and immediate on hide, so it cannot
flash while a thread switch settles. An animation runs only for a row inside the viewport.

#### `ToolRow`
**Purpose.** The 30 px row every tool call is drawn as.
**Anatomy.** state glyph · 60 px kind column (`AGENT_TOOL_KIND_W`, a verb: `Read`, `Edit`,
`Run`) · one-line summary (`flex_1 min_w_0`, ellipsized) · result `Chip` (filled, toned) ·
duration · hover verbs · chevron. A pointer-first row (ADR 0023): `sm` inset, `radii.sm`, a
`row_hover` band under the pointer and the pointer cursor when it can expand — never over the
focus ring the keyboard set. Children indent behind a 1 px left divider; an expanded body is
capped at `AGENT_BODY_MAX_H` and scrolls inside the row.
**API.** `ToolRow::new(id, kind, summary).icon(Icon).state(ToolRowState).result(..)
.result_tone(Tone).detail(..).body(..).expanded(bool).actions(RowActions)`; `.is_expandable()`;
`ToolRowElement::new(row, key).focused(bool).body(..).children(..).on_toggle(..)
.on_action(Fn(RowAction, ..)).action_kbd(RowActionKbd).harness_part("agents.tool")`;
`RowActions { copy, diff, open, revert }` with `.list()` / `.any()`; `RowAction::{label, icon}`;
`expand_hint(bool)`. `key` is the row's list index, not an `ElementId`: the row needs several
stable ids and `("tool-line", key)` tuples produce them with no per-frame `String`.
**States.** `ToolRowState::{Running, Done, Failed, Denied, Stopped, Severe}` → `.glyph(kind,
dimmed)` gives the icon, tone, opacity and whether it spins; `.heading_tone()` and
`.is_severe()` give the summary's voice; `.result_tone()` the chip's default (danger for a
failure, warning for a denial, secondary otherwise) unless the projection names one (`waiting for
you` in warning). Pointer: hovered (verbs shown as compact `IconButton`s, their chips only where
the owner's lookup resolves one) · right-clicked (a `ContextMenu` with the same verbs).
**Usage rule (five states plus one).** `Severe` is reserved for a runtime error or a broken side
effect — *the turn or a core side effect broke, not that a command exited nonzero*. A `git grep`
finding nothing is not red, and an exit status is a structured field (`format_exit`) carried in
the chip, never the heading's colour. Fleet never substring-matches English error text to infer
failure.
**Usage rule (geometry).** The geometry does not change while the row streams — a row that grows
under the reader is how a transcript starts to jitter. The click target is **the 30 px line
only**, so clicking inside an expanded body or a nested child never folds the row; a verb button
stops its click from reaching the line. The chevron is `invisible`, not absent, when a row cannot
expand, so alignment never shifts. A `Failed` row is always expandable, so its truncated label can
be read in full.
**Usage rule (verbs).** A row offers only the verbs its projection can honour, and every hover
verb is also in the row's right-click menu and on its key while the row holds the focus — hover
reveals, it never hides the only way (ADR 0023).

#### `DelegationRow` / `DelegationResultCard`
**Purpose.** Keep a native child reachable at its caller item and render the one result delivered
back to the caller without making it look user-authored.
**API.** `DelegationRow { provider, title, status, headline, elapsed, hint }` with
`DelegationRowStatus::{Starting, Working, Blocked, Done, Incomplete, Failed, Cancelled}`;
`DelegationResultCard { header, body, collapsible, expanded, hint }`.
**States.** The live row uses a spinner for starting/working, amber for blocked, and frozen terminal
marks/durations; its line is a pointer-first click target whose click reports the row's toggle,
which the owner answers by attaching the child, as `⏎` does. The result card uses `⏎ show` /
`⏎ hide`; Enter only folds it and never attaches.

#### `DecisionDock`
**Purpose.** A permission approval, a model question or a ready plan, in a drawer docked to the top
edge of the composer. Nothing is ever a modal, and an approval is never a transcript card: a card
can be scrolled off screen while it owns the keyboard, which is a modal with the chrome removed.
**Anatomy.** `AGENT_CONTENT_W` wide, panel background, rounded (`radii.lg`) on its **top** corners
only, a `border_strong` hairline, overlapping the composer by one hairline and masking the border
they share, so the two read as one panel rather than a card stacked on a field. A header — the
occupant's amber glyph (`lock`, `message-square-warning`, `list-checks`), the title, the `1 of N`
counter — then the body, then a row of `Button`s: the first verb `Primary`, the rest `Secondary`,
and `Deny and stop` a `GhostDanger` set apart at the far right. An approval's body is the payload
well, a one-line caution, and the diff under a file header (`README.md  +1 −1`); a question's is
its header, prompt and one clickable option row per option, each led by its digit's `Kbd` and, on
a multi-select question, a `square` / `square-check` box; a plan's is its title and body.
**API.** `DecisionDock::new(Decision).diff(..).diff_header(..).payload_focus(FocusHandle)
.on_action(Fn(DecisionAction, ..)).kbd_for(Fn(&DecisionAction, &Window, &App) -> Option<Kbd>)`.
`Decision::new(id, title, DecisionKind).queued(index, total).answering(bool)`;
`Decision::head(&[Decision])`, `.options()`, `.key_hints()`,
`.action_for_key(&str) -> Option<DecisionAction>`.
**Variants.** `DecisionKind::{Approval(ApprovalRequest), Question(QuestionSet), PlanReady{title,
markdown}}`, in that strict priority order — one slot, one occupant, and **no "approve all"**.
The proposed plan itself is a transcript row (`TranscriptRowKind::Plan`) with no actions of its
own; its verbs — **Implement** and **Refine** — are this dock's buttons.
**States.** approval (with and without **Edit**, with a caution, with a diff) · question (single,
multi-select with checkboxes, free-text) · wizard at `i+1 of n` with **Previous** · plan ready ·
queued `1 of N` · `answering…` (every control disabled) · option row hovered / selected.
**Usage rule (keys).** The type owns the key *vocabulary*; the surface holding the focus handle
owns the key *event* and routes it here, and a click on a control reports the very
`DecisionAction` its key resolves to — one path. The chips come from the owner's `kbd_for`, which
reads the live keymap in the decision's own context, never a hand-typed key. That split is what
stops one thread from answering another thread's request. **`⏎` is not bound on an approval**: a
queued Return keystroke must never approve a shell command. While a reply is in flight nothing is
claimed and nothing dispatches.
**Usage rule (copy).** An action always spells out its effective scope in sentence case —
`Allow once`, `Allow for this session`. The word "always" never appears, and there is no directory
or project scope in v1. **Edit** is drawn only where the harness accepts an amended invocation.
**Usage rule (payload).** The payload well is the **invocation**, never the model's prose about
it, because it is what **Edit** seeds the composer with. It is bounded by `AGENT_WELL_MAX_H`,
scrolls in both axes, and is **never truncated and never line-clamped**.

#### `MetadataRow`
**Purpose.** An ordered strip of blocks that collapses from the right: in the agent tab, the muted
link line above the composer — the caller or the card a thread works for.
**API.** `MetadataRow::new(Vec<MetadataSegment>, MetadataFitResult).trailing(..)`;
`MetadataSegment::new(text)` / `::pinned(text)` / `.target(id)` / `.width(px)`; the owner holds a `MetadataFit`
and calls `fit(available, revision, &segments, gap, overflow)`, or the free `metadata_fit` for a
one-shot.
**States.** everything visible · collapsed from the right with an overflow count · the pinned
segment alone, truncating · targeted segment in link tone with focus ring and click/Enter action.
**Usage rule.** **The hidden count is memoised per width**, in a `MetadataFit` the owner holds
across frames, never recomputed per frame. **The model segment never collapses; it truncates** —
losing which model is answering is worse than losing its name's tail. No segment is ever
invented: a tab that has not published an effort has three blocks, not four.

#### `ComposerChip` / `ContextMeter`
**Purpose.** The composer's settings strip. A `ComposerChip` is a value and a chevron that opens a
menu — `gpt-5 · high ⌄`, a shield and `asks before edits ⌄` — used as a `PopoverMenu` trigger; a
`ContextMeter` is how much of the model's window the thread has used, a short track and `34%`.
**Anatomy.** Chip: `button_h_compact` tall, `sm` side padding, `radii.control`, no fill or border
until hovered or open (`control_hover` / `control_border`), `text_secondary` value and chevron,
optional leading icon. Meter: an `AGENT_CONTEXT_METER_W` × `space.xs` track in `control`, filled
`accent` to the fraction, then the prepared label in the hint role.
**API.** `ComposerChip::new(id, label).icon(Icon).open(bool).tooltip(text, Option<Kbd>)`;
`ContextMeter::new(percent, label)`.
**States.** chip: rest · hover · open (holds the hover look, announced expanded) · with a tooltip
naming what it changes and its key. Meter: 0–100.
**Usage rule.** Use a `Dropdown` for a labelled form field; a composer chip is chrome inside the
composer, so it draws nothing until the pointer is on it. The chip knows nothing about models or
modes: the owner builds the menu, with a check on the value the next send carries.

#### `MultilineInput`
**Purpose.** The docked composer: a thin owner of a shared multi-line `TextInput` that adds the
prompt glyph, submission, prompt history and completion-trigger reporting.
**API.** Entity. `MultilineInput::new(&mut Context<Self>, placeholder)`, `text(cx)`, `is_empty(cx)`,
`is_composing(cx)`,
`set_text(.., cx)`, `clear(cx)`, `set_placeholder(.., cx)`, `set_focus_visible(bool, cx)`,
`set_read_only(bool, cx)`, `set_framed(bool, cx)`, `submit(cx)`, `push_history(..)`,
`active_trigger(cx)`,
`focus_handle()`, readers `buffer(cx)` and `history()`, and the three `-> bool` motions an owner
falls through on — `recall_previous(cx)`, `caret_up(cx)`, `caret_down(cx)`, each answering
whether it moved; emits `MultilineInputEvent::{Submit(String), Trigger(Trigger), Changed,
Escape}` under `MULTILINE_INPUT_KEY_CONTEXT`. `buffer(cx)` returns the inner `InputBuffer`;
`PromptHistory` owns the last `HISTORY_LIMIT` (100) prompts plus the draft `↑` opened from.
**Framing.** Framed by default: its own fill, hairline, focus border and `❯` prompt. The agent
composer calls `set_framed(false, cx)` and frames the editor together with its settings strip, so
the fill, border and focus border are the container's and there is no prompt glyph.
**States.** empty (placeholder) · typing · multi-line (grows one visual row at a time, to eight) ·
capped (internal scroll thumb; wheel is consumed) · IME composition · read-only · dimmed while a
decision owns the bare keys.
**Keyboard.** The inner `TextInput` owns every editing action. The wrapper sets `enter = owner`:
plain `⏎` submits through the app's Send/Steer binding (or the wrapper fallback), `⇧⏎` inserts a
newline, `↑`/`↓` recall history at the visual buffer edge, and `Esc` reports escape when no owner
binding takes it. `@` `$` `/` report a `Trigger`. Plain `⏎` is consumed without submit while
`is_composing(cx)`; an owner-level Send/Steer action guards the same state.
**Usage rule (triggers report).** A trigger character is **inserted and reported, never
consumed**, so all three stay typable: the owner opens a picker on `Trigger` and re-filters it
from `active_trigger(cx)` on every `Changed`. `@` and `$` fire wherever a token starts; `/` fires
at **line start only**, because a harness expands a slash command only when it opens the whole
message and offering it elsewhere is a whole class of "why didn't my command run?" bugs.
**Usage rule (history).** `↑` recalls only at the **visual** (soft-wrapped) edge, and a caret at
a wrap boundary belongs to two rows — the one *farthest* from the edge under test wins, so an
ambiguous caret never claims the key. It declines while a selection is being extended and while
an IME composition is live, and browsing ends on any edit, even one the user immediately undoes.
**Usage rule (layout).** Long tokens break at character boundaries inside the resolved width.
Caret motion, hit-testing, drag/double-click selection and visual-row bounds are the shared
`TextInput` behavior; stale geometry falls back to logical motion. Hard tabs survive in the
stored/submitted draft.
**Usage rule.** It never acts on a thread: a submit, a trigger, a change and an escape are
reported, and the owner decides what they mean. The app binds owner keys in `Agent > …`; it does
not install `NoAction` shadows in the composer context.

#### `Markdown`
**Purpose.** Assistant prose, rendered from a stream.
**API.** `parse_markdown_document(&str) -> MarkdownDocument`;
`parse_markdown_prefix(&str, &HighlightCache)` for cached full-prefix parsing;
`MarkdownDocument::default().append(delta)` for incremental streaming;
`markdown(&MarkdownDocument, &App)` or `document.render_with_caret(bool, &App)`.
`MarkdownBlock::{Paragraph, Code{lang,text,highlights,closed}, List{ordered,items},
Heading{level,inlines}, Quote, Table{header,alignments,rows}, Rule}`
built through `MarkdownBlock::code(lang, text)`, `::cached_code(lang, text, &cache)` or
`::streaming_code(lang, text)`; `MarkdownInline::{Text, Code, Strong, Emphasis, Link}`.
**Usage rule.** Two invariants, both tested: parsing never panics or loops on any input, and for
any prefix `p` of `s`, every block of `parse_markdown_document(p)` except its last is a block of
`parse_markdown_document(s)` at the same index — a transcript must not reflow behind the reader
while the model keeps typing. Setext headings are deliberately absent, because they would
retroactively turn a finished paragraph into a heading. A GFM table begins only when its pipe
header is followed by a complete delimiter row; before that lookahead arrives the header remains
the last paragraph. Escaped `\|` stays inside its cell. Images and indented code are out of scope
and survive as their own source text.
**Tokens and layout.** Paragraphs, headings, list-item prose, quotes and table cells each use one
`StyledText`; strong, emphasis, inline-code background/mono family, link colour/underline and the
optional streaming caret are text runs. Table columns are equal `flex_1` children with `min_w_0`,
`space.sm` horizontal and `space.xs` vertical cell padding; the emphasised header uses
`text.ui_strong` over a `metrics.hairline` / `colors.border` divider. Fenced code uses the existing
`surface`, `radii.sm`, `space.sm`/`space.md` and `text.data` tokens, preserves whitespace, and
scrolls horizontally with axis restriction so a vertical wheel bubbles to the transcript.
**Usage rule (highlighting).** A fence is lexed when the document is built, never in `render`.
Code fences are **not** highlighted while streaming, and **a partial fence is neither read from
nor written to the `HighlightCache`** — it must never poison it, and a fence whose colours
changed per chunk would move the reader's eye on every token.

#### `DiffView` (`fleet_lazygit::diff_view`, not the kit)
**Purpose.** The inline diff under an `Edit` / `Write` tool row, emitted as its own
`TranscriptRowKind::Diff` row so its height is measured independently and an expanded diff never
inflates the tool row's own measurement.
**API.** Entity. `DiffView::new(unified, cx)`, `DiffView::for_path(..)`, `unified()`,
`set_unified(.., cx)`, `expanded()`, `set_actions(..)`.
**Usage rule.** It takes unified-diff *text*, so an embedder needs no git plumbing and
`fleet-ui-kit` gains no `fleet-git` dependency — which is the only reason this one lives outside
the kit. Rows come from the ADR 0005 stack and `views::row_layout`, so an inline diff has the
same geometry as a full-window one; only the wash differs (`diff_added` / `diff_removed`).
Beyond `MAX_ROWS` (400) it folds with a "show all" affordance rather than flooding the thread.
The action row (`[u] revert this edit · [o] open in nvim`) is the embedder's, handed in through
`set_actions`.

### 6.7 Board

The kanban surface of `docs/BOARD.md` §7. It obeys the same two rules as the rest of the
kit: no domain type crosses the boundary (a card arrives as `SharedString`s, scalars and
closures), and no color, size, radius or duration is a literal.

#### `PriorityGlyph` / `PriorityLevel`
**Purpose.** The five priority marks, drawn as shapes.
**Anatomy.** `Urgent` is a filled 8 px `danger` square · `High` / `Medium` / `Low` light 3 / 2 /
1 of a three-bar ladder (2 px wide, 4 → 6 → 8 px tall, secondary lit and muted unlit) · `None`
is a dashed hollow 6 px dot.
**API.** `PriorityGlyph::new(PriorityLevel).with_label(bool)`;
`PriorityLevel::{Urgent, High, Medium, Low, None}` with `ALL`, `.label()`, `.lit_bars()`;
`PRIORITY_BARS` = 3.
**States.** The mark has none: it is a pure function of its level.
**Variants.** mark only (a card tile) · mark + word (`with_label`, the card detail and the
pickers).
**Usage rule.** Only `Urgent` gets a color (§1.4: color is semantic, shape carries the
information), so the ladder still reads in grayscale. `None` is dashed and hollow because
"nobody decided" is not the same as `Low`.

#### `CardTile`
**Purpose.** One card on the board — the list row of the kanban world.
**Anatomy.** A `surface_raised` card, `radii.card`, `border` hairline. Three lines:
a **key line** — priority glyph · key (muted data face) · a flex spacer · at its right end one
`tile_chip_h` **state pill**, the run (spinner, amber dot or check, then its words in `Caption`
on the tone's fill) or else what blocks the card, never both · the **title** (`UiStrong`, wrapped
to `CARD_TITLE_LINES` = 2 with an ellipsis on the last line) · a zero-suppressed **meta row**:
label chips · estimate (`n pt`) · due date behind a `clock` glyph · the linked branch (`git-branch`
glyph + mono name) or a bare `git-branch` glyph · its `PrBadge` · amber dirty dot · red conflict
dot · `show_on_card` extras · then, at the right end, the state's **action** slot (`Answer`) and
the assignee's round `avatar_size` initials. A **menu** slot (the `⋯` trigger) floats over the
key line's right end.
**API.** `CardTile::new(id, key, title).priority(PriorityLevel)
.labels(Vec<(SharedString, Option<SharedString>)>).assignee(..).estimate(..).due(..)
.worktree(bool).branch(Option<SharedString>).pr(Option<(u64, PrBadgeState)>).dirty(bool)
.conflict(bool).selected(bool).focused(bool).extras(..).run(RunMark).run_label(..)
.blocked(u32, BlockedTone).blocked_label(..).action(impl IntoElement).menu(impl IntoElement)
.on_click(..).on_double_click(..).on_secondary_click(..)`; helpers `label_tone(Option<&str>) ->
Tone` and `initials(&str) -> String` (`ASSIGNEE_INITIALS` = 2); `RunMark::{Pending, Stalled,
Working, NeedsYou, Succeeded}` and `BlockedTone::{Muted, Warning}`, both with `ALL` for the
gallery.
**States.** default · hover (a `border_strong` hairline and `theme.lift_shadow()`, and the `⋯`
appears) · selected (`row_selected`; the `⋯` stays, and the key line makes room for it so it
never covers the state pill) · focused (2 px cursor bar) · selected +
focused.
**Pointer** (UX-SPEC §5.1). A press selects, a double-click's second press opens, a right click
calls `on_secondary_click` — the caller selects there and wraps the tile in a `ContextMenu`. The
`⋯` is that same menu's visible twin, so no card action is right-click-only.
**Usage rule.** Selection and focus are the **same two tokens `ListView`'s cursor row uses**, so
a board and a list say "where am I" identically. A label's color arrives as a **token name**
(`"accent"`, `"danger"`), never as a hex string: a remote backend cannot smuggle a color into a
Fleet surface, and an unknown name falls back to the neutral chip. Everything but the key line
and the title is zero-suppressed, so a bare card costs exactly a key and a title. `run` wins over
`blocked` — a card that is already running has nothing left to wait for — and `blocked(0, _)`
draws nothing, as every other zero does. Both marks are **kit vocabularies, not domain types**
(§5.2), and every word on the tile (`working 4m · codex`, `blocked by FLT-5`, the branch) arrives
as a string: the app folds the run, the blockers and the worktree; the tile only draws them.

#### `KanbanColumn` / `KanbanBoard`
**Purpose.** The column and the horizontal scroller that holds the columns.
**Anatomy.** A `surface` well, `radii.dialog`, `border` hairline, `sm` padding and gap. A header
row: the category **dot** (`dot_size_small`) · the status name as written (`UiStrong`) · the count
(`Caption`, faint) · the **add button** slot at the right end. Under it, when entering the column starts
something, the **automation pill** (`tile_chip_h`, `control` fill, an amber `zap`, `Caption`
words). Then the gapped body of tiles, virtualized through `gpui::list` when the caller supplies
a `ListState`, ending in the **footer** slot · a faint `Caption` hint when the column is empty ·
`Pane`'s 2 px focus ring.
**API.** `KanbanColumn::new(id, title).count(usize).accent(Option<Hsla>).automation(label)
.on_automation_click(..).focused(bool).width(Pixels).empty_hint(..).add_button(impl IntoElement)
.footer(impl IntoElement)` then either `.tiles(impl IntoIterator<Item = AnyElement>)
.scroll_handle(ScrollHandle)` or `.rows(ListState, usize, impl FnMut(usize, &mut Window,
&mut App) -> AnyElement)`; `KanbanColumn::list_state() -> ListState` builds the state with the
column's own overdraw, and `list_state_with_footer()` one already holding the footer's item.
`COLUMN_WIDTH_CH` = 34. `KanbanBoard::new(id).columns(..).scroll_handle(ScrollHandle)`.
**States.** default · focused (the 2 px pane ring) · empty (the hint, then the footer) · with and
without the automation pill.
**Variants.** column (vertical, `COLUMN_WIDTH_CH` wide) · board (the horizontal scroller).
**Usage rule.** The count renders even at `0` — a column header is a ledger, and a missing
count reads as "unknown", not as "empty". `automation(..)` takes the words, and which columns
deserve them and what they say is domain knowledge the app keeps (`BOARD.md` §11.8). `accent`
takes a resolved `Hsla` because the status category → token mapping lives in the app; the call
site passes a theme token and never a literal. In the `rows(..)` form the footer is the list's
**last item**, so it follows the cards rather than sinking to the column's floor, and the caller's
`ListState` holds `count + 1` items with tiles spliced in before it. `tiles(..)` is for a fixed
handful of rows; anything bounded by data uses `rows(..)`, because a column handed finished
elements builds and measures every one of them every frame. `gpui::list` and not
`uniform_list`: a `CardTile` is not uniform-height. Neither container binds a key: `h` / `l` /
`j` / `k` move a cursor the screen owns, exactly as they do for `ListView`. Every pointer
affordance is a slot, so a drop target or a drag handle can join a column without the kit
learning what a card is.

**Gallery.** The board group's bench is `examples/gallery_board.rs` (live cursor, live editor,
`[` / `]` moving a card, `p` cycling the priority); `kit_gallery`'s `board` section shows the
same components as a static overview in both themes.

### 6.8 Controls

The primitives every redesigned surface is built from (ADR 0023): a key chip that always says
what the keymap says, a button that shows one, an icon-only button, a tooltip, and the menus a
button or a right-click opens. No button, trigger or tooltip is focusable; a menu takes the focus
only while it is open. The keyboard path to a control is the key on its chip, so a click and a key
press are one dispatch, and `Tab`, `j`/`k` and pane focus keep their meaning.

**How the app passes a key in.** The kit takes no domain type, but a gpui `Action` is not one: it
is the same currency the keymap binds. A view hands the kit the action (`Button::action(Box<dyn
Action>)`, `Kbd::for_action(&dyn Action, &Window, &App)`), and the kit asks the window for the
highest-precedence binding in the context the focused handle has in the last painted frame
(`Window::highest_precedence_binding_for_action_in`), or in the context another `FocusHandle`
would have (`Kbd::for_action_in`). It deliberately does not call
`Window::highest_precedence_binding_for_action`: at gpui v1.18.1 that reads whatever context stack
the dispatch tree was left holding when painting ended rather than the focused element's path, and
in the gallery it resolved no chip at all. A binding the focused context does not reach, or an action nothing binds, gives
`None` and the control shows no chip. So a Workspace button's chip carries the `⌃S` prefix
because the binding does, and a remapped key shows its new spelling with no app change.

**Lookup at render, not cached.** Like Zed's `KeyBinding::for_action`, the chip is resolved inside
`render`. The lookup is a hash probe for the action's bindings plus a walk of the keymap for each
candidate against the context stack to rule out shadowing — a few thousand predicate checks per
chip, under the 8 ms budget for the handful of chips a surface shows, and a render happens only
on a notify. A `Global` cache per keymap revision was rejected: its key would have to include the
focused context stack, which changes with every focus move, so it would miss exactly when it
matters. The one visible cost is the same as Zed's: the chip reads the *previous* frame's dispatch
tree, so on the first frame after a focus change it can show the old context's key for one frame,
and a window's very first frame (nothing painted yet) shows no chips until focus lands and gpui
redraws.
Revisit if a list ever renders a chip per row.

#### `Kbd`
**Purpose.** A keystroke sequence as chips: one chip per stroke, the modifiers inside it —
`⌃S` `a`, `g` `b`, `⇧Y`.
**Anatomy.** Per stroke: a `kbd_h` (or `kbd_h_small`) chip at least as wide as it is tall, `xs`
padding, `radii.sm`, `kbd_bg` fill inside a `kbd_border` hairline, the key in the `Hint` role in
`text_secondary`; strokes `xs` apart.
**Spelling.** Modifiers in Apple's order (⌃ ⌥ ⇧ ⌘): stacked glyphs on macOS (`⌃⌥⇧⌘K`), words
joined by `+` elsewhere (`Ctrl+Alt+Shift+Super+K`). Named keys: `⏎ esc ⇥ ␣ ⌫ del ↑ ↓ ← →`,
`home`, `end`, `pgup`, `pgdn`. A key under a modifier is a key cap (`⌃S`, `Alt+F5`); a bare
key is the character typed (`g`). An uppercase key in the keymap is `shift` (`Y` → `⇧Y`).
**API.** `Kbd::for_action(&dyn Action, &Window, &App) -> Option<Kbd>`,
`Kbd::for_action_in(&dyn Action, &FocusHandle, &Window) -> Option<Kbd>`,
`Kbd::for_action_preferring(&dyn Action, keys, &Window, &App) -> Option<Kbd>` (show `keys` when
the focused context binds the action to it, else the highest-precedence binding),
`Kbd::from_binding(&KeyBinding)`, `Kbd::new(&[Keystroke])`,
`Kbd::parse("ctrl-s a") -> Result<Kbd, InvalidKeystrokeError>`; then
`.tone(KbdTone::{Default, OnAccent, OnDanger, Warning}) .size(KbdSize::{Default, Small})`;
`Kbd::{strokes, chip_labels, aria_shortcut}` for tests and accessibility.
**States.** default · small · on accent · on danger · warning (the held prefix in the ⌃S command
menu's header, and nowhere else). Not focusable, not disabled on its own (it dims with its
button).
**Usage rule.** `fleet-app` resolves a chip with `for_action` / `for_action_in` and never types a
key. A chip for a key bound somewhere other than the focused context — Help describing the
surface under it, the agent popup's prefix keys — is built from that row of `keymap::table()`
(`Kbd::parse` on its keys, `Kbd::new` on its strokes), which is still the live keymap; a
literal key string is for galleries and tests only. `pretty_keys` (the old `^s` / `S-⇥` text
spelling) lives beside it only until the palette and the prefix toast are rebuilt (Help no
longer uses it); `fleet_app::presentation::pretty_keys` re-exports it.

#### `Button`
**Purpose.** A verb the pointer can press, showing the key that does the same thing.
**Anatomy.** `[icon] label [Kbd]`, `sm` apart, `md` side padding (`sm` compact), `button_h` (or
`button_h_compact`) tall, `radii.control`, a hairline. Label in `UiStrong`; icon 14 px (12
compact); the chip `kbd_h` (`kbd_h_small` compact) and toned for the fill.
**API.** `Button::new(id, label)` then `.style(ButtonStyle::{Primary, Secondary, Ghost, Danger, GhostDanger})
.size(ButtonSize::{Default, Compact}) .icon(Icon) .kbd(Kbd) .action(Box<dyn Action>) .prefer_key(keys)
.on_click(Fn(&ClickEvent, &mut Window, &mut App)) .disabled(bool) .selected(bool) .full_width()
.tooltip(text)`.
**Styles.** `Primary`: `accent_fill` / `accent_fill_hover` / `accent_fill_active`, label and chip
in `accent_fill_text` — a surface's one primary action. `Secondary` (default): `control` /
`control_hover` / `control_active` in a `control_border` hairline. `Ghost`: no fill until
hovered, label `text_secondary`. `Danger`: `danger` / `danger_fill_hover` / `danger_fill_active`
under `text_inverse` — the strong form of a destructive action, the `Y` of a
`ConfirmKey::Upper` confirmation (§4). `GhostDanger`: a `Ghost` whose label is `danger` — a
destructive action that is not the surface's main one, set apart rather than shouted (an
approval's "Deny and stop", §6.6).
**States.** default · hover (pointer only) · pressed · selected (`row_selected` fill, announced as
toggled — only for a toggle, and only on `Secondary` / `Ghost`) · disabled (40 %, no hover, no
click). Never focused.
**Usage rule.** Wire it with `.action(..)`: the click dispatches that action to the focused element
with `window.dispatch_action`, and the chip is `Kbd::for_action` of the same action unless `.kbd`
overrides it — so the button and its key cannot disagree. `.on_click` is for a control that is not
an action, and runs before the action when both are set. An action invalid on the surface is
**not rendered**; `.disabled(true)` is only for an action that will become valid on this surface
(§4). The accessible name is the label; a single-stroke chip is also announced as the shortcut
(`aria-keyshortcuts`, which cannot express a sequence).

#### `StatusButton`
**Purpose.** A live count that opens what it counts: `1 needs you`, `2 jobs`, `1 failed`,
`Update 0.2.0`, `fleetd down` — the title bar's status cluster.
**Anatomy.** A compact `Ghost` frame; a mark (`StatusMark::{Dot, Icon(Icon), Spinner}`) then the
label, both in the button's tone, `xs` apart, `sm` side padding.
**API.** `StatusButton::new(id, label).mark(StatusMark).tone(Tone).tooltip(text)` and the shared
`.action .on_click .kbd .disabled .selected .size .style`.
**States.** as `Button`; the tone is the state (amber waits for a person, red failed, secondary is
running or informational).
**Usage rule.** The caller zero-suppresses: render one only while its count is non-zero. The key
rides in the tooltip beside the label rather than on the face, because a status cluster is read at
a glance. Prefer `.action(..)`; `.on_click` only where the target depends on state (`1 needs you`
opens that thread, `2 needs you` the agents picker).

#### `SwitcherButton`
**Purpose.** Names the current choice and opens a menu of the others: the title bar's context
switcher (`[A] Acme ⌄`), a breadcrumb's worktree.
**Anatomy.** A compact `Ghost` frame; an optional monogram tile (`monogram_size`, `accent_subtle`
fill, the label's first letter in `accent`) or glyph, the label in `UiStrong` `text`, a trailing
`chevron-down`.
**API.** `SwitcherButton::new(id, label).monogram().icon(Icon).tooltip(text)` and the shared
builders; pass the popover's `open` to `.selected(..)`.
**Usage rule.** Always a `PopoverMenu` trigger with no action of its own. The chevron says a menu
opens; each choice's key is on its menu row, not on the switcher.

#### `IconButton`
**Purpose.** A square, glyph-only button for dense headers and toolbars.
**Anatomy.** `button_h` (or `button_h_compact`) square, the glyph centred, `Ghost` by default.
**API.** `IconButton::new(id, Icon, label)` then the same `.style .size .kbd .action .on_click
.disabled .selected` as `Button`.
**States.** as `Button`.
**Usage rule.** The label is a constructor argument so it cannot be forgotten: it is the tooltip,
and the accessible name. The key is not drawn on the button; it rides in the tooltip, resolved
the same way. Use a labelled `Button` whenever there is room for the word.

#### `Tooltip` / `WithTooltip`
**Purpose.** A label, plus its key when it has one, floating by the pointer after
`motion.tooltip_delay`.
**Anatomy.** `elevated` fill, `border_strong` hairline, `radii.popover`, `popover_shadow()`;
`Caption` label in `text` and a small `Kbd`, `sm` apart; inset `sm` from the pointer.
**API.** `Tooltip::new(label).kbd(impl Into<Option<Kbd>>)`; `element.with_tooltip(Tooltip, cx)`
on any element with an id (a blanket trait over gpui's `StatefulInteractiveElement`), which sets
gpui's tooltip builder and show delay. `Tooltip` is also an `IntoElement`, for the gallery.
**States.** hidden · shown. Pointer only: a tooltip never holds anything the keyboard user
lacks, so it names an icon-only control and repeats its key.
**Usage rule.** `IconButton` always has one; a `Button` takes one only to explain itself further
(`.tooltip(text)`), since its key is already on it. Never put the only statement of a fact in a
tooltip.

#### `Menu` / `MenuItem`
**Purpose.** A floating list of verbs: a row's ⋯ menu, its right-click menu, the `+` new-tab
menu, the context switcher, a dropdown's options. Never placed by hand: one of the three wrappers
below opens it.
**Anatomy.** `elevated` fill, `border_strong` hairline, `radii.popover`, `popover_shadow()`, `xs`
padding, at least `menu_min_w` wide. An item is `row_h` tall, `sm` side padding, `radii.control`:
`[icon] label [detail] [✓] [Kbd]`, icon 14 px, label `Ui` in `text_secondary` (`text` when highlighted),
the detail a muted `Ui` word naming the choice's state,
the chip small and right-aligned. The highlight is a `control_hover` fill. A header is a
`SentenceLabel` in `text_muted`, `section_header_h` tall; a separator is a `border` hairline inset
`sm` with `xs` above and below.
**API.** `Menu::build(window, cx, |menu, window, cx| menu.header(..).item(..).separator())` (the
wrappers call it); `MenuItem::new(label).icon(Icon).detail(text).action(Box<dyn Action>)
.on_select(Fn(&mut Window, &mut App)).kbd(Kbd).destructive(bool).checked(bool)`;
`Menu::{item_labels, highlighted, is_empty, dismiss}`. Keys: `menu_actions::{SelectNext,
SelectPrevious, Confirm, Cancel}` against `MENU_KEY_CONTEXT` (`FleetMenu`); `menu_key_bindings()`
binds them for a gallery or a test, and `fleet-app`'s key table binds the same keys
(`KEYMAP.md` § *Open menus*). `menu_holds_focus(window, cx)` says whether an open menu has the
focus, for a shell that reconciles focus from its own state (`APP-CONTRACTS.md` §3).
**States.** item: default · highlighted (hover or `↑`/`↓`) · destructive (icon and label in
`danger`) · checked (a `✓`, announced as a radio item) · with a detail word (the worktree
switcher's `sleeping`) · with or without icon and chip. No
disabled item.
**Behaviour.** Opening captures the focused element as the menu's origin and moves the focus into
the menu. `↓`/`ctrl-n` and `↑`/`ctrl-p` move the highlight and stop at the ends; `⏎` activates;
`esc` closes. Hover moves the highlight, a click activates, a mouse-down anywhere outside closes.
Closing gives the focus back to the origin. Activating closes first, then runs the item's
`on_select`, then dispatches its action to the origin with `window.dispatch_action` — the same
dispatch its key makes. The chip is `Kbd::for_action_in(action, origin)`, so it names the key the
action has *where the menu was opened*.
**Usage rule.** An item carries the same action its key dispatches; `on_select` is for what is
not an action (a dropdown option). An item whose action nothing on the origin's dispatch path
handles is **left out** by `build`, and a header or separator left with nothing to head or
separate goes with it (§4: hidden, never greyed). A menu opens with its checked item highlighted,
else its first. Build it from an event, never before the window has painted: checking an action
needs a painted dispatch tree. There is no `j`/`k` in a menu (§4). Each open item paints
`menu.item[N]` (`TESTING-HARNESS.md` §3).

#### `PopoverMenu`
**Purpose.** A trigger that opens a `Menu` hanging below it.
**API.** `PopoverMenu::new(id).trigger(impl IntoElement)` or `.trigger_with(|open, window, cx|
..)`, `.menu(|menu, window, cx| ..)`, `.anchor(MenuAnchor::{BottomLeft, BottomRight})`,
`.full_width()`. The id must be stable across frames: the open menu is kept under it.
**Anatomy.** The menu paints through `anchored()` inside `deferred()` at `OverlayLayer::Menu`,
`xs` below the trigger, its left edge under the trigger's (`BottomLeft`, the default) or its right
edge under the trigger's right edge (`BottomRight`, for a trigger at the end of a row or header),
and flips or snaps to stay `sm` inside the window.
**States.** closed · open (`trigger_with` receives `open`, so an `IconButton` passes it to
`.selected(open)` and stays pressed; the wrapper announces `aria-expanded`).
**Usage rule.** The trigger is a `Button` or `IconButton` with **no** action of its own: the
popover owns the click. Clicking the trigger of an open menu closes it. A popover is a way to
*reach* actions that also have keys; it never holds the only path to one (ADR 0023).

#### `ContextMenu`
**Purpose.** A right-click menu for the row, card or pane it wraps.
**API.** `ContextMenu::new(id, child).menu(|menu, window, cx| ..)`; a list keys the id by row,
`("worktree-menu", ix)`.
**Behaviour.** A right mouse-down anywhere in the child opens the menu with its top-left corner at
the pointer (flipped or snapped to stay inside the window), and stops the event there. A second
right-click elsewhere closes the first menu and opens the new one.
**Usage rule.** Every verb in a right-click menu is also on a visible control — the row's ⋯
`PopoverMenu`, its detail panel — and has a key; the ⋯ menu and the right-click menu of a row are
built by the same function so they cannot drift.

#### `Dropdown`
**Purpose.** A field showing the current value and a chevron, opening a `Menu` of the options with
a `✓` on the chosen one. It replaces the `Cycler`'s `‹ value ›` in Settings, Create worktree and
the card detail.
**Anatomy.** `text_field_h` tall, `md` side padding, `radii.control`, `bg` fill in a `border`
hairline (`border_strong` on hover, `focus_ring` while open); the value in `Ui`, a 12 px
`chevron-down` in `text_secondary`. An optional `SentenceLabel` above it, `xxs` apart.
**API.** `Dropdown::new(id, value).label(text).menu(|menu, window, cx| ..).full_width()
.compact()`; the caller builds one `MenuItem` per option with `.checked(is_current)` and an
`.on_select(..)` (or an action). `compact()` is `button_h_compact` tall, to sit in a `row_h`
settings row. With no `menu` the field is drawn only: no pointer, no hover, no list.
**States.** closed · hover · open · compact · drawn only. The list opens with the chosen option
highlighted.
**Usage rule.** A dropdown for a set longer than a segmented control holds and short enough to
read whole; a `FuzzyList` under a text field for a set to search.

#### `SegmentedControl` / `Segment`
**Purpose.** Two to four options side by side, the chosen one raised: the Hub's screens, the agent
popup's provider, and (through `Cycler`) every short settings choice.
**Anatomy.** A `chrome` trough in a `border` hairline, `radii.control`, `xxs` inset and gap. Each
segment is `segment_h` tall, `md` side padding, `radii.md`: optional 14 px icon · label in `Ui` at
the `UiStrong` weight, sentence case as given · optional count in `Caption` (`…` while loading, in
`warning`) · optional small `Kbd`. The raised segment is the `control` fill in a `control_border`
hairline with `text`; the others are clear with `text_secondary` and keep a clear hairline, so
raising one never shifts its neighbours. `row_hover` on a clickable, unraised segment.
**API.** `SegmentedControl::new(id, [Segment::new(label).icon(Icon).count(Option<usize>)
.loading(bool).kbd(Option<Kbd>).disabled(bool)]).active(Option<usize>).disabled(bool).full_width()
.on_select(Fn(ix, window, app)).harness_segments(part)`; `Segment::count_text()`,
`SegmentedControl::{len, is_empty}`.
**States.** raised · none raised (`active(None)`) · hover · with icons · with counts and a
loading count · with a key chip · one segment unavailable (dimmed, takes no click) · disabled ·
full width.
**Usage rule.** No keyboard of its own and not focusable (ADR 0023): the surface binds the keys,
and `on_select` dispatches the action those keys dispatch. A key chip shows only on a segment a
click would switch to. Past four options, or labels that no longer fit, use a `Dropdown`.

#### `Switch`
**Purpose.** A boolean at a glance: a pill track with a knob.
**Anatomy.** `switch_w` × `switch_h`, `radii.pill`; on is `accent_fill` with the knob at the end,
off is `text_muted` with the knob at the start; the knob is an `accent_fill_text` circle `xxs`
inside the track. Hover deepens the track (`accent_fill_hover` / `text_secondary`).
**API.** `Switch::new(id, on).name(label).disabled(bool).on_toggle(Fn(bool, window, app))`.
**States.** on · off · hover · disabled on · disabled off.
**Usage rule.** Inside a settings list, use `Toggle`, which owns the row, the label and `Space`.
Not focusable; announced as `Role::Switch` with the setting's name.

#### `Checkbox`
**Purpose.** A boolean that qualifies the action beside it: "Open after creating" next to Create.
**Anatomy.** A `checkbox_size` square, `radii.xs`: checked is `accent_fill` with an
`accent_fill_text` check, unchecked is `control` in a `border_strong` hairline; then the label in
`Ui`. The whole box-and-label takes the click; hover deepens the square.
**API.** `Checkbox::new(id, label, checked).disabled(bool).on_toggle(Fn(bool, window, app))`;
`.is_checked()`.
**States.** checked · unchecked · hover · disabled.
**Usage rule.** A boolean *setting* is a `Toggle` row, not a checkbox. Not focusable (ADR 0023):
the surface keeps its key for the same choice (`⌥⏎` creates without opening whatever the box
says). Announced as `Role::CheckBox` with its label.

#### `Callout`
**Purpose.** State what an action *will* do before the user commits: "Prepared copy ready —
about 2 s".
**Anatomy.** A `radii.md` box filled with the tone's fill in a tone-coloured hairline at
`banner_border_opacity`, `md`/`sm` padding: a 16 px icon in the tone, one line of `Ui`, and an
optional muted detail line under it.
**API.** `Callout::new(Tone, Icon, text).detail(..).actions(impl IntoElement)`.
**States.** success · warning (any `Tone`), with and without detail, with and without actions.
**Usage rule.** Information, not an alarm: a `Banner` spans a screen and says something is wrong
now; a `Dialog`'s footer error says the last action failed. On a page, a callout may state what
is wrong with it and carry the compact buttons that answer it at its right end — the board's
load error with `Reload`, its sync error with `Board settings`.

**Gallery.** `examples/gallery_buttons.rs` binds a keymap and wires every button with
`.action(..)`, so each chip there is resolved live and clicking or pressing the key reports the
same action in the status bar; it shows every style × size with and without icon and chip, the
disabled, selected and full-width states, an unbound action with no chip, icon buttons with their
tooltips, and each `Kbd` spelling and tone. `examples/gallery_menus.rs` binds the menu keys and a
keymap, and shows a row with a ⋯ `PopoverMenu` (`BottomRight`), a `+ New tab` button menu
(`BottomLeft`, with a header, icons and `⌃S` chips), a right-click area, a labelled and a
full-width `Dropdown`, an item whose action nothing handles (left out of every menu), and an
always-open `Menu` showing a header, icon, chip, check, separator and destructive item at once.
`examples/gallery_input.rs` shows every `SegmentedControl`, `Switch`, `Checkbox`, `Callout`,
`Cycler` form and `Toggle` state, the confirm in its compact and danger forms with every button
live, with the host and step cyclers, the segmented control and the first toggle live under the
pointer. `kit_gallery`'s `controls` section is the overview.

---

## 7. What is deliberately not in the kit

- **A generic `Card`.** `Pane`, `Dialog`, `Sheet` and `Overlay` are the four surfaces; a fifth
  would erode the meaning of the other four.
- **Progress bars.** A phase word (`copying files…`, `hooks 2/3`) is more honest than a
  percentage, and `JobRow` shows a percent only when the job actually parses one.
- **A disabled row or field.** "You cannot edit this here" is said by the *absence* of an input
  box (a read-only `FactRow`), and an unavailable command is not listed at all. Only a button
  that becomes valid on the same surface is drawn disabled (§4).
- **Decorative color.** Four semantic colors and three neutrals. Nothing else.

## 8. Changing the system

1. A new token goes in `theme/tokens.rs` **and** in §2 of this document, in the same commit.
2. A new icon means downloading the SVG into `crates/fleet-ui-kit/assets/icons/`, rewriting its
   `stroke-width` to 1.5, adding the variant to the `lucide_icons!` list in `src/icons.rs`, and
   adding it to §5.1 here.
3. A new component gets its own module under `src/components/`, a `pub use` in
   `components/mod.rs`, an entry in §6 here, and a panel in the matching per-group gallery
   (`gallery_structure`, `gallery_data`, `gallery_input`, `gallery_buttons`, `gallery_menus`, `gallery_terminal`,
   `gallery_agent`, or `gallery_board`) showing **every** state. `kit_gallery` remains the combined overview. If a
   state is not in a gallery, it is not implemented.
4. `cargo check -p fleet-ui-kit --examples` and
   `cargo run -p fleet-ui-kit --example kit_gallery` are the acceptance gate.
