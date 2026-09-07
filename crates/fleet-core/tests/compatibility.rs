use fleet_core::{
    config::{Config, SleepConfig, default_config, merge_config},
    ids::{ContextId, HostId, JobId, RepoId, SessionId, WorktreeId},
    sleep::{KeepAliveKind, KeepAliveRule},
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
