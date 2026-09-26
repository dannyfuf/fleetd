//! Native-agent composer attachment staging.
//!
//! The view owns only chip state. Upload tasks and progress remain in `AppState`'s phase-one
//! registry, while the Workspace supplies its `Bridge` and app-state entity at this boundary.

use std::fs;

use fleet_proto::request::MediaAnchor;
use fleet_ui_kit::{PendingAttachmentState, TextInputMedia};
use gpui::{App, Entity, TaskExt as _};

use super::{AgentThreadEvent, AgentThreadView, PendingAttachment};
use crate::{
    bridge::Bridge,
    media::{self, Attachment, StageOutcome, UploadState},
    state::AppState,
};

pub(crate) const MAX_ATTACHMENTS: usize = 8;
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

pub(crate) const ATTACHMENT_COUNT_REFUSAL: &str = "A message may carry at most 8 attachments.";
pub(crate) const IMAGE_SIZE_REFUSAL: &str = "An image attachment is larger than 10 MiB.";
pub(crate) const FILE_SIZE_REFUSAL: &str = "A file attachment is larger than 50 MiB.";
pub(crate) const FOLDER_REFUSAL: &str = "Folders cannot be attached; drop the files instead.";

struct PreparedAttachment {
    attachment: Attachment,
    name: String,
    media_type: String,
}

/// Preflights one kit media event, then stages every accepted file into this thread's leaf.
pub(crate) fn stage_event(
    view: &Entity<AgentThreadView>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    event: TextInputMedia,
    cx: &mut App,
) {
    let attachment = match event {
        TextInputMedia::Pasted(item) => {
            let Some(attachment) = media::from_clipboard(&item) else {
                return;
            };
            attachment
        }
        TextInputMedia::Dropped(paths) => media::from_external_paths(&paths),
    };
    let count = match &attachment {
        Attachment::Paths(paths) => paths.len(),
        Attachment::Blob { .. } => 1,
    };
    let name = attachment_name(&attachment);
    let Some(preflight_id) = view.update(cx, |view, cx| {
        view.reserve_attachment_slots(count, name, cx)
    }) else {
        return;
    };

    let inspection = cx
        .background_executor()
        .spawn(async move { preflight(attachment) });
    let weak_view = view.downgrade();
    let weak_state = state.downgrade();
    let bridge = bridge.clone();
    cx.spawn(async move |cx| {
        let prepared = inspection.await;
        let Some(state) = weak_state.upgrade() else {
            weak_view.update(cx, |view, cx| {
                view.finish_attachment_preflight(
                    preflight_id,
                    Err("The attachment could not be copied.".to_owned()),
                    None,
                    &bridge,
                    cx,
                );
            })?;
            return anyhow::Ok(());
        };
        weak_view.update(cx, |view, cx| {
            view.finish_attachment_preflight(preflight_id, prepared, Some(&state), &bridge, cx);
        })?;
        anyhow::Ok(())
    })
    // Filesystem failures become notices in `finish_attachment_preflight`; an entity-lifetime
    // failure is still logged instead of being silently detached.
    .detach_and_log_err(cx);
}

fn attachment_name(attachment: &Attachment) -> String {
    match attachment {
        Attachment::Blob { name, .. } => name.clone(),
        Attachment::Paths(paths) => paths.first().map_or_else(
            || "attachment".to_owned(),
            |path| {
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
            },
        ),
    }
}

fn preflight(attachment: Attachment) -> Result<Vec<PreparedAttachment>, String> {
    match attachment {
        Attachment::Blob {
            name,
            format,
            bytes,
        } => {
            let media_type = media::media_type_for_image(format).to_owned();
            validate_size(&media_type, u64::try_from(bytes.len()).unwrap_or(u64::MAX))?;
            Ok(vec![PreparedAttachment {
                attachment: Attachment::Blob {
                    name: name.clone(),
                    format,
                    bytes,
                },
                name,
                media_type,
            }])
        }
        Attachment::Paths(paths) => {
            let mut prepared = Vec::with_capacity(paths.len());
            for path in paths {
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| format!("Could not inspect attachment: {error}"))?;
                if metadata.is_dir() {
                    return Err(FOLDER_REFUSAL.to_owned());
                }
                if !metadata.is_file() {
                    return Err("Attachments must be regular files.".to_owned());
                }
                let media_type = media::media_type_for_path(&path).to_owned();
                validate_size(&media_type, metadata.len())?;
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into(),
                );
                prepared.push(PreparedAttachment {
                    attachment: Attachment::Paths(vec![path]),
                    name,
                    media_type,
                });
            }
            Ok(prepared)
        }
    }
}

