//! The `codex app-server` player.
//!
//! Transport and frame shapes come from `docs/research/harness-codex-app-server.md`, verified
//! there against `codex --version` → `codex-cli 0.147.0`. Four properties of that surface shape
//! this file, and each is also a rule `agents::codex::envelope` enforces on the way in:
//!
//! - **It is JSON-RPC-*shaped* and it is not JSON-RPC 2.0.** There is no `jsonrpc` field in either
//!   direction, and `params` is omitted entirely when absent — not `null`, not `{}`.
//! - **It is bidirectional.** Approvals are server→client *requests* the client must answer, keyed
//!   by the envelope id, and the client's answer comes back as `{"id":…,"result":{…}}`.
//! - **The completed item is the result.** There is no separate tool-result frame, and
//!   `turn/completed.turn.items` is a summary governed by `itemsView`, so this player accumulates
//!   the transcript through `item/started`/`item/completed` and lets `turn/completed` carry only
//!   status, error and duration — the trap §9.5 of the research document names.
//! - **A shell row renders from `commandActions`, never from `command`.** Codex parses its own
//!   command for the client, so the player computes that parse too; without it every scripted tool
//!   row would show a `bash -lc` wrapper where Claude shows a clean `Read`.

use serde_json::{Map, Value, json};

use super::{
    Transcript, TranscriptStep,
    ids::Ids,
    peer::Peer,
    transcript::{Catalogue, Playback, Settlement, text_chunks},
};

/// The JSON-RPC error code for a method the scripted server does not implement.
const METHOD_NOT_FOUND: i64 = -32601;
/// The code for a call that cannot be served while a gate is open.
const SERVER_BUSY: i64 = -32001;

/// Plays one transcript as `codex app-server`.
pub(crate) fn play<R: std::io::BufRead, W: std::io::Write>(
    transcript: &Transcript,
    peer: &mut Peer<R, W>,
) -> anyhow::Result<i32> {
    let mut session = Session {
        transcript,
        catalogue: Catalogue::of(transcript),
        playback: Playback::new(transcript),
        ids: Ids::new(),
        interrupted: false,
        turn_diff: String::new(),
        peer,
    };
    loop {
        let Some(frame) = session.peer.read_frame()? else {
            return Ok(0);
        };
        let Some(method) = frame
            .get("method")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
        else {
            // A response to a request no gate is waiting on, or a frame this build ignores.
            continue;
        };
        // Classification is structural: `method` plus a string-or-number `id` is a request, and
        // `method` with the key `id` absent is a notification. `{"id":null}` is neither.
        let Some(id) = frame
            .get("id")
            .filter(|id| id.is_string() || id.is_number())
        else {
            continue;
        };
        let id = id.clone();
        let params = frame.get("params").cloned().unwrap_or(Value::Null);
        if let Some(code) = session.handle_request(&id, &method, &params)? {
            return Ok(code);
        }
    }
}

/// What the client decided about one gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// `accept` or `acceptForSession`, and the amendment-carrying object forms.
    Accept,
    /// `decline` — the turn continues.
    Decline,
    /// `cancel` — the turn is interrupted as well.
    Cancel,
}

/// One scripted Codex session.
struct Session<'a, R, W> {
    transcript: &'a Transcript,
    catalogue: Catalogue,
    playback: Playback<'a>,
    ids: Ids,
    /// Set by `turn/interrupt`; consumed by the running turn.
    interrupted: bool,
    /// The turn-level unified diff Codex publishes for free.
    turn_diff: String,
    peer: &'a mut Peer<R, W>,
}

