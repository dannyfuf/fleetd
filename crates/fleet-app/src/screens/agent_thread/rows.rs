//! The keyed row model of a thread projection (`docs/NATIVE-AGENTS.md` §5).
//!
//! Building rows is a pure function of the projection plus the view-local expansion, queue and
//! decision state, so the whole §5 vocabulary — folds, footers, error cards, queued messages and
//! the trailing decision card — is testable without a window.

use std::collections::{HashMap, HashSet};

use fleet_core::agents::{
    Attachment, AttachmentSource, CheckpointKind, CheckpointRecord, Item, ItemId, ItemKind,
    ItemStatus, NoticeRecord, ThreadProjection, ToolKind, TurnEnd, TurnId, TurnOutcome, TurnRecord,
    Usage,
};
use fleet_ui_kit::{
    DecisionCard, ToolRow, ToolRowState, TranscriptRow, format_compacted, format_duration,
    format_resumed, format_retrying, format_token_count, format_turn_footer, format_worked,
    parse_markdown_document,
};
use gpui::SharedString;

/// Everything outside the projection that a row build depends on.
pub(crate) struct RowInputs<'a> {
    /// The daemon projection the rows describe.
    pub(crate) projection: &'a ThreadProjection,
    /// Items whose body the user expanded with `⏎`.
    pub(crate) expanded: &'a HashSet<ItemId>,
    /// Turns whose `worked …` fold the user opened with `⏎`.
    pub(crate) unfolded: &'a HashSet<TurnId>,
    /// Composer text waiting behind active work, oldest first.
    pub(crate) queued: &'a [String],
    /// Decision cards for the currently open gates, in opening order.
    pub(crate) cards: &'a [DecisionCard],
}

/// What a row toggles when it is clicked, by row index.
///
/// Row focus does not exist yet (§10), so a click is the only way to open a fold or a reasoning
/// body; the anchors are what turn one back into the item or turn it belongs to.
#[derive(Debug, Default)]
pub(crate) struct RowAnchors {
    /// Items whose body a row exposes.
    pub(crate) items: HashMap<usize, ItemId>,
    /// Turns whose `worked …` fold a row draws.
    pub(crate) folds: HashMap<usize, TurnId>,
}

/// The row model plus the anchors that make its rows actionable.
#[derive(Debug, Default)]
pub(crate) struct BuiltRows {
    /// Rows in display order.
    pub(crate) rows: Vec<TranscriptRow>,
    /// What each row toggles.
    pub(crate) anchors: RowAnchors,
}

/// Builds the complete ordered row model for one thread.
pub(crate) fn build_rows(inputs: &RowInputs<'_>) -> BuiltRows {
    let projection = inputs.projection;
    let mut anchors = RowAnchors::default();
    let mut rows = Vec::new();
    let items: HashMap<ItemId, &Item> = projection
        .items
        .iter()
        .map(|item| (item.id, item))
        .collect();

    // §5: a compaction or a resume is its own row, at the boundary it happened on, and §3.2's
    // user-facing notice is a row at the point it was said rather than a line only the log has.
    rows.extend(checkpoint_rows(&projection.checkpoints, None));
    rows.extend(notice_rows(&projection.notices, None));
    for turn in &projection.turns {
        let built = turn_rows(inputs, &items, turn);
        anchors.extend(&built, rows.len());
        rows.extend(built.rows);
        rows.extend(checkpoint_rows(&projection.checkpoints, Some(turn.id)));
        rows.extend(notice_rows(&projection.notices, Some(turn.id)));
    }
    // Items the reducer accepted before their turn was recorded still have to be visible; they
    // are appended in projection order rather than silently dropped.
    let recorded: HashSet<TurnId> = projection.turns.iter().map(|turn| turn.id).collect();
    for item in &projection.items {
        if item.parent.is_none() && !recorded.contains(&item.turn) {
            let built = item_row(inputs, &items, item);
            anchors.extend(&built, rows.len());
            rows.extend(built.rows);
        }
    }

    if let Some(retry) = &projection.retrying {
        rows.push(TranscriptRow::ErrorCard {
            // DESIGN-SYSTEM §6.6: `components::agent::format` holds the copy for these lines,
            // so the app states the attempt beside it rather than rewording the whole row.
            message: SharedString::from(format!(
                "{} \u{b7} attempt {}",
                format_retrying(&retry.reason, retry.retry_in_ms),
                retry.attempt
            )),
            // A backoff still counting down is progress: the card shows the spinner, not the
            // red cross.
            retrying: true,
        });
    }
    if let Some(code) = projection.exit_code {
        rows.push(TranscriptRow::ErrorCard {
            message: SharedString::from(format!("{} exited {code}", provider_word(projection))),
            retrying: false,
        });
    }

    for text in inputs.queued {
        rows.push(TranscriptRow::QueuedMessage {
            text: SharedString::new(text.as_str()),
        });
    }
    // §2: a decision card is always the last item in the thread.
    for card in inputs.cards {
        rows.push(TranscriptRow::DecisionCard(card.clone()));
    }

    if rows.is_empty() {
        rows.push(TranscriptRow::EmptyState {
            message: SharedString::from(format!("Message {} to start", provider_word(projection))),
        });
    }
    BuiltRows { rows, anchors }
}

