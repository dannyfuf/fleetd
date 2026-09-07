use fleet_core::inspection::WorktreeInspection;

/// One entry of the client's inspection cache.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspected {
    /// The daemon's facts, absent until the first answer lands.
    pub data: Option<WorktreeInspection>,
    /// A transport or job failure, shown verbatim (§3.4).
    pub error: Option<String>,
    /// Whether a refresh is in flight; previous values dim but never blank.
    pub loading: bool,
}

impl Inspected {
    /// Transport failures take precedence over errors returned inside the inspection payload.
    pub(crate) fn failure(&self) -> Option<&str> {
        self.error
            .as_deref()
            .or_else(|| self.data.as_ref().and_then(|data| data.error.as_deref()))
    }

    /// A slot with fresh facts.
    #[must_use]
    pub fn ready(data: WorktreeInspection) -> Self {
        Self {
            data: Some(data),
            error: None,
            loading: false,
        }
    }

    /// A slot whose inspection failed.
    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            data: None,
            error: Some(error.into()),
            loading: false,
        }
    }
}
