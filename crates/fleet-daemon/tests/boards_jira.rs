//! The Jira backend against a scripted `acli`, and one board synchronizing through it.
//!
//! Nothing here ever reaches a real site: every invocation is matched by a [`FakeShell`] rule,
//! and every assertion is about the argv the backend produced — because on a CLI with no API
//! the argv *is* the contract. `docs/BOARD-JIRA.md` §7 lists the commands being asserted.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use fleet_core::{board::*, ids::BoardId, model::Context, paths::FleetHome, state::default_state};
use fleet_daemon::{
    DaemonError, DaemonResult,
    adapters::{
        Adapters,
        board::{
            BoardBackend, BoardBackends, JiraBackend, LocalBackend,
            jira::{JiraSettings, map::VIEW_FIELDS, map::view_fields},
        },
        files::RealFiles,
        github::GhCli,
        process::RealProcess,
        shell::{DetachedProcess, LineCallback, Shell, ShellCommand, ShellResult},
    },
    jobs::JobManager,
    server::BroadcastBus,
    services::{boards::Boards, sessions::Sessions, worktrees::Worktrees},
    stores::{board::BoardStore, config::ConfigStore, state::StateStore},
    testing::fakes::{FakeGit, FakeShell, FakeShellCall, FixedClock},
};
use fleet_proto::job::JobStatus;
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-09-06T12:00:00+00:00";

fn ok(stdout: impl Into<String>) -> ShellResult {
    ShellResult {
        status: 0,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

fn refused(stderr: &str) -> ShellResult {
    ShellResult {
        status: 1,
        stdout: String::new(),
        stderr: stderr.into(),
    }
}

/// Answers every `acli` call whose argv contains all of `needles` as substrings.
///
/// Rules are tried in the order they were added, so the specific ones go in first.
fn script(shell: &FakeShell, needles: &[&str], result: ShellResult) {
    let needles: Vec<String> = needles.iter().map(|needle| (*needle).to_owned()).collect();
    shell.when(
        move |command| {
            command.program == "acli"
                && needles.iter().all(|needle| {
                    command
                        .args
                        .iter()
                        .any(|argument| argument.contains(needle.as_str()))
                })
        },
        result,
    );
}

/// Whether a command is exactly `workitem view <key> --fields <fields>`.
fn views(command: &ShellCommand, key: &str, fields: &str) -> bool {
    command.program == "acli"
        && command.args.iter().any(|argument| argument == "view")
        && command.args.iter().any(|argument| argument == key)
        && command
            .args
            .windows(2)
            .any(|pair| pair[0] == "--fields" && pair[1] == fields)
}

/// Answers one `workitem view <key> --fields <fields>` and nothing else.
///
/// The field list is what separates the pull's view of an issue from the two-field ones a
/// push makes, and a rule that confused them would answer the wrong question.
fn script_view(shell: &FakeShell, key: &str, fields: &str, result: ShellResult) {
    let (key, fields) = (key.to_owned(), fields.to_owned());
    shell.when(move |command| views(command, &key, &fields), result);
}

/// The `--fields` list a pull of this board asks every issue for.
fn pull_fields() -> String {
    view_fields(&JiraSettings::parse(&jira_settings()).unwrap())
}

/// A shell that answers `acli --version` and `acli jira auth status` like a signed-in machine.
fn signed_in() -> Arc<FakeShell> {
    let shell = Arc::new(FakeShell::new());
    shell.when(
        |command| command.program == "acli" && command.args == ["--version"],
        ok("acli version 1.3.18\n"),
    );
    script(
        &shell,
        &["auth", "status"],
        ok("✓ Authenticated\nSite: buk.atlassian.net\nEmail: danny@example.com\n"),
    );
    shell
}

/// The argv of every `acli` call, in order.
fn acli_calls(shell: &FakeShell) -> Vec<Vec<String>> {
    shell
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            FakeShellCall::Run(command) if command.program == "acli" => Some(command.args),
            _ => None,
        })
        .collect()
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/jira")
            .join(name),
    )
    .unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

/// A view payload for one of the keys the search fixture lists.
fn issue(key: &str, summary: &str, status: &str, category: &str) -> String {
    serde_json::json!({
        "key": key,
        "fields": {
            "summary": summary,
            "description": null,
            "status": {"name": status, "statusCategory": {"key": category}},
            "labels": [],
            "updated": "2026-09-05T10:00:00.000-0300",
            "created": "2026-09-01T09:00:00.000-0300",
            "issuetype": {"name": "Task"},
        }
    })
    .to_string()
}

/// The same issue after somebody else renamed it in Jira.
fn renamed_issue() -> String {
    let mut issue: serde_json::Value = serde_json::from_str(&fixture("issue.json")).unwrap();
    issue["fields"]["summary"] = serde_json::json!("Renamed in Jira");
    issue["fields"]["updated"] = serde_json::json!("2026-09-06T11:00:00.000-0300");
    issue.to_string()
}

fn jira_settings() -> serde_json::Value {
    serde_json::json!({
        "project": "SP",
        "site": "buk.atlassian.net",
        "storyPointsField": "customfield_10102"
    })
}

fn board_with(settings: serde_json::Value) -> Board {
    let mut board = new_board(
        &Context {
            id: "work".parse().unwrap(),
            name: "Work".into(),
            owners: vec![],
            created_at: NOW.into(),
        },
        NOW,
    );
    board.backend = BackendRef {
        kind: "jira".into(),
        settings,
    };
    board
}

fn clock() -> Arc<FixedClock> {
    Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).single().unwrap(),
    ))
}

fn backend(shell: Arc<dyn Shell>) -> JiraBackend {
    JiraBackend::new(shell, clock())
}

/// A shell that records how many `acli` calls were ever in flight at once.
struct Counting {
    inner: Arc<FakeShell>,
    live: AtomicUsize,
    peak: AtomicUsize,
}

