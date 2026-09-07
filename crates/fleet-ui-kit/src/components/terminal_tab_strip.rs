//! `TerminalTabStrip` — numbered tabs with activity, keep-alive, agent and exit marks.
//!
//! §3.6: the index is the argument to `ctrl-s 1`-`9`, so the strip is the legend for that
//! binding. The amber activity dot marks unseen output; agent glyphs show working or finished.
//!
//! The strip is a **legend**, not a control surface: every tab is reachable from the keyboard
//! without it. The click handlers ([`TerminalTabStrip::on_select`],
//! [`TerminalTabStrip::on_new`]) exist for mouse parity only, which is why they are optional
//! and why nothing here draws a close button — `^s x` is the way a tab is closed, and a hover
//! target that kills a running dev server is not worth the pixel.

use gpui::{App, ElementId, SharedString, Window, div, prelude::*};

use crate::{
    components::{Spinner, StatusDot, StatusGlyph, StatusKind},
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// What draws a tab's content.
///
/// The strip is the legend for `ctrl-s 1`-`9`, so a tab that is *not* a terminal has to say so:
/// otherwise the only difference the user can see between a PTY and a Fleet-drawn pane is that
/// one of them ignores every key they type into it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalTabKind {
    /// A PTY. The default, and what the rest of this component assumes.
    #[default]
    Pty,
    /// A surface the app draws itself; marked with a glyph in front of the name.
    Native,
}

/// One terminal tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalTab {
    /// The `1`-`9` index.
    pub index: usize,
    /// Stable identity across tab reordering; falls back to the numbered index.
    pub id: Option<ElementId>,
    /// The tab name (`nvim`, `cc`, `lg`).
    pub name: SharedString,
    /// Output happened since this tab was last visited.
    pub activity: bool,
    /// The PTY is still spawning: §3.6 "Waking a slept session" rebuilds the strip and each
    /// tab shows a `loader-circle` until its process is up.
    pub starting: bool,
    /// The keep-alive kind glyph: `bot`, `server`, `file-pen`.
    pub keep_alive: Option<Icon>,
    /// Recognized agent activity, limited to `AgentWorking` or `AgentFinished`.
    pub agent_status: Option<StatusKind>,
    /// The command exited. `Some(None)` is a signal-killed process, which has **no** exit
    /// code; the strip renders `—` rather than inventing one.
    pub exited: Option<Option<i32>>,
    /// Whether a process or the app itself provides the tab's content.
    pub kind: TerminalTabKind,
}

impl TerminalTab {
    /// A tab.
    pub fn new(index: usize, name: impl Into<SharedString>) -> Self {
        Self {
            index,
            id: None,
            name: name.into(),
            activity: false,
            starting: false,
            keep_alive: None,
            agent_status: None,
            exited: None,
            kind: TerminalTabKind::default(),
        }
    }

    /// Key this tab by its stable terminal identity.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// What draws the tab's content.
    pub fn kind(mut self, kind: TerminalTabKind) -> Self {
        self.kind = kind;
        self
    }

    /// Mark unread output.
    pub fn activity(mut self, activity: bool) -> Self {
        self.activity = activity;
        self
    }

    /// Mark the PTY as still spawning.
    pub fn starting(mut self, starting: bool) -> Self {
        self.starting = starting;
        self
    }

    /// Mark a keep-alive process.
    pub fn keep_alive(mut self, icon: Icon) -> Self {
        self.keep_alive = Some(icon);
        self
    }

    /// Mark recognized agent activity on this terminal.
    pub fn agent_status(mut self, status: StatusKind) -> Self {
        debug_assert!(matches!(
            status,
            StatusKind::AgentWorking | StatusKind::AgentFinished
        ));
        self.agent_status = Some(status);
        self
    }

    /// Mark the command as exited. Takes `1` or `None`: a signal-killed process has no code.
    pub fn exited(mut self, code: impl Into<Option<i32>>) -> Self {
        self.exited = Some(code.into());
        self
    }

    /// The tab's element id: the caller's, or one derived from the `ctrl-s` index.
    fn element_id(&self) -> ElementId {
        self.id
            .clone()
            .unwrap_or_else(|| ("tab", self.index).into())
    }

    /// The exit code as it is written on the tab: the number, or `—` for a signal.
    fn exit_label(code: Option<i32>) -> SharedString {
        match code {
            Some(code) => SharedString::from(code.to_string()),
            None => SharedString::new_static("\u{2014}"),
        }
    }
}

/// Mouse parity for `ctrl-s <n>`, called with the clicked tab's position. `Rc` because every
/// tab element gets a handle to the same closure.
type SelectHandler = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

/// Mouse parity for `ctrl-s c`, fired by the trailing `+` tab.
type NewHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// The tab strip.
#[derive(IntoElement)]
pub struct TerminalTabStrip {
    id: ElementId,
    tabs: Vec<TerminalTab>,
    active: usize,
    show_plus: bool,
    on_select: Option<SelectHandler>,
    on_new: Option<NewHandler>,
}

