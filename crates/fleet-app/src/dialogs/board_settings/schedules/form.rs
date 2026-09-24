//! The schedule form's model: its fields, the draft they edit, and the rows prepared from them.

use super::*;

/// One field of a schedule's form, in the order UX-SPEC's Board settings table fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::dialogs::board_settings) enum ScheduleField {
    /// The schedule's name.
    Name,
    /// Which harness runs it.
    Provider,
    /// Which model it asks for, free text.
    Model,
    /// Which reasoning effort it asks for, free text.
    Effort,
    /// The permission mode, cycled over the provider's supported modes.
    Mode,
    /// `every` or `once`.
    Cadence,
    /// Minutes between runs, for `every`.
    Every,
    /// The local time of the one run, for `once`.
    OnceAt,
    /// Minutes a run may take.
    Timeout,
    /// The prompt, multi-line.
    Prompt,
    /// Whether the schedule fires.
    Enabled,
}

impl ScheduleField {
    /// The row label.
    #[must_use]
    pub(in crate::dialogs::board_settings) const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Provider => "Provider",
            Self::Model => "Model",
            Self::Effort => "Effort",
            Self::Mode => "Mode",
            Self::Cadence => "Cadence",
            Self::Every => "Every",
            Self::OnceAt => "Once at",
            Self::Timeout => "Timeout",
            Self::Prompt => "Prompt",
            Self::Enabled => "Enabled",
        }
    }

    /// Whether `⏎` opens an editor over this row.
    #[must_use]
    pub(in crate::dialogs::board_settings) const fn is_text(self) -> bool {
        matches!(
            self,
            Self::Name
                | Self::Model
                | Self::Effort
                | Self::Every
                | Self::OnceAt
                | Self::Timeout
                | Self::Prompt
        )
    }

    /// Whether the editor takes more than one line: the prompt, in the column `Instructions`
    /// row's widget.
    #[must_use]
    pub(in crate::dialogs::board_settings) const fn is_multiline(self) -> bool {
        matches!(self, Self::Prompt)
    }

    /// Whether the editor takes digits only.
    #[must_use]
    pub(in crate::dialogs::board_settings) const fn is_number(self) -> bool {
        matches!(self, Self::Every | Self::Timeout)
    }

    /// Whether the editor draws in the mono face.
    #[must_use]
    pub(in crate::dialogs::board_settings) const fn is_mono(self) -> bool {
        matches!(self, Self::Every | Self::Timeout | Self::OnceAt)
    }

    /// What an empty editor suggests.
    #[must_use]
    pub(in crate::dialogs::board_settings) const fn placeholder(self) -> &'static str {
        match self {
            Self::Name => STARTER_NAME,
            Self::Model | Self::Effort => "provider default",
            Self::Every => "15",
            Self::OnceAt => "2026-09-23 09:00",
            Self::Timeout => "20",
            Self::Prompt => "What the agent does on every run. Markdown.",
            _ => "",
        }
    }
}

/// `every` or `once`, the form's `Cadence` cycler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::dialogs::board_settings) enum CadenceKind {
    /// On a fixed interval.
    Every,
    /// Exactly once.
    Once,
}

/// The two cadence kinds, in cycle order.
pub(super) const CADENCES: [CadenceKind; 2] = [CadenceKind::Every, CadenceKind::Once];

impl CadenceKind {
    /// The word the cycler shows, which is also `fleet schedule`'s flag name.
    #[must_use]
    pub(super) const fn word(self) -> &'static str {
        match self {
            Self::Every => "every",
            Self::Once => "once",
        }
    }
}

