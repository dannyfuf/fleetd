//! `BackendRef.settings` for a Jira board, and the generic schema clients render it from.
//!
//! Nothing here talks to Jira: the type is the contract between what a user types into
//! `fleet board set --setting k=v` (or the app's settings dialog) and what the backend reads.
//! Unknown keys are rejected outright — a misspelled `proyect` that silently defaulted would
//! sync the wrong project and blame the backend.

use fleet_core::board::{BoardError, PropertyKind, PropertySchema, PropertySource};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// A trailing `ORDER BY …` in the user's filter, which `base_jql` would parenthesize.
///
/// Every search appends its own ordering, and Jira refuses `(… ORDER BY rank) ORDER BY updated
/// ASC` — a saved filter pasted verbatim would break every sync with a parse error that names
/// nothing.
static TRAILING_ORDER_BY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\s*\border\s+by\s+").expect("static regex"));

/// Drops a trailing `ORDER BY …` clause, but only when it is one.
///
/// The same words inside a quoted literal — `summary ~ "order by date"` — are the user's search
/// text, and cutting there leaves an unbalanced quote that fails every search with a parse error
/// nothing connects back to what they typed.
fn strip_trailing_order_by(jql: &str) -> String {
    // The rightmost `ORDER BY` that is not inside a literal and has no bracket after it: an
    // earlier one is either quoted text or a clause of a subexpression the filter closes again.
    let Some(found) = TRAILING_ORDER_BY
        .find_iter(jql)
        .filter(|found| !quoted_at(jql, found.start()))
        .filter(|found| !jql[found.end()..].contains(['(', ')']))
        .last()
    else {
        return jql.to_owned();
    };
    jql[..found.start()].trim_end().to_owned()
}

/// Whether `offset` falls inside a `'`- or `"`-quoted literal, honouring JQL's `\` escape.
fn quoted_at(jql: &str, offset: usize) -> bool {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (at, character) in jql.char_indices() {
        if at >= offset {
            break;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, character) {
            (_, '\\') => escaped = true,
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), character) if character == open => quote = None,
            _ => {}
        }
    }
    quote.is_some()
}

/// Whether every quote is closed and every parenthesis outside a literal is matched.
///
/// The filter is ANDed into `project = "X" AND ( … )`; anything that can close that group is a
/// widening of the board past the project it names.
fn is_balanced(jql: &str) -> bool {
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for character in jql.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, character) {
            (_, '\\') => escaped = true,
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), character) if character == open => quote = None,
            (None, '(') => depth += 1,
            (None, ')') => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0 && quote.is_none() && !escaped
}

/// A Jira project key: a letter, then letters, digits or underscores.
///
/// The key is the one fragment of every search's JQL that comes from the user, and it is
/// interpolated, not bound: `SP OR project = OTHER` would quietly widen the board onto another
/// project, and a key with a space or a quote breaks every search with a parse error that names
/// nothing. Checking it here is what lets `base_jql` interpolate it at all.
static PROJECT_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9_]*$").expect("static regex"));

/// The shape of a Jira field id, which is what `--fields` is built out of.
///
/// `summary`, `duedate`, `customfield_10016`. Every id a board configures lands in the
/// comma-joined `--fields` of every `workitem view`, so one carrying a comma reads fields the
/// user never named and one carrying a space or a quote fails every view of the board.
static FIELD_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("static regex"));

/// Default minutes of overlap re-scanned on every incremental pull.
///
/// Jira's `updated` has minute resolution and the site clock is not ours; a window that starts
/// exactly at the watermark drops every issue edited inside the same minute as the last pull.
#[must_use]
pub const fn default_overlap() -> u32 {
    10
}
/// Default number of `acli` invocations in flight.
#[must_use]
pub const fn default_concurrency() -> usize {
    4
}
/// Default number of incremental pulls between two full ones.
#[must_use]
pub const fn default_full_every() -> u32 {
    10
}
/// Default number of issues `describe` samples to discover statuses, labels and people.
#[must_use]
pub const fn default_sample() -> u32 {
    200
}

/// The lowest and highest `maxConcurrency` a board may ask for.
const CONCURRENCY: std::ops::RangeInclusive<usize> = 1..=8;

