//! One function per list row. Each takes plain git data plus two flags and returns an element.
//!
//! `selected` and `cursor` are separate on purpose: a side panel keeps its highlighted row when
//! focus moves elsewhere, which is lazygit's `HighlightInactive` behaviour exactly.

use fleet_git::{Branch, Commit, FileStatus, ReflogEntry, Remote, RemoteBranch, StashEntry, Tag};
use fleet_ui_kit::prelude::*;
use fleet_ui_kit::theme::ch;
use fleet_ui_kit::{ColumnAlign, Row, RowColumn};
use gpui::{AnyElement, App, Hsla, div};

use super::Ansi;
use super::file_tree::FileRow;
use crate::state::{has_staged, has_unstaged, recency, short_oid, upstream_status};

/// Finishes a row, painting the cursor row of an **unfocused** pane in the dimmer background.
///
/// lazygit's `HighlightInactive`: the pane that lost focus keeps showing where its cursor is, but
/// at a weaker weight than the pane that owns the keyboard, so only one row on screen reads as
/// "the selection". `Row` paints one selection colour, so the dim variant is painted behind it.
fn finish(row: Row, selected: bool, focused: bool, cx: &App) -> AnyElement {
    if selected && !focused {
        return div()
            .w_full()
            .bg(cx.theme().colors.row_hover)
            .child(row)
            .into_any_element();
    }
    row.into_any_element()
}

/// lazygit's three file-name colours: green fully staged, yellow partially staged, default
/// otherwise. There is no special case for conflicts, deletions or untracked files.
///
/// A directory row is coloured from the same two flags, aggregated over its descendants, which
/// is the only place a directory's status shows: lazygit prints no status characters on one.
#[must_use]
pub fn name_color(staged: bool, unstaged: bool, cx: &App) -> Hsla {
    let theme = cx.theme();
    if staged && !unstaged {
        Ansi::Green.color(theme)
    } else if staged {
        Ansi::Yellow.color(theme)
    } else {
        theme.colors.text
    }
}

/// lazygit's three file-name colours for a working-tree file.
#[must_use]
pub fn file_name_color(file: &FileStatus, cx: &App) -> Hsla {
    name_color(has_staged(file), has_unstaged(file), cx)
}

/// One Files-pane row: two spaces of indent per depth, then either the two independently
/// coloured status characters of a file or the `▼` / `▶` of a directory, then the name.
///
/// The line shapes are `pkg/gui/presentation/files.go:143`-`:179` exactly — no line art, no
/// status characters on a directory, and the indent sitting *before* the glyph column.
#[must_use]
pub fn file_tree_row(
    row: &FileRow,
    budget: usize,
    selected: bool,
    focused: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let name_color = name_color(row.staged, row.unstaged, cx);
    let glyph = if row.is_dir {
        div()
            .flex()
            .flex_row()
            .child(Text::data(row.arrow()).color(name_color))
            .into_any_element()
    } else {
        let [first, second] = row.status;
        let unstaged_color = Ansi::Red.color(theme);
        // The first character is green unless it is `?` (red) or blank (the name colour); the
        // second is red unless blank. `UU` therefore gets a green `U` and a red `U`, exactly as
        // lazygit does.
        let first_color = match first {
            '?' => unstaged_color,
            ' ' => name_color,
            _ => Ansi::Green.color(theme),
        };
        let second_color = if second == ' ' {
            name_color
        } else {
            unstaged_color
        };
        div()
            .flex()
            .flex_row()
            .child(Text::data(first.to_string()).color(first_color))
            .child(Text::data(second.to_string()).color(second_color))
            .into_any_element()
    };
    let indent = ch(2.0 * row.depth as f32);
    let leading = div()
        .flex()
        .flex_row()
        .items_center()
        .child(div().w(indent).flex_none())
        .child(glyph);

    let label = match &row.previous {
        // lazygit shortens a rename that did not leave its directory to the bare file name.
        Some(previous) if previous.parent() == row.path.parent() => format!(
            "{} → {}",
            previous
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            row.name
        ),
        Some(previous) => format!("{} → {}", previous.display(), row.name),
        None => row.name.clone(),
    };
    let budget = budget.saturating_sub(2 * row.depth).max(8);

    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed(indent + ch(2.0), leading))
        .column(RowColumn::flex(
            Text::data(label)
                .color(name_color)
                .truncate_at(budget, Truncate::Middle)
                .ellipsize(),
        ));
    finish(row, selected, focused, cx)
}

