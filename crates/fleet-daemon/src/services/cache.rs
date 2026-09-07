use crate::{DaemonError, DaemonResult, adapters::files::Files, stores::state::StateStore};
use chrono::{DateTime, Utc};
use fleet_core::paths::FleetHome;
use std::path::Path;

pub(super) fn cache_is_fresh(fetched_at: &str, ttl_seconds: i64) -> bool {
    if ttl_seconds < 0 {
        return false;
    }
    DateTime::parse_from_rfc3339(fetched_at)
        .ok()
        .map(|fetched| Utc::now().signed_duration_since(fetched.with_timezone(&Utc)))
        .is_some_and(|age| age.num_milliseconds() >= 0 && age.num_seconds() < ttl_seconds)
}

pub(super) fn read_cache<T: serde::de::DeserializeOwned>(
    files: &dyn Files,
    path: &Path,
) -> Option<T> {
    files
        .read_text(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

pub(super) fn write_cache<T: serde::Serialize>(
    files: &dyn Files,
    path: &Path,
    cache: &T,
) -> DaemonResult<()> {
    let mut text = serde_json::to_string(cache)?;
    text.push('\n');
    files.atomic_write_text(path, &text)
}

pub(super) fn fleet_home(state: &StateStore) -> DaemonResult<FleetHome> {
    state
        .path()
        .parent()
        .map(|path| FleetHome::new(path.to_path_buf()))
        .ok_or_else(|| DaemonError::Validation("state path has no parent".to_owned()))
}
