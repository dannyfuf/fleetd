//! Reusable inline unified-diff entity for embedders such as native-agent tool rows.
//!
//! The surface is domain-neutral: it takes unified-diff *text* — a `fleet_core::agents::ToolDiff`
//! carries exactly that — rather than a `fleet_git::Diff`, so an embedder needs no git plumbing.
//! Everything below the text is the ADR 0005 stack the lazygit panels already run on: the patch
//! is parsed and flattened into uniform rows on the background executor, `syntect` colours the
//! payload in a second background pass, `similar` marks the words that changed, and the rows
//! themselves come from [`crate::views::row_layout`], so an inline diff has the same geometry as
//! a full-window one. Only the wash differs: inline rows use the semantic `diff_added` /
//! `diff_removed` tokens.
//!
//! Two things the panels do not need:
//!
//! - **A content-keyed cache.** A transcript re-renders constantly and the same edit is drawn
//!   under the same tool row every time, so prepared models are memoised by a hash of their
//!   unified text and shared between views. The panels key on the source `Arc`'s address, which
//!   an embedder holding only a string cannot reproduce.
//! - **A row cap.** A tool row is one item in a scrolling transcript, not a pane, so a
//!   thousand-line rewrite folds to [`MAX_ROWS`] with a "show all" affordance instead of
//!   flooding the thread.
//!
//! The action row (`[u] revert this edit · [o] open in nvim`) is *not* drawn here: reverting and
//! opening an editor are the embedder's verbs, so it hands them in through [`DiffView::set_actions`].

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, Context, MouseButton, SharedString, Task, Window, div};

use crate::views::diff_model::{DiffModel, DiffViewMode, RowKind};
use crate::views::row_layout::{Gutters, RowPalette, RowStyle, line_row};
use crate::views::{diff, long_line, syntax};

/// How many rows the inline view draws before it folds the rest behind "show all".
pub const MAX_ROWS: usize = 400;

/// How many prepared models the content cache retains, per thread.
///
/// A transcript shows a handful of diffs at a time; the cache exists to survive re-renders and
/// tab switches, not to hold a session's history.
const CACHE_LIMIT: usize = 16;

/// The path a headerless patch is attributed to, so it still parses.
const UNTITLED: &str = "file";

/// The caller-rendered row under the diff, e.g. `[u] revert this edit · [o] open in nvim`.
///
/// It is a builder rather than an element because an element cannot be retained across frames.
pub type DiffActions = Rc<dyn Fn(&App) -> AnyElement>;

thread_local! {
    /// Prepared models, most recently used first. `DiffModel` is `!Sync` by design (its syntax
    /// runs are filled in through a `RefCell` after the model is installed), so the cache is
    /// per-thread — which is exactly the foreground thread every view renders on.
    static CACHE: RefCell<Vec<(u64, Rc<DiffModel>)>> = const { RefCell::new(Vec::new()) };
}

/// The cache key for one patch: a hash of the text that produced the model.
#[must_use]
fn cache_key(unified: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    unified.hash(&mut hasher);
    hasher.finish()
}

/// The cached model for `key`, promoted to the front of the cache.
#[must_use]
fn cached(key: u64) -> Option<Rc<DiffModel>> {
    CACHE.with_borrow_mut(|cache| {
        let at = cache.iter().position(|(cached, _)| *cached == key)?;
        let entry = cache.remove(at);
        let model = entry.1.clone();
        cache.insert(0, entry);
        Some(model)
    })
}

/// Retains a prepared model, evicting the least recently used one over [`CACHE_LIMIT`].
fn retain(key: u64, model: &Rc<DiffModel>) {
    CACHE.with_borrow_mut(|cache| {
        cache.retain(|(cached, _)| *cached != key);
        cache.insert(0, (key, model.clone()));
        cache.truncate(CACHE_LIMIT);
    });
}

/// What one background preparation produced.
enum Prepared {
    /// A flattened model and the syntax jobs it still needs.
    Ready(Box<DiffModel>, Vec<syntax::Job>),
    /// The text is not a patch this parser accepts.
    Failed(String),
    /// The view moved on before the pass finished.
    Cancelled,
}

