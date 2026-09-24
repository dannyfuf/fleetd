//! End-to-end board automation against a private `fleetd` and a scripted provider.
//!
//! Everything here drives the real daemon over its socket: a worktree board with real columns,
//! real cards, and runs served by a `/bin/sh` stand-in for `codex` that the run's own `PATH`
//! shadows the vendor binary with. Nothing reaches the network, the developer's `~/.fleet`, or
//! a real agent.
//!
//! The world is `fleet_harness::fixture::Preset::BoardWorkflow`, which builds real git origins,
//! clones them, publishes two worktrees and installs the `gh`, `acli`, `fleet` and agent shims
//! the children need (`docs/TESTING-HARNESS.md` §4, §5). The preset seeds its board on
//! `acme/api#agent`; every test here builds its own board on the untouched `acme/api#other`, so
//! what a run sees is what the test wrote and nothing else.
//!
//! The child transcript is this file's own rather than the preset's: the shipped one sleeps five
//! seconds before reporting, and a four-run diamond would pay that four times over.
//!
//! `fleet-harness`'s own binary plays the transcripts. `make test` exports `FLEET_HARNESS_BIN`;
//! a bare `cargo test` falls back to the directory walk in `fleet_harness::agent::launcher`,
//! which finds `target/<profile>/fleet-harness` when it has been built. When neither answers,
//! `launcher_script` fails with the sentence that says so, and so does the test.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use fleet_client::{Client, Result as ClientResult};
use fleet_core::{
    agents::AgentKind,
    board::{
        Action, ActionKind, BackendRef, BoardPatch, BoardView, Card, CardDraft, ColumnAgentPrefs,
        ColumnAutomation, RunOutcome, Status, workflow_preset,
    },
    ids::{CardId, StatusId, WorktreeId},
    paths::FleetHome,
    state::State,
};
use fleet_harness::{
    agent::{Provider, launcher_script},
    env::{Daemon, HarnessEnv},
    fixture::{Preset, apply_preset},
};

/// The worktree the preset leaves without a board, so every test here owns its own.
const WORKTREE: &str = "acme/api#other";
/// The column a card runs in.
const RUNNING: &str = "in-progress";
/// The column a card waits in until nothing blocks it.
const WAITING: &str = "ready";
/// The column a successful run moves a card to.
const FINISHED: &str = "done";
/// How long the whole diamond may take before the test calls it wedged.
const SETTLE_BUDGET: Duration = Duration::from_secs(240);
/// How long one run may take to appear or to end.
const RUN_BUDGET: Duration = Duration::from_secs(120);
/// How often the board is re-read while waiting.
const POLL: Duration = Duration::from_millis(200);

/// A private daemon, the world it runs in, and the directory that holds both.
///
/// Declaration order is drop order, exactly as `IsolatedDaemon` orders its own fields: the
/// daemon dies before the temporary root it lives in is deleted.
struct World {
    daemon: Daemon,
    environment: HarnessEnv,
    root: tempfile::TempDir,
}

impl World {
    /// Builds the board-workflow world, installs this file's scripted Codex, and starts `fleetd`.
    ///
    /// The seeding the preset does needs the environment *before* a daemon owns the home, which
    /// is why this lays the environment out itself instead of taking
    /// `IsolatedDaemon::start`'s — that constructor spawns the daemon in the same call, leaving
    /// no window to seed in. Everything else is `IsolatedDaemon`'s own code path: the same
    /// `HarnessEnv::rooted`, the same `Daemon`, the same temporary root.
    async fn boot(label: &str) -> anyhow::Result<Self> {
        let root = tempfile::Builder::new()
            .prefix(&format!("fleet-boards-automation-{label}-"))
            .tempdir()?;
        let mut environment = HarnessEnv::rooted(root.path())?;
        // The `fleetd` under test, not whichever one happens to be on the developer's PATH or in
        // an inherited `FLEET_DAEMON`. Cargo builds it for this integration test by definition.
        environment.daemon = PathBuf::from(env!("CARGO_BIN_EXE_fleetd"));
        apply_preset(Preset::BoardWorkflow, &environment).await?;
        install_scripted_codex(&environment, root.path(), CHILD_REPORTS)?;
        let daemon = Self::spawn(&environment).await?;
        Ok(Self {
            daemon,
            environment,
            root,
        })
    }

