//! The Columns section: the column vector as a draft, and the rows that edit it (§5.4).
//!
//! # Why the vector is a draft
//!
//! `fleet board columns` is a read-modify-write of the whole `statuses` vector, and so is this
//! pane: `n`, `d`, `J`/`K`, `P` and every row edit change a local `Vec<ColumnDraft>` and
//! nothing is sent until `^s`. That is what lets a user reorder three columns, rename one and
//! route another in one request instead of five, and what makes `esc` mean "discard" rather
//! than "undo four writes".
//!
//! # Why two rows keep their text beside the status
//!
//! `on enter` is spelled the way the CLI spells it — `none`, `prompt`, `skill:<name>[:<args>]`
//! — and `env` is one `KEY=VALUE` per line. Neither half-typed spelling fits in a `Status`:
//! `skil` is not an `ActionKind` and `FOO` is not a pair. Both therefore live as text on
//! [`ColumnDraft`] and are parsed by [`ColumnDraft::status`], which is also where their
//! refusals come from.

use super::*;

/// One row of a column's form, in the order contracts §5.4 fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ColumnField {
    /// Display name.
    Name,
    /// Which category the column counts as.
    Category,
    /// What the column runs on a card entering it.
    OnEnter,
    /// Which provider the run asks for.
    Provider,
    /// Which model the run asks for.
    Model,
    /// Which reasoning effort the run asks for.
    Effort,
    /// Which permission mode the run is given.
    Mode,
    /// Markdown prepended to the card's brief.
    Instructions,
    /// What the card is expected to have achieved.
    Expect,
    /// `KEY=VALUE` pairs handed to the run.
    Env,
    /// Where a succeeded card goes.
    OnSuccess,
    /// Where a card waiting here goes once nothing blocks it.
    WhenUnblocked,
}

/// The rows that exist only while the column runs something.
const ACTION_FIELDS: [ColumnField; 7] = [
    ColumnField::Provider,
    ColumnField::Model,
    ColumnField::Effort,
    ColumnField::Mode,
    ColumnField::Instructions,
    ColumnField::Expect,
    ColumnField::Env,
];

/// The categories, in cycle order.
const CATEGORIES: [StatusCategory; 5] = [
    StatusCategory::Backlog,
    StatusCategory::Unstarted,
    StatusCategory::Started,
    StatusCategory::Completed,
    StatusCategory::Canceled,
];

/// The permission modes, in cycle order after "column default".
const MODES: [PermissionMode; 6] = [
    PermissionMode::Ask,
    PermissionMode::AcceptEdits,
    PermissionMode::Plan,
    PermissionMode::Auto,
    PermissionMode::DontAsk,
    PermissionMode::FullAccess,
];

/// The providers, in cycle order after "column default".
const PROVIDERS: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];

/// What a row with no opinion of its own reads as.
const INHERITED: &str = "column default";
/// What an unset routing row reads as.
const ROUTE_OFF: &str = "off";
/// What an empty list reads as.
const EMPTY: &str = "\u{2014}";
/// What `on enter` says when the column runs nothing.
const ON_ENTER_NONE: &str = "none";
/// What `on enter` says when the column runs the card itself.
const ON_ENTER_PROMPT: &str = "prompt";
/// The prefix a skill action is spelled with, as `--on-enter` spells it.
const ON_ENTER_SKILL: &str = "skill:";

/// The refusal an `on enter` spelling that is none of the three earns.
///
/// `crates/fleet-cli/src/commands/board/columns.rs::parse_on_enter` refuses the same text with
/// the same words plus its flag name; neither sentence belongs to the daemon, because neither
/// spelling ever reaches the wire.
const ON_ENTER_RULE: &str = "on enter must be none, prompt, or skill:<name>[:<args>]";

