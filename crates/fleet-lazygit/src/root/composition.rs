use super::*;

impl Lazygit {
    /// Recomputes how many whole rows each pane can show, and how wide the side column is.
    ///
    /// gpui gives an element its size during layout, which is too late for a key handler, so the
    /// frame's own arithmetic is repeated here from the window size: the bands and the two fixed
    /// pane heights are constants, and every other side pane is one equal share of what is left.
    /// The numbers feed two things: `ctrl-d` / `ctrl-u` page by half a real viewport, and each
    /// side list is clipped to a whole number of rows so no row is ever half painted.
    pub(super) fn measure(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (status_bar_h, banner_h, header_h, row_h, hairline, status_pane_h, stash_pane_h) = {
            let metrics = &cx.theme().metrics;
            (
                f32::from(metrics.status_bar_h),
                f32::from(metrics.banner_h),
                f32::from(metrics.pane_header_h),
                f32::from(metrics.row_h),
                f32::from(metrics.hairline),
                f32::from(metrics.status_pane_h),
                f32::from(metrics.stash_pane_h),
            )
        };
        let viewport = self
            .pane_size
            .get()
            .filter(|size| size.width > gpui::px(0.0) && size.height > gpui::px(0.0))
            .unwrap_or_else(|| window.viewport_size());
        let banner = if self.state.operation() == OperationState::None {
            0.0
        } else {
            banner_h
        };
        let body_h = (f32::from(viewport.height) - status_bar_h - banner).max(0.0);
        let rows_in = |height: f32, row: f32| {
            if row <= 0.0 {
                return 1;
            }
            (((height - header_h) / row).floor().max(1.0)) as usize
        };

        let normal = self.state.screen_mode == crate::state::ScreenMode::Normal;
        let stash_focused = self.state.focused == PanelId::Stash;
        let (side_h, stash_h) = if normal {
            let fixed =
                status_pane_h + 4.0 * hairline + if stash_focused { 0.0 } else { stash_pane_h };
            let shares = if stash_focused { 4.0 } else { 3.0 };
            let flex = (body_h - fixed).max(0.0) / shares;
            (flex, if stash_focused { flex } else { stash_pane_h })
        } else {
            (body_h, body_h)
        };
        self.rows_side = rows_in(side_h, row_h);
        self.rows_stash = rows_in(stash_h, row_h);

        let main_h = body_h
            - if self.state.show_command_log {
                f32::from(crate::panels::log_band_h(cx))
            } else {
                0.0
            };
        self.rows_main = rows_in(main_h, f32::from(cx.theme().metrics.diff_row_h));

        let ratio = crate::panels::side_ratio(self.state.screen_mode, self.state.focused);
        let column = f32::from(viewport.width) * ratio;
        let one_ch = f32::from(fleet_ui_kit::theme::ch(1.0)).max(1.0);
        self.side_ch = (column / one_ch).floor().max(8.0) as usize;
        // What is left of the window once the side column and the gutters are taken: the
        // horizontal-scroll clamp measures against this.
        self.main_px_w = (f32::from(viewport.width)
            - column
            - crate::views::diff::gutter_width(&self.main_model()))
        .max(80.0);

        // Every list pages by half its own viewport (`ListCursor::set_page_from_visible`).
        for cursor in [
            &mut self.state.cursors.files,
            &mut self.state.cursors.branches,
            &mut self.state.cursors.remotes,
            &mut self.state.cursors.remote_branches,
            &mut self.state.cursors.tags,
            &mut self.state.cursors.commits,
            &mut self.state.cursors.reflog,
        ] {
            cursor.set_page_from_visible(self.rows_side);
        }
        self.state
            .cursors
            .stashes
            .set_page_from_visible(self.rows_stash);
        let main_rows = match self.state.main {
            // The two drill-downs split the pane between a list and a patch.
            MainContent::SubCommits { .. } | MainContent::CommitFiles { .. } => {
                (self.rows_main / 2).max(1)
            }
            _ => self.rows_main,
        };
        self.state.cursors.main.set_page_from_visible(main_rows);
    }

