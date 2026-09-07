//! New card (BOARD §8) — *a title is enough; everything else has a key of its own.*
//!
//! Two fields and two ways out: `Enter` creates and closes, `ctrl-Enter` creates and opens the
//! card it made. Nothing else is asked for here — status, priority, labels and the rest all
//! have a one-letter picker on the board, and a create dialog that asks for them up front is a
//! form, not a keystroke.

use fleet_core::{board::CardDraft, ids::BoardId};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{board as board_actions, dialog},
    bridge::Bridge,
    dialogs::{DialogHost, Dialogs, notify, root, typed_char, with_host},
    screens::board,
    state::AppState,
};

/// Which field of the create dialog owns the keyboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Field {
    /// The one-line title.
    #[default]
    Title,
    /// The optional markdown description.
    Description,
}

/// Draft for a new card on the current board.
#[derive(Debug, Clone, Default)]
pub struct CardCreateState {
    /// Identity of this opening, used to ignore replies to discarded drafts.
    pub generation: u64,
    /// A request is in flight; retain and freeze the draft until it answers.
    pub saving: bool,
    /// Destination board.
    pub board_id: Option<BoardId>,
    /// Pending card fields.
    pub draft: CardDraft,
    /// Which field owns the keyboard.
    pub field: Field,
    /// Caret in `draft.title`, as a character offset.
    pub title_caret: usize,
    /// Persistent description editor, including caret and preferred column.
    pub description_area: TextAreaState,
    /// Pixel scroll of the description box, which owns what `scroll_row` cannot: how tall the
    /// box made the lines it wrapped.
    pub description_scroll: gpui::ScrollHandle,
    /// The exact message from a refused create.
    pub error: Option<String>,
}

impl CardCreateState {
    /// Completes only the request belonging to this opening.
    pub fn finish_save(&mut self, generation: u64, error: Option<String>) -> bool {
        if self.generation != generation || !self.saving {
            return false;
        }
        self.saving = false;
        self.error = error;
        self.error.is_none()
    }

    /// Whether `Enter` may create: a card is its title, so a blank one is not a card.
    #[must_use]
    pub fn can_submit(&self) -> bool {
        !self.saving && self.board_id.is_some() && !self.draft.title.trim().is_empty()
    }

    /// Runs `edit` against the focused buffer, keeping its caret in range.
    fn edit(&mut self, edit: impl FnOnce(&mut TextAreaState)) {
        if self.saving {
            return;
        }
        if self.field == Field::Description {
            edit(&mut self.description_area);
            self.description_area.reveal_cursor(6);
            self.draft.description = self.description_area.text().to_owned();
            return;
        }
        let mut area = TextAreaState::from_text(self.draft.title.clone());
        area.set_cursor(self.title_byte_cursor());
        edit(&mut area);
        // A title is one line even when pasted text contains newlines.
        self.draft.title = area.text().replace('\n', " ");
        self.title_caret = self.draft.title[..area.cursor().min(self.draft.title.len())]
            .chars()
            .count();
    }

    /// The title caret as a byte offset, which is what [`TextAreaState`] counts in.
    fn title_byte_cursor(&self) -> usize {
        self.draft
            .title
            .char_indices()
            .nth(self.title_caret)
            .map_or(self.draft.title.len(), |(index, _)| index)
    }
}

pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let board_id = state.read(cx).board().map(|view| view.board.id.clone());
    with_host(state, cx, |host| {
        host.card_create = CardCreateState {
            generation: host.card_create.generation.wrapping_add(1),
            board_id,
            ..Default::default()
        }
    });
}

