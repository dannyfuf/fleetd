//! Jira JSON ⇄ the backend-independent board model. Pure: no I/O, no clock, no shell.

use super::{
    adf::{adf_plain_text, adf_to_markdown},
    settings::JiraSettings,
};
use chrono::{DateTime, FixedOffset, SecondsFormat, TimeZone, Utc};
use fleet_core::board::{
    BoardError, Priority, PropertyKind, PropertyValue, RemoteCard, RemoteComment, RemoteStatus,
    StatusCategory,
};
use regex::Regex;
use std::{collections::BTreeMap, sync::LazyLock};

/// The fields every `workitem view` asks for; a board adds its story-points and extra fields.
///
/// `search` returns only a fraction of these, which is why a pull costs one `view` per key.
pub const VIEW_FIELDS: &[&str] = &[
    "summary",
    "description",
    "status",
    "priority",
    "labels",
    "assignee",
    "duedate",
    "parent",
    "updated",
    "created",
    "comment",
    "issuetype",
];

/// The property key prefix every backend-owned Jira property is namespaced under.
pub const PROPERTY_PREFIX: &str = "jira.";
/// The property holding `issuetype.name`.
pub const ISSUE_TYPE_KEY: &str = "jira.issue_type";
/// The property holding the issue's creation day.
pub const CREATED_KEY: &str = "jira.created";

/// The property key one configured extra field is surfaced under.
#[must_use]
pub fn extra_field_key(id: &str) -> String {
    format!("{PROPERTY_PREFIX}{id}")
}

/// The `--fields` value one board's `workitem view` needs: the standard set plus its own.
#[must_use]
pub fn view_fields(settings: &JiraSettings) -> String {
    let mut fields: Vec<&str> = VIEW_FIELDS.to_vec();
    if let Some(story_points) = settings
        .story_points_field
        .as_deref()
        .map(str::trim)
        .filter(|field| !field.is_empty())
    {
        fields.push(story_points);
    }
    for extra in &settings.extra_fields {
        let id = extra.id.trim();
        if !id.is_empty() && !fields.contains(&id) {
            fields.push(id);
        }
    }
    fields.join(",")
}

/// Where an incremental pull resumes, and how many pulls are left before a full one.
///
/// Jira has no cursor pagination, so "the cursor" is a watermark on `updated` plus the count
/// that forces a periodic full pull — the only thing that ever notices a deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// The instant the last pull started; the next one re-scans from here minus the overlap.
    pub watermark: DateTime<Utc>,
    /// Incremental pulls performed since the last full one.
    pub pulls_since_full: u32,
}

