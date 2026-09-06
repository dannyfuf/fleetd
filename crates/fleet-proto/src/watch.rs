//! Watch catch-up response.
use fleet_core::watches::{Watch, WatchChunk};
use serde::{Deserialize, Serialize};

/// An atomic metadata/output snapshot for reconnect and gap recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchTail {
    /// Current watch metadata.
    pub watch: Watch,
    /// Retained output at or after the requested inclusive cursor.
    pub chunks: Vec<WatchChunk>,
    /// Oldest cursor still available, including when the request is newer.
    pub first_retained_seq: u64,
    /// Next sequence to request.
    pub next_seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{Event, EventKind},
        request::RequestBody,
        response::ResponseBody,
    };
    use fleet_core::{
        ids::TerminalId,
        watches::{WatchId, WatchSource, WatchStatus, WatchStream},
    };

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: T,
    ) {
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<T>(&json).unwrap(), value);
    }
    #[test]
    fn all_watch_requests_responses_and_events_round_trip() {
        let id = WatchId(1);
        let watch = Watch {
            id,
            session: "repo/main".parse().unwrap(),
            terminal: TerminalId(2),
            label: "codex".into(),
            command: vec!["codex".into(), "exec".into()],
            cwd: Some("/tmp".into()),
            pid: Some(123),
            started_at: "2026-09-05T00:00:00Z".into(),
            status: WatchStatus::Running,
            source: WatchSource::Discovered,
            log_file: Some("/tmp/watch.log".into()),
        };
        let chunks = vec![WatchChunk {
            seq: 0,
            stream: WatchStream::Stderr,
            text: "hello\n".into(),
        }];
        for request in [
            RequestBody::StartWatch {
                terminal: watch.terminal,
                label: watch.label.clone(),
                command: watch.command.clone(),
                cwd: watch.cwd.clone(),
                pid: watch.pid,
            },
            RequestBody::AppendWatchOutput {
                watch: id,
                stream: WatchStream::Stdout,
                text: "out".into(),
            },
            RequestBody::FinishWatch {
                watch: id,
                code: Some(3),
                signal: None,
            },
            RequestBody::ListWatches {
                session: watch.session.clone(),
            },
            RequestBody::TailWatch {
                watch: id,
                from_seq: Some(0),
            },
            RequestBody::DismissWatch { watch: id },
        ] {
            round_trip(request);
        }
        round_trip(ResponseBody::WatchStarted(id));
        round_trip(ResponseBody::Watches(vec![watch.clone()]));
        round_trip(ResponseBody::WatchTail(WatchTail {
            watch: watch.clone(),
            chunks: chunks.clone(),
            first_retained_seq: 0,
            next_seq: 1,
        }));
        round_trip(Event::WatchStarted(watch.clone()));
        round_trip(Event::WatchOutput { watch: id, chunks });
        round_trip(Event::WatchExited(Watch {
            status: WatchStatus::Exited {
                code: None,
                signal: Some(9),
            },
            ..watch
        }));
        round_trip(Event::WatchDismissed(id));
        for kind in [
            EventKind::WatchStarted,
            EventKind::WatchOutput,
            EventKind::WatchExited,
            EventKind::WatchDismissed,
        ] {
            round_trip(kind);
        }
    }

    #[test]
    fn watch_fields_are_camel_case_and_missing_new_fields_default_to_cooperative() {
        let value = serde_json::json!({
            "id": 1,
            "session": "repo/main",
            "terminal": 2,
            "label": "old",
            "command": ["codex"],
            "cwd": null,
            "pid": 123,
            "startedAt": "2026-09-05T00:00:00Z",
            "status": "running"
        });
        let watch: Watch = serde_json::from_value(value).unwrap();
        assert_eq!(watch.source, WatchSource::Cooperative);
        assert_eq!(watch.log_file, None);

        let serialized = serde_json::to_value(Watch {
            source: WatchSource::Discovered,
            log_file: Some("/tmp/job.log".into()),
            ..watch
        })
        .unwrap();
        assert_eq!(serialized["source"], "discovered");
        assert_eq!(serialized["logFile"], "/tmp/job.log");
    }
}
