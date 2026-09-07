use super::*;

impl AgentPopup {
    pub(super) fn header(&self, model: &Model, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let label = match model.agent {
            Agent::Claude => "claude",
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
            .child(
                KeyHintRow::new()
                    .key("^s q", "hide")
                    .key("^s a/A", "switch")
                    .key("^q", "hide"),
            )
            .into_any_element()
    }
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
