//! One transcript item, projected into the row it draws (`spec-B` §B1.2).
//!
//! Everything here is pure: the collapse thresholds, the 60 px kind column, the one-line
//! summary, the expanded body's block order, and the promotion of a plan's first heading out of
//! its body. The row renderer receives prepared values and composes them.

use fleet_core::agents::{
    Attachment, AttachmentSource, GateAnswer, GateKind, Item, ItemId, ItemKind, ItemStatus,
    OpenGate, PermissionChoice, ToolCall, ToolKind,
};
use fleet_ui_kit::{
    AssistantMetaRow, AssistantRow, DiffRow, ErrorRow, GateOutcome, GateRow, Icon, PlanRow,
    ReasoningRow, SubagentRow, ToolRow, ToolRowState, TranscriptRow, TranscriptRowId,
    TranscriptRowKind, UserRow, UserRowState, format_exit, parse_markdown_document,
};
use gpui::SharedString;

use super::{PendingSend, RowInputs, RowTarget};

/// A user bubble collapses past this many characters (`MAX_COLLAPSED_USER_MESSAGE_LENGTH`).
const USER_COLLAPSE_CHARS: usize = 600;

/// …or this many lines (`MAX_COLLAPSED_USER_MESSAGE_LINES`).
const USER_COLLAPSE_LINES: usize = 8;

/// A plan card fades out past this many characters.
const PLAN_COLLAPSE_CHARS: usize = 900;

/// …or this many lines.
const PLAN_COLLAPSE_LINES: usize = 20;

/// How many activity lines a subagent's rolling ring keeps.
const SUBAGENT_RING: usize = 6;

/// How wide one of those lines may be before it is cut.
const SUBAGENT_LINE_CHARS: usize = 180;

/// The row for one top-level item, plus the detail rows it owns while expanded.
///
/// `None` for an item that is not a row at all: a settled message with nothing in it is a
/// placeholder whose `[⏎] show` would expand to nothing, and Claude opens a thinking block for a
/// signature-only reasoning frame and settles it with no delta.
pub(crate) fn rows_for(
    inputs: &RowInputs<'_>,
    items: &std::collections::HashMap<ItemId, &Item>,
    item: &Item,
) -> Vec<(TranscriptRow, Option<RowTarget>)> {
    let id = TranscriptRowId::Item(SharedString::from(item.id.to_string()));
    let expanded = inputs.is_expanded(item.id);
    match &item.kind {
        ItemKind::UserMessage {
            text,
            attachments,
            steered,
        } => {
            let text = text.as_str();
            vec![(
                TranscriptRow::new(
                    id,
                    TranscriptRowKind::User(UserRow {
                        text: SharedString::new(text),
                        attachments: attachments.iter().map(attachment_name).collect(),
                        state: UserRowState::Sent,
                        // The harness's own answer, recorded on the item: a `↳` the optimistic
                        // row drew now survives the reload that replaces it (§7.2).
                        steered: *steered,
                        collapsible: collapses(text, USER_COLLAPSE_CHARS, USER_COLLAPSE_LINES),
                        expanded,
                    }),
                ),
                Some(RowTarget::Item(item.id)),
            )]
        }
        ItemKind::AssistantText { text } => {
            if text.trim().is_empty() && item.status == ItemStatus::InProgress {
                return Vec::new();
            }
            vec![(
                TranscriptRow::new(
                    id,
                    TranscriptRowKind::Assistant(AssistantRow {
                        markdown: parse_markdown_document(text),
                        streaming: item.status == ItemStatus::InProgress,
                        empty: text.trim().is_empty(),
                    }),
                ),
                None,
            )]
        }
        ItemKind::Reasoning { .. } => {
            let text = item_text(item);
            if text.trim().is_empty() && item.status != ItemStatus::InProgress {
                return Vec::new();
            }
            let streaming = item.status == ItemStatus::InProgress;
            // A streaming reasoning block *is* the live row: it shares `LiveActivity` with the
            // live work row and the working row so the handoff between them keeps one measured
            // height and never remounts.
            let id = if streaming {
                TranscriptRowId::LiveActivity
            } else {
                id
            };
            vec![(
                TranscriptRow::new(
                    id,
                    TranscriptRowKind::Reasoning(ReasoningRow {
                        text: SharedString::from(text),
                        duration_ms: (!streaming).then(|| duration_ms(item)),
                        expanded,
                    }),
                ),
                Some(RowTarget::Item(item.id)),
            )]
        }
        ItemKind::Plan { text } => {
            let (title, body) = split_plan(text);
            vec![(
                TranscriptRow::new(
                    id,
                    TranscriptRowKind::Plan(PlanRow {
                        title,
                        markdown: parse_markdown_document(&body),
                        collapsible: collapses(&body, PLAN_COLLAPSE_CHARS, PLAN_COLLAPSE_LINES),
                        expanded,
                    }),
                ),
                Some(RowTarget::Item(item.id)),
            )]
        }
        ItemKind::Error { message } => vec![(
            TranscriptRow::new(
                id,
                TranscriptRowKind::Error(ErrorRow {
                    message: SharedString::new(if message.trim().is_empty() {
                        "the harness reported an error"
                    } else {
                        message.as_str()
                    }),
                    // Retryability is the daemon's judgement and nothing on the wire carries it
                    // per item yet, so `[r] retry` is not drawn on an item-level error.
                    retryable: false,
                }),
            ),
            None,
        )],
        ItemKind::Subagent { .. } => vec![(
            TranscriptRow::new(
                id,
                TranscriptRowKind::Subagent(subagent_row(inputs, items, item, expanded)),
            ),
            Some(RowTarget::Item(item.id)),
        )],
        ItemKind::Tool(call) => {
            let mut rows = vec![(
                TranscriptRow::new(
                    id.clone(),
                    TranscriptRowKind::Work(tool_row(inputs, item, call)),
                )
                .attached(expanded && call.diff.is_some()),
                Some(RowTarget::Item(item.id)),
            )];
            // §5: a diff is its own row under an expanded edit row, so its height is measured
            // independently and an expanded diff never inflates the tool row's own measurement.
            if expanded && let Some(diff) = &call.diff {
                rows.push((
                    TranscriptRow::new(
                        id,
                        TranscriptRowKind::Diff(DiffRow {
                            item: SharedString::from(item.id.to_string()),
                            unified: SharedString::new(diff.unified.as_str()),
                        }),
                    ),
                    None,
                ));
            }
            rows
        }
    }
}

