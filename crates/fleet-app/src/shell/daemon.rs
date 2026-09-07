//! The daemon surfaces of UX-SPEC §3.12: the cold-start splash, the "will not start" window
//! and the 28 px banner, plus the wording each of them is allowed to use.

use std::time::Instant;

use fleet_core::paths::FleetHome;
use fleet_ui_kit::{Banner, DaemonSplash, DaemonState, Icon, KeyHintRow};
use gpui::{AnyElement, Entity, IntoElement, SharedString, prelude::*};

use crate::state::{
    AppState, DaemonLink, RECONNECT_BANNER_DWELL, RESTART_BANNER_DWELL, SPLASH_DETAIL_DELAY,
    reconnect_backoff,
};

/// PTYs do not survive a daemon restart; the recovery banner must say so explicitly.
const RESTART_SENTENCE: &str =
    "fleetd restarted. Terminal sessions did not survive; worktrees, jobs and state are intact.";
/// The connected process predates the fleetd binary currently on disk.
const OUTDATED_SENTENCE: &str = "fleetd is outdated, restart with `fleet daemon restart`";

/// What the 28 px banner under the context bar says right now.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BannerSpec {
    /// The sentence.
    text: &'static str,
    /// The reconnect countdown, when one is running.
    countdown: Option<String>,
    /// The keys this banner owns while it is showing.
    hints: &'static [(&'static str, &'static str)],
}

/// The reconnect countdown of §3.12 C, cycling `3s → reconnecting… → 6s`.
#[must_use]
fn countdown_label(attempt: u32) -> String {
    format!("reconnecting in {}s", reconnect_backoff(attempt).as_secs())
}

/// The banner for the current daemon state, or `None` when there is nothing to say.
///
/// A dismissed `Lost` banner returns `None`; the daemon dot stays red, which is exactly what
/// §3.12 asks for.
#[must_use]
fn banner_spec(daemon: &DaemonLink, outdated: bool, now: Instant) -> Option<BannerSpec> {
    match daemon {
        DaemonLink::Lost {
            attempt,
            dismissed: false,
        } => Some(BannerSpec {
            text: "fleetd stopped",
            countdown: Some(countdown_label(*attempt)),
            hints: &[
                ("r", "reconnect now"),
                ("l", "open the log"),
                ("esc", "dismiss"),
            ],
        }),
        DaemonLink::Connected if outdated => Some(BannerSpec {
            text: OUTDATED_SENTENCE,
            countdown: None,
            hints: &[],
        }),
        DaemonLink::Reconnected { restarted, since } => {
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
                    RESTART_SENTENCE
                } else {
                    "reconnected"
                },
                countdown: None,
                hints: &[],
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
        DaemonLink::Lost { .. } => Some("stopped"),
        DaemonLink::Failed { .. } => Some("down"),
        DaemonLink::Connected | DaemonLink::Reconnected { .. } => None,
    }
}

/// Renders the banner, or nothing.
#[must_use]
pub(super) fn banner(daemon: &DaemonLink, outdated: bool, now: Instant) -> Option<AnyElement> {
    let spec = banner_spec(daemon, outdated, now)?;
    let mut banner = Banner::warning(spec.text).icon(Icon::Unplug);
    if let Some(countdown) = spec.countdown {
        banner = banner.countdown(countdown);
    }
    if !spec.hints.is_empty() {
        let mut hints = KeyHintRow::new();
        for (key, label) in spec.hints {
            hints = hints.key(*key, *label);
        }
        banner = banner.hints(hints);
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

    use super::*;

    #[test]
    fn a_dismissed_banner_says_nothing_but_the_dot_stays_red() {
        let now = Instant::now();
        let lost = DaemonLink::Lost {
            attempt: 0,
            dismissed: true,
        };
        assert_eq!(banner_spec(&lost, false, now), None);
        assert_eq!(dot_state(&lost), DaemonState::Lost);
    }

    #[test]
    fn the_countdown_follows_the_backoff() {
        assert_eq!(countdown_label(0), "reconnecting in 1s");
        assert_eq!(countdown_label(1), "reconnecting in 2s");
        assert_eq!(countdown_label(2), "reconnecting in 4s");
        assert_eq!(countdown_label(9), "reconnecting in 8s");
    }

    #[test]
    fn a_restart_must_say_the_sessions_are_gone() {
        let now = Instant::now();
        let restarted = DaemonLink::Reconnected {
            restarted: true,
            since: now,
        };
        let spec =
            banner_spec(&restarted, false, now).unwrap_or_else(|| panic!("expected a banner"));
        assert_eq!(spec.text, RESTART_SENTENCE);
        assert!(spec.text.contains("did not survive"));

        // It stays for the full six seconds, not 800 ms.
        assert!(banner_spec(&restarted, false, now + Duration::from_secs(5)).is_some());
        assert!(banner_spec(&restarted, false, now + Duration::from_secs(7)).is_none());
    }

    #[test]
    fn a_dropped_connection_only_says_reconnected() {
        let now = Instant::now();
        let warm = DaemonLink::Reconnected {
            restarted: false,
            since: now,
        };
        let spec = banner_spec(&warm, false, now).unwrap_or_else(|| panic!("expected a banner"));
        assert_eq!(spec.text, "reconnected");
        assert!(banner_spec(&warm, false, now + Duration::from_millis(900)).is_none());
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
        let spec = banner_spec(&DaemonLink::Connected, true, Instant::now())
            .unwrap_or_else(|| panic!("expected the existing banner"));
        assert_eq!(spec.text, OUTDATED_SENTENCE);
        assert!(spec.hints.is_empty());
    }
}
