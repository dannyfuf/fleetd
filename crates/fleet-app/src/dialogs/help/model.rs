//! What Help shows, prepared off the frame: where it was opened, what can run there, and what a
//! search matches.
//!
//! Everything here is derived from the key table and the action catalogue. The static halves
//! (the searchable index of every shortcut and every guide) are built once per process; the
//! per-opening half ([`Here`]) is built when Help opens, and search [`Results`] are rebuilt when
//! the query, the tab or the place filter changes — never in `render` (`docs/APP-CONTRACTS.md`).

use std::{collections::HashMap, rc::Rc, sync::OnceLock};

use fleet_ui_kit::Kbd;
use gpui::{ScrollHandle, SharedString, UniformListScrollHandle};

use super::guides::GUIDES;
use crate::{
    action_catalogue::{self, BindingSpec, Entry, Place, context_title},
    keymap,
};

/// The action that opens Help, which Help never offers to run.
const OPEN_HELP: &str = "fleet::OpenHelp";

/// How many actions "Here in …" lists.
pub(crate) const HERE_ROWS: usize = 6;

/// How many actions a Guides-tab search lists under the guides it matched.
const GUIDE_SEARCH_ACTIONS: usize = 40;

/// A context whose keys follow a prefix key, and the action that arms the prefix.
///
/// `Workspace > Prefix` holds `s`, not `ctrl-s s`: the `ctrl-s` is its own binding, which is
/// what a person has to press first, so Help spells the pair out.
const PREFIXED: &[(&str, &str)] = &[
    ("Workspace > Prefix", "workspace::EnterPrefix"),
    ("Agent > Prefix", "agent::EnterPrefix"),
];

/// The key table, read once.
fn table() -> &'static [BindingSpec] {
    static TABLE: OnceLock<Vec<BindingSpec>> = OnceLock::new();
    TABLE.get_or_init(keymap::table)
}

/// The prefix key a prefixed context's keys follow, as the table binds it in the surface that
/// arms it (`Workspace > Terminal`, `Agent > Terminal`).
fn prefix_of(context: &str) -> Option<&'static str> {
    let (_, action) = PREFIXED.iter().find(|(known, _)| *known == context)?;
    table()
        .iter()
        .find(|spec| spec.action == *action && spec.context.ends_with("> Terminal"))
        .map(|spec| spec.keys)
}

/// What a person presses for one binding, in keymap syntax: `ctrl-s z` for `z` after the
/// prefix, the keys themselves everywhere else.
pub(crate) fn full_keys(spec: &BindingSpec) -> String {
    match prefix_of(spec.context) {
        Some(prefix) => format!("{prefix} {}", spec.keys),
        None => spec.keys.to_owned(),
    }
}

/// The place a surface chain belongs to, and the deepest context of it the catalogue knows.
fn place_of_chain(chain: &[&str]) -> Option<(Place, &'static str)> {
    (1..=chain.len()).rev().find_map(|len| {
        let context = chain[..len].join(" > ");
        let place = Place::of_context(&context)?;
        Some((place, context_title(&context)?))
    })
}

/// One of the actions featured in "Here in …".
#[derive(Debug, Clone)]
pub(crate) struct HereRow {
    /// The action a click runs.
    pub action: &'static str,
    /// What the catalogue calls it.
    pub label: SharedString,
    /// Its key where Help was opened.
    pub kbd: Option<Kbd>,
}

/// What can be run from the surface Help was opened over.
#[derive(Debug, Default)]
pub(crate) struct Here {
    /// "Here in Worktrees", "Here in Board": the sidebar heading, naming that surface.
    pub heading: SharedString,
    /// The place "you are here" marks in the Where list.
    pub place: Option<Place>,
    /// Every action whose key works there, with the key that runs it.
    keys: HashMap<&'static str, String>,
    /// The most useful of them, lowest rank first.
    pub featured: Vec<HereRow>,
}

