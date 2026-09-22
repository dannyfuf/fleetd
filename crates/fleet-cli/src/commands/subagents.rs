//! Delegated native-agent command group.

use std::{collections::BTreeMap, io::Read, path::Path, time::SystemTime};

use fleet_client::{Client, DelegationRunRequest};
use fleet_core::agents::{AgentKind, Delegation, ModelSelection, PermissionMode, ThreadId};
use fleet_proto::{agents::ITEM_BODY_MAX_CHUNK_BYTES, error::ProtoError};

use super::{CommandOutput, validation};
use crate::{
    args::{
        AgentChoice, AgentModeChoice, SubagentCommand, SubagentCompleteArgs, SubagentIdArgs,
        SubagentListArgs, SubagentRunArgs, SubagentWaitArgs,
    },
    envelope::{PROTOCOL, SubagentEnvelope, SubagentsEnvelope, to_json},
    human,
};

const RESULT_CAP_BYTES: usize = ITEM_BODY_MAX_CHUNK_BYTES as usize;

/// How much of a brief the eliding envelopes keep.
///
/// Enough to recognise which delegation a line belongs to, and far too little to be mistaken for
/// the brief itself. `fleet subagent status --json` is where the whole one lives.
const BRIEF_PREVIEW_CHARS: usize = 200;

/// Environment-variable names a caller may never set on a child.
///
/// `FLEET_*` is the delegation's own identity — `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN`
/// are what `fleet subagent complete` authenticates with — and the daemon overwrites them anyway,
/// so accepting one here would only mislead. The prefix is refused rather than the two exact
/// names so a variable Fleet adds later is refused by the rule that already exists.
const RESERVED_ENV_PREFIX: &str = "FLEET_";

/// Process context injected into delegated child sessions by the daemon.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Environment {
    pub(super) session: Option<String>,
    pub(super) delegation: Option<String>,
    pub(super) token: Option<String>,
}

impl Environment {
    pub(super) fn from_process() -> Self {
        Self {
            session: std::env::var("FLEET_SESSION").ok(),
            delegation: std::env::var("FLEET_DELEGATION").ok(),
            token: std::env::var("FLEET_DELEGATION_TOKEN").ok(),
        }
    }

    /// The card a column-started run is working on, as the daemon injects it (`FLEET_CARD`).
    ///
    /// It sits beside the other injected names rather than in the board command that reads it,
    /// so the variables the daemon owns are all declared in one place. It is not a field of
    /// `Environment`, because nothing validates it: `fleet board card move` compares it to the
    /// key the caller typed and every other verb ignores it.
    pub(super) fn card_from_process() -> Option<String> {
        std::env::var("FLEET_CARD").ok()
    }
}

pub(super) fn validate_context(
    command: &SubagentCommand,
    environment: &Environment,
) -> Result<(), ProtoError> {
    match command {
        SubagentCommand::Run(arguments) => {
            fallback_id(
                arguments.caller,
                environment.session.as_deref(),
                "fleet subagent run requires --caller <thread> or FLEET_SESSION",
                "FLEET_SESSION",
            )?;
        }
        SubagentCommand::Complete(arguments) => {
            fallback_id(
                arguments.id,
                environment.delegation.as_deref(),
                "fleet subagent complete requires <id> or FLEET_DELEGATION",
                "FLEET_DELEGATION",
            )?;
            required_id::<ThreadId>(
                environment.session.as_deref(),
                "fleet subagent complete requires FLEET_SESSION",
                "FLEET_SESSION",
            )?;
            required_value(
                environment.token.as_deref(),
                "fleet subagent complete requires FLEET_DELEGATION_TOKEN",
            )?;
        }
        SubagentCommand::Wait(arguments) => {
            // A missing caller is not an error — a wait from a plain shell simply consumes
            // nothing — but a *malformed* FLEET_SESSION is, because silently ignoring it would
            // turn a typo into a delegation whose result is delivered twice.
            optional_caller(arguments.caller, environment.session.as_deref())?;
        }
        SubagentCommand::Status(_) | SubagentCommand::List(_) | SubagentCommand::Cancel(_) => {}
    }
    Ok(())
}

