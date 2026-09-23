//! Design tokens: the raw values every Fleet surface reads.
//!
//! Nothing in here knows about a component. Values are the ones documented in
//! `docs/DESIGN-SYSTEM.md`; that document and this file must change together.

use gpui::{FontWeight, Hsla, Pixels, px, rgb, rgba};

/// Opaque hex (`0xRRGGBB`) to [`Hsla`].
#[inline]
pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Hex with alpha (`0xRRGGBBAA`) to [`Hsla`].
#[inline]
pub fn ca(hex: u32) -> Hsla {
    rgba(hex).into()
}

/// The base unit of the whole system. Every spacing and size token is a multiple of it.
pub const BASE_UNIT: f32 = 4.0;

/// Width of one monospace cell at the data type size, in logical pixels (`1 ch`).
pub const CH: f32 = 7.5;

/// Convert a `ch` budget (the unit the UX spec measures column ladders in) to pixels.
#[inline]
pub fn ch(n: f32) -> Pixels {
    px(n * CH)
}

/// Semantic color roles. Exactly one instance per theme mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTokens {
    /// App ground. The largest area on screen.
    pub bg: Hsla,
    /// Window chrome: the title bar, the sidebar and the status bar. One step darker than
    /// [`Self::bg`], so the content area reads as the lit part of the window.
    pub chrome: Hsla,
    /// Rails, panes, lists — one step above the ground.
    pub surface: Hsla,
    /// Cards: a board tile, a hub card, a grouped settings block. One step above
    /// [`Self::surface`], always with a [`Self::border`] hairline.
    pub surface_raised: Hsla,
    /// Dialogs, sheets, toasts, palette — the floating layer.
    pub elevated: Hsla,
    /// Scrim painted over the base screen behind a dialog, sheet or popover. Flat and dark:
    /// gpui has no backdrop blur, so the scrim does the blur's job by darkening.
    pub overlay: Hsla,
    /// Cursor row background.
    pub row_selected: Hsla,
    /// Pointer hover on a row. Never used to express state.
    pub row_hover: Hsla,
    /// Primary text: branch, title, value.
    pub text: Hsla,
    /// Secondary text: repo, host, age, count, section label.
    pub text_secondary: Hsla,
    /// Muted text: draft, disabled, key hint, the null dash.
    pub text_muted: Hsla,
    /// Text drawn on top of an accent or semantic fill.
    pub text_inverse: Hsla,
    /// Cursor and focus. The only decorative-looking color, and it is never decorative.
    pub accent: Hsla,
    /// Fill of a surface's one primary button. The accent hue; never a second blue.
    pub accent_fill: Hsla,
    /// Pointer hover on [`Self::accent_fill`].
    pub accent_fill_hover: Hsla,
    /// [`Self::accent_fill`] while the pointer is pressed on it.
    pub accent_fill_active: Hsla,
    /// Label and key chip drawn on [`Self::accent_fill`].
    pub accent_fill_text: Hsla,
    /// Low-alpha accent wash behind a blue chip or an informational callout.
    pub accent_subtle: Hsla,
    /// Secondary button and other resting control fill.
    pub control: Hsla,
    /// Pointer hover on a control, a ghost button or an icon button.
    pub control_hover: Hsla,
    /// A control, a ghost button or an icon button while the pointer is pressed on it.
    pub control_active: Hsla,
    /// Hairline around a resting control.
    pub control_border: Hsla,
    /// Key chip (`Kbd`) fill.
    pub kbd_bg: Hsla,
    /// Key chip (`Kbd`) outline.
    pub kbd_border: Hsla,
    /// Healthy / done / approved.
    pub success: Hsla,
    /// Needs attention / in flight / unknown / degraded.
    pub warning: Hsla,
    /// Broken / destructive. Also the fill of a destructive button's strong form, with
    /// [`Self::text_inverse`] on it.
    pub danger: Hsla,
    /// Pointer hover on a [`Self::danger`] button fill.
    pub danger_fill_hover: Hsla,
    /// A [`Self::danger`] button fill while the pointer is pressed on it.
    pub danger_fill_active: Hsla,
    /// Neutral information. Shares the accent hue; never used for state.
    pub info: Hsla,
    /// 1 px hairlines.
    pub border: Hsla,
    /// Hairline that has to survive on top of `elevated`.
    pub border_strong: Hsla,
    /// 2 px inset ring on the focused pane.
    pub focus_ring: Hsla,
    /// 2 px left bar on the cursor row.
    pub cursor_bar: Hsla,
    /// Text selection fill (inputs and terminal).
    pub selection: Hsla,
    /// Scroll thumb on a pane edge.
    pub scroll_thumb: Hsla,
    /// Cold-load placeholder rows.
    pub skeleton: Hsla,
    /// Low-alpha green wash behind added diff lines.
    pub diff_added: Hsla,
    /// Low-alpha red wash behind removed diff lines.
    pub diff_removed: Hsla,
}

