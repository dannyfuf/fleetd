//! Turn-usage arithmetic and changed-file totals.

use super::TurnRecord;
use crate::agents::{FileDelta, Usage};

pub(super) fn file_totals(files: &[FileDelta]) -> (u64, u64) {
    files.iter().fold((0, 0), |(added, removed), file| {
        (
            added.saturating_add(file.added),
            removed.saturating_add(file.removed),
        )
    })
}

pub(super) fn aggregate_usage(turns: &[TurnRecord]) -> Usage {
    turns.iter().filter_map(|turn| turn.ended.as_ref()).fold(
        Usage::default(),
        |mut cumulative, ended| {
            add_usage(&mut cumulative, &ended.usage);
            cumulative
        },
    )
}

pub(super) fn add_usage(cumulative: &mut Usage, usage: &Usage) {
    cumulative.input_tokens = cumulative.input_tokens.saturating_add(usage.input_tokens);
    cumulative.output_tokens = cumulative.output_tokens.saturating_add(usage.output_tokens);
    cumulative.reasoning_tokens = cumulative
        .reasoning_tokens
        .saturating_add(usage.reasoning_tokens);
    cumulative.cache_read_tokens = cumulative
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    cumulative.cache_write_tokens = cumulative
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
    cumulative.total_tokens = cumulative.total_tokens.saturating_add(usage.total_tokens);
    cumulative.web_search_requests = cumulative
        .web_search_requests
        .saturating_add(usage.web_search_requests);
    cumulative.tool_uses = cumulative.tool_uses.saturating_add(usage.tool_uses);
    for (key, value) in &usage.extra {
        cumulative.extra.insert(key.clone(), value.clone());
    }
}

pub(super) fn subtract_usage(cumulative: &Usage, completed: &Usage) -> Usage {
    Usage {
        input_tokens: cumulative
            .input_tokens
            .saturating_sub(completed.input_tokens),
        output_tokens: cumulative
            .output_tokens
            .saturating_sub(completed.output_tokens),
        reasoning_tokens: cumulative
            .reasoning_tokens
            .saturating_sub(completed.reasoning_tokens),
        cache_read_tokens: cumulative
            .cache_read_tokens
            .saturating_sub(completed.cache_read_tokens),
        cache_write_tokens: cumulative
            .cache_write_tokens
            .saturating_sub(completed.cache_write_tokens),
        total_tokens: cumulative
            .total_tokens
            .saturating_sub(completed.total_tokens),
        web_search_requests: cumulative
            .web_search_requests
            .saturating_sub(completed.web_search_requests),
        tool_uses: cumulative.tool_uses.saturating_sub(completed.tool_uses),
        extra: cumulative.extra.clone(),
    }
}
