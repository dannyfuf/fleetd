//! Remote daemon build-and-install jobs.

use std::{sync::Arc, time::Duration};

use fleet_core::ids::{HostId, JobId};
use fleet_proto::{job::JobKind, request::RequestBody};

use crate::{
    DaemonError, DaemonResult,
    jobs::{JobCtx, JobManager},
    machines::{ExecOutput, MachineProvider, Machines, RemoteEndpoint},
};
use fleet_proto::snapshot::LinkState;
use tokio_util::sync::CancellationToken;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
// Covers one worst-case reconnect envelope after the restart: the link Hello timeout (10 s), the
// remote bridge start timeout (10 s), and one retry, with slack.
const PROBE_TIMEOUT: Duration = Duration::from_secs(45);
const PROBE_INTERVAL: Duration = Duration::from_millis(200);
const LINK_REFRESH_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_REMOTE_HOME: &str = "~/.fleet";

type ProviderLookup = dyn Fn(&HostId) -> Option<Arc<dyn MachineProvider>> + Send + Sync;
type EndpointLookup = dyn Fn(&HostId) -> Option<Arc<dyn RemoteEndpoint>> + Send + Sync;

/// Starts remote build-and-install jobs and records every remote step in the ordinary job log.
#[derive(Clone, Default)]
pub struct Bootstrap {
    jobs: Option<Arc<JobManager>>,
    provider: Option<Arc<ProviderLookup>>,
    endpoint: Option<Arc<EndpointLookup>>,
    origin_url: Option<String>,
    build_commit: Option<String>,
}

impl Bootstrap {
    /// Creates an unconfigured service for compatibility with the initial composition skeleton.
    ///
    /// Production composition must call [`Self::with_registry`] so jobs have a manager and a
    /// machine registry. Keeping this constructor non-panicking lets mixed-stage builds compile.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a production bootstrap service backed by the configured machine registry.
    #[must_use]
    pub fn with_registry(
        jobs: Arc<JobManager>,
        machines: Arc<Machines>,
        origin_url: impl Into<String>,
        build_commit: Option<String>,
    ) -> Self {
        Self {
            jobs: Some(jobs),
            provider: Some(Arc::new({
                let machines = Arc::clone(&machines);
                move |host| machines.get(host)
            })),
            endpoint: Some(Arc::new(move |host| machines.endpoint(host))),
            origin_url: Some(origin_url.into()),
            build_commit,
        }
    }

    /// Creates a bootstrap service around one provider, primarily for deterministic tests.
    #[must_use]
    pub fn with_machine(
        jobs: Arc<JobManager>,
        machine: Arc<dyn MachineProvider>,
        origin_url: impl Into<String>,
        build_commit: Option<String>,
    ) -> Self {
        Self {
            jobs: Some(jobs),
            provider: Some(Arc::new(move |host| {
                (machine.id() == host).then(|| Arc::clone(&machine))
            })),
            endpoint: None,
            origin_url: Some(origin_url.into()),
            build_commit,
        }
    }

    /// Creates a deterministic bootstrap service with an observable daemon endpoint.
    #[must_use]
    pub fn with_machine_and_endpoint(
        jobs: Arc<JobManager>,
        machine: Arc<dyn MachineProvider>,
        endpoint: Arc<dyn RemoteEndpoint>,
        origin_url: impl Into<String>,
        build_commit: Option<String>,
    ) -> Self {
        let host = machine.id().clone();
        Self {
            jobs: Some(jobs),
            provider: Some(Arc::new(move |requested| {
                (requested == &host).then(|| Arc::clone(&machine))
            })),
            endpoint: Some(Arc::new(move |requested| {
                (endpoint.host() == requested).then(|| Arc::clone(&endpoint))
            })),
            origin_url: Some(origin_url.into()),
            build_commit,
        }
    }

    /// Submits a cancellable job that builds, installs, restarts, and probes a remote daemon.
    pub async fn start(&self, host: HostId, git_ref: Option<String>) -> DaemonResult<JobId> {
        let jobs = self.jobs.as_ref().cloned().ok_or_else(not_configured)?;
        let lookup = self.provider.as_ref().cloned().ok_or_else(not_configured)?;
        let machine = lookup(&host)
            .ok_or_else(|| DaemonError::NotFound(format!("configured host `{host}`")))?;
        let origin_url = self
            .origin_url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| {
                DaemonError::Validation(
                    "Fleet source origin is unavailable; configure a bootstrap repository URL"
                        .to_owned(),
                )
            })?;
        let checkout_ref = git_ref
            .or_else(|| self.build_commit.clone())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                DaemonError::Validation(
                    "bootstrap requires --ref because this fleetd has no build commit".to_owned(),
                )
            })?;
        let endpoint = self.endpoint.as_ref().and_then(|lookup| lookup(&host));
        let target = host.to_string();
        let title = format!("Bootstrap {host}");

        Ok(jobs.submit(
            JobKind::Custom("host.bootstrap".to_owned()),
            target,
            title,
            true,
            true,
            move |context| {
                let machine = Arc::clone(&machine);
                let origin_url = origin_url.clone();
                let checkout_ref = checkout_ref.clone();
                async move {
                    run_bootstrap(&context, machine, endpoint, &origin_url, &checkout_ref).await
                }
            },
        ))
    }
}

