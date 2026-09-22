//! `DelegationRun`: validate, mint the token, create the child, seed the transcript.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use chrono::Utc;
use fleet_core::agents::{
    Delegation, DelegationCaller, DelegationId, DelegationStatus, DeliveryState, ItemId, ItemKind,
    MessageOrigin, UserInput,
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    response::ResponseBody,
};
use sha2::{Digest as _, Sha256};

use crate::{
    DaemonError, DaemonResult,
    error::{REMOTE_UNSUPPORTED, from_proto_error},
    services::agents::{
        manager::CreateOptions,
        store::{OutboxAction, delegations},
    },
};

use super::{
    CardRunRequest, DelegationService, RunRequest,
    footer::{
        BARE_FLEET, SAME_WORKTREE_WARNING, card_first_message, card_footer, child_title,
        first_message,
    },
    limits::{MAX_DEPTH, MAX_LIVE_CHILDREN_PER_CALLER, MAX_LIVE_DELEGATIONS},
    storage_failure,
};

/// The child-environment keys Fleet mints itself and a caller may never set.
///
/// They are the child's delegation identity: whatever a request carries under these names is
/// dropped, because a child that reports against a delegation it was not started for would be
/// completing someone else's work. `FLEET_CARD` and `FLEET_BOARD` are here for the same reason
/// one step out: a card run's child refuses to move its own card by reading them, so a column
/// environment that could name a different card would hand the child a licence to move it.
/// `pub(in crate::services::agents)` so the resume path can enforce the same rule on the
/// environment it replays.
pub(in crate::services::agents) const FLEET_OWNED_CHILD_ENV: [&str; 4] = [
    "FLEET_DELEGATION",
    "FLEET_DELEGATION_TOKEN",
    "FLEET_CARD",
    "FLEET_BOARD",
];

/// The subset of [`FLEET_OWNED_CHILD_ENV`] that is minted fresh on every start *and* every resume.
///
/// Only these are kept out of the persisted `env_json`: a stored copy could only ever be a stale
/// secret, and the resume path mints its own. `FLEET_CARD` and `FLEET_BOARD` are not here — they
/// name the caller, which is fixed for the life of the delegation, and the column-env rule above
/// means the stored values are the ones this daemon minted, never a column author's. Persisting
/// them is what lets a resumed card child still refuse to move its own card.
pub(in crate::services::agents) const FLEET_ROTATED_CHILD_ENV: [&str; 2] =
    ["FLEET_DELEGATION", "FLEET_DELEGATION_TOKEN"];

/// Where a `fleet` the child can execute lives, and which rule found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::services::agents) struct FleetProgram {
    /// Prepended to the child's `PATH`, so bare `fleet` resolves.
    pub(in crate::services::agents) directory: PathBuf,
    /// Named verbatim in the child's footer, so a `PATH` that still fails is survivable.
    pub(in crate::services::agents) program: PathBuf,
    /// Which rule below chose it, for the daemon log.
    pub(in crate::services::agents) source: &'static str,
}

/// Picks the `fleet` a delegated child should use, preferring the caller's own.
///
/// A child that cannot run `fleet subagent complete` is this feature's worst failure mode, so the
/// order is: the binary the caller itself ran, when that exact file also exists here — which is
/// both the same-host case and the only one where wire compatibility is guaranteed; then a `fleet`
/// sitting next to this daemon's own `fleetd`, which is what a remote bootstrap leaves behind;
/// then nothing, and the child falls back to whatever its login shell's `PATH` holds.
///
/// `pub(in crate::services::agents)` rather than `pub(super)` because the resume path in
/// [`crate::services::agents::manager`] re-runs rule 2 for a child it is restarting: the caller's
/// hint is not durable, but the daemon's own sibling does not depend on a request at all.
pub(in crate::services::agents) fn resolve_fleet_program(
    caller_hint: Option<&str>,
    daemon_exe: Option<&Path>,
) -> Option<FleetProgram> {
    if let Some(hint) = caller_hint.map(Path::new)
        && hint.is_file()
        && let Some(directory) = hint.parent()
    {
        return Some(FleetProgram {
            directory: directory.to_path_buf(),
            program: hint.to_path_buf(),
            source: "caller",
        });
    }
    // `current_exe` is `fleetd`, so the sibling is the interesting file, not the parent alone.
    let directory = daemon_exe.and_then(Path::parent)?;
    let sibling = directory.join("fleet");
    sibling.is_file().then(|| FleetProgram {
        directory: directory.to_path_buf(),
        program: sibling,
        source: "daemon-sibling",
    })
}

