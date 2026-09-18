//! The Claude Code stream-json player.
//!
//! Transport and frame shapes come from `docs/research/harness-protocols.md` §A, verified there
//! against `claude --version` → `2.1.266 (Claude Code)`: UTF-8 NDJSON in both directions, one
//! complete JSON object per line, stdin held open across turns, and `result` as the single
//! authoritative per-turn terminal.
//!
//! Three details are load-bearing rather than decorative, and each one is a Fleet surface that
//! goes dark without it:
//!
//! - **`system/init` must declare `capabilities`.** `agents::claude::session` gates the whole
//!   agent tab on `msg_lifecycle_v1` appearing there, never on a version compare, and adopts the
//!   durable session id from the same frame as the resume cursor.
//! - **A gate is a `control_request { subtype: "can_use_tool" }`, and the tool name is the
//!   discriminator** — there is no separate frame kind. A command approval is `Bash`, an edit
//!   approval is `Edit`, and `agents::claude::map::gates` reads the card's payload out of `input`.
//! - **A streamed block is completed by its `assistant` snapshot.** Snapshots backfill, they never
//!   stream, so the player emits the stream events *and* the snapshot, exactly as the CLI does.

use serde_json::{Value, json};
use std::time::{Duration, Instant};

use super::{
    GATE_BUDGET, Transcript, TranscriptStep,
    ids::Ids,
    peer::Peer,
    transcript::{
        Catalogue, Playback, Settlement, diff_counts, diff_sides, run_shell, text_chunks,
    },
};

/// Plays one transcript as Claude Code.
pub(crate) fn play<R: std::io::BufRead + Send + 'static, W: std::io::Write>(
    transcript: &Transcript,
    peer: &mut Peer<R, W>,
) -> anyhow::Result<i32> {
    play_with_gate_budget(transcript, peer, GATE_BUDGET)
}

fn play_with_gate_budget<R: std::io::BufRead + Send + 'static, W: std::io::Write>(
    transcript: &Transcript,
    peer: &mut Peer<R, W>,
    gate_budget: Duration,
) -> anyhow::Result<i32> {
    let mut session = Session {
        transcript,
        catalogue: Catalogue::of(transcript),
        playback: Playback::new(transcript),
        ids: Ids::new(),
        interrupted: false,
        gate_budget,
        peer,
    };
    session.greet()?;
    loop {
        let Some(frame) = session.peer.read_frame()? else {
            return Ok(0);
        };
        match frame.get("type").and_then(Value::as_str) {
            // The prompt. One turn's worth of steps, then its `result`.
            Some("user") => {
                if let Some(code) = session.run_turn()? {
                    return Ok(code);
                }
            }
            Some("control_request") => session.answer_control(&frame)?,
            // No gate is open between turns, so a stray answer is not ours to correlate.
            _ => {}
        }
    }
}

/// What the client decided about one gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// `{behavior:"allow"}` — `[y] allow once` or `[a] allow for this session`.
    Allow,
    /// `{behavior:"deny", interrupt}` — `[n] deny`, or `[esc] deny and stop`.
    Deny {
        /// True for `deny and stop`, which ends the turn as well as the tool.
        interrupt: bool,
    },
    /// The client withdrew the request. Answer nothing and carry on.
    Withdrawn,
}

/// One scripted Claude session.
struct Session<'a, R, W> {
    transcript: &'a Transcript,
    catalogue: Catalogue,
    playback: Playback<'a>,
    ids: Ids,
    /// Set by an `interrupt` control request; consumed by the running turn.
    interrupted: bool,
    /// One absolute budget per open approval gate.
    gate_budget: Duration,
    peer: &'a mut Peer<R, W>,
}

