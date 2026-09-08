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
   `accent` — and `accent` is *only* the cursor and the focus ring. Draft, muted, disabled and
   "not applicable" lower contrast; they never add a hue. This is why components take a
   [`Tone`], not a color.
5. **Errors are sticky; successes are transient.** `StickyErrorSlot` persists until dismissed;
   `Toast` decays. `Toast` never carries `Tone::Danger`.
6. **The mode is always a visible word.** `ModeWord` is mandatory on every screen, at a fixed
   84 px, including the Workspace and including zoom.
7. **Everything is a key.** There are no buttons anywhere in Fleet. Affordances are `KeyHint`s.

---

## 2. Tokens

Rust: `fleet_ui_kit::theme` — `ColorTokens`, `TerminalPalette`, `TypeScale`, `Spacing`, `Radii`,
`Elevation`, `Motion`, `Metrics`, assembled into `Theme` and installed as a gpui `Global`.

```rust
Theme::init(ThemeMode::Dark, cx);          // once, before any component renders
Theme::change(ThemeMode::Light, cx);       // explicit switch
Theme::toggle(cx);                         // returns the new mode

let theme = cx.theme();                    // ActiveTheme, on anything that derefs to App
theme.colors.warning;  theme.space.md;  theme.metrics.row_h;  theme.motion.sheet;
```

`ActiveTheme` is implemented for `App`; `Context<T>` derefs to `App`, so `cx.theme()` works in
both `Render::render` and `RenderOnce::render`.

### 2.1 Color roles

`bg` / `surface` / `elevated` / `overlay` are the four grounds; there is no fifth. `row_hover`
exists only for the pointer and never expresses state.

| Role | Dark | Light | Use |
| --- | --- | --- | --- |
| `bg` | `#0E1013` | `#FBFBFC` | app ground, context bar, status bar |
| `surface` | `#16181D` | `#FFFFFF` | rails, panes, scroll pill, prefix hint |
| `elevated` | `#1B1E24` | `#FFFFFF` | dialogs, sheets, toasts, palette |
| `overlay` | `rgba(0,0,0,.45)` | `rgba(0,0,0,.25)` | dialog scrim (ghosts the base screen) |
| `row_selected` | `#1E2430` | `#EDF2FB` | cursor row |
| `row_hover` | `#191C22` | `#F3F4F6` | pointer hover only |
| `text` | `#E6E8EB` | `#16181D` | branch, title, value |
| `text_secondary` | `#8A9099` | `#6B7280` | repo, host, age, counts, labels |
| `text_muted` | `#5A6069` | `#9CA3AF` | draft, disabled, key hints, the null dash |
| `text_inverse` | `#0E1013` | `#FBFBFC` | text on an accent or semantic fill |
| `accent` | `#58A6FF` | `#0969DA` | cursor and focus **only** |
| `success` | `#3FB950` | `#1A7F37` | attached, pass, approved, done |
| `warning` | `#D29922` | `#9A6700` | running, pending, dirty, unknown, degraded |
| `danger` | `#F85149` | `#CF222E` | failed, changes requested, destructive |
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
| `ui` | system | 13 / 18 | 400 | as written | body text, values, row content |
| `ui_strong` | system | 13 / 18 | 500 | as written | active tab, row title, primary action |
| `title` | system | 15 / 20 | 500 | as written | dialog and detail-panel titles |
| `data` | mono | 12.5 / 18 | 400 | as written | branch, path, sha, target, head ref |
| `data_small` | mono | 11.5 / 16 | 400 | as written | job progress sub-line, log tail |
| `label` | system | 11 / 14 | 500 | **UPPERCASED** | pane and section labels, counts, mode word |
| `hint` | mono | 11 / 14 | 400 | as written | key hints |

One mono cell is **7.5 × 18 px**; `1 ch = 7.5 px` (`theme::CH`, `theme::ch(n)`). Every column
budget in the UX spec is stated in `ch` and resolved with `ch()` or `ColumnLadder`.

`TypeStyle::tracking` records the spec's `.06em` on the label role, but gpui 1.18.1's `Styled`
exposes no letter-spacing setter, so `Text` cannot apply it yet. That is the one token in this
document that is data rather than behaviour.

### 2.4 Spacing (4 px base)

`xxs 2` · `xs 4` · `sm 8` · `md 12` · `lg 16` · `xl 24` · `xxl 32`.

`md` (12 px) is the gap between list columns and the horizontal padding of a row; `lg` (16 px)
is pane padding and dialog padding. `xxs` exists only inside a chip.

### 2.5 Radii

`none 0` · `xs 3` · `sm 4` · `md 6` · `lg 12` · `full 9999`.

Rows, panes and the terminal grid are square (`none`). Inputs, badges and in-dialog lists are
`sm`. Toasts, sheets and the scroll pill are `md`. Dialogs and the palette are `lg`. Chips and
status dots are `full`.

### 2.6 Elevation

Four levels, and only two of them have a shadow.

| Level | Surface | Treatment |
| --- | --- | --- |
| 0 | `bg` | nothing |
| 1 | `surface` | 1 px `border` hairline, no shadow |
| 2 | `elevated` | `0 8px 24px rgba(0,0,0,.35)` dark / `.12` light — sheets, toasts |
| 3 | `elevated` | `0 16px 48px rgba(0,0,0,.45)` dark / `.16` light — dialogs, palette |

`theme.sheet_shadow()` and `theme.dialog_shadow()` return the `Vec<BoxShadow>`.

### 2.7 Motion

Fleet animates four things and nothing else. Nothing blinks, nothing re-announces itself,
nothing decays on a timer the user did not set.

| Token | ms | What |
| --- | --- | --- |
| `toast` | 140 | toast slide + fade |
| `sheet` | 160 | Jobs sheet slide |
| `highlight` | 120 | value-swap highlight when a fact refreshes in place |
| `prefix_hint_delay` | 400 | how long `^S` waits before showing its keys |
| `spinner` | 1000 | one turn of `loader-circle` |
| `toast_short` | 1600 | dwell for instant acknowledgements |
| `toast_normal` | 3200 | dwell for everything else the toast law allows |

### 2.8 Metrics

