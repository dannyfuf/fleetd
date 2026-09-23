//! The visual test bench for `fleet-ui-kit`.
//!
//! Every component, in every state, in both themes. `t` toggles light/dark, `q` / `cmd-q`
//! quits. This example is the acceptance test for a component change: if a state is not
//! visible here, it is not implemented.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example kit_gallery
//! ```

pub mod support;
const LAYOUT: support::layout::GalleryLayout = support::layout::GalleryLayout {
    label_width: 150.0,
    column: false,
    divided: false,
    compact: false,
};
use fleet_ui_kit::prelude::*;
use fleet_ui_kit::theme::{CONTRAST_AA, contrast_ratio};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, KeyBinding, SharedString,
    UniformListScrollHandle, WeakEntity, Window, actions, div, px,
};

actions!(kit_gallery, [ToggleTheme, Quit, ConfirmYes, ConfirmStrong]);

struct Gallery {
    focus_handle: FocusHandle,
    list_scroll: UniformListScrollHandle,
    cursor: usize,
    /// The filter bar and the palette both edit a live input, so the gallery owns one each.
    filter_query: Entity<TextInput>,
    palette_query: Entity<TextInput>,
    /// Every other input the gallery shows is live too: `(label, entity)` in display order.
    fields: Vec<(&'static str, Entity<TextInput>)>,
    areas: Vec<Entity<TextInput>>,
    dialog_branch: Entity<TextInput>,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            list_scroll: UniformListScrollHandle::new(),
            cursor: 1,
            filter_query: demo_query(cx, "rut"),
            palette_query: demo_query(cx, "pay fix"),
            fields: demo_fields(cx),
            areas: demo_areas(cx),
            dialog_branch: demo_branch(cx, "feat/rut-validator"),
        }
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }
}

impl Focusable for Gallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

use support::layout::strip;

/// A gallery-only key chip for a key the gallery has no live keymap for.
fn gallery_kbd(keys: &str) -> Kbd {
    Kbd::parse(keys).unwrap_or_else(|error| panic!("{keys:?}: {error}"))
}

fn box_of(t: &Theme, height: gpui::Pixels, child: impl IntoElement) -> AnyElement {
    let theme = t;
    div()
        .relative()
        .w_full()
        .h(height)
        .rounded(theme.radii.sm)
        .bg(theme.colors.surface)
        .border(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

/// One colour role: a chip of the colour over its label, drawn in `palette`'s own colours so
/// the dark and the light column can sit side by side whatever the active mode is.
fn role_swatch(palette: &Theme, name: &'static str, color: gpui::Hsla) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(palette.space.xxs)
        .w(px(120.0))
        .child(
            div()
                .h(px(28.0))
                .w_full()
                .rounded(palette.radii.control)
                .bg(color)
                .border(palette.metrics.hairline)
                .border_color(palette.colors.border),
        )
        .child(Text::hint(name).color(palette.colors.text_secondary))
        .into_any_element()
}

/// A text role drawn on a ground, with its WCAG contrast ratio.
fn contrast_swatch(palette: &Theme, name: &'static str, text: gpui::Hsla) -> AnyElement {
    let ratios: Vec<AnyElement> = [
        ("chrome", palette.colors.chrome),
        ("bg", palette.colors.bg),
        ("surface", palette.colors.surface),
        ("raised", palette.colors.surface_raised),
        ("elevated", palette.colors.elevated),
    ]
    .into_iter()
    .map(|(ground_name, ground)| {
        let ratio = contrast_ratio(text, ground);
        div()
            .flex()
            .flex_col()
            .px(palette.space.sm)
            .py(palette.space.xs)
            .rounded(palette.radii.control)
            .bg(ground)
            .child(Text::ui(name).color(text))
            .child(Text::hint(format!("{ground_name} {ratio:.1}:1")).color(
                if ratio >= CONTRAST_AA {
                    palette.colors.text_secondary
                } else {
                    palette.colors.danger
                },
            ))
            .into_any_element()
    })
    .collect();
    strip(palette, ratios)
}

/// A key chip and a pair of buttons built from raw tokens, the way the kit's `Kbd` and `Button`
/// will read them.
fn control_samples(palette: &Theme) -> AnyElement {
    let c = &palette.colors;
    let m = &palette.metrics;
    let kbd = |label: &'static str, height: gpui::Pixels, bg, border, fg| {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(height)
            .min_w(height)
            .px(palette.space.xs)
            .rounded(palette.radii.sm)
            .bg(bg)
            .border(m.hairline)
            .border_color(border)
            .child(Text::hint(label).color(fg))
            .into_any_element()
    };
    let button = |label: &'static str, height: gpui::Pixels, bg, border, fg, chip: AnyElement| {
        div()
            .flex()
            .items_center()
            .gap(palette.space.sm)
            .h(height)
            .px(palette.space.md)
            .rounded(palette.radii.control)
            .bg(bg)
            .border(m.hairline)
            .border_color(border)
            .child(Text::ui_strong(label).color(fg))
            .child(chip)
            .into_any_element()
    };
    strip(
        palette,
        vec![
            button(
                "Create worktree",
                m.button_h,
                c.accent_fill,
                c.accent_fill,
                c.accent_fill_text,
                kbd(
                    "⏎",
                    m.kbd_h,
                    c.accent_fill_text
                        .opacity(palette.metrics.semantic_fill_opacity),
                    c.accent_fill_text
                        .opacity(palette.metrics.semantic_fill_opacity),
                    c.accent_fill_text,
                ),
            ),
            button(
                "hover",
                m.button_h,
                c.accent_fill_hover,
                c.accent_fill_hover,
                c.accent_fill_text,
                div().into_any_element(),
            ),
            button(
                "Cancel",
                m.button_h,
                c.control,
                c.control_border,
                c.text,
                kbd("esc", m.kbd_h, c.kbd_bg, c.kbd_border, c.text_secondary),
            ),
            button(
                "hover",
                m.button_h,
                c.control_hover,
                c.control_border,
                c.text,
                div().into_any_element(),
            ),
            button(
                "Compact",
                m.button_h_compact,
                c.control,
                c.control_border,
                c.text,
                kbd(
                    "⌃S",
                    m.kbd_h_small,
                    c.kbd_bg,
                    c.kbd_border,
                    c.text_secondary,
                ),
            ),
            div()
                .flex()
                .items_center()
                .h(m.chip_h)
                .px(palette.space.sm)
                .rounded(palette.radii.pill)
                .bg(c.accent_subtle)
                .child(Text::sentence_label("Accent subtle").color(c.accent))
                .into_any_element(),
        ],
    )
}

/// Every colour role of one mode, on that mode's own ground.
fn palette_column(palette: &Theme) -> AnyElement {
    let c = &palette.colors;
    let group = |title: &'static str, swatches: Vec<AnyElement>| {
        div()
            .flex()
            .flex_col()
            .gap(palette.space.xs)
            .child(Text::sentence_label(title).color(c.text_muted))
            .child(strip(palette, swatches))
    };
    let r = |name, color| role_swatch(palette, name, color);
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(palette.space.md)
        .p(palette.space.lg)
        .rounded(palette.radii.card)
        .bg(c.bg)
        .border(palette.metrics.hairline)
        .border_color(c.border)
        .child(Text::section_title(if palette.is_dark() { "Dark" } else { "Light" }).color(c.text))
        .child(group(
            "Grounds",
            vec![
                r("chrome", c.chrome),
                r("bg", c.bg),
                r("surface", c.surface),
                r("surface_raised", c.surface_raised),
                r("elevated", c.elevated),
                r("overlay", c.overlay),
                r("row_selected", c.row_selected),
                r("row_hover", c.row_hover),
            ],
        ))
        .child(group(
            "Accent",
            vec![
                r("accent", c.accent),
                r("accent_fill", c.accent_fill),
                r("accent_fill_hover", c.accent_fill_hover),
                r("accent_fill_text", c.accent_fill_text),
                r("accent_subtle", c.accent_subtle),
                r("focus_ring", c.focus_ring),
                r("selection", c.selection),
            ],
        ))
        .child(group(
            "Controls and keys",
            vec![
                r("control", c.control),
                r("control_hover", c.control_hover),
                r("control_border", c.control_border),
                r("kbd_bg", c.kbd_bg),
                r("kbd_border", c.kbd_border),
                r("border", c.border),
                r("border_strong", c.border_strong),
            ],
        ))
        .child(group(
            "Semantic",
            vec![
                r("success", c.success),
                r("warning", c.warning),
                r("danger", c.danger),
                r("info", c.info),
                r("skeleton", c.skeleton),
            ],
        ))
        .child(group(
            "Text contrast (WCAG AA is 4.5:1)",
            vec![
                contrast_swatch(palette, "text", c.text),
                contrast_swatch(palette, "text_secondary", c.text_secondary),
                contrast_swatch(palette, "text_muted", c.text_muted),
            ],
        ))
        .child(group("Controls", vec![control_samples(palette)]))
        .into_any_element()
}