impl RowAnchors {
    /// Shifts one nested build's anchors onto the position its rows landed at.
    fn extend(&mut self, built: &BuiltRows, offset: usize) {
        for (index, item) in &built.anchors.items {
            self.items.insert(index + offset, *item);
        }
        for (index, turn) in &built.anchors.folds {
            self.folds.insert(index + offset, *turn);
        }
    }

    /// Makes room for one row spliced in at `at`, so every anchor below it still points at the
    /// row it was built for.
    fn shift_for_insert(&mut self, at: usize) {
        fn shift<T>(map: &mut HashMap<usize, T>, at: usize) {
            let moved = std::mem::take(map);
            map.extend(
                moved
                    .into_iter()
                    .map(|(index, value)| (if index >= at { index + 1 } else { index }, value)),
            );
        }
        shift(&mut self.items, at);
        shift(&mut self.folds, at);
    }
}

/// The boundary lines recorded after one turn, in the order they were observed.
fn checkpoint_rows(
    checkpoints: &[CheckpointRecord],
    after_turn: Option<TurnId>,
) -> Vec<TranscriptRow> {
    checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.after_turn == after_turn)
        .map(|checkpoint| TranscriptRow::CheckpointLine {
            text: match &checkpoint.kind {
                CheckpointKind::CompactBoundary { before, after } => {
                    format_compacted(*before, *after)
                }
                CheckpointKind::Resumed { age_ms } => format_resumed(*age_ms),
            },
        })
        .collect()
}

/// The notices recorded after one turn, in the order they were observed.
fn notice_rows(notices: &[NoticeRecord], after_turn: Option<TurnId>) -> Vec<TranscriptRow> {
    notices
        .iter()
        .filter(|notice| notice.after_turn == after_turn)
        .map(|notice| TranscriptRow::Notice {
            text: SharedString::new(notice.text.as_str()),
        })
        .collect()
}

/// The executable name used in transcript copy, which is what the tab strip also shows.
fn provider_word(projection: &ThreadProjection) -> &'static str {
    projection.provider.executable()
}

/// The rows of one recorded turn: the user block, its work, its fold and its footer.
fn turn_rows(
    inputs: &RowInputs<'_>,
    items: &HashMap<ItemId, &Item>,
    turn: &TurnRecord,
) -> BuiltRows {
    let mut anchors = RowAnchors::default();
    let mut rows = Vec::new();
    let mut folded = 0_usize;
    // §2: "completed work folds to `worked 12s · 3 tool calls`" — the fold *stands in for the
    // work*, so it belongs where that work was. A real turn is text → tools → text, and
    // appending the fold after the loop put it below the assistant's closing paragraph, as if
    // the tools had run after the answer.
    let mut work_at = None;
    for item in inputs
        .projection
        .items
        .iter()
        .filter(|item| item.turn == turn.id && item.parent.is_none())
    {
        // Only successful tool work folds; a failed row stays exposed after the turn settles.
        let completed_work = is_tool(item) && item.status == ItemStatus::Done;
        if completed_work && turn.ended.is_some() {
            work_at.get_or_insert(rows.len());
        }
        if completed_work && turn.ended.is_some() && !inputs.unfolded.contains(&turn.id) {
            folded += 1;
            continue;
        }
        let built = item_row(inputs, items, item);
        anchors.extend(&built, rows.len());
        rows.extend(built.rows);
    }
    // Only the `[⏎]` label changes between the two states: the fold keeps saying what it folds,
    // so opening one never loses the duration and the count (§2).
    let unfolded = turn.ended.is_some() && inputs.unfolded.contains(&turn.id);
    let folded_tools = folded + unfolded_tool_count(inputs, turn);
    if folded > 0 || unfolded {
        // §2: `worked 12s` is the turn's *own* time, so it reads the same corrected duration
        // the footer does — the provider's wall clock still has every gate's open window in it.
        let duration = turn.footer().map_or(0, |footer| footer.duration_ms);
        let at = work_at.unwrap_or(rows.len());
        anchors.shift_for_insert(at);
        anchors.folds.insert(at, turn.id);
        rows.insert(
            at,
            TranscriptRow::WorkedFold {
                text: format_worked(duration, folded_tools),
                expanded: unfolded,
            },
        );
    }
    if let Some(end) = &turn.ended {
        rows.push(TranscriptRow::TurnFooter {
            text: SharedString::from(footer_text(turn, end)),
        });
        if let TurnOutcome::Error { message } = &end.outcome {
            rows.push(TranscriptRow::ErrorCard {
                message: SharedString::from(
                    message
                        .clone()
                        .unwrap_or_else(|| "the turn failed".to_owned()),
                ),
                retrying: false,
            });
        }
    }
    BuiltRows { rows, anchors }
}