impl Counting {
    fn new(inner: Arc<FakeShell>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            live: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Shell for Counting {
    async fn run(&self, command: ShellCommand) -> DaemonResult<ShellResult> {
        let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(live, Ordering::SeqCst);
        // Every in-flight call gets a chance to start before any of them finishes, so the
        // peak reflects the bound rather than the scheduler.
        tokio::task::yield_now().await;
        let result = self.inner.run(command).await;
        self.live.fetch_sub(1, Ordering::SeqCst);
        result
    }
    async fn run_detached(
        &self,
        command: ShellCommand,
        log_path: &std::path::Path,
    ) -> DaemonResult<DetachedProcess> {
        self.inner.run_detached(command, log_path).await
    }
    async fn run_streaming(
        &self,
        command: ShellCommand,
        cancel: CancellationToken,
        on_line: LineCallback,
    ) -> DaemonResult<ShellResult> {
        self.inner.run_streaming(command, cancel, on_line).await
    }
}

#[tokio::test]
async fn validate_checks_the_cli_the_session_and_the_site() {
    let shell = signed_in();
    backend(shell.clone())
        .validate(&jira_settings())
        .await
        .unwrap();
    assert_eq!(acli_calls(&shell)[0], ["--version"]);
    assert_eq!(acli_calls(&shell)[1], ["jira", "auth", "status"]);

    // One `acli` account per machine: a board naming another site would mirror the wrong
    // issues under the right project key.
    let error = backend(signed_in())
        .validate(&serde_json::json!({"project": "SP", "site": "https://other.atlassian.net/"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("buk.atlassian.net"), "{error}");

    // The same settings without a site are fine, because nothing contradicts the session.
    backend(signed_in())
        .validate(&serde_json::json!({"project": "SP"}))
        .await
        .unwrap();

    // A machine with no `acli` at all must say so, not report a Jira problem.
    let error = backend(Arc::new(FakeShell::new()))
        .validate(&jira_settings())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("install the Atlassian CLI"), "{error}");
}

#[tokio::test]
async fn an_unauthenticated_cli_fails_with_the_command_that_fixes_it() {
    let shell = Arc::new(FakeShell::new());
    shell.when(
        |command| command.args == ["--version"],
        ok("acli version 1.3.18\n"),
    );
    script(
        &shell,
        &["auth"],
        refused("✗ Error: you are not authenticated"),
    );
    let error = backend(shell.clone())
        .validate(&jira_settings())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("acli jira auth login"), "{error}");

    // And a pull that runs into it says the same thing rather than "empty board".
    let shell = Arc::new(FakeShell::new());
    script(&shell, &["search"], refused("✗ Error: unauthorized"));
    let error = backend(shell)
        .pull(&board_with(jira_settings()), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("acli jira auth login"), "{error}");
}

#[tokio::test]
async fn describe_samples_statuses_labels_and_people_and_declares_what_it_cannot_write() {
    let shell = signed_in();
    script(&shell, &["search"], ok(fixture("sample.json")));
    script(&shell, &["project", "view"], ok(fixture("project.json")));
    let schema = backend(shell.clone())
        .describe(&board_with(jira_settings()))
        .await
        .unwrap();

    assert_eq!(
        acli_calls(&shell)[0],
        [
            "jira",
            "workitem",
            "search",
            "--jql",
            "project = \"SP\" ORDER BY updated DESC",
            "--fields",
            "status,labels,assignee",
            "--limit",
            "200",
            "--json"
        ]
    );
    assert_eq!(
        acli_calls(&shell)[1],
        ["jira", "project", "view", "--key", "SP", "--json"]
    );

    // Columns read left to right the way work does, whatever order the sample arrived in.
    assert_eq!(
        schema
            .statuses
            .iter()
            .map(|status| (status.name.as_str(), status.category))
            .collect::<Vec<_>>(),
        vec![
            ("Backlog", Some(StatusCategory::Backlog)),
            ("In Progress", Some(StatusCategory::Started)),
            ("Done", Some(StatusCategory::Completed)),
            // Jira files a cancellation under `done`; a board that showed it as completed
            // would report work that never happened.
            ("Cancelled", Some(StatusCategory::Canceled)),
        ]
    );
    // Transitions move by name, so the name is the identity the status map is keyed on.
    assert!(
        schema
            .statuses
            .iter()
            .all(|status| status.id == status.name)
    );
    assert_eq!(schema.labels, ["payroll", "sector-publico", "spike"]);
    assert_eq!(schema.assignees, ["Ana Rojas", "Danny Fuentes"]);
    assert_eq!(schema.key_prefix.as_deref(), Some("SP"));
    assert_eq!(
        schema.readonly_fields,
        ["priority", "estimate", "due_date", "parent_id"]
    );
    let issue_type = &schema.properties[0];
    assert_eq!(issue_type.key, "jira.issue_type");
    assert!(!issue_type.editable && issue_type.source == PropertySource::Backend);
    assert_eq!(
        issue_type
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        ["Epic", "Task", "Bug", "Sub-task"]
    );
    assert_eq!(schema.properties[1].key, "jira.created");
    assert_eq!(schema.properties[1].kind, PropertyKind::Date);
}

#[tokio::test]
async fn a_board_that_names_its_own_columns_still_samples_its_people() {
    let shell = signed_in();
    script(&shell, &["project", "view"], ok(fixture("project.json")));
    script(&shell, &["search"], ok(fixture("sample.json")));
    let schema = backend(shell.clone())
        .describe(&board_with(serde_json::json!({
            "project": "SP",
            "statuses": ["Backlog", "In Progress", "Done"],
            "extraFields": [{"id": "customfield_10001", "name": "Team", "kind": "select"}]
        })))
        .await
        .unwrap();
    // The columns are the user's, but the accounts are not: `edit --assignee` wants an account
    // id, and an issue payload is the only place this backend ever learns one. A restarted
    // daemon that skipped this could not assign anyone outside the next incremental window —
    // and that failure aborts the whole sync, not just the one card's push.
    let sample = acli_calls(&shell)
        .into_iter()
        .find(|call| call.contains(&"search".to_owned()))
        .expect("the people sample still runs");
    assert!(
        sample.contains(&"labels,assignee".to_owned()),
        "the sample asks for people and labels, not statuses: {sample:?}"
    );
    assert_eq!(schema.assignees, ["Ana Rojas", "Danny Fuentes"]);
    assert_eq!(
        schema
            .statuses
            .iter()
            .map(|status| (status.name.as_str(), status.category))
            .collect::<Vec<_>>(),
        vec![
            ("Backlog", Some(StatusCategory::Backlog)),
            ("In Progress", Some(StatusCategory::Started)),
            ("Done", Some(StatusCategory::Completed)),
        ]
    );
    assert_eq!(schema.properties[2].key, "jira.customfield_10001");
    assert_eq!(schema.properties[2].name, "Team");
    assert_eq!(schema.properties[2].kind, PropertyKind::Select);
}

#[tokio::test]
async fn a_full_pull_searches_once_and_views_every_key_within_the_concurrency_limit() {
    let inner = signed_in();
    script(&inner, &["search"], ok(fixture("search.json")));
    script(&inner, &["view", "SP-42"], ok(fixture("issue.json")));
    script(
        &inner,
        &["view", "SP-1"],
        ok(issue("SP-1", "Payroll epic", "Done", "done")),
    );
    script(
        &inner,
        &["view", "SP-77"],
        ok(issue("SP-77", "Retire the old exporter", "Backlog", "new")),
    );
    let shell = Counting::new(inner.clone());
    let mut settings = jira_settings();
    settings["maxConcurrency"] = serde_json::json!(2);
    let result = backend(shell.clone())
        .pull(&board_with(settings), None)
        .await
        .unwrap();

    let calls = acli_calls(&inner);
    assert_eq!(
        calls[0],
        [
            "jira",
            "workitem",
            "search",
            "--jql",
            // No window: a pull with no cursor is the complete remote set.
            "project = \"SP\" ORDER BY updated ASC",
            "--fields",
            "summary",
            "--paginate",
            "--json"
        ]
    );
    let fields = format!("{},customfield_10102", VIEW_FIELDS.join(","));
    // The views overlap, so their start order belongs to the scheduler; what each one asks
    // for does not.
    let mut views: Vec<Vec<String>> = calls[1..].to_vec();
    views.sort();
    for (view, key) in views.iter().zip(["SP-1", "SP-42", "SP-77"]) {
        assert_eq!(
            view,
            &[
                "jira", "workitem", "view", key, "--fields", &fields, "--json"
            ]
        );
    }
    assert_eq!(calls.len(), 4, "one search and one view per key, no more");
    let peak = shell.peak.load(Ordering::SeqCst);
    assert!(
        peak <= 2,
        "{peak} acli calls ran at once, the board allows 2"
    );
    assert_eq!(
        peak, 2,
        "views must actually overlap, or a sprint takes minutes"
    );

    assert!(result.full);
    assert!(result.deleted_keys.is_empty());
    assert_eq!(
        result
            .cards
            .iter()
            .map(|card| card.key.as_str())
            .collect::<Vec<_>>(),
        ["SP-1", "SP-42", "SP-77"]
    );
    let card = &result.cards[1];
    assert_eq!(card.title, "Rework the payroll export");
    assert_eq!(card.status.name, "In Progress");
    assert_eq!(card.estimate, Some(3));
    assert_eq!(card.parent_key.as_deref(), Some("SP-1"));
    assert_eq!(card.priority, Some(Priority::High));
    assert!(card.description.contains("Context"), "{}", card.description);
    assert!(
        card.description.contains("Payroll::Export"),
        "{}",
        card.description
    );
    assert_eq!(card.comments.len(), 2);
    // A fresh cursor is due for its next full pull ten pulls from now.
    assert_eq!(result.cursor.as_deref(), Some("2026-09-06T12:00:00Z|0"));
}

/// `--paginate` is documented to answer one merged array; a CLI that answers one document per
/// page instead must not lose every key past the first, because a full pull reports a key it
/// never listed as deleted and archives the card holding it.
#[tokio::test]
async fn a_paginated_search_reads_every_document_the_cli_printed() {
    let shell = signed_in();
    script(
        &shell,
        &["search"],
        // Two pages, and the first key repeated on the second: `SP-1` must be viewed once.
        ok("[{\"key\":\"SP-1\"}]\n[{\"key\":\"SP-1\"},{\"key\":\"SP-77\"}]\n"),
    );
    script(
        &shell,
        &["view", "SP-1"],
        ok(issue("SP-1", "Payroll epic", "Done", "done")),
    );
    script(
        &shell,
        &["view", "SP-77"],
        ok(issue("SP-77", "Retire the old exporter", "Backlog", "new")),
    );
    let result = backend(shell.clone())
        .pull(&board_with(jira_settings()), None)
        .await
        .unwrap();

    let mut keys: Vec<&str> = result.cards.iter().map(|card| card.key.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["SP-1", "SP-77"], "page two was dropped");
    assert!(result.deleted_keys.is_empty());
    assert_eq!(
        acli_calls(&shell)
            .iter()
            .filter(|call| call.contains(&"view".to_owned()))
            .count(),
        2,
        "a key listed on both pages is viewed once"
    );
}

#[tokio::test]
async fn an_incremental_pull_windows_by_the_watermark_and_goes_full_every_n_pulls() {
    let shell = signed_in();
    script(&shell, &["search"], ok("[]"));
    let mut settings = jira_settings();
    settings["fullSyncEvery"] = serde_json::json!(3);
    settings["overlapMinutes"] = serde_json::json!(10);
    let board = board_with(settings);
    let backend = backend(shell.clone());

    // Two hours since the last pull, plus the overlap Jira's minute resolution needs.
    let result = backend
        .pull(&board, Some("2026-09-06T10:00:00Z|1"))
        .await
        .unwrap();
    assert!(!result.full);
    assert_eq!(
        acli_calls(&shell)[0][4],
        "project = \"SP\" AND updated >= \"-130m\" ORDER BY updated ASC"
    );
    assert_eq!(result.cursor.as_deref(), Some("2026-09-06T12:00:00Z|2"));

    // The Nth pull is full: an incremental window can never see a deletion.
    let result = backend
        .pull(&board, Some("2026-09-06T10:00:00Z|3"))
        .await
        .unwrap();
    assert!(result.full);
    assert_eq!(
        acli_calls(&shell)[1][4],
        "project = \"SP\" ORDER BY updated ASC"
    );
    assert_eq!(result.cursor.as_deref(), Some("2026-09-06T12:00:00Z|0"));

    // A cursor nobody can read is a cursor nobody can resume from.
    let result = backend.pull(&board, Some("corrupted")).await.unwrap();
    assert!(result.full);
    assert_eq!(
        acli_calls(&shell)[2][4],
        "project = \"SP\" ORDER BY updated ASC"
    );
}

/// A busy site is not an answer. A search that came back throttled and was reported as an
/// empty remote would archive every card on the board.
#[tokio::test]
async fn a_throttled_search_is_retried_rather_than_read_as_an_empty_remote() {
    let shell = signed_in();
    let throttled = std::sync::atomic::AtomicBool::new(true);
    shell.when(
        move |command| {
            command.program == "acli"
                && command.args.iter().any(|argument| argument == "search")
                && throttled.swap(false, Ordering::SeqCst)
        },
        refused("✗ Error: 429 Too Many Requests"),
    );
    script(&shell, &["search"], ok("[]"));
    let result = backend(shell.clone())
        .pull(&board_with(jira_settings()), None)
        .await
        .unwrap();
    assert!(result.cards.is_empty() && result.full);
    assert_eq!(
        acli_calls(&shell)
            .iter()
            .filter(|call| call.contains(&"search".to_owned()))
            .count(),
        2,
        "the first search was throttled and must have been tried again"
    );
}

#[tokio::test]
async fn a_key_that_vanishes_between_search_and_view_is_reported_as_deleted() {
    let shell = signed_in();
    script(&shell, &["search"], ok(fixture("search.json")));
    script(&shell, &["view", "SP-42"], ok(fixture("issue.json")));
    script(
        &shell,
        &["view", "SP-1"],
        ok(issue("SP-1", "Payroll epic", "Done", "done")),
    );
    script(
        &shell,
        &["view", "SP-77"],
        refused("✗ Error: Issue does not exist or you do not have permission to see it"),
    );
    let result = backend(shell)
        .pull(&board_with(jira_settings()), None)
        .await
        .unwrap();
    assert_eq!(
        result
            .cards
            .iter()
            .map(|card| card.key.as_str())
            .collect::<Vec<_>>(),
        ["SP-1", "SP-42"]
    );
    // Reported, not silently dropped: on an incremental pull nothing else would ever notice.
    assert_eq!(result.deleted_keys, ["SP-77"]);
}

/// The one that emptied a board: a field the account cannot read is not a deleted issue.
#[tokio::test]
async fn a_field_error_on_every_view_fails_the_pull_instead_of_deleting_the_board() {
    let shell = signed_in();
    script(&shell, &["search"], ok(fixture("search.json")));
    script(
        &shell,
        &["view"],
        refused(
            "✗ Error: Field 'customfield_10102' does not exist or you do not have permission to view it",
        ),
    );
    let error = backend(shell)
        .pull(&board_with(jira_settings()), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("customfield_10102"), "{error}");
}

/// One issue that will not load must not cost the pull the ones that did: a full pull that
/// aborts here never advances its cursor, so a project big enough to keep hitting it can never
/// complete a pull again — and stops seeing deletions along with everything else.
#[tokio::test]
async fn one_unreadable_issue_is_reported_and_keeps_the_rest_of_the_pull() {
    let shell = signed_in();
    script(&shell, &["search"], ok(fixture("search.json")));
    script(&shell, &["view", "SP-42"], ok(fixture("issue.json")));
    script(
        &shell,
        &["view", "SP-1"],
        ok(issue("SP-1", "Payroll epic", "Done", "done")),
    );
    script(
        &shell,
        &["view", "SP-77"],
        refused(
            "✗ Error: Field 'customfield_10102' does not exist or you do not have permission to view it",
        ),
    );
    let result = backend(shell)
        .pull(&board_with(jira_settings()), Some("2026-09-06T10:00:00Z|3"))
        .await
        .unwrap();
    assert_eq!(
        result
            .cards
            .iter()
            .map(|card| card.key.as_str())
            .collect::<Vec<_>>(),
        ["SP-1", "SP-42"]
    );
    assert_eq!(result.failed_keys.len(), 1);
    assert!(
        result.failed_keys[0].starts_with("SP-77: "),
        "{:?}",
        result.failed_keys
    );
    // A key that failed is never a key that was deleted, and the watermark stays where it was
    // so the next pull offers the same window again. The *counter* still moves: rewinding the
    // whole cursor froze `pulls_since_full`, so one permanently unreadable key meant no full
    // pull ever came due again and remote deletions stopped being noticed for its lifetime.
    assert!(result.deleted_keys.is_empty());
    assert_eq!(result.cursor.as_deref(), Some("2026-09-06T10:00:00Z|4"));
}

/// The counter keeps moving under a permanently unreadable key, so the full pull that notices
/// deletions still comes due.
#[tokio::test]
async fn a_permanently_unreadable_key_never_postpones_the_full_pull_forever() {
    let shell = signed_in();
    script(&shell, &["search"], ok(fixture("search.json")));
    script(&shell, &["view", "SP-42"], ok(fixture("issue.json")));
    script(
        &shell,
        &["view", "SP-1"],
        ok(issue("SP-1", "Payroll epic", "Done", "done")),
    );
    script(
        &shell,
        &["view", "SP-77"],
        refused(
            "✗ Error: Field 'customfield_10102' does not exist or you do not have permission to view it",
        ),
    );
    let result = backend(shell)
        .pull(&board_with(jira_settings()), Some("2026-09-06T10:00:00Z|9"))
        .await
        .unwrap();
    assert!(result.full, "the tenth pull is the full one");
    assert_eq!(result.failed_keys.len(), 1);
    assert_eq!(result.cursor.as_deref(), Some("2026-09-06T10:00:00Z|0"));
}

/// A cursor nobody can read is the same case as no cursor at all: `parse_cursor` answers both
/// with the invented epoch watermark. Persisting it under a failed key would have sent
/// `updated >= "-~29000000m"` on every later pull and spent the counter that says a full pull
/// is overdue, so deletions would go unnoticed for the next nine syncs.
#[tokio::test]
async fn an_unreadable_cursor_is_never_persisted_as_the_epoch_window() {
    let shell = signed_in();
    script(&shell, &["search"], ok(fixture("search.json")));
    script(&shell, &["view", "SP-42"], ok(fixture("issue.json")));
    script(
        &shell,
        &["view", "SP-1"],
        ok(issue("SP-1", "Payroll epic", "Done", "done")),
    );
    script(
        &shell,
        &["view", "SP-77"],
        refused(
            "✗ Error: Field 'customfield_10102' does not exist or you do not have permission to view it",
        ),
    );
    let result = backend(shell)
        .pull(&board_with(jira_settings()), Some("not-a-cursor"))
        .await
        .unwrap();
    assert!(
        result.full,
        "an unresumable cursor is overdue for a full pull"
    );
    assert_eq!(result.failed_keys.len(), 1);
    assert_eq!(result.cursor, None);
}

/// Even a message that does read as a deletion cannot mean *every* issue at once.
#[tokio::test]
async fn a_pull_whose_every_key_is_missing_refuses_to_call_it_a_deletion() {
    let shell = signed_in();
    script(&shell, &["search"], ok(fixture("search.json")));
    script(
        &shell,
        &["view"],
        refused("✗ Error: Issue does not exist or you do not have permission to see it"),
    );
    let error = backend(shell)
        .pull(&board_with(jira_settings()), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("came back missing"), "{error}");
}

/// A linked, dirty card with a label and an assignee, as `reconcile` would hand it over.
fn linked_card(board: &mut Board) -> Card {
    board.labels = vec![
        Label {
            id: "payroll".parse().unwrap(),
            name: "payroll".into(),
            color: None,
        },
        Label {
            id: "urgent".parse().unwrap(),
            name: "urgent".into(),
            color: None,
        },
    ];
    let mut card = create_card(
        board,
        &[],
        "11111111-1111-4111-8111-111111111111".parse().unwrap(),
        CardDraft {
            title: "Rework the payroll export".into(),
            description: "One line".into(),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    card.labels = vec!["urgent".parse().unwrap()];
    card.assignee = Some("Danny Fuentes".into());
    card.dirty = true;
    card.remote = Some(RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "SP-42".into(),
        url: Some("https://buk.atlassian.net/browse/SP-42".into()),
        version: Some("2026-09-05T14:32:11.000-0300".into()),
        synced_at: NOW.into(),
        remote_updated_at: Some("2026-09-05T14:32:11-03:00".into()),
    });
    card
}

#[tokio::test]
async fn an_update_writes_only_what_edit_can_write_and_diffs_the_labels() {
    let shell = signed_in();
    // The issue currently carries `payroll`; the card carries `urgent`.
    script_view(
        &shell,
        "SP-42",
        "labels",
        ok("{\"fields\":{\"labels\":[\"payroll\"]}}"),
    );
    script_view(
        &shell,
        "SP-42",
        "updated",
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    script_view(&shell, "SP-42", &pull_fields(), ok(fixture("issue.json")));
    script(&shell, &["search"], ok("[{\"key\":\"SP-42\"}]"));
    script(&shell, &["edit"], ok("{\"key\":\"SP-42\"}"));
    let mut board = board_with(jira_settings());
    let card = linked_card(&mut board);
    // `--assignee` takes an account id, and the only place one ever appears is a pulled
    // issue: the cache the pull filled is what makes the display name pushable.
    let backend = backend(shell.clone());
    backend.pull(&board, None).await.unwrap();

    let result = backend
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::Update {
                card_id: card.id.clone(),
                // `priority` is read-only and can only arrive here from a remote change; it
                // is skipped rather than failing the operations that can be written.
                fields: vec![
                    "title".into(),
                    "description".into(),
                    "labels".into(),
                    "assignee".into(),
                    "priority".into(),
                ],
            }],
        )
        .await
        .unwrap();

    let calls = acli_calls(&shell);
    let edit = calls
        .iter()
        .find(|call| call.contains(&"edit".to_owned()))
        .expect("an edit must have been issued");
    // Writes are always driven by an explicit key, never by a JQL that could match more.
    assert_eq!(edit[..4], ["jira", "workitem", "edit", "--key"]);
    assert_eq!(edit[4], "SP-42");
    assert!(!edit.iter().any(|argument| argument == "--jql"));
    let flag = |name: &str| {
        edit.iter()
            .position(|argument| argument == name)
            .map(|index| edit[index + 1].clone())
    };
    assert_eq!(
        flag("--summary").as_deref(),
        Some("Rework the payroll export")
    );
    assert_eq!(flag("--labels").as_deref(), Some("urgent"));
    assert_eq!(flag("--remove-labels").as_deref(), Some("payroll"));
    // The description travels as an ADF file, and the file is gone by the time we look.
    let body = flag("--description-file").expect("a description must be staged");
    assert!(body.ends_with(".json"), "{body}");
    assert!(
        !std::path::Path::new(&body).exists(),
        "a staged body outlived its call"
    );
    assert_eq!(flag("--assignee").as_deref(), Some("5b10a2844c"));
    assert!(edit.contains(&"--yes".to_owned()) && edit.contains(&"--json".to_owned()));
    // Nothing tried to write the read-only field.
    assert!(!edit.iter().any(|argument| argument.contains("priority")));

    assert!(result.failures.is_empty());
    let ack = &result.acks[0];
    assert_eq!(ack.key, "SP-42");
    assert_eq!(
        ack.url.as_deref(),
        Some("https://buk.atlassian.net/browse/SP-42")
    );
    // The version is read back, because Jira's is the only one a later conflict check can use.
    assert_eq!(ack.version.as_deref(), Some("2026-09-06T09:00:00.000-0300"));
}

#[tokio::test]
async fn an_assignee_nobody_has_ever_seen_is_refused_before_jira_ignores_it() {
    let shell = signed_in();
    script(
        &shell,
        &["view", "SP-42"],
        ok("{\"fields\":{\"updated\":\"x\"}}"),
    );
    let mut board = board_with(jira_settings());
    let mut card = linked_card(&mut board);
    card.assignee = Some("Somebody Else".into());
    let result = backend(shell)
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::Update {
                card_id: card.id.clone(),
                fields: vec!["assignee".into()],
            }],
        )
        .await
        .unwrap();
    assert!(result.acks.is_empty());
    assert!(
        result.failures[0].error.contains("unknown assignee"),
        "{}",
        result.failures[0].error
    );
}