impl ColumnField {
    /// The row label.
    #[must_use]
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Category => "Category",
            Self::OnEnter => "On enter",
            Self::Provider => "Provider",
            Self::Model => "Model",
            Self::Effort => "Effort",
            Self::Mode => "Mode",
            Self::Instructions => "Instructions",
            Self::Expect => "Expect",
            Self::Env => "Env",
            Self::OnSuccess => "On success",
            Self::WhenUnblocked => "When unblocked",
        }
    }

    /// Whether typing lands in this row instead of cycling a closed choice.
    #[must_use]
    pub(super) const fn is_text(self) -> bool {
        matches!(
            self,
            Self::Name
                | Self::OnEnter
                | Self::Model
                | Self::Effort
                | Self::Instructions
                | Self::Expect
                | Self::Env
        )
    }

    /// Whether the editor this row opens takes more than one line.
    #[must_use]
    pub(super) const fn is_multiline(self) -> bool {
        matches!(self, Self::Instructions | Self::Env)
    }

    /// Whether the row belongs to the automation a non-worktree board may not carry.
    ///
    /// Name and Category are the whole of what a context or Jira board keeps; everything else
    /// on the form describes a run, and a board that cannot run anything must not offer it.
    #[must_use]
    pub(super) const fn is_automation(self) -> bool {
        !matches!(self, Self::Name | Self::Category)
    }

    /// What an empty editor suggests.
    #[must_use]
    pub(super) const fn placeholder(self) -> &'static str {
        match self {
            Self::Name => "In review",
            Self::OnEnter => "none | prompt | skill:<name>",
            Self::Model | Self::Effort => INHERITED,
            Self::Instructions => "Prepended to the card's brief. Markdown.",
            Self::Expect => "the review finds no blocking issue",
            Self::Env => "KEY=VALUE, one per line",
            _ => "",
        }
    }
}

/// One column being edited, with the two rows whose text cannot live in a [`Status`].
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ColumnDraft {
    /// The column itself. Its `automation.on_enter` keeps the instructions, expectation and
    /// agent preferences even while [`Self::on_enter`] reads `none`, so cycling the action off
    /// and back on does not lose what was typed into it.
    pub(super) status: Status,
    /// The `on enter` row, as typed.
    pub(super) on_enter: String,
    /// The `env` row, one `KEY=VALUE` per line, as typed.
    pub(super) env: String,
}

impl ColumnDraft {
    /// Loads one stored column into an editable draft.
    #[must_use]
    pub(super) fn load(status: &Status) -> Self {
        let action = action_of(status);
        Self {
            on_enter: action
                .map_or_else(|| ON_ENTER_NONE.to_owned(), |action| spelling(&action.kind)),
            env: action.map_or_else(String::new, |action| action.env.join("\n")),
            status: status.clone(),
        }
    }

    /// Whether the column runs something, which is what the `⚡` mark and the seven action rows
    /// follow.
    #[must_use]
    pub(super) fn has_action(&self) -> bool {
        parse_spelling(&self.on_enter).is_ok_and(|kind| kind.is_some())
    }

    /// The column this draft would save, or the sentence that stops the save.
    pub(super) fn status(&self) -> Result<Status, String> {
        let kind = parse_spelling(&self.on_enter)?;
        let mut status = self.status.clone();
        let mut automation = status.automation.clone().unwrap_or_default();
        automation.on_enter = kind.map(|kind| {
            let mut action = action_of(&self.status).cloned().unwrap_or_else(|| Action {
                kind: kind.clone(),
                instructions: String::new(),
                expect: String::new(),
                env: Vec::new(),
                agent: ColumnAgentPrefs::default(),
            });
            action.kind = kind;
            action.env = env_entries(&self.env);
            action
        });
        // An emptied block is not automation: `normalise_automation` is what keeps a column the
        // user cleared from holding the document at version 2 (`fleet-core` §1.1).
        status.automation = Some(automation);
        let mut one = [status];
        normalise_automation(&mut one);
        let [status] = one;
        Ok(status)
    }