/// The narrowest and widest `overlapMinutes` a board may ask for: a day of clock skew, and no
/// more — and never zero, which removes the protection the setting exists for. Jira's `updated`
/// has minute resolution and the `-Nm` window is evaluated on the *site's* clock, so a window
/// that starts exactly at our watermark hides every issue edited inside the previous pull's
/// minute until the next full pull comes due, and asks for `updated >= "-0m"` besides.
const OVERLAP: std::ops::RangeInclusive<u32> = 1..=1440;

/// One additional read-only Jira field surfaced as a card property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraField {
    /// Jira field id, e.g. `customfield_10000`.
    pub id: String,
    /// Human name shown as the property's name.
    pub name: String,
    /// How the value is rendered; text unless stated.
    #[serde(default)]
    pub kind: PropertyKind,
}

/// Everything a Jira board needs, deserialized from `BackendRef.settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JiraSettings {
    /// Jira project key, e.g. `SP`.
    ///
    /// Defaulted so that settings without it reach the check below, which names the key, rather
    /// than serde's "missing field" wording: this message is what the CLI and the settings
    /// dialog print verbatim after `fleet board set --backend jira`.
    #[serde(default)]
    pub project: String,
    /// Site host, e.g. `buk.atlassian.net`; checked against `acli jira auth status` and used
    /// to build browse URLs.
    #[serde(default)]
    pub site: Option<String>,
    /// Extra JQL ANDed with `project = <project>`.
    #[serde(default)]
    pub jql: Option<String>,
    /// Issue type used for issues this board creates.
    #[serde(default)]
    pub issue_type: Option<String>,
    /// Custom field id holding story points, mapped to `Card.estimate`.
    #[serde(default)]
    pub story_points_field: Option<String>,
    /// Explicit column order by Jira status name; sampled from the project when empty.
    #[serde(default)]
    pub statuses: Vec<String>,
    /// Additional read-only fields surfaced as card properties.
    #[serde(default)]
    pub extra_fields: Vec<ExtraField>,
    /// Minutes of overlap re-scanned on every incremental pull.
    #[serde(default = "default_overlap")]
    pub overlap_minutes: u32,
    /// How many `acli` invocations may run at once (1..=8).
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,
    /// Every Nth pull is full, which is what catches deletions and filter exits.
    #[serde(default = "default_full_every")]
    pub full_sync_every: u32,
    /// How many issues `describe` samples.
    #[serde(default = "default_sample")]
    pub sample_limit: u32,
}

impl Default for JiraSettings {
    fn default() -> Self {
        Self {
            project: String::new(),
            site: None,
            jql: None,
            issue_type: None,
            story_points_field: None,
            statuses: Vec::new(),
            extra_fields: Vec::new(),
            overlap_minutes: default_overlap(),
            max_concurrency: default_concurrency(),
            full_sync_every: default_full_every(),
            sample_limit: default_sample(),
        }
    }
}

