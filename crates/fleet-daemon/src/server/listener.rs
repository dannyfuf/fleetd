//! Unix socket binding and connection acceptance.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use fleet_core::paths::FleetHome;
use fleet_proto::event::Event;
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    server::{broadcast::BroadcastBus, connection::Connection},
    services::Services,
};

/// Bound Fleet Unix listener and graceful-shutdown coordinator.
pub struct Listener {
    listener: UnixListener,
    socket_path: PathBuf,
    pid_path: PathBuf,
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
}

impl Listener {
    /// Binds the core socket path, reclaiming it only when its recorded PID is not alive.
    pub async fn bind(
        home: impl Into<PathBuf>,
        services: Arc<Services>,
        events: BroadcastBus,
        shutdown: CancellationToken,
    ) -> DaemonResult<Self> {
        let layout = FleetHome::new(home.into());
        let socket_path = layout.socket_path();
        let pid_path = layout.pid_path();
        tokio::fs::create_dir_all(layout.root())
            .await
            .map_err(|error| DaemonError::fs(layout.root(), error))?;
        if tokio::fs::try_exists(&socket_path)
            .await
            .map_err(|error| DaemonError::fs(&socket_path, error))?
        {
            let live = read_pid(&pid_path).await.is_some_and(pid_is_alive);
            if live {
                return Err(DaemonError::Conflict(format!(
                    "daemon already running at {}",
                    socket_path.display()
                )));
            }
            tokio::fs::remove_file(&socket_path)
                .await
                .map_err(|error| DaemonError::fs(&socket_path, error))?;
            let _ignored = tokio::fs::remove_file(&pid_path).await;
        }
        let listener = UnixListener::bind(&socket_path)
            .map_err(|error| DaemonError::fs(&socket_path, error))?;
        tokio::fs::write(&pid_path, format!("{}\n", std::process::id()))
            .await
            .map_err(|error| DaemonError::fs(&pid_path, error))?;
        Ok(Self {
            listener,
            socket_path,
            pid_path,
            services,
            events,
            shutdown,
        })
    }

    /// Accepts connections until shutdown and removes the owned socket and PID files.
    pub async fn run(self) -> DaemonResult<()> {
        let mut job_updates = self.services.jobs.subscribe();
        let job_events = self.events.clone();
        let job_shutdown = self.shutdown.clone();
        let forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = job_shutdown.cancelled() => break,
                    update = job_updates.recv() => match update {
                        Ok(job) => { job_events.publish(Event::JobUpdated(job)); }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => break,
                accepted = self.listener.accept() => {
                    let (stream, _address) = accepted.map_err(|error| DaemonError::fs(&self.socket_path, error))?;
                    let connection = Connection::new(stream, Arc::clone(&self.services), self.events.clone(), self.shutdown.clone());
                    tokio::spawn(async move {
                        if let Err(error) = connection.run().await {
                            tracing::warn!(%error, "client connection ended with an error");
                        }
                    });
                }
            }
        }
        self.events.publish(Event::DaemonShuttingDown);
        forwarder.abort();
        remove_owned(&self.pid_path, &self.socket_path).await;
        Ok(())
    }

    /// Returns the bound socket path.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

async fn read_pid(path: &Path) -> Option<u32> {
    tokio::fs::read_to_string(path)
        .await
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: signal zero checks existence without delivering a signal.
    let result = unsafe { libc::kill(pid.cast_signed(), 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

async fn remove_owned(pid_path: &Path, socket_path: &Path) {
    if read_pid(pid_path).await == Some(std::process::id()) {
        let _ignored = tokio::fs::remove_file(pid_path).await;
        let _ignored = tokio::fs::remove_file(socket_path).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fleet_proto::{
        codec::FleetCodec,
        request::{Request, RequestBody},
        response::{Response, ResponseBody},
    };
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::UnixStream;
    use tokio_util::codec::Framed;

    use crate::{
        adapters::{clock::SystemClock, files::RealFiles},
        jobs::JobManager,
        stores::{config::ConfigStore, state::StateStore},
    };

    use super::*;

    #[tokio::test]
    async fn hello_ping_and_snapshot_round_trip() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = temp.path().join("fleet");
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(&home, files.clone()));
        let state = Arc::new(StateStore::new(&home, files, Arc::new(SystemClock)));
        let jobs = Arc::new(JobManager::new(&home));
        let services = Arc::new(Services::new(&home, config, state, jobs));
        let shutdown = CancellationToken::new();
        let listener = Listener::bind(&home, services, BroadcastBus::default(), shutdown.clone())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let socket = listener.socket_path().to_path_buf();
        let task = tokio::spawn(listener.run());
        let stream = UnixStream::connect(&socket)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let mut client = Framed::new(stream, FleetCodec::<Request, serde_json::Value>::new());

        client
            .send(Request {
                id: 1,
                body: RequestBody::Hello {
                    protocol: 1,
                    client: "test".to_owned(),
                },
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            next_response(&mut client).await.result,
            Ok(ResponseBody::Hello { protocol: 1, .. })
        ));
        client
            .send(Request {
                id: 2,
                body: RequestBody::DaemonPing,
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            next_response(&mut client).await.result,
            Ok(ResponseBody::Pong)
        );
        client
            .send(Request {
                id: 3,
                body: RequestBody::GetSnapshot,
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            next_response(&mut client).await.result,
            Ok(ResponseBody::Snapshot(_))
        ));

        client
            .send(Request {
                id: 4,
                body: RequestBody::DaemonShutdown {
                    stop_sessions: false,
                },
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            next_response(&mut client).await.result,
            Ok(ResponseBody::ShuttingDown)
        );
        task.await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));
    }

    async fn next_response(
        client: &mut Framed<UnixStream, FleetCodec<Request, serde_json::Value>>,
    ) -> Response {
        let value = client
            .next()
            .await
            .unwrap_or_else(|| panic!("connection closed"))
            .unwrap_or_else(|error| panic!("{error}"));
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }
}