    /// The row's value, ready to draw.
    #[must_use]
    fn value(&self, field: ColumnField, columns: &[Self]) -> ColumnValue {
        let action = action_of(&self.status);
        let agent = action.map(|action| &action.agent);
        match field {
            ColumnField::Name => ColumnValue::text(self.status.name.clone(), false),
            ColumnField::Category => choice(
                category_word(self.status.category).to_owned(),
                position_of(&CATEGORIES, self.status.category),
                CATEGORIES.len(),
            ),
            ColumnField::OnEnter => ColumnValue::text(self.on_enter.clone(), true),
            ColumnField::Provider => {
                let provider = agent.and_then(|agent| agent.provider);
                choice(
                    provider.map_or_else(
                        || INHERITED.to_owned(),
                        |kind| provider_word(kind).to_owned(),
                    ),
                    provider.map_or(0, |kind| position_of(&PROVIDERS, kind) + 1),
                    PROVIDERS.len() + 1,
                )
            }
            ColumnField::Model => ColumnValue::text(
                agent
                    .and_then(|agent| agent.model.clone())
                    .unwrap_or_default(),
                false,
            ),
            ColumnField::Effort => ColumnValue::text(
                agent
                    .and_then(|agent| agent.effort.clone())
                    .unwrap_or_default(),
                false,
            ),
            ColumnField::Mode => {
                let mode = agent.and_then(|agent| agent.mode);
                choice(
                    mode.map_or_else(|| INHERITED.to_owned(), |mode| mode_word(mode).to_owned()),
                    mode.map_or(0, |mode| position_of(&MODES, mode) + 1),
                    MODES.len() + 1,
                )
            }
            // The row states the first line only: an instruction block is paragraphs long and
            // the form is a list of one-line facts.
            ColumnField::Instructions => ColumnValue::text(
                first_line(action.map_or("", |action| action.instructions.as_str())),
                false,
            ),
            ColumnField::Expect => ColumnValue::text(
                action.map_or(String::new(), |action| action.expect.clone()),
                false,
            ),
            ColumnField::Env => ColumnValue::text(one_line_env(&self.env), false),
            ColumnField::OnSuccess | ColumnField::WhenUnblocked => {
                route_value(self.route(field), columns)
            }
        }
    }

    /// The routing target of one of the two routing rows.
    #[must_use]
    fn route(&self, field: ColumnField) -> Option<&StatusId> {
        let automation = self.status.automation.as_ref()?;
        match field {
            ColumnField::WhenUnblocked => automation.advance_when_unblocked.as_ref(),
            _ => automation.on_success.as_ref(),
        }
    }
}

/// The rows the drilled-into column draws, in §5.4's order.
#[must_use]
pub(super) fn column_fields(column: &ColumnDraft) -> Vec<ColumnField> {
    let mut fields = vec![
        ColumnField::Name,
        ColumnField::Category,
        ColumnField::OnEnter,
    ];
    if column.has_action() {
        fields.extend(ACTION_FIELDS);
    }
    fields.extend([ColumnField::OnSuccess, ColumnField::WhenUnblocked]);
    fields
}

/// What one prepared row of the Columns pane draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ColumnValue {
    /// A list row: the column's name, and whether it runs anything.
    Column {
        /// The column's name.
        name: String,
        /// Whether the `⚡` mark is drawn after it.
        has_action: bool,
    },
    /// A closed list: `h` / `l` cycle it.
    Choice {
        /// The value shown.
        value: String,
        /// Whether a previous value exists.
        has_prev: bool,
        /// Whether a next value exists.
        has_next: bool,
    },
    /// Free text: `⏎` opens an editor over it.
    Text {
        /// The value shown, already cut to one line.
        value: String,
        /// Whether it is drawn in the mono face.
        mono: bool,
    },
}