async fn run_bootstrap(
    context: &JobCtx,
    machine: Arc<dyn MachineProvider>,
    endpoint: Option<Arc<dyn RemoteEndpoint>>,
    origin_url: &str,
    checkout_ref: &str,
) -> DaemonResult<()> {
    let host = machine.id().clone();
    let remote_home = machine.fleet_home().unwrap_or(DEFAULT_REMOTE_HOME);
    let checkout = format!("{remote_home}/src/fleet");
    let checkout_shell = shell_path(&checkout);
    let parent_shell = shell_path(&format!("{remote_home}/src"));
    let fleet_home_shell = shell_path(remote_home);
    let install_target = install_path(machine.fleetd_binary());
    let fleetd_command = shell_words::quote(machine.fleetd_binary());

    checked_exec(
        context,
        &machine,
        vec!["git".to_owned(), "--version".to_owned()],
        COMMAND_TIMEOUT,
        Some("git"),
    )
    .await?;
    checked_exec(
        context,
        &machine,
        vec!["cargo".to_owned(), "--version".to_owned()],
        COMMAND_TIMEOUT,
        Some("cargo"),
    )
    .await?;

    let origin = shell_words::quote(origin_url);
    let clone_or_fetch = format!(
        "if [ -d {checkout_shell}/.git ]; then git -C {checkout_shell} fetch --prune origin; \
         else mkdir -p {parent_shell} && git clone -- {origin} {checkout_shell}; fi"
    );
    checked_exec(
        context,
        &machine,
        shell_command(clone_or_fetch),
        COMMAND_TIMEOUT,
        None,
    )
    .await?;

    let reference = shell_words::quote(checkout_ref);
    checked_exec(
        context,
        &machine,
        shell_command(format!(
            "git -C {checkout_shell} checkout --detach {reference}"
        )),
        COMMAND_TIMEOUT,
        None,
    )
    .await?;
    checked_exec(
        context,
        &machine,
        shell_command(format!(
            "cd {checkout_shell} && FLEET_BUILD_COMMIT={reference} cargo build --release -p fleet-daemon"
        )),
        BUILD_TIMEOUT,
        None,
    )
    .await?;
    checked_exec(
        context,
        &machine,
        shell_command(format!(
            "mkdir -p \"$(dirname -- {install_target})\" && install -m 755 \
             {checkout_shell}/target/release/fleetd {install_target}"
        )),
        COMMAND_TIMEOUT,
        None,
    )
    .await?;

    let restart = format!(
        "fleetd_pid=''; kill_pid=''; \
         if [ -s {fleet_home_shell}/fleetd.pid ]; then \
         fleetd_pid=\"$(cat {fleet_home_shell}/fleetd.pid)\"; kill_pid=\"$fleetd_pid\"; \
         case \"$(ps -p \"$kill_pid\" -o comm= 2>/dev/null)\" in \
         *fleetd*) ;; *) kill_pid='' ;; esac; fi; \
         if [ -n \"$kill_pid\" ]; then \
         kill -TERM \"$kill_pid\" 2>/dev/null || true; \
         i=0; while [ $i -lt 50 ] && kill -0 \"$kill_pid\" 2>/dev/null; \
         do i=$((i + 1)); sleep 0.1; done; \
         if kill -0 \"$kill_pid\" 2>/dev/null; then \
         kill -KILL \"$kill_pid\" 2>/dev/null || true; \
         i=0; while [ $i -lt 20 ] && kill -0 \"$kill_pid\" 2>/dev/null; \
         do i=$((i + 1)); sleep 0.1; done; fi; fi; \
         mkdir -p {fleet_home_shell}/logs; \
         nohup {fleetd_command} --home {fleet_home_shell} \
         >>{fleet_home_shell}/logs/fleetd.out 2>&1 </dev/null & \
         i=0; while [ $i -lt 100 ]; do \
         if [ -s {fleet_home_shell}/fleetd.pid ]; then \
         started_pid=\"$(cat {fleet_home_shell}/fleetd.pid)\"; \
         if [ -n \"$started_pid\" ] && [ \"$started_pid\" != \"$fleetd_pid\" ] \
         && kill -0 \"$started_pid\" 2>/dev/null; then exit 0; fi; fi; \
         i=$((i + 1)); sleep 0.1; done; \
         echo 'fleetd failed to start' >&2; \
         tail -n 40 {fleet_home_shell}/logs/fleetd.out >&2 || true; \
         exit 1"
    );
    checked_exec(
        context,
        &machine,
        shell_command(restart),
        COMMAND_TIMEOUT,
        None,
    )
    .await?;

    let endpoint = endpoint.ok_or_else(|| {
        DaemonError::Validation(format!(
            "host `{host}` bootstrap cannot verify the restarted daemon link"
        ))
    })?;
    // The restart drops the link. Wake it out of any reconnect backoff so the probe observes the
    // new daemon instead of waiting out a grown backoff; a Ready link is unaffected.
    endpoint.nudge_reconnect();
    context.progress(format!("waiting for host {host} build {checkout_ref}"))?;
    probe_for_build(
        &endpoint,
        &host,
        checkout_ref,
        &context.cancel,
        PROBE_TIMEOUT,
    )
    .await?;
    context.progress(format!("host {host} is ready ({checkout_ref})"))?;
    Ok(())
}