`Metrics` holds every pixel constant the UX spec pins down, so no component hard-codes one:
`context_bar_h 36` · `status_bar_h 26` · `pane_header_h 30` · `row_h 30` · `palette_row_h 34` ·
`job_row_h 44` · `section_header_h 20` · `dialog_header_h 44` · `dialog_footer_h 44` ·
`banner_h 28` · `strip_h 22` · `chip_h 22` · `rail_w 240` · `detail_w 340` · `sheet_w 440` ·
`sheet_expanded_w 640` · `toast_w 320` · `palette_w 640` · `palette_top 120` ·
`mode_word_w 84` · `scroll_thumb_w 3` · `focus_ring_w 2` · `cell_w 7.5` · `cell_h 18`.
The smaller component metrics live here too: `hairline 1` · `dot_size 8` ·
`dot_size_small 6` · `fact_label_w 104` · `doctor_check_w 120` · `doctor_status_w 64` ·
`text_field_h 36` · `field_status_h 18` · `palette_input_h 44` · `number_field_w 96`.
So do the Git UI's own dimensions: `status_pane_h 62` · `stash_pane_h 92` ·
`overlay_help_w 640` · `editor_box_h 160` · `diff_caret_h 14`.
The same token set owns the opacity ladder: `veil 0.55` · `dimmed 0.40` ·
`refreshing 0.60` · `stale 0.55` · `skeleton 0.30` · `no_session 0.30`.

The native-agent canvas has its own six constants, and they live in
`components::agent::metrics` rather than in `Metrics`: `AGENT_CONTENT_W 760` ·
`AGENT_TOOL_KIND_W 60` · `AGENT_CARET_H 17` · `AGENT_BODY_MAX_H 240` ·
`AGENT_LIST_OVERDRAW 256` · `AGENT_SCROLLBAR_INSET 3`. `Metrics` is the density ladder a
theme may restate; these are fixed product decisions from `NATIVE-AGENTS.md` §2 that no theme
may move, which is exactly why they are constants and not tokens. They are still exported from
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
| hover | automatic with `.with_id(..)` | `row_hover` | pointer only, never state |
| dimmed | `.dimmed(true)` | 40 % opacity | a row being deleted |
| disabled | `.disabled(true)` | 40 % opacity, no hover | not selectable |
| loading | `SkeletonRows` | 30 % placeholder | cold load only |
| error | leading `StatusGlyph`, not a row color | — | state is a glyph, never a row tint |

The detail panel is **never** in the focus cycle: `j`/`k` always move the list cursor and the
panel mirrors it.

---

## 4. Keyboard hint conventions

- A hint is `KeyHint::labeled(keys, label)`; a footer is a `KeyHintRow`, whose items are joined
  with `·`.
- Keys render in the `hint` role (mono 11, `text_muted`). The label is `text_muted` too; raise
  the **key** with `.key_tone(Tone::Default)` only for a dialog's primary action.
- **Lowercase = safe, uppercase = stronger variant.** `FactList::confirm_key()` returns
  `ConfirmKey::Lower` (`y`, and `Enter` is accepted) or `ConfirmKey::Upper` (`Y`, and `Enter` is
  **not** accepted). Never hand-write that decision.
- **Inside the Workspace every hint carries its prefix.** Terminal mode sends all keys to the
  PTY except `ctrl-s`, so bare-key hints are forbidden there: write `^s r`, `^s x`, `^s ⏎`.
  `ExitStrip` defaults to the prefixed form for exactly this reason.
- A list **under a text field** moves with `ctrl-n`/`ctrl-p` or `↓`/`↑` and never `j`/`k`; a
  dialog with no text field does bind `j`/`k`. `FuzzyList::binds_jk()` states which case a list
  is in.
- An invalid command is **not listed**, never greyed: a greyed row costs a `j`.

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

### 5.1 The closed icon set (68 glyphs)

| Purpose | Icons |
| --- | --- |
| Session state | `circle-dot` `circle` `moon` `dot` `circle-question-mark` |
| Jobs | `loader-circle` `clock` `circle-stop` `circle-slash` `circle-check` `circle-x` `activity` `copy-plus` `refresh-cw` `search-check` `import` `trash-2` |
| Warnings | `triangle-alert` `message-square-warning` `info` |
| Git / PR | `git-branch` `git-branch-plus` `git-merge` `git-fork` `git-pull-request` `git-pull-request-draft` `git-commit-horizontal` `file-diff` `file-pen` `folder-git-2` `eye` |
| Remote | `cloud` `cloud-off` `cloud-upload` `cloud-download` `globe` `lock` `unplug` `server` |
| Keep-alive | `zap` `bot` `sparkles` `server` `file-pen` |
| Terminal | `terminal` `square-terminal` `chevrons-up` `command` `maximize-2` `plus` |
| Dialogs | `trash` `scissors` `power` `x` `boxes` `arrow-right-left` `settings-2` `hourglass` |
| Chrome | `flag` `circle-arrow-up` `circle-arrow-down` `search` `clipboard-check` `delete` `ellipsis` `check` `minus` `chevron-left` `chevron-right` `sailboat` |

Two Lucide renames the UX spec predates: `circle-help` is now **`circle-question-mark`**, and
`arrow-up-circle` is now **`circle-arrow-up`**. The kit keeps both `trash` and `trash-2`, since
the former labels destructive dialogs while the latter distinguishes permanent job cleanup.

### 5.2 Status glyph vocabulary (`StatusKind`)

`StatusGlyph` is the single source of truth: the shape a user learns in the worktrees list is
the same shape in the palette, in a confirm and in the Workspace header.

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

---

## 6. Component catalog

Every component is a `RenderOnce` + `IntoElement` struct with a `new`-style constructor and
chained builder methods. None of them owns state: the view's entity owns the cursor, the query,
the focus and the timers, and passes them down each frame. That is deliberate — `RenderOnce`
cannot hold state, and the alternative (an entity per component) would make cursor stability
under background updates impossible to reason about.

Legend for the "states" rows: **default · focused · selected · disabled · loading · error**.
A component that cannot be in a state says so rather than pretending.

### 6.1 Foundation