impl TerminalTabStrip {
    /// A strip over the session's terminals.
    pub fn new(tabs: impl IntoIterator<Item = TerminalTab>) -> Self {
        Self {
            id: ElementId::Name(SharedString::new_static("terminal-tab-strip")),
            tabs: tabs.into_iter().collect(),
            active: 0,
            show_plus: true,
            on_select: None,
            on_new: None,
        }
    }

    /// A stable id, so two strips in one window keep their hover and animation state apart.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Which tab index (position, not `Terminal.index`) is active.
    pub fn active(mut self, active: usize) -> Self {
        self.active = active;
        self
    }

    /// Hide the trailing `+` tab.
    pub fn show_plus(mut self, show: bool) -> Self {
        self.show_plus = show;
        self
    }

    /// Mouse parity for `ctrl-s 1`-`9`: called with the clicked tab's **position**.
    pub fn on_select(mut self, on_select: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(on_select));
        self
    }

    /// Mouse parity for `ctrl-s c`, fired by the trailing `+` tab.
    pub fn on_new(mut self, on_new: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_new = Some(Box::new(on_new));
        self
    }
}

impl RenderOnce for TerminalTabStrip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Self {
            id: strip_id,
            tabs,
            active,
            show_plus,
            on_select,
            on_new,
        } = self;

        let state = window.use_keyed_state(
            ElementId::NamedChild(std::sync::Arc::new(strip_id.clone()), "scroll-state".into()),
            cx,
            |_, _| TabStripState::default(),
        );
        let identities: Vec<_> = tabs.iter().map(TerminalTab::element_id).collect();
        let scroll = state.read(cx).scroll.clone();

        let tab_theme = theme.clone();
        let tabs = tabs.into_iter().enumerate().map(move |(pos, tab)| {
            tab_element(tab, pos, pos == active, &tab_theme, on_select.clone())
        });

        div()
            .id(strip_id)
            .flex()
            .items_stretch()
            .h(theme.metrics.pane_header_h)
            .w_full()
            .bg(theme.colors.bg)
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .overflow_hidden()
            .child(
                div()
                    .on_children_prepainted(move |_, window, cx| {
                        state.update(cx, |state, _| state.reveal(active, &identities, window));
                    })
                    .id("tabs")
                    .flex()
                    .items_stretch()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    .children(tabs),
            )
            .children(show_plus.then(|| new_tab_button(&theme, on_new)))
    }
}

/// One tab: the index, the name, and whatever marks apply to it.
fn tab_element(
    tab: TerminalTab,
    pos: usize,
    is_active: bool,
    theme: &Theme,
    on_select: Option<SelectHandler>,
) -> gpui::Stateful<gpui::Div> {
    let hover_bg = theme.colors.row_hover;

    div()
        .id(tab.element_id())
        .flex()
        .flex_col()
        .justify_between()
        .flex_none()
        .min_w(theme.metrics.terminal_tab_min_w)
        .max_w(theme.metrics.terminal_tab_max_w)
        .h_full()
        .border_r(theme.metrics.hairline)
        .border_color(theme.colors.border)
        // The active tab sits one step above the strip so the eye finds "where am I" before it
        // reads any name.
        .when(is_active, |el| el.bg(theme.colors.surface))
        .when(!is_active, |el| el.hover(move |s| s.bg(hover_bg)))
        .when_some(on_select, |el, select| {
            super::control::on_activate(el, move |window, cx| select(pos, window, cx))
        })
        .child(tab_body(tab, is_active, theme))
        .child(
            div()
                .flex_none()
                .h(theme.metrics.focus_ring_w)
                .w_full()
                .bg(if is_active {
                    theme.colors.accent
                } else {
                    gpui::transparent_black()
                }),
        )
}

/// The tab's single line: index, kind glyph, name, then the status marks.
fn tab_body(tab: TerminalTab, is_active: bool, theme: &Theme) -> gpui::Div {
    let exited = tab.exited;
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(theme.space.xs)
        .px(theme.space.sm)
        // The index is the argument to `ctrl-s <n>`; it never dims away, because the moment it
        // does the binding stops being discoverable.
        .child(Text::hint(tab.index.to_string()).faint())
        // A Fleet-drawn pane is not a terminal: the glyph is the only thing on the strip that
        // says why this tab answers `j` and `k` instead of typing them.
        .children((tab.kind == TerminalTabKind::Native).then(|| {
            Icon::GitBranch
                .el()
                .size(IconSize::Small)
                .color(theme.colors.text_secondary)
        }))
        .child(
            if exited.is_some() {
                Text::ui(tab.name).faint()
            } else if is_active {
                Text::ui_strong(tab.name)
            } else {
                Text::ui(tab.name).muted()
            }
            .ellipsize(),
        )
        .children(
            tab.starting
                .then(|| Spinner::new("starting").size(IconSize::Small)),
        )
        .children(tab.keep_alive.map(|icon| {
            icon.el()
                .size(IconSize::Small)
                .color(theme.colors.text_secondary)
        }))
        .children(
            tab.agent_status
                .map(|status| StatusGlyph::new(status).size(IconSize::Small).id("agent")),
        )
        .children(exited.map(|code| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.space.xxs)
                .child(
                    Icon::CircleX
                        .el()
                        .size(IconSize::Small)
                        .color(theme.colors.text_muted),
                )
                .child(Text::hint(TerminalTab::exit_label(code)).faint())
        }))
        .children(
            // Activity on the tab you are already looking at is not news.
            (tab.activity && !is_active && exited.is_none())
                .then(|| StatusDot::small(Tone::Warning)),
        )
}

