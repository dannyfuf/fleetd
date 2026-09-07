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
    if !model.reachable || model.exit_code.is_some() || model.terminal.is_none() {
        return Tone::Danger;
    }
    match model.activity {
        AgentActivity::Working => Tone::Warning,
        AgentActivity::Idle | AgentActivity::Unknown => Tone::Success,
    }
}
