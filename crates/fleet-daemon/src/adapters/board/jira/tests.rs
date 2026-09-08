use super::push::TempJson;
use super::*;
use crate::{adapters::clock::SystemClock, testing::fakes::FakeShell};

fn backend() -> JiraBackend {
    JiraBackend::new(Arc::new(FakeShell::new()), Arc::new(SystemClock))
}

/// The identity half of the trait is real from this stage on: the registry, the settings
/// dialog and `fleet board backends` all read it before anything talks to Jira.
#[test]
fn identity_and_capabilities_are_answered_without_touching_jira() {
    let backend = backend();
    assert_eq!(backend.kind(), "jira");
    assert_eq!(backend.label(), "Jira (acli)");
    let caps = backend.capabilities();
    assert!(caps.pull && caps.incremental && caps.transitions && caps.comments);
    assert!(caps.push_updates && caps.push_create && caps.custom_properties);
    assert_eq!(backend.settings_schema(), settings::settings_schema());
}

/// Every read-only field must be a field name the core knows, or the guard silently passes.
#[test]
fn the_read_only_fields_are_card_field_names() {
    for field in READONLY_FIELDS {
        assert!(
            [
                "title",
                "description",
                "status_id",
                "priority",
                "labels",
                "assignee",
                "estimate",
                "due_date",
                "parent_id"
            ]
            .contains(field),
            "{field} is not a standard card field"
        );
    }
    // The two halves must not overlap: a field that is both written and refused would be
    // pushed by `edit` and rejected by the core in the same sync.
    for field in WRITABLE_FIELDS {
        assert!(!READONLY_FIELDS.contains(field), "{field}");
    }
}

#[tokio::test]
async fn a_staged_body_is_removed_once_the_call_is_over() {
    let path = {
        let file = TempJson::new(&serde_json::json!({"type": "doc"}))
            .await
            .unwrap();
        let path = PathBuf::from(file.argument());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"type\":\"doc\"}"
        );
        path
    };
    assert!(!path.exists(), "a staged ADF body outlived its call");
}
