//! Native-agent thread command group.

use std::io::Write;

use fleet_client::Client;
use fleet_core::agents::{
    AgentKind, Attention, GateAnswer, GateKind, ModelSelection, PermissionChoice, PermissionMode,
    PlanAnswer, Seq, SeqEvent, SessionState, ThreadProjection, UserInput,
};
use fleet_proto::{error::ProtoError, event::Event};

use super::{CommandOutput, unknown, validation, wait_event};
use crate::args::{
    AgentChoice, AgentModeChoice, AgentNewArgs, AgentRespondArgs, AgentSendArgs, AgentTailArgs,
    AgentThreadArgs,
};

pub(super) async fn list(client: &Client) -> Result<CommandOutput, ProtoError> {
    let threads = client.agent_thread_list().await?;
    let worktrees = client.get_snapshot().await?.worktrees;
    let text = threads
        .iter()
        .map(|thread| {
            let host = worktrees
                .iter()
                .find(|worktree| worktree.id == thread.worktree)
                .and_then(|worktree| worktree.host.as_ref())
                .map_or("local", |host| host.as_str());
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                thread.thread,
                thread.provider.executable(),
                host,
                session_word(thread.session),
                attention_word(thread.attention),
                thread.worktree,
                single_line(&thread.title)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CommandOutput::success(text))
}

pub(super) async fn new(
    client: &Client,
    arguments: AgentNewArgs,
) -> Result<CommandOutput, ProtoError> {
    let provider = match arguments.provider {
        AgentChoice::Claude => AgentKind::Claude,
        AgentChoice::Opencode => AgentKind::OpenCode,
    };
    let model = arguments
        .model
        .map(|model| model_selection(provider, model))
        .transpose()?;
    let mode = match arguments.mode {
        AgentModeChoice::Ask => PermissionMode::Ask,
        AgentModeChoice::AcceptEdits => PermissionMode::AcceptEdits,
        AgentModeChoice::Plan => PermissionMode::Plan,
        AgentModeChoice::FullAccess => PermissionMode::FullAccess,
    };
    let thread = client
        .agent_thread_create(arguments.worktree, provider, model, mode, None, None)
        .await?;
    Ok(CommandOutput::success(thread.thread.to_string()))
}

pub(super) async fn send(
    client: &Client,
    arguments: AgentSendArgs,
) -> Result<CommandOutput, ProtoError> {
    client
        .agent_send(
            arguments.thread,
            UserInput {
                text: arguments.text,
                attachments: Vec::new(),
            },
        )
        .await?;
    Ok(CommandOutput::success(String::new()))
}

pub(super) async fn respond(
    client: &Client,
    arguments: AgentRespondArgs,
) -> Result<CommandOutput, ProtoError> {
    let projection = read_projection(client, arguments.thread).await?;
    let gate = projection
        .gates
        .iter()
        .find(|gate| gate.id == arguments.gate)
        .ok_or_else(|| validation(format!("gate {} is not open", arguments.gate)))?;
    let answer = parse_answer(&gate.kind, &arguments.answer)?;
    client
        .agent_respond(arguments.thread, arguments.gate, answer)
        .await?;
    Ok(CommandOutput::success(String::new()))
}

pub(super) async fn interrupt(
    client: &Client,
    arguments: AgentThreadArgs,
) -> Result<CommandOutput, ProtoError> {
    client.agent_interrupt(arguments.thread).await?;
    Ok(CommandOutput::success(String::new()))
}

pub(super) async fn stop(
    client: &Client,
    arguments: AgentThreadArgs,
) -> Result<CommandOutput, ProtoError> {
    client.agent_stop(arguments.thread).await?;
    Ok(CommandOutput::success(String::new()))
}

pub(super) async fn tail(
    client: &Client,
    arguments: AgentTailArgs,
) -> Result<CommandOutput, ProtoError> {
    let mut stdout = std::io::stdout().lock();
    tail_to(client, arguments, &mut stdout).await?;
    Ok(CommandOutput::success(String::new()))
}

async fn tail_to(
    client: &Client,
    arguments: AgentTailArgs,
    output: &mut impl Write,
) -> Result<(), ProtoError> {
    let mut events = client.events();
    // A cursored open is a catch-up, which the daemon never resumes a provider for (§6): a tail
    // is a reader, and reading a thread must not start a `claude --resume` nobody asked for.
    let snapshot = client
        .agent_thread_open(arguments.thread, Some(Seq(0)))
        .await?;
    let mut projection = snapshot.projection;
    if arguments.replay {
        if !print_tail(&mut projection, snapshot.events_after, output)? {
            return Ok(());
        }
    } else {
        advance(&mut projection, snapshot.events_after)?;
    }
    if terminal(&projection) {
        return Ok(());
    }

    loop {
        let next = wait_event(
            &mut events,
            |event| matches!(event, Event::Agent { thread, .. } if *thread == arguments.thread),
        )
        .await?;
        match next {
            Some(Event::Agent { event, .. }) if event.seq <= projection.last_seq => {}
            Some(Event::Agent { event, .. }) if event.seq == projection.last_seq.next() => {
                if !print_tail(&mut projection, vec![event], output)? || terminal(&projection) {
                    return Ok(());
                }
            }
            Some(Event::Agent { .. }) | None => {
                let snapshot = client
                    .agent_thread_open(arguments.thread, Some(projection.last_seq))
                    .await?;
                projection = snapshot.projection;
                if !print_tail(&mut projection, snapshot.events_after, output)?
                    || terminal(&projection)
                {
                    return Ok(());
                }
            }
            Some(_) => {}
        }
    }
}

/// Reads a thread's current projection without asking the daemon to resume it.
///
/// §6 resumes a stopped thread "the next time it is opened", which is a user opening a tab. A
/// CLI verb that only needs to look something up is not that, so it opens with a cursor.
async fn read_projection(
    client: &Client,
    thread: fleet_core::agents::ThreadId,
) -> Result<ThreadProjection, ProtoError> {
    let snapshot = client.agent_thread_open(thread, Some(Seq(0))).await?;
    let mut projection = snapshot.projection;
    advance(&mut projection, snapshot.events_after)?;
    Ok(projection)
}

/// Replays events into a projection without printing them.
fn advance(projection: &mut ThreadProjection, events: Vec<SeqEvent>) -> Result<(), ProtoError> {
    for event in events {
        if event.seq <= projection.last_seq {
            continue;
        }
        projection
            .apply(&event)
            .map_err(|error| unknown(format!("could not apply agent event: {error}")))?;
    }
    Ok(())
}

fn print_tail(
    projection: &mut ThreadProjection,
    events: Vec<SeqEvent>,
    output: &mut impl Write,
) -> Result<bool, ProtoError> {
    for event in events {
        if event.seq <= projection.last_seq {
            continue;
        }
        projection
            .apply(&event)
            .map_err(|error| unknown(format!("could not apply agent event: {error}")))?;
        let json = serde_json::to_string(&event)
            .map_err(|error| unknown(format!("could not serialize agent event: {error}")))?;
        match writeln!(output, "{json}").and_then(|()| output.flush()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => return Ok(false),
            Err(error) => {
                return Err(unknown(format!("could not write agent event: {error}")));
            }
        }
    }
    Ok(true)
}

fn parse_answer(kind: &GateKind, words: &[String]) -> Result<GateAnswer, ProtoError> {
    let Some(action) = words.first().map(|word| word.to_ascii_lowercase()) else {
        return Err(validation("an answer is required"));
    };
    match kind {
        GateKind::Permission { .. } => {
            let (choice, edited_payload) = match action.as_str() {
                "once" | "allow-once" | "y" => (PermissionChoice::AllowOnce, None),
                "session" | "allow-session" | "a" => (PermissionChoice::AllowSession, None),
                "directory" | "allow-directory" => (PermissionChoice::AllowDirectory, None),
                "deny" | "n" => (PermissionChoice::Deny, None),
                "deny-and-stop" | "stop" => (PermissionChoice::DenyAndStop, None),
                "edit" | "e" => {
                    let payload = words[1..].join(" ");
                    if payload.is_empty() {
                        return Err(validation("edit requires a replacement payload"));
                    }
                    (PermissionChoice::Edit, Some(payload))
                }
                _ => {
                    return Err(validation(
                        "permission answer must be once, session, directory, deny, edit, or deny-and-stop",
                    ));
                }
            };
            Ok(GateAnswer::Permission {
                choice,
                edited_payload,
            })
        }
        GateKind::Question { questions } => {
            let answers = if questions.len() == 1 {
                vec![words.to_vec()]
            } else {
                if words.len() != questions.len() {
                    return Err(validation(format!(
                        "question gate expects {} answers, got {}",
                        questions.len(),
                        words.len()
                    )));
                }
                words
                    .iter()
                    .map(|word| word.split(',').map(str::to_owned).collect())
                    .collect()
            };
            Ok(GateAnswer::Question { answers })
        }
        GateKind::Plan { .. } => match action.as_str() {
            "approve" | "y" => Ok(GateAnswer::Plan(PlanAnswer::Approve)),
            "changes" | "change" | "n" => {
                let note = words[1..].join(" ");
                if note.is_empty() {
                    return Err(validation("plan changes require a note"));
                }
                Ok(GateAnswer::Plan(PlanAnswer::AskForChanges { note }))
            }
            _ => Err(validation("plan answer must be approve or changes <note>")),
        },
    }
}

fn model_selection(provider: AgentKind, value: String) -> Result<ModelSelection, ProtoError> {
    if value.trim().is_empty() {
        return Err(validation("model cannot be empty"));
    }
    if provider == AgentKind::OpenCode {
        let (provider, model) = value.split_once('/').ok_or_else(|| {
            validation("OpenCode models use provider/model, for example anthropic/claude-sonnet-4")
        })?;
        if provider.is_empty() || model.is_empty() {
            return Err(validation("OpenCode models use provider/model"));
        }
        return Ok(ModelSelection {
            model: model.to_owned(),
            effort: None,
            provider: Some(provider.to_owned()),
        });
    }
    Ok(ModelSelection {
        model: value,
        effort: None,
        provider: None,
    })
}

fn terminal(projection: &ThreadProjection) -> bool {
    matches!(
        projection.session,
        SessionState::Stopped | SessionState::Error
    ) || matches!(projection.turn, fleet_core::agents::TurnState::Failed(_))
}

fn session_word(state: SessionState) -> &'static str {
    match state {
        SessionState::Starting => "starting",
        SessionState::Ready => "ready",
        SessionState::Running => "running",
        SessionState::Stopped => "stopped",
        SessionState::Error => "error",
    }
}

