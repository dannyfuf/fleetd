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

use gpui::{App, ElementId, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*};

use crate::{
    components::{Chip, PriorityGlyph, PriorityLevel, StatusDot},
    focus::FocusRing,
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
    tone::Tone,
};

/// How many lines of the title a tile shows before it clips.
pub const CARD_TITLE_LINES: usize = 2;

/// How many characters an assignee chip shows.
pub const ASSIGNEE_INITIALS: usize = 2;

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

    /// Mouse parity for `enter`: open the card.
    pub fn on_click(
        mut self,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(on_click));
        self
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
        let theme = cx.theme().clone();
        let selected = self.selected;
        let focused = self.focused;
        let has_meta = self.has_meta();
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
            .child(Text::data_small(self.key).faint())
            .child(
                // The title is the only thing on a tile allowed two lines; anything longer is
                // a description, and a board that grows its cards stops being scannable.
                styled_with(div(), TextRole::UiStrong.style(&theme), &theme)
                    .w_full()
                    .min_w_0()
                    .line_clamp(CARD_TITLE_LINES)
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
            .border_1()
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
            .child(FocusRing::cursor_row(focused).child(body))
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
