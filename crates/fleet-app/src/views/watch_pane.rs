//! Read-only watch split and foreground catch-up orchestration.

use crate::{bridge::Bridge, state::AppState, watches::WatchMirror};
use fleet_core::{
    ids::SessionId,
    watches::{WatchId, WatchSource, WatchStatus},
};
use fleet_proto::{error::ErrorKind, request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{ActiveTheme, Icon, KeyHint, LogView, StatusDot, Text, Tone, Truncate};
use gpui::{
    AnyElement, App, Div, Entity, SharedString, Stateful, UniformListScrollHandle, div, prelude::*,
};
use std::time::Instant;

mod controller;
pub(crate) use controller::{dismiss_selected, sync};

/// How many characters of a watch label survive before it is ellipsized.
const LABEL_BUDGET: usize = 24;

/// Header status text and its existing semantic tone.
#[must_use]
fn status(status: &WatchStatus) -> (SharedString, Tone) {
    match status {
        WatchStatus::Running => ("running".into(), Tone::Warning),
        WatchStatus::Exited {
            code: Some(code), ..
        } => (
            format!("exited {code}").into(),
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

/// The identity line: label, live status, elapsed time, and the two ways to dismiss it.
fn header(mirror: &WatchMirror, state: &Entity<AppState>, bridge: &Bridge, cx: &App) -> Div {
    let theme = cx.theme();
    let (label, tone) = status(&mirror.watch.status);
    let (close_state, close_bridge) = (state.clone(), bridge.clone());
    div()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .h(theme.metrics.row_h)
        .flex_none()
        .px(theme.space.md)
        .child(
            div().flex_1().min_w_0().overflow_hidden().child(
                Text::ui_strong(display_label(mirror.watch.source, &mirror.watch.label))
                    .truncate_at(LABEL_BUDGET, Truncate::Tail),
            ),
        )
        .child(StatusDot::new(tone))
        .child(Text::data_small(label).tone(tone))
        .child(Text::data_small(elapsed(mirror)).tone(Tone::Secondary))
        .child(KeyHint::new("^s v"))
        .child(
            div()
                .id("watch-close")
                .cursor_pointer()
                .px(theme.space.sm)
                .hover(|s| s.bg(theme.colors.row_hover))
                .on_click(move |_, _, cx| dismiss_selected(&close_state, &close_bridge, cx))
                .child(Icon::X.el().color(Tone::Secondary.color(theme))),
        )
}

/// One tab per watch of the session, shown only once there is a choice to make.
fn tabs(
    session: &SessionId,
    selected: WatchId,
    state: &Entity<AppState>,
    cx: &App,
) -> Option<Stateful<Div>> {
    let theme = cx.theme();
    let app = state.read(cx);
    let ids = app.watches.ids(session);
    if ids.len() < 2 {
        return None;
    }
    let mut strip = div()
        .id("watch-tabs")
        .flex()
        .flex_none()
        .overflow_x_scroll()
        .px(theme.space.sm)
        .gap(theme.space.xs);
    for id in ids {
        let entry = &app.watches.entries[&id];
        let (state, session) = (state.clone(), session.clone());
        let (_, tone) = status(&entry.watch.status);
        strip = strip.child(
            div()
                .id(("watch-tab", id.0))
                .cursor_pointer()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.space.xs)
                .px(theme.space.sm)
                .py(theme.space.xs)
                // Hover is pointer feedback only (§3): it never paints over the selection
                // background of the tab the keyboard already put the cursor on.
                .when(id == selected, |el| el.bg(theme.colors.row_selected))
                .when(id != selected, |el| {
                    el.hover(|s| s.bg(theme.colors.row_hover))
                })
                .on_click(move |_, _, cx| {
                    state.update(cx, |app, cx| {
                        app.watches.select(&session, id);
                        cx.notify();
                    });
                })
                .child(StatusDot::new(tone))
                .child(
                    Text::data_small(display_label(entry.watch.source, &entry.watch.label))
                        .truncate_at(LABEL_BUDGET, Truncate::Tail),
                ),
        );
    }
    Some(strip)
}

/// A non-focusable following log, with the only pointer actions on close and tabs.
pub(crate) fn render(
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
    let (lines, tones) = mirror.shared_display();
    let log = LogView::from_shared(("watch-output", selected.0), lines)
        .shared_line_tones(tones)
        .following(true)
        .track_scroll(scroll)
        .show_badge(false)
        .empty("waiting for output\u{2026}");
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
            .child(header(mirror, state, bridge, cx))
            .children(tabs(session, selected, state, cx))
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
pub(crate) fn toggle(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.toggle_watch_pane(Instant::now());
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_labels_have_the_quiet_marker() {
        assert_eq!(display_label(WatchSource::Discovered, "codex"), "◦ codex");
        assert_eq!(display_label(WatchSource::Cooperative, "codex"), "codex");
    }

    #[test]
    fn dismiss_failure_is_persistent() {
        use fleet_core::watches::{Watch, WatchStatus};
        use fleet_proto::error::ProtoError;

        let watch = Watch {
            id: WatchId(7),
            session: "acme/api#fix".parse().expect("session id"),
            terminal: fleet_core::ids::TerminalId(1),
            label: "codex".into(),
            command: vec!["codex".into()],
            cwd: None,
            pid: Some(123),
            started_at: "2026-09-04T12:00:00Z".into(),
            status: WatchStatus::Exited {
                code: Some(0),
                signal: None,
            },
            source: WatchSource::Cooperative,
            log_file: None,
        };
        let mut app = AppState::new("/tmp/fleet-watch-dismiss", Instant::now());
        app.watches.started(watch.clone(), Instant::now());

        controller::apply_dismiss_reply(
            &mut app,
            watch.id,
            Ok(Err(ProtoError {
                kind: ErrorKind::Conflict,
                message: "watch dismissal refused".into(),
            })),
        );

        assert!(app.watches.entries.contains_key(&watch.id));
        assert_eq!(
            app.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("watch dismissal refused")
        );
        assert!(app.toasts.is_empty());
    }
}
