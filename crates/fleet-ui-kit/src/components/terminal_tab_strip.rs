//! `TerminalTabStrip` — the Workspace's tabs: what each one is, its state, and the controls to
//! open, close and manage them.
//!
//! §3.6: every tab has a kind icon and its name; the `ctrl-s 1`-`9` index is no longer painted,
//! it is in the tab's tooltip (`Tab 2 · ⌃S 2`) with the rest of the keys. One state mark at most
//! beside the name: a spinner while the tab starts or its agent works, an amber *needs you* chip,
//! a blue unread-output dot, or `exited 1` in red.
//!
//! Every verb has a key and the strip only mirrors it (ADR 0023): a click selects
//! ([`TerminalTabStrip::on_select`]), the `✕` on a hovered or active tab and a middle-click close
//! ([`TerminalTabStrip::on_close`]), a right-click selects and opens the tab's menu
//! ([`TerminalTabStrip::tab_menu`]), and the `+` opens the new-tab menu
//! ([`TerminalTabStrip::new_menu`]). All of them are optional, so the strip still draws in a
//! gallery with none. The trailing slot holds the surface's toggles (Watch, Zoom).
//!
//! The active tab joins the content below it: it takes the content ground, a hairline on three
//! sides and none underneath, while the strip's own bottom hairline runs behind every other tab.

use std::rc::Rc;

use gpui::{App, Context, ElementId, MouseButton, SharedString, Window, div, prelude::*};

use crate::{
    components::{
        Chip, ContextMenu, IconButton, Kbd, Menu, PopoverMenu, Spinner, StatusDot, StatusGlyph,
        StatusKind, Tooltip,
    },
    harness::{self, HarnessTargetExt as _},
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// The hover group each tab names itself with, so its `✕` shows while that tab is hovered.
const TAB_GROUP: &str = "terminal-tab";

/// What draws a tab's content.
///
/// A tab that is *not* a terminal has to say so: otherwise the only difference the user can see
/// between a PTY and a Fleet-drawn pane is that one of them ignores every key typed into it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalTabKind {
    /// A PTY. The default; drawn with the `terminal` glyph.
    #[default]
    Pty,
    /// A surface the app draws itself; drawn with the `git-branch` glyph unless
    /// [`TerminalTab::icon`] names a better one.
    Native,
}

impl TerminalTabKind {
    fn icon(self) -> Icon {
        match self {
            Self::Pty => Icon::Terminal,
            Self::Native => Icon::GitBranch,
        }
    }
}

/// The only process states that can appear as agent activity on a terminal tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalAgentState {
    /// The recognized agent is actively working.
    Working,
    /// The recognized agent is heuristically idle.
    Finished,
}

impl TerminalAgentState {
    fn status_kind(self) -> StatusKind {
        match self {
            Self::Working => StatusKind::AgentWorking,
            Self::Finished => StatusKind::AgentFinished,
        }
    }
}

/// The one state mark a tab draws beside its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabMark {
    /// Blocked on the reader: the amber `needs you` chip.
    NeedsYou,
    /// Output or content the reader has not seen: the blue dot.
    Unread,
}

/// One tab.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalTab {
    /// The `1`-`9` index, shown in the tooltip.
    pub index: usize,
    /// Stable identity across tab reordering; falls back to the numbered index.
    pub id: Option<ElementId>,
    /// The tab name (`nvim`, `cc`, `lg`).
    pub name: SharedString,
    /// Output happened since this tab was last visited.
    pub activity: bool,
    /// The tab is blocked on the user, so its *needs you* chip survives being selected.
    ///
    /// `NATIVE-AGENTS.md` §2/§3.3: an open gate is not "news you have already read", it is work
    /// only the reader can unblock, and the strip is where the workspace says so — including on
    /// the tab that is currently open, which is the one place [`TerminalTab::activity`] is
    /// deliberately silent.
    pub attention: bool,
    /// Content changed but nothing is waiting on the user: the same blue dot as
    /// [`TerminalTab::activity`], for a tab that has no PTY output to speak of.
    pub unread: bool,
    /// The tab is still spawning, or its agent is working: a spinner.
    pub starting: bool,
    /// The keep-alive kind glyph: `bot`, `server`, `file-pen`.
    pub keep_alive: Option<Icon>,
    /// Recognized agent activity.
    pub agent_status: Option<TerminalAgentState>,
    /// The command exited. `Some(None)` is a signal-killed process, which has **no** exit
    /// code; the strip writes `killed` rather than inventing one.
    pub exited: Option<Option<i32>>,
    /// Whether a process or the app itself provides the tab's content.
    pub kind: TerminalTabKind,
    /// The kind glyph, when the caller knows better than [`TerminalTabKind`]: a board, a
    /// provider mark.
    pub icon: Option<Icon>,
    /// The key that selects this tab, shown in its tooltip.
    pub kbd: Option<Kbd>,
}

