//! The settings grammar of the input gallery: `SettingsRow`, `SettingsCard`, `ValueBox`,
//! `SearchField`, `Breadcrumb` and the inline `Cycler`, each in every state, then one pane drawn
//! as the *Settings · Agents* artboard draws it — the acceptance panel for the configuration
//! dialogs.
//!
//! Every state that needs focus (an editing box, a focused search field) is live: click the box
//! or the field, or press the key its label names.

use fleet_ui_kit::{prelude::*, theme::ch};
use gpui::{Entity, SharedString, WeakEntity};

use super::{Escape, InputGallery, LAYOUT, chip};

/// A settings dialog's pane: the 760 px dialog less its 196 px rail.
const PANE_W: f32 = 564.0;
/// The settings the live search runs over, one per row of the Settings dialog it samples.
pub(super) const SETTING_LABELS: &[&str] = &[
    "Default agent",
    "Terminal command",
    "Binary for threads",
    "Default access",
    "Default model",
    "Effort",
    "Sleep on switch",
    "Grace",
    "Warn before quitting with running jobs",
    "Keep finished jobs for",
    "Trash retention",
    "Hot pool size",
    "Freshness",
    "Refresh interval",
    "Clone protocol",
    "Repo cache",
    "PR cache",
    "Local status refresh",
    "Remote status refresh",
];
/// The efforts a harness declares, as the `Effort` row lists them.
pub(super) const EFFORTS: &[&str] = &["Default", "Low", "Medium", "High"];
/// The agents the `Default agent` row chooses between.
pub(super) const AGENTS: &[&str] = &["Claude", "Codex"];
/// Claude's permission modes: four phrases, too long to sit side by side, so a dropdown.
pub(super) const ACCESS: &[&str] = &[
    "asks before edits",
    "accepts edits",
    "plans only",
    "full access",
];
/// The model choice: harness default, the reported models, then the free-form entry.
pub(super) const MODELS: &[&str] = &[
    "Harness default",
    "Opus 5.5",
    "Sonnet 5",
    "Haiku 4.5",
    "Other model id\u{2026}",
];
/// What each model option resolves to, drawn as the dropdown item's trailing detail.
const MODEL_DETAILS: &[&str] = &[
    "whatever claude starts with",
    "claude-opus-5-5",
    "claude-sonnet-5",
    "claude-haiku-4-5",
    "type it",
];
/// The `Grace` row's clamp, stated by `number_rule` when `+` / `-` step past it.
const GRACE_MIN: i64 = 0;
const GRACE_MAX: i64 = 60_000;

/// How many of [`SETTING_LABELS`] a search query keeps.
pub(super) fn search_hits(query: &str) -> usize {
    let query = query.trim().to_lowercase();
    SETTING_LABELS
        .iter()
        .filter(|label| label.to_lowercase().contains(&query))
        .count()
}

/// A settings pane's ground (`elevated`, `lg` padding, `md` between cards), so a card and its
/// rows are judged on the surface they really sit on.
fn pane(theme: &Theme, children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    div()
        .w(px(PANE_W))
        .flex()
        .flex_col()
        .gap(theme.space.md)
        .p(theme.space.lg)
        .rounded(theme.radii.sm)
        .bg(theme.colors.elevated)
        .border(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .children(children)
        .into_any_element()
}

/// One untitled card holding `row`, on a pane.
fn one_row(theme: &Theme, id: &'static str, row: SettingsRow) -> AnyElement {
    pane(theme, [SettingsCard::new(id).row(row).into_any_element()])
}

/// A click that focuses `editor`: what `⏎` does on a box row.
fn focus_on_click(editor: &Entity<TextInput>) -> impl Fn(&mut Window, &mut App) + 'static {
    let editor = editor.clone();
    move |window, cx| editor.update(cx, |input, cx| input.focus(window, cx))
}

/// A category glyph for a column row's leading slot.
fn glyph(icon: Icon, tone: Tone) -> IconElement {
    icon.el().size(IconSize::Small).tone(tone)
}

/// A column row's hover actions: the pointer twins of `K`, `J` and `d`.
fn column_actions(theme: &Theme, prefix: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(theme.space.xxs)
        .child(
            IconButton::new((prefix, 0usize), Icon::CircleArrowUp, "Move up")
                .size(ButtonSize::Compact),
        )
        .child(
            IconButton::new((prefix, 1usize), Icon::CircleArrowDown, "Move down")
                .size(ButtonSize::Compact),
        )
        .child(
            IconButton::new((prefix, 2usize), Icon::Trash2, "Delete column")
                .size(ButtonSize::Compact),
        )
}