/// Maps one `workitem view --json` payload onto a [`RemoteCard`].
pub fn issue_to_remote_card(
    issue: &serde_json::Value,
    settings: &JiraSettings,
) -> Result<RemoteCard, BoardError> {
    // `acli` wraps the issue the way the REST API does, but a caller that already unwrapped
    // `fields` is holding the same issue: read whichever shape arrived.
    let fields = issue.get("fields").unwrap_or(issue);
    let key = issue
        .get("key")
        .or_else(|| fields.get("key"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| BoardError::Backend("acli returned an issue without a key".into()))?;
    let status = fields.get("status");
    let status_name = text(status.and_then(|status| status.get("name"))).unwrap_or_default();
    let category_key = text(
        status
            .and_then(|status| status.get("statusCategory"))
            .and_then(|category| category.get("key")),
    )
    .unwrap_or_default();
    let updated = text(fields.get("updated"));
    // `updated` is the version the conflict check compares *and* the `remoteUpdatedAt` the
    // board document stores and `board show --json` publishes, where the contract says RFC3339.
    // A value that is not a timestamp would be persisted and shipped verbatim, so this key
    // fails here — named, like any other issue this pull could not read — instead.
    if let Some(updated) = updated.as_deref()
        && parse_time(updated).is_none()
    {
        return Err(BoardError::Backend(format!(
            "`updated` is not a timestamp: {updated}"
        )));
    }
    let mut properties = BTreeMap::new();
    if let Some(issue_type) = text(
        fields
            .get("issuetype")
            .and_then(|issue_type| issue_type.get("name")),
    ) {
        properties.insert(ISSUE_TYPE_KEY.to_owned(), PropertyValue::Select(issue_type));
    }
    // `get`, never a byte slice: a `created` whose tenth byte falls inside a multibyte character
    // would panic the pull task rather than skip one property.
    if let Some(created) = text(fields.get("created"))
        .as_deref()
        .and_then(|created| created.get(..10))
    {
        properties.insert(
            CREATED_KEY.to_owned(),
            PropertyValue::Date(created.to_owned()),
        );
    }
    for extra in &settings.extra_fields {
        if let Some(value) = fields
            .get(extra.id.trim())
            .and_then(|value| extra_value(extra.kind, value))
        {
            properties.insert(extra_field_key(extra.id.trim()), value);
        }
    }
    Ok(RemoteCard {
        url: settings.browse_url(&key),
        // Jira's `updated` is the only monotonic thing an issue carries; it is both the
        // version the conflict check compares and the watermark a pull resumes from.
        version: updated.clone(),
        updated_at: updated.as_deref().map(normalize_time),
        title: text(fields.get("summary")).unwrap_or_default(),
        description: fields
            .get("description")
            .filter(|description| !description.is_null())
            .map(adf_to_markdown)
            .unwrap_or_default(),
        status: RemoteStatus {
            // Transitions move by status *name*, so the name is the identity as well.
            id: status_name.clone(),
            name: status_name.clone(),
            category: Some(status_category(&category_key, &status_name)),
        },
        priority: Some(priority_from_jira(
            text(
                fields
                    .get("priority")
                    .and_then(|priority| priority.get("name")),
            )
            .as_deref(),
        )),
        labels: fields
            .get("labels")
            .and_then(serde_json::Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| text(Some(label)))
                    .collect()
            })
            .unwrap_or_default(),
        assignee: text(
            fields
                .get("assignee")
                .and_then(|assignee| assignee.get("displayName")),
        ),
        estimate: settings
            .story_points_field
            .as_deref()
            .map(str::trim)
            .and_then(|field| fields.get(field))
            // Sites — and some `acli` versions — answer a numeric custom field as a JSON
            // string. Reading only `as_f64` dropped the estimate on every pull, and `estimate`
            // is read-only on a Jira board, so nothing could ever put it back.
            .and_then(|value| {
                value.as_f64().or_else(|| {
                    value
                        .as_str()
                        .map(str::trim)
                        .and_then(|points| points.parse::<f64>().ok())
                })
            })
            .filter(|points| points.is_finite())
            .map(|points| points.round().clamp(0.0, f64::from(u32::MAX)) as u32),
        due_date: text(fields.get("duedate")),
        parent_key: text(fields.get("parent").and_then(|parent| parent.get("key"))),
        properties,
        comments: fields
            .get("comment")
            .and_then(|comment| comment.get("comments"))
            .and_then(serde_json::Value::as_array)
            .map(|comments| comments.iter().map(remote_comment).collect())
            .unwrap_or_default(),
        key,
    })
}

/// Maps one comment payload; a comment without an id is still a comment worth showing.
fn remote_comment(comment: &serde_json::Value) -> RemoteComment {
    RemoteComment {
        id: text(comment.get("id")).unwrap_or_default(),
        author: text(
            comment
                .get("author")
                .and_then(|author| author.get("displayName")),
        ),
        body: comment
            .get("body")
            .filter(|body| !body.is_null())
            .map(adf_to_markdown)
            .unwrap_or_default(),
        created_at: text(comment.get("created"))
            .as_deref()
            .map(normalize_time)
            .unwrap_or_default(),
    }
}