/// Waits until the endpoint reports `checkout_ref`, refreshing a stale Hello on a Ready link.
///
/// Each iteration reads the Hello first, then checks the deadline, so a refresh that lands while
/// the last ping is still in flight is honoured instead of discarded.
async fn probe_for_build(
    endpoint: &Arc<dyn RemoteEndpoint>,
    host: &HostId,
    checkout_ref: &str,
    cancel: &CancellationToken,
    budget: Duration,
) -> DaemonResult<()> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if cancel.is_cancelled() {
            return Err(DaemonError::Cancelled);
        }
        if endpoint
            .hello()
            .and_then(|hello| hello.build_commit)
            .as_deref()
            == Some(checkout_ref)
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(DaemonError::Remote(format!(
                "host `{host}` did not report build `{checkout_ref}` after restart"
            )));
        }
        if endpoint.state() == LinkState::Ready {
            // A bridge from the replaced binary can keep its stdio pipes open after the old
            // daemon exits. A ping makes that stale bridge observe the closed socket so the
            // persistent endpoint reconnects and refreshes its Hello metadata.
            let _ = tokio::time::timeout(
                LINK_REFRESH_TIMEOUT,
                endpoint.request(RequestBody::DaemonPing),
            )
            .await;
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

async fn checked_exec(
    context: &JobCtx,
    machine: &Arc<dyn MachineProvider>,
    argv: Vec<String>,
    timeout: Duration,
    required_tool: Option<&str>,
) -> DaemonResult<ExecOutput> {
    check_cancelled(context)?;
    context.progress(format!("$ {}", shell_words::join(&argv)))?;
    let output = machine.exec(&argv, timeout).await.map_err(|error| {
        if let Some(tool) = required_tool {
            missing_tool(machine.id(), tool, &error.to_string())
        } else {
            error.into()
        }
    })?;
    log_output(context, &output)?;
    if output.status != 0 {
        let detail = command_detail(&output);
        return Err(if let Some(tool) = required_tool {
            missing_tool(machine.id(), tool, detail)
        } else {
            DaemonError::Remote(format!(
                "host `{}` command exited {}: {detail}",
                machine.id(),
                output.status
            ))
        });
    }
    Ok(output)
}

fn log_output(context: &JobCtx, output: &ExecOutput) -> DaemonResult<()> {
    for line in output.stdout.lines().chain(output.stderr.lines()) {
        context.progress(line.to_owned())?;
    }
    Ok(())
}

fn command_detail(output: &ExecOutput) -> &str {
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        output.stdout.trim()
    } else {
        stderr
    }
}

fn missing_tool(host: &HostId, tool: &str, detail: &str) -> DaemonError {
    let suffix = if detail.trim().is_empty() {
        String::new()
    } else {
        format!(": {}", detail.trim())
    };
    DaemonError::Validation(format!(
        "remote host `{host}` is missing required tool `{tool}`{suffix}"
    ))
}

fn check_cancelled(context: &JobCtx) -> DaemonResult<()> {
    if context.cancel.is_cancelled() {
        Err(DaemonError::Cancelled)
    } else {
        Ok(())
    }
}

fn shell_command(script: String) -> Vec<String> {
    vec!["sh".to_owned(), "-lc".to_owned(), script]
}

