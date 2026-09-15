//! What each named preset asks for, as data.
//!
//! A preset is a *description*, never a procedure: [`Fixture`] says which repositories,
//! worktrees, pull requests, cards and agents a scenario needs, and the sibling modules turn
//! that description into a real git origin ([`super::git`]), real daemon state
//! ([`super::seed`]) and real fake executables ([`super::tools`]). Keeping the description
//! separate is what lets the determinism test compare two builds of the same preset, and what
//! lets a reader answer "what does `fixture: busy` mean?" by reading one function.

use super::Preset;
use serde::{Deserialize, Serialize};

/// The world one preset describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixture {
    /// The preset this description came from.
    pub preset: Preset,
    /// Display name of the single context every seeded preset uses.
    pub context: String,
    /// GitHub owners the context claims.
    pub owners: Vec<String>,
    /// Repositories, each of which becomes a real bare origin and a real daemon clone.
    pub repositories: Vec<Repository>,
    /// The board seeded through the daemon's board API, when the preset has one.
    pub board: Option<Board>,
    /// Native-agent providers pointed at a scripted transcript.
    pub agents: Vec<Agent>,
    /// Transcript files written for [`Fixture::agents`], filled in once they exist on disk.
    pub transcripts: Vec<std::path::PathBuf>,
}

impl Fixture {
    /// Whether the preset asks for anything a daemon has to write.
    ///
    /// `empty` is the only description that answers `false`: the hermetic home already *is* a
    /// Fleet that has never been run.
    #[must_use]
    pub fn is_seeded(&self) -> bool {
        !self.repositories.is_empty() || self.board.is_some() || !self.agents.is_empty()
    }

    /// The repository a job injection and the acceptance scenarios drive work against.
    #[must_use]
    pub fn primary_repository(&self) -> Option<&Repository> {
        self.repositories.first()
    }
}

/// One repository: a real bare origin, a real daemon clone, and what hangs off it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// GitHub owner.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Branches created in the origin beyond `main`.
    pub branches: Vec<String>,
    /// Worktrees the daemon publishes from the clone.
    pub worktrees: Vec<Worktree>,
    /// Pull requests the fake `gh` answers with for this repository.
    pub pull_requests: Vec<PullRequest>,
}

impl Repository {
    /// The `owner/name` the daemon knows this repository by.
    #[must_use]
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// One published worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// Worktree slug, which is also its branch name.
    pub slug: String,
    /// Base ref, or the repository's default branch when absent.
    pub base: Option<String>,
    /// Files written into the published worktree, which is how a fixture gets a dirty row.
    pub dirty_files: Vec<String>,
    /// What the post-create hook does, which is how a fixture gets a degraded row.
    pub hook: Hook,
}

impl Worktree {
    /// A clean worktree with no hooks.
    #[must_use]
    pub fn clean(slug: &str) -> Self {
        Self {
            slug: slug.to_owned(),
            base: None,
            dirty_files: Vec::new(),
            hook: Hook::None,
        }
    }

    /// A worktree carrying uncommitted changes.
    #[must_use]
    pub fn dirty(slug: &str, files: &[&str]) -> Self {
        Self {
            dirty_files: files.iter().map(|file| (*file).to_owned()).collect(),
            ..Self::clean(slug)
        }
    }

    /// A worktree whose post-create hook fails, leaving the row degraded.
    #[must_use]
    pub fn degraded(slug: &str) -> Self {
        Self {
            hook: Hook::Fails,
            ..Self::clean(slug)
        }
    }
}

/// What a worktree's post-create hook should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hook {
    /// No hook at all, so no `PostCreateHooks` job runs.
    None,
    /// A hook that exits zero.
    Succeeds,
    /// A hook that exits non-zero, so the worktree ends up degraded.
    Fails,
}

impl Hook {
    /// The shell commands the daemon runs after publishing the worktree.
    #[must_use]
    pub fn commands(self) -> Vec<String> {
        match self {
            Self::None => Vec::new(),
            Self::Succeeds => vec!["printf 'fixture hook ok\\n'".to_owned()],
            Self::Fails => vec!["printf 'fixture hook failed\\n'; exit 3".to_owned()],
        }
    }
}

/// One pull request the fake `gh` reports, in the fields the daemon's adapter reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    /// Pull-request number.
    pub number: u64,
    /// Title.
    pub title: String,
    /// Author login; `fleet-test` is the fake `gh`'s viewer.
    pub author: String,
    /// Head branch.
    pub head: String,
    /// Base branch.
    pub base: String,
    /// Whether the pull request is a draft.
    pub draft: bool,
    /// Which tab the pull request appears on.
    pub tab: PrTab,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or none.
    pub review_decision: Option<String>,
    /// Check conclusions, each `SUCCESS`, `FAILURE` or `PENDING`.
    pub checks: Vec<String>,
    /// Added lines.
    pub additions: u64,
    /// Removed lines.
    pub deletions: u64,
    /// Labels.
    pub labels: Vec<String>,
}

