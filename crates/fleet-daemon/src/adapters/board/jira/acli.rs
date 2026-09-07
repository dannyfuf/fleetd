//! The only place in the daemon that spawns `acli`.
//!
//! Every Jira call funnels through here so the timeout, the concurrency permit, the retry
//! policy and the failure classification exist once. `acli` reports failures as prose on
//! whichever stream it feels like, so the text is the only signal there is: everything the
//! backend reacts to differently is decided by [`classify_failure`] and nowhere else.

use crate::adapters::shell::{Shell, ShellCommand};
use fleet_core::board::BoardError;
use std::sync::Arc;

/// How long one `acli` invocation may take before it is killed.
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
/// Backoff between the three attempts a transient failure is retried with.
pub const BACKOFF: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(3),
    std::time::Duration::from_secs(9),
];

/// What an unauthenticated CLI is reported as, wherever it is noticed.
///
/// One string, because the CLI, the app and the daemon log all show it verbatim and the only
/// useful part of it is the command that fixes it.
pub const NOT_AUTHENTICATED: &str = "acli is not authenticated (run `acli jira auth login`)";
/// What a missing CLI is reported as.
pub const NOT_INSTALLED: &str =
    "acli not found: install the Atlassian CLI (https://developer.atlassian.com/cloud/acli/)";

/// What `acli jira auth status` reports about the machine's single account.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthStatus {
    /// Whether a session is active.
    pub authenticated: bool,
    /// The site the session is bound to, e.g. `buk.atlassian.net`.
    pub site: Option<String>,
    /// The account's email, when the CLI prints one.
    pub email: Option<String>,
}

/// The classes of `acli` failure the backend reacts to differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcliFailure {
    /// No active session; the user must run `acli jira auth login`.
    NotAuthenticated,
    /// Rate limited or otherwise transient: retry with backoff.
    RateLimited,
    /// The call was killed at [`TIMEOUT`]. Transient like a rate limit, but not the same
    /// thing: four 90 s attempts of one command is six minutes of wall clock spent before
    /// the user is told, in a sentence that names throttling that never happened.
    TimedOut,
    /// The issue or project does not exist (or is no longer visible).
    NotFound,
    /// A transition Jira refused, with its own message.
    Transition(String),
    /// Anything else, with the most useful line of output.
    Other(String),
}

/// One finished invocation that did not succeed, kept classified until the caller decides.
///
/// `pull` treats a missing issue as a deletion rather than an error, so the class has to
/// survive as far as the call site instead of collapsing into a message on the way out.
#[derive(Debug, Clone)]
struct Failed {
    status: i32,
    failure: AcliFailure,
    message: String,
}

impl From<Failed> for BoardError {
    fn from(failed: Failed) -> Self {
        // A CLI that is not installed answers every command the same way, so the install hint
        // outranks whatever the shell had to say about the missing program.
        if failed.status == 127 {
            return Self::Backend(NOT_INSTALLED.to_owned());
        }
        match failed.failure {
            AcliFailure::NotAuthenticated => Self::Backend(NOT_AUTHENTICATED.to_owned()),
            AcliFailure::RateLimited => {
                Self::Backend(format!("acli is rate limited: {}", failed.message))
            }
            AcliFailure::TimedOut => Self::Backend(format!(
                "acli timed out after {}s: {}",
                TIMEOUT.as_secs(),
                failed.message
            )),
            AcliFailure::NotFound => Self::Backend(format!("acli: {}", failed.message)),
            AcliFailure::Transition(message) | AcliFailure::Other(message) => {
                Self::Backend(message)
            }
        }
    }
}

/// Serialized, bounded access to the Atlassian CLI.
pub struct Acli {
    shell: Arc<dyn Shell>,
    permits: Arc<tokio::sync::Semaphore>,
    program: String,
}

impl Acli {
    /// Creates a runner allowing `max_concurrency` invocations at once.
    #[must_use]
    pub fn new(shell: Arc<dyn Shell>, max_concurrency: usize) -> Self {
        Self {
            shell,
            permits: Arc::new(tokio::sync::Semaphore::new(max_concurrency.clamp(1, 8))),
            program: "acli".to_owned(),
        }
    }