impl<R: std::io::BufRead, W: std::io::Write> Session<'_, R, W> {
    /// Answers one client request, playing a whole turn when the request is `turn/start`.
    fn handle_request(
        &mut self,
        id: &Value,
        method: &str,
        params: &Value,
    ) -> anyhow::Result<Option<i32>> {
        if method == "turn/start" {
            return self.start_turn(id, params);
        }
        self.answer_simple(id, method, params)?;
        Ok(None)
    }

    /// Answers every request that does not itself run a turn.
    ///
    /// Split out because the gate wait re-enters here: a `turn/interrupt` arriving while an
    /// approval is open must be answered, and a second `turn/start` must not recurse into a
    /// nested turn.
    fn answer_simple(&mut self, id: &Value, method: &str, _params: &Value) -> anyhow::Result<()> {
        let result = match method {
            // `codexHome` comes back resolved, and the `clientInfo` Fleet sent is echoed into the
            // user agent — which is also the only place the CLI version appears.
            "initialize" => json!({
                "userAgent": "fleet-harness/0.147.0 (scripted; x86_64) codex-cli",
                "codexHome": std::env::var("CODEX_HOME").unwrap_or_else(|_| "/tmp/codex".to_owned()),
                "platformFamily": "unix",
                "platformOs": "linux",
            }),
            "thread/start" | "thread/resume" => {
                let thread = self.thread_object();
                self.notify("thread/started", json!({"thread": thread.clone()}))?;
                json!({
                    "thread": thread,
                    "cwd": self.cwd(),
                    "model": self.catalogue.default_model(),
                    "modelProvider": "openai",
                    "reasoningEffort": self.catalogue.default_effort(),
                    "approvalPolicy": "on-request",
                    "approvalsReviewer": "user",
                    "sandboxPolicy": {"mode": "workspace-write"},
                })
            }
            // Nothing changes the running turn; a steer into a settled one is refused with the
            // typed error Fleet falls back to a queued `turn/start` on.
            "turn/steer" => {
                return self.error(
                    id,
                    METHOD_NOT_FOUND,
                    "activeTurnNotSteerable: the scripted turn already finished",
                );
            }
            "turn/interrupt" => {
                self.interrupted = true;
                json!({})
            }
            "model/list" => json!({"models": self.model_catalogue()}),
            "skills/list" => json!({"skills": []}),
            "permissionProfile/list" => json!({"permissionProfiles": []}),
            "collaborationMode/list" => json!({"collaborationModes": []}),
            "thread/settings/update"
            | "thread/unsubscribe"
            | "thread/compact/start"
            | "thread/fork"
            | "thread/rollback" => json!({}),
            "thread/items/list" => json!({"data": [], "nextCursor": Value::Null}),
            "thread/turns/list" => {
                json!({"data": [], "nextCursor": Value::Null, "backwardsCursor": Value::Null})
            }
            other => {
                return self.error(
                    id,
                    METHOD_NOT_FOUND,
                    &format!("the scripted agent does not implement `{other}`"),
                );
            }
        };
        self.peer.emit(&json!({"id": id, "result": result}))
    }

    /// Answers `turn/start`, then plays the turn its result released.
    fn start_turn(&mut self, id: &Value, params: &Value) -> anyhow::Result<Option<i32>> {
        let turn = self.ids.turn();
        // The result comes first: `thread/status/changed` and `turn/started` follow it in the
        // live capture, and `turn/started` is authoritative for "a turn is running".
        self.peer.emit(&json!({
            "id": id,
            "result": {"turn": {"id": turn, "items": [], "status": "inProgress"}},
        }))?;
        let prompt = prompt_text(params);
        let client_id = params
            .get("clientUserMessageId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        self.notify(
            "thread/status/changed",
            json!({
                "threadId": self.transcript.thread_id,
                "status": {"type": "active", "activeFlags": []},
            }),
        )?;
        self.notify(
            "turn/started",
            json!({
                "threadId": self.transcript.thread_id,
                "turn": {"id": turn, "items": [], "status": "inProgress"},
            }),
        )?;
        self.echo_user_message(&turn, &prompt, client_id.as_deref())?;
        self.run_turn(&turn)
    }

    /// Plays one turn's steps and settles it.
    fn run_turn(&mut self, turn: &str) -> anyhow::Result<Option<i32>> {
        let script = self.playback.next_turn();
        let mut settlement = script.settlement;
        let mut failure: Option<String> = None;
        let mut pending_change: Option<String> = None;
        self.turn_diff.clear();
        for (index, step) in script.steps.iter().enumerate() {
            match step {
                TranscriptStep::Text { text, pace_ms } => self.stream_text(turn, text, *pace_ms)?,
                TranscriptStep::ToolCall {
                    name,
                    arguments,
                    output,
                    ..
                } => {
                    let item = self.ids.item();
                    let command = command_line(name, arguments);
                    self.start_command(turn, &item, &command)?;
                    self.complete_command(turn, &item, &command, output, "completed", Some(0))?;
                }
                TranscriptStep::FileChange { path, diff } => {
                    let item = self.ids.item();
                    self.start_file_change(turn, &item, path, diff)?;
                    if matches!(
                        script.steps.get(index + 1),
                        Some(TranscriptStep::Approval { .. })
                    ) {
                        pending_change = Some(item);
                    } else {
                        self.complete_file_change(turn, &item, path, diff, "completed")?;
                    }
                }
                TranscriptStep::Permission { id, command } => {
                    let item = self.ids.item();
                    self.start_command(turn, &item, command)?;
                    let decision = self.ask(
                        id,
                        "item/commandExecution/requestApproval",
                        turn,
                        &item,
                        json!({
                            "command": command,
                            "commandActions": command_actions(command),
                            "cwd": self.cwd(),
                            "environmentId": Value::Null,
                            "reason": "This command needs approval",
                            // Authoritative and ordered: Fleet renders exactly these options, in
                            // this order, and never invents an "always allow".
                            "availableDecisions": ["accept", "acceptForSession", "decline", "cancel"],
                        }),
                    )?;
                    let (status, code) = match decision {
                        Decision::Accept => ("completed", Some(0)),
                        // `declined` is a first-class terminal status, not an error.
                        Decision::Decline | Decision::Cancel => ("declined", None),
                    };
                    self.complete_command(turn, &item, command, "", status, code)?;
                    if decision == Decision::Cancel {
                        settlement = Settlement::Interrupted;
                        break;
                    }
                }
                TranscriptStep::Approval { id, summary } => {
                    let (Some(item), Some(TranscriptStep::FileChange { path, diff })) = (
                        pending_change.take(),
                        index
                            .checked_sub(1)
                            .and_then(|previous| script.steps.get(previous)),
                    ) else {
                        anyhow::bail!(
                            "an approval reached the player with no file change to gate; \
                             the transcript should have been rejected at load"
                        );
                    };
                    // The approval carries **no** diff and no paths: the changes live on the
                    // `fileChange` item this names by `itemId`, and the card joins on that.
                    let decision = self.ask(
                        id,
                        "item/fileChange/requestApproval",
                        turn,
                        &item,
                        json!({"reason": summary, "grantRoot": Value::Null}),
                    )?;
                    let status = match decision {
                        Decision::Accept => "completed",
                        Decision::Decline | Decision::Cancel => "declined",
                    };
                    self.complete_file_change(turn, &item, path, diff, status)?;
                    if decision == Decision::Cancel {
                        settlement = Settlement::Interrupted;
                        break;
                    }
                }
                TranscriptStep::Models { .. } => {}
                TranscriptStep::Error { message } => {
                    // An `error` notification is **not** terminal: `turn/completed` still arrives
                    // and is still the authority, and `willRetry` says whether it was informational.
                    self.notify(
                        "error",
                        json!({
                            "threadId": self.transcript.thread_id,
                            "turnId": turn,
                            "willRetry": false,
                            "error": {
                                "message": message,
                                "codexErrorInfo": "serverOverloaded",
                                "additionalDetails": Value::Null,
                            },
                        }),
                    )?;
                    failure = Some(message.clone());
                    settlement = Settlement::Failed;
                }
                TranscriptStep::EndTurn { .. } => {}
                TranscriptStep::Exit { code } => return Ok(Some(*code)),
            }
            if self.interrupted {
                self.interrupted = false;
                settlement = Settlement::Interrupted;
                break;
            }
        }
        self.settle(turn, settlement, failure.as_deref())?;
        Ok(None)
    }

    /// Echoes the user's own message back as a `userMessage` item.
    ///
    /// Fleet reconciles its optimistic bubble against this item by `clientId` rather than
    /// appending a duplicate, so the echo has to carry the id the client sent.
    fn echo_user_message(
        &mut self,
        turn: &str,
        prompt: &str,
        client_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let item = self.ids.item();
        let body = json!({
            "type": "userMessage",
            "id": item,
            "clientId": client_id,
            "content": [{"type": "text", "text": prompt}],
        });
        self.item_started(turn, body.clone())?;
        self.item_completed(turn, body)
    }

    /// Streams one assistant message as an `agentMessage` item and its deltas.
    fn stream_text(&mut self, turn: &str, text: &str, pace_ms: u64) -> anyhow::Result<()> {
        let item = self.ids.item();
        self.item_started(
            turn,
            json!({"type": "agentMessage", "id": item, "text": ""}),
        )?;
        for chunk in text_chunks(text) {
            self.peer.pause(pace_ms);
            self.notify(
                "item/agentMessage/delta",
                json!({
                    "threadId": self.transcript.thread_id,
                    "turnId": turn,
                    "itemId": item,
                    "delta": chunk,
                }),
            )?;
        }
        self.item_completed(
            turn,
            json!({"type": "agentMessage", "id": item, "text": text}),
        )?;
        self.publish_token_usage(turn)
    }

    /// `item/started` for a `commandExecution`, already carrying the `commandActions` parse.
    fn start_command(&mut self, turn: &str, item: &str, command: &str) -> anyhow::Result<()> {
        let body = self.command_item(item, command, "", "inProgress", None);
        self.item_started(turn, body)
    }

    /// `item/completed` for a `commandExecution`: the completed item **is** the result.
    fn complete_command(
        &mut self,
        turn: &str,
        item: &str,
        command: &str,
        output: &str,
        status: &str,
        exit_code: Option<i64>,
    ) -> anyhow::Result<()> {
        let body = self.command_item(item, command, output, status, exit_code);
        self.item_completed(turn, body)
    }

    /// The `commandExecution` item body.
    fn command_item(
        &self,
        item: &str,
        command: &str,
        output: &str,
        status: &str,
        exit_code: Option<i64>,
    ) -> Value {
        json!({
            "type": "commandExecution",
            "id": item,
            "command": command,
            "commandActions": command_actions(command),
            "cwd": self.cwd(),
            "status": status,
            "aggregatedOutput": output,
            "exitCode": exit_code,
            "durationMs": 4,
        })
    }

    /// `item/started` for a `fileChange`, plus the turn-level diff Codex publishes for free.
    fn start_file_change(
        &mut self,
        turn: &str,
        item: &str,
        path: &str,
        diff: &str,
    ) -> anyhow::Result<()> {
        self.item_started(turn, file_change_item(item, path, diff, "inProgress"))?;
        self.notify(
            "item/fileChange/patchUpdated",
            json!({
                "threadId": self.transcript.thread_id,
                "turnId": turn,
                "itemId": item,
                "changes": [change(path, diff)],
            }),
        )?;
        self.turn_diff.push_str(diff);
        if !self.turn_diff.ends_with('\n') {
            self.turn_diff.push('\n');
        }
        let diff = self.turn_diff.clone();
        self.notify(
            "turn/diff/updated",
            json!({
                "threadId": self.transcript.thread_id,
                "turnId": turn,
                "diff": diff,
            }),
        )
    }

    /// `item/completed` for a `fileChange`, applied or declined.
    fn complete_file_change(
        &mut self,
        turn: &str,
        item: &str,
        path: &str,
        diff: &str,
        status: &str,
    ) -> anyhow::Result<()> {
        self.item_completed(turn, file_change_item(item, path, diff, status))
    }

    /// `thread/tokenUsage/updated`: pushed per model step, not once per turn.
    fn publish_token_usage(&mut self, turn: &str) -> anyhow::Result<()> {
        let breakdown = json!({
            "totalTokens": 141,
            "inputTokens": 120,
            "cachedInputTokens": 96,
            "cacheWriteInputTokens": 0,
            "outputTokens": 21,
            "reasoningOutputTokens": 0,
        });
        self.notify(
            "thread/tokenUsage/updated",
            json!({
                "threadId": self.transcript.thread_id,
                "turnId": turn,
                "tokenUsage": {
                    "total": breakdown,
                    "last": breakdown,
                    "modelContextWindow": self.transcript.context_window,
                },
            }),
        )
    }

    /// `turn/completed`: terminal and authoritative.
    fn settle(
        &mut self,
        turn: &str,
        settlement: Settlement,
        failure: Option<&str>,
    ) -> anyhow::Result<()> {
        self.notify(
            "thread/status/changed",
            json!({"threadId": self.transcript.thread_id, "status": {"type": "idle"}}),
        )?;
        let status = match settlement {
            Settlement::Completed => "completed",
            Settlement::Failed => "failed",
            Settlement::Interrupted => "interrupted",
        };
        let mut body = json!({
            "id": turn,
            // A *summary*, deliberately: a client that rebuilds the transcript from here loses
            // every tool call, and this player proves Fleet does not.
            "items": [],
            "itemsView": "summary",
            "status": status,
            "durationMs": 13,
            "startedAt": self.ids.stamp_secs(),
            "completedAt": self.ids.stamp_secs(),
        });
        if let (Some(message), Some(object)) = (failure, body.as_object_mut()) {
            object.insert(
                "error".to_owned(),
                json!({
                    "message": message,
                    "codexErrorInfo": "serverOverloaded",
                    "additionalDetails": Value::Null,
                }),
            );
        }
        self.notify(
            "turn/completed",
            json!({"threadId": self.transcript.thread_id, "turn": body}),
        )
    }

    /// Issues one server→client approval request and blocks until the client answers.
    fn ask(
        &mut self,
        gate: &str,
        method: &str,
        turn: &str,
        item: &str,
        extra: Value,
    ) -> anyhow::Result<Decision> {
        let mut params = Map::new();
        params.insert("threadId".to_owned(), json!(self.transcript.thread_id));
        params.insert("turnId".to_owned(), json!(turn));
        params.insert("itemId".to_owned(), json!(item));
        params.insert("startedAtMs".to_owned(), json!(self.ids.stamp_ms()));
        if let Value::Object(extra) = extra {
            params.extend(extra);
        }
        self.peer.emit(&json!({
            "id": gate,
            "method": method,
            "params": Value::Object(params),
        }))?;
        loop {
            let Some(frame) = self.peer.read_frame()? else {
                anyhow::bail!("the client closed the connection with gate {gate:?} open");
            };
            let id = frame.get("id").cloned().unwrap_or(Value::Null);
            match frame.get("method").and_then(Value::as_str) {
                Some(method) if id.is_string() || id.is_number() => {
                    if method == "turn/start" {
                        self.error(&id, SERVER_BUSY, "a gate is open on the running turn")?;
                    } else {
                        self.answer_simple(&id, method, &Value::Null)?;
                    }
                }
                // A notification from the client; nothing to correlate.
                Some(_) => {}
                None => {
                    if id.as_str() == Some(gate) {
                        return Ok(decision_of(&frame));
                    }
                }
            }
        }
    }

    /// Writes one server notification, stamped with the server's own emission clock.
    ///
    /// `emittedAtMs` is a **sibling** of `params`, not a field inside it: it is the only
    /// server-side clock Fleet gets, and what makes ordering measurable across a remote link.
    fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let emitted = self.ids.stamp_ms();
        self.peer.emit(&json!({
            "method": method,
            "params": params,
            "emittedAtMs": emitted,
        }))
    }

    /// `item/started` for one item body.
    fn item_started(&mut self, turn: &str, item: Value) -> anyhow::Result<()> {
        let started = self.ids.stamp_ms();
        self.notify(
            "item/started",
            json!({
                "threadId": self.transcript.thread_id,
                "turnId": turn,
                "item": item,
                "startedAtMs": started,
            }),
        )
    }

    /// `item/completed` for one item body.
    fn item_completed(&mut self, turn: &str, item: Value) -> anyhow::Result<()> {
        let completed = self.ids.stamp_ms();
        self.notify(
            "item/completed",
            json!({
                "threadId": self.transcript.thread_id,
                "turnId": turn,
                "item": item,
                "completedAtMs": completed,
            }),
        )
    }

    /// A JSON-RPC-shaped error answer.
    fn error(&mut self, id: &Value, code: i64, message: &str) -> anyhow::Result<()> {
        self.peer
            .emit(&json!({"id": id, "error": {"code": code, "message": message}}))
    }

    /// The `Thread` object `thread/start` and `thread/started` both carry.
    fn thread_object(&mut self) -> Value {
        let created = self.ids.stamp_secs();
        json!({
            "id": self.transcript.thread_id,
            "sessionId": self.transcript.thread_id,
            "cliVersion": "0.147.0",
            "createdAt": created,
            "updatedAt": created,
            "cwd": self.cwd(),
            "ephemeral": false,
            "modelProvider": "openai",
            "preview": "",
            "source": "appServer",
            "status": {"type": "idle"},
            "turns": [],
        })
    }

    /// The working directory the scripted thread reports.
    fn cwd(&self) -> String {
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    /// The `model/list` catalogue, with the per-model effort ladder Fleet must not hardcode.
    fn model_catalogue(&self) -> Value {
        let efforts: Vec<Value> = self
            .catalogue
            .efforts
            .iter()
            .map(|effort| json!({"reasoningEffort": effort, "description": effort}))
            .collect();
        Value::Array(
            self.catalogue
                .models
                .iter()
                .enumerate()
                .map(|(index, model)| {
                    json!({
                        "id": model,
                        "model": model,
                        "displayName": model,
                        "description": "a scripted model",
                        "hidden": false,
                        "supportedReasoningEfforts": efforts,
                        "defaultReasoningEffort": self.catalogue.default_effort(),
                        "isDefault": index == 0,
                    })
                })
                .collect(),
        )
    }
}