/// The hover-revealed footer of a terminal assistant message.
pub(crate) fn assistant_meta(item: &Item) -> (TranscriptRow, Option<RowTarget>) {
    (
        TranscriptRow::new(
            TranscriptRowId::Turn(SharedString::from(format!("meta-{}", item.id))),
            TranscriptRowKind::AssistantMeta(AssistantMetaRow {
                updated_at: item.ended.map_or_else(SharedString::default, |at| {
                    SharedString::from(at.format("%H:%M").to_string())
                }),
            }),
        ),
        None,
    )
}

/// One optimistic user bubble, at `Sending` until the projection catches up.
pub(crate) fn pending_row(pending: &PendingSend) -> TranscriptRow {
    TranscriptRow::new(
        TranscriptRowId::Item(SharedString::from(pending.id.to_string())),
        TranscriptRowKind::User(UserRow {
            text: SharedString::new(pending.text.as_str()),
            attachments: Vec::new(),
            state: if pending.failed {
                UserRowState::Failed
            } else {
                UserRowState::Sending
            },
            steered: pending.steered,
            collapsible: false,
            expanded: false,
        }),
    )
}

/// The settled record a resolved gate leaves at the position it was asked.
///
/// Fleet's own addition: it is what makes docking the live decision safe, because the drawer is
/// always on screen and the history of what was asked stays in the transcript.
pub(crate) fn gate_row(
    gate: &OpenGate,
    outcome: GateOutcome,
    answer: Option<&GateAnswer>,
    expanded: bool,
) -> (TranscriptRow, Option<RowTarget>) {
    let (label, detail, payload) = gate_record(gate, outcome, answer);
    (
        TranscriptRow::new(
            TranscriptRowId::Item(SharedString::from(gate.id.to_string())),
            TranscriptRowKind::Gate(GateRow {
                outcome,
                label,
                detail,
                payload,
                expanded,
            }),
        ),
        Some(RowTarget::Gate(gate.id)),
    )
}

