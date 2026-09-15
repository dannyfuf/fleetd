//! Frozen line-oriented scenario grammar and central dispatch table.

use crate::{
    capture,
    client::Client,
    env::{Daemon, HarnessEnv},
    fault::{self, DaemonFault},
    fixture::{self, Injected, Injection, Preset},
    lane::{Lane, LaneBackend},
    report::{self, StepReport},
    rundir::RunDirectory,
};
use anyhow::Context as _;
use fleet_drive::protocol::*;
use std::{
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tokio::process::{Child, Command as Process};

/// `await`'s timeout when a scenario line does not give one.
pub const DEFAULT_AWAIT_TIMEOUT_MS: u64 = 5_000;
/// `drag`'s interpolation steps when a scenario line does not give them.
pub const DEFAULT_DRAG_STEPS: u16 = 8;

/// File extensions a directory run treats as scenarios, so a corpus directory may also hold
/// baselines and transcripts.
const SCENARIO_EXTENSIONS: [&str; 2] = ["scenario", "txt"];
/// How long the runner waits for Fleet to create its harness socket.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(60);
/// How often it probes for that socket while waiting.
const SOCKET_PROBE: Duration = Duration::from_millis(50);
/// How long teardown lets Fleet finish quitting before killing it.
const QUIT_GRACE: Duration = Duration::from_secs(5);
/// How far above `fleet-harness` a sibling binary is looked for: `target/<profile>/deps/` up
/// to `target/`.
const BINARY_SEARCH_DEPTH: usize = 3;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub lane: Lane,
    pub keep: bool,
    pub run_dir: Option<PathBuf>,
    pub continue_on_failure: bool,
    pub update_baselines: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub fixture: Preset,
    pub lines: Vec<ScenarioLine>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioLine {
    pub number: usize,
    pub source: String,
    pub instruction: Instruction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    App(Command),
    Daemon(DaemonFault),
    SocketRemove,
    Fixture(Preset),
    Job(Injected),
}

pub struct ExecutionContext<'a> {
    pub client: &'a mut Client,
    pub environment: &'a HarnessEnv,
    pub daemon: &'a mut Daemon,
    pub lane: &'a dyn LaneBackend,
    pub run_dir: &'a RunDirectory,
    /// The scenario *source* file, which keys its screenshot baselines.
    ///
    /// Deliberately not `run_dir.scenario`: that is the copy at `<run>/scenario.txt`, and keying
    /// baselines by it would file every scenario's images under one `scenario/` directory.
    pub scenario: &'a Path,
    /// The world the preset built, which a `job` line submits work against.
    pub fixture: &'a fixture::Fixture,
    /// Injections still running, so teardown can end a long-running one it started.
    pub injections: &'a mut Vec<Injection>,
    /// Per-run suffix for successful fixture jobs, advanced before submission.
    pub success_ordinal: u8,
    pub update_baselines: bool,
    /// The artifact the last dispatched line wrote, taken by the runner for its report.
    pub artifact: Option<PathBuf>,
}

/// Runs one scenario file, or every scenario under a directory in lexical order.
///
/// The run directory is created and printed before anything starts, the scenario is parsed
/// before Fleet is launched, and teardown runs on every path out — success, failure, a
/// malformed line, or SIGINT.
pub async fn run_path(path: &Path, options: RunOptions) -> anyhow::Result<()> {
    let scenarios = collect_scenarios(path)?;
    // One interrupt stops the whole invocation, not one scenario of it. Shared rather than
    // returned so that teardown still runs unconditionally inside `run_one`.
    let interrupted = AtomicBool::new(false);
    if path.is_file() {
        return run_one(path, options.run_dir.as_deref(), &options, &interrupted).await;
    }
    let suite = match options.run_dir.as_deref() {
        Some(directory) => directory.to_owned(),
        None => RunDirectory::create(None, path)?.root,
    };
    println!(
        "harness suite: {} ({} scenarios, requested lane {})",
        suite.display(),
        scenarios.len(),
        options.lane
    );
    let mut failures = Vec::new();
    let mut outcomes = Vec::with_capacity(scenarios.len());
    for (index, scenario) in scenarios.iter().enumerate() {
        let stem = scenario.file_stem().unwrap_or_default().to_string_lossy();
        let directory = suite.join(format!("{:03}-{stem}", index + 1));
        let started = Instant::now();
        let outcome = run_one(scenario, Some(&directory), &options, &interrupted).await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let error = outcome.as_ref().err().map(|error| format!("{error:#}"));
        outcomes.push(report::ScenarioOutcome {
            scenario: scenario.clone(),
            run_dir: directory,
            ok: error.is_none(),
            duration_ms,
            error: error.clone(),
        });
        if let Some(error) = error {
            failures.push(format!("{}: {error}", scenario.display()));
            if !options.continue_on_failure {
                break;
            }
        }
        if interrupted.load(Ordering::Acquire) {
            break;
        }
    }
    // Written for a green suite as well as a red one: the report is how a reader finds each
    // scenario's own evidence, and `scenarios.len()` is what says how many a stopped suite
    // never reached.
    if let Err(error) =
        report::write_suite_report(&suite, options.lane, &outcomes, scenarios.len()).await
    {
        eprintln!("warning: no suite report was written: {error:#}");
    }
    if failures.is_empty() {
        println!("harness suite passed: {} scenarios", scenarios.len());
        return Ok(());
    }
    // `outcomes.len()` is how many actually ran: a suite that stopped at its first failure
    // never reached the rest, and reporting them as "1 of 38 failed" reads as though 37 passed.
    let not_reached = scenarios.len() - outcomes.len();
    let reached = match not_reached {
        0 => String::new(),
        _ => format!("; {not_reached} never ran"),
    };
    anyhow::bail!(
        "{} of the {} scenarios that ran failed{reached}\n{}\nsuite directory: {}",
        failures.len(),
        outcomes.len(),
        failures.join("\n"),
        suite.display()
    )
}

/// Everything one run started, so teardown can stop it in reverse order no matter how the
/// run ended.
#[derive(Default)]
struct Stage {
    lane: Option<Box<dyn LaneBackend>>,
    daemon: Option<Daemon>,
    app: Option<Child>,
    client: Option<Client>,
    /// Jobs a `job` line submitted; a long-running one has to be told to stop.
    injections: Vec<Injection>,
}

