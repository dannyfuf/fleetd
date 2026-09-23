//! The worktree detail panel (UX-SPEC §3.4): what you can do first, then the session, the git
//! safety facts in words, and where the worktree lives.
//!
//! Every word comes from the row's prepared [`WorktreeDetail`]; the panel only lays it out. Its
//! buttons run the same actions as the row's keys and menus, on the row under the cursor.

use fleet_ui_kit::{
    ActiveTheme, Button, ButtonSize, ButtonStyle, CopyField, FactRow, HarnessTargetExt, Icon,
    IconButton, IconSize, InfoCard, MenuAnchor, PopoverMenu, PrBadge, StatusGlyph, Text, Tone,
    format_age,
};
use gpui::{AnyElement, App, IntoElement, SharedString, div, prelude::*};

use crate::{
    actions::{hub, worktrees},
    views::worktrees_list::{
        GitFacts, KnownGit, OPEN_KEY, WorktreeDetail, WorktreeRow, label, row_menu,
    },
};

/// The primary button's label: the panel says what opening *is*.
const OPEN_WORKSPACE: &str = "Open workspace";
/// The Git section's title.
const GIT: &str = "Git";
/// The Location section's title.
const LOCATION: &str = "Location";
/// The Session card's title.
const SESSION: &str = "Session";
/// A fact git could not state (§1.3): never `0`.
const NULL: &str = "\u{2014}";

/// Everything the worktree panel needs.
#[derive(Clone, Copy)]
pub struct WorktreeProps<'a> {
    /// The row under the cursor, with its prepared detail.
    pub row: &'a WorktreeRow,
    /// Seconds since the row was prepared, so the ages stay current.
    pub age_offset: i64,
    /// Whether `u` has a delete to undo, so the menu offers it.
    pub undo_available: bool,
}

/// §3.4's worktree panel: head, actions, Session, Git, Location.
#[must_use]
pub fn worktree(props: WorktreeProps<'_>, cx: &App) -> AnyElement {
    let WorktreeProps {
        row,
        age_offset,
        undo_available,
    } = props;
    let detail = &row.detail;
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .w_full()
        .flex_none()
        .px(theme.space.lg)
        .py(theme.space.xl)
        .gap(theme.space.lg)
        .child(head(row, cx))
        .child(actions(undo_available, cx))
        .child(session_card(detail, age_offset, cx))
        .child(git_section(&detail.git, cx))
        .child(location(detail, age_offset, cx))
        .into_any_element()
}

/// `⑂ spike` over `acme/web · from origin/main · on this Mac`.
fn head(row: &WorktreeRow, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(theme.space.xs)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(
                    row.name_icon
                        .icon
                        .el()
                        .size(IconSize::Large)
                        .tone(row.name_icon.tone)
                        .spinning(row.name_icon.spins)
                        .id("detail-name-icon"),
                )
                .child(Text::section_title(row.branch.clone()).ellipsize()),
        )
        .child(
            Text::caption(row.detail.subtitle.clone())
                .muted()
                .ellipsize(),
        )
        .into_any_element()
}

/// `[Open workspace ⏎] [Sleep s] [⋯]`: the panel leads with what you can do.
fn actions(undo_available: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .child(
            div().flex_1().min_w_0().child(
                Button::new("detail-open", OPEN_WORKSPACE)
                    .style(ButtonStyle::Primary)
                    .full_width()
                    .action(Box::new(worktrees::Open))
                    .prefer_key(OPEN_KEY)
                    .harness_target("detail.open"),
            ),
        )
        .child(
            Button::new("detail-sleep", label(&worktrees::Sleep))
                .action(Box::new(worktrees::Sleep))
                .harness_target("detail.sleep"),
        )
        .child(
            PopoverMenu::new("detail-more")
                .anchor(MenuAnchor::BottomRight)
                .trigger_with(|open, _, _| {
                    IconButton::new("detail-more-trigger", Icon::Ellipsis, "More actions")
                        .style(ButtonStyle::Secondary)
                        .selected(open)
                })
                .menu(move |menu, _, _| row_menu(menu, undo_available))
                .harness_target("detail.menu"),
        )
        .into_any_element()
}

/// The Session card: the state and its age, then the tabs fleetd keeps.
fn session_card(detail: &WorktreeDetail, age_offset: i64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let age = detail
        .session_age
        .map(|age| format_age(age.saturating_add(age_offset)));
    let card = InfoCard::new().title(SESSION).line(
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .child(StatusGlyph::new(detail.session_kind).id("detail-session-glyph"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Text::ui(detail.session_state.clone()).ellipsize()),
            )
            .children(age.map(|age| Text::caption(age).muted().flex_none())),
    );
    match detail.session_tabs.clone() {
        Some(tabs) => card.line(Text::caption(tabs).muted()).into_any_element(),
        None => card.into_any_element(),
    }
}

