//! Attachment validation, inline-byte materialisation, and orphan collection.

use std::time::Duration;

use fleet_core::agents::{AttachmentSource, ThreadId, UserInput};
use fleet_proto::error::ProtoError;
use tokio_util::sync::CancellationToken;

use super::{AgentSessionManager, validation};
use crate::adapters::files::FileKind;

pub(super) const MAX_INPUT_CHARS: usize = 120_000;
pub(super) const MAX_ATTACHMENTS: usize = 8;
pub(super) const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
pub(super) const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
const ATTACHMENT_SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const IMAGE_TYPES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];

impl AgentSessionManager {
    pub(super) async fn prepare_input(
        &self,
        thread: ThreadId,
        mut input: UserInput,
    ) -> Result<UserInput, ProtoError> {
        if input.text.chars().count() > MAX_INPUT_CHARS {
            return Err(validation(format!(
                "The message is longer than {MAX_INPUT_CHARS} characters."
            )));
        }
        if input.attachments.len() > MAX_ATTACHMENTS {
            return Err(validation(format!(
                "A message may carry at most {MAX_ATTACHMENTS} attachments."
            )));
        }
        for attachment in &mut input.attachments {
            let maximum = attachment_limit(&attachment.media_type);
            match &attachment.source {
                AttachmentSource::Base64(data) => {
                    let size = crate::services::media::decoded_size(data)
                        .map_err(|error| validation(error.to_string()))?;
                    validate_attachment_size(&attachment.media_type, size)?;
                    let template = attachment.clone();
                    let path = self
                        .inner
                        .media
                        .materialize_base64(thread, &template, data.clone())
                        .await
                        .map_err(|error| validation(error.to_string()))?;
                    attachment.source = AttachmentSource::Path(path);
                }
                AttachmentSource::Path(path) => {
                    let metadata = self
                        .inner
                        .media
                        .attachment_metadata(path.clone())
                        .await
                        .map_err(|error| {
                            validation(format!(
                                "Could not inspect attachment against the {} MiB limit: {error}",
                                maximum / (1024 * 1024)
                            ))
                        })?;
                    if metadata.kind != FileKind::File {
                        return Err(validation("Attachments must be regular files."));
                    }
                    validate_attachment_size(&attachment.media_type, metadata.len)?;
                }
                AttachmentSource::Url(_) => {}
            }
        }
        Ok(input)
    }

    /// Runs the start-time attachment collection pass and repeats it on a slow tick.
    pub(crate) async fn run_attachment_sweep(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(ATTACHMENT_SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = self.sweep_attachment_orphans_once().await {
                        tracing::warn!(%error, "failed to sweep orphaned agent attachments");
                    }
                }
            }
        }
    }

    pub(super) async fn sweep_attachment_orphans_once(&self) -> anyhow::Result<usize> {
        let referenced = self.inner.store()?.referenced_attachment_paths().await?;
        self.inner
            .media
            .sweep_orphan_attachments(referenced)
            .await
            .map_err(anyhow::Error::from)
    }
}

fn attachment_limit(media_type: &str) -> u64 {
    if IMAGE_TYPES.contains(&media_type) {
        MAX_IMAGE_BYTES
    } else {
        MAX_FILE_BYTES
    }
}

fn validate_attachment_size(media_type: &str, size: u64) -> Result<(), ProtoError> {
    let maximum = attachment_limit(media_type);
    if size <= maximum {
        return Ok(());
    }
    if maximum == MAX_IMAGE_BYTES {
        Err(validation(format!(
            "An image attachment is larger than {} MiB.",
            MAX_IMAGE_BYTES / (1024 * 1024)
        )))
    } else {
        Err(validation(format!(
            "A file attachment is larger than {} MiB.",
            MAX_FILE_BYTES / (1024 * 1024)
        )))
    }
}