impl Stage {
    /// Stops the client, Fleet, the private daemon and the display lane, collecting rather
    /// than propagating problems so every later step still runs.
    async fn teardown(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        // A long-running injected hook polls for a file and gives up after a minute; telling it
        // to stop now is what keeps it from outliving the run that submitted it.
        for injection in self.injections.drain(..) {
            if let Err(error) = injection.stop() {
                problems.push(format!("stop an injected job: {error:#}"));
            }
        }
        // A scenario may have run `daemon stop` without a matching `daemon cont`. A stopped
        // process answers nothing — not a shutdown request, and not the requests Fleet makes
        // while quitting — so it would spend the whole grace period being waited on and then be
        // killed, turning a passing run into "the scenario passed but teardown failed".
        if let Some(daemon) = self.daemon.as_ref()
            && let Err(error) = fault::resume(daemon).await
        {
            problems.push(format!(
                "resume fleetd before tearing the run down: {error:#}"
            ));
        }
        // A run that ended on a failure never reached its `quit` line. Asking now is what lets
        // Fleet unlink its socket and flush its log, instead of being killed after the grace
        // period with both still on disk.
        if let Some(mut client) = self.client.take()
            && self
                .app
                .as_mut()
                .is_some_and(|app| matches!(app.try_wait(), Ok(None)))
        {
            match client.send(Command::Quit(EmptyArgs {})).await {
                Ok(response) if response.ok => {}
                Ok(response) => problems.push(format!(
                    "Fleet refused to quit: {}",
                    response
                        .error
                        .unwrap_or_else(|| "no reason given".to_owned())
                )),
                // A dead or wedged socket is what the grace period below is for, not a problem
                // worth reporting over whatever actually failed the run.
                Err(_) => {}
            }
        }
        if let Some(mut app) = self.app.take() {
            match stop(&mut app, "Fleet", QUIT_GRACE).await {
                Ok(Some(status)) if !status.success() => problems.push(format!(
                    "Fleet exited with {status} during teardown; see app.log"
                )),
                Ok(_) => {}
                Err(problem) => problems.push(problem),
            }
        }
        if let Some(mut daemon) = self.daemon.take() {
            // The orderly path asks fleetd to exit and then verifies its socket is gone; a kill
            // would leave the socket file behind until `Daemon::drop` reaped it.
            if let Err(error) = daemon.shutdown().await {
                problems.push(format!("shut down fleetd: {error:#}"));
                if let Err(problem) = stop(daemon.child_mut(), "fleetd", Duration::ZERO).await {
                    problems.push(problem);
                }
            }
        }
        if let Some(mut lane) = self.lane.take()
            && let Err(error) = lane.teardown()
        {
            problems.push(format!("tear down the display lane: {error:#}"));
        }
        problems
    }
}

/// Waits out a grace period for a child that was asked to exit, then kills it.
async fn stop(
    child: &mut Child,
    what: &str,
    grace: Duration,
) -> Result<Option<ExitStatus>, String> {
    match child.try_wait() {
        Ok(Some(status)) => return Ok(Some(status)),
        Ok(None) => {}
        Err(error) => return Err(format!("poll {what}: {error}")),
    }
    if !grace.is_zero()
        && let Ok(waited) = tokio::time::timeout(grace, child.wait()).await
    {
        return waited
            .map(Some)
            .map_err(|error| format!("wait for {what}: {error}"));
    }
    child
        .kill()
        .await
        .map_err(|error| format!("kill {what}: {error}"))?;
    Ok(None)
}

/// Awaits the orderly exit promised by a successful `quit` exchange.
async fn await_fleet_exit_after_quit(app: &mut Child) -> anyhow::Result<ExitStatus> {
    let status = match tokio::time::timeout(QUIT_GRACE, app.wait()).await {
        Ok(waited) => waited.context("wait for Fleet after `quit`")?,
        Err(_elapsed) => anyhow::bail!(
            "Fleet answered `quit` but its exit status was still pending after {QUIT_GRACE:?}; \
             the shutdown timed out — see app.log"
        ),
    };
    anyhow::ensure!(
        status.success(),
        "Fleet answered `quit` and then exited with {status}; \
         the shutdown is not orderly — see app.log"
    );
    Ok(status)
}

/// Runs one scenario end to end into its own run directory.
async fn run_one(
    scenario_path: &Path,
    run_dir: Option<&Path>,
    options: &RunOptions,
    interrupted: &AtomicBool,
) -> anyhow::Result<()> {
    let source = tokio::fs::read_to_string(scenario_path)
        .await
        .with_context(|| format!("read scenario {}", scenario_path.display()))?;
    let run_dir = RunDirectory::create(run_dir, scenario_path)?;
    println!(
        "run directory: {} (requested lane {}, scenario {})",
        run_dir.root.display(),
        options.lane,
        scenario_path.display()
    );
    run_dir.write_scenario(&source).await?;

    // A malformed line is rejected before Fleet is ever launched.
    let scenario = match parse(&source) {
        Ok(scenario) => scenario,
        Err(error) => {
            let message = format!("{error:#}");
            run_dir
                .record_event(
                    "scenario-error",
                    serde_json::json!({
                        "scenario": scenario_path.display().to_string(),
                        "error": message.clone(),
                    }),
                )
                .await?;
            // The report is the run directory's index (§8), and a run that was rejected before
            // anything launched needs one just as much: without it the directory holds a
            // journal and a scenario copy with no account of why the run ended.
            run_dir
                .record_event("failed", serde_json::json!({ "error": message.clone() }))
                .await?;
            if let Err(problem) = report::write_report(&run_dir, options.lane, &[]).await {
                eprintln!("warning: no report was written: {problem:#}");
            }
            anyhow::bail!(
                "{}: {message}\nrun directory: {}",
                scenario_path.display(),
                run_dir.root.display()
            );
        }
    };

    let mut stage = Stage::default();
    let mut steps = Vec::new();
    let outcome = if interrupted.load(Ordering::Acquire) {
        Err(anyhow::anyhow!("interrupted before this scenario started"))
    } else {
        tokio::select! {
            biased;
            signal = tokio::signal::ctrl_c() => {
                // Recorded for the *suite*: without this, a `--continue-on-failure` directory
                // run would tear this scenario down and start the next one, and a 40-scenario
                // corpus would need 40 interrupts to stop.
                interrupted.store(true, Ordering::Release);
                Err(match signal {
                    Ok(()) => anyhow::anyhow!("interrupted; tearing the run down"),
                    Err(error) => anyhow::Error::new(error).context("listen for SIGINT"),
                })
            }
            result = execute(&mut stage, scenario_path, &scenario, &run_dir, options, &mut steps) => result,
        }
    };
    // Read before teardown consumes the backend. A `virtual` run can fall back to `attach`
    // *during* the run — the window refuses to move onto the isolated output — and until this
    // was read here, `run.jsonl` and `report.md` both went on calling such a run `virtual`,
    // which is the one thing a reader uses to decide whether its pixels are trustworthy.
    let effective_lane = stage
        .lane
        .as_deref()
        .map_or(options.lane, LaneBackend::lane);
    let fallback = stage.lane.as_deref().and_then(LaneBackend::fallback);
    let problems = stage.teardown().await;
    if let Some(fallback) = fallback {
        match serde_json::to_value(&fallback) {
            Ok(value) => {
                if let Err(error) = run_dir.record_event("lane-fallback", value).await {
                    eprintln!("warning: the lane fallback was not journalled: {error:#}");
                }
            }
            Err(error) => eprintln!("warning: the lane fallback was not journalled: {error}"),
        }
    }

    // Journalled before the report is rendered, not after: a run that died before its first
    // line has no failed *step*, so this `failed` entry is the only account of it the report
    // can show, and the report reads the journal exactly once.
    if let Err(problem) = match &outcome {
        Err(error) => {
            run_dir
                .record_event(
                    "failed",
                    serde_json::json!({ "error": format!("{error:#}") }),
                )
                .await
        }
        Ok(()) => Ok(()),
    } {
        eprintln!("warning: the failure was not journalled: {problem:#}");
    }

    match report::write_report(&run_dir, effective_lane, &steps).await {
        Ok(_path) => {}
        Err(error) => eprintln!("warning: no report was written: {error:#}"),
    }
    for problem in &problems {
        eprintln!("warning: teardown: {problem}");
    }

    match outcome {
        Ok(()) => {
            if options.keep {
                println!("kept the hermetic home at {}", run_dir.home.display());
            } else {
                run_dir.remove_home()?;
            }
            println!(
                "ok: {} steps; run directory: {}",
                steps.len(),
                run_dir.root.display()
            );
            match problems.first() {
                Some(problem) => {
                    anyhow::bail!("the scenario passed but teardown failed: {problem}")
                }
                None => Ok(()),
            }
        }
        // The hermetic home is evidence for a failed run, so it survives regardless of --keep.
        Err(error) => Err(error.context(format!("run directory: {}", run_dir.root.display()))),
    }
}