impl Here {
    /// Resolves the key table against the chain of the surface under Help, exactly as gpui
    /// would: a binding is reachable when its context is a subsequence of the chain, and a
    /// deeper binding for the same keys shadows a shallower one.
    pub(crate) fn new(chain: &[&'static str]) -> Self {
        let mut stack: Vec<&str> = vec![keymap::ROOT_CONTEXT];
        stack.extend(chain.iter().copied());
        let table = table();

        // (depth, table order, action, keys): deepest first, then in table order.
        let mut reachable: Vec<(usize, usize, &'static str, String)> = table
            .iter()
            .enumerate()
            .filter_map(|(order, spec)| {
                let depth = keymap::context_depth(spec.context, &stack)?;
                Some((depth, order, spec.action, spec.keys.to_owned()))
            })
            .collect();
        // A prefixed context is never on the chain itself — it is the one key after the prefix —
        // so its keys are reachable wherever the prefix is, one level deeper than it.
        for (context, arming) in PREFIXED {
            let Some(&(depth, _, _, ref prefix)) = reachable
                .iter()
                .filter(|(_, _, action, _)| action == arming)
                .max_by_key(|(depth, ..)| *depth)
            else {
                continue;
            };
            let prefix = prefix.clone();
            reachable.extend(
                table
                    .iter()
                    .enumerate()
                    .filter(|(_, spec)| spec.context == *context)
                    .map(|(order, spec)| {
                        (
                            depth + 1,
                            order,
                            spec.action,
                            format!("{prefix} {}", spec.keys),
                        )
                    }),
            );
        }
        reachable.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

        let mut claimed: HashMap<String, &'static str> = HashMap::new();
        let mut keys: HashMap<&'static str, String> = HashMap::new();
        for (_, _, action, sequence) in reachable {
            if claimed.contains_key(&sequence) {
                continue;
            }
            claimed.insert(sequence.clone(), action);
            // Help's own key reaches the surface too, but running it from Help only reopens Help.
            if action != OPEN_HELP {
                keys.entry(action).or_insert(sequence);
            }
        }

        let mut featured: Vec<(u8, usize, HereRow)> = action_catalogue::entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.is_range())
            .filter_map(|(order, entry)| {
                let rank = entry.info.rank?;
                let sequence = keys.get(entry.action())?;
                Some((
                    rank,
                    order,
                    HereRow {
                        action: entry.action(),
                        label: SharedString::new_static(entry.info.label),
                        kbd: Kbd::parse(sequence).ok(),
                    },
                ))
            })
            .collect();
        featured.sort_by_key(|(rank, order, _)| (*rank, *order));

        let (place, title) = match place_of_chain(chain) {
            Some((Place::Hub, context)) => (Some(Place::Hub), context),
            Some((place, _)) => (Some(place), place.title()),
            None => (None, Place::Everywhere.title()),
        };
        Self {
            heading: format!("Here in {title}").into(),
            place,
            keys,
            featured: featured
                .into_iter()
                .take(HERE_ROWS)
                .map(|(_, _, row)| row)
                .collect(),
        }
    }

    /// Whether `action`'s key works where Help was opened.
    pub(crate) fn runs(&self, action: &str) -> bool {
        self.keys.contains_key(action)
    }

    /// The first of `actions` that runs here.
    pub(crate) fn first_runnable(&self, actions: &[&'static str]) -> Option<&'static str> {
        actions.iter().copied().find(|action| self.runs(action))
    }

    /// The key `action` has here, else its key in the place it is filed under.
    pub(crate) fn kbd(&self, action: &str) -> Option<Kbd> {
        match self.keys.get(action) {
            Some(sequence) => Kbd::parse(sequence).ok(),
            None => home_keys(action).and_then(|sequence| Kbd::parse(&sequence).ok()),
        }
    }
}

/// `action`'s key in the place the catalogue files it under, else its first key anywhere.
fn home_keys(action: &str) -> Option<String> {
    let place = action_catalogue::info(action)?.place;
    let table = table();
    let home = table
        .iter()
        .find(|spec| spec.action == action && Place::of_context(spec.context) == Some(place));
    home.or_else(|| table.iter().find(|spec| spec.action == action))
        .map(full_keys)
}

/// The prefix key, as a chip: what the terminal callout says Fleet's keys start with.
pub(crate) fn prefix_kbd() -> Option<Kbd> {
    Kbd::parse(prefix_of("Workspace > Prefix")?).ok()
}

/// The legend under the Where list: three real keys and how to press them.
pub(crate) fn legend() -> &'static [(Kbd, SharedString)] {
    static LEGEND: OnceLock<Vec<(Kbd, SharedString)>> = OnceLock::new();
    LEGEND.get_or_init(|| {
        let chips = |action: &str, context: &str| -> Option<Kbd> {
            let spec = table()
                .iter()
                .find(|spec| spec.action == action && spec.context == context)?;
            Kbd::parse(&full_keys(spec)).ok()
        };
        let sequence = |kbd: Kbd| -> Option<(Kbd, SharedString)> {
            let labels = kbd.chip_labels();
            let [first, then] = labels.as_slice() else {
                return None;
            };
            let text = format!("press {first}, then {then}");
            Some((kbd, text.into()))
        };
        [
            chips("native_agent::NewClaude", "Workspace > Prefix").and_then(sequence),
            chips("board::GoBoard", "Hub").and_then(sequence),
            chips("fleet::OpenAgentCodex", "Hub").map(|kbd| {
                let letter = kbd
                    .strokes()
                    .first()
                    .map(|stroke| stroke.key.to_lowercase())
                    .unwrap_or_default();
                let text = format!("hold Shift, press {letter}");
                (kbd, text.into())
            }),
        ]
        .into_iter()
        .flatten()
        .collect()
    })
}

/// One row of the All shortcuts table, prepared once per process.
#[derive(Debug)]
pub(crate) struct Shortcut {
    /// The catalogue entry.
    pub entry: &'static Entry,
    /// Its label.
    pub label: SharedString,
    /// The place it is filed under, and that place's name for the Where column.
    pub place: Place,
    /// The place's name.
    pub place_title: SharedString,
    /// Its key, spelled in full.
    pub kbd: Option<Kbd>,
    /// For a numbered range, the last member's key (`9`), drawn after a dash.
    pub range_end: Option<Kbd>,
    label_lc: String,
    description_lc: String,
    rest_lc: String,
}

impl Shortcut {
    /// The action a click or `⏎` runs, when it runs here. A numbered range never runs from
    /// Help: which of its nine members was meant is the one thing the row cannot say.
    pub(crate) fn runnable(&self, here: &Here) -> Option<&'static str> {
        (!self.entry.is_range())
            .then(|| self.entry.action())
            .filter(|action| here.runs(action))
    }
}

/// Every catalogued action with a key, in catalogue order.
pub(crate) fn shortcuts() -> &'static [Shortcut] {
    static INDEX: OnceLock<Vec<Shortcut>> = OnceLock::new();
    INDEX.get_or_init(|| {
        action_catalogue::all()
            .filter_map(|placed| {
                let info = placed.info();
                let home = placed
                    .bindings
                    .iter()
                    .find(|spec| Place::of_context(spec.context) == Some(info.place))
                    .or_else(|| placed.bindings.first())?;
                let range_end = placed
                    .entry
                    .is_range()
                    .then(|| {
                        let last = placed.entry.actions.last()?;
                        let spec = placed
                            .bindings
                            .iter()
                            .find(|spec| spec.action == *last && spec.context == home.context)?;
                        Kbd::parse(spec.keys.rsplit(' ').next()?).ok()
                    })
                    .flatten();
                let keys: Vec<String> = placed.bindings.iter().map(full_keys).collect::<Vec<_>>();
                Some(Shortcut {
                    entry: placed.entry,
                    label: SharedString::new_static(info.label),
                    place: info.place,
                    place_title: SharedString::new_static(info.place.title()),
                    kbd: Kbd::parse(&full_keys(home)).ok(),
                    range_end,
                    label_lc: info.label.to_lowercase(),
                    description_lc: info.description.to_lowercase(),
                    rest_lc: format!("{} {}", info.place.title(), keys.join(" ")).to_lowercase(),
                })
            })
            .collect()
    })
}