    /// Starts a `fleetd` on this world's home, logging into the root so a failure can be read.
    async fn spawn(environment: &HarnessEnv) -> anyhow::Result<Daemon> {
        let logs = environment.fleet_home.join("logs");
        std::fs::create_dir_all(&logs)?;
        let log = std::fs::File::create(logs.join("fleetd.out"))?;
        let errors = log.try_clone()?;
        Daemon::start(
            &environment.daemon,
            environment,
            Stdio::from(log),
            Stdio::from(errors),
        )
        .await
    }

    /// Replaces the running daemon with a fresh one on the same home.
    async fn restart(&mut self) -> anyhow::Result<()> {
        self.daemon.shutdown().await?;
        self.daemon = Self::spawn(&self.environment).await?;
        Ok(())
    }

    async fn client(&self) -> anyhow::Result<Client> {
        self.daemon.client().await
    }

    /// Reads the daemon's state document, for a test that has to describe a world the client
    /// has no verb for.
    fn state(&self) -> anyhow::Result<State> {
        let path = FleetHome::new(self.environment.fleet_home.clone()).state_path();
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    fn write_state(&self, state: &State) -> anyhow::Result<()> {
        let path = FleetHome::new(self.environment.fleet_home.clone()).state_path();
        std::fs::write(path, serde_json::to_vec_pretty(state)?)?;
        Ok(())
    }

    /// Stops the daemon and says whether it left anything behind.
    ///
    /// Called explicitly rather than left to `Drop`, because a test that ends by killing its
    /// daemon never learns that the daemon refused to go.
    async fn shutdown(mut self) -> anyhow::Result<()> {
        self.environment.stop_fleet_cli()?;
        let stopped = self.daemon.shutdown().await;
        drop(self.root);
        stopped
    }
}

/// A child that reports a result and completes, as fast as the shell can do it.
const CHILD_REPORTS: &str = "report=$(mktemp) && \
     printf '%s\\n' 'Did what the card asked.' > \"$report\" && \
     fleet subagent complete --result-file \"$report\"; \
     status=$?; rm -f \"$report\"; exit \"$status\"";

/// A child that is still working when the daemon under it goes away.
///
/// Bounded rather than endless: the process outlives the daemon that spawned it, and a test
/// suite may not leave one behind that waits forever.
const CHILD_WORKS: &str = "sleep 60";

/// Writes this file's Codex transcripts and installs the launcher that plays them.
///
/// The launcher serves the *child* transcript whenever `FLEET_DELEGATION` is set, which is
/// every card run: a card's run is a delegation, and the caller transcript beside it exists
/// only because `launcher_script` derives the child's path from it.
fn install_scripted_codex(
    environment: &HarnessEnv,
    root: &Path,
    child_command: &str,
) -> anyhow::Result<()> {
    let directory = root.join("automation");
    std::fs::create_dir_all(&directory)?;
    let caller = directory.join("codex-transcript.json");
    std::fs::write(
        &caller,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "steps": [
                {"type": "text", "text": "No card run ever plays this turn.", "pace_ms": 0},
                {"type": "end_turn", "status": "completed"},
            ],
        }))?,
    )?;
    std::fs::write(
        directory.join("subagent-child.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "steps": [
                {"type": "text", "text": "Working on the card.", "pace_ms": 0},
                {"type": "shell", "command": child_command},
                {"type": "end_turn", "status": "completed"},
            ],
        }))?,
    )?;
    environment.install_fake("codex", &launcher_script(Provider::Codex, &caller)?)?;
    Ok(())
}