impl<R: std::io::BufRead + Send + 'static, W: std::io::Write> Session<'_, R, W> {
    /// The `system/init` frame, emitted once as the process starts.
    ///
    /// The real CLI re-emits it every turn; once is enough for Fleet, whose `initialized` latch is
    /// idempotent, and one frame is one fewer thing for a snapshot diff to explain.
    fn greet(&mut self) -> anyhow::Result<()> {
        let uuid = self.ids.uuid(0x5157_0000);
        let cwd = std::env::current_dir().unwrap_or_default();
        let frame = json!({
            "type": "system",
            "subtype": "init",
            "uuid": uuid,
            "session_id": self.transcript.session_id,
            "cwd": cwd.to_string_lossy(),
            "claude_code_version": "2.1.266",
            "apiKeySource": "none",
            "model": self.catalogue.default_model(),
            // Advisory: the CLI reports the `manual` mode Fleet launches with as `default`.
            "permissionMode": "default",
            "tools": ["Read", "Edit", "Write", "Bash", "Grep", "Glob"],
            "mcp_servers": [],
            "slash_commands": ["compact", "clear"],
            "skills": [],
            "plugins": [],
            "output_style": "default",
            // The capability gate. Omitting `msg_lifecycle_v1` is how Fleet refuses a CLI that is
            // too old, so a stand-in that drops it refuses itself.
            "capabilities": [
                "interrupt_receipt_v1",
                "interrupt_cancel_queued_v1",
                "msg_lifecycle_v1",
            ],
        });
        self.peer.emit(&frame)
    }

    /// Plays one turn and settles it.
    ///
    /// `Some(code)` means the transcript asked the process to exit rather than settle.
    fn run_turn(&mut self) -> anyhow::Result<Option<i32>> {
        let script = self.playback.next_turn();
        let mut settlement = if std::mem::take(&mut self.interrupted) {
            Settlement::Interrupted
        } else {
            script.settlement
        };
        let mut failure: Option<String> = None;
        let mut final_text = String::new();
        // The `tool_use` a file change opened and an approval is about to gate.
        let mut pending_edit: Option<String> = None;
        for (index, step) in script.steps.iter().enumerate() {
            match step {
                TranscriptStep::Text { text, pace_ms } => {
                    self.stream_text(text, *pace_ms)?;
                    final_text.clone_from(text);
                }
                TranscriptStep::ToolCall {
                    id,
                    name,
                    arguments,
                    output,
                } => {
                    let tool_use = self.tool_use(name, arguments.clone(), Some(id.clone()))?;
                    self.tool_result(&tool_use, output, None, false)?;
                }
                TranscriptStep::Shell { command, name } => {
                    let result = run_shell(command)?;
                    let tool_use = self.tool_use(name, json!({ "command": command }), None)?;
                    self.tool_result(&tool_use, &result.output, None, !result.success)?;
                }
                TranscriptStep::FileChange { path, diff } => {
                    let tool_use = self.tool_use("Edit", edit_input(path, diff), None)?;
                    // An approval immediately after gates this change; otherwise the model simply
                    // wrote the file and the result lands now, in order.
                    if matches!(
                        script.steps.get(index + 1),
                        Some(TranscriptStep::Approval { .. })
                    ) {
                        pending_edit = Some(tool_use);
                    } else {
                        self.finish_edit(&tool_use, path, diff)?;
                    }
                }
                TranscriptStep::Permission { id, command } => {
                    let input = json!({ "command": command });
                    let tool_use = self.tool_use("Bash", input.clone(), None)?;
                    let decision = match self.ask(id, "Bash", command, &input, &tool_use) {
                        Ok(decision) => decision,
                        Err(error) => {
                            self.settle(Settlement::Interrupted, failure.as_deref(), &final_text)?;
                            return Err(error);
                        }
                    };
                    match decision {
                        Decision::Allow => self.tool_result(&tool_use, "", None, false)?,
                        Decision::Deny { interrupt } => {
                            self.permission_denied(&tool_use, "Bash")?;
                            if interrupt {
                                settlement = Settlement::Interrupted;
                                break;
                            }
                        }
                        Decision::Withdrawn if std::mem::take(&mut self.interrupted) => {
                            settlement = Settlement::Interrupted;
                            break;
                        }
                        Decision::Withdrawn => {}
                    }
                }
                TranscriptStep::Approval { id, summary } => {
                    let (Some(tool_use), Some(TranscriptStep::FileChange { path, diff })) = (
                        pending_edit.take(),
                        index
                            .checked_sub(1)
                            .and_then(|previous| script.steps.get(previous)),
                    ) else {
                        anyhow::bail!(
                            "an approval reached the player with no file change to gate; \
                             the transcript should have been rejected at load"
                        );
                    };
                    let decision =
                        match self.ask(id, "Edit", summary, &edit_input(path, diff), &tool_use) {
                            Ok(decision) => decision,
                            Err(error) => {
                                self.settle(
                                    Settlement::Interrupted,
                                    failure.as_deref(),
                                    &final_text,
                                )?;
                                return Err(error);
                            }
                        };
                    match decision {
                        Decision::Allow => self.finish_edit(&tool_use, path, diff)?,
                        Decision::Deny { interrupt } => {
                            self.permission_denied(&tool_use, "Edit")?;
                            if interrupt {
                                settlement = Settlement::Interrupted;
                                break;
                            }
                        }
                        Decision::Withdrawn if std::mem::take(&mut self.interrupted) => {
                            settlement = Settlement::Interrupted;
                            break;
                        }
                        Decision::Withdrawn => {}
                    }
                }
                // The catalogue answers `list_models`; playing the step emits nothing.
                TranscriptStep::Models { .. } => {}
                TranscriptStep::Error { message } => {
                    self.error_latch(message)?;
                    failure = Some(message.clone());
                    settlement = Settlement::Failed;
                }
                // `end_turn` never reaches the player: it is the boundary the script was cut on.
                TranscriptStep::EndTurn { .. } => {}
                TranscriptStep::Exit { code } => return Ok(Some(*code)),
            }
            if self.interrupted {
                self.interrupted = false;
                settlement = Settlement::Interrupted;
                break;
            }
        }
        self.settle(settlement, failure.as_deref(), &final_text)?;
        Ok(None)
    }