/// A guide's searchable text, lowercased once.
struct GuideText {
    title: String,
    summary: String,
    rest: String,
}

fn guide_texts() -> &'static [GuideText] {
    static TEXTS: OnceLock<Vec<GuideText>> = OnceLock::new();
    TEXTS.get_or_init(|| {
        GUIDES
            .iter()
            .map(|guide| {
                let mut rest = String::from(guide.intro);
                for step in guide.steps {
                    rest.push(' ');
                    rest.push_str(step.title);
                    rest.push(' ');
                    rest.push_str(step.body);
                    for button in step.actions {
                        rest.push(' ');
                        rest.push_str(button.label);
                    }
                }
                GuideText {
                    title: guide.title.to_lowercase(),
                    summary: guide.summary.to_lowercase(),
                    rest: rest.to_lowercase(),
                }
            })
            .collect()
    })
}

/// How well a text matches: `0` the whole query in the title, `1` every word in the title, `2`
/// every word in the title or the second field, `3` every word somewhere; `None` otherwise.
fn score(query: &str, words: &[&str], title: &str, second: &str, rest: &str) -> Option<u8> {
    if words.is_empty() {
        return Some(3);
    }
    let everywhere =
        |word: &&str| title.contains(word) || second.contains(word) || rest.contains(word);
    if !words.iter().all(everywhere) {
        return None;
    }
    if title.contains(query) {
        Some(0)
    } else if words.iter().all(|word| title.contains(word)) {
        Some(1)
    } else if words
        .iter()
        .all(|word| title.contains(word) || second.contains(word))
    {
        Some(2)
    } else {
        Some(3)
    }
}