/// Renders the two fields and their two ways out.
pub fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.md;
    let draft = with_host(state, cx, |host| host.card_create.clone());
    let board_name = state
        .read(cx)
        .board()
        .map(|view| format!("\u{00b7} {}", view.board.name));

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(
            TextField::new(draft.draft.title.clone())
                .label("Title")
                .placeholder("Fix the login redirect")
                .caret(draft.title_caret)
                .focused(draft.field == Field::Title),
        )
        .child(
            TextArea::new(draft.draft.description.clone())
                .label("Description")
                .placeholder("Markdown. Optional.")
                .rows(6)
                .max_rows(6)
                .scroll_row(draft.description_area.scroll_row())
                .scroll(
                    "card-create-description-scroll",
                    draft.description_scroll.clone(),
                )
                .cursor(draft.description_area.cursor())
                .focused(draft.field == Field::Description),
        );

    let mut card = Dialog::new("New card")
        .icon(Icon::Plus)
        .width(Dialogs::CardCreate.width(cx))
        .body(body)
        .hint_row(
            KeyHintRow::new()
                .key(
                    "\u{21e5}",
                    if draft.field == Field::Description {
                        "indent"
                    } else {
                        "description"
                    },
                )
                .key("⇧tab", "title")
                .key("\u{2303}\u{23ce}", "create & open")
                .key("esc", "cancel"),
        )
        .primary(if draft.field == Field::Description {
            "⌃↵ Create & open"
        } else {
            "↵ Create"
        });
    if let Some(name) = board_name {
        card = card.subtitle(name);
    }
    if let Some(message) = draft.error.clone() {
        card = card.error(message);
    }

    let submit_state = state.clone();
    let submit_bridge = bridge.clone();
    let open_state = state.clone();
    let open_bridge = bridge.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let Some(text) = typed_char(event) else {
                    return;
                };
                with_host(&state, cx, |host| {
                    host.card_create.error = None;
                    host.card_create.edit(|area| area.insert(text));
                });
                notify(&state, cx);
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| {
                if with_host(&state, cx, |host| {
                    host.card_create.field == Field::Description
                }) {
                    edit(&state, cx, TextAreaState::insert_tab);
                } else {
                    cycle_field(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| cycle_field(&state, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _window, cx| {
                edit(&state, cx, |area| {
                    area.move_down();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| {
                edit(&state, cx, |area| {
                    area.move_up();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                edit(&state, cx, |area| {
                    area.move_left();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                edit(&state, cx, |area| {
                    area.move_right();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                edit(&state, cx, |area| {
                    area.backspace();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                edit(&state, cx, |area| {
                    area.delete_word_before();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                // DESIGN-SYSTEM §TextArea: `ctrl-u` clears the line, not the whole draft.
                edit(&state, cx, |area| {
                    area.delete_to_line_start();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                edit(&state, cx, |area| {
                    area.move_to_line_start();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                edit(&state, cx, |area| {
                    area.move_to_line_end();
                });
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            if with_host(&submit_state, cx, |host| {
                host.card_create.field == Field::Description
            }) {
                edit(&submit_state, cx, TextAreaState::insert_newline);
            } else {
                submit(false, &submit_state, &submit_bridge, cx);
            }
            cx.stop_propagation();
        })
        .on_action(move |_: &board_actions::CreateAndOpen, _window, cx| {
            submit(true, &open_state, &open_bridge, cx);
            cx.stop_propagation();
        })
        .child(card)
        .into_any_element()
}

/// `Tab` / `S-Tab`: two fields, so both keys do the same thing.
fn cycle_field(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        host.card_create.field = match host.card_create.field {
            Field::Title => Field::Description,
            Field::Description => Field::Title,
        };
    });
    notify(state, cx);
}

/// Runs a text edit against the focused field and repaints.
fn edit(state: &Entity<AppState>, cx: &mut App, edit: impl FnOnce(&mut TextAreaState)) {
    with_host(state, cx, |host| host.card_create.edit(edit));
    notify(state, cx);
    cx.stop_propagation();
}

/// Creates the card; `open_after` also opens its detail once the daemon answers.
fn submit(open_after: bool, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let current_board = state.read(cx).board().map(|view| view.board.id.clone());
    let Some((board_id, draft, generation)) = with_host(state, cx, |host| {
        let draft = &mut host.card_create;
        if draft.saving || draft.board_id.is_none() || draft.board_id != current_board {
            return None;
        }
        if !draft.can_submit() {
            draft.error = Some("A card needs a title.".into());
            return None;
        }
        draft.saving = true;
        draft.error = None;
        let mut fields = draft.draft.clone();
        fields.title = fields.title.trim().to_owned();
        Some((draft.board_id.clone()?, fields, draft.generation))
    }) else {
        notify(state, cx);
        return;
    };
    let reply = bridge.request(RequestBody::CreateCard {
        board_id: board_id.clone(),
        draft,
    });
    notify(state, cx);
    let handle = state.clone();
    cx.spawn(async move |cx| {
        let answer = match reply.recv().await {
            Ok(Ok(ResponseBody::Card(card))) => Ok(card),
            Ok(Err(error)) => Err(error.message),
            Ok(Ok(_)) => Err("Unexpected create response".into()),
            Err(_) => Err("Daemon disconnected before replying".into()),
        };
        cx.update(|cx| {
            if handle.read(cx).overlay != Some(crate::state::Overlay::Dialog(Dialogs::CardCreate))
                || !handle
                    .read(cx)
                    .board()
                    .is_some_and(|view| view.board.id == board_id)
            {
                return;
            }
            let completed = with_host(&handle, cx, |host| {
                host.card_create
                    .finish_save(generation, answer.as_ref().err().cloned())
            });
            if completed && let Ok(card) = answer {
                let id = card.id.clone();
                handle.update(cx, |app, cx| {
                    app.apply_card(card);
                    board::focus_card(app, &id);
                    app.close_overlay();
                    app.toast_short("✓ card created", Icon::Boxes, std::time::Instant::now());
                    cx.notify();
                });
                if open_after {
                    board::open_dialog(&handle, Dialogs::CardDetail, cx);
                }
            }
            notify(&handle, cx);
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> CardCreateState {
        CardCreateState {
            board_id: Some("work".parse().unwrap_or_else(|error| panic!("{error}"))),
            ..CardCreateState::default()
        }
    }

    #[test]
    fn description_retains_preferred_column_and_accepts_newlines_and_tabs() {
        let mut draft = draft();
        draft.field = Field::Description;
        draft.edit(|area| area.insert("abcdef\nx\nabcdef"));
        draft.edit(|area| area.set_cursor(5));
        draft.edit(|area| {
            area.move_down();
        });
        draft.edit(|area| {
            area.move_down();
        });
        assert_eq!(draft.description_area.line_col(), (2, 5));
        draft.edit(TextAreaState::insert_newline);
        draft.edit(TextAreaState::insert_tab);
        assert!(draft.draft.description.ends_with("abcde\n  f"));
    }

    #[test]
    fn a_card_needs_a_title_and_a_board() {
        let mut state = draft();
        assert!(!state.can_submit());
        state.draft.title = "   ".to_owned();
        assert!(!state.can_submit(), "whitespace is not a title");
        state.draft.title = "Fix login".to_owned();
        assert!(state.can_submit());
        state.board_id = None;
        assert!(!state.can_submit());
    }

    #[test]
    fn typing_lands_in_the_focused_field_only() {
        let mut state = draft();
        state.edit(|area| area.insert("Fix"));
        assert_eq!(state.draft.title, "Fix");
        assert_eq!(state.title_caret, 3);
        assert!(state.draft.description.is_empty());
        state.field = Field::Description;
        state.edit(|area| area.insert("why"));
        assert_eq!(state.draft.description, "why");
        assert_eq!(state.draft.title, "Fix");
    }

    #[test]
    fn the_title_never_becomes_two_lines() {
        let mut state = draft();
        state.edit(|area| area.insert("one"));
        state.edit(TextAreaState::insert_newline);
        state.edit(|area| area.insert("two"));
        assert_eq!(state.draft.title, "one two");
    }

    #[test]
    fn the_title_caret_counts_characters_not_bytes() {
        let mut state = draft();
        state.edit(|area| area.insert("ñand\u{fa}"));
        assert_eq!(state.title_caret, 5);
        state.edit(|area| {
            area.backspace();
        });
        assert_eq!(state.draft.title, "ñand");
        assert_eq!(state.title_caret, 4);
    }
}
