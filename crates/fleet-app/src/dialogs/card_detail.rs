//! Card detail (BOARD §8) — *the card as prose on the left, as facts on the right.*
//!
//! Three text surfaces (title, description, one new comment) share a single buffer, because at
//! most one of them is ever being edited: `i`, `d` and `c` open it, `ctrl-s` saves it and `Esc`
//! throws it away. While it is open the bare letters type — `d` in a description is the letter
//! `d`, not "edit description" — which is the same rule §3.8.6 applies to its text rows.
//!
//! Nothing here is optimistic. Every save sends its request and waits: the reducer applies the
//! `Card` the daemon returns, and a refusal becomes a sticky line at the top of the dialog
//! rather than a change the user believes happened.

use fleet_core::{
    board::{Card, CardPatch, ConflictResolution},
    ids::CardId,
};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div, px};

use crate::{
    actions::{card_detail as card_actions, dialog},
    bridge::Bridge,
    dialogs::{Dialogs, notify, now_epoch, root, typed_char, with_host},
    screens::board,
    state::AppState,
    views::board_card_detail::{self as detail, PropertyTarget},
};

/// The width of the right-hand property pane.
const PROPERTIES_WIDTH: f32 = 260.0;
/// The dialog's fixed height: the left pane scrolls inside it.
const DETAIL_HEIGHT: f32 = 620.0;

/// Which text surface the shared buffer is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardEdit {
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
    pub const fn label(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::Comment => "comment",
        }
    }
}

/// Selected card and pending text edit for the detail surface.
#[derive(Debug, Clone, Default)]
pub struct CardDetailState {
    /// Card selected when the dialog opened.
    pub card_id: Option<CardId>,
    /// Scroll position of the prose pane.
    pub scroll: gpui::ScrollHandle,
    /// Selected row in the property pane.
    pub property_row: usize,
    /// Pending title, description or comment text.
    pub area: TextAreaState,
    /// Pixel scroll of the text editor, which owns what `area.scroll_row` cannot: how tall the
    /// box made the lines it wrapped.
    pub area_scroll: gpui::ScrollHandle,
    /// Which surface `area` belongs to, when one is being edited.
    pub edit: Option<CardEdit>,
    /// Revision guarding asynchronous save replies against a newer edit.
    pub revision: u64,
    /// Revision currently being saved.
    pub saving: Option<u64>,
    /// The last refusal, kept until the next successful edit.
    pub error: Option<String>,
}

impl CardDetailState {
    /// Whether a text surface currently owns the keyboard.
    #[must_use]
    pub const fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// Starts an edit on `surface`, seeded with `text`.
    ///
    /// `comment_item` is the comment editor's index among the left pane's children, which the
    /// optional conflict banner shifts: scrolling to a fixed index lands on Activity instead
    /// of the editor on every card that is not conflicted, which is nearly all of them.
    pub fn begin(&mut self, surface: CardEdit, text: String, comment_item: usize) {
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
    pub fn cancel(&mut self) {
        self.edit = None;
        self.area.clear();
        self.revision = self.revision.wrapping_add(1);
        self.saving = None;
    }

    /// Completes only the save that still belongs to this draft; failures retain its text.
    pub fn finish_save(&mut self, revision: u64, error: Option<String>) {
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
    pub fn apply(&mut self, edit: impl FnOnce(&mut TextAreaState)) {
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
pub fn card<'a>(state: &'a AppState, draft: &CardDetailState) -> Option<&'a Card> {
    let view = state.board()?;
    let id = draft.card_id.as_ref()?;
    view.cards.iter().find(|card| &card.id == id)
}

pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let card_id = crate::screens::board::selected_card(state.read(cx)).map(|card| card.id.clone());
    with_host(cx, |host| {
        host.card_detail = CardDetailState {
            card_id,
            revision: host.card_detail.revision.wrapping_add(1),
            ..Default::default()
        }
    });
}

/// Renders the two panes and wires every §8 key.
pub fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut draft = with_host(cx, |host| host.card_detail.clone());
    let Some((board, card)) = state.read(cx).board().and_then(|view| {
        card(state.read(cx), &draft).map(|card| (view.board.clone(), card.clone()))
    }) else {
        return missing(state, focus);
    };
    let (board, card) = (&board, &card);
    let now = now_epoch();
    let theme = cx.theme().clone();
    // Borrowed, never cloned: this runs on every frame, and the card set of a real board
    // carries every comment and activity entry on it.
    let rows = state.read(cx).board().map_or_else(Vec::new, |view| {
        detail::property_rows(&view.board, &view.cards, card, now)
    });
    draft.property_row = draft.property_row.min(rows.len().saturating_sub(1));
    with_host(cx, |host| {
        host.card_detail.property_row = draft.property_row
    });

