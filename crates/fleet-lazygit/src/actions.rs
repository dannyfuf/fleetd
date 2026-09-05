//! Every gpui action, one per row of the key table in [`crate::keymap`].
//!
//! Actions are grouped into namespaces that mirror the key contexts, so the same word (`Delete`,
//! `Confirm`, `Enter`) can mean different things in disjoint panels without colliding: gpui
//! registers an action under `namespace::Name`.
//!
//! Nothing here holds state. A handler on [`crate::root::Lazygit`] reduces the action against
//! [`crate::state::GitUiState`] and dispatches to the git bridge.

/// Global actions, live in every panel.
pub mod global {
    use gpui::actions;

    actions!(
        global,
        [
            /// `q` — quit; asks first when an operation is in progress.
            Quit,
            /// `R` — refresh the git state. Never fetches.
            Refresh,
            /// `?` — open the generated keybinding help.
            OpenHelp,
            /// `+` — next screen mode (normal → half → full).
            NextScreenMode,
            /// `_` — previous screen mode.
            PrevScreenMode,
            /// `@` — show or hide the command log.
            ToggleCommandLog,
            /// `esc` — leave the main panel, or close the top overlay.
            Cancel,
            /// `1` — focus the Status panel.
            FocusStatus,
            /// `2` — focus the Files panel.
            FocusFiles,
            /// `3` — focus the Branches panel.
            FocusBranches,
            /// `4` — focus the Commits panel.
            FocusCommits,
            /// `5` — focus the Stash panel.
            FocusStash,
            /// `0` — focus the main panel.
            FocusMain,
            /// `tab` — focus the next side panel.
            NextPanel,
            /// `shift-tab` — focus the previous side panel.
            PrevPanel,
            /// `]` — next tab of the focused panel.
            NextTab,
            /// `[` — previous tab of the focused panel.
            PrevTab,
            /// `p` — pull.
            Pull,
            /// `P` — push.
            Push,
            /// `f` — fetch.
            Fetch,
            /// `m` — merge/rebase options (continue, abort, skip).
            OperationMenu,
        ]
    );
}

/// List navigation, inherited by every panel.
pub mod list {
    use gpui::actions;

    actions!(
        list,
        [
            /// `j` / `down` — next item.
            MoveDown,
            /// `k` / `up` — previous item.
            MoveUp,
            /// `g g` / `<` / `home` — first item.
            GoTop,
            /// `G` / `>` / `end` — last item.
            GoBottom,
            /// `ctrl-d` / `.` — half page down.
            PageDown,
            /// `ctrl-u` / `,` — half page up.
            PageUp,
            /// `H` — scroll the main panel left.
            ScrollLeft,
            /// `L` — scroll the main panel right.
            ScrollRight,
        ]
    );
}

/// The diff view itself: layout and how much context it shows.
pub mod diff {
    use gpui::actions;

    actions!(
        diff,
        [
            /// `|` — switch the diff between unified and side by side.
            ToggleSplit,
            /// `{` — show fewer context lines around each hunk.
            LessContext,
            /// `}` — show more context lines around each hunk.
            MoreContext,
        ]
    );
}

/// The Files panel.
pub mod files {
    use gpui::actions;

    actions!(
        files,
        [
            /// `space` — stage or unstage the selected file.
            ToggleStaged,
            /// `a` — stage everything, or unstage everything when nothing is unstaged.
            ToggleStagedAll,
            /// `d` — discard the selected file's changes.
            Discard,
            /// `c` — commit the index.
            Commit,
            /// `A` — amend the last commit with the index.
            Amend,
            /// `S` — stash options.
            StashMenu,
            /// `` ` `` / `~` — switch the pane between the tree and the flat layout.
            ToggleTree,
            /// `-` — collapse every directory.
            CollapseAll,
            /// `=` — expand every directory.
            ExpandAll,
            /// `enter` — enter staging mode, the conflict view, or collapse a directory.
            Enter,
        ]
    );
}

/// The Branches panel, Local tab.
pub mod branches {
    use gpui::actions;

    actions!(
        branches,
        [
            /// `space` — check out the selected branch.
            Checkout,
            /// `n` — create a branch.
            New,
            /// `d` — delete the selected branch.
            Delete,
            /// `R` — rename the selected branch.
            Rename,
            /// `M` — merge the selected branch into the current one.
            Merge,
            /// `r` — rebase the current branch onto the selected one.
            Rebase,
            /// `u` — upstream options.
            UpstreamMenu,
            /// `T` — create a tag at the selected branch.
            Tag,
            /// `enter` — show the branch's diff in the main panel.
            Enter,
        ]
    );
}

/// The Branches panel, Remotes tab.
pub mod remotes {
    use gpui::actions;

    actions!(
        remotes,
        [
            /// `enter` — list the remote's branches, or show a branch's diff.
            Enter,
            /// `space` — check out a local tracking branch for the selected remote branch.
            Checkout,
        ]
    );
}

/// The Branches panel, Tags tab.
pub mod tags {
    use gpui::actions;

    actions!(
        tags,
        [
            /// `n` — create a tag at HEAD.
            New,
            /// `d` — delete the selected tag.
            Delete,
            /// `space` — check out the selected tag as a detached HEAD.
            Checkout,
        ]
    );
}

