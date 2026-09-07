//! Atomic board document persistence and recoverable quarantine/deletion.
use crate::{DaemonError, DaemonResult, adapters::files::Files};
use fleet_core::{
    board::{BOARD_DOCUMENT_VERSION, BoardDocument, validate_board, validate_card},
    ids::BoardId,
    paths::FleetHome,
};
use std::sync::Arc;

/// One versioned JSON document per board.
#[derive(Clone)]
pub struct BoardStore {
    home: FleetHome,
    files: Arc<dyn Files>,
}

impl BoardStore {
    /// Creates a store using the supplied filesystem boundary.
    pub fn new(home: FleetHome, files: Arc<dyn Files>) -> Self {
        Self { home, files }
    }
    /// Lists valid board filenames in stable identifier order.
    pub fn list(&self) -> DaemonResult<Vec<BoardId>> {
        if !self.files.exists(&self.home.boards_dir()) {
            return Ok(Vec::new());
        }
        let mut ids: Vec<_> = self
            .files
            .list(&self.home.boards_dir())?
            .into_iter()
            .filter_map(|p| {
                if p.extension()?.to_str()? != "json" {
                    return None;
                }
                BoardId::try_from(p.file_stem()?.to_str()?).ok()
            })
            .collect();
        ids.sort();
        Ok(ids)
    }
    /// Loads and validates a document, quarantining malformed documents before returning an error.
    pub fn load(&self, id: &BoardId) -> DaemonResult<Option<BoardDocument>> {
        self.read(id, true)
    }
    /// Loads a document without quarantining a malformed one.
    ///
    /// Scanning the store is a read: `load` moves a damaged document aside so that the caller
    /// about to write can never write over it, but a *scan* that did the same would quarantine
    /// the file and then hand the very next `ensure` an empty directory to make a fresh board
    /// in — losing the board to a plain read of it.
    pub fn peek(&self, id: &BoardId) -> DaemonResult<Option<BoardDocument>> {
        self.read(id, false)
    }
    /// The quarantined documents this board left behind, in stable path order.
    pub fn quarantined(&self, id: &BoardId) -> DaemonResult<Vec<std::path::PathBuf>> {
        if !self.files.exists(&self.home.boards_dir()) {
            return Ok(Vec::new());
        }
        let prefix = format!("{id}.json.broken-");
        Ok(self
            .files
            .list(&self.home.boards_dir())?
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
            })
            .collect())
    }
    /// A cheap change stamp for a document, used to skip re-reading an unchanged board.
    ///
    /// This is the one read that goes around the [`Files`] boundary: it observes nothing but
    /// the size and modification time, and a filesystem that reports neither simply makes the
    /// caller parse the document again.
    #[must_use]
    pub fn stamp(&self, id: &BoardId) -> Option<(u64, std::time::SystemTime)> {
        let metadata = std::fs::metadata(self.home.board_path(id)).ok()?;
        Some((metadata.len(), metadata.modified().ok()?))
    }
    fn read(&self, id: &BoardId, quarantine: bool) -> DaemonResult<Option<BoardDocument>> {
        let path = self.home.board_path(id);
        if !self.files.exists(&path) {
            return Ok(None);
        }
        let text = self.files.read_text(&path)?;
        // A document this build is too old to read is intact, not damaged: quarantining it
        // would make the board disappear from a downgraded daemon and never come back.
        if let Ok(version) = serde_json::from_str::<DocumentVersion>(&text)
            && version.version != BOARD_DOCUMENT_VERSION
        {
            return Err(DaemonError::Unsupported(format!(
                "board {id} uses document version {} (this build reads {BOARD_DOCUMENT_VERSION})",
                version.version
            )));
        }
        let parsed = serde_json::from_str::<BoardDocument>(&text)
            .map_err(DaemonError::from)
            .and_then(|doc| {
                validate_document(&doc)?;
                if doc.board.id != *id {
                    return Err(DaemonError::Validation(
                        "board id does not match filename".into(),
                    ));
                }
                Ok(doc)
            });
        match parsed {
            Ok(doc) => Ok(Some(doc)),
            Err(error) => {
                if quarantine {
                    let broken =
                        path.with_file_name(format!("{id}.json.broken-{}", uuid::Uuid::new_v4()));
                    self.files.rename(&path, &broken)?;
                }
                Err(DaemonError::Validation(format!(
                    "invalid board document: {error}"
                )))
            }
        }
    }
    /// Validates and atomically replaces a complete board document.
    pub fn save(&self, doc: &BoardDocument) -> DaemonResult<()> {
        validate_document(doc)?;
        self.files.create_dir_all(&self.home.boards_dir())?;
        let mut text = serde_json::to_string_pretty(doc)?;
        text.push('\n');
        self.files
            .atomic_write_text(&self.home.board_path(&doc.board.id), &text)
    }
    /// Moves a board and anything quarantined from it into Fleet's trash directory.
    pub fn delete(&self, id: &BoardId) -> DaemonResult<()> {
        // A quarantined remnant left in place would refuse every later board this id derives
        // from, so a deleted context could never be recreated with a working board.
        for path in self.quarantined(id)? {
            self.trash(&path, id)?;
        }
        let path = self.home.board_path(id);
        if !self.files.exists(&path) {
            return Ok(());
        }
        self.trash(&path, id)
    }
    fn trash(&self, path: &std::path::Path, id: &BoardId) -> DaemonResult<()> {
        self.files.create_dir_all(&self.home.trash_dir())?;
        let destination = self
            .home
            .trash_dir()
            .join(format!("board-{id}-{}.json", uuid::Uuid::new_v4()));
        self.files.rename(path, &destination)
    }
}