/// `SettingsRow` in each of its states, and with each of its slots.
pub(super) fn row_section(
    gallery: &InputGallery,
    this: WeakEntity<InputGallery>,
    theme: &Theme,
) -> AnyElement {
    let set_checked = {
        let this = this.clone();
        move |on: bool, _: &mut Window, cx: &mut App| {
            this.update(cx, |gallery, cx| {
                gallery.checked = on;
                cx.notify();
            })
            .ok();
        }
    };
    let grace_rule = number_rule(gallery.grace, Some(GRACE_MIN), Some(GRACE_MAX), Some("ms"));
    let noop = |_: &gpui::MouseDownEvent, _: &mut Window, _: &mut App| {};
    let model_row = |id: &'static str| {
        SettingsRow::new(id)
            .label("Default model")
            .helper("Empty means the harness picks.")
    };

    LAYOUT.section(
        "settings row \u{b7} one grammar, every state",
        theme,
        vec![
            LAYOUT.labeled(
                "rest",
                theme,
                one_row(
                    theme,
                    "row-rest-card",
                    model_row("row-rest")
                        .control(ValueBox::new("row-rest-box", "").placeholder("Harness default")),
                ),
            ),
            LAYOUT.labeled(
                "pointer hover (point at it: row_hover; a click lands nothing here)",
                theme,
                one_row(
                    theme,
                    "row-hover-card",
                    model_row("row-hover")
                        .on_click(noop)
                        .control(ValueBox::new("row-hover-box", "").placeholder("Harness default")),
                ),
            ),
            LAYOUT.labeled(
                "cursor (row_selected + the 2 px bar; nothing opened)",
                theme,
                one_row(
                    theme,
                    "row-cursor-card",
                    model_row("row-cursor").cursor(true).control(
                        ValueBox::new("row-cursor-box", "").placeholder("Harness default"),
                    ),
                ),
            ),
            LAYOUT.labeled(
                "editing (click the box or ^e: focus ring inset, same height)",
                theme,
                one_row(
                    theme,
                    "row-editing-card",
                    model_row("row-editing").cursor(true).control(
                        ValueBox::new("row-editing-box", "claude-opus-5")
                            .mono(true)
                            .editor(gallery.value_row_editor.clone())
                            .on_click(focus_on_click(&gallery.value_row_editor)),
                    ),
                ),
            ),
            LAYOUT.labeled(
                "invalid (the rule replaces the helper)",
                theme,
                one_row(
                    theme,
                    "row-invalid-card",
                    SettingsRow::new("row-invalid")
                        .label("Local status refresh")
                        .helper("How often local sessions and worktrees are polled.")
                        .when_some(
                            number_rule(200, Some(500), None, Some("ms")),
                            SettingsRow::invalid,
                        )
                        .control(
                            ValueBox::new("row-invalid-box", "200")
                                .mono(true)
                                .width(ValueBoxWidth::Number)
                                .unit("ms")
                                .invalid(true),
                        ),
                ),
            ),
            LAYOUT.labeled(
                "number, live (+ \u{2212} step 500; past 0..60000 the rule shows)",
                theme,
                one_row(
                    theme,
                    "row-grace-card",
                    SettingsRow::new("row-grace")
                        .label("Grace")
                        .helper("How long a terminal gets to finish before sleep closes it.")
                        .when_some(grace_rule.clone(), SettingsRow::invalid)
                        .control(
                            ValueBox::new("row-grace-box", gallery.grace.to_string())
                                .mono(true)
                                .width(ValueBoxWidth::Number)
                                .unit("ms")
                                .invalid(grace_rule.is_some()),
                        ),
                ),
            ),
            LAYOUT.labeled(
                "on / off (space, click the switch)",
                theme,
                one_row(
                    theme,
                    "row-switch-card",
                    SettingsRow::new("row-switch")
                        .label("Warn before quitting with running jobs")
                        .helper("They keep running in fleetd either way.")
                        .cursor(true)
                        .control(
                            Switch::new("row-switch-control", gallery.checked)
                                .on_toggle(set_checked.clone()),
                        ),
                ),
            ),
            LAYOUT.labeled(
                "disabled (dimmed, no hover, the helper says why)",
                theme,
                one_row(
                    theme,
                    "row-disabled-card",
                    SettingsRow::new("row-disabled")
                        .label("Push new cards")
                        .helper("Files a card as an issue on the backend. This board has none.")
                        .disabled(true)
                        .on_click(noop)
                        .control(Switch::new("row-disabled-control", false)),
                ),
            ),
            LAYOUT.labeled(
                "tall (a multi-line box; click it to edit, it grows to 8 rows)",
                theme,
                one_row(
                    theme,
                    "row-tall-card",
                    SettingsRow::new("row-tall")
                        .label("Instructions")
                        .helper(
                            "Prepended to the card's brief. Markdown. {key}, {title} and, with a \
                             pull request, {pr_url} are filled in.",
                        )
                        .tall(true)
                        .control(
                            ValueBox::new("row-tall-box", "")
                                .multiline(true)
                                .editor(gallery.multiline_row_editor.clone())
                                .on_click(focus_on_click(&gallery.multiline_row_editor)),
                        ),
                ),
            ),
            LAYOUT.labeled(
                "leading switch + badge + mono helper (a keep-alive rule)",
                theme,
                pane(
                    theme,
                    [SettingsCard::new("row-rule-card")
                        .row(
                            SettingsRow::new("row-rule-running")
                                .leading(
                                    Switch::new("row-rule-running-switch", gallery.checked)
                                        .on_toggle(set_checked),
                                )
                                .label("claude")
                                .label_badge(Badge::new("command").style(BadgeStyle::Filled))
                                .helper_mono("^claude(\\s|$)")
                                .control(
                                    Chip::labeled(Icon::Zap, "2 running")
                                        .tone(Tone::Success)
                                        .filled(true),
                                ),
                        )
                        .row(
                            SettingsRow::new("row-rule-none")
                                .leading(Switch::new("row-rule-none-switch", true))
                                .label("dev server")
                                .label_badge(Badge::new("port").style(BadgeStyle::Filled))
                                .helper("Any process listening on a port.")
                                .control(Text::caption("none running").muted()),
                        )
                        .into_any_element()],
                ),
            ),
            LAYOUT.labeled(
                "hover actions (revealed on hover, always on the cursor row) + trailing chevron",
                theme,
                pane(
                    theme,
                    [SettingsCard::new("row-columns-card")
                        .title("Columns")
                        .note("in board order \u{b7} a card can only route forward")
                        .row(
                            SettingsRow::new("row-column-backlog")
                                .leading(glyph(Icon::Dot, Tone::Muted))
                                .label("Backlog")
                                .helper("backlog")
                                .hover_actions(column_actions(theme, "row-column-backlog-act"))
                                .trailing(glyph(Icon::ChevronRight, Tone::Muted))
                                .on_click(noop)
                                .on_double_click(noop)
                                .on_secondary_click(noop),
                        )
                        .row(
                            SettingsRow::new("row-column-review")
                                .leading(glyph(Icon::CircleDot, Tone::Secondary))
                                .label("In review")
                                .helper(
                                    "started \u{b7} \u{26a1} agent runs the prompt \u{b7} then \
                                     \u{2192} Done",
                                )
                                .cursor(true)
                                .hover_actions(column_actions(theme, "row-column-review-act"))
                                .trailing(glyph(Icon::ChevronRight, Tone::Secondary))
                                .on_click(noop)
                                .on_double_click(noop)
                                .on_secondary_click(noop),
                        )
                        .into_any_element()],
                ),
            ),
            LAYOUT.labeled(
                "read-only + copy",
                theme,
                one_row(
                    theme,
                    "row-readonly-card",
                    SettingsRow::new("row-readonly")
                        .label("FLEET_HOME")
                        .control(Text::data("~/.fleet"))
                        .trailing(
                            IconButton::new("row-readonly-copy", Icon::Copy, "Copy")
                                .size(ButtonSize::Compact),
                        ),
                ),
            ),
            LAYOUT.labeled(
                "search hit (label_spans: the matched word strong, the section at the end)",
                theme,
                one_row(
                    theme,
                    "row-hit-card",
                    SettingsRow::new("row-hit")
                        .leading(glyph(Icon::Clock, Tone::Secondary))
                        .label_spans([
                            (SharedString::from("Keep finished "), false),
                            (SharedString::from("jobs"), true),
                            (SharedString::from(" for"), false),
                        ])
                        .helper("How long a finished job stays in the Jobs panel.")
                        .trailing(Text::caption("Jobs & warnings").muted())
                        .on_click(noop),
                ),
            ),
        ],
    )
}