impl DelegationService {
    /// Starts one delegation: a child thread, a durable record, and a row in the caller's
    /// transcript.
    pub(crate) async fn run(&self, request: RunRequest) -> Result<ResponseBody, ProtoError> {
        let caller = request.caller;
        let caller_exists = self
            .inner
            .store
            .delegation_write("check delegation caller", move |tx| {
                Ok((delegations::caller_exists(tx, caller)?, false))
            })
            .await
            .map_err(storage_error)?;
        if !caller_exists {
            return Err(not_found(format!(
                "caller-exists rule: delegation caller {caller} does not exist"
            )));
        }

        if let Some(owner) = self.inner.manager.owner_of(caller).await? {
            return Err(unsupported(format!(
                "caller-locality rule: delegation caller {caller} is a mirror owned by host {owner}"
            )));
        }

        let caller_turn = self
            .inner
            .manager
            .running_turn(caller)
            .await?
            .ok_or_else(|| {
                conflict(format!(
                    "running-turn rule: delegation caller {caller} has no running turn"
                ))
            })?;

        let caller_depth = self
            .inner
            .store
            .delegation_by_child(caller)
            .await
            .map_err(storage_error)?
            .map_or(0, |delegation| delegation.depth);
        if caller_depth >= MAX_DEPTH {
            return Err(conflict(format!(
                "depth-limit rule: caller depth {caller_depth} has reached the maximum {MAX_DEPTH}"
            )));
        }

        let live_children = self
            .inner
            .store
            .live_delegations(Some(caller))
            .await
            .map_err(storage_error)?;
        if live_children.len() >= MAX_LIVE_CHILDREN_PER_CALLER {
            return Err(conflict(format!(
                "live-child-limit rule: caller {caller} already has {} live children (maximum {MAX_LIVE_CHILDREN_PER_CALLER})",
                live_children.len()
            )));
        }

        let live_delegations = self
            .inner
            .store
            .live_delegations(None)
            .await
            .map_err(storage_error)?;
        if live_delegations.len() >= MAX_LIVE_DELEGATIONS {
            return Err(conflict(format!(
                "daemon-live-limit rule: this daemon already has {} live delegations (maximum {MAX_LIVE_DELEGATIONS})",
                live_delegations.len()
            )));
        }

        let config = self.inner.config.load().await.map_err(|error| {
            validation(format!(
                "provider-binary rule: could not read agent binary configuration: {error}"
            ))
        })?;
        let binary = config.agent_binaries.binary(request.provider);
        if binary.trim().is_empty() {
            return Err(unsupported(format!(
                "provider-binary rule: {} has no configured executable",
                request.provider.display_name()
            )));
        }
        crate::agents::harness::process::command_parts(binary).map_err(|error| {
            unsupported(format!(
                "provider-binary rule: the configured {} executable is invalid: {error}",
                request.provider.display_name()
            ))
        })?;

        let caller_record = self.inner.manager.record(caller).await?;
        // Read before `unwrap_or_else` consumes it: an explicitly named worktree is a decision,
        // and only the implicit default is warned about below.
        let explicit_worktree = request.worktree.is_some();
        let worktree = request
            .worktree
            .clone()
            .unwrap_or_else(|| caller_record.worktree.clone());
        self.inner
            .worktrees
            .path(worktree.clone())
            .await
            .map_err(|error| worktree_error(&worktree, error))?;

        let delegation_id = DelegationId::new();
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let token_sha256 = format!("{:x}", Sha256::digest(token.as_bytes()));
        let title = request
            .title
            .clone()
            .unwrap_or_else(|| child_title(request.provider, &request.brief));
        // The child's Fleet identity is this daemon's to state. Every owned name a request
        // carries is *dropped* rather than overwritten: only two of the four are minted here, so
        // overwriting alone would leave a caller-supplied `FLEET_CARD` or `FLEET_BOARD` in the
        // child's environment — naming a card it has nothing to do with, and taking the
        // self-move refusal with it. The CLI rejects these keys too, but the daemon must not
        // depend on one client to enforce it.
        let mut extra_env = request.env.clone();
        let overridden: Vec<&str> = FLEET_OWNED_CHILD_ENV
            .iter()
            .copied()
            .filter(|key| extra_env.contains_key(*key))
            .collect();
        if !overridden.is_empty() {
            tracing::warn!(
                delegation = %delegation_id,
                keys = ?overridden,
                "ignoring caller-supplied Fleet identity variables for the delegated child"
            );
        }
        for key in FLEET_OWNED_CHILD_ENV {
            extra_env.remove(key);
        }
        extra_env.insert("FLEET_DELEGATION".to_owned(), delegation_id.to_string());
        extra_env.insert("FLEET_DELEGATION_TOKEN".to_owned(), token);
        let daemon_exe = std::env::current_exe()
            .inspect_err(
                |error| tracing::debug!(%error, "the daemon cannot locate its own executable"),
            )
            .ok();
        let fleet = resolve_fleet_program(request.fleet_path.as_deref(), daemon_exe.as_deref());
        match &fleet {
            Some(fleet) => tracing::info!(
                delegation = %delegation_id,
                source = fleet.source,
                directory = %fleet.directory.display(),
                "prepending a fleet directory to the delegated child's PATH"
            ),
            None => tracing::warn!(
                delegation = %delegation_id,
                caller_hint = request.fleet_path.as_deref().unwrap_or("<none>"),
                "no fleet executable resolved for the delegated child; it must find one on its \
                 own PATH to report a result"
            ),
        }
        // Quoted because the footer is a command line the child copies: a Fleet installed under a
        // path with a space must still produce something runnable.
        let fleet_program = fleet.as_ref().map_or_else(
            || BARE_FLEET.to_owned(),
            |fleet| shell_words::quote(&fleet.program.to_string_lossy()).into_owned(),
        );
        let child = fleet_core::agents::ThreadId::new();
        let caller_item = ItemId::new();
        let delegation = Delegation {
            id: delegation_id,
            caller: DelegationCaller::Thread(caller),
            caller_turn: Some(caller_turn),
            caller_item: Some(caller_item),
            child,
            provider: request.provider,
            depth: caller_depth + 1,
            brief: request.brief,
            expectation: request.expectation,
            eager: request.eager,
            status: DelegationStatus::Starting,
            status_payload: None,
            result: None,
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created: Utc::now(),
            finished: None,
            headline: None,
            usage: None,
        };

        let stored = delegation.clone();
        // Kept so a resume replays it; `reserve` drops the Fleet identity keys on the way in.
        let stored_env = extra_env.clone();
        let refused = self
            .inner
            .store
            .delegation_write("reserve delegation", move |tx| {
                let refusal = delegations::reserve(
                    tx,
                    &stored,
                    &token_sha256,
                    &stored_env,
                    MAX_LIVE_CHILDREN_PER_CALLER,
                    MAX_LIVE_DELEGATIONS,
                )?;
                Ok((refusal, false))
            })
            .await
            .map_err(storage_error)?;
        if let Some(message) = refused {
            return Err(conflict(message));
        }

        let created = self
            .inner
            .manager
            .create_with(CreateOptions {
                thread: Some(child),
                worktree: worktree.clone(),
                provider: request.provider,
                model: request.model,
                mode: request.mode,
                resume_cursor: None,
                title: Some(title),
                parent: Some(caller),
                delegation: Some(delegation_id),
                extra_env,
                path_prepend: fleet.map(|fleet| fleet.directory),
            })
            .await;
        let created = match created {
            Ok(created) => created,
            Err(error) => {
                self.release_reservation(delegation_id).await?;
                return Err(error);
            }
        };
        match created {
            ResponseBody::AgentThreadCreated(summary) if summary.thread == child => {}
            other => {
                self.release_reservation(delegation_id).await?;
                return Err(validation(format!(
                    "child-creation rule: manager returned an unexpected response: {other:?}"
                )));
            }
        }

        if let Err(error) = self
            .inner
            .manager
            .append_item(
                caller,
                caller_turn,
                caller_item,
                ItemKind::Delegation {
                    id: delegation.id,
                    provider: delegation.provider,
                    child,
                    status: DelegationStatus::Starting,
                },
            )
            .await
        {
            let cleanup = self
                .compensate_created_child(&delegation, false, "caller item append failed")
                .await;
            return Err(with_cleanup(error, cleanup));
        }

        if let Err(error) = self
            .inner
            .manager
            .send(
                child,
                UserInput {
                    text: first_message(
                        &delegation.brief,
                        delegation.id,
                        &delegation.expectation,
                        &fleet_program,
                    ),
                    origin: MessageOrigin::User,
                    ..UserInput::default()
                },
            )
            .await
        {
            let cleanup = self
                .compensate_created_child(&delegation, true, "initial child send failed")
                .await;
            return Err(with_cleanup(error, cleanup));
        }

        let warning = (!explicit_worktree && worktree == caller_record.worktree)
            .then(|| SAME_WORKTREE_WARNING.to_owned());
        Ok(ResponseBody::DelegationStarted {
            delegation,
            warning,
        })
    }