fn attention_word(attention: Attention) -> &'static str {
    match attention {
        Attention::NeedsYou(_) => "needs-you",
        Attention::Failed => "failed",
        Attention::Working => "working",
        Attention::Unread => "unread",
        Attention::Idle => "idle",
    }
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::{Question, QuestionOption};

    use super::*;

    #[test]
    fn parses_permission_plan_and_question_answers() {
        let permission = GateKind::Permission {
            tool: fleet_core::agents::ToolKind::Bash,
            title: "run".to_owned(),
            payload: "echo old".to_owned(),
            rationale: None,
            options: Vec::new(),
        };
        assert!(matches!(
            parse_answer(&permission, &["once".to_owned()]),
            Ok(GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                ..
            })
        ));
        let plan = GateKind::Plan {
            markdown: "plan".to_owned(),
            steps: Vec::new(),
        };
        assert_eq!(
            parse_answer(&plan, &["changes".to_owned(), "add tests".to_owned()]),
            Ok(GateAnswer::Plan(PlanAnswer::AskForChanges {
                note: "add tests".to_owned()
            }))
        );
        let question = GateKind::Question {
            questions: vec![Question {
                text: "which?".to_owned(),
                header: "choice".to_owned(),
                options: vec![QuestionOption {
                    label: "one".to_owned(),
                    description: String::new(),
                }],
                multi_select: false,
                allow_other: true,
            }],
        };
        assert_eq!(
            parse_answer(&question, &["custom answer".to_owned()]),
            Ok(GateAnswer::Question {
                answers: vec![vec!["custom answer".to_owned()]]
            })
        );
    }
}