/// The three columns a run travels: wait, run, done.
///
/// `workflow_preset()`'s own columns are the starting point, so the ids, names and routing are
/// `fleet-core`'s rather than this file's guesses. Two changes make them runnable here: the
/// action column names Codex, because the scripted Claude child answers a permission gate and
/// never completes; and `in-review` — a skill column, which `validate_automation` refuses on
/// Codex — is dropped, so a successful run goes straight to Done.
fn runnable_columns() -> Vec<Status> {
    let mut statuses = workflow_preset();
    statuses.retain(|status| status.id.as_str() != "in-review");
    let running = status_id(RUNNING);
    for status in &mut statuses {
        if status.id != running {
            continue;
        }
        status.automation = Some(ColumnAutomation {
            on_enter: Some(Action {
                kind: ActionKind::Prompt,
                instructions: "Run {key}.".to_owned(),
                expect: "A report.".to_owned(),
                agent: ColumnAgentPrefs {
                    provider: Some(AgentKind::Codex),
                    ..ColumnAgentPrefs::default()
                },
                env: Vec::new(),
            }),
            on_success: Some(status_id(FINISHED)),
            advance_when_unblocked: None,
        });
    }
    statuses
}

#[track_caller]
fn status_id(id: &str) -> StatusId {
    StatusId::try_from(id).unwrap_or_else(|error| panic!("{id} is a valid status id: {error}"))
}

#[track_caller]
fn worktree_id(id: &str) -> WorktreeId {
    id.parse()
        .unwrap_or_else(|error| panic!("{id} is a valid worktree id: {error}"))
}

/// Gives the worktree's board the runnable columns and a one-run throttle.
async fn automated_board(client: &Client, max_live_runs: u32) -> anyhow::Result<BoardView> {
    let view = client
        .ensure_worktree_board(worktree_id(WORKTREE))
        .await
        .map_err(|error| anyhow::anyhow!("ensure the {WORKTREE} board: {error}"))?;
    let mut settings = view.board.settings.clone();
    settings.max_live_runs = Some(max_live_runs);
    client
        .update_board(
            view.board.id.clone(),
            BoardPatch {
                statuses: Some(runnable_columns()),
                settings: Some(settings),
                ..BoardPatch::default()
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("automate the {WORKTREE} board: {error}"))
}

/// Creates one card in the waiting column, blocked by the cards already created.
async fn waiting_card(
    client: &Client,
    view: &BoardView,
    title: &str,
    blocked_by: &[CardId],
) -> anyhow::Result<CardId> {
    let card = client
        .create_card(
            view.board.id.clone(),
            CardDraft {
                title: title.to_owned(),
                status_id: Some(status_id(WAITING)),
                blocked_by: blocked_by.to_vec(),
                ..CardDraft::default()
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("create the card {title:?}: {error}"))?;
    Ok(card.id)
}

#[track_caller]
fn card<'view>(view: &'view BoardView, id: &CardId) -> &'view Card {
    view.cards
        .iter()
        .find(|card| card.id == *id)
        .unwrap_or_else(|| panic!("the board still holds {id}"))
}

/// How many cards the board shows a live run for right now.
fn live_now(view: &BoardView) -> usize {
    view.cards
        .iter()
        .filter(|card| card.runs.last().is_some_and(|run| run.outcome.is_none()))
        .count()
}

