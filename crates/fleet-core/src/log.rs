//! Structured append-only application log records.

use serde::{Deserialize, Serialize};

/// Severity of one structured Fleet log record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Informational operational event.
    Info,
    /// Recoverable or degraded behavior.
    Warn,
    /// Failed operation requiring attention.
    Error,
}

/// One JSONL record appended to Fleet's structured application log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    /// ISO-8601 event time.
    pub ts: String,
    /// Event severity.
    pub level: LogLevel,
    /// Stable subsystem or operation scope.
    pub scope: String,
    /// Human-readable message.
    pub msg: String,
    /// Optional structured context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_record_round_trips() {
        let record = LogRecord {
            ts: "2026-09-04T12:00:00Z".to_owned(),
            level: LogLevel::Warn,
            scope: "pool".to_owned(),
            msg: "hook failed".to_owned(),
            data: Some(serde_json::json!({"step": 2})),
        };
        let encoded = serde_json::to_string(&record).unwrap_or_else(|error| panic!("{error}"));
        let decoded: LogRecord =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, record);
    }
}