    /// Runs `acli jira <args>` and parses stdout as JSON.
    ///
    /// Retries transient and rate-limited failures three times with [`BACKOFF`]; an
    /// unauthenticated CLI becomes [`NOT_AUTHENTICATED`].
    pub async fn json(&self, args: &[&str]) -> Result<serde_json::Value, BoardError> {
        let stdout = self.jira(args).await?;
        parse_json(&stdout)
    }

    /// [`Acli::json`], except that *every* JSON document stdout carries is returned.
    ///
    /// The one caller is the paginated `workitem search` of a pull. `--paginate` is documented
    /// to answer one merged array, and against that this is exactly [`Acli::json`] wrapped in a
    /// one-element vector — but a CLI that answers one document per page instead would have had
    /// every key past page one silently dropped by a parser that stops at the first value, and a
    /// full pull reports a key it never listed as deleted and archives its card.
    pub async fn json_all(&self, args: &[&str]) -> Result<Vec<serde_json::Value>, BoardError> {
        let stdout = self.jira(args).await?;
        parse_json_all(&stdout)
    }

    /// [`Acli::json`], except that an issue Jira no longer has is `Ok(None)`.
    ///
    /// A key can vanish between the search that listed it and the view that reads it; that is
    /// a deletion the pull reports, not a failure that aborts it.
    pub async fn json_optional(
        &self,
        args: &[&str],
    ) -> Result<Option<serde_json::Value>, BoardError> {
        match self.jira_classified(args).await {
            Ok(stdout) => parse_json(&stdout).map(Some),
            Err(failed) if failed.failure == AcliFailure::NotFound => Ok(None),
            Err(failed) => Err(failed.into()),
        }
    }

    /// [`Acli::json`] without the retry, for a call that must not run twice.
    ///
    /// `workitem create` and `comment create` are not idempotent: a request Jira accepted and
    /// then failed to answer (a gateway 504, a killed 90 s call) is classified transient like
    /// any other, and retrying it files a second issue or a second comment. A write that is
    /// lost is a failed push the next sync repeats; a write that is doubled is data the user
    /// has to delete by hand.
    pub async fn json_once(&self, args: &[&str]) -> Result<serde_json::Value, BoardError> {
        let mut full = Vec::with_capacity(args.len() + 1);
        full.push("jira");
        full.extend_from_slice(args);
        let stdout = self.once(&full).await.map_err(BoardError::from)?;
        parse_json(&stdout)
    }

    /// Runs `acli jira <args>` and returns stdout verbatim.
    pub async fn text(&self, args: &[&str]) -> Result<String, BoardError> {
        self.jira(args).await
    }

    /// Parses `acli jira auth status`.
    ///
    /// A CLI that answers "not authenticated" is answering the question, so it is reported as
    /// an unauthenticated status rather than as a failed call.
    pub async fn auth_status(&self) -> Result<AuthStatus, BoardError> {
        let text = match self.jira_classified(&["auth", "status"]).await {
            Ok(text) => text,
            Err(failed) if failed.failure == AcliFailure::NotAuthenticated => {
                return Ok(AuthStatus::default());
            }
            Err(failed) => return Err(failed.into()),
        };
        Ok(parse_auth_status(&text))
    }

    /// Returns the `acli --version` string.
    pub async fn version(&self) -> Result<String, BoardError> {
        let stdout = self.run(&["--version"]).await.map_err(BoardError::from)?;
        Ok(stdout.trim().to_owned())
    }

    async fn jira(&self, args: &[&str]) -> Result<String, BoardError> {
        self.jira_classified(args).await.map_err(BoardError::from)
    }

    async fn jira_classified(&self, args: &[&str]) -> Result<String, Failed> {
        let mut full = Vec::with_capacity(args.len() + 1);
        full.push("jira");
        full.extend_from_slice(args);
        self.run(&full).await
    }

