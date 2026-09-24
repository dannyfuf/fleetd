//! `CardTile` — one card on the board.
//!
//! The tile is the list row of the kanban world, so it borrows the row's state vocabulary
//! verbatim (§3.3): `selected` paints `row_selected`, `focused` draws the 2 px cursor bar
//! through [`crate::focus::FocusRing`], and hover is pointer feedback that never survives a
//! selection. A board and a list therefore express "where am I" with the same two tokens, and
//! moving the cursor from a list into a column teaches the eye nothing new.
//!
//! Three lines, top to bottom: the **key line** (priority bars, the key, and at its right end the
//! one state pill the card has — its run, or what blocks it), the **title** (two lines at most),
//! and the **meta row** (labels, estimate, due date, the linked branch and pull request, the card's
//! [`CardTile::reference`], then the state's own button and the assignee's avatar at its right end). Everything but the key and the
//! title is **zero-suppressed** (§1.2): a tile with only a key and a title is two lines tall.
//!
//! **Pointer** (UX-SPEC §5.1). A press selects ([`CardTile::on_click`]), the second press of a
//! double-click opens ([`CardTile::on_double_click`]) and a right click opens the card's menu
//! ([`CardTile::on_secondary_click`]). [`CardTile::menu`] is the visible twin of that right-click
//! menu — normally a `⋯` [`super::PopoverMenu`] — drawn over the key line's right end while the
//! tile is hovered or selected. Hover lifts the tile: a `border_strong` hairline and
//! `theme.lift_shadow()`.
//!
//! The tile takes words, never domain types: what a run is called, which card blocks this one
//! and which branch it is on are folded by the caller and handed over as strings.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ElementId, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*,
};

