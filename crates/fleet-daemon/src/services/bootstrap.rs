//! Remote daemon build-and-install jobs.

use std::{sync::Arc, time::Duration};

use fleet_core::ids::{HostId, JobId};
use fleet_proto::job::JobKind;

use crate::{
    DaemonError, DaemonResult,
    jobs::{JobCtx, JobManager},
    machines::{ExecOutput, MachineProvider, Machines, RemoteEndpoint},
};
use fleet_proto::snapshot::LinkState;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
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
        "if [ -s {fleet_home_shell}/fleetd.pid ]; then \
         kill -TERM \"$(cat {fleet_home_shell}/fleetd.pid)\" 2>/dev/null || true; \
         i=0; while [ $i -lt 50 ] && kill -0 \"$(cat {fleet_home_shell}/fleetd.pid)\" 2>/dev/null; \
         do i=$((i + 1)); sleep 0.1; done; fi; \
         mkdir -p {fleet_home_shell}/logs; \
         nohup {fleetd_command} --home {fleet_home_shell} \
         >>{fleet_home_shell}/logs/fleetd.out 2>&1 </dev/null &"
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
    context.progress(format!("waiting for host {host} build {checkout_ref}"))?;
    let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
    loop {
        check_cancelled(context)?;
        if endpoint.state() == LinkState::Ready
            && endpoint
                .hello()
                .and_then(|hello| hello.build_commit)
                .as_deref()
                == Some(checkout_ref)
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(DaemonError::Remote(format!(
                "host `{host}` did not report build `{checkout_ref}` after restart"
            )));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    context.progress(format!("host {host} is ready ({checkout_ref})"))?;
    Ok(())
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
    use super::*;

    #[test]
    fn install_target_honors_custom_and_default_binary_paths() {
        assert_eq!(
            install_path("~/bin/custom-fleetd"),
            "\"$HOME\"/bin/custom-fleetd"
        );
        assert_eq!(install_path("fleetd"), "\"$HOME/.local/bin\"/fleetd");
    }
}