/// One local branch: recency, name, upstream divergence.
#[must_use]
pub fn branch_row(
    branch: &Branch,
    now: i64,
    budget: usize,
    selected: bool,
    focused: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let recency_text = recency(branch, now);
    let recency_color = if branch.is_head {
        Ansi::Green.color(theme)
    } else {
        Ansi::Cyan.color(theme)
    };
    let status = upstream_status(branch);
    let status_color = match status.as_deref() {
        Some("✓") => Ansi::Green.color(theme),
        Some(_) => Ansi::Yellow.color(theme),
        None => theme.colors.text_muted,
    };
    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed_ch(
            3.0,
            Text::data(recency_text).color(recency_color),
        ))
        .column(RowColumn::flex(
            Text::data(branch.name.clone())
                .truncate_at(budget, Truncate::Middle)
                .ellipsize(),
        ))
        .column(
            RowColumn::fixed_ch(
                9.0,
                Text::data(status.unwrap_or_default()).color(status_color),
            )
            .align(ColumnAlign::Right),
        );
    finish(row, selected, focused, cx)
}

/// The one-cell graph glyph: `◎` for a merge, `○` otherwise.
#[must_use]
pub fn graph_glyph(commit: &Commit) -> &'static str {
    if commit.parents.len() > 1 {
        "◎"
    } else {
        "○"
    }
}

/// What a commit row needs beyond the commit itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommitStyle {
    /// Characters the subject column may spend before the ellipsis.
    pub budget: usize,
    /// Reachable from a main branch: the sha turns green.
    pub merged: bool,
    /// Marked with `c` for a later paste: the sha turns cyan.
    pub copied: bool,
}

/// One commit: graph glyph, short sha coloured by push state, author initials, age, subject.
#[must_use]
pub fn commit_row(
    commit: &Commit,
    now: i64,
    style: CommitStyle,
    selected: bool,
    focused: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let CommitStyle {
        budget,
        merged,
        copied,
    } = style;
    // lazygit's single most recognisable signal: unpushed red, pushed yellow, merged green.
    let hash_color = if copied {
        Ansi::Cyan.color(theme)
    } else if merged {
        Ansi::Green.color(theme)
    } else if commit.pushed {
        Ansi::Yellow.color(theme)
    } else {
        Ansi::Red.color(theme)
    };
    let decorations: Vec<String> = commit
        .decorations
        .iter()
        .filter(|decoration| !decoration.is_empty())
        .cloned()
        .collect();
    // The decorations are capped at half the row's budget and pinned, so they can never squeeze
    // the subject out; the subject then takes what is left and ellipsizes. The flex row needs
    // `min_w_0` for that — without it the column overflows and the text is sliced mid-word with
    // no ellipsis at all.
    let decoration_text = decorations.join(" ");
    let mut subject = div()
        .flex()
        .flex_row()
        .items_center()
        .min_w_0()
        .overflow_hidden()
        .gap(theme.space.sm);
    let mut spent = 0;
    if !decoration_text.is_empty() {
        let cap = (budget / 2).max(8);
        spent = decoration_text.chars().count().min(cap) + 1;
        subject = subject.child(
            Text::data(decoration_text)
                .color(Ansi::Magenta.color(theme))
                .truncate_at(cap, Truncate::Tail)
                .flex_none(),
        );
    }
    subject = subject.child(
        Text::data(commit.subject.clone())
            .truncate_at(budget.saturating_sub(spent).max(8), Truncate::Tail)
            .ellipsize(),
    );

    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed_ch(
            2.0,
            Text::data(graph_glyph(commit)).color(hash_color),
        ))
        .column(RowColumn::fixed_ch(
            9.0,
            Text::data(short_oid(&commit.oid)).color(hash_color),
        ))
        .column(RowColumn::fixed_ch(
            3.0,
            Text::data(initials(&commit.author_name)).muted(),
        ))
        .column(
            RowColumn::fixed_ch(
                4.0,
                Text::data(crate::state::time_ago(
                    now.saturating_sub(commit.committed_at),
                ))
                .faint(),
            )
            .align(ColumnAlign::Right),
        )
        .column(RowColumn::flex(subject));
    finish(row, selected, focused, cx)
}

/// lazygit's two-character author column.
#[must_use]
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    match (words.next(), words.next()) {
        (Some(first), Some(second)) => {
            let mut initials = String::new();
            initials.extend(first.chars().next());
            initials.extend(second.chars().next());
            initials
        }
        (Some(only), None) => only.chars().take(2).collect(),
        _ => String::new(),
    }
}

/// One reflog entry.
#[must_use]
pub fn reflog_row(entry: &ReflogEntry, selected: bool, focused: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed_ch(
            9.0,
            Text::data(short_oid(&entry.oid)).color(Ansi::Blue.color(theme)),
        ))
        .column(RowColumn::fixed_ch(
            10.0,
            Text::data(entry.selector.clone()).faint(),
        ))
        .column(RowColumn::flex(
            Text::data(entry.subject.clone()).ellipsize(),
        ));
    finish(row, selected, focused, cx)
}

