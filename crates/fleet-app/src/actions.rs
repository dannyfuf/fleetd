//! GPUI action names shared by keys, menus, and command surfaces.
//!
//! There is exactly **one action per row of `docs/KEYMAP.md`**, and the keystrokes that reach
//! them live in [`crate::keymap`]; registered names remain stable across refactors. Namespaces
//! mirror the key contexts of the keymap, so the same word (`Open`, `Delete`, `Cancel`) can mean
//! different things in disjoint panes without colliding: gpui registers `namespace::Name`.

pub mod fleet {
    use gpui::actions;

    actions!(
        fleet,
        [
            /// `ctrl-q` — quit the app; the daemon keeps running.
            Quit,
            /// `ctrl-shift-q` — quit the app and stop the daemon.
            QuitAndStopDaemon,
            /// `:` — open the command palette.
            OpenPalette,
            /// `,` — open settings.
            OpenSettings,
            /// `?` — open the help overlay.
            OpenHelp,
            /// `J` — open the jobs panel.
            OpenJobs,
            /// `!` — focus the sticky error slot.
            FocusStickyError,
            /// `r` — refresh status, PRs and discovery as a job.
            Refresh,
            /// `U` — update Fleet as a job.
            UpdateFleet,
            /// `Esc` — clear the filter, else close the topmost overlay, else no-op. Never quits.
            Cancel,
            /// `a` / `ctrl-s a` — toggle the floating Claude agent popup.
            OpenAgentClaude,
            /// `A` / `ctrl-s A` — toggle the floating Codex agent popup.
            OpenAgentCodex,
        ]
    );
}

pub mod hub {
    use gpui::actions;

    actions!(
        hub,
        [
            /// `j` / `↓` — move the cursor down one row.
            MoveDown,
            /// `k` / `↑` — move the cursor up one row.
            MoveUp,
            /// `gg` — jump to the first row.
            GoTop,
            /// `G` — jump to the last row.
            GoBottom,
            /// `ctrl-d` — move down half a page.
            HalfPageDown,
            /// `ctrl-u` — move up half a page.
            HalfPageUp,
            /// `h` / `←` / `S-Tab` — focus the previous pane (repos ⇄ list only).
            FocusPrevPane,
            /// `l` / `→` / `Tab` — focus the next pane (repos ⇄ list only).
            FocusNextPane,
            /// `gr` — go to the repos rail.
            GoRepos,
            /// `gw` — go to the worktrees list.
            GoWorktrees,
            /// `gp` — go to the pull-requests screen.
            GoPrs,
            /// `gj` — go to the jobs panel.
            GoJobs,
            /// `ga` — select the `All` pseudo-repo.
            GoAllRepos,
            /// `gt` — switch to the next context.
            NextContext,
            /// `gT` — switch to the previous context.
            PrevContext,
            /// `1` — switch to the first context.
            SelectContext1,
            /// `2` — switch to the second context.
            SelectContext2,
            /// `3` — switch to the third context.
            SelectContext3,
            /// `4` — switch to the fourth context.
            SelectContext4,
            /// `5` — switch to the fifth context.
            SelectContext5,
            /// `6` — switch to the sixth context.
            SelectContext6,
            /// `7` — switch to the seventh context.
            SelectContext7,
            /// `8` — switch to the eighth context.
            SelectContext8,
            /// `9` — switch to the ninth context.
            SelectContext9,
            /// `p` — toggle worktrees ⇄ pull requests.
            TogglePrScreen,
            /// `/` — filter the current list.
            OpenFilter,
            /// `i` — toggle the detail panel.
            ToggleDetail,
            /// `H` — collapse or expand the repos rail.
            ToggleRepoRail,
            /// `N` — create a context.
            NewContext,
            /// `E` — edit the active context.
            EditContext,
            /// `D` — delete the active context.
            DeleteContext,
            /// `b` — open the selected PR or repo in the browser.
            OpenInBrowser,
        ]
    );
}

pub mod repos {
    use gpui::actions;