impl JiraSettings {
    /// Parses and validates `BackendRef.settings`.
    ///
    /// Every failure names the offending key, because this message is what `fleet board set`
    /// and the settings dialog print verbatim.
    pub fn parse(value: &serde_json::Value) -> Result<Self, BoardError> {
        let value = if value.is_null() {
            &serde_json::Value::Object(serde_json::Map::new())
        } else {
            value
        };
        let mut settings: Self =
            serde_json::from_value(value.clone()).map_err(|error| BoardError::Invalid {
                field: "settings".into(),
                reason: error.to_string(),
            })?;
        // The filter is ANDed and parenthesized, and every search appends its own ordering:
        // an `ORDER BY` the user pasted in from a saved filter would make Jira reject the JQL.
        if let Some(jql) = settings.jql.as_mut() {
            *jql = strip_trailing_order_by(jql.trim());
        }
        settings.jql = settings.jql.filter(|jql| !jql.is_empty());
        // The filter is interpolated inside `( … )`, exactly like `project` is interpolated
        // into `project = "…"`, and `project` is regex-guarded against precisely this: a filter
        // that closes the parenthesis itself — `1=1) OR (project = "OTHER"` — reopens the board
        // onto issues its `project` does not name, and the board would then edit and transition
        // them. An unbalanced quote breaks every search with a parse error instead.
        if let Some(jql) = settings.jql.as_deref()
            && !is_balanced(jql)
        {
            return Err(BoardError::Invalid {
                field: "jql".into(),
                reason: "unbalanced parentheses or quotes".into(),
            });
        }
        let invalid = |field: &str, reason: &str| BoardError::Invalid {
            field: field.into(),
            reason: reason.into(),
        };
        if settings.project.trim().is_empty() {
            return Err(invalid("project", "a Jira project key is required"));
        }
        if !PROJECT_KEY.is_match(settings.project.trim()) {
            return Err(invalid(
                "project",
                "a Jira project key is a letter followed by letters, digits or underscores",
            ));
        }
        if !CONCURRENCY.contains(&settings.max_concurrency) {
            return Err(invalid(
                "maxConcurrency",
                &format!(
                    "must be between {} and {}",
                    CONCURRENCY.start(),
                    CONCURRENCY.end()
                ),
            ));
        }
        if settings.sample_limit == 0 {
            return Err(invalid("sampleLimit", "must be at least 1"));
        }
        // `0` makes `pulls_since_full >= full_sync_every` true on every pull, so every sync
        // becomes a full one — one `view` per issue on the board, forever, with nothing said.
        if settings.full_sync_every == 0 {
            return Err(invalid("fullSyncEvery", "must be at least 1"));
        }
        // The overlap is subtracted from `updated >= "-Nm"`, and Jira refuses a window it cannot
        // read: an unbounded one fails every incremental pull. A day is far past any clock skew.
        if !OVERLAP.contains(&settings.overlap_minutes) {
            return Err(invalid(
                "overlapMinutes",
                &format!("must be between {} and {}", OVERLAP.start(), OVERLAP.end()),
            ));
        }
        if settings
            .extra_fields
            .iter()
            .any(|field| field.id.trim().is_empty())
        {
            return Err(invalid("extraFields", "every extra field needs an id"));
        }
        // Both ids are interpolated into the comma-joined `--fields` of every `workitem view`.
        // One carrying a comma silently reads fields nobody asked for; one carrying a quote or
        // a space makes every view of the board fail with a message that never names the
        // setting it came from. A Jira field id is `summary` or `customfield_10016`.
        for (field, id) in settings
            .extra_fields
            .iter()
            .map(|extra| ("extraFields", extra.id.trim()))
            .chain(
                settings
                    .story_points_field
                    .as_deref()
                    .map(|id| ("storyPointsField", id.trim())),
            )
        {
            if !id.is_empty() && !FIELD_ID.is_match(id) {
                return Err(invalid(
                    field,
                    &format!(
                        "`{id}` is not a Jira field id (a letter or underscore followed by letters, digits or underscores)"
                    ),
                ));
            }
        }
        // `statuses` is handed to `describe` as the board's whole column list. A blank entry
        // becomes a nameless column `validate_board` then refuses, so every later sync of the
        // board fails with a sentence that never names the setting that caused it; a repeated
        // one becomes a second column nothing can ever be mapped to.
        let mut seen: Vec<String> = Vec::new();
        for status in &settings.statuses {
            let name = status.trim();
            if name.is_empty() {
                return Err(invalid("statuses", "a column name must not be empty"));
            }
            let lowered = name.to_lowercase();
            if seen.contains(&lowered) {
                return Err(invalid("statuses", &format!("`{name}` is listed twice")));
            }
            seen.push(lowered);
        }
        Ok(settings)
    }

    /// The JQL every search starts from: the project, narrowed by the optional extra filter.
    #[must_use]
    pub fn base_jql(&self) -> String {
        // Quoted even though `parse` has already refused everything that would need quoting:
        // a key that happens to be a JQL reserved word is legal and still needs it.
        let project = format!("project = \"{}\"", self.project.trim());
        match self
            .jql
            .as_ref()
            .map(|jql| jql.trim())
            .filter(|jql| !jql.is_empty())
        {
            Some(jql) => format!("{project} AND ({jql})"),
            None => project,
        }
    }

    /// The browsable URL of one issue, when the board knows its site.
    #[must_use]
    pub fn browse_url(&self, key: &str) -> Option<String> {
        let site = self.site.as_ref().map(|site| site.trim())?;
        if site.is_empty() {
            return None;
        }
        let host = site
            .trim_end_matches('/')
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        Some(format!("https://{host}/browse/{key}"))
    }
}