/// A one-line description of every card, for a failure that has to say what the board was doing.
fn face(view: &BoardView) -> String {
    view.cards
        .iter()
        .map(|card| {
            let runs: Vec<String> = card
                .runs
                .iter()
                .map(|run| match run.outcome {
                    Some(outcome) => format!("{}:{}", run.status_id, outcome.word()),
                    None => format!("{}:live", run.status_id),
                })
                .collect();
            let pending = card
                .pending_run
                .as_ref()
                .map_or_else(String::new, |pending| {
                    format!(" pending:{}", pending.status_id)
                });
            format!(
                "{} in {} runs[{}]{pending}",
                card.title,
                card.status_id,
                runs.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n  ")
}

/// Polls the board until `done` answers true, asserting the board's ceiling at every sample.
///
/// The throttle is asserted *while* the work runs and not only at the end, because a board that
/// ran two cards at once and then finished both would look identical afterwards. A card-called
/// `DelegationChanged` would be the sharper signal, but the client's own Hello names only the
/// agent capabilities today, so the daemon filters every card-called event away from it
/// (`docs/NATIVE-AGENTS.md` §15.7); the run rows the board itself keeps are what a peer can see.
async fn settle(
    client: &Client,
    view: &BoardView,
    budget: Duration,
    ceiling: usize,
    done: impl Fn(&BoardView) -> bool,
) -> anyhow::Result<BoardView> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut last = client
        .get_board(view.board.id.clone())
        .await
        .map_err(|error| anyhow::anyhow!("read the board: {error}"))?;
    loop {
        let live = live_now(&last);
        anyhow::ensure!(
            live <= ceiling,
            "the board ran {live} cards at once with a ceiling of {ceiling}:\n  {}",
            face(&last)
        );
        if done(&last) {
            return Ok(last);
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the board did not settle within {budget:?}:\n  {}",
            face(&last)
        );
        tokio::time::sleep(POLL).await;
        last = client
            .get_board(view.board.id.clone())
            .await
            .map_err(|error| anyhow::anyhow!("read the board: {error}"))?;
    }
}

/// A four-card diamond runs every card exactly once, one at a time, and lands them all in Done.
///
/// `A` blocks `B` and `C`; `B` and `C` both block `D`. Every card is created in the waiting
/// column, which releases a card into the action column as soon as nothing blocks it — `A` the
/// moment it is created (`docs/BOARD.md` §11.7 rule 0). From there the board drives itself: a
/// success moves the card to Done, Done satisfies its dependants, and the one-run ceiling parks
/// whichever of `B` and `C` the cascade reached second until the slot the other one holds is
/// released.
#[tokio::test]
async fn a_four_card_diamond_never_runs_two_delegations_at_once() {
    let world = World::boot("diamond")
        .await
        .unwrap_or_else(|error| panic!("boot the board-workflow world: {error:#}"));
    let outcome = diamond(&world).await;
    world
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the daemon down: {error:#}"));
    outcome.unwrap_or_else(|error| panic!("{error:#}"));
}

async fn diamond(world: &World) -> anyhow::Result<()> {
    let client = world.client().await?;
    let view = automated_board(&client, 1).await?;
    let a = waiting_card(&client, &view, "A", &[]).await?;
    let b = waiting_card(&client, &view, "B", std::slice::from_ref(&a)).await?;
    let c = waiting_card(&client, &view, "C", std::slice::from_ref(&a)).await?;
    let d = waiting_card(&client, &view, "D", &[b.clone(), c.clone()]).await?;
    let cards = [a.clone(), b.clone(), c.clone(), d.clone()];

    let settled = settle(&client, &view, SETTLE_BUDGET, 1, |view| {
        view.cards
            .iter()
            .all(|card| card.status_id.as_str() == FINISHED)
    })
    .await?;

    for id in &cards {
        let card = card(&settled, id);
        anyhow::ensure!(
            card.runs.len() == 1,
            "{} ran {} times, not once:\n  {}",
            card.title,
            card.runs.len(),
            face(&settled)
        );
        let run = &card.runs[0];
        anyhow::ensure!(
            run.outcome == Some(RunOutcome::Succeeded),
            "{}'s run ended {:?}, not succeeded:\n  {}",
            card.title,
            run.outcome,
            face(&settled)
        );
        anyhow::ensure!(
            run.status_id.as_str() == RUNNING,
            "{}'s run names the column {} rather than {RUNNING}",
            card.title,
            run.status_id
        );
        anyhow::ensure!(
            run.provider == AgentKind::Codex,
            "{}'s run used {:?}, not the column's Codex",
            card.title,
            run.provider
        );
        anyhow::ensure!(
            card.pending_run.is_none(),
            "{} is still owed a run it will never get",
            card.title
        );
    }

    // The persisted record says the same thing the polling did, and says it without sampling:
    // sort the runs by when they started and no run may begin before its predecessor ended.
    let mut intervals: Vec<(&str, String, String)> = settled
        .cards
        .iter()
        .filter_map(|card| {
            let run = card.runs.first()?;
            Some((
                card.title.as_str(),
                run.started_at.clone(),
                run.ended_at.clone()?,
            ))
        })
        .collect();
    intervals.sort_by(|left, right| left.1.cmp(&right.1));
    anyhow::ensure!(
        intervals.len() == 4,
        "only {} of the four runs ended",
        intervals.len()
    );
    for pair in intervals.windows(2) {
        let (earlier, _, ended) = &pair[0];
        let (later, started, _) = &pair[1];
        anyhow::ensure!(
            started >= ended,
            "{later} started at {started}, before {earlier} ended at {ended}"
        );
    }

    // The diamond's shape, not just its throughput: A ran first and D ran last.
    anyhow::ensure!(
        intervals[0].0 == "A" && intervals[3].0 == "D",
        "the runs went {:?}, which is not the diamond's order",
        intervals.iter().map(|run| run.0).collect::<Vec<_>>()
    );
    Ok(())
}

/// A daemon restarted mid-run adopts the live delegation instead of starting a second one.
#[tokio::test]
async fn a_restart_mid_run_adopts_the_live_delegation() {
    let mut world = World::boot("restart")
        .await
        .unwrap_or_else(|error| panic!("boot the board-workflow world: {error:#}"));
    install_scripted_codex(&world.environment, world.root.path(), CHILD_WORKS)
        .unwrap_or_else(|error| panic!("install the working child: {error:#}"));
    let outcome = restart_adopts(&mut world).await;
    world
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the daemon down: {error:#}"));
    outcome.unwrap_or_else(|error| panic!("{error:#}"));
}

async fn restart_adopts(world: &mut World) -> anyhow::Result<()> {
    let client = world.client().await?;
    let view = automated_board(&client, 1).await?;
    // Nothing blocks the card, so it advances and starts the moment it is created (§11.7 rule 0).
    let only = waiting_card(&client, &view, "Adopted", &[]).await?;

    let working = settle(&client, &view, RUN_BUDGET, 1, |view| {
        view.cards
            .iter()
            .any(|card| card.runs.last().is_some_and(|run| run.outcome.is_none()))
    })
    .await?;
    let before = card(&working, &only).runs[0].clone();
    anyhow::ensure!(
        before.thread_id.is_some(),
        "the run never reached a thread, so there is nothing to adopt:\n  {}",
        face(&working)
    );
    drop(client);

    world.restart().await?;
    let client = world.client().await?;
    // The boot sweep and the first answer race by design — recovery is idempotent, so it does
    // not have to win — which is why the window below follows this read rather than replacing it.
    let after = client.get_board(view.board.id.clone()).await?;
    let adopted = card(&after, &only);
    anyhow::ensure!(
        adopted.runs.len() == 1,
        "the restart left {} runs on the card, not the one it adopted:\n  {}",
        adopted.runs.len(),
        face(&after)
    );
    anyhow::ensure!(
        adopted.runs[0].id == before.id,
        "the restart replaced run {} with {}",
        before.id,
        adopted.runs[0].id
    );
    anyhow::ensure!(
        adopted.runs[0].outcome.is_none(),
        "the restart closed a run that was still working: {:?}",
        adopted.runs[0].outcome
    );
    anyhow::ensure!(
        adopted.status_id.as_str() == RUNNING,
        "the adopted card left its action column for {}",
        adopted.status_id
    );

    // And it stays adopted. This is the one place the suite waits on the wall clock, because the
    // claim is that nothing happens: the delegation worker's first drain and the boot sweep both
    // race the read above, and a window an order of magnitude longer than a child spawn is the
    // only way to watch them not start a second run. There is no virtual clock to advance — the
    // subject is a separate process.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let later = client.get_board(view.board.id.clone()).await?;
    let adopted = card(&later, &only);
    anyhow::ensure!(
        adopted.runs.len() == 1 && adopted.runs[0].id == before.id,
        "a second run appeared after the sweep:\n  {}",
        face(&later)
    );
    Ok(())
}

/// The three `automation` refusals of `docs/BOARD.md` §11.4, each raised by the real daemon.
///
/// A context board is not a worktree board; a worktree board on a non-local backend is not a
/// local board; and a worktree another host owns is a checkout this daemon must not run in. The
/// sentences are what every surface prints, so they are asserted verbatim.
#[tokio::test]
async fn automation_is_refused_on_a_context_board_a_jira_board_and_a_hosted_worktree() {
    let world = World::boot("refusals")
        .await
        .unwrap_or_else(|error| panic!("boot the board-workflow world: {error:#}"));
    let outcome = refusals(&world).await;
    world
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the daemon down: {error:#}"));
    outcome.unwrap_or_else(|error| panic!("{error:#}"));
}

async fn refusals(world: &World) -> anyhow::Result<()> {
    let client = world.client().await?;
    let context = client
        .get_snapshot()
        .await
        .map_err(|error| anyhow::anyhow!("read the snapshot: {error}"))?
        .active_context
        .ok_or_else(|| anyhow::anyhow!("the preset selects a context"))?;

    let board = client
        .ensure_board(context)
        .await
        .map_err(|error| anyhow::anyhow!("ensure the context board: {error}"))?;
    let refusal = automation_patch(&client, &board).await;
    assert_refusal(&refusal, "automation is available on worktree boards only");

    let board = client
        .ensure_worktree_board(worktree_id(WORKTREE))
        .await
        .map_err(|error| anyhow::anyhow!("ensure the {WORKTREE} board: {error}"))?;
    let board = client
        .update_board(
            board.board.id.clone(),
            BoardPatch {
                backend: Some(BackendRef {
                    kind: "jira".to_owned(),
                    settings: serde_json::json!({
                        "project": "FLT",
                        "site": "fleet.atlassian.test",
                    }),
                }),
                ..BoardPatch::default()
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("point the board at the scripted Jira: {error}"))?;
    let refusal = automation_patch(&client, &board).await;
    assert_refusal(&refusal, "automation is available on local boards only");

    let board = client
        .update_board(
            board.board.id.clone(),
            BoardPatch {
                backend: Some(BackendRef::default()),
                ..BoardPatch::default()
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("put the board back on the local backend: {error}"))?;

    // A worktree another host owns. The daemon reads that off its state document on every
    // refusal check and there is no client verb that hands a local checkout to a host, so the
    // record is rewritten underneath the running daemon. It is *not* written before a boot:
    // startup recovery migrates every persisted remote worktree into the legacy archive and
    // drops it from state (`services/worktrees/recovery.rs`), which would leave nothing to
    // refuse. Nothing else is running on this board, and `StateStore::load` reads the file
    // rather than a memo, so the very next check sees it.
    let mut state = world.state()?;
    let wanted = worktree_id(WORKTREE);
    let worktree = state
        .worktrees
        .iter_mut()
        .find(|worktree| worktree.id == wanted)
        .ok_or_else(|| anyhow::anyhow!("the preset published {WORKTREE}"))?;
    worktree.host = Some(
        "devbox"
            .parse()
            .map_err(|error| anyhow::anyhow!("devbox is a host id: {error}"))?,
    );
    world.write_state(&state)?;

    let refusal = automation_patch(&client, &board).await;
    assert_refusal(
        &refusal,
        "automation is unavailable on a worktree owned by host devbox",
    );
    Ok(())
}

/// Asks a board for automation and returns whatever the daemon answered.
async fn automation_patch(client: &Client, board: &BoardView) -> ClientResult<BoardView> {
    let mut statuses = board.board.statuses.clone();
    let first = statuses
        .first()
        .map(|status| status.id.clone())
        .unwrap_or_else(|| status_id(WAITING));
    let target = statuses
        .last()
        .map(|status| status.id.clone())
        .unwrap_or(first.clone());
    if let Some(status) = statuses.iter_mut().find(|status| status.id == first) {
        status.automation = Some(ColumnAutomation {
            on_enter: Some(Action {
                kind: ActionKind::Prompt,
                instructions: String::new(),
                expect: String::new(),
                agent: ColumnAgentPrefs::default(),
                env: Vec::new(),
            }),
            on_success: (target != first).then_some(target),
            advance_when_unblocked: None,
        });
    }
    client
        .update_board(
            board.board.id.clone(),
            BoardPatch {
                statuses: Some(statuses),
                ..BoardPatch::default()
            },
        )
        .await
}

#[track_caller]
fn assert_refusal(result: &ClientResult<BoardView>, sentence: &str) {
    match result {
        Ok(_) => panic!("the daemon accepted automation it had to refuse with {sentence:?}"),
        Err(error) => {
            let rendered = error.to_string();
            assert!(
                rendered.contains(sentence),
                "the refusal was {rendered:?}, which does not carry {sentence:?}"
            );
        }
    }
}
