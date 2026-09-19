use super::*;

#[test]
fn save_failure_retains_draft_and_old_replies_cannot_clear_new_edits() {
    let mut draft = CardDetailState::default();
    draft.begin(CardEdit::Comment, 3);
    let revision = draft.revision;
    draft.saving = Some(revision);
    draft.finish_save(revision, Some("disk full".into()));
    assert!(draft.is_editing());
    draft.saving = Some(revision);
    draft.revision = draft.revision.wrapping_add(1);
    draft.finish_save(revision, None);
    assert!(draft.is_editing());
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
    draft.begin(CardEdit::Title, 3);
    assert_eq!(draft.edit, Some(CardEdit::Title));
    draft.cancel();
    assert!(!draft.is_editing());
}

#[test]
fn every_edit_surface_names_itself_for_the_footer() {
    assert_eq!(CardEdit::Title.label(), "title");
    assert_eq!(CardEdit::Description.label(), "description");
    assert_eq!(CardEdit::Comment.label(), "comment");
}
