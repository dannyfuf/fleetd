use std::{collections::HashMap, ffi::OsString, path::Path, sync::Arc};

use async_trait::async_trait;
use fleet_core::{
    agents::{AgentKind, PermissionMode},
    ids::{BoardId, ScheduleId},
    paths::FleetHome,
    schedule::{Cadence, Schedule, ScheduleAgent, ScheduleOutcome, ScheduleRun},
};
use tokio_util::sync::CancellationToken;

use super::{
    CANCELED_SUMMARY, EnvironmentSource, HeadlessRunner, ScheduleRunner, claude_argv, codex_argv,
    lock,
};
use crate::{
    DaemonError, DaemonResult,
    adapters::{
        files::RealFiles,
        shell::{DetachedProcess, LineCallback, Shell, ShellCommand, ShellResult},
    },
    stores::config::ConfigStore,
};

/// How the scripted child ends.
#[derive(Clone, Copy)]
enum Exit {
    Status(i32),
    Timeout,
    /// Waits for the run's cancellation token, then reports the cancellation.
    WaitForCancel,
}

/// A shell that prints fixed lines, optionally writes Codex's `-o` file, and ends as told.
struct ScriptedShell {
    lines: Vec<String>,
    last_message: Option<String>,
    exit: Exit,
    commands: std::sync::Mutex<Vec<ShellCommand>>,
}

