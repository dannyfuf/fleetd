//! The ⌃S command menu (UX-SPEC §3.6): every command the held prefix reaches on this surface,
//! grouped, each one a clickable row led by its second key.
//!
//! The menu is built in the update pass once a held prefix has waited out its delay, never in
//! render: [`PrefixMenuState::reconcile`] reads the action catalogue for the surface's [`Place`] and keeps each entry only when its key
//! resolves, through the very resolver the shell's keystroke interceptor uses, to that entry's own
//! action on the live context chain. The menu therefore lists exactly the keys that would work,
//! and a click runs the same action the key would, after leaving the prefix the same way.
//!
//! It appears after `motion.prefix_hint_delay`: the expert types the second key first and never
//! sees it. It takes no focus, so the prefix keeps every key it had.

use std::{cell::Cell, rc::Rc, time::Duration};

use fleet_ui_kit::{
    ActiveTheme, Button, ButtonSize, ButtonStyle, HarnessTargetExt, Kbd, KbdSize, PrefixMenu,
    PrefixMenuColumn, PrefixMenuItem, Text,
};
use gpui::{Action, App, Entity, Keystroke, Task, Window, div, prelude::*};

use crate::{
    action_catalogue::{self, BindingSpec, Group, Place},
    keymap,
    state::AppState,
};

/// The heading the menu is titled with.
const TITLE: &str = "Fleet commands";

/// The columns the menu reads left to right when their entries are present. Any other heading an
/// entry is filed under follows them, in catalogue order.
const COLUMN_ORDER: &[Group] = &[
    Group::Tabs,
    Group::Session,
    Group::Terminal,
    Group::Agents,
    Group::Panels,
];

/// Which prefix is held: each surface has its own table and its own way of resolving the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrefixSurface {
    /// A Workspace terminal or Fleet-drawn tab, in the one-shot `Workspace > Prefix` context.
    Workspace,
    /// The floating agent terminal, in `Agent > Prefix`.
    AgentPopup,
    /// A native agent thread tab, whose `^s <key>` rows are chords on its own contexts.
    AgentThread,
}

impl PrefixSurface {
    /// The catalogue place whose entries this prefix reaches.
    const fn place(self) -> Place {
        match self {
            Self::Workspace => Place::Terminal,
            Self::AgentPopup => Place::AgentPopup,
            Self::AgentThread => Place::AgentThread,
        }
    }

    /// The one-shot context the second key is resolved in, for the two surfaces that publish
    /// one; a native thread resolves its chords against the live chain instead.
    const fn prefix_context(self) -> Option<&'static str> {
        match self {
            Self::Workspace => Some("Workspace > Prefix"),
            Self::AgentPopup => Some("Agent > Prefix"),
            Self::AgentThread => None,
        }
    }

    /// The prefix and the second key a binding spells, when it is a key this prefix takes.
    fn keys_of(self, spec: &BindingSpec) -> Option<(Option<Keystroke>, Keystroke)> {
        let strokes = spec
            .keys
            .split_whitespace()
            .map(Keystroke::parse)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        match (self.prefix_context(), strokes.as_slice()) {
            (Some(context), [second]) if spec.context == context => Some((None, second.clone())),
            (None, [prefix, second]) if keymap::is_prefix_key(prefix) => {
                Some((Some(prefix.clone()), second.clone()))
            }
            _ => None,
        }
    }

    /// What pressing `key` after the prefix runs here, exactly as the interceptor resolves it.
    fn resolve(self, chain: &[&'static str], key: &Keystroke) -> Option<Box<dyn Action>> {
        match self.prefix_context() {
            Some(context) => keymap::action_for_keystroke(context, key),
            None => keymap::chord_action_for_chain(chain, key),
        }
    }

    /// The action whose binding is the prefix itself, for the surfaces that enter a context.
    const fn enter_action(self) -> Option<&'static str> {
        match self {
            Self::Workspace => Some("workspace::EnterPrefix"),
            Self::AgentPopup => Some("agent::EnterPrefix"),
            Self::AgentThread => None,
        }
    }
}

/// One clickable command.
pub(crate) struct MenuItem {
    /// The key after the prefix.
    pub(crate) key: Keystroke,
    /// For a numbered range, its last key (`9` for `1`–`9`).
    pub(crate) range_end: Option<Keystroke>,
    /// The catalogue's short label.
    pub(crate) label: &'static str,
    /// What the key runs; a range runs its first member.
    action: Box<dyn Action>,
}

