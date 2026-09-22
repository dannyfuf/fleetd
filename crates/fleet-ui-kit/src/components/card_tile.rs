//! `CardTile` — one card on the board.
//!
//! The tile is the list row of the kanban world, so it borrows the row's state vocabulary
//! verbatim (§3.3): `selected` paints `row_selected`, `focused` draws the 2 px cursor bar
//! through [`crate::focus::FocusRing`], and hover is pointer feedback that never survives a
//! selection. A board and a list therefore express "where am I" with the same two tokens, and
//! moving the cursor from a list into a column teaches the eye nothing new.
//!
//! Everything below the title is **zero-suppressed** (§1.2): an unset priority, an empty label
//! set, a card with no assignee and a clean worktree all cost zero pixels. A tile with only a
//! key and a title is one line taller than the title itself.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ElementId, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*,
};

use crate::{
    components::{Chip, PriorityGlyph, PriorityLevel, Spinner, StatusDot},
    focus::FocusRing,
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
    tone::Tone,
};

/// How many lines of the title a tile wraps to before the last one ends in an ellipsis.
pub const CARD_TITLE_LINES: usize = 2;

/// How many characters an assignee chip shows.
pub const ASSIGNEE_INITIALS: usize = 2;

/// The glyph in front of the number of cards a tile is waiting on.
const BLOCKED_GLYPH: &str = "⊘";

/// The [`Tone`] a board color token names.
///
/// Backends describe a label's color by **token name** (`"accent"`, `"danger"`), never by a hex
/// string, so a remote board cannot smuggle a color into a Fleet surface. An unknown or absent
/// name is [`Tone::Secondary`], the neutral chip.
pub fn label_tone(token: Option<&str>) -> Tone {
    match token.unwrap_or_default() {
        "accent" => Tone::Accent,
        "info" => Tone::Info,
        "success" => Tone::Success,
        "warning" => Tone::Warning,
        "danger" => Tone::Danger,
        "muted" => Tone::Muted,
        "text" | "default" => Tone::Default,
        _ => Tone::Secondary,
    }
}

/// The up-to-[`ASSIGNEE_INITIALS`] uppercase initials of a person's name or handle.
///
/// `"Danny Fuentes"` and `"danny.fuentes"` and `"danny-fuentes@fleet.dev"` all give `DF`;
/// a single-word handle gives its first letter. Non-alphanumeric characters are separators, so
/// an email address never shows its domain.
pub fn initials(name: &str) -> String {
    name.split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .take(ASSIGNEE_INITIALS)
        .filter_map(|part| part.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

/// What a card's run is doing, as the tile says it.
///
/// A kit vocabulary, not a domain type: the app folds a run's state, its child's state and how
/// long it has waited into one of these five marks, and the tile only draws it. The two amber
/// marks are deliberately the same glyph — [`RunMark::Stalled`] and [`RunMark::NeedsYou`] both
/// mean "this card wants you", and a tile is not the surface that explains which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunMark {
    /// Queued behind the board's live-run limit.
    Pending,
    /// Queued long enough that the wait itself is worth noticing.
    Stalled,
    /// A child is working on the card right now.
    Working,
    /// The run stopped short of done and wants the user.
    NeedsYou,
    /// The run finished and the card is still here.
    Succeeded,
}

impl RunMark {
    /// Every mark, in the order a gallery shows them.
    pub const ALL: [RunMark; 5] = [
        RunMark::Pending,
        RunMark::Stalled,
        RunMark::Working,
        RunMark::NeedsYou,
        RunMark::Succeeded,
    ];
}

/// How loudly a tile states the cards it is waiting on.
///
/// `Muted` is the ordinary case — the blockers are simply not done yet. `Warning` is a wait that
/// will not end on its own, so the eye should stop on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedTone {
    /// Secondary contrast: an ordinary, still-moving wait.
    Muted,
    /// Amber: the wait needs a person.
    Warning,
}

