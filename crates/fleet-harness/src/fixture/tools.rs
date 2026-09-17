//! The network-facing executables a fixture shadows, and the data they answer from.
//!
//! Every one of these is a `/bin/sh` script written into [`HarnessEnv::fake_bin`], which is
//! the first entry on the child `PATH`. They exist so a scenario can photograph the pull
//! request, board and agent surfaces without GitHub, Atlassian or a vendor CLI — and so a run
//! that *would* have reached the network fails loudly on a fixture file that is missing
//! rather than quietly on the developer's own credentials.
//!
//! The queries each script answers are exactly the ones the daemon's adapters issue:
//! `crates/fleet-daemon/src/adapters/github.rs` for `gh`,
//! `crates/fleet-daemon/src/adapters/board/jira/acli.rs` for `acli`.

use super::plan::{Agent, Fixture, PrTab, PullRequest, Repository};
use crate::env::HarnessEnv;
use anyhow::Context as _;
use std::path::{Path, PathBuf};

/// The login the fake `gh` reports as the authenticated viewer.
const VIEWER: &str = "fleet-test";

/// Installs every fake executable the fixture needs and writes the data they read.
///
/// Returns the transcript files written for the preset's agents, in provider order.
pub fn install(
    environment: &HarnessEnv,
    fixture: &Fixture,
    workspace: &Path,
) -> anyhow::Result<Vec<PathBuf>> {
    let data = workspace.join("tools");
    std::fs::create_dir_all(&data).with_context(|| format!("create {}", data.display()))?;
    install_gh(environment, fixture, &data)?;
    install_acli(environment, &data)?;
    install_agents(environment, fixture, &data)
}

/// Writes the pull-request and repository answers, then the `gh` that reads them.
fn install_gh(environment: &HarnessEnv, fixture: &Fixture, data: &Path) -> anyhow::Result<PathBuf> {
    let mut owners: std::collections::BTreeMap<&str, Vec<serde_json::Value>> =
        std::collections::BTreeMap::new();
    for repository in &fixture.repositories {
        owners
            .entry(repository.owner.as_str())
            .or_default()
            .push(remote_repo(repository));
        for tab in [PrTab::Mine, PrTab::Review] {
            let rows: Vec<serde_json::Value> = repository
                .pull_requests
                .iter()
                .filter(|pull| pull.tab == tab)
                .map(|pull| pull_request(repository, pull))
                .collect();
            write_json(
                &data.join(format!("pr-list-{}-{}.json", key(repository), tab.key())),
                &serde_json::Value::Array(rows),
            )?;
        }
        for pull in &repository.pull_requests {
            let row = pull_request(repository, pull);
            write_json(
                &data.join(format!("pr-head-{}-{}.json", key(repository), pull.head)),
                &serde_json::Value::Array(vec![row.clone()]),
            )?;
            write_json(
                &data.join(format!("pr-view-{}-{}.json", key(repository), pull.number)),
                &row,
            )?;
        }
    }
    for (owner, repos) in owners {
        write_json(
            &data.join(format!("repos-{owner}.json")),
            &serde_json::Value::Array(repos),
        )?;
    }
    let script = tool_script(GH, data)?.replace("@VIEWER@", VIEWER);
    environment.install_fake("gh", &script)
}

/// Writes the Jira answers, then the `acli` that reads them.
///
/// The `board` preset itself uses Fleet's own local backend, so nothing in a default run
/// reaches this script. It exists so a scenario that points a board at the Jira backend gets
/// an authenticated-looking CLI answering from fixture data in milliseconds, instead of a
/// ninety-second timeout against a site the run cannot see.
fn install_acli(environment: &HarnessEnv, data: &Path) -> anyhow::Result<PathBuf> {
    write_json(
        &data.join("project.json"),
        &serde_json::json!({
            "key": "FLT",
            "name": "Fleet",
            "issueTypes": [{"name": "Task"}, {"name": "Bug"}]
        }),
    )?;
    write_json(
        &data.join("workitems.json"),
        &serde_json::json!({"issues": []}),
    )?;
    environment.install_fake("acli", &tool_script(ACLI, data)?)
}