    /// Runs one command with the retry policy, holding a permit for each attempt only.
    async fn run(&self, args: &[&str]) -> Result<String, Failed> {
        let mut attempt = 0;
        loop {
            let failed = match self.once(args).await {
                Ok(stdout) => return Ok(stdout),
                Err(failed) => failed,
            };
            // A timeout has already cost `TIMEOUT` seconds; three more attempts of it spend
            // six minutes before the caller hears anything. One retry covers the call that
            // was merely unlucky, and nothing beyond it.
            let budget = match failed.failure {
                AcliFailure::RateLimited => BACKOFF.len(),
                AcliFailure::TimedOut => 1,
                _ => 0,
            };
            if attempt < budget {
                tracing::warn!(
                    attempt = attempt + 1,
                    message = %failed.message,
                    "acli call was throttled; retrying"
                );
                tokio::time::sleep(BACKOFF[attempt]).await;
                attempt += 1;
                continue;
            }
            return Err(failed);
        }
    }

    async fn once(&self, args: &[&str]) -> Result<String, Failed> {
        let command = ShellCommand::new(self.program.clone())
            .args(args.iter().copied())
            .timeout(TIMEOUT);
        // The permit is held for the invocation and released before the backoff sleep, so a
        // throttled call does not also block the calls that are not throttled.
        let permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| failed_shell("acli runner is shut down"))?;
        let result = self.shell.run(command).await;
        drop(permit);
        let result = result.map_err(|error| failed_shell(&format!("acli: {error}")))?;
        if result.success() {
            return Ok(result.stdout);
        }
        Err(Failed {
            status: result.status,
            failure: classify_failure(result.status, &result.stdout, &result.stderr),
            message: message(&result.stdout, &result.stderr, result.status),
        })
    }
}

fn failed_shell(message: &str) -> Failed {
    // A spawn that failed because there is no such program is the missing CLI — the one case
    // `NOT_INSTALLED` exists for, and one no exit status ever reports, because the exec never
    // happened. Reporting it as transient answers "acli is rate limited" after thirteen
    // seconds of pointless backoff for a machine that simply has nothing to run.
    let lowered = message.to_lowercase();
    if lowered.contains("no such file or directory")
        || lowered.contains("os error 2")
        || lowered.contains("program not found")
    {
        return Failed {
            // 127 is what a shell reports for a command it could not find, and what
            // `From<Failed>` already turns into the install hint.
            status: 127,
            failure: AcliFailure::Other(NOT_INSTALLED.to_owned()),
            message: message.to_owned(),
        };
    }
    // The shell kills a command at `TIMEOUT` and reports it here, not as a non-zero status:
    // the class has to be decided from the message on this path too, or a killed call is
    // retried three more times and finally named a rate limit.
    if lowered.contains("timed out") || lowered.contains("timeout") {
        return Failed {
            status: -1,
            failure: AcliFailure::TimedOut,
            message: message.to_owned(),
        };
    }
    Failed {
        status: -1,
        // A shell that could not even start the process is worth one more try: a machine that
        // just woke up refuses spawns for a moment.
        failure: AcliFailure::RateLimited,
        message: message.to_owned(),
    }
}

/// Classifies one finished `acli` invocation from its status and output.
///
/// `acli` reports failures as prose on either stream, so the text is the only signal there is.
#[must_use]
pub fn classify_failure(status: i32, stdout: &str, stderr: &str) -> AcliFailure {
    let message = message(stdout, stderr, status);
    let text = format!("{stdout}\n{stderr}").to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if has(&[
        "not authenticated",
        "auth login",
        "unauthorized",
        "not logged in",
    ]) {
        return AcliFailure::NotAuthenticated;
    }
    // Checked before the rate-limit family: a killed call is transient too, but it has already
    // spent `TIMEOUT` seconds, and "acli is rate limited" names a cause that never happened.
    if has(&["timed out", "timeout", "deadline exceeded"]) {
        return AcliFailure::TimedOut;
    }
    if has(&[
        "429",
        "rate limit",
        "usage limit",
        "too many requests",
        "temporarily unavailable",
        "connection reset",
        "503",
        "502",
        "504",
    ]) {
        return AcliFailure::RateLimited;
    }
    // Checked before the not-found family: "no allowed transitions found" is a refusal to
    // move an issue that exists, not a missing one.
    if has(&[
        "no allowed transitions",
        "be transitioned",
        "transition not",
        "invalid transition",
    ]) {
        return AcliFailure::Transition(message);
    }
    // A missing issue has to be recognized by the *subject* of the sentence, never by the
    // phrase alone: Jira says "Field 'customfield_10102' does not exist or you do not have
    // permission to view it" about a field the account cannot read, and a pull that reads that
    // as a deleted issue reports every key of the board as deleted and archives all of it.
    // The same subject guard both not-found families answer to. A bare `404` is not a subject
    // at all: Jira custom-field ids and issue keys carry one of their own (`customfield_10404`,
    // `SP-404`), so "Field 'customfield_10404' does not exist or you do not have permission to
    // view it" — the very message the phrase guard below exists for — used to come back through
    // the literal-`404` branch as a deleted issue, archiving the card and dropping its queued
    // push. A status code only counts when something says it is one.
    let about_an_issue = !has(&[
        "field",
        "customfield",
        "property",
        "screen",
        "sprint",
        "board",
        "user",
        "permission scheme",
    ]);
    if about_an_issue
        && has(&[
            "issue does not exist",
            "issue doesn't exist",
            "issue no longer exists",
            "issue not found",
            "work item does not exist",
            "work item not found",
            "workitem does not exist",
            "workitem not found",
            "http 404",
            "status 404",
            "status code 404",
            "404 not found",
        ])
    {
        return AcliFailure::NotFound;
    }
    if about_an_issue && has(&["does not exist", "doesn't exist", "no longer exists"]) {
        return AcliFailure::NotFound;
    }
    AcliFailure::Other(message)
}

