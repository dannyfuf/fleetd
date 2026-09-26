//! The one row grammar of the settings shell (§3.8.6), as Board settings draws it: a
//! [`SettingsRow`] with a label, a helper and one of a handful of controls, every control wired
//! to the pointer twin of its key (ADR 0023).
//!
//! The panes (`view.rs`, `view/column_pane.rs`, `schedules/view.rs`) describe each row as a
//! [`RowSpec`] and hand it to [`settings_row`], so a switch, a choice or a box is drawn — and
//! answers a click — the same way in every section.

use gpui::MouseDownEvent;

use super::*;

/// Everything a click on this dialog needs, cloned into each handler that needs it.
#[derive(Clone)]
pub(in crate::dialogs::board_settings) struct Wire {
    pub(in crate::dialogs::board_settings) state: Entity<AppState>,
    pub(in crate::dialogs::board_settings) bridge: Bridge,
    pub(in crate::dialogs::board_settings) focus: FocusHandle,
    /// Where each cursor row is painted, for the reveal: the draft's [`RowBounds`].
    pub(in crate::dialogs::board_settings) bounds: RowBounds,
}

impl Wire {
    /// Row `index` wrapped so the pane knows where it was painted: the reveal scrolls to the
    /// cursor's *row*, because its card can be taller than the pane (`view.rs`). Every cursor
    /// row of every pane goes through here.
    pub(in crate::dialogs::board_settings) fn track(
        &self,
        index: usize,
        row: AnyElement,
    ) -> AnyElement {
        let bounds = self.bounds.clone();
        div()
            .w_full()
            .on_children_prepainted(move |children, _, _| {
                let mut bounds = bounds.borrow_mut();
                if bounds.len() <= index {
                    bounds.resize(index + 1, None);
                }
                bounds[index] = children.first().copied();
            })
            .child(row)
            .into_any_element()
    }

    /// A press on row `row`: the cursor lands there, as `j` / `k` would put it.
    pub(in crate::dialogs::board_settings) fn select(
        &self,
        row: usize,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        let wire = self.clone();
        move |_, window, cx| select_row(&wire.state, row, &wire.focus, window, cx)
    }

    /// A double-click on row `row`: the `⏎` that opens it.
    pub(in crate::dialogs::board_settings) fn open(
        &self,
        row: usize,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        let wire = self.clone();
        move |_, window, cx| open_row(&wire.state, row, &wire.focus, window, cx)
    }

    /// A click on row `row`'s value box: the `⏎` that opens its editor in place.
    pub(in crate::dialogs::board_settings) fn open_box(
        &self,
        row: usize,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let wire = self.clone();
        move |window, cx| open_row(&wire.state, row, &wire.focus, window, cx)
    }

    /// A closed choice's click, for the row showing option `current` (`None`: off the grid).
    ///
    /// On a model row the last option, `Other model id…`, opens the editor instead of choosing
    /// anything: `other` is its index.
    pub(in crate::dialogs::board_settings) fn pick(
        &self,
        row: usize,
        current: Option<usize>,
        other: Option<usize>,
    ) -> impl Fn(usize, &mut Window, &mut App) + 'static {
        let wire = self.clone();
        move |option, window, cx| {
            if other == Some(option) {
                open_row(&wire.state, row, &wire.focus, window, cx);
            } else {
                pick(&wire.state, row, option, current, &wire.focus, window, cx);
            }
        }
    }

    /// A flag's switch.
    pub(in crate::dialogs::board_settings) fn switch(
        &self,
        row: usize,
    ) -> impl Fn(bool, &mut Window, &mut App) + 'static {
        let wire = self.clone();
        move |on, window, cx| switch(&wire.state, row, on, &wire.focus, window, cx)
    }
}

/// A pane's children, with the child each cursor row sits in so the scroller can reveal it.
pub(in crate::dialogs::board_settings) struct PaneBlocks {
    /// The pane's children, top to bottom: a breadcrumb, cards, a callout.
    pub(in crate::dialogs::board_settings) blocks: Vec<AnyElement>,
    /// For each cursor row, in row order, the index of the block that holds it.
    pub(in crate::dialogs::board_settings) row_block: Vec<usize>,
}

impl PaneBlocks {
    pub(in crate::dialogs::board_settings) const fn new() -> Self {
        Self {
            blocks: Vec::new(),
            row_block: Vec::new(),
        }
    }

    /// Appends a block holding the next `rows` cursor rows.
    pub(in crate::dialogs::board_settings) fn push(
        &mut self,
        block: impl IntoElement,
        rows: usize,
    ) {
        let index = self.blocks.len();
        self.row_block.extend(std::iter::repeat_n(index, rows));
        self.blocks.push(block.into_any_element());
    }
}