    actions!(
        repos,
        [
            /// `Enter` / `o` / `l` — select the repo and focus the worktrees list.
            Open,
            /// `n` — clone a repository.
            Clone,
            /// `d` — delete the repository, cascading to its worktrees.
            Delete,
            /// `x` — dismiss a failed clone row.
            DismissClone,
            /// `e` — edit the repository's `prepare` / `postCreate` hooks.
            EditHooks,
            /// `m` — move the repository to another context.
            MoveToContext,
        ]
    );
}

pub mod worktrees {
    use gpui::actions;

    actions!(
        worktrees,
        [
            /// `Enter` / `o` — open the session, sleeping the previous one.
            Open,
            /// `O` — open the session keeping the previous one awake.
            OpenKeepAwake,
            /// `n` — create a worktree.
            Create,
            /// `d` — delete the worktree.
            Delete,
            /// `u` — undo the last delete while its trash entry survives.
            UndoDelete,
            /// `x` — prune eligible worktrees of the selected repo.
            Prune,
            /// `s` — sleep the session.
            Sleep,
            /// `K` — kill the session.
            Kill,
            /// `I` — inspect, refreshing the safety facts as a job.
            Inspect,
            /// `y` — copy the worktree path.
            CopyPath,
            /// `Y` — copy the branch name.
            CopyBranch,
        ]
    );
}

pub mod prs {
    use gpui::actions;

    actions!(
        prs,
        [
            /// `Tab` / `l` — move to the next PR tab.
            NextTab,
            /// `S-Tab` / `h` — move to the previous PR tab.
            PrevTab,
            /// `Enter` / `o` — open or create the PR worktree.
            Open,
            /// `O` — open or create the PR worktree keeping the previous session awake.
            OpenKeepAwake,
            /// `c` — create the PR worktree without opening it.
            CreateWithoutOpening,
            /// `I` — inspect the local worktree matching the selected PR.
            Inspect,
            /// `y` — copy the PR URL.
            CopyUrl,
            /// `r` — force refresh both PR tabs.
            Refresh,
            /// `p` / `q` — go back to the worktrees screen.
            Back,
        ]
    );
}

pub mod workspace {
    use gpui::actions;

    actions!(
        workspace,
        [
            /// `ctrl-s` — enter the one-shot prefix mode.
            EnterPrefix,
            /// `cmd-c` — copy the current terminal selection, when one exists.
            CopySelection,
            /// `cmd-v` — paste the clipboard through the terminal's mode-aware paste path.
            PasteClipboard,
        ]
    );
}

/// Actions owned by the floating agent terminal.
pub mod agent {
    use gpui::actions;

    actions!(
        agent,
        [
            /// `ctrl-s` — enter the popup's one-shot prefix mode.
            EnterPrefix,
            /// `ctrl-q` / `ctrl-s q` — hide the popup without stopping its session.
            Hide,
            /// `cmd-c` — copy the popup terminal selection, when one exists.
            CopySelection,
            /// `cmd-v` — paste through the terminal's mode-aware paste path.
            PasteClipboard,
        ]
    );
}

/// Actions owned by the native structured agent thread.
pub mod native_agent {
    use gpui::actions;