/// Every value the form edits, as typed.
///
/// Numbers and the one-off time stay text, as the backend rows' values do: a half-typed
/// number and an emptied field both need somewhere to live until `^s`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::dialogs::board_settings) struct ScheduleFields {
    /// The name.
    pub(in crate::dialogs::board_settings) name: String,
    /// The harness.
    pub(in crate::dialogs::board_settings) provider: AgentKind,
    /// The model, empty for the provider's default.
    pub(in crate::dialogs::board_settings) model: String,
    /// The effort, empty for the provider's default.
    pub(in crate::dialogs::board_settings) effort: String,
    /// The permission mode.
    pub(in crate::dialogs::board_settings) mode: PermissionMode,
    /// Which cadence the form shows.
    pub(in crate::dialogs::board_settings) cadence: CadenceKind,
    /// Minutes between runs, as typed.
    pub(in crate::dialogs::board_settings) every: String,
    /// The one run's local time, as typed.
    pub(in crate::dialogs::board_settings) once_at: String,
    /// Minutes a run may take, as typed.
    pub(in crate::dialogs::board_settings) timeout: String,
    /// The prompt.
    pub(in crate::dialogs::board_settings) prompt: String,
    /// Whether the schedule fires.
    pub(in crate::dialogs::board_settings) enabled: bool,
}

impl ScheduleFields {
    /// A new schedule's form: the starter on a Reviews board, an empty prompt elsewhere.
    #[must_use]
    pub(in crate::dialogs::board_settings) fn new(reviews: bool, now: DateTime<Local>) -> Self {
        let agent = ScheduleAgent::default();
        Self {
            name: if reviews {
                STARTER_NAME.to_owned()
            } else {
                String::new()
            },
            provider: agent.provider,
            model: String::new(),
            effort: String::new(),
            mode: agent.mode,
            cadence: CadenceKind::Every,
            every: DEFAULT_EVERY_MINUTES.to_string(),
            once_at: next_hour(now),
            timeout: SCHEDULE_DEFAULT_TIMEOUT_MINUTES.to_string(),
            prompt: if reviews {
                STARTER_PROMPT_GITHUB_REVIEWS.to_owned()
            } else {
                String::new()
            },
            enabled: true,
        }
    }

    /// A stored schedule's form.
    #[must_use]
    pub(in crate::dialogs::board_settings) fn load(
        schedule: &Schedule,
        now: DateTime<Local>,
    ) -> Self {
        let (cadence, every, once_at) = match &schedule.cadence {
            Cadence::Every { minutes } => (CadenceKind::Every, minutes.to_string(), next_hour(now)),
            Cadence::Once { at } => (
                CadenceKind::Once,
                DEFAULT_EVERY_MINUTES.to_string(),
                parse_time(at).map_or_else(
                    || at.clone(),
                    |at| at.with_timezone(&Local).format(ONCE_FORMAT).to_string(),
                ),
            ),
        };
        Self {
            name: schedule.name.clone(),
            provider: schedule.agent.provider,
            model: schedule.agent.model.clone().unwrap_or_default(),
            effort: schedule.agent.effort.clone().unwrap_or_default(),
            mode: schedule.agent.mode,
            cadence,
            every,
            once_at,
            timeout: schedule.timeout_minutes.to_string(),
            prompt: schedule.prompt.clone(),
            enabled: schedule.enabled,
        }
    }

    /// The cadence this form would send.
    ///
    /// A number that does not parse is sent as `0` and a time that does not parse is sent as
    /// typed, so the refusal is the daemon's own sentence (`every must be between 5 and 1440
    /// minutes`, `once needs an RFC 3339 time`) rather than a second wording of the same rule.
    #[must_use]
    pub(in crate::dialogs::board_settings) fn cadence(&self) -> Cadence {
        match self.cadence {
            CadenceKind::Every => Cadence::Every {
                minutes: number(&self.every),
            },
            CadenceKind::Once => Cadence::Once {
                at: once_at_wire(&self.once_at),
            },
        }
    }

    /// The agent this form would send.
    #[must_use]
    pub(in crate::dialogs::board_settings) fn agent(&self) -> ScheduleAgent {
        ScheduleAgent {
            provider: self.provider,
            model: optional(&self.model),
            effort: optional(&self.effort),
            mode: self.mode,
        }
    }

    /// The text an editor over `field` starts with, when it is a text row.
    #[must_use]
    pub(super) fn text(&self, field: ScheduleField) -> Option<String> {
        Some(match field {
            ScheduleField::Name => self.name.clone(),
            ScheduleField::Model => self.model.clone(),
            ScheduleField::Effort => self.effort.clone(),
            ScheduleField::Every => self.every.clone(),
            ScheduleField::OnceAt => self.once_at.clone(),
            ScheduleField::Timeout => self.timeout.clone(),
            ScheduleField::Prompt => self.prompt.clone(),
            _ => return None,
        })
    }

