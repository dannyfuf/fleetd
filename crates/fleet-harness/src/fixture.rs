//! Declarative hermetic fixture presets.
//!
//! A scenario's first line names the world it is about — `fixture: busy` — and this module
//! builds that world before `fleetd` or Fleet read the run's private `FLEET_HOME`. Five
//! presets are frozen in `docs/TESTING-HARNESS.md` §4; each one is a [`plan::Fixture`]
//! description turned into three real things:
//!
//! - **real git repositories** ([`git`]), built with `git` and never by fabricating a `.git`
//!   directory, because Fleet's git layer shells out to the real binary and deserves real
//!   input;
//! - **real daemon state** ([`seed`]), written by a private `fleetd` the fixture starts and
//!   shuts down again, so a preset cannot drift from what the daemon would have written
//!   itself;
//! - **real fake executables** ([`tools`]) — `gh`, `acli` and the two agent shims — answering
//!   from fixture data on the `PATH` every child of the run inherits.
//!
//! Everything a running scenario needs *after* the daemon is up lives in [`jobs`]: the job
//! registry is in-memory, so the jobs panel, the toast stack and the sticky error can only be
//! lit from a live client (P4-T05).
//!
//! Two builds of the same preset differ only in what the daemon generates — job ids and
//! ISO-8601 timestamps. Commit ids, paths relative to the home, row order and row content are
//! pinned; `tests::two_runs_of_a_preset_differ_only_in_ids_and_times` proves it.

mod git;
pub mod jobs;
mod plan;
mod seed;
mod tools;

#[cfg(test)]
mod tests;

pub use jobs::{Injected, Injection};
pub use plan::{
    Agent, Board, Card, Fixture, Hook, PrTab, Provider, PullRequest, Repository, Worktree,
};
pub use seed::await_job;

use crate::env::HarnessEnv;
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The five frozen worlds plus additive subagent variants a scenario may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    /// A Fleet that has never been run.
    Empty,
    /// One clean repository and one clean worktree.
    OneRepo,
    /// Several repositories, worktrees and pull requests, one of them degraded.
    Busy,
    /// A board with cards, plus the fake `acli`.
    Board,
    /// Native-agent configuration pointed at scripted transcripts.
    Agents,
    /// Native-agent configuration whose Claude transcript delegates in the current worktree.
    AgentsSubagent,
    /// Native-agent configuration whose Claude transcript delegates to a second worktree.
    AgentsSubagentOtherWorktree,
}

impl Preset {
    /// The word this preset is written with in a scenario file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::OneRepo => "one-repo",
            Self::Busy => "busy",
            Self::Board => "board",
            Self::Agents => "agents",
            Self::AgentsSubagent => "agents-subagent",
            Self::AgentsSubagentOtherWorktree => "agents-subagent-other-worktree",
        }
    }

    /// Every preset, for tests and for `--help` text.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Empty,
            Self::OneRepo,
            Self::Busy,
            Self::Board,
            Self::Agents,
            Self::AgentsSubagent,
            Self::AgentsSubagentOtherWorktree,
        ]
    }
}

impl std::fmt::Display for Preset {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for Preset {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::all()
            .into_iter()
            .find(|preset| preset.as_str() == value)
            .ok_or_else(|| anyhow::anyhow!("unknown fixture preset {value:?}"))
    }
}

/// Seeds the run's private `FLEET_HOME` for one preset, before `fleetd` or Fleet read it.
///
/// The fake executables are installed for every preset, `empty` included: the point of a
/// hermetic run is that nothing can reach the network, not that nothing has data. `empty`
/// then stops there, because [`HarnessEnv`] has already given the run an empty `FLEET_HOME`
/// and a child-only `HOME`, which is precisely what "a Fleet that has never been run" means.
pub async fn apply_preset(preset: Preset, environment: &HarnessEnv) -> anyhow::Result<Fixture> {
    let mut fixture = plan::describe(preset);
    let workspace = workspace(environment);
    std::fs::create_dir_all(&workspace)
        .with_context(|| format!("create the fixture workspace {}", workspace.display()))?;
    fixture.transcripts = tools::install(environment, &fixture, &workspace)
        .with_context(|| format!("install the {preset} fixture's fake executables"))?;
    if !fixture.is_seeded() {
        return Ok(fixture);
    }
    let origins = git::build_origins(&workspace, &fixture, &environment.child_home)
        .await
        .with_context(|| format!("build the {preset} fixture's git origins"))?;
    seed::seed(environment, &fixture, &origins, &workspace)
        .await
        .with_context(|| format!("seed the {preset} fixture through the daemon"))?;
    Ok(fixture)
}

/// Where a fixture keeps the things that are not part of the Fleet home: origins that stand
/// in for GitHub, the data the fake tools answer from, and the seeding daemon's log.
///
/// It sits beside the home rather than inside it, because a Fleet home that contained its own
/// remotes would not be the home a user has.
fn workspace(environment: &HarnessEnv) -> PathBuf {
    environment
        .fleet_home
        .parent()
        .unwrap_or(Path::new(&environment.fleet_home))
        .join("fixture")
}
