//! Typed watch registration, output, and catch-up helpers.

use crate::{
    Client,
    api::{Result, expect_ack, unexpected},
};
use fleet_core::{
    ids::{SessionId, TerminalId},
    watches::{Watch, WatchId, WatchStream},
};
use fleet_proto::{request::RequestBody, response::ResponseBody, watch::WatchTail};
use std::path::PathBuf;

impl Client {
    /// Registers a read-only child watch on this connection.
    pub async fn start_watch(
        &self,
        terminal: TerminalId,
        label: String,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        pid: Option<u32>,
    ) -> Result<WatchId> {
        match self
            .request(RequestBody::StartWatch {
                terminal,
                label,
                command,
                cwd,
                pid,
            })
            .await?
        {
            ResponseBody::WatchStarted(id) => Ok(id),
            other => Err(unexpected("start_watch", other)),
        }
    }

    /// Appends a display copy; original child bytes should be forwarded independently.
    pub async fn append_watch_output(
        &self,
        watch: WatchId,
        stream: WatchStream,
        text: String,
    ) -> Result<()> {
        expect_ack(
            "append_watch_output",
            self.request(RequestBody::AppendWatchOutput {
                watch,
                stream,
                text,
            })
            .await?,
        )
    }

    /// Reports normal or signalled completion.
    pub async fn finish_watch(
        &self,
        watch: WatchId,
        code: Option<i32>,
        signal: Option<i32>,
    ) -> Result<()> {
        expect_ack(
            "finish_watch",
            self.request(RequestBody::FinishWatch {
                watch,
                code,
                signal,
            })
            .await?,
        )
    }

    /// Lists a session's watches; subscribe before listing to avoid missing new watches.
    pub async fn list_watches(&self, session: SessionId) -> Result<Vec<Watch>> {
        match self.request(RequestBody::ListWatches { session }).await? {
            ResponseBody::Watches(watches) => Ok(watches),
            other => Err(unexpected("list_watches", other)),
        }
    }

    /// Returns retained chunks at or after an inclusive cursor and current metadata.
    pub async fn tail_watch(&self, watch: WatchId, from_seq: Option<u64>) -> Result<WatchTail> {
        match self
            .request(RequestBody::TailWatch { watch, from_seq })
            .await?
        {
            ResponseBody::WatchTail(tail) => Ok(tail),
            other => Err(unexpected("tail_watch", other)),
        }
    }

    /// Removes a completed watch; running watches return Conflict. Never kills a child.
    pub async fn dismiss_watch(&self, watch: WatchId) -> Result<()> {
        expect_ack(
            "dismiss_watch",
            self.request(RequestBody::DismissWatch { watch }).await?,
        )
    }
}