fn shell_path(path: &str) -> String {
    if path == "~" {
        return "\"$HOME\"".to_owned();
    }
    if let Some(relative) = path.strip_prefix("~/") {
        return format!("\"$HOME\"/{}", shell_words::quote(relative));
    }
    shell_words::quote(path).into_owned()
}

fn install_path(binary: &str) -> String {
    if binary.contains('/') {
        shell_path(binary)
    } else {
        format!("\"$HOME/.local/bin\"/{}", shell_words::quote(binary))
    }
}

fn not_configured() -> DaemonError {
    DaemonError::Unsupported("Bootstrap service is not configured".to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use fleet_proto::{event::Event, response::ResponseBody};
    use tokio::sync::{broadcast, watch};

    use super::*;
    use crate::machines::RemoteHello;

    /// Always-Ready endpoint whose ping never returns and whose Hello flips mid-ping.
    struct StallingRemote {
        host: HostId,
        hello: Mutex<Option<RemoteHello>>,
        events_tx: broadcast::Sender<Event>,
        state_tx: watch::Sender<LinkState>,
        pings: AtomicUsize,
        refreshed_build: String,
    }

    impl StallingRemote {
        fn new(refreshed_build: &str) -> Self {
            let (events_tx, _) = broadcast::channel(8);
            let (state_tx, _) = watch::channel(LinkState::Ready);
            Self {
                host: HostId::try_from("dev-box").unwrap_or_else(|error| panic!("{error}")),
                hello: Mutex::new(None),
                events_tx,
                state_tx,
                pings: AtomicUsize::new(0),
                refreshed_build: refreshed_build.to_owned(),
            }
        }

        fn pings(&self) -> usize {
            self.pings.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl RemoteEndpoint for StallingRemote {
        fn host(&self) -> &HostId {
            &self.host
        }
        fn state(&self) -> LinkState {
            *self.state_tx.borrow()
        }
        fn hello(&self) -> Option<RemoteHello> {
            self.hello
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
        async fn request(&self, _body: RequestBody) -> DaemonResult<ResponseBody> {
            self.pings.fetch_add(1, Ordering::Relaxed);
            // The refresh lands while this ping is still in flight.
            *self
                .hello
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(RemoteHello {
                version: "fleetd test".to_owned(),
                daemon_id: "remote-daemon".to_owned(),
                build_commit: Some(self.refreshed_build.clone()),
                capabilities: Vec::new(),
            });
            std::future::pending::<()>().await;
            unreachable!("stalling endpoint never answers")
        }
        fn events(&self) -> broadcast::Receiver<Event> {
            self.events_tx.subscribe()
        }
        fn state_changes(&self) -> watch::Receiver<LinkState> {
            self.state_tx.subscribe()
        }
        async fn close(&self) {}
    }

    #[test]
    fn install_target_honors_custom_and_default_binary_paths() {
        assert_eq!(
            install_path("~/bin/custom-fleetd"),
            "\"$HOME\"/bin/custom-fleetd"
        );
        assert_eq!(install_path("fleetd"), "\"$HOME/.local/bin\"/fleetd");
    }

    #[tokio::test]
    async fn probe_honours_a_hello_refresh_that_lands_during_the_last_ping() {
        let endpoint = Arc::new(StallingRemote::new("abc123"));
        let host = endpoint.host().clone();
        // The budget expires while the first ping is still stalled, so the refresh it triggered
        // is only observable if the loop re-reads the Hello before giving up.
        let outcome = probe_for_build(
            &(Arc::clone(&endpoint) as Arc<dyn RemoteEndpoint>),
            &host,
            "abc123",
            &CancellationToken::new(),
            Duration::from_millis(700),
        )
        .await;

        assert!(outcome.is_ok(), "probe failed: {outcome:?}");
        assert_eq!(endpoint.pings(), 1);
    }

    #[tokio::test]
    async fn probe_never_pings_a_link_that_is_not_ready() {
        let endpoint = Arc::new(crate::testing::FakeRemote::new(
            HostId::try_from("dev-box").unwrap_or_else(|error| panic!("{error}")),
        ));
        endpoint.set_state(LinkState::Down);
        let host = endpoint.host().clone();

        let outcome = probe_for_build(
            &(Arc::clone(&endpoint) as Arc<dyn RemoteEndpoint>),
            &host,
            "abc123",
            &CancellationToken::new(),
            Duration::from_millis(100),
        )
        .await;

        assert!(outcome.is_err(), "expected a probe timeout");
        assert!(endpoint.requests().is_empty());
    }
}
