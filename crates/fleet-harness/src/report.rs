//! `report.md` — the one file a reader who did not watch a run opens first.
//!
//! `run.jsonl` stays the complete record. This is its digest, and it is deliberately small: a
//! scenario's report fits on a screen, a suite's report fits on two. Both lead with the failure,
//! because the only question a red run is ever asked is "what broke, and where is the evidence?".
//!
//! Everything here is assembled from what is already on disk — the journal, the shots, the dumps —
//! so the report can only ever describe what actually happened. A report that claimed something
//! `run.jsonl` does not show would be worse than no report at all.

use crate::{lane::Lane, rundir::RunDirectory};
use anyhow::Context as _;
use fleet_drive::protocol::{Request, Response};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The report's file name, in a run directory and in a suite directory alike.
const REPORT: &str = "report.md";
/// The journal the report digests, named by `rundir.rs`.
const JOURNAL: &str = "run.jsonl";
/// The screenshot subdirectory the report inlines from, named by `rundir.rs`.
const SHOTS: &str = "shots";
/// How much of a scenario line or a detail a table cell shows before it is clipped.
const CELL: usize = 88;
/// Journal entry kinds that stand one-to-one with the scenario lines that were executed.
const EXCHANGES: [&str; 3] = ["command", "runner", "error"];
/// The seven `idle` counters, in the order `docs/TESTING-HARNESS.md` §3 lists them.
const IDLE_COUNTERS: [&str; 7] = [
    "in_flight_requests",
    "settling_mutations",
    "running_jobs",
    "pending_frame",
    "live_toast_timers",
    "armed_debounces",
    "link_opening",
];

/// One executed scenario line, as the runner saw it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StepReport {
    /// The 1-based scenario line this step came from.
    pub line: usize,
    /// That line, verbatim.
    pub source: String,
    /// Whether the line succeeded.
    pub ok: bool,
    /// Wall-clock time the line took, dispatch to answer.
    pub duration_ms: u64,
    /// The screenshot or dump the line wrote, if it wrote one.
    pub artifact: Option<PathBuf>,
    /// Why the line failed.
    pub error: Option<String>,
}

/// One scenario's place in a suite run.
///
/// The runner fills this in as it walks a directory; everything else the suite report shows is
/// read back out of the scenario's own run directory.
#[derive(Debug, Clone)]
pub struct ScenarioOutcome {
    /// The scenario file that ran.
    pub scenario: PathBuf,
    /// Its run directory, below the suite directory.
    pub run_dir: PathBuf,
    /// Whether it passed.
    pub ok: bool,
    /// How long it took, start to teardown.
    pub duration_ms: u64,
    /// The failure, already formatted by the runner.
    pub error: Option<String>,
}

/// Writes one scenario's `report.md` and echoes any failure to stdout.
///
/// Stdout is the contract: a failure must be diagnosable without opening a file, with the
/// artifacts as backup. The returned path is the report.
///
/// # Errors
///
/// Only if `report.md` itself cannot be written. An unreadable or truncated journal degrades the
/// report and is reported inside it, because a run that died mid-write is when one is needed most.
pub async fn write_report(
    run_dir: &RunDirectory,
    lane: Lane,
    steps: &[StepReport],
) -> anyhow::Result<PathBuf> {
    let journal = Journal::read(&run_dir.journal).await;
    let entries = align(steps, &journal);
    let failures = failures(run_dir, steps, &entries, &journal).await;
    let report = RunReport {
        run_dir,
        lane,
        steps,
        journal: &journal,
        entries,
        failures,
    };
    let path = run_dir.root.join(REPORT);
    tokio::fs::write(&path, report.render())
        .await
        .with_context(|| format!("write {}", path.display()))?;
    report.echo(&path);
    Ok(path)
}

/// Writes the suite `report.md` above a directory of scenario run directories.
///
/// `planned` is how many scenarios the run set out to execute, so a suite that stopped at its
/// first failure says how many it never reached rather than silently shrinking.
///
/// # Errors
///
/// Only if the suite `report.md` cannot be written.
pub async fn write_suite_report(
    suite: &Path,
    lane: Lane,
    outcomes: &[ScenarioOutcome],
    planned: usize,
) -> anyhow::Result<PathBuf> {
    let mut scenarios = Vec::with_capacity(outcomes.len());
    for outcome in outcomes {
        scenarios.push(SuiteScenario::read(outcome).await);
    }
    let report = SuiteReport {
        suite,
        lane,
        planned,
        scenarios,
    };
    let path = suite.join(REPORT);
    tokio::fs::write(&path, report.render())
        .await
        .with_context(|| format!("write {}", path.display()))?;
    report.echo(&path);
    Ok(path)
}

// ---------------------------------------------------------------- the journal, read back

/// The run journal, read as far as it is readable.
#[derive(Debug, Default)]
struct Journal {
    entries: Vec<JournalEntry>,
    /// Why the journal is incomplete, when it is.
    problem: Option<String>,
}

/// One journal line. Unknown fields are ignored so an entry kind this report does not know about
/// costs nothing.
#[derive(Debug, Deserialize)]
struct JournalEntry {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    request: Option<Request>,
    #[serde(default)]
    response: Option<Response>,
    #[serde(default)]
    data: Option<Value>,
}

impl JournalEntry {
    /// The command name, for an exchange with the app.
    fn command(&self) -> Option<&str> {
        self.request.as_ref().map(|request| request.cmd.as_str())
    }

    /// The scenario line this entry names, for the runner-side kinds that carry one.
    fn line(&self) -> Option<usize> {
        let line = self.data.as_ref()?.get("line")?.as_u64()?;
        usize::try_from(line).ok()
    }

    /// A field of the response payload.
    fn answer(&self, key: &str) -> Option<&Value> {
        self.response.as_ref()?.data.get(key)
    }

    /// A field of a runner-side event payload.
    fn event(&self, key: &str) -> Option<&Value> {
        self.data.as_ref()?.get(key)
    }
}

impl Journal {
    async fn read(path: &Path) -> Self {
        let text = match tokio::fs::read_to_string(path).await {
            Ok(text) => text,
            Err(error) => {
                return Self {
                    entries: Vec::new(),
                    problem: Some(format!("`{}` could not be read: {error}", display(path))),
                };
            }
        };
        let mut entries = Vec::new();
        let mut malformed = 0_usize;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<JournalEntry>(line) {
                Ok(entry) => entries.push(entry),
                Err(_) => malformed += 1,
            }
        }
        Self {
            entries,
            problem: (malformed > 0).then(|| {
                format!("{malformed} journal lines were malformed and are not summarised here")
            }),
        }
    }

    fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a JournalEntry> {
        self.entries.iter().filter(move |entry| entry.kind == kind)
    }

    fn started(&self) -> Option<&str> {
        self.entries.first()?.at.as_deref()
    }
}

