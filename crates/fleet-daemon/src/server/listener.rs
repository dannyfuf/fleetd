//! Unix socket binding and connection acceptance.

use std::{
    fs::{File, OpenOptions},
    io::{Seek, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use fleet_core::paths::FleetHome;
use fleet_proto::{
    event::{Event, ToastLevel},
    job::JobStatus,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    server::{broadcast::BroadcastBus, connection::Connection},
    services::Services,
};

const MAX_CONNECTIONS: usize = 128;

/// Bound Fleet Unix listener and graceful-shutdown coordinator.
pub struct Listener {
    listener: UnixListener,
    socket_path: PathBuf,
    _singleton: SingletonGuard,
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn read(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

/// Lifetime ownership of the daemon PID file and socket pathname.
pub struct SingletonGuard {
    _pid_file: File,
    pid_path: PathBuf,
    pid_identity: FileIdentity,
    socket_path: PathBuf,
    socket_identity: Option<FileIdentity>,
}

impl SingletonGuard {
    /// Acquires daemon ownership before any service recovery or background work starts.
    pub async fn acquire(home: impl Into<PathBuf>) -> DaemonResult<Self> {
        let layout = FleetHome::new(home.into());
        tokio::fs::create_dir_all(layout.root())
            .await
            .map_err(|error| DaemonError::fs(layout.root(), error))?;
        let pid_path = layout.pid_path();
        let socket_path = layout.socket_path();
        let mut pid_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&pid_path)
            .map_err(|error| DaemonError::fs(&pid_path, error))?;
        // SAFETY: flock operates on this guard's valid, open descriptor and is released when the
        // descriptor is dropped. LOCK_NB prevents startup from waiting behind the active daemon.
        if unsafe { libc::flock(pid_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK)
                || error.raw_os_error() == Some(libc::EAGAIN)
            {
                return Err(already_running(&socket_path));
            }
            return Err(DaemonError::fs(&pid_path, error));
        }
        // The lock is authoritative for current daemons. The socket probe also protects a daemon
        // from an older release that created the PID file without holding this lock.
        if UnixStream::connect(&socket_path).await.is_ok() {
            return Err(already_running(&socket_path));
        }
        pid_file
            .set_len(0)
            .and_then(|()| pid_file.rewind())
            .and_then(|()| writeln!(pid_file, "{}", std::process::id()))
            .and_then(|()| pid_file.sync_all())
            .map_err(|error| DaemonError::fs(&pid_path, error))?;
        let pid_identity =
            FileIdentity::read(&pid_path).map_err(|error| DaemonError::fs(&pid_path, error))?;
        Ok(Self {
            _pid_file: pid_file,
            pid_path,
            pid_identity,
            socket_path,
            socket_identity: None,
        })
    }
}

impl Drop for SingletonGuard {
    fn drop(&mut self) {
        remove_if_owned(&self.socket_path, self.socket_identity);
        remove_if_owned(&self.pid_path, Some(self.pid_identity));
    }
}

impl Listener {
    /// Binds the core socket path, reclaiming it only when its recorded PID is not alive.
    pub async fn bind(
        home: impl Into<PathBuf>,
        services: Arc<Services>,
        events: BroadcastBus,
        shutdown: CancellationToken,
    ) -> DaemonResult<Self> {
        let singleton = SingletonGuard::acquire(home).await?;
        Self::bind_owned(singleton, services, events, shutdown).await
    }

    /// Binds the socket after startup has already acquired singleton ownership.
    pub async fn bind_owned(
        mut singleton: SingletonGuard,
        services: Arc<Services>,
        events: BroadcastBus,
        shutdown: CancellationToken,
    ) -> DaemonResult<Self> {
        let socket_path = singleton.socket_path.clone();
        let socket_exists = tokio::fs::try_exists(&socket_path)
            .await
            .map_err(|error| DaemonError::fs(&socket_path, error))?;
        if socket_exists && UnixStream::connect(&socket_path).await.is_ok() {
            return Err(already_running(&socket_path));
        }
        if socket_exists {
            tokio::fs::remove_file(&socket_path)
                .await
                .map_err(|error| DaemonError::fs(&socket_path, error))?;
        }
        let listener = UnixListener::bind(&socket_path)
            .map_err(|error| DaemonError::fs(&socket_path, error))?;
        singleton.socket_identity = Some(
            FileIdentity::read(&socket_path)
                .map_err(|error| DaemonError::fs(&socket_path, error))?,
        );
        Ok(Self {
            listener,
            socket_path,
            _singleton: singleton,
            services,
            events,
            shutdown,
        })
    }

    /// Accepts connections until shutdown and removes the owned socket and PID files.
    pub async fn run(self) -> DaemonResult<()> {
        let job_forwarder = spawn_job_forwarder(
            Arc::clone(&self.services),
            self.events.clone(),
            self.shutdown.clone(),
        );
        let session_forwarder = spawn_session_forwarder(
            Arc::clone(&self.services),
            self.events.clone(),
            self.shutdown.clone(),
        );
        let mut connections = JoinSet::new();
        let admission = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let result = loop {
            tokio::select! {
                () = self.shutdown.cancelled() => break Ok(()),
                accepted = self.listener.accept() => {
                    let (stream, _address) = match accepted {
                        Ok(accepted) => accepted,
                        Err(error) => break Err(DaemonError::fs(&self.socket_path, error)),
                    };
                    let Some(permit) = try_admit(&admission) else {
                        tracing::warn!(limit = MAX_CONNECTIONS, "rejecting client above connection limit");
                        drop(stream);
                        continue;
                    };
                    let connection = Connection::new(stream, Arc::clone(&self.services), self.events.clone(), self.shutdown.clone());
                    connections.spawn(async move {
                        let _permit = permit;
                        if let Err(error) = connection.run().await {
                            tracing::warn!(%error, "client connection ended with an error");
                        }
                    });
                }
                joined = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(error)) = joined {
                        tracing::warn!(%error, "client connection task panicked");
                    }
                }
            }
        };
        self.events.publish(Event::DaemonShuttingDown);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(joined) = connections.join_next().await {
                if let Err(error) = joined {
                    tracing::warn!(%error, "client connection task panicked during shutdown");
                }
            }
        })
        .await;
        connections.abort_all();
        job_forwarder.abort();
        session_forwarder.abort();
        let _ignored = job_forwarder.await;
        let _ignored = session_forwarder.await;
        result
    }

    /// Returns the bound socket path.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