/// The display name every account in an issue payload is known by, with its account id.
///
/// Both `assignee` and every comment author carry one, and a push can only address the id.
#[must_use]
pub fn issue_users(issue: &serde_json::Value) -> Vec<(String, String, Option<String>)> {
    let fields = issue.get("fields").unwrap_or(issue);
    let comments = fields
        .get("comment")
        .and_then(|comment| comment.get("comments"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    std::iter::once(fields.get("assignee"))
        .chain(comments.iter().map(|comment| comment.get("author")))
        .flatten()
        .filter_map(|user| {
            Some((
                text(user.get("displayName"))?,
                text(user.get("accountId"))?,
                text(user.get("emailAddress")),
            ))
        })
        .collect()
}

/// Reads one extra field into the property kind the board declared for it.
fn extra_value(kind: PropertyKind, value: &serde_json::Value) -> Option<PropertyValue> {
    if value.is_null() {
        return None;
    }
    let named = |value: &serde_json::Value| {
        value
            .as_str()
            .map(str::to_owned)
            .or_else(|| text(value.get("value")))
            .or_else(|| text(value.get("name")))
            .or_else(|| text(value.get("displayName")))
    };
    Some(match kind {
        // The same string-encoded number story points already answer to: a site — or an `acli`
        // version — that answers a numeric custom field as a JSON string would otherwise drop
        // the property from every card, and a backend-owned property is one nothing local can
        // supply in its place.
        PropertyKind::Number => PropertyValue::Number(value.as_f64().or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .and_then(|number| number.parse::<f64>().ok())
        })?),
        PropertyKind::Bool => PropertyValue::Bool(value.as_bool()?),
        PropertyKind::Date => {
            let date = value.as_str()?;
            PropertyValue::Date(date.get(..10)?.to_owned())
        }
        PropertyKind::Select => PropertyValue::Select(named(value)?),
        PropertyKind::MultiSelect => PropertyValue::MultiSelect(
            value
                .as_array()?
                .iter()
                .filter_map(named)
                .collect::<Vec<_>>(),
        ),
        PropertyKind::User => PropertyValue::User(named(value)?),
        PropertyKind::Url => PropertyValue::Url(value.as_str()?.to_owned()),
        // A custom field's value is as likely to be an ADF document as a string, and a
        // serialized `{"type":"doc"…}` in a one-line field shows the user nothing.
        PropertyKind::Text => PropertyValue::Text(match value {
            serde_json::Value::String(text) => text.clone(),
            object if object.get("type").and_then(serde_json::Value::as_str) == Some("doc") => {
                adf_plain_text(object)
            }
            other => named(other).unwrap_or_else(|| other.to_string()),
        }),
    })
}

/// Names Jira files under `new` that are really a backlog rather than a queue.
static BACKLOG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)backlog|icebox|pendiente").expect("static regex"));
/// Names Jira files under `done` that mean the work never happened.
static CANCELED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)cancel|descart|won'?t|rechaz").expect("static regex"));
/// Names that read as work in flight.
static STARTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(?i)progress|review|doing|curso|desarrollo|qa").expect("static regex")
});
/// Names that read as work finished.
static COMPLETED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(?i)done|closed|resolved|complet|listo|cerrad|termin").expect("static regex")
});

/// Maps a Jira status category key and name onto a board category.
///
/// The name matters as much as the key: Jira files "Cancelled" under `done`, and a board that
/// shows it as completed reports work that never happened.
#[must_use]
pub fn status_category(key: &str, name: &str) -> StatusCategory {
    match key {
        "new" if BACKLOG.is_match(name) => StatusCategory::Backlog,
        "new" => StatusCategory::Unstarted,
        "indeterminate" => StatusCategory::Started,
        "done" if CANCELED.is_match(name) => StatusCategory::Canceled,
        "done" => StatusCategory::Completed,
        // A board may name its columns without ever asking Jira for their categories.
        _ => guess_category(name),
    }
}

/// Guesses a category from a status name alone, for a board that lists its own columns.
#[must_use]
pub fn guess_category(name: &str) -> StatusCategory {
    if CANCELED.is_match(name) {
        StatusCategory::Canceled
    } else if BACKLOG.is_match(name) {
        StatusCategory::Backlog
    } else if STARTED.is_match(name) {
        StatusCategory::Started
    } else if COMPLETED.is_match(name) {
        StatusCategory::Completed
    } else {
        StatusCategory::Unstarted
    }
}

