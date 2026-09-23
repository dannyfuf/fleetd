//! The daemon surfaces of UX-SPEC §3.12: the cold-start splash, the "will not start" window
//! and the 40 px banner, plus the wording each of them is allowed to use.

use std::time::Instant;

use fleet_core::paths::FleetHome;
use fleet_ui_kit::{Banner, Button, ButtonStyle, DaemonSplash, DaemonState, Icon};
use gpui::{AnyElement, Entity, IntoElement, SharedString, prelude::*};

use crate::actions::daemon as daemon_actions;
use crate::state::{
    AppState, DaemonLink, DaemonLossReason, RECONNECT_BANNER_DWELL, RESTART_BANNER_DWELL,
    SPLASH_DETAIL_DELAY, reconnect_backoff,
};

/// What a restart did to the user's terminals, said out loud rather than guessed at.
///
/// Terminals normally survive — each PTY lives in a detached holder the new daemon reattaches to —
/// but not always: `pkill fleetd` matches `fleetd pty-hold` too, and nothing survives a reboot. The
/// banner is the one place the app states which happened, so it is counted, never assumed.
fn restart_sentence(reattached: usize) -> SharedString {
    match reattached {
        0 => SharedString::from(
            "fleetd restarted. No terminals survived; worktrees, jobs and state are intact.",
        ),
        1 => SharedString::from(
            "fleetd restarted. 1 terminal was reattached; worktrees, jobs and state are intact.",
        ),
        many => SharedString::from(format!(
            "fleetd restarted. {many} terminals were reattached; worktrees, jobs and state are \
             intact."
        )),
    }
}
/// The connected process predates the fleetd binary currently on disk.
const OUTDATED_SENTENCE: &str = "fleetd is outdated, restart with `fleet daemon restart`";

/// What a lost link promises: nothing the daemon owns went away with the window's connection.
const LOST_REASSURANCE: &str = "Your terminals and agents keep running.";

/// What the 40 px banner under the title bar says right now.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BannerSpec {
    /// The sentence. Owned because the restart sentence counts what actually came back.
    text: SharedString,
    /// The reconnect countdown, when one is running.
    countdown: Option<String>,
    /// The reassurance after the sentence.
    detail: Option<&'static str>,
    /// Whether the banner carries Reconnect now, Open log and its ✕: only while the link is
    /// lost, which is when `Daemon > Banner` binds their keys.
    actionable: bool,
}

/// The reconnect countdown of §3.12 C, derived from the absolute retry deadline.
#[must_use]
fn countdown_label(retry_at: Instant, now: Instant) -> String {
    let remaining = retry_at.saturating_duration_since(now);
    if remaining.is_zero() {
        return "reconnecting…".to_owned();
    }
    let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    format!("reconnecting in {seconds}s")
}

/// The banner for the current daemon state, or `None` when there is nothing to say.
///
/// A dismissed `Lost` banner returns `None`; the daemon dot stays red, which is exactly what
/// §3.12 asks for.
#[must_use]
fn banner_spec(
    daemon: &DaemonLink,
    daemon_since: Instant,
    outdated: bool,
    now: Instant,
) -> Option<BannerSpec> {
    match daemon {
        DaemonLink::Lost {
            attempt,
            dismissed: false,
            reason,
        } => Some(BannerSpec {
            text: SharedString::from(match reason {
                DaemonLossReason::ConnectionLost => "Lost connection to fleetd",
                DaemonLossReason::Stopped => "fleetd stopped",
            }),
            countdown: Some(format!(
                "\u{2014} {}.",
                countdown_label(daemon_since + reconnect_backoff(*attempt), now)
            )),
            detail: Some(LOST_REASSURANCE),
            actionable: true,
        }),
        DaemonLink::Connected if outdated => Some(BannerSpec {
            text: SharedString::from(OUTDATED_SENTENCE),
            countdown: None,
            detail: None,
            actionable: false,
        }),
        DaemonLink::Reconnected {
            restarted,
            reattached,
            since,
        } => {
            let dwell = if *restarted {
                RESTART_BANNER_DWELL
            } else {
                RECONNECT_BANNER_DWELL
            };
            if now.saturating_duration_since(*since) >= dwell {
                return None;
            }
            Some(BannerSpec {
                text: if *restarted {
                    restart_sentence(*reattached)
                } else {
                    SharedString::from("reconnected")
                },
                countdown: None,
                detail: None,
                actionable: false,
            })
        }
        DaemonLink::Lost {
            dismissed: true, ..
        }
        | DaemonLink::Starting
        | DaemonLink::Failed { .. }
        | DaemonLink::Connected => None,
    }
}