    /// Streams one assistant message: the partial events, then the snapshot that completes it.
    fn stream_text(&mut self, text: &str, pace_ms: u64) -> anyhow::Result<()> {
        let message = self.ids.message();
        let model = self.catalogue.default_model().to_owned();
        self.peer.emit(&json!({
            "type": "stream_event",
            "parent_tool_use_id": Value::Null,
            "session_id": self.transcript.session_id,
            "event": {
                "type": "message_start",
                "message": {
                    "id": message,
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [],
                    "stop_reason": Value::Null,
                    "usage": {"input_tokens": 12, "output_tokens": 0},
                },
            },
        }))?;
        self.peer.emit(&self.stream_event(json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""},
        })))?;
        let chunks = text_chunks(text);
        for chunk in &chunks {
            self.peer.pause(pace_ms);
            self.peer.emit(&self.stream_event(json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": chunk},
            })))?;
        }
        self.peer
            .emit(&self.stream_event(json!({"type": "content_block_stop", "index": 0})))?;
        self.peer.emit(&self.stream_event(json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": chunks.len()},
        })))?;
        self.peer
            .emit(&self.stream_event(json!({"type": "message_stop"})))?;
        let uuid = self.ids.uuid(0x5157_0001);
        self.peer.emit(&json!({
            "type": "assistant",
            "uuid": uuid,
            "session_id": self.transcript.session_id,
            "parent_tool_use_id": Value::Null,
            "message": {
                "id": message,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{"type": "text", "text": text}],
                "stop_reason": "end_turn",
            },
        }))
    }

    /// Wraps one Anthropic streaming event in its `stream_event` envelope.
    fn stream_event(&self, event: Value) -> Value {
        json!({
            "type": "stream_event",
            "parent_tool_use_id": Value::Null,
            "session_id": self.transcript.session_id,
            "event": event,
        })
    }

    /// An `assistant` snapshot carrying one `tool_use` block. Returns the block's id.
    fn tool_use(
        &mut self,
        name: &str,
        input: Value,
        provider_id: Option<String>,
    ) -> anyhow::Result<String> {
        let tool_use = provider_id.unwrap_or_else(|| self.ids.tool_use());
        let message = self.ids.message();
        let uuid = self.ids.uuid(0x5157_0002);
        self.peer.emit(&json!({
            "type": "assistant",
            "uuid": uuid,
            "session_id": self.transcript.session_id,
            "parent_tool_use_id": Value::Null,
            "message": {
                "id": message,
                "type": "message",
                "role": "assistant",
                "model": self.catalogue.default_model(),
                "content": [{"type": "tool_use", "id": tool_use, "name": name, "input": input}],
                "stop_reason": "tool_use",
            },
        }))?;
        Ok(tool_use)
    }

    /// The `user` frame carrying a `tool_result`, which is how a tool's output arrives.
    fn tool_result(
        &mut self,
        tool_use: &str,
        output: &str,
        structured: Option<Value>,
        is_error: bool,
    ) -> anyhow::Result<()> {
        let uuid = self.ids.uuid(0x5157_0003);
        let mut frame = json!({
            "type": "user",
            "uuid": uuid,
            "session_id": self.transcript.session_id,
            "parent_tool_use_id": Value::Null,
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": tool_use,
                    "content": output,
                    "is_error": is_error,
                }],
            },
        });
        if let (Some(structured), Some(object)) = (structured, frame.as_object_mut()) {
            // `tool_use_result` is the structured result, and it is where the completed row's
            // diff is read from (`map::tools::derive_tool_diff`).
            object.insert("tool_use_result".to_owned(), structured);
        }
        self.peer.emit(&frame)
    }

    /// Completes an `Edit` with the transcript's own unified diff.
    fn finish_edit(&mut self, tool_use: &str, path: &str, diff: &str) -> anyhow::Result<()> {
        let (added, removed) = diff_counts(diff);
        let (before, after) = diff_sides(diff);
        let structured = json!({
            "filePath": path,
            "unified_diff": diff,
            "oldString": before,
            "newString": after,
            "additions": added,
            "deletions": removed,
        });
        self.tool_result(
            tool_use,
            &format!("Updated {path}"),
            Some(structured),
            false,
        )
    }

    /// `system/permission_denied`: the frame that says "this was refused without asking you".
    ///
    /// It must render as a denied tool row. Dropping it is what makes a refusal look like a hang,
    /// which is the whole reason the research document calls it out.
    fn permission_denied(&mut self, tool_use: &str, tool_name: &str) -> anyhow::Result<()> {
        let uuid = self.ids.uuid(0x5157_0004);
        self.peer.emit(&json!({
            "type": "system",
            "subtype": "permission_denied",
            "uuid": uuid,
            "session_id": self.transcript.session_id,
            "tool_name": tool_name,
            "tool_use_id": tool_use,
            "decision_reason_type": "other",
            "decision_reason": "The user declined this request.",
            "message": "The user declined this request.",
        }))
    }

    /// An `assistant` frame carrying only the failure latch.
    ///
    /// The latch speaks only when the `result` names no cause of its own; the scripted `result`
    /// always does, so this frame exists to exercise the latch path rather than to be read.
    fn error_latch(&mut self, message: &str) -> anyhow::Result<()> {
        let uuid = self.ids.uuid(0x5157_0005);
        let id = self.ids.message();
        self.peer.emit(&json!({
            "type": "assistant",
            "uuid": uuid,
            "session_id": self.transcript.session_id,
            "parent_tool_use_id": Value::Null,
            "error": "server_error",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": self.catalogue.default_model(),
                "content": [],
                "stop_reason": Value::Null,
                "error_message": message,
            },
        }))
    }

    /// The `result` frame: exactly one per turn, and the completion authority.
    fn settle(
        &mut self,
        settlement: Settlement,
        failure: Option<&str>,
        final_text: &str,
    ) -> anyhow::Result<()> {
        let (subtype, is_error, terminal_reason) = match settlement {
            Settlement::Completed => ("success", false, "completed"),
            Settlement::Failed => ("error_during_execution", true, "api_error"),
            Settlement::Interrupted => ("success", false, "aborted_streaming"),
        };
        let uuid = self.ids.uuid(0x5157_0006);
        let duration = self.ids.stamp_ms() % 1_000;
        let model = self.catalogue.default_model().to_owned();
        self.peer.emit(&json!({
            "type": "result",
            "subtype": subtype,
            "uuid": uuid,
            "session_id": self.transcript.session_id,
            "is_error": is_error,
            "terminal_reason": terminal_reason,
            "duration_ms": duration,
            "duration_api_ms": duration,
            "num_turns": 1,
            "result": final_text,
            "stop_reason": Value::Null,
            "total_cost_usd": 0.0,
            "usage": {
                "input_tokens": 12,
                "output_tokens": final_text.split_whitespace().count(),
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            // The context meter's denominator. Claude publishes it only at turn end.
            "modelUsage": {
                model: {
                    "inputTokens": 12,
                    "outputTokens": final_text.split_whitespace().count(),
                    "cacheReadInputTokens": 0,
                    "cacheCreationInputTokens": 0,
                    "webSearchRequests": 0,
                    "costUSD": 0.0,
                    "contextWindow": self.transcript.context_window,
                    "maxOutputTokens": 64_000,
                },
            },
            "permission_denials": [],
            "errors": failure.map(|message| vec![message]).unwrap_or_default(),
        }))
    }

    /// Asks for permission and blocks until the client answers or withdraws.
    fn ask(
        &mut self,
        gate: &str,
        tool_name: &str,
        display: &str,
        input: &Value,
        tool_use: &str,
    ) -> anyhow::Result<Decision> {
        self.peer.emit(&json!({
            "type": "control_request",
            "request_id": gate,
            "request": {
                "subtype": "can_use_tool",
                "tool_name": tool_name,
                "input": input,
                "tool_use_id": tool_use,
                "title": format!("Claude wants to use {tool_name}"),
                "display_name": display,
                "decision_reason": "This request needs approval",
                "decision_reason_type": "mode",
                // Rescoped to `session` by Fleet before it is echoed back: the live capture came
                // back `localSettings`, and echoing that writes a permanent rule into the user's
                // own settings file.
                "permission_suggestions": [{
                    "type": "addRules",
                    "rules": [{"toolName": tool_name}],
                    "behavior": "allow",
                    "destination": "localSettings",
                }],
                "suppress_always_allow_rule": false,
                "default_to_no": false,
                "requires_user_interaction": true,
            },
        }))?;
        let deadline = Instant::now()
            .checked_add(self.gate_budget)
            .ok_or_else(|| anyhow::anyhow!("the gate deadline overflows Instant"))?;
        loop {
            let Some(frame) = self.peer.read_frame_before(deadline, self.gate_budget)? else {
                anyhow::bail!("the client closed the connection with gate {gate:?} open");
            };
            match frame.get("type").and_then(Value::as_str) {
                Some("control_response") => {
                    let response = frame.pointer("/response").unwrap_or(&Value::Null);
                    if response.get("request_id").and_then(Value::as_str) != Some(gate) {
                        continue;
                    }
                    return Ok(decision_of(response));
                }
                // A withdrawn request closes with **no** response sent.
                Some("control_cancel_request") => {
                    if frame.get("request_id").and_then(Value::as_str) == Some(gate) {
                        return Ok(Decision::Withdrawn);
                    }
                }
                Some("control_request") => {
                    self.answer_control(&frame)?;
                    if self.interrupted {
                        return Ok(Decision::Withdrawn);
                    }
                }
                // A second prompt during a running turn is coalesced by the real CLI.
                _ => {}
            }
        }
    }

    /// Answers one inbound control request.
    fn answer_control(&mut self, frame: &Value) -> anyhow::Result<()> {
        let request_id = frame
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let subtype = frame
            .pointer("/request/subtype")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let response = match subtype {
            "initialize" => json!({
                "commands": [],
                "agents": [],
                "output_style": "default",
                "available_output_styles": ["default"],
                "models": self.model_catalogue(),
                "account": {"email": "scripted@example.invalid"},
            }),
            "interrupt" => {
                self.interrupted = true;
                json!({"still_queued": [], "cancelled": []})
            }
            "list_models" => json!({"models": self.model_catalogue()}),
            "get_binary_version" => json!({"version": "2.1.266"}),
            "set_permission_mode"
            | "set_model"
            | "set_max_thinking_tokens"
            | "rename_session"
            | "set_color"
            | "apply_flag_settings"
            | "update_settings"
            | "mcp_reconnect"
            | "mcp_toggle"
            | "stop_task" => json!({}),
            other => {
                return self.peer.emit(&json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "error",
                        "request_id": request_id,
                        "error": format!(
                            "the scripted agent does not implement the `{other}` control request"
                        ),
                    },
                }));
            }
        };
        self.peer.emit(&json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": response,
            },
        }))
    }

    /// The model list a `models` step configured.
    fn model_catalogue(&self) -> Value {
        Value::Array(
            self.catalogue
                .models
                .iter()
                .map(|model| {
                    json!({
                        "model": model,
                        "displayName": model,
                        "description": "a scripted model",
                    })
                })
                .collect(),
        )
    }
}

