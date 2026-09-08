//! Context and status bars with independent state/theme invalidation.

use fleet_ui_kit::{ActiveTheme, Chip, ContextBar, ContextTab, Icon, StatusBar, Theme, Tone};
use gpui::{AnyElement, App, Entity, IntoElement, Render, SharedString, Subscription, Window};

use crate::{
    screens::hub::effective_context,
    shell::daemon::{dot_label, dot_state},
    state::{AppState, ChipCounts, RepoScope, breadcrumb},
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
    // §3.3: the agent counters are one vocabulary with the tab badge and the session header,
    // and they include the thread on the current tab.
    for (icon, label, tone) in agent_segments(state.agents.counts()) {
        let mut chip = Chip::labeled(icon, label).tone(tone);
        if icon == Icon::LoaderCircle {
            chip = chip.id("context-bar-agents-working").spinning(true);
        }
        bar = bar.chip(chip);
    }
    if let Some(version) = &state.update_version {
        bar = bar.chip(Chip::labeled(
            Icon::CircleArrowUp,
            format!("\u{2191}{version}"),
        ));
    }
    bar.into_any_element()
}

/// The §3.3 agent segments of the context bar, in `needs you · working · failed` order.
///
/// A zero segment is omitted. §1.2's zero-suppression cannot reach these on its own: the count
/// lives inside the chip's *word* (`3 needs you`), and [`Chip::zero_suppress`] only drops a bare
/// `Some(0)` count — so the bar read `0 needs you` in amber next to `0 failed` in red while
/// nothing needed anyone. Colour is semantic (§1.4); a count of nothing has no semantics to
/// paint, so it does not get a segment at all.
#[must_use]
fn agent_segments(counts: crate::state::AgentCounts) -> Vec<(Icon, String, Tone)> {
    [
        (Icon::Bot, "needs you", Tone::Warning, counts.needs_you),
        (
            Icon::LoaderCircle,
            "working",
            Tone::Secondary,
            counts.working,
        ),
        (Icon::CircleX, "failed", Tone::Danger, counts.failed),
    ]
    .into_iter()
    .filter(|(_, _, _, count)| *count > 0)
    .map(|(icon, word, tone, count)| (icon, format!("{count} {word}"), tone))
    .collect()
}

/// The §9 key hints of the active agent tab, which the status bar mirrors.
#[must_use]
fn agent_key_hints(state: &AppState) -> Option<fleet_ui_kit::KeyHintRow> {
    use crate::screens::agent_thread::presentation::{decision_hints, key_hints};
    use fleet_core::agents::AgentKind;

    // §9: the floating fallback shadows every workspace binding while it is up — the mode word
    // already reads `TERMINAL` — so the bar mirrors the popup's keys, not the tab's.
    if state.agent_popup.is_some() {
        return Some(crate::screens::agent_popup::chrome::key_hints());
    }
    let thread = state.active_agent_thread()?;
    let projection = state.agents.projection(thread);
    let gate = projection.and_then(|projection| projection.gates.last());
    // §9: while a correction or a plan note is being typed the card's keys stand down, so the
    // status bar must not keep advertising `y allow once` at a composer that owns the letters.
    if let Some(gate) = gate.filter(|_| !state.agents.is_composing(thread)) {
        let provider = projection.map_or(AgentKind::Claude, |projection| projection.provider);
        return Some(decision_hints(
            gate,
            provider,
            state.agents.question_cursor(thread),
        ));
    }
    // §9 and DESIGN-SYSTEM §4: the bar mirrors the keys that actually fire, so it reads the
    // same working predicate `agent_context_chain` picks the key context from.
    Some(key_hints(state.agents.is_working(thread)))
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
    let row = state.breadcrumb_row.as_deref().unwrap_or_default();
    breadcrumb(&[context, repo, row])
}

/// The 26 px status bar (§2.2): breadcrumb · mode word · job ticker · sticky error slot.
#[must_use]
fn status_bar(state: &AppState) -> AnyElement {
    let mut bar = StatusBar::new()
        .breadcrumb(SharedString::from(breadcrumb_text(state)))
        .mode(state.mode().word());
    // §9: an agent tab advertises its own key set, which changes with the open decision card.
    if let Some(hints) = agent_key_hints(state) {
        bar = bar.trailing(hints);
    }

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

    fn one_context_snapshot(active: bool) -> fleet_proto::snapshot::Snapshot {
        let id: fleet_core::ids::ContextId =
            "acme".parse().unwrap_or_else(|error| panic!("{error}"));
        fleet_proto::snapshot::Snapshot {
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
            agent_threads: Vec::new(),
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
    fn the_status_bar_mirrors_the_popups_keys_while_the_popup_is_up() {
        let now = std::time::Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        assert!(agent_key_hints(&state).is_none());

        state.toggle_agent_popup(fleet_core::config::Agent::Claude, None);
        assert_eq!(state.mode().word().word(), "TERMINAL");
        let hints = agent_key_hints(&state).expect("the popup advertises its own keys");
        assert_eq!(
            hints.pairs(),
            crate::screens::agent_popup::chrome::key_hints().pairs(),
            "§9: the bar names the keys that actually fire, and while the popup owns the \
             keyboard those are the popup's — not `⏎ send · ⇧⇥ plan mode · …`"
        );

        state.hide_agent_popup();
        assert!(agent_key_hints(&state).is_none());
    }

    #[test]
    fn the_agent_segments_omit_every_count_of_nothing() {
        use crate::state::AgentCounts;

        assert!(agent_segments(AgentCounts::default()).is_empty());
        let some = AgentCounts {
            needs_you: 3,
            working: 0,
            failed: 1,
        };
        assert_eq!(
            agent_segments(some)
                .into_iter()
                .map(|(_, label, tone)| (label, tone))
                .collect::<Vec<_>>(),
            vec![
                ("3 needs you".to_owned(), Tone::Warning),
                ("1 failed".to_owned(), Tone::Danger),
            ],
            "§2's example is `3 needs you · 2 working · 1 failed`; a zero segment is not in it"
        );
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
}
