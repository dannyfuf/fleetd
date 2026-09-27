//! Local media delivery for path-bearing dialog inputs.

use std::{path::PathBuf, time::Instant};

use fleet_proto::{event::ToastLevel, request::MediaAnchor};
use fleet_ui_kit::{TextInput, TextInputMedia};
use gpui::{App, Entity};

use crate::{bridge::Bridge, media, state::AppState};

/// How a field consumes the local paths returned by staging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PathInsertion {
    /// A filesystem value consumed directly rather than parsed by a shell.
    Bare,
    /// One or more shell words in a command field.
    ShellQuoted,
}

/// Retains the media-event half of a path-bearing dialog input's wiring.
pub(super) fn subscribe(
    input: &Entity<TextInput>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    insertion: PathInsertion,
    cx: &mut App,
) -> gpui::Subscription {
    let weak_input = input.downgrade();
    let weak_state = state.downgrade();
    let bridge = bridge.clone();
    cx.subscribe(input, move |_input, event: &TextInputMedia, cx| {
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        let attachment = match event {
            TextInputMedia::Pasted(item) => {
                let Some(attachment) = media::from_clipboard(item) else {
                    return;
                };
                attachment
            }
            TextInputMedia::Dropped(paths) => media::from_external_paths(paths),
        };
        let delivery_state = state.downgrade();
        let delivery_input = weak_input.clone();
        drop(media::stage(
            &state,
            &bridge,
            MediaAnchor::Local,
            attachment,
            false,
            move |outcomes, cx| {
                let mut paths = Vec::new();
                let mut failures = Vec::new();
                for outcome in outcomes {
                    match outcome.result {
                        Ok(path) => paths.push(path),
                        Err(message) => failures.push(message),
                    }
                }
                // A local path can complete synchronously while `TextInput` is emitting the
                // media event. Leave that entity's update stack before inserting at its caret.
                cx.defer(move |cx| {
                    if !failures.is_empty()
                        && let Some(state) = delivery_state.upgrade()
                    {
                        state.update(cx, |app, cx| {
                            app.apply_toast_event(
                                ToastLevel::Error,
                                failures.join("; "),
                                Instant::now(),
                            );
                            cx.notify();
                        });
                    }
                    if paths.is_empty() {
                        return;
                    }
                    let text = insertion.text(&paths);
                    delivery_input
                        .update(cx, |input, cx| input.insert(&text, cx))
                        .ok();
                });
            },
            cx,
        ));
    })
}

impl PathInsertion {
    fn text(self, paths: &[PathBuf]) -> String {
        match self {
            Self::Bare => paths
                .iter()
                .map(|path| path.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            Self::ShellQuoted => media::insert_text(paths),
        }
    }
}
