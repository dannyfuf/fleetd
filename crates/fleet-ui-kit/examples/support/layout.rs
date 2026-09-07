use fleet_ui_kit::prelude::*;

pub struct GalleryLayout {
    pub label_width: f32,
    pub column: bool,
    pub divided: bool,
    pub compact: bool,
}

impl GalleryLayout {
    pub fn section(&self, title: &str, theme: &Theme, children: Vec<AnyElement>) -> AnyElement {
        let divided = self.divided;
        let gap = if self.compact {
            theme.space.sm
        } else {
            theme.space.md
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.md)
            .pb(theme.space.xl)
            .child(SectionHeader::new(SharedString::new(title)))
            .when(divided, |el| el.child(Divider::horizontal()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(gap)
                    .when(divided, |el| el.pt(theme.space.xs))
                    .children(children),
            )
            .into_any_element()
    }

    pub fn labeled(&self, label: &str, theme: &Theme, child: impl IntoElement) -> AnyElement {
        div()
            .flex()
            .items_start()
            .w_full()
            .gap(theme.space.md)
            .child(
                Text::hint(SharedString::new(label))
                    .faint()
                    .w(px(self.label_width)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .map(|el| {
                        if self.column {
                            el.flex_col()
                        } else {
                            el.items_center()
                        }
                    })
                    .child(child),
            )
            .into_any_element()
    }
}

pub fn strip(theme: &Theme, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme.space.md)
        .children(children)
        .into_any_element()
}
