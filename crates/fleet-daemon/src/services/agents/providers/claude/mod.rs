//! Claude Code provider over bidirectional stream-json.

mod map;
mod process;
mod wire;

use std::sync::Arc;

use async_trait::async_trait;
use fleet_core::agents::{
    AgentEvent, AgentKind, AttachmentSource, Capabilities, GateAnswer, GateId, ModelSelection,
    PermissionMode, StartRequest, TurnId, UserInput,
};
use map::{ClaudeMapper, permission_mode_to_wire};
use process::RunningProcess;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

use super::{
    AgentProvider, ProviderError, ProviderEvent, ProviderEvents, ProviderResult, ProviderSink,
    empty_events,
};

/// Claude Code stream-json provider.
#[derive(Debug)]
pub struct ClaudeProvider {
    command: String,
    fork_session: bool,
    process: Option<RunningProcess>,
    mapper: Arc<Mutex<ClaudeMapper>>,
    event_sender: ProviderSink,
    event_receiver: Option<ProviderEvents>,
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new(AgentKind::Claude.executable())
    }
}

impl ClaudeProvider {
    /// Creates a provider using the configured Claude shell command.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        Self {
            command: command.into(),
            fork_session: false,
            process: None,
            mapper: Arc::new(Mutex::new(ClaudeMapper::default())),
            event_sender,
            event_receiver: Some(event_receiver),
        }
    }

    /// Requests `--fork-session` when the next start resumes a cursor.
    #[must_use]
    pub fn fork_on_resume(mut self, fork: bool) -> Self {
        self.fork_session = fork;
        self
    }

    fn running(&self) -> ProviderResult<&RunningProcess> {
        self.process
            .as_ref()
            .ok_or_else(|| ProviderError::Unavailable {
                reason: "Claude provider has not been started".to_owned(),
            })
    }

    fn emit(&self, event: AgentEvent) -> ProviderResult<()> {
        self.event_sender
            .send(event.into())
            .map_err(|_| ProviderError::Exited { code: None })
    }
}

#[async_trait]
impl AgentProvider for ClaudeProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            resume: true,
            fork: true,
            steer: true,
            interrupt: true,
            modes: true,
            models: true,
        }
    }

    async fn start(&mut self, req: StartRequest) -> ProviderResult<()> {
        if self.process.is_some() {
            return Err(ProviderError::Protocol {
                message: "Claude provider was started more than once".to_owned(),
            });
        }
        if req.provider != AgentKind::Claude {
            return Err(ProviderError::Protocol {
                message: format!("Claude provider cannot start {:?}", req.provider),
            });
        }
        let environment = process::login_environment(&req.worktree_path).await;
        process::check_version(&self.command, &req.worktree_path, &environment).await?;
        let args = launch_args(&req, Uuid::new_v4(), self.fork_session);
        let process = process::spawn(
            &self.command,
            &args,
            &req.worktree_path,
            environment,
            req.thread.to_string(),
            Arc::clone(&self.mapper),
            self.event_sender.clone(),
        )
        .await?;
        if let Some(title) = req.title {
            process
                .writer
                .write(json!({
                    "type": "control_request",
                    "request_id": Uuid::new_v4().to_string(),
                    "request": {"subtype": "initialize", "title": title},
                }))
                .await?;
        }
        self.process = Some(process);
        Ok(())
    }

    /// Writes one user line, announcing the turn before the CLI can answer it.
    ///
    /// §3.3 rule 1 drops a delta or an item whose turn is not the active one, and the reader
    /// task maps frames against `active_turn` the moment `begin_turn` sets it. `TurnStarted`
    /// therefore has to reach the channel *before* the write, or the opening `ItemStarted` of a
    /// fast turn can overtake it and be rejected. A failed write settles the turn it announced
    /// rather than leaving the reducer with a turn nothing will ever complete.
    async fn send(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<()> {
        let line = user_message(input)?;
        // Resolved before the turn is announced: a provider that is not running must fail the
        // send without having started anything.
        let writer = self.running()?.writer.clone();
        let turn_started = self.mapper.lock().await.begin_turn(turn)?;
        let announced = turn_started.is_some();
        match turn_started {
            Some(event) => self.emit(event)?,
            // The write joined a running turn: it is a steer, and the supersede result the CLI
            // sends for it must not settle the turn (§3.3 rule 6).
            None => self.mapper.lock().await.record_steer(turn),
        }
        if let Err(error) = writer.write(line).await {
            if announced {
                self.mapper.lock().await.rollback_turn_start(turn);
                self.emit(AgentEvent::TurnAborted {
                    turn,
                    reason: fleet_core::agents::AbortReason::Other(
                        "the prompt could not be written to Claude Code".to_owned(),
                    ),
                })?;
            }
            return Err(error);
        }
        Ok(())
    }

    async fn interrupt(&mut self, turn: TurnId) -> ProviderResult<()> {
        self.mapper.lock().await.mark_interrupted(turn)?;
        self.running()?
            .writer
            .write(interrupt_message(Uuid::new_v4().to_string()))
            .await
    }

    async fn respond(&mut self, gate: GateId, answer: GateAnswer) -> ProviderResult<()> {
        let response = self.mapper.lock().await.response_for(gate, &answer)?;
        self.running()?.writer.write(response).await?;
        let resolved = self.mapper.lock().await.resolve_gate(gate, answer)?;
        self.emit(resolved)
    }

    async fn set_mode(&mut self, mode: PermissionMode) -> ProviderResult<()> {
        self.running()?
            .writer
            .write(json!({
                "type": "control_request",
                "request_id": Uuid::new_v4().to_string(),
                "request": {
                    "subtype": "set_permission_mode",
                    "mode": permission_mode_to_wire(mode),
                },
            }))
            .await
    }

    async fn set_model(&mut self, model: ModelSelection) -> ProviderResult<()> {
        self.running()?
            .writer
            .write(json!({
                "type": "control_request",
                "request_id": Uuid::new_v4().to_string(),
                "request": {"subtype": "set_model", "model": model.model},
            }))
            .await?;
        self.mapper.lock().await.set_model(&model.model);
        Ok(())
    }

    async fn stop(&mut self) -> ProviderResult<()> {
        let Some(process) = self.process.take() else {
            return Ok(());
        };
        if let Some(turn) = self.mapper.lock().await.active_turn() {
            self.mapper.lock().await.mark_interrupted(turn)?;
            let _ = process
                .writer
                .write(interrupt_message(Uuid::new_v4().to_string()))
                .await;
        }
        process.stop().await
    }

    fn events(&mut self) -> ProviderEvents {
        self.event_receiver.take().unwrap_or_else(empty_events)
    }
}