impl TerminalTab {
    /// A tab.
    pub fn new(index: usize, name: impl Into<SharedString>) -> Self {
        Self {
            index,
            id: None,
            name: name.into(),
            activity: false,
            attention: false,
            unread: false,
            starting: false,
            keep_alive: None,
            agent_status: None,
            exited: None,
            kind: TerminalTabKind::default(),
            icon: None,
            kbd: None,
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

    /// Lead the name with this glyph instead of the kind's.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The key that selects this tab, for its tooltip.
    pub fn kbd(mut self, kbd: impl Into<Option<Kbd>>) -> Self {
        self.kbd = kbd.into();
        self
    }

    /// Mark unseen output with the blue dot.
    pub fn activity(mut self, activity: bool) -> Self {
        self.activity = activity;
        self
    }

    /// Mark the tab as blocked on the user; the *needs you* chip then stays while it is
    /// selected.
    pub fn attention(mut self, attention: bool) -> Self {
        self.attention = attention;
        self
    }

    /// Mark unseen content that is not waiting on the user.
    pub fn unread(mut self, unread: bool) -> Self {
        self.unread = unread;
        self
    }

    /// Mark the tab as still spawning, or its agent as working.
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
    pub fn agent_status(mut self, status: TerminalAgentState) -> Self {
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

    /// The single mark the tab draws beside its name, if any.
    ///
    /// *Needs you* wins over unread, and an exited tab draws neither: its `exited` word already
    /// says everything a mark could, and two marks on one tab make the strip unreadable. Unread
    /// output on the tab you are already looking at is not news, so the active tab draws no dot —
    /// *unless* the tab is blocked on the reader, which stays true whichever tab is open (§3.3).
    fn mark(&self, is_active: bool) -> Option<TabMark> {
        if self.exited.is_some() {
            return None;
        }
        if self.attention {
            return Some(TabMark::NeedsYou);
        }
        if !is_active && (self.activity || self.unread) {
            return Some(TabMark::Unread);
        }
        None
    }

    /// The exit as it is written on the tab: `exited 1`, or `killed` for a signal.
    fn exit_label(code: Option<i32>) -> SharedString {
        match code {
            Some(code) => SharedString::from(format!("exited {code}")),
            None => SharedString::new_static("killed"),
        }
    }
}

/// A per-tab handler, called with the tab's **position**. `Rc` because every tab element gets
/// a handle to the same closure.
type PositionHandler = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

/// Builds a tab's right-click menu for its position.
type TabMenuBuilder = Rc<dyn Fn(usize, Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static>;

/// Builds the `+` menu.
type NewMenuBuilder = Rc<dyn Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static>;

/// Mouse parity for `ctrl-s c` when there is no new-tab menu: the `+` runs it directly.
type NewHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// The tab strip.
#[derive(IntoElement)]
pub struct TerminalTabStrip {
    id: ElementId,
    tabs: Vec<TerminalTab>,
    active: usize,
    show_plus: bool,
    agents_from: Option<usize>,
    on_select: Option<PositionHandler>,
    on_close: Option<PositionHandler>,
    close_kbd: Option<Kbd>,
    tab_menu: Option<TabMenuBuilder>,
    new_menu: Option<NewMenuBuilder>,
    on_new: Option<NewHandler>,
    trailing: Vec<gpui::AnyElement>,
}

impl TerminalTabStrip {
    /// A strip over the session's tabs.
    pub fn new(tabs: impl IntoIterator<Item = TerminalTab>) -> Self {
        Self {
            id: ElementId::Name(SharedString::new_static("terminal-tab-strip")),
            tabs: tabs.into_iter().collect(),
            active: 0,
            show_plus: true,
            agents_from: None,
            on_select: None,
            on_close: None,
            close_kbd: None,
            tab_menu: None,
            new_menu: None,
            on_new: None,
            trailing: Vec::new(),
        }
    }

    /// A stable id, so two strips in one window keep their hover and menu state apart.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Which tab index (position, not `Terminal.index`) is active.
    pub fn active(mut self, active: usize) -> Self {
        self.active = active;
        self
    }

    /// Hide the `+`.
    pub fn show_plus(mut self, show: bool) -> Self {
        self.show_plus = show;
        self
    }

    /// The position at which tabs stop addressing a process and start addressing a
    /// conversation.
    ///
    /// The strip draws both the same way, but they are not the same thing to anything reading
    /// the window from outside: the harness names a process tab `tabs.tab[N]` and a
    /// conversation tab `agents.tabs.tab[N]`, which is the same split the snapshot's `focused`
    /// field reports (`docs/TESTING-HARNESS.md` §3). Left unset, every tab is a process tab.
    pub fn agents_from(mut self, position: usize) -> Self {
        self.agents_from = Some(position);
        self
    }

    /// Mouse parity for `ctrl-s 1`-`9`: called with the clicked tab's **position**. A
    /// right-click selects as well, before the tab's menu opens.
    pub fn on_select(mut self, on_select: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// Mouse parity for `ctrl-s x`: the `✕` and a middle-click, with the tab's **position**.
    /// Without it no tab draws a `✕`.
    pub fn on_close(mut self, on_close: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }

    /// The close key, for the `✕`'s tooltip.
    pub fn close_kbd(mut self, kbd: impl Into<Option<Kbd>>) -> Self {
        self.close_kbd = kbd.into();
        self
    }

    /// The menu a right-click on the tab at `position` opens.
    pub fn tab_menu(
        mut self,
        builder: impl Fn(usize, Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.tab_menu = Some(Rc::new(builder));
        self
    }

    /// The menu the `+` opens: every kind of tab the surface can open, with its key.
    pub fn new_menu(
        mut self,
        builder: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.new_menu = Some(Rc::new(builder));
        self
    }

    /// Mouse parity for `ctrl-s c`, fired by the `+` when the strip has no
    /// [`Self::new_menu`].
    pub fn on_new(mut self, on_new: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_new = Some(Box::new(on_new));
        self
    }

    /// A control at the strip's right end: the surface's toggles, each a compact ghost button.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }
}

/// What every tab element of one strip shares.
struct TabHandlers {
    on_select: Option<PositionHandler>,
    on_close: Option<PositionHandler>,
    close_kbd: Option<Kbd>,
}

impl RenderOnce for TerminalTabStrip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Self {
            id: strip_id,
            tabs,
            active,
            show_plus,
            agents_from,
            on_select,
            on_close,
            close_kbd,
            tab_menu,
            new_menu,
            on_new,
            trailing,
        } = self;
        let agents_from = agents_from.unwrap_or(usize::MAX);

        let state = window.use_keyed_state(
            ElementId::NamedChild(std::sync::Arc::new(strip_id.clone()), "scroll-state".into()),
            cx,
            |_, _| TabStripState::default(),
        );
        let identities: Vec<_> = tabs.iter().map(TerminalTab::element_id).collect();
        let scroll = state.read(cx).scroll.clone();

        let handlers = Rc::new(TabHandlers {
            on_select,
            on_close,
            close_kbd,
        });
        let tab_theme = theme.clone();
        let tabs = tabs.into_iter().enumerate().map(move |(pos, tab)| {
            let part = if pos >= agents_from {
                "agents.tabs.tab"
            } else {
                "tabs.tab"
            };
            let element = tab_element(tab, pos, pos == active, part, &tab_theme, &handlers)
                .harness_target_indexed(part, pos);
            match &tab_menu {
                Some(builder) => {
                    let builder = Rc::clone(builder);
                    ContextMenu::new(("terminal-tab-menu", pos), element)
                        .menu(move |menu, window, cx| builder(pos, menu, window, cx))
                        .into_any_element()
                }
                None => element.into_any_element(),
            }
        });

        div()
            .id(strip_id)
            .relative()
            .flex()
            .flex_none()
            .items_end()
            .h(theme.metrics.tab_strip_h)
            .w_full()
            .px(theme.space.sm)
            .bg(theme.colors.chrome)
            // The strip's bottom hairline runs *behind* the tabs, so the active tab, which
            // reaches the bottom edge on the content ground, covers it and joins the content.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(theme.metrics.hairline)
                    .bg(theme.colors.border),
            )
            .child(
                div()
                    .on_children_prepainted(move |_, window, cx| {
                        state.update(cx, |state, _| state.reveal(active, &identities, window));
                    })
                    .id("tabs")
                    .flex()
                    .items_end()
                    .gap(theme.space.xxs)
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    .children(tabs),
            )
            .children(show_plus.then(|| new_tab_control(&theme, new_menu, on_new)))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.xxs)
                    .h_full()
                    .children(trailing),
            )
    }
}

/// One tab: the kind glyph, the name, its state mark and, when it can be closed, the `✕`.
fn tab_element(
    tab: TerminalTab,
    pos: usize,
    is_active: bool,
    part: &'static str,
    theme: &Theme,
    handlers: &Rc<TabHandlers>,
) -> gpui::Stateful<gpui::Div> {
    let hover_bg = theme.colors.row_hover;
    // The strip is a row of anonymous hit targets without this: the tab's own name is the only
    // thing that tells `nvim` from `cc` outside the pixels.
    let name = tab.name.clone();
    let tooltip_kbd = tab.kbd.clone();
    let index = tab.index;
    let close = handlers.on_close.as_ref().map(|on_close| {
        close_button(
            pos,
            is_active,
            part,
            Rc::clone(on_close),
            handlers.close_kbd.clone(),
            theme,
        )
    });
    let tooltip_delay = std::time::Duration::from_millis(theme.motion.tooltip_delay);

    div()
        .id(tab.element_id())
        .group(TAB_GROUP)
        .flex()
        .flex_none()
        .items_center()
        .gap(theme.space.xs)
        .min_w(theme.metrics.terminal_tab_min_w)
        .max_w(theme.metrics.terminal_tab_max_w)
        .h(theme.metrics.terminal_tab_h)
        .pl(theme.space.md)
        .pr(theme.space.xs)
        .rounded_t(theme.radii.control)
        .map(|el| {
            if is_active {
                // The active tab is cut from the content below it: same ground, a hairline on
                // three sides, and nothing underneath.
                el.bg(theme.colors.bg)
                    .border_t(theme.metrics.hairline)
                    .border_l(theme.metrics.hairline)
                    .border_r(theme.metrics.hairline)
                    .border_color(theme.colors.border)
            } else {
                el.hover(move |s| s.bg(hover_bg))
            }
        })
        // The index is the argument to `ctrl-s <n>`; it lives in the tooltip beside that key,
        // which is where the reader who wants it is already looking.
        .tooltip(move |window, cx| {
            Tooltip::new(SharedString::from(format!("Tab {index}")))
                .kbd(tooltip_kbd.clone())
                .build(window, cx)
        })
        .tooltip_show_delay(tooltip_delay)
        .when_some(handlers.on_select.clone(), |el, select| {
            let right = Rc::clone(&select);
            super::control::on_activate(el, name, move |window, cx| select(pos, window, cx))
                // A right-click is about this tab, so it selects it before its menu opens;
                // the menu's verbs then act on what the reader is looking at.
                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                    right(pos, window, cx);
                })
        })
        .when_some(handlers.on_close.clone(), |el, close| {
            el.on_aux_click(move |event, window, cx| {
                if event.is_middle_click() {
                    close(pos, window, cx);
                }
            })
        })
        .child(tab_body(tab, is_active, theme))
        .children(close)
}

