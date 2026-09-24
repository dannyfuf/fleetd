//! The versioned `schedules.json` store: atomic saves, validation on load and save, and
//! quarantine of an unreadable document. A readable document keeps every schedule this build
//! accepts: one it refuses is set aside in a copy of the file, not the whole document with it.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use fleet_core::{
    paths::FleetHome,
    schedule::{SCHEDULES_DOCUMENT_VERSION, ScheduleError, SchedulesDocument, validate_schedule},
};

use crate::{
    DaemonError, DaemonResult,
    adapters::{clock::Clock, files::Files},
};

#[cfg(test)]
mod tests;

/// Validated schedule document store at `<FLEET_HOME>/schedules.json`.
#[derive(Clone)]
pub struct ScheduleStore {
    path: PathBuf,
    files: Arc<dyn Files>,
    clock: Arc<dyn Clock>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl ScheduleStore {
    /// Creates a schedule store rooted at a Fleet home.
    #[must_use]
    pub fn new(home: &FleetHome, files: Arc<dyn Files>, clock: Arc<dyn Clock>) -> Self {
        Self {
            path: home.schedules_path(),
            files,
            clock,
            gate: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Loads the document; a missing file is an empty document at the current version.
    ///
    /// An unreadable document is quarantined and read as empty; a readable one loses only the
    /// schedules this build refuses, which stay in a `schedules.json.broken-<epoch>` copy; a
    /// document written by a newer daemon is refused and left in place.
    pub async fn load(&self) -> DaemonResult<SchedulesDocument> {
        let _guard = self.gate.lock().await;
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        let epoch = self.clock.epoch_millis();
        tokio::task::spawn_blocking(move || load_sync(&path, files.as_ref(), epoch))
            .await
            .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Runs one load-apply-validate-save transaction under the store's gate.
    ///
    /// Nothing is written when the operation fails or leaves an invalid document behind.
    pub async fn transaction<F, R>(&self, operation: F) -> DaemonResult<R>
    where
        F: FnOnce(&mut SchedulesDocument) -> DaemonResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let _guard = self.gate.lock().await;
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        let epoch = self.clock.epoch_millis();
        tokio::task::spawn_blocking(move || {
            let mut document = load_sync(&path, files.as_ref(), epoch)?;
            let result = operation(&mut document)?;
            save_sync(&path, files.as_ref(), &document)?;
            Ok(result)
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Returns the backing `schedules.json` path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Holds the transaction gate so cancellation paths can be tested at an exact await.
    #[cfg(test)]
    pub(crate) async fn hold_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.gate.lock().await
    }
}

/// The one field read before the whole document, so a newer version is refused, not quarantined.
#[derive(serde::Deserialize)]
struct DocumentVersion {
    version: u32,
}

fn load_sync(path: &Path, files: &dyn Files, epoch: i64) -> DaemonResult<SchedulesDocument> {
    if !files.exists(path) {
        return Ok(SchedulesDocument::default());
    }
    let text = files.read_text(path)?;
    // A document from another version is intact, not damaged: quarantining it would lose a
    // newer daemon's schedules the moment an older one starts.
    if let Ok(version) = serde_json::from_str::<DocumentVersion>(&text)
        && version.version != SCHEDULES_DOCUMENT_VERSION
    {
        return Err(DaemonError::Unsupported(format!(
            "schedules.json uses document version {} (this build reads \
             {SCHEDULES_DOCUMENT_VERSION})",
            version.version
        )));
    }
    let parsed = serde_json::from_str::<SchedulesDocument>(&text)
        .map_err(|error| DaemonError::Validation(format!("invalid schedules JSON: {error}")))
        .and_then(|document| {
            if document.version == SCHEDULES_DOCUMENT_VERSION {
                Ok(document)
            } else {
                Err(DaemonError::Validation(format!(
                    "schedules document version must be {SCHEDULES_DOCUMENT_VERSION}"
                )))
            }
        });
    match parsed {
        Ok(mut document) => {
            repair(&mut document);
            let dropped = drop_invalid(&mut document);
            if !dropped.is_empty() {
                // The rest of the document is intact: only what this build refuses is set
                // aside, in a copy of the file as it was, and the repaired document is written
                // once so the next load does not set it aside again.
                let broken = path.with_file_name(format!("schedules.json.broken-{epoch}"));
                files.atomic_write_text(&broken, &text)?;
                for (id, error) in &dropped {
                    tracing::warn!(
                        schedule = %id,
                        %error,
                        path = %broken.display(),
                        "set aside a schedule this build refuses"
                    );
                }
                save_sync(path, files, &document)?;
            }
            Ok(document)
        }
        Err(error) => {
            let broken = quarantine(path, files, epoch)?;
            tracing::warn!(
                %error,
                path = %broken.display(),
                "quarantined unreadable schedules document"
            );
            Ok(SchedulesDocument::default())
        }
    }
}

/// Brings a stored schedule up to rules added since it was written, where the old value has one
/// meaning under the new rule: a blank `model` or `effort` always meant "the provider's default".
fn repair(document: &mut SchedulesDocument) {
    let blank =
        |value: &Option<String>| value.as_ref().is_some_and(|value| value.trim().is_empty());
    for schedule in &mut document.schedules {
        if blank(&schedule.agent.model) {
            schedule.agent.model = None;
        }
        if blank(&schedule.agent.effort) {
            schedule.agent.effort = None;
        }
    }
}

/// Removes every schedule this build refuses — an invalid one, or a second with an id already
/// seen — and answers each one's id and why.
fn drop_invalid(document: &mut SchedulesDocument) -> Vec<(String, DaemonError)> {
    let mut ids = BTreeSet::new();
    let mut dropped = Vec::new();
    document.schedules.retain(|schedule| {
        let refused = match validate_schedule(schedule) {
            Err(error) => Some(schedule_error(error)),
            Ok(()) if !ids.insert(schedule.id.clone()) => Some(DaemonError::Validation(format!(
                "duplicate schedule {}",
                schedule.id
            ))),
            Ok(()) => None,
        };
        match refused {
            Some(error) => {
                dropped.push((schedule.id.to_string(), error));
                false
            }
            None => true,
        }
    });
    dropped
}

fn save_sync(path: &Path, files: &dyn Files, document: &SchedulesDocument) -> DaemonResult<()> {
    validate_document(document)?;
    let mut text = serde_json::to_string_pretty(document)?;
    text.push('\n');
    files.atomic_write_text(path, &text)
}

fn validate_document(document: &SchedulesDocument) -> DaemonResult<()> {
    if document.version != SCHEDULES_DOCUMENT_VERSION {
        return Err(DaemonError::Validation(format!(
            "schedules document version must be {SCHEDULES_DOCUMENT_VERSION}"
        )));
    }
    let mut ids = BTreeSet::new();
    for schedule in &document.schedules {
        validate_schedule(schedule).map_err(schedule_error)?;
        if !ids.insert(&schedule.id) {
            return Err(DaemonError::Validation(format!(
                "duplicate schedule {}",
                schedule.id
            )));
        }
    }
    Ok(())
}

fn schedule_error(error: ScheduleError) -> DaemonError {
    match error {
        ScheduleError::Invalid { .. } => DaemonError::Validation(error.to_string()),
        ScheduleError::NotFound(id) => DaemonError::NotFound(format!("schedule {id}")),
    }
}

fn quarantine(path: &Path, files: &dyn Files, epoch: i64) -> DaemonResult<PathBuf> {
    let broken = path.with_file_name(format!("schedules.json.broken-{epoch}"));
    files.rename(path, &broken)?;
    Ok(broken)
}