/// The generic settings schema clients render, so no client knows a Jira field by name.
#[must_use]
pub fn settings_schema() -> Vec<PropertySchema> {
    let row = |key: &str, name: &str, kind: PropertyKind| PropertySchema {
        key: key.to_owned(),
        name: name.to_owned(),
        kind,
        options: Vec::new(),
        editable: true,
        source: PropertySource::Backend,
        show_on_card: false,
    };
    vec![
        // `PropertySchema` has no `required` flag, and `project` is the one row without a
        // working default: the name carries it so a generic dialog can say so without
        // knowing anything about Jira.
        row("project", "Project key (required)", PropertyKind::Text),
        row("site", "Site", PropertyKind::Text),
        row("jql", "Extra JQL", PropertyKind::Text),
        row("issueType", "Issue type", PropertyKind::Text),
        row("storyPointsField", "Story points field", PropertyKind::Text),
        row("statuses", "Columns", PropertyKind::MultiSelect),
        row("overlapMinutes", "Overlap (minutes)", PropertyKind::Number),
        row("maxConcurrency", "Max concurrency", PropertyKind::Number),
        row("fullSyncEvery", "Full sync every", PropertyKind::Number),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_in_everything_but_the_project_key() {
        let settings = JiraSettings::parse(&serde_json::json!({"project": "SP"})).unwrap();
        assert_eq!(
            settings,
            JiraSettings {
                project: "SP".into(),
                ..JiraSettings::default()
            }
        );
        assert_eq!(settings.overlap_minutes, 10);
        assert_eq!(settings.max_concurrency, 4);
        assert_eq!(settings.full_sync_every, 10);
        assert_eq!(settings.sample_limit, 200);
        assert!(settings.extra_fields.is_empty());
    }

    #[test]
    fn every_documented_key_round_trips() {
        let json = serde_json::json!({
            "project": "SP",
            "site": "buk.atlassian.net",
            "jql": "sprint in openSprints()",
            "issueType": "Task",
            "storyPointsField": "customfield_10102",
            "statuses": ["To Do", "In Progress", "Done"],
            "extraFields": [{"id": "customfield_10001", "name": "Team", "kind": "select"}],
            "overlapMinutes": 5,
            "maxConcurrency": 8,
            "fullSyncEvery": 3,
            "sampleLimit": 50
        });
        let settings = JiraSettings::parse(&json).unwrap();
        assert_eq!(settings.statuses.len(), 3);
        assert_eq!(
            settings.extra_fields,
            vec![ExtraField {
                id: "customfield_10001".into(),
                name: "Team".into(),
                kind: PropertyKind::Select,
            }]
        );
        assert_eq!(serde_json::to_value(&settings).unwrap(), json);
        // An extra field with no `kind` is text, which is what most custom fields render as.
        let defaulted = JiraSettings::parse(&serde_json::json!({
            "project": "SP",
            "extraFields": [{"id": "customfield_1", "name": "Team"}]
        }))
        .unwrap();
        assert_eq!(defaulted.extra_fields[0].kind, PropertyKind::Text);
    }

    /// A saved filter pasted verbatim carries its own ordering, which every search appends to.
    #[test]
    fn a_trailing_order_by_is_dropped_from_the_extra_filter() {
        let settings = JiraSettings::parse(&serde_json::json!({
            "project": "SP",
            "jql": "assignee = currentUser() ORDER BY Rank ASC"
        }))
        .unwrap();
        assert_eq!(settings.jql.as_deref(), Some("assignee = currentUser()"));
        assert_eq!(
            settings.base_jql(),
            "project = \"SP\" AND (assignee = currentUser())"
        );
        // An ordering is all the filter said: what is left is no filter at all.
        let only_order =
            JiraSettings::parse(&serde_json::json!({"project": "SP", "jql": "order by rank"}))
                .unwrap();
        assert_eq!(only_order.jql, None);
        assert_eq!(only_order.base_jql(), "project = \"SP\"");
        // An ordering inside a function call is part of the filter, not a trailing clause.
        let kept = JiraSettings::parse(
            &serde_json::json!({"project": "SP", "jql": "sprint in openSprints()"}),
        )
        .unwrap();
        assert_eq!(kept.jql.as_deref(), Some("sprint in openSprints()"));
    }

    #[test]
    fn a_misspelled_or_missing_key_is_named_in_the_error() {
        // A silently ignored `proyect` would sync a project nobody asked for.
        let error = JiraSettings::parse(&serde_json::json!({"project": "SP", "proyect": "SP"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("proyect"), "{error}");
        // Not serde's "missing field": the friendly branch owns the empty case.
        let error = JiraSettings::parse(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("a Jira project key is required"), "{error}");
        for (json, needle) in [
            (serde_json::json!({}), "project"),
            (serde_json::json!({"project": "  "}), "project"),
            (
                serde_json::json!({"project": "SP", "maxConcurrency": 0}),
                "maxConcurrency",
            ),
            (
                serde_json::json!({"project": "SP", "maxConcurrency": 9}),
                "maxConcurrency",
            ),
            (
                serde_json::json!({"project": "SP", "sampleLimit": 0}),
                "sampleLimit",
            ),
            (
                serde_json::json!({"project": "SP", "extraFields": [{"id": "", "name": "T"}]}),
                "extraFields",
            ),
            // `statuses` is this board's whole column list. A blank entry became a nameless
            // column `validate_board` refuses, so every later sync failed with a sentence that
            // never named the setting behind it; a repeat became a column nothing maps to.
            (
                serde_json::json!({"project": "SP", "statuses": ["To Do", ""]}),
                "statuses",
            ),
            (
                serde_json::json!({"project": "SP", "statuses": ["To Do", " to do "]}),
                "statuses",
            ),
        ] {
            let error = JiraSettings::parse(&json).unwrap_err().to_string();
            assert!(error.contains(needle), "{needle} missing from {error}");
        }
        // A null settings object is an empty one, so the message is still about `project`.
        assert!(
            JiraSettings::parse(&serde_json::Value::Null)
                .unwrap_err()
                .to_string()
                .contains("project")
        );
    }

    #[test]
    fn base_jql_ands_the_extra_filter_inside_parentheses() {
        let mut settings = JiraSettings::parse(&serde_json::json!({"project": "SP"})).unwrap();
        assert_eq!(settings.base_jql(), "project = \"SP\"");
        settings.jql = Some("  ".into());
        assert_eq!(settings.base_jql(), "project = \"SP\"");
        // The parentheses matter: `a OR b` ANDed bare would widen the search to the whole site.
        settings.jql = Some("labels = x OR labels = y".into());
        assert_eq!(
            settings.base_jql(),
            "project = \"SP\" AND (labels = x OR labels = y)"
        );
    }

    #[test]
    fn browse_url_needs_a_site_and_tolerates_a_pasted_one() {
        let mut settings = JiraSettings::parse(&serde_json::json!({"project": "SP"})).unwrap();
        assert_eq!(settings.browse_url("SP-1"), None);
        settings.site = Some("buk.atlassian.net".into());
        assert_eq!(
            settings.browse_url("SP-1").as_deref(),
            Some("https://buk.atlassian.net/browse/SP-1")
        );
        // Users paste the address bar, not the host.
        settings.site = Some("https://buk.atlassian.net/".into());
        assert_eq!(
            settings.browse_url("SP-1").as_deref(),
            Some("https://buk.atlassian.net/browse/SP-1")
        );
        settings.site = Some("  ".into());
        assert_eq!(settings.browse_url("SP-1"), None);
    }

    /// The project key is the one JQL fragment that comes from the user and is interpolated.
    #[test]
    fn a_project_key_that_is_not_a_key_is_refused_by_name() {
        for key in ["SP OR project = OTHER", "S P", "SP\"", "1SP", "SP-1"] {
            let error = JiraSettings::parse(&serde_json::json!({"project": key}))
                .expect_err("{key} is not a project key");
            assert!(
                matches!(&error, BoardError::Invalid { field, .. } if field == "project"),
                "{key}: {error}"
            );
        }
        assert!(JiraSettings::parse(&serde_json::json!({"project": "SP_2"})).is_ok());
    }

    /// `0` makes every pull a full one, and an unbounded overlap makes every incremental JQL
    /// one Jira refuses; both are as unusable as the `maxConcurrency` already bounded above.
    #[test]
    fn the_pull_windows_are_bounded_the_way_concurrency_is() {
        let zero = JiraSettings::parse(&serde_json::json!({"project": "SP", "fullSyncEvery": 0}))
            .expect_err("every pull would be a full one");
        assert!(matches!(&zero, BoardError::Invalid { field, .. } if field == "fullSyncEvery"));
        let wide = JiraSettings::parse(
            &serde_json::json!({"project": "SP", "overlapMinutes": 4_294_967_295_u32}),
        )
        .expect_err("Jira refuses a window that wide");
        assert!(matches!(&wide, BoardError::Invalid { field, .. } if field == "overlapMinutes"));
        assert!(
            JiraSettings::parse(&serde_json::json!({"project": "SP", "overlapMinutes": 1440}))
                .is_ok()
        );
        // Zero is the one value that removes the protection the setting exists for: Jira's
        // `updated` has minute resolution and the window is evaluated on the site's clock.
        let none = JiraSettings::parse(&serde_json::json!({"project": "SP", "overlapMinutes": 0}))
            .expect_err("a window with no overlap hides a whole minute of edits");
        assert!(matches!(&none, BoardError::Invalid { field, .. } if field == "overlapMinutes"));
    }

    /// `project` is regex-guarded against widening the board past the project it names; the
    /// extra filter is interpolated into `( … )` and has to be guarded against closing it.
    #[test]
    fn a_filter_that_could_close_its_own_parenthesis_is_refused() {
        for jql in [
            "1=1) OR (project = \"OTHER\"",
            "(labels = a",
            "summary ~ \"unclosed",
        ] {
            let error = JiraSettings::parse(&serde_json::json!({"project": "SP", "jql": jql}))
                .err()
                .unwrap_or_else(|| panic!("{jql} widens the board past its project"));
            assert!(matches!(&error, BoardError::Invalid { field, .. } if field == "jql"));
        }
        // A filter that balances its own groups and quotes is still the user's to write.
        let ok = JiraSettings::parse(&serde_json::json!({
            "project": "SP",
            "jql": "(labels = a OR labels = b) AND summary ~ \"a (b) c\""
        }))
        .expect("a balanced filter is accepted");
        assert_eq!(
            ok.base_jql(),
            "project = \"SP\" AND ((labels = a OR labels = b) AND summary ~ \"a (b) c\")"
        );
    }

    /// The same words inside a quoted literal are the user's search text, not a clause.
    #[test]
    fn an_order_by_inside_a_quoted_literal_is_left_alone() {
        let settings = JiraSettings::parse(&serde_json::json!({
            "project": "SP",
            "jql": "summary ~ \"order by date\""
        }))
        .unwrap();
        assert_eq!(
            settings.jql.as_deref(),
            Some("summary ~ \"order by date\""),
            "cutting here leaves an unbalanced quote and fails every search"
        );
        // A real trailing clause after one is still dropped.
        let both = JiraSettings::parse(&serde_json::json!({
            "project": "SP",
            "jql": "summary ~ \"order by date\" ORDER BY rank ASC"
        }))
        .unwrap();
        assert_eq!(both.jql.as_deref(), Some("summary ~ \"order by date\""));
    }

    #[test]
    fn the_schema_names_every_key_the_settings_accept() {
        let schema = settings_schema();
        let keys: Vec<_> = schema.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "project",
                "site",
                "jql",
                "issueType",
                "storyPointsField",
                "statuses",
                "overlapMinutes",
                "maxConcurrency",
                "fullSyncEvery"
            ]
        );
        assert!(schema[0].name.contains("required"));
        // Every row must be renderable and nameable: an empty name draws a blank row.
        for row in &schema {
            assert!(!row.name.is_empty(), "{} has no name", row.key);
            assert!(row.editable, "{} cannot be typed into", row.key);
        }
        // Every schema key must be a key `JiraSettings` accepts, or the dialog writes settings
        // `parse` then rejects.
        let mut object = serde_json::Map::new();
        for row in &schema {
            object.insert(
                row.key.clone(),
                match row.kind {
                    PropertyKind::Number => serde_json::json!(1),
                    PropertyKind::MultiSelect => serde_json::json!(["To Do"]),
                    _ => serde_json::json!("x"),
                },
            );
        }
        object.insert("project".into(), serde_json::json!("SP"));
        JiraSettings::parse(&serde_json::Value::Object(object)).unwrap();
    }
}