impl ColorTokens {
    /// The dark theme, which is the default.
    pub fn dark() -> Self {
        Self {
            bg: c(0x111317),
            chrome: c(0x0E0F12),
            surface: c(0x16181D),
            surface_raised: c(0x1A1C22),
            elevated: c(0x1B1E24),
            overlay: ca(0x0506088C),
            row_selected: c(0x1E2430),
            row_hover: c(0x191C22),
            text: c(0xE6E8EB),
            text_secondary: c(0xA3A9B5),
            text_muted: c(0x808794),
            text_inverse: c(0x0E1013),
            accent: c(0x58A6FF),
            accent_fill: c(0x58A6FF),
            accent_fill_hover: c(0x79B8FF),
            accent_fill_active: c(0x4493F8),
            accent_fill_text: c(0x0B0E14),
            accent_subtle: ca(0x58A6FF24),
            control: c(0x1A1D23),
            control_hover: c(0x22262D),
            control_active: c(0x2A2E36),
            control_border: c(0x2C3039),
            kbd_bg: c(0x23262D),
            kbd_border: c(0x30343D),
            success: c(0x3FB950),
            warning: c(0xD29922),
            danger: c(0xF85149),
            danger_fill_hover: c(0xFF6A63),
            danger_fill_active: c(0xEA4A42),
            info: c(0x58A6FF),
            border: c(0x22262E),
            border_strong: c(0x2C313A),
            focus_ring: c(0x58A6FF),
            cursor_bar: c(0x58A6FF),
            selection: ca(0x58A6FF47),
            scroll_thumb: c(0x2C313A),
            skeleton: c(0x1E222A),
            diff_added: ca(0x3FB95024),
            diff_removed: ca(0xF8514924),
        }
    }

    /// The light theme.
    pub fn light() -> Self {
        Self {
            bg: c(0xFBFBFC),
            chrome: c(0xF3F4F6),
            surface: c(0xFFFFFF),
            surface_raised: c(0xFFFFFF),
            elevated: c(0xFFFFFF),
            overlay: ca(0x0000004D),
            row_selected: c(0xEDF2FB),
            row_hover: c(0xF3F4F6),
            text: c(0x16181D),
            text_secondary: c(0x4B5563),
            text_muted: c(0x636A77),
            text_inverse: c(0xFBFBFC),
            accent: c(0x0969DA),
            accent_fill: c(0x0969DA),
            accent_fill_hover: c(0x0858C0),
            accent_fill_active: c(0x0A4A9E),
            accent_fill_text: c(0xFFFFFF),
            accent_subtle: ca(0x0969DA1F),
            control: c(0xFFFFFF),
            control_hover: c(0xF3F4F6),
            control_active: c(0xE8EAEE),
            control_border: c(0xD3D6DC),
            kbd_bg: c(0xF3F4F6),
            kbd_border: c(0xD3D6DC),
            success: c(0x1A7F37),
            warning: c(0x9A6700),
            danger: c(0xCF222E),
            danger_fill_hover: c(0xB31D28),
            danger_fill_active: c(0x9A1822),
            info: c(0x0969DA),
            border: c(0xE3E5E9),
            border_strong: c(0xD3D6DC),
            focus_ring: c(0x0969DA),
            cursor_bar: c(0x0969DA),
            selection: ca(0x0969DA33),
            scroll_thumb: c(0xD3D6DC),
            skeleton: c(0xEEF0F3),
            diff_added: ca(0x1A7F3724),
            diff_removed: ca(0xCF222E24),
        }
    }
}