/// Starts the hermetic stage, then walks the scenario one line at a time.
async fn execute(
    stage: &mut Stage,
    scenario_path: &Path,
    scenario: &Scenario,
    run_dir: &RunDirectory,
    options: &RunOptions,
    steps: &mut Vec<StepReport>,
) -> anyhow::Result<()> {
    let run_id = run_dir.run_id();
    let environment =
        HarnessEnv::new(run_dir).context("prepare the hermetic harness environment")?;
    // The preset seeds FLEET_HOME, so it is applied before fleetd and Fleet read it.
    let fixture = fixture::apply_preset(scenario.fixture, &environment)
        .await
        .with_context(|| format!("apply the {:?} fixture", scenario.fixture))?;
    stage.lane = Some(
        crate::lane::backend(options.lane, &run_id)
            .with_context(|| format!("prepare the {} display lane", options.lane))?,
    );
    stage.daemon = Some(
        Daemon::start(
            &binary("FLEET_DAEMON", "fleetd")?,
            &environment,
            log_handle(&run_dir.daemon_log)?,
            log_handle(&run_dir.daemon_log)?,
        )
        .await
        .context("start the private fleetd")?,
    );
    let app = {
        let Some(lane) = stage.lane.as_deref_mut() else {
            anyhow::bail!("the display lane was not prepared");
        };
        spawn_app(&environment, lane, run_dir, &run_id)?
    };
    stage.app = Some(app);
    let client = {
        let Some(app) = stage.app.as_mut() else {
            anyhow::bail!("Fleet was not started");
        };
        connect(&environment.socket, app).await?
    };
    stage.client = Some(client);
    // The window exists as soon as the socket answers, so the lane can place it now rather than
    // at the first `shot` — a `dump` taken before any screenshot then reports bound geometry.
    if let Some(lane) = stage.lane.as_deref() {
        lane.bind_window()
            .context("bind the harness window to its display lane")?;
    }

    let injections = &mut stage.injections;
    let (Some(lane), Some(daemon), Some(app), Some(client)) = (
        stage.lane.as_deref(),
        stage.daemon.as_mut(),
        stage.app.as_mut(),
        stage.client.as_mut(),
    ) else {
        anyhow::bail!("the harness stage was not fully prepared");
    };
    let mut context = ExecutionContext {
        client,
        environment: &environment,
        daemon,
        lane,
        run_dir,
        scenario: scenario_path,
        fixture: &fixture,
        injections,
        success_ordinal: 0,
        update_baselines: options.update_baselines,
        artifact: None,
    };

    let mut failure = None;
    for line in &scenario.lines {
        if matches!(line.instruction, Instruction::Fixture(_)) {
            // Applied above, before the daemon and Fleet started; journalled here for order.
            run_dir
                .record_event(
                    "fixture",
                    serde_json::json!({ "line": line.number, "source": line.source }),
                )
                .await?;
            continue;
        }

        let started = Instant::now();
        let outcome = dispatch(line, &mut context).await;
        let quit = matches!(line.instruction, Instruction::App(Command::Quit(_)));
        let quit_problem = if quit && matches!(outcome, Ok(Some(_))) {
            await_fleet_exit_after_quit(app)
                .await
                .err()
                .map(|error| format!("{error:#}"))
        } else {
            None
        };
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let artifact = context.artifact.take();

        let (ok, error) = journal_outcome(run_dir, line, &outcome, quit_problem).await?;
        steps.push(StepReport {
            line: line.number,
            source: line.source.clone(),
            ok,
            duration_ms,
            artifact,
            error: error.clone(),
        });

        if !ok {
            capture_failure(&mut context, line.number).await;
            if failure.is_none() {
                // The response object carries the whole failing snapshot, which is written to
                // `dumps/failure-NNN.json` and to `run.jsonl` already. Repeating it here buried
                // the error, the clause and the evidence paths that the reader actually needs
                // under a page of JSON (`docs/TESTING-HARNESS.md` §8: "a failure must be
                // diagnosable without opening a file").
                failure = Some(anyhow::anyhow!(
                    "line {}: {}\n  error: {}",
                    line.number,
                    line.source.trim(),
                    error.unwrap_or_else(|| "the command answered ok:false".to_owned()),
                ));
            }
            if !options.continue_on_failure {
                break;
            }
        }

        if quit {
            break;
        }
        if let Some(status) = app.try_wait().context("poll the Fleet process")? {
            anyhow::bail!(
                "Fleet exited with {status} after line {}: {}; see app.log",
                line.number,
                line.source.trim()
            )
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Writes exactly one journal entry for one executable scenario line.
async fn journal_outcome(
    run_dir: &RunDirectory,
    line: &ScenarioLine,
    outcome: &anyhow::Result<Option<Response>>,
    quit_problem: Option<String>,
) -> anyhow::Result<(bool, Option<String>)> {
    Ok(match (outcome, &line.instruction) {
        (Ok(Some(response)), Instruction::App(command)) => {
            run_dir
                .record(line.number, &Request::new(response.id, command)?, response)
                .await?;
            match quit_problem {
                Some(problem) => (false, Some(problem)),
                None => (response.ok, response.error.clone()),
            }
        }
        (Ok(_), _) => {
            run_dir
                .record_event(
                    "runner",
                    serde_json::json!({ "line": line.number, "source": line.source }),
                )
                .await?;
            (true, None)
        }
        (Err(error), _) => {
            let message = format!("{error:#}");
            run_dir
                .record_event(
                    "error",
                    serde_json::json!({
                        "line": line.number,
                        "source": line.source,
                        "error": message,
                    }),
                )
                .await?;
            (false, Some(message))
        }
    })
}

/// Collects the evidence a failed line leaves behind. Best effort: a dead app or a lane with
/// no pixels must not replace the failure that is actually being reported.
async fn capture_failure(context: &mut ExecutionContext<'_>, line: usize) {
    match context
        .client
        .send(Command::Dump(DumpArgs {
            name: "failure".to_owned(),
        }))
        .await
    {
        Ok(response) => {
            if let Err(error) = context
                .run_dir
                .write_failure_dump(line, &response.data)
                .await
            {
                eprintln!("warning: no failure dump for line {line}: {error:#}");
            }
        }
        Err(error) => eprintln!("warning: no failure dump for line {line}: {error:#}"),
    }
    if let Err(error) = capture::capture_failure(context.lane, context.run_dir, line) {
        eprintln!("warning: no failure screenshot for line {line}: {error:#}");
    }
}

/// Starts Fleet in harness mode on the prepared lane, logging to the run directory.
fn spawn_app(
    environment: &HarnessEnv,
    backend: &mut dyn LaneBackend,
    run_dir: &RunDirectory,
    run_id: &str,
) -> anyhow::Result<Child> {
    let executable = binary("FLEET_APP", "fleet")?;
    let mut command = Process::new(&executable);
    environment.apply_to_app(&mut command);
    command
        .env("FLEET_HARNESS", "1")
        .env("FLEET_HARNESS_RUN_ID", run_id)
        // Only the runner can tell a dedicated virtual output from the developer's own
        // session, so it labels the run for the app's `meta` response.
        .env("FLEET_HARNESS_LANE", backend.lane().as_str())
        .env("FLEET_DAEMON", binary("FLEET_DAEMON", "fleetd")?)
        .stdin(Stdio::null())
        .stdout(log_handle(&run_dir.app_log)?)
        .stderr(log_handle(&run_dir.app_log)?)
        .kill_on_drop(true);
    backend
        .prepare(command.as_std_mut())
        .context("prepare the display lane for Fleet")?;
    command
        .spawn()
        .with_context(|| format!("start {}", executable.display()))
}

/// Waits for Fleet to create its harness socket, failing fast if Fleet exits first.
async fn connect(socket: &Path, app: &mut Child) -> anyhow::Result<Client> {
    let deadline = Instant::now() + SOCKET_TIMEOUT;
    loop {
        if let Some(status) = app.try_wait().context("poll the Fleet process")? {
            anyhow::bail!(
                "Fleet exited with {status} before opening {}; see app.log",
                socket.display()
            );
        }
        if socket.exists()
            && let Ok(client) = Client::connect(socket).await
        {
            return Ok(client);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Fleet did not open {} within {SOCKET_TIMEOUT:?}; see app.log",
            socket.display()
        );
        tokio::time::sleep(SOCKET_PROBE).await;
    }
}

/// Opens a run log for a child's stdout or stderr, appending so both handles share a file.
fn log_handle(path: &Path) -> anyhow::Result<Stdio> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    Ok(Stdio::from(file))
}

/// Resolves a workspace binary: the named override first, then beside `fleet-harness`.
///
/// The search is bounded to the profile directory and its parents inside `target/`, so an
/// unrelated executable further up the filesystem is never picked up.
fn binary(variable: &str, name: &str) -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os(variable) {
        let path = PathBuf::from(path);
        anyhow::ensure!(
            path.is_file(),
            "{variable} points at {}, which is not a file",
            path.display()
        );
        return Ok(path);
    }
    let current = std::env::current_exe().context("locate the running fleet-harness")?;
    let mut directory = current.parent();
    for _level in 0..BINARY_SEARCH_DEPTH {
        let Some(candidate) = directory else { break };
        let path = candidate.join(name);
        if path.is_file() {
            return Ok(path);
        }
        directory = candidate.parent();
    }
    anyhow::bail!("{name} is missing; run cargo build --workspace or set {variable}")
}

/// Lists the scenarios a run covers, recursing in lexical order for a directory.
fn collect_scenarios(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_owned()]);
    }
    anyhow::ensure!(
        path.is_dir(),
        "there is no scenario file or directory at {}",
        path.display()
    );
    let mut found = Vec::new();
    collect_into(path, &mut found)?;
    anyhow::ensure!(
        !found.is_empty(),
        "{} holds no .{} scenario files",
        path.display(),
        SCENARIO_EXTENSIONS.join("/.")
    );
    Ok(found)
}