/// The daemon dot's state (§2.2). Healthy is a bare dot; anything else grows a word.
#[must_use]
pub(super) const fn dot_state(daemon: &DaemonLink) -> DaemonState {
    match daemon {
        DaemonLink::Connected | DaemonLink::Reconnected { .. } => DaemonState::Healthy,
        DaemonLink::Starting => DaemonState::Degraded,
        DaemonLink::Lost { .. } | DaemonLink::Failed { .. } => DaemonState::Lost,
    }
}

/// The word beside a degraded or lost dot.
#[must_use]
pub(super) const fn dot_label(daemon: &DaemonLink) -> Option<&'static str> {
    match daemon {
        DaemonLink::Starting => Some("starting"),
        DaemonLink::Lost { reason, .. } => Some(match reason {
            DaemonLossReason::ConnectionLost => "connection lost",
            DaemonLossReason::Stopped => "stopped",
        }),
        DaemonLink::Failed { .. } => Some("down"),
        DaemonLink::Connected | DaemonLink::Reconnected { .. } => None,
    }
}

/// Renders the banner, or nothing.
#[must_use]
pub(super) fn banner(
    daemon: &DaemonLink,
    daemon_since: Instant,
    outdated: bool,
    now: Instant,
) -> Option<AnyElement> {
    let spec = banner_spec(daemon, daemon_since, outdated, now)?;
    let mut banner = Banner::warning(spec.text).icon(Icon::Unplug);
    if let Some(countdown) = spec.countdown {
        banner = banner.countdown(countdown);
    }
    if let Some(detail) = spec.detail {
        banner = banner.detail(detail);
    }
    if spec.actionable {
        // Each button dispatches the action its key runs in `Daemon > Banner`, and shows that key
        // from the live keymap; the ✕ is the banner's `Esc`.
        banner = banner
            .button(
                Button::new("banner-reconnect", "Reconnect now")
                    .action(Box::new(daemon_actions::Reconnect)),
            )
            .button(
                Button::new("banner-open-log", "Open log")
                    .style(ButtonStyle::Ghost)
                    .action(Box::new(daemon_actions::OpenLog)),
            )
            .dismiss_action(Box::new(daemon_actions::DismissBanner));
    }
    Some(banner.into_any_element())
}