/// Which font family a type role uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontRole {
    /// The system UI face.
    Ui,
    /// The monospace face used for data: branches, paths, shas, logs, terminal.
    Mono,
}

/// One entry of the type scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyle {
    /// Font size in logical pixels.
    pub size: Pixels,
    /// Fixed line height in logical pixels. Never relative.
    pub line_height: Pixels,
    /// Weight.
    pub weight: FontWeight,
    /// Which family this role resolves to.
    pub font: FontRole,
    /// Whether callers must uppercase the string before rendering.
    pub uppercase: bool,
    /// Letter spacing in `em`. Recorded for the spec; gpui 1.18.1 exposes no tracking setter,
    /// so `Text` cannot apply it yet.
    pub tracking: f32,
}

/// The type roles of the app.
///
/// `ui` is the body role (13 / 18); there is no separate `body` field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeScale {
    /// Screen heading: 20 / 26 semibold. One per page, above its content.
    pub page_title: TypeStyle,
    /// Heading of a group inside a page or a sheet: 16 / 22 semibold.
    pub section_title: TypeStyle,
    /// Body UI text: 13 / 18 regular. The design's `body` role.
    pub ui: TypeStyle,
    /// Emphasised UI text: 13 / 18 medium. Titles, active tabs, primary values.
    pub ui_strong: TypeStyle,
    /// Dialog and panel titles: 15 / 20 medium.
    pub title: TypeStyle,
    /// Data text: mono 12.5 / 18. Branch, path, sha, target, log.
    pub data: TypeStyle,
    /// Small data text: mono 11.5 / 16. Log tails, progress sub-lines.
    pub data_small: TypeStyle,
    /// Supporting text under a title or beside a value: 12 / 16 regular.
    pub caption: TypeStyle,
    /// Sentence-case small label: 11 / 14 semibold, no uppercase, no tracking. Field labels,
    /// sidebar and card group headings. Replaces [`Self::label`] screen by screen.
    pub sentence_label: TypeStyle,
    /// Legacy uppercase micro-header: 11 / 14 uppercase. Being retired from every screen in
    /// favour of [`Self::sentence_label`]; do not use it on a redesigned surface.
    pub label: TypeStyle,
    /// Key hints: mono 11 / 14.
    pub hint: TypeStyle,
}

impl Default for TypeScale {
    fn default() -> Self {
        Self {
            page_title: TypeStyle {
                size: px(20.0),
                line_height: px(26.0),
                weight: FontWeight::SEMIBOLD,
                font: FontRole::Ui,
                uppercase: false,
                tracking: 0.0,
            },
            section_title: TypeStyle {
                size: px(16.0),
                line_height: px(22.0),
                weight: FontWeight::SEMIBOLD,
                font: FontRole::Ui,
                uppercase: false,
                tracking: 0.0,
            },
            ui: TypeStyle {
                size: px(13.0),
                line_height: px(18.0),
                weight: FontWeight::NORMAL,
                font: FontRole::Ui,
                uppercase: false,
                tracking: 0.0,
            },
            ui_strong: TypeStyle {
                size: px(13.0),
                line_height: px(18.0),
                weight: FontWeight::MEDIUM,
                font: FontRole::Ui,
                uppercase: false,
                tracking: 0.0,
            },
            title: TypeStyle {
                size: px(15.0),
                line_height: px(20.0),
                weight: FontWeight::MEDIUM,
                font: FontRole::Ui,
                uppercase: false,
                tracking: 0.0,
            },
            data: TypeStyle {
                size: px(12.5),
                line_height: px(18.0),
                weight: FontWeight::NORMAL,
                font: FontRole::Mono,
                uppercase: false,
                tracking: 0.0,
            },
            data_small: TypeStyle {
                size: px(11.5),
                line_height: px(16.0),
                weight: FontWeight::NORMAL,
                font: FontRole::Mono,
                uppercase: false,
                tracking: 0.0,
            },
            caption: TypeStyle {
                size: px(12.0),
                line_height: px(16.0),
                weight: FontWeight::NORMAL,
                font: FontRole::Ui,
                uppercase: false,
                tracking: 0.0,
            },
            sentence_label: TypeStyle {
                size: px(11.0),
                line_height: px(14.0),
                weight: FontWeight::SEMIBOLD,
                font: FontRole::Ui,
                uppercase: false,
                tracking: 0.0,
            },
            label: TypeStyle {
                size: px(11.0),
                line_height: px(14.0),
                weight: FontWeight::MEDIUM,
                font: FontRole::Ui,
                uppercase: true,
                tracking: 0.06,
            },
            hint: TypeStyle {
                size: px(11.0),
                line_height: px(14.0),
                weight: FontWeight::NORMAL,
                font: FontRole::Mono,
                uppercase: false,
                tracking: 0.0,
            },
        }
    }
}

