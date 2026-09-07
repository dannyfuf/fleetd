use super::*;

impl Lazygit {
    /// The "rebasing / merging" strip above the body.
    pub(crate) fn banner(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        let operation = self.state.operation();
        let word = mode_word(&operation)?;
        let mut banner = fleet_ui_kit::Banner::warning(format!("{word} in progress"))
            .icon(Icon::TriangleAlert)
            .hints(
                KeyHintRow::new()
                    .key("m", "continue / abort")
                    .key("R", "refresh"),
            );
        if let OperationState::Rebasing {
            done: Some(done),
            total: Some(total),
            ..
        } = operation
        {
            banner = banner.countdown(format!("{done}/{total}"));
        }
        Some(banner.into_any_element())
    }

    /// The one-row bottom bar: key hints on the left, mode and version on the right.
    pub(crate) fn status_bar(&self, chain: &[&'static str], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut hints = KeyHintRow::new();
        for hint in keymap::hints_for_chain(chain, 8).iter() {
            hints = hints.key(hint.keys.clone(), hint.label.clone());
        }
        let operation = self.state.operation();
        let mode = mode_word(&operation);

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .size_full()
            .px(theme.space.md)
            .gap(theme.space.md)
            .bg(theme.colors.bg)
            .border_t(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(div().flex_1().min_w_0().overflow_hidden().child(hints));
        if let Some(error) = &self.state.last_error {
            bar = bar.child(
                Text::ui(error.clone())
                    .color(theme.colors.danger)
                    .truncate_at(60, Truncate::Tail)
                    .flex_none(),
            );
        }
        if self.state.refreshing {
            bar = bar.child(Text::hint("…").flex_none());
        }
        match mode {
            Some(word) => {
                bar = bar.child(ModeWord::word(word).tone(Tone::Warning));
            }
            None => {
                bar = bar.child(ModeWord::word("NORMAL").tone(Tone::Secondary));
            }
        }
        bar.child(Text::hint(format!("fleet-lazygit {}", env!("CARGO_PKG_VERSION"))).flex_none())
            .into_any_element()
    }
}