fn collect_into(directory: &Path, found: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let mut entries = std::fs::read_dir(directory)
        .with_context(|| format!("read {}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("read {}", directory.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_into(&path, found)?;
        } else if is_scenario(&path) {
            found.push(path);
        }
    }
    Ok(())
}

fn is_scenario(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| SCENARIO_EXTENSIONS.contains(&extension))
}

/// Parses one scenario. `fixture:` is optional, and when present must come first.
///
/// A scenario that names no preset gets [`Preset::Empty`], which is a first-run Fleet against
/// the private `FLEET_HOME` the run directory already owns — exactly the state a bare
/// `wait`/`key`/`shot` scenario expects. A corpus scenario still names its preset, and naming it
/// anywhere but the first directive is an error, because a preset seeds the home before the
/// daemon reads it and cannot be applied half way through a run.
pub fn parse(source: &str) -> anyhow::Result<Scenario> {
    let mut fixture = None;
    let mut lines = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let number = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("fixture:") {
            if fixture.is_some() || !lines.is_empty() {
                anyhow::bail!("line {number}: fixture must be the first directive and appear once");
            }
            let preset = Preset::from_str(name.trim())?;
            fixture = Some(preset);
            lines.push(ScenarioLine {
                number,
                source: raw.to_owned(),
                instruction: Instruction::Fixture(preset),
            });
            continue;
        }
        lines.push(ScenarioLine {
            number,
            source: raw.to_owned(),
            instruction: parse_instruction(trimmed)
                .map_err(|error| anyhow::anyhow!("line {number}: {error}"))?,
        });
    }
    Ok(Scenario {
        fixture: fixture.unwrap_or(Preset::Empty),
        lines,
    })
}

/// Dispatches every frozen instruction without requiring changes to this table.
pub async fn dispatch(
    line: &ScenarioLine,
    context: &mut ExecutionContext<'_>,
) -> anyhow::Result<Option<Response>> {
    match &line.instruction {
        // `execute` applies the preset before the daemon starts and skips this line, so reaching
        // it means a direct caller drove `dispatch` itself. Seeding starts a private fleetd
        // against the same FLEET_HOME, which the run's own daemon is already holding.
        Instruction::Fixture(preset) => anyhow::bail!(
            "the `{preset}` preset cannot be applied after the daemon has started; \
             `fixture:` must be the scenario's first directive"
        ),
        Instruction::Job(shape) => {
            let injection = inject_job(context, *shape).await?;
            context.injections.push(injection);
            Ok(None)
        }
        // The scenario-line operation, not the bare runner half: it returns once Fleet has seen
        // the consequence. `fault::daemon` answers in milliseconds while the app needs seconds
        // to reach its banner, so a `dump` on the next line would photograph a connected app.
        Instruction::Daemon(command) => {
            fault::inject(context.client, context.daemon, *command).await?;
            Ok(None)
        }
        Instruction::SocketRemove => {
            fault::remove_socket(context.daemon).await?;
            Ok(None)
        }
        Instruction::App(Command::Shot(args)) => {
            let mut response = context.client.send(Command::Shot(args.clone())).await?;
            if response.ok {
                // An update outside the virtual lane is a deliberate non-recording outcome.
                // Classify it before capture so a headless lane can report `not-recorded`
                // without first failing for its intentional lack of pixels. Ordinary headless
                // shots still reach `capture_command` and fail clearly.
                let path = if context.update_baselines && context.lane.lane() != Lane::Virtual {
                    capture::command_path(context.run_dir, line.number, &args.name)
                } else {
                    capture::capture_command(
                        context.lane,
                        context.run_dir,
                        line.number,
                        &args.name,
                    )?
                };
                let outcome = crate::baseline::Baselines::for_scenario(context.scenario).check(
                    context.lane.lane(),
                    &path,
                    context.update_baselines,
                    crate::baseline::Tolerance::default(),
                )?;
                finish_shot(
                    context.run_dir,
                    &mut context.artifact,
                    path,
                    &outcome,
                    &mut response,
                )
                .await?;
            }
            Ok(Some(response))
        }
        Instruction::App(Command::Dump(args)) => {
            let response = context.client.send(Command::Dump(args.clone())).await?;
            context.artifact = Some(
                context
                    .run_dir
                    .write_dump(line.number, &args.name, &response.data)
                    .await?,
            );
            Ok(Some(response))
        }
        Instruction::App(command) => Ok(Some(context.client.send(command.clone()).await?)),
    }
}