/// The glyph, the name, then the status marks.
fn tab_body(tab: TerminalTab, is_active: bool, theme: &Theme) -> gpui::Div {
    let exited = tab.exited;
    let mark = tab.mark(is_active);
    let icon = tab.icon.unwrap_or_else(|| tab.kind.icon());
    let fg = if is_active {
        theme.colors.text
    } else {
        theme.colors.text_secondary
    };
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(theme.space.xs)
        .child(icon.el().size(IconSize::Medium).color(fg))
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
            // §2: "gray spinner (running), amber (needs you)". Colour is semantic, so progress
            // is gray everywhere on the strip; amber belongs to the attention chip alone.
            tab.starting.then(|| {
                Spinner::new("starting")
                    .size(IconSize::Small)
                    .tone(Tone::Secondary)
            }),
        )
        .children(tab.keep_alive.map(|icon| {
            icon.el()
                .size(IconSize::Small)
                .color(theme.colors.text_secondary)
        }))
        .children(tab.agent_status.map(|status| {
            StatusGlyph::new(status.status_kind())
                .size(IconSize::Small)
                .id("agent")
        }))
        .children(exited.map(|code| {
            Text::hint(TerminalTab::exit_label(code))
                .tone(Tone::Danger)
                .flex_none()
        }))
        .children(mark.map(|mark| {
            match mark {
                TabMark::NeedsYou => Chip::new()
                    .text("needs you")
                    .tone(Tone::Warning)
                    .filled(true)
                    .into_any_element(),
                TabMark::Unread => StatusDot::small(Tone::Info).into_any_element(),
            }
        }))
}