impl ColumnValue {
    /// A text value, with `—` standing in for nothing at all.
    #[must_use]
    fn text(value: String, mono: bool) -> Self {
        Self::Text { value, mono }
    }
}

/// One prepared row of the Columns pane.
///
/// Every label, value and disabled flag is computed here, when the draft changes, so `render`
/// only picks a control (`docs/APP-CONTRACTS.md`: render prepares nothing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ColumnRow {
    /// Which row this is, for the cursor and the key handlers.
    pub(super) row: SettingRow,
    /// The row label.
    pub(super) label: String,
    /// What it draws.
    pub(super) value: ColumnValue,
    /// Whether the board may not carry this row at all.
    pub(super) disabled: bool,
}

/// Prepares the Columns pane: the list, or the drilled-into column's form.
#[must_use]
pub(super) fn prepare(
    columns: &[ColumnDraft],
    opened: Option<usize>,
    locked: bool,
) -> Vec<ColumnRow> {
    let Some(index) = opened.filter(|index| *index < columns.len()) else {
        return columns
            .iter()
            .enumerate()
            .map(|(index, column)| ColumnRow {
                row: SettingRow::Column(index),
                label: column.status.name.clone(),
                value: ColumnValue::Column {
                    name: column.status.name.clone(),
                    has_action: column.has_action(),
                },
                disabled: false,
            })
            .collect();
    };
    let column = &columns[index];
    column_fields(column)
        .into_iter()
        .map(|field| ColumnRow {
            row: SettingRow::ColumnField(field),
            label: field.label().to_owned(),
            value: column.value(field, columns),
            disabled: locked && field.is_automation(),
        })
        .collect()
}

/// `n`: a column named after nothing yet, ready to be renamed.
///
/// The id is generated from the name the way `fleet board columns add` generates it, so a
/// column added here and one added there are the same document.
#[must_use]
pub(super) fn new_column(columns: &[ColumnDraft]) -> Option<ColumnDraft> {
    let mut suffix = 1_u32;
    loop {
        let name = if suffix == 1 {
            "New column".to_owned()
        } else {
            format!("New column {suffix}")
        };
        let slug = fleet_core::slug::slugify(&name);
        if let Ok(id) = StatusId::try_from(slug)
            && !columns.iter().any(|column| column.status.id == id)
        {
            return Some(ColumnDraft::load(&Status {
                id,
                name,
                // A column nobody categorised is planned work: the one category that neither
                // starts a card's clock nor closes it. `columns add` chooses the same.
                category: StatusCategory::Unstarted,
                color: None,
                automation: None,
            }));
        }
        suffix = suffix.checked_add(1)?;
        if suffix > 64 {
            return None;
        }
    }
}

/// `P`: adds the preset columns this board is missing, by id, in preset order.
///
/// Returns whether anything changed, which is what the notice line reports: a preset that
/// added nothing has to say so, or `P` looks like a dead key on a board already shaped this way.
/// `template` is the board the dialog was seeded from. `apply_workflow_preset` takes a whole
/// `Board` and `Board` has no `Default` — every id on one is validated — so the board being
/// edited is what the candidate column vector is swapped into and back out of.
pub(super) fn apply_preset(template: &Board, columns: &mut Vec<ColumnDraft>) -> bool {
    let mut board = template.clone();
    board.statuses = columns.iter().map(|column| column.status.clone()).collect();
    if !apply_workflow_preset(&mut board) {
        return false;
    }
    // The preset only ever inserts, so every column already being edited keeps the text rows
    // it was carrying: they are matched back by id rather than by position.
    *columns = board
        .statuses
        .iter()
        .map(|status| {
            columns
                .iter()
                .find(|column| column.status.id == status.id)
                .cloned()
                .unwrap_or_else(|| ColumnDraft::load(status))
        })
        .collect();
    true
}