/// Writes each provider's transcript and the shim that serves it.
fn install_agents(
    environment: &HarnessEnv,
    fixture: &Fixture,
    data: &Path,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for agent in &fixture.agents {
        let transcript = data.join(format!("{}-transcript.json", agent.provider.flag()));
        write_json(&transcript, &agent.transcript)?;
        environment.install_fake(agent.provider.executable(), &shim(agent, &transcript)?)?;
        written.push(transcript);
    }
    Ok(written)
}

/// The `/bin/sh` shim the seeded `agentCommands` **and** `agentBinaries` entries point at,
/// installed under the *vendor's* name so it also shadows a real `claude` or `codex` on the
/// child `PATH`.
///
/// The script is `crate::agent::launcher_script`'s, not one of this module's own: it has to
/// answer the daemon's `<command> --version` probe with a version the probe accepts
/// (`agents::harness::probe` refuses a Claude older than 2.1) and it has to *drop* the vendor
/// launch flags the adapter appends rather than forward them to a `clap` parser that rejects
/// them. Both were got wrong here once; there is now one copy.
fn shim(agent: &Agent, transcript: &Path) -> anyhow::Result<String> {
    crate::agent::launcher_script(agent.provider.player(), transcript)
}

fn key(repository: &Repository) -> String {
    format!("{}-{}", repository.owner, repository.name)
}

/// One row of `gh repo list <owner> --json name,owner,nameWithOwner,…`.
fn remote_repo(repository: &Repository) -> serde_json::Value {
    serde_json::json!({
        "name": repository.name,
        "owner": {"login": repository.owner},
        "nameWithOwner": repository.slug(),
        "description": format!("The {} fixture repository.", repository.name),
        "sshUrl": format!("git@github.com:{}.git", repository.slug()),
        "isPrivate": false,
        "updatedAt": "2026-01-01T00:00:00Z",
        "defaultBranchRef": {"name": super::git::DEFAULT_BRANCH}
    })
}

/// One row of `gh pr list --json …`, carrying the union of every field set the daemon asks
/// for, so one document answers the list, head and inspection queries alike. Unknown fields
/// are ignored by each of the adapter's three deserializers.
fn pull_request(repository: &Repository, pull: &PullRequest) -> serde_json::Value {
    serde_json::json!({
        "number": pull.number,
        "title": pull.title,
        "url": format!("https://github.com/{}/pull/{}", repository.slug(), pull.number),
        "author": {"login": pull.author},
        "headRefName": pull.head,
        "baseRefName": pull.base,
        "headRefOid": "0000000000000000000000000000000000000000",
        "state": "OPEN",
        "isDraft": pull.draft,
        "isCrossRepository": false,
        "headRepository": {"name": repository.name, "nameWithOwner": repository.slug()},
        "headRepositoryOwner": {"login": repository.owner},
        "reviewDecision": pull.review_decision,
        "statusCheckRollup": pull
            .checks
            .iter()
            .map(|conclusion| serde_json::json!({
                "conclusion": conclusion,
                "status": "COMPLETED",
                "state": conclusion
            }))
            .collect::<Vec<_>>(),
        "additions": pull.additions,
        "deletions": pull.deletions,
        "labels": pull
            .labels
            .iter()
            .map(|name| serde_json::json!({"name": name}))
            .collect::<Vec<_>>(),
        "updatedAt": "2026-01-01T00:00:00Z"
    })
}

fn write_json(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    let body = serde_json::to_string(value).context("serialize fixture tool data")?;
    std::fs::write(path, body).with_context(|| format!("write {}", path.display()))
}

/// Substitutes the fixture-data directory as one shell word.
fn tool_script(template: &str, data: &Path) -> anyhow::Result<String> {
    let data = crate::agent::shell_word(data)?;
    Ok(template.replace("@DATA@", &data))
}

/// A `gh` that answers the daemon's exact queries from fixture data and nothing else.
const GH: &str = r#"#!/bin/sh
# Fake gh for the Fleet harness: answers from fixture data, never reaches the network.
set -u
data=@DATA@

