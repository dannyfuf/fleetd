//! Read-only watch split and foreground catch-up orchestration.

use crate::{bridge::Bridge, state::AppState, watches::WatchMirror};
use fleet_core::{
    ids::SessionId,
    watches::{WatchSource, WatchStatus, WatchStream},
};
use fleet_proto::{error::ErrorKind, request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{ActiveTheme, Icon, KeyHint, LogView, Text, Tone, Truncate};
use gpui::{AnyElement, App, Entity, SharedString, UniformListScrollHandle, div, prelude::*};
use std::time::Instant;

/// Requests session catch-up on entry/reconnect and repairs event sequence gaps.
pub fn sync(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if state.read(cx).drops_terminal_keys() {
        return;
    }
    let generation = state.read(cx).link_generation;
    let session = state.read(cx).active_session().map(|s| s.id.clone());
    let list = state.update(cx, |app, _| app.watches.enter(session, generation));
    if let Some(session) = list {
        let known = state.read(cx).watches.ids(&session);
        let reply = bridge.request(RequestBody::ListWatches {
            session: session.clone(),
        });
        let state = state.clone();
        cx.spawn(async move |cx| {
            let Ok(Ok(ResponseBody::Watches(watches))) = reply.recv().await else {
                return;
            };
            state.update(cx, |app, cx| {
                if app.link_generation == generation {
                    app.watches.listed(&session, known, watches, Instant::now());
                    cx.notify();
                }
            });
        })
        .detach();
    }
    let tails = state.update(cx, |app, _| app.watches.take_tails());
    for (watch, from_seq) in tails {
        let reply = bridge.request(RequestBody::TailWatch { watch, from_seq });
        let state = state.clone();
        cx.spawn(async move |cx| {
            let response = reply.recv().await;
            state.update(cx, |app, cx| {
                if app.link_generation != generation {
                    return;
                }
                match response {
                    Ok(Ok(ResponseBody::WatchTail(tail))) => {
                        app.watches.tailed(tail, Instant::now())
                    }
                    Ok(Err(error)) if error.kind == ErrorKind::NotFound => {
                        app.watches.dismissed(watch)
                    }
                    _ => app.watches.tail_failed(watch),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Header status text and its existing semantic tone.
#[must_use]
pub fn status(status: &WatchStatus) -> (String, Tone) {
    match status {
        WatchStatus::Running => ("running".into(), Tone::Warning),
        WatchStatus::Exited {
            code: Some(code), ..
        } => (
            format!("exited {code}"),
            if *code == 0 {
                Tone::Success
            } else {
                Tone::Danger
            },
        ),
        WatchStatus::Exited {
            signal: Some(_), ..
        } => ("interrupted".into(), Tone::Warning),
        WatchStatus::Exited { .. } => ("exited".into(), Tone::Secondary),
    }
}
fn elapsed(mirror: &WatchMirror) -> String {
    let seconds = mirror.elapsed(Instant::now()).as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn display_label(source: WatchSource, label: &str) -> String {
    if source == WatchSource::Discovered {
        format!("◦ {label}")
    } else {
        label.to_owned()
    }
}

/// A non-focusable following log, with the only pointer actions on close and tabs.
pub fn render(
    session: &SessionId,
    state: &Entity<AppState>,
    bridge: &Bridge,
    scroll: &UniformListScrollHandle,
    cx: &App,
) -> Option<AnyElement> {
    let app = state.read(cx);
    let pane = app.watches.panes.get(session).filter(|p| p.visible)?;
    let selected = pane.selected?;
    let mirror = app.watches.entries.get(&selected)?;
    let theme = cx.theme();
    let (label, tone) = status(&mirror.watch.status);
    let close_state = state.clone();
    let close_bridge = bridge.clone();
    let close = div()
        .id("watch-close")
        .cursor_pointer()
        .px(theme.space.sm)
        .hover(|s| s.bg(theme.colors.row_hover))
        .on_click(move |_, _, cx| dismiss_selected(&close_state, &close_bridge, cx))
        .child(Text::ui("×").tone(Tone::Secondary));
    let header = div()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .h(theme.metrics.row_h)
        .flex_none()
        .px(theme.space.md)
        .child(
            div().flex_1().min_w_0().overflow_hidden().child(
                Text::ui_strong(display_label(mirror.watch.source, &mirror.watch.label))
                    .truncate_at(24, Truncate::Tail),
            ),
        )
        .child(Text::data_small("●").tone(tone))
        .child(Text::data_small(label).tone(tone))
        .child(Text::data_small(elapsed(mirror)).tone(Tone::Secondary))
        .child(KeyHint::new("^s v"))
        .child(close);
    let ids = app.watches.ids(session);
    let tabs = (ids.len() > 1).then(|| {
        let mut tabs = div()
            .id("watch-tabs")
            .flex()
            .flex_none()
            .overflow_x_scroll()
            .px(theme.space.sm)
            .gap(theme.space.xs);
        for id in ids {
            let entry = &app.watches.entries[&id];
            let state = state.clone();
            let session = session.clone();
            let (_, tone) = status(&entry.watch.status);
            tabs = tabs.child(
                div()
                    .id(("watch-tab", id.0))
                    .cursor_pointer()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.xs)
                    .px(theme.space.sm)
                    .py(theme.space.xs)
                    .when(id == selected, |el| el.bg(theme.colors.row_selected))
                    .hover(|s| s.bg(theme.colors.row_hover))
                    .on_click(move |_, _, cx| {
                        state.update(cx, |app, cx| {
                            app.watches.select(&session, id);
                            cx.notify();
                        });
                    })
                    .child(Text::data_small("●").tone(tone))
                    .child(
                        Text::data_small(display_label(entry.watch.source, &entry.watch.label))
                            .truncate_at(24, Truncate::Tail),
                    ),
            );
        }
        tabs
    });
    let lines: Vec<_> = mirror.display_lines().collect();
    let tones = lines
        .iter()
        .map(|line| {
            if line.stream == WatchStream::Stderr {
                Tone::Secondary
            } else {
                Tone::Default
            }
        })
        .collect::<Vec<_>>();
    let log = LogView::new(
        ("watch-output", selected.0),
        lines.into_iter().map(|line| SharedString::from(line.text)),
    )
    .line_tones(tones)
    .following(true)
    .track_scroll(scroll)
    .show_badge(false)
    .empty("waiting for output…");
    Some(
        div()
            .id("watch-pane")
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.colors.surface)
            .child(header)
            .children(tabs)
            .when(mirror.trimmed, |el| {
                el.child(
                    div()
                        .px(theme.space.md)
                        .child(Text::data_small("older output trimmed").tone(Tone::Secondary)),
                )
            })
            .child(div().flex_1().min_h_0().overflow_hidden().child(log))
            .into_any_element(),
    )
}

/// `^s v`: visibility only; does not cycle or mutate daemon state.
pub fn toggle(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.toggle_watch_pane(Instant::now());
        cx.notify();
    });
}

/// `^s V` and ×: dismiss a finished watch, or locally hide a running one.
pub fn dismiss_selected(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let selected = state.update(cx, |app, cx| {
        let selected = app.close_selected_watch(Instant::now());
        cx.notify();
        selected
    });
    let Some(watch) = selected else {
        return;
    };
    let generation = state.read(cx).link_generation;
    let reply = bridge.request(RequestBody::DismissWatch { watch });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let response = reply.recv().await;
        state.update(cx, |app, cx| {
            if app.link_generation != generation {
                return;
            }
            match response {
                Ok(Ok(ResponseBody::Ack)) => app.watches.dismissed(watch),
                Ok(Err(error)) if error.kind == ErrorKind::NotFound => app.watches.dismissed(watch),
                Ok(Err(error)) => app.toast_short(error.message, Icon::Info, Instant::now()),
                _ => app.toast_short("could not dismiss watch", Icon::Info, Instant::now()),
            }
            cx.notify();
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_labels_have_the_quiet_marker() {
        assert_eq!(display_label(WatchSource::Discovered, "codex"), "◦ codex");
        assert_eq!(display_label(WatchSource::Cooperative, "codex"), "codex");
    }
}