/// The first failing rule of the whole column vector, in the wording §1.7 states it in.
///
/// Routing, skill names, the Codex refusal, `env` and the throttle are all
/// `fleet_core::board::validate_automation`'s, so the dialog and the daemon cannot drift; only
/// the two spellings this pane invents — `on enter` and an empty name — are refused here.
#[must_use]
pub(super) fn validate(
    template: &Board,
    columns: &[ColumnDraft],
    max_live_runs: u32,
) -> Option<String> {
    let mut statuses = Vec::with_capacity(columns.len());
    for column in columns {
        if column.status.name.trim().is_empty() {
            return Some("a column needs a name".to_owned());
        }
        match column.status() {
            Ok(status) => statuses.push(status),
            Err(message) => return Some(message),
        }
    }
    let mut board = template.clone();
    board.statuses = statuses;
    board.settings.max_live_runs = Some(max_live_runs);
    validate_automation(&board).err().map(|error| match error {
        fleet_core::board::BoardError::Invalid { reason, .. } => reason,
        other => other.to_string(),
    })
}

/// The action a column runs, if it runs one.
#[must_use]
fn action_of(status: &Status) -> Option<&Action> {
    status
        .automation
        .as_ref()
        .and_then(|automation| automation.on_enter.as_ref())
}

/// How an action spells itself in the `on enter` row, as `--on-enter` spells it.
#[must_use]
fn spelling(kind: &ActionKind) -> String {
    match kind {
        ActionKind::Prompt => ON_ENTER_PROMPT.to_owned(),
        ActionKind::Skill { name, args } if args.is_empty() => format!("{ON_ENTER_SKILL}{name}"),
        ActionKind::Skill { name, args } => format!("{ON_ENTER_SKILL}{name}:{args}"),
    }
}

/// What the `on enter` row says, or the sentence that refuses it.
///
/// `skill:` with no name parses: the missing name is `validate_automation`'s refusal, and
/// repeating it here would let the two sentences drift.
fn parse_spelling(text: &str) -> Result<Option<ActionKind>, String> {
    match text.trim() {
        "" | ON_ENTER_NONE => Ok(None),
        ON_ENTER_PROMPT => Ok(Some(ActionKind::Prompt)),
        value => {
            let Some(rest) = value.strip_prefix(ON_ENTER_SKILL) else {
                return Err(ON_ENTER_RULE.to_owned());
            };
            let (name, args) = rest.split_once(':').unwrap_or((rest, ""));
            Ok(Some(ActionKind::Skill {
                name: name.to_owned(),
                args: args.to_owned(),
            }))
        }
    }
}

/// The `KEY=VALUE` entries a multi-line `env` row holds.
#[must_use]
fn env_entries(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// How a multi-line `env` row reads on one line.
#[must_use]
fn one_line_env(text: &str) -> String {
    let entries = env_entries(text);
    if entries.is_empty() {
        return EMPTY.to_owned();
    }
    entries.join(", ")
}

/// The first line of a block, which is all a one-line row can state.
#[must_use]
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_owned()
}

/// A routing row: `off`, or the column it points at.
#[must_use]
fn route_value(target: Option<&StatusId>, columns: &[ColumnDraft]) -> ColumnValue {
    let at = target.and_then(|target| {
        columns
            .iter()
            .position(|column| column.status.id == *target)
    });
    let value = at.map_or_else(
        || {
            target.map_or_else(
                || ROUTE_OFF.to_owned(),
                // A target the vector no longer holds — the column it named was deleted in this
                // same draft — is shown as the id it still is, never silently as `off`.
                |target| format!("\u{2192} {target}"),
            )
        },
        |at| format!("\u{2192} {}", columns[at].status.name),
    );
    choice(value, at.map_or(0, |at| at + 1), columns.len() + 1)
}

/// A cycler value with the arrows its position earns.
#[must_use]
fn choice(value: String, at: usize, len: usize) -> ColumnValue {
    ColumnValue::Choice {
        value,
        has_prev: at > 0,
        has_next: at + 1 < len,
    }
}