/// The scope clause, the one-line subject and the expandable payload of a settled gate.
fn gate_record(
    gate: &OpenGate,
    outcome: GateOutcome,
    answer: Option<&GateAnswer>,
) -> (SharedString, SharedString, Option<SharedString>) {
    match (&gate.kind, answer) {
        (
            GateKind::Permission { tool, payload, .. },
            Some(GateAnswer::Permission { choice, .. }),
        ) => (
            SharedString::new_static(permission_clause(*choice)),
            SharedString::from(format!("{}: {payload}", kind_word(tool))),
            Some(SharedString::new(payload.as_str())),
        ),
        (GateKind::Permission { tool, payload, .. }, _) => (
            SharedString::new_static(outcome_clause(outcome)),
            SharedString::from(format!("{}: {payload}", kind_word(tool))),
            Some(SharedString::new(payload.as_str())),
        ),
        (GateKind::Question { questions }, Some(GateAnswer::Question { answers })) => {
            let detail = questions
                .iter()
                .zip(answers)
                .map(|(question, chosen)| {
                    format!("{} \u{2192} {}", question.prompt, chosen.join(", "))
                })
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
            (
                SharedString::new_static("answered"),
                SharedString::from(detail.clone()),
                Some(SharedString::from(detail)),
            )
        }
        (GateKind::Question { questions }, _) => (
            SharedString::new_static(outcome_clause(outcome)),
            SharedString::from(
                questions
                    .first()
                    .map(|question| question.prompt.clone())
                    .unwrap_or_default(),
            ),
            None,
        ),
        (GateKind::Plan { markdown, .. }, _) => (
            SharedString::new_static(outcome_clause(outcome)),
            split_plan(markdown).0,
            Some(SharedString::new(markdown.as_str())),
        ),
    }
}

/// The clause a permission answer earns, which always spells its effective scope.
const fn permission_clause(choice: PermissionChoice) -> &'static str {
    match choice {
        PermissionChoice::AllowOnce | PermissionChoice::Edit => "allowed once",
        PermissionChoice::AllowSession => "allowed for this session",
        PermissionChoice::AllowDirectory => "allowed for this directory",
        PermissionChoice::Deny => "declined",
        PermissionChoice::DenyAndStop => "declined and stopped",
    }
}

/// The clause a gate that nobody answered earns.
const fn outcome_clause(outcome: GateOutcome) -> &'static str {
    match outcome {
        GateOutcome::Allowed => "allowed",
        GateOutcome::Declined => "declined",
        GateOutcome::Answered => "answered",
        GateOutcome::Withdrawn => "withdrawn \u{b7} the agent stopped waiting",
    }
}

/// Whether a body is long enough to collapse.
fn collapses(text: &str, chars: usize, lines: usize) -> bool {
    text.chars().count() > chars || text.lines().count() > lines
}

/// The plan's title and the body it was promoted out of.
///
/// The first Markdown heading becomes the title and is removed from the body; a redundant
/// `## Summary` immediately after it is dropped too. A plan with no heading keeps `plan`.
pub(crate) fn split_plan(markdown: &str) -> (SharedString, String) {
    let mut lines = markdown.lines().peekable();
    let mut leading = Vec::new();
    let mut title = None;
    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            title = Some(heading.trim_start_matches('#').trim().to_owned());
            lines.next();
            break;
        }
        if trimmed.is_empty() {
            lines.next();
            continue;
        }
        leading.push((*line).to_owned());
        lines.next();
    }
    // A `## Summary` heading straight after the title says nothing the title did not.
    let mut rest: Vec<String> = lines.map(str::to_owned).collect();
    let redundant = rest
        .iter()
        .position(|line| !line.trim().is_empty())
        .filter(|index| {
            let line = rest[*index].trim().trim_start_matches('#').trim();
            line.eq_ignore_ascii_case("summary")
        });
    if let Some(index) = redundant {
        rest.remove(index);
    }
    leading.extend(rest);
    let body = leading.join("\n").trim().to_owned();
    (
        title
            .filter(|title| !title.is_empty())
            .map_or_else(|| SharedString::new_static("plan"), SharedString::from),
        body,
    )
}