impl ScriptedShell {
    fn new(lines: &[&str], exit: Exit) -> Self {
        Self {
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            last_message: None,
            exit,
            commands: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn with_last_message(mut self, text: &str) -> Self {
        self.last_message = Some(text.to_owned());
        self
    }

    fn only_command(&self) -> ShellCommand {
        let commands = lock(&self.commands);
        assert_eq!(commands.len(), 1, "exactly one launch");
        commands[0].clone()
    }
}

#[async_trait]
impl Shell for ScriptedShell {
    async fn run(&self, _command: ShellCommand) -> DaemonResult<ShellResult> {
        Err(DaemonError::Shell("the runner only streams".to_owned()))
    }

    async fn run_detached(
        &self,
        _command: ShellCommand,
        _log_path: &Path,
    ) -> DaemonResult<DetachedProcess> {
        Err(DaemonError::Shell("the runner only streams".to_owned()))
    }

    async fn run_streaming(
        &self,
        command: ShellCommand,
        cancel: CancellationToken,
        on_line: LineCallback,
    ) -> DaemonResult<ShellResult> {
        if let (Some(text), Some(index)) = (
            &self.last_message,
            command.args.iter().position(|argument| argument == "-o"),
        ) {
            let path = &command.args[index + 1];
            tokio::fs::write(path, text)
                .await
                .map_err(|error| DaemonError::fs(path, error))?;
        }
        lock(&self.commands).push(command);
        for line in &self.lines {
            on_line(line.clone());
        }
        match self.exit {
            Exit::Status(status) => Ok(ShellResult {
                status,
                stdout: String::new(),
                stderr: String::new(),
            }),
            Exit::Timeout => Err(DaemonError::Timeout("claude".to_owned())),
            Exit::WaitForCancel => {
                cancel.cancelled().await;
                Err(DaemonError::Cancelled)
            }
        }
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    home: FleetHome,
    shell: Arc<ScriptedShell>,
    runner: HeadlessRunner,
    fleet_dir: std::path::PathBuf,
}

fn fixture(shell: ScriptedShell) -> Fixture {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join(".fleet");
    let home = FleetHome::new(&root);
    let files = Arc::new(RealFiles::new(root.join("trash"), [root.join("schedules")]));
    let config = Arc::new(ConfigStore::new(&root, files));
    let shell = Arc::new(shell);
    let fleet_dir = temp.path().join("fleet-bin");
    let mut runner = HeadlessRunner::new(shell.clone(), config, home.clone());
    runner.fleet_program = Some(fleet_dir.join("fleet"));
    runner.environment = EnvironmentSource::Fixed(HashMap::from([
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
        (OsString::from("HOME"), OsString::from("/home/test")),
        (
            OsString::from("CLAUDE_CODE_SESSION_ID"),
            OsString::from("inherited"),
        ),
        (OsString::from("CODEX_HOME"), OsString::from("/elsewhere")),
    ]));
    Fixture {
        _temp: temp,
        home,
        shell,
        runner,
        fleet_dir,
    }
}

fn agent(
    provider: AgentKind,
    mode: PermissionMode,
    model: Option<&str>,
    effort: Option<&str>,
) -> ScheduleAgent {
    ScheduleAgent {
        provider,
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
        mode,
    }
}

fn schedule(provider: AgentKind) -> Schedule {
    Schedule {
        id: ScheduleId::try_from("sch-0123abcd").expect("schedule id"),
        board_id: BoardId::try_from("reviews-home").expect("board id"),
        name: "Reviews".to_owned(),
        prompt: "Find reviews".to_owned(),
        cadence: Cadence::Every { minutes: 15 },
        agent: agent(provider, PermissionMode::FullAccess, None, None),
        enabled: true,
        timeout_minutes: 20,
        created_at: "2026-09-24T10:00:00Z".to_owned(),
        updated_at: "2026-09-24T10:00:00Z".to_owned(),
        runs: vec![ScheduleRun {
            job_id: None,
            started_at: "2026-09-24T10:15:00Z".to_owned(),
            ended_at: None,
            outcome: None,
            summary: None,
            cost_usd: None,
            log_path: None,
        }],
        next_run_at: None,
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

const RESULT_LINE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"Done.\nSUMMARY: 2 created, 1 existing, 0 reopened","total_cost_usd":0.125}"#;

#[test]
fn claude_argv_covers_every_mode_model_and_effort() {
    let cases = [
        (
            agent(AgentKind::Claude, PermissionMode::FullAccess, None, None),
            strings(&[
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "bypassPermissions",
                "--",
                "PROMPT",
            ]),
        ),
        (
            agent(
                AgentKind::Claude,
                PermissionMode::AcceptEdits,
                Some("sonnet"),
                None,
            ),
            strings(&[
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "acceptEdits",
                "--model",
                "sonnet",
                "--",
                "PROMPT",
            ]),
        ),
        (
            agent(AgentKind::Claude, PermissionMode::Ask, None, Some("high")),
            strings(&[
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "manual",
                "--effort",
                "high",
                "--",
                "PROMPT",
            ]),
        ),
        (
            agent(
                AgentKind::Claude,
                PermissionMode::Plan,
                Some("opus"),
                Some("low"),
            ),
            strings(&[
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "plan",
                "--model",
                "opus",
                "--effort",
                "low",
                "--",
                "PROMPT",
            ]),
        ),
    ];
    for (agent, expected) in cases {
        assert_eq!(claude_argv(&agent, "PROMPT"), expected, "{agent:?}");
    }
}

#[test]
fn codex_argv_covers_every_mode_model_and_effort() {
    let last = Path::new("/tmp/last-message");
    let cases = [
        (
            agent(AgentKind::Codex, PermissionMode::FullAccess, None, None),
            strings(&[
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "-o",
                "/tmp/last-message",
                "--",
                "PROMPT",
            ]),
        ),
        (
            agent(
                AgentKind::Codex,
                PermissionMode::AcceptEdits,
                Some("gpt-5.6-sol"),
                None,
            ),
            strings(&[
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-s",
                "workspace-write",
                "-m",
                "gpt-5.6-sol",
                "-o",
                "/tmp/last-message",
                "--",
                "PROMPT",
            ]),
        ),
        (
            agent(AgentKind::Codex, PermissionMode::Ask, None, Some("high")),
            strings(&[
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-s",
                "read-only",
                "-c",
                "model_reasoning_effort=\"high\"",
                "-o",
                "/tmp/last-message",
                "--",
                "PROMPT",
            ]),
        ),
        (
            agent(
                AgentKind::Codex,
                PermissionMode::Plan,
                Some("gpt-5.6-sol"),
                Some("low"),
            ),
            strings(&[
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-s",
                "read-only",
                "-m",
                "gpt-5.6-sol",
                "-c",
                "model_reasoning_effort=\"low\"",
                "-o",
                "/tmp/last-message",
                "--",
                "PROMPT",
            ]),
        ),
    ];
    for (agent, expected) in cases {
        assert_eq!(codex_argv(&agent, "PROMPT", last), expected, "{agent:?}");
    }
}

#[test]
fn the_prompt_is_the_last_argument_whatever_it_contains() {
    let prompt = "line one\n--model evil \"quoted\" $(rm -rf /) 'x'";
    let every = agent(
        AgentKind::Claude,
        PermissionMode::Ask,
        Some("sonnet"),
        Some("high"),
    );
    assert_eq!(
        claude_argv(&every, prompt).last().map(String::as_str),
        Some(prompt)
    );
    let codex = ScheduleAgent {
        provider: AgentKind::Codex,
        ..every
    };
    assert_eq!(
        codex_argv(&codex, prompt, Path::new("/tmp/last"))
            .last()
            .map(String::as_str),
        Some(prompt)
    );
}

/// A prompt that opens with `-` (a Markdown list) follows `--`, so neither CLI reads it as an
/// option.
#[test]
fn a_prompt_opening_with_a_dash_follows_the_end_of_options() {
    let prompt = "- list my reviews\n- record each one";
    let claude = agent(AgentKind::Claude, PermissionMode::FullAccess, None, None);
    let argv = claude_argv(&claude, prompt);
    assert_eq!(argv[argv.len() - 2..], strings(&["--", prompt]));
    let codex = agent(AgentKind::Codex, PermissionMode::FullAccess, None, None);
    let argv = codex_argv(&codex, prompt, Path::new("/tmp/last"));
    assert_eq!(argv[argv.len() - 2..], strings(&["--", prompt]));
}

/// Prints the launch line for the manual `claude -p` check: run with `--nocapture` and paste.
#[test]
fn print_a_claude_launch_line_for_a_manual_check() {
    let agent = agent(AgentKind::Claude, PermissionMode::FullAccess, None, None);
    let argv = claude_argv(&agent, "Reply with exactly one line: SUMMARY: ok");
    let quoted: Vec<String> = argv
        .iter()
        .map(|argument| shell_words::quote(argument).into_owned())
        .collect();
    println!("claude {}", quoted.join(" "));
}

#[tokio::test]
async fn a_claude_result_line_yields_success_the_summary_and_the_cost() {
    let fixture = fixture(ScriptedShell::new(
        &[
            r#"{"type":"system","subtype":"init"}"#,
            "not json at all",
            RESULT_LINE,
        ],
        Exit::Status(0),
    ));
    let schedule = schedule(AgentKind::Claude);
    let result = fixture
        .runner
        .run(&schedule, "PROMPT", CancellationToken::new())
        .await;
    assert_eq!(result.outcome, ScheduleOutcome::Succeeded);
    assert_eq!(
        result.summary.as_deref(),
        Some("2 created, 1 existing, 0 reopened")
    );
    assert_eq!(result.cost_usd, Some(0.125));

    let command = fixture.shell.only_command();
    assert_eq!(command.program, "claude");
    assert_eq!(command.args.last().map(String::as_str), Some("PROMPT"));
    assert_eq!(
        command.cwd.as_deref(),
        Some(fixture.home.schedule_work_dir(&schedule.id).as_path())
    );
    assert!(fixture.home.schedule_work_dir(&schedule.id).is_dir());
    assert_eq!(
        command.timeout,
        Some(std::time::Duration::from_secs(20 * 60))
    );
}

#[tokio::test]
async fn an_error_result_fails_the_run() {
    let fixture = fixture(ScriptedShell::new(
        &[
            r#"{"type":"result","is_error":true,"result":"API Error: overloaded","total_cost_usd":0.01}"#,
        ],
        Exit::Status(0),
    ));
    let result = fixture
        .runner
        .run(
            &schedule(AgentKind::Claude),
            "PROMPT",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(result.outcome, ScheduleOutcome::Failed);
    assert_eq!(result.summary.as_deref(), Some("API Error: overloaded"));
    assert_eq!(result.cost_usd, Some(0.01));
}

#[tokio::test]
async fn a_non_zero_exit_fails_with_the_last_output_line() {
    let fixture = fixture(ScriptedShell::new(
        &["starting", "fatal: not logged in", ""],
        Exit::Status(1),
    ));
    let result = fixture
        .runner
        .run(
            &schedule(AgentKind::Claude),
            "PROMPT",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(result.outcome, ScheduleOutcome::Failed);
    assert_eq!(result.summary.as_deref(), Some("fatal: not logged in"));
    assert_eq!(result.cost_usd, None);
}

#[tokio::test]
async fn a_timeout_yields_timed_out() {
    let fixture = fixture(ScriptedShell::new(&["working"], Exit::Timeout));
    let result = fixture
        .runner
        .run(
            &schedule(AgentKind::Claude),
            "PROMPT",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(result.outcome, ScheduleOutcome::TimedOut);
}

#[tokio::test]
async fn a_timeout_without_output_says_when_it_stopped_once() {
    let fixture = fixture(ScriptedShell::new(&[], Exit::Timeout));
    let mut schedule = schedule(AgentKind::Claude);
    schedule.timeout_minutes = 37;
    let result = fixture
        .runner
        .run(&schedule, "PROMPT", CancellationToken::new())
        .await;
    assert_eq!(result.outcome, ScheduleOutcome::TimedOut);
    assert_eq!(result.summary.as_deref(), Some("stopped after 37 min"));
    assert!(
        !result
            .summary
            .as_deref()
            .is_some_and(|summary| summary.contains("timed out"))
    );
}

#[tokio::test]
async fn cancelling_yields_failed_and_canceled() {
    let fixture = fixture(ScriptedShell::new(&["working"], Exit::WaitForCancel));
    let schedule = schedule(AgentKind::Claude);
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(
        fixture.runner.run(&schedule, "PROMPT", cancel.clone()),
        async { cancel.cancel() }
    );
    assert_eq!(result.outcome, ScheduleOutcome::Failed);
    assert_eq!(result.summary.as_deref(), Some(CANCELED_SUMMARY));
}

#[tokio::test]
async fn the_log_file_receives_every_line() {
    let lines = [
        r#"{"type":"system","subtype":"init"}"#,
        "plain",
        RESULT_LINE,
    ];
    let fixture = fixture(ScriptedShell::new(&lines, Exit::Status(0)));
    let mut schedule = schedule(AgentKind::Claude);
    let log_path = fixture.home.schedule_dir(&schedule.id).join("chosen.log");
    schedule.runs[0].log_path = Some(log_path.display().to_string());
    fixture
        .runner
        .run(&schedule, "PROMPT", CancellationToken::new())
        .await;
    let written = std::fs::read_to_string(&log_path).expect("log written");
    assert_eq!(written, format!("{}\n", lines.join("\n")));
}

#[tokio::test]
async fn without_a_recorded_log_path_the_run_logs_under_its_start_time() {
    let fixture = fixture(ScriptedShell::new(&["only line"], Exit::Status(1)));
    let schedule = schedule(AgentKind::Claude);
    fixture
        .runner
        .run(&schedule, "PROMPT", CancellationToken::new())
        .await;
    let path = fixture
        .home
        .schedule_log_path(&schedule.id, "2026-09-24T10:15:00Z");
    assert_eq!(
        std::fs::read_to_string(path).expect("log written"),
        "only line\n"
    );
}

#[tokio::test]
async fn the_environment_puts_fleet_first_and_names_the_board_and_schedule() {
    let fixture = fixture(ScriptedShell::new(&[RESULT_LINE], Exit::Status(0)));
    fixture
        .runner
        .run(
            &schedule(AgentKind::Claude),
            "PROMPT",
            CancellationToken::new(),
        )
        .await;
    let command = fixture.shell.only_command();
    let path = command.env.get("PATH").expect("PATH set");
    let first = std::env::split_paths(path).next().expect("a PATH entry");
    assert_eq!(first, fixture.fleet_dir);
    assert!(path.ends_with("/usr/bin:/bin"), "{path}");
    assert_eq!(
        command.env.get("FLEET_BOARD").map(String::as_str),
        Some("reviews-home")
    );
    assert_eq!(
        command.env.get("FLEET_SCHEDULE").map(String::as_str),
        Some("sch-0123abcd")
    );
    assert_eq!(
        command.env.get("HOME").map(String::as_str),
        Some("/home/test")
    );
    assert!(!command.env.contains_key("CLAUDE_CODE_SESSION_ID"));
    assert!(
        command.clear_env,
        "a stripped variable must not come back from the daemon's own environment"
    );
}

#[tokio::test]
async fn codex_reads_its_final_reply_from_the_output_file_and_reports_no_cost() {
    let fixture = fixture(
        ScriptedShell::new(&[r#"{"type":"turn.completed"}"#], Exit::Status(0))
            .with_last_message("Looked at 3.\nSUMMARY: 3 created, 0 existing, 0 reopened\n"),
    );
    let schedule = schedule(AgentKind::Codex);
    let result = fixture
        .runner
        .run(&schedule, "PROMPT", CancellationToken::new())
        .await;
    assert_eq!(result.outcome, ScheduleOutcome::Succeeded);
    assert_eq!(
        result.summary.as_deref(),
        Some("3 created, 0 existing, 0 reopened")
    );
    assert_eq!(result.cost_usd, None);

    let command = fixture.shell.only_command();
    assert_eq!(command.program, "codex");
    assert!(!command.env.contains_key("CODEX_HOME"));
    let output = command
        .args
        .iter()
        .position(|argument| argument == "-o")
        .map(|index| std::path::PathBuf::from(&command.args[index + 1]))
        .expect("an -o file");
    assert!(!output.exists(), "the reply file is removed once read");
}

/// Codex's final reply is read as lossily as its streamed output: one invalid byte keeps the
/// run's `SUMMARY:` line.
#[tokio::test]
async fn a_final_reply_with_invalid_utf8_keeps_its_summary() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("last-message.txt");
    std::fs::write(
        &path,
        b"reviewed \xff things\nSUMMARY: 2 created, 0 existing, 0 reopened\n",
    )
    .expect("the reply writes");

    let text = super::read_last_message(&path)
        .await
        .expect("the reply is read");

    assert_eq!(
        super::summary_line(&text).as_deref(),
        Some("2 created, 0 existing, 0 reopened")
    );
    assert!(!path.exists(), "the reply is removed once read");
}