fn launch_args(req: &StartRequest, session_id: Uuid, fork_session: bool) -> Vec<String> {
    let mut args = vec![
        "-p".to_owned(),
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        "--input-format".to_owned(),
        "stream-json".to_owned(),
        "--verbose".to_owned(),
        "--include-partial-messages".to_owned(),
        "--permission-prompt-tool".to_owned(),
        "stdio".to_owned(),
        "--permission-mode".to_owned(),
        permission_mode_to_wire(req.mode).to_owned(),
    ];
    if let Some(model) = &req.model {
        args.push("--model".to_owned());
        args.push(model.model.clone());
    }
    if let Some(cursor) = &req.resume_cursor {
        args.push(format!("--resume={cursor}"));
        if fork_session {
            args.push("--fork-session".to_owned());
        }
    } else {
        args.push(format!("--session-id={session_id}"));
    }
    args
}

fn interrupt_message(request_id: String) -> Value {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {"subtype": "interrupt", "cancel_queued": true},
    })
}

fn user_message(input: UserInput) -> ProviderResult<Value> {
    if input.attachments.is_empty() {
        return Ok(json!({
            "type": "user",
            "message": {"role": "user", "content": input.text},
            "parent_tool_use_id": null,
            "isSynthetic": false,
            "priority": "now",
            "origin": {"kind": "human"},
            "shouldQuery": true,
            "uuid": Uuid::new_v4().to_string(),
        }));
    }
    let mut content = vec![json!({"type": "text", "text": input.text})];
    for attachment in input.attachments {
        let AttachmentSource::Base64(data) = attachment.source else {
            return Err(ProviderError::Protocol {
                message: format!(
                    "Claude stream-json requires base64 attachment data for {}",
                    attachment.name.as_deref().unwrap_or("attachment")
                ),
            });
        };
        let block_type = if attachment.media_type.starts_with("image/") {
            "image"
        } else {
            "document"
        };
        content.push(json!({
            "type": block_type,
            "source": {
                "type": "base64",
                "media_type": attachment.media_type,
                "data": data,
            }
        }));
    }
    Ok(json!({
        "type": "user",
        "message": {"role": "user", "content": content},
        "parent_tool_use_id": null,
        "isSynthetic": false,
        "priority": "now",
        "origin": {"kind": "human"},
        "shouldQuery": true,
        "uuid": Uuid::new_v4().to_string(),
    }))
}

#[cfg(test)]
mod tests;