/// The presentation properties of one 30 px tool row.
pub(crate) fn tool_row(inputs: &RowInputs<'_>, item: &Item, call: &ToolCall) -> ToolRow {
    let mut row = ToolRow::new(
        SharedString::from(item.id.to_string()),
        SharedString::from(kind_word(&call.kind)),
        SharedString::from(summary_text(item)),
    )
    .state(row_state(item.status))
    .icon(tool_icon(&call.kind))
    .expanded(inputs.is_expanded(item.id));
    // §5: an exit code is a structured field rendered explicitly, never a colour and never a
    // substring match on English error text.
    if let Some(code) = call.exit_code.filter(|code| *code != 0) {
        row = row.result(format_exit(code));
    }
    if let Some(body) = expanded_body(call, &row.summary) {
        row = row.body(body);
    }
    row
}

/// The lifecycle glyph a projected item status maps to.
pub(crate) const fn row_state(status: ItemStatus) -> ToolRowState {
    match status {
        ItemStatus::InProgress => ToolRowState::Running,
        ItemStatus::Completed => ToolRowState::Done,
        ItemStatus::Failed => ToolRowState::Failed,
        ItemStatus::Denied => ToolRowState::Denied,
        ItemStatus::Stopped => ToolRowState::Stopped,
    }
}

/// The expanded body, assembled once here and never in render.
///
/// Deduped blocks joined by a blank line, in `spec-B` §B1.2's order: the MCP call's arguments,
/// the raw command when it differs from the displayed one, the output detail, then the changed
/// paths. The collapsed line is never repeated.
fn expanded_body(call: &ToolCall, summary: &str) -> Option<SharedString> {
    let mut blocks: Vec<String> = Vec::new();
    let mut push = |block: String| {
        let block = block.trim().to_owned();
        if block.is_empty() || block == summary.trim() || blocks.contains(&block) {
            return;
        }
        blocks.push(block);
    };
    if matches!(call.kind, ToolKind::Mcp { .. })
        && let Ok(pretty) = serde_json::to_string_pretty(&call.input)
    {
        push(pretty);
    }
    if let Some(command) = call.input.get("command").and_then(|value| value.as_str()) {
        push(command.to_owned());
    }
    if !call.output.is_empty() {
        push(call.output.clone());
    }
    if let Some(result) = call.result.as_ref().map(display_json) {
        push(result);
    }
    if let Some(diff) = &call.diff {
        push(diff.path.display().to_string());
    }
    (!blocks.is_empty()).then(|| SharedString::from(blocks.join("\n\n")))
}

/// The fixed 60 px kind column: the normalized taxonomy, never the harness's spelling.
pub(crate) fn kind_word(kind: &ToolKind) -> String {
    match kind {
        ToolKind::Read => "read".to_owned(),
        ToolKind::Edit => "edit".to_owned(),
        ToolKind::Write => "write".to_owned(),
        ToolKind::Bash => "bash".to_owned(),
        ToolKind::Search => "search".to_owned(),
        ToolKind::Grep => "grep".to_owned(),
        ToolKind::Fetch => "fetch".to_owned(),
        ToolKind::Agent => "agent".to_owned(),
        ToolKind::Todo => "todo".to_owned(),
        ToolKind::Skill => "skill".to_owned(),
        ToolKind::Mcp { server } => format!("mcp:{server}"),
        // An unrecognized tool shows its raw harness name (§3.2).
        ToolKind::Unknown { name } => name.clone(),
    }
}

/// The glyph a tool kind draws when its state does not replace it (`spec-B` §B2).
pub(crate) const fn tool_icon(kind: &ToolKind) -> Icon {
    match kind {
        ToolKind::Read => Icon::Eye,
        ToolKind::Edit => Icon::FilePen,
        ToolKind::Write => Icon::SquarePen,
        ToolKind::Bash => Icon::Terminal,
        ToolKind::Search => Icon::Search,
        ToolKind::Grep => Icon::SearchCheck,
        ToolKind::Fetch => Icon::Globe,
        ToolKind::Agent => Icon::Bot,
        ToolKind::Todo => Icon::ClipboardCheck,
        ToolKind::Skill => Icon::Sparkles,
        ToolKind::Mcp { .. } => Icon::Boxes,
        ToolKind::Unknown { .. } => Icon::Wrench,
    }
}

