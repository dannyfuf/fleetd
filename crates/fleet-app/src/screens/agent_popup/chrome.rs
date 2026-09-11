use super::*;

impl AgentPopup {
    pub(super) fn header(&self, model: &Model, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let label = match model.agent {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            // ADR 0014: a config still naming OpenCode runs OpenCode in this terminal.
            Agent::Opencode => "opencode",
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(theme.metrics.dialog_header_h)
            .px(theme.space.md)
            .gap(theme.space.sm)
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(StatusDot::new(header_status_tone(model)))
            .child(Text::ui_strong(label))
            .child(Text::data(model.session.to_string()).muted().ellipsize())
            .child(div().flex_1())
            .child(key_hints())
            .into_any_element()
    }
}

/// The popup's own key set (§9), which its header shows and the status bar mirrors.
///
/// While the popup owns the keyboard the workspace's bindings are shadowed, so a status bar
/// still advertising `⏎ send · ⇧⇥ plan mode · …` beside the word `TERMINAL` names six commands
/// none of which fire (DESIGN-SYSTEM §4). One source, two surfaces.
#[must_use]
pub(crate) fn key_hints() -> KeyHintRow {
    KeyHintRow::new()
        .key("^s q", "hide")
        .key("^s a/A", "switch")
        .key("^q", "hide")
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