pub(super) async fn execute(
    client: &Client,
    command: SubagentCommand,
    environment: &Environment,
) -> Result<CommandOutput, ProtoError> {
    match command {
        SubagentCommand::Run(arguments) => run(client, arguments, environment).await,
        SubagentCommand::Complete(arguments) => complete(client, arguments, environment).await,
        SubagentCommand::Wait(arguments) => wait(client, arguments, environment).await,
        SubagentCommand::Status(arguments) => status(client, arguments).await,
        SubagentCommand::List(arguments) => list(client, arguments).await,
        SubagentCommand::Cancel(arguments) => cancel(client, arguments).await,
    }
}

async fn run(
    client: &Client,
    arguments: SubagentRunArgs,
    environment: &Environment,
) -> Result<CommandOutput, ProtoError> {
    let caller = fallback_id(
        arguments.caller,
        environment.session.as_deref(),
        "fleet subagent run requires --caller <thread> or FLEET_SESSION",
        "FLEET_SESSION",
    )?;
    let brief = read_text(arguments.brief_file.as_deref(), "brief")?;
    let provider = provider(arguments.provider);
    let mode = arguments.mode.map(permission_mode);
    let model = model_selection(arguments.model, arguments.effort)?;
    let env = child_environment(&arguments.env)?;
    let (delegation, warning) = client
        .delegation_run(DelegationRunRequest {
            caller,
            provider,
            brief,
            expectation: arguments.expectation,
            worktree: arguments.worktree,
            mode,
            model,
            title: arguments.title,
            fleet_path: caller_fleet_path(),
            env,
            eager: arguments.eager,
        })
        .await?;
    if arguments.json {
        return Ok(CommandOutput::success(elided_envelope(
            delegation,
            warning.as_deref(),
        )?));
    }
    let mut text = format!(
        "delegation {} started, child thread {}",
        delegation.id, delegation.child
    );
    if let Some(warning) = warning {
        text.push('\n');
        text.push_str(&warning);
    }
    Ok(CommandOutput::success(text))
}

/// The absolute path of the `fleet` this process is, for the daemon to hand a child.
///
/// Sent as a hint so a delegated child can run a bare `fleet subagent complete`; the daemon
/// prepends the *directory* this names to the child's `PATH`. Whatever file name the
/// orchestrator invoked us as is sent verbatim — by definition it is a `fleet` that speaks this
/// protocol version, which is the only property the daemon needs of it.
///
/// Both steps are fallible and neither is worth failing a delegation over: without the hint the
/// daemon falls back to its own resolution and the child behaves exactly as it did before the
/// field existed. So every failure — no `/proc`-equivalent answer, a deleted binary, a path that
/// is not UTF-8 and therefore has no wire representation — degrades to `None`. The CLI has no
/// `tracing` subscriber and its stdout is a machine-readable envelope, so there is nowhere to
/// report this that a caller would benefit from reading.
fn caller_fleet_path() -> Option<String> {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(_) => return None,
    };
    // Canonicalising resolves a symlinked launcher to the real binary, so the directory the
    // daemon derives is the one that actually holds it.
    let resolved = match executable.canonicalize() {
        Ok(resolved) => resolved,
        Err(_) => return None,
    };
    resolved.to_str().map(str::to_owned)
}