/// `SettingsCard` titled, untitled, with a note, a subtitle and a caption.
pub(super) fn card_section(theme: &Theme) -> AnyElement {
    let command = |id: &'static str, n: usize, value: &'static str| {
        SettingsRow::new((id, n))
            .leading(div().w(ch(2.0)).child(Text::caption(n.to_string()).muted()))
            .control(
                ValueBox::new((id, n + 100), value)
                    .mono(true)
                    .width(ValueBoxWidth::Fill)
                    .placeholder("Type the next command\u{2026}"),
            )
            .trailing(
                IconButton::new((id, n + 200), Icon::X, "Remove command")
                    .size(ButtonSize::Compact)
                    .disabled(value.is_empty()),
            )
    };
    LAYOUT.section(
        "settings card",
        theme,
        vec![
            LAYOUT.labeled(
                "untitled (hairline rows)",
                theme,
                pane(
                    theme,
                    [SettingsCard::new("card-untitled")
                        .row(
                            SettingsRow::new("card-untitled-sleep")
                                .label("Sleep on switch")
                                .helper(
                                    "Opening another worktree sleeps the one you leave. Agents, \
                                     servers and unsaved editors stay.",
                                )
                                .control(Switch::new("card-untitled-sleep-switch", true)),
                        )
                        .row(
                            SettingsRow::new("card-untitled-grace")
                                .label("Grace")
                                .helper(
                                    "How long a terminal gets to finish before sleep closes it.",
                                )
                                .control(
                                    ValueBox::new("card-untitled-grace-box", "2000")
                                        .mono(true)
                                        .width(ValueBoxWidth::Number)
                                        .unit("ms"),
                                ),
                        )
                        .into_any_element()],
                ),
            ),
            LAYOUT.labeled(
                "titled + note + caption (a broken rule)",
                theme,
                pane(
                    theme,
                    [
                        SettingsCard::new("card-titled")
                            .title("Keep awake while running")
                            .note("matched against running processes now")
                            .row(
                                SettingsRow::new("card-titled-nvim")
                                    .leading(Switch::new("card-titled-nvim-switch", true))
                                    .label("nvim")
                                    .label_badge(Badge::new("command").style(BadgeStyle::Filled))
                                    .helper_mono("^n?vim")
                                    .control(Text::caption("none running").muted()),
                            )
                            .row(
                                SettingsRow::new("card-titled-tests")
                                    .leading(Switch::new("card-titled-tests-switch", true))
                                    .label("tests")
                                    .label_badge(Badge::new("command").style(BadgeStyle::Filled))
                                    .helper_mono("(pytest|cargo test")
                                    .disabled(true),
                            )
                            .caption(
                                Text::caption(
                                    "tests: the pattern does not compile, so the rule is skipped.",
                                )
                                .tone(Tone::Danger),
                            )
                            .into_any_element(),
                        div()
                            .px(theme.space.xs)
                            .child(
                                Text::caption(
                                    "Rules are defined in config.json. Open it from the footer \
                                     to add or change one.",
                                )
                                .muted(),
                            )
                            .into_any_element(),
                    ],
                ),
            ),
            LAYOUT.labeled(
                "titled + subtitle (the hooks card)",
                theme,
                pane(
                    theme,
                    [SettingsCard::new("card-subtitle")
                        .title("Prepare")
                        .subtitle(
                            "Runs on the prepared copy, once, before any worktree is made from it.",
                        )
                        .row(command(
                            "card-subtitle-row",
                            1,
                            "pnpm install --frozen-lockfile",
                        ))
                        .row(command("card-subtitle-row", 2, "cp ../.env.local .env"))
                        .row(command("card-subtitle-row", 3, ""))
                        .into_any_element()],
                ),
            ),
        ],
    )
}