    /// Starts one delegation whose caller is a board card, at `depth = 1`.
    ///
    /// The caller-exists, caller-locality, running-turn, depth-limit and live-child-limit rules
    /// and the caller-transcript append all belong to a *thread* caller and are skipped; the
    /// provider-binary rule, worktree resolution and the daemon-wide ceiling are kept, and a
    /// worktree another host owns is refused. Answers the record and the same optional warning
    /// [`DelegationService::run`] answers — always `None` today, because a card run names its
    /// worktree and so can never inherit one by default.
    ///
    /// # Errors
    ///
    /// Returns the refusal that stopped the run, or the storage failure behind it.
    pub(crate) async fn run_for_card(
        &self,
        request: CardRunRequest,
    ) -> DaemonResult<(Delegation, Option<String>)> {
        let CardRunRequest {
            board,
            card,
            key,
            worktree,
            provider,
            brief,
            expectation,
            mode,
            model,
            title,
            env,
        } = request;

        // Column-env rule, before anything is read or minted. The `FLEET_` namespace is the
        // daemon's: `FLEET_CARD` and `FLEET_BOARD` are what the child's CLI reads to refuse a
        // move of its own card, so a column that could set them would be writing its own licence.
        // The overwrite-with-warn path below would win anyway; this refuses instead of warning,
        // because a column is authored once and read by every run it starts.
        if let Some(name) = env
            .iter()
            .map(|(name, _)| name.as_str())
            .find(|name| name.starts_with("FLEET_"))
        {
            return Err(DaemonError::Validation(format!(
                "column-env rule: a column may not set {name}; every FLEET_ variable a child reads is minted by this daemon"
            )));
        }

        let live_delegations = self
            .inner
            .store
            .live_delegations(None)
            .await
            .map_err(storage_failure)?;
        if live_delegations.len() >= MAX_LIVE_DELEGATIONS {
            return Err(DaemonError::Conflict(format!(
                "daemon-live-limit rule: this daemon already has {} live delegations (maximum {MAX_LIVE_DELEGATIONS})",
                live_delegations.len()
            )));
        }

        let config = self.inner.config.load().await.map_err(|error| {
            DaemonError::Validation(format!(
                "provider-binary rule: could not read agent binary configuration: {error}"
            ))
        })?;
        let binary = config.agent_binaries.binary(provider);
        if binary.trim().is_empty() {
            return Err(DaemonError::Unsupported(format!(
                "provider-binary rule: {} has no configured executable",
                provider.display_name()
            )));
        }
        crate::agents::harness::process::command_parts(binary).map_err(|error| {
            DaemonError::Unsupported(format!(
                "provider-binary rule: the configured {} executable is invalid: {error}",
                provider.display_name()
            ))
        })?;

        // Worktree resolution and the worktree-host rule are one call: `Worktrees::path` refuses
        // every worktree another host owns. Its refusal does not say *whose* it is, so the owner
        // is read back for the sentence contracts §1.7 fixes. Both land before the token is
        // minted, so a refused run leaves no row behind.
        if let Err(error) = self.inner.worktrees.path(worktree.clone()).await {
            let host = if matches!(&error, DaemonError::Unsupported(message) if message == REMOTE_UNSUPPORTED)
            {
                self.inner
                    .worktrees
                    .host_of(worktree.clone())
                    .await
                    .unwrap_or_default()
            } else {
                None
            };
            return Err(card_worktree_error(&worktree, error, host));
        }

        let delegation_id = DelegationId::new();
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let token_sha256 = format!("{:x}", Sha256::digest(token.as_bytes()));
        // Column variables first, Fleet identity second, for the reason `run` gives: `insert`
        // overwrites, so the values this run just minted are the ones the child sees. The
        // column-env rule above already made the collision unreachable; the order is what keeps
        // it unreachable if that rule ever moves.
        let mut extra_env: BTreeMap<String, String> = env.into_iter().collect();
        extra_env.insert("FLEET_DELEGATION".to_owned(), delegation_id.to_string());
        extra_env.insert("FLEET_DELEGATION_TOKEN".to_owned(), token);
        extra_env.insert("FLEET_CARD".to_owned(), key.clone());
        extra_env.insert("FLEET_BOARD".to_owned(), board.to_string());

        let daemon_exe = std::env::current_exe()
            .inspect_err(
                |error| tracing::debug!(%error, "the daemon cannot locate its own executable"),
            )
            .ok();
        // No caller hint: a card is not a process and ran no `fleet` of its own, so rule 2 — the
        // `fleet` beside this daemon's own `fleetd` — is the only rule that can fire.
        let fleet = resolve_fleet_program(None, daemon_exe.as_deref());
        match &fleet {
            Some(fleet) => tracing::info!(
                delegation = %delegation_id,
                board = %board,
                card = %key,
                source = fleet.source,
                directory = %fleet.directory.display(),
                "prepending a fleet directory to the card run's child PATH"
            ),
            None => tracing::warn!(
                delegation = %delegation_id,
                board = %board,
                card = %key,
                "no fleet executable resolved for the card run's child; it must find one on its \
                 own PATH to report a result"
            ),
        }
        let fleet_program = fleet.as_ref().map_or_else(
            || BARE_FLEET.to_owned(),
            |fleet| shell_words::quote(&fleet.program.to_string_lossy()).into_owned(),
        );

        let child = fleet_core::agents::ThreadId::new();
        let delegation = Delegation {
            id: delegation_id,
            caller: DelegationCaller::Card {
                board: board.clone(),
                card,
            },
            // A card has no turn and no transcript, so it has neither of the two thread-caller
            // fields; the record itself says so, rather than carrying a placeholder.
            caller_turn: None,
            caller_item: None,
            child,
            provider,
            // A card is the root of its chain by construction: nothing delegated *to* the board,
            // so the depth ladder starts here rather than being read from a caller.
            depth: 1,
            brief,
            expectation,
            // `eager` is a thread caller's choice to be interrupted mid-turn. A card run is
            // delivered through the board hook, which has no turn to interrupt.
            eager: false,
            status: DelegationStatus::Starting,
            status_payload: None,
            result: None,
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created: Utc::now(),
            finished: None,
            headline: None,
            usage: None,
        };

        let stored = delegation.clone();
        // Kept so a resume replays it; `reserve` drops the Fleet identity keys on the way in.
        let stored_env = extra_env.clone();
        // The per-caller ceiling is passed for the one signature both callers share: the store
        // applies it to thread callers only, because a card's own reservation already holds it to
        // one live run.
        let refused = self
            .inner
            .store
            .delegation_write("reserve card delegation", move |tx| {
                let refusal = delegations::reserve(
                    tx,
                    &stored,
                    &token_sha256,
                    &stored_env,
                    MAX_LIVE_CHILDREN_PER_CALLER,
                    MAX_LIVE_DELEGATIONS,
                )?;
                Ok((refusal, false))
            })
            .await
            .map_err(storage_failure)?;
        if let Some(message) = refused {
            return Err(DaemonError::Conflict(message));
        }

        let created = self
            .inner
            .manager
            .create_with(CreateOptions {
                thread: Some(child),
                worktree,
                provider,
                model,
                // A column's permission mode is a workflow policy, so it is always explicit and
                // never falls back to the provider's configured default.
                mode: Some(mode),
                resume_cursor: None,
                title: Some(title),
                // No parent thread: the child of a card is nobody's sub-thread.
                parent: None,
                delegation: Some(delegation_id),
                extra_env,
                path_prepend: fleet.map(|fleet| fleet.directory),
            })
            .await;
        let created = match created {
            Ok(created) => created,
            Err(error) => {
                self.release_reservation(delegation_id)
                    .await
                    .map_err(from_proto_error)?;
                return Err(from_proto_error(error));
            }
        };
        match created {
            ResponseBody::AgentThreadCreated(summary) if summary.thread == child => {}
            other => {
                self.release_reservation(delegation_id)
                    .await
                    .map_err(from_proto_error)?;
                return Err(DaemonError::Validation(format!(
                    "child-creation rule: manager returned an unexpected response: {other:?}"
                )));
            }
        }

        let footer = card_footer(
            delegation_id,
            &key,
            &board,
            &delegation.expectation,
            &fleet_program,
        );
        if let Err(error) = self
            .inner
            .manager
            .send(
                child,
                UserInput {
                    text: card_first_message(&delegation.brief, &footer),
                    origin: MessageOrigin::User,
                    ..UserInput::default()
                },
            )
            .await
        {
            // `false`: a card run commits no caller transcript row, so the reservation is still
            // invisible and releasing it is the whole compensation. The board records the failure.
            let cleanup = self
                .compensate_created_child(&delegation, false, "initial card child send failed")
                .await;
            return Err(from_proto_error(with_cleanup(error, cleanup)));
        }

        Ok((delegation, None))
    }