fn try_admit(admission: &Arc<Semaphore>) -> Option<OwnedSemaphorePermit> {
    Arc::clone(admission).try_acquire_owned().ok()
}

/// Republishes job records as client events, with an error toast and a snapshot on completion.
fn spawn_job_forwarder(
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let mut updates = services.jobs.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                update = updates.recv() => match update {
                    Ok(job) => {
                        let failure = match &job.status {
                            JobStatus::Failed { error } => Some(error.clone()),
                            _ => None,
                        };
                        let finished = matches!(
                            job.status,
                            JobStatus::Succeeded | JobStatus::Failed { .. } | JobStatus::Cancelled
                        );
                        events.publish(Event::JobUpdated(job));
                        if let Some(message) = failure {
                            events.publish(Event::Toast {
                                level: ToastLevel::Error,
                                message,
                            });
                        }
                        if finished {
                            events.request_snapshot(Arc::clone(&services));
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        events.request_snapshot(Arc::clone(&services));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    })
}

/// Forwards service events onto the connection bus when the two are separate channels.
///
/// Session transitions — including a background terminal's first unseen output — are published
/// by the services themselves, which also request the coalesced snapshot that follows.
fn spawn_session_forwarder(
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    // Services constructed without `new_with_events` still need snapshot assembly attached.
    services.events.attach_services(Arc::downgrade(&services));
    let mut forwarded_events =
        (!events.same_channel(&services.events)).then(|| services.events.subscribe());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                event = async {
                    match &mut forwarded_events {
                        Some(receiver) => receiver.recv().await,
                        None => std::future::pending().await,
                    }
                }, if forwarded_events.is_some() => match event {
                    Ok(event) => { events.publish(event); }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        events.request_snapshot(Arc::clone(&services));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    })
}

fn already_running(socket_path: &Path) -> DaemonError {
    DaemonError::Conflict(format!(
        "daemon already running at {}",
        socket_path.display()
    ))
}

fn remove_if_owned(path: &Path, expected: Option<FileIdentity>) {
    if expected.is_some_and(|expected| FileIdentity::read(path).ok() == Some(expected)) {
        let _ignored = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use fleet_proto::{
        PROTOCOL_VERSION,
        codec::FleetCodec,
        event::Event,
        job::JobKind,
        request::{Request, RequestBody},
        response::{Response, ResponseBody},
    };
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::UnixStream;
    use tokio_util::codec::Framed;

    use crate::{
        adapters::{Adapters, clock::SystemClock, files::RealFiles},
        jobs::JobManager,
        stores::{config::ConfigStore, state::StateStore},
    };

    use super::*;

    #[test]
    fn connection_admission_is_bounded() {
        let admission = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let mut permits = (0..MAX_CONNECTIONS)
            .map(|_| try_admit(&admission).expect("connection admitted within limit"))
            .collect::<Vec<_>>();
        assert!(try_admit(&admission).is_none());
        permits.pop();
        assert!(try_admit(&admission).is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn job_event_lag_requests_authoritative_snapshot() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = temp.path().join("fleet");
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(&home, files.clone()));
        let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
        let jobs = Arc::new(JobManager::new(&home));
        let services = Arc::new(Services::new(
            &home,
            config,
            state,
            Arc::clone(&jobs),
            Adapters::system(files),
        ));
        let events = BroadcastBus::new(1_024);
        let mut receiver = events.subscribe();
        let shutdown = CancellationToken::new();
        let forwarder = spawn_job_forwarder(Arc::clone(&services), events, shutdown.clone());

        for index in 0..257 {
            jobs.submit(
                JobKind::Custom("lag-test".to_owned()),
                format!("target-{index}"),
                format!("job {index}"),
                false,
                false,
                |_| async { std::future::pending::<DaemonResult<()>>().await },
            );
        }

        let snapshot = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if matches!(receiver.recv().await, Ok(Event::SnapshotChanged(_))) {
                    break;
                }
            }
        })
        .await;
        shutdown.cancel();
        forwarder
            .await
            .unwrap_or_else(|error| panic!("job forwarder failed: {error}"));

        assert!(snapshot.is_ok(), "job event lag did not force a snapshot");
    }

    #[tokio::test]
    async fn hello_ping_and_snapshot_round_trip() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = temp.path().join("fleet");
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(&home, files.clone()));
        let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
        let jobs = Arc::new(JobManager::new(&home));
        let services = Arc::new(Services::new(
            &home,
            config,
            state,
            jobs,
            Adapters::system(files),
        ));
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
                    protocol: PROTOCOL_VERSION,
                    client: "test".into(),
                },
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            next_response(&mut client).await.result,
            Ok(ResponseBody::Hello {
                protocol: PROTOCOL_VERSION,
                ..
            })
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
