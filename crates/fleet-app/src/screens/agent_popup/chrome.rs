//! The popup's window header: the provider switch, the session line, and Restart / Hide.
//!
//! Every control dispatches the same action its key does, so key and pointer share one path, and
//! shows that key as a [`Kbd`] chip read from the key table (DESIGN-SYSTEM §4).

use std::sync::OnceLock;

use fleet_ui_kit::{Button, ButtonSize, ButtonStyle, Icon, Kbd, Segment, SegmentedControl};
use gpui::{Action, SharedString};

use super::*;
use crate::{
    actions::fleet::{OpenAgentClaude, OpenAgentCodex},
    keymap,
};

/// The context the popup is in until `ctrl-s` is pressed.
const TERMINAL_CONTEXT: &str = "Agent > Terminal";
/// The popup's one-shot prefix context: the key after `ctrl-s`.
const PREFIX_CONTEXT: &str = "Agent > Prefix";

/// What the header says after the session id: the state, then the promise that makes hiding safe.
const HIDING_IS_SAFE: &str = "runs in fleetd, hiding keeps it alive";

/// The header of a popup whose session is live.
pub(super) fn header(model: &Model, cx: &App) -> AnyElement {
    window_header(model.agent, Some(model), cx)
}

/// The header while the daemon is still ensuring the session: the provider switch and Hide stay
/// usable, so a pointer can leave or switch away from a session that is slow to start.
pub(super) fn attaching_header(agent: Agent, cx: &App) -> AnyElement {
    window_header(agent, None, cx)
}

fn window_header(agent: Agent, model: Option<&Model>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let keys = header_keys();
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(theme.metrics.title_bar_h)
        .pl(theme.space.md)
        .pr(theme.space.sm)
        .gap(theme.space.md)
        .border_b(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .child(provider_switch(agent, keys))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(theme.space.sm)
                .overflow_hidden()
                .children(model.map(|model| StatusDot::new(header_status_tone(model))))
                .children(
                    model.map(|model| Text::data(model.session_label.clone()).muted().ellipsize()),
                )
                .children(
                    model.map(|model| Text::ui(model.status_line.clone()).muted().ellipsize()),
                ),
        )
        .children(model.map(|model| {
            // Restart is valid on this same surface once the agent exits, so it stays in place,
            // drawn disabled, rather than appearing only then.
            Button::new("agent-popup-restart", "Restart")
                .style(ButtonStyle::Ghost)
                .size(ButtonSize::Compact)
                .action(Box::new(prefix::RestartCommand))
                .map_kbd(keys.restart.clone())
                .disabled(model.terminal_state != AgentTerminalState::Exited)
                .harness_target("agents.popup.restart")
        }))
        .child(
            Button::new("agent-popup-hide", "Hide")
                .size(ButtonSize::Compact)
                .action(Box::new(agent::Hide))
                .map_kbd(keys.hide.clone())
                .harness_target("agents.popup.hide"),
        )
        .into_any_element()
}

/// `Claude | Codex`: the selected provider is the raised segment; the other one switches to it
/// and carries the key that does the same (`⌃S a` / `⌃S A`).
///
/// A click on the selected segment does nothing, because its key would *hide* the popup (`a` on
/// the open Claude popup toggles it), and clicking the provider that is already showing must not.
fn provider_switch(agent: Agent, keys: &HeaderKeys) -> SegmentedControl {
    // ADR 0014: a config still naming OpenCode runs OpenCode in the second slot.
    let second = if agent == Agent::Opencode {
        ("OpenCode", Icon::Bot)
    } else {
        ("Codex", Icon::SquareTerminal)
    };
    let selected = usize::from(agent != Agent::Claude);
    let claude = Segment::new("Claude").icon(Icon::Sparkles);
    let other = Segment::new(second.0).icon(second.1);
    // Only the provider a click would switch to shows its key.
    let (claude, other) = if selected == 0 {
        (claude, other.kbd(keys.codex.clone()))
    } else {
        (claude.kbd(keys.claude.clone()), other)
    };
    SegmentedControl::new("agent-popup-provider", [claude, other])
        .active(Some(selected))
        .harness_segments("agents.popup.agent")
        .on_select(move |index, window, cx| {
            if index == selected {
                return;
            }
            let action: Box<dyn Action> = if index == 0 {
                Box::new(OpenAgentClaude)
            } else {
                Box::new(OpenAgentCodex)
            };
            window.dispatch_action(action, cx);
        })
}