/// The decision a `control_response` carries.
///
/// Anything that is not an explicit allow is a denial, `error` subtypes included: refusing is the
/// only safe reading of an answer the player could not understand, and a scripted agent that
/// treated an unparsed answer as consent would be a scripted agent that runs commands nobody
/// approved.
fn decision_of(response: &Value) -> Decision {
    let body = response.get("response").unwrap_or(&Value::Null);
    if body.get("behavior").and_then(Value::as_str) == Some("allow") {
        return Decision::Allow;
    }
    Decision::Deny {
        interrupt: body
            .get("interrupt")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// The `Edit` input a unified diff describes.
///
/// Claude's permission card and its completed tool row are both built from `old_string` and
/// `new_string` (`agents::claude::map::tools::synthetic_diff`), never from a patch, so the diff is
/// split back into its two sides here.
fn edit_input(path: &str, diff: &str) -> Value {
    let (before, after) = diff_sides(diff);
    json!({ "file_path": path, "old_string": before, "new_string": after })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::{
        io::{BufReader, Write},
        time::Duration,
    };

    use serde_json::{Value, json};

    #[cfg(unix)]
    use super::super::peer::{Pace, Peer};
    use super::super::{
        Provider,
        tests::{claude_prompt, claude_streamed_text, drive, of_type, starter},
    };

    #[test]
    fn an_interrupt_during_an_open_claude_gate_emits_an_aborted_result() {
        let interrupt = json!({
            "type": "control_request",
            "request_id": "int-open-gate",
            "request": {"subtype": "interrupt", "cancel_queued": true},
        });
        let (frames, code) = drive(
            Provider::Claude,
            &starter("edit-approval.json"),
            &[claude_prompt("fix the readme"), interrupt],
        );

        assert_eq!(code, 0);
        let results = of_type(&frames, "result");
        assert_eq!(
            results.len(),
            1,
            "the interrupted turn settles exactly once"
        );
        assert_eq!(
            results[0].get("terminal_reason").and_then(Value::as_str),
            Some("aborted_streaming")
        );
    }

    #[test]
    fn a_stale_interrupt_between_claude_prompts_marks_but_does_not_truncate_the_next_turn() {
        let interrupt = json!({
            "type": "control_request",
            "request_id": "int-between-turns",
            "request": {"subtype": "interrupt", "cancel_queued": true},
        });
        let (frames, code) = drive(
            Provider::Claude,
            &starter("two-turns.json"),
            &[
                claude_prompt("what is this crate?"),
                interrupt,
                claude_prompt("and its binary?"),
            ],
        );

        assert_eq!(code, 0);
        let results = of_type(&frames, "result");
        assert_eq!(results.len(), 2, "each prompt settles exactly once");
        assert_eq!(
            results[1].get("terminal_reason").and_then(Value::as_str),
            Some("aborted_streaming"),
            "the between-turn interrupt is consumed by the next turn's settlement"
        );
        assert!(
            claude_streamed_text(&frames).contains("The binary name is demo"),
            "the second turn streams through its final text"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unanswered_claude_gate_fails_within_budget_after_emitting_an_aborted_result() {
        let (server, mut client) = std::os::unix::net::UnixStream::pair()
            .unwrap_or_else(|error| panic!("create the agent socket pair: {error}"));
        writeln!(client, "{}", claude_prompt("fix the readme"))
            .unwrap_or_else(|error| panic!("write the prompt to the agent: {error}"));
        client
            .flush()
            .unwrap_or_else(|error| panic!("flush the prompt to the agent: {error}"));

        let transcript = starter("edit-approval.json");
        let mut output = Vec::new();
        let error = {
            let mut peer = Peer::new(BufReader::new(server), &mut output, Pace::Instant);
            super::play_with_gate_budget(&transcript, &mut peer, Duration::ZERO)
                .err()
                .unwrap_or_else(|| panic!("an unanswered gate must fail"))
        };

        assert!(
            error
                .to_string()
                .contains("no frame from the client within 0ns"),
            "the failure names the gate deadline: {error}"
        );

        let rendered = String::from_utf8(output)
            .unwrap_or_else(|error| panic!("the player wrote UTF-8: {error}"));
        let frames: Vec<Value> = rendered
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("the player wrote JSON: {error}: {line}"))
            })
            .collect();
        let results = of_type(&frames, "result");
        assert_eq!(results.len(), 1, "the timed-out turn settles exactly once");
        assert_eq!(
            results[0].get("terminal_reason").and_then(Value::as_str),
            Some("aborted_streaming")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unrelated_claude_frames_cannot_renew_an_open_gate_deadline() {
        let (server, mut client) = std::os::unix::net::UnixStream::pair()
            .unwrap_or_else(|error| panic!("create the agent socket pair: {error}"));
        writeln!(client, "{}", claude_prompt("fix the readme"))
            .unwrap_or_else(|error| panic!("write the prompt to the agent: {error}"));
        client
            .flush()
            .unwrap_or_else(|error| panic!("flush the prompt to the agent: {error}"));
        let traffic = std::thread::spawn(move || {
            for index in 0..12 {
                let frame = json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": format!("wrong-gate-{index}"),
                        "response": {"behavior": "allow"},
                    },
                });
                if writeln!(client, "{frame}").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });

        let transcript = starter("edit-approval.json");
        let mut output = Vec::new();
        let error = {
            let mut peer = Peer::new(BufReader::new(server), &mut output, Pace::Instant);
            super::play_with_gate_budget(&transcript, &mut peer, Duration::from_millis(20))
                .expect_err("unrelated frames cannot keep the gate open")
        };
        traffic
            .join()
            .unwrap_or_else(|_| panic!("the unrelated-frame writer panicked"));

        assert!(
            error
                .to_string()
                .contains("no frame from the client within 20ms"),
            "{error}"
        );
    }
}
