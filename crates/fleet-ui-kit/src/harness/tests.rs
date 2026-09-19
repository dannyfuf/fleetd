use gpui::{IntoElement, ParentElement as _, Point, Styled as _, TestAppContext, div, px, size};

use super::{HarnessTargetExt as _, RecordedTarget, begin_frame, painted, set_recording};
use crate::theme::Theme;

/// Two boxes stacked in a column, so both rects are fully determined by the tokens-free
/// literals this test owns.
fn two_targets() -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .child(div().w(px(100.0)).h(px(20.0)).harness_target("demo.header"))
        .child(
            div()
                .w(px(60.0))
                .h(px(30.0))
                .harness_target_indexed("demo.row", 2),
        )
}

fn rects(targets: &[RecordedTarget]) -> Vec<(String, f32, f32, f32, f32, u64)> {
    targets
        .iter()
        .map(|target| {
            (
                target.name.to_string(),
                target.rect.x,
                target.rect.y,
                target.rect.w,
                target.rect.h,
                target.rect.frame,
            )
        })
        .collect()
}

#[gpui::test]
fn recording_captures_every_painted_target_rect(cx: &mut TestAppContext) {
    set_recording(true);
    let cx = cx.add_empty_window();

    cx.draw(Point::default(), size(px(200.0), px(100.0)), |window, _| {
        begin_frame(window);
        two_targets().into_any_element()
    });

    let targets = cx.update(|window, _| painted(window));
    assert_eq!(
        rects(&targets),
        vec![
            ("demo.header".to_owned(), 0.0, 0.0, 100.0, 20.0, 1),
            ("demo.row[2]".to_owned(), 0.0, 20.0, 60.0, 30.0, 1),
        ],
        "both targets must report their painted rect in logical window coordinates"
    );

    // A second frame clears the previous one and advances the frame number, so a stale target
    // is detectable against `harness::frame`.
    cx.draw(Point::default(), size(px(200.0), px(100.0)), |window, _| {
        begin_frame(window);
        two_targets().into_any_element()
    });

    let (targets, frame) = cx.update(|window, _| (painted(window), super::frame(window)));
    assert_eq!(frame, 2, "each begun frame bumps the window's frame number");
    assert_eq!(
        targets.len(),
        2,
        "the table describes one frame, not every frame ever painted"
    );
    assert!(
        targets.iter().all(|target| target.rect.frame == 2),
        "every rect carries the frame it was painted in: {targets:?}"
    );

    set_recording(false);
}

#[gpui::test]
fn nothing_is_recorded_while_the_harness_is_off(cx: &mut TestAppContext) {
    assert!(
        !super::is_recording(),
        "recording must be off unless the harness turns it on"
    );
    let cx = cx.add_empty_window();

    cx.draw(Point::default(), size(px(200.0), px(100.0)), |window, _| {
        begin_frame(window);
        two_targets().into_any_element()
    });

    let (targets, frame) = cx.update(|window, _| (painted(window), super::frame(window)));
    assert!(
        targets.is_empty(),
        "the paint path must record nothing with harness mode off: {targets:?}"
    );
    assert_eq!(frame, 0, "no frame is begun with harness mode off");
}

#[gpui::test]
fn the_app_frame_is_the_frame_boundary(cx: &mut TestAppContext) {
    set_recording(true);
    cx.update(|cx| cx.set_global(Theme::dark()));
    let cx = cx.add_empty_window();

    cx.draw(Point::default(), size(px(400.0), px(200.0)), |_, _| {
        crate::AppFrame::new()
            .body(div().w(px(80.0)).h(px(24.0)).harness_target("demo.body"))
            .into_any_element()
    });

    let (targets, frame) = cx.update(|window, _| (painted(window), super::frame(window)));
    assert_eq!(
        frame, 1,
        "AppFrame begins the window's frame without any application wiring"
    );
    assert_eq!(
        targets.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>(),
        vec!["demo.body"],
        "a target inside the frame body is recorded"
    );

    set_recording(false);
}

/// A view over the kit components that own a frozen target name, so each one renders inside a
/// real window: several of them reach for view-scoped window state and cannot be drawn as a
/// bare element.
struct FrozenTargets {
    palette_query: gpui::Entity<crate::components::TextInput>,
}

impl gpui::Render for FrozenTargets {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        use crate::components::{
            FuzzyItem, FuzzyList, Palette, PaletteRow, PaletteSection, PaletteSectionKind,
            SegmentedTab, SegmentedTabs, TerminalTab, TerminalTabStrip, Toast, ToastStack,
        };

        crate::AppFrame::new().body(
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    TerminalTabStrip::new([
                        TerminalTab::new(1, "nvim"),
                        TerminalTab::new(2, "cc"),
                        TerminalTab::new(3, "claude"),
                    ])
                    .show_plus(false)
                    .agents_from(2),
                )
                .child(
                    Palette::new(self.palette_query.clone()).section(PaletteSection::new(
                        PaletteSectionKind::Go,
                        [PaletteRow::new("Worktrees"), PaletteRow::new("Board")],
                    )),
                )
                .child(
                    SegmentedTabs::new([
                        SegmentedTab::new("mine", 2),
                        SegmentedTab::new("review", 1),
                    ])
                    .harness_tabs("prs.tab"),
                )
                .child(FuzzyList::new([FuzzyItem::new("acme/web")]).harness_rows("dialog.row", 0))
                .child(ToastStack::new([
                    Toast::new("Worktree created"),
                    Toast::new("Path copied"),
                ])),
        )
    }
}

/// The names `docs/TESTING-HARNESS.md` §3 freezes inside kit components, which no application
/// wrapper can supply because each of these components takes typed items rather than elements.
#[gpui::test]
fn the_kit_paints_the_frozen_component_targets(cx: &mut TestAppContext) {
    use gpui::AppContext as _;

    set_recording(true);
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let palette_query = cx.new(|cx| {
                crate::components::TextInput::new(crate::components::InputMode::SingleLine, cx)
            });
            cx.new(|_| FrozenTargets { palette_query })
        })
        .expect("test window")
    });
    let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let names: Vec<String> = cx.update(|window, _| {
        painted(window)
            .iter()
            .map(|target| target.name.to_string())
            .collect()
    });
    for expected in [
        "tabs.tab[0]",
        "tabs.tab[1]",
        // The strip draws a conversation exactly like a process, but they are not the same
        // thing to a scenario, and the index keeps counting across the boundary.
        "agents.tabs.tab[2]",
        "palette.input",
        "palette.row[0]",
        "palette.row[1]",
        "prs.tab[0]",
        "prs.tab[1]",
        "dialog.row[0]",
        "toasts.toast[0]",
        "toasts.toast[1]",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "{expected} must be painted; got {names:?}"
        );
    }
    assert!(
        !names.iter().any(|name| name.starts_with("hub.tab")),
        "a bar whose caller named no prefix records nothing: {names:?}"
    );

    set_recording(false);
}
