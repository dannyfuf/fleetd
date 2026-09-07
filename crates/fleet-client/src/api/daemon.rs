use super::{DaemonVersion, HelloResult, Result, expect_ack, unexpected};
use crate::Client;
use fleet_core::config::Config;
use fleet_proto::{
    PROTOCOL_VERSION,
    event::EventKind,
    job::JobRecord,
    request::RequestBody,
    response::{DoctorCheck, KeepAliveRuleMatch, ResponseBody},
    snapshot::Snapshot,
};

impl Client {
    /// Renegotiates the protocol and returns daemon identity information.
    pub async fn hello(&self, client: impl Into<String>) -> Result<HelloResult> {
        match self
            .request(RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: client.into(),
            })
            .await?
        {
            ResponseBody::Hello { protocol, server } => Ok(HelloResult { protocol, server }),
            response => Err(unexpected("hello", response)),
        }
    }

    /// Fetches a complete daemon snapshot.
    pub async fn get_snapshot(&self) -> Result<Snapshot> {
        match self.request(RequestBody::GetSnapshot).await? {
            ResponseBody::Snapshot(snapshot) => Ok(snapshot),
            response => Err(unexpected("get_snapshot", response)),
        }
    }

    /// Replaces this connection's event subscriptions.
    pub async fn subscribe(&self, events: Vec<EventKind>) -> Result<()> {
        expect_ack(
            "subscribe",
            self.request(RequestBody::Subscribe { events }).await?,
        )
    }

    /// Fetches the effective configuration.
    pub async fn get_config(&self) -> Result<Config> {
        match self.request(RequestBody::GetConfig).await? {
            ResponseBody::Config(config) => Ok(config),
            response => Err(unexpected("get_config", response)),
        }
    }

    /// Counts current process matches for configured keep-alive rules.
    pub async fn match_keep_alive_rules(&self) -> Result<Vec<KeepAliveRuleMatch>> {
        match self.request(RequestBody::MatchKeepAliveRules).await? {
            ResponseBody::KeepAliveRuleMatches(matches) => Ok(matches),
            response => Err(unexpected("match_keep_alive_rules", response)),
        }
    }

    /// Starts a non-destructive import from the default swarm home.
    pub async fn import_from_swarm(&self) -> Result<JobRecord> {
        match self.request(RequestBody::ImportFromSwarm).await? {
            ResponseBody::Job(job) => Ok(job),
            response => Err(unexpected("import_from_swarm", response)),
        }
    }

    /// Runs daemon environment diagnostics.
    pub async fn doctor(&self) -> Result<Vec<DoctorCheck>> {
        match self.request(RequestBody::Doctor).await? {
            ResponseBody::Doctor(checks) => Ok(checks),
            response => Err(unexpected("doctor", response)),
        }
    }

    /// Starts a Fleet self-update job.
    pub async fn update(&self) -> Result<JobRecord> {
        match self.request(RequestBody::Update).await? {
            ResponseBody::Job(job) => Ok(job),
            response => Err(unexpected("update", response)),
        }
    }

    /// Checks daemon liveness.
    pub async fn daemon_ping(&self) -> Result<()> {
        match self.request(RequestBody::DaemonPing).await? {
            ResponseBody::Pong => Ok(()),
            response => Err(unexpected("daemon_ping", response)),
        }
    }

    /// Fetches daemon build and protocol versions.
    pub async fn daemon_version(&self) -> Result<DaemonVersion> {
        match self.request(RequestBody::DaemonVersion).await? {
            ResponseBody::Version { version, protocol } => Ok(DaemonVersion { version, protocol }),
            response => Err(unexpected("daemon_version", response)),
        }
    }

    /// Asks the daemon to shut down explicitly.
    pub async fn daemon_shutdown(&self, stop_sessions: bool) -> Result<()> {
        match self
            .request(RequestBody::DaemonShutdown { stop_sessions })
            .await?
        {
            ResponseBody::ShuttingDown => Ok(()),
            response => Err(unexpected("daemon_shutdown", response)),
        }
    }
}
