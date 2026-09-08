//! Typed custom property schemas and values.
use super::ops::default_true;
use serde::{Deserialize, Serialize};
/// Schema for a backend-independent custom property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertySchema {
    /// Key.
    pub key: String,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: PropertyKind,
    /// Options.
    #[serde(default)]
    pub options: Vec<PropertyOption>,
    /// Editable.
    #[serde(default = "default_true")]
    pub editable: bool,
    /// Source.
    #[serde(default)]
    pub source: PropertySource,
    /// Show on card.
    #[serde(default)]
    pub show_on_card: bool,
}
/// The suffix a settings schema row's name carries when the row has no working default.
///
/// `PropertySchema` has no `required` flag and gains none for this: the marker lives in the name
/// so a generic settings dialog can say "required" without knowing what backend it is drawing.
/// It is also what identifies a backend's *identity* rows — the ones a board cannot change under
/// its own linked cards — so the daemon and the clients must read it the same way.
pub const REQUIRED_MARKER: &str = "(required)";

/// Whether a settings schema row says it has no working default.
#[must_use]
pub fn is_required(schema: &PropertySchema) -> bool {
    schema.name.trim_end().ends_with(REQUIRED_MARKER)
}

/// A settings schema row's label: its name with [`REQUIRED_MARKER`] stripped.
#[must_use]
pub fn schema_row_name(schema: &PropertySchema) -> String {
    schema
        .name
        .trim_end()
        .trim_end_matches(REQUIRED_MARKER)
        .trim_end()
        .to_owned()
}

/// One selectable property value and its presentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyOption {
    /// Value.
    pub value: String,
    /// Label.
    pub label: String,
    /// Color.
    #[serde(default)]
    pub color: Option<String>,
}
/// The value kind accepted by a custom property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PropertyKind {
    /// Text.
    #[default]
    Text,
    /// Number.
    Number,
    /// Bool.
    Bool,
    /// Date.
    Date,
    /// Select.
    Select,
    /// Multi select.
    MultiSelect,
    /// User.
    User,
    /// Url.
    Url,
}
/// Whether a property is locally defined or backend-managed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PropertySource {
    /// Local.
    #[default]
    Local,
    /// Backend.
    Backend,
}
/// Tagged custom property data, including a null clearing sentinel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PropertyValue {
    /// Text.
    Text(String),
    /// Number.
    Number(f64),
    /// Bool.
    Bool(bool),
    /// Date.
    Date(String),
    /// Select.
    Select(String),
    /// Multi select.
    MultiSelect(Vec<String>),
    /// User.
    User(String),
    /// Url.
    Url(String),
    /// Null.
    Null,
}
impl PropertyValue {
    /// Formats a property for a generic UI field.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Text(s) | Self::Date(s) | Self::Select(s) | Self::User(s) | Self::Url(s) => {
                s.clone()
            }
            Self::Number(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
            Self::MultiSelect(values) => values.join(", "),
            Self::Null => String::new(),
        }
    }
    /// Whether this value can inhabit the given schema kind; null clears any kind.
    #[must_use]
    pub fn matches_kind(&self, kind: PropertyKind) -> bool {
        matches!(
            (self, kind),
            (Self::Null, _)
                | (Self::Text(_), PropertyKind::Text)
                | (Self::Number(_), PropertyKind::Number)
                | (Self::Bool(_), PropertyKind::Bool)
                | (Self::Date(_), PropertyKind::Date)
                | (Self::Select(_), PropertyKind::Select)
                | (Self::MultiSelect(_), PropertyKind::MultiSelect)
                | (Self::User(_), PropertyKind::User)
                | (Self::Url(_), PropertyKind::Url)
        )
    }
}
