//! Repository hook execution shared by the prepared-copy pool and worktree creation.

use std::path::Path;

use fleet_core::model::RepoHooks;

use crate::{
    DaemonError, DaemonResult,
    adapters::shell::{Shell, ShellCommand, ShellResult},
    jobs::JobCtx,
};

use super::check_cancelled;

/// Runs every `prepare` hook in order, reporting failures as job progress rather than
/// aborting: a prepared copy stays usable when a project hook is broken. Only cancellation
/// stops the loop.
pub(super) async fn run_prepare(
    shell: &dyn Shell,
    cwd: &Path,
    hooks: &RepoHooks,
    context: &JobCtx,
) -> DaemonResult<()> {
    for command in &hooks.prepare {
        check_cancelled(context)?;
        context.progress(format!("prepare: {command}"))?;
        match run(shell, cwd, command, Some(context)).await {
            Ok(output) if output.success() => record_output(context, &output)?,
            Ok(output) => {
                record_output(context, &output)?;
                context.progress(format!(
                    "warning: prepare hook exited {}: {command}",
                    output.status
                ))?;
            }
            Err(DaemonError::Cancelled) => return Err(DaemonError::Cancelled),
            Err(error) => context.progress(format!("warning: prepare hook failed: {error}"))?,
        }
    }
    Ok(())
}

/// Runs one hook through `sh -c`, refusing to start and refusing to report success once
/// the owning job is cancelled.
pub(super) async fn run(
    shell: &dyn Shell,
    cwd: &Path,
    command: &str,
    context: Option<&JobCtx>,
) -> DaemonResult<ShellResult> {
    let cancelled = || context.is_some_and(|context| context.cancel.is_cancelled());
    if cancelled() {
        return Err(DaemonError::Cancelled);
    }
    let output = shell
        .run(ShellCommand::new("sh").args(["-c", command]).cwd(cwd))
        .await?;
    if cancelled() {
        return Err(DaemonError::Cancelled);
    }
    Ok(output)
}

/// Streams captured hook output into the job log one line at a time.
pub(super) fn record_output(context: &JobCtx, output: &ShellResult) -> DaemonResult<()> {
    for line in output.stdout.lines().chain(output.stderr.lines()) {
        context.progress(line)?;
    }
    Ok(())
}