/// Names a renamed priority scheme uses for its top level.
static URGENT_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)block|critic|crític|urgen|sev.?1|p0").expect("static regex"));
/// Names that read as above-normal.
static HIGH_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)high|alta|major|p1").expect("static regex"));
/// Names that read as below-normal.
static LOW_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)low|baja|minor|trivial|p3|p4").expect("static regex"));

/// The board priority one Jira priority name means.
///
/// The five default names are matched case-insensitively; a site with a renamed scheme
/// ("Blocker", "Critical", "Trivial") is read by name, because `priority` is read-only here and
/// a card whose priority this function drops cannot be corrected locally either.
#[must_use]
pub fn priority_from_jira(name: Option<&str>) -> Priority {
    let name = name.map(str::trim).unwrap_or_default();
    match name.to_lowercase().as_str() {
        "highest" => Priority::Urgent,
        "high" => Priority::High,
        "medium" | "normal" => Priority::Medium,
        // Jira's two low priorities collapse: the board has one, and a card that came back
        // as `Lowest` must still push back as something Jira accepts.
        "low" | "lowest" => Priority::Low,
        "" | "none" => Priority::None,
        _ if URGENT_NAME.is_match(name) => Priority::Urgent,
        _ if HIGH_NAME.is_match(name) => Priority::High,
        _ if LOW_NAME.is_match(name) => Priority::Low,
        _ => Priority::None,
    }
}

/// The Jira priority name for a board priority.
#[must_use]
pub fn priority_to_jira(priority: Priority) -> &'static str {
    match priority {
        Priority::Urgent => "Highest",
        Priority::High => "High",
        // Jira's default for an issue nobody prioritized is `Medium`, and there is no way to
        // spell "no priority" on a site whose field is required.
        Priority::Medium | Priority::None => "Medium",
        Priority::Low => "Low",
    }
}

/// Builds the search JQL for a full pull, or for one narrowed to the last `since_minutes`.
#[must_use]
pub fn build_search_jql(settings: &JiraSettings, since_minutes: Option<u32>) -> String {
    let mut jql = settings.base_jql();
    if let Some(minutes) = since_minutes {
        jql.push_str(&format!(" AND updated >= \"-{minutes}m\""));
    }
    // Ascending, so a pull that dies halfway has still consumed the oldest changes and the
    // watermark it leaves behind is honest.
    jql.push_str(" ORDER BY updated ASC");
    jql
}

/// Parses a stored cursor; an absent or unreadable one means "pull everything".
#[must_use]
pub fn parse_cursor(cursor: Option<&str>) -> Cursor {
    // A cursor nobody can read is a cursor nobody can resume from: counting it as overdue
    // for a full pull is the only reading that cannot silently skip issues.
    let overdue = Cursor {
        watermark: Utc.timestamp_opt(0, 0).single().unwrap_or_default(),
        pulls_since_full: u32::MAX,
    };
    let Some((watermark, pulls)) = cursor.and_then(|cursor| cursor.split_once('|')) else {
        return overdue;
    };
    match (
        DateTime::parse_from_rfc3339(watermark.trim()),
        pulls.trim().parse::<u32>(),
    ) {
        (Ok(watermark), Ok(pulls_since_full)) => Cursor {
            watermark: watermark.with_timezone(&Utc),
            pulls_since_full,
        },
        _ => overdue,
    }
}

/// Renders a cursor as `<rfc3339>|<n>`.
#[must_use]
pub fn render_cursor(cursor: &Cursor) -> String {
    format!(
        "{}|{}",
        cursor.watermark.to_rfc3339_opts(SecondsFormat::Secs, true),
        cursor.pulls_since_full
    )
}

/// Normalizes a Jira timestamp to RFC3339, keeping the site's offset.
///
/// Jira prints `2026-09-05T14:32:11.000-0300`, which is not RFC3339 (the offset has no colon)
/// and which every local comparison would then treat as an opaque string.
pub fn normalize_time(value: &str) -> String {
    parse_time(value).map_or_else(|| value.to_owned(), |time| time.to_rfc3339())
}

