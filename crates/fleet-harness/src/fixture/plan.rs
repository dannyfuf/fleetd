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
    /// Pull request cards seeded onto the context's Reviews board, which the preset asks for
    /// with `EnsureReviewsBoard` only when this list is not empty.
    #[serde(default)]
    pub reviews: Vec<ReviewCard>,
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
        !self.repositories.is_empty()
            || self.board.is_some()
            || !self.reviews.is_empty()
            || !self.agents.is_empty()
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
    /// Cards seeded onto this worktree's own board, which is how a fixture gets one.
    ///
    /// The board itself is asked for with `EnsureWorktreeBoard`, so its id, name and prefix
    /// are the daemon's derivation rather than a fixture's guess; only the cards are described
    /// here. An empty list is a worktree with no board at all, which is every worktree but the
    /// `board` and `board-workflow` presets'.
    pub board: Vec<Card>,
    /// The column automation this worktree's board opts into, applied before its cards are
    /// created so a card can name a column the preset adds.
    #[serde(default)]
    pub workflow: Option<Workflow>,
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
            board: Vec::new(),
            workflow: None,
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

/// What a seeded worktree board asks of `fleet_core::board::apply_workflow_preset`.
///
/// The preset itself is the daemon's, not the fixture's: applying it through `UpdateBoard`
/// means a run sees the same seven columns, the same actions and the same routing a person
/// gets from `fleet board columns --preset`, and a fixture cannot drift from them. Only the
/// run throttle is stated here, because a board that leaves it unset is already at one and a
/// scenario proving the `pending` mark needs to say so out loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// Live runs the board allows at once.
    pub max_live_runs: u32,
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
    /// Cards that block this one, as indices into the same list.
    ///
    /// Cards are created in list order, so an index here always names a card that already
    /// exists; a forward reference is a fixture bug and [`super::seed`] says so rather than
    /// seeding a board with a link missing.
    #[serde(default)]
    pub blocked_by: Vec<usize>,
}

impl Card {
    /// A card in one column that nothing blocks.
    #[must_use]
    pub fn new(title: &str, description: &str, column: usize) -> Self {
        Self {
            title: title.to_owned(),
            description: description.to_owned(),
            column,
            blocked_by: Vec::new(),
        }
    }

    /// The same card, blocked by the cards at these indices.
    #[must_use]
    pub fn blocked_by(mut self, cards: &[usize]) -> Self {
        self.blocked_by = cards.to_vec();
        self
    }
}

/// One pull request card on the context's Reviews board.
///
/// The board itself is `EnsureReviewsBoard`'s, so its id, `REV` prefix and five review columns
/// are the daemon's own; the card goes in through `UpsertPullRequestCard`, the request a review
/// schedule's run sends, so it carries its pull request the way a real one does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCard {
    /// Title.
    pub title: String,
    /// The pull request, `owner/name#number`.
    pub pull_request: String,
    /// Index into the Reviews board's columns, clamped to the last one.
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
        Preset::AgentsSubagent => subagent_agents(
            Preset::AgentsSubagent,
            include_str!("../../transcripts/subagent-caller.json"),
            "subagent-caller.json",
        ),
        Preset::AgentsSubagentOtherWorktree => subagent_agents(
            Preset::AgentsSubagentOtherWorktree,
            include_str!("../../transcripts/subagent-caller-other-worktree.json"),
            "subagent-caller-other-worktree.json",
        ),
        Preset::BoardWorkflow => board_workflow(),
        Preset::Reviews => reviews(),
    }
}

fn base(preset: Preset) -> Fixture {
    Fixture {
        preset,
        context: "Acme".to_owned(),
        owners: vec!["acme".to_owned()],
        repositories: Vec::new(),
        board: None,
        reviews: Vec::new(),
        agents: Vec::new(),
        transcripts: Vec::new(),
    }
}

fn empty() -> Fixture {
    base(Preset::Empty)
}

fn one_repo() -> Fixture {
    one_repo_with(Preset::OneRepo)
}