/// Splits a `start[,count]` range field, ignoring the count the patch claims.
#[must_use]
fn range_start(field: &str, sign: char) -> Option<u32> {
    let field = field.strip_prefix(sign)?;
    let start = field.split(',').next()?;
    start.parse().ok()
}

/// The two starts and the trailing function context of an `@@` header.
#[must_use]
fn hunk_header(line: &str) -> Option<(u32, u32, String)> {
    let body = line.strip_prefix("@@ ")?;
    let (ranges, rest) = body.split_once(" @@")?;
    let mut fields = ranges.split_whitespace();
    let old = range_start(fields.next()?, '-')?;
    let new = range_start(fields.next()?, '+')?;
    Some((old, new, rest.to_owned()))
}

/// Whether a line is part of a hunk body rather than the start of the next section.
#[must_use]
fn is_body(line: &str) -> bool {
    line.is_empty() || line.starts_with([' ', '+', '-', '\\'])
}

/// The path a `---` / `+++` header names, with `a/` or `b/` and any trailing tab field removed.
#[must_use]
fn header_path(line: &str) -> Option<&str> {
    let path = line.split('\t').next()?.trim_end();
    if path == "/dev/null" {
        return None;
    }
    Some(
        path.strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(path),
    )
}

/// Rewrites a provider's patch into one `fleet_git`'s parser accepts.
///
/// Two repairs, both of which real harness output needs:
///
/// - **A missing `diff --git` line.** The parser frames files on it, so a bare `--- / +++ / @@`
///   patch — which is what a tool result usually carries — would otherwise yield no files at all.
/// - **Wrong `@@` line counts.** The parser trusts the counts to frame the hunk body and rejects
///   the patch when they disagree with it, so the counts are recomputed from the body that is
///   actually there. A body line stripped of its leading space by a careless producer is restored
///   to a context line for the same reason.
#[must_use]
fn normalize(unified: &str, path: Option<&str>) -> String {
    let lines: Vec<&str> = unified.lines().collect();
    let framed = lines.iter().any(|line| line.starts_with("diff --git "));
    let old = lines
        .iter()
        .find_map(|line| line.strip_prefix("--- "))
        .and_then(header_path);
    let new = lines
        .iter()
        .find_map(|line| line.strip_prefix("+++ "))
        .and_then(header_path);
    let named = old.or(new).or(path).unwrap_or(UNTITLED);

    let mut out = String::with_capacity(unified.len() + 128);
    if !framed {
        out.push_str(&format!(
            "diff --git a/{} b/{}\n",
            old.unwrap_or(named),
            new.unwrap_or(named)
        ));
        // Headerless patches lose the added/deleted distinction; naming both sides keeps the
        // file `Modified`, which is the honest answer when the patch itself does not say.
        if old.is_none() && new.is_none() {
            out.push_str(&format!("--- a/{named}\n+++ b/{named}\n"));
        }
    }

    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let Some((old_start, new_start, context)) = hunk_header(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let mut body: Vec<&str> = Vec::new();
        while index < lines.len() && is_body(lines[index]) {
            body.push(lines[index]);
            index += 1;
        }
        let count = |signs: [char; 2]| {
            body.iter()
                .filter(|line| line.is_empty() || line.starts_with(signs))
                .count()
        };
        out.push_str(&format!(
            "@@ -{old_start},{} +{new_start},{} @@{context}\n",
            count([' ', '-']),
            count([' ', '+'])
        ));
        for line in body {
            out.push_str(if line.is_empty() { " " } else { line });
            out.push('\n');
        }
    }
    out
}