/// `users::scope` is the site when the board names one and the project key otherwise, so
/// `describe` and `push` have to compute it from the *same* settings. Filling the cache under
/// the raw ones and reading it under the located ones made the whole sample dead on every board
/// created without `--setting site=…`, which is documented as optional — and the push that
/// needed it failed with "unknown assignee", aborting the rest of that card's batch.
#[tokio::test]
async fn a_board_with_no_site_setting_still_pushes_the_people_its_description_sampled() {
    let shell = signed_in();
    script(&shell, &["project", "view"], ok(fixture("project.json")));
    script(&shell, &["search"], ok(fixture("sample.json")));
    script_view(
        &shell,
        "SP-42",
        "updated",
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    script(&shell, &["edit"], ok("{\"key\":\"SP-42\"}"));
    let mut board = board_with(serde_json::json!({"project": "SP"}));
    let mut card = linked_card(&mut board);
    card.assignee = Some("Danny Fuentes".into());
    let backend = backend(shell.clone());
    backend.describe(&board).await.unwrap();
    let result = backend
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::Update {
                card_id: card.id.clone(),
                fields: vec!["assignee".into()],
            }],
        )
        .await
        .unwrap();
    assert!(
        result.failures.is_empty(),
        "the sample must be readable by the push: {:?}",
        result.failures
    );
    let edit = acli_calls(&shell)
        .into_iter()
        .find(|call| call.contains(&"edit".to_owned()))
        .expect("an edit must have been issued");
    let assignee = edit
        .iter()
        .position(|argument| argument == "--assignee")
        .map(|index| edit[index + 1].clone());
    assert_eq!(assignee.as_deref(), Some("5b10a2844c"));
}