/// Parses either RFC3339 or the offset-without-colon shape Jira actually prints.
#[must_use]
pub fn parse_time(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value.trim())
        .or_else(|_| DateTime::parse_from_str(value.trim(), "%Y-%m-%dT%H:%M:%S%.f%z"))
        .ok()
}

/// Reads a JSON string field, dropping empty and absent ones alike.
fn text(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::super::settings::ExtraField;
    use super::*;

    fn settings() -> JiraSettings {
        JiraSettings::parse(&serde_json::json!({
            "project": "SP",
            "site": "buk.atlassian.net",
            "storyPointsField": "customfield_10102"
        }))
        .unwrap()
    }

    #[test]
    fn the_view_field_list_carries_the_board_s_own_fields_once() {
        assert_eq!(
            view_fields(&JiraSettings::parse(&serde_json::json!({"project": "SP"})).unwrap()),
            VIEW_FIELDS.join(",")
        );
        let settings = JiraSettings::parse(&serde_json::json!({
            "project": "SP",
            "storyPointsField": "customfield_10102",
            "extraFields": [
                {"id": "customfield_10102", "name": "Points"},
                {"id": "customfield_10001", "name": "Team"}
            ]
        }))
        .unwrap();
        assert_eq!(
            view_fields(&settings),
            format!(
                "{},customfield_10102,customfield_10001",
                VIEW_FIELDS.join(",")
            )
        );
    }

    #[test]
    fn jira_status_categories_follow_the_key_and_the_name() {
        assert_eq!(status_category("new", "To Do"), StatusCategory::Unstarted);
        assert_eq!(status_category("new", "Backlog"), StatusCategory::Backlog);
        assert_eq!(
            status_category("indeterminate", "In Progress"),
            StatusCategory::Started
        );
        assert_eq!(status_category("done", "Done"), StatusCategory::Completed);
        // Jira files every terminal status under `done`, cancellations included.
        for name in [
            "Cancelled",
            "Canceled",
            "Won't Do",
            "Wont do",
            "Descartado",
            "Rechazado",
        ] {
            assert_eq!(
                status_category("done", name),
                StatusCategory::Canceled,
                "{name}"
            );
        }
        // An unknown key is what a board that lists its own columns has: guess by name.
        assert_eq!(status_category("", "In Review"), StatusCategory::Started);
        assert_eq!(status_category("", "Closed"), StatusCategory::Completed);
        assert_eq!(status_category("", "Ready"), StatusCategory::Unstarted);
    }

    #[test]
    fn priorities_round_trip_through_the_names_jira_accepts() {
        for (name, priority) in [
            ("Highest", Priority::Urgent),
            ("High", Priority::High),
            ("Medium", Priority::Medium),
            ("Low", Priority::Low),
            ("Lowest", Priority::Low),
        ] {
            assert_eq!(priority_from_jira(Some(name)), priority);
        }
        assert_eq!(priority_from_jira(None), Priority::None);
        // A renamed scheme is read by name rather than erased: `priority` is read-only, so a
        // dropped one cannot be put back locally either.
        for (name, priority) in [
            ("blocker", Priority::Urgent),
            ("Critical", Priority::Urgent),
            ("Major", Priority::High),
            ("Trivial", Priority::Low),
            ("MEDIUM", Priority::Medium),
            ("Undefined", Priority::None),
        ] {
            assert_eq!(priority_from_jira(Some(name)), priority, "{name}");
        }
        for priority in Priority::ALL {
            // Whatever a card holds must be a name Jira takes back.
            assert!(
                ["Highest", "High", "Medium", "Low"].contains(&priority_to_jira(priority)),
                "{priority:?}"
            );
        }
        assert_eq!(priority_to_jira(Priority::None), "Medium");
    }

    #[test]
    fn the_search_jql_narrows_by_project_then_filter_then_window() {
        let mut settings = settings();
        assert_eq!(
            build_search_jql(&settings, None),
            "project = \"SP\" ORDER BY updated ASC"
        );
        settings.jql = Some("sprint in openSprints()".into());
        assert_eq!(
            build_search_jql(&settings, Some(70)),
            "project = \"SP\" AND (sprint in openSprints()) AND updated >= \"-70m\" ORDER BY updated ASC"
        );
    }

    #[test]
    fn a_cursor_round_trips_and_an_unreadable_one_forces_a_full_pull() {
        let cursor = Cursor {
            watermark: Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).single().unwrap(),
            pulls_since_full: 3,
        };
        assert_eq!(render_cursor(&cursor), "2026-09-06T12:00:00Z|3");
        assert_eq!(parse_cursor(Some(&render_cursor(&cursor))), cursor);
        for broken in [
            None,
            Some(""),
            Some("nonsense"),
            Some("2026-09-06T12:00:00Z"),
            Some("nope|3"),
            Some("2026-09-06T12:00:00Z|x"),
        ] {
            assert_eq!(
                parse_cursor(broken).pulls_since_full,
                u32::MAX,
                "{broken:?} must be overdue for a full pull"
            );
        }
    }

    #[test]
    fn an_issue_maps_onto_a_remote_card_field_by_field() {
        let issue: serde_json::Value =
            serde_json::from_str(include_str!("../../../../tests/fixtures/jira/issue.json"))
                .unwrap();
        let mut settings = settings();
        settings.extra_fields = vec![ExtraField {
            id: "customfield_10001".into(),
            name: "Team".into(),
            kind: PropertyKind::Select,
        }];
        let card = issue_to_remote_card(&issue, &settings).unwrap();
        assert_eq!(card.key, "SP-42");
        assert_eq!(
            card.url.as_deref(),
            Some("https://buk.atlassian.net/browse/SP-42")
        );
        assert_eq!(card.title, "Rework the payroll export");
        assert_eq!(card.status.id, "In Progress");
        assert_eq!(card.status.category, Some(StatusCategory::Started));
        assert_eq!(card.priority, Some(Priority::High));
        assert_eq!(card.labels, ["payroll", "sector-publico"]);
        assert_eq!(card.assignee.as_deref(), Some("Danny Fuentes"));
        assert_eq!(card.estimate, Some(3));
        assert_eq!(card.due_date.as_deref(), Some("2026-09-30"));
        assert_eq!(card.parent_key.as_deref(), Some("SP-1"));
        assert_eq!(
            card.version.as_deref(),
            Some("2026-09-05T14:32:11.000-0300")
        );
        assert_eq!(
            card.updated_at.as_deref(),
            Some("2026-09-05T14:32:11-03:00")
        );
        assert_eq!(
            card.properties.get(ISSUE_TYPE_KEY),
            Some(&PropertyValue::Select("Task".into()))
        );
        assert_eq!(
            card.properties.get(CREATED_KEY),
            Some(&PropertyValue::Date("2026-09-01".into()))
        );
        assert_eq!(
            card.properties.get("jira.customfield_10001"),
            Some(&PropertyValue::Select("Sector Público".into()))
        );
        assert_eq!(card.comments.len(), 2);
        assert_eq!(card.comments[0].id, "10501");
        assert_eq!(card.comments[0].author.as_deref(), Some("Ana Rojas"));
        assert_eq!(card.comments[1].created_at, "2026-09-05T14:30:00-03:00");
        // Every account the issue named is one a push can address.
        assert_eq!(
            issue_users(&issue),
            vec![
                (
                    "Danny Fuentes".to_owned(),
                    "5b10a2844c".to_owned(),
                    Some("danny@example.com".to_owned())
                ),
                ("Ana Rojas".to_owned(), "5b10ac8d82".to_owned(), None),
                (
                    "Danny Fuentes".to_owned(),
                    "5b10a2844c".to_owned(),
                    Some("danny@example.com".to_owned())
                ),
            ]
        );
    }

    #[test]
    fn an_issue_without_optional_fields_maps_to_an_empty_card_not_an_error() {
        let card = issue_to_remote_card(
            &serde_json::json!({"key": "SP-9", "fields": {"summary": "Bare", "description": null}}),
            &settings(),
        )
        .unwrap();
        assert_eq!(card.title, "Bare");
        assert_eq!(card.description, "");
        assert_eq!(card.priority, Some(Priority::None));
        assert!(card.labels.is_empty() && card.comments.is_empty());
        assert_eq!(card.estimate, None);
        assert_eq!(card.status.name, "");
        // An issue with no key is the one shape nothing downstream can hold.
        assert!(issue_to_remote_card(&serde_json::json!({"fields": {}}), &settings()).is_err());
    }

    /// `updated` becomes the link's `version` and the `remoteUpdatedAt` the snapshot publishes
    /// as RFC3339. Accepting a value that is not a timestamp persists and ships it verbatim.
    #[test]
    fn an_updated_stamp_that_is_not_a_timestamp_fails_its_own_key() {
        let error = issue_to_remote_card(
            &serde_json::json!({"key": "SP-9", "fields": {"summary": "Bare", "updated": "not-a-date"}}),
            &settings(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("not-a-date"), "{error}");
        // The shape Jira actually prints — an offset with no colon — is still accepted, and
        // normalized on the way in.
        let card = issue_to_remote_card(
            &serde_json::json!({"key": "SP-9", "fields": {"summary": "Bare", "updated": "2026-09-06T10:00:00.000+0000"}}),
            &settings(),
        )
        .unwrap();
        assert_eq!(
            card.updated_at.as_deref(),
            Some("2026-09-06T10:00:00+00:00")
        );
    }

    /// The same string-encoded number story points already answer to. A backend-owned property
    /// is one nothing local can supply, so dropping it means the card never shows it at all.
    #[test]
    fn a_number_property_answered_as_a_string_still_reads_as_a_number() {
        let mut settings = settings();
        settings.story_points_field = Some("customfield_10102".into());
        settings.extra_fields = vec![ExtraField {
            id: "customfield_10500".into(),
            name: "Impact".into(),
            kind: PropertyKind::Number,
        }];
        let issue = serde_json::json!({
            "key": "SP-9",
            "fields": {
                "summary": "Task",
                "customfield_10102": "3",
                "customfield_10500": " 7.5 "
            }
        });
        let card = issue_to_remote_card(&issue, &settings).unwrap();
        assert_eq!(card.estimate, Some(3));
        assert_eq!(
            card.properties.get("jira.customfield_10500"),
            Some(&PropertyValue::Number(7.5))
        );
        // A string that is not a number is still no value at all, not a zero.
        let issue = serde_json::json!({
            "key": "SP-9",
            "fields": { "summary": "Task", "customfield_10500": "n/a" }
        });
        assert_eq!(
            issue_to_remote_card(&issue, &settings)
                .unwrap()
                .properties
                .get("jira.customfield_10500"),
            None
        );
    }

    /// A `created` whose tenth byte falls inside a character used to panic the pull task.
    #[test]
    fn a_multibyte_created_stamp_skips_the_property_instead_of_panicking() {
        let settings = JiraSettings::parse(&serde_json::json!({"project": "SP"})).unwrap();
        let issue = serde_json::json!({
            "key": "SP-1",
            "fields": { "summary": "Task", "created": "2026-01-0\u{5e9c}T00:00:00.000+0000" }
        });
        let card = issue_to_remote_card(&issue, &settings).unwrap();
        assert_eq!(card.key, "SP-1");
        assert_eq!(card.properties.get(CREATED_KEY), None);
        // A well-formed stamp still lands.
        let plain = serde_json::json!({
            "key": "SP-2",
            "fields": { "summary": "Task", "created": "2026-01-05T00:00:00.000+0000" }
        });
        assert_eq!(
            issue_to_remote_card(&plain, &settings)
                .unwrap()
                .properties
                .get(CREATED_KEY),
            Some(&PropertyValue::Date("2026-01-05".into()))
        );
    }
}