/// `ValueBox` in each width, face and state.
pub(super) fn value_box_section(gallery: &InputGallery, theme: &Theme) -> AnyElement {
    // A box paints `bg`; judge it on the raised card it sits on.
    let ground = |child: AnyElement| {
        div()
            .flex()
            .w(px(460.0))
            .p(theme.space.md)
            .rounded(theme.radii.card)
            .bg(theme.colors.surface_raised)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(child)
            .into_any_element()
    };
    LAYOUT.section(
        "value box",
        theme,
        vec![
            LAYOUT.labeled(
                "text (UI face)",
                theme,
                ground(ValueBox::new("vb-text", "In review").into_any_element()),
            ),
            LAYOUT.labeled(
                "text \u{b7} mono (a command, a path, an id)",
                theme,
                ground(
                    ValueBox::new(
                        "vb-mono",
                        "claude --dangerously-skip-permissions --model opus",
                    )
                    .mono(true)
                    .on_click(|_, _| {})
                    .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "placeholder (what empty means)",
                theme,
                ground(
                    ValueBox::new("vb-placeholder", "")
                        .placeholder("Harness default")
                        .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "number + unit (96 px)",
                theme,
                ground(
                    ValueBox::new("vb-number", "2000")
                        .mono(true)
                        .width(ValueBoxWidth::Number)
                        .unit("ms")
                        .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "number + range unit",
                theme,
                ground(
                    ValueBox::new("vb-number-of", "2")
                        .mono(true)
                        .width(ValueBoxWidth::Number)
                        .unit("of 8")
                        .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "short (120 px, a board prefix)",
                theme,
                ground(
                    ValueBox::new("vb-short", "FLT")
                        .mono(true)
                        .width(ValueBoxWidth::Short)
                        .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "fill (the row's width)",
                theme,
                ground(
                    ValueBox::new("vb-fill", "pnpm install --frozen-lockfile")
                        .mono(true)
                        .width(ValueBoxWidth::Fill)
                        .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "multi-line \u{b7} resting (wraps, 3 lines minimum)",
                theme,
                ground(
                    ValueBox::new(
                        "vb-multiline",
                        "Review pull request {pr_url} for correctness and test coverage. Leave \
                         one summary comment; do not push.",
                    )
                    .multiline(true)
                    .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "multi-line \u{b7} editing (click it; grows to 8 rows)",
                theme,
                ground(
                    ValueBox::new("vb-multiline-editing", "")
                        .multiline(true)
                        .mono(true)
                        .editor(gallery.multiline_box_editor.clone())
                        .on_click(focus_on_click(&gallery.multiline_box_editor))
                        .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "invalid (danger border and value)",
                theme,
                ground(
                    ValueBox::new("vb-invalid", "200")
                        .mono(true)
                        .width(ValueBoxWidth::Number)
                        .unit("ms")
                        .invalid(true)
                        .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "editing \u{b7} focus ring (click it; the box does not grow)",
                theme,
                ground(
                    ValueBox::new("vb-editing", "2500")
                        .width(ValueBoxWidth::Number)
                        .unit("ms")
                        .editor(gallery.number_row_editor.clone())
                        .on_click(focus_on_click(&gallery.number_row_editor))
                        .into_any_element(),
                ),
            ),
            LAYOUT.labeled(
                "editor present, not focused (a hooks row at rest)",
                theme,
                ground(
                    ValueBox::new("vb-editor-rest", "")
                        .mono(true)
                        .width(ValueBoxWidth::Fill)
                        .editor(gallery.hooks_row_editor.clone())
                        .into_any_element(),
                ),
            ),
        ],
    )
}

/// `SearchField` empty, typed, matching nothing and focused.
pub(super) fn search_section(
    gallery: &InputGallery,
    this: WeakEntity<InputGallery>,
    theme: &Theme,
    cx: &App,
) -> AnyElement {
    let total = SETTING_LABELS.len();
    let shown = search_hits(gallery.search_live.read(cx).text());
    let missed = search_hits(gallery.search_no_match.read(cx).text());
    let width = px(240.0);
    LAYOUT.section(
        "search field",
        theme,
        vec![
            LAYOUT.labeled(
                "empty (the key chip)",
                theme,
                SearchField::new("sf-empty", gallery.search_empty.clone())
                    .kbd(chip("/"))
                    .width(width),
            ),
            LAYOUT.labeled(
                "typed, live (count + clear \u{2715}; click it or ^f to focus: focus_ring)",
                theme,
                SearchField::new("sf-live", gallery.search_live.clone())
                    .kbd(chip("/"))
                    .count(shown, total)
                    .on_clear(move |_, cx| {
                        this.update(cx, |_, cx| cx.notify()).ok();
                    })
                    .width(width),
            ),
            LAYOUT.labeled(
                "no match (the count turns amber)",
                theme,
                SearchField::new("sf-no-match", gallery.search_no_match.clone())
                    .kbd(chip("/"))
                    .count(missed, total)
                    .on_clear(|_, _| {})
                    .width(width),
            ),
        ],
    )
}

/// `Breadcrumb` plain, with a badge and a caption, and squeezed.
pub(super) fn breadcrumb_section(theme: &Theme) -> AnyElement {
    let ground = |width: f32, child: Breadcrumb| {
        div()
            .w(px(width))
            .px(theme.space.lg)
            .bg(theme.colors.elevated)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .rounded(theme.radii.sm)
            .child(child)
            .into_any_element()
    };
    LAYOUT.section(
        "breadcrumb (\u{2039} Parent goes back: esc)",
        theme,
        vec![
            LAYOUT.labeled(
                "badge + trailing",
                theme,
                ground(
                    PANE_W,
                    Breadcrumb::new("bc-column", "Columns", "In review")
                        .back_action(Box::new(Escape))
                        .badge(Badge::new("started").tone(Tone::Accent))
                        .trailing("4 of 6"),
                ),
            ),
            LAYOUT.labeled(
                "trailing only",
                theme,
                ground(
                    PANE_W,
                    Breadcrumb::new("bc-schedule", "Schedules", "Nightly triage")
                        .back_action(Box::new(Escape))
                        .trailing("next 14:05"),
                ),
            ),
            LAYOUT.labeled(
                "plain (a new item)",
                theme,
                ground(
                    PANE_W,
                    Breadcrumb::new("bc-new", "Schedules", "New schedule")
                        .back_action(Box::new(Escape)),
                ),
            ),
            LAYOUT.labeled(
                "narrow (the name ellipsises, the rest keeps its width)",
                theme,
                ground(
                    320.0,
                    Breadcrumb::new(
                        "bc-narrow",
                        "Columns",
                        "Waiting for a second reviewer before merge",
                    )
                    .back_action(Box::new(Escape))
                    .badge(Badge::new("started").tone(Tone::Accent))
                    .trailing("5 of 6"),
                ),
            ),
        ],
    )
}

/// The inline `Cycler`: the control alone, segmented or dropdown by the form rule.
pub(super) fn inline_cycler_section(
    gallery: &InputGallery,
    this: WeakEntity<InputGallery>,
    theme: &Theme,
) -> AnyElement {
    let set = move |apply: fn(&mut InputGallery, usize)| {
        let this = this.clone();
        move |ix: usize, _: &mut Window, cx: &mut App| {
            this.update(cx, |gallery, cx| {
                apply(gallery, ix);
                cx.notify();
            })
            .ok();
        }
    };
    LAYOUT.section(
        "cycler \u{b7} inline (a settings row's control)",
        theme,
        vec![
            LAYOUT.labeled(
                "segmented (\u{2264} 4 short options; click a segment)",
                theme,
                Cycler::new(EFFORTS[gallery.effort])
                    .id("cy-inline-segmented")
                    .inline(true)
                    .options(EFFORTS.iter().copied())
                    .on_select(set(|gallery, ix| gallery.effort = ix)),
            ),
            LAYOUT.labeled(
                "dropdown (a longer set; pick from the list)",
                theme,
                Cycler::new(ACCESS[gallery.access])
                    .id("cy-inline-dropdown")
                    .inline(true)
                    .options(ACCESS.iter().copied())
                    .on_select(set(|gallery, ix| gallery.access = ix)),
            ),
            LAYOUT.labeled(
                "dropdown + details (a model id beside its name)",
                theme,
                Cycler::new(MODELS[gallery.model])
                    .id("cy-inline-details")
                    .inline(true)
                    .options(MODELS.iter().copied())
                    .details(MODEL_DETAILS.iter().copied())
                    .on_select(set(|gallery, ix| gallery.model = ix)),
            ),
            LAYOUT.labeled(
                "segmented \u{b7} one option unavailable",
                theme,
                Cycler::new("manual")
                    .id("cy-inline-unavailable")
                    .inline(true)
                    .options(["manual", "remote wins", "local wins"])
                    .unavailable([2])
                    .on_select(|_, _, _| {}),
            ),
            LAYOUT.labeled(
                "disabled",
                theme,
                Cycler::new("ssh")
                    .id("cy-inline-disabled")
                    .inline(true)
                    .options(["ssh", "https"])
                    .disabled(true),
            ),
            LAYOUT.labeled(
                "off grid (a value none of the options name)",
                theme,
                Cycler::new("claude-opus-4")
                    .id("cy-inline-off-grid")
                    .inline(true)
                    .options(MODELS.iter().copied())
                    .off_grid(true),
            ),
        ],
    )
}

/// The acceptance panel: the Agents pane exactly as the *Settings · Agents* artboard draws it —
/// an untitled card, then one card per harness, the cursor on `Default model`. Every row is
/// live: a click lands the cursor, the controls change their values.
pub(super) fn agents_pane_section(
    gallery: &InputGallery,
    this: WeakEntity<InputGallery>,
    theme: &Theme,
) -> AnyElement {
    let cursor = gallery.settings_cursor;
    let land = |ix: usize| {
        let this = this.clone();
        move |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut App| {
            this.update(cx, |gallery, cx| {
                gallery.settings_cursor = ix;
                cx.notify();
            })
            .ok();
        }
    };
    let set = |apply: fn(&mut InputGallery, usize)| {
        let this = this.clone();
        move |ix: usize, _: &mut Window, cx: &mut App| {
            this.update(cx, |gallery, cx| {
                apply(gallery, ix);
                cx.notify();
            })
            .ok();
        }
    };
    let row = |ix: usize, label: &'static str, helper: &'static str| {
        SettingsRow::new(("agents-row", ix))
            .label(label)
            .helper(helper)
            .cursor(cursor == ix)
            .on_click(land(ix))
    };
    let command_box = |id: &'static str, value: &'static str| {
        ValueBox::new(id, value).mono(true).on_click(|_, _| {})
    };

    let default_agent = SettingsCard::new("agents-default").row(
        row(
            0,
            "Default agent",
            "Used by the Agent buttons and by a new thread.",
        )
        .control(
            Cycler::new(AGENTS[gallery.agent])
                .id("agents-default-agent")
                .inline(true)
                .options(AGENTS.iter().copied())
                .on_select(set(|gallery, ix| gallery.agent = ix)),
        ),
    );
    let claude = SettingsCard::new("agents-claude")
        .title("Claude")
        .row(
            row(
                1,
                "Terminal command",
                "Typed into a terminal tab. Aliases work.",
            )
            .control(command_box("agents-claude-command", "claude")),
        )
        .row(
            row(
                2,
                "Binary for threads",
                "Run directly for an agent thread, without a shell.",
            )
            .control(command_box("agents-claude-binary", "claude")),
        )
        .row(
            row(
                3,
                "Default access",
                "New threads start with it. Each thread can change it.",
            )
            .control(
                Cycler::new(ACCESS[gallery.access])
                    .id("agents-claude-access")
                    .inline(true)
                    .options(ACCESS.iter().copied())
                    .on_select(set(|gallery, ix| gallery.access = ix)),
            ),
        )
        .row(
            row(
                4,
                "Default model",
                "The models claude reported. Harness default lets claude pick.",
            )
            .control(
                Cycler::new(MODELS[gallery.model])
                    .id("agents-claude-model")
                    .inline(true)
                    .options(MODELS.iter().copied())
                    .details(MODEL_DETAILS.iter().copied())
                    .on_select(set(|gallery, ix| gallery.model = ix)),
            ),
        )
        .row(
            row(5, "Effort", "Used with the default model.").control(
                Cycler::new(EFFORTS[gallery.effort])
                    .id("agents-claude-effort")
                    .inline(true)
                    .options(EFFORTS.iter().copied())
                    .on_select(set(|gallery, ix| gallery.effort = ix)),
            ),
        );
    let codex = SettingsCard::new("agents-codex")
        .title("Codex")
        .row(
            row(
                6,
                "Terminal command",
                "Typed into a terminal tab. Aliases work.",
            )
            .control(command_box("agents-codex-command", "codex")),
        )
        .row(
            row(
                7,
                "Binary for threads",
                "Run directly for an agent thread, without a shell.",
            )
            .control(command_box("agents-codex-binary", "codex")),
        );

    LAYOUT.section(
        "settings pane \u{b7} Agents, as the artboard draws it (acceptance)",
        theme,
        vec![LAYOUT.labeled(
            "click a row: the cursor lands; the controls are live",
            theme,
            pane(
                theme,
                [
                    default_agent.into_any_element(),
                    claude.into_any_element(),
                    codex.into_any_element(),
                ],
            ),
        )],
    )
}
