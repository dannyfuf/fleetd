//! `KeyValueList` — a block of [`super::FactRow`]s under an optional [`super::SectionHeader`].
//!
//! This is the shape of every detail-panel block (§3.4) and of the PR detail table (§3.5).
//! It exists so a view never has to remember the label column width twice.

use gpui::{AnyElement, App, Pixels, SharedString, Window, div, prelude::*};

use crate::{
    components::{FactRow, FactValue, SectionHeader},
    theme::ActiveTheme,
};

/// A titled block of label/value rows.
#[derive(IntoElement)]
pub struct KeyValueList {
    title: Option<SharedString>,
    trailing: Option<AnyElement>,
    rows: Vec<(SharedString, FactValue, bool)>,
    label_width: Option<Pixels>,
    refreshing: bool,
}

impl KeyValueList {
    /// An untitled block.
    pub fn new() -> Self {
        Self {
            title: None,
            trailing: None,
            rows: Vec::new(),
            label_width: None,
            refreshing: false,
        }
    }

    /// A block under a section header.
    pub fn titled(title: impl Into<SharedString>) -> Self {
        Self::new().title(title)
    }

    /// A block under a section header carrying a right-aligned element on that header, normally
    /// a [`super::FreshnessStamp`].
    ///
    /// The trailing element is a constructor argument and not a builder because the header is
    /// the only place it can hang: an untitled block has nowhere to put it, and a header with a
    /// blank label is not a shape the design system has.
    pub fn titled_with_trailing(
        title: impl Into<SharedString>,
        trailing: impl IntoElement,
    ) -> Self {
        let mut list = Self::titled(title);
        list.trailing = Some(trailing.into_any_element());
        list
    }

    /// Set the section title.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Append a row.
    pub fn row(mut self, label: impl Into<SharedString>, value: FactValue) -> Self {
        self.rows.push((label.into(), value, false));
        self
    }

    /// Append a row whose value renders in the data face.
    pub fn mono_row(mut self, label: impl Into<SharedString>, value: FactValue) -> Self {
        self.rows.push((label.into(), value, true));
        self
    }

    /// Width of the label column.
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// A re-inspection is in flight: every value dims to 60 % and stays readable (§3.4).
    pub fn refreshing(mut self, refreshing: bool) -> Self {
        self.refreshing = refreshing;
        self
    }
}

impl Default for KeyValueList {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for KeyValueList {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let label_width = self.label_width.unwrap_or(theme.metrics.fact_label_w);
        let refreshing = self.refreshing;
        let title = self.title;
        let trailing = self.trailing;
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.xxs)
            .children(title.map(|title| {
                let mut header = SectionHeader::new(title);
                if let Some(trailing) = trailing {
                    header = header.trailing(trailing);
                }
                header
            }))
            .children(self.rows.into_iter().map(|(label, value, mono)| {
                FactRow::new(label, value)
                    .label_width(label_width)
                    .mono(mono)
                    .refreshing(refreshing)
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;

    /// Records that it was actually laid out. Writing in `render` is only tolerable because
    /// this element exists to prove a slot reached the tree at all.
    #[derive(IntoElement)]
    struct Probe(Rc<Cell<bool>>);

    impl RenderOnce for Probe {
        fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
            self.0.set(true);
            div()
        }
    }

    struct TestBlock {
        rendered: Rc<Cell<bool>>,
    }

    impl Render for TestBlock {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            KeyValueList::titled_with_trailing("safety", Probe(self.rendered.clone()))
                .row("path", FactValue::known("/x"))
        }
    }

    #[gpui::test]
    fn the_trailing_slot_reaches_the_header(cx: &mut gpui::TestAppContext) {
        let rendered = Rc::new(Cell::new(false));
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let block = rendered.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| TestBlock { rendered: block })
            })
            .expect("test window")
        });
        let cx = gpui::VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        assert!(
            rendered.get(),
            "the trailing element never reached the tree"
        );
    }
}
