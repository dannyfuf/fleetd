use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// Preparation belongs to the coordinating entity; renderers only borrow completed models.
/// Retaining the source prevents allocation-address reuse while its key is cached.
pub(super) struct DiffView {
    source: Arc<fleet_git::Diff>,
    key: ModelKey,
    style: crate::views::long_line::Style,
    model: Option<Rc<DiffModel>>,
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for DiffView {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// Flattens one patch into rows, shapes the lines too long to shape during paint, and collects
/// the syntax jobs the model will need.
///
/// Returns `None` as soon as `cancelled` is set: a superseded pass stops inside the row loop
/// rather than running to completion and being thrown away, which is what dropping the task
/// alone would do.
fn build_model(
    slot: &'static str,
    source: &fleet_git::Diff,
    mode: DiffViewMode,
    style: &crate::views::long_line::Style,
    text_system: Arc<gpui::TextSystem>,
    cancelled: &AtomicBool,
) -> Option<(DiffModel, Vec<crate::views::syntax::Job>)> {
    let started = Instant::now();
    let mut model = DiffModel::build_cancellable(source, mode, cancelled)?;
    let text_system = gpui::WindowTextSystem::new(text_system);
    for (index, row) in model.rows.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        if row.line.is_some() && row.text.len() > crate::views::long_line::SHAPE_LIMIT {
            let line = crate::views::long_line::LongLine::prepare(
                row.text.clone(),
                row.kind,
                style,
                &text_system,
                cancelled,
            )?;
            model.long_lines.insert(index, Arc::new(line));
        }
    }
    let jobs = model.syntax_jobs();
    tracing::debug!(
        slot,
        rows = model.rows.len(),
        elapsed_us = started.elapsed().as_micros(),
        "diff model prepared"
    );
    Some((model, jobs))
}

impl Lazygit {
    pub(super) fn prepare_models(&mut self, cx: &mut Context<Self>) {
        self.prepare_conflict(cx);
        let caret = match self.state.overlay() {
            Some(Overlay::Prompt(prompt)) if prompt.buffer.is_multiline() => {
                let (_, line, column) = prompt.buffer.lines_with_caret();
                Some((line, column))
            }
            _ => None,
        };
        if self.editor_caret != caret {
            self.editor_caret = caret;
            if let Some((line, _)) = caret {
                self.scroll_editor
                    .scroll_to_item(line, gpui::ScrollStrategy::Top);
            }
        }
        let style = crate::views::long_line::Style::new(cx.theme());
        let visible = self.active
            && crate::panels::side_ratio(self.state.screen_mode, self.state.focused) < 1.0;
        let main = self.main_diff();
        let secondary = match &self.state.main {
            MainContent::FileDiff {
                unstaged, staged, ..
            } => match self.state.staging.as_ref().map(|staging| staging.side) {
                Some(DiffSide::Staged) => unstaged.clone(),
                Some(DiffSide::Unstaged) => staged.clone(),
                None if unstaged.as_ref().is_some_and(|diff| !diff.files.is_empty()) => {
                    staged.clone()
                }
                None => None,
            },
            _ => None,
        };
        let patch = match &self.state.main {
            MainContent::SubCommits { diff, .. } | MainContent::CommitFiles { diff, .. } => {
                diff.clone()
            }
            _ => None,
        };
        for (slot, source, mode) in [
            (SLOT_MAIN, main, self.main_mode()),
            (SLOT_SECONDARY, secondary, DiffViewMode::Unified),
            (SLOT_PATCH, patch, self.state.diff_mode),
        ] {
            let Some(source) = source.filter(|_| visible) else {
                self.models.remove(slot);
                continue;
            };
            let key = ModelKey::new(&source, mode);
            if self
                .models
                .get(slot)
                .is_some_and(|view| view.key == key && view.style == style)
            {
                continue;
            }
            self.models.remove(slot);
            let cancelled = Arc::new(AtomicBool::new(false));
            let cancellation = cancelled.clone();
            let input = source.clone();
            let text_system = cx.text_system().clone();
            let text_style = style.clone();
            let task = cx.spawn(async move |this, cx| {
                let build_cancel = cancellation.clone();
                let prepared = cx
                    .background_spawn(async move {
                        build_model(slot, &input, mode, &text_style, text_system, &build_cancel)
                    })
                    .await;
                let Some((model, jobs)) = prepared else {
                    return;
                };
                if cancellation.load(Ordering::Relaxed) {
                    return;
                }
                let model = Rc::new(model);
                let target = model.clone();
                let installed = this
                    .update(cx, |this, cx| {
                        let Some(view) = this.models.get_mut(slot).filter(|view| view.key == key)
                        else {
                            return false;
                        };
                        view.model = Some(model);
                        this.sync_main_len();
                        this.snap_to_change();
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !installed {
                    return;
                }
                let syntax_cancel = cancellation.clone();
                let runs = cx
                    .background_spawn(async move {
                        crate::views::syntax::run_cancellable(&jobs, &syntax_cancel)
                    })
                    .await;
                if cancellation.load(Ordering::Relaxed) {
                    return;
                }
                target.apply_syntax(runs);
                let _ = this.update(cx, |this, cx| {
                    if this.active && this.models.get(slot).is_some_and(|view| view.key == key) {
                        cx.notify();
                    }
                });
            });
            self.models.insert(
                slot,
                DiffView {
                    source,
                    key,
                    style: style.clone(),
                    model: None,
                    cancelled,
                    _task: task,
                },
            );
        }
    }

    pub(crate) fn slot_model(
        &self,
        slot: &'static str,
        diff: Option<&Arc<fleet_git::Diff>>,
        mode: DiffViewMode,
    ) -> Rc<DiffModel> {
        if let Some(diff) = diff
            && let Some(view) = self.models.get(slot)
            && Arc::ptr_eq(diff, &view.source)
            && view.key == ModelKey::new(diff, mode)
            && let Some(model) = &view.model
        {
            return model.clone();
        }
        self.empty_models[usize::from(mode == DiffViewMode::Split)].clone()
    }

    pub(crate) fn main_model(&self) -> Rc<DiffModel> {
        self.slot_model(SLOT_MAIN, self.main_diff().as_ref(), self.main_mode())
    }

    pub(super) fn main_model_pending(&self) -> bool {
        self.main_diff().is_some_and(|source| {
            self.models.get(SLOT_MAIN).is_none_or(|view| {
                view.key != ModelKey::new(&source, self.main_mode()) || view.model.is_none()
            })
        })
    }
}

impl Lazygit {
    /// Never leaves staging mode looking at an empty side.
    ///
    /// Staging the last hunk of the unstaged half empties it; lazygit moves the selection to the
    /// other half rather than showing "no changes" over a file that still has staged work.
    pub(super) fn retarget_staging_side(&mut self) {
        let Some(side) = self.state.staging.as_ref().map(|staging| staging.side) else {
            return;
        };
        if self.main_diff().is_some_and(|diff| !diff.files.is_empty()) {
            return;
        }
        let flipped = match side {
            DiffSide::Unstaged => DiffSide::Staged,
            DiffSide::Staged => DiffSide::Unstaged,
        };
        if let Some(staging) = &mut self.state.staging {
            staging.side = flipped;
            staging.cursor = 0;
            staging.anchor = None;
            staging.snapped = false;
        }
        // Both halves empty: the file has no changes left, so keep the side the user chose and
        // let the next snapshot move the Files cursor off it.
        if self.main_diff().is_none_or(|diff| diff.files.is_empty())
            && let Some(staging) = &mut self.state.staging
        {
            staging.side = side;
        }
    }

    /// Keeps the main panel's cursor length in step with the list it renders.
    pub(super) fn sync_main_len(&mut self) {
        if self.main_model_pending() {
            return;
        }
        let rows = self.main_len();
        self.state.cursors.main.set_len(rows);
        if let Some(staging) = &mut self.state.staging {
            staging.cursor = staging.cursor.min(rows.saturating_sub(1));
        }
    }

    /// How many items the main panel's cursor moves over.
    ///
    /// Usually one per diff row, but the two drill-down views put a list there instead: the
    /// sub-commits view one row per commit, the commit-files view a header row plus one per file.
    pub(crate) fn main_len(&self) -> usize {
        match &self.state.main {
            MainContent::SubCommits { commits, .. } => commits.len(),
            MainContent::CommitFiles { files, .. } => files.len() + 1,
            _ => self.main_model().len(),
        }
    }

    /// The patch the main panel's primary list is showing, if any.
    pub(crate) fn main_diff(&self) -> Option<Arc<fleet_git::Diff>> {
        match &self.state.main {
            MainContent::FileDiff {
                unstaged, staged, ..
            } => match self.state.staging.as_ref().map(|staging| staging.side) {
                Some(DiffSide::Staged) => staged.clone(),
                Some(DiffSide::Unstaged) => unstaged.clone(),
                None => match unstaged {
                    Some(diff) if !diff.files.is_empty() => unstaged.clone(),
                    _ => staged.clone(),
                },
            },
            MainContent::CommitDiff { diff, .. }
            | MainContent::BranchDiff { diff, .. }
            | MainContent::StashDiff { diff, .. } => diff.clone(),
            // The drill-downs cursor over their list, not over the patch they render below it.
            _ => None,
        }
    }

    /// Which layout the main panel's primary list uses.
    ///
    /// Staging always renders unified: `Staging::cursor` is an index into the *unified* rows,
    /// which is also what `state::hunk_range` and `state::selection_hunks` key on, so a split
    /// layout there would silently stage the wrong lines.
    pub(crate) fn main_mode(&self) -> DiffViewMode {
        if self.state.staging.is_some() {
            DiffViewMode::Unified
        } else {
            self.state.diff_mode
        }
    }
}