/// Journals a shot verdict and folds a baseline failure into the command response.
async fn finish_shot(
    run_dir: &RunDirectory,
    artifact: &mut Option<PathBuf>,
    path: PathBuf,
    outcome: &crate::baseline::BaselineOutcome,
    response: &mut Response,
) -> anyhow::Result<()> {
    if let Some(event) = outcome.journal_event(&path) {
        run_dir.record_event("baseline", event).await?;
    }
    if path.is_file() {
        *artifact = Some(path);
    }
    if !outcome.passed() {
        response.ok = false;
        response.error = Some(outcome.summary());
    }
    Ok(())
}

/// Submits one `job` line's shape against the run's live daemon.
///
/// The job registry is in-memory, so nothing a preset writes survives to the run: the jobs
/// panel, the toast stack and the sticky error can only be lit from a client talking to the
/// daemon Fleet is attached to. This opens a second connection of its own rather than borrowing
/// Fleet's, because the app's socket carries app commands and nothing else.
async fn inject_job(
    context: &mut ExecutionContext<'_>,
    shape: Injected,
) -> anyhow::Result<Injection> {
    let client = context
        .daemon
        .client()
        .await
        .context("connect to the run's fleetd to inject a job")?;
    let snapshot = client
        .get_snapshot()
        .await
        .map_err(|error| anyhow::anyhow!("read the daemon snapshot: {error}"))?;
    let context_id = snapshot
        .active_context
        .or_else(|| snapshot.contexts.first().map(|entry| entry.id.clone()))
        .context("job injection needs a seeded context; use a preset other than `empty`")?;
    fixture::jobs::inject(
        &client,
        context.fixture,
        &context_id,
        &mut context.success_ordinal,
        shape,
    )
    .await
}

fn parse_instruction(line: &str) -> anyhow::Result<Instruction> {
    let (command, rest) = split_command(line);
    let app = |command| Ok(Instruction::App(command));
    match command {
        "meta" => app(Command::Meta(EmptyArgs {})),
        "quit" => app(Command::Quit(EmptyArgs {})),
        "wait" => app(Command::Wait(WaitArgs {
            millis: number(rest, "wait milliseconds")?,
        })),
        "key" => app(Command::Key(KeyArgs {
            keys: required(rest, "key sequence")?
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        })),
        "type" => app(Command::Type(TypeArgs {
            text: required(rest, "text")?.to_owned(),
        })),
        "shot" => app(Command::Shot(ShotArgs {
            name: required(rest, "shot name")?.to_owned(),
        })),
        "dump" => app(Command::Dump(DumpArgs {
            name: required(rest, "dump name")?.to_owned(),
        })),
        "await" => {
            let (predicate, timeout_ms) = predicate_and_timeout(rest)?;
            fleet_drive::predicate::parse(&predicate)?;
            app(Command::Await(AwaitArgs {
                predicate,
                timeout_ms,
            }))
        }
        "assert" => {
            let predicate = required(rest, "predicate")?.to_owned();
            // Parsing here is the point of a runner-side grammar: a typo fails the scenario
            // before a daemon, a window or a compositor output has been created.
            fleet_drive::predicate::parse(&predicate)?;
            app(Command::Assert(AssertArgs { predicate }))
        }
        "move" => app(Command::Move(MoveArgs {
            to: location(rest)?,
        })),
        "click" => app(Command::Click(parse_click(rest)?)),
        "press" => app(Command::Press(parse_press(rest)?)),
        "release" => app(Command::Release(parse_release(rest)?)),
        "drag" => app(Command::Drag(parse_drag(rest)?)),
        "hover" => {
            let (at, dwell_ms) = location_and_optional_number(rest, 0)?;
            app(Command::Hover(HoverArgs { at, dwell_ms }))
        }
        "scroll" => app(Command::Scroll(parse_scroll(rest)?)),
        "clipboard" => parse_clipboard(rest),
        "resize" => {
            let (width, height) = parse_size(rest)?;
            app(Command::Resize(ResizeArgs { width, height }))
        }
        "blur" => app(Command::Blur(EmptyArgs {})),
        "focus" => app(Command::Focus(EmptyArgs {})),
        "advance" => app(Command::Advance(AdvanceArgs {
            millis: number(rest, "advance milliseconds")?,
        })),
        "daemon" => Ok(Instruction::Daemon(match rest {
            "kill" => DaemonFault::Kill,
            "stop" => DaemonFault::Stop,
            "cont" => DaemonFault::Continue,
            "restart" => DaemonFault::Restart,
            other => anyhow::bail!("unknown daemon command {other:?}"),
        })),
        "job" => Ok(Instruction::Job(parse_job(rest)?)),
        "socket" if rest == "remove" => Ok(Instruction::SocketRemove),
        other => anyhow::bail!("unknown command {other:?}"),
    }
}

/// Parses `job success|failure|long|repeat <count>`.
fn parse_job(rest: &str) -> anyhow::Result<Injected> {
    let (shape, argument) = split_command(rest);
    match shape {
        "success" if argument.is_empty() => Ok(Injected::Success),
        "failure" if argument.is_empty() => Ok(Injected::Failure),
        "long" if argument.is_empty() => Ok(Injected::LongRunning),
        "repeat" => {
            let count = required(argument, "repeat count")?
                .parse::<u8>()
                .map_err(|_| anyhow::anyhow!("invalid repeat count: {argument:?}"))?;
            anyhow::ensure!(count > 0, "a repeated job needs at least one repetition");
            Ok(Injected::Repeated { count })
        }
        "" => anyhow::bail!("job needs one of success, failure, long, repeat <count>"),
        other => anyhow::bail!("unknown job shape {other:?}"),
    }
}