fn colors_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let swatch = |name: &'static str, color: gpui::Hsla| role_swatch(&t, name, color);
    let both = div()
        .flex()
        .w_full()
        .gap(t.space.md)
        .child(palette_column(&Theme::dark()))
        .child(palette_column(&Theme::light()))
        .into_any_element();
    let ansi: Vec<AnyElement> = (0u8..16)
        .map(|ix| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(t.space.xxs)
                .w(px(40.0))
                .child(
                    div()
                        .size(px(24.0))
                        .rounded(t.radii.xs)
                        .bg(t.terminal.color(ix))
                        .border(t.metrics.hairline)
                        .border_color(t.colors.border),
                )
                .child(Text::hint(ix.to_string()).faint())
                .into_any_element()
        })
        .collect();

    let children = vec![
        LAYOUT.labeled("color roles, both modes", &t, both),
        LAYOUT.labeled("terminal ansi", &t, strip(&t, ansi)),
        LAYOUT.labeled(
            "terminal default",
            &t,
            strip(
                &t,
                vec![
                    swatch("fg", t.terminal.foreground),
                    swatch("bg", t.terminal.background),
                    swatch("cursor", t.terminal.cursor),
                ],
            ),
        ),
    ];
    LAYOUT.section("colors", &t, children)
}

fn type_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let children = vec![
        LAYOUT.labeled("page_title 20/26", &t, Text::page_title("Worktrees")),
        LAYOUT.labeled("section_title 16/22", &t, Text::section_title("Agents")),
        LAYOUT.labeled(
            "ui 13/18",
            &t,
            Text::ui("feat/payroll-fix — the branch you think in"),
        ),
        LAYOUT.labeled(
            "ui_strong 13/18",
            &t,
            Text::ui_strong("Fix RUT validation on payroll import"),
        ),
        LAYOUT.labeled("title 15/20", &t, Text::title("New worktree")),
        LAYOUT.labeled(
            "data 12.5/18",
            &t,
            Text::data("~/.fleet/worktrees/buk/payroll/feat-payroll-fix"),
        ),
        LAYOUT.labeled(
            "data_small 11.5/16",
            &t,
            Text::data_small("Receiving objects: 40% (81/202)"),
        ),
        LAYOUT.labeled(
            "caption 12/16",
            &t,
            Text::caption("Updated 2 minutes ago · 3 files changed"),
        ),
        LAYOUT.labeled(
            "sentence_label 11/14",
            &t,
            Text::sentence_label("Base branch"),
        ),
        LAYOUT.labeled("label 11/14 (legacy)", &t, Text::label("worktrees")),
        LAYOUT.labeled("hint mono 11/14", &t, Text::hint("⏎ open · esc cancel")),
        LAYOUT.labeled(
            "truncate head/middle/tail",
            &t,
            strip(
                &t,
                vec![
                    Text::data(truncate("dannyfuf/fleetd", 10, Truncate::Head)).into_any_element(),
                    Text::data(truncate("feat/payroll-fix", 10, Truncate::Middle))
                        .into_any_element(),
                    Text::data(truncate("Fix RUT validation", 10, Truncate::Tail))
                        .into_any_element(),
                ],
            ),
        ),
    ];
    LAYOUT.section("type", &t, children)
}

/// Radii, elevation, the redesign's control metrics and the delays, each shown at its value.
fn geometry_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let c = &t.colors;
    let tile = |name: String, radius: gpui::Pixels| {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(72.0))
            .rounded(radius)
            .bg(c.surface_raised)
            .border(t.metrics.hairline)
            .border_color(c.border)
            .child(Text::hint(name).faint())
            .into_any_element()
    };
    let radii = vec![
        tile(
            format!("control {}", f32::from(t.radii.control)),
            t.radii.control,
        ),
        tile(format!("card {}", f32::from(t.radii.card)), t.radii.card),
        tile(
            format!("popover {}", f32::from(t.radii.popover)),
            t.radii.popover,
        ),
        tile(
            format!("dialog {}", f32::from(t.radii.dialog)),
            t.radii.dialog,
        ),
        tile("pill".into(), t.radii.pill),
    ];
    let level = |name: &'static str, shadow: Vec<gpui::BoxShadow>, radius: gpui::Pixels| {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(160.0))
            .h(px(72.0))
            .rounded(radius)
            .bg(c.elevated)
            .border(t.metrics.hairline)
            .border_color(c.border_strong)
            .shadow(shadow)
            .child(Text::caption(name))
            .into_any_element()
    };
    let elevation = div()
        .flex()
        .gap(t.space.xl)
        .p(t.space.xl)
        .rounded(t.radii.card)
        .bg(c.overlay)
        .child(level("sheet", t.sheet_shadow(), t.radii.md))
        .child(level("dialog", t.dialog_shadow(), t.radii.dialog))
        .child(level("popover", t.popover_shadow(), t.radii.popover))
        .into_any_element();
    let bar = |name: String, width: gpui::Pixels, height: gpui::Pixels| {
        div()
            .flex()
            .items_center()
            .px(t.space.sm)
            .w(width)
            .h(height)
            .rounded(t.radii.xs)
            .bg(c.accent_subtle)
            .child(Text::hint(name).faint())
            .into_any_element()
    };
    let m = &t.metrics;
    let heights = vec![
        bar(
            format!("title_bar_h {}", f32::from(m.title_bar_h)),
            px(180.0),
            m.title_bar_h,
        ),
        bar(
            format!("status_bar_h {}", f32::from(m.status_bar_h)),
            px(180.0),
            m.status_bar_h,
        ),
        bar(
            format!("row_h_comfortable {}", f32::from(m.row_h_comfortable)),
            px(180.0),
            m.row_h_comfortable,
        ),
        bar(format!("row_h {}", f32::from(m.row_h)), px(180.0), m.row_h),
        bar(
            format!("button_h {}", f32::from(m.button_h)),
            px(180.0),
            m.button_h,
        ),
        bar(
            format!("button_h_compact {}", f32::from(m.button_h_compact)),
            px(180.0),
            m.button_h_compact,
        ),
        bar(format!("kbd_h {}", f32::from(m.kbd_h)), px(120.0), m.kbd_h),
        bar(
            format!("kbd_h_small {}", f32::from(m.kbd_h_small)),
            px(120.0),
            m.kbd_h_small,
        ),
    ];
    let widths = vec![
        bar(
            format!("sidebar_w {}", f32::from(m.sidebar_w)),
            m.sidebar_w,
            m.row_h,
        ),
        bar(
            format!("detail_w {}", f32::from(m.detail_w)),
            m.detail_w,
            m.row_h,
        ),
        bar(
            format!("sheet_w_detail {}", f32::from(m.sheet_w_detail)),
            m.sheet_w_detail,
            m.row_h,
        ),
    ];
    let motion = Text::data(format!(
        "tooltip_delay {} ms · prefix_hint_delay {} ms",
        t.motion.tooltip_delay, t.motion.prefix_hint_delay
    ));
    let children = vec![
        LAYOUT.labeled("radii", &t, strip(&t, radii)),
        LAYOUT.labeled("elevation over overlay", &t, elevation),
        LAYOUT.labeled("heights", &t, strip(&t, heights)),
        LAYOUT.labeled(
            "widths",
            &t,
            div().flex().flex_col().gap(t.space.xs).children(widths),
        ),
        LAYOUT.labeled("delays", &t, motion),
    ];
    LAYOUT.section("geometry", &t, children)
}

fn icons_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let icons: Vec<AnyElement> = Icon::ALL
        .iter()
        .map(|icon| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(t.space.xxs)
                .w(px(88.0))
                .child(icon.el().size(IconSize::Large).color(t.colors.text))
                .child(Text::hint(icon.name()).faint())
                .into_any_element()
        })
        .collect();
    let sizes = strip(
        &t,
        vec![
            Icon::Moon.el().size(IconSize::Small).into_any_element(),
            Icon::Moon.el().size(IconSize::Medium).into_any_element(),
            Icon::Moon.el().size(IconSize::Large).into_any_element(),
            Spinner::new("gallery-spinner").into_any_element(),
        ],
    );
    let children = vec![
        LAYOUT.labeled("sizes 12 / 14 / 16 + spinner", &t, sizes),
        strip(&t, icons),
    ];
    LAYOUT.section("icons (lucide, stroke 1.5)", &t, children)
}