/// One heading and its commands.
pub(crate) struct MenuColumn {
    /// The catalogue heading.
    pub(crate) title: &'static str,
    /// The commands, in catalogue order.
    pub(crate) items: Vec<MenuItem>,
}

/// Everything one frame of the menu shows, built once per arming.
pub(crate) struct PrefixMenuModel {
    surface: PrefixSurface,
    chain: Vec<&'static str>,
    /// The held prefix, for the header chip.
    prefix: Option<Keystroke>,
    /// Whether pressing the prefix again sends it through to a program.
    literal: bool,
    /// The key and action that cancel the prefix, for the header's Close button.
    close: Option<(Keystroke, Box<dyn Action>)>,
    /// The columns, in reading order.
    pub(crate) columns: Vec<MenuColumn>,
}

impl PrefixMenuModel {
    /// Builds the menu for one surface on one live context chain.
    pub(crate) fn build(surface: PrefixSurface, chain: &[&'static str]) -> Self {
        let mut prefix = surface.enter_action().and_then(|action| {
            keymap::table()
                .into_iter()
                .find(|spec| spec.action == action)
                .and_then(|spec| Keystroke::parse(spec.keys).ok())
        });
        let mut literal = false;
        let mut close = None;
        let mut columns: Vec<(Group, Vec<MenuItem>)> = Vec::new();
        for placed in action_catalogue::for_place(surface.place()) {
            let mut keys: Vec<(Keystroke, Box<dyn Action>)> = Vec::new();
            for spec in &placed.bindings {
                let Some((held, key)) = surface.keys_of(spec) else {
                    continue;
                };
                // A row shadowed by a deeper one on this chain is not what the key does here.
                let Some(action) = surface
                    .resolve(chain, &key)
                    .filter(|action| action.name() == spec.action)
                else {
                    continue;
                };
                if prefix.is_none() {
                    prefix = held;
                }
                if keymap::is_prefix_key(&key) {
                    literal = true;
                } else if key.key == "escape" && !key.modifiers.modified() {
                    close.get_or_insert((key, action));
                } else if chorded(&key) {
                    // `cmd-c`, `ctrl-q`: keys that already work without the prefix and are only
                    // kept alive while it is held. The menu lists what the prefix adds.
                    continue;
                } else if !keys.iter().any(|(known, _)| *known == key) {
                    keys.push((key, action));
                }
            }
            let range_end = placed
                .entry
                .is_range()
                .then(|| keys.last().map(|(key, _)| key.clone()))
                .flatten();
            let Some((key, action)) = keys.into_iter().next() else {
                continue;
            };
            let item = MenuItem {
                range_end: range_end.filter(|last| *last != key),
                key,
                label: placed.info().short_label,
                action,
            };
            let group = placed.info().group;
            match columns.iter_mut().find(|(known, _)| *known == group) {
                Some((_, items)) => items.push(item),
                None => columns.push((group, vec![item])),
            }
        }
        // Stable: headings outside the preferred five keep their catalogue order.
        columns.sort_by_key(|(group, _)| {
            COLUMN_ORDER
                .iter()
                .position(|known| known == group)
                .unwrap_or(COLUMN_ORDER.len())
        });
        Self {
            surface,
            chain: chain.to_vec(),
            prefix,
            literal,
            close,
            columns: columns
                .into_iter()
                .map(|(group, items)| MenuColumn {
                    title: group.title(),
                    items,
                })
                .collect(),
        }
    }

    /// Every command in harness order: column by column, top to bottom.
    #[cfg(test)]
    pub(crate) fn items(&self) -> impl Iterator<Item = &MenuItem> {
        self.columns.iter().flat_map(|column| column.items.iter())
    }
}

/// The delayed reveal and the model of one surface's menu.
#[derive(Default)]
pub(crate) struct PrefixMenuState {
    visible: Rc<Cell<bool>>,
    task: Option<Task<()>>,
    model: Option<Rc<PrefixMenuModel>>,
}

impl PrefixMenuState {
    /// Whether the delay has run out and the menu is drawn.
    pub(crate) fn visible(&self) -> bool {
        self.visible.get()
    }

    /// Forgets the menu: the prefix was released.
    pub(crate) fn clear(&mut self) {
        self.task = None;
        self.model = None;
        self.visible.set(false);
    }