async fn complete(
    client: &Client,
    arguments: SubagentCompleteArgs,
    environment: &Environment,
) -> Result<CommandOutput, ProtoError> {
    let delegation = fallback_id(
        arguments.id,
        environment.delegation.as_deref(),
        "fleet subagent complete requires <id> or FLEET_DELEGATION",
        "FLEET_DELEGATION",
    )?;
    let child = required_id(
        environment.session.as_deref(),
        "fleet subagent complete requires FLEET_SESSION",
        "FLEET_SESSION",
    )?;
    let token = required_value(
        environment.token.as_deref(),
        "fleet subagent complete requires FLEET_DELEGATION_TOKEN",
    )?;
    let mut result = read_text(arguments.result_file.as_deref(), "result")?;
    if arguments.json_result {
        serde_json::from_str::<serde_json::Value>(&result)
            .map_err(|error| validation(format!("result is not valid JSON: {error}")))?;
    }
    let original_bytes = result.len();
    let notice = if original_bytes > RESULT_CAP_BYTES {
        truncate_utf8(&mut result, RESULT_CAP_BYTES);
        Some(format!(
            "result exceeded {RESULT_CAP_BYTES} bytes and was truncated"
        ))
    } else {
        None
    };
    let delegation = client
        .delegation_complete(delegation, child, token, result, arguments.blocked)
        .await?;
    let text = if arguments.json {
        whole_envelope(&delegation)?
    } else {
        "reported".to_owned()
    };
    Ok(CommandOutput::success(text).with_stderr(notice))
}

async fn wait(
    client: &Client,
    arguments: SubagentWaitArgs,
    environment: &Environment,
) -> Result<CommandOutput, ProtoError> {
    // Naming the delegation's own caller is what marks the result read, so the daemon does not
    // also inject this report into that thread's transcript. Anyone else's wait reads it.
    let caller = optional_caller(arguments.caller, environment.session.as_deref())?;
    let delegation = client
        .delegation_wait(
            arguments.id,
            arguments.timeout.saturating_mul(1_000),
            caller,
        )
        .await?;
    let terminal = delegation.status.is_terminal();
    // `delivered_message` is the *terminal* template: it opens with "finished:" and prints a
    // report body. Rendering it for a timed-out wait told the caller its live child had finished
    // running, so the non-terminal answer gets its own line. JSON is unchanged either way — the
    // envelope already carries the status, and callers parse it.
    let text = if arguments.json {
        elided_envelope(delegation, None)?
    } else if terminal {
        human::delivered_message(&delegation, SystemTime::now())
    } else {
        human::still_running_message(&delegation, SystemTime::now())
    };
    Ok(CommandOutput::with_exit_code(
        text,
        if terminal { 0 } else { 2 },
    ))
}

async fn status(client: &Client, arguments: SubagentIdArgs) -> Result<CommandOutput, ProtoError> {
    let delegation = client.delegation_get(arguments.id).await?;
    // The one verb that answers the whole record, in both renderings: the brief is not elided
    // here and the child's report is printed in full, which is what makes a missed `wait`
    // recoverable.
    let text = if arguments.json {
        whole_envelope(&delegation)?
    } else {
        let keys = caller_keys(client, std::slice::from_ref(&delegation)).await;
        human::subagent_status_with_keys(&delegation, SystemTime::now(), &keys)
    };
    Ok(CommandOutput::success(text))
}

/// The display key of every card that called one of these delegations.
///
/// A delegation names its caller by id; the key a person reads is the board's. One `GetBoard`
/// per distinct board answers every card on it, and a board that cannot be read leaves its cards
/// printing the id they already would have — a listing must not fail because of its own chrome.
async fn caller_keys(client: &Client, delegations: &[Delegation]) -> human::CallerKeys {
    let mut keys = human::CallerKeys::new();
    let mut seen: Vec<fleet_core::ids::BoardId> = Vec::new();
    for delegation in delegations {
        let Some((board, _)) = delegation.caller.card() else {
            continue;
        };
        if seen.iter().any(|known| known == board) {
            continue;
        }
        seen.push(board.clone());
        let Ok(view) = client.get_board(board.clone()).await else {
            continue;
        };
        for card in &view.cards {
            keys.insert(card.id.clone(), card.display_key(&view.board));
        }
    }
    keys
}

async fn list(client: &Client, arguments: SubagentListArgs) -> Result<CommandOutput, ProtoError> {
    let mut delegations = client.delegation_list(arguments.caller).await?;
    let text = if arguments.json {
        let mut elided = false;
        for delegation in &mut delegations {
            // Every brief is cut, and the flag is true if any of them was. Deliberately not
            // `any`: that short-circuits on the first long brief and would leave every later
            // one in the list whole, which is the opposite of what the flag then claims.
            elided |= elide_brief(delegation);
        }
        to_json(&SubagentsEnvelope {
            protocol: PROTOCOL,
            delegations: &delegations,
            brief_elided: elided,
        })?
    } else {
        let keys = caller_keys(client, &delegations).await;
        human::subagents_with_keys(&delegations, SystemTime::now(), &keys)
    };
    Ok(CommandOutput::success(text))
}

