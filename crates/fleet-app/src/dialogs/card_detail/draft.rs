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
    /// Which surface the host's one live input belongs to, when one is being edited.
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
    pub(crate) const fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// Starts an edit on `surface`, seeded with `text`.
    ///
    /// `comment_item` is the comment editor's index among the left pane's children, which the
    /// optional conflict banner shifts: scrolling to a fixed index lands on Activity instead
    /// of the editor on every card that is not conflicted, which is nearly all of them.
    pub(super) fn begin(&mut self, surface: CardEdit, comment_item: usize) {
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
}

/// The card this dialog is showing, resolved against the loaded board.
#[must_use]
pub(super) fn card<'a>(state: &'a AppState, draft: &CardDetailState) -> Option<&'a Card> {
    let view = state.board()?;
    let id = draft.card_id.as_ref()?;
    view.cards.iter().find(|card| &card.id == id)
}

/// Opens a text surface.
pub(super) fn begin(
    state: &Entity<AppState>,
    surface: CardEdit,
    text: String,
    window: &mut Window,
    cx: &mut App,
) {
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    // `render` builds the left pane as: conflict banner (only when there is one), title,
    // description, comments, comment editor.
    let comment_item =
        3 + usize::from(card(state.read(cx), &draft).is_some_and(|card| card.conflict.is_some()));
    let input = cx.new(|cx| {
        let mode = match surface {
            CardEdit::Title => InputMode::SingleLine,
            CardEdit::Description => InputMode::Multiline {
                min_rows: 8,
                max_rows: 8,
            },
            CardEdit::Comment => InputMode::Multiline {
                min_rows: 4,
                max_rows: 4,
            },
        };
        let mut input = TextInput::new(mode, cx);
        input.set_label(
            Some(
                match surface {
                    CardEdit::Title => "Title",
                    CardEdit::Description => "Description",
                    CardEdit::Comment => "New comment",
                }
                .into(),
            ),
            cx,
        );
        if surface != CardEdit::Title {
            input.set_placeholder("Markdown.", cx);
        }
        input.set_text(text, cx);
        input
    });
    let host = crate::dialogs::host::host_for(state, cx);
    let weak_host = host.downgrade();
    let subscription = cx.subscribe(&input, move |_, event, cx| {
        if matches!(event, TextInputEvent::Changed)
            && let Some(host) = weak_host.upgrade()
        {
            host.update(cx, |host, cx| {
                host.card_detail.revision = host.card_detail.revision.wrapping_add(1);
                host.card_detail.error = None;
                cx.notify();
            });
        }
    });
    host.update(cx, |host, _| {
        host.card_detail.begin(surface, comment_item);
        host.card_detail_input = Some(input.clone());
        host.card_detail_input_subscription = Some(subscription);
    });
    input.update(cx, |input, cx| input.focus(window, cx));
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
