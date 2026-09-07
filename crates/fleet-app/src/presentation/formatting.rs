use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Parses daemon RFC3339 timestamps; invalid or missing offsets remain unknown.
pub fn parse_timestamp(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|date| date.timestamp())
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

pub fn age_secs(timestamp: &str, now: i64) -> Option<i64> {
    parse_timestamp(timestamp).map(|then| now.saturating_sub(then).max(0))
}

pub fn age_label(timestamp: &str, now: i64) -> String {
    age_secs(timestamp, now).map_or_else(|| "–".to_owned(), fleet_ui_kit::format_age)
}

/// The caller supplies the observed user home, independently of Fleet's storage directory.
pub fn tilde<'a>(path: &'a str, home: Option<&Path>) -> Cow<'a, str> {
    let Some(home) = home else {
        return Cow::Borrowed(path);
    };
    let home = home.to_string_lossy();
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return Cow::Borrowed(path);
    }
    match path.strip_prefix(home) {
        Some("") => Cow::Borrowed("~"),
        Some(rest) if rest.starts_with('/') => Cow::Owned(format!("~{rest}")),
        _ => Cow::Borrowed(path),
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn bare_version(version: &str) -> &str {
    version
        .strip_prefix("fleetd ")
        .or_else(|| version.strip_prefix("Fleet "))
        .unwrap_or(version)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_validate_calendar_and_offset() {
        assert_eq!(parse_timestamp("1970-01-01T02:30:00+02:30"), Some(0));
        assert_eq!(parse_timestamp("1969-12-31T23:00:00-01:00"), Some(0));
        assert_eq!(parse_timestamp("1970-01-01T00:00:01.999Z"), Some(1));
        for invalid in [
            "2026-02-29T00:00:00Z",
            "2026-01-01T25:00:00Z",
            "2026-01-01T00:00:00",
            "2026-01-01T00:00:00+99:00",
            "not a date",
        ] {
            assert_eq!(parse_timestamp(invalid), None, "{invalid}");
        }
        assert_eq!(age_secs("1970-01-01T00:00:01Z", 0), Some(0));
        assert_eq!(age_label("bad", 0), "–");
    }
    #[test]
    fn home_replacement_respects_component_boundaries() {
        let home = Some(Path::new("/Users/danny/"));
        assert_eq!(tilde("/Users/danny/file", home), "~/file");
        assert_eq!(tilde("/Users/danny", home), "~");
        assert!(matches!(
            tilde("/Users/danny2/file", home),
            Cow::Borrowed("/Users/danny2/file")
        ));
        assert_eq!(tilde("/file", Some(Path::new(""))), "/file");
        assert_eq!(tilde("/file", Some(Path::new("/"))), "/file");
    }
}