    let description: AnyElement = if draft.edit == Some(CardEdit::Description) {
        TextArea::new(draft.area.text().to_owned())
            .label("Description")
            .placeholder("Markdown.")
            .rows(8)
            .max_rows(8)
            .scroll_row(draft.area.scroll_row())
            .scroll("card-detail-description-scroll", draft.area_scroll.clone())
            .cursor(draft.area.cursor())
            .focused(true)
            .into_any_element()
    } else if card.description.trim().is_empty() {
        Text::ui("No description \u{2014} d writes one.")
            .faint()
            .into_any_element()
    } else {
        MarkdownText::new(card.description.clone()).into_any_element()
    };

    let title: AnyElement = if draft.edit == Some(CardEdit::Title) {
        TextField::new(draft.area.text().to_owned())
            .label("Title")
            .caret(
                draft.area.text()[..draft.area.cursor().min(draft.area.text().len())]
                    .chars()
                    .count(),
            )
            .focused(true)
            .into_any_element()
    } else {
        detail::title_line(board, card, cx)
    };

    let comment_editor = (draft.edit == Some(CardEdit::Comment)).then(|| {
        TextArea::new(draft.area.text().to_owned())
            .label("New comment")
            .placeholder("Markdown.")
            .rows(4)
            .max_rows(4)
            .scroll_row(draft.area.scroll_row())
            .scroll("card-detail-comment-scroll", draft.area_scroll.clone())
            .cursor(draft.area.cursor())
            .focused(true)
    });

    let left = div()
        .id("card-detail-left")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .gap(theme.space.md)
        .pr(theme.space.md)
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .children(detail::conflict_banner(card))
        .child(title)
        .child(description)
        .child(detail::comments(card, now, cx))
        .children(comment_editor)
        .child(detail::activity(card, now, cx));