    actions!(
        native_agent,
        [
            /// `Enter` — send the composer text.
            Send,
            /// `Enter` while a turn runs — a **steer**, dispatched immediately. There is no
            /// queue: both harnesses already fold a second message into the live turn.
            Steer,
            /// `cmd-Enter` on a thread that has not started — start it in the background.
            SendBackground,
            /// `Shift+Tab` — enter plan mode.
            PlanMode,
            /// `/` — open command completion, at line start only.
            Commands,
            /// `$` — open skill completion.
            Skills,
            /// `@` — open file completion.
            Files,
            /// `Up` / `ctrl-p` — the previous completion row, else composer history.
            History,
            /// `Down` / `ctrl-n` — the next completion row, else one caret row down.
            HistoryNext,
            /// `ctrl-s m` — choose the model and the instance it routes to.
            Model,
            /// `ctrl-s e` — the harness-declared traits of the selected model.
            Traits,
            /// `ctrl-s t` — the access ladder.
            AccessMode,
            /// `ctrl-s [` — enter transcript scroll mode.
            Scroll,
            /// `j` in transcript scroll mode — move the viewport down one row.
            ScrollLineDown,
            /// `k` in transcript scroll mode — move the viewport up one row.
            ScrollLineUp,
            /// `ctrl-d` in transcript scroll mode — down half a viewport.
            ScrollHalfPageDown,
            /// `ctrl-u` in transcript scroll mode — up half a viewport.
            ScrollHalfPageUp,
            /// `ctrl-f` in transcript scroll mode — down a viewport.
            ScrollPageDown,
            /// `ctrl-b` in transcript scroll mode — up a viewport.
            ScrollPageUp,
            /// `gg` in transcript scroll mode — the oldest retained row.
            ScrollTop,
            /// `G` in transcript scroll mode — back to the live bottom.
            ScrollBottom,
            /// `q` / `i` in transcript scroll mode — leave it; the viewport snaps to the bottom.
            ScrollExit,
            /// `ctrl-s a` — create a Claude thread.
            NewClaude,
            /// `ctrl-s A` — create a Codex thread.
            NewCodex,
            /// `Esc` — interrupt the active turn.
            Stop,
            /// `y` — allow this invocation once.
            AllowOnce,
            /// `a` — allow matching calls for this session or directory.
            AllowSession,
            /// `n` — deny this invocation.
            Deny,
            /// `e` — edit the protected command.
            EditCommand,
            /// `Esc` on permission — deny and interrupt.
            DenyAndStop,
            /// `1` — choose question option one.
            Choose1,
            /// `2` — choose question option two.
            Choose2,
            /// `3` — choose question option three.
            Choose3,
            /// `4` — choose question option four.
            Choose4,
            /// `5` — choose question option five, the free-text row a question appends.
            Choose5,
            /// `Space` — toggle a multi-select option.
            Toggle,
            /// `Enter` — submit the question, or advance the wizard.
            Answer,
            /// `p` — step back to the previous question of a multi-question request.
            Previous,
            /// `y` — implement the ready plan.
            Implement,
            /// `n` — refine the ready plan, which opens the composer rather than answering.
            Refine,
            /// `Enter` on a focused row — expand or collapse it.
            ExpandRow,
            /// `u` — revert the focused edit, or this turn on a footer.
            Revert,
            /// `o` — open the focused row's file in the editor.
            OpenInEditor,
            /// `y` — copy the focused row's payload.
            CopyRow,
            /// `d` — open the diff of a focused edit row or turn footer.
            DiffRow,
            /// `ctrl-s x` — close the native agent tab.
            CloseTab,
            /// `ctrl-s F` — open the PTY agent session instead (migration fallback).
            TerminalFallback,
        ]
    );
}

pub mod prefix {
    use gpui::actions;