fn split_command(line: &str) -> (&str, &str) {
    line.split_once(char::is_whitespace)
        .map_or((line, ""), |(head, tail)| (head, tail.trim_start()))
}
fn required<'a>(value: &'a str, what: &str) -> anyhow::Result<&'a str> {
    if value.is_empty() {
        anyhow::bail!("missing {what}")
    } else {
        Ok(value)
    }
}
fn number(value: &str, what: &str) -> anyhow::Result<u64> {
    required(value, what)?
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid {what}: {value:?}"))
}
fn location(value: &str) -> anyhow::Result<Location> {
    let parts: Vec<_> = required(value, "target or point")?
        .split_whitespace()
        .collect();
    match parts.as_slice() {
        [target] => Ok(Location::Target {
            target: (*target).to_owned(),
        }),
        [x, y] => Ok(Location::Point {
            x: x.parse()?,
            y: y.parse()?,
        }),
        _ => anyhow::bail!("location must be a target or x y"),
    }
}
fn parse_click(rest: &str) -> anyhow::Result<ClickArgs> {
    let parts: Vec<_> = rest.split_whitespace().collect();
    let (button, button_tokens) = optional_button(&parts);
    let remaining = &parts[button_tokens..];
    let (count, count_tokens) = if remaining.len() >= 2
        && remaining[0].parse::<u8>().is_ok()
        && parse_location_tokens(&remaining[1..]).is_ok()
    {
        (remaining[0].parse()?, 1)
    } else {
        (1, 0)
    };
    let (at, consumed) = parse_location_tokens(&remaining[count_tokens..])?;
    if consumed != remaining[count_tokens..].len() {
        anyhow::bail!("click has unexpected trailing arguments");
    }
    Ok(ClickArgs { at, button, count })
}
fn parse_press(rest: &str) -> anyhow::Result<PressArgs> {
    let parts: Vec<_> = rest.split_whitespace().collect();
    let (button, offset) = optional_button(&parts);
    let (at, consumed) = parse_location_tokens(&parts[offset..])?;
    if consumed != parts[offset..].len() {
        anyhow::bail!("press has unexpected trailing arguments");
    }
    Ok(PressArgs { at, button })
}
fn parse_release(rest: &str) -> anyhow::Result<ReleaseArgs> {
    let press = parse_press(rest)?;
    Ok(ReleaseArgs {
        at: press.at,
        button: press.button,
    })
}
fn parse_drag(rest: &str) -> anyhow::Result<DragArgs> {
    let parts: Vec<_> = rest.split_whitespace().collect();
    let (from, from_tokens) = parse_location_tokens(&parts)?;
    let (to, to_tokens) = parse_location_tokens(&parts[from_tokens..])?;
    let trailing = &parts[from_tokens + to_tokens..];
    let steps = match trailing {
        [] => DEFAULT_DRAG_STEPS,
        [steps] => steps.parse()?,
        _ => anyhow::bail!("drag has unexpected trailing arguments"),
    };
    Ok(DragArgs {
        from,
        to,
        button: MouseButton::Left,
        steps,
    })
}
fn parse_scroll(rest: &str) -> anyhow::Result<ScrollArgs> {
    let mut parts = rest.split_whitespace();
    let dx = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("scroll needs dx dy"))?
        .parse()?;
    let dy = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("scroll needs dx dy"))?
        .parse()?;
    let remaining = parts.collect::<Vec<_>>();
    let at = if remaining.is_empty() {
        None
    } else {
        let slice = if remaining.first() == Some(&"at") {
            &remaining[1..]
        } else {
            &remaining[..]
        };
        Some(location(&slice.join(" "))?)
    };
    Ok(ScrollArgs { dx, dy, at })
}
fn parse_clipboard(rest: &str) -> anyhow::Result<Instruction> {
    let (verb, value) = split_command(rest);
    match verb {
        "set" => Ok(Instruction::App(Command::ClipboardSet(ClipboardSetArgs {
            text: required(value, "clipboard text")?.to_owned(),
        }))),
        "get" if value.is_empty() => Ok(Instruction::App(Command::ClipboardGet(EmptyArgs {}))),
        other => anyhow::bail!("unknown clipboard command {other:?}"),
    }
}
fn parse_size(rest: &str) -> anyhow::Result<(u32, u32)> {
    let (width, height) = rest
        .split_once(['x', 'X'])
        .ok_or_else(|| anyhow::anyhow!("resize needs WxH"))?;
    Ok((width.parse()?, height.parse()?))
}
/// Splits `await`'s optional trailing timeout from the predicate it follows.
///
/// Only the final `&&` atom can carry a timeout, and a trailing token is read as one only when
/// that atom is already complete in one of the grammar's four shapes. So
/// `await toasts[0].count == 3` stays a predicate while
/// `await toasts[0].text == saved 3000` yields a 3000 ms timeout, and a conjunction keeps its
/// timeout instead of swallowing it into the last value.
fn predicate_and_timeout(rest: &str) -> anyhow::Result<(String, u64)> {
    let value = required(rest, "predicate")?;
    let parts = predicate_tokens(value);
    let atom = parts
        .iter()
        .rposition(|token| token == "&&")
        .map_or(0, |index| index + 1);
    // Every arm fixes the atom's length exactly, so the token after it is the last one.
    let atom_words = match parts[atom..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [_, "exists" | "absent", _] => 2,
        ["idle", timeout] if timeout.parse::<u64>().is_ok() => 1,
        [_, "==" | "!=" | "~=" | ">" | "<", _, _] => 3,
        _ => return Ok((value.to_owned(), DEFAULT_AWAIT_TIMEOUT_MS)),
    };
    let timeout = parts[atom + atom_words].parse()?;
    Ok((parts[..atom + atom_words].join(" "), timeout))
}