/// Normalizes, parses and flattens one patch. `Ok(None)` means the pass was cancelled.
///
/// Malformed text is reported rather than swallowed: a provider that emits something this
/// parser rejects is a bug worth seeing in the transcript, not an empty box.
fn parse_model(
    unified: &str,
    path: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Option<DiffModel>, String> {
    let patch = normalize(unified, path);
    let source =
        fleet_git::parse::diff::parse(patch.as_bytes()).map_err(|error| error.to_string())?;
    Ok(DiffModel::build_cancellable(
        &source,
        DiffViewMode::Unified,
        cancelled,
    ))
}

/// Parses and flattens one patch, then shapes the lines too long to shape during paint.
///
/// Mirrors `crate::root::diff_view`'s preparation, minus the panels' three slots: the same
/// cancellation discipline, the same long-line pre-shaping, the same syntax jobs.
fn prepare(
    unified: &str,
    path: Option<&str>,
    style: &long_line::Style,
    text_system: Arc<gpui::TextSystem>,
    cancelled: &AtomicBool,
) -> Prepared {
    let mut model = match parse_model(unified, path, cancelled) {
        Ok(Some(model)) => model,
        Ok(None) => return Prepared::Cancelled,
        Err(message) => return Prepared::Failed(message),
    };
    let text_system = gpui::WindowTextSystem::new(text_system);
    for (index, row) in model.rows.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Prepared::Cancelled;
        }
        if row.line.is_some() && row.text.len() > long_line::SHAPE_LIMIT {
            let Some(line) = long_line::LongLine::prepare(
                row.text.clone(),
                row.kind,
                style,
                &text_system,
                cancelled,
            ) else {
                return Prepared::Cancelled;
            };
            model.long_lines.insert(index, Arc::new(line));
        }
    }
    if !diff::measure_payload_advances(&model, style, &text_system, cancelled) {
        return Prepared::Cancelled;
    }
    let jobs = model.syntax_jobs();
    Prepared::Ready(Box::new(model), jobs)
}

/// Highlights one model off the foreground thread and installs the runs on it.
///
/// Shared by the two paths that need it: a model this view just built, and one adopted from the
/// cache whose own pass was cancelled before it landed. The runs go onto the model itself, which
/// the cache shares, so every view holding it is highlighted at once.
async fn fill_syntax(
    this: &gpui::WeakEntity<DiffView>,
    key: u64,
    model: Rc<DiffModel>,
    jobs: Vec<syntax::Job>,
    cancellation: Arc<AtomicBool>,
    cx: &mut gpui::AsyncApp,
) {
    let syntax_cancel = cancellation.clone();
    let runs = cx
        .background_spawn(async move { syntax::run_cancellable(&jobs, &syntax_cancel) })
        .await;
    // A cancelled pass returns what it had, which is not the whole model: leave the runs off so
    // `syntax_ready` stays false and the next view of this patch runs the jobs again.
    if cancellation.load(Ordering::Relaxed) {
        return;
    }
    model.apply_syntax(runs);
    let _ignored = this.update(cx, |this, cx| {
        if this.key == key {
            cx.notify();
        }
    });
}

/// Which of a model's rows the inline view draws, in order.
///
/// The file-header *card* is the panels' affordance: it is two rows tall and repeats a path the
/// embedding tool row already shows. A single-file patch therefore drops it entirely; a
/// multi-file one keeps a one-line path instead, so the reader can still tell the files apart.
#[must_use]
fn inline_rows(model: &DiffModel) -> Vec<usize> {
    let single = model.files.len() < 2;
    model
        .rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| match row.kind {
            RowKind::FileHeaderFoot => None,
            RowKind::FileHeader if single => None,
            _ => Some(index),
        })
        .collect()
}

/// The rows drawn now, and how many the cap is holding back.
#[must_use]
fn visible(rows: &[usize], expanded: bool) -> (&[usize], usize) {
    if expanded || rows.len() <= MAX_ROWS {
        return (rows, 0);
    }
    (&rows[..MAX_ROWS], rows.len() - MAX_ROWS)
}