    /// Mirrors what was typed into a text row.
    pub(super) fn set_text(&mut self, field: ScheduleField, text: &str) {
        let slot = match field {
            ScheduleField::Name => &mut self.name,
            ScheduleField::Model => &mut self.model,
            ScheduleField::Effort => &mut self.effort,
            ScheduleField::Every => &mut self.every,
            ScheduleField::OnceAt => &mut self.once_at,
            ScheduleField::Timeout => &mut self.timeout,
            ScheduleField::Prompt => &mut self.prompt,
            _ => return,
        };
        text.clone_into(slot);
    }

    /// `h` / `l` on a closed choice; returns whether anything changed.
    ///
    /// Every step is clamped, as every other cycler in this dialog is.
    pub(super) fn cycle(&mut self, field: ScheduleField, delta: isize) -> bool {
        match field {
            ScheduleField::Provider => {
                let at = position_of(&PROVIDERS, self.provider);
                let next = PROVIDERS[step(at, delta, PROVIDERS.len())];
                if next == self.provider {
                    return false;
                }
                self.provider = next;
                // The model and the effort name the old harness's options, so they go back to
                // the new provider's defaults, as `fleet schedule edit --provider` does
                // (BOARD §12); every run would otherwise fail on a model the harness lacks.
                self.model.clear();
                self.effort.clear();
                // A mode the new harness does not accept would only come back as the daemon's
                // `{mode} is not supported by {provider}`; full access is the default and is
                // accepted by both.
                if !next.supported_modes().contains(&self.mode) {
                    self.mode = ScheduleAgent::default().mode;
                }
                true
            }
            ScheduleField::Mode => {
                let modes = self.provider.supported_modes();
                let at = position_of(modes, self.mode);
                let next = modes[step(at, delta, modes.len())];
                let changed = next != self.mode;
                self.mode = next;
                changed
            }
            ScheduleField::Cadence => {
                let at = position_of(&CADENCES, self.cadence);
                let next = CADENCES[step(at, delta, CADENCES.len())];
                let changed = next != self.cadence;
                self.cadence = next;
                changed
            }
            // `h` is off and `l` is on, as the dialog's other flag rows are.
            ScheduleField::Enabled => {
                let changed = self.enabled != (delta > 0);
                self.enabled = delta > 0;
                changed
            }
            _ => false,
        }
    }
}

/// The form the Schedules pane has drilled into.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::dialogs::board_settings) struct ScheduleForm {
    /// The schedule being edited, or `None` for a new one.
    pub(in crate::dialogs::board_settings) id: Option<ScheduleId>,
    /// The values being edited.
    pub(in crate::dialogs::board_settings) fields: ScheduleFields,
    /// The values the form opened with, for the dirty check and the patch.
    pub(in crate::dialogs::board_settings) baseline: ScheduleFields,
    /// The read-only `Last runs` block, newest first, prepared when the form opened.
    pub(in crate::dialogs::board_settings) runs: Vec<RunLine>,
}

impl ScheduleForm {
    /// Whether anything differs from what the form opened with.
    #[must_use]
    pub(in crate::dialogs::board_settings) fn dirty(&self) -> bool {
        self.fields != self.baseline
    }

    /// The rows the form draws, in order: the cadence shows only its own value row.
    #[must_use]
    pub(in crate::dialogs::board_settings) fn rows(&self) -> Vec<ScheduleField> {
        vec![
            ScheduleField::Name,
            ScheduleField::Provider,
            ScheduleField::Model,
            ScheduleField::Effort,
            ScheduleField::Mode,
            ScheduleField::Cadence,
            match self.fields.cadence {
                CadenceKind::Every => ScheduleField::Every,
                CadenceKind::Once => ScheduleField::OnceAt,
            },
            ScheduleField::Timeout,
            ScheduleField::Prompt,
            ScheduleField::Enabled,
        ]
    }

