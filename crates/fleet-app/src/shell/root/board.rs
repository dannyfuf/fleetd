use super::Shell;
use crate::actions::{board, card_detail};
use gpui::{Context, Window};

impl Shell {
    pub(super) fn board_go_board(
        &mut self,
        _: &board::GoBoard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::go_board(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_prev_column(
        &mut self,
        _: &board::PrevColumn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::prev_column(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_next_column(
        &mut self,
        _: &board::NextColumn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::next_column(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_next_card(
        &mut self,
        _: &board::NextCard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::next_card(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_prev_card(
        &mut self,
        _: &board::PrevCard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::prev_card(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_open_card(
        &mut self,
        _: &board::OpenCard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::open_card(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_new_card(
        &mut self,
        _: &board::NewCard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::new_card(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_pick_status(
        &mut self,
        _: &board::PickStatus,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::pick_status(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_pick_priority(
        &mut self,
        _: &board::PickPriority,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::pick_priority(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_pick_assignee(
        &mut self,
        _: &board::PickAssignee,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::pick_assignee(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_pick_labels(
        &mut self,
        _: &board::PickLabels,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::pick_labels(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_pick_estimate(
        &mut self,
        _: &board::PickEstimate,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::pick_estimate(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_move_prev_column(
        &mut self,
        _: &board::MovePrevColumn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::move_prev_column(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_move_next_column(
        &mut self,
        _: &board::MoveNextColumn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::move_next_column(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_create_worktree(
        &mut self,
        _: &board::CreateWorktree,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::create_worktree(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_open_worktree(
        &mut self,
        _: &board::OpenWorktree,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::open_worktree(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_sync(&mut self, _: &board::Sync, _: &mut Window, cx: &mut Context<Self>) {
        crate::screens::board::sync(&self.state, &self.bridge, false, cx);
    }

    pub(super) fn board_full_sync(
        &mut self,
        _: &board::FullSync,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::sync(&self.state, &self.bridge, true, cx);
    }

    pub(super) fn board_open_remote(
        &mut self,
        _: &board::OpenRemote,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::open_remote(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_delete_card(
        &mut self,
        _: &board::DeleteCard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::delete_card(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_settings(
        &mut self,
        _: &board::Settings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::settings(&self.state, cx);
    }

    pub(super) fn board_reload(
        &mut self,
        _: &board::Reload,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::reload(&self.state, &self.bridge, cx);
    }

    pub(super) fn board_filter(
        &mut self,
        _: &board::Filter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::filter(&self.state, &self.bridge, cx);
        self.board.focus_filter(window, cx);
    }

    /// `A` — attach the focused card's run as an ordinary agent tab (contracts §5.5).
    pub(super) fn board_attach_run(
        &mut self,
        _: &board::AttachRun,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::attach_run(&self.state, &self.bridge, cx);
    }

    /// `X` — cancel the focused card's live run, or drop the slot it waits for.
    pub(super) fn board_cancel_run(
        &mut self,
        _: &board::CancelRun,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::cancel_run(&self.state, &self.bridge, cx);
    }

    /// `>` — ask the focused card's column to run its action now.
    pub(super) fn board_run_now(
        &mut self,
        _: &board::RunNow,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::run_now(&self.state, &self.bridge, cx);
    }

    /// `b` — the cards the focused card waits for.
    pub(super) fn board_pick_blocked_by(
        &mut self,
        _: &board::PickBlockedBy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::pick_blocked_by(&self.state, &self.bridge, cx);
    }

    /// `m` — which agent runs the focused card.
    pub(super) fn board_pick_agent(
        &mut self,
        _: &board::PickAgent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::pick_agent(&self.state, &self.bridge, cx);
    }

    /// `C` — board settings, opened on its Columns section (contracts §5.4).
    pub(super) fn board_columns(
        &mut self,
        _: &board::Columns,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Through the same guards `,` uses: an unreachable daemon flashes the banner and a
        // board that is not loaded says so, and neither remembers Columns as the section the
        // next `,` should open on.
        crate::screens::board::settings(&self.state, cx);
        if self.state.read(cx).overlay
            == Some(crate::state::Overlay::Dialog(
                crate::dialogs::Dialogs::BoardSettings,
            ))
        {
            crate::dialogs::open_board_section(
                &self.state,
                crate::dialogs::BoardSection::Columns,
                cx,
            );
        }
    }

    /// `T` — board settings, opened on its Schedules section (BOARD §11.8).
    pub(super) fn board_schedules(
        &mut self,
        _: &board::Schedules,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The same guards `C` goes through; the section switch only lands on an open dialog.
        crate::screens::board::settings(&self.state, cx);
        if self.state.read(cx).overlay
            == Some(crate::state::Overlay::Dialog(
                crate::dialogs::Dialogs::BoardSettings,
            ))
        {
            crate::dialogs::open_schedules_section(&self.state, false, cx);
        }
    }

    /// `R` — run every enabled schedule of the shown board now.
    pub(super) fn board_run_schedules(
        &mut self,
        _: &board::RunSchedules,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::run_schedules(&self.state, &self.bridge, cx);
    }

    /// `B` — open the review card's pull request in the browser.
    pub(super) fn board_open_pull_request(
        &mut self,
        _: &board::OpenPullRequest,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::open_pull_request(&self.state, &self.bridge, cx);
    }

    /// `y` — copy the review card's pull request URL.
    pub(super) fn board_copy_pull_request_url(
        &mut self,
        _: &board::CopyPullRequestUrl,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::screens::board::copy_pull_request_url(&self.state, &self.bridge, cx);
    }

    pub(super) fn card_detail_close(
        &mut self,
        _: &card_detail::Close,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if crate::dialogs::card_detail::close(&self.state, &self.bridge, cx) {
            window.focus(&self.overlay_focus, cx);
        }
    }

    pub(super) fn card_detail_edit_title(
        &mut self,
        _: &card_detail::EditTitle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::edit_title(&self.state, &self.bridge, window, cx);
    }

    pub(super) fn card_detail_edit_description(
        &mut self,
        _: &card_detail::EditDescription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::edit_description(&self.state, &self.bridge, window, cx);
    }

    pub(super) fn card_detail_add_comment(
        &mut self,
        _: &card_detail::AddComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::add_comment(&self.state, &self.bridge, window, cx);
    }

    pub(super) fn card_detail_next_property(
        &mut self,
        _: &card_detail::NextProperty,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::next_property(&self.state, &self.bridge, cx);
    }

    pub(super) fn card_detail_prev_property(
        &mut self,
        _: &card_detail::PrevProperty,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::prev_property(&self.state, &self.bridge, cx);
    }

    pub(super) fn card_detail_edit_property(
        &mut self,
        _: &card_detail::EditProperty,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::edit_property(&self.state, &self.bridge, cx);
    }

    pub(super) fn card_detail_create_worktree(
        &mut self,
        _: &card_detail::CreateWorktree,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::create_worktree(&self.state, &self.bridge, cx);
    }

    pub(super) fn card_detail_open_remote(
        &mut self,
        _: &card_detail::OpenRemote,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::open_remote(&self.state, &self.bridge, cx);
    }

    pub(super) fn card_detail_keep_local(
        &mut self,
        _: &card_detail::KeepLocal,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::keep_local(&self.state, &self.bridge, cx);
    }

    pub(super) fn card_detail_take_remote(
        &mut self,
        _: &card_detail::TakeRemote,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::take_remote(&self.state, &self.bridge, cx);
    }

    pub(super) fn card_detail_save(
        &mut self,
        _: &card_detail::Save,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::dialogs::card_detail::save(&self.state, &self.bridge, cx);
    }
}
