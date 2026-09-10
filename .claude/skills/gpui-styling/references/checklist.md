# gpui-styling — reviewer checklist

Standalone: apply this to any fleetd diff that touches styling, theming, layout, typography,
icons or motion. Every question is yes/no; "no" means the fix in the right-hand sentence.
Authority for this area is `docs/DESIGN-SYSTEM.md` (`docs/README.md:9`).

Mechanical pre-pass — run these first, they catch most defects in seconds:

```sh
# colour literals outside the token module (expected: 0)
rg 'rgb\(0x|rgba\(0x|hsla\(' crates --glob '!**/theme/**' --glob '!**/target/**'

# pixel literals in app/view code (expect only named consts and test setup)
rg 'px\([0-9]' crates/fleet-app/src crates/fleet-lazygit/src

# Zed idioms that must not appear in fleetd
rg 'rems\(|rems_from_px|set_rem_size|DynamicSpacing|group_hover|visible_on_hover' crates

# typography escaping its role module
rg '\.text_size\(|\.font_family\(|\.line_height\(' crates --glob '!**/text.rs' --glob '!**/app_frame.rs'
```

---

## Tokens and colour

- [ ] **No colour literal outside `crates/fleet-ui-kit/src/theme/`.** A hex in a component
      detaches from light/dark and from every future theme. fleetd is currently at zero — keep it.
      Fix: add or reuse a `ColorTokens` role, reach it through `Tone`.
- [ ] **Colour chosen via `Tone`, or a named `theme.colors.*` role — never a nearby role "because
      it looks right".** A borrowed role breaks the moment the palette moves. Fix: name the intent
      (`Tone::Warning`), not the appearance.
- [ ] **A near-variant is derived with `.opacity(theme.metrics.<named>_opacity)`, not a new hex
      and not a bare float.** A raw `.opacity(0.4)` is an unnamed token. Fix: use an existing
      `Metrics` opacity or add a named one (`theme/tokens.rs:521-543`).
- [ ] **A genuinely new token landed in `theme/tokens.rs` *and* §2 of `docs/DESIGN-SYSTEM.md` in
      the same commit.** The two halves are one contract (`theme/tokens.rs:3-4`). Fix: add the doc
      row before merging.
- [ ] **Both `ColorTokens::dark()` and `ColorTokens::light()` were updated, and the gallery was
      viewed in both** (`t` toggles). A one-mode token is a guaranteed light-mode bug.
      Fix: `cargo run -p fleet-ui-kit --example kit_gallery`.
- [ ] **No new token without a consumer.** A `Metrics`/`Motion` field nothing reads is a promise
      the app never keeps; §2.8 is an inventory, not a wish list. Fix: wire it or drop it.

## Size and spacing

- [ ] **No `px(<literal>)` in a component or a view.** Spacing comes from `theme.space.*`,
      geometry from `theme.metrics.*`, radii from `theme.radii.*`, column widths from `ch()`.
      Fix: use the token; if none fits, add a `Metrics` field.
- [ ] **New fixed geometry became a `Metrics` field with a doc comment naming its UX-spec
      clause** — or, if it must live in `fleet-app`, a named `const` with the same doc comment
      (the accepted shape is `crates/fleet-app/src/dialogs/mod.rs:40-47`). Every dialog view now
      takes both axes from that ladder, and `dialog_views_take_their_geometry_from_named_consts`
      (`dialogs/mod.rs`) fails the build if a new inline `px()` height appears.
- [ ] **No `rems(`, `rems_from_px`, `set_rem_size`, `WithRemSize` or `DynamicSpacing` imported
      from Zed.** fleetd has no rem basis and no density setting; a mixed system scales half the
      UI. Fix: use the pixel token scales.
- [ ] **Borders use `theme.metrics.hairline`, shadows use `theme.dialog_shadow()` /
      `theme.sheet_shadow()`.** Hand-built `BoxShadow`s bypass the two-mode `Elevation` recipes
      (`theme/tokens.rs:334-339`, `theme/theme.rs:202-220`).

## Layout

- [ ] **Every shrinkable flex child carries `min_w_0()` (usually with `flex_1()`).** Without it a
      flex item's `min-width: auto` prevents shrinking, so nothing ever truncates and the row
      overflows. Reference: `crates/fleet-ui-kit/src/components/row.rs:279-292`.
- [ ] **Every fixed slot carries `flex_none()` / `flex_shrink_0()`.** Otherwise a glyph column or
      an age stamp shrinks before the text does.
- [ ] **Text that can overflow is budgeted** — `Text::truncate_at(n, mode)` when the spec names a
      `ch` number, `Text::ellipsize()` for a flex column with no budget. Nothing relies on the
      parent clipping.
- [ ] **`overflow_hidden()` on any container that must not push the window wider.**
- [ ] **Layering uses child order, `absolute()` inside `relative()`, or
      `deferred().with_priority(n)`.** GPUI has no z-index. Fix: reorder or defer.
- [ ] **An overlay that must swallow clicks calls `.occlude()`.** Otherwise the scrim leaks
      pointer events to the screen behind it (`components/dialog.rs:203,218`,
      `components/overlay.rs:127,142`).

## Typography