#### `Theme` / tokens
**Purpose.** One resolved token set per appearance, installed as a gpui `Global`.
**API.** `Theme::{dark, light, for_mode, init, change, toggle, is_dark}`,
`Theme::{resolve_mono_family, with_mono_family, shadow, dialog_shadow, sheet_shadow}`,
`ThemeMode::{toggled, is_dark}`,
`trait ActiveTheme { fn theme(&self) -> &Theme }`.
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
**Purpose.** A run of text in one of the seven type roles.
**API.** `Text::{ui, ui_strong, title, data, data_small, label, hint}(impl Into<SharedString>)`,
then `.tone(Tone) .muted() .faint() .color(Hsla) .opacity(f32) .weight(FontWeight)
.truncate_at(usize, Truncate) .ellipsize() .w(Pixels) .w_ch(f32) .flex_none()`. `Text::resolved_text()`
returns the string after the `ch` budget, for tests.
**Usage rule.** `truncate_at` when the spec names a `ch` budget; `ellipsize` when the column is
flex. `label` uppercases for you — do not pre-uppercase the string.

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
**Purpose.** Context bar (36) + optional banner (28) + flexible body + status bar (26), plus the
overlay layer.
**API.** `AppFrame::new().context_bar(..).banner(..).body(..).body_overlay(..).status_bar(..)
.overlay(..)`.
**Variants.** Hub (body = panes) · Workspace (body = terminal) · first run (body = one card).
**Usage rule.** `overlay` is the window-wide layer for dialogs and the palette; `body_overlay`
is the band between the bars for `Sheet` and `ToastStack`. Putting a sheet or toast in `overlay`
covers the status bar. Floating children never reflow what the user was looking at. The
Workspace keeps both bars at the same pixel positions as the Hub — same chrome, same saccade.

#### `OverlayLayer`
**Purpose.** The shared deferred-paint ordering for floating surfaces.
**API.** `OverlayLayer::{Sheet, Anchored, Dialog, Toast}.priority()` resolves to
`Sheet(100) → Anchored(200) → Dialog(300) → Toast(400)`.
**Usage rule.** Never pass a literal deferred priority. This order encodes the Jobs sheet,
anchored palette, blocking dialogs, and transient acknowledgements.

#### `SplitLayout`
**Purpose.** A fixed region, a hairline, and a flexible region.
**API.** `SplitLayout::{horizontal, vertical}().leading(..).trailing(..).leading_size(Pixels)
.trailing_size(Pixels).divider(bool)`.
**Usage rule.** The Hub is two nested splits: `rail | (list | detail)`. Fix the **rail** and the
**detail panel**, never the list — that is what makes "the rail never moves when `i` opens the
detail panel" a property of the layout instead of a convention.

#### `ContextBar`
**Purpose.** Which slice of the world am I in, and is anything moving in it?
**Anatomy.** `[84 px inset][numbered tabs, 2 px accent underline on the active][+n overflow]
… [status chips][daemon dot]`.
**API.** `ContextBar::new([ContextTab::new("buk", 1), ..]).active(usize).overflow(usize)
.chip(impl IntoElement).daemon(DaemonState).daemon_label(..).leading_inset(Pixels)
.empty(fact, action)`.
**States.** default · empty (`no contexts` + `N create your first context`) · daemon degraded.
**Keyboard.** `1`–`9` jump · `gt`/`gT` cycle · `N` new · `E` edit (the bar does not bind them;
the screen does).
**Usage rule.** Pass every chip, including zero-valued ones — `Chip` suppresses itself. The
84 px inset clears the macOS traffic lights; use 12 px on a platform without them.
The default is `theme.metrics.traffic_light_inset`.

#### `StatusBar`
**Purpose.** Breadcrumb · mode word · job ticker · sticky error slot.
**API.** `StatusBar::new().breadcrumb(..).breadcrumb_ch(usize).mode(Mode)
.ticker(..).error(..).trailing(..)`.
**Variants.** With ticker · with error (the error **replaces** the ticker) · Workspace (breadcrumb
is the session name).
**Usage rule.** `mode` is not optional in practice: §2.8 requires the word on every screen.

#### `Pane`
**Purpose.** A bordered region with a header slot, a body slot, a scroll thumb and the focus ring.
**API.** `Pane::{new, fixed(Pixels)}().header(..).body(..).footer(..).focused(bool)
.width(Pixels).border(PaneBorder).raised(bool).scroll_thumb(offset: f32, visible: f32)`.
**States.** default · focused (2 px ring) · scrolled (3 px thumb).
**Usage rule.** `Pane::fixed` for the 240 px rail and the 340 px detail panel; `Pane::new` for
the list, which must flex. Only one pane in a screen is `focused` at a time.

#### `PaneHeader`
**Purpose.** Label · scope · `shown/total` · visible range · stale stamp.
**API.** `PaneHeader::new("worktrees").scope(..).shown(usize).total(usize).range(first, last)
.stale(age).filter_chip(query).filter(impl IntoElement).trailing(..)`.
**Variants.** normal · filtering (`.filter(FilterBar::..)` replaces the left side **in place**) ·
filter retained (`.filter_chip("rut")`) · stale (`· stale · 2m`, amber).
**Usage rule.** Never swap the header element for a filter bar — pass the filter bar *through*
the header, or the row shifts by a pixel and the illusion of "the list did not move" breaks.

#### `Sheet`
**Purpose.** A right-docked, non-blocking panel (the Jobs panel).
**API.** `Sheet::new(open: bool).expanded(bool).width(Pixels).header(..).body(..).footer(..)`;
`.resolved_width(&Theme)`.
**Variants.** 440 px · 640 px expanded for an inline log.
**Usage rule.** Use a `Sheet`, not a `Dialog`, whenever the content is *about* the rows behind
it. A centered modal would hide exactly what the jobs refer to.

#### `Dialog`
**Purpose.** The shared modal frame: scrim + card + 44 px header + 44 px footer.
**Anatomy.** header = icon + title + subtitle, no close button; footer = key hints on the left,
the primary action **label** on the right.
**API.** `Dialog::new(title).icon(Icon).subtitle(..).width(Pixels).height(Pixels).tone(Tone)
.body(..).hints(..).hint_row(KeyHintRow).primary("⏎ Create").error(..).warning(..)`.
**Widths.** 460 context/assign · 480 compact confirm · 520 quit · 560 create/clone/expanded
confirm · 720 settings/prune · 880 help.
**States.** default · error (red footer line, dialog stays open).
**Keyboard.** `Esc` closes. There is **no OK/Cancel button pair anywhere** — the hint row states
the keys.
**Usage rule.** `Dialog` always ghosts the base screen. `Overlay` can do so when configured with
`scrim(true)` and `OverlayLayer::Dialog`, as the floating Agent terminal does. Use `ConfirmDialog`
for anything destructive so the `y`/`Y` escalation is computed, not typed.