/// The `✕`: always on the active tab, on the others while they are hovered.
fn close_button(
    pos: usize,
    is_active: bool,
    part: &'static str,
    on_close: PositionHandler,
    kbd: Option<Kbd>,
    theme: &Theme,
) -> impl IntoElement {
    let hover_bg = theme.colors.control_hover;
    // Formatted only while the harness records, so production pays one flag read.
    let target = if harness::is_recording() {
        SharedString::from(format!("{part}[{pos}].close"))
    } else {
        SharedString::new_static("")
    };
    div()
        .id(("close", pos))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(theme.metrics.tab_close_size)
        .rounded(theme.radii.sm)
        .hover(move |s| s.bg(hover_bg))
        // Hidden, not removed: the box keeps its width, so hovering a tab never reflows the
        // strip.
        .when(!is_active, |el| {
            el.invisible()
                .group_hover(TAB_GROUP, |style| style.visible())
        })
        .map(|el| {
            super::control::on_click_named(el, "Close tab", move |_, window, cx| {
                cx.stop_propagation();
                on_close(pos, window, cx);
            })
        })
        .tooltip(move |window, cx| Tooltip::new("Close tab").kbd(kbd.clone()).build(window, cx))
        .tooltip_show_delay(std::time::Duration::from_millis(theme.motion.tooltip_delay))
        .child(
            Icon::X
                .el()
                .size(IconSize::Small)
                .color(theme.colors.text_secondary),
        )
        .harness_target(target)
}