use crate::{
    components::{Chip, PrBadge, PrBadgeState, PriorityGlyph, PriorityLevel, Spinner, StatusDot},
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

impl KeyMark {
    /// The pill's tone: its text colour and, through [`Tone::fill`], its ground.
    fn tone(self) -> Tone {
        match self {
            KeyMark::Run(RunMark::Pending | RunMark::Working) => Tone::Secondary,
            KeyMark::Run(RunMark::Stalled | RunMark::NeedsYou) => Tone::Warning,
            KeyMark::Run(RunMark::Succeeded) => Tone::Success,
            KeyMark::Blocked(_, BlockedTone::Muted) => Tone::Secondary,
            KeyMark::Blocked(_, BlockedTone::Warning) => Tone::Warning,
        }
    }

    /// What the pill says when the caller gave it no words of its own.
    fn default_label(self) -> SharedString {
        match self {
            KeyMark::Run(RunMark::Pending | RunMark::Stalled) => "waiting".into(),
            KeyMark::Run(RunMark::Working) => "working".into(),
            KeyMark::Run(RunMark::NeedsYou) => "needs you".into(),
            KeyMark::Run(RunMark::Succeeded) => "done".into(),
            KeyMark::Blocked(count, _) => format!("{BLOCKED_GLYPH} {count}").into(),
        }
    }
}

/// A pointer handler on a tile: it receives the raw press.
type PressHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

/// The hover group a tile names itself with, so its `⋯` can reveal itself while *its own* tile
/// is hovered. gpui resolves a group name to the innermost enclosing element that declared it.
const TILE_GROUP: &str = "fleet-card-tile";

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
    branch: Option<SharedString>,
    pr: Option<(u64, PrBadgeState)>,
    reference: Option<SharedString>,
    dirty: bool,
    conflict: bool,
    selected: bool,
    focused: bool,
    lifted: bool,
    left_behind: bool,
    extras: Vec<SharedString>,
    run: Option<RunMark>,
    run_label: Option<SharedString>,
    blocked: Option<(u32, BlockedTone)>,
    blocked_label: Option<SharedString>,
    action: Option<AnyElement>,
    menu: Option<AnyElement>,
    on_click: Option<PressHandler>,
    on_double_click: Option<PressHandler>,
    on_secondary_click: Option<PressHandler>,
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
            branch: None,
            pr: None,
            reference: None,
            dirty: false,
            conflict: false,
            selected: false,
            focused: false,
            lifted: false,
            left_behind: false,
            extras: Vec::new(),
            run: None,
            run_label: None,
            blocked: None,
            blocked_label: None,
            action: None,
            menu: None,
            on_click: None,
            on_double_click: None,
            on_secondary_click: None,
        }
    }

    /// The priority bars at the start of the key line. [`PriorityLevel::None`] draws nothing.
    pub fn priority(mut self, priority: PriorityLevel) -> Self {
        self.priority = priority;
        self
    }

    /// The label chips: `(name, color token)`. See [`label_tone`] for the token names.
    pub fn labels(mut self, labels: Vec<(SharedString, Option<SharedString>)>) -> Self {
        self.labels = labels;
        self
    }

    /// The assignee, shown as a round initials avatar at the right end of the meta row.
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

    /// Whether the card has a worktree: a branch glyph in the meta row. [`Self::branch`] names
    /// it; this alone is for a worktree whose branch the caller cannot name.
    pub fn worktree(mut self, worktree: bool) -> Self {
        self.worktree = worktree;
        self
    }

    /// The linked worktree's branch, drawn in mono after a branch glyph.
    pub fn branch(mut self, branch: Option<SharedString>) -> Self {
        self.branch = branch;
        self
    }

    /// The linked branch's pull request, as a [`PrBadge`] beside the branch.
    pub fn pr(mut self, pr: Option<(u64, PrBadgeState)>) -> Self {
        self.pr = pr;
        self
    }

    /// A short identifier the card points at (`acme/api#412`), drawn muted in mono in the meta
    /// row after the branch.
    ///
    /// The tile does not know what it names: the caller folds whatever external thing the card
    /// is about into one line. Unlike [`Self::pr`], which is the *branch's* pull request with its
    /// state, this is the card's own subject and carries no state of its own.
    pub fn reference(mut self, reference: Option<SharedString>) -> Self {
        self.reference = reference;
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

    /// Draw the tile as the one under the pointer mid-drag: an accent hairline, the popover
    /// shadow and a little transparency. This is the face a drag preview wears.
    pub fn lifted(mut self, lifted: bool) -> Self {
        self.lifted = lifted;
        self
    }

    /// Draw the tile as the place a dragged card left: faded, with a dashed hairline and no
    /// hover. It keeps its height, so the column does not reflow under the drag.
    pub fn left_behind(mut self, left_behind: bool) -> Self {
        self.left_behind = left_behind;
        self
    }

    /// Extra values from the board's `show_on_card` custom properties, in schema order.
    pub fn extras(mut self, extras: Vec<SharedString>) -> Self {
        self.extras = extras;
        self
    }

    /// The state of the card's run, drawn as a pill at the right end of the key line.
    ///
    /// Wins over [`Self::blocked`]: a card that is already running has nothing left to wait for.
    pub fn run(mut self, mark: RunMark) -> Self {
        self.run = Some(mark);
        self
    }

    /// The words of the run pill (`working 4m · codex`, `review passed`). Without them the pill
    /// says the mark's own word.
    pub fn run_label(mut self, label: impl Into<SharedString>) -> Self {
        self.run_label = Some(label.into());
        self
    }

    /// How many unsatisfied cards this one waits on, and how loudly to say so.
    ///
    /// Drawn as a pill at the right end of the key line. A count of zero draws nothing, like
    /// every other meta slot (§1.2).
    pub fn blocked(mut self, count: u32, tone: BlockedTone) -> Self {
        self.blocked = Some((count, tone));
        self
    }

    /// The words of the blocked pill (`blocked by FLT-5`). Without them it reads `⊘ n`.
    pub fn blocked_label(mut self, label: impl Into<SharedString>) -> Self {
        self.blocked_label = Some(label.into());
        self
    }

    /// The one button the card's state asks for — `Answer` on a card whose run needs you —
    /// drawn at the right end of the meta row, before the avatar.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// The `⋯` trigger of the card's menu, drawn over the key line's right end while the tile
    /// is hovered or selected. On a selected tile the key line makes room for it, so it never
    /// covers the state pill the keyboard reader is looking at. It must open the same menu [`Self::on_secondary_click`] does.
    pub fn menu(mut self, menu: impl IntoElement) -> Self {
        self.menu = Some(menu.into_any_element());
        self
    }

    /// A single press of the primary button: **select** this card (UX-SPEC §5.1).
    ///
    /// Fires on the press, so the card is already selected when a double-click's second press
    /// arrives, and when a `⋯` or a button inside the tile is clicked.
    pub fn on_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// The second press of a double-click: **open** this card, the pointer twin of `⏎`.
    pub fn on_double_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_double_click = Some(Box::new(handler));
        self
    }

    /// A right click: the handler normally selects the card; the caller wraps the tile in a
    /// [`super::ContextMenu`] holding every card action.
    pub fn on_secondary_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_secondary_click = Some(Box::new(handler));
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
        !self.labels.is_empty()
            || self.assignee.is_some()
            || self.estimate.is_some()
            || self.due.is_some()
            || self.worktree
            || self.branch.is_some()
            || self.pr.is_some()
            || self.reference.is_some()
            || self.dirty
            || self.conflict
            || !self.extras.is_empty()
            || self.action.is_some()
    }
}