/// Where a value sits in its cycle, or the first position when it sits nowhere.
#[must_use]
fn position_of<T: PartialEq>(values: &[T], value: T) -> usize {
    values.iter().position(|entry| *entry == value).unwrap_or(0)
}

/// The wire name of a category, which is also the word `--category` takes.
#[must_use]
pub(super) const fn category_word(category: StatusCategory) -> &'static str {
    match category {
        StatusCategory::Backlog => "backlog",
        StatusCategory::Unstarted => "unstarted",
        StatusCategory::Started => "started",
        StatusCategory::Completed => "completed",
        StatusCategory::Canceled => "canceled",
    }
}

/// The word `--provider` takes.
#[must_use]
const fn provider_word(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    }
}

/// The word `--mode` takes.
#[must_use]
const fn mode_word(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::AcceptEdits => "accept-edits",
        PermissionMode::Plan => "plan",
        PermissionMode::Auto => "auto",
        PermissionMode::DontAsk => "dont-ask",
        PermissionMode::FullAccess => "full-access",
    }
}

/// `h` / `l` on a column row: step whatever closed choice it holds.
///
/// A locked row is refused here rather than at the key, so every path into it — the arrows,
/// the cyclers and a future palette command — refuses the same way.
pub(super) fn cycle_field(
    columns: &mut [ColumnDraft],
    index: usize,
    field: ColumnField,
    delta: isize,
    locked: bool,
) -> bool {
    if locked && field.is_automation() {
        return false;
    }
    let ids: Vec<StatusId> = columns
        .iter()
        .map(|column| column.status.id.clone())
        .collect();
    let Some(column) = columns.get_mut(index) else {
        return false;
    };
    match field {
        ColumnField::Category => {
            let at = position_of(&CATEGORIES, column.status.category);
            column.status.category = CATEGORIES[step(at, delta, CATEGORIES.len())];
        }
        // `on enter` is the one row that both cycles and types: the three legal spellings are a
        // closed set, but the skill's *name* is free text. `h` and `l` step the set — clamped,
        // like every other cycler in this dialog — and keep whatever name was last typed, so
        // cycling an action off and back on does not lose it.
        ColumnField::OnEnter => {
            let at = match column.on_enter.trim() {
                "" | ON_ENTER_NONE => 0,
                ON_ENTER_PROMPT => 1,
                _ => 2,
            };
            // The name is remembered even once the row reads `none`, because the column's
            // stored action is still holding it: `ColumnDraft::status` is what drops an
            // action, and it only runs on a save.
            let kept = if column.on_enter.trim().starts_with(ON_ENTER_SKILL) {
                Some(column.on_enter.trim().to_owned())
            } else {
                match action_of(&column.status).map(|action| &action.kind) {
                    Some(kind @ ActionKind::Skill { .. }) => Some(spelling(kind)),
                    _ => None,
                }
            };
            column.on_enter = match step(at, delta, 3) {
                0 => ON_ENTER_NONE.to_owned(),
                1 => ON_ENTER_PROMPT.to_owned(),
                _ => kept.unwrap_or_else(|| ON_ENTER_SKILL.to_owned()),
            };
        }
        ColumnField::Provider => {
            let agent = agent_mut(column);
            let at = agent
                .provider
                .map_or(0, |kind| position_of(&PROVIDERS, kind) + 1);
            let next = step(at, delta, PROVIDERS.len() + 1);
            agent.provider = next
                .checked_sub(1)
                .and_then(|at| PROVIDERS.get(at).copied());
        }
        ColumnField::Mode => {
            let agent = agent_mut(column);
            let at = agent.mode.map_or(0, |mode| position_of(&MODES, mode) + 1);
            let next = step(at, delta, MODES.len() + 1);
            agent.mode = next.checked_sub(1).and_then(|at| MODES.get(at).copied());
        }
        ColumnField::OnSuccess | ColumnField::WhenUnblocked => {
            let current = column.route(field).cloned();
            let at = current
                .as_ref()
                .and_then(|target| ids.iter().position(|id| id == target))
                .map_or(0, |at| at + 1);
            let next = step(at, delta, ids.len() + 1);
            let target = next.checked_sub(1).and_then(|at| ids.get(at).cloned());
            let automation = automation_mut(column);
            if field == ColumnField::WhenUnblocked {
                automation.advance_when_unblocked = target;
            } else {
                automation.on_success = target;
            }
        }
        // A text row is typed into, never stepped.
        _ => return false,
    }
    true
}

