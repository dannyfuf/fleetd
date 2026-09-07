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