impl BlockedTone {
    /// Both tones, in the order a gallery shows them.
    pub const ALL: [BlockedTone; 2] = [BlockedTone::Muted, BlockedTone::Warning];
}

/// What the right end of the key line carries. A run mark wins over a blocked count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyMark {
    Run(RunMark),
    Blocked(u32, BlockedTone),
}

/// One kanban card.
#[derive(IntoElement)]
pub struct CardTile {
    id: ElementId,
    key: SharedString,
    title: SharedString,
    priority: PriorityLevel,
    labels: Vec<(SharedString, Option<SharedString>)>,
    assignee: Option<SharedString>,
    estimate: Option<u32>,
    due: Option<SharedString>,
    worktree: bool,
    dirty: bool,
    conflict: bool,
    selected: bool,
    focused: bool,
    extras: Vec<SharedString>,
    run: Option<RunMark>,
    blocked: Option<(u32, BlockedTone)>,
    #[allow(clippy::type_complexity)]
    on_click: Option<Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>>,
}

impl CardTile {
    /// A tile for `key` (`FLT-12`) titled `title`.
    pub fn new(
        id: impl Into<ElementId>,
        key: impl Into<SharedString>,
        title: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            key: key.into(),
            title: title.into(),
            priority: PriorityLevel::None,
            labels: Vec::new(),
            assignee: None,
            estimate: None,
            due: None,
            worktree: false,
            dirty: false,
            conflict: false,
            selected: false,
            focused: false,
            extras: Vec::new(),
            run: None,
            blocked: None,
            on_click: None,
        }
    }

    /// The priority mark. [`PriorityLevel::None`] draws nothing on a tile.
    pub fn priority(mut self, priority: PriorityLevel) -> Self {
        self.priority = priority;
        self
    }

    /// The label chips: `(name, color token)`. See [`label_tone`] for the token names.
    pub fn labels(mut self, labels: Vec<(SharedString, Option<SharedString>)>) -> Self {
        self.labels = labels;
        self
    }

    /// The assignee, shown as an initials chip.
    pub fn assignee(mut self, assignee: Option<SharedString>) -> Self {
        self.assignee = assignee;
        self
    }

    /// The estimate, in points.
    pub fn estimate(mut self, estimate: Option<u32>) -> Self {
        self.estimate = estimate;
        self
    }

    /// The due date, already formatted by the caller (`2026-03-04`, `Mar 4`).
    pub fn due(mut self, due: Option<SharedString>) -> Self {
        self.due = due;
        self
    }

    /// Whether the card has a worktree: a branch glyph in the meta row.
    pub fn worktree(mut self, worktree: bool) -> Self {
        self.worktree = worktree;
        self
    }

    /// Whether the card has local edits not yet pushed to its remote: an amber dot.
    pub fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }

    /// Whether the card is in sync conflict with its backend: a red dot.
    pub fn conflict(mut self, conflict: bool) -> Self {
        self.conflict = conflict;
        self
    }

    /// Paint the selection background: this is the board's current card.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Draw the 2 px cursor bar: the column holding this card has focus.
    ///
    /// Selection and focus are separate flags for the same reason they are on
    /// [`super::Row`]: a board keeps its selected card while the keyboard is in a dialog, and
    /// the card then keeps the background and loses the bar.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Extra values from the board's `show_on_card` custom properties, in schema order.
    pub fn extras(mut self, extras: Vec<SharedString>) -> Self {
        self.extras = extras;
        self
    }

    /// The state of the card's run, drawn at the right end of the key line.
    ///
    /// Wins over [`Self::blocked`]: a card that is already running has nothing left to wait for.
    pub fn run(mut self, mark: RunMark) -> Self {
        self.run = Some(mark);
        self
    }

    /// How many unsatisfied cards this one waits on, and how loudly to say so.
    ///
    /// Drawn as `⊘ n` at the right end of the key line. A count of zero draws nothing, like
    /// every other meta slot (§1.2).
    pub fn blocked(mut self, count: u32, tone: BlockedTone) -> Self {
        self.blocked = Some((count, tone));
        self
    }

    /// Mouse parity for `enter`: open the card.
    pub fn on_click(
        mut self,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(on_click));
        self
    }

    /// What the right end of the key line shows, if anything.
    fn key_mark(&self) -> Option<KeyMark> {
        self.run.map(KeyMark::Run).or_else(|| {
            self.blocked
                .filter(|(count, _)| *count > 0)
                .map(|(count, tone)| KeyMark::Blocked(count, tone))
        })
    }

    /// Whether the meta row has anything in it. Zero-suppression, §1.2.
    fn has_meta(&self) -> bool {
        self.priority != PriorityLevel::None
            || !self.labels.is_empty()
            || self.assignee.is_some()
            || self.estimate.is_some()
            || self.due.is_some()
            || self.worktree
            || self.dirty
            || self.conflict
            || !self.extras.is_empty()
    }
}