/// Sets the chip only when the key table gave one, so a missing row shows no chip rather than
/// falling back to whatever the focused context happens to bind.
trait MapKbd: Sized {
    fn map_kbd(self, kbd: Option<Kbd>) -> Self;
}

impl MapKbd for Button {
    fn map_kbd(self, kbd: Option<Kbd>) -> Self {
        match kbd {
            Some(kbd) => self.kbd(kbd),
            None => self,
        }
    }
}

/// The chips the header's controls show, read once from the key table.
///
/// They cannot come from [`Kbd::for_action`]: the popup's prefix keys are bound in
/// `Agent > Prefix`, a mode the popup is in for one key only, so resolving against the focused
/// context would show `⌃S r` only while `⌃S` is held and nothing the rest of the time. The chip
/// is the full sequence instead — the prefix key, then the key bound after it.
pub(super) struct HeaderKeys {
    pub(super) hide: Option<Kbd>,
    pub(super) restart: Option<Kbd>,
    pub(super) claude: Option<Kbd>,
    pub(super) codex: Option<Kbd>,
}

pub(super) fn header_keys() -> &'static HeaderKeys {
    static KEYS: OnceLock<HeaderKeys> = OnceLock::new();
    KEYS.get_or_init(|| {
        let table = keymap::table();
        let prefix = bound(&table, TERMINAL_CONTEXT, &agent::EnterPrefix);
        let prefixed = |action: &dyn Action| -> Option<Kbd> {
            let mut strokes = prefix.clone()?;
            strokes.extend(bound(&table, PREFIX_CONTEXT, action)?);
            Some(Kbd::new(&strokes))
        };
        HeaderKeys {
            hide: bound(&table, TERMINAL_CONTEXT, &agent::Hide).map(|strokes| Kbd::new(&strokes)),
            restart: prefixed(&prefix::RestartCommand),
            claude: prefixed(&OpenAgentClaude),
            codex: prefixed(&OpenAgentCodex),
        }
    })
}

/// The keystrokes of the first row binding `action` in exactly `context`.
fn bound(
    table: &[keymap::BindingSpec],
    context: &str,
    action: &dyn Action,
) -> Option<Vec<Keystroke>> {
    let name = action.name();
    let spec = table
        .iter()
        .find(|spec| spec.context == context && spec.action == name)?;
    spec.keys
        .split_whitespace()
        .map(Keystroke::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| tracing::error!(keys = spec.keys, %error, "unparsable popup key"))
        .ok()
}

/// The words after the session id: what the agent is doing, then that hiding is safe.
pub(super) fn status_line(
    model_state: AgentTerminalState,
    activity: AgentActivity,
    reachable: bool,
) -> SharedString {
    let state = if !reachable {
        "fleetd unreachable"
    } else {
        match (model_state, activity) {
            (AgentTerminalState::Exited, _) => "exited",
            (AgentTerminalState::Unknown | AgentTerminalState::Starting, _) => "starting",
            (AgentTerminalState::Running, AgentActivity::Working) => "working",
            (AgentTerminalState::Running, AgentActivity::Idle | AgentActivity::Unknown) => "idle",
        }
    };
    SharedString::from(format!("· {state} · {HIDING_IS_SAFE}"))
}

pub(super) fn header_status_tone(model: &Model) -> Tone {
    if !model.reachable || model.terminal_state == AgentTerminalState::Exited {
        return Tone::Danger;
    }
    match (model.terminal_state, model.activity) {
        (AgentTerminalState::Running, AgentActivity::Idle | AgentActivity::Unknown) => {
            Tone::Success
        }
        (
            AgentTerminalState::Unknown | AgentTerminalState::Starting,
            AgentActivity::Unknown | AgentActivity::Working | AgentActivity::Idle,
        )
        | (AgentTerminalState::Running, AgentActivity::Working) => Tone::Warning,
        (AgentTerminalState::Exited, _) => Tone::Danger,
    }
}
