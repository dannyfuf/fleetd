//! The Workspace's Changes panel: which worktrees show it, and what it reads (UX-SPEC §3.6).
//!
//! The panel's rows are prepared here, when a reading lands, so the panel's render only lays
//! out what is already a `SharedString`. The reading itself comes from a `fleet-git` worker the
//! Workspace owns ([`fleet_lazygit::changes::ChangesWorker`]); this module holds no handle to it.

use std::{collections::HashSet, path::PathBuf, rc::Rc};

use fleet_core::ids::WorktreeId;
use fleet_lazygit::changes::{BranchChanges, BranchFile, DiffKind};
use fleet_ui_kit::Tone;
use gpui::SharedString;

/// How many characters of a commit id the panel shows.
const SHORT_OID: usize = 7;

/// The panel's state across every Workspace this window has shown.
#[derive(Debug, Default)]
pub struct ChangesPanel {
    /// The worktrees whose Workspace shows the panel. Kept in memory for the app's lifetime, so
    /// leaving a Workspace and coming back finds the panel as it was left.
    open: HashSet<WorktreeId>,
    /// The reading for the worktree the Workspace shows, while its panel is open.
    reading: Option<ChangesReading>,
    /// The one file diff the Changes diff sheet shows.
    diff: Option<ChangesDiff>,
    /// Moves whenever anything above does: the harness projection's key.
    revision: u64,
}

/// One worktree's reading, and the base it was read against.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangesReading {
    /// The worktree read.
    pub worktree: WorktreeId,
    /// The ref the worktree was created from, as the panel's header names it.
    pub base: SharedString,
    /// What the panel draws.
    pub body: ReadingBody,
}

/// What the panel knows about its worktree right now.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadingBody {
    /// The first reading has not landed.
    Loading,
    /// The worktree lives on another machine; this app reads git only on its own.
    Remote,
    /// The base names no commit in the worktree's repository.
    MissingBase,
    /// The read failed; the sentence is git's.
    Failed(SharedString),
    /// The prepared rows.
    Ready(Rc<ChangesModel>),
}

/// The panel's rows, prepared once per reading.
#[derive(Debug, PartialEq)]
pub struct ChangesModel {
    /// Changed files, sorted by path.
    pub files: Vec<FileRow>,
    /// The newest commits ahead of the base.
    pub commits: Vec<CommitRow>,
    /// Commits ahead of the base in total, which may exceed `commits.len()`.
    pub ahead: u64,
}

/// One changed file as the panel draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    /// The path, relative to the worktree root.
    pub path: PathBuf,
    /// The path as drawn.
    pub label: SharedString,
    /// `M`, `A`, `D`, `R` …
    pub letter: SharedString,
    /// The letter's colour.
    pub tone: Tone,
    /// `+12`, absent for a binary file.
    pub added: Option<SharedString>,
    /// `−3`, absent for a binary file or when nothing was removed.
    pub removed: Option<SharedString>,
}

/// One commit ahead of the base.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitRow {
    /// The abbreviated id.
    pub short: SharedString,
    /// The subject line.
    pub subject: SharedString,
}

/// The file diff the sheet shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangesDiff {
    /// The worktree the file belongs to.
    pub worktree: WorktreeId,
    /// The file.
    pub path: PathBuf,
    /// The path as the sheet's title draws it.
    pub label: SharedString,
    /// The base, for the sheet's subtitle.
    pub base: SharedString,
    /// The diff, once read.
    pub body: DiffBody,
}

/// What the diff sheet knows about its file.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffBody {
    /// The read is in flight.
    Loading,
    /// The base names no commit.
    MissingBase,
    /// The read failed.
    Failed(SharedString),
    /// The unified diff text; empty when the file no longer differs.
    Ready(SharedString),
}

impl ChangesPanel {
    /// Whether `worktree`'s Workspace shows the panel.
    #[must_use]
    pub fn is_open(&self, worktree: &WorktreeId) -> bool {
        self.open.contains(worktree)
    }

    /// Opens or closes the panel for `worktree`, returning whether it is now open.
    pub fn toggle(&mut self, worktree: &WorktreeId) -> bool {
        let open = if self.open.remove(worktree) {
            if self
                .reading
                .as_ref()
                .is_some_and(|reading| &reading.worktree == worktree)
            {
                self.reading = None;
            }
            false
        } else {
            self.open.insert(worktree.clone());
            true
        };
        self.bump();
        open
    }