    actions!(
        prefix,
        [
            /// `ctrl-s ctrl-s` — send a literal `ctrl-s` to the terminal.
            SendLiteral,
            /// `ctrl-s s` — go to the Hub; the session keeps running.
            GoHub,
            /// `ctrl-s S` — sleep this session and return to the Hub.
            SleepAndGoHub,
            /// `ctrl-s 1` — switch to terminal tab 1.
            SelectTab1,
            /// `ctrl-s 2` — switch to terminal tab 2.
            SelectTab2,
            /// `ctrl-s 3` — switch to terminal tab 3.
            SelectTab3,
            /// `ctrl-s 4` — switch to terminal tab 4.
            SelectTab4,
            /// `ctrl-s 5` — switch to terminal tab 5.
            SelectTab5,
            /// `ctrl-s 6` — switch to terminal tab 6.
            SelectTab6,
            /// `ctrl-s 7` — switch to terminal tab 7.
            SelectTab7,
            /// `ctrl-s 8` — switch to terminal tab 8.
            SelectTab8,
            /// `ctrl-s 9` — switch to terminal tab 9.
            SelectTab9,
            /// `ctrl-s h` / `ctrl-s p` — previous terminal tab.
            PrevTab,
            /// `ctrl-s l` / `ctrl-s n` — next terminal tab.
            NextTab,
            /// `ctrl-s Tab` — last terminal tab, MRU within this session.
            LastTab,
            /// `ctrl-s w` — last session, the MRU alternate.
            LastSession,
            /// `ctrl-s W` — the session switcher: the palette pre-filtered to sessions.
            SessionSwitcher,
            /// `ctrl-s c` — new terminal tab.
            NewTerminal,
            /// `ctrl-s x` — close the current terminal.
            CloseTerminal,
            /// `ctrl-s r` — restart the exited command in this terminal.
            RestartCommand,
            /// `ctrl-s y` — copy the worktree path of the current session.
            CopyWorktreePath,
            /// `ctrl-s ,` — rename the current terminal.
            RenameTerminal,
            /// `ctrl-s [` — enter scroll mode.
            EnterScroll,
            /// `ctrl-s ]` — paste the clipboard.
            Paste,
            /// `ctrl-s z` — zoom: hide the session header and tab strip.
            ToggleZoom,
            /// `ctrl-s v` — hide/show the read-only watch split.
            ToggleWatchPane,
            /// `ctrl-s N` — select the next watch, wrapping in start order.
            NextWatch,
            /// `ctrl-s P` — select the previous watch, wrapping in start order.
            PrevWatch,
            /// `ctrl-s V` — dismiss a completed watch, or hide a running one.
            DismissWatch,
            /// `ctrl-s Esc` — cancel the prefix.
            Cancel,
        ]
    );
}

pub mod scroll {
    use gpui::actions;

    actions!(
        scroll,
        [
            /// Shift+PageUp in Terminal mode; forwarded on the alternate screen.
            TerminalPageUp,
            /// Shift+PageDown in Terminal mode; forwarded on the alternate screen.
            TerminalPageDown,
            /// Cmd+Home in Terminal mode; forwarded on the alternate screen.
            TerminalTop,
            /// Cmd+End in Terminal mode; forwarded on the alternate screen.
            TerminalBottom,
            /// `j` — move the viewport down one line.
            LineDown,
            /// `k` — move the viewport up one line.
            LineUp,
            /// `ctrl-d` — move the viewport down half a page.
            HalfPageDown,
            /// `ctrl-u` — move the viewport up half a page.
            HalfPageUp,
            /// `ctrl-f` — move the viewport down a page.
            PageDown,
            /// `ctrl-b` — move the viewport up a page.
            PageUp,
            /// `gg` — jump to the oldest retained row.
            Top,
            /// `G` — jump back to the live bottom.
            Bottom,
            /// `v` — start a selection.
            StartSelection,
            /// `y` — yank the selection.
            Yank,
            /// `/` — search the scrollback.
            Search,
            /// `n` — next scrollback match.
            SearchNext,
            /// `N` — previous scrollback match.
            SearchPrev,
            /// `q` / `i` — leave scroll mode; the viewport snaps to the bottom.
            Exit,
            /// `Esc` — clear the selection, else leave scroll mode.
            Escape,
        ]
    );
}

pub mod filter {
    use gpui::actions;

    actions!(
        filter,
        [
            /// `Enter` — open the highlighted row from inside the input.
            Accept,
            /// `Esc` — first press keeps the filter, second clears it. Never quits.
            Escape,
            /// `ctrl-n` / `↓` — move the list cursor down while still typing.
            CursorDown,
            /// `ctrl-p` / `↑` — move the list cursor up while still typing.
            CursorUp,
            /// `Backspace` — delete the character before the caret.
            Backspace,
            /// `ctrl-w` — delete the word before the caret.
            DeleteWord,
            /// `ctrl-u` — clear the query.
            Clear,
        ]
    );
}