async fn cancel(client: &Client, arguments: SubagentIdArgs) -> Result<CommandOutput, ProtoError> {
    let delegation = client.delegation_cancel(arguments.id).await?;
    if arguments.json {
        // `cancel` shares `status`'s contract: one whole record, brief included.
        Ok(CommandOutput::success(whole_envelope(&delegation)?))
    } else {
        Ok(CommandOutput::success("cancelled".to_owned()))
    }
}

/// The JSON envelope for one delegation, carrying its brief whole.
///
/// `complete`, `status` and `cancel` all answer the record as it stands and none of them has a
/// warning to attach, so this helper takes neither a flag nor one.
fn whole_envelope(delegation: &Delegation) -> Result<String, ProtoError> {
    to_json(&SubagentEnvelope {
        protocol: PROTOCOL,
        delegation,
        warning: None,
        brief_elided: false,
    })
}

/// The JSON envelope for one delegation, with its brief cut to a preview.
///
/// Takes the record by value because the cut is made on this copy: the wire still carries the
/// brief whole, and `status` still answers it whole from the same daemon record.
fn elided_envelope(
    mut delegation: Delegation,
    warning: Option<&str>,
) -> Result<String, ProtoError> {
    let elided = elide_brief(&mut delegation);
    to_json(&SubagentEnvelope {
        protocol: PROTOCOL,
        delegation: &delegation,
        warning,
        brief_elided: elided,
    })
}

/// Cuts a brief to [`BRIEF_PREVIEW_CHARS`] characters, reporting whether anything was removed.
///
/// Counted in characters and cut on their boundary, so a brief of multi-byte text is shortened
/// rather than made invalid. A brief already short enough is untouched and the envelope then
/// omits `briefElided` entirely, which is the truthful answer: that caller did get the whole one.
fn elide_brief(delegation: &mut Delegation) -> bool {
    let Some((boundary, _)) = delegation.brief.char_indices().nth(BRIEF_PREVIEW_CHARS) else {
        return false;
    };
    delegation.brief.truncate(boundary);
    true
}

/// Parses repeated `--env KEY=VALUE` flags into the map the child is started with.
///
/// Every refusal names the offending key, because the caller is usually a program assembling
/// these from a template and the key is the only part of the pair it can act on.
fn child_environment(pairs: &[String]) -> Result<BTreeMap<String, String>, ProtoError> {
    let mut environment = BTreeMap::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(validation(format!(
                "--env {pair} is not KEY=VALUE: every entry needs an `=`"
            )));
        };
        if key.is_empty() {
            return Err(validation(format!("--env {pair} has an empty key")));
        }
        if key.starts_with(RESERVED_ENV_PREFIX) {
            return Err(validation(format!(
                "--env {key} is refused: {RESERVED_ENV_PREFIX}* names are the delegation's own identity and the daemon sets them itself"
            )));
        }
        if key == "PATH" {
            return Err(validation(
                "--env PATH is refused: an entry replaces the value outright rather than extending the login shell's, and Fleet already prepends the directory holding this fleet so the child can run `fleet subagent complete`",
            ));
        }
        // Keeping the last of a repeated key would turn a template that emitted one twice into a
        // child silently missing a variable, so say so instead.
        if environment
            .insert(key.to_owned(), value.to_owned())
            .is_some()
        {
            return Err(validation(format!("--env {key} is given twice")));
        }
    }
    Ok(environment)
}

/// Resolves the thread a `wait` is issued for: the flag, else `FLEET_SESSION`, else nobody.
fn optional_caller(
    explicit: Option<ThreadId>,
    fallback: Option<&str>,
) -> Result<Option<ThreadId>, ProtoError> {
    if let Some(caller) = explicit {
        return Ok(Some(caller));
    }
    match fallback.filter(|value| !value.trim().is_empty()) {
        Some(value) => value
            .parse()
            .map(Some)
            .map_err(|error| validation(format!("invalid FLEET_SESSION: {error}"))),
        None => Ok(None),
    }
}