/// A value drawn in a box that becomes its editor in place.
pub(in crate::dialogs::board_settings) struct BoxValue {
    /// The value at rest.
    pub(in crate::dialogs::board_settings) value: String,
    /// What empty means, drawn faint.
    pub(in crate::dialogs::board_settings) placeholder: &'static str,
    /// Whether the value is an identifier, drawn in the data face.
    pub(in crate::dialogs::board_settings) mono: bool,
    /// The muted unit after the value: `min`, `of 8`.
    pub(in crate::dialogs::board_settings) unit: Option<SharedString>,
    /// How wide the box is.
    pub(in crate::dialogs::board_settings) width: ValueBoxWidth,
    /// Whether it holds a block of lines; its row is then a tall one.
    pub(in crate::dialogs::board_settings) multiline: bool,
}

impl BoxValue {
    /// A single-line text box.
    pub(in crate::dialogs::board_settings) fn text(
        value: impl Into<String>,
        placeholder: &'static str,
    ) -> Self {
        Self {
            value: value.into(),
            placeholder,
            mono: false,
            unit: None,
            width: ValueBoxWidth::Text,
            multiline: false,
        }
    }

    /// Draw the value in the data face.
    pub(in crate::dialogs::board_settings) const fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// A number box with its unit.
    pub(in crate::dialogs::board_settings) fn number(mut self, unit: Option<SharedString>) -> Self {
        self.width = ValueBoxWidth::Number;
        self.unit = unit;
        self
    }

    /// A box of the given width.
    pub(in crate::dialogs::board_settings) const fn width(mut self, width: ValueBoxWidth) -> Self {
        self.width = width;
        self
    }

    /// A multi-line box.
    pub(in crate::dialogs::board_settings) const fn multiline(mut self, multiline: bool) -> Self {
        self.multiline = multiline;
        self
    }
}

/// How a settings row's control is drawn.
pub(in crate::dialogs::board_settings) enum Control {
    /// A switch, `Space`'s twin.
    Switch(bool),
    /// A closed choice drawn inline, segmented or as a dropdown by the kit's rule.
    Choice {
        /// The value shown: an option, or a typed value off the grid.
        value: String,
        /// Every option, in cycle order.
        options: Vec<String>,
        /// Each option's detail, aligned with `options`.
        details: Vec<String>,
        /// Where the value sits among the steppable options; `None` off the grid.
        at: Option<usize>,
        /// Whether the last option opens the editor instead (`Other model id…`).
        other: bool,
        /// Whether the set reads as a list whatever its size (a repository id), so it draws
        /// as a dropdown even when short enough to segment. An `other` set always does: an
        /// `Other model id…` segment would read as a model.
        dropdown: bool,
    },
    /// A value in a box.
    Box(BoxValue),
    /// A read-only value.
    Fact(String),
}

/// One cursor row of a pane: what [`settings_row`] draws.
pub(in crate::dialogs::board_settings) struct RowSpec {
    /// The row's position in the pane: its id, its harness name and its handlers' target.
    pub(in crate::dialogs::board_settings) index: usize,
    /// Whether the keyboard is on it.
    pub(in crate::dialogs::board_settings) cursor: bool,
    /// The setting's name.
    pub(in crate::dialogs::board_settings) label: SharedString,
    /// The sentence under it.
    pub(in crate::dialogs::board_settings) helper: Option<SharedString>,
    /// The rule it breaks, which replaces the helper and turns its box red.
    pub(in crate::dialogs::board_settings) invalid: Option<SharedString>,
    /// Whether the setting does not apply here.
    pub(in crate::dialogs::board_settings) disabled: bool,
    /// What sits at its end.
    pub(in crate::dialogs::board_settings) control: Control,
}

impl RowSpec {
    /// A row with no helper, no rule, enabled.
    pub(in crate::dialogs::board_settings) fn new(
        index: usize,
        cursor: bool,
        label: impl Into<SharedString>,
        control: Control,
    ) -> Self {
        Self {
            index,
            cursor,
            label: label.into(),
            helper: None,
            invalid: None,
            disabled: false,
            control,
        }
    }

    /// The sentence under the label.
    pub(in crate::dialogs::board_settings) fn helper(
        mut self,
        helper: Option<impl Into<SharedString>>,
    ) -> Self {
        self.helper = helper.map(Into::into);
        self
    }

    /// The rule the value breaks.
    pub(in crate::dialogs::board_settings) fn invalid(
        mut self,
        rule: Option<impl Into<SharedString>>,
    ) -> Self {
        self.invalid = rule.map(Into::into);
        self
    }

