use super::*;

/// The one line of a failure the status bar's error slot shows.
///
/// Git puts the useful sentence anywhere in its stderr — `git merge` prints `Auto-merging …`
/// before `CONFLICT (content): …` — so the first line is often not the one worth 60 characters
/// of a one-row bar. The first line carrying a Git error marker wins; otherwise the first
/// non-blank line does.
#[must_use]
pub(crate) fn error_line(message: &str) -> String {
    const MARKERS: [&str; 5] = ["fatal:", "error:", "CONFLICT", "warning:", "hint:"];
    let mut lines = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let mut first = None;
    let mut marked = None;
    for line in &mut lines {
        if first.is_none() {
            first = Some(line);
        }
        if marked.is_none() && MARKERS.iter().any(|marker| line.starts_with(marker)) {
            marked = Some(line);
            break;
        }
    }
    marked
        .or(first)
        .unwrap_or_else(|| message.trim())
        .to_owned()
}

/// The lazygit mode word for an operation state, or `None` when nothing is in progress.
#[must_use]
pub(crate) fn mode_word(operation: &OperationState) -> Option<&'static str> {
    match operation {
        OperationState::None => None,
        OperationState::Merging => Some("MERGING"),
        OperationState::Rebasing { .. } => Some("REBASING"),
        OperationState::CherryPicking => Some("PICKING"),
        OperationState::Reverting => Some("REVERTING"),
        OperationState::Bisecting => Some("BISECTING"),
    }
}

/// lazygit's lowercase, parenthesised mode word for the status panel.
#[must_use]
pub(crate) fn lower_mode_word(operation: &OperationState) -> Option<&'static str> {
    match operation {
        OperationState::None => None,
        OperationState::Merging => Some("merging"),
        OperationState::Rebasing { .. } => Some("rebasing"),
        OperationState::CherryPicking => Some("cherry-picking"),
        OperationState::Reverting => Some("reverting"),
        OperationState::Bisecting => Some("bisecting"),
    }
}

/// The two-character short status lazygit prints, index char first.
#[must_use]
pub(crate) fn short_status(file: &FileStatus) -> [char; 2] {
    if file.index == ChangeKind::Untracked || file.worktree == ChangeKind::Untracked {
        return ['?', '?'];
    }
    [status_char(file.index), status_char(file.worktree)]
}

/// Whether the file has anything in the index (lazygit's `hasStagedChanges`).
#[must_use]
pub(crate) fn has_staged(file: &FileStatus) -> bool {
    let [index, _] = short_status(file);
    !matches!(index, ' ' | 'U' | '?')
}

/// Whether the file has anything in the worktree (lazygit's `HasUnstagedChanges`).
#[must_use]
pub(crate) fn has_unstaged(file: &FileStatus) -> bool {
    let [_, worktree] = short_status(file);
    worktree != ' '
}

/// lazygit's `↓n↑m` / `✓` upstream string, or `None` when the branch has no upstream.
#[must_use]
pub(crate) fn upstream_status(branch: &Branch) -> Option<String> {
    let upstream = branch.upstream.as_ref()?;
    if upstream.ahead == 0 && upstream.behind == 0 {
        return Some("✓".to_owned());
    }
    let mut text = String::new();
    if upstream.behind > 0 {
        text.push_str(&format!("↓{}", upstream.behind));
    }
    if upstream.ahead > 0 {
        text.push_str(&format!("↑{}", upstream.ahead));
    }
    Some(text)
}

/// lazygit's three-character recency string (`"  *"` for the checked-out branch).
///
/// The column is *checkout* recency: `Branch::checked_out_at` when the HEAD reflog mentions the
/// branch, and the tip's committer date only as a fallback, exactly as lazygit's branch loader
/// fills `models.Branch.Recency`.
#[must_use]
pub(crate) fn recency(branch: &Branch, now: i64) -> String {
    if branch.is_head {
        return "  *".to_owned();
    }
    let at = branch.checked_out_at.unwrap_or(branch.committed_at);
    time_ago(now.saturating_sub(at))
}

/// lazygit's `utils.UnixToTimeAgo`: a number plus one of `s m h d w M y`.
#[must_use]
pub(crate) fn time_ago(seconds: i64) -> String {
    let seconds = seconds.max(0);
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    if seconds < MINUTE {
        format!("{seconds}s")
    } else if seconds < HOUR {
        format!("{}m", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}h", seconds / HOUR)
    } else if seconds < WEEK {
        format!("{}d", seconds / DAY)
    } else if seconds < MONTH {
        format!("{}w", seconds / WEEK)
    } else if seconds < YEAR {
        format!("{}M", seconds / MONTH)
    } else {
        format!("{}y", seconds / YEAR)
    }
}

/// The short hash lazygit shows: the first eight characters.
#[must_use]
pub(crate) fn short_oid(oid: &ObjectId) -> String {
    oid.as_str().chars().take(8).collect()
}
fn status_char(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Unmodified => ' ',
        ChangeKind::Added => 'A',
        ChangeKind::Modified => 'M',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
        ChangeKind::Copied => 'C',
        ChangeKind::TypeChanged => 'T',
        ChangeKind::Untracked => '?',
        ChangeKind::Ignored => '!',
        ChangeKind::Unmerged => 'U',
        ChangeKind::Unknown(byte) => byte as char,
    }
}