/// Which pull-request tab a fixture pull request belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrTab {
    /// Authored by the viewer.
    Mine,
    /// Awaiting the viewer's review.
    Review,
}

impl PrTab {
    /// The name the fake `gh`'s data files are keyed by.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::Review => "review",
        }
    }
}

/// A board seeded through the daemon's own board API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    /// Identifier prefix, e.g. `FLT` for `FLT-12`.
    pub prefix: String,
    /// Cards, in creation order.
    pub cards: Vec<Card>,
}

/// One card on a seeded board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Index into the board's own status list, clamped to the last column.
    pub column: usize,
}

/// A native-agent provider wired to a scripted transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// Which provider the shim stands in for.
    pub provider: Provider,
    /// The transcript document, in the frozen shape of `docs/TESTING-HARNESS.md` §5.
    pub transcript: serde_json::Value,
}

/// The two native-agent providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Anthropic Claude Code.
    Claude,
    /// OpenAI Codex.
    Codex,
}

impl Provider {
    /// The executable name `fleet_core::agents::AgentKind::executable` resolves on `PATH`.
    #[must_use]
    pub const fn executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The value `fleet-harness agent --provider` takes.
    #[must_use]
    pub const fn flag(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The scripted player this provider is served by.
    #[must_use]
    pub const fn player(self) -> crate::agent::Provider {
        match self {
            Self::Claude => crate::agent::Provider::Claude,
            Self::Codex => crate::agent::Provider::Codex,
        }
    }
}

/// The description behind one `fixture:` line.
#[must_use]
pub fn describe(preset: Preset) -> Fixture {
    match preset {
        Preset::Empty => empty(),
        Preset::OneRepo => one_repo(),
        Preset::Busy => busy(),
        Preset::Board => board(),
        Preset::Agents => agents(),
    }
}

fn base(preset: Preset) -> Fixture {
    Fixture {
        preset,
        context: "Acme".to_owned(),
        owners: vec!["acme".to_owned()],
        repositories: Vec::new(),
        board: None,
        agents: Vec::new(),
        transcripts: Vec::new(),
    }
}

fn empty() -> Fixture {
    base(Preset::Empty)
}

fn one_repo() -> Fixture {
    Fixture {
        repositories: vec![Repository {
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            branches: Vec::new(),
            worktrees: vec![Worktree::clean("feature")],
            pull_requests: Vec::new(),
        }],
        ..base(Preset::OneRepo)
    }
}

fn busy() -> Fixture {
    Fixture {
        repositories: vec![
            Repository {
                owner: "acme".to_owned(),
                name: "api".to_owned(),
                branches: vec!["release".to_owned()],
                worktrees: vec![
                    Worktree::dirty("feature", &["NOTES.md"]),
                    Worktree::clean("hotfix"),
                    Worktree::degraded("broken"),
                ],
                pull_requests: vec![
                    PullRequest {
                        number: 12,
                        title: "Add the worktree ticker".to_owned(),
                        author: "fleet-test".to_owned(),
                        head: "feature".to_owned(),
                        base: "main".to_owned(),
                        draft: false,
                        tab: PrTab::Mine,
                        review_decision: Some("APPROVED".to_owned()),
                        checks: vec!["SUCCESS".to_owned(), "SUCCESS".to_owned()],
                        additions: 120,
                        deletions: 8,
                        labels: vec!["enhancement".to_owned()],
                    },
                    PullRequest {
                        number: 13,
                        title: "Draft: rework the hub header".to_owned(),
                        author: "fleet-test".to_owned(),
                        head: "hotfix".to_owned(),
                        base: "main".to_owned(),
                        draft: true,
                        tab: PrTab::Mine,
                        review_decision: None,
                        checks: vec!["FAILURE".to_owned()],
                        additions: 4,
                        deletions: 40,
                        labels: Vec::new(),
                    },
                    PullRequest {
                        number: 21,
                        title: "Tighten the reconnect banner".to_owned(),
                        author: "sam".to_owned(),
                        head: "banner".to_owned(),
                        base: "main".to_owned(),
                        draft: false,
                        tab: PrTab::Review,
                        review_decision: Some("REVIEW_REQUIRED".to_owned()),
                        checks: vec!["PENDING".to_owned()],
                        additions: 16,
                        deletions: 2,
                        labels: vec!["review".to_owned()],
                    },
                ],
            },
            Repository {
                owner: "acme".to_owned(),
                name: "web".to_owned(),
                branches: Vec::new(),
                worktrees: vec![Worktree::clean("spike")],
                pull_requests: vec![PullRequest {
                    number: 4,
                    title: "Ship the settings dialog".to_owned(),
                    author: "fleet-test".to_owned(),
                    head: "spike".to_owned(),
                    base: "main".to_owned(),
                    draft: false,
                    tab: PrTab::Mine,
                    review_decision: None,
                    checks: Vec::new(),
                    additions: 9,
                    deletions: 9,
                    labels: Vec::new(),
                }],
            },
        ],
        ..base(Preset::Busy)
    }
}

fn board() -> Fixture {
    Fixture {
        repositories: vec![Repository {
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            branches: Vec::new(),
            worktrees: vec![Worktree::clean("feature")],
            pull_requests: Vec::new(),
        }],
        board: Some(Board {
            prefix: "FLT".to_owned(),
            cards: vec![
                Card {
                    title: "Photograph the daemon-down screen".to_owned(),
                    description: "Kill fleetd and assert the banner.".to_owned(),
                    column: 0,
                },
                Card {
                    title: "Freeze the scenario grammar".to_owned(),
                    description: "Phase 5 writes against it.".to_owned(),
                    column: 0,
                },
                Card {
                    title: "Seed the board preset".to_owned(),
                    description: "Cards through the daemon's own API.".to_owned(),
                    column: 1,
                },
                Card {
                    title: "Wire the fake acli".to_owned(),
                    description: "Board scenarios must not reach Atlassian.".to_owned(),
                    column: 1,
                },
                Card {
                    title: "Build real git origins".to_owned(),
                    description: "Fleet's git layer deserves real input.".to_owned(),
                    column: 2,
                },
            ],
        }),
        ..base(Preset::Board)
    }
}

fn agents() -> Fixture {
    Fixture {
        repositories: vec![Repository {
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            branches: Vec::new(),
            worktrees: vec![Worktree::clean("agent")],
            pull_requests: Vec::new(),
        }],
        agents: vec![
            Agent {
                provider: Provider::Claude,
                transcript: conversation(),
            },
            Agent {
                provider: Provider::Codex,
                transcript: approval(),
            },
        ],
        ..base(Preset::Agents)
    }
}

/// The three-turn conversation the `agents` preset gives Claude.
///
/// The first two turns and the failing third turn come from the shipped starters rather than
/// copies written here. That keeps this preset tied to the same documents
/// `crate::agent::tests::all_three_starter_transcripts_load_and_validate` checks.
fn conversation() -> serde_json::Value {
    let mut conversation = starter(
        include_str!("../../transcripts/two-turns.json"),
        "two-turns.json",
    );
    let mut failure = starter(
        include_str!("../../transcripts/error-mid-stream.json"),
        "error-mid-stream.json",
    );
    let failure_steps = failure
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
        .map(std::mem::take)
        .expect("the embedded error-mid-stream transcript has a steps array");
    conversation
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
        .expect("the embedded two-turns transcript has a steps array")
        .extend(failure_steps);
    conversation
}

/// The conversation that stops at an edit approval, so the decision surface is reachable.
fn approval() -> serde_json::Value {
    starter(
        include_str!("../../transcripts/edit-approval.json"),
        "edit-approval.json",
    )
}

/// Parses one embedded starter transcript.
///
/// The file is compiled into the binary, so a malformed one is a build-time fact rather than a
/// runtime condition; `agent::tests::all_three_starter_transcripts_load_and_validate` is what catches it.
fn starter(document: &str, name: &str) -> serde_json::Value {
    serde_json::from_str(document)
        .unwrap_or_else(|error| panic!("the embedded transcript {name} is not valid JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_preset_serves_the_error_starter_as_claudes_third_turn() {
        let fixture = agents();
        let claude = fixture
            .agents
            .iter()
            .find(|agent| agent.provider == Provider::Claude)
            .unwrap_or_else(|| panic!("the agents preset must configure Claude"));
        let steps = claude
            .transcript
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("Claude's embedded transcript must have a steps array"));
        let terminals: Vec<&serde_json::Value> = steps
            .iter()
            .filter(|step| step.get("type").and_then(serde_json::Value::as_str) == Some("end_turn"))
            .collect();

        assert_eq!(terminals.len(), 3, "Claude must expose all three turns");
        assert_eq!(
            terminals[2]
                .get("status")
                .and_then(serde_json::Value::as_str),
            Some("failed"),
            "the third turn must preserve error-mid-stream's failed settlement"
        );
        assert!(
            steps.iter().any(|step| {
                step.get("type").and_then(serde_json::Value::as_str) == Some("error")
                    && step.get("message").and_then(serde_json::Value::as_str)
                        == Some("Selected model is at capacity. Please try a different model.")
            }),
            "the third turn must preserve error-mid-stream's provider error"
        );
    }
}