fn provider(choice: AgentChoice) -> AgentKind {
    match choice {
        AgentChoice::Claude => AgentKind::Claude,
        AgentChoice::Codex => AgentKind::Codex,
    }
}

fn permission_mode(choice: AgentModeChoice) -> PermissionMode {
    match choice {
        AgentModeChoice::Ask => PermissionMode::Ask,
        AgentModeChoice::AcceptEdits => PermissionMode::AcceptEdits,
        AgentModeChoice::Plan => PermissionMode::Plan,
        AgentModeChoice::Auto => PermissionMode::Auto,
        AgentModeChoice::DontAsk => PermissionMode::DontAsk,
        AgentModeChoice::FullAccess => PermissionMode::FullAccess,
    }
}

/// Builds the optional `ModelSelection` from `--model` and `--effort`.
///
/// Neither flag means no selection at all, which is what lets the daemon apply its configured
/// per-provider defaults; naming only the model keeps that default effort, because `create_with`
/// fills an absent effort and leaves a stated one alone.
///
/// An effort without a model is sent as a selection whose `model` is the empty string, which is
/// that field's documented "keep the configured default" sentinel: the daemon fills it from the
/// per-provider default in `create_with`, and an adapter handed an empty one names no model but
/// still spends the effort. A *present but blank* `--model` is still a validation error — the
/// caller typed the flag, so they meant something by it, and silently reading `--model ""` as
/// "the default" would hide a shell-quoting mistake.
fn model_selection(
    model: Option<String>,
    effort: Option<String>,
) -> Result<Option<ModelSelection>, ProtoError> {
    if effort
        .as_ref()
        .is_some_and(|effort| effort.trim().is_empty())
    {
        return Err(validation("effort cannot be empty"));
    }
    if model.as_ref().is_some_and(|model| model.trim().is_empty()) {
        return Err(validation("model cannot be empty"));
    }
    match (model, effort) {
        (None, None) => Ok(None),
        (model, effort) => Ok(Some(ModelSelection {
            model: model.unwrap_or_default(),
            effort,
            provider: None,
        })),
    }
}

fn fallback_id<T>(
    explicit: Option<T>,
    fallback: Option<&str>,
    missing: &str,
    variable: &str,
) -> Result<T, ProtoError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match explicit {
        Some(value) => Ok(value),
        None => required_id(fallback, missing, variable),
    }
}

fn required_id<T>(value: Option<&str>, missing: &str, variable: &str) -> Result<T, ProtoError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = required_value(value, missing)?;
    value
        .parse()
        .map_err(|error| validation(format!("invalid {variable}: {error}")))
}

fn required_value(value: Option<&str>, missing: &str) -> Result<String, ProtoError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| validation(missing))
}

/// Reads `name`'s text from `path`, or from stdin when no path was given.
///
/// `pub(super)` because `board card new --desc-file` reads a description exactly as
/// `subagent run --brief-file` reads a brief, down to the refusal text.
pub(super) fn read_text(path: Option<&Path>, name: &str) -> Result<String, ProtoError> {
    match path {
        Some(path) => std::fs::read_to_string(path).map_err(|error| read_error(name, path, error)),
        None => {
            let mut text = String::new();
            std::io::stdin()
                .lock()
                .read_to_string(&mut text)
                .map_err(|error| {
                    validation(format!("could not read {name} from stdin: {error}"))
                })?;
            Ok(text)
        }
    }
}

fn read_error(name: &str, path: &Path, error: std::io::Error) -> ProtoError {
    validation(format!(
        "could not read {name} file {}: {error}",
        path.display()
    ))
}