/// One clean repository and worktree, labelled as `preset`.
fn one_repo_with(preset: Preset) -> Fixture {
    Fixture {
        repositories: vec![Repository {
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            branches: Vec::new(),
            worktrees: vec![Worktree::clean("feature")],
            pull_requests: Vec::new(),
        }],
        ..base(preset)
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

/// The board preset: one context board, and one worktree board under the same context.
///
/// The two card sets are deliberately disjoint and their prefixes differ — the context board
/// is `FLT`, and `acme/api#feature`'s board takes the `FEA` the daemon derives from the slug —
/// so a dump says which of the two a surface is showing. That is what
/// `scenarios/workspace/board-tab.scenario` reads to prove the Hub still shows the *context*
/// board after the Workspace tab has shown the worktree's.
fn board() -> Fixture {
    Fixture {
        repositories: vec![Repository {
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            branches: Vec::new(),
            worktrees: vec![Worktree {
                board: vec![
                    Card::new(
                        "Open the tab with ctrl-s b",
                        "Created on demand, selected every time.",
                        0,
                    ),
                    Card::new(
                        "Scope the mirror to this worktree",
                        "One BoardState, two scopes.",
                        0,
                    ),
                    Card::new(
                        "Draw the board inside the Workspace",
                        "The Hub's board view, lent to the tab.",
                        1,
                    ),
                ],
                ..Worktree::clean("feature")
            }],
            pull_requests: Vec::new(),
        }],
        board: Some(Board {
            prefix: "FLT".to_owned(),
            cards: vec![
                Card::new(
                    "Photograph the daemon-down screen",
                    "Kill fleetd and assert the banner.",
                    0,
                ),
                Card::new(
                    "Freeze the scenario grammar",
                    "Phase 5 writes against it.",
                    0,
                ),
                Card::new(
                    "Seed the board preset",
                    "Cards through the daemon's own API.",
                    1,
                ),
                Card::new(
                    "Wire the fake acli",
                    "Board scenarios must not reach Atlassian.",
                    1,
                ),
                Card::new(
                    "Build real git origins",
                    "Fleet's git layer deserves real input.",
                    2,
                ),
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

/// A caller fixture with two worktrees, so both shipped caller variants address real ids.
fn subagent_agents(preset: Preset, document: &str, caller: &str) -> Fixture {
    Fixture {
        repositories: vec![Repository {
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            branches: Vec::new(),
            worktrees: vec![Worktree::clean("agent"), Worktree::clean("other")],
            pull_requests: Vec::new(),
        }],
        agents: vec![
            Agent {
                provider: Provider::Claude,
                transcript: starter(document, caller),
            },
            Agent {
                provider: Provider::Codex,
                transcript: starter(
                    include_str!("../../transcripts/subagent-caller-blocked.json"),
                    "subagent-caller-blocked.json",
                ),
            },
        ],
        ..base(preset)
    }
}

/// The board-workflow preset: the subagent world, plus a worktree board that runs cards.
///
/// The agent wiring is `agents-subagent`'s, because the transcript that decides what a run
/// does here is the *child* one. A card run is a delegation, so the launcher serves Codex
/// `subagent-child.json`, which reports a result and completes, and Claude
/// `subagent-child-blocked.json`, whose permission gate stands in for a child that has to ask
/// its caller something (`crate::agent::launcher`). One provider therefore drives a column
/// through `on_success` and the other parks a card on `needs you`, and this preset needs no
/// transcript of its own to get both.
///
/// The board is `acme/api#agent`'s — `EnsureWorktreeBoard`'s own derivation, so its id, name
/// and `AGE` prefix are the daemon's — with the workflow preset applied and one live run
/// allowed, which is what makes a second card moved into an action column `pending` rather
/// than a second run. All four cards start in Todo, the one column the preset deliberately
/// leaves human: a run started by the *seeding* daemon would be gone by the time the
/// scenario's own daemon reads the home, so every run a scenario sees is one it started.
fn board_workflow() -> Fixture {
    let agents = subagent_agents(
        Preset::BoardWorkflow,
        include_str!("../../transcripts/subagent-caller.json"),
        "subagent-caller.json",
    )
    .agents;
    Fixture {
        repositories: vec![Repository {
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            branches: Vec::new(),
            worktrees: vec![
                Worktree {
                    board: workflow_cards(),
                    workflow: Some(Workflow { max_live_runs: 1 }),
                    ..Worktree::clean("agent")
                },
                Worktree::clean("other"),
            ],
            pull_requests: Vec::new(),
        }],
        agents,
        ..base(Preset::BoardWorkflow)
    }
}

/// The reviews preset: one-repo's world, plus the context's Reviews board with one card.
///
/// The card sits in Reviewed, the one column of the review pipeline that neither routes nor
/// runs: Pending review advances a card it can start into Reviewing, whose action is a run, and
/// a run the *seeding* daemon started would be gone by the time the scenario's own daemon reads
/// the home. `acme/api#1` names the fixture's own repository, so the tile's reference and the
/// detail's `Pull request` row read a pull request the context owns.
fn reviews() -> Fixture {
    /// Index of `Reviewed` in `fleet_core::board::reviews_preset`.
    const REVIEWED_COLUMN: usize = 2;
    Fixture {
        reviews: vec![ReviewCard {
            title: "Review the retry budget".to_owned(),
            pull_request: "acme/api#1".to_owned(),
            column: REVIEWED_COLUMN,
        }],
        ..one_repo_with(Preset::Reviews)
    }
}

/// The four linked cards `board-workflow` seeds, in creation order.
///
/// Card 1 blocks card 3, and cards 1 and 2 both block card 4, so a scenario reads `⊘ 1` and
/// `⊘ 2` off the face without moving anything, and completing card 1 releases exactly one of
/// the two.
fn workflow_cards() -> Vec<Card> {
    /// Index of `Todo` in the workflow preset's seven columns.
    const TODO_COLUMN: usize = 1;
    vec![
        Card::new(
            "Throttle the board to one run",
            "Every card on a worktree board edits the same checkout.",
            TODO_COLUMN,
        ),
        Card::new(
            "Give the column an action",
            "A column decides what running a card means.",
            TODO_COLUMN,
        ),
        Card::new(
            "Review the throttle",
            "Runs once the throttle is in.",
            TODO_COLUMN,
        )
        .blocked_by(&[0]),
        Card::new(
            "Ship the workflow",
            "Waits for the throttle and the action together.",
            TODO_COLUMN,
        )
        .blocked_by(&[0, 1]),
    ]
}

/// The three-turn conversation the `agents` preset gives Claude.
///
/// The first two turns and the failing third turn come from the shipped starters rather than
/// copies written here. That keeps this preset tied to the same documents
/// `crate::agent::tests::every_starter_transcript_loads_and_validates` checks.
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
/// runtime condition; `agent::tests::every_starter_transcript_loads_and_validates` catches it.
fn starter(document: &str, name: &str) -> serde_json::Value {
    serde_json::from_str(document)
        .unwrap_or_else(|error| panic!("the embedded transcript {name} is not valid JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_pacing_ms(step: &serde_json::Value) -> u64 {
        let pace_ms = step
            .get("pace_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let chunks = step
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map_or(0, |text| text.split_inclusive(char::is_whitespace).count());
        pace_ms.saturating_mul(u64::try_from(chunks).unwrap_or(u64::MAX))
    }

    #[test]
    fn agents_starter_keeps_each_short_turn_observable() {
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
        let mut turn_pacing = Vec::new();
        let mut pacing_ms = 0_u64;
        for step in steps {
            if step.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                pacing_ms = pacing_ms.saturating_add(text_pacing_ms(step));
            }
            if step.get("type").and_then(serde_json::Value::as_str) == Some("end_turn") {
                turn_pacing.push(pacing_ms);
                pacing_ms = 0;
            }
        }

        assert!(turn_pacing.len() >= 2, "the starter must keep two turns");
        assert!(
            turn_pacing[..2].iter().all(|pacing| *pacing >= 1_000),
            "the two completed starter turns must stay visible long enough for the app harness: {turn_pacing:?}"
        );
    }

    #[test]
    fn blocked_child_keeps_working_state_observable_before_its_gate() {
        let transcript = starter(
            include_str!("../../transcripts/subagent-child-blocked.json"),
            "subagent-child-blocked.json",
        );
        let steps = transcript
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("the blocked child transcript must have a steps array"));
        let pacing_ms = steps
            .iter()
            .take_while(|step| {
                step.get("type").and_then(serde_json::Value::as_str) != Some("permission")
            })
            .map(text_pacing_ms)
            .fold(0_u64, u64::saturating_add);

        assert!(
            pacing_ms >= 1_000,
            "the child must stay working long enough for the app harness before it blocks: {pacing_ms} ms"
        );
    }

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

    #[test]
    fn the_board_workflow_preset_seeds_four_linked_cards_in_one_human_column() {
        let fixture = describe(Preset::BoardWorkflow);
        let worktree = fixture
            .repositories
            .first()
            .and_then(|repository| repository.worktrees.first())
            .unwrap_or_else(|| panic!("board-workflow must seed acme/api#agent"));

        assert_eq!(
            worktree.slug, "agent",
            "the board hangs off the first worktree"
        );
        assert_eq!(
            worktree.workflow,
            Some(Workflow { max_live_runs: 1 }),
            "a second card in an action column has to queue rather than run"
        );
        assert_eq!(worktree.board.len(), 4);
        let columns: std::collections::BTreeSet<usize> =
            worktree.board.iter().map(|card| card.column).collect();
        assert_eq!(
            columns.len(),
            1,
            "every seeded card starts in Todo, which the preset leaves human, saw {columns:?}"
        );
        assert_eq!(
            worktree
                .board
                .iter()
                .map(|card| card.blocked_by.as_slice())
                .collect::<Vec<_>>(),
            vec![&[][..], &[][..], &[0][..], &[0, 1][..]],
            "card 1 blocks card 3, and cards 1 and 2 block card 4"
        );
        for (index, card) in worktree.board.iter().enumerate() {
            assert!(
                card.blocked_by.iter().all(|blocker| *blocker < index),
                "a blocker has to be created before the card it blocks: {}",
                card.title
            );
        }
        assert_eq!(
            fixture.board, None,
            "board-workflow seeds no context board, so a dump names one board only"
        );
    }

    #[test]
    fn subagent_presets_select_their_caller_and_seed_the_other_worktree() {
        for (preset, expected_command) in [
            (Preset::AgentsSubagent, "--brief-file"),
            (
                Preset::AgentsSubagentOtherWorktree,
                "--worktree acme/api#other",
            ),
        ] {
            let fixture = describe(preset);
            assert_eq!(
                fixture.repositories[0]
                    .worktrees
                    .iter()
                    .map(|worktree| worktree.slug.as_str())
                    .collect::<Vec<_>>(),
                vec!["agent", "other"]
            );
            let caller = fixture
                .agents
                .iter()
                .find(|agent| agent.provider == Provider::Claude)
                .unwrap_or_else(|| panic!("{preset} must configure the Claude caller"));
            let command = caller
                .transcript
                .get("steps")
                .and_then(serde_json::Value::as_array)
                .and_then(|steps| {
                    steps.iter().find_map(|step| {
                        (step.get("type").and_then(serde_json::Value::as_str) == Some("shell"))
                            .then(|| step.get("command"))
                            .flatten()
                            .and_then(serde_json::Value::as_str)
                    })
                })
                .unwrap_or_else(|| panic!("{preset} must serve a shell caller step"));
            assert!(command.contains(expected_command), "{preset}: {command}");

            let blocked_caller = fixture
                .agents
                .iter()
                .find(|agent| agent.provider == Provider::Codex)
                .unwrap_or_else(|| panic!("{preset} must configure the Codex caller"));
            let blocked_command = blocked_caller
                .transcript
                .get("steps")
                .and_then(serde_json::Value::as_array)
                .and_then(|steps| {
                    steps.iter().find_map(|step| {
                        (step.get("type").and_then(serde_json::Value::as_str) == Some("shell"))
                            .then(|| step.get("command"))
                            .flatten()
                            .and_then(serde_json::Value::as_str)
                    })
                })
                .unwrap_or_else(|| panic!("{preset} must serve a blocked shell caller step"));
            assert!(
                blocked_command.contains("--provider claude"),
                "{preset}: {blocked_command}"
            );
        }
    }
}