/// How many successful tool rows an opened fold is showing.
fn unfolded_tool_count(inputs: &RowInputs<'_>, turn: &TurnRecord) -> usize {
    if turn.ended.is_none() || !inputs.unfolded.contains(&turn.id) {
        return 0;
    }
    inputs
        .projection
        .items
        .iter()
        .filter(|item| {
            item.turn == turn.id
                && item.parent.is_none()
                && is_tool(item)
                && item.status == ItemStatus::Done
        })
        .count()
}

/// Whether an item occupies a 30 px tool row.
fn is_tool(item: &Item) -> bool {
    matches!(item.kind, ItemKind::Tool { .. } | ItemKind::Subagent { .. })
}

/// One top-level or nested transcript item, expanded into its rows.
fn item_row(inputs: &RowInputs<'_>, items: &HashMap<ItemId, &Item>, item: &Item) -> BuiltRows {
    let mut anchors = RowAnchors::default();
    // §2 makes `thinking · 6s  [⏎] show` a fold over reasoning that exists. Claude opens a
    // thinking block for a signature-only reasoning frame and settles it with no delta at all,
    // and prose is likewise sometimes announced and never written: a settled item with nothing
    // in it is not a row, it is a placeholder whose `[⏎] show` expands to nothing.
    if item.text.as_deref().unwrap_or_default().trim().is_empty()
        && item.status != ItemStatus::Running
        && matches!(item.kind, ItemKind::Thinking | ItemKind::AssistantText)
    {
        return BuiltRows {
            rows: Vec::new(),
            anchors,
        };
    }
    let rows = match &item.kind {
        ItemKind::UserMessage { attachments, text } => vec![TranscriptRow::UserBlock {
            text: SharedString::new(text.as_str()),
            attachments: attachments.iter().map(attachment_name).collect(),
        }],
        ItemKind::AssistantText => vec![TranscriptRow::AssistantText {
            markdown: parse_markdown_document(item.text.as_deref().unwrap_or_default()),
        }],
        ItemKind::Thinking => {
            anchors.items.insert(0, item.id);
            vec![TranscriptRow::Thinking {
                text: SharedString::new(item.text.as_deref().unwrap_or_default()),
                duration_ms: item_duration_ms(item),
                expanded: inputs.expanded.contains(&item.id),
            }]
        }
        ItemKind::Error => vec![TranscriptRow::ErrorCard {
            retrying: false,
            message: SharedString::new(
                item.text
                    .as_deref()
                    .or(item.result.as_deref())
                    .unwrap_or("the provider reported an error"),
            ),
        }],
        ItemKind::Tool { .. } | ItemKind::Subagent { .. } => {
            let children = item
                .children
                .iter()
                .filter_map(|child| items.get(child).copied())
                .flat_map(|child| item_row(inputs, items, child).rows)
                .collect();
            anchors.items.insert(0, item.id);
            vec![TranscriptRow::ToolRow {
                row: tool_row(inputs, item),
                children,
            }]
        }
    };
    BuiltRows { rows, anchors }
}

/// How long an item ran, which is what a collapsed reasoning line states.
fn item_duration_ms(item: &Item) -> u64 {
    let Some(ended) = item.ended else {
        return 0;
    };
    u64::try_from(ended.signed_duration_since(item.started).num_milliseconds()).unwrap_or_default()
}