pub mod palette {
    use gpui::actions;

    actions!(
        palette,
        [
            /// `Enter` — run the highlighted row.
            Run,
            /// `Esc` — close the palette.
            Close,
            /// `ctrl-n` / `↓` — move to the next row.
            CursorDown,
            /// `ctrl-p` / `↑` — move to the previous row.
            CursorUp,
            /// `Backspace` — delete the character before the caret.
            Backspace,
            /// `ctrl-w` — delete the word before the caret.
            DeleteWord,
            /// `ctrl-u` — clear the query.
            Clear,
        ]
    );
}

pub mod jobs {
    use gpui::actions;

    actions!(
        jobs,
        [
            /// `J` / `Esc` — close the panel, restoring the exact prior focus.
            Close,
            /// `Esc` with an expanded log — collapse it without closing Jobs.
            CollapseLog,
            /// `j` — move the job cursor down, or scroll an expanded log.
            MoveDown,
            /// `k` — move the job cursor up, or scroll an expanded log.
            MoveUp,
            /// `gg` — first job.
            Top,
            /// `G` — last job, and re-enable follow inside an expanded log.
            Bottom,
            /// `Enter` — expand or collapse the log of the selected job.
            ToggleLog,
            /// `c` — cancel the selected job when it is cancellable.
            CancelJob,
            /// `X` — cancel every cancellable job.
            CancelAll,
            /// `R` — retry a failed job with identical parameters.
            Retry,
            /// `y` — copy the log path of the selected job.
            CopyLogPath,
            /// `D` — dismiss finished and failed jobs.
            DismissFinished,
            /// `f` — cycle the filter all → running → failed, or toggle follow in an expanded log.
            CycleFilter,
        ]
    );
}

pub mod dialog {
    use gpui::actions;

    actions!(
        dialog,
        [
            /// `Enter` — confirm the dialog.
            Confirm,
            /// `Esc` — cancel the dialog.
            Cancel,
            /// `Tab` — move to the next field.
            NextField,
            /// `S-Tab` — move to the previous field.
            PrevField,
            /// `ctrl-n` / `↓` — move the list selection down.
            CursorDown,
            /// `ctrl-p` / `↑` — move the list selection up.
            CursorUp,
            /// `Backspace` — delete the character before the caret.
            Backspace,
            /// `ctrl-w` — delete the word before the caret.
            DeleteWord,
            /// `ctrl-u` — clear the focused text input.
            ClearInput,
            /// `ctrl-a` — move the caret to the start of the input.
            LineStart,
            /// `ctrl-e` — move the caret to the end of the input.
            LineEnd,
            /// `←` — move the caret left.
            CursorLeft,
            /// `→` — move the caret right.
            CursorRight,
        ]
    );
}

pub mod confirm {
    use gpui::actions;

    actions!(
        confirm,
        [
            /// `y` / `Enter` — confirm when every decisive fact is known.
            Accept,
            /// `Y` — confirm when a decisive fact is unknown, and for repo / context delete.
            AcceptStrong,
            /// `n` / `q` — cancel.
            Reject,
            /// `I` — re-check the safety facts.
            Recheck,
            /// `s` — toggle the KEEP list of a prune preview.
            ToggleKeep,
        ]
    );
}

pub mod create_worktree {
    use gpui::actions;

    actions!(
        create_worktree,
        [
            /// `←` — cycle the host backwards.
            HostPrev,
            /// `→` — cycle the host forwards.
            HostNext,
            /// `alt-Enter` — create the worktree without opening it.
            CreateWithoutOpening,
        ]
    );
}

pub mod context_dialog {
    use gpui::actions;

    actions!(
        context_dialog,
        [
            /// `ctrl-d` — delete this context, routed through the expanded `Y` confirm.
            Delete,
        ]
    );
}

pub mod settings {
    use gpui::actions;