/// Pairs each step with its journal entry.
///
/// The runner writes exactly one `command`, `runner` or `error` entry per executed line, in order,
/// so the pairing is positional. It is verified against the entry's required line number and
/// abandoned the moment the two disagree: an unenriched report is honest, a misaligned one is a lie
/// with a table around it.
fn align<'a>(steps: &[StepReport], journal: &'a Journal) -> Vec<Option<&'a JournalEntry>> {
    let mut exchanges = journal
        .entries
        .iter()
        .filter(|entry| EXCHANGES.contains(&entry.kind.as_str()));
    let mut aligned = Vec::with_capacity(steps.len());
    let mut trustworthy = true;
    for step in steps {
        let paired = trustworthy
            .then(|| exchanges.next())
            .flatten()
            .filter(|entry| entry.line() == Some(step.line));
        trustworthy = paired.is_some();
        aligned.push(paired);
    }
    aligned
}

// ---------------------------------------------------------------- one failed line

/// Everything the report and stdout say about one failure.
#[derive(Debug, Default)]
struct Failure {
    /// The scenario line, or `None` for a failure that happened before the first line ran.
    line: Option<usize>,
    source: String,
    error: String,
    /// The predicate clauses that did not hold, with the value actually seen.
    actuals: Vec<String>,
    /// The `idle` counters still busy when an `await idle` timed out.
    busy: Vec<String>,
    /// A one-line reading of the snapshot the app answered with.
    state: Option<String>,
    shot: Option<PathBuf>,
    dump: Option<PathBuf>,
}

/// Collects the failures of one run, newest evidence attached.
async fn failures(
    run_dir: &RunDirectory,
    steps: &[StepReport],
    entries: &[Option<&JournalEntry>],
    journal: &Journal,
) -> Vec<Failure> {
    let mut failures = Vec::new();
    for (step, entry) in steps.iter().zip(entries) {
        if step.ok {
            continue;
        }
        let mut failure = Failure {
            line: Some(step.line),
            source: step.source.trim().to_owned(),
            error: step
                .error
                .clone()
                .unwrap_or_else(|| "the command answered ok:false".to_owned()),
            ..Failure::default()
        };
        if let Some(entry) = entry {
            failure.actuals = unmet_clauses(entry);
            failure.busy = busy_counters(entry);
            failure.state = entry.answer("snapshot").and_then(snapshot_summary);
        }
        failure.shot = exists(run_dir.shots.join(format!("failure-{:03}.png", step.line))).await;
        failure.dump = exists(run_dir.dumps.join(format!("failure-{:03}.json", step.line))).await;
        failures.push(failure);
    }
    // A run that fell over before its first line has no step to hang the failure on; the runner's
    // own journalled failure is the only account of it there is.
    if failures.is_empty()
        && let Some(entry) = journal.of_kind("failed").next()
        && let Some(error) = entry.event("error").map(scalar)
    {
        failures.push(Failure {
            error,
            source: "the run failed outside any scenario line".to_owned(),
            ..Failure::default()
        });
    }
    failures
}

/// The clauses of a predicate that did not hold, each with the value the app actually saw.
fn unmet_clauses(entry: &JournalEntry) -> Vec<String> {
    let Some(clauses) = entry.answer("clauses").and_then(Value::as_array) else {
        return Vec::new();
    };
    clauses
        .iter()
        .filter(|clause| clause.get("satisfied").and_then(Value::as_bool) != Some(true))
        .map(|clause| {
            let text = clause.get("clause").map_or_else(
                || "the predicate".to_owned(),
                |clause| scalar(clause).trim().to_owned(),
            );
            match clause.get("actual") {
                Some(Value::Null) | None => format!("`{text}` — nothing at that path"),
                Some(actual) => format!("`{text}` — actually `{}`", scalar(actual)),
            }
        })
        .collect()
}

/// The `idle` counters that were still non-zero, so an `await idle` timeout says which half is busy.
fn busy_counters(entry: &JournalEntry) -> Vec<String> {
    let Some(idle) = entry.answer("idle") else {
        return Vec::new();
    };
    IDLE_COUNTERS
        .iter()
        .filter_map(|counter| {
            let value = idle.get(counter)?;
            busy(value).then(|| format!("`{counter}` = {}", scalar(value)))
        })
        .collect()
}

/// Whether an `idle` counter reports outstanding work.
fn busy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_u64().unwrap_or(0) > 0,
        _ => false,
    }
}

/// Reads a snapshot the way the app's own `dump` summary does, for a snapshot that arrived without
/// one because it rode along on a failure.
fn snapshot_summary(snapshot: &Value) -> Option<String> {
    let field = |key: &str| snapshot.get(key).map(scalar);
    let mut parts = vec![format!("screen={}", field("screen")?)];
    for key in ["mode", "overlay", "focused"] {
        if let Some(value) = field(key) {
            parts.push(format!("{key}={value}"));
        }
    }
    if let Some(contexts) = snapshot.get("key_contexts").and_then(Value::as_array)
        && !contexts.is_empty()
    {
        let names: Vec<_> = contexts.iter().map(scalar).collect();
        parts.push(format!("keys={}", names.join(" > ")));
    }
    if let Some(text) = snapshot.pointer("/toasts/0/text") {
        parts.push(format!("toast={}", scalar(text)));
    }
    if let Some(error) = snapshot
        .get("sticky_error")
        .filter(|value| !value.is_null())
    {
        parts.push(format!("sticky_error={}", scalar(error)));
    }
    Some(parts.join(" "))
}

// ---------------------------------------------------------------- one scenario's report

struct RunReport<'a> {
    run_dir: &'a RunDirectory,
    lane: Lane,
    steps: &'a [StepReport],
    journal: &'a Journal,
    entries: Vec<Option<&'a JournalEntry>>,
    failures: Vec<Failure>,
}