/// The `+`: the new-tab menu, or `ctrl-s c` directly when the strip has no menu.
fn new_tab_control(
    theme: &Theme,
    new_menu: Option<NewMenuBuilder>,
    on_new: Option<NewHandler>,
) -> impl IntoElement {
    let control = match new_menu {
        Some(builder) => PopoverMenu::new("terminal-tab-new")
            .trigger_with(|open, _, _| {
                IconButton::new("terminal-tab-new-trigger", Icon::Plus, "New tab")
                    .size(super::ButtonSize::Compact)
                    .selected(open)
            })
            .menu(move |menu, window, cx| builder(menu, window, cx))
            .into_any_element(),
        None => {
            let button = IconButton::new("terminal-tab-new-trigger", Icon::Plus, "New terminal")
                .size(super::ButtonSize::Compact);
            match on_new {
                Some(on_new) => button
                    .on_click(move |_, window, cx| on_new(window, cx))
                    .into_any_element(),
                None => button.disabled(true).into_any_element(),
            }
        }
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .h_full()
        .ml(theme.space.xs)
        .child(control)
        .harness_target("tabs.new")
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

    #[test]
    fn one_mark_at_most_and_needs_you_outranks_unread() {
        let plain = TerminalTab::new(1, "sh");
        assert_eq!(plain.mark(false), None);
        assert_eq!(
            plain.clone().unread(true).mark(false),
            Some(TabMark::Unread)
        );
        assert_eq!(
            plain.clone().activity(true).mark(false),
            Some(TabMark::Unread)
        );
        assert_eq!(
            plain.clone().activity(true).attention(true).mark(false),
            Some(TabMark::NeedsYou)
        );
        // The tab you are looking at, and an exited tab, draw no dot at all.
        assert_eq!(plain.clone().unread(true).mark(true), None);
        assert_eq!(plain.clone().activity(true).mark(true), None);
        assert_eq!(plain.clone().unread(true).exited(0).mark(false), None);
        // …except a tab that is blocked on the reader: §3.3's chip survives selection, and
        // only the exit word still outranks it.
        assert_eq!(
            plain.clone().attention(true).mark(true),
            Some(TabMark::NeedsYou)
        );
        assert_eq!(plain.attention(true).exited(0).mark(true), None);
    }

    struct TestStrip {
        tabs: usize,
        selected: Option<usize>,
        closed: Option<usize>,
        created: bool,
    }

    impl Render for TestStrip {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            let weak = cx.weak_entity();
            let (new_weak, close_weak) = (weak.clone(), weak.clone());
            div().w(gpui::px(300.0)).child(
                TerminalTabStrip::new((0..self.tabs).map(|ix| TerminalTab::new(ix + 1, "sh")))
                    .active(self.tabs - 1)
                    .on_select(move |index, _, cx| {
                        weak.update(cx, |view, _| view.selected = Some(index))
                            .unwrap_or_else(|error| panic!("{error}"));
                    })
                    .on_close(move |index, _, cx| {
                        close_weak
                            .update(cx, |view, _| view.closed = Some(index))
                            .unwrap_or_else(|error| panic!("{error}"));
                    })
                    .on_new(move |_, cx| {
                        new_weak
                            .update(cx, |view, _| view.created = true)
                            .unwrap_or_else(|error| panic!("{error}"));
                    }),
            )
        }
    }

    #[gpui::test]
    fn overflow_reveals_the_active_tab_and_preserves_the_new_button(cx: &mut gpui::TestAppContext) {
        use gpui::{AppContext, Modifiers, MouseButton, point, px};
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| TestStrip {
                    tabs: 8,
                    selected: None,
                    closed: None,
                    created: false,
                })
            })
            .expect("test window")
        });
        let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
        let view = window.root(&mut cx).expect("test strip");
        cx.run_until_parked();
        // The tabs sit at the bottom of the 40 px strip; y = 30 is inside every one of them.
        let tab_y = px(30.0);
        cx.simulate_mouse_move(point(px(200.0), tab_y), None, Modifiers::none());
        cx.simulate_click(point(px(200.0), tab_y), Modifiers::none());
        view.read_with(&cx, |view, _| assert_eq!(view.selected, Some(7)));
        // A middle-click closes the tab under the pointer, and does not select it.
        cx.simulate_mouse_down(
            point(px(200.0), tab_y),
            MouseButton::Middle,
            Modifiers::none(),
        );
        cx.simulate_mouse_up(
            point(px(200.0), tab_y),
            MouseButton::Middle,
            Modifiers::none(),
        );
        view.read_with(&cx, |view, _| assert_eq!(view.closed, Some(7)));
        cx.simulate_click(point(px(279.0), px(20.0)), Modifiers::none());
        view.read_with(&cx, |view, _| assert!(view.created));
        view.update(&mut cx, |view, cx| {
            view.tabs = 2;
            view.created = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_mouse_move(point(px(250.0), tab_y), None, Modifiers::none());
        cx.simulate_click(point(px(195.0), px(20.0)), Modifiers::none());
        view.read_with(&cx, |view, _| assert!(view.created));
    }

    #[test]
    fn a_signal_killed_tab_says_killed_and_never_a_number() {
        assert_eq!(TerminalTab::exit_label(Some(1)).as_ref(), "exited 1");
        assert_eq!(TerminalTab::exit_label(None).as_ref(), "killed");
    }

    #[test]
    fn exited_accepts_both_a_code_and_none() {
        assert_eq!(TerminalTab::new(1, "cc").exited(1).exited, Some(Some(1)));
        assert_eq!(TerminalTab::new(1, "cc").exited(None).exited, Some(None));
    }

    /// `docs/DESIGN-SYSTEM.md` §8: "if a state is not in a gallery, it is not implemented".
    /// A `#[test]` inside `examples/*.rs` is compiled but never run, so the guard that every
    /// mark this component can draw is previewable lives here, beside the marks themselves.
    #[test]
    fn every_mark_this_strip_draws_appears_in_a_gallery_panel() {
        const MARKS: [&str; 7] = [
            ".attention(true)",
            ".unread(true)",
            ".starting(true)",
            ".exited(",
            "TerminalAgentState::Working",
            "TerminalAgentState::Finished",
            "TerminalTabKind::Native",
        ];
        const CONTROLS: [&str; 3] = [".on_close(", ".tab_menu(", ".new_menu("];
        for (gallery, source) in [
            (
                "gallery_terminal.rs",
                include_str!("../../examples/gallery_terminal.rs"),
            ),
            (
                "kit_gallery.rs",
                include_str!("../../examples/kit_gallery.rs"),
            ),
        ] {
            for mark in MARKS.iter().chain(CONTROLS.iter()) {
                assert!(
                    source.contains(mark),
                    "{gallery} has no tab-strip panel showing `{mark}`"
                );
            }
        }
    }

    #[test]
    fn agent_state_maps_only_working_and_finished() {
        assert_eq!(
            TerminalAgentState::Working.status_kind(),
            StatusKind::AgentWorking
        );
        assert_eq!(
            TerminalAgentState::Finished.status_kind(),
            StatusKind::AgentFinished
        );
    }
}