- [ ] **No `.text_size` / `.font_family` / `.line_height` outside `text.rs` and the root frame.**
      Views pick a `TextRole`; that is what keeps a branch name identical across panes
      (`crates/fleet-ui-kit/src/text.rs:3-5`). Fix: `Text::data(..)` etc., or
      `text::styled_with(el, style, theme)` for a container.
- [ ] **`TextRole::Label` content is *not* pre-uppercased by the caller.** `Text` applies the
      role's casing itself, before the `ch` budget (`text.rs:199-214`). Pre-uppercasing double-
      applies and can break the budget test.
- [ ] **A new type role, if any, was added to `TypeScale` and to §2.3 of the design doc together.**

## States and motion

- [ ] **Hover is pointer feedback only.** It never encodes state and never paints over
      `row_selected`; the gate is `hoverable && !disabled && !selected` (`components/row.rs:255-257`).
- [ ] **No `.hover(` / `.cursor_pointer()` added in `fleet-app`.** `docs/DESIGN-SYSTEM.md:9-12`
      forbids ad-hoc styling there; the two existing sites
      (`crates/fleet-app/src/views/watch_pane.rs:82,123`) are debt, not precedent.
      Fix: move the affordance into a kit component.
- [ ] **Focus and cursor affordances go through `FocusRing`.** No component draws
      `colors.accent` as its own border (`crates/fleet-ui-kit/src/focus.rs:1-5`,
      `docs/DESIGN-SYSTEM.md:230-242`).
- [ ] **No disabled *colour* style was invented.** Fleet has none by design
      (`docs/DESIGN-SYSTEM.md:1238-1240`): unavailability is expressed by absence, or by
      `Row::disabled(true)` lowering opacity.
- [ ] **Any animation uses `with_animation`** — that is what makes `App::reduce_motion` work
      (`zed/crates/gpui/src/elements/animation.rs:76-81`). A hand-rolled frame timer is an
      accessibility regression.
- [ ] **Its duration comes from `theme.motion.*`, not a literal `Duration`.**
- [ ] **Repeated animated items pass an explicit `ElementId`** (`Icon::…el().spinning(true).id(..)`,
      `crates/fleet-ui-kit/src/icons.rs:242-246`). A call-site-derived id makes every row share
      one animation state.

## Icons

- [ ] **A new glyph is an SVG in `crates/fleet-ui-kit/assets/icons/` with `stroke-width` 1.5, a
      variant in `lucide_icons!`, and an entry in §5.1 of `docs/DESIGN-SYSTEM.md`** — one commit
      (`docs/DESIGN-SYSTEM.md:1245-1247`). The set is closed on purpose.
- [ ] **`svg()` keeps `.flex_none()`, is explicitly `.size(..)`, and is coloured with
      `text_color`, never `bg`** (`crates/fleet-ui-kit/src/icons.rs:256-269`). Without
      `flex_none()` the glyph stretches in a flex row.

## Surfaces

- [ ] **A new surface is composed from `Pane` / `Dialog` / `Sheet` / `Overlay`, not hand-assembled.**
      There are four surfaces and deliberately no fifth (`docs/DESIGN-SYSTEM.md:1233-1235`).
- [ ] **If the bg + radius + hairline + shadow recipe was retyped, that is a signal**, not a
      finding to wave through: it already exists at `components/dialog.rs:212-216`,
      `components/overlay.rs:136-140`, `components/toast_stack.rs:250-254`,
      `components/sheet.rs:98-101`. Prefer extracting a helper over adding a fifth copy.

## Contract

- [ ] **`fleet-app` gained no styling.** The change landed as a kit component or a kit token
      (`docs/DESIGN-SYSTEM.md:9-12`).
- [ ] **Every new component state has a panel in the matching gallery.** "If a state is not in a
      gallery, it is not implemented" (`docs/DESIGN-SYSTEM.md:1252`).
- [ ] **`cargo check -p fleet-ui-kit --examples` passes and `kit_gallery` was actually run** —
      the stated acceptance gate (`docs/DESIGN-SYSTEM.md:1253-1254`) — followed by `make lint`
      and `make test`.
- [ ] **Code and doc moved in the same commit**, message shaped `ui-kit: <imperative lowercase
      summary>` (or `app:` / `lazygit:`).

---

## Known-good baselines (do not flag these as defects)

- Pixels rather than rems, everywhere. Deliberate: Fleet exposes no UI-scale setting.
- Fixed pixel `line_height` rather than a ratio (`theme/tokens.rs:158-163`).
- `ch`-budget truncation instead of `Styled::truncate()` (`crates/fleet-ui-kit/src/truncate.rs:3-6`).
- `TypeStyle::tracking` recorded but unapplied — gpui v1.18.1 exposes no letter-spacing setter
  (`docs/DESIGN-SYSTEM.md:157-159`).
- No `h_flex()`/`v_flex()` helpers yet; spelled-out flex triplets are the current norm.
- The five gated `.hover(` sites inside `fleet-ui-kit` (`row.rs`, `card_tile.rs`, `terminal_tab_strip.rs` ×2, `sticky_error_slot.rs`); Fleet is keyboard-first, so hover stays rare and gated.
- Dark-only default with no system-appearance follow.