/// Stateful reusable inline unified-diff surface.
pub struct DiffView {
    unified: SharedString,
    path: Option<SharedString>,
    key: u64,
    model: Option<Rc<DiffModel>>,
    error: Option<SharedString>,
    expanded: bool,
    actions: Option<DiffActions>,
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for DiffView {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl DiffView {
    /// Creates an inline diff from complete unified-diff text.
    #[must_use]
    pub fn new(unified: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self::for_path(None::<SharedString>, unified, cx)
    }

    /// Creates an inline diff that knows which file it is a patch of.
    ///
    /// The path is what picks the grammar, so a `ToolDiff` should come through here: its
    /// `unified` text may have no `---` header for the language detector to read.
    #[must_use]
    pub fn for_path(
        path: Option<impl Into<SharedString>>,
        unified: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            unified: unified.into(),
            path: path.map(Into::into),
            key: 0,
            model: None,
            error: None,
            expanded: false,
            actions: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            _task: Task::ready(()),
        };
        view.reload(cx);
        view
    }

    /// Returns the source unified-diff text.
    #[must_use]
    pub fn unified(&self) -> &str {
        self.unified.as_ref()
    }

    /// Replaces the unified diff and schedules a render.
    pub fn set_unified(&mut self, unified: impl Into<SharedString>, cx: &mut Context<Self>) {
        let unified = unified.into();
        if unified == self.unified {
            return;
        }
        self.unified = unified;
        self.expanded = false;
        self.reload(cx);
        cx.notify();
    }

    /// Whether the row cap is lifted.
    #[must_use]
    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Lifts or restores the row cap, so an embedder's own key can drive "show all".
    pub fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if self.expanded == expanded {
            return;
        }
        self.expanded = expanded;
        cx.notify();
    }

    /// Installs the caller's action row, drawn under the diff inside the same card.
    pub fn set_actions(
        &mut self,
        actions: impl Fn(&App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) {
        self.actions = Some(Rc::new(actions));
        cx.notify();
    }

    /// Removes the action row.
    pub fn clear_actions(&mut self, cx: &mut Context<Self>) {
        self.actions = None;
        cx.notify();
    }

    /// Takes the prepared model from the cache, or builds one off the foreground thread.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.error = None;
        self.key = cache_key(&self.unified);
        if let Some(model) = cached(self.key) {
            self.model = Some(model.clone());
            // A pass cancelled after its model was cached — the view was dropped or moved on
            // mid-flight — leaves that model in the cache with no syntax at all. Re-run the jobs
            // instead of adopting an un-highlighted model for the life of the process.
            if !model.syntax_ready() {
                let key = self.key;
                let jobs = model.syntax_jobs();
                let cancellation = self.cancelled.clone();
                self._task = cx.spawn(async move |this, cx| {
                    fill_syntax(&this, key, model, jobs, cancellation, cx).await;
                });
            }
            return;
        }
        self.model = None;

        let key = self.key;
        let unified = self.unified.clone();
        let path = self.path.clone();
        let style = long_line::Style::new(cx.theme());
        let text_system = cx.text_system().clone();
        let cancellation = self.cancelled.clone();
        self._task = cx.spawn(async move |this, cx| {
            let build_cancel = cancellation.clone();
            let prepared = cx
                .background_spawn(async move {
                    prepare(
                        &unified,
                        path.as_deref(),
                        &style,
                        text_system,
                        &build_cancel,
                    )
                })
                .await;
            let (model, jobs) = match prepared {
                Prepared::Ready(model, jobs) => (Rc::new(*model), jobs),
                Prepared::Failed(message) => {
                    let _ignored = this.update(cx, |this, cx| {
                        if this.key == key {
                            this.error = Some(message.into());
                            cx.notify();
                        }
                    });
                    return;
                }
                Prepared::Cancelled => return,
            };
            if cancellation.load(Ordering::Relaxed) {
                return;
            }
            retain(key, &model);
            let target = model.clone();
            let installed = this
                .update(cx, |this, cx| {
                    if this.key != key {
                        return false;
                    }
                    this.model = Some(model);
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !installed {
                return;
            }
            fill_syntax(&this, key, target, jobs, cancellation, cx).await;
        });
    }

    /// One `@@` separator: the range summary alone. The panels' "more context" affordance needs
    /// a repository and a pane to widen into, neither of which an inline diff has.
    fn separator(text: SharedString, cx: &App) -> AnyElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(theme.metrics.diff_row_h)
            .overflow_hidden()
            .child(Text::data(text).faint().ellipsize())
            .into_any_element()
    }

    /// The affordance that lifts the row cap.
    fn show_all(&self, hidden: usize, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .id("diff-show-all")
            .flex()
            .flex_row()
            .items_center()
            .h(theme.metrics.diff_row_h)
            .px(theme.space.xs)
            .rounded(theme.radii.xs)
            .cursor_pointer()
            .hover(|style| style.bg(theme.colors.row_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.expanded = true;
                    cx.notify();
                }),
            )
            .child(Text::hint(format!("show all · {hidden} more lines")).faint())
            .into_any_element()
    }
}