/// The 4 px spacing scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spacing {
    /// 2 px. Only inside a chip.
    pub xxs: Pixels,
    /// 4 px.
    pub xs: Pixels,
    /// 8 px.
    pub sm: Pixels,
    /// 12 px. The gap between list columns and the row padding.
    pub md: Pixels,
    /// 16 px. Pane padding.
    pub lg: Pixels,
    /// 24 px.
    pub xl: Pixels,
    /// 32 px.
    pub xxl: Pixels,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            xxs: px(2.0),
            xs: px(4.0),
            sm: px(8.0),
            md: px(12.0),
            lg: px(16.0),
            xl: px(24.0),
            xxl: px(32.0),
        }
    }
}

/// Corner radii.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radii {
    /// 0 px. Rows, panes, the terminal grid.
    pub none: Pixels,
    /// 3 px. Inline marks.
    pub xs: Pixels,
    /// 4 px. Badges, inputs, list items inside a dialog.
    pub sm: Pixels,
    /// 6 px. Toasts, sheets, scroll pill.
    pub md: Pixels,
    /// 12 px. Dialog and palette cards.
    pub lg: Pixels,
    /// Pill. Context-bar chips and status dots.
    pub full: Pixels,
    /// 7 px. Buttons, inputs, nav rows, segmented controls.
    pub control: Pixels,
    /// 10 px. Cards ([`ColorTokens::surface_raised`]).
    pub card: Pixels,
    /// 12 px. Menus, popovers, tooltips.
    pub popover: Pixels,
    /// 12 px. Dialogs, sheets that float, the palette.
    pub dialog: Pixels,
    /// 9999 px. Chips, status pills, avatars.
    pub pill: Pixels,
}

impl Default for Radii {
    fn default() -> Self {
        Self {
            none: px(0.0),
            xs: px(3.0),
            sm: px(4.0),
            md: px(6.0),
            lg: px(12.0),
            full: px(9999.0),
            control: px(7.0),
            card: px(10.0),
            popover: px(12.0),
            dialog: px(12.0),
            pill: px(9999.0),
        }
    }
}

/// One shadow recipe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowToken {
    /// Vertical offset.
    pub y: Pixels,
    /// Blur radius.
    pub blur: Pixels,
    /// Spread radius.
    pub spread: Pixels,
    /// Shadow color, alpha included.
    pub color: Hsla,
}

/// The elevation levels. Level 0 and 1 are flat and carry a hairline instead of a shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Elevation {
    /// Level 2: right-docked sheet and toasts.
    pub sheet: ShadowToken,
    /// Level 3: dialogs and the palette.
    pub dialog: ShadowToken,
    /// Level 4: menus, popovers and tooltips, which float over everything including a dialog.
    pub popover: ShadowToken,
}

impl Elevation {
    /// Dark-mode shadows.
    pub fn dark() -> Self {
        Self {
            sheet: ShadowToken {
                y: px(8.0),
                blur: px(24.0),
                spread: px(0.0),
                color: ca(0x00000059),
            },
            dialog: ShadowToken {
                y: px(16.0),
                blur: px(48.0),
                spread: px(0.0),
                color: ca(0x00000073),
            },
            popover: ShadowToken {
                y: px(24.0),
                blur: px(64.0),
                spread: px(0.0),
                color: ca(0x0000008C),
            },
        }
    }