impl RenderOnce for CardTile {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        // A tile mid-drag — the preview or the place it left — is not the cursor: the cursor
        // is the card itself, which is neither of the two while it is in the air.
        let moving = self.lifted || self.left_behind;
        let selected = self.selected && !moving;
        let focused = self.focused && !moving;
        let has_meta = self.has_meta();
        let key_mark = self.key_mark();
        let makes_room = selected && self.menu.is_some();
        let pill = key_mark.map(|mark| {
            let tone = mark.tone();
            let color = tone.color(theme);
            let label = match mark {
                KeyMark::Run(_) => self.run_label.clone(),
                KeyMark::Blocked(..) => self.blocked_label.clone(),
            }
            .unwrap_or_else(|| mark.default_label());
            let glyph: Option<AnyElement> = match mark {
                KeyMark::Run(RunMark::Pending | RunMark::Working) => Some(
                    Spinner::new(ElementId::NamedChild(
                        Arc::new(self.id.clone()),
                        SharedString::new_static("run"),
                    ))
                    .size(IconSize::Small)
                    .tone(tone)
                    .into_any_element(),
                ),
                KeyMark::Run(RunMark::Stalled | RunMark::NeedsYou) => {
                    Some(StatusDot::small(tone).into_any_element())
                }
                KeyMark::Run(RunMark::Succeeded) => Some(
                    Icon::Check
                        .el()
                        .size(IconSize::Small)
                        .color(color)
                        .into_any_element(),
                ),
                KeyMark::Blocked(..) => None,
            };
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.space.xxs)
                .h(theme.metrics.tile_chip_h)
                .px(theme.space.xs)
                .rounded(theme.radii.pill)
                .bg(tone.fill(theme))
                .children(glyph)
                .child(Text::caption(label).color(color))
        });
        let hover_border = theme.colors.border_strong;
        let lift = theme.lift_shadow();
        let (lifted, left_behind) = (self.lifted, self.left_behind);

        let unnamed_worktree = self.worktree && self.branch.is_none();
        let meta = has_meta.then(|| {
            let avatar = self
                .assignee
                .map(|assignee| super::avatar::Avatar::new(&assignee));
            let branch = self.branch.map(|branch| {
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(theme.space.xxs)
                    .child(
                        Icon::GitBranch
                            .el()
                            .size(IconSize::Small)
                            .color(theme.colors.text_muted),
                    )
                    .child(Text::data_small(branch).faint().ellipsize())
            });
            div()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .gap(theme.space.xs)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(theme.space.xs)
                        .children(self.labels.into_iter().map(|(name, token)| {
                            Chip::new()
                                .text(name)
                                .tone(label_tone(token.as_deref()))
                                .filled(true)
                        }))
                        .children(
                            self.estimate
                                .map(|estimate| Text::caption(format!("{estimate} pt")).faint()),
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
                        .children(branch)
                        .children(unnamed_worktree.then(|| {
                            Icon::GitBranch
                                .el()
                                .size(IconSize::Small)
                                .color(theme.colors.text_secondary)
                        }))
                        .children(self.pr.map(|(number, state)| PrBadge::new(number, state)))
                        .children(
                            self.reference
                                .map(|reference| Text::data_small(reference).faint().ellipsize()),
                        )
                        .children(self.dirty.then(|| StatusDot::small(Tone::Warning)))
                        .children(self.conflict.then(|| StatusDot::small(Tone::Danger)))
                        .children(
                            self.extras
                                .into_iter()
                                .map(|extra| Text::data_small(extra).faint()),
                        ),
                )
                .children(self.action.map(|action| div().flex_none().child(action)))
                .children(avatar)
        });

        let body = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(theme.space.xs)
            .px(theme.space.md)
            .py(theme.space.sm)
            // The key line is a row so the state pill can sit at its right end without costing
            // the tile a line: the key is short, the spacer is what pushes the pill over.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .gap(theme.space.sm)
                    // The `⋯` floats over this line's right end; a selected tile shows it for
                    // good, so the pill moves over rather than sit under it.
                    .when(makes_room, |el| el.pr(theme.metrics.button_h_compact))
                    .children(
                        (self.priority != PriorityLevel::None)
                            .then(|| PriorityGlyph::new(self.priority)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Text::data_small(self.key).faint()),
                    )
                    .children(pill),
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

        let menu = self.menu.map(|menu| {
            div()
                .absolute()
                .top(theme.space.xs)
                .right(theme.space.xs)
                .rounded(theme.radii.control)
                .bg(theme.colors.surface_raised)
                // Hidden, not removed, so revealing it never reflows the tile. The selected tile
                // keeps it showing, like a selected row keeps its hover actions — which is also
                // what keeps an open menu alive once the pointer moves into it, since pressing
                // the `⋯` selected the card first.
                .when(!selected, |el| {
                    el.invisible()
                        .group_hover(TILE_GROUP, |style| style.visible())
                })
                .child(menu)
        });
        let primary = (self.on_click.is_some() || self.on_double_click.is_some())
            .then_some((self.on_click, self.on_double_click));
        let pressable = primary.is_some() || self.on_secondary_click.is_some();

        div()
            .id(self.id)
            .group(TILE_GROUP)
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .flex_none()
            .rounded(theme.radii.card)
            .border(theme.metrics.hairline)
            .border_color(if lifted {
                theme.colors.accent
            } else {
                theme.colors.border
            })
            .bg(if selected {
                theme.colors.row_selected
            } else {
                theme.colors.surface_raised
            })
            .when(lifted, |el| {
                el.shadow(theme.popover_shadow())
                    .opacity(theme.metrics.drag_preview_opacity)
            })
            .when(left_behind, |el| {
                el.border_dashed().opacity(theme.metrics.dimmed_opacity)
            })
            // Hover lifts the tile: pointer feedback only, and never over a selection (§3.3).
            .when(!selected && !moving, |el| {
                el.hover(move |style| style.border_color(hover_border).shadow(lift.clone()))
            })
            .when(pressable, |el| el.cursor_pointer())
            .when_some(primary, |el, (on_click, on_double_click)| {
                el.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    if event.click_count >= 2
                        && let Some(open) = &on_double_click
                    {
                        open(event, window, cx);
                    } else if let Some(select) = &on_click {
                        select(event, window, cx);
                    }
                })
            })
            .when_some(self.on_secondary_click, |el, menu| {
                el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    menu(event, window, cx)
                })
            })
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .rounded(theme.radii.card)
                    .child(FocusRing::cursor_row(focused).content(body)),
            )
            .children(menu)
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
    fn a_reference_alone_draws_the_meta_row() {
        assert!(
            !CardTile::new("card", "FLT-1", "t")
                .reference(None)
                .has_meta()
        );
        assert!(
            CardTile::new("card", "FLT-1", "t")
                .reference(Some("acme/api#412".into()))
                .has_meta()
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
        assert!(
            !CardTile::new("card", "FLT-1", "t")
                .priority(PriorityLevel::Low)
                .has_meta(),
            "the priority bars live on the key line"
        );
        assert!(
            CardTile::new("card", "FLT-1", "t")
                .branch(Some("flt-1-login".into()))
                .has_meta()
        );
        assert!(CardTile::new("card", "FLT-1", "t").dirty(true).has_meta());
        assert!(
            CardTile::new("card", "FLT-1", "t")
                .extras(vec!["QA".into()])
                .has_meta()
        );
    }
}