    let properties = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(PROPERTIES_WIDTH))
        .h_full()
        .gap(theme.space.xxs)
        .border_l(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .pl(theme.space.md)
        .child(SectionHeader::new("Properties"))
        .children(rows.iter().enumerate().map(|(index, row)| {
            detail::property_row(
                row,
                index == draft.property_row,
                !draft.is_editing(),
                &theme,
            )
        }));

    let body = div()
        .flex()
        .flex_row()
        .size_full()
        .min_h_0()
        .child(left)
        .child(properties);

    let hints = if let Some(surface) = draft.edit {
        KeyHintRow::new()
            .key("\u{2303}s", format!("save {}", surface.label()))
            .key("esc", "discard")
    } else {
        KeyHintRow::new()
            .key("i/d/c", "title/desc/comment")
            .key("j/k", "property")
            .key("\u{23ce}", "edit")
            .key("w", "worktree")
            // `x` is otherwise nowhere on this surface: the conflict banner names `K`/`R` when
            // there is a conflict, but nothing ever names the key that opens the issue.
            .key("x", "remote")
            .key("esc", "close")
    };

    let mut dialog_card = Dialog::new(Dialogs::CardDetail.title())
        .icon(Dialogs::CardDetail.icon())
        .width(Dialogs::CardDetail.width())
        .height(px(DETAIL_HEIGHT))
        .subtitle(format!("\u{00b7} {}", card.display_key(board)))
        .body(body)
        .hint_row(hints);
    if let Some(message) = draft.error.clone() {
        dialog_card = dialog_card.error(message);
    }

    let rows_len = rows.len();
    let targets: Vec<detail::PropertyRow> = rows;

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let Some(text) = typed_char(event) else {
                    return;
                };
                if type_text(&state, &text, cx) {
                    cx.stop_propagation();
                }
            }
        })
        .on_action({
            let state = state.clone();
            let title = card.title.clone();
            move |_: &card_actions::EditTitle, _window, cx| {
                if type_text(&state, "i", cx) {
                    return;
                }
                begin(&state, CardEdit::Title, title.clone(), cx);
            }
        })
        .on_action({
            let state = state.clone();
            let description = card.description.clone();
            move |_: &card_actions::EditDescription, _window, cx| {
                if type_text(&state, "d", cx) {
                    return;
                }
                begin(&state, CardEdit::Description, description.clone(), cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::AddComment, _window, cx| {
                if type_text(&state, "c", cx) {
                    return;
                }
                begin(&state, CardEdit::Comment, String::new(), cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::NextProperty, _window, cx| {
                if type_text(&state, "j", cx) {
                    return;
                }
                move_row(&state, 1, rows_len, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::PrevProperty, _window, cx| {
                if type_text(&state, "k", cx) {
                    return;
                }
                move_row(&state, -1, rows_len, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _window, cx| {
                if edit_buffer(&state, cx, |area| {
                    area.move_down();
                }) {
                    return;
                }
                move_row(&state, 1, rows_len, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| {
                if edit_buffer(&state, cx, |area| {
                    area.move_up();
                }) {
                    return;
                }
                move_row(&state, -1, rows_len, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| {
                edit_buffer(&state, cx, TextAreaState::insert_tab);
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.move_left();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.move_right();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.backspace();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.delete_word_before();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                // DESIGN-SYSTEM §TextArea: `ctrl-u` clears the line, not the whole draft.
                edit_buffer(&state, cx, |area| {
                    area.delete_to_line_start();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.move_to_line_start();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.move_to_line_end();
                });
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::EditProperty, _window, cx| {
                enter(&state, &bridge, &targets, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::Save, _window, cx| commit_edit(&state, &bridge, cx)
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::CreateWorktree, _window, cx| {
                if type_text(&state, "w", cx) {
                    return;
                }
                start_worktree(&state, &bridge, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::OpenRemote, _window, cx| {
                // The guard belongs to the *key*, not to the command: this handler is what a
                // literal `x` reaches while a text edit is open, and the palette dispatches
                // the same action from a surface where nothing was typed. Held inside
                // `open_remote` it turned the palette's "Open remote issue" row into an `x`
                // inserted in the user's description, with no browser and no message.
                if type_text(&state, "x", cx) {
                    return;
                }
                open_remote(&state, &bridge, cx);
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::KeepLocal, _window, cx| {
                if type_text(&state, "K", cx) {
                    return;
                }
                resolve(&state, &bridge, ConflictResolution::KeepLocal, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::TakeRemote, _window, cx| {
                if type_text(&state, "R", cx) {
                    return;
                }
                resolve(&state, &bridge, ConflictResolution::TakeRemote, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::Close, _window, cx| {
                close(&state, &bridge, cx);
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &dialog::Cancel, _window, cx| {
                close(&state, &bridge, cx);
                cx.stop_propagation();
            }
        })
        .child(dialog_card)
        .into_any_element()
}

/// The card went away while the dialog was open — say so instead of showing an empty card.
fn missing(state: &Entity<AppState>, focus: &FocusHandle) -> AnyElement {
    let state = state.clone();
    root(focus)
        .on_action(move |_: &dialog::Cancel, _window, cx| {
            state.update(cx, |app, cx| {
                app.close_overlay();
                cx.notify();
            });
            cx.stop_propagation();
        })
        .child(
            Dialog::new(Dialogs::CardDetail.title())
                .icon(Dialogs::CardDetail.icon())
                .width(Dialogs::CardDetail.width())
                .body(EmptyState::new("That card is no longer on this board."))
                .hint_row(KeyHintRow::new().key("esc", "close")),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------- editing

/// Types into the open buffer. Returns whether there was one to type into.
fn type_text(state: &Entity<AppState>, text: &str, cx: &mut App) -> bool {
    let typed = with_host(cx, |host| {
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
fn edit_buffer(
    state: &Entity<AppState>,
    cx: &mut App,
    edit: impl FnOnce(&mut TextAreaState),
) -> bool {
    let edited = with_host(cx, |host| {
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
fn begin(state: &Entity<AppState>, surface: CardEdit, text: String, cx: &mut App) {
    let draft = with_host(cx, |host| host.card_detail.clone());
    // `render` builds the left pane as: conflict banner (only when there is one), title,
    // description, comments, comment editor.
    let comment_item =
        3 + usize::from(card(state.read(cx), &draft).is_some_and(|card| card.conflict.is_some()));
    with_host(cx, |host| {
        host.card_detail.begin(surface, text, comment_item);
    });
    notify(state, cx);
    cx.stop_propagation();
}

/// Moves the property selection.
fn move_row(state: &Entity<AppState>, delta: isize, len: usize, cx: &mut App) {
    with_host(cx, |host| {
        host.card_detail.property_row =
            crate::dialogs::step(host.card_detail.property_row, delta, len);
    });
    notify(state, cx);
    cx.stop_propagation();
}

// ---------------------------------------------------------------------------- commands

/// `Enter`: a newline inside a multi-line edit, a save inside the title, otherwise the picker
/// or worktree the selected property row points at.
fn enter(state: &Entity<AppState>, bridge: &Bridge, targets: &[detail::PropertyRow], cx: &mut App) {
    match with_host(cx, |host| host.card_detail.edit) {
        Some(CardEdit::Title) => {
            commit_edit(state, bridge, cx);
            return;
        }
        Some(CardEdit::Description | CardEdit::Comment) => {
            edit_buffer(state, cx, TextAreaState::insert_newline);
            return;
        }
        None => {}
    }
    let row = with_host(cx, |host| host.card_detail.property_row);
    let selected = targets.get(row);
    match selected.map(|row| &row.target) {
        Some(PropertyTarget::Pick(kind)) => {
            // The board applies this gate before opening the same picker (§3.12 C): without it
            // the row opens a dialog whose Enter can only fail, instead of flashing the banner.
            if board::refuses(state, cx) {
                return;
            }
            // A field the backend owns is refused here rather than by the daemon two dialogs
            // later. This surface's scrim covers the toast stack, so the sentence goes on the
            // dialog's own error line — the same place every other refusal here lands.
            if let Some(message) = board::readonly_message(state.read(cx), kind) {
                with_host(cx, |host| host.card_detail.error = Some(message));
                notify(state, cx);
                cx.stop_propagation();
                return;
            }
            let kind = kind.clone();
            crate::dialogs::with_host(cx, |host| {
                host.card_picker.kind = kind;
                host.card_picker.then_worktree = false;
            });
            board::open_dialog(state, Dialogs::CardPicker, cx);
        }
        Some(PropertyTarget::Worktree) => {
            let draft = with_host(cx, |host| host.card_detail.clone());
            let Some((id, card_id)) = card(state.read(cx), &draft)
                .and_then(|card| Some((card.worktree_id.clone()?, card.id.clone())))
            else {
                return;
            };
            board::open_session(id, board::Refusal::CardDetail(card_id), state, bridge, cx);
        }
        Some(PropertyTarget::ReadOnly) => {
            // A locked row is a backend-declared property the remote owns; it answers with the
            // same sentence the standard read-only rows do, because two rows that carry the
            // same lock glyph must not answer `Enter` differently. Every other read-only row
            // (Remote, URL, Synced) states a fact with nothing to say about editing.
            let message = selected.filter(|row| row.locked).map(|row| {
                let backend = state.read(cx).board().map_or_else(String::new, |view| {
                    state.read(cx).backend_label(&view.board.backend.kind)
                });
                format!("{} is read-only on {backend} boards", row.label)
            });
            if let Some(message) = message {
                with_host(cx, |host| host.card_detail.error = Some(message));
                notify(state, cx);
            }
        }
        None => {}
    }
    cx.stop_propagation();
}

/// `ctrl-s`: send the open buffer and wait for the daemon's answer.
fn commit_edit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    cx.stop_propagation();
    // The same gate every other mutating key on this surface applies (`enter`, `w`, `K`/`R`):
    // without it a save with fleetd down answered with the bridge's own "the Fleet daemon is
    // not connected" on the dialog's error line and never flashed the offline banner — two
    // different sentences for one condition.
    if board::refuses(state, cx) {
        return;
    }
    let draft = with_host(cx, |host| host.card_detail.clone());
    if draft.saving.is_some() {
        return;
    }
    let (Some(surface), Some(card_id)) = (draft.edit, draft.card_id.clone()) else {
        return;
    };
    let text = draft.area.text().trim().to_owned();
    let request = match surface {
        CardEdit::Title if text.is_empty() => {
            with_host(cx, |host| {
                host.card_detail.error = Some("A card needs a title.".to_owned());
            });
            notify(state, cx);
            return;
        }
        CardEdit::Title => RequestBody::UpdateCard {
            card_id,
            patch: CardPatch {
                title: Some(text),
                ..CardPatch::default()
            },
        },
        CardEdit::Description => RequestBody::UpdateCard {
            card_id,
            patch: CardPatch {
                description: Some(draft.area.text().to_owned()),
                ..CardPatch::default()
            },
        },
        CardEdit::Comment if text.is_empty() => {
            with_host(cx, |host| host.card_detail.cancel());
            notify(state, cx);
            return;
        }
        CardEdit::Comment => RequestBody::AddCardComment {
            card_id,
            body: text,
        },
    };
    let revision = draft.revision;
    with_host(cx, |host| host.card_detail.saving = Some(revision));
    let reply = bridge.request(request);
    let state = state.clone();
    cx.spawn(async move |cx| {
        let result = match reply.recv().await {
            Ok(Ok(ResponseBody::Card(card))) => Ok(card),
            Ok(Ok(_)) => Err("Unexpected card save response".to_owned()),
            Ok(Err(error)) => Err(error.message),
            Err(error) => Err(format!("Card save channel closed: {error}")),
        };
        cx.update(|cx| {
            with_host(cx, |host| {
                host.card_detail
                    .finish_save(revision, result.as_ref().err().cloned())
            });
            if let Ok(card) = result {
                state.update(cx, |app, cx| {
                    app.apply_card(card);
                    cx.notify();
                });
            }
            notify(&state, cx);
        });
    })
    .detach();
}

/// `w`: create the card's worktree, asking for a repository first when nothing knows one.
fn start_worktree(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    cx.stop_propagation();
    if board::refuses(state, cx) {
        return;
    }
    let draft = with_host(cx, |host| host.card_detail.clone());
    let Some(card) = card(state.read(cx), &draft).cloned() else {
        return;
    };
    let default_repo = state
        .read(cx)
        .board()
        .and_then(|view| view.board.default_repo_id.clone());
    if card.repo_id.is_none() && default_repo.is_none() {
        // The same guard the board surface applies to the same key: with no repository in the
        // context the picker's only row is the "No repository" clear row, whose Enter answers
        // "Pick a repository" forever — a dialog with no way to succeed.
        if !board::has_repo_in_context(state.read(cx)) {
            with_host(cx, |host| {
                host.card_detail.error = Some(board::NO_REPO_IN_CONTEXT.to_owned());
            });
            notify(state, cx);
            return;
        }
        with_host(cx, |host| {
            host.card_picker.kind = crate::dialogs::card_picker::PickerKind::Repo;
            host.card_picker.then_worktree = true;
        });
        board::open_dialog(state, Dialogs::CardPicker, cx);
        return;
    }
    let refusal = board::Refusal::CardDetail(card.id.clone());
    board::request_worktree_reporting(card.id, None, state, bridge, refusal, cx);
    cx.stop_propagation();
}

/// `K` / `R`: resolve the card's conflict one way or the other.
fn resolve(
    state: &Entity<AppState>,
    bridge: &Bridge,
    resolution: ConflictResolution,
    cx: &mut App,
) {
    // Every other mutating key on this surface answers "fleetd is not reachable" rather than
    // sending into a dead channel and reporting the channel's own closure.
    if board::refuses(state, cx) {
        return;
    }
    let draft = with_host(cx, |host| host.card_detail.clone());
    let Some(card) = card(state.read(cx), &draft) else {
        return;
    };
    if card.conflict.is_none() {
        // §8 gives every board key an answer; a key that neither acts nor says anything reads
        // as broken.
        with_host(cx, |host| {
            host.card_detail.error = Some(NO_CONFLICT.to_owned());
        });
        notify(state, cx);
        return;
    }
    let card_id = card.id.clone();
    // The dialog's scrim covers the status bar: a refusal must land on this surface (§Board).
    board::send_card_reporting(
        state,
        bridge,
        RequestBody::ResolveCardConflict {
            card_id: card_id.clone(),
            resolution,
        },
        board::Refusal::CardDetail(card_id),
        cx,
    );
    cx.stop_propagation();
}

// ---------------------------------------------------------------------------- extension points

/// `esc` — discard an open edit, else close the dialog.
pub fn close(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let editing = with_host(cx, |host| {
        let editing = host.card_detail.is_editing();
        if editing {
            host.card_detail.cancel();
        }
        editing
    });
    if editing {
        notify(state, cx);
        return;
    }
    state.update(cx, |state, cx| {
        state.close_overlay();
        cx.notify();
    });
}

/// `i` — edit the title.
pub fn edit_title(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let draft = with_host(cx, |host| host.card_detail.clone());
    let Some(title) = card(state.read(cx), &draft).map(|card| card.title.clone()) else {
        return;
    };
    begin(state, CardEdit::Title, title, cx);
}

/// `d` — edit the description.
pub fn edit_description(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let draft = with_host(cx, |host| host.card_detail.clone());
    let Some(description) = card(state.read(cx), &draft).map(|card| card.description.clone())
    else {
        return;
    };
    begin(state, CardEdit::Description, description, cx);
}

/// `c` — write a comment.
pub fn add_comment(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let draft = with_host(cx, |host| host.card_detail.clone());
    if card(state.read(cx), &draft).is_none() {
        // The dialog is rendering `missing()`; an edit opened over it takes the keyboard and
        // the first `Esc` only cancels a buffer nothing is showing.
        return;
    }
    begin(state, CardEdit::Comment, String::new(), cx);
}

/// `j` — next property row.
pub fn next_property(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let len = property_count(state, cx);
    move_row(state, 1, len, cx);
}

/// `k` — previous property row.
pub fn prev_property(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let len = property_count(state, cx);
    move_row(state, -1, len, cx);
}

/// `Enter` — edit the selected property.
pub fn edit_property(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let targets = property_targets(state, cx);
    enter(state, bridge, &targets, cx);
}

/// `w` — create the card's worktree.
pub fn create_worktree(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    start_worktree(state, bridge, cx);
}

/// `x` — open this card's remote issue in the browser.
///
/// The dialog stays open: the browser is another window, and closing the card the user is
/// reading to show it somewhere else loses the place they were in.
pub fn open_remote(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let draft = with_host(cx, |host| host.card_detail.clone());
    let Some(card) = card(state.read(cx), &draft) else {
        return;
    };
    let Some(url) = board::remote_url(card) else {
        // The toast stack sits under this dialog's scrim, so the sentence goes where every
        // other refusal on this surface goes — and it is the same sentence the board screen's
        // `x` answers with: a linked card whose board has no address for its backend is a
        // different fact from a card with no remote issue at all, and only one of them names
        // the setting that fixes it.
        let refusal = board::no_remote_reason(card);
        with_host(cx, |host| {
            host.card_detail.error = Some(refusal.to_owned());
        });
        notify(state, cx);
        return;
    };
    cx.open_url(&url);
}

/// What `K`/`R` answer on a card that has no conflict to resolve.
const NO_CONFLICT: &str = "No conflict on this card";

/// `K` — resolve the conflict by keeping the local card.
pub fn keep_local(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    resolve(state, bridge, ConflictResolution::KeepLocal, cx);
}

/// `R` — resolve the conflict by taking the remote card.
pub fn take_remote(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    resolve(state, bridge, ConflictResolution::TakeRemote, cx);
}

/// `ctrl-s` — save the open text edit.
pub fn save(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    commit_edit(state, bridge, cx);
}

/// How many property rows the shown card has.
fn property_count(state: &Entity<AppState>, cx: &App) -> usize {
    property_targets(state, cx).len()
}

/// The property rows of the shown card, for the palette-driven entry points.
fn property_targets(state: &Entity<AppState>, cx: &App) -> Vec<detail::PropertyRow> {
    let host = cx.try_global::<crate::dialogs::DialogHost>();
    let Some(draft) = host.map(|host| host.card_detail.clone()) else {
        return Vec::new();
    };
    let app = state.read(cx);
    let (Some(view), Some(card)) = (app.board(), card(app, &draft)) else {
        return Vec::new();
    };
    detail::property_rows(&view.board, &view.cards, card, now_epoch())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_column_survives_multiple_vertical_actions() {
        let mut draft = CardDetailState::default();
        draft.begin(CardEdit::Description, "abcdef\nx\nabcdef".into(), 3);
        draft.apply(|area| {
            area.set_cursor(5);
        });
        draft.apply(|area| {
            area.move_down();
        });
        draft.apply(|area| {
            area.move_down();
        });
        assert_eq!(draft.area.line_col(), (2, 5));
        draft.apply(TextAreaState::insert_tab);
        assert!(draft.area.text().ends_with("abcde  f"));
    }

    #[test]
    fn save_failure_retains_draft_and_old_replies_cannot_clear_new_edits() {
        let mut draft = CardDetailState::default();
        draft.begin(CardEdit::Comment, "draft".into(), 3);
        let revision = draft.revision;
        draft.saving = Some(revision);
        draft.finish_save(revision, Some("disk full".into()));
        assert_eq!(draft.area.text(), "draft");
        assert!(draft.is_editing());
        draft.saving = Some(revision);
        draft.apply(|area| area.insert(" continued"));
        draft.finish_save(revision, None);
        assert_eq!(draft.area.text(), "draft continued");
        let current = draft.revision;
        draft.saving = Some(current);
        draft.finish_save(revision, None);
        assert!(draft.is_editing());
        draft.finish_save(current, None);
        assert!(!draft.is_editing());
    }

    #[test]
    fn one_buffer_serves_the_three_text_surfaces() {
        let mut draft = CardDetailState::default();
        assert!(!draft.is_editing());
        draft.begin(CardEdit::Title, "Fix login".to_owned(), 3);
        assert_eq!(draft.edit, Some(CardEdit::Title));
        assert_eq!(draft.area.cursor(), "Fix login".len());
        draft.apply(|area| area.insert("!"));
        assert_eq!(draft.area.text(), "Fix login!");
        draft.cancel();
        assert!(!draft.is_editing());
        assert!(draft.area.is_empty());
    }

    #[test]
    fn the_caret_survives_a_multibyte_edit() {
        let mut draft = CardDetailState::default();
        draft.begin(CardEdit::Comment, String::new(), 3);
        draft.apply(|area| area.insert("ñ"));
        assert_eq!(draft.area.cursor(), "ñ".len());
        draft.apply(|area| {
            area.backspace();
        });
        assert!(draft.area.is_empty());
        assert_eq!(draft.area.cursor(), 0);
    }

    #[test]
    fn every_edit_surface_names_itself_for_the_footer() {
        assert_eq!(CardEdit::Title.label(), "title");
        assert_eq!(CardEdit::Description.label(), "description");
        assert_eq!(CardEdit::Comment.label(), "comment");
    }
}