/// What a query matches.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Results {
    /// Indices into [`GUIDES`], best first.
    pub guides: Vec<usize>,
    /// Indices into [`shortcuts`] in the place filter, best first.
    pub shortcuts: Vec<usize>,
    /// How many shortcuts match in every place: `[0]` is all places, then [`Place::ALL`].
    pub counts: Vec<usize>,
}

/// Searches the guides and every shortcut. `place` narrows the shortcuts; `None` is every place
/// but *Editing text*, whose forty text-field keys are there when asked for and not otherwise.
pub(crate) fn search(query: &str, place: Option<Place>) -> Results {
    let query = query.trim().to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();

    let mut guides: Vec<(u8, usize)> = guide_texts()
        .iter()
        .enumerate()
        .filter_map(|(ix, text)| {
            score(&query, &words, &text.title, &text.summary, &text.rest).map(|score| (score, ix))
        })
        .collect();
    guides.sort();

    let mut counts = vec![0; Place::ALL.len() + 1];
    let mut matched: Vec<(u8, usize)> = Vec::new();
    for (ix, row) in shortcuts().iter().enumerate() {
        let Some(score) = score(
            &query,
            &words,
            &row.label_lc,
            &row.description_lc,
            &row.rest_lc,
        ) else {
            continue;
        };
        let place_ix = Place::ALL.iter().position(|known| *known == row.place);
        if let Some(place_ix) = place_ix {
            counts[place_ix + 1] += 1;
        }
        if row.place != Place::EditingText {
            counts[0] += 1;
        }
        let shown = match place {
            Some(place) => row.place == place,
            None => row.place != Place::EditingText,
        };
        if shown {
            matched.push((score, ix));
        }
    }
    // An empty query keeps the catalogue's own order; a search puts the best matches first.
    if !words.is_empty() {
        matched.sort();
    }
    Results {
        guides: guides.into_iter().map(|(_, ix)| ix).collect(),
        shortcuts: matched.into_iter().map(|(_, ix)| ix).collect(),
        counts,
    }
}

/// Help's two halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    /// "Here in …", the guide list and the selected guide.
    #[default]
    Guides,
    /// Every shortcut, by place, searchable.
    Shortcuts,
}

impl Tab {
    /// The other one.
    pub(crate) const fn other(self) -> Self {
        match self {
            Self::Guides => Self::Shortcuts,
            Self::Shortcuts => Self::Guides,
        }
    }

    /// Its position in the tab switch.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Guides => 0,
            Self::Shortcuts => 1,
        }
    }
}

/// One row the cursor can stand on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    /// A "Here in …" row, by position.
    Here(usize),
    /// A guide, by its index in [`GUIDES`].
    Guide(usize),
    /// A shortcut, by its index in [`shortcuts`].
    Shortcut(usize),
}

/// Help's draft for one opening.
#[derive(Debug)]
pub(crate) struct HelpState {
    /// Which half is showing.
    pub tab: Tab,
    /// The search query, mirrored from the field.
    pub query: String,
    /// What runs where Help was opened. Shared with the table's row builder, which outlives
    /// the frame that made it.
    pub here: Rc<Here>,
    /// The guide the Guides tab shows when nothing is searched.
    pub guide: usize,
    /// The Where filter; `None` is every place.
    pub place: Option<Place>,
    /// What the query matches.
    pub results: Results,
    /// The rows of the list the cursor moves in, in order.
    pub items: Vec<Item>,
    /// The cursor, an index into [`Self::items`].
    pub cursor: usize,
    /// The sidebar's scroll, when a search fills it.
    pub side_scroll: ScrollHandle,
    /// The guide column's scroll.
    pub guide_scroll: ScrollHandle,
    /// The shortcut table's scroll.
    pub table_scroll: UniformListScrollHandle,
}