    /// Light-mode shadows: same geometry, lower alpha.
    pub fn light() -> Self {
        Self {
            sheet: ShadowToken {
                y: px(8.0),
                blur: px(24.0),
                spread: px(0.0),
                color: ca(0x0000001F),
            },
            dialog: ShadowToken {
                y: px(16.0),
                blur: px(48.0),
                spread: px(0.0),
                color: ca(0x00000029),
            },
            popover: ShadowToken {
                y: px(24.0),
                blur: px(64.0),
                spread: px(0.0),
                color: ca(0x00000033),
            },
        }
    }
}

/// Motion. Fleet animates the spinner and smooths native-agent prose reveal; the rest of these
/// are dwell and delay durations, not animations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    /// 400 ms: how long the held `^S` prefix waits before its hint (the prefix menu) appears.
    pub prefix_hint_delay: u64,
    /// 500 ms: how long the pointer rests on a control before its tooltip appears.
    pub tooltip_delay: u64,
    /// 1000 ms: one full turn of the spinner.
    pub spinner: u64,
    /// 150 ms: how long the jump-to-latest chip waits before appearing.
    pub jump_chip_delay: u64,
    /// 1000 ms: how often the working row's elapsed label re-reads the clock.
    pub working_tick: u64,
    /// 16 ms: cadence of native-agent text reveal while motion is enabled.
    pub reveal_tick_ms: u64,
    /// 200 ms: maximum horizon over which one native-agent text burst is revealed.
    pub reveal_horizon_ms: u64,
    /// 1600 ms: short toast dwell.
    pub toast_short: u64,
    /// 3200 ms: normal toast dwell.
    pub toast_normal: u64,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            prefix_hint_delay: 400,
            tooltip_delay: 500,
            spinner: 1000,
            jump_chip_delay: 150,
            working_tick: 1000,
            reveal_tick_ms: 16,
            reveal_horizon_ms: 200,
            toast_short: 1600,
            toast_normal: 3200,
        }
    }
}