    /// Arms the menu while `active` names a held prefix and clears it otherwise.
    ///
    /// The delay starts once per prefix. The model is built by the update pass that follows the
    /// reveal, never in render, and rebuilt only if the live chain moves under a held prefix (a
    /// thread that starts working while `^s` is held). Building it on the reveal rather than on
    /// the arming keeps the expert's `^s a` free: the key after the prefix lands before the
    /// delay runs out, so that path never reads the catalogue at all.
    pub(crate) fn reconcile(
        &mut self,
        active: Option<PrefixSurface>,
        state: &Entity<AppState>,
        cx: &mut App,
    ) {
        let Some(surface) = active else {
            self.clear();
            return;
        };
        if self.visible() {
            let chain = state.read(cx).context_chain();
            if self
                .model
                .as_ref()
                .is_none_or(|model| model.surface != surface || model.chain != chain)
            {
                self.model = Some(Rc::new(PrefixMenuModel::build(surface, &chain)));
            }
        }
        if self.task.is_some() {
            return;
        }
        let visible = Rc::clone(&self.visible);
        let state = state.downgrade();
        let delay = Duration::from_millis(cx.theme().motion.prefix_hint_delay);
        self.task = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            visible.set(true);
            // The surface may be gone by now; then there is nothing left to reveal.
            if let Err(error) = state.update(cx, |_, cx| cx.notify()) {
                tracing::debug!(%error, "prefix menu revealed after its surface closed");
            }
        }));
    }

    /// The menu, once the delay has run out.
    pub(crate) fn render(&self, state: &Entity<AppState>, cx: &App) -> Option<PrefixMenu> {
        if !self.visible() {
            return None;
        }
        let model = self.model.as_ref()?;
        let theme = cx.theme();
        let surface = model.surface;
        let small = |key: &Keystroke| Kbd::new(std::slice::from_ref(key)).size(KbdSize::Small);
        let note = div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .child(Text::caption("Press a key or click."))
            .when_some(
                model.prefix.as_ref().filter(|_| model.literal),
                |note, prefix| {
                    note.child(small(prefix))
                        .child(Text::caption("again sends it to the terminal."))
                },
            );
        let mut menu = PrefixMenu::new("prefix-menu", TITLE)
            .harness("prefix_menu")
            .note(note);
        if let Some(prefix) = &model.prefix {
            menu = menu.prefix(Kbd::new(std::slice::from_ref(prefix)));
        }
        if let Some((key, action)) = &model.close {
            let state = state.clone();
            let action = action.boxed_clone();
            menu = menu.close(
                Button::new("prefix-menu-close", "Close")
                    .style(ButtonStyle::Ghost)
                    .size(ButtonSize::Compact)
                    // The ⌃S menu is a table of keys, so even its close keeps `esc` on its face.
                    .kbd(Kbd::new(std::slice::from_ref(key)))
                    .show_kbd()
                    .on_click(move |_, window, cx| {
                        run(surface, action.boxed_clone(), &state, window, cx);
                    })
                    .harness_target("prefix_menu.close"),
            );
        }
        let mut index = 0;
        for column in &model.columns {
            let mut built = PrefixMenuColumn::new(column.title);
            for item in &column.items {
                let state = state.clone();
                let action = item.action.boxed_clone();
                let row =
                    PrefixMenuItem::new(("prefix-menu-item", index), small(&item.key), item.label)
                        .on_click(move |_, window, cx| {
                            run(surface, action.boxed_clone(), &state, window, cx);
                        });
                let row = match &item.range_end {
                    Some(last) => row.range_end(small(last)),
                    None => row,
                };
                built = built.item(row.harness_target_indexed("prefix_menu.item", index));
                index += 1;
            }
            menu = menu.column(built);
        }
        Some(menu)
    }
}

/// Whether a key carries a modifier other than shift, which makes it a shortcut of its own rather
/// than a key typed after the prefix.
fn chorded(key: &Keystroke) -> bool {
    let modifiers = key.modifiers;
    modifiers.control || modifiers.alt || modifiers.platform || modifiers.function
}

/// Runs a clicked command exactly as its key would: the prefix is released first, then the
/// action is dispatched to the focused element, which is where the key's action goes too.
fn run(
    surface: PrefixSurface,
    action: Box<dyn Action>,
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        match surface {
            PrefixSurface::Workspace => {
                state.leave_prefix();
            }
            PrefixSurface::AgentPopup => {
                state.leave_agent_prefix();
            }
            PrefixSurface::AgentThread => state.agent_chord_armed = false,
        }
        cx.notify();
    });
    window.dispatch_action(action, cx);
}

#[cfg(test)]
mod tests;