impl RunReport<'_> {
    fn render(&self) -> String {
        let mut out = String::new();
        self.header(&mut out);
        // The failure comes before the narrative, never after it.
        failure_section(&mut out, &self.run_dir.root, &self.failures);
        self.steps_section(&mut out);
        self.shots_section(&mut out);
        self.assertions_section(&mut out);
        self.dumps_section(&mut out);
        self.artifacts_section(&mut out);
        out
    }

    fn header(&self, out: &mut String) {
        out.push_str(&format!(
            "# Fleet harness run — {}\n\n",
            self.run_dir.run_id()
        ));
        match (self.failures.first(), self.steps.is_empty()) {
            (Some(failure), false) => {
                out.push_str(&format!("**FAILED** — {}\n\n", failure.headline()));
            }
            // A run with a failure but no lines never got as far as its scenario, so the logs of
            // the two processes it was starting are the only place the reason can be.
            (Some(failure), true) => out.push_str(&format!(
                "**FAILED** — {}\n\nNo scenario line ran: the stage never reached the first one. \
                 See `app.log` and `fleetd.log`.\n\n",
                failure.headline()
            )),
            (None, true) => out.push_str(
                "**Nothing ran.** The stage never reached the first scenario line; \
                 see `app.log` and `fleetd.log`.\n\n",
            ),
            (None, false) => out.push_str(&format!(
                "**Passed** — {} lines in {}.\n\n",
                self.steps.len(),
                duration(total_ms(self.steps))
            )),
        }
        let mut rows = vec![
            (
                "Scenario".to_owned(),
                "[scenario.txt](scenario.txt)".to_owned(),
            ),
            ("Lane".to_owned(), self.lane_cell()),
            ("Fixture".to_owned(), code(&self.fixture())),
            (
                "Lines".to_owned(),
                format!(
                    "{} run, {} failed",
                    self.steps.len(),
                    self.steps.iter().filter(|step| !step.ok).count()
                ),
            ),
            ("Duration".to_owned(), duration(total_ms(self.steps))),
            (
                "Run directory".to_owned(),
                code(&display(&self.run_dir.root)),
            ),
        ];
        if let Some(started) = self.journal.started() {
            rows.insert(4, ("Started".to_owned(), code(started)));
        }
        table(
            out,
            &["", ""],
            rows.iter()
                .map(|(key, value)| vec![format!("**{key}**"), value.clone()]),
        );
    }

    /// The requested lane, and the lane the run actually got when it could not have the first.
    fn lane_cell(&self) -> String {
        let Some(fallback) = self.journal.of_kind("lane-fallback").next() else {
            return code(self.lane.as_str());
        };
        let effective = fallback
            .event("effective")
            .map_or_else(|| "unknown".to_owned(), scalar);
        let reason = fallback
            .event("reason")
            .map_or_else(String::new, |value| format!(" — {}", scalar(value)));
        format!(
            "{} (requested {}){reason}",
            code(&effective),
            code(self.lane.as_str())
        )
    }

    /// The preset that seeded `FLEET_HOME`. A scenario that names none gets `empty`.
    fn fixture(&self) -> String {
        self.journal
            .of_kind("fixture")
            .next()
            .and_then(|entry| entry.event("source").map(scalar))
            .and_then(|source| {
                source
                    .split_once(':')
                    .map(|(_, preset)| preset.trim().to_owned())
            })
            .unwrap_or_else(|| "empty".to_owned())
    }

    fn steps_section(&self, out: &mut String) {
        if self.steps.is_empty() {
            return;
        }
        out.push_str("## Lines\n\n");
        table(
            out,
            &["line", "command", "result", "took", "what happened"],
            self.steps.iter().zip(&self.entries).map(|(step, entry)| {
                vec![
                    step.line.to_string(),
                    code(&cell(&step.source)),
                    if step.ok {
                        "ok".to_owned()
                    } else {
                        "**FAILED**".to_owned()
                    },
                    duration(step.duration_ms),
                    cell(&self.detail(step, *entry)),
                ]
            }),
        );
    }

    /// The one useful sentence about a line, taken from what the app answered.
    fn detail(&self, step: &StepReport, entry: Option<&JournalEntry>) -> String {
        if let Some(error) = &step.error {
            return error.clone();
        }
        let Some(entry) = entry else {
            return String::new();
        };
        match entry.command() {
            Some("meta") => meta_detail(entry),
            Some("dump") => entry.answer("summary").map(scalar).unwrap_or_default(),
            Some("shot") => match step.artifact.as_deref() {
                Some(path) => link(&self.run_dir.root, path),
                None => {
                    let prefix = format!("{:03}-", step.line);
                    baseline_notes(self.journal)
                        .into_iter()
                        .find(|(name, _)| name.starts_with(&prefix))
                        .map(|(_, note)| note.summary)
                        .unwrap_or_default()
                }
            },
            Some("key") => key_detail(entry),
            Some("assert" | "await") => {
                if entry.answer("satisfied").and_then(Value::as_bool) == Some(true) {
                    "held".to_owned()
                } else {
                    unmet_clauses(entry).join("; ")
                }
            }
            Some("clipboard_get") => entry
                .answer("text")
                .map(|text| format!("clipboard {}", code(&scalar(text))))
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn shots_section(&self, out: &mut String) {
        let baselines = baseline_notes(self.journal);
        let shots: Vec<_> = self
            .steps
            .iter()
            .filter(|step| is_png(step.artifact.as_deref()))
            .collect();
        if shots.is_empty() {
            return;
        }
        out.push_str("## Screenshots\n\n");
        for step in shots {
            let Some(path) = step.artifact.as_deref() else {
                continue;
            };
            let relative = display_relative(&self.run_dir.root, path);
            out.push_str(&format!(
                "**line {}** — `{}`\n\n![{relative}]({relative})\n\n",
                step.line,
                cell(&step.source)
            ));
            if let Some(note) = baselines.get(file_name(path)) {
                render_baseline(out, &self.run_dir.root, note);
            }
        }
    }

    fn assertions_section(&self, out: &mut String) {
        let assertions: Vec<_> = self
            .steps
            .iter()
            .zip(&self.entries)
            .filter(|(_, entry)| {
                entry.is_some_and(|entry| matches!(entry.command(), Some("assert" | "await")))
            })
            .collect();
        if assertions.is_empty() {
            return;
        }
        out.push_str("## Assertions\n\n");
        table(
            out,
            &["line", "predicate", "outcome", "took"],
            assertions.into_iter().map(|(step, entry)| {
                let entry = entry.as_ref();
                vec![
                    step.line.to_string(),
                    code(&cell(predicate(entry.copied()).unwrap_or(&step.source))),
                    cell(&verdict(step, entry.copied())),
                    duration(step.duration_ms),
                ]
            }),
        );
    }

    fn dumps_section(&self, out: &mut String) {
        let dumps: Vec<_> = self
            .steps
            .iter()
            .zip(&self.entries)
            .filter(|(step, _)| is_json(step.artifact.as_deref()))
            .collect();
        if dumps.is_empty() {
            return;
        }
        out.push_str("## Dumps\n\n");
        table(
            out,
            &["line", "dump", "summary"],
            dumps.into_iter().map(|(step, entry)| {
                vec![
                    step.line.to_string(),
                    step.artifact
                        .as_deref()
                        .map(|path| link(&self.run_dir.root, path))
                        .unwrap_or_default(),
                    cell(
                        &entry
                            .and_then(|entry| entry.answer("summary"))
                            .map(scalar)
                            .unwrap_or_default(),
                    ),
                ]
            }),
        );
    }

    fn artifacts_section(&self, out: &mut String) {
        out.push_str(
            "## Everything else\n\n\
             The complete record is [run.jsonl](run.jsonl), one JSON object per exchange. \
             Fleet's output is [app.log](app.log) and the private daemon's is \
             [fleetd.log](fleetd.log).\n",
        );
        if !self.failures.is_empty() {
            out.push_str(
                "\nThe hermetic `home/` is kept for a failed run, whatever `--keep` said.\n",
            );
        }
        if let Some(problem) = &self.journal.problem {
            out.push_str(&format!("\n> This report is incomplete: {problem}.\n"));
        }
    }

    /// Says on stdout what a failed run needs said, so nobody has to open a file to diagnose it.
    fn echo(&self, path: &Path) {
        for failure in &self.failures {
            println!("{}", failure.stdout_block());
        }
        println!("report: {}", display(path));
    }
}

impl Failure {
    /// The one line that names the failure.
    fn headline(&self) -> String {
        match self.line {
            Some(line) => format!("line {line}: `{}`", cell(&self.source)),
            None => cell(&self.source),
        }
    }

    /// The stdout form: the failure, the value that was actually there, and the evidence paths.
    fn stdout_block(&self) -> String {
        let mut block = match self.line {
            Some(line) => format!("harness failure at line {line}: {}", self.source),
            None => format!("harness failure: {}", self.source),
        };
        block.push_str(&format!("\n  error: {}", self.error));
        for actual in &self.actuals {
            block.push_str(&format!("\n  actual: {}", plain(actual)));
        }
        if !self.busy.is_empty() {
            let busy: Vec<_> = self.busy.iter().map(|counter| plain(counter)).collect();
            block.push_str(&format!("\n  still busy: {}", busy.join(", ")));
        }
        if let Some(state) = &self.state {
            block.push_str(&format!("\n  state: {state}"));
        }
        if let Some(shot) = &self.shot {
            block.push_str(&format!("\n  shot: {}", display(shot)));
        }
        if let Some(dump) = &self.dump {
            block.push_str(&format!("\n  dump: {}", display(dump)));
        }
        block
    }
}

/// Renders every failure, with its evidence, above everything else in the report.
fn failure_section(out: &mut String, root: &Path, failures: &[Failure]) {
    if failures.is_empty() {
        return;
    }
    out.push_str(if failures.len() == 1 {
        "## Failure\n\n"
    } else {
        "## Failures\n\n"
    });
    for failure in failures {
        out.push_str(&format!("### {}\n\n", failure.headline()));
        out.push_str(&format!("- error: {}\n", failure.error));
        for actual in &failure.actuals {
            out.push_str(&format!("- {actual}\n"));
        }
        if !failure.busy.is_empty() {
            out.push_str(&format!("- still busy: {}\n", failure.busy.join(", ")));
        }
        if let Some(state) = &failure.state {
            out.push_str(&format!("- state at the failure: `{}`\n", cell(state)));
        }
        for (label, path) in [("screenshot", &failure.shot), ("dump", &failure.dump)] {
            if let Some(path) = path {
                out.push_str(&format!("- {label}: {}\n", link(root, path)));
            }
        }
        out.push('\n');
        if let Some(shot) = &failure.shot {
            let relative = display_relative(root, shot);
            out.push_str(&format!("![{relative}]({relative})\n\n"));
        }
    }
}

// ---------------------------------------------------------------- the suite report

struct SuiteReport<'a> {
    suite: &'a Path,
    lane: Lane,
    planned: usize,
    scenarios: Vec<SuiteScenario<'a>>,
}