/// Picks the most useful line of a failed invocation's output.
fn message(stdout: &str, stderr: &str, status: i32) -> String {
    let lines = || {
        stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .filter(|line| !line.is_empty())
    };
    if let Some(line) = lines().find(|line| line.starts_with('✗') || line.contains("Error")) {
        return clean_message(line);
    }
    // An undecorated failure is usually Jira's own error body, pretty-printed: its first line
    // is `{`, and taking it left `board sync failed: SP-993: {` in the job log, in the CLI and
    // in the persisted `sync.lastError`, naming nothing at all.
    if let Some(reported) = json_message(stderr).or_else(|| json_message(stdout)) {
        return reported;
    }
    lines()
        .next()
        .map_or_else(|| format!("acli exited {status}"), clean_message)
}

/// The sentence inside a JSON error body, in the shapes Jira answers with.
fn json_message(stream: &str) -> Option<String> {
    let body = parse_json(stream).ok()?;
    let text = |value: Option<&serde_json::Value>| {
        value
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    // `errorMessages` is the list Jira puts a whole sentence in; `errors` is the object that
    // names one field at a time, and a field's name is worth keeping.
    body.get("errorMessages")
        .and_then(serde_json::Value::as_array)
        .and_then(|messages| messages.iter().find_map(|message| text(Some(message))))
        .or_else(|| {
            body.get("errors")
                .and_then(serde_json::Value::as_object)
                .and_then(|errors| {
                    errors.iter().find_map(|(field, message)| {
                        text(Some(message)).map(|m| format!("{field}: {m}"))
                    })
                })
        })
        .or_else(|| text(body.get("message")))
        .or_else(|| text(body.get("error")))
}

/// Strips the decoration `acli` prefixes its errors with.
fn clean_message(line: &str) -> String {
    line.trim_start_matches(['✗', '✘', '×'])
        .trim()
        .trim_start_matches("Error:")
        .trim()
        .to_owned()
}

/// Parses JSON out of stdout, tolerating the chatter `acli` frames it with.
///
/// A CLI one release behind prints an "outdated version" nag before it prints the payload;
/// dropping the whole pull over it would make every sync depend on the user upgrading.
fn parse_json(stdout: &str) -> Result<serde_json::Value, BoardError> {
    let trimmed = stdout.trim();
    if let Ok(value) = serde_json::from_str(trimmed) {
        return Ok(value);
    }
    // Every candidate opening brace, not just the first: a nag that carries one of its own
    // ("a new version {1.4.0} is available") would otherwise commit the parse to the banner and
    // fail the whole pull naming only that line.
    for (start, _) in trimmed.match_indices(['{', '[']) {
        if let Some(Ok(value)) = serde_json::Deserializer::from_str(&trimmed[start..])
            .into_iter::<serde_json::Value>()
            .next()
        {
            return Ok(value);
        }
    }
    Err(unreadable(trimmed))
}

/// [`parse_json`], keeping every document of the stream instead of only the first.
///
/// The scan starts where `parse_json`'s does — at the first candidate brace whose document
/// parses, so a version nag carrying braces of its own is skipped rather than parsed — and then
/// takes every further value the stream yields, stopping at the first one that does not parse
/// so trailing chatter ends the run instead of failing it.
fn parse_json_all(stdout: &str) -> Result<Vec<serde_json::Value>, BoardError> {
    let trimmed = stdout.trim();
    for (start, _) in trimmed.match_indices(['{', '[']) {
        let values: Vec<serde_json::Value> = serde_json::Deserializer::from_str(&trimmed[start..])
            .into_iter::<serde_json::Value>()
            .map_while(Result::ok)
            .collect();
        if !values.is_empty() {
            return Ok(values);
        }
    }
    Err(unreadable(trimmed))
}

/// The one sentence an unparseable `acli` answer is reported as.
fn unreadable(trimmed: &str) -> BoardError {
    BoardError::Backend(format!(
        "acli returned unreadable output: {}",
        trimmed.lines().next().unwrap_or("<empty>")
    ))
}

/// Parses the three lines `acli jira auth status` prints.
fn parse_auth_status(text: &str) -> AuthStatus {
    let field = |name: &str| {
        text.lines()
            .filter_map(|line| {
                let line = line.trim().trim_start_matches(['✓', '✔', '-', '*']).trim();
                // The `:` is required, not optional: without it any line merely *beginning*
                // with the name matched, so a `Sites: 3` status line answered `Site` with
                // `s: 3` — a site `validate` then refuses every correctly configured board
                // against, and that every `browse_url` is built from.
                let rest = line.strip_prefix(name)?.trim_start().strip_prefix(':')?;
                Some(rest.trim().to_owned())
            })
            .find(|value| !value.is_empty())
    };
    let lowered = text.to_lowercase();
    AuthStatus {
        // "Unauthenticated" contains "authenticated" and not "not authenticated": read as a
        // live session, `validate` accepts the board and every later call fails with a worse
        // sentence than the one this line had in its hands.
        authenticated: lowered.contains("authenticated")
            && !lowered.contains("not authenticated")
            && !lowered.contains("unauthenticated")
            && !lowered.contains("logged out"),
        site: field("Site"),
        email: field("Email"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{adapters::shell::ShellResult, testing::fakes::FakeShell};

    fn ok(stdout: &str) -> ShellResult {
        ShellResult {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn failure(status: i32, stderr: &str) -> ShellResult {
        ShellResult {
            status,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    fn acli(shell: &Arc<FakeShell>) -> Acli {
        Acli::new(shell.clone(), 2)
    }

    #[tokio::test]
    async fn a_call_is_prefixed_with_jira_bounded_and_deadlined() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            ok("{\"key\":\"SP-1\"}"),
        );
        let value = acli(&shell)
            .json(&["workitem", "view", "SP-1", "--json"])
            .await
            .unwrap();
        assert_eq!(value["key"], "SP-1");
        let crate::testing::fakes::FakeShellCall::Run(command) = &shell.calls()[0] else {
            panic!("acli must run, never stream or detach");
        };
        assert_eq!(command.program, "acli");
        assert_eq!(command.args, ["jira", "workitem", "view", "SP-1", "--json"]);
        assert_eq!(command.timeout, Some(TIMEOUT));
    }

    /// A CLI one release behind prints a nag first; the payload is still the payload.
    #[tokio::test]
    async fn json_survives_an_outdated_version_nag_around_the_payload() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            ok("A new version of acli is available!\n[{\"key\":\"SP-1\"}]\nRun acli update\n"),
        );
        let value = acli(&shell).json(&["workitem", "search"]).await.unwrap();
        assert_eq!(value[0]["key"], "SP-1");
        let shell = Arc::new(FakeShell::new());
        shell.when(|command| command.program == "acli", ok("no json here"));
        assert!(
            acli(&shell)
                .json(&["workitem", "search"])
                .await
                .unwrap_err()
                .to_string()
                .contains("unreadable")
        );
    }

    /// `--paginate` may answer one document per page; every page is a page of the board.
    #[tokio::test]
    async fn json_all_reads_every_document_past_a_version_nag() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            ok("A new version {1.4.0} is available!\n[{\"key\":\"SP-1\"}]\n[{\"key\":\"SP-2\"}]\nRun acli update\n"),
        );
        let pages = acli(&shell)
            .json_all(&["workitem", "search"])
            .await
            .unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0][0]["key"], "SP-1");
        assert_eq!(pages[1][0]["key"], "SP-2");
        // One merged array — what the CLI documents — is still one document.
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            ok("[{\"key\":\"SP-1\"},{\"key\":\"SP-2\"}]"),
        );
        let pages = acli(&shell)
            .json_all(&["workitem", "search"])
            .await
            .unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].as_array().unwrap().len(), 2);
        let shell = Arc::new(FakeShell::new());
        shell.when(|command| command.program == "acli", ok("no json here"));
        assert!(
            acli(&shell)
                .json_all(&["workitem", "search"])
                .await
                .unwrap_err()
                .to_string()
                .contains("unreadable")
        );
    }

    /// The whole point of the retry: a throttled call is the site being busy, not an answer.
    ///
    /// Only the first backoff step is exercised, because the rest of the policy is real
    /// seconds and a test suite that waits them out teaches nothing more.
    #[tokio::test]
    async fn a_throttled_call_is_retried_and_a_refused_one_is_not() {
        let shell = Arc::new(FakeShell::new());
        let throttled = std::sync::atomic::AtomicBool::new(true);
        shell.when(
            move |command| {
                command.program == "acli"
                    && throttled.swap(false, std::sync::atomic::Ordering::SeqCst)
            },
            failure(1, "✗ Error: 429 Too Many Requests"),
        );
        shell.when(|command| command.program == "acli", ok("[]"));
        assert_eq!(
            acli(&shell).json(&["workitem", "search"]).await.unwrap(),
            serde_json::json!([])
        );
        assert_eq!(shell.calls().len(), 2);

        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            failure(1, "✗ Error: Issue does not exist"),
        );
        assert!(acli(&shell).json(&["workitem", "view"]).await.is_err());
        assert_eq!(shell.calls().len(), 1);
    }

    #[tokio::test]
    async fn a_missing_issue_is_absence_and_everything_else_is_an_error() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.args.contains(&"SP-404".to_owned()),
            failure(
                1,
                "✗ Error: Issue does not exist or you do not have permission",
            ),
        );
        shell.when(
            |command| command.args.contains(&"SP-500".to_owned()),
            failure(1, "✗ Error: something broke"),
        );
        let acli = acli(&shell);
        assert_eq!(
            acli.json_optional(&["workitem", "view", "SP-404"])
                .await
                .unwrap(),
            None
        );
        assert!(
            acli.json_optional(&["workitem", "view", "SP-500"])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_unauthenticated_cli_answers_the_status_question_and_fails_every_other_call() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            failure(1, "✗ Error: You are not authenticated"),
        );
        let acli = acli(&shell);
        assert_eq!(acli.auth_status().await.unwrap(), AuthStatus::default());
        assert_eq!(
            acli.json(&["workitem", "search"])
                .await
                .unwrap_err()
                .to_string(),
            format!("backend error: {NOT_AUTHENTICATED}")
        );
    }

    #[tokio::test]
    async fn auth_status_reads_the_site_and_email_it_prints() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            ok("✓ Authenticated\nSite: buk.atlassian.net\nEmail: danny@example.com\n"),
        );
        assert_eq!(
            acli(&shell).auth_status().await.unwrap(),
            AuthStatus {
                authenticated: true,
                site: Some("buk.atlassian.net".into()),
                email: Some("danny@example.com".into()),
            }
        );
    }

    /// The fake answers an unmatched command with 127, which is exactly what a machine with
    /// no `acli` on its PATH looks like.
    #[tokio::test]
    async fn a_missing_cli_says_how_to_install_it() {
        let shell = Arc::new(FakeShell::new());
        assert_eq!(
            acli(&shell).version().await.unwrap_err().to_string(),
            format!("backend error: {NOT_INSTALLED}")
        );
        let shell = Arc::new(FakeShell::new());
        shell.when(|command| command.program == "acli", ok("acli 1.3.18\n"));
        assert_eq!(acli(&shell).version().await.unwrap(), "acli 1.3.18");
        let crate::testing::fakes::FakeShellCall::Run(command) = &shell.calls()[0] else {
            panic!("version must be a plain run");
        };
        // `--version` is not a `jira` subcommand, and prefixing it prints Jira's help instead.
        assert_eq!(command.args, ["--version"]);
    }

    /// A create that Jira may already have applied is never sent twice.
    #[tokio::test]
    async fn a_write_that_may_have_landed_is_not_retried() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            failure(1, "✗ Error: 504 Gateway Timeout"),
        );
        let acli = acli(&shell);
        assert!(
            acli.json_once(&["workitem", "create", "--from-json", "f.json"])
                .await
                .is_err()
        );
        assert_eq!(shell.calls().len(), 1, "a create must never be repeated");
    }

    /// The one that archived a board: a field the account cannot read is not a missing issue.
    #[tokio::test]
    async fn a_field_the_account_cannot_read_is_a_failure_not_a_deletion() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            failure(
                1,
                "✗ Error: Field 'customfield_10102' does not exist or you do not have permission to view it",
            ),
        );
        let error = acli(&shell)
            .json_optional(&["workitem", "view", "SP-1"])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("customfield_10102"), "{error}");
    }

    #[test]
    fn every_failure_text_in_the_command_reference_is_classified() {
        for (stderr, expected) in [
            ("✗ Error: not authenticated", AcliFailure::NotAuthenticated),
            ("unauthorized", AcliFailure::NotAuthenticated),
            ("✗ Error: 429 usage limit reached", AcliFailure::RateLimited),
            ("rate limit exceeded", AcliFailure::RateLimited),
            ("✗ Error: Issue does not exist", AcliFailure::NotFound),
            (
                "SP-1 can't be transitioned: No allowed transitions found",
                AcliFailure::Transition(
                    "SP-1 can't be transitioned: No allowed transitions found".into(),
                ),
            ),
            (
                "✗ Error: field 'priority' is not on the screen",
                AcliFailure::Other("field 'priority' is not on the screen".into()),
            ),
            // The phrase alone is not the issue: this one is a field, and reading it as a
            // deletion is what archived a whole board.
            (
                "✗ Error: Field 'customfield_1' does not exist or you do not have permission to view it",
                AcliFailure::Other(
                    "Field 'customfield_1' does not exist or you do not have permission to view it"
                        .into(),
                ),
            ),
            ("✗ Error: SP-9 does not exist", AcliFailure::NotFound),
            // A bare `404` is not a subject. Jira custom-field ids and issue keys carry one of
            // their own, and reading that as a deleted issue archived the card and dropped the
            // push it had queued — the same class the field guard above exists for, through the
            // one branch that guard did not cover.
            (
                "✗ Error: Field 'customfield_10404' does not exist or you do not have permission to view it",
                AcliFailure::Other(
                    "Field 'customfield_10404' does not exist or you do not have permission to view it"
                        .into(),
                ),
            ),
            (
                "✗ Error: sprint 404 is not on board 4040",
                AcliFailure::Other("sprint 404 is not on board 4040".into()),
            ),
            // A status code is a subject when something says it is one.
            ("✗ Error: HTTP 404 for SP-7", AcliFailure::NotFound),
            ("✗ Error: 404 Not Found", AcliFailure::NotFound),
            // A killed call is transient, but it is not throttling and must not be retried
            // three more times at 90 s apiece.
            (
                "✗ Error: acli jira workitem search timed out",
                AcliFailure::TimedOut,
            ),
        ] {
            assert_eq!(classify_failure(1, "", stderr), expected, "{stderr}");
        }
        // With nothing printed at all the status is the only thing left to say.
        assert_eq!(
            classify_failure(2, "", ""),
            AcliFailure::Other("acli exited 2".into())
        );
    }

    /// A timeout is retried once, not three times: four 90 s attempts is six minutes of wall
    /// clock spent before the user hears a sentence that names throttling instead.
    #[tokio::test]
    async fn a_timed_out_call_is_retried_once_and_named_for_what_it_was() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "acli",
            failure(1, "✗ Error: acli jira workitem search timed out"),
        );
        let error = acli(&shell)
            .json(&["workitem", "search"])
            .await
            .unwrap_err();
        assert_eq!(shell.calls().len(), 2);
        assert!(error.to_string().contains("timed out after 90s"), "{error}");
        assert!(!error.to_string().contains("rate limited"), "{error}");
    }

    /// "Unauthenticated" contains "authenticated" and not "not authenticated": read as a live
    /// session, `validate` accepts the board and every later call fails with a worse sentence.
    #[test]
    fn an_unauthenticated_status_is_not_read_as_a_session() {
        assert!(!parse_auth_status("Status: Unauthenticated").authenticated);
        assert!(!parse_auth_status("You are logged out").authenticated);
        assert!(!parse_auth_status("Not authenticated").authenticated);
        let status = parse_auth_status("✓ Authenticated\nSite: buk.atlassian.net\nEmail: a@b.c");
        assert!(status.authenticated);
        assert_eq!(status.site.as_deref(), Some("buk.atlassian.net"));
    }

    /// The nag `parse_json` exists for can carry a brace of its own; committing the parse to
    /// the first one fails the whole pull naming only the banner.
    #[test]
    fn a_banner_with_a_brace_in_it_does_not_hide_the_payload() {
        let value = parse_json("A new version {1.4.0} is available\n{\"key\": \"SP-1\"}\n")
            .expect("the payload is still in there");
        assert_eq!(value["key"], "SP-1");
        // Nothing parseable at all is still an honest failure.
        assert!(parse_json("A new version {1.4.0} is available").is_err());
    }

    /// Jira answers a refused write with its own error body, pretty-printed. Its first line is
    /// `{`, and taking it left the whole board's failure reading `SP-993: {` — in the job log,
    /// in the CLI, in the app header and in the persisted `sync.lastError`.
    #[test]
    fn a_json_error_body_is_reported_by_its_sentence_and_not_by_its_first_brace() {
        let body = "{\n  \"errorMessages\": [\n    \"Issue type is a sub-task but parent issue key or id not specified.\"\n  ],\n  \"errors\": {}\n}";
        assert_eq!(
            message("", body, 1),
            "Issue type is a sub-task but parent issue key or id not specified."
        );
        // The field-keyed shape keeps the field's name, which is the useful half.
        assert_eq!(
            message(
                "{\n  \"errorMessages\": [],\n  \"errors\": {\"duedate\": \"Date not valid\"}\n}",
                "",
                1
            ),
            "duedate: Date not valid"
        );
        assert_eq!(message("{\"message\": \"Nope\"}", "", 1), "Nope");
        // A decorated line still outranks the body, and prose that is not JSON is untouched.
        assert_eq!(
            message("{\"message\": \"Nope\"}", "✗ Error: Nope, louder", 1),
            "Nope, louder"
        );
        assert_eq!(message("plain trouble", "", 1), "plain trouble");
        assert_eq!(message("", "", 7), "acli exited 7");
    }

    /// `Site` is a prefix of `Sites`, and the delimiter is what tells them apart: without it a
    /// `Sites: 3` line answered `Site` with `s: 3`, which `validate` then refuses every
    /// correctly configured board against and `browse_url` builds every card's link from.
    #[test]
    fn an_auth_field_is_only_read_from_its_own_labelled_line() {
        let status = parse_auth_status("✓ Authenticated\nSites: 3\nSite: buk.atlassian.net\n");
        assert_eq!(status.site.as_deref(), Some("buk.atlassian.net"));
        assert_eq!(parse_auth_status("✓ Authenticated\nSites: 3\n").site, None);
    }

    /// A binary that is not on PATH never reaches an exit status, so the install hint has to
    /// come from the spawn failure itself — and it must not be retried three times first.
    #[test]
    fn a_missing_binary_is_the_install_hint_and_never_a_rate_limit() {
        let failed = failed_shell("acli: No such file or directory (os error 2)");
        assert_eq!(failed.status, 127);
        assert_ne!(failed.failure, AcliFailure::RateLimited);
        let BoardError::Backend(message) = BoardError::from(failed) else {
            panic!("a failed acli call is a backend error");
        };
        assert_eq!(message, NOT_INSTALLED);
        // A shell that refused the spawn for any other reason is still worth one more try.
        assert_eq!(
            failed_shell("acli: Resource temporarily unavailable (os error 35)").failure,
            AcliFailure::RateLimited
        );
    }
}