/// One stash entry, rendered as `stash@{n}: subject`.
#[must_use]
pub fn stash_row(entry: &StashEntry, selected: bool, focused: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed_ch(
            10.0,
            Text::data(format!("stash@{{{}}}", entry.index)).color(Ansi::Cyan.color(theme)),
        ))
        .column(RowColumn::flex(
            Text::data(entry.subject.clone()).ellipsize(),
        ));
    finish(row, selected, focused, cx)
}

/// One tag.
#[must_use]
pub fn tag_row(tag: &Tag, selected: bool, focused: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed_ch(
            24.0,
            Text::data(tag.name.clone()).ellipsize(),
        ))
        .column(RowColumn::flex(
            Text::data(tag.subject.clone())
                .color(Ansi::Yellow.color(theme))
                .ellipsize(),
        ));
    finish(row, selected, focused, cx)
}

/// The header row of the commit-files view: the commit itself, whose patch is the whole commit.
#[must_use]
pub fn commit_files_header_row(label: &str, selected: bool, focused: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed_ch(
            3.0,
            Text::data("◆").color(Ansi::Magenta.color(theme)),
        ))
        .column(RowColumn::flex(
            Text::data(label.to_owned())
                .weight(gpui::FontWeight::MEDIUM)
                .ellipsize(),
        ));
    finish(row, selected, focused, cx)
}

/// One file of a commit, with its `A`/`M`/`D` status character.
#[must_use]
pub fn commit_file_row(
    file: &fleet_git::CommitFile,
    budget: usize,
    selected: bool,
    focused: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (glyph, color) = match file.kind {
        fleet_git::DiffKind::Added => ("A", Ansi::Green.color(theme)),
        fleet_git::DiffKind::Deleted => ("D", Ansi::Red.color(theme)),
        fleet_git::DiffKind::Renamed => ("R", Ansi::Yellow.color(theme)),
        fleet_git::DiffKind::Copied => ("C", Ansi::Yellow.color(theme)),
        fleet_git::DiffKind::TypeChanged => ("T", Ansi::Yellow.color(theme)),
        fleet_git::DiffKind::Unmerged => ("U", Ansi::Red.color(theme)),
        fleet_git::DiffKind::Modified | fleet_git::DiffKind::Unknown => {
            ("M", Ansi::Yellow.color(theme))
        }
    };
    let mut label = file.path.display().to_string();
    if let Some(previous) = &file.previous_path {
        label = format!("{} → {label}", previous.display());
    }
    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed_ch(3.0, Text::data(glyph).color(color)))
        .column(RowColumn::flex(
            Text::data(label)
                .truncate_at(budget, Truncate::Middle)
                .ellipsize(),
        ));
    finish(row, selected, focused, cx)
}

/// One remote, with its branch count.
#[must_use]
pub fn remote_row(
    remote: &Remote,
    branches: usize,
    selected: bool,
    focused: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::fixed_ch(
            16.0,
            Text::data(remote.name.clone()).color(Ansi::Green.color(theme)),
        ))
        .column(RowColumn::flex(
            Text::data(format!("{branches} branches")).color(Ansi::Blue.color(theme)),
        ));
    finish(row, selected, focused, cx)
}

/// One remote branch.
#[must_use]
pub fn remote_branch_row(
    branch: &RemoteBranch,
    budget: usize,
    selected: bool,
    focused: bool,
    cx: &App,
) -> AnyElement {
    let row = Row::new()
        .selected(selected && focused)
        .cursor(selected && focused)
        .column(RowColumn::flex(
            Text::data(branch.name.clone())
                .truncate_at(budget, Truncate::Middle)
                .ellipsize(),
        ));
    finish(row, selected, focused, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_two_characters() {
        assert_eq!(initials("Danny Fuentes"), "DF");
        assert_eq!(initials("madonna"), "ma");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn a_merge_commit_gets_the_merge_glyph() {
        let mut commit = Commit {
            oid: fleet_git::ObjectId::from("a"),
            parents: vec![fleet_git::ObjectId::from("b")],
            author_name: String::new(),
            author_email: String::new(),
            authored_at: 0,
            committed_at: 0,
            subject: String::new(),
            body: String::new(),
            decorations: Vec::new(),
            pushed: false,
        };
        assert_eq!(graph_glyph(&commit), "○");
        commit.parents.push(fleet_git::ObjectId::from("c"));
        assert_eq!(graph_glyph(&commit), "◎");
    }
}