/// A version Jira could not spell as a timestamp is not one this board may persist: the pull
/// path refuses it for that reason, and the push read-back reaches the same stored field.
#[tokio::test]
async fn a_push_read_back_that_is_not_a_timestamp_leaves_no_remote_stamp_behind() {
    let shell = signed_in();
    script_view(
        &shell,
        "SP-42",
        "updated",
        ok("{\"fields\":{\"updated\":\"soon\"}}"),
    );
    script(&shell, &["edit"], ok("{\"key\":\"SP-42\"}"));
    let mut board = board_with(jira_settings());
    let card = linked_card(&mut board);
    let result = backend(shell)
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::Update {
                card_id: card.id.clone(),
                fields: vec!["title".into()],
            }],
        )
        .await
        .unwrap();
    let ack = &result.acks[0];
    // The version is opaque and compared for equality only, so it survives as it was.
    assert_eq!(ack.version.as_deref(), Some("soon"));
    // `remoteUpdatedAt` is published as RFC3339 by `board show --json`, so it stays absent.
    assert_eq!(ack.remote_updated_at, None);
}

/// Two comments filed in one batch, both answered without an id: `card` is the pre-push
/// snapshot, so the id the first one bound is invisible to the second, and both used to bind to
/// the same remote comment — the merge the fallback exists to avoid.
#[tokio::test]
async fn two_comments_answered_without_an_id_never_bind_to_the_same_one() {
    let shell = signed_in();
    script(&shell, &["comment", "create"], ok("{\"ok\":true}"));
    script(
        &shell,
        &["view", "SP-42", "comment"],
        ok(fixture("issue.json")),
    );
    script_view(
        &shell,
        "SP-42",
        "updated",
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    let mut board = board_with(jira_settings());
    let mut card = linked_card(&mut board);
    for (id, body) in [("first", "One"), ("second", "Two")] {
        add_comment(&mut card, id.into(), None, body.into(), NOW).unwrap();
    }
    let result = backend(shell)
        .push(
            &board,
            std::slice::from_ref(&card),
            &[
                PushOp::AddComment {
                    card_id: card.id.clone(),
                    comment_id: "first".into(),
                },
                PushOp::AddComment {
                    card_id: card.id.clone(),
                    comment_id: "second".into(),
                },
            ],
        )
        .await
        .unwrap();
    let bound: Vec<&str> = result.acks[0]
        .comment_ids
        .iter()
        .map(|(_, remote)| remote.as_str())
        .collect();
    assert_eq!(bound.len(), 2, "{bound:?}");
    assert_ne!(bound[0], bound[1], "two comments bound to one remote id");
}

#[tokio::test]
async fn a_refused_transition_becomes_a_push_failure_with_jira_s_own_words() {
    let shell = signed_in();
    script(
        &shell,
        &["transition"],
        refused("✗ Error: SP-42 can't be transitioned: No allowed transitions found"),
    );
    script(
        &shell,
        &["view", "SP-42"],
        ok("{\"fields\":{\"updated\":\"x\"}}"),
    );
    let mut board = board_with(jira_settings());
    let card = linked_card(&mut board);
    let result = backend(shell.clone())
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::Transition {
                card_id: card.id.clone(),
                remote_status: "Done".into(),
            }],
        )
        .await
        .unwrap();
    assert_eq!(
        acli_calls(&shell)[0],
        [
            "jira",
            "workitem",
            "transition",
            "--key",
            "SP-42",
            "--status",
            "Done",
            "--yes",
            "--json"
        ]
    );
    assert!(result.acks.is_empty());
    assert!(
        result.failures[0].error.contains("No allowed transitions"),
        "{}",
        result.failures[0].error
    );
}