    /// The reading the panel draws, for the worktree it belongs to.
    #[must_use]
    pub fn reading(&self) -> Option<&ChangesReading> {
        self.reading.as_ref()
    }

    /// The reading for `worktree`, if that is the worktree it belongs to.
    #[must_use]
    pub fn reading_for(&self, worktree: &WorktreeId) -> Option<&ChangesReading> {
        self.reading
            .as_ref()
            .filter(|reading| &reading.worktree == worktree)
    }

    /// Starts a reading over, or forgets it with `None`.
    pub fn begin(&mut self, reading: Option<(WorktreeId, String, ReadingBody)>) {
        let next = reading.map(|(worktree, base, body)| ChangesReading {
            worktree,
            base: base.into(),
            body,
        });
        if self.reading != next {
            self.reading = next;
            self.bump();
        }
    }

    /// Applies a worker's reading for `worktree`; one for a worktree no longer shown is dropped.
    /// Returns whether anything the panel draws changed.
    pub fn apply(
        &mut self,
        worktree: &WorktreeId,
        result: Result<Option<BranchChanges>, String>,
    ) -> bool {
        let Some(reading) = self
            .reading
            .as_mut()
            .filter(|reading| &reading.worktree == worktree)
        else {
            return false;
        };
        let body = match result {
            Ok(Some(changes)) => ReadingBody::Ready(Rc::new(ChangesModel::from(changes))),
            Ok(None) => ReadingBody::MissingBase,
            // A failed refresh keeps the rows it had: a panel that blinks empty every time git
            // is briefly busy is worse than one a few seconds old.
            Err(error) if matches!(reading.body, ReadingBody::Ready(_)) => {
                tracing::debug!(%worktree, %error, "changes: a refresh failed");
                return false;
            }
            Err(error) => ReadingBody::Failed(error.into()),
        };
        if reading.body == body {
            return false;
        }
        reading.body = body;
        self.bump();
        true
    }

    /// The diff the sheet shows.
    #[must_use]
    pub fn diff(&self) -> Option<&ChangesDiff> {
        self.diff.as_ref()
    }

    /// Points the sheet at `path`, loading.
    pub fn begin_diff(&mut self, worktree: WorktreeId, path: PathBuf, base: SharedString) {
        self.diff = Some(ChangesDiff {
            label: path.to_string_lossy().into_owned().into(),
            worktree,
            path,
            base,
            body: DiffBody::Loading,
        });
        self.bump();
    }

    /// Applies a worker's diff; one for another file is dropped. Returns whether it applied.
    pub fn apply_diff(
        &mut self,
        worktree: &WorktreeId,
        path: &PathBuf,
        result: Result<Option<String>, String>,
    ) -> bool {
        let Some(diff) = self
            .diff
            .as_mut()
            .filter(|diff| &diff.worktree == worktree && &diff.path == path)
        else {
            return false;
        };
        diff.body = match result {
            Ok(Some(text)) => DiffBody::Ready(text.into()),
            Ok(None) => DiffBody::MissingBase,
            Err(error) => DiffBody::Failed(error.into()),
        };
        self.bump();
        true
    }

    /// The harness projection's key.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

impl From<BranchChanges> for ChangesModel {
    fn from(changes: BranchChanges) -> Self {
        Self {
            files: changes.files.into_iter().map(file_row).collect(),
            commits: changes
                .commits
                .into_iter()
                .map(|commit| CommitRow {
                    short: commit
                        .oid
                        .0
                        .chars()
                        .take(SHORT_OID)
                        .collect::<String>()
                        .into(),
                    subject: commit.subject.into(),
                })
                .collect(),
            ahead: changes.ahead,
        }
    }
}

fn file_row(file: BranchFile) -> FileRow {
    let (letter, tone) = kind_letter(file.kind);
    FileRow {
        label: file.path.to_string_lossy().into_owned().into(),
        path: file.path,
        letter: SharedString::new_static(letter),
        tone,
        added: file.added.map(|added| format!("+{added}").into()),
        // U+2212, the minus sign the mockup's counts use, not a hyphen.
        removed: file
            .removed
            .filter(|removed| *removed > 0)
            .map(|removed| format!("\u{2212}{removed}").into()),
    }
}

/// The status letter and its tone: `M` amber, `A` green, `D` red, a move blue.
const fn kind_letter(kind: DiffKind) -> (&'static str, Tone) {
    match kind {
        DiffKind::Added => ("A", Tone::Success),
        DiffKind::Deleted => ("D", Tone::Danger),
        DiffKind::Modified => ("M", Tone::Warning),
        DiffKind::Renamed => ("R", Tone::Accent),
        DiffKind::Copied => ("C", Tone::Accent),
        DiffKind::TypeChanged => ("T", Tone::Warning),
        DiffKind::Unmerged => ("U", Tone::Danger),
        DiffKind::Unknown => ("?", Tone::Secondary),
    }
}

#[cfg(test)]
mod tests {
    use fleet_lazygit::changes::AheadCommit;