#### `Overlay`
**Purpose.** A centered floating layer used by the top-anchored palette and the Agent popup.
**API.** `Overlay::new().top(Pixels).width(Pixels).scrim(bool).layer(OverlayLayer).content(..)`.
**Usage rule.** Default `top` is 120 px — the thinking position, not screen center. Leave
`scrim` off and use `OverlayLayer::Anchored` for the palette: it is a jump, not a decision. The
Agent popup is a modal floating surface, so it opts into the scrim and `OverlayLayer::Dialog`.

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
```
**States.** default · empty (`EmptyState`) · loading (`SkeletonRows` in the body instead).
**Keyboard.** `j`/`k`, `gg`/`G`, `ctrl-d`/`ctrl-u` — the view binds them and calls `ListCursor`.
**Usage rule — cursor stability.** Background events (status polls, PR fetches, job completions)
must **never** re-sort, re-scroll or re-focus. Adopt new data with `ListCursor::retain`, and
re-sort only on an explicit user action (`r`, filter change, repo change, screen change).
Use `ListView` for lists; use `TerminalGrid` for a terminal — `uniform_list` assumes one element
per item, which is far too much overhead per terminal row.

#### `Row` / `RowColumn`
**Purpose.** One list row: leading glyph slot, flex content, trailing columns.
**API.** `Row::{new, with_id}().leading(..).column(RowColumn).columns(..).second_line(..)
.height(Pixels).selected(bool).cursor(bool).dimmed(bool).disabled(bool).hoverable(bool)`;
`RowColumn::{fixed(Pixels, ..), fixed_ch(f32, ..), flex(..), auto(..), resolved(..)}
.align(ColumnAlign).min_width(Pixels).min_width_ch(f32)`.
**Variants.** 30 px one-line · 44 px two-line (`second_line`) · 34 px palette row.
**States.** the table in §3.
**Usage rule.** A row that changes state changes its **glyph** in place; it does not change its
background color and it does not move. Leave `leading` unset when the column does not apply —
that is the blank cell of §2.5.

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
(12 ch at 70 ch, 16 ch at 130 ch).

#### `StatusGlyph`
**Purpose.** The §2.5 vocabulary, in one place. See §5.2 for the table.
**API.** `StatusGlyph::new(StatusKind).size(IconSize).id(ElementId)`;
`StatusKind::{icon, tone, opacity, spins, frozen, detail_word, detail_sentence}`.
**Usage rule.** Never assemble a session glyph from an `Icon` and a color: a second call site is
how `unknown` starts rendering like `none`, which is the exact live defect §1.3 exists to close.
Spinning kinds need `.id(..)`.

#### `Chip`
**Purpose.** The 22 px pill of the context bar and the row chrome.
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
**API.** `PrBadge::{new(number, PrBadgeState), state_only(PrBadgeState)}().stale(bool)`.
**Variants.** `Draft` faint · `CI fail` red · `Changes` amber · `CI ···` amber · `Approved`
green · `Review` secondary · `Merged` green.
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
**API.** `FreshnessStamp::new(verb, age_secs).action(key, label).error(message).refreshing(bool)`;
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
`KeyValueList::{new, titled(title)}().trailing(..).row(label, value).mono_row(label, value)
.label_width(Pixels).refreshing(bool)`.
**Usage rule.** Use `FactValue::from_option` for every nullable inspection fact. Warnings are
rendered **verbatim** — swarm's soft-warning strings are greppable diagnostics and must never be
paraphrased.

#### `Fact` / `FactList`
**Purpose.** Risks first, safe facts after, and the decision of which confirm to show.
**API.** `Fact::{safe, risk, unknown}(text)`;
`FactList::{new, from_facts}().fact(Fact).loading(bool)`,
`.ordered()`, `.is_compact()`, `.risk_count()`, `.unknown_count()`,
`.confirm_key() -> ConfirmKey`.
**Usage rule.** Never hand-write the `y` vs `Y` decision: `confirm_key()` returns `Upper` when
any decisive fact is unknown or the facts are still loading, and `ConfirmKey::accepts_enter()`
says whether `Enter` also confirms.

#### `SectionHeader`
**Purpose.** A 20 px label row with an optional right-aligned stamp or action.
**API.** `SectionHeader::new(label).trailing(..)`.

#### `EmptyState`
**Purpose.** Exactly two centered lines: the fact, then the key.
**API.** `EmptyState::new(fact).action(key_line)`.
**Usage rule.** Rendered **in the affected pane only**, never full-screen, so neighbouring panes
stay usable. Copy is swarm's, verbatim.

#### `SkeletonRows`
**Purpose.** 30 % placeholder rows.
**API.** `SkeletonRows::new(count).row_height(Pixels)`.
**Usage rule.** Cold load **only** — used exactly once, for a cold PR fetch. Everything else
renders from `state.json` immediately; a skeleton where cached truth exists is a lie.

#### `KeyHint` / `KeyHintRow`
**Purpose.** The most repeated component in the app.
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

All input components are **presentational**: the caller owns the string, the caret, the cursor
and the focus, and handles the keys. `RenderOnce` cannot own state, and the dialogs already own
theirs.

#### `TextField`
**Purpose.** A single-line input with a blue caret and a zero-shift validation line.
**Anatomy.** optional label · 36 px box (optional leading icon, value, caret) · an 18 px line
below that holds **either** the derived preview **or** the validation message — never both, and
the slot is always present.
**API.** `TextField::new(value).label(..).placeholder(..).caret(usize).focused(bool).icon(Icon)
.preview(..).invalid(message).mono(bool).height(Pixels).hide_status_line(bool)`.
`TextFieldState` is the editing model. `TextInput` is its live, IME-safe entity — `.is_invalid()`
reports its validation state — and emits `TextInputEvent` under `TEXT_FIELD_KEY_CONTEXT`.
**States.** default · focused (accent border + caret) · placeholder (muted) · invalid (red
border, red message) · disabled (not modelled: Fleet has no disabled inputs — a field that
cannot be edited is rendered as a read-only `FactRow` with no box).
**Keyboard (the caller implements).** printable · `Backspace` · `ctrl-w` · `ctrl-u` · `ctrl-a` ·
`ctrl-e` · `←`/`→`.
**Usage rule.** The validation line replaces the preview so a failing branch name causes **zero
layout shift**. Fail before a job starts, with the exact failing rule.
An entity that installs the platform input handler must call `handle_edit_keystroke`, not
`handle_keystroke`, or every printable character is inserted twice. Dialog-level bare-letter
bindings must also be shadowed with `gpui::NoAction` in `TEXT_FIELD_KEY_CONTEXT`, or the outer
action must be removed while the field owns the keyboard.

#### `FuzzyList` / `FuzzyItem`
**Purpose.** A capped list of already-ranked results.
**API.** `FuzzyItem::new(primary).detail(..).secondary(..).trailing(..).leading(..)
.disabled(bool).key(..).matches(..).destructive(bool)`;
`FuzzyList::new(items).cursor(usize).cap(usize).under_text_field(bool).empty(..).row_height(Pixels)`;
`.binds_jk()`, `.shown()`, `FuzzyList::{next_cursor, prev_cursor}`.
**Variants.** one-line (no `secondary`) · two-line. An item with an empty description collapses
to one line — zero-suppression. `detail` is a muted qualifier drawn **on the same line**, after
the primary label: use it when the qualifier is part of the row's identity (§3.8.5's context
owners), and `secondary` only for a description that earns a second line.
**Caps.** 8 (Clone results) · 6 (Create base list) · 10 (palette).
**Keyboard.** `ctrl-n`/`ctrl-p` or `↓`/`↑` under a text field; `j`/`k` too when there is none.
**Usage rule.** Matching, ranking and the 150 ms debounce belong to the caller — they need the
domain's fields. Never render a list longer than its cap: a predictable `Enter` matters more
than completeness, and the footer says `9 of 63`.

#### `FilterBar`
**Purpose.** Narrow a list without moving it.
**API.** `FilterBar::new(query, shown, total).focused(bool).caret(usize).placeholder(..)`;
`.is_empty_result()`, `.count_tone()`.
**States.** typing (caret, `esc` hint) · exited but retained (rendered by
`PaneHeader::filter_chip`) · no match (`shown/total` turns amber).
**Keyboard.** printable · `Backspace` · `ctrl-w` · `ctrl-u` · `ctrl-n`/`↓` and `ctrl-p`/`↑` move
the **list** cursor while still typing · `Enter` opens the selected row · first `Esc` leaves the
input keeping the filter · second `Esc` clears it. `Esc` **never** quits the app.
**Usage rule.** Pass it into `PaneHeader::query_slot`, so it replaces the header in the same 30 px
row. A hidden active filter is the classic "where did my rows go" bug, so always show the
retained chip afterwards.

#### `Cycler`
**Purpose.** `◂ value ▸` for a two-to-five option set.
**API.** `Cycler::{new(value), labeled(label, value)}().has_prev(bool).has_next(bool).focused(bool)
.disabled(bool).label_width(Pixels).off_grid(bool)`; `.is_visible()`.
**Keyboard.** `←`/`→`.
**Usage rule.** A cycler, not a dropdown, when the set is short: a dropdown costs a second key.
Zero-suppress the whole control when the set has one member (the host cycler is hidden when no
hosts are configured).

#### `Toggle`
**Purpose.** `[x]` / `[ ]`.
**API.** `Toggle::{new(checked), labeled(label, checked)}().detail(..).focused(bool).disabled(bool)
.label_width(Pixels)`.
**Keyboard.** `Space`.
**Usage rule.** Brackets, not a switch: the whole Settings dialog is a keyboard list, and a
switch implies a pointer.

#### `NumberField`
**Purpose.** An integer with a unit suffix and a clamp.
**API.** `NumberField::{new(value), labeled(label, value)}().unit(..).range(min, max).min(i64)
.focused(bool).invalid(message).label_width(Pixels)`; `.clamp(i64)`, `.is_in_range()`,
`.range_message()`, `.message()`.
**States.** default · focused · invalid (out of range, red border).
**Usage rule.** The clamp is part of the contract: §3.8.6 states minimums, and an out-of-range
value must be refused at the field, not at save time.

#### `SegmentedTabs`
**Purpose.** Underlined tabs with counts.
**API.** `SegmentedTabs::new([SegmentedTab::new("mine", 7), SegmentedTab::bare("help")
.loading(bool)]).active(usize)`; `SegmentedTab::count_text()`, `SegmentedTabs::{len, is_empty,
next_index, prev_index}`.
**Keyboard.** `Tab`/`S-Tab`/`h`/`l`.
**Usage rule.** A tab is **not** a chip: `Some(0)` renders `0`, because an empty tab must still
say it is empty. `loading(true)` shows `…` while a refresh is in flight and keeps the cached
rows at full opacity.

#### `Select`
**Purpose.** A closed-list chooser that opens a `FuzzyList`.
**API.** `Select::new(value).label(..).placeholder(..).open(bool).focused(bool).options(..)
.disabled(bool).invalid(..).hint(..)`.
**Usage rule.** `Cycler` for 2–5 options, `Select` for a closed list, `FuzzyList` under a
`TextField` for a searchable set.

#### `ConfirmDialog`
**Purpose.** Show exactly what will be lost, in facts, with their age.
**Anatomy.** compact 480 px (title carries the target, one line of `✓` facts, stamp,
consequence) · expanded 560 px (`⚠` title tone, target on its own line, one line per fact,
stamp, consequence).
**API.** `ConfirmDialog::new(title, FactList).target(..).consequence(..).stamp(FreshnessStamp)
.icon(Icon).hints(KeyHintRow).action_label(..).width(Pixels).body(..).error(..)
.force_confirm_key(ConfirmKey)`; `.is_compact()`, `.confirm_key()`, `.resolved_width(&Theme)`.
**Keyboard.** `y`/`Y`/`Enter` confirm · `n`/`Esc`/`q` cancel · `I` re-check (delete) · `s` toggle
the KEEP list (prune). Nothing else is bound, so muscle memory cannot misfire.
**Usage rule.** No "don't ask again" checkbox, no second confirmation step, no countdown, no
typed-name confirmation, no disabled-button delay. The **compact form** is the real answer to
confirm fatigue. Users confirm the *consequence sentence*, so state it in plain future tense
and name the irreversibility.

#### `Palette`
**Purpose.** Jump to anything by name, or do the thing whose key you do not remember.
**Anatomy.** 640 px card at y = 120 · 44 px input · sections `GO` → `DO` → `CONTEXT` · ≤ 10 rows
of 34 px · footer `9 of 63 · ⏎ run · esc cancel`.
**API.** `Palette::new(query).section(PaletteSection::new(PaletteSectionKind::Go, rows))
.cursor(usize).caret(usize).cap(usize).total(usize).empty(..)`; `.shown()`, `.flat_len()`;
`PaletteSection::{len, is_empty}`;
`PaletteRow::new(label).icon(Icon).leading(..).detail(..).key(..).destructive(bool).matches(..)`.
**Usage rule.** `GO` (objects) always first — that is what makes a session reachable from inside
another session. Every `DO` row shows its bound key, right-aligned, so the palette trains itself
out of the loop. A command that is invalid here is **not listed**, never greyed. Destructive
commands are prefixed with `triangle-alert` and still routed through their confirm.

### 6.5 Jobs and terminal

#### `JobRow`
**Purpose.** glyph · kind (7 ch) · target · elapsed · percent, plus a progress sub-line.
**API.** `JobRow::new(JobStatus, kind, target).id(..).elapsed(..).percent(u8).progress(..)
.retryable(bool).trailing_key(..).selected(bool).cursor(bool)`;
`JobStatus::{Queued, Running, Cancelling, Cancelled, Done, Failed}` with `.icon()` and `.tone()`.
**Usage rule (`retryable`).** Renders §3.8.9's `(restartable)` / `(not restartable)` label in
the quit-and-stop confirm. Leave it **unset** when retryability is unknown: a job that claims
either is worse than one that says nothing. `JobStatus::Cancelling` mirrors
`fleet_proto::job::JobStatus::Cancelling`: cancellation was requested and shutdown is still in
progress, which is not the same row as `Cancelled`.
**Variants.** 30 px one-line (finished) · 44 px two-line (running, with the last stdout line).
**Usage rule.** `kind` is a fixed 7-character slug (`clone`, `pool`, `hooks`, `prune`, `create`,
`delete`, `fetch`, `prs`, `inspect`, `update`, `import`) so the column scans as a shape.
`target` is the real domain id — swarm's footer showed `hot-copy:<repo>`, which matched no row
anywhere in the app. Failed jobs are **never** auto-dismissed.

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
the cells are the two scroll overlays and the prefix hint (§3.6); `.modes(..)` feeds the
alt-screen suppression and paints nothing — the badges belong to the Workspace header.

#### `TerminalModes`
**Purpose.** Zero-suppressed badges for the VT modes a `FrameUpdate` reports.
**API.** `TerminalModes::new([TerminalMode]).glyphs_only()`; `.is_visible() .is_alt_screen()`;
`TerminalMode::{AltScreen, MouseReporting, BracketedPaste, ApplicationCursor}` with
`.label() .icon() .tone()`.
**Usage rule.** These modes are the only explanation for the keymap appearing to lie: in
alt-screen there is no scrollback (`ctrl-s [` refuses), with mouse reporting on the app owns
drag-select, and paste (`cmd-v` or `ctrl-s ]`) follows the program's bracketed-paste mode. A plain shell shows
no badge, so the row costs nothing in the common case. `alt` is the only amber one, because it
is the only one that changes what a documented key does. The row lives in the Workspace
**header**, in reserved chrome — never over the grid, whose cells are live output that a badge
would hide (zsh with `zle` sets bracketed paste and application cursor keys, so those two badges
are on in every plain shell).

#### `ScrollbackBadge`
**Purpose.** `↥ <offset>/<len>` in the grid's top-right corner.
**API.** `ScrollbackBadge::new(offset, len).alt_screen(bool)`; `.is_visible()`.
**Usage rule.** `ScrollPill` is the *mode* affordance and exists only while Scroll mode is
active; this badge is the *state* affordance. A viewport scrolled up with the wheel is not in
Scroll mode and would otherwise look exactly like a live one — which is how "my agent stopped
printing" bug reports are born. Zero-suppressed at `offset == 0` and in alt-screen.

#### `TerminalTabStrip`
**Purpose.** Numbered tabs, 84–200 px, with activity, keep-alive, agent-status and exit marks.
**API.** `TerminalTabStrip::new([TerminalTab::new(1, "nvim").activity(bool).starting(bool)
.keep_alive(Icon).agent_status(StatusKind).unread(bool).exited(impl Into<Option<i32>>)]).id(ElementId).active(usize).show_plus(bool)
.on_select(Fn(position, ..)).on_new(Fn(..))`.
**States.** active (accent underline + `ui_strong`) · inactive · activity (6 px amber dot) ·
unread (6 px neutral dot) · starting (per-tab `loader-circle`) · agent working (`loader-circle`) · agent finished
(`circle-check`) · exited (faint label + `circle-x` + code, or `—` when the process was killed
by a signal and has no code).
**Usage rule (`starting`).** §3.6's "Waking a slept session" rebuilds the strip and spawns one
PTY per tab; without a per-tab spinner the strip claims six live terminals that do not exist
yet. **Usage rule (`exited`).** `.exited(1)` and `.exited(None)` both compile: exit codes are
`Option<i32>` end-to-end (`fleet-core::TerminalStatus`, `Event::TerminalExited`), because a
`SIGKILL` from `^s x` produces none.
**Usage rule.** The index is the argument to `ctrl-s 1`–`9`, so the strip is the legend for that
binding. Agent activity appears only on terminals with a recognized agent.
**Usage rule (`unread` vs `activity`).** A tab draws **at most one** dot, and amber wins: amber
means the tab is waiting on the user, neutral only that content arrived while they were
elsewhere. A native agent tab uses both (`NATIVE-AGENTS.md` §3.3 maps `NeedsYou` to amber and
`Unread` to neutral); the active tab and an exited tab draw neither.

#### `ScrollPill`
**Purpose.** `SCROLL <offset>/<len>` while in scroll mode.
**API.** `ScrollPill::new(offset, len).selecting(bool).alt_screen(bool)`; `.is_visible()`.
**Usage rule.** **Suppressed in alt-screen** — when an alt-screen app is running, `ctrl-s [`
shows the 1.6 s toast `no scrollback in alt-screen` instead. Top-right inside the terminal area,
because during scroll the eyes are on content and the top right never covers the prompt.

#### `PrefixHint`
**Purpose.** The `^S` pill and its six keys.
**API.** `PrefixHint::new(visible).prefix(..).hints(KeyHintRow)`; `.is_visible()`. The hints
default to the six §3.6 prefix keys rather than an empty row.
**Usage rule.** The 400 ms delay is the caller's timer (`theme.motion.prefix_hint_delay`). The
expert types the second key in under 200 ms and never sees this; the returning user gets it
exactly when they hesitate — 0 px and 0 frames of permanent cost.

#### `ExitStrip`
**Purpose.** `⚠ process exited (<code>)` and the prefixed recovery keys.
**API.** `ExitStrip::new(impl Into<Option<i32>>).hints(KeyHintRow)`.
**Usage rule.** Defaults to `^s r restart · ^s x close · ^s c new`. Fleet must not silently
swallow a crashed dev server. `ExitStrip::new(None)` reads `process exited (killed)`: a
signal-killed process has no exit code and the strip must not invent `128 + signo`.

#### `ModeWord`
**Purpose.** The fixed 84 px word in the center of the status bar.
**API.** `ModeWord::{new(Mode), word(text)}().tone(Tone)`;
`Mode::{Normal, Terminal, Agent, Prefix, Scroll, Filter, Palette, Dialog, Jobs}` with
`.word() .tone() .keys_reach_pty()` and `Mode::ALL`.
**Usage rule.** Present on **every** screen, including the Workspace and including zoom. Only
`Prefix` is amber, because it is the one mode that expires on its own. `Agent` is a separate
word from `Terminal` because keys reach Fleet's own composer rather than a PTY: `keys_reach_pty()`
is false for it, so a view's hints keep their bare form.

#### `Banner`
**Purpose.** A 28 px full-width strip with a countdown and recovery keys.
**API.** `Banner::{warning, danger}(text).icon(Icon).countdown(..).hints(KeyHintRow)`.
**Usage rule.** After a daemon reconnect the banner must say, verbatim, *"fleetd restarted.
Terminal sessions did not survive; worktrees, jobs and state are intact."* A warm "reconnected"
banner that implies the agents came back is the single most damaging false reassurance in the
app.

#### `DaemonSplash`
**Purpose.** The two **full-window** daemon surfaces of §3.12, cases A and B.
**API.** `DaemonSplash::{starting, failed}(title).detail(..).log_lines(..).hints(KeyHintRow)`;
`DaemonSplashKind::{Starting, Failed}`.
**Usage rule.** `Banner` covers case C only, because that one is a 28 px strip under the
context bar. Cases A and B are chrome-less full-window surfaces and neither fits `EmptyState`,
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

Four of these are the exception to §6's "no component owns state": a transcript, a composer and
their key routing cannot be `RenderOnce`, because the list caches measured row heights and the
composer owns a caret, a selection and an IME session. They are gpui **entities** that emit
events and never act on a thread; the screen that owns them decides what an event means.
`components::agent::metrics` holds their six fixed dimensions (§2.8), and
`components::agent::format` holds their copy — `format_duration`, `format_token_count`,
`format_file_delta`, `format_files_changed`, `format_turn_footer`, `format_worked`,
`format_thinking`, `format_retrying`, `format_compacted`, `format_resumed`, and `MINUS`, the
U+2212 the design uses for a removed-line count. Keycaps are never inside those strings: they
are `KeyHint`s drawn by the row that owns them.

#### `TranscriptList`
**Purpose.** The bottom-anchored, variable-height conversation.
**API.** Entity. `TranscriptList::new(&mut Context<Self>)`, `set_rows(Vec<TranscriptRow>, cx)`,
`set_tool_body(ToolBodyRenderer, cx)`, `scroll_to_bottom(cx)`, `scroll_mode(bool, cx)`,
`set_streaming(bool, cx)`, `focus_row(Option<usize>, cx)`; the scroll-mode motions
`scroll_rows(f32, cx)`, `scroll_viewports(f32, cx)`, `scroll_to_top(cx)` and `scroll_to_end(cx)`;
readers `rows()`, `focused_row()`,
`is_scroll_mode()`, `is_at_bottom()`, `open_decision()`, `focus_handle()`,
`event_for_key(&str) -> Option<TranscriptEvent>`. Free helpers: `diff_rows` (the `RowSplice`
`set_rows` applies), `scroll_fraction`, `user_block`, `user_block_with`, `attachment_pill`,
`error_card`.
**Rows.** `TranscriptRow::{UserBlock{attachments}, AssistantText, Thinking, ToolRow{children},
WorkedFold, TurnFooter, DecisionCard, ErrorCard{retrying}, CheckpointLine, Notice,
QueuedMessage, EmptyState}`. `Notice` is a provider's own user-facing message — a config warning,
a deprecation — which is not an error and must not be drawn as one.
**States.** following the tail · scroll mode (tail frozen) · streaming (caret after the last
paragraph) · empty (`EmptyState`).
**Usage rule.** GPUI `list`, **not** `uniform_list` — an assistant paragraph, a 30 px tool row
and an inline diff are not one height. ADR 0005's uniform-row rule still governs the diff, which
stays uniform *inside* its row. `set_rows` splices only what `diff_rows` says changed, so a
streaming turn re-measures its last row and nothing else.
**Usage rule (key routing).** `event_for_key` resolves the open decision card **before** the
focused row, so a `y` can never toggle a row behind an unanswered permission.

#### `ToolRow`
**Purpose.** The 30 px row every tool call is drawn as.
**Anatomy.** state glyph · 60 px kind column (`AGENT_TOOL_KIND_W`) · one-line summary ·
right-aligned result. Children indent 16 px behind a 1 px left divider; an expanded body is
capped at `AGENT_BODY_MAX_H` and scrolls inside the row.
**API.** `ToolRow::new(id, kind, summary).state(ToolRowState).result(..).output(..).diff(..)
.expanded(bool)`; `.has_body()`; `tool_row(&ToolRow, &App)` for the plain element, or
`ToolRowElement::new(row).focused(bool).body(..).children(..).on_toggle(..)`; `expand_hint(bool)`.
**States.** `ToolRowState::{Running, Done, Error, Denied}` → `.glyph()` gives the icon and tone.
**Usage rule.** The geometry does not change while the row streams — a row that grows under the
reader is how a transcript starts to jitter. Only successful rows fold into `WorkedFold`; a
failed row stays exposed after the turn settles.

#### `DecisionCard`
**Purpose.** A permission, question or plan asked **inside** the thread.
**Anatomy.** `AGENT_CONTENT_W` wide, panel background, radius 6, a 2 px amber bar flush left,
always the last row.
**API.** `DecisionCard::new(id, title, DecisionCardKind).actions(Vec<DecisionOption>)`;
`.option_count(usize)`, `.action_for_key(&str) -> Option<DecisionAction>`;
`DecisionCardElement::new(card).default_action(..).selected(..).expanded(bool).answer(..)
.on_action(..)`; `permission_actions`, `question_actions`, `plan_actions`, `decision_key_hints`,
and `SOMETHING_ELSE` (the `Something else…` free-text option).
**Variants.** `Permission { tool, payload, rationale }` · `Question { questions }` ·
`Plan { markdown, steps }`.
**Usage rule.** The card owns the key *vocabulary*; the surface holding the focus handle owns
the key *event* and routes it here. That split is what stops one thread from answering another
thread's card. This type mirrors rather than imports `fleet_core::agents::GateKind`, so the kit
keeps its "no domain dependencies" rule.
**Usage rule (copy).** An action always spells out its effective scope — `allow once`, `allow
for this session`, `allow for this directory` — never the word "always".

#### `MultilineInput`
**Purpose.** The docked composer: `TextInput`'s wrapping, multi-line sibling.
**API.** Entity. `MultilineInput::new(&mut Context<Self>, placeholder)`, `text()`, `is_empty()`,
`set_text(.., cx)`, `clear(cx)`, `set_placeholder(.., cx)`, `set_focus_visible(bool, cx)`,
`submit(cx)`, `push_history(..)`, `focus_handle()`, readers `buffer()` and `history()`, and the
three `-> bool` motions an owner falls through on — `recall_previous(cx)`, `caret_up(cx)`,
`caret_down(cx)`, each answering whether it moved; emits
`MultilineInputEvent::{Submit(String), Trigger(char), Escape}` under
`MULTILINE_INPUT_KEY_CONTEXT`. `MultilineBuffer` is the pure editing model and `PromptHistory`
the last `HISTORY_LIMIT` (100) prompts plus the draft `↑` was opened from.
**States.** empty (placeholder) · typing · multi-line (grows one line at a time, to eight) ·
IME composition · dimmed while a decision card is open.
**Keyboard.** printable · `⏎` submit · `⇧⏎` newline · `Backspace`/`Delete` · word-wise deletion ·
line/word motion · shift-selection · select-all · paste · `↑` history · `/` and `@` emit
`Trigger`.
**Usage rule.** It never acts on a thread: a submit, a completion trigger and an escape are
reported, and the owner decides what they mean. Like `TextField`, an entity installing the
platform input handler calls `handle_edit_keystroke`, and bare-letter bindings above it must be
shadowed in its key context.

#### `Markdown`
**Purpose.** Assistant prose, rendered from a stream.
**API.** `parse_markdown(&str) -> MarkdownDocument`; `markdown(&MarkdownDocument, &App)`.
`MarkdownBlock::{Paragraph, Code{lang,text}, List{ordered,items}, Heading{level,inlines},
Quote, Rule}`, `MarkdownInline::{Text, Code, Strong, Emphasis, Link}`.
**Usage rule.** Two invariants, both tested: `parse_markdown` never panics or loops on any
input, and for any prefix `p` of `s`, every block of `parse_markdown(p)` except its last is a
block of `parse_markdown(s)` at the same index — a transcript must not reflow behind the reader
while the model keeps typing. Tables, images and indented code are out of scope and survive as
their own source text.

#### `DiffView` (`fleet_lazygit::diff_view`, not the kit)
**Purpose.** The inline diff under an `Edit` / `Write` tool row.
**API.** Entity. `DiffView::new(unified, cx)`, `DiffView::for_path(..)`, `unified()`,
`set_unified(.., cx)`, `expanded()`, `set_actions(..)`.
**Usage rule.** It takes unified-diff *text*, so an embedder needs no git plumbing and
`fleet-ui-kit` gains no `fleet-git` dependency — which is the only reason this one lives outside
the kit. Rows come from the ADR 0005 stack and `views::row_layout`, so an inline diff has the
same geometry as a full-window one; only the wash differs (`diff_added` / `diff_removed`).
Beyond `MAX_ROWS` (400) it folds with a "show all" affordance rather than flooding the thread.
The action row (`[u] revert this edit · [o] open in nvim`) is the embedder's, handed in through
`set_actions`.

---

## 7. What is deliberately not in the kit

- **Buttons.** There are none in Fleet. Affordances are `KeyHint`s.
- **Tooltips.** Every affordance already states its key.
- **A generic `Card`.** `Pane`, `Dialog`, `Sheet` and `Overlay` are the four surfaces; a fifth
  would erode the meaning of the other four.
- **Progress bars.** A phase word (`copying files…`, `hooks 2/3`) is more honest than a
  percentage, and `JobRow` shows a percent only when the job actually parses one.
- **A disabled visual style.** "You cannot edit this here" is said by the *absence* of an input
  box (a read-only `FactRow`), and an unavailable command is not listed at all.
- **Decorative color.** Four semantic colors and three neutrals. Nothing else.

## 8. Changing the system

1. A new token goes in `theme/tokens.rs` **and** in §2 of this document, in the same commit.
2. A new icon means downloading the SVG into `crates/fleet-ui-kit/assets/icons/`, rewriting its
   `stroke-width` to 1.5, adding the variant to the `lucide_icons!` list in `src/icons.rs`, and
   adding it to §5.1 here.
3. A new component gets its own module under `src/components/`, a `pub use` in
   `components/mod.rs`, an entry in §6 here, and a panel in the matching per-group gallery
   (`gallery_structure`, `gallery_data`, `gallery_input`, `gallery_terminal`, or
   `gallery_agent`) showing **every** state. `kit_gallery` remains the combined overview. If a
   state is not in a gallery, it is not implemented.
4. `cargo check -p fleet-ui-kit --examples` and
   `cargo run -p fleet-ui-kit --example kit_gallery` are the acceptance gate.