    async fn release_reservation(&self, delegation: DelegationId) -> Result<(), ProtoError> {
        self.inner
            .store
            .delegation_write("release delegation reservation", move |tx| {
                delegations::delete(tx, delegation)?;
                Ok(((), false))
            })
            .await
            .map_err(storage_error)
    }

    /// Stops a child whose creation succeeded but whose delegation setup did not finish.
    ///
    /// Before the caller row commits, the reservation is invisible and can be released. After it
    /// commits, the durable row must remain and become terminal even when provider shutdown also
    /// fails, so capacity is never held by an unreachable live child.
    async fn compensate_created_child(
        &self,
        delegation: &Delegation,
        caller_item_committed: bool,
        reason: &'static str,
    ) -> Result<(), ProtoError> {
        let stop = self.inner.manager.stop(delegation.child).await;
        if !caller_item_committed {
            let release = self.release_reservation(delegation.id).await;
            return match (stop, release) {
                (_, Ok(())) => Ok(()),
                (Ok(_), Err(error)) | (Err(_), Err(error)) => Err(error),
            };
        }
        if stop.is_ok() {
            return Ok(());
        }

        let id = delegation.id;
        let now = Utc::now();
        let reason = reason.to_owned();
        let changed = self
            .inner
            .store
            .delegation_write("terminalize failed delegation startup", move |tx| {
                let Some(mut current) = delegations::get(tx, id)? else {
                    anyhow::bail!("delegation {id} does not exist");
                };
                if !current.status.is_terminal() {
                    current.status = DelegationStatus::Failed;
                    current.status_payload = Some(reason);
                    current.finished = Some(now);
                    delegations::update(tx, &current)?;
                    delegations::mark_done_for(tx, id, OutboxAction::Recover, now)?;
                    delegations::enqueue(tx, id, OutboxAction::Deliver, now)?;
                }
                Ok((current, true))
            })
            .await
            .map_err(storage_error)?;
        self.publish_changed(changed);
        Ok(())
    }
}