/// What an attachment pill is labelled with.
///
/// The provider's own display name wins; otherwise a path shows its file name and a URL or a
/// blob shows its media type, because neither a 4 kB data URI nor an absolute path reads as a
/// label on a 22 px pill.
fn attachment_name(attachment: &Attachment) -> SharedString {
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

/// The presentation properties of one tool or subagent row.
fn tool_row(inputs: &RowInputs<'_>, item: &Item) -> ToolRow {
    ToolRow {
        id: SharedString::from(item.id.to_string()),
        state: row_state(item.status),
        kind: SharedString::from(kind_label(item)),
        summary: SharedString::from(summary_text(item)),
        result: item.result.as_deref().map(SharedString::new),
        output: item.output.as_deref().map(SharedString::new),
        diff: item
            .diff
            .as_ref()
            .map(|diff| SharedString::new(diff.unified.as_str())),
        expanded: inputs.expanded.contains(&item.id),
    }
}

/// The lifecycle glyph a projected item status maps to.
const fn row_state(status: ItemStatus) -> ToolRowState {
    match status {
        ItemStatus::Pending | ItemStatus::Running => ToolRowState::Running,
        ItemStatus::Done => ToolRowState::Done,
        ItemStatus::Error => ToolRowState::Error,
        ItemStatus::Denied => ToolRowState::Denied,
    }
}

/// The fixed 60 px kind column (§2): the normalized taxonomy, never the provider's spelling.
fn kind_label(item: &Item) -> String {
    let ItemKind::Tool { kind, name, .. } = &item.kind else {
        return "agent".to_owned();
    };
    let label = match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Write => "write",
        ToolKind::Bash => "bash",
        ToolKind::Search => "search",
        ToolKind::Grep => "grep",
        ToolKind::Fetch => "fetch",
        ToolKind::Agent => "agent",
        ToolKind::Todo => "todo",
        ToolKind::Skill => "skill",
        ToolKind::Mcp { server } => return format!("mcp:{server}"),
        // An unrecognized tool shows its raw provider name (§3.2).
        ToolKind::Unknown { name } => name.as_str(),
    };
    if label.is_empty() {
        name.clone()
    } else {
        label.to_owned()
    }
}

/// The one-line summary: the adapter's, or the provider input's most identifying field.
fn summary_text(item: &Item) -> String {
    if let Some(summary) = item
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return summary.to_owned();
    }
    match &item.kind {
        ItemKind::Subagent { name, description } => format!("{name} · {description}"),
        ItemKind::Tool { name, input, .. } => ["command", "file_path", "path", "pattern", "url"]
            .iter()
            .find_map(|field| input.get(field).and_then(|value| value.as_str()))
            .map_or_else(|| name.clone(), str::to_owned),
        _ => String::new(),
    }
}

/// The right-aligned completed-turn footer (§2).
///
/// DESIGN-SYSTEM §6.6: keycaps are never inside these strings — `[⏎] diff` and `[u] revert turn`
/// are `KeyHint`s the footer row owns — and the copy itself is the kit's, so the app and the
/// canvas cannot round a duration differently.
///
/// The numbers come from [`TurnRecord::footer`], never from [`TurnEnd`] directly: §2 reads the
/// footer as the turn's own duration, and only the record knows how long the turn stood parked
/// on a gate. `TurnEnd.duration_ms` is the provider's wall clock, gate window included.
fn footer_text(turn: &TurnRecord, end: &TurnEnd) -> String {
    let mut parts = Vec::new();
    match &end.outcome {
        TurnOutcome::Interrupted => parts.push("stopped".to_owned()),
        TurnOutcome::Error { .. } => parts.push("failed".to_owned()),
        TurnOutcome::Denied => parts.push("denied".to_owned()),
        TurnOutcome::MaxTurns => parts.push("turn limit".to_owned()),
        TurnOutcome::BudgetExhausted => parts.push("budget exhausted".to_owned()),
        TurnOutcome::Other { reason } => parts.push(reason.clone()),
        TurnOutcome::Completed => {}
    }
    let Some(footer) = turn.footer() else {
        return parts.join(" \u{b7} ");
    };
    parts.push(
        format_turn_footer(
            footer.duration_ms,
            token_total(&end.usage),
            footer.files_changed,
            footer.added,
            footer.removed,
        )
        .to_string(),
    );
    parts.join(" \u{b7} ")
}

/// The token count a footer states, falling back to the parts when no total was reported.
fn token_total(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input_tokens + usage.output_tokens
    }
}

/// Thousands and millions collapse to one decimal so the footer never reflows.
pub(crate) fn count_label(value: u64) -> String {
    format_token_count(value).to_string()
}

/// `0.4s`, `48s`, `12m`, `2h` — the coarsest unit that still reads as a duration.
pub(crate) fn duration_label(ms: u64) -> String {
    format_duration(ms).to_string()
}