impl HelpState {
    /// A fresh opening over the surface whose chain is `chain`.
    pub(crate) fn new(chain: &[&'static str]) -> Self {
        let mut state = Self {
            tab: Tab::Guides,
            query: String::new(),
            here: Rc::new(Here::new(chain)),
            guide: 0,
            place: None,
            results: Results::default(),
            items: Vec::new(),
            cursor: 0,
            side_scroll: ScrollHandle::new(),
            guide_scroll: ScrollHandle::new(),
            table_scroll: UniformListScrollHandle::new(),
        };
        state.refresh();
        state
    }

    /// Whether a search is narrowing what shows.
    pub(crate) fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Recomputes the matches and the rows, and puts the cursor where a new list starts.
    pub(crate) fn refresh(&mut self) {
        let place = match self.tab {
            Tab::Guides => None,
            Tab::Shortcuts => self.place,
        };
        self.results = search(&self.query, place);
        self.items = match (self.tab, self.searching()) {
            (Tab::Guides, false) => (0..self.here.featured.len())
                .map(Item::Here)
                .chain((0..GUIDES.len()).map(Item::Guide))
                .collect(),
            (Tab::Guides, true) => self
                .results
                .guides
                .iter()
                .copied()
                .map(Item::Guide)
                .chain(
                    self.results
                        .shortcuts
                        .iter()
                        .copied()
                        .take(GUIDE_SEARCH_ACTIONS)
                        .map(Item::Shortcut),
                )
                .collect(),
            (Tab::Shortcuts, _) => self
                .results
                .shortcuts
                .iter()
                .copied()
                .map(Item::Shortcut)
                .collect(),
        };
        self.cursor = self.default_cursor();
    }

    /// Where the cursor starts: on the shown guide; on a search's best action that runs here,
    /// so `?`, a word and `⏎` does the thing; else on the first row.
    fn default_cursor(&self) -> usize {
        match (self.tab, self.searching()) {
            (Tab::Guides, false) => self.here.featured.len() + self.guide,
            (Tab::Guides, true) => self
                .items
                .iter()
                .position(|item| self.runnable(*item).is_some())
                .unwrap_or(0),
            (Tab::Shortcuts, _) => 0,
        }
    }

    /// The row under the cursor.
    pub(crate) fn selected(&self) -> Option<Item> {
        self.items.get(self.cursor).copied()
    }

    /// The guide the content column shows: the one under the cursor, else the last one shown.
    pub(crate) fn shown_guide(&self) -> usize {
        match self.selected() {
            Some(Item::Guide(guide)) => guide,
            _ => self.guide,
        }
    }

    /// Moves the cursor by `delta`, clamped, and remembers a guide it lands on.
    pub(crate) fn move_cursor(&mut self, delta: isize) {
        self.set_cursor(crate::state::move_cursor(
            self.cursor,
            delta,
            self.items.len(),
        ));
    }

    /// Puts the cursor on row `cursor`, clamped, and remembers a guide it lands on.
    pub(crate) fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.items.len().saturating_sub(1));
        if let Some(Item::Guide(guide)) = self.selected() {
            self.guide = guide;
        }
    }

    /// The action `item` runs from here, if it runs here.
    pub(crate) fn runnable(&self, item: Item) -> Option<&'static str> {
        match item {
            Item::Here(ix) => self.here.featured.get(ix).map(|row| row.action),
            Item::Guide(_) => None,
            Item::Shortcut(ix) => shortcuts().get(ix)?.runnable(&self.here),
        }
    }

    /// Switches to `tab`, keeping the query.
    pub(crate) fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.refresh();
        }
    }

    /// Narrows the table to `place` (`None`: every place).
    pub(crate) fn set_place(&mut self, place: Option<Place>) {
        if self.place != place {
            self.place = place;
            self.refresh();
        }
    }

    /// Takes a new query from the field.
    pub(crate) fn set_query(&mut self, query: String) {
        if self.query != query {
            self.query = query;
            self.refresh();
        }
    }

    /// Shows `guide` on the Guides tab, as if it had been clicked in the list.
    pub(crate) fn open_guide(&mut self, guide: usize) {
        self.guide = guide.min(GUIDES.len().saturating_sub(1));
        self.tab = Tab::Guides;
        self.query.clear();
        self.refresh();
    }

    /// The guide the query matches best, for the table's "Related guide".
    pub(crate) fn related_guide(&self) -> Option<usize> {
        self.searching()
            .then(|| self.results.guides.first().copied())
            .flatten()
    }
}

#[cfg(test)]
mod tests;
