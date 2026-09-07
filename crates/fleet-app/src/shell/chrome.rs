//! Context and status bars with independent state/theme invalidation.

use fleet_ui_kit::{ActiveTheme, Chip, ContextBar, ContextTab, Icon, StatusBar, Theme, Tone};
use gpui::{AnyElement, App, Entity, IntoElement, Render, SharedString, Subscription, Window};

use crate::{
    dialogs::Dialogs,
    screens::hub::effective_context,
    shell::daemon::{dot_label, dot_state},
    state::{AppState, ChipCounts, HubTab, Overlay, RepoScope, Screen, breadcrumb},
    views::{job_ticker, sticky_error},
};

/// The 36 px context bar (§2.1, §2.3, §3.1).
#[must_use]
fn context_bar(state: &AppState, cx: &App) -> AnyElement {
    let contexts = state
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.contexts.as_slice())
        .unwrap_or_default();
    let active = effective_context(state)
        .and_then(|active| contexts.iter().position(|context| context.id == active.id))
        .unwrap_or(0);
    let tabs = contexts.iter().take(9).enumerate().map(|(index, context)| {
        ContextTab::new(SharedString::new(context.name.as_str()), index + 1)
    });
    let overflow = contexts.len().saturating_sub(9);

    let counts = state.snapshot.as_ref().map_or_else(
        || ChipCounts {
            review: state.review_pr_count,
            ..ChipCounts::default()
        },
        |snapshot| ChipCounts::from_snapshot(snapshot, state.review_pr_count),
    );
    #[cfg(target_os = "macos")]
    let leading_inset = cx.theme().metrics.traffic_light_inset;
    #[cfg(not(target_os = "macos"))]
    let leading_inset = cx.theme().space.md;

    let mut bar = ContextBar::new(tabs)
        .active(active)
        .overflow(overflow)
        .leading_inset(leading_inset)
        .daemon(dot_state(&state.daemon));
    if let Some(label) = dot_label(&state.daemon) {
        bar = bar.daemon_label(label);
    }
    if contexts.is_empty() {
        let (fact, action) = crate::views::first_run::EmptySurface::Contexts.copy(None);
        bar = bar.empty(fact, action);
    }

    // §2.3: the failed count replaces the jobs chip's color, it is never a second chip.
    let jobs_chip = if counts.failed > 0 {
        Chip::counter(Icon::TriangleAlert, counts.failed).tone(Tone::Danger)
    } else {
        Chip::counter(Icon::LoaderCircle, counts.running)
            .tone(Tone::Warning)
            .spinning(true)
            .id("context-bar-jobs")
    };

    bar = bar
        .chip(jobs_chip)
        .chip(Chip::counter(Icon::CircleDot, counts.live).tone(Tone::Success))
        .chip(Chip::counter(Icon::Moon, counts.sleeping))
        .chip(Chip::counter(Icon::CircleQuestionMark, counts.unknown).tone(Tone::Warning))
        .chip(Chip::counter(Icon::Flag, counts.review));
    if let Some(version) = &state.update_version {
        bar = bar.chip(Chip::labeled(
            Icon::CircleArrowUp,
            format!("\u{2191}{version}"),
        ));
    }
    bar.into_any_element()
}

/// The status-bar breadcrumb `context › repo › row` (§2.2).
#[must_use]
fn breadcrumb_text(state: &AppState) -> String {
    let context = effective_context(state)
        .map(|context| context.name.as_str())
        .unwrap_or_default();
    let repo = match &state.scope {
        RepoScope::All => "",
        RepoScope::Repo(repo) => repo.name(),
    };
    // The board's row is derived here, not cached by its render: the status bar is built before
    // the body, so a row written during the body's render names the previously selected card.
    let row = if matches!(state.screen, Screen::Hub { tab: HubTab::Board }) {
        board_row(state)
    } else {
        state.breadcrumb_row.clone().unwrap_or_default()
    };
    breadcrumb(&[context, repo, &row])
}

/// The board's breadcrumb row: the focused card, unless the surface is not about a card.
///
/// Board settings edits the *board*, so naming a card there points the reader at the one thing
/// the dialog cannot change. The board's name would be no better — it defaults to the context's,
/// which the breadcrumb already says — so the row is simply dropped and `breadcrumb` suppresses
/// it.
fn board_row(state: &AppState) -> String {
    if matches!(state.overlay, Some(Overlay::Dialog(Dialogs::BoardSettings))) {
        return String::new();
    }
    crate::screens::board::selected_card(state)
        .map(|card| {
            state
                .board()
                .map_or_else(|| card.title.clone(), |view| card.display_key(&view.board))
        })
        .unwrap_or_default()
}

/// The 26 px status bar (§2.2): breadcrumb · mode word · job ticker · sticky error slot.
#[must_use]
fn status_bar(state: &AppState) -> AnyElement {
    let mut bar = StatusBar::new()
        .breadcrumb(SharedString::from(breadcrumb_text(state)))
        .mode(state.mode().word());

    let jobs = state
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.jobs.as_slice())
        .unwrap_or_default();
    match job_ticker::status_slot(jobs, state.sticky_error.as_ref()) {
        job_ticker::StatusSlot::Error(error) => {
            bar = bar.error(sticky_error::render(&error, &state.screen));
        }
        job_ticker::StatusSlot::Ticker(content) => {
            bar = bar.ticker(job_ticker::render(&content));
        }
        job_ticker::StatusSlot::Idle => {}
    }
    bar.into_any_element()
}