/// One scenario of a suite, with what its own run directory still holds.
struct SuiteScenario<'a> {
    outcome: &'a ScenarioOutcome,
    journal: Journal,
    shots: Vec<PathBuf>,
    shots_problem: Option<String>,
}

impl<'a> SuiteScenario<'a> {
    async fn read(outcome: &'a ScenarioOutcome) -> Self {
        let shots_directory = outcome.run_dir.join(SHOTS);
        let (shots, shots_problem) = match pngs(&shots_directory).await {
            Ok(shots) => (shots, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        Self {
            journal: Journal::read(&outcome.run_dir.join(JOURNAL)).await,
            shots,
            shots_problem,
            outcome,
        }
    }

    fn name(&self) -> String {
        display(&self.outcome.scenario)
    }

    fn report(&self, suite: &Path) -> String {
        link(suite, &self.outcome.run_dir.join(REPORT))
    }
}

impl SuiteReport<'_> {
    fn render(&self) -> String {
        let mut out = String::new();
        self.header(&mut out);
        // A red suite answers "which one, and why?" before it answers anything else.
        self.failures_section(&mut out);
        self.scenarios_section(&mut out);
        self.shots_section(&mut out);
        self.assertions_section(&mut out);
        out.push_str(
            "\nEach scenario's own `report.md` has its lines, its dumps and its screenshots; \
             its `run.jsonl` has every exchange.\n",
        );
        out
    }

    fn failed(&self) -> impl Iterator<Item = &SuiteScenario<'_>> {
        self.scenarios
            .iter()
            .filter(|scenario| !scenario.outcome.ok)
    }

    fn header(&self, out: &mut String) {
        let name = self.suite.file_name().map_or_else(
            || display(self.suite),
            |name| name.to_string_lossy().into_owned(),
        );
        out.push_str(&format!("# Fleet harness suite — {name}\n\n"));
        let failed = self.failed().count();
        if failed == 0 {
            out.push_str(&format!(
                "**Passed** — {} scenarios in {}.\n\n",
                self.scenarios.len(),
                duration(self.total_ms())
            ));
        } else {
            out.push_str(&format!(
                "**FAILED** — {failed} of {} scenarios that ran.\n\n",
                self.scenarios.len()
            ));
        }
        let mut rows = vec![
            ("Lane".to_owned(), code(self.lane.as_str())),
            (
                "Scenarios".to_owned(),
                format!("{} of {} ran", self.scenarios.len(), self.planned),
            ),
            ("Duration".to_owned(), duration(self.total_ms())),
            ("Suite directory".to_owned(), code(&display(self.suite))),
        ];
        if self.planned > self.scenarios.len() {
            rows.insert(
                2,
                (
                    "Not reached".to_owned(),
                    format!(
                        "{} scenarios, after the suite stopped at its first failure",
                        self.planned - self.scenarios.len()
                    ),
                ),
            );
        }
        table(
            out,
            &["", ""],
            rows.iter()
                .map(|(key, value)| vec![format!("**{key}**"), value.clone()]),
        );
    }

    fn total_ms(&self) -> u64 {
        self.scenarios.iter().fold(0_u64, |total, scenario| {
            total.saturating_add(scenario.outcome.duration_ms)
        })
    }

    fn failures_section(&self, out: &mut String) {
        if self.failed().next().is_none() {
            return;
        }
        out.push_str("## Failures\n\n");
        for scenario in self.failed() {
            out.push_str(&format!("### {}\n\n", scenario.name()));
            if let Some(error) = &scenario.outcome.error {
                out.push_str(&format!("```text\n{}\n```\n\n", error.trim()));
            }
            out.push_str(&format!("- report: {}\n", scenario.report(self.suite)));
            if let Some(problem) = &scenario.shots_problem {
                out.push_str(&format!(
                    "- screenshots could not be listed: {}\n",
                    cell(problem)
                ));
            }
            for evidence in scenario
                .shots
                .iter()
                .filter(|shot| is_failure_evidence(shot) && !is_diff(shot))
            {
                out.push_str(&format!("- screenshot: {}\n", link(self.suite, evidence)));
            }
            out.push('\n');
            for evidence in scenario
                .shots
                .iter()
                .filter(|shot| is_failure_evidence(shot) && !is_diff(shot))
            {
                let relative = display_relative(self.suite, evidence);
                out.push_str(&format!("![{relative}]({relative})\n\n"));
            }
        }
    }

    fn scenarios_section(&self, out: &mut String) {
        if self.scenarios.is_empty() {
            return;
        }
        out.push_str("## Scenarios\n\n");
        table(
            out,
            &["#", "scenario", "result", "lines", "took", "report"],
            self.scenarios.iter().enumerate().map(|(index, scenario)| {
                vec![
                    (index + 1).to_string(),
                    code(&cell(&scenario.name())),
                    if scenario.outcome.ok {
                        "passed".to_owned()
                    } else {
                        "**FAILED**".to_owned()
                    },
                    scenario
                        .journal
                        .entries
                        .iter()
                        .filter(|entry| EXCHANGES.contains(&entry.kind.as_str()))
                        .count()
                        .to_string(),
                    duration(scenario.outcome.duration_ms),
                    scenario.report(self.suite),
                ]
            }),
        );
    }