/// Fixed pixel geometry that the UX spec pins down. Components must not hard-code these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Default expanded dialog width.
    pub dialog_w: Pixels,
    /// Compact confirmation width.
    pub confirm_compact_w: Pixels,
    /// Minimum terminal tab width.
    pub terminal_tab_min_w: Pixels,
    /// Maximum terminal tab width.
    pub terminal_tab_max_w: Pixels,
    /// New-terminal control width.
    pub new_terminal_tab_w: Pixels,
    /// Scroll-mode pill width.
    pub scroll_pill_w: Pixels,
    /// macOS titlebar inset.
    pub traffic_light_inset: Pixels,
    /// Toast inset from the overlay edges.
    pub toast_inset: Pixels,
    /// Diff row height.
    pub diff_row_h: Pixels,
    /// Diff position scrollbar width.
    pub diff_scrollbar_w: Pixels,
    /// Minimum diff scrollbar thumb height.
    pub diff_thumb_min_h: Pixels,
    /// Horizontal diff keyboard step (four monospace columns).
    pub diff_horizontal_step: Pixels,
    /// Compact detail overlay width.
    pub detail_overlay_w: Pixels,
    /// 640 px two-column help overlay.
    pub overlay_help_w: Pixels,
    /// 160 px multi-line editor box: a commit subject plus a short body.
    pub editor_box_h: Pixels,
    /// 14 px caret bar inside a [`Self::diff_row_h`] editor line, inset top and bottom.
    pub diff_caret_h: Pixels,
    /// 1 px separator hairline.
    pub hairline: Pixels,
    /// 8 px daemon status dot.
    pub dot_size: Pixels,
    /// 6 px terminal activity dot.
    pub dot_size_small: Pixels,
    /// 44 px title bar: context switcher, section nav, command field, status buttons. It is the
    /// unified macOS titlebar, so it starts after [`Self::traffic_light_inset`].
    pub title_bar_h: Pixels,
    /// 28 px status bar.
    pub status_bar_h: Pixels,
    /// 30 px pane header.
    pub pane_header_h: Pixels,
    /// 30 px dense list row: menus, pickers, the terminal-side lists.
    pub row_h: Pixels,
    /// 44 px comfortable row: the hub lists (worktrees, pull requests, agents).
    pub row_h_comfortable: Pixels,
    /// 30 px button.
    pub button_h: Pixels,
    /// 26 px compact button: inside a row, a header or a toolbar.
    pub button_h_compact: Pixels,
    /// 24 px segment of a `SegmentedControl`. With the control's `xxs` inset and hairline on
    /// each side the whole control is exactly `row_h`, so it sits in a settings row or a pane
    /// header without growing it.
    pub segment_h: Pixels,
    /// 34 px `Switch` track width.
    pub switch_w: Pixels,
    /// 20 px `Switch` track height. The knob is this less an `xxs` inset on each side.
    pub switch_h: Pixels,
    /// 16 px `Checkbox` box.
    pub checkbox_size: Pixels,
    /// 34 px icon tile that leads an alert dialog's title (a confirm).
    pub alert_tile: Pixels,
    /// 18 px key chip.
    pub kbd_h: Pixels,
    /// 16 px key chip inside a compact button or a menu item.
    pub kbd_h_small: Pixels,
    /// 62 px Git status pane: one row under its header.
    pub status_pane_h: Pixels,
    /// 92 px unfocused Git stash pane.
    pub stash_pane_h: Pixels,
    /// 34 px palette row.
    pub palette_row_h: Pixels,
    /// 22 px icon tile that leads a palette row.
    pub palette_tile: Pixels,
    /// 44 px two-line job row.
    pub job_row_h: Pixels,
    /// 4 px track of a running job's progress bar.
    pub progress_bar_h: Pixels,
    /// 20 px section header.
    pub section_header_h: Pixels,
    /// 44 px dialog header.
    pub dialog_header_h: Pixels,
    /// 44 px dialog footer.
    pub dialog_footer_h: Pixels,
    /// 28 px banner.
    pub banner_h: Pixels,
    /// 22 px terminal exit strip and context-bar chip height.
    pub strip_h: Pixels,
    /// 22 px chip pill.
    pub chip_h: Pixels,
    /// 240 px repos rail (drag range 200-320).
    pub rail_w: Pixels,
    /// 232 px sidebar (repos and agents).
    pub sidebar_w: Pixels,
    /// 344 px detail panel.
    pub detail_w: Pixels,
    /// 736 px right-side sheet showing a full detail (a board card).
    pub sheet_w_detail: Pixels,
    /// 440 px docked sheet.
    pub sheet_w: Pixels,
    /// 640 px docked sheet with a log expanded.
    pub sheet_expanded_w: Pixels,
    /// 320 px toast.
    pub toast_w: Pixels,
    /// 240 px: the narrowest a [`crate::Menu`] draws. It grows to fit its widest item.
    pub menu_min_w: Pixels,
    /// 640 px palette.
    pub palette_w: Pixels,
    /// 900 px widest ⌃S command menu: five columns of commands, centred over the status bar.
    pub prefix_menu_w: Pixels,
    /// y = 120 px: where the palette is anchored.
    pub palette_top: Pixels,
    /// 84 px fixed-width mode word of the embedded Git UI's status bar. Fleet's own chrome draws
    /// no mode word (ADR 0023).
    pub mode_word_w: Pixels,
    /// 340 px command field centred in the title bar: the button that opens the palette.
    pub command_field_w: Pixels,
    /// 18 px monogram tile: the letter of a context in the title bar's context switcher.
    pub monogram_size: Pixels,
    /// 3 px scroll thumb.
    pub scroll_thumb_w: Pixels,
    /// 2 px focus ring / cursor bar.
    pub focus_ring_w: Pixels,
    /// 7.5 x 18 px terminal cell at the data type size.
    pub cell_w: Pixels,
    /// Terminal cell height.
    pub cell_h: Pixels,
    /// 104 px fact-label column, wide enough for the longest §3.4 label (`unique commits`).
    pub fact_label_w: Pixels,
    /// 120 px doctor check column.
    pub doctor_check_w: Pixels,
    /// 64 px doctor status column.
    pub doctor_status_w: Pixels,
    /// 36 px text-input box.
    pub text_field_h: Pixels,
    /// 18 px validation/preview slot beneath a field.
    pub field_status_h: Pixels,
    /// 44 px command-palette input.
    pub palette_input_h: Pixels,
    /// 96 px minimum number-field value box.
    pub number_field_w: Pixels,
    /// Terminal-unavailable scrim opacity.
    pub veil_opacity: f32,
    /// Disabled and dimmed row opacity.
    pub dimmed_opacity: f32,
    /// Cached value opacity during refresh.
    pub refreshing_opacity: f32,
    /// Derived-mark opacity for stale facts.
    pub stale_opacity: f32,
    /// Blinking terminal text opacity without a repaint timer.
    pub terminal_blink_opacity: f32,
    /// Semantic banner border opacity.
    pub banner_border_opacity: f32,
    /// Hover fill on sticky errors.
    pub error_hover_opacity: f32,
    /// Neutral chip and badge fill opacity.
    pub neutral_fill_opacity: f32,
    /// Semantic chip and badge fill opacity.
    pub semantic_fill_opacity: f32,
    /// Cold-load skeleton opacity.
    pub skeleton_opacity: f32,
    /// No-session status glyph opacity.
    pub no_session_opacity: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            dialog_w: px(560.0),
            confirm_compact_w: px(480.0),
            terminal_tab_min_w: px(84.0),
            terminal_tab_max_w: px(200.0),
            new_terminal_tab_w: px(36.0),
            scroll_pill_w: px(176.0),
            traffic_light_inset: px(84.0),
            toast_inset: px(12.0),
            diff_row_h: px(18.0),
            diff_scrollbar_w: px(5.0),
            diff_thumb_min_h: px(24.0),
            diff_horizontal_step: ch(4.0),
            detail_overlay_w: px(320.0),
            overlay_help_w: px(640.0),
            editor_box_h: px(160.0),
            diff_caret_h: px(14.0),
            hairline: px(1.0),
            dot_size: px(8.0),
            dot_size_small: px(6.0),
            title_bar_h: px(44.0),
            status_bar_h: px(28.0),
            pane_header_h: px(30.0),
            row_h: px(30.0),
            row_h_comfortable: px(44.0),
            button_h: px(30.0),
            button_h_compact: px(26.0),
            segment_h: px(24.0),
            switch_w: px(34.0),
            switch_h: px(20.0),
            checkbox_size: px(16.0),
            alert_tile: px(34.0),
            kbd_h: px(18.0),
            kbd_h_small: px(16.0),
            status_pane_h: px(62.0),
            stash_pane_h: px(92.0),
            palette_row_h: px(34.0),
            palette_tile: px(22.0),
            job_row_h: px(44.0),
            progress_bar_h: px(4.0),
            section_header_h: px(20.0),
            dialog_header_h: px(44.0),
            dialog_footer_h: px(44.0),
            banner_h: px(28.0),
            strip_h: px(22.0),
            chip_h: px(22.0),
            rail_w: px(240.0),
            sidebar_w: px(232.0),
            detail_w: px(344.0),
            sheet_w_detail: px(736.0),
            sheet_w: px(440.0),
            sheet_expanded_w: px(640.0),
            toast_w: px(320.0),
            menu_min_w: px(240.0),
            palette_w: px(640.0),
            prefix_menu_w: px(900.0),
            palette_top: px(120.0),
            mode_word_w: px(84.0),
            command_field_w: px(340.0),
            monogram_size: px(18.0),
            scroll_thumb_w: px(3.0),
            focus_ring_w: px(2.0),
            cell_w: px(CH),
            cell_h: px(18.0),
            fact_label_w: px(104.0),
            doctor_check_w: px(120.0),
            doctor_status_w: px(64.0),
            text_field_h: px(36.0),
            field_status_h: px(18.0),
            palette_input_h: px(44.0),
            number_field_w: px(96.0),
            veil_opacity: 0.55,
            dimmed_opacity: 0.40,
            refreshing_opacity: 0.60,
            stale_opacity: 0.55,
            terminal_blink_opacity: 0.70,
            banner_border_opacity: 0.35,
            error_hover_opacity: 0.22,
            neutral_fill_opacity: 0.08,
            semantic_fill_opacity: 0.14,
            skeleton_opacity: 0.30,
            no_session_opacity: 0.30,
        }
    }
}

#[cfg(test)]
mod tests;