/// The text an editor over this row starts with.
#[must_use]
pub(super) fn field_text(column: &ColumnDraft, field: ColumnField) -> Option<String> {
    if !field.is_text() {
        return None;
    }
    let action = action_of(&column.status);
    Some(match field {
        ColumnField::Name => column.status.name.clone(),
        ColumnField::OnEnter => column.on_enter.clone(),
        ColumnField::Model => action
            .and_then(|action| action.agent.model.clone())
            .unwrap_or_default(),
        ColumnField::Effort => action
            .and_then(|action| action.agent.effort.clone())
            .unwrap_or_default(),
        ColumnField::Instructions => {
            action.map_or(String::new(), |action| action.instructions.clone())
        }
        ColumnField::Expect => action.map_or(String::new(), |action| action.expect.clone()),
        _ => column.env.clone(),
    })
}

/// Mirrors what was typed into a column row back into the draft.
pub(super) fn set_field_text(
    columns: &mut [ColumnDraft],
    index: usize,
    field: ColumnField,
    text: &str,
    locked: bool,
) {
    if locked && field.is_automation() {
        return;
    }
    let Some(column) = columns.get_mut(index) else {
        return;
    };
    match field {
        ColumnField::Name => column.status.name = text.to_owned(),
        ColumnField::OnEnter => column.on_enter = text.to_owned(),
        ColumnField::Env => column.env = text.to_owned(),
        ColumnField::Model => {
            agent_mut(column).model = (!text.trim().is_empty()).then(|| text.trim().to_owned());
        }
        ColumnField::Effort => {
            agent_mut(column).effort = (!text.trim().is_empty()).then(|| text.trim().to_owned());
        }
        ColumnField::Instructions => action_mut(column).instructions = text.to_owned(),
        ColumnField::Expect => action_mut(column).expect = text.to_owned(),
        _ => {}
    }
}

/// `J` / `K`: move one column past its neighbour, and report where the cursor followed it to.
#[must_use]
pub(super) fn reorder(columns: &mut [ColumnDraft], index: usize, delta: isize) -> Option<usize> {
    let target = index.checked_add_signed(delta)?;
    if index >= columns.len() || target >= columns.len() {
        return None;
    }
    columns.swap(index, target);
    Some(target)
}

/// The automation block of a column, created empty when it had none.
fn automation_mut(column: &mut ColumnDraft) -> &mut ColumnAutomation {
    column
        .status
        .automation
        .get_or_insert_with(ColumnAutomation::default)
}

/// The action of a column, created from its spelling when it had none.
///
/// Only ever called from a row that exists — the seven action rows are drawn exactly while
/// `has_action()` — so the kind it creates is immediately replaced by [`ColumnDraft::status`].
fn action_mut(column: &mut ColumnDraft) -> &mut Action {
    automation_mut(column)
        .on_enter
        .get_or_insert_with(|| Action {
            kind: ActionKind::Prompt,
            instructions: String::new(),
            expect: String::new(),
            env: Vec::new(),
            agent: ColumnAgentPrefs::default(),
        })
}

/// The agent preferences of a column's action.
fn agent_mut(column: &mut ColumnDraft) -> &mut ColumnAgentPrefs {
    &mut action_mut(column).agent
}