fn glyphs_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let kinds = [
        StatusKind::Attached,
        StatusKind::DetachedAwake,
        StatusKind::Sleeping,
        StatusKind::NoSession,
        StatusKind::Unknown,
        StatusKind::Degraded,
        StatusKind::JobRunning,
        StatusKind::Cloning,
        StatusKind::CloneFailed,
        StatusKind::HostUnreachable,
    ];
    let glyphs: Vec<AnyElement> = kinds
        .iter()
        .enumerate()
        .map(|(ix, kind)| {
            div()
                .flex()
                .items_center()
                .gap(t.space.xs)
                .w(px(190.0))
                .child(StatusGlyph::new(*kind).id(("glyph", ix)))
                .child(Text::ui(kind.detail_word()).muted())
                .into_any_element()
        })
        .collect();

    let pr_states = [
        PrBadgeState::Draft,
        PrBadgeState::CiFail,
        PrBadgeState::Changes,
        PrBadgeState::CiPending,
        PrBadgeState::Approved,
        PrBadgeState::Review,
        PrBadgeState::Merged,
    ];
    let badges: Vec<AnyElement> = pr_states
        .iter()
        .map(|state| PrBadge::new(412, *state).into_any_element())
        .collect();

    let chips: Vec<AnyElement> = pr_states
        .iter()
        .map(|state| PrBadge::new(412, *state).chip().into_any_element())
        .collect();

    let children = vec![
        LAYOUT.labeled("status glyphs", &t, strip(&t, glyphs)),
        LAYOUT.labeled("pr badges", &t, strip(&t, badges)),
        LAYOUT.labeled("pr badges · chip", &t, strip(&t, chips)),
        LAYOUT.labeled(
            "pr badge, stale (>10 min)",
            &t,
            PrBadge::new(412, PrBadgeState::CiFail).stale(true),
        ),
        LAYOUT.labeled(
            "chips",
            &t,
            strip(
                &t,
                vec![
                    Chip::counter(Icon::LoaderCircle, 2)
                        .tone(Tone::Warning)
                        .spinning(true)
                        .id("chip-jobs")
                        .into_any_element(),
                    Chip::counter(Icon::CircleDot, 3)
                        .tone(Tone::Success)
                        .into_any_element(),
                    Chip::counter(Icon::Moon, 5)
                        .tone(Tone::Secondary)
                        .into_any_element(),
                    Chip::counter(Icon::CircleQuestionMark, 1)
                        .tone(Tone::Warning)
                        .into_any_element(),
                    Chip::counter(Icon::Flag, 4)
                        .tone(Tone::Secondary)
                        .into_any_element(),
                    Chip::counter(Icon::CircleArrowUp, 0).into_any_element(),
                    Chip::labeled(Icon::Cloud, "devbox")
                        .filled(true)
                        .into_any_element(),
                    Chip::labeled(Icon::TriangleAlert, "1 failed")
                        .tone(Tone::Danger)
                        .filled(true)
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "badges",
            &t,
            strip(
                &t,
                vec![
                    Badge::new("default").into_any_element(),
                    Badge::new("current")
                        .style(BadgeStyle::Outlined)
                        .into_any_element(),
                    Badge::new("ready")
                        .tone(Tone::Success)
                        .style(BadgeStyle::Filled)
                        .into_any_element(),
                    Badge::new("danger")
                        .tone(Tone::Danger)
                        .style(BadgeStyle::Filled)
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "status dots",
            &t,
            strip(
                &t,
                vec![
                    StatusDot::new(Tone::Success).into_any_element(),
                    StatusDot::new(Tone::Warning).into_any_element(),
                    StatusDot::new(Tone::Danger).into_any_element(),
                    StatusDot::small(Tone::Warning).into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "daemon dot",
            &t,
            strip(
                &t,
                vec![
                    DaemonDot::new(DaemonState::Healthy).into_any_element(),
                    DaemonDot::new(DaemonState::Degraded)
                        .label("reconnecting")
                        .into_any_element(),
                    DaemonDot::new(DaemonState::Lost)
                        .label("fleetd stopped")
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "keep-alive / degraded",
            &t,
            strip(
                &t,
                vec![
                    KeepAliveChips::new([
                        KeepAliveLabel::with_icon("claude", Icon::Bot),
                        KeepAliveLabel::with_icon(":3000", Icon::Server),
                        KeepAliveLabel::with_icon("nvim", Icon::FilePen),
                        KeepAliveLabel::new("vitest"),
                    ])
                    .width_ch(18.0)
                    .into_any_element(),
                    DegradedChip::hooks_failed()
                        .hint("J", "for log")
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "age labels",
            &t,
            strip(
                &t,
                vec![
                    AgeLabel::from_secs(42).into_any_element(),
                    AgeLabel::from_secs(7_200).into_any_element(),
                    AgeLabel::from_secs(432_000).into_any_element(),
                    AgeLabel::none().into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "freshness ladder",
            &t,
            strip(
                &t,
                vec![
                    FreshnessStamp::new("checked", 8).into_any_element(),
                    FreshnessStamp::new("checked", 240).into_any_element(),
                    FreshnessStamp::new("checked", 1_800)
                        .action("I", "re-check")
                        .into_any_element(),
                    FreshnessStamp::new("inspected", 60)
                        .error("gh: HTTP 502")
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "git ui status words",
            &t,
            strip(
                &t,
                vec![
                    ModeWord::word("NORMAL").into_any_element(),
                    ModeWord::word("STAGING").into_any_element(),
                    ModeWord::word("REBASING")
                        .tone(Tone::Warning)
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "key hints",
            &t,
            KeyHintRow::new()
                .key("⏎", "open")
                .key("^s x", "close")
                .key("esc", "cancel"),
        ),
        LAYOUT.labeled("divider", &t, div().w_full().child(Divider::horizontal())),
    ];
    LAYOUT.section("vocabulary", &t, children)
}

fn facts_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let compact = FactList::from_facts([
        Fact::safe("clean"),
        Fact::safe("merged into origin/main"),
        Fact::safe("no session"),
    ]);
    let expanded = FactList::from_facts([
        Fact::risk("12 uncommitted files"),
        Fact::risk("3 commits not on origin/main"),
        Fact::risk("session attached · claude, :3000 running"),
        Fact::unknown("unique commit count unavailable (gh unavailable)"),
        Fact::safe("PR #412 open (not merged)"),
    ]);
    let children = vec![
        LAYOUT.labeled(
            "key/value list",
            &t,
            KeyValueList::titled_with_trailing(
                "safety",
                FreshnessStamp::new("checked", 14).action("I", "re-check"),
            )
            .row("dirty", FactValue::known("12 files"))
            .row("ahead / behind", FactValue::known("⇡3 ⇣0"))
            .row("unique commits", FactValue::Null)
            .row("published", FactValue::known("no"))
            .mono_row(
                "path",
                FactValue::known("~/.fleet/worktrees/buk/payroll/feat"),
            )
            .row("warning", FactValue::warning("gh unavailable")),
        ),
        LAYOUT.labeled("fact list · compact (y)", &t, compact),
        LAYOUT.labeled("fact list · expanded (Y)", &t, expanded),
        LAYOUT.labeled(
            "doctor table",
            &t,
            DoctorTable::new([
                DoctorRow::new("git", DoctorStatus::Ok, "git version 2.49.0"),
                DoctorRow::new(
                    "gh auth",
                    DoctorStatus::Fail,
                    "gh: not logged in to github.com",
                ),
                DoctorRow::new("copy-on-write", DoctorStatus::Ok, "cp -c (APFS clonefile)"),
                DoctorRow::new(
                    "host devbox",
                    DoctorStatus::Warn,
                    "ssh: slow handshake (2.1s)",
                ),
            ]),
        ),
        LAYOUT.labeled(
            "empty state",
            &t,
            box_of(
                &t,
                px(72.0),
                EmptyState::new("No worktrees yet.").action("n  create one"),
            ),
        ),
        LAYOUT.labeled(
            "empty state · button",
            &t,
            box_of(
                &t,
                px(96.0),
                EmptyState::new("No worktrees yet").button(
                    Button::new("kit-empty-new", "New worktree")
                        .icon(Icon::Plus)
                        .style(ButtonStyle::Primary)
                        .kbd(gallery_kbd("n")),
                ),
            ),
        ),
        LAYOUT.labeled(
            "copy field",
            &t,
            div().w(px(304.0)).child(
                CopyField::new("~/worktrees/acme/web/spike").button(
                    IconButton::new("kit-copy-path", Icon::Copy, "Copy path")
                        .size(ButtonSize::Compact)
                        .kbd(gallery_kbd("y")),
                ),
            ),
        ),
        LAYOUT.labeled(
            "info card",
            &t,
            div().w(px(304.0)).child(
                InfoCard::new()
                    .title("Session")
                    .line(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(StatusGlyph::new(StatusKind::AgentWorking).id("kit-card-glyph"))
                            .child(div().flex_1().child(Text::ui("claude is working")))
                            .child(Text::caption("4m").muted()),
                    )
                    .line(Text::caption("2 tabs: zsh, claude \u{2014} kept by fleetd").muted()),
            ),
        ),
        LAYOUT.labeled(
            "skeleton rows",
            &t,
            box_of(&t, px(120.0), SkeletonRows::new(4)),
        ),
    ];
    LAYOUT.section("facts and tables", &t, children)
}

fn rows_section(
    cx: &mut App,
    this: WeakEntity<Gallery>,
    cursor: usize,
    scroll: &UniformListScrollHandle,
) -> AnyElement {
    let t = cx.theme().clone();
    let sample_row = |glyph: StatusKind, branch: &'static str, ix: usize| {
        Row::with_id(("row", ix))
            .leading(StatusGlyph::new(glyph).id(("row-glyph", ix)))
            .column(RowColumn::flex(Text::ui(branch)))
            .column(RowColumn::fixed(
                fleet_ui_kit::theme::ch(14.0),
                Text::ui("buk/payroll").muted(),
            ))
            .column(RowColumn::fixed(
                fleet_ui_kit::theme::ch(15.0),
                PrBadge::new(412, PrBadgeState::CiFail),
            ))
            .column(
                RowColumn::fixed(fleet_ui_kit::theme::ch(7.0), AgeLabel::from_secs(7_200))
                    .align(ColumnAlign::Right),
            )
    };

    let states = box_of(
        &t,
        px(160.0),
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(sample_row(StatusKind::Attached, "feat/payroll-fix", 0))
            .child(
                sample_row(StatusKind::Sleeping, "fix/rut-validator", 1)
                    .selected(true)
                    .cursor(true),
            )
            .child(sample_row(StatusKind::NoSession, "spike/gpui-vt", 2).dimmed(true))
            .child(sample_row(StatusKind::Unknown, "api-poc", 3).disabled(true))
            .child(
                Row::with_id(("row", 4usize))
                    .height(t.metrics.job_row_h)
                    .leading(StatusGlyph::new(StatusKind::Cloning).id("row-glyph-4"))
                    .column(RowColumn::flex(Text::ui("nixos")))
                    .second_line(Text::data_small("Receiving objects: 40% (81/202)").faint()),
            ),
    );

    // The pointer contract of UX-SPEC §5.1: a press selects, a double-click would open, a
    // right click would open the row's menu. The overview wires select only.
    let select = ListPointer::new().on_select(move |ix, _window, cx| {
        let Some(gallery) = this.upgrade() else {
            return;
        };
        gallery.update(cx, |gallery, cx| {
            gallery.cursor = ix;
            cx.notify();
        });
    });
    let actions_color = t.colors.text_secondary;
    let list = box_of(
        &t,
        px(180.0),
        ListView::new("gallery-list", 24, move |ix, is_cursor, _window, _cx| {
            Row::with_id(("list-row", ix))
                .hover_actions(
                    Icon::Ellipsis
                        .el()
                        .size(IconSize::Medium)
                        .color(actions_color),
                )
                .selected(is_cursor)
                .cursor(is_cursor)
                .leading(StatusGlyph::new(StatusKind::DetachedAwake).id(("list-glyph", ix)))
                .column(RowColumn::flex(Text::ui(format!(
                    "row {ix} — j/k moves the cursor"
                ))))
                .column(
                    RowColumn::fixed(
                        fleet_ui_kit::theme::ch(7.0),
                        AgeLabel::from_secs(60 * ix as i64 + 30),
                    )
                    .align(ColumnAlign::Right),
                )
                .into_any_element()
        })
        .pointer(select)
        .cursor(cursor)
        .track_scroll(scroll)
        .empty(EmptyState::new("Nothing matches.").action("esc  clear")),
    );

    let jobs = box_of(
        &t,
        px(320.0),
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                JobRow::new("job-0", JobStatus::Running, "Clone")
                    .subject("acme/infra")
                    .elapsed("0:42")
                    .percent(64)
                    .progress("Receiving objects: 64% (5121/8002)")
                    .hover_action(
                        Button::new("job-0-cancel", "Cancel")
                            .size(ButtonSize::Compact)
                            .kbd(gallery_kbd("c")),
                    )
                    .selected(true)
                    .cursor(true),
            )
            .child(
                JobRow::new("job-1", JobStatus::Running, "Run hooks for")
                    .subject("buk/payroll#feat-rut")
                    .elapsed("0:08")
                    .progress("pnpm install (2/3)"),
            )
            .child(
                JobRow::new("job-2", JobStatus::Failed, "Fetch review pull requests for")
                    .subject("acme/api")
                    .elapsed("1m")
                    .error("gh: HTTP 502 upstream connect error")
                    .actions(
                        Button::new("job-2-retry", "Retry")
                            .style(ButtonStyle::Primary)
                            .size(ButtonSize::Compact)
                            .kbd(gallery_kbd("R")),
                    ),
            )
            .child(
                JobRow::new("job-3", JobStatus::Done, "Prune worktrees")
                    .elapsed("12s \u{b7} 1m ago"),
            )
            .child(JobRow::new("job-4", JobStatus::Cancelled, "Fetch").subject("dannyfuf/fleetd"))
            .child(
                JobRow::new("job-5", JobStatus::Queued, "Prepare copies for")
                    .subject("buk/payroll"),
            )
            .child(
                JobRow::new("job-6", JobStatus::Cancelling, "Delete")
                    .subject("buk/www#chore-deps")
                    .elapsed("0:03")
                    .progress("waiting for the worker to stop"),
            )
            // The quit-and-stop confirm (§3.8.9) labels every cancellable job.
            .child(
                JobRow::new("job-7", JobStatus::Running, "Clone")
                    .subject("nixos")
                    .retryable(true),
            )
            .child(
                JobRow::new("job-8", JobStatus::Running, "Run hooks for")
                    .subject("payroll#feat-rut")
                    .retryable(false),
            ),
    );

    let children = vec![
        LAYOUT.labeled("row states", &t, states),
        LAYOUT.labeled("list view (virtualized)", &t, list),
        LAYOUT.labeled("job rows", &t, jobs),
        LAYOUT.labeled(
            "ticker / sticky error",
            &t,
            strip(
                &t,
                vec![
                    JobTicker::new("clone", "nixos")
                        .percent(40)
                        .extra(1)
                        .into_any_element(),
                    StickyErrorSlot::new("gh: HTTP 502 upstream connect error").into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "column ladder (worktrees @ 138 / 93 / 60 ch)",
            &t,
            strip(
                &t,
                [138.0f32, 93.0, 60.0]
                    .iter()
                    .map(|w| {
                        let keys = ColumnLadder::worktrees()
                            .resolve(*w)
                            .iter()
                            .map(|c| c.key.to_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        Text::hint(format!("{w:.0}ch: {keys}"))
                            .faint()
                            .into_any_element()
                    })
                    .collect(),
            ),
        ),
    ];
    LAYOUT.section("rows and lists", &t, children)
}

/// An embedded single-line editor seeded with the text a static demo card shows.
fn demo_query(cx: &mut Context<Gallery>, text: &str) -> Entity<TextInput> {
    let text = text.to_owned();
    cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_embedded(true, cx);
        input.set_text(text, cx);
        input
    })
}

/// The branch field a dialog body carries, with its derived preview line.
fn demo_branch(cx: &mut Context<Gallery>, text: &str) -> Entity<TextInput> {
    let text = text.to_owned();
    cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_label(Some("branch".into()), cx);
        input.set_preview(Some("\u{2192} buk/payroll#feat-rut-validator".into()), cx);
        input.set_text(text, cx);
        input
    })
}

/// The single-line states the input group shows, in display order.
fn demo_fields(cx: &mut Context<Gallery>) -> Vec<(&'static str, Entity<TextInput>)> {
    vec![
        ("text input", demo_branch(cx, "feat/rut-validator")),
        (
            "text input \u{b7} invalid",
            cx.new(|cx| {
                let mut input = TextInput::new(InputMode::SingleLine, cx);
                input.set_label(Some("branch".into()), cx);
                input.set_invalid(Some("branch cannot contain \"..\"".into()), cx);
                input.set_text("feat/../rut", cx);
                input
            }),
        ),
        (
            "text input \u{b7} placeholder",
            cx.new(|cx| {
                let mut input = TextInput::new(InputMode::SingleLine, cx);
                input.set_placeholder("Type to search GitHub repos in buk's owners.", cx);
                input.set_icon(Some(Icon::Search), cx);
                input
            }),
        ),
    ]
}

/// The multi-line states the board group shows.
fn demo_areas(cx: &mut Context<Gallery>) -> Vec<Entity<TextInput>> {
    let build = |cx: &mut Context<Gallery>,
                 label: &'static str,
                 rows: usize,
                 text: &str,
                 placeholder: Option<&'static str>,
                 mono: bool,
                 invalid: Option<&'static str>| {
        let text = text.to_owned();
        cx.new(|cx| {
            let mut input = TextInput::new(
                InputMode::Multiline {
                    min_rows: rows,
                    max_rows: rows,
                },
                cx,
            );
            input.set_label(Some(label.into()), cx);
            if let Some(placeholder) = placeholder {
                input.set_placeholder(placeholder, cx);
            }
            input.set_mono(mono, cx);
            input.set_invalid(invalid.map(Into::into), cx);
            input.set_text(text, cx);
            input
        })
    };
    vec![
        build(
            cx,
            "description",
            5,
            "Reproduce with `fleet board sync`.\nThe second line wraps as soon as the box is narrower than the sentence it holds.",
            None,
            false,
            None,
        ),
        build(
            cx,
            "comment",
            3,
            "",
            Some("Leave a comment. ctrl-s saves, esc cancels."),
            false,
            None,
        ),
        build(
            cx,
            "mono \u{b7} invalid",
            3,
            "fleet board move FLT-12 done\nfleet worktree new --card FLT-12",
            None,
            true,
            Some("the second command names no board"),
        ),
    ]
}

fn structure_section(cx: &mut App, filter_query: Entity<TextInput>) -> AnyElement {
    let t = cx.theme().clone();
    let pane = box_of(
        &t,
        px(150.0),
        SplitLayout::horizontal()
            .leading_size(px(200.0))
            .leading(
                Pane::fixed(px(200.0))
                    .border(PaneBorder::None)
                    .header(PaneHeader::new("repos").total(6))
                    .body(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                Row::new()
                                    .leading(StatusGlyph::new(StatusKind::Attached).id("rail-0"))
                                    .column(RowColumn::flex(Text::ui("All")))
                                    .column(RowColumn::auto(Text::ui("27").muted()))
                                    .selected(true)
                                    .cursor(true),
                            )
                            .child(
                                Row::new()
                                    .leading(StatusGlyph::new(StatusKind::Sleeping).id("rail-1"))
                                    .column(RowColumn::flex(Text::ui("payroll")))
                                    .column(RowColumn::auto(Text::ui("8").muted())),
                            ),
                    ),
            )
            .trailing(
                Pane::new()
                    .focused(true)
                    .header(
                        PaneHeader::new("worktrees")
                            .scope("payroll")
                            .shown(8)
                            .total(12)
                            .range(1, 8),
                    )
                    .scroll_thumb(0.0, 0.6)
                    .body(
                        div().flex().flex_col().child(
                            Row::new()
                                .leading(StatusGlyph::new(StatusKind::Attached).id("wt-0"))
                                .column(RowColumn::flex(Text::ui("feat/payroll-fix")))
                                .selected(true)
                                .cursor(true),
                        ),
                    ),
            ),
    );

    let filtered = box_of(
        &t,
        t.metrics.pane_header_h,
        PaneHeader::new("worktrees")
            .shown(2)
            .total(12)
            .query_slot(FilterBar::new(filter_query.clone(), 2, 12).query_slot()),
    );
    let retained = box_of(
        &t,
        t.metrics.pane_header_h,
        PaneHeader::new("worktrees")
            .scope("payroll")
            .filter_chip("rut")
            .shown(2)
            .total(12)
            .range(1, 2),
    );
    let stale = box_of(
        &t,
        t.metrics.pane_header_h,
        PaneHeader::new("worktrees")
            .scope("payroll")
            .total(12)
            .stale("2m"),
    );

    let bars = box_of(
        &t,
        t.metrics.title_bar_h,
        support::chrome::title_bar(
            "kit-title",
            support::chrome::TitleSample::Hub,
            support::chrome::TitleStatus {
                needs_you: 1,
                running: 2,
                ..support::chrome::TitleStatus::QUIET
            },
        ),
    );

    let status = box_of(
        &t,
        t.metrics.status_bar_h,
        support::chrome::status_buttons(
            StatusBar::new()
                .daemon(DaemonState::Healthy, None)
                .breadcrumb("buk › payroll › feat/payroll-fix")
                .ticker(JobTicker::new("clone", "nixos").percent(40).extra(1)),
            "kit-status",
            false,
        ),
    );
    let status_error = box_of(
        &t,
        t.metrics.status_bar_h,
        support::chrome::status_buttons(
            StatusBar::new()
                .daemon(DaemonState::Healthy, None)
                .breadcrumb("buk › payroll › feat/payroll-fix")
                .ticker(JobTicker::new("clone", "nixos"))
                .error(StickyErrorSlot::new("clone failed: gh: HTTP 502")),
            "kit-status-error",
            true,
        ),
    );

    let children = vec![
        LAYOUT.labeled("title bar", &t, bars),
        LAYOUT.labeled("status bar", &t, status),
        LAYOUT.labeled("status bar · error", &t, status_error),
        LAYOUT.labeled("panes + split", &t, pane),
        LAYOUT.labeled("pane header · filtering", &t, filtered),
        LAYOUT.labeled("pane header · retained", &t, retained),
        LAYOUT.labeled("pane header · stale", &t, stale),
        LAYOUT.labeled(
            "page header",
            &t,
            PageHeader::new("Worktrees")
                .subtitle("4 across 2 repositories \u{b7} 1 needs attention")
                .action(FilterField::new("kit-filter-idle", "Filter").kbd(gallery_kbd("/")))
                .action(Button::new("kit-page-clone", "Clone repo"))
                .action(
                    Button::new("kit-page-new", "New worktree")
                        .icon(Icon::Plus)
                        .style(ButtonStyle::Primary)
                        .kbd(gallery_kbd("n")),
                ),
        ),
        LAYOUT.labeled(
            "page header · stale",
            &t,
            PageHeader::new("Worktrees")
                .subtitle("2 in payroll")
                .stale("2m"),
        ),
        LAYOUT.labeled(
            "filter field · retained / editing / no match",
            &t,
            strip(
                &t,
                vec![
                    FilterField::new("kit-filter-kept", "Filter")
                        .query("rut")
                        .kbd(gallery_kbd("/"))
                        .into_any_element(),
                    FilterField::new("kit-filter-edit", "Filter")
                        .editor(filter_query.clone())
                        .counts(2, 12)
                        .into_any_element(),
                    FilterField::new("kit-filter-none", "Filter")
                        .editor(filter_query)
                        .counts(0, 12)
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "banner",
            &t,
            box_of(
                &t,
                t.metrics.banner_h,
                Banner::danger("fleetd stopped")
                    .countdown("reconnecting in 3s")
                    .hints(KeyHintRow::new().key("r", "reconnect").key("l", "log")),
            ),
        ),
    ];
    LAYOUT.section("structure", &t, children)
}

fn terminal_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let mut rows = Vec::new();
    for (ix, line) in [
        "\u{276f} claude",
        "\u{23fa} Reading src/payroll/rounding.rb\u{2026}",
        "  \u{b7} 128 lines \u{b7} 3 matches",
    ]
    .iter()
    .enumerate()
    {
        let mut cells = Vec::new();
        for ch in line.chars() {
            let mut cell = GridCell::new(ch.to_string(), &t);
            if ix == 0 {
                cell.fg = t.terminal.ansi[2];
                cell.bold = true;
            } else if ix == 2 {
                cell.fg = t.terminal.ansi[8];
            }
            cells.push(cell);
        }
        rows.push(GridRow::new(cells));
    }

    let grid = box_of(
        &t,
        px(96.0),
        TerminalGrid::from_shared(rows)
            .cursor(GridCursor {
                row: 1,
                col: 34,
                visible: true,
                shape: CursorShape::Block,
            })
            .selection(GridSelection::new(2, 2, 2, 18))
            .scrollback(412, 2000),
    );

    // Every cell attribute, so a regression in the proto -> kit conversion is visible here.
    let attr_cells = |text: &str, style: fn(GridCell) -> GridCell| {
        text.chars()
            .map(|ch| style(GridCell::new(ch.to_string(), &t)))
            .collect::<Vec<_>>()
    };
    let attrs_row = GridRow::new(
        [
            attr_cells("bold ", |c| c.bold(true)),
            attr_cells("dim ", |c| c.dim(true)),
            attr_cells("italic ", |c| c.italic(true)),
            attr_cells("under ", |c| c.underline(UnderlineStyle::Single)),
            attr_cells("double ", |c| c.underline(UnderlineStyle::Double)),
            attr_cells("curly ", |c| c.underline(UnderlineStyle::Curly)),
            attr_cells("strike ", |c| c.strikethrough(true)),
            attr_cells("blink ", |c| c.blink(true)),
            attr_cells("hidden ", |c| c.invisible(true)),
        ]
        .into_iter()
        .flatten(),
    );
    let attrs_row_2 = GridRow::new(
        attr_cells("inverse ", |c| c.inverse(true))
            .into_iter()
            .chain("err".chars().map(|ch| {
                GridCell::new(ch.to_string(), &t)
                    .underline(UnderlineStyle::Curly)
                    .underline_color(t.colors.danger)
            }))
            .chain([
                GridCell::new(" ", &t),
                GridCell::new("広", &t).width(CellWidth::Wide),
                GridCell::new("", &t).width(CellWidth::Spacer),
                GridCell::new("い", &t).width(CellWidth::Wide),
                GridCell::new("", &t).width(CellWidth::Spacer),
            ])
            .chain(attr_cells(" wide + spacer", |c| c)),
    );
    let attrs = box_of(
        &t,
        px(56.0),
        TerminalGrid::from_shared([attrs_row, attrs_row_2]),
    );

    let unfocused = box_of(
        &t,
        px(38.0),
        TerminalGrid::from_shared([GridRow::new(
            "hollow cursor \u{2014} terminal not focused"
                .chars()
                .map(|c| GridCell::new(c.to_string(), &t)),
        )])
        .focused(false)
        .cursor(GridCursor {
            row: 0,
            col: 14,
            visible: true,
            shape: CursorShape::Block,
        }),
    );

    let cursor_shapes = box_of(
        &t,
        px(38.0),
        div().flex().size_full().bg(t.terminal.background).children(
            [CursorShape::Block, CursorShape::Bar, CursorShape::Underline].map(|shape| {
                div().w(px(120.0)).h_full().child(
                    TerminalGrid::from_shared([GridRow::new(
                        "  shape".chars().map(|c| GridCell::new(c.to_string(), &t)),
                    )])
                    .cursor(GridCursor {
                        row: 0,
                        col: 0,
                        visible: true,
                        shape,
                    }),
                )
            }),
        ),
    );

    let strip_el = box_of(
        &t,
        t.metrics.pane_header_h,
        // Tab 2 is the active one and carries the attention dot: NATIVE-AGENTS.md §3.3's amber
        // is the single mark that survives selection, so the overview has to show it selected.
        TerminalTabStrip::new([
            TerminalTab::new(1, "nvim").keep_alive(Icon::FilePen),
            TerminalTab::new(2, "cc")
                .keep_alive(Icon::Bot)
                .attention(true),
            TerminalTab::new(3, "lg").activity(true),
            TerminalTab::new(4, "test").exited(1),
            TerminalTab::new(5, "server").exited(None),
            TerminalTab::new(6, "waking").starting(true),
            TerminalTab::new(7, "agent").agent_status(TerminalAgentState::Working),
            TerminalTab::new(8, "done")
                .agent_status(TerminalAgentState::Finished)
                .unread(true),
            TerminalTab::new(9, "review").kind(TerminalTabKind::Native),
        ])
        .active(1),
    );

    let overlays = box_of(
        &t,
        px(300.0),
        div()
            .relative()
            .size_full()
            .bg(t.terminal.background)
            .child(ScrollPill::new(412, 2000).selecting(true))
            .child(support::prefix_menu::sample(
                &t,
                "overview-prefix-menu",
                true,
                2,
            )),
    );

    let badge = box_of(
        &t,
        px(46.0),
        div()
            .relative()
            .size_full()
            .bg(t.terminal.background)
            .child(ScrollbackBadge::new(412, 2000)),
    );

    let veiled = box_of(
        &t,
        px(72.0),
        Veil::new(true).content(
            div()
                .size_full()
                .bg(t.terminal.background)
                .p(t.space.sm)
                .child(Text::data("keys typed here are dropped, not buffered")),
        ),
    );

    let children = vec![
        LAYOUT.labeled("terminal grid", &t, grid),
        LAYOUT.labeled("cell attributes", &t, attrs),
        LAYOUT.labeled("cursor shapes", &t, cursor_shapes),
        LAYOUT.labeled("unfocused cursor", &t, unfocused),
        LAYOUT.labeled("tab strip", &t, strip_el),
        LAYOUT.labeled("scroll pill + prefix hint", &t, overlays),
        LAYOUT.labeled("scrollback badge", &t, badge),
        LAYOUT.labeled(
            "vt modes",
            &t,
            strip(
                &t,
                vec![
                    TerminalModes::new([
                        TerminalMode::AltScreen,
                        TerminalMode::MouseReporting,
                        TerminalMode::BracketedPaste,
                    ])
                    .into_any_element(),
                    TerminalModes::new([TerminalMode::ApplicationCursor])
                        .glyphs_only()
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "exit strip",
            &t,
            box_of(&t, t.metrics.strip_h, ExitStrip::new(1)),
        ),
        LAYOUT.labeled(
            "exit strip (signal, no code)",
            &t,
            box_of(&t, t.metrics.strip_h, ExitStrip::new(None)),
        ),
        LAYOUT.labeled("veil (daemon lost)", &t, veiled),
        LAYOUT.labeled(
            "log view",
            &t,
            box_of(
                &t,
                px(96.0),
                LogView::from_shared(
                    "gallery-log",
                    (0..40)
                        .map(|i| {
                            SharedString::from(format!("[{i:03}] remote: Counting objects\u{2026}"))
                        })
                        .collect::<Vec<_>>(),
                ),
            ),
        ),
        LAYOUT.labeled(
            "daemon splash (cold start)",
            &t,
            box_of(
                &t,
                px(120.0),
                DaemonSplash::starting("Starting fleetd\u{2026}").detail("~/.fleet/fleetd.sock"),
            ),
        ),
        LAYOUT.labeled(
            "daemon splash (will not start)",
            &t,
            box_of(
                &t,
                px(200.0),
                DaemonSplash::failed("fleetd could not start.")
                    .detail("The socket ~/.fleet/fleetd.sock is stale.")
                    .log_lines([
                        SharedString::from("ERROR listen: address already in use"),
                        SharedString::from("ERROR socket owner pid 4211 is gone"),
                        SharedString::from("ERROR giving up after 3 attempts"),
                    ])
                    .hints(
                        KeyHintRow::new()
                            .key("r", "retry")
                            .key("L", "open log")
                            .key("D", "doctor")
                            .key("ctrl-q", "quit"),
                    ),
            ),
        ),
    ];
    LAYOUT.section("terminal", &t, children)
}

fn input_section(cx: &mut App, fields: &[(&'static str, Entity<TextInput>)]) -> AnyElement {
    let t = cx.theme().clone();
    let mut children: Vec<AnyElement> = fields
        .iter()
        .map(|(label, input)| {
            LAYOUT.labeled(
                label,
                &t,
                div().flex().flex_col().w(px(360.0)).child(input.clone()),
            )
        })
        .collect();
    children.extend([
        LAYOUT.labeled(
            "cycler",
            &t,
            div()
                .flex()
                .flex_col()
                .w(px(360.0))
                // Up to four listed options draw side by side, more as a dropdown field.
                .child(
                    Cycler::labeled("agent", "claude")
                        .options(["claude", "codex"])
                        .has_prev(false),
                )
                .child(
                    Cycler::labeled("keep jobs for", "10 min")
                        .options(["1 min", "5 min", "10 min", "30 min", "1 h"]),
                )
                // Off grid: a persisted value outside the configured steps is none of the
                // segments, so it draws as a field, and the next move returns to a known step.
                .child(
                    Cycler::labeled("host", "devbox (removed)")
                        .options(["local", "devbox"])
                        .off_grid(true)
                        .has_prev(false)
                        .has_next(false),
                ),
        ),
        LAYOUT.labeled(
            "toggles",
            &t,
            div()
                .flex()
                .flex_col()
                .w(px(360.0))
                .child(Toggle::labeled("Sleep on switch", true).focused(true))
                .child(Toggle::labeled("Warn before quitting", false))
                .child(
                    Toggle::labeled("Claude keep-alive", true)
                        .detail("matching 2 processes now")
                        .disabled(true),
                ),
        ),
        LAYOUT.labeled(
            "number fields",
            &t,
            div()
                .flex()
                .flex_col()
                .w(px(360.0))
                .child(
                    NumberField::labeled("grace", 2000)
                        .unit("ms")
                        .min(0)
                        .focused(true),
                )
                .child(
                    NumberField::labeled("local status refresh", 200)
                        .unit("ms")
                        .min(500),
                )
                .child(
                    ValueField::new("kit-value", "")
                        .label("Default model")
                        .placeholder("Harness default"),
                ),
        ),
        LAYOUT.labeled(
            "segmented tabs",
            &t,
            SegmentedTabs::new([
                SegmentedTab::new("mine", 7),
                SegmentedTab::new("review", 4).loading(true),
            ])
            .active(0),
        ),
        LAYOUT.labeled(
            "segmented control",
            &t,
            SegmentedControl::new(
                "kit-segmented",
                [
                    Segment::new("Worktrees"),
                    Segment::new("Pull requests").count(Some(4)),
                    Segment::new("Board").count(Some(0)),
                ],
            ),
        ),
        LAYOUT.labeled(
            "switch",
            &t,
            div()
                .flex()
                .gap(t.space.md)
                .child(Switch::new("kit-switch-on", true))
                .child(Switch::new("kit-switch-off", false)),
        ),
        LAYOUT.labeled(
            "select + fuzzy list",
            &t,
            div().w(px(420.0)).child(
                Select::new("origin/main")
                    .label("base")
                    .open(true)
                    .focused(true)
                    .options(
                        FuzzyList::new(
                            "kit-select-options",
                            [
                                FuzzyItem::new("origin/main").trailing("default"),
                                FuzzyItem::new("origin/release-2026"),
                                FuzzyItem::new("pull/412/head").trailing("previous base"),
                            ],
                        )
                        .cursor(0)
                        .cap(6),
                    ),
            ),
        ),
        LAYOUT.labeled(
            "fuzzy list · two-line",
            &t,
            div().w(px(420.0)).child(
                FuzzyList::new(
                    "kit-fuzzy-two-line",
                    [
                        FuzzyItem::new("bukhr/payroll")
                            .secondary("Nómina y remuneraciones")
                            .trailing("2d")
                            .leading(Icon::Lock.el().size(IconSize::Medium)),
                        FuzzyItem::new("acme/payrolls")
                            .trailing("3w")
                            .leading(Icon::Globe.el().size(IconSize::Medium)),
                    ],
                )
                .cursor(0),
            ),
        ),
    ]);
    LAYOUT.section("input", &t, children)
}

fn overlays_section(
    cx: &mut App,
    dialog_branch: Entity<TextInput>,
    palette_query: Entity<TextInput>,
) -> AnyElement {
    let t = cx.theme().clone();
    let dialog = box_of(
        &t,
        px(300.0),
        Dialog::new("New worktree")
            .subtitle("buk/payroll")
            .icon(Icon::GitBranchPlus)
            .width(px(460.0))
            .body(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(dialog_branch)
                    .child(
                        Callout::new(Tone::Success, Icon::Zap, "Prepared copy ready — about 2 s")
                            .detail("Hooks: pnpm install (run in background)"),
                    ),
            )
            .footer_start(Checkbox::new(
                "kit-dialog-open-after",
                "Open after creating",
                true,
            ))
            .on_dismiss(|_, _| {})
            .actions(vec![
                Button::new("kit-dialog-cancel", "Cancel").kbd(gallery_kbd("escape")),
                Button::new("kit-dialog-create", "Create")
                    .style(ButtonStyle::Primary)
                    .kbd(gallery_kbd("enter")),
            ]),
    );

    let confirm_compact = box_of(
        &t,
        px(220.0),
        ConfirmDialog::new(
            "Delete worktree fix-rut-validator?",
            FactList::from_facts([
                Fact::safe("clean"),
                Fact::safe("merged into origin/main"),
                Fact::safe("no session"),
            ]),
        )
        .target("buk/payroll · ~/worktrees/buk/payroll/fix-rut-validator")
        .stamp(FreshnessStamp::new("Checked", 8))
        .consequence("Moves the copy to trash, then removes it in the background.")
        .on_dismiss(|_, _| {})
        .accept_actions(Box::new(ConfirmYes), Box::new(ConfirmStrong)),
    );

    let confirm_expanded = box_of(
        &t,
        px(320.0),
        ConfirmDialog::new(
            "Delete worktree feat-payroll-fix?",
            FactList::from_facts([
                Fact::risk("12 uncommitted files").strong("12 uncommitted files"),
                Fact::risk("3 commits not on origin/main").strong("3 commits"),
                Fact::unknown("unique commit count unavailable (gh unavailable)"),
                Fact::safe("PR #412 open (not merged)"),
            ]),
        )
        .target("buk/payroll · ~/worktrees/buk/payroll/feat-payroll-fix")
        .stamp(FreshnessStamp::new("Checked", 180))
        .consequence(
            "The 3 unpushed commits and 12 uncommitted files exist only here and will be lost.",
        )
        .on_dismiss(|_, _| {})
        .accept_actions(Box::new(ConfirmYes), Box::new(ConfirmStrong)),
    );

    let palette = box_of(
        &t,
        px(320.0),
        Overlay::new().top(px(12.0)).width(px(560.0)).content(
            Palette::new(palette_query)
                .scope("All")
                .prefix_hint([(">", "commands"), ("@", "worktrees"), ("#", "cards")])
                .selected_label("payroll#feat-payroll-fix")
                .visible_rows(6)
                .section(PaletteSection::new(
                    "Go to",
                    [
                        PaletteRow::new("payroll#feat-payroll-fix")
                            .leading(StatusGlyph::new(StatusKind::Attached).id("pal-0"))
                            .detail("session attached"),
                        PaletteRow::new("payroll#fix-rut-validator")
                            .leading(StatusGlyph::new(StatusKind::Sleeping).id("pal-1"))
                            .detail("sleeping"),
                    ],
                ))
                .section(PaletteSection::new(
                    "Commands",
                    [
                        PaletteRow::new("Prune worktrees")
                            .qualifier("· buk/payroll")
                            .icon(Icon::Scissors)
                            .detail("asks first")
                            .kbd(gallery_kbd("x"))
                            .destructive(true),
                        PaletteRow::new("Clone repo")
                            .icon(Icon::CloudDownload)
                            .detail("Hub")
                            .kbd(gallery_kbd("n")),
                    ],
                ))
                .section(PaletteSection::new(
                    "Cards",
                    [PaletteRow::new("FLT-7 Model delivery windows")
                        .icon(Icon::SquareCheck)
                        .badge("Todo")],
                ))
                .section(PaletteSection::new(
                    "Agents",
                    [
                        PaletteRow::new("↳ codex — verify payroll")
                            .leading(StatusGlyph::new(StatusKind::Unknown).id("pal-agent-0"))
                            .secondary("blocked · which rounding rule?")
                            .trailing("attach"),
                        PaletteRow::new("claude — fix the reducer")
                            .leading(StatusGlyph::new(StatusKind::Attached).id("pal-agent-1"))
                            .secondary("working · 14m")
                            .trailing("go"),
                    ],
                )),
        ),
    );

    let sheet =
        box_of(
            &t,
            px(260.0),
            Sheet::new(true)
                .on_dismiss(|_, _| {})
                .header(
                    div()
                        .flex()
                        .flex_col()
                        .p(t.space.md)
                        .child(Text::section_title("Jobs"))
                        .child(Text::ui("1 running \u{b7} 1 failed").muted()),
                )
                .body(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            JobRow::new("sheet-job-0", JobStatus::Running, "Clone")
                                .subject("acme/infra")
                                .elapsed("0:42")
                                .percent(40)
                                .progress("Receiving objects: 40% (81/202)")
                                .selected(true)
                                .cursor(true),
                        )
                        .child(
                            JobRow::new("sheet-job-1", JobStatus::Failed, "Fetch")
                                .subject("acme/api")
                                .error("gh: HTTP 502 upstream connect error"),
                        ),
                )
                .footer(div().p(t.space.md).child(
                    Text::ui("Jobs run in fleetd and survive closing this window.").muted(),
                )),
        );

    let toasts = box_of(
        &t,
        px(160.0),
        ToastStack::new([
            Toast::new("Path copied").icon(Icon::ClipboardCheck).short(),
            Toast::new("Slept · kept cc (claude)").icon(Icon::Moon),
            Toast {
                count: 2,
                ..Toast::new("Already running").icon(Icon::Info).short()
            },
        ]),
    );

    // The Agent popup's frame: a scrimmed overlay lifted to the popover elevation.
    let floating_window = box_of(
        &t,
        px(200.0),
        Overlay::new()
            .top(t.space.xl)
            .width(px(560.0))
            .scrim(true)
            .layer(OverlayLayer::Dialog)
            .popover_elevation(true)
            .content(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .p(t.space.md)
                    .child(Text::ui_strong("Agent window"))
                    .child(Text::hint("popover elevation · scrim · dialog layer").muted()),
            ),
    );

    let children = vec![
        LAYOUT.labeled("dialog", &t, dialog),
        LAYOUT.labeled("confirm · compact", &t, confirm_compact),
        LAYOUT.labeled("confirm · expanded", &t, confirm_expanded),
        LAYOUT.labeled("palette (overlay)", &t, palette),
        LAYOUT.labeled(
            "floating window (overlay · popover elevation)",
            &t,
            floating_window,
        ),
        LAYOUT.labeled("sheet (jobs)", &t, sheet),
        LAYOUT.labeled("toast stack", &t, toasts),
    ];
    LAYOUT.section("overlays", &t, children)
}

/// The document the markdown panel renders: every construct the parser knows, in one card.
const MARKDOWN_SAMPLE: &str = "\
# Board sync
The reconciler is **pure**: it takes the local card, the remote card and the policy, and
returns ops. A bare URL such as https://fleet.dev/docs/board renders in the accent color.

## Steps
1. `fleet board sync` fetches the remote cards.
2. A conflict stops at the card, never at the board.

- Unknown syntax such as ~~strike~~ stays text.
- An unclosed **bold survives as two asterisks.

```sh
fleet board sync --board work
```";

/// The width one card panel is measured at; a real column is `COLUMN_WIDTH_CH` wide.
const TILE_W: f32 = 260.0;

fn board_section(cx: &mut App, areas: &[Entity<TextInput>]) -> AnyElement {
    let t = cx.theme().clone();

    let board = KanbanBoard::new("gallery-board").columns([
        KanbanColumn::new("col-backlog", "Backlog")
            .count(2)
            .accent(Some(t.colors.text_muted))
            .empty_hint("Nothing queued.")
            .tiles([
                CardTile::new("bl-1", "FLT-31", "Board backend adapter for Jira")
                    .priority(PriorityLevel::Medium)
                    .labels(vec![("backend".into(), Some("info".into()))])
                    .estimate(Some(5))
                    .into_any_element(),
                CardTile::new("bl-2", "FLT-32", "Decide the conflict policy defaults")
                    .labels(vec![("spec".into(), None)])
                    .into_any_element(),
            ])
            .into_any_element(),
        KanbanColumn::new("col-progress", "In progress")
            .count(2)
            .accent(Some(t.colors.warning))
            .focused(true)
            .tiles([
                CardTile::new("ip-1", "FLT-12", "Kanban column and card tile in the kit")
                    .priority(PriorityLevel::Urgent)
                    .labels(vec![
                        ("ui-kit".into(), Some("accent".into())),
                        ("board".into(), Some("success".into())),
                    ])
                    .assignee(Some("Danny Fuentes".into()))
                    .estimate(Some(3))
                    .due(Some("Mar 4".into()))
                    .worktree(true)
                    .dirty(true)
                    .selected(true)
                    .focused(true)
                    .into_any_element(),
                CardTile::new("ip-2", "FLT-13", "Markdown read mode for card descriptions")
                    .priority(PriorityLevel::High)
                    .assignee(Some("ana.perez".into()))
                    .worktree(true)
                    .conflict(true)
                    .into_any_element(),
            ])
            .into_any_element(),
        KanbanColumn::new("col-done", "Done")
            .count(0)
            .accent(Some(t.colors.success))
            .empty_hint("Nothing shipped yet.")
            .tiles([])
            .into_any_element(),
    ]);

    let tiles = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(t.space.md)
        .child(div().w(px(TILE_W)).child(CardTile::new(
            "tile-bare",
            "FLT-40",
            "A bare card: key and title, no meta row at all",
        )))
        .child(
            div().w(px(TILE_W)).child(
                CardTile::new(
                    "tile-full",
                    "FLT-41",
                    "Every meta slot at once, with a title long enough to clip at two lines",
                )
                .priority(PriorityLevel::High)
                .labels(vec![
                    ("bug".into(), Some("danger".into())),
                    ("infra".into(), Some("warning".into())),
                ])
                .assignee(Some("Danny Fuentes".into()))
                .estimate(Some(8))
                .due(Some("2026-03-04".into()))
                .worktree(true)
                .dirty(true)
                .conflict(true)
                .extras(vec!["QA: pending".into()]),
            ),
        )
        .child(
            div().w(px(TILE_W)).child(
                CardTile::new("tile-selected", "FLT-42", "Selected, keyboard elsewhere")
                    .priority(PriorityLevel::Low)
                    .selected(true),
            ),
        )
        .child(
            div().w(px(TILE_W)).child(
                CardTile::new("tile-focused", "FLT-43", "Selected and focused")
                    .priority(PriorityLevel::Urgent)
                    .selected(true)
                    .focused(true),
            ),
        );

    let priorities = strip(
        &t,
        PriorityLevel::ALL
            .into_iter()
            .map(|level| {
                PriorityGlyph::new(level)
                    .with_label(true)
                    .into_any_element()
            })
            .collect(),
    );

    let markdown = div()
        .w_full()
        .p(t.space.md)
        .rounded(t.radii.sm)
        .bg(t.colors.surface)
        .border_1()
        .border_color(t.colors.border)
        .child(MarkdownText::new(MARKDOWN_SAMPLE));

    let text_areas = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(t.space.md)
        .children(
            areas
                .iter()
                .map(|input| div().w(px(320.0)).child(input.clone())),
        );

    let children = vec![
        LAYOUT.labeled("kanban board", &t, box_of(&t, px(360.0), board)),
        LAYOUT.labeled("card tiles", &t, tiles),
        LAYOUT.labeled("priority glyphs", &t, priorities),
        LAYOUT.labeled("markdown", &t, markdown),
        LAYOUT.labeled("multi-line text inputs", &t, text_areas),
    ];
    LAYOUT.section("board", &t, children)
}

/// The controls group in brief; `gallery_buttons` has every state, live against a keymap.
fn controls_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let kbd = |keys: &str| Kbd::parse(keys).unwrap_or_else(|error| panic!("{keys:?}: {error}"));
    let children = vec![
        LAYOUT.labeled(
            "button",
            &t,
            strip(
                &t,
                vec![
                    Button::new("kit-primary", "Create worktree")
                        .style(ButtonStyle::Primary)
                        .kbd(kbd("enter"))
                        .into_any_element(),
                    Button::new("kit-secondary", "Cancel")
                        .kbd(kbd("escape"))
                        .into_any_element(),
                    Button::new("kit-ghost", "Board")
                        .style(ButtonStyle::Ghost)
                        .icon(Icon::GitBranch)
                        .kbd(kbd("g b"))
                        .into_any_element(),
                    Button::new("kit-danger", "Delete anyway")
                        .style(ButtonStyle::Danger)
                        .kbd(kbd("shift-y"))
                        .into_any_element(),
                    Button::new("kit-ghost-danger", "Deny and stop")
                        .style(ButtonStyle::GhostDanger)
                        .kbd(kbd("escape"))
                        .into_any_element(),
                    Button::new("kit-compact", "Close")
                        .size(ButtonSize::Compact)
                        .kbd(kbd("ctrl-s x"))
                        .into_any_element(),
                    Button::new("kit-disabled", "Save")
                        .kbd(kbd("ctrl-enter"))
                        .disabled(true)
                        .into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "composer chips · context meter",
            &t,
            strip(
                &t,
                vec![
                    ComposerChip::new("kit-composer-model", "gpt-5 \u{b7} high")
                        .tooltip("Switch the agent's model", Some(kbd("ctrl-s m")))
                        .into_any_element(),
                    ComposerChip::new("kit-composer-access", "asks before edits")
                        .icon(Icon::Shield)
                        .into_any_element(),
                    ContextMeter::new(34, "34%").into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "icon button · tooltip",
            &t,
            strip(
                &t,
                vec![
                    IconButton::new("kit-refresh", Icon::RefreshCw, "Refresh")
                        .kbd(kbd("r"))
                        .into_any_element(),
                    IconButton::new("kit-search", Icon::Search, "Filter")
                        .style(ButtonStyle::Secondary)
                        .kbd(gallery_kbd("/"))
                        .into_any_element(),
                    Tooltip::new("Refresh").kbd(kbd("r")).into_any_element(),
                ],
            ),
        ),
        LAYOUT.labeled(
            "kbd",
            &t,
            strip(
                &t,
                ["ctrl-s a", "g b", "cmd-k", "shift-tab", "enter", "escape"]
                    .into_iter()
                    .map(|keys| kbd(keys).into_any_element())
                    .collect(),
            ),
        ),
        // `gallery_menus` has the keyboard, the right-click area and every item state.
        LAYOUT.labeled(
            "menu · dropdown",
            &t,
            strip(
                &t,
                vec![
                    PopoverMenu::new("kit-more")
                        .trigger_with(|open, _, _| {
                            IconButton::new("kit-more-trigger", Icon::Ellipsis, "More actions")
                                .selected(open)
                        })
                        .menu(move |menu, _, _| {
                            menu.item(
                                MenuItem::new("Open")
                                    .icon(Icon::SquareTerminal)
                                    .kbd(kbd("o"))
                                    .on_select(|_, _| {}),
                            )
                            .item(MenuItem::new("Rename").kbd(kbd("r")).on_select(|_, _| {}))
                            .separator()
                            .item(
                                MenuItem::new("Delete worktree")
                                    .icon(Icon::Trash2)
                                    .kbd(kbd("shift-d"))
                                    .destructive(true)
                                    .on_select(|_, _| {}),
                            )
                        })
                        .into_any_element(),
                    div()
                        .w(px(220.0))
                        .child(
                            Dropdown::new("kit-dropdown", "Ask each time").menu(|menu, _, _| {
                                ["Read only", "Ask each time", "Full access"]
                                    .into_iter()
                                    .fold(menu, |menu, label| {
                                        menu.item(
                                            MenuItem::new(label)
                                                .checked(label == "Ask each time")
                                                .on_select(|_, _| {}),
                                        )
                                    })
                            }),
                        )
                        .into_any_element(),
                ],
            ),
        ),
    ];
    LAYOUT.section("controls", &t, children)
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = cx.theme().mode;
        let pad = cx.theme().space.xl;
        let cursor = self.cursor;
        let scroll = self.list_scroll.clone();
        let this = cx.entity().downgrade();
        let filter_query = self.filter_query.clone();
        let palette_query = self.palette_query.clone();
        let dialog_branch = self.dialog_branch.clone();
        let fields = self.fields.clone();
        let areas = self.areas.clone();

        let sections = vec![
            colors_section(cx),
            type_section(cx),
            geometry_section(cx),
            icons_section(cx),
            glyphs_section(cx),
            facts_section(cx),
            rows_section(cx, this, cursor, &scroll),
            structure_section(cx, filter_query),
            terminal_section(cx),
            input_section(cx, &fields),
            controls_section(cx),
            board_section(cx, &areas),
            overlays_section(cx, dialog_branch, palette_query),
        ];

        AppFrame::new()
            .title_bar(
                TitleBar::new()
                    .leading(Text::ui_strong("fleet-ui-kit gallery"))
                    .leading_inset(px(84.0))
                    .trailing(Chip::labeled(
                        if mode.is_dark() {
                            Icon::Moon
                        } else {
                            Icon::CircleArrowUp
                        },
                        if mode.is_dark() { "dark" } else { "light" },
                    )),
            )
            .body(
                div()
                    .id("gallery-scroll")
                    .track_focus(&self.focus_handle)
                    .key_context("Gallery")
                    .on_action(cx.listener(Self::toggle_theme))
                    .on_action(cx.listener(Self::quit))
                    .size_full()
                    .overflow_y_scroll()
                    .p(pad)
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .status_bar(
                StatusBar::new()
                    .breadcrumb("fleet-ui-kit · every component, every state")
                    .ticker(KeyHintRow::new().key("t", "toggle theme").key("q", "quit")),
            )
    }
}

fn main() {
    support::runtime::run(
        "fleet-ui-kit gallery",
        (1280.0, 800.0),
        Quit,
        |cx| {
            cx.bind_keys(menu_key_bindings());
            cx.bind_keys([
                KeyBinding::new("t", ToggleTheme, None),
                KeyBinding::new("q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
            ]);
        },
        Gallery::new,
    );
}