    actions!(
        settings,
        [
            /// `Space` — toggle the selected switch.
            Toggle,
            /// `h` / `←` — cycle the selected choice backwards.
            CyclePrev,
            /// `l` / `→` — cycle the selected choice forwards.
            CycleNext,
            /// `j` — move down one setting.
            MoveDown,
            /// `k` — move up one setting.
            MoveUp,
            /// `E` — open `config.json` in a new terminal tab.
            OpenConfigFile,
            /// `D` — run doctor.
            RunDoctor,
        ]
    );
}

pub mod quit_dialog {
    use gpui::actions;

    actions!(
        quit_dialog,
        [
            /// `y` — quit; the listed work keeps running in fleetd.
            Accept,
            /// `n` — cancel.
            Reject,
            /// `J` — open the jobs panel instead of quitting.
            OpenJobs,
            /// `W` — never warn again (`jobs.warnBeforeQuit = false`) and quit.
            NeverWarn,
        ]
    );
}

pub mod quit_daemon_dialog {
    use gpui::actions;

    actions!(
        quit_daemon_dialog,
        [
            /// `Y` — stop fleetd and quit.
            Accept,
            /// `n` — cancel.
            Reject,
        ]
    );
}

pub mod help {
    use gpui::actions;

    actions!(
        help,
        [
            /// `?` / `Esc` — close the help overlay.
            Close,
        ]
    );
}

pub mod daemon {
    use gpui::actions;

    actions!(
        daemon,
        [
            /// `r` — retry starting fleetd (case B).
            Retry,
            /// `r` — reconnect now (case C).
            Reconnect,
            /// `L` / `l` — open `~/.fleet/logs/fleetd.log`.
            OpenLog,
            /// `D` — run doctor.
            RunDoctor,
            /// `Esc` — dismiss the banner; the daemon dot stays red.
            DismissBanner,
        ]
    );
}

pub mod first_run {
    use gpui::actions;

    actions!(
        first_run,
        [
            /// `i` — run `fleet import --from-swarm` as a job.
            Import,
        ]
    );
}

/// Actions for the board surface (BOARD §8).
pub mod board {
    use gpui::actions;
    actions!(
        board,
        [
            /// Go to board.
            GoBoard,
            /// Previous column.
            PrevColumn,
            /// Next column.
            NextColumn,
            /// Next card.
            NextCard,
            /// Previous card.
            PrevCard,
            /// Open card.
            OpenCard,
            /// New card.
            NewCard,
            /// `ctrl-enter` in the New card dialog: create it and open its detail.
            CreateAndOpen,
            /// Status picker.
            PickStatus,
            /// Priority picker.
            PickPriority,
            /// Assignee picker.
            PickAssignee,
            /// Labels picker.
            PickLabels,
            /// Estimate picker.
            PickEstimate,
            /// Move card to previous column.
            MovePrevColumn,
            /// Move card to next column.
            MoveNextColumn,
            /// Create worktree from card.
            CreateWorktree,
            /// Open linked worktree.
            OpenWorktree,
            /// Sync.
            Sync,
            /// Full sync.
            FullSync,
            /// Open remote issue.
            OpenRemote,
            /// Delete card.
            DeleteCard,
            /// Settings.
            Settings,
            /// Reload.
            Reload,
            /// Filter cards.
            Filter,
        ]
    );
}

/// Actions for the card detail surface (BOARD §8).
pub mod card_detail {
    use gpui::actions;
    actions!(
        card_detail,
        [
            /// Close.
            Close,
            /// Edit title.
            EditTitle,
            /// Edit description.
            EditDescription,
            /// Add comment.
            AddComment,
            /// Next property.
            NextProperty,
            /// Previous property.
            PrevProperty,
            /// Edit selected property.
            EditProperty,
            /// Create worktree.
            CreateWorktree,
            /// Open remote issue.
            OpenRemote,
            /// Resolve conflict: keep local.
            KeepLocal,
            /// Resolve conflict: take remote.
            TakeRemote,
            /// Save text edit.
            Save,
        ]
    );
}