fn truncate_utf8(value: &mut String, cap: usize) {
    let mut boundary = cap.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    value.truncate(boundary);
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::{DelegationId, ThreadId};

    use super::*;

    #[test]
    fn environment_fallbacks_are_validated_and_explicit_values_win() {
        let explicit: ThreadId = "00000000-0000-4000-8000-000000000001".parse().unwrap();
        assert_eq!(
            fallback_id(Some(explicit), Some("invalid"), "missing", "FLEET_SESSION").unwrap(),
            explicit
        );
        assert_eq!(
            fallback_id::<ThreadId>(
                None,
                Some("00000000-0000-4000-8000-000000000002"),
                "missing",
                "FLEET_SESSION"
            )
            .unwrap()
            .to_string(),
            "00000000-0000-4000-8000-000000000002"
        );
        assert!(
            fallback_id::<DelegationId>(None, None, "required fallback", "FLEET_DELEGATION")
                .unwrap_err()
                .message
                .contains("required fallback")
        );
    }

    #[test]
    fn a_model_selection_is_built_from_every_legal_flag_pairing() {
        assert_eq!(model_selection(None, None).unwrap(), None);
        assert_eq!(
            model_selection(Some("opus".to_owned()), None).unwrap(),
            Some(ModelSelection {
                model: "opus".to_owned(),
                effort: None,
                provider: None,
            })
        );
        assert_eq!(
            model_selection(Some("opus".to_owned()), Some("high".to_owned())).unwrap(),
            Some(ModelSelection {
                model: "opus".to_owned(),
                effort: Some("high".to_owned()),
                provider: None,
            })
        );
        // Free text, never an enum: the ladder belongs to the provider, which is the only thing
        // that can say a value is wrong.
        assert_eq!(
            model_selection(Some("opus".to_owned()), Some("xhigh".to_owned()))
                .unwrap()
                .and_then(|selection| selection.effort),
            Some("xhigh".to_owned())
        );
    }

    /// `--effort` alone rides on the empty-model sentinel rather than being refused or dropped.
    #[test]
    fn an_effort_without_a_model_asks_the_daemon_for_its_default_model() {
        assert_eq!(
            model_selection(None, Some("high".to_owned())).unwrap(),
            Some(ModelSelection {
                model: String::new(),
                effort: Some("high".to_owned()),
                provider: None,
            })
        );
    }

    #[test]
    fn a_blank_flag_value_is_a_validation_error() {
        for empty in ["", "   "] {
            assert!(
                model_selection(Some("opus".to_owned()), Some(empty.to_owned()))
                    .unwrap_err()
                    .message
                    .contains("effort cannot be empty")
            );
            assert!(
                model_selection(Some(empty.to_owned()), None)
                    .unwrap_err()
                    .message
                    .contains("model cannot be empty")
            );
        }
    }

    #[test]
    fn a_child_environment_is_sorted_and_keeps_every_legal_pair() {
        let environment = child_environment(&[
            "RUST_LOG=debug".to_owned(),
            "CARGO_TARGET_DIR=/tmp/child".to_owned(),
            // A value may hold anything, `=` and emptiness included: only the first `=` splits.
            "CONNECTION=host=db user=fleet".to_owned(),
            "EMPTY=".to_owned(),
        ])
        .unwrap();
        assert_eq!(
            environment.into_iter().collect::<Vec<_>>(),
            vec![
                ("CARGO_TARGET_DIR".to_owned(), "/tmp/child".to_owned()),
                ("CONNECTION".to_owned(), "host=db user=fleet".to_owned()),
                ("EMPTY".to_owned(), String::new()),
                ("RUST_LOG".to_owned(), "debug".to_owned()),
            ]
        );
        assert!(child_environment(&[]).unwrap().is_empty());
    }

    /// One refusal per rule, each naming what the caller has to fix.
    #[test]
    fn every_child_environment_refusal_names_the_offending_entry() {
        let refusal = |pair: &str| {
            child_environment(std::slice::from_ref(&pair.to_owned()))
                .unwrap_err()
                .message
        };

        assert_eq!(
            refusal("CARGO_TARGET_DIR"),
            "--env CARGO_TARGET_DIR is not KEY=VALUE: every entry needs an `=`"
        );
        assert_eq!(refusal("=orphan"), "--env =orphan has an empty key");
        // The delegation's own identity, which the daemon sets and the child authenticates with.
        for key in [
            "FLEET_DELEGATION",
            "FLEET_DELEGATION_TOKEN",
            "FLEET_SESSION",
        ] {
            assert_eq!(
                refusal(&format!("{key}=forged")),
                format!(
                    "--env {key} is refused: FLEET_* names are the delegation's own identity and the daemon sets them itself"
                )
            );
        }
        // An `env` entry is a whole-value override, so a PATH here would discard the login
        // shell's rather than extend it — and take Fleet's own prepended directory with it.
        assert!(refusal("PATH=/usr/bin").starts_with("--env PATH is refused:"));
        assert!(refusal("PATH=/usr/bin").contains("replaces the value outright"));

        assert_eq!(
            child_environment(&["A=1".to_owned(), "A=2".to_owned()])
                .unwrap_err()
                .message,
            "--env A is given twice"
        );
    }

    #[test]
    fn a_brief_is_cut_on_a_character_boundary_and_only_when_it_is_too_long() {
        let mut short = delegation_with_brief("brief");
        assert!(!elide_brief(&mut short));
        assert_eq!(short.brief, "brief");

        // Exactly the preview length is still whole: the cut is "more than fits", not "at least".
        let mut exact = delegation_with_brief(&"a".repeat(BRIEF_PREVIEW_CHARS));
        assert!(!elide_brief(&mut exact));
        assert_eq!(exact.brief.len(), BRIEF_PREVIEW_CHARS);

        // Multi-byte characters: the result is `BRIEF_PREVIEW_CHARS` *characters*, and it is
        // still valid UTF-8 because `truncate` is given a boundary rather than a byte count.
        let mut wide = delegation_with_brief(&"é".repeat(BRIEF_PREVIEW_CHARS + 10));
        assert!(elide_brief(&mut wide));
        assert_eq!(wide.brief.chars().count(), BRIEF_PREVIEW_CHARS);
        assert_eq!(wide.brief.len(), BRIEF_PREVIEW_CHARS * 2);
    }

    #[test]
    fn a_wait_resolves_its_caller_from_the_flag_then_the_session_then_nobody() {
        let explicit: ThreadId = "00000000-0000-4000-8000-000000000001".parse().unwrap();
        let session = "00000000-0000-4000-8000-000000000002";

        assert_eq!(
            optional_caller(Some(explicit), Some(session)).unwrap(),
            Some(explicit)
        );
        assert_eq!(
            optional_caller(None, Some(session)).unwrap(),
            Some(session.parse().unwrap())
        );
        // No session at all is not an error: the wait works and simply consumes nothing.
        assert_eq!(optional_caller(None, None).unwrap(), None);
        assert_eq!(optional_caller(None, Some("   ")).unwrap(), None);
        // A session that is present but unusable is, because ignoring it would silently deliver
        // the result a second time.
        assert!(
            optional_caller(None, Some("not-a-thread"))
                .unwrap_err()
                .message
                .starts_with("invalid FLEET_SESSION:")
        );
    }

    fn delegation_with_brief(brief: &str) -> Delegation {
        serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-4000-8000-000000000003",
            "caller": "00000000-0000-4000-8000-000000000001",
            "callerTurn": "00000000-0000-4000-8000-000000000004",
            "callerItem": "00000000-0000-4000-8000-000000000005",
            "child": "00000000-0000-4000-8000-000000000002",
            "provider": "codex",
            "depth": 1,
            "brief": brief,
            "expectation": "tests pass",
            "eager": false,
            "status": "running",
            "nudges": 0,
            "recoveries": 0,
            "delivery": {"type": "pending"},
            "created": "2026-09-18T12:00:00Z"
        }))
        .expect("the fixture names every required delegation field")
    }

    #[test]
    fn utf8_truncation_never_splits_a_character() {
        let mut value = format!("{}é", "a".repeat(RESULT_CAP_BYTES - 1));
        truncate_utf8(&mut value, RESULT_CAP_BYTES);
        assert_eq!(value.len(), RESULT_CAP_BYTES - 1);
    }
}