/// Reads back every `create --from-json` body, keyed by the summary it carried.
///
/// The staged file is removed the moment the call returns, so the only place to read it is the
/// rule's own predicate, which runs while `acli` would have been running.
fn capture_create(
    shell: &FakeShell,
    summary: &'static str,
    key: &'static str,
) -> Arc<Mutex<Option<serde_json::Value>>> {
    let seen = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);
    shell.when(
        move |command| {
            if command.program != "acli"
                || !command.args.iter().any(|argument| argument == "create")
            {
                return false;
            }
            let Some(path) = command
                .args
                .iter()
                .find(|argument| argument.ends_with(".json"))
            else {
                return false;
            };
            let Ok(body) = std::fs::read_to_string(path) else {
                return false;
            };
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(&body) else {
                return false;
            };
            if payload["summary"] != summary {
                return false;
            }
            *sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(payload);
            true
        },
        ok(format!("{{\"key\":\"{key}\"}}")),
    );
    seen
}

/// `cards` is the pre-push snapshot, so a parent created in the same batch still reads as
/// unlinked there; without the keys this batch has already minted the hierarchy is dropped in
/// silence, and `parent_id` is read-only so no later `Update` can put it back.
#[tokio::test]
async fn a_parent_created_in_the_same_batch_is_still_the_child_s_parent() {
    let shell = signed_in();
    let epic = capture_create(&shell, "The epic", "SP-90");
    let task = capture_create(&shell, "The task", "SP-91");
    script(
        &shell,
        &["view", "SP-9"],
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    let mut settings = jira_settings();
    settings["issueType"] = serde_json::json!("Task");
    let mut board = board_with(settings);
    let mut parent = linked_card(&mut board);
    parent.id = "22222222-2222-4222-8222-222222222222".parse().unwrap();
    parent.remote = None;
    parent.assignee = None;
    parent.title = "The epic".into();
    let mut child = linked_card(&mut board);
    child.remote = None;
    child.assignee = None;
    child.title = "The task".into();
    child.parent_id = Some(parent.id.clone());

    // The child is queued first: the order inside a batch is the backend's problem to solve.
    let result = backend(shell.clone())
        .push(
            &board,
            &[parent.clone(), child.clone()],
            &[
                PushOp::Create {
                    card_id: child.id.clone(),
                },
                PushOp::Create {
                    card_id: parent.id.clone(),
                },
            ],
        )
        .await
        .unwrap();
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    assert!(
        epic.lock().unwrap().is_some(),
        "the parent must be created too"
    );
    let child_payload = task.lock().unwrap().clone().expect("the child was created");
    assert_eq!(
        child_payload["parentIssueId"], "SP-90",
        "the parent this batch just minted a key for is still the parent"
    );
}

/// A create carries no status, so the column a card was born in has to be pushed behind it —
/// but Jira has already filed the issue somewhere, and transitioning to the status it is
/// already in answers "no allowed transitions found" and fails a push with nothing left to do.
#[tokio::test]
async fn a_transition_behind_a_create_checks_the_status_the_issue_was_filed_in() {
    let already_there = signed_in();
    script(&already_there, &["create"], ok("{\"key\":\"SP-95\"}"));
    script_view(
        &already_there,
        "SP-95",
        "status",
        ok("{\"fields\":{\"status\":{\"name\":\"To Do\"}}}"),
    );
    script_view(
        &already_there,
        "SP-95",
        "updated",
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    let mut settings = jira_settings();
    settings["issueType"] = serde_json::json!("Task");
    let mut board = board_with(settings);
    let mut card = linked_card(&mut board);
    card.remote = None;
    card.assignee = None;
    let ops = [
        PushOp::Create {
            card_id: card.id.clone(),
        },
        PushOp::Transition {
            card_id: card.id.clone(),
            remote_status: "To Do".into(),
        },
    ];
    let result = backend(already_there.clone())
        .push(&board, std::slice::from_ref(&card), &ops)
        .await
        .unwrap();
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    assert!(
        !acli_calls(&already_there)
            .iter()
            .any(|call| call.contains(&"transition".to_owned())),
        "the issue is already there; there is nothing to move"
    );

    // A different column is moved for real.
    let elsewhere = signed_in();
    script(&elsewhere, &["create"], ok("{\"key\":\"SP-96\"}"));
    script_view(
        &elsewhere,
        "SP-96",
        "status",
        ok("{\"fields\":{\"status\":{\"name\":\"To Do\"}}}"),
    );
    script_view(
        &elsewhere,
        "SP-96",
        "updated",
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    script(&elsewhere, &["transition"], ok("{\"key\":\"SP-96\"}"));
    let moved = [
        PushOp::Create {
            card_id: card.id.clone(),
        },
        PushOp::Transition {
            card_id: card.id.clone(),
            remote_status: "Done".into(),
        },
    ];
    let result = backend(elsewhere.clone())
        .push(&board, std::slice::from_ref(&card), &moved)
        .await
        .unwrap();
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    let transition = acli_calls(&elsewhere)
        .into_iter()
        .find(|call| call.contains(&"transition".to_owned()))
        .expect("the column the card was born in is pushed");
    assert!(transition.contains(&"Done".to_owned()), "{transition:?}");
}

/// A filter the backend rewrites before it reads it must be stored rewritten, or the settings
/// dialog and `board show --json` keep showing a clause every search has been ignoring.
#[tokio::test]
async fn the_stored_filter_is_the_one_the_searches_actually_run() {
    let normalized = backend(signed_in())
        .normalize(&serde_json::json!({
            "project": "SP",
            "jql": "labels = backend ORDER BY rank ASC",
            "maxConcurrency": 2
        }))
        .await
        .unwrap();
    assert_eq!(normalized["jql"], "labels = backend");
    // Only the keys the caller supplied: filling defaults in would turn every unset row of the
    // settings dialog into a value the user never chose.
    assert_eq!(normalized["maxConcurrency"], 2);
    assert_eq!(normalized.as_object().unwrap().len(), 3);
    // A filter that was nothing but an ordering is no filter at all.
    let empty = backend(signed_in())
        .normalize(&serde_json::json!({"project": "SP", "jql": "ORDER BY rank ASC"}))
        .await
        .unwrap();
    assert_eq!(empty.get("jql"), None);
}

#[tokio::test]
async fn a_create_stages_its_own_json_and_acknowledges_the_key_url_and_version() {
    let shell = signed_in();
    script(&shell, &["create"], ok("{\"key\":\"SP-99\"}"));
    script(
        &shell,
        &["view", "SP-99"],
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    let mut settings = jira_settings();
    settings["issueType"] = serde_json::json!("Task");
    let mut board = board_with(settings);
    let mut parent = linked_card(&mut board);
    parent.id = "22222222-2222-4222-8222-222222222222".parse().unwrap();
    let mut card = linked_card(&mut board);
    card.remote = None;
    card.assignee = None;
    card.parent_id = Some(parent.id.clone());
    card.title = "Brand new".into();

    let result = backend(shell.clone())
        .push(
            &board,
            &[parent, card.clone()],
            &[PushOp::Create {
                card_id: card.id.clone(),
            }],
        )
        .await
        .unwrap();
    let create = &acli_calls(&shell)[0];
    assert_eq!(create[..4], ["jira", "workitem", "create", "--from-json"]);
    assert!(create[4].ends_with(".json"));
    assert!(!std::path::Path::new(&create[4]).exists());
    let ack = &result.acks[0];
    assert_eq!(ack.key, "SP-99");
    assert_eq!(
        ack.url.as_deref(),
        Some("https://buk.atlassian.net/browse/SP-99")
    );
    assert_eq!(ack.version.as_deref(), Some("2026-09-06T09:00:00.000-0300"));

    // A board with no issue type cannot create anything, and says which setting is missing.
    let mut board = board_with(jira_settings());
    let mut card = linked_card(&mut board);
    card.remote = None;
    let result = backend(signed_in())
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::Create {
                card_id: card.id.clone(),
            }],
        )
        .await
        .unwrap();
    assert!(
        result.failures[0].error.contains("issueType"),
        "{}",
        result.failures[0].error
    );
}

/// A batch that half landed has to say so: dropping the ack for the comment that was posted
/// posts it again on the next sync, and dropping the version an edit produced turns the user's
/// own change into a conflict.
#[tokio::test]
async fn a_batch_that_fails_halfway_still_acknowledges_what_landed() {
    let shell = signed_in();
    script(&shell, &["comment", "create"], ok("{\"id\":\"10777\"}"));
    script(
        &shell,
        &["transition"],
        refused("✗ Error: SP-42 can't be transitioned: No allowed transitions found"),
    );
    script(
        &shell,
        &["view", "SP-42", "updated"],
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    let mut board = board_with(jira_settings());
    let mut card = linked_card(&mut board);
    add_comment(
        &mut card,
        "local-comment".into(),
        None,
        "Picking this up".into(),
        NOW,
    )
    .unwrap();
    let result = backend(shell)
        .push(
            &board,
            std::slice::from_ref(&card),
            &[
                PushOp::AddComment {
                    card_id: card.id.clone(),
                    comment_id: "local-comment".into(),
                },
                PushOp::Transition {
                    card_id: card.id.clone(),
                    remote_status: "Done".into(),
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        result.acks[0].comment_ids,
        [("local-comment".to_owned(), "10777".to_owned())],
        "the comment that was posted must never be posted twice"
    );
    assert_eq!(
        result.acks[0].version.as_deref(),
        Some("2026-09-06T09:00:00.000-0300")
    );
    // Both halves of the answer, for the same card.
    assert_eq!(result.acks[0].card_id, card.id);
    assert_eq!(result.failures[0].card_id, card.id);
    assert!(
        result.failures[0].error.contains("No allowed transitions"),
        "{}",
        result.failures[0].error
    );
    // Jira's sentence, with no `backend error:` in front of it.
    assert!(!result.failures[0].error.contains("backend error"));
}

/// A one-word display name is not an account handle: Jira would reject it or drop it silently.
#[tokio::test]
async fn a_display_name_that_is_not_an_account_handle_is_refused() {
    let shell = signed_in();
    script(
        &shell,
        &["view", "SP-42"],
        ok("{\"fields\":{\"updated\":\"x\"}}"),
    );
    let mut board = board_with(jira_settings());
    let mut card = linked_card(&mut board);
    card.assignee = Some("dfuentes".into());
    let result = backend(shell)
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::Update {
                card_id: card.id.clone(),
                fields: vec!["assignee".into()],
            }],
        )
        .await
        .unwrap();
    assert!(result.acks.is_empty());
    assert!(
        result.failures[0].error.contains("unknown assignee"),
        "{}",
        result.failures[0].error
    );

    // An email or an account id is handed over as it stands.
    for handle in [
        "danny@example.com",
        "712020:8f6c1e2a-0b3d-4c5e-9a7b-1d2e3f4a5b6c",
    ] {
        let shell = signed_in();
        script(&shell, &["edit"], ok("{}"));
        script(
            &shell,
            &["view", "SP-42"],
            ok("{\"fields\":{\"updated\":\"x\"}}"),
        );
        let mut board = board_with(jira_settings());
        let mut card = linked_card(&mut board);
        card.assignee = Some(handle.to_owned());
        let result = backend(shell.clone())
            .push(
                &board,
                std::slice::from_ref(&card),
                &[PushOp::Update {
                    card_id: card.id.clone(),
                    fields: vec!["assignee".into()],
                }],
            )
            .await
            .unwrap();
        assert!(
            result.failures.is_empty(),
            "{handle}: {:?}",
            result.failures
        );
        assert!(
            acli_calls(&shell)
                .iter()
                .any(|call| call.contains(&handle.to_owned())),
            "{handle} was never passed to --assignee"
        );
    }
}

#[tokio::test]
async fn a_comment_is_published_and_matched_back_to_its_remote_id() {
    let shell = signed_in();
    script(&shell, &["comment", "create"], ok("{\"id\":\"10777\"}"));
    script(
        &shell,
        &["view", "SP-42", "updated"],
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    let mut board = board_with(jira_settings());
    let mut card = linked_card(&mut board);
    add_comment(
        &mut card,
        "local-comment".into(),
        Some("Danny".into()),
        "Picking this up".into(),
        NOW,
    )
    .unwrap();
    let result = backend(shell.clone())
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::AddComment {
                card_id: card.id.clone(),
                comment_id: "local-comment".into(),
            }],
        )
        .await
        .unwrap();
    let call = &acli_calls(&shell)[0];
    assert_eq!(
        call[..6],
        ["jira", "workitem", "comment", "create", "--key", "SP-42"]
    );
    assert_eq!(call[6], "--body-file");
    assert!(!std::path::Path::new(&call[7]).exists());
    assert_eq!(
        result.acks[0].comment_ids,
        [("local-comment".to_owned(), "10777".to_owned())]
    );
}

#[tokio::test]
async fn a_comment_whose_id_jira_withheld_is_recovered_from_the_issue() {
    let shell = signed_in();
    script(&shell, &["comment", "create"], ok("{\"ok\":true}"));
    script(
        &shell,
        &["view", "SP-42", "comment"],
        ok(fixture("issue.json")),
    );
    script(
        &shell,
        &["view", "SP-42", "updated"],
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    let mut board = board_with(jira_settings());
    let mut card = linked_card(&mut board);
    add_comment(
        &mut card,
        "local-comment".into(),
        None,
        "Schema landed; picking this up.".into(),
        NOW,
    )
    .unwrap();
    let result = backend(shell)
        .push(
            &board,
            std::slice::from_ref(&card),
            &[PushOp::AddComment {
                card_id: card.id.clone(),
                comment_id: "local-comment".into(),
            }],
        )
        .await
        .unwrap();
    // A local comment with no remote id is published again on the next sync, so the id has
    // to be recovered even when the write itself did not report one.
    assert_eq!(
        result.acks[0].comment_ids,
        [("local-comment".to_owned(), "10502".to_owned())]
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    clock: Arc<FixedClock>,
    shell: Arc<FakeShell>,
    jobs: Arc<JobManager>,
    boards: Boards,
    /// Flipped by the conflict test to make Jira answer with an issue somebody else edited.
    renamed: Arc<std::sync::atomic::AtomicBool>,
}

impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = FleetHome::new(temp.path().join("fleet"));
        let files = Arc::new(RealFiles::new(
            home.trash_dir(),
            [home.boards_dir(), home.repos_dir(), home.worktrees_dir()],
        ));
        let clock = clock();
        let state = Arc::new(StateStore::new(
            temp.path().join("fleet"),
            files.clone(),
            clock.clone(),
        ));
        let mut initial = default_state();
        initial.contexts.push(Context {
            id: "work".parse().unwrap(),
            name: "Work".into(),
            owners: vec!["acme".into()],
            created_at: NOW.into(),
        });
        state.save(initial).await.unwrap();
        let config = Arc::new(ConfigStore::new(temp.path().join("fleet"), files.clone()));
        config
            .update(serde_json::json!({
                "reposDir": home.repos_dir(),
                "worktreesDir": home.worktrees_dir(),
                "hotPoolSize": 0,
                "hotRefreshIntervalMs": 0
            }))
            .await
            .unwrap();
        let jobs = Arc::new(JobManager::with_clock(
            temp.path().join("fleet"),
            clock.clone(),
        ));
        let shell = signed_in();
        shell.when(|command| command.program == "git", ok("fixture-sha\n"));
        let real_shell: Arc<dyn Shell> = shell.clone();
        let adapters = Adapters {
            board_backends: BoardBackends::system(Arc::clone(&real_shell), clock.clone()),
            git: Arc::new(FakeGit::new(shell.clone())),
            github: Arc::new(GhCli::new(Arc::clone(&real_shell))),
            process: Arc::new(RealProcess::new(Arc::clone(&real_shell))),
            files: files.clone(),
            shell: real_shell,
            clock: clock.clone(),
        };
        let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
        let worktrees = Arc::new(Worktrees::new(
            config,
            state.clone(),
            jobs.clone(),
            &adapters,
            sessions,
        ));
        let store = Arc::new(BoardStore::new(home.clone(), files));
        let boards = Boards::new(
            store,
            state,
            BoardBackends::new(vec![
                Arc::new(LocalBackend),
                Arc::new(JiraBackend::new(shell.clone(), clock.clone())),
            ]),
            clock.clone(),
            jobs.clone(),
            worktrees,
            BroadcastBus::default(),
        );
        Self {
            _temp: temp,
            clock,
            shell,
            jobs,
            boards,
            renamed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// A Jira board nothing has scripted an answer for beyond signing in.
    async fn bare_board(&self) -> Board {
        self.boards
            .create(
                &"work".parse().unwrap(),
                None,
                None,
                Some(BackendRef {
                    kind: "jira".into(),
                    settings: jira_settings(),
                }),
            )
            .await
            .unwrap()
            .board
    }

    /// A Jira board whose `acli` answers the fixtures a sync needs.
    async fn board(&self) -> Board {
        script(
            &self.shell,
            &["ORDER BY updated DESC"],
            ok(fixture("sample.json")),
        );
        script(
            &self.shell,
            &["project", "view"],
            ok(fixture("project.json")),
        );
        script(&self.shell, &["search"], ok(fixture("search.json")));
        // Registered first, so flipping the switch is what changes Jira's answer.
        let renamed = self.renamed.clone();
        let fields = pull_fields();
        self.shell.when(
            move |command| {
                views(command, "SP-42", &fields)
                    && renamed.load(std::sync::atomic::Ordering::SeqCst)
            },
            ok(renamed_issue()),
        );
        script_view(
            &self.shell,
            "SP-42",
            &pull_fields(),
            ok(fixture("issue.json")),
        );
        script_view(
            &self.shell,
            "SP-1",
            &pull_fields(),
            ok(issue("SP-1", "Payroll epic", "Done", "done")),
        );
        script_view(
            &self.shell,
            "SP-77",
            &pull_fields(),
            ok(issue("SP-77", "Retire the old exporter", "Backlog", "new")),
        );
        self.boards
            .create(
                &"work".parse().unwrap(),
                None,
                None,
                Some(BackendRef {
                    kind: "jira".into(),
                    settings: jira_settings(),
                }),
            )
            .await
            .unwrap()
            .board
    }

    async fn sync(&self, board: &BoardId, full: bool) -> JobStatus {
        let id = self.boards.sync(board, full).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), self.jobs.wait(&id))
            .await
            .unwrap()
            .unwrap()
            .status
    }

    fn card(&self, view: &BoardView, key: &str) -> Card {
        view.cards
            .iter()
            .find(|card| card.remote.as_ref().is_some_and(|link| link.key == key))
            .unwrap_or_else(|| panic!("no card for {key}"))
            .clone()
    }
}

#[tokio::test]
async fn a_jira_sync_adopts_the_columns_it_sampled_and_imports_every_issue() {
    let f = Fixture::new().await;
    let board = f.board().await;
    assert_eq!(f.sync(&board.id, false).await, JobStatus::Succeeded);

    let view = f.boards.get(&board.id).await.unwrap();
    assert_eq!(
        view.board
            .statuses
            .iter()
            .map(|status| status.name.as_str())
            .collect::<Vec<_>>(),
        ["Backlog", "In Progress", "Done", "Cancelled"]
    );
    assert_eq!(view.cards.len(), 3);
    let card = f.card(&view, "SP-42");
    assert_eq!(card.title, "Rework the payroll export");
    assert_eq!(
        view.board
            .statuses
            .iter()
            .find(|status| status.id == card.status_id)
            .unwrap()
            .name,
        "In Progress"
    );
    assert_eq!(card.priority, Priority::High);
    assert_eq!(card.estimate, Some(3));
    assert_eq!(card.due_date.as_deref(), Some("2026-09-30"));
    assert_eq!(
        card.properties.get("jira.issue_type"),
        Some(&PropertyValue::Select("Task".into()))
    );
    assert_eq!(card.comments.len(), 2);
    // The parent arrived in the same pull, keyed by the remote key.
    assert_eq!(card.parent_id, Some(f.card(&view, "SP-1").id));
    assert_eq!(
        card.remote.as_ref().unwrap().url.as_deref(),
        Some("https://buk.atlassian.net/browse/SP-42")
    );
    // The backend's refusal list is what the core enforces from here on.
    assert_eq!(
        view.board.sync.readonly_fields,
        ["priority", "estimate", "due_date", "parent_id"]
    );
    assert!(
        view.board
            .sync
            .cursor
            .as_deref()
            .unwrap()
            .starts_with("2026-09-06T12:00:00Z|"),
        "{:?}",
        view.board.sync.cursor
    );
}

/// `project view` only ever supplied option *labels*; a call that fails must not take the
/// stored values with it.
#[tokio::test]
async fn a_failed_project_read_does_not_delete_the_issue_type_of_every_card() {
    let f = Fixture::new().await;
    let broken = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = broken.clone();
    // Registered before the fixture's own rule, so flipping the switch is what breaks it.
    f.shell.when(
        move |command| {
            command.program == "acli"
                && command.args.iter().any(|argument| argument == "project")
                && flag.load(Ordering::SeqCst)
        },
        refused("✗ Error: something broke"),
    );
    let board = f.board().await;
    assert_eq!(f.sync(&board.id, false).await, JobStatus::Succeeded);
    assert_eq!(
        f.card(&f.boards.get(&board.id).await.unwrap(), "SP-42")
            .properties
            .get("jira.issue_type"),
        Some(&PropertyValue::Select("Task".into()))
    );
    broken.store(true, Ordering::SeqCst);
    assert_eq!(f.sync(&board.id, false).await, JobStatus::Succeeded);
    assert_eq!(
        f.card(&f.boards.get(&board.id).await.unwrap(), "SP-42")
            .properties
            .get("jira.issue_type"),
        Some(&PropertyValue::Select("Task".into())),
        "one failed metadata call must not erase a stored property"
    );
}

#[tokio::test]
async fn editing_a_read_only_field_on_a_jira_board_is_refused_with_the_field_named() {
    let f = Fixture::new().await;
    let board = f.board().await;
    f.sync(&board.id, false).await;
    let card = f.card(&f.boards.get(&board.id).await.unwrap(), "SP-42");

    for (patch, field) in [
        (
            CardPatch {
                priority: Some(Priority::Low),
                ..Default::default()
            },
            "priority",
        ),
        (
            CardPatch {
                due_date: Some(Some("2026-10-01".into())),
                ..Default::default()
            },
            "due_date",
        ),
    ] {
        let error = f
            .boards
            .update_card(&card.id, patch)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(field) && error.contains("read-only"),
            "{error}"
        );
    }
    // What `acli jira workitem edit` *can* write is still writable.
    f.boards
        .update_card(
            &card.id,
            CardPatch {
                title: Some("Rework the payroll export twice".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_local_edit_a_move_and_a_comment_reach_jira_as_edit_transition_and_comment_create() {
    let f = Fixture::new().await;
    let board = f.board().await;
    f.sync(&board.id, false).await;
    let view = f.boards.get(&board.id).await.unwrap();
    let card = f.card(&view, "SP-42");
    let done = view
        .board
        .statuses
        .iter()
        .find(|status| status.name == "Done")
        .unwrap()
        .id
        .clone();

    f.boards
        .update_card(
            &card.id,
            CardPatch {
                title: Some("Rework the payroll export properly".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.boards.move_card(&card.id, &done, None).await.unwrap();
    f.boards
        .add_comment(&card.id, "Pushing this now".into())
        .await
        .unwrap();

    script(&f.shell, &["edit"], ok("{\"key\":\"SP-42\"}"));
    script(&f.shell, &["transition"], ok("{\"key\":\"SP-42\"}"));
    script(&f.shell, &["comment", "create"], ok("{\"id\":\"10999\"}"));
    script(
        &f.shell,
        &["view", "SP-42", "updated"],
        ok("{\"fields\":{\"updated\":\"2026-09-06T09:00:00.000-0300\"}}"),
    );
    f.clock
        .set(Utc.with_ymd_and_hms(2026, 9, 6, 13, 0, 0).single().unwrap());
    assert_eq!(f.sync(&board.id, false).await, JobStatus::Succeeded);

    let calls = acli_calls(&f.shell);
    let named = |name: &str| {
        calls
            .iter()
            .find(|call| call.contains(&name.to_owned()))
            .unwrap_or_else(|| panic!("no {name} call in {calls:?}"))
            .clone()
    };
    let edit = named("edit");
    assert_eq!(edit[3], "--key");
    assert_eq!(edit[4], "SP-42");
    assert_eq!(
        edit.iter()
            .position(|argument| argument == "--summary")
            .map(|i| edit[i + 1].clone()),
        Some("Rework the payroll export properly".to_owned())
    );
    let transition = named("transition");
    assert_eq!(transition[5], "--status");
    // Transitions move by status *name*: the map the schema adopted is keyed on it.
    assert_eq!(transition[6], "Done");
    let comment = named("comment");
    assert_eq!(comment[4], "--key");
    assert_eq!(comment[5], "SP-42");

    let card = f.card(&f.boards.get(&board.id).await.unwrap(), "SP-42");
    assert!(!card.dirty, "an acknowledged push leaves nothing pending");
    assert_eq!(
        card.comments.last().unwrap().remote_id.as_deref(),
        Some("10999")
    );
}

#[tokio::test]
async fn a_dirty_card_whose_issue_changed_underneath_becomes_a_conflict() {
    let f = Fixture::new().await;
    let board = f.board().await;
    f.sync(&board.id, false).await;
    let card = f.card(&f.boards.get(&board.id).await.unwrap(), "SP-42");
    f.boards
        .update_card(
            &card.id,
            CardPatch {
                title: Some("Locally renamed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Somebody edited the same issue in Jira between the two syncs.
    f.renamed.store(true, std::sync::atomic::Ordering::SeqCst);
    f.clock
        .set(Utc.with_ymd_and_hms(2026, 9, 6, 13, 0, 0).single().unwrap());
    f.sync(&board.id, false).await;

    let card = f.card(&f.boards.get(&board.id).await.unwrap(), "SP-42");
    let conflict = card
        .conflict
        .as_ref()
        .expect("two edits to one field is a decision the user has to make");
    assert!(
        conflict.fields.contains(&"title".to_owned()),
        "{:?}",
        conflict.fields
    );
    assert_eq!(conflict.remote.title, "Renamed in Jira");
    assert_eq!(card.title, "Locally renamed");
}

#[tokio::test]
async fn a_full_sync_ignores_the_cursor_the_last_one_left() {
    let f = Fixture::new().await;
    let board = f.board().await;
    f.sync(&board.id, false).await;
    assert!(
        f.boards
            .get(&board.id)
            .await
            .unwrap()
            .board
            .sync
            .cursor
            .is_some()
    );

    let before = acli_calls(&f.shell).len();
    f.clock
        .set(Utc.with_ymd_and_hms(2026, 9, 6, 13, 0, 0).single().unwrap());
    assert_eq!(f.sync(&board.id, true).await, JobStatus::Succeeded);
    let search = acli_calls(&f.shell)[before..]
        .iter()
        .find(|call| call.contains(&"--paginate".to_owned()))
        .expect("a pull always searches")
        .clone();
    // No window at all: a full sync is the complete remote set, which is the only pull that
    // can notice an issue that was deleted or filtered out.
    assert_eq!(search[4], "project = \"SP\" ORDER BY updated ASC");
}

/// A board whose `acli` cannot answer fails the sync loudly, rather than reporting an empty
/// remote and archiving every card on it.
#[tokio::test]
async fn a_sync_that_cannot_reach_jira_fails_instead_of_emptying_the_board() {
    let f = Fixture::new().await;
    let board = f.bare_board().await;
    let id = f.boards.sync(&board.id, false).await.unwrap();
    let record = tokio::time::timeout(Duration::from_secs(10), f.jobs.wait(&id))
        .await
        .unwrap()
        .unwrap();
    let JobStatus::Failed { error } = record.status else {
        panic!("a board that cannot reach Jira must not report a successful sync");
    };
    assert!(error.contains("install the Atlassian CLI"), "{error}");
    assert!(matches!(
        f.boards.describe_backend(&board.id).await,
        Err(DaemonError::Shell(_))
    ));
}