impl Render for DiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // 2 px above and 4 px below: the diff hangs off the tool row it belongs to, and the
        // larger gap below separates it from the next row rather than from its own heading.
        let card = div()
            .flex()
            .flex_col()
            .w_full()
            .mt(theme.space.xxs)
            .mb(theme.space.xs)
            .px(theme.space.md)
            .py(theme.space.sm)
            .rounded(theme.radii.sm)
            .bg(theme.colors.surface);

        if let Some(error) = &self.error {
            return card.child(Text::data(error.clone()).muted().ellipsize());
        }
        // Preparation is one background hop, so drawing an empty card here would flash a box
        // under every tool row. Nothing at all is the quieter wrong answer for one frame.
        let Some(model) = self.model.clone() else {
            return div();
        };
        let rows = inline_rows(&model);
        if rows.is_empty() {
            return div();
        }
        let (drawn, hidden) = visible(&rows, self.expanded);
        let palette = RowPalette::tokens(theme);
        let style = RowStyle::default();

        let mut card = card;
        for &index in drawn {
            let Some(row) = model.rows.get(index) else {
                continue;
            };
            card = card.child(match row.kind {
                RowKind::HunkHeader | RowKind::FileHeader | RowKind::Note => {
                    Self::separator(row.text.clone(), cx)
                }
                _ => line_row(&model, index, style, Gutters::Both, palette, cx),
            });
        }
        if hidden > 0 {
            card = card.child(self.show_all(hidden, cx));
        }
        match &self.actions {
            Some(actions) => card.child(actions(cx)),
            None => card,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A headerless patch, which is what a tool result usually carries, with counts that lie.
    const HEADERLESS: &str = concat!(
        "--- a/demo.ts\n",
        "+++ b/demo.ts\n",
        "@@ -1,9 +1,9 @@\n",
        " const one = 1;\n",
        "-const two = 2;\n",
        "+const two = 3;\n",
        "+const three = 4;\n",
    );

    fn model(unified: &str) -> DiffModel {
        parse_model(unified, None, &AtomicBool::new(false))
            .expect("a normalized patch parses")
            .expect("the pass was not cancelled")
    }

    #[test]
    fn normalize_frames_a_headerless_patch_and_recounts_its_hunk() {
        let patch = normalize(HEADERLESS, None);
        assert!(patch.starts_with("diff --git a/demo.ts b/demo.ts\n"));
        // The header claimed nine lines a side; the body has two old and three new.
        assert!(patch.contains("@@ -1,2 +1,3 @@\n"), "{patch}");
        assert!(patch.contains("+const three = 4;\n"));
    }

    #[test]
    fn normalize_leaves_a_git_framed_patch_framed_once() {
        let patch = normalize(
            &format!("diff --git a/demo.ts b/demo.ts\n{HEADERLESS}"),
            None,
        );
        assert_eq!(patch.matches("diff --git ").count(), 1);
    }

    #[test]
    fn normalize_names_a_patch_that_carries_no_paths_at_all() {
        let bare = "@@ -1,1 +1,1 @@\n-old\n+new\n";
        let anonymous = normalize(bare, None);
        assert!(
            anonymous.starts_with("diff --git a/file b/file\n"),
            "{anonymous}"
        );
        assert!(anonymous.contains("--- a/file\n+++ b/file\n"));
        // A caller that knows the path gets the grammar with it.
        let named = normalize(bare, Some("src/main.rs"));
        assert!(named.starts_with("diff --git a/src/main.rs b/src/main.rs\n"));
    }

    #[test]
    fn normalize_restores_a_context_line_stripped_of_its_space() {
        let stripped = "@@ -1,2 +1,2 @@\n\n-old\n+new\n";
        let patch = normalize(stripped, Some("a.txt"));
        assert!(
            patch.contains("@@ -1,2 +1,2 @@\n \n-old\n+new\n"),
            "{patch}"
        );
        // And the result is a patch the parser accepts, which the raw text was not.
        assert_eq!(model(stripped).files.len(), 1);
    }

    #[test]
    fn a_new_file_patch_keeps_its_dev_null_side() {
        let added = "--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,1 @@\n+fn main() {}\n";
        let model = model(added);
        assert_eq!(model.files[0].path, "src/new.rs");
        assert_eq!(model.files[0].language.as_deref(), Some("Rust"));
        assert_eq!(model.files[0].added, 1);
    }

    #[test]
    fn the_model_classifies_context_removals_and_additions() {
        let model = model(HEADERLESS);
        let kinds: Vec<RowKind> = model.rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            vec![
                RowKind::FileHeader,
                RowKind::FileHeaderFoot,
                RowKind::HunkHeader,
                RowKind::Context,
                RowKind::Removed,
                RowKind::Added,
                RowKind::Added,
            ]
        );
        assert_eq!(model.files[0].language.as_deref(), Some("TypeScript"));
        assert_eq!((model.files[0].added, model.files[0].removed), (2, 1));
    }

    #[test]
    fn the_paired_lines_carry_intra_line_ranges() {
        let model = model(HEADERLESS);
        let removed = &model.rows[4];
        let added = &model.rows[5];
        assert_eq!(&removed.text[removed.words[0].clone()], "2");
        assert_eq!(&added.text[added.words[0].clone()], "3");
        // The unpaired addition has no partner to diff against, so it is marked whole or not
        // at all — never against the line above it.
        assert!(model.rows[6].words.is_empty());
    }

    #[test]
    fn inline_rows_drop_the_two_row_file_card_of_a_single_file_patch() {
        let model = model(HEADERLESS);
        let rows = inline_rows(&model);
        assert_eq!(rows, vec![2, 3, 4, 5, 6]);
        assert!(
            !rows
                .iter()
                .any(|&index| model.rows[index].kind == RowKind::FileHeader)
        );
    }

    #[test]
    fn inline_rows_keep_one_path_line_per_file_when_a_patch_spans_several() {
        let two = concat!(
            "diff --git a/a.txt b/a.txt\n",
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-one\n+ONE\n",
            "diff --git a/b.txt b/b.txt\n",
            "--- a/b.txt\n+++ b/b.txt\n@@ -1,1 +1,1 @@\n-two\n+TWO\n",
        );
        let model = model(two);
        let rows = inline_rows(&model);
        let headers: Vec<&RowKind> = rows
            .iter()
            .map(|&index| &model.rows[index].kind)
            .filter(|kind| **kind == RowKind::FileHeader)
            .collect();
        assert_eq!(headers.len(), 2);
        // The card's second row never survives, whatever the file count.
        assert!(
            !rows
                .iter()
                .any(|&index| model.rows[index].kind == RowKind::FileHeaderFoot)
        );
    }

    #[test]
    fn the_cap_folds_a_long_patch_and_show_all_lifts_it() {
        let count = MAX_ROWS + 120;
        let mut patch = String::from("--- a/big.txt\n+++ b/big.txt\n@@ -1,0 +1,0 @@\n");
        for line in 0..count {
            patch.push_str(&format!("+line {line}\n"));
        }
        let model = model(&patch);
        let rows = inline_rows(&model);
        // One hunk separator plus every added line.
        assert_eq!(rows.len(), count + 1);
        let (drawn, hidden) = visible(&rows, false);
        assert_eq!(drawn.len(), MAX_ROWS);
        assert_eq!(hidden, rows.len() - MAX_ROWS);
        let (all, none) = visible(&rows, true);
        assert_eq!(all.len(), rows.len());
        assert_eq!(none, 0);
    }

    #[test]
    fn a_patch_shorter_than_the_cap_hides_nothing() {
        let model = model(HEADERLESS);
        let rows = inline_rows(&model);
        assert_eq!(visible(&rows, false), (&rows[..], 0));
    }

    #[test]
    fn the_cache_key_follows_the_content_and_nothing_else() {
        let copied: String = HEADERLESS.chars().collect();
        assert_eq!(cache_key(HEADERLESS), cache_key(&copied));
        // A one-character edit is a different patch, and so is a whitespace-only one: the key
        // is what decides whether a stale model is reused.
        assert_ne!(
            cache_key(HEADERLESS),
            cache_key(&HEADERLESS.replace('3', "4"))
        );
        assert_ne!(cache_key(HEADERLESS), cache_key(&format!("{HEADERLESS} ")));
    }

    #[test]
    fn the_cache_returns_the_same_model_and_evicts_the_oldest() {
        let key = cache_key("cache-round-trip");
        assert!(cached(key).is_none());
        let model = Rc::new(DiffModel::empty(DiffViewMode::Unified));
        retain(key, &model);
        assert!(cached(key).is_some_and(|hit| Rc::ptr_eq(&hit, &model)));
        // Retaining the same key twice keeps one entry, not two.
        retain(key, &model);
        for filler in 0..CACHE_LIMIT {
            retain(
                cache_key(&format!("filler {filler}")),
                &Rc::new(DiffModel::empty(DiffViewMode::Unified)),
            );
        }
        assert!(cached(key).is_none(), "the oldest entry must be evicted");
        CACHE.with_borrow_mut(Vec::clear);
    }

    /// A pass that is cancelled after its model is cached leaves that model without syntax, and
    /// every later view adopts it from the cache. The next view must re-run the pass instead of
    /// rendering flat text for the life of the process.
    #[gpui::test]
    async fn a_cached_model_without_syntax_is_highlighted_by_the_next_view(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
        let flat = Rc::new(model(HEADERLESS));
        assert!(flat.runs_for(5).is_empty(), "the pass has not run yet");
        retain(cache_key(HEADERLESS), &flat);

        let view = cx.update(|cx| cx.new(|cx| DiffView::new(HEADERLESS, cx)));
        cx.run_until_parked();

        cx.update(|cx| {
            assert!(
                view.read(cx)
                    .model
                    .as_ref()
                    .is_some_and(|model| Rc::ptr_eq(model, &flat)),
                "the cached model is still the one that is shared"
            );
            assert!(
                !flat.runs_for(5).is_empty(),
                "an incomplete cached model must have its syntax pass re-run"
            );
        });
        CACHE.with_borrow_mut(Vec::clear);
    }

    /// Shaping every payload row costs the whole diff, so it belongs to the background pass that
    /// builds the model rather than to the first frame that draws it.
    #[gpui::test]
    async fn payload_advances_are_measured_before_the_model_is_installed(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
        let view = cx.update(|cx| cx.new(|cx| DiffView::new(HEADERLESS, cx)));
        cx.run_until_parked();

        cx.update(|cx| {
            let model = view.read(cx).model.clone().expect("the model is installed");
            assert!(
                model.payload_advances.get().is_some(),
                "the advances must be measured off the foreground thread, before any draw"
            );
        });
        CACHE.with_borrow_mut(Vec::clear);
    }

    #[test]
    fn text_that_is_not_a_patch_reports_instead_of_rendering() {
        // An `@@` line with no closing `@@` is past repair: the counts can be recomputed from a
        // body, but a header this malformed says nothing about where its hunk starts.
        let malformed = "diff --git a/a.txt b/a.txt\n@@ -1,1 +1,1\n-old\n+new\n";
        let failure = parse_model(malformed, None, &AtomicBool::new(false))
            .expect_err("a malformed hunk header is reported");
        assert!(failure.contains("@@"), "{failure}");
    }

    #[test]
    fn a_cancelled_pass_produces_no_model() {
        assert!(
            parse_model(HEADERLESS, None, &AtomicBool::new(true))
                .expect("cancellation is not a parse failure")
                .is_none()
        );
    }
}