/// The Commits panel.
pub mod commits {
    use gpui::actions;

    actions!(
        commits,
        [
            /// `enter` — show the commit's files and diff.
            Enter,
            /// `space` — check out the commit as a detached HEAD.
            Checkout,
            /// `r` — reword the commit.
            Reword,
            /// `s` — squash the commit into its parent.
            Squash,
            /// `f` — fixup the commit into its parent.
            Fixup,
            /// `d` — drop the commit.
            Drop,
            /// `e` — start an interactive rebase that stops at the commit.
            Edit,
            /// `ctrl-j` — move the commit one place down.
            MoveDown,
            /// `ctrl-k` — move the commit one place up.
            MoveUp,
            /// `g` — reset options (soft/mixed/hard).
            ResetMenu,
            /// `c` — mark the commit as copied for a later paste.
            Copy,
            /// `v` — cherry-pick the copied commits.
            Paste,
            /// `t` — revert the commit.
            Revert,
            /// `T` — tag the commit.
            Tag,
            /// `A` — amend the commit with the staged changes.
            Amend,
            /// `n` — create a branch at the commit.
            NewBranch,
        ]
    );
}

/// The Stash panel.
pub mod stash {
    use gpui::actions;

    actions!(
        stash,
        [
            /// `space` — apply the entry.
            Apply,
            /// `g` — pop the entry.
            Pop,
            /// `d` — drop the entry.
            Drop,
            /// `n` — create a branch from the entry.
            NewBranch,
            /// `enter` — show the entry's diff.
            Enter,
        ]
    );
}

/// The main panel in staging mode.
pub mod staging {
    use gpui::actions;

    actions!(
        staging,
        [
            /// `space` — stage or unstage the selection.
            Apply,
            /// `d` — discard the selection.
            Discard,
            /// `v` — start or clear a range selection.
            ToggleRange,
            /// `a` — switch between hunk and line selection.
            ToggleLineMode,
            /// `tab` — switch between the unstaged and staged halves.
            SwitchSide,
            /// `h` — previous hunk.
            PrevHunk,
            /// `l` — next hunk.
            NextHunk,
        ]
    );
}

/// The main panel showing a ref's commits (lazygit's sub-commits view).
pub mod subcommits {
    use gpui::actions;

    actions!(
        subcommits,
        [
            /// `enter` / `space` — show the selected sub-commit's patch below the list.
            ShowDiff,
        ]
    );
}

/// The main panel showing a commit's changed files (lazygit's commit-files view).
pub mod commitfiles {
    use gpui::actions;

    actions!(
        commitfiles,
        [
            /// `enter` — show the selected file's patch, or the whole commit on the header row.
            ShowPatch,
        ]
    );
}

/// The main panel in conflict-resolution mode.
pub mod conflict {
    use gpui::actions;

    actions!(
        conflict,
        [
            /// `o` — keep our side.
            TakeOurs,
            /// `t` — keep their side.
            TakeTheirs,
            /// `b` — keep both sides.
            TakeBoth,
            /// `l` — next conflict section.
            NextSection,
            /// `h` — previous conflict section.
            PrevSection,
        ]
    );
}

/// The confirmation dialog.
pub mod confirm {
    use gpui::actions;

    actions!(
        confirm,
        [
            /// `enter` / `y` — confirm.
            Accept,
            /// `esc` / `n` — cancel.
            Cancel,
        ]
    );
}

/// The text prompt. **No single-character key may be bound here**, or typing runs a command.
pub mod prompt {
    use gpui::actions;

    actions!(
        prompt,
        [
            /// `enter` — confirm, or insert a newline in a multi-line prompt.
            Accept,
            /// `cmd-enter` / `ctrl-enter` — always confirm.
            Submit,
            /// `esc` — cancel.
            Cancel,
            /// `backspace` — delete the character before the caret.
            Backspace,
            /// `ctrl-w` — delete the word before the caret.
            DeleteWord,
            /// `ctrl-u` — delete to the start of the line.
            DeleteToStart,
            /// `left` — move the caret left.
            Left,
            /// `right` — move the caret right.
            Right,
            /// `ctrl-a` / `home` — move the caret to the start.
            Home,
            /// `ctrl-e` / `end` — move the caret to the end.
            End,
            /// `cmd-v` / `ctrl-v` — paste the clipboard.
            Paste,
        ]
    );
}

/// The option menu.
pub mod menu {
    use gpui::actions;

    actions!(
        menu,
        [
            /// `enter` — run the selected row.
            Accept,
            /// `esc` — close.
            Cancel,
            /// `j` / `down` — next row.
            Down,
            /// `k` / `up` — previous row.
            Up,
            /// `/` — filter the rows.
            StartFilter,
        ]
    );
}

/// The keybinding help overlay.
pub mod help {
    use gpui::actions;

    actions!(
        help,
        [
            /// `esc` / `q` / `?` — close.
            Close,
            /// `j` / `down` — scroll down.
            Down,
            /// `k` / `up` — scroll up.
            Up,
        ]
    );
}