/// The Git section: the safety facts in words, or why there are none yet.
fn git_section(git: &GitFacts, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let body: AnyElement = match git {
        // §1.3: absence of knowledge still renders — and says how to get it.
        GitFacts::NotChecked => div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .child(Text::ui("Not checked").faint())
            .child(inspect_button())
            .into_any_element(),
        GitFacts::Checking => Text::ui("Checking\u{2026}").muted().into_any_element(),
        GitFacts::Failed(error) => div()
            .flex()
            .flex_col()
            .gap(theme.space.xs)
            .child(Text::ui(error.clone()).tone(Tone::Danger))
            .child(inspect_button())
            .into_any_element(),
        GitFacts::Known(known) => known_git(known, cx),
    };
    div()
        .flex()
        .flex_col()
        .gap(theme.space.sm)
        .child(Text::sentence_label(GIT))
        .child(body)
        .into_any_element()
}

/// `Inspect I`: how to get the facts that are missing.
fn inspect_button() -> impl IntoElement {
    Button::new("detail-inspect", label(&worktrees::Inspect))
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact)
        .action(Box::new(worktrees::Inspect))
        .harness_target("detail.inspect")
}

fn known_git(known: &KnownGit, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let fact = |label: SharedString, value: AnyElement| {
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .child(
                div()
                    .flex_none()
                    .w(theme.metrics.fact_label_w)
                    .child(Text::caption(label).muted()),
            )
            .child(div().flex().items_center().flex_1().min_w_0().child(value))
            .into_any_element()
    };
    let changes = Text::caption(known.changes.clone())
        .tone(if known.dirty {
            Tone::Warning
        } else {
            Tone::Default
        })
        .into_any_element();
    let ahead_behind = match known.ahead_behind.clone() {
        Some(text) => Text::data_small(text).into_any_element(),
        None => Text::caption(NULL).faint().into_any_element(),
    };
    let pr = known.pr.as_ref().map(|pr| {
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .min_w_0()
            .child(
                Button::new("detail-pr-link", pr.link.clone())
                    .style(ButtonStyle::Ghost)
                    .size(ButtonSize::Compact)
                    .action(Box::new(hub::OpenInBrowser)),
            )
            .children(pr.state.map(|state| PrBadge::state_only(state).chip()))
            .into_any_element()
    });
    let mut rows = vec![
        fact(SharedString::new_static("Changes"), changes),
        fact(known.versus.clone(), ahead_behind),
        fact(
            SharedString::new_static("Published"),
            Text::caption(known.published.clone()).into_any_element(),
        ),
    ];
    if let Some(pr) = pr {
        rows.push(fact(SharedString::new_static("Pull request"), pr));
    }
    rows.extend(
        known
            .warnings
            .iter()
            .map(|warning| FactRow::warning(warning.clone()).into_any_element()),
    );
    div()
        .flex()
        .flex_col()
        .gap(theme.space.sm)
        // §3.4: a running inspection dims the previous values, it never blanks them.
        .when(known.refreshing, |el| {
            el.opacity(theme.metrics.refreshing_opacity)
        })
        .children(rows)
        .into_any_element()
}

/// The Location section: the path with its copy button, then when it was made and checked.
fn location(detail: &WorktreeDetail, age_offset: i64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let checked = match &detail.git {
        GitFacts::Known(known) => known.checked_age,
        _ => None,
    };
    let stamp = [
        detail
            .created_age
            .map(|age| format!("Created {} ago", format_age(age.saturating_add(age_offset)))),
        checked.map(|age| {
            format!(
                "safety checked {} ago",
                format_age(age.saturating_add(age_offset))
            )
        }),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" \u{b7} ");
    div()
        .flex()
        .flex_col()
        .gap(theme.space.sm)
        .child(Text::sentence_label(LOCATION))
        .child(
            CopyField::new(detail.path.clone()).button(
                IconButton::new("detail-copy-path", Icon::Copy, label(&worktrees::CopyPath))
                    .size(ButtonSize::Compact)
                    .action(Box::new(worktrees::CopyPath))
                    .harness_target("detail.copy_path"),
            ),
        )
        .when(!stamp.is_empty(), |el| {
            el.child(Text::caption(stamp).muted())
        })
        .into_any_element()
}