/// Splits a predicate into whitespace-separated tokens, keeping a quoted value in one piece.
///
/// The predicate lexer treats `"…"` as one value, so the timeout split has to as well: without
/// this, `await toasts[0].text == "job failed" 3000` reads `failed"` as its timeout and fails a
/// scenario that is perfectly legal. Only `\"` and `\\` are escapes inside quotes, which is the
/// rule `fleet_drive::predicate` documents; the quotes are preserved so the joined predicate is
/// byte-identical to the source the app parses.
fn predicate_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut characters = value.chars();
    let mut quoted = false;
    while let Some(character) = characters.next() {
        match character {
            '"' => {
                quoted = !quoted;
                token.push(character);
            }
            '\\' if quoted => {
                token.push(character);
                if let Some(escaped) = characters.next() {
                    token.push(escaped);
                }
            }
            _ if character.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            _ => token.push(character),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}
fn location_and_optional_number(rest: &str, default: u64) -> anyhow::Result<(Location, u64)> {
    let parts: Vec<_> = rest.split_whitespace().collect();
    let (location, consumed) = parse_location_tokens(&parts)?;
    let number = match &parts[consumed..] {
        [] => default,
        [number] => number.parse()?,
        _ => anyhow::bail!("location has unexpected trailing arguments"),
    };
    Ok((location, number))
}
fn parse_location_tokens(parts: &[&str]) -> anyhow::Result<(Location, usize)> {
    let first = parts
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing target or point"))?;
    if let Ok(x) = first.parse::<f32>() {
        let y = parts
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("point needs x y"))?
            .parse()?;
        Ok((Location::Point { x, y }, 2))
    } else {
        Ok((
            Location::Target {
                target: (*first).to_owned(),
            },
            1,
        ))
    }
}
fn optional_button(parts: &[&str]) -> (MouseButton, usize) {
    match parts.first().copied() {
        Some("left") => (MouseButton::Left, 1),
        Some("right") => (MouseButton::Right, 1),
        Some("middle") => (MouseButton::Middle, 1),
        _ => (MouseButton::Left, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
        net::UnixListener,
    };

    const FAKE_FLEET_EXIT_134: &str = "FLEET_HARNESS_FAKE_FLEET_EXIT_134";

    /// Builds a throwaway corpus directory; the caller removes it.
    fn corpus(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fleet-harness-scenario-{}-{name}",
            std::process::id()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear corpus");
        }
        std::fs::create_dir_all(root.join("hub")).expect("create corpus");
        std::fs::create_dir_all(root.join("baselines")).expect("create baselines");
        for (path, body) in [
            ("b.scenario", "fixture: empty\nquit\n"),
            ("a.txt", "fixture: empty\nquit\n"),
            ("hub/palette.scenario", "fixture: busy\nquit\n"),
            ("baselines/001-hub.png", "not a scenario"),
            ("notes.md", "not a scenario"),
        ] {
            std::fs::write(root.join(path), body).expect("write corpus file");
        }
        root
    }

    #[test]
    fn a_directory_run_walks_scenarios_recursively_in_lexical_order() {
        let root = corpus("collect");
        let found = collect_scenarios(&root).expect("collect scenarios");
        let relative: Vec<_> = found
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            relative,
            vec!["a.txt", "b.scenario", "hub/palette.scenario"]
        );
        assert_eq!(
            collect_scenarios(&root.join("a.txt")).expect("collect one"),
            vec![root.join("a.txt")]
        );
        assert!(collect_scenarios(&root.join("baselines")).is_err());
        assert!(collect_scenarios(&root.join("missing")).is_err());
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[tokio::test]
    async fn a_malformed_line_is_rejected_before_anything_launches() {
        let root = std::env::temp_dir().join(format!(
            "fleet-harness-scenario-{}-malformed",
            std::process::id()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear");
        }
        std::fs::create_dir_all(&root).expect("create");
        let scenario = root.join("bad.scenario");
        std::fs::write(&scenario, "fixture: empty\nwait 100\nwiggle a bit\nquit\n")
            .expect("write scenario");
        let run_dir = root.join("run");
        let error = run_one(
            &scenario,
            Some(&run_dir),
            &RunOptions {
                lane: Lane::Headless,
                keep: false,
                run_dir: None,
                continue_on_failure: false,
                update_baselines: false,
            },
            &AtomicBool::new(false),
        )
        .await
        .expect_err("a malformed line fails the run");
        let message = format!("{error:#}");
        assert!(message.contains("line 3"), "{message}");
        assert!(message.contains("wiggle"), "{message}");
        assert!(
            message.contains(&run_dir.display().to_string()),
            "{message}"
        );
        // The evidence survives even though nothing was launched.
        assert_eq!(
            std::fs::read_to_string(run_dir.join("scenario.txt")).expect("scenario copy"),
            "fixture: empty\nwait 100\nwiggle a bit\nquit\n"
        );
        assert!(
            std::fs::read_to_string(run_dir.join("run.jsonl"))
                .expect("journal")
                .contains("scenario-error")
        );
        // A run rejected before anything launched still has the report that indexes it (§8).
        let report = std::fs::read_to_string(run_dir.join("report.md")).expect("report");
        assert!(
            report.contains("wiggle"),
            "the report says why the run never started: {report}"
        );
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn the_grammar_reaches_every_command_family_and_defaults_its_fixture() {
        let scenario = parse("# KEYMAP.md\nfixture: busy\nkey ?\nawait idle 50\nclick worktrees.row[0]\nclipboard set hello world\ndaemon stop\nsocket remove\n").expect("parse scenario");
        assert_eq!(scenario.fixture, Preset::Busy);
        assert_eq!(scenario.lines.len(), 7);

        // A scenario that names no preset gets the first-run one, and keeps every other line.
        let bare = parse("wait 500\nkey ?\nshot help\nquit\n").expect("parse scenario");
        assert_eq!(bare.fixture, Preset::Empty);
        assert_eq!(bare.lines.len(), 4);
        assert!(
            !bare
                .lines
                .iter()
                .any(|line| matches!(line.instruction, Instruction::Fixture(_)))
        );

        // Naming it late is still an error: a preset seeds the home before fleetd reads it.
        let late = parse("key ?\nfixture: busy\n").expect_err("a late fixture is rejected");
        assert!(
            format!("{late:#}").contains("must be the first directive"),
            "{late:#}"
        );
    }

    #[test]
    fn await_idle_forms_keep_operators_separate_from_numeric_timeouts() {
        for (source, predicate, timeout_ms) in [
            ("await idle", "idle", DEFAULT_AWAIT_TIMEOUT_MS),
            ("await idle 3000", "idle", 3_000),
            ("await idle exists", "idle exists", DEFAULT_AWAIT_TIMEOUT_MS),
            ("await idle absent", "idle absent", DEFAULT_AWAIT_TIMEOUT_MS),
            ("await idle exists 3000", "idle exists", 3_000),
        ] {
            let parsed = parse_instruction(source).expect("parse await idle form");
            assert_eq!(
                parsed,
                Instruction::App(Command::Await(AwaitArgs {
                    predicate: predicate.to_owned(),
                    timeout_ms,
                })),
                "{source}"
            );
        }
    }

    #[test]
    fn a_quoted_predicate_value_keeps_its_spaces_and_its_timeout() {
        assert_eq!(
            predicate_and_timeout(r#"toasts[0].text == "job failed" 3000"#).expect("predicate"),
            (r#"toasts[0].text == "job failed""#.into(), 3000)
        );
        assert_eq!(
            predicate_and_timeout(r#"toasts[0].text == "job failed""#).expect("predicate"),
            (
                r#"toasts[0].text == "job failed""#.into(),
                DEFAULT_AWAIT_TIMEOUT_MS
            )
        );
        // An escaped quote does not end the value, so the token after it is not a timeout.
        assert_eq!(
            predicate_tokens(r#"a == "say \"hi\" now" 12"#),
            vec!["a", "==", r#""say \"hi\" now""#, "12"]
        );
        // A malformed predicate fails at scenario load, before anything is launched.
        assert!(parse_instruction("await screen ==").is_err());
        assert!(parse_instruction("assert screen ==").is_err());
    }

    #[test]
    fn pointer_forms_preserve_buttons_counts_points_and_defaults() {
        assert_eq!(
            parse_instruction("click right 2 10 20").expect("click"),
            Instruction::App(Command::Click(ClickArgs {
                at: Location::Point { x: 10.0, y: 20.0 },
                button: MouseButton::Right,
                count: 2,
            }))
        );
        assert_eq!(
            parse_instruction("drag 1 2 board.column[1] 12").expect("drag"),
            Instruction::App(Command::Drag(DragArgs {
                from: Location::Point { x: 1.0, y: 2.0 },
                to: Location::Target {
                    target: "board.column[1]".into()
                },
                button: MouseButton::Left,
                steps: 12,
            }))
        );
        assert_eq!(
            predicate_and_timeout("jobs.running > 0").expect("predicate"),
            ("jobs.running > 0".into(), DEFAULT_AWAIT_TIMEOUT_MS)
        );
        // A trailing timeout survives a conjunction instead of being parsed as the last value.
        assert_eq!(
            predicate_and_timeout("overlay == Palette && idle 750").expect("predicate"),
            ("overlay == Palette && idle".into(), 750)
        );
        assert_eq!(
            predicate_and_timeout("overlay == Palette && palette.rows[0].label ~= ^open 900")
                .expect("predicate"),
            (
                "overlay == Palette && palette.rows[0].label ~= ^open".into(),
                900
            )
        );
        assert_eq!(
            predicate_and_timeout("overlay == Palette && idle").expect("predicate"),
            (
                "overlay == Palette && idle".into(),
                DEFAULT_AWAIT_TIMEOUT_MS
            )
        );
    }

    #[test]
    fn every_frozen_scenario_line_parses_before_implementation_stages_fan_out() {
        let source = r#"fixture: agents
meta
wait 1
key ctrl-s ?
type hello world
shot hub
dump hub
await idle 200
assert overlay absent
move 10 20
click right 2 worktrees.row[0]
press middle 10 20
release middle 10 20
drag worktrees.row[0] worktrees.row[1] 4
hover worktrees.row[0] 50
scroll 0 -3 at worktrees.row[0]
clipboard set hello world
clipboard get
resize 1440x900
blur
focus
advance 800
daemon kill
daemon stop
daemon cont
daemon restart
job success
job failure
job long
job repeat 3
socket remove
quit
"#;
        let scenario = parse(source).expect("parse complete frozen grammar");
        assert_eq!(scenario.lines.len(), 32);
    }

    #[test]
    fn a_job_line_names_the_shape_it_injects() {
        let shape = |line: &str| match parse_instruction(line) {
            Ok(Instruction::Job(shape)) => shape,
            other => panic!("{line:?} is not a job line: {other:?}"),
        };
        assert_eq!(shape("job success"), Injected::Success);
        assert_eq!(shape("job failure"), Injected::Failure);
        assert_eq!(shape("job long"), Injected::LongRunning);
        assert_eq!(shape("job repeat 3"), Injected::Repeated { count: 3 });
        for bad in [
            "job",
            "job repeat",
            "job repeat 0",
            "job repeat x",
            "job nope",
        ] {
            assert!(parse_instruction(bad).is_err(), "{bad:?} must be rejected");
        }
        // A shape that takes no argument rejects one rather than ignoring it.
        assert!(parse_instruction("job success 3").is_err());
    }

    #[tokio::test]
    async fn fake_fleet_process_exits_134_after_answering_quit() {
        if std::env::var_os(FAKE_FLEET_EXIT_134).is_none() {
            return;
        }
        let socket = PathBuf::from(
            std::env::var_os("FLEET_HARNESS_SOCK")
                .expect("the parent test gives fake Fleet a socket"),
        );
        let listener = UnixListener::bind(&socket).expect("bind fake Fleet socket");
        println!("FAKE_FLEET_READY");
        std::io::Write::flush(&mut std::io::stdout()).expect("flush fake Fleet readiness");
        let (stream, _address) = listener.accept().await.expect("accept runner");
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read quit");
        let request: Request = serde_json::from_str(line.trim_end()).expect("decode quit");
        assert_eq!(request.cmd, "quit");
        let mut response = serde_json::to_vec(&Response::ok(
            request.id,
            serde_json::json!({ "closed": true }),
        ))
        .expect("encode quit response");
        response.push(b'\n');
        writer.write_all(&response).await.expect("answer quit");
        writer.flush().await.expect("flush quit response");
        std::process::exit(134);
    }

    #[tokio::test]
    async fn a_run_fails_when_fake_fleet_exits_134_after_quit() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let socket = directory.path().join("fleet.sock");
        let executable = std::env::current_exe().expect("locate this test binary");
        let mut app = Process::new(executable)
            .arg("--exact")
            .arg("scenario::tests::fake_fleet_process_exits_134_after_answering_quit")
            .arg("--nocapture")
            .env(FAKE_FLEET_EXIT_134, "1")
            .env("FLEET_HARNESS_SOCK", &socket)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start fake Fleet");
        let stdout = app.stdout.take().expect("capture fake Fleet readiness");
        let mut stdout = BufReader::new(stdout);
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut line = String::new();
            loop {
                line.clear();
                let read = stdout
                    .read_line(&mut line)
                    .await
                    .expect("read fake Fleet readiness");
                assert!(read > 0, "fake Fleet exited before opening its socket");
                if line.trim_end() == "FAKE_FLEET_READY" {
                    break;
                }
            }
        })
        .await
        .expect("fake Fleet opens its socket");
        let mut client = Client::connect(&socket)
            .await
            .expect("connect after explicit readiness");
        let response = client
            .send(Command::Quit(EmptyArgs {}))
            .await
            .expect("fake Fleet answers quit");
        assert!(response.ok);

        let failure = await_fleet_exit_after_quit(&mut app)
            .await
            .expect_err("exit 134 fails the run");
        let message = format!("{failure:#}");
        assert!(
            message.contains("134"),
            "the failure names the status: {message}"
        );
    }

    #[tokio::test]
    async fn a_differed_shot_records_one_failed_exchange_before_the_next_line() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let run_dir = RunDirectory::create(
            Some(&directory.path().join("run")),
            Path::new("shot.scenario"),
        )
        .expect("create run directory");
        let args = ShotArgs {
            name: "hub".to_owned(),
        };
        let mut response = Response::ok(
            7,
            serde_json::json!({ "window": { "x": 11, "y": 22, "width": 1440, "height": 900 } }),
        );
        let shot = run_dir.shots.join("003-hub.png");
        std::fs::write(&shot, b"captured png").expect("write captured shot");
        let outcome = crate::baseline::BaselineOutcome::Differed {
            baseline: directory.path().join("baseline.png"),
            differing_pixels: 2,
            total_pixels: 10,
            diff: run_dir.shots.join("003-hub-diff.png"),
        };
        let mut artifact = None;
        finish_shot(
            &run_dir,
            &mut artifact,
            shot.clone(),
            &outcome,
            &mut response,
        )
        .await
        .expect("fold the baseline verdict into the response");
        assert!(!response.ok);
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("2 of 10 pixels"))
        );

        let shot_line = ScenarioLine {
            number: 3,
            source: "shot hub".to_owned(),
            instruction: Instruction::App(Command::Shot(args)),
        };
        let shot_outcome = Ok(Some(response.clone()));
        let (ok, error) = journal_outcome(&run_dir, &shot_line, &shot_outcome, None)
            .await
            .expect("journal failed shot");
        assert!(!ok);
        assert_eq!(error, response.error);

        let dump_line = ScenarioLine {
            number: 4,
            source: "dump after-shot".to_owned(),
            instruction: Instruction::App(Command::Dump(DumpArgs {
                name: "after-shot".to_owned(),
            })),
        };
        let dump_response = Response::ok(8, serde_json::json!({ "screen": "Hub" }));
        journal_outcome(&run_dir, &dump_line, &Ok(Some(dump_response)), None)
            .await
            .expect("journal the next executed line");

        let journal = std::fs::read_to_string(&run_dir.journal).expect("read journal");
        let exchanges = journal
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("decode journal"))
            .filter(|entry| entry["kind"] == "command")
            .collect::<Vec<_>>();
        assert_eq!(exchanges.len(), 2, "one exchange per executed line");
        assert_eq!(exchanges[0]["data"]["line"], 3);
        assert_eq!(exchanges[0]["request"]["cmd"], "shot");
        assert_eq!(exchanges[0]["response"]["data"], response.data);
        assert_eq!(
            exchanges[0]["response"]["error"],
            serde_json::json!(response.error.as_deref())
        );
        assert_eq!(exchanges[1]["data"]["line"], 4);
        assert_eq!(exchanges[1]["request"]["cmd"], "dump");
        assert_eq!(artifact.as_deref(), Some(shot.as_path()));
    }

    #[tokio::test]
    async fn a_not_recorded_shot_has_a_verdict_but_no_screenshot_artifact() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let run_dir = RunDirectory::create(
            Some(&directory.path().join("run")),
            Path::new("shot.scenario"),
        )
        .expect("create run directory");
        let path = capture::command_path(&run_dir, 14, "help");
        let outcome = crate::baseline::BaselineOutcome::NotRecorded {
            lane: "headless".to_owned(),
        };
        let mut response = Response::ok(1, serde_json::json!({ "name": "help" }));
        let mut artifact = None;

        finish_shot(&run_dir, &mut artifact, path, &outcome, &mut response)
            .await
            .expect("record the non-recording verdict");

        assert!(response.ok);
        assert!(
            artifact.is_none(),
            "no PNG was captured in the headless lane"
        );
        let journal = std::fs::read_to_string(&run_dir.journal).expect("read journal");
        assert!(journal.contains("not recorded (lane: headless)"));
        assert!(journal.contains("not-recorded"));
    }
}