impl RenderOnce for CardTile {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.selected;
        let focused = self.focused;
        let has_meta = self.has_meta();
        let mark: Option<AnyElement> = self.key_mark().map(|mark| match mark {
            KeyMark::Run(RunMark::Pending | RunMark::Working) => Spinner::new(
                ElementId::NamedChild(Arc::new(self.id.clone()), SharedString::new_static("run")),
            )
            .size(IconSize::Small)
            .tone(Tone::Secondary)
            .into_any_element(),
            KeyMark::Run(RunMark::Stalled | RunMark::NeedsYou) => {
                StatusDot::small(Tone::Warning).into_any_element()
            }
            KeyMark::Run(RunMark::Succeeded) => Icon::Check
                .el()
                .size(IconSize::Small)
                .tone(Tone::Muted)
                .into_any_element(),
            KeyMark::Blocked(count, tone) => {
                let text = Text::data_small(format!("{BLOCKED_GLYPH} {count}"));
                match tone {
                    BlockedTone::Muted => text.muted(),
                    BlockedTone::Warning => text.tone(Tone::Warning),
                }
                .into_any_element()
            }
        });
        let hover_bg = theme.colors.row_hover;
        let on_click = self.on_click;

        let meta = has_meta.then(|| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .w_full()
                .gap(theme.space.xs)
                .children(
                    (self.priority != PriorityLevel::None)
                        .then(|| PriorityGlyph::new(self.priority)),
                )
                .children(self.labels.into_iter().map(|(name, token)| {
                    Chip::new()
                        .text(name)
                        .tone(label_tone(token.as_deref()))
                        .filled(true)
                }))
                .children(
                    self.assignee
                        .map(|assignee| Chip::new().text(initials(&assignee)).filled(true)),
                )
                .children(
                    self.estimate
                        .map(|estimate| Text::data_small(format!("{estimate} pt")).faint()),
                )
                .children(self.due.map(|due| {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(theme.space.xxs)
                        .child(
                            Icon::Clock
                                .el()
                                .size(IconSize::Small)
                                .color(theme.colors.text_muted),
                        )
                        .child(Text::data_small(due).faint())
                }))
                .children(self.worktree.then(|| {
                    Icon::GitBranch
                        .el()
                        .size(IconSize::Small)
                        .color(theme.colors.text_secondary)
                }))
                .children(self.dirty.then(|| StatusDot::small(Tone::Warning)))
                .children(self.conflict.then(|| StatusDot::small(Tone::Danger)))
                .children(
                    self.extras
                        .into_iter()
                        .map(|extra| Text::data_small(extra).faint()),
                )
        });

        let body = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(theme.space.xxs)
            .p(theme.space.sm)
            // The key line is a row so a run mark or a blocked count can sit at its right end
            // without costing the tile a line: the key is short, the spacer is what pushes the
            // mark over, and the mark is no taller than the key text.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .gap(theme.space.xs)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Text::data_small(self.key).faint()),
                    )
                    .children(mark.map(|mark| div().flex_none().child(mark))),
            )
            .child(
                // The title is the only thing on a tile allowed two lines; anything longer is
                // a description, and a board that grows its cards stops being scannable. The
                // ellipsis is what tells the eye the clamp cut something: `line_clamp` alone
                // hides the tail mid-word, which reads as a title that simply ends there.
                styled_with(div(), TextRole::UiStrong.style(theme), theme)
                    .w_full()
                    .min_w_0()
                    .line_clamp(CARD_TITLE_LINES)
                    .text_ellipsis()
                    .text_color(theme.colors.text)
                    .child(self.title),
            )
            .children(meta);

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .w_full()
            .flex_none()
            .overflow_hidden()
            .rounded(theme.radii.sm)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .bg(if selected {
                theme.colors.row_selected
            } else {
                theme.colors.surface
            })
            .when(!selected, |el| el.hover(move |s| s.bg(hover_bg)))
            .when_some(on_click, |el, on_click| {
                el.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        on_click(event, window, cx)
                    })
            })
            .child(FocusRing::cursor_row(focused).content(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_the_first_two_parts() {
        assert_eq!(initials("Danny Fuentes"), "DF");
        assert_eq!(initials("danny.fuentes"), "DF");
        assert_eq!(initials("danny-fuentes@fleet.dev"), "DF");
        assert_eq!(initials("dannyfuf"), "D");
        assert_eq!(initials(""), "");
        assert_eq!(initials("   "), "");
    }

    #[test]
    fn initials_uppercase_multibyte_names() {
        assert_eq!(initials("ñandú ávila"), "ÑÁ");
    }

    #[test]
    fn label_tokens_map_onto_tones() {
        assert_eq!(label_tone(Some("danger")), Tone::Danger);
        assert_eq!(label_tone(Some("accent")), Tone::Accent);
        assert_eq!(label_tone(Some("nonsense")), Tone::Secondary);
        assert_eq!(label_tone(None), Tone::Secondary);
    }

    #[test]
    fn a_run_mark_wins_over_a_blocked_count() {
        let tile = CardTile::new("card", "FLT-1", "t")
            .blocked(2, BlockedTone::Muted)
            .run(RunMark::Working);
        assert_eq!(tile.key_mark(), Some(KeyMark::Run(RunMark::Working)));
    }

    #[test]
    fn a_blocked_count_of_zero_draws_nothing() {
        assert_eq!(CardTile::new("card", "FLT-1", "t").key_mark(), None);
        assert_eq!(
            CardTile::new("card", "FLT-1", "t")
                .blocked(0, BlockedTone::Warning)
                .key_mark(),
            None
        );
        assert_eq!(
            CardTile::new("card", "FLT-1", "t")
                .blocked(2, BlockedTone::Warning)
                .key_mark(),
            Some(KeyMark::Blocked(2, BlockedTone::Warning))
        );
    }

    #[test]
    fn a_mark_is_not_a_meta_row() {
        let tile = CardTile::new("card", "FLT-1", "t")
            .run(RunMark::NeedsYou)
            .blocked(3, BlockedTone::Warning);
        assert!(
            !tile.has_meta(),
            "the key line carries the mark, not the meta row"
        );
    }

    #[test]
    fn a_bare_tile_has_no_meta_row() {
        let bare = CardTile::new("card", "FLT-1", "Ship the board");
        assert!(!bare.has_meta());
        assert!(bare.priority(PriorityLevel::Low).has_meta());
        assert!(CardTile::new("card", "FLT-1", "t").dirty(true).has_meta());
        assert!(
            CardTile::new("card", "FLT-1", "t")
                .extras(vec!["QA".into()])
                .has_meta()
        );
    }
}
