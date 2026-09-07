use fleet_core::{
    config::{Config, SleepConfig, default_config, merge_config},
    ids::{ContextId, HostId, JobId, RepoId, SessionId, WorktreeId},
    sleep::{KeepAliveKind, KeepAliveRule},
    state::{State, StateValidationError, validate_state},
};

#[test]
fn string_identifiers_keep_their_wire_shape_and_validated_decoding() {
    macro_rules! assert_identifier {
        ($id:ty, $valid:literal, $invalid:literal) => {
            let id: $id = $valid.parse().unwrap();
            let golden = concat!("\"", $valid, "\"");
            assert_eq!(serde_json::to_string(&id).unwrap(), golden);
            assert_eq!(serde_json::from_str::<$id>(golden).unwrap(), id);
            assert!(serde_json::from_str::<$id>(concat!("\"", $invalid, "\"")).is_err());
        };
    }
    assert_identifier!(ContextId, "work", "Work");
    assert_identifier!(RepoId, "acme/api", "acme/api/extra");
    assert_identifier!(WorktreeId, "acme/api#feature", "acme/api#bad#slug");
    assert_identifier!(HostId, "devbox", "local");
    assert_identifier!(SessionId, "api/feature", "missing-slash");
    assert_identifier!(JobId, "job-1", "job 1");
}

#[test]
fn sleep_config_wire_golden_and_legacy_defaults() {
    let config = SleepConfig {
        enabled: true,
        keep_alive: vec![KeepAliveRule {
            id: "worker".to_owned(),
            label: "build".to_owned(),
            kind: KeepAliveKind::Process,
            pattern: r"cargo\s+test".to_owned(),
            enabled: false,
        }],
        grace_ms: 2000,
    };
    let golden = r#"{"enabled":true,"keepAlive":[{"id":"worker","label":"build","kind":"process","pattern":"cargo\\s+test","enabled":false}],"graceMs":2000}"#;
    assert_eq!(serde_json::to_string(&config).unwrap(), golden);
    assert_eq!(serde_json::from_str::<SleepConfig>(golden).unwrap(), config);
    let legacy = r#"{"enabled":true,"keepAlive":[{"id":"ports","label":"server","kind":"listening-port"}],"graceMs":2000}"#;
    let legacy: SleepConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        r#"{"enabled":true,"keepAlive":[{"id":"ports","label":"server","kind":"listening-port","pattern":"","enabled":true}],"graceMs":2000}"#
    );
}

#[test]
fn omitted_new_config_sections_retain_default_values() {
    let defaults = default_config("/home/me/.fleet");
    let mut legacy = serde_json::to_value(&defaults).unwrap();
    for key in ["discoveredWatches", "trash", "jobs", "terminal"] {
        legacy.as_object_mut().unwrap().remove(key);
    }
    legacy["ui"]
        .as_object_mut()
        .unwrap()
        .remove("notifications");
    assert_eq!(serde_json::from_value::<Config>(legacy).unwrap(), defaults);
    assert_eq!(
        merge_config("/home/me/.fleet", serde_json::json!({})).unwrap(),
        defaults
    );
}

#[test]
fn rejects_mismatched_identity_fields() {
    let value = serde_json::json!({
        "version": 1,
        "contexts": [{
            "id": "work",
            "name": "Work",
            "owners": ["acme"],
            "createdAt": "2026-01-01T00:00:00Z"
        }],
        "repos": [{
            "id": "acme/api",
            "owner": "acme",
            "name": "api",
            "url": "git@example.com:acme/api.git",
            "contextId": "work",
            "defaultBranch": "main",
            "path": "/repos/acme/api",
            "clonedAt": "2026-01-01T00:00:00Z"
        }],
        "clones": [],
        "worktrees": [{
            "id": "acme/api#feature",
            "repoId": "acme/api",
            "slug": "feature",
            "branch": "feature",
            "baseRef": "main",
            "path": "/trees/acme/api/feature",
            "session": "api/feature",
            "createdAt": "2026-01-01T00:00:00Z"
        }]
    });
    let valid: State = serde_json::from_value(value).unwrap();
    validate_state(&valid).unwrap();

    for mismatch in [
        |state: &mut State| state.repos[0].owner = "other".to_owned(),
        |state: &mut State| state.repos[0].name = "other".to_owned(),
        |state: &mut State| state.worktrees[0].repo_id = "other/api".parse().unwrap(),
        |state: &mut State| state.worktrees[0].slug = "other".to_owned(),
    ] {
        let mut state = valid.clone();
        mismatch(&mut state);
        assert!(matches!(
            validate_state(&state),
            Err(StateValidationError::MismatchedIdentity { .. })
        ));
    }
}

#[test]
fn legacy_mismatched_identity_fields_are_normalized_on_load() {
    let legacy = serde_json::json!({
        "version": 1,
        "contexts": [{
            "id": "work",
            "name": "Work",
            "owners": ["acme"],
            "createdAt": "2026-01-01T00:00:00Z"
        }],
        "repos": [{
            "id": "acme/api",
            "owner": "Acme",
            "name": "legacy-name",
            "url": "git@example.com:acme/api.git",
            "contextId": "work",
            "defaultBranch": "main",
            "path": "/repos/acme/api",
            "clonedAt": "2026-01-01T00:00:00Z"
        }],
        "clones": [],
        "worktrees": [{
            "id": "acme/api#feature",
            "repoId": "other/repo",
            "slug": "legacy-slug",
            "branch": "feature",
            "baseRef": "main",
            "path": "/trees/acme/api/feature",
            "session": "api/feature",
            "createdAt": "2026-01-01T00:00:00Z"
        }]
    });

    let state: State = serde_json::from_value(legacy).unwrap();

    validate_state(&state).unwrap();
    assert_eq!(state.repos[0].owner, "acme");
    assert_eq!(state.repos[0].name, "api");
    assert_eq!(state.worktrees[0].repo_id.as_str(), "acme/api");
    assert_eq!(state.worktrees[0].slug, "feature");
}
