use super::*;

/// Which text surface the shared buffer is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CardEdit {
    /// The card's title.
    Title,
    /// The markdown description.
    Description,
    /// A new comment.
    Comment,
}

impl CardEdit {
    /// What the footer calls this edit.
    #[must_use]
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::Comment => "comment",
        }
    }
}

/// Selected card and pending text edit for the detail surface.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardDetailState {
    /// Card selected when the dialog opened.
    pub(crate) card_id: Option<CardId>,
    /// Scroll position of the prose pane.
    pub(super) scroll: gpui::ScrollHandle,
    /// Selected row in the property pane.
    pub(super) property_row: usize,
    /// Pending title, description or comment text.
    pub(super) area: TextAreaState,
    /// Pixel scroll of the text editor, which owns what `area.scroll_row` cannot: how tall the
    /// box made the lines it wrapped.
    pub(super) area_scroll: gpui::ScrollHandle,
    /// Which surface `area` belongs to, when one is being edited.
    pub(super) edit: Option<CardEdit>,
    /// Revision guarding asynchronous save replies against a newer edit.
    pub(super) revision: u64,
    /// Revision currently being saved.
    pub(super) saving: Option<u64>,
    /// The last refusal, kept until the next successful edit.
    pub(crate) error: Option<String>,
}

impl CardDetailState {
    /// Whether a text surface currently owns the keyboard.
    #[must_use]
    pub(super) const fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// Starts an edit on `surface`, seeded with `text`.
    ///
    /// `comment_item` is the comment editor's index among the left pane's children, which the
    /// optional conflict banner shifts: scrolling to a fixed index lands on Activity instead
    /// of the editor on every card that is not conflicted, which is nearly all of them.
    pub(super) fn begin(&mut self, surface: CardEdit, text: String, comment_item: usize) {
        self.area = TextAreaState::from_text(text);
        self.area_scroll = gpui::ScrollHandle::new();
        self.area
            .reveal_cursor(if surface == CardEdit::Comment { 4 } else { 8 });
        self.scroll.scroll_to_item(if surface == CardEdit::Comment {
            comment_item
        } else {
            0
        });
        self.revision = self.revision.wrapping_add(1);
        self.edit = Some(surface);
        self.error = None;
    }

    /// Throws the buffer away.
    pub(super) fn cancel(&mut self) {
        self.edit = None;
        self.area.clear();
        self.revision = self.revision.wrapping_add(1);
        self.saving = None;
    }

    /// Completes only the save that still belongs to this draft; failures retain its text.
    pub(super) fn finish_save(&mut self, revision: u64, error: Option<String>) {
        if self.saving != Some(revision) {
            return;
        }
        self.saving = None;
        if error.is_none() && self.revision == revision {
            self.cancel();
        }
        self.error = error;
    }

    /// Runs `edit` against the buffer, keeping the caret in range.
    pub(super) fn apply(&mut self, edit: impl FnOnce(&mut TextAreaState)) {
        edit(&mut self.area);
        self.area
            .reveal_cursor(if self.edit == Some(CardEdit::Comment) {
                4
            } else {
                8
            });
        self.revision = self.revision.wrapping_add(1);
    }
}

/// The card this dialog is showing, resolved against the loaded board.
#[must_use]
pub(super) fn card<'a>(state: &'a AppState, draft: &CardDetailState) -> Option<&'a Card> {
    let view = state.board()?;
    let id = draft.card_id.as_ref()?;
    view.cards.iter().find(|card| &card.id == id)
}

/// Types into the open buffer. Returns whether there was one to type into.
pub(super) fn type_text(state: &Entity<AppState>, text: &str, cx: &mut App) -> bool {
    let typed = with_host(state, cx, |host| {
        if !host.card_detail.is_editing() {
            return false;
        }
        host.card_detail.apply(|area| area.insert(text));
        true
    });
    if typed {
        notify(state, cx);
        cx.stop_propagation();
    }
    typed
}

/// Runs an edit against the open buffer. Returns whether there was one.
pub(super) fn edit_buffer(
    state: &Entity<AppState>,
    cx: &mut App,
    edit: impl FnOnce(&mut TextAreaState),
) -> bool {
    let edited = with_host(state, cx, |host| {
        if !host.card_detail.is_editing() {
            return false;
        }
        host.card_detail.apply(edit);
        true
    });
    if edited {
        notify(state, cx);
        cx.stop_propagation();
    }
    edited
}

/// Opens a text surface.
pub(super) fn begin(state: &Entity<AppState>, surface: CardEdit, text: String, cx: &mut App) {
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    // `render` builds the left pane as: conflict banner (only when there is one), title,
    // description, comments, comment editor.
    let comment_item =
        3 + usize::from(card(state.read(cx), &draft).is_some_and(|card| card.conflict.is_some()));
    with_host(state, cx, |host| {
        host.card_detail.begin(surface, text, comment_item);
    });
    notify(state, cx);
    cx.stop_propagation();
}

/// Moves the property selection.
pub(super) fn move_row(state: &Entity<AppState>, delta: isize, len: usize, cx: &mut App) {
    with_host(state, cx, |host| {
        host.card_detail.property_row =
            crate::dialogs::step(host.card_detail.property_row, delta, len);
    });
    notify(state, cx);
    cx.stop_propagation();
}