    /// Wraps `child` in one div per key-context word, outermost first.
    pub(super) fn contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
        let mut element = child;
        for context in chain.iter().rev() {
            element = div()
                .size_full()
                .min_h_0()
                .key_context(*context)
                .child(element)
                .into_any_element();
        }
        element
    }

    /// The same nesting as a layer rather than a flex child, so the frame's bands never move.
    pub(super) fn overlay_contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
        let mut element = child;
        for context in chain.iter().rev() {
            element = div()
                .absolute()
                .inset_0()
                .key_context(*context)
                .child(element)
                .into_any_element();
        }
        element
    }

    /// Installs every action listener. They live on the root div, above the focused element, so
    /// gpui's upward dispatch reaches them from any context.
    pub(super) fn with_actions(root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::open_help))
            .on_action(cx.listener(Self::next_screen_mode))
            .on_action(cx.listener(Self::prev_screen_mode))
            .on_action(cx.listener(Self::toggle_command_log))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::focus_status))
            .on_action(cx.listener(Self::focus_files))
            .on_action(cx.listener(Self::focus_branches))
            .on_action(cx.listener(Self::focus_commits))
            .on_action(cx.listener(Self::focus_stash))
            .on_action(cx.listener(Self::focus_main))
            .on_action(cx.listener(Self::next_panel))
            .on_action(cx.listener(Self::prev_panel))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::pull))
            .on_action(cx.listener(Self::push))
            .on_action(cx.listener(Self::fetch))
            .on_action(cx.listener(Self::operation_menu))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::go_top))
            .on_action(cx.listener(Self::go_bottom))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::scroll_left))
            .on_action(cx.listener(Self::scroll_right))
            .on_action(cx.listener(Self::toggle_split))
            .on_action(cx.listener(Self::less_context))
            .on_action(cx.listener(Self::more_context))
            .on_action(cx.listener(Self::toggle_staged))
            .on_action(cx.listener(Self::toggle_staged_all))
            .on_action(cx.listener(Self::discard_file))
            .on_action(cx.listener(Self::commit))
            .on_action(cx.listener(Self::amend))
            .on_action(cx.listener(Self::stash_menu))
            .on_action(cx.listener(Self::toggle_file_tree))
            .on_action(cx.listener(Self::collapse_all_files))
            .on_action(cx.listener(Self::expand_all_files))
            .on_action(cx.listener(Self::enter_file))
            .on_action(cx.listener(Self::checkout_branch))
            .on_action(cx.listener(Self::new_branch))
            .on_action(cx.listener(Self::delete_branch))
            .on_action(cx.listener(Self::rename_branch))
            .on_action(cx.listener(Self::merge_branch))
            .on_action(cx.listener(Self::rebase_branch))
            .on_action(cx.listener(Self::upstream_menu))
            .on_action(cx.listener(Self::tag_branch))
            .on_action(cx.listener(Self::enter_branch))
            .on_action(cx.listener(Self::enter_remote))
            .on_action(cx.listener(Self::checkout_remote))
            .on_action(cx.listener(Self::new_tag))
            .on_action(cx.listener(Self::delete_tag))
            .on_action(cx.listener(Self::checkout_tag))
            .on_action(cx.listener(Self::enter_commit))
            .on_action(cx.listener(Self::checkout_commit))
            .on_action(cx.listener(Self::reword_commit))
            .on_action(cx.listener(Self::squash_commit))
            .on_action(cx.listener(Self::fixup_commit))
            .on_action(cx.listener(Self::drop_commit))
            .on_action(cx.listener(Self::edit_commit))
            .on_action(cx.listener(Self::move_commit_down))
            .on_action(cx.listener(Self::move_commit_up))
            .on_action(cx.listener(Self::reset_menu))
            .on_action(cx.listener(Self::copy_commit))
            .on_action(cx.listener(Self::paste_commits))
            .on_action(cx.listener(Self::revert_commit))
            .on_action(cx.listener(Self::tag_commit))
            .on_action(cx.listener(Self::amend_commit))
            .on_action(cx.listener(Self::branch_from_commit))
            .on_action(cx.listener(Self::stash_apply))
            .on_action(cx.listener(Self::stash_pop))
            .on_action(cx.listener(Self::stash_drop))
            .on_action(cx.listener(Self::stash_branch))
            .on_action(cx.listener(Self::enter_stash))
            .on_action(cx.listener(Self::sub_commit_show_diff))
            .on_action(cx.listener(Self::commit_file_show_patch))
            .on_action(cx.listener(Self::staging_apply))
            .on_action(cx.listener(Self::staging_discard))
            .on_action(cx.listener(Self::staging_toggle_range))
            .on_action(cx.listener(Self::staging_toggle_line_mode))
            .on_action(cx.listener(Self::staging_switch_side))
            .on_action(cx.listener(Self::staging_prev_hunk))
            .on_action(cx.listener(Self::staging_next_hunk))
            .on_action(cx.listener(Self::take_ours))
            .on_action(cx.listener(Self::take_theirs))
            .on_action(cx.listener(Self::take_both))
            .on_action(cx.listener(Self::next_section))
            .on_action(cx.listener(Self::prev_section))
            .on_action(cx.listener(Self::confirm_accept))
            .on_action(cx.listener(Self::confirm_cancel))
            .on_action(cx.listener(Self::prompt_accept))
            .on_action(cx.listener(Self::prompt_submit))
            .on_action(cx.listener(Self::prompt_cancel))
            .on_action(cx.listener(Self::prompt_backspace))
            .on_action(cx.listener(Self::prompt_delete_word))
            .on_action(cx.listener(Self::prompt_delete_to_start))
            .on_action(cx.listener(Self::prompt_left))
            .on_action(cx.listener(Self::prompt_right))
            .on_action(cx.listener(Self::prompt_home))
            .on_action(cx.listener(Self::prompt_end))
            .on_action(cx.listener(Self::prompt_paste))
            .on_action(cx.listener(Self::menu_accept))
            .on_action(cx.listener(Self::menu_cancel))
            .on_action(cx.listener(Self::menu_down))
            .on_action(cx.listener(Self::menu_up))
            .on_action(cx.listener(Self::menu_start_filter))
            .on_action(cx.listener(Self::help_close))
            .on_action(cx.listener(Self::help_down))
            .on_action(cx.listener(Self::help_up))
    }
}