emit() {
  if [ -f "$1" ]; then cat "$1"; else printf '[]\n'; fi
  exit 0
}

case "${1:-}" in
  --version) echo 'gh version 0.0.0-fleet-test'; exit 0 ;;
  auth)
    echo 'github.com'
    echo "  ✓ Logged in to github.com account @VIEWER@"
    exit 0 ;;
esac

case "${1:-} ${2:-}" in
  'repo list') emit "$data/repos-${3:-}.json" ;;
  'pr view')
    number="${3:-}"
    repo=''
    shift 3
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --repo) repo="${2:-}"; shift ;;
      esac
      shift
    done
    emit "$data/pr-view-$(printf '%s' "$repo" | tr '/' '-')-$number.json" ;;
  'pr list')
    repo=''
    head=''
    tab='mine'
    shift 2
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --repo) repo="${2:-}"; shift ;;
        --head) head="${2:-}"; shift ;;
        --search) tab='review'; shift ;;
        --author) tab='mine'; shift ;;
      esac
      shift
    done
    slug=$(printf '%s' "$repo" | tr '/' '-')
    if [ -n "$head" ]; then
      emit "$data/pr-head-$slug-$head.json"
    fi
    emit "$data/pr-list-$slug-$tab.json" ;;
esac

printf '[]\n'
"#;

/// An `acli` that is always authenticated and always answers from fixture data.
const ACLI: &str = r#"#!/bin/sh
# Fake acli for the Fleet harness: answers from fixture data, never reaches Atlassian.
set -u
data=@DATA@

emit() {
  if [ -f "$1" ]; then cat "$1"; else printf '{}\n'; fi
  exit 0
}

if [ "${1:-}" = '--version' ]; then
  echo 'acli version 0.0.0-fleet-test'
  exit 0
fi
if [ "${1:-}" != 'jira' ]; then
  echo 'fake acli: only `acli jira` is implemented' >&2
  exit 2
fi
shift

case "${1:-} ${2:-}" in
  'auth status')
    echo 'Authenticated'
    echo 'Site: fleet.atlassian.test'
    echo 'Email: fleet-test@fleet.test'
    exit 0 ;;
  'project view') emit "$data/project.json" ;;
  'workitem search') emit "$data/workitems.json" ;;
  'workitem view') emit "$data/workitem-${3:-}.json" ;;
esac

printf '{}\n'
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_fake_tools_render_a_normal_data_path_as_one_quoted_word() {
        let data = Path::new("/tmp/fleet harness/fixture/tools");

        for (name, template) in [("gh", GH), ("acli", ACLI)] {
            let script = tool_script(template, data)
                .unwrap_or_else(|error| panic!("render the fake {name}: {error}"));
            assert!(
                script.contains("data='/tmp/fleet harness/fixture/tools'"),
                "the fake {name} must receive one quoted data path: {script}"
            );
        }
    }

    #[test]
    fn both_fake_tools_execute_with_an_apostrophe_in_their_data_path() {
        let temporary = tempfile::tempdir().expect("temporary fixture root");
        let data = temporary.path().join("operator's-run/fixture/tools");
        std::fs::create_dir_all(&data).expect("create the apostrophe data path");
        std::fs::write(data.join("repos-acme.json"), "[{\"name\":\"api\"}]\n")
            .expect("write gh fixture data");
        std::fs::write(data.join("project.json"), "{\"key\":\"FLT\"}\n")
            .expect("write acli fixture data");

        for (name, template, arguments, expected) in [
            (
                "gh",
                GH,
                &["repo", "list", "acme"][..],
                "[{\"name\":\"api\"}]",
            ),
            (
                "acli",
                ACLI,
                &["jira", "project", "view"][..],
                "{\"key\":\"FLT\"}",
            ),
        ] {
            let script = tool_script(template, &data)
                .unwrap_or_else(|error| panic!("render the fake {name}: {error}"));
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&script)
                .arg(name)
                .args(arguments)
                .output()
                .unwrap_or_else(|error| panic!("execute the fake {name} with sh -c: {error}"));
            assert!(output.status.success(), "the fake {name} must run");
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
        }
    }
}