/// The one-line summary: the adapter's, or the harness input's most identifying field.
pub(crate) fn summary_text(item: &Item) -> String {
    match &item.kind {
        ItemKind::Subagent {
            name, description, ..
        } => format!("{name} \u{b7} {description}"),
        ItemKind::Tool(call) => call
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                ["command", "file_path", "path", "pattern", "url", "query"]
                    .iter()
                    .find_map(|field| call.input.get(field).and_then(|value| value.as_str()))
                    .map_or_else(|| call.name.clone(), str::to_owned)
            }),
        _ => String::new(),
    }
}

/// A subagent spawn and its roster.
fn subagent_row(
    inputs: &RowInputs<'_>,
    items: &std::collections::HashMap<ItemId, &Item>,
    item: &Item,
    expanded: bool,
) -> SubagentRow {
    let children: Vec<SharedString> = item
        .children
        .iter()
        .filter_map(|child| items.get(child).copied())
        .rev()
        .take(SUBAGENT_RING)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|child| SharedString::from(cut(&child_line(child), SUBAGENT_LINE_CHARS)))
        .collect();
    let live =
        !super::is_settled(item.status) || inputs.projection.background_tasks.contains(&item.id);
    let status = if live {
        let running = item
            .children
            .iter()
            .filter(|child| {
                items
                    .get(*child)
                    .is_some_and(|child| !super::is_settled(child.status))
            })
            .count();
        Some(SharedString::from(format!("{running} working")))
    } else {
        Some(SharedString::new_static(match item.status {
            ItemStatus::Failed => "failed",
            ItemStatus::Stopped => "stopped",
            ItemStatus::Denied => "declined",
            _ => "\u{2713} done",
        }))
    };
    SubagentRow {
        summary: SharedString::from(summary_text(item)),
        status,
        tokens: None,
        children,
        expanded,
        live,
    }
}

/// One roster line of a subagent's child.
fn child_line(child: &Item) -> String {
    let summary = summary_text(child);
    if summary.is_empty() {
        item_text(child)
    } else {
        summary
    }
}

/// Cut a line to `max` characters without splitting a `char`.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    text.chars().take(max).collect()
}

/// How long an item ran, which is what a collapsed reasoning line states.
pub(crate) fn duration_ms(item: &Item) -> u64 {
    let Some(ended) = item.ended else {
        return 0;
    };
    u64::try_from(ended.signed_duration_since(item.started).num_milliseconds()).unwrap_or_default()
}

/// What an attachment pill is labelled with.
///
/// The harness's own display name wins; otherwise a path shows its file name and a URL or a blob
/// shows its media type, because neither a 4 kB data URI nor an absolute path reads as a label.
pub(crate) fn attachment_name(attachment: &Attachment) -> SharedString {
    if let Some(name) = attachment.name.as_deref() {
        return SharedString::new(name);
    }
    match &attachment.source {
        AttachmentSource::Path(path) => path
            .file_name()
            .map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
            .into(),
        AttachmentSource::Url(_) | AttachmentSource::Base64(_) => {
            SharedString::new(attachment.media_type.as_str())
        }
    }
}

/// Text rendered by the content-bearing item variants.
pub(crate) fn item_text(item: &Item) -> String {
    match &item.kind {
        ItemKind::UserMessage { text, .. }
        | ItemKind::AssistantText { text }
        | ItemKind::Plan { text } => text.clone(),
        ItemKind::Reasoning { summary, raw } => {
            // The part index is load-bearing: Codex marks paragraph boundaries with it, and
            // concatenating across one produces a run-on wall of text.
            let parts = if summary.is_empty() { raw } else { summary };
            parts
                .values()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n\n")
        }
        ItemKind::Error { message } => message.clone(),
        ItemKind::Tool(_) | ItemKind::Subagent { .. } => String::new(),
    }
}

/// A JSON value as one line of display text, without quoting a bare string.
fn display_json(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}