    use super::*;

    fn worktree(slug: &str) -> WorktreeId {
        format!("acme/api#{slug}")
            .parse()
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn changes() -> BranchChanges {
        BranchChanges {
            base: fleet_git_ref("origin/main"),
            merge_base: fleet_lazygit_oid("3a1279e0000"),
            files: vec![
                BranchFile {
                    path: "README.md".into(),
                    previous_path: None,
                    kind: DiffKind::Modified,
                    added: Some(2),
                    removed: Some(1),
                },
                BranchFile {
                    path: "src/run.rs".into(),
                    previous_path: None,
                    kind: DiffKind::Added,
                    added: Some(96),
                    removed: Some(0),
                },
            ],
            commits: vec![AheadCommit {
                oid: fleet_lazygit_oid("3a1279e0123456"),
                subject: "mark a card's run".to_owned(),
            }],
            ahead: 2,
        }
    }

    fn fleet_git_ref(name: &str) -> fleet_lazygit::changes::Ref {
        fleet_lazygit::changes::Ref(name.to_owned())
    }

    fn fleet_lazygit_oid(oid: &str) -> fleet_lazygit::changes::ObjectId {
        fleet_lazygit::changes::ObjectId(oid.to_owned())
    }

    #[test]
    fn a_reading_prepares_the_rows_the_panel_draws() {
        let mut panel = ChangesPanel::default();
        let feature = worktree("feature");
        assert!(panel.toggle(&feature));
        panel.begin(Some((
            feature.clone(),
            "origin/main".to_owned(),
            ReadingBody::Loading,
        )));
        assert!(panel.apply(&feature, Ok(Some(changes()))));

        let Some(ReadingBody::Ready(model)) = panel.reading_for(&feature).map(|r| r.body.clone())
        else {
            panic!("the reading is ready");
        };
        assert_eq!(model.files[0].letter, "M");
        assert_eq!(model.files[0].added.as_deref(), Some("+2"));
        assert_eq!(model.files[0].removed.as_deref(), Some("\u{2212}1"));
        assert_eq!(model.files[1].removed, None, "an added file shows no −0");
        assert_eq!(model.commits[0].short, "3a1279e");
        assert_eq!(model.ahead, 2);
    }

    #[test]
    fn a_reading_for_another_worktree_is_dropped_and_a_failed_refresh_keeps_the_rows() {
        let mut panel = ChangesPanel::default();
        let feature = worktree("feature");
        panel.toggle(&feature);
        panel.begin(Some((
            feature.clone(),
            "origin/main".to_owned(),
            ReadingBody::Loading,
        )));
        assert!(!panel.apply(&worktree("hotfix"), Ok(Some(changes()))));
        assert!(panel.apply(&feature, Ok(Some(changes()))));
        let revision = panel.revision();
        assert!(!panel.apply(&feature, Err("index.lock exists".to_owned())));
        assert_eq!(panel.revision(), revision);
        assert!(matches!(
            panel.reading().map(|r| &r.body),
            Some(ReadingBody::Ready(_))
        ));
    }

    #[test]
    fn open_state_is_kept_per_worktree() {
        let mut panel = ChangesPanel::default();
        let feature = worktree("feature");
        let hotfix = worktree("hotfix");
        panel.toggle(&feature);
        assert!(panel.is_open(&feature));
        assert!(!panel.is_open(&hotfix));
        assert!(!panel.toggle(&feature));
        assert!(!panel.is_open(&feature));
    }
}