    fn shots_section(&self, out: &mut String) {
        if self.scenarios.iter().all(|scenario| {
            scenario
                .shots
                .iter()
                .all(|shot| is_failure_evidence(shot) || is_diff(shot))
        }) {
            return;
        }
        out.push_str("## Screenshots\n\n");
        for scenario in &self.scenarios {
            if scenario
                .shots
                .iter()
                .all(|shot| is_failure_evidence(shot) || is_diff(shot))
            {
                continue;
            }
            out.push_str(&format!("**{}**\n\n", scenario.name()));
            let baselines = baseline_notes(&scenario.journal);
            for shot in &scenario.shots {
                // Failure evidence is already above; diffs belong beside their source screenshot.
                if is_failure_evidence(shot) || is_diff(shot) {
                    continue;
                }
                let relative = display_relative(self.suite, shot);
                out.push_str(&format!("![{relative}]({relative})\n\n"));
                if let Some(note) = baselines.get(file_name(shot)) {
                    render_baseline(out, self.suite, note);
                }
            }
        }
    }

    fn assertions_section(&self, out: &mut String) {
        let rows: Vec<_> = self
            .scenarios
            .iter()
            .flat_map(|scenario| {
                scenario
                    .journal
                    .entries
                    .iter()
                    .filter(|entry| matches!(entry.command(), Some("assert" | "await")))
                    .map(move |entry| {
                        let held = entry.answer("satisfied").and_then(Value::as_bool) == Some(true);
                        vec![
                            code(&cell(&scenario.name())),
                            code(&cell(predicate(Some(entry)).unwrap_or("the predicate"))),
                            if held {
                                "held".to_owned()
                            } else {
                                cell(&format!(
                                    "**did not hold**: {}",
                                    unmet_clauses(entry).join("; ")
                                ))
                            },
                        ]
                    })
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        out.push_str("## Assertion timeline\n\n");
        table(out, &["scenario", "predicate", "outcome"], rows.into_iter());
    }

    fn echo(&self, path: &Path) {
        for scenario in self.failed() {
            println!("harness failure in {}", scenario.name());
            if let Some(error) = &scenario.outcome.error {
                for line in error.lines() {
                    println!("  {line}");
                }
            }
            println!(
                "  report: {}",
                display(&scenario.outcome.run_dir.join(REPORT))
            );
        }
        println!("suite report: {}", display(path));
    }
}

// ---------------------------------------------------------------- small shared helpers

/// The predicate a request carried, for an `assert` or an `await`.
fn predicate(entry: Option<&JournalEntry>) -> Option<&str> {
    entry?.request.as_ref()?.args.get("predicate")?.as_str()
}

/// How an assertion turned out, as one cell.
fn verdict(step: &StepReport, entry: Option<&JournalEntry>) -> String {
    if let Some(entry) = entry
        && entry.answer("satisfied").and_then(Value::as_bool) == Some(true)
    {
        return "held".to_owned();
    }
    let reasons = entry.map(unmet_clauses).unwrap_or_default();
    let detail = if reasons.is_empty() {
        step.error.clone().unwrap_or_default()
    } else {
        reasons.join("; ")
    };
    format!("**did not hold**: {detail}")
}

/// A `meta` answer, as one cell.
fn meta_detail(entry: &JournalEntry) -> String {
    let lane = entry.answer("lane").map(scalar).unwrap_or_default();
    let bounds = entry.answer("bounds");
    let size = bounds
        .and_then(|bounds| Some((bounds.get("w")?, bounds.get("h")?)))
        .map(|(width, height)| format!("{}x{}", scalar(width), scalar(height)))
        .unwrap_or_default();
    let frame = entry.answer("frame").map(scalar).unwrap_or_default();
    format!("lane {lane}, {size} logical, frame {frame}")
}

/// A `key` answer, as one cell: which keystrokes the app said it handled.
fn key_detail(entry: &JournalEntry) -> String {
    let Some(request) = entry.request.as_ref() else {
        return String::new();
    };
    let keys = request
        .args
        .get("keys")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().map(scalar).collect::<Vec<_>>())
        .unwrap_or_default();
    let handled = entry
        .answer("handled")
        .and_then(Value::as_array)
        .map(|handled| {
            handled
                .iter()
                .map(|value| value.as_bool() == Some(true))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    keys.iter()
        .enumerate()
        .map(|(index, key)| match handled.get(index) {
            Some(true) => format!("{key} handled"),
            Some(false) => format!("{key} unhandled"),
            None => key.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// One shot's baseline comparison, as the report puts it under the shot.
#[derive(Debug, Clone)]
struct BaselineNote {
    summary: String,
    /// The diff image, written only when the comparison blew its budget.
    diff: Option<PathBuf>,
}

/// Reads a `baseline` journal event.
///
/// `baseline.rs` writes its own sentence into the event, and that sentence is the one the report
/// shows: the comparison's owner says what a comparison means, and this file only places it.
fn baseline_note(entry: &JournalEntry) -> BaselineNote {
    let passed = entry.event("passed").and_then(Value::as_bool) == Some(true);
    let summary = entry.event("summary").map(scalar).unwrap_or_else(|| {
        let differing = entry
            .event("differing_pixels")
            .map(scalar)
            .unwrap_or_default();
        let total = entry.event("total_pixels").map(scalar).unwrap_or_default();
        format!("baseline: {differing} of {total} pixels differ")
    });
    BaselineNote {
        summary: if passed {
            summary
        } else {
            format!("**{summary}**")
        },
        diff: entry
            .event("diff")
            .filter(|value| !value.is_null())
            .map(|value| PathBuf::from(scalar(value))),
    }
}

/// Writes a baseline comparison under its shot, with the diff image inlined when there is one: a
/// visual regression is not diagnosable from a pixel count.
fn render_baseline(out: &mut String, root: &Path, note: &BaselineNote) {
    out.push_str(&format!("{}\n\n", note.summary));
    if let Some(diff) = &note.diff {
        let relative = display_relative(root, diff);
        out.push_str(&format!("**Diff:**\n\n![{relative}]({relative})\n\n"));
    }
}

/// Every baseline comparison a journal recorded, keyed by the shot it was made against.
fn baseline_notes(journal: &Journal) -> std::collections::BTreeMap<String, BaselineNote> {
    journal
        .of_kind("baseline")
        .filter_map(|entry| {
            let shot = entry.event("shot").map(scalar)?;
            Some((file_name(Path::new(&shot)).to_owned(), baseline_note(entry)))
        })
        .collect()
}

/// Writes a GitHub-flavoured table. An empty body writes nothing at all.
fn table(out: &mut String, headers: &[&str], rows: impl Iterator<Item = Vec<String>>) {
    let rows: Vec<_> = rows.collect();
    if rows.is_empty() {
        return;
    }
    out.push_str(&format!("| {} |\n", headers.join(" | ")));
    out.push_str(&format!(
        "| {} |\n",
        headers
            .iter()
            .map(|_| "---")
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    for row in rows {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out.push('\n');
}

/// Sums the line durations of a run without ever wrapping.
fn total_ms(steps: &[StepReport]) -> u64 {
    steps
        .iter()
        .fold(0_u64, |total, step| total.saturating_add(step.duration_ms))
}

/// Milliseconds a human reads at a glance.
fn duration(millis: u64) -> String {
    if millis < 1_000 {
        format!("{millis} ms")
    } else {
        // Integer hundredths, so a duration never picks up a float's rounding surprises.
        format!("{}.{:02} s", millis / 1_000, (millis % 1_000) / 10)
    }
}

/// A JSON scalar as the text a reader wants: a string without its quotes, anything else verbatim.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Strips the backticks a markdown fragment carries, for stdout.
fn plain(text: &str) -> String {
    text.replace('`', "")
}

/// One table cell: no pipes, no newlines, and never longer than the table is wide.
fn cell(text: &str) -> String {
    let flat = text.trim().replace(['\n', '\r'], " ").replace('|', "\\|");
    clip(&flat, CELL)
}

/// An inline code span that cannot break out of itself.
fn code(text: &str) -> String {
    format!("`{}`", text.replace('`', "'"))
}

/// Clips on a character boundary, marking that it did.
fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// A markdown link to a run artifact, relative to the directory the report sits in.
fn link(root: &Path, path: &Path) -> String {
    let relative = display_relative(root, path);
    format!("[{relative}]({relative})")
}

/// A path relative to the directory the report sits in, so the links work wherever it is opened.
fn display_relative(root: &Path, path: &Path) -> String {
    display(path.strip_prefix(root).unwrap_or(path))
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
}

/// Whether a shot is the evidence a failed line left behind, named `failure-NNN.png` by the runner.
fn is_failure_evidence(path: &Path) -> bool {
    file_name(path).starts_with("failure-")
}

/// Whether a shot is a baseline diff, named `<stem>-diff.png` by the comparator.
fn is_diff(path: &Path) -> bool {
    file_name(path).ends_with("-diff.png")
}

fn is_png(path: Option<&Path>) -> bool {
    has_extension(path, "png")
}

fn is_json(path: Option<&Path>) -> bool {
    has_extension(path, "json")
}

fn has_extension(path: Option<&Path>, extension: &str) -> bool {
    path.and_then(Path::extension)
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

/// The path, if it is really there. Evidence that was never captured is not linked.
async fn exists(path: PathBuf) -> Option<PathBuf> {
    tokio::fs::metadata(&path).await.is_ok().then_some(path)
}

/// Every PNG in a directory, in name order, so a report lists shots the way the run took them.
async fn pngs(directory: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut entries = tokio::fs::read_dir(directory)
        .await
        .with_context(|| format!("list {}", display(directory)))?;
    let mut found = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("read the next entry in {}", display(directory)))?
    {
        let path = entry.path();
        if is_png(Some(&path)) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    #[cfg(unix)]
    struct PermissionGuard {
        path: PathBuf,
        mode: u32,
    }

    #[cfg(unix)]
    impl PermissionGuard {
        fn make_unreadable(path: &Path) -> Self {
            let mode = std::fs::metadata(path)
                .expect("read the shots directory metadata")
                .permissions()
                .mode();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
                .expect("make the shots directory unreadable");
            Self {
                path: path.to_owned(),
                mode,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for PermissionGuard {
        fn drop(&mut self) {
            if let Err(error) =
                std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode))
            {
                eprintln!(
                    "failed to restore permissions on {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    fn step(line: usize, source: &str, ok: bool) -> StepReport {
        StepReport {
            line,
            source: source.to_owned(),
            ok,
            duration_ms: 12,
            artifact: None,
            error: (!ok).then(|| "screen == \"Workspace\" but it was \"Hub\"".to_owned()),
        }
    }

    fn command(id: u64, cmd: &str, args: Value, data: Value, ok: bool) -> Value {
        json!({
            "at": "2026-09-11T09:12:03.114Z",
            "kind": "command",
            "data": { "line": id + 1 },
            "request": { "id": id, "cmd": cmd, "args": args },
            "response": {
                "id": id,
                "ok": ok,
                "data": data,
                "error": (!ok).then(|| "screen == \"Workspace\" but it was \"Hub\"".to_owned()),
            },
        })
    }

    #[test]
    fn settling_and_link_opening_are_reported_as_sole_idle_blockers() {
        for (counter, value) in [
            ("settling_mutations", json!(1)),
            ("link_opening", json!(true)),
        ] {
            let mut idle = json!({
                "in_flight_requests": 0,
                "settling_mutations": 0,
                "running_jobs": 0,
                "pending_frame": false,
                "live_toast_timers": 0,
                "armed_debounces": 0,
                "link_opening": false,
            });
            idle[counter] = value.clone();
            let entry: JournalEntry = serde_json::from_value(command(
                1,
                "await",
                json!({ "predicate": "idle" }),
                json!({ "idle": idle }),
                false,
            ))
            .expect("decode journal entry");
            assert_eq!(
                busy_counters(&entry),
                vec![format!("`{counter}` = {value}")]
            );
        }
    }

    async fn run_directory(name: &str) -> (tempfile::TempDir, RunDirectory) {
        let temporary = tempfile::tempdir().expect("a temporary directory");
        let root = temporary.path().join(name);
        let run = RunDirectory::create(Some(&root), Path::new("help.scenario"))
            .expect("create the run directory");
        (temporary, run)
    }

    async fn journal(run: &RunDirectory, entries: &[Value]) {
        let body: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
        tokio::fs::write(&run.journal, body)
            .await
            .expect("write the journal");
    }

    #[tokio::test]
    async fn a_passing_run_reads_as_a_pass_with_its_summaries_shots_and_assertions() {
        let (_temporary, run) = run_directory("green").await;
        let shot = run.shots.join("003-help.png");
        tokio::fs::write(&shot, b"png").await.expect("write a shot");
        let dump = run.dumps.join("004-help.json");
        tokio::fs::write(&dump, b"{}").await.expect("write a dump");
        journal(
            &run,
            &[
                json!({ "at": "2026-09-11T09:12:03.000Z", "kind": "fixture",
                        "data": { "line": 1, "source": "fixture: one-repo" } }),
                command(
                    1,
                    "await",
                    json!({ "predicate": "overlay == Help", "timeout_ms": 5000 }),
                    json!({ "predicate": "overlay == Help", "satisfied": true }),
                    true,
                ),
                command(
                    2,
                    "shot",
                    json!({ "name": "help" }),
                    json!({ "name": "help" }),
                    true,
                ),
                command(
                    3,
                    "dump",
                    json!({ "name": "help" }),
                    json!({ "summary": "screen=Hub mode=Normal focus=- selected=- idle=true" }),
                    true,
                ),
            ],
        )
        .await;
        let mut steps = vec![
            step(2, "await overlay == Help", true),
            step(3, "shot help", true),
            step(4, "dump help", true),
        ];
        steps[1].artifact = Some(shot);
        steps[2].artifact = Some(dump);

        let path = write_report(&run, Lane::Virtual, &steps)
            .await
            .expect("write the report");
        let body = tokio::fs::read_to_string(&path)
            .await
            .expect("read it back");

        assert!(body.contains("**Passed** — 3 lines"), "{body}");
        assert!(body.contains("`one-repo`"), "{body}");
        assert!(!body.contains("## Failure"), "{body}");
        assert!(
            body.contains("![shots/003-help.png](shots/003-help.png)"),
            "{body}"
        );
        assert!(body.contains("screen=Hub mode=Normal"), "{body}");
        assert!(body.contains("## Assertions"), "{body}");
        assert!(body.contains("`overlay == Help`"), "{body}");
        assert!(
            body.contains("[dumps/004-help.json](dumps/004-help.json)"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_failing_run_leads_with_the_failure_its_actual_value_and_its_evidence() {
        let (_temporary, run) = run_directory("red").await;
        tokio::fs::write(run.shots.join("failure-003.png"), b"png")
            .await
            .expect("write the failure shot");
        tokio::fs::write(run.dumps.join("failure-003.json"), b"{}")
            .await
            .expect("write the failure dump");
        journal(
            &run,
            &[
                command(1, "await", json!({ "predicate": "idle", "timeout_ms": 5000 }),
                        json!({ "predicate": "idle", "satisfied": true }), true),
                command(
                    2,
                    "assert",
                    json!({ "predicate": "screen == Workspace" }),
                    json!({
                        "predicate": "screen == Workspace",
                        "satisfied": false,
                        "clauses": [{ "clause": "screen == Workspace", "satisfied": false, "actual": "Hub" }],
                        "idle": { "idle": false, "in_flight_requests": 0, "running_jobs": 2,
                                  "pending_frame": false, "live_toast_timers": 0, "armed_debounces": 0 },
                        "snapshot": { "screen": "Hub", "mode": "Normal", "overlay": null,
                                      "focused": "worktrees.row[0]", "key_contexts": ["Hub", "List"] },
                    }),
                    false,
                ),
            ],
        )
        .await;
        let steps = vec![
            step(2, "await idle", true),
            step(3, "assert screen == Workspace", false),
        ];

        let path = write_report(&run, Lane::Headless, &steps)
            .await
            .expect("write the report");
        let body = tokio::fs::read_to_string(&path)
            .await
            .expect("read it back");

        let failure = body.find("## Failure").expect("a failure section");
        let lines = body.find("## Lines").expect("a lines section");
        assert!(failure < lines, "the failure must come first:\n{body}");
        assert!(body.contains("**FAILED** — line 3:"), "{body}");
        assert!(
            body.contains("`screen == Workspace` — actually `Hub`"),
            "{body}"
        );
        assert!(body.contains("still busy: `running_jobs` = 2"), "{body}");
        assert!(body.contains("screen=Hub mode=Normal"), "{body}");
        assert!(body.contains("keys=Hub > List"), "{body}");
        assert!(
            body.contains("[shots/failure-003.png](shots/failure-003.png)")
                && body.contains("[dumps/failure-003.json](dumps/failure-003.json)"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_shot_that_blew_its_baseline_budget_shows_the_sentence_and_the_diff() {
        let (_temporary, run) = run_directory("baseline").await;
        let shot = run.shots.join("002-hub.png");
        tokio::fs::write(&shot, b"png").await.expect("write a shot");
        journal(
            &run,
            &[
                command(
                    1,
                    "shot",
                    json!({ "name": "hub" }),
                    json!({ "name": "hub" }),
                    true,
                ),
                json!({ "at": "2026-09-11T09:12:03.000Z", "kind": "baseline", "data": {
                    "shot": shot, "baseline": "scenarios/baselines/virtual/hub/002-hub.png",
                    "passed": false, "differing_pixels": 41_234, "total_pixels": 1_296_000,
                    "diff": run.shots.join("002-hub-diff.png"), "status": "differed",
                    "summary": "baseline: differs by 41234 of 1296000 pixels, 3.181%",
                } }),
            ],
        )
        .await;
        let mut steps = vec![step(2, "shot hub", true)];
        steps[0].artifact = Some(shot);

        let path = write_report(&run, Lane::Virtual, &steps)
            .await
            .expect("write the report");
        let body = tokio::fs::read_to_string(&path)
            .await
            .expect("read it back");

        assert!(body.contains("**baseline: differs by 41234"), "{body}");
        assert!(
            body.contains("**Diff:**\n\n![shots/002-hub-diff.png](shots/002-hub-diff.png)"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_not_recorded_shot_reports_its_verdict_without_a_png() {
        let (_temporary, run) = run_directory("not-recorded").await;
        journal(
            &run,
            &[
                command(
                    13,
                    "shot",
                    json!({ "name": "help" }),
                    json!({ "name": "help" }),
                    true,
                ),
                json!({ "at": "2026-09-11T09:12:03.000Z", "kind": "baseline", "data": {
                    "shot": run.shots.join("014-help.png"), "baseline": null,
                    "passed": true, "differing_pixels": null, "total_pixels": null,
                    "diff": null, "status": "not-recorded",
                    "summary": "not recorded (lane: headless)",
                } }),
            ],
        )
        .await;

        let path = write_report(&run, Lane::Headless, &[step(14, "shot help", true)])
            .await
            .expect("write the report");
        let body = tokio::fs::read_to_string(&path)
            .await
            .expect("read it back");

        assert!(body.contains("not recorded (lane: headless)"), "{body}");
        assert!(!body.contains("## Screenshots"), "{body}");
    }

    #[tokio::test]
    async fn a_suite_lists_a_baseline_diff_once_and_labels_it_beside_its_screenshot() {
        let temporary = tempfile::tempdir().expect("a temporary directory");
        let suite = temporary.path().join("suite");
        tokio::fs::create_dir(&suite)
            .await
            .expect("create the suite directory");
        let directory = suite.join("001-hub");
        let run = RunDirectory::create(Some(&directory), Path::new("hub.scenario"))
            .expect("create the run directory");
        let shot = run.shots.join("002-hub.png");
        let diff = run.shots.join("002-hub-diff.png");
        tokio::fs::write(&shot, b"png")
            .await
            .expect("write the screenshot");
        tokio::fs::write(&diff, b"png")
            .await
            .expect("write the baseline diff");
        journal(
            &run,
            &[
                command(
                    1,
                    "shot",
                    json!({ "name": "hub" }),
                    json!({ "name": "hub" }),
                    true,
                ),
                json!({ "at": "2026-09-11T09:12:03.000Z", "kind": "baseline", "data": {
                    "shot": shot, "baseline": "scenarios/baselines/virtual/hub/002-hub.png",
                    "passed": false, "differing_pixels": 12, "total_pixels": 100,
                    "diff": diff, "status": "differed", "summary": "baseline differs",
                } }),
            ],
        )
        .await;
        let outcomes = [ScenarioOutcome {
            scenario: PathBuf::from("scenarios/hub/hub.scenario"),
            run_dir: directory,
            ok: true,
            duration_ms: 100,
            error: None,
        }];

        let path = write_suite_report(&suite, Lane::Virtual, &outcomes, 1)
            .await
            .expect("write the suite report");
        let body = tokio::fs::read_to_string(path)
            .await
            .expect("read the suite report");
        let shot_markup = "![001-hub/shots/002-hub.png](001-hub/shots/002-hub.png)";
        let diff_markup = "![001-hub/shots/002-hub-diff.png](001-hub/shots/002-hub-diff.png)";
        let shot_at = body.find(shot_markup).expect("the screenshot is listed");
        let label_at = body.find("**Diff:**").expect("the diff is labelled");
        let diff_at = body.find(diff_markup).expect("the labelled diff is listed");

        assert!(shot_at < label_at && label_at < diff_at, "{body}");
        assert_eq!(body.matches(diff_markup).count(), 1, "{body}");
    }

    #[tokio::test]
    async fn a_run_that_never_reached_its_first_line_says_so() {
        let (_temporary, run) = run_directory("stillborn").await;
        journal(
            &run,
            &[json!({ "at": "2026-09-11T09:12:03.000Z", "kind": "failed",
                      "data": { "error": "start the private fleetd: no such file" } })],
        )
        .await;

        let path = write_report(&run, Lane::Headless, &[])
            .await
            .expect("write the report");
        let body = tokio::fs::read_to_string(&path)
            .await
            .expect("read it back");

        assert!(body.contains("**FAILED**"), "{body}");
        assert!(
            body.contains("start the private fleetd: no such file"),
            "{body}"
        );
        assert!(body.contains("`app.log`"), "{body}");
    }

    #[tokio::test]
    async fn a_suite_report_leads_with_the_failed_scenario_and_counts_the_ones_never_reached() {
        let temporary = tempfile::tempdir().expect("a temporary directory");
        let suite = temporary.path().join("suite");
        tokio::fs::create_dir(&suite)
            .await
            .expect("create the suite directory");
        let mut outcomes = Vec::new();
        for (index, (stem, ok)) in [("hub-help", true), ("hub-palette", false)]
            .iter()
            .enumerate()
        {
            let directory = suite.join(format!("{:03}-{stem}", index + 1));
            let run = RunDirectory::create(Some(&directory), Path::new("x.scenario"))
                .expect("create the run directory");
            journal(
                &run,
                &[command(1, "assert", json!({ "predicate": "screen == Hub" }),
                          json!({ "predicate": "screen == Hub", "satisfied": *ok,
                                  "clauses": [{ "clause": "screen == Hub", "satisfied": *ok, "actual": "Workspace" }] }),
                          *ok)],
            )
            .await;
            let shot = if *ok {
                "001-help.png"
            } else {
                "failure-002.png"
            };
            tokio::fs::write(run.shots.join(shot), b"png")
                .await
                .expect("write a shot");
            outcomes.push(ScenarioOutcome {
                scenario: PathBuf::from(format!("scenarios/hub/{stem}.scenario")),
                run_dir: directory,
                ok: *ok,
                duration_ms: 1_500,
                error: (!*ok)
                    .then(|| "line 2: assert screen == Hub\n  error: it was Workspace".to_owned()),
            });
        }

        let path = write_suite_report(&suite, Lane::Virtual, &outcomes, 4)
            .await
            .expect("write the suite report");
        let body = tokio::fs::read_to_string(&path)
            .await
            .expect("read it back");

        let failures = body.find("## Failures").expect("a failures section");
        let scenarios = body.find("## Scenarios").expect("a scenarios section");
        assert!(failures < scenarios, "the failure must come first:\n{body}");
        assert!(
            body.contains("**FAILED** — 1 of 2 scenarios that ran."),
            "{body}"
        );
        assert!(
            body.contains("2 scenarios, after the suite stopped"),
            "{body}"
        );
        assert!(
            body.contains("scenarios/hub/hub-palette.scenario"),
            "{body}"
        );
        assert!(body.contains("002-hub-palette/report.md"), "{body}");
        assert!(
            body.contains("![002-hub-palette/shots/failure-002.png]"),
            "{body}"
        );
        assert!(body.contains("## Assertion timeline"), "{body}");
        assert!(
            body.contains("**did not hold**: `screen == Hub` — actually `Workspace`"),
            "{body}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_shots_directory_is_reported_in_the_suite_failure() {
        let temporary = tempfile::tempdir().expect("a temporary directory");
        let suite = temporary.path().join("suite");
        tokio::fs::create_dir(&suite)
            .await
            .expect("create the suite directory");
        let directory = suite.join("001-red");
        let run = RunDirectory::create(Some(&directory), Path::new("red.scenario"))
            .expect("create the run directory");
        let permission_guard = PermissionGuard::make_unreadable(&run.shots);
        let outcomes = [ScenarioOutcome {
            scenario: PathBuf::from("scenarios/hub/red.scenario"),
            run_dir: directory,
            ok: false,
            duration_ms: 100,
            error: Some("line 2 failed".to_owned()),
        }];

        let path = write_suite_report(&suite, Lane::Headless, &outcomes, 1)
            .await
            .expect("write the suite report");
        let body = tokio::fs::read_to_string(path)
            .await
            .expect("read the suite report");

        assert!(body.contains("screenshots could not be listed"), "{body}");
        drop(permission_guard);
    }

    #[tokio::test]
    async fn a_journal_that_drifts_from_the_steps_stops_enriching_rather_than_inventing() {
        let (_temporary, run) = run_directory("drift").await;
        journal(
            &run,
            &[json!({ "at": "2026-09-11T09:12:03.000Z", "kind": "runner",
                      "data": { "line": 99, "source": "daemon kill" } })],
        )
        .await;
        let steps = vec![step(2, "daemon kill", true), step(3, "dump after", true)];

        let path = write_report(&run, Lane::Headless, &steps)
            .await
            .expect("write the report");
        let body = tokio::fs::read_to_string(&path)
            .await
            .expect("read it back");

        // The lines are still reported; nothing from the mismatched journal is attributed to them.
        assert!(body.contains("daemon kill"), "{body}");
        assert!(!body.contains("## Dumps"), "{body}");
        assert!(!body.contains("## Assertions"), "{body}");
    }

    #[test]
    fn command_entries_align_only_when_their_data_names_the_step_line() {
        let steps = [step(2, "assert screen == Hub", true)];
        let with_line = serde_json::from_value::<JournalEntry>(command(
            1,
            "assert",
            json!({ "predicate": "screen == Hub" }),
            json!({ "predicate": "screen == Hub", "satisfied": true }),
            true,
        ))
        .expect("deserialize a command entry with its line");
        let without_line = serde_json::from_value::<JournalEntry>(json!({
            "at": "2026-09-11T09:12:03.114Z",
            "kind": "command",
            "request": {
                "id": 1,
                "cmd": "assert",
                "args": { "predicate": "screen == Hub" },
            },
            "response": {
                "id": 1,
                "ok": true,
                "data": { "predicate": "screen == Hub", "satisfied": true },
            },
        }))
        .expect("deserialize a command entry without its line");
        let trustworthy = Journal {
            entries: vec![with_line],
            problem: None,
        };
        let untrustworthy = Journal {
            entries: vec![without_line],
            problem: None,
        };

        assert!(align(&steps, &trustworthy)[0].is_some());
        assert!(align(&steps, &untrustworthy)[0].is_none());
    }

    #[test]
    fn table_cells_cannot_break_the_table_or_run_off_its_edge() {
        assert_eq!(cell("a | b\nc"), "a \\| b c");
        assert_eq!(code("back`tick"), "`back'tick`");
        assert_eq!(clip("abcdef", 3), "ab…");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(duration(999), "999 ms");
        assert_eq!(duration(3_420), "3.42 s");
    }
}