fn validate_size(media_type: &str, bytes: u64) -> Result<(), String> {
    if media::is_agent_image_type(media_type) {
        if bytes > MAX_IMAGE_BYTES {
            return Err(IMAGE_SIZE_REFUSAL.to_owned());
        }
    } else if bytes > MAX_FILE_BYTES {
        return Err(FILE_SIZE_REFUSAL.to_owned());
    }
    Ok(())
}

impl AgentThreadView {
    fn reserve_attachment_slots(
        &mut self,
        count: usize,
        name: String,
        cx: &mut gpui::Context<Self>,
    ) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let reserved = self
            .attachment_preflights
            .iter()
            .map(|preflight| preflight.count)
            .sum::<usize>();
        if self
            .pending_attachments
            .len()
            .saturating_add(reserved)
            .saturating_add(count)
            > MAX_ATTACHMENTS
        {
            self.notice(ATTACHMENT_COUNT_REFUSAL, cx);
            return None;
        }
        let id = self.next_attachment_id;
        self.next_attachment_id = self.next_attachment_id.wrapping_add(1);
        self.attachment_preflights
            .push(super::PendingAttachmentPreflight { id, count, name });
        Some(id)
    }

    fn finish_attachment_preflight(
        &mut self,
        preflight: usize,
        prepared: Result<Vec<PreparedAttachment>, String>,
        state: Option<&Entity<AppState>>,
        bridge: &Bridge,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(index) = self
            .attachment_preflights
            .iter()
            .position(|candidate| candidate.id == preflight)
        else {
            // A thread switch invalidates in-flight preflights. Their late results must not stage
            // into the replacement thread.
            return;
        };
        self.attachment_preflights.remove(index);
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(message) => {
                self.notice(message, cx);
                return;
            }
        };
        let mode = self.composer_mode();
        if self.is_unreachable() {
            if let Some(host) = &self.host {
                self.notice(super::presentation::unreachable_placeholder(&host.name), cx);
            }
            return;
        }
        if !mode.attachments_enabled(!self.question_is_choice_only()) {
            self.notice("Attachments are not available in this composer state.", cx);
            return;
        }
        if self
            .pending_attachments
            .len()
            .saturating_add(prepared.len())
            > MAX_ATTACHMENTS
        {
            self.notice(ATTACHMENT_COUNT_REFUSAL, cx);
            return;
        }
        let Some(state) = state else {
            self.notice("The attachment could not be copied.", cx);
            return;
        };
        if state.read(cx).agents.summary(self.thread).is_none() {
            self.notice("This agent thread is no longer available.", cx);
            return;
        }

        for prepared in prepared {
            self.start_staging(prepared, state, bridge, cx);
        }
        cx.notify();
    }

    fn start_staging(
        &mut self,
        prepared: PreparedAttachment,
        state: &Entity<AppState>,
        bridge: &Bridge,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(host) = &self.host {
            self.notice(format!("Copying {} to {}…", prepared.name, host.name), cx);
        }
        let id = self.next_attachment_id;
        self.next_attachment_id = self.next_attachment_id.wrapping_add(1);
        let weak_view = cx.entity().downgrade();
        let uploads = media::stage(
            state,
            bridge,
            MediaAnchor::Thread {
                thread: self.thread,
            },
            prepared.attachment,
            true,
            move |outcomes, cx| {
                weak_view
                    .update(cx, |view, cx| view.complete_attachment(id, outcomes, cx))
                    .ok();
            },
            cx,
        );
        let Some(upload) = uploads.first().copied() else {
            self.notice("The attachment could not be copied.", cx);
            return;
        };
        self.pending_attachments.push(PendingAttachment {
            id,
            upload,
            name: prepared.name,
            media_type: prepared.media_type,
            state: PendingAttachmentState::Staging(0.0),
            path: None,
        });
    }

    fn complete_attachment(
        &mut self,
        id: usize,
        outcomes: Vec<StageOutcome>,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(attachment) = self
            .pending_attachments
            .iter_mut()
            .find(|attachment| attachment.id == id)
        else {
            // The chip was removed while the registry task was finishing; its late answer has no
            // composer owner and must not recreate it.
            return;
        };
        let result = outcomes
            .into_iter()
            .find(|outcome| outcome.upload == attachment.upload)
            .map_or_else(
                || Err("The attachment copy returned no result.".to_owned()),
                |outcome| outcome.result,
            );
        match result {
            Ok(path) => {
                attachment.path = Some(path);
                attachment.state = PendingAttachmentState::Ready;
            }
            Err(message) => {
                attachment.path = None;
                attachment.state = PendingAttachmentState::Failed(message.into());
            }
        }
        cx.notify();
    }

    /// Copies upload-registry progress into the prepared chip model from a Workspace update.
    pub(crate) fn refresh_attachment_progress(
        &mut self,
        app: &AppState,
        cx: &mut gpui::Context<Self>,
    ) {
        let mut changed = false;
        for attachment in &mut self.pending_attachments {
            let Some(progress) = media::progress(app, attachment.upload) else {
                continue;
            };
            let next = match progress.state {
                UploadState::Waiting => PendingAttachmentState::Waiting,
                UploadState::Sending | UploadState::Finishing => {
                    let ratio = if progress.total == 0 {
                        f32::from(progress.state == UploadState::Finishing)
                    } else {
                        progress.sent as f32 / progress.total as f32
                    };
                    PendingAttachmentState::Staging(ratio.clamp(0.0, 1.0))
                }
                UploadState::Failed => {
                    PendingAttachmentState::Failed("The attachment could not be copied.".into())
                }
            };
            if attachment.state != next {
                attachment.state = next;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Removes a chip immediately and asks the Workspace to cancel only a live copy.
    pub(crate) fn remove_pending_attachment(&mut self, id: usize, cx: &mut gpui::Context<Self>) {
        let Some(index) = self
            .pending_attachments
            .iter()
            .position(|attachment| attachment.id == id)
        else {
            return;
        };
        let attachment = self.pending_attachments.remove(index);
        if matches!(
            attachment.state,
            PendingAttachmentState::Staging(_) | PendingAttachmentState::Waiting
        ) {
            cx.emit(AgentThreadEvent::CancelMedia(attachment.upload));
        }
        cx.notify();
    }

    pub(super) fn ready_attachment_count(&self) -> usize {
        self.pending_attachments
            .iter()
            .filter(|attachment| {
                attachment.path.is_some()
                    && matches!(attachment.state, PendingAttachmentState::Ready)
            })
            .count()
    }

    pub(super) fn copying_attachment_name(&self) -> Option<&str> {
        self.pending_attachments
            .iter()
            .find_map(|attachment| {
                matches!(
                    attachment.state,
                    PendingAttachmentState::Staging(_) | PendingAttachmentState::Waiting
                )
                .then_some(attachment.name.as_str())
            })
            .or_else(|| {
                self.attachment_preflights
                    .first()
                    .map(|preflight| preflight.name.as_str())
            })
    }

    pub(super) fn failed_attachment_names(&self) -> Vec<String> {
        self.pending_attachments
            .iter()
            .filter(|attachment| matches!(attachment.state, PendingAttachmentState::Failed(_)))
            .map(|attachment| attachment.name.clone())
            .collect()
    }

    pub(super) fn take_ready_attachments(&mut self) -> Vec<PendingAttachment> {
        let attachments = std::mem::take(&mut self.pending_attachments);
        attachments
            .into_iter()
            .filter(|attachment| {
                attachment.path.is_some()
                    && matches!(attachment.state, PendingAttachmentState::Ready)
            })
            .collect()
    }

    pub(super) fn restore_dispatched_attachments(&mut self, item: fleet_core::agents::ItemId) {
        let Some(mut attachments) = self.dispatched_attachments.remove(&item) else {
            return;
        };
        attachments.append(&mut self.pending_attachments);
        self.pending_attachments = attachments;
    }

    pub(super) fn cancel_pending_attachments(&mut self, cx: &mut gpui::Context<Self>) {
        for attachment in self.pending_attachments.drain(..) {
            if matches!(
                attachment.state,
                PendingAttachmentState::Staging(_) | PendingAttachmentState::Waiting
            ) {
                cx.emit(AgentThreadEvent::CancelMedia(attachment.upload));
            }
        }
        self.attachment_preflights.clear();
    }

    #[cfg(test)]
    pub(crate) fn test_pending_attachments(&self) -> &[PendingAttachment] {
        &self.pending_attachments
    }

    #[cfg(test)]
    pub(crate) fn test_complete_attachment(
        &mut self,
        id: usize,
        path: std::path::PathBuf,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(upload) = self
            .pending_attachments
            .iter()
            .find(|attachment| attachment.id == id)
            .map(|attachment| attachment.upload)
        else {
            return;
        };
        self.complete_attachment(
            id,
            vec![StageOutcome {
                upload,
                result: Ok(path),
            }],
            cx,
        );
    }

    #[cfg(test)]
    pub(crate) fn test_add_ready_attachment(&mut self, name: &str, path: std::path::PathBuf) {
        let id = self.next_attachment_id;
        self.next_attachment_id = self.next_attachment_id.wrapping_add(1);
        self.pending_attachments.push(PendingAttachment {
            id,
            upload: fleet_proto::request::UploadId::new(),
            name: name.to_owned(),
            media_type: media::media_type_for_path(&path).to_owned(),
            state: PendingAttachmentState::Ready,
            path: Some(path),
        });
    }
}