/// The horizontal bands a frame is assembled from, prepared before the frame chooses between
/// the standalone [`AppFrame`] and the embedded pane.
struct Bands {
    banner: Option<AnyElement>,
    body: AnyElement,
    toasts: Option<AnyElement>,
    status_bar: AnyElement,
    overlay: Option<AnyElement>,
}

impl Render for Lazygit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.window_active = window.is_window_active();
        self.measure(window, cx);
        let chain = self.state.context_chain();
        let overlay_open = self.state.overlay().is_some();

        let overlay_element = crate::overlays::render(self, cx).map(|element| {
            Self::overlay_contexts(
                &chain,
                div()
                    .absolute()
                    .inset_0()
                    .track_focus(&self.overlay_focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        this.typed(event, cx);
                    }))
                    .child(element)
                    .into_any_element(),
            )
        });

        let body = self.body(window, cx);
        let body = if overlay_open {
            body
        } else {
            Self::contexts(
                &chain,
                div()
                    .size_full()
                    .min_h_0()
                    .track_focus(&self.focus)
                    .child(body)
                    .into_any_element(),
            )
        };

        let toasts = self.state.visible_toasts();
        let toasts = (!toasts.is_empty()).then(|| ToastStack::new(toasts).into_any_element());

        let bands = Bands {
            banner: self.banner(cx),
            body,
            toasts,
            status_bar: self.status_bar(&chain, cx),
            overlay: overlay_element,
        };

        let frame: AnyElement = if self.embedded {
            self.pane(bands, cx)
        } else {
            let mut frame = AppFrame::new()
                .body(bands.body)
                .status_bar(bands.status_bar);
            if let Some(banner) = bands.banner {
                frame = frame.banner(banner);
            }
            if let Some(overlay) = bands.overlay {
                frame = frame.overlay(overlay);
            }
            if let Some(toasts) = bands.toasts {
                frame = frame.body_overlay(toasts);
            }
            frame.into_any_element()
        };

        Self::with_actions(div().size_full().key_context(ROOT_CONTEXT), cx).child(frame)
    }
}

impl Lazygit {
    /// The embedded frame: the same bands as [`AppFrame`] minus the window chrome.
    ///
    /// There is no context bar and no window background wash, because the host already drew
    /// both; the banner, the body, the overlay layer and lazygit's own one-row key-hint bar
    /// stay, because they are the pane's own UI and not the window's.
    fn pane(&self, bands: Bands, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let size = Rc::clone(&self.pane_size);
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.colors.bg)
            .text_color(theme.colors.text)
            .font_family(theme.font_ui.clone())
            .text_size(theme.text.ui.size)
            .line_height(theme.text.ui.line_height)
            // A click anywhere in the pane hands the keyboard back to it, which is how a
            // mouse-first user leaves a Fleet dialog and lands back in the panels.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    if this.active && !this.owns_keyboard(window) {
                        window.focus(this.wanted_focus(), cx);
                        cx.notify();
                    }
                }),
            )
            .child(
                canvas(
                    move |bounds, _window, _cx| size.set(Some(bounds.size)),
                    |_bounds, (), _window, _cx| {},
                )
                .absolute()
                .size_full(),
            )
            .children(bands.banner.map(|banner| {
                div()
                    .flex()
                    .flex_none()
                    .h(theme.metrics.banner_h)
                    .w_full()
                    .overflow_hidden()
                    .child(banner)
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .size_full()
                            .min_w_0()
                            .min_h_0()
                            .child(bands.body),
                    )
                    .children(bands.toasts),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(theme.metrics.status_bar_h)
                    .w_full()
                    .overflow_hidden()
                    .child(bands.status_bar),
            )
            .children(bands.overlay)
            .into_any_element()
    }
}