/// One `FileUpdateChange`.
fn change(path: &str, diff: &str) -> Value {
    json!({"path": path, "diff": diff, "kind": {"type": "update"}})
}

/// The `fileChange` item body.
fn file_change_item(item: &str, path: &str, diff: &str, status: &str) -> Value {
    json!({
        "type": "fileChange",
        "id": item,
        "changes": [change(path, diff)],
        "status": status,
    })
}

/// The decision a client response carries.
fn decision_of(frame: &Value) -> Decision {
    let decision = frame.pointer("/result/decision").unwrap_or(&Value::Null);
    match decision.as_str() {
        Some("accept" | "acceptForSession") => Decision::Accept,
        Some("cancel") => Decision::Cancel,
        Some(_) => Decision::Decline,
        // The object forms — an execpolicy or network-policy amendment — are accepts carrying a
        // rule; an answer with no decision at all is a decline, which is the safe reading.
        None if decision.is_object() => Decision::Accept,
        None => Decision::Decline,
    }
}

/// The prompt text a `turn/start` carried.
fn prompt_text(params: &Value) -> String {
    params
        .get("input")
        .and_then(Value::as_array)
        .map(|input| {
            input
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// The shell command line a transcript tool call becomes.
///
/// Codex funnels almost everything through the shell, so a structured tool call is rendered as
/// the command a real Codex would have run, with the arguments it carried.
fn command_line(name: &str, arguments: &Value) -> String {
    if let Some(command) = arguments.get("command").and_then(Value::as_str) {
        return command.to_owned();
    }
    if let Some(path) = arguments
        .get("path")
        .or_else(|| arguments.get("file_path"))
        .and_then(Value::as_str)
    {
        return format!("{name} {path}");
    }
    match arguments {
        Value::Null => name.to_owned(),
        other => format!("{name} {other}"),
    }
}

/// Codex's own parse of a shell command — the kind column, computed by the harness.
///
/// A piped command decomposes into several actions; this player emits the one action a scripted
/// command describes, and `unknown` when it describes nothing in particular, which is exactly the
/// case Fleet must degrade to the raw command line for.
fn command_actions(command: &str) -> Value {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    let argument = words.next_back().unwrap_or_default();
    let action = match program {
        "cat" | "head" | "tail" | "sed" | "read_file" | "Read" if !argument.is_empty() => json!({
            "type": "read",
            "command": command,
            "name": argument.rsplit('/').next().unwrap_or(argument),
            "path": argument,
        }),
        "ls" | "find" => json!({"type": "listFiles", "command": command, "path": argument}),
        "rg" | "grep" => json!({"type": "search", "command": command, "query": argument}),
        _ => json!({"type": "unknown", "command": command}),
    };
    json!([action])
}