/// Both bars have definite AppFrame geometry. All non-frame state notifications and theme
/// changes invalidate them; title-bearing terminal frames also take that state path.
pub(super) struct Chrome {
    state: Entity<AppState>,
    kind: ChromeKind,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
pub(super) enum ChromeKind {
    Context,
    Status,
}

impl Chrome {
    pub(super) fn new(
        state: Entity<AppState>,
        kind: ChromeKind,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.observe_global::<Theme>(|_, cx| cx.notify()),
        ];
        Self {
            state,
            kind,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for Chrome {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        match self.kind {
            ChromeKind::Context => context_bar(self.state.read(cx), cx),
            ChromeKind::Status => status_bar(self.state.read(cx)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::model::Context;
    use fleet_proto::job::JobKind;

    fn one_context_snapshot(active: bool) -> fleet_proto::snapshot::Snapshot {
        let id: fleet_core::ids::ContextId =
            "acme".parse().unwrap_or_else(|error| panic!("{error}"));
        fleet_proto::snapshot::Snapshot {
            boards: Vec::new(),
            generated_at: "2026-09-04T12:00:00Z".to_owned(),
            contexts: vec![Context {
                id: id.clone(),
                name: "acme".to_owned(),
                owners: vec!["acme".to_owned()],
                created_at: "2026-09-04T09:00:00Z".to_owned(),
            }],
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: active.then_some(id),
            sessions: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: "0.1.0".to_owned(),
                pid: 1,
                started_at: "2026-09-04T09:00:00Z".to_owned(),
                home: "/tmp/fleet".to_owned(),
            },
        }
    }

    #[test]
    fn the_breadcrumb_names_the_context_the_bar_underlines() {
        let now = std::time::Instant::now();
        for active in [true, false] {
            let mut state = AppState::new("/tmp/fleet", now);
            state.apply_snapshot(one_context_snapshot(active), now);
            state.breadcrumb_row = Some("feature-one".to_owned());
            state.scope = RepoScope::Repo(
                "acme/widgets"
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
            );
            assert_eq!(
                breadcrumb_text(&state),
                "acme \u{203a} widgets \u{203a} feature-one",
                "§2.2 wants `context › repo › row`; an unset `active_context` is still the \
                 first tab, which is what the bar underlines (active_context set: {active})"
            );
        }
    }

    #[test]
    fn the_board_breadcrumb_names_the_card_that_is_selected_right_now() {
        let now = std::time::Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_snapshot(one_context_snapshot(true), now);
        state.screen = Screen::Hub { tab: HubTab::Board };
        let context = Context {
            id: "acme".parse().unwrap_or_else(|error| panic!("{error}")),
            name: "acme".to_owned(),
            owners: Vec::new(),
            created_at: "2026-09-04T09:00:00Z".to_owned(),
        };
        let mut board = fleet_core::board::new_board(&context, &context.created_at);
        let mut cards = Vec::new();
        for (index, title) in ["First", "Second"].iter().enumerate() {
            let card = fleet_core::board::create_card(
                &mut board,
                &cards,
                format!("card-{index}")
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                fleet_core::board::CardDraft {
                    title: (*title).to_owned(),
                    ..Default::default()
                },
                &context.created_at,
            )
            .unwrap_or_else(|error| panic!("{error}"));
            cards.push(card);
        }
        let column = board
            .statuses
            .iter()
            .position(|status| status.id == cards[0].status_id)
            .unwrap_or_else(|| panic!("no column"));
        let keys: Vec<String> = cards.iter().map(|card| card.display_key(&board)).collect();
        state.board.view = Some(fleet_core::board::BoardView { board, cards });
        state.board.focus = crate::state::BoardFocus { column, row: 0 };
        // Stale by construction: the status bar is built before the body, so a row the board's
        // render caches always names the previously selected card.
        state.breadcrumb_row = Some("stale".to_owned());
        assert!(breadcrumb_text(&state).ends_with(&keys[0]), "{keys:?}");
        state.board.focus.row = 1;
        assert!(breadcrumb_text(&state).ends_with(&keys[1]), "{keys:?}");

        // A dialog about the card keeps naming it; the one about the board does not, because
        // the card is the one thing board settings cannot change.
        state.open_overlay(Overlay::Dialog(Dialogs::CardDetail));
        assert!(breadcrumb_text(&state).ends_with(&keys[1]), "{keys:?}");
        state.close_overlay();
        state.open_overlay(Overlay::Dialog(Dialogs::BoardSettings));
        assert_eq!(
            breadcrumb_text(&state),
            "acme",
            "board settings edits the board, so the breadcrumb names no card"
        );
        state.close_overlay();
        assert!(breadcrumb_text(&state).ends_with(&keys[1]), "{keys:?}");
    }

    #[test]
    fn job_kind_labels_are_single_words() {
        for kind in [
            JobKind::Clone,
            JobKind::PoolBuild,
            JobKind::CreateWorktree,
            JobKind::PrFetch,
            JobKind::Custom("thing".to_owned()),
        ] {
            let label = crate::presentation::job_kind_label(&kind);
            assert!(!label.is_empty());
            assert!(!label.contains(' '));
        }
    }
}