    /// The `UpdateSchedule` patch: only what the form changed, so a toggle made elsewhere
    /// while the form was open is not overwritten by a save about the prompt.
    #[must_use]
    pub(in crate::dialogs::board_settings) fn patch(&self) -> SchedulePatch {
        let (now, then) = (&self.fields, &self.baseline);
        SchedulePatch {
            name: (now.name != then.name).then(|| now.name.trim().to_owned()),
            prompt: (now.prompt != then.prompt).then(|| now.prompt.clone()),
            cadence: (now.cadence != then.cadence
                || now.every != then.every
                || now.once_at != then.once_at)
                .then(|| now.cadence()),
            agent: (now.agent() != then.agent()).then(|| now.agent()),
            enabled: (now.enabled != then.enabled).then_some(now.enabled),
            timeout_minutes: (now.timeout != then.timeout).then(|| number(&now.timeout)),
        }
    }

    /// The `CreateSchedule` draft, with every field set.
    #[must_use]
    pub(in crate::dialogs::board_settings) fn draft(&self, board_id: BoardId) -> ScheduleDraft {
        let fields = &self.fields;
        ScheduleDraft {
            board_id,
            name: fields.name.trim().to_owned(),
            prompt: fields.prompt.clone(),
            cadence: fields.cadence(),
            agent: Some(fields.agent()),
            enabled: Some(fields.enabled),
            timeout_minutes: Some(number(&fields.timeout)),
        }
    }
}

/// The form's rows with their values, ready to draw.
#[must_use]
pub(super) fn prepare_form(form: &ScheduleForm) -> Vec<ScheduleFormRow> {
    let fields = &form.fields;
    form.rows()
        .into_iter()
        .map(|field| ScheduleFormRow {
            field,
            value: match field {
                ScheduleField::Provider => choice(
                    provider_word(fields.provider),
                    position_of(&PROVIDERS, fields.provider),
                    PROVIDERS.iter().map(|kind| provider_word(*kind)),
                ),
                ScheduleField::Mode => {
                    let modes = fields.provider.supported_modes();
                    choice(
                        mode_word(fields.mode),
                        position_of(modes, fields.mode),
                        modes.iter().map(|mode| mode_word(*mode)),
                    )
                }
                ScheduleField::Cadence => choice(
                    fields.cadence.word(),
                    position_of(&CADENCES, fields.cadence),
                    CADENCES.iter().map(|cadence| cadence.word()),
                ),
                ScheduleField::Enabled => ScheduleValue::Flag(fields.enabled),
                ScheduleField::Every | ScheduleField::Timeout => ScheduleValue::Text {
                    value: fields
                        .text(field)
                        .filter(|text| !text.trim().is_empty())
                        .map_or_else(String::new, |text| format!("{} minutes", text.trim())),
                },
                // The prompt states its first line: it is paragraphs long, and the form is a
                // list of one-line facts, exactly as the column `Instructions` row is.
                _ => ScheduleValue::Text {
                    value: fields
                        .text(field)
                        .map(|text| text.lines().next().unwrap_or_default().trim().to_owned())
                        .unwrap_or_default(),
                },
            },
        })
        .collect()
}

/// A cycler value with its options.
#[must_use]
pub(super) fn choice<'a>(
    value: &str,
    at: usize,
    options: impl IntoIterator<Item = &'a str>,
) -> ScheduleValue {
    ScheduleValue::Choice {
        value: value.to_owned(),
        options: options.into_iter().map(str::to_owned).collect(),
        at,
    }
}

/// Where a value sits in its cycle, or the first position when it sits nowhere.
#[must_use]
pub(super) fn position_of<T: PartialEq>(values: &[T], value: T) -> usize {
    values.iter().position(|entry| *entry == value).unwrap_or(0)
}

/// A typed number, or `0` for anything that is not one: the daemon's range refusal names it.
#[must_use]
pub(super) fn number(text: &str) -> u32 {
    text.trim().parse().unwrap_or(0)
}

/// Free text as an optional wire value: empty is "the provider's default".
#[must_use]
pub(super) fn optional(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}