    /// Whether the setting does not apply here.
    pub(in crate::dialogs::board_settings) const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Draws one cursor row in the one row grammar, every control wired to the pointer.
///
/// `editor` is the live editor while one is open over the cursor row; a box draws it in place of
/// its value, and a typed choice (`On enter`, a model, an effort) draws the box that holds it in
/// place of its segments, so opening an editor never moves anything.
pub(in crate::dialogs::board_settings) fn settings_row(
    wire: &Wire,
    spec: RowSpec,
    editor: Option<&Entity<TextInput>>,
) -> AnyElement {
    let RowSpec {
        index,
        cursor,
        label,
        helper,
        invalid,
        disabled,
        control,
    } = spec;
    let editor = editor.filter(|_| cursor).cloned();
    let invalid_value = invalid.is_some();
    let tall = matches!(&control, Control::Box(value) if value.multiline);
    let control = match control {
        Control::Switch(on) => Switch::new(("board-settings-switch", index), on)
            .name(label.clone())
            .disabled(disabled)
            .on_toggle(wire.switch(index))
            .harness_target_named(cursor.then_some("board_settings.switch"))
            .into_any_element(),
        Control::Choice { .. } if editor.is_some() => value_box(
            wire,
            index,
            cursor,
            BoxValue::text(String::new(), ""),
            false,
            editor,
        ),
        Control::Choice {
            value,
            options,
            details,
            at,
            other,
            dropdown,
        } => {
            let steps = options.len().saturating_sub(usize::from(other));
            Cycler::new(value)
                .id(("board-settings-choice", index))
                .inline(true)
                .dropdown(dropdown || other)
                .options(options)
                .details(details)
                .has_prev(at.is_none_or(|at| at > 0))
                .has_next(at.is_none_or(|at| at + 1 < steps))
                .off_grid(at.is_none())
                .disabled(disabled)
                .on_select(wire.pick(index, at, other.then_some(steps)))
                .when(cursor, |cycler| {
                    cycler.harness("board_settings.option", "board_settings.dropdown")
                })
                .into_any_element()
        }
        Control::Box(value) => value_box(wire, index, cursor, value, invalid_value, editor),
        Control::Fact(value) => Text::ui(value).into_any_element(),
    };
    let row = SettingsRow::new(("board-settings-row", index))
        .cursor(cursor)
        .label(label)
        .when_some(helper, SettingsRow::helper)
        .when_some(invalid, SettingsRow::invalid)
        .disabled(disabled)
        .tall(tall)
        .control(control)
        .on_click(wire.select(index))
        .on_double_click(wire.open(index))
        .harness_target_indexed("board_settings.row", index)
        .into_any_element();
    wire.track(index, row)
}

/// A row's value box: the value at rest, or the live editor inside the same box.
fn value_box(
    wire: &Wire,
    index: usize,
    cursor: bool,
    value: BoxValue,
    invalid: bool,
    editor: Option<Entity<TextInput>>,
) -> AnyElement {
    let editing = editor.is_some();
    ValueBox::new(("board-settings-box", index), value.value)
        .placeholder(value.placeholder)
        .mono(value.mono)
        .width(value.width)
        .multiline(value.multiline)
        .invalid(invalid)
        .when_some(value.unit, ValueBox::unit)
        .when_some(editor, ValueBox::editor)
        // A click inside an open editor places its caret; only a box at rest opens.
        .when(!editing, |value_box| {
            value_box.on_click(wire.open_box(index))
        })
        .harness_target_named(cursor.then_some("board_settings.box"))
        .into_any_element()
}

/// A titled (or untitled) card of rows.
pub(in crate::dialogs::board_settings) fn card(
    id: impl Into<ElementId>,
    title: Option<&'static str>,
    rows: Vec<AnyElement>,
) -> SettingsCard {
    SettingsCard::new(id)
        .when_some(title, SettingsCard::title)
        .rows(rows)
}

/// One verb a list offers: a footer button, a row's hover action or menu item.
///
/// The footer, the hover actions and the right-click menu are all built from these tables, so a
/// verb cannot be offered by pointer in one place and missing from another (ADR 0023).
#[derive(Clone, Copy)]
pub(in crate::dialogs::board_settings) struct Verb {
    /// What the button or the menu item reads.
    pub(in crate::dialogs::board_settings) label: &'static str,
    /// Its glyph.
    pub(in crate::dialogs::board_settings) icon: Icon,
    /// The action its key runs; the button and the item dispatch it, so their chips are live.
    pub(in crate::dialogs::board_settings) action: fn() -> Box<dyn gpui::Action>,
    /// The harness name its control paints (on the cursor row, for a row's hover action);
    /// `None` for a verb that is only a menu item.
    pub(in crate::dialogs::board_settings) harness: Option<&'static str>,
    /// Whether it deletes something: the menu draws it in `danger`, after a separator.
    pub(in crate::dialogs::board_settings) destructive: bool,
}

/// A row's right-click menu over `verbs`, the destructive ones after a separator.
pub(in crate::dialogs::board_settings) fn verb_menu(menu: Menu, verbs: &[Verb]) -> Menu {
    verbs.iter().fold(menu, |menu, verb| {
        let menu = if verb.destructive {
            menu.separator()
        } else {
            menu
        };
        menu.item(
            MenuItem::new(verb.label)
                .icon(verb.icon)
                .destructive(verb.destructive)
                .action((verb.action)()),
        )
    })
}

/// A sentence as a row states it: the rule's own words, capitalised, with a full stop.
pub(in crate::dialogs::board_settings) fn sentence(rule: &str) -> SharedString {
    let mut chars = rule.chars();
    let mut sentence: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !sentence.ends_with('.') {
        sentence.push('.');
    }
    sentence.into()
}