/// The trailing `+`: mouse parity for `ctrl-s c`.
fn new_tab_button(theme: &Theme, on_new: Option<NewHandler>) -> gpui::Stateful<gpui::Div> {
    div()
        .id("new")
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(theme.metrics.new_terminal_tab_w)
        .h_full()
        .hover(|s| s.bg(theme.colors.row_hover))
        .when_some(on_new, |el, on_new| super::control::on_activate(el, on_new))
        .child(
            Icon::Plus
                .el()
                .size(IconSize::Medium)
                .color(theme.colors.text_muted),
        )
}

#[derive(Default)]
struct TabStripState {
    scroll: gpui::ScrollHandle,
    active: Option<usize>,
    identities: Vec<ElementId>,
    viewport: Option<gpui::Size<gpui::Pixels>>,
}

impl TabStripState {
    /// Scroll the active tab into view once the strip's geometry actually changed.
    ///
    /// GPUI initializes the scroll viewport during prepaint, so this runs from
    /// `on_children_prepainted` and asks for one more frame when it moves the handle.
    fn reveal(&mut self, active: usize, identities: &[ElementId], window: &mut Window) {
        let viewport = self.scroll.bounds().size;
        if self.active == Some(active)
            && self.identities == identities
            && self.viewport == Some(viewport)
        {
            return;
        }
        self.scroll.scroll_to_item(active);
        self.active = Some(active);
        self.identities.clear();
        self.identities.extend_from_slice(identities);
        self.viewport = Some(viewport);
        window.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestStrip {
        tabs: usize,
        selected: Option<usize>,
        created: bool,
    }

    impl Render for TestStrip {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            let weak = cx.weak_entity();
            let new_weak = weak.clone();
            div().w(gpui::px(300.0)).child(
                TerminalTabStrip::new((0..self.tabs).map(|ix| TerminalTab::new(ix + 1, "sh")))
                    .active(self.tabs - 1)
                    .on_select(move |index, _, cx| {
                        let _ = weak.update(cx, |view, _| view.selected = Some(index));
                    })
                    .on_new(move |_, cx| {
                        let _ = new_weak.update(cx, |view, _| view.created = true);
                    }),
            )
        }
    }

    #[gpui::test]
    fn overflow_reveals_the_active_tab_and_preserves_the_new_button(cx: &mut gpui::TestAppContext) {
        use gpui::{AppContext, Modifiers, point, px};
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| TestStrip {
                    tabs: 8,
                    selected: None,
                    created: false,
                })
            })
            .expect("test window")
        });
        let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
        let view = window.root(&mut cx).expect("test strip");
        cx.run_until_parked();
        cx.simulate_mouse_move(point(px(250.0), px(12.0)), None, Modifiers::none());
        cx.simulate_click(point(px(250.0), px(12.0)), Modifiers::none());
        view.read_with(&cx, |view, _| assert_eq!(view.selected, Some(7)));
        cx.simulate_click(point(px(282.0), px(12.0)), Modifiers::none());
        view.read_with(&cx, |view, _| assert!(view.created));
        view.update(&mut cx, |view, cx| {
            view.tabs = 2;
            view.created = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_mouse_move(point(px(250.0), px(12.0)), None, Modifiers::none());
        cx.simulate_click(point(px(190.0), px(12.0)), Modifiers::none());
        view.read_with(&cx, |view, _| assert!(view.created));
    }

    #[test]
    fn a_signal_killed_tab_shows_a_dash_and_never_a_number() {
        assert_eq!(TerminalTab::exit_label(Some(1)).as_ref(), "1");
        assert_eq!(TerminalTab::exit_label(None).as_ref(), "\u{2014}");
    }

    #[test]
    fn exited_accepts_both_a_code_and_none() {
        assert_eq!(TerminalTab::new(1, "cc").exited(1).exited, Some(Some(1)));
        assert_eq!(TerminalTab::new(1, "cc").exited(None).exited, Some(None));
    }
}