fn with_cleanup(mut original: ProtoError, cleanup: Result<(), ProtoError>) -> ProtoError {
    if let Err(error) = cleanup {
        original.message = format!(
            "{}; child cleanup also failed: {}",
            original.message, error.message
        );
    }
    original
}

fn storage_error(error: anyhow::Error) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Fs,
        message: one_line(&format!("native-agent storage failed: {error:#}")),
    }
}

fn worktree_error(worktree: &fleet_core::ids::WorktreeId, error: DaemonError) -> ProtoError {
    let message =
        format!("worktree-resolution rule: worktree {worktree} could not be resolved: {error}");
    match error {
        DaemonError::NotFound(_) => not_found(message),
        DaemonError::Remote(_) | DaemonError::Unsupported(_) => unsupported(message),
        _ => validation(message),
    }
}

/// How a card run reports a worktree it cannot use.
///
/// The remote refusal is restated in automation's own words: it is not a transport failure a
/// retry could clear, it is a standing property of the worktree, and a board prints this sentence
/// on the card as the reason its run never started. `host` is the owner when the state store
/// could still be read for it; without it the sentence keeps its shape and says `another host`.
fn card_worktree_error(
    worktree: &fleet_core::ids::WorktreeId,
    error: DaemonError,
    host: Option<String>,
) -> DaemonError {
    if matches!(&error, DaemonError::Unsupported(message) if message == REMOTE_UNSUPPORTED) {
        return DaemonError::Unsupported(host.map_or_else(
            || "automation is unavailable on a worktree owned by another host".to_owned(),
            |host| format!("automation is unavailable on a worktree owned by host {host}"),
        ));
    }
    let message =
        format!("worktree-resolution rule: worktree {worktree} could not be resolved: {error}");
    match error {
        DaemonError::NotFound(_) => DaemonError::NotFound(message),
        DaemonError::Remote(_) | DaemonError::Unsupported(_) => DaemonError::Unsupported(message),
        _ => DaemonError::Validation(message),
    }
}

fn not_found(message: impl Into<String>) -> ProtoError {
    error(ErrorKind::NotFound, message)
}

fn conflict(message: impl Into<String>) -> ProtoError {
    error(ErrorKind::Conflict, message)
}

fn validation(message: impl Into<String>) -> ProtoError {
    error(ErrorKind::Validation, message)
}

fn unsupported(message: impl Into<String>) -> ProtoError {
    error(ErrorKind::Unsupported, message)
}

fn error(kind: ErrorKind, message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind,
        message: one_line(&message.into()),
    }
}

fn one_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}