/// Just enough of the document to read its version before trusting the rest of the shape.
#[derive(serde::Deserialize)]
struct DocumentVersion {
    version: u32,
}

fn validate_document(doc: &BoardDocument) -> DaemonResult<()> {
    if doc.version != BOARD_DOCUMENT_VERSION {
        return Err(DaemonError::Validation(format!(
            "unsupported board document version {}",
            doc.version
        )));
    }
    validate_board(&doc.board)?;
    let mut ids = std::collections::HashSet::new();
    let mut numbers = std::collections::HashSet::new();
    for card in &doc.cards {
        validate_card(&doc.board, card)?;
        if !ids.insert(&card.id) || !numbers.insert(card.number) {
            return Err(DaemonError::Validation(
                "duplicate card id or number".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::files::RealFiles;
    use fleet_core::{board::new_board, model::Context};

    fn fixture() -> (tempfile::TempDir, FleetHome, BoardStore, BoardDocument) {
        let temp = tempfile::tempdir().unwrap();
        let home = FleetHome::new(temp.path());
        let files = Arc::new(RealFiles::new(home.trash_dir(), [home.boards_dir()]));
        let store = BoardStore::new(home.clone(), files);
        let board = new_board(
            &Context {
                id: "work".parse().unwrap(),
                name: "Work".into(),
                owners: vec![],
                created_at: "now".into(),
            },
            "now",
        );
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board,
            cards: vec![],
        };
        (temp, home, store, doc)
    }
    #[test]
    fn atomic_round_trip_list_and_recoverable_delete() {
        let (_temp, home, store, doc) = fixture();
        assert!(store.list().unwrap().is_empty());
        assert!(store.load(&doc.board.id).unwrap().is_none());
        store.save(&doc).unwrap();
        assert_eq!(store.load(&doc.board.id).unwrap(), Some(doc.clone()));
        assert_eq!(store.list().unwrap(), vec![doc.board.id.clone()]);
        assert_eq!(std::fs::read_dir(home.boards_dir()).unwrap().count(), 1);
        store.delete(&doc.board.id).unwrap();
        assert!(store.list().unwrap().is_empty());
        let trash = std::fs::read_dir(home.trash_dir())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let recovered: BoardDocument =
            serde_json::from_str(&std::fs::read_to_string(trash).unwrap()).unwrap();
        assert_eq!(recovered, doc);
        store.delete(&doc.board.id).unwrap();
    }
    #[test]
    fn malformed_and_invalid_documents_are_quarantined() {
        let (_temp, home, store, doc) = fixture();
        store.save(&doc).unwrap();
        std::fs::write(home.board_path(&doc.board.id), "not json").unwrap();
        assert!(store.load(&doc.board.id).is_err());
        assert!(!home.board_path(&doc.board.id).exists());
        assert!(store.list().unwrap().is_empty());
        let mut invalid = doc.clone();
        invalid.board.statuses.clear();
        std::fs::write(
            home.board_path(&doc.board.id),
            serde_json::to_string(&invalid).unwrap(),
        )
        .unwrap();
        assert!(store.load(&doc.board.id).is_err());
        assert_eq!(std::fs::read_dir(home.boards_dir()).unwrap().count(), 2);
        assert!(store.save(&invalid).is_err());
    }
    #[test]
    fn peek_reports_a_damaged_document_without_moving_it() {
        let (_temp, home, store, doc) = fixture();
        store.save(&doc).unwrap();
        std::fs::write(home.board_path(&doc.board.id), "not json").unwrap();
        // Scanning must be non-destructive: quarantining here would let the `ensure` that
        // scanned for this board create an empty one in its place.
        assert!(store.peek(&doc.board.id).is_err());
        assert!(home.board_path(&doc.board.id).exists());
        assert!(store.quarantined(&doc.board.id).unwrap().is_empty());
        // Only `load` — the read a writer makes — moves it aside, and it stays findable.
        assert!(store.load(&doc.board.id).is_err());
        assert_eq!(store.quarantined(&doc.board.id).unwrap().len(), 1);
        assert!(store.peek(&doc.board.id).unwrap().is_none());
        assert!(store.stamp(&doc.board.id).is_none());
        // Deleting the board takes its quarantined remains with it, into the same trash.
        store.delete(&doc.board.id).unwrap();
        assert!(store.quarantined(&doc.board.id).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(home.trash_dir()).unwrap().count(), 1);
    }

    #[test]
    fn rejects_version_and_filename_mismatches() {
        let (_temp, home, store, doc) = fixture();
        store.save(&doc).unwrap();
        let mut bad = doc.clone();
        bad.version += 1;
        assert!(store.save(&bad).is_err());
        let other: BoardId = "other".parse().unwrap();
        std::fs::write(
            home.board_path(&other),
            serde_json::to_string(&doc).unwrap(),
        )
        .unwrap();
        assert!(store.load(&other).is_err());
        assert!(!home.board_path(&other).exists());
    }

    #[test]
    fn a_future_document_version_is_reported_without_quarantining_the_file() {
        let (_temp, home, store, doc) = fixture();
        store.save(&doc).unwrap();
        let mut future = doc.clone();
        future.version = BOARD_DOCUMENT_VERSION + 1;
        std::fs::write(
            home.board_path(&doc.board.id),
            serde_json::to_string(&future).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.load(&doc.board.id),
            Err(DaemonError::Unsupported(_))
        ));
        // The document is intact and must still be there for a build that can read it.
        assert!(home.board_path(&doc.board.id).exists());
        assert_eq!(store.list().unwrap(), vec![doc.board.id.clone()]);
    }
}