/// The full-window daemon surface of §3.12 A and B, or `None` when the daemon is reachable.
#[must_use]
pub(super) fn splash(
    state: &AppState,
    now: Instant,
    diagnostics: &Entity<crate::views::doctor_view::DiagnosticView>,
) -> Option<AnyElement> {
    match &state.daemon {
        DaemonLink::Starting => {
            let mut splash = DaemonSplash::starting("Starting fleetd…");
            if now.saturating_duration_since(state.daemon_since) >= SPLASH_DETAIL_DELAY {
                splash = splash.detail(SharedString::from(
                    FleetHome::new(&state.home)
                        .socket_path()
                        .display()
                        .to_string(),
                ));
            }
            Some(splash.into_any_element())
        }
        DaemonLink::Failed { .. } => Some(
            diagnostics
                .clone()
                .cached(gpui::StyleRefinement::default().size_full())
                .into_any_element(),
        ),
        DaemonLink::Connected | DaemonLink::Lost { .. } | DaemonLink::Reconnected { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::bridge::BridgeEvent;
    use fleet_proto::event::Event;

    use super::*;

    #[test]
    fn a_dismissed_banner_says_nothing_but_the_dot_stays_red() {
        let now = Instant::now();
        let lost = DaemonLink::Lost {
            attempt: 0,
            dismissed: true,
            reason: DaemonLossReason::ConnectionLost,
        };
        assert_eq!(banner_spec(&lost, now, false, now), None);
        assert_eq!(dot_state(&lost), DaemonState::Lost);
    }

    #[test]
    fn countdown_tracks_retry_deadline() {
        let now = Instant::now();
        let retry_at = now + Duration::from_millis(2_250);
        assert_eq!(countdown_label(retry_at, now), "reconnecting in 3s");
        assert_eq!(
            countdown_label(retry_at, now + Duration::from_millis(1_251)),
            "reconnecting in 1s"
        );
        assert_eq!(
            countdown_label(retry_at, now + Duration::from_millis(2_250)),
            "reconnecting…"
        );
    }

    #[test]
    fn lost_wording_distinguishes_a_failed_health_probe_from_daemon_shutdown() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);

        state.apply_bridge_event(BridgeEvent::Disconnected { attempt: 0 }, now);
        let connection_lost = banner_spec(&state.daemon, state.daemon_since, false, now)
            .unwrap_or_else(|| panic!("expected a connection-lost banner"));
        assert_eq!(connection_lost.text, "Lost connection to fleetd");
        assert_eq!(connection_lost.detail, Some(LOST_REASSURANCE));
        assert!(connection_lost.actionable);
        assert_eq!(dot_label(&state.daemon), Some("connection lost"));

        state.apply_daemon_event(Event::DaemonShuttingDown, now);
        let stopped = banner_spec(&state.daemon, state.daemon_since, false, now)
            .unwrap_or_else(|| panic!("expected a stopped banner"));
        assert_eq!(stopped.text, "fleetd stopped");
        assert_eq!(dot_label(&state.daemon), Some("stopped"));

        state.apply_bridge_event(BridgeEvent::Disconnected { attempt: 1 }, now);
        let retrying = banner_spec(&state.daemon, state.daemon_since, false, now)
            .unwrap_or_else(|| panic!("expected the stopped banner during retries"));
        assert_eq!(retrying.text, "fleetd stopped");
    }

    #[test]
    fn a_restart_must_say_what_became_of_the_terminals() {
        let now = Instant::now();
        let reconnected = |reattached| DaemonLink::Reconnected {
            restarted: true,
            reattached,
            since: now,
        };
        let sentence = |reattached| {
            banner_spec(&reconnected(reattached), now, false, now)
                .unwrap_or_else(|| panic!("expected a banner"))
                .text
        };

        // The promise is only made when it is true.
        assert!(sentence(3).contains("3 terminals were reattached"));
        assert!(sentence(1).contains("1 terminal was reattached"));
        let none = sentence(0);
        assert!(
            none.contains("No terminals survived"),
            "a restart that took the holders with it must not claim a reattach: {none}"
        );
        assert!(!none.contains("reattached"));

        // It stays for the full six seconds, not 800 ms.
        assert!(banner_spec(&reconnected(3), now, false, now + Duration::from_secs(5)).is_some());
        assert!(banner_spec(&reconnected(3), now, false, now + Duration::from_secs(7)).is_none());
    }

    #[test]
    fn a_dropped_connection_only_says_reconnected() {
        let now = Instant::now();
        let warm = DaemonLink::Reconnected {
            restarted: false,
            reattached: 2,
            since: now,
        };
        let spec =
            banner_spec(&warm, now, false, now).unwrap_or_else(|| panic!("expected a banner"));
        assert_eq!(spec.text, "reconnected");
        assert!(banner_spec(&warm, now, false, now + Duration::from_millis(900)).is_none());
    }

    #[test]
    fn the_dot_is_bare_only_while_the_daemon_is_healthy() {
        assert_eq!(dot_state(&DaemonLink::Connected), DaemonState::Healthy);
        assert_eq!(dot_label(&DaemonLink::Connected), None);
        assert_eq!(dot_state(&DaemonLink::Starting), DaemonState::Degraded);
        assert_eq!(dot_label(&DaemonLink::Starting), Some("starting"));
    }

    #[test]
    fn a_newer_binary_uses_the_existing_daemon_banner_surface() {
        let now = Instant::now();
        let spec = banner_spec(&DaemonLink::Connected, now, true, now)
            .unwrap_or_else(|| panic!("expected the existing banner"));
        assert_eq!(spec.text, OUTDATED_SENTENCE);
        assert!(!spec.actionable);
    }
}
