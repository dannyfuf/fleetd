//! The closed choices and switches of §3.8.6, as one table: what a row offers, which option the
//! configuration holds, and how choosing an option or flipping a switch writes it back.
//!
//! The keyboard (`h`/`l`, `Space`) and the pointer (a segment, a dropdown option, a switch) both
//! land here, so a click and a key can never disagree about what a setting means.

use super::*;

/// The effort vocabulary every harness accepts, offered before any harness has declared its own.
const BASE_EFFORTS: &[&str] = &["low", "medium", "high"];

/// The reasoning efforts the Agents section offers per harness, lowest first.
///
/// [`BASE_EFFORTS`] always, then whatever a harness this app has opened declared beyond them, in
/// its own order. A configured effort outside the list stays shown (off the grid) until the row
/// is moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Efforts {
    claude: Vec<String>,
    codex: Vec<String>,
}

impl Default for Efforts {
    fn default() -> Self {
        let base: Vec<String> = BASE_EFFORTS
            .iter()
            .map(|effort| (*effort).to_owned())
            .collect();
        Self {
            claude: base.clone(),
            codex: base,
        }
    }
}

impl Efforts {
    /// The base vocabulary joined to what each harness has declared on this app's threads.
    #[must_use]
    pub fn declared(app: &AppState) -> Self {
        let mut efforts = Self::default();
        for (kind, list) in [
            (AgentKind::Claude, &mut efforts.claude),
            (AgentKind::Codex, &mut efforts.codex),
        ] {
            for effort in crate::dialogs::card_picker::declared_effort_ids(app, kind) {
                if !list.contains(&effort) {
                    list.push(effort);
                }
            }
        }
        efforts
    }

    /// The efforts offered for `kind`.
    #[must_use]
    pub fn of(&self, kind: AgentKind) -> &[String] {
        match kind {
            AgentKind::Claude => &self.claude,
            AgentKind::Codex => &self.codex,
        }
    }
}

/// The models each harness reported on this client's threads, for the Default model rows.
///
/// A harness declares its models on the threads it runs; there is no catalogue beyond them, so a
/// harness this app has not opened a thread for offers none and its row stays a text box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Models {
    claude: Vec<ModelOption>,
    codex: Vec<ModelOption>,
}

impl Models {
    /// Every model each harness has declared on this app's threads, first declaration first.
    #[must_use]
    pub fn declared(app: &AppState) -> Self {
        let mut models = Self::default();
        for (kind, list) in [
            (AgentKind::Claude, &mut models.claude),
            (AgentKind::Codex, &mut models.codex),
        ] {
            for model in crate::dialogs::card_picker::declared_models(app, Some(kind)) {
                let name = if model.display_name.trim().is_empty() {
                    model.id.clone()
                } else {
                    model.display_name
                };
                list.push(ModelOption { id: model.id, name });
            }
        }
        models
    }

    /// A catalogue from explicit lists, for tests.
    #[cfg(test)]
    #[must_use]
    pub fn from_lists(claude: Vec<ModelOption>, codex: Vec<ModelOption>) -> Self {
        Self { claude, codex }
    }

    /// The models offered for `kind`.
    #[must_use]
    pub fn of(&self, kind: AgentKind) -> &[ModelOption] {
        match kind {
            AgentKind::Claude => &self.claude,
            AgentKind::Codex => &self.codex,
        }
    }
}

/// The harness whose default model row `id` is, if it is one.
#[must_use]
pub const fn model_harness(id: &RowId) -> Option<AgentKind> {
    match id {
        RowId::ClaudeDefaultModel => Some(AgentKind::Claude),
        RowId::CodexDefaultModel => Some(AgentKind::Codex),
        _ => None,
    }
}

/// The label of the model option that leaves the model to the harness.
pub(super) const MODEL_DEFAULT: &str = "Harness default";

/// The Default model row of `kind`, drawn from the draft and the catalogue.
#[must_use]
pub fn model_of(config: &Config, kind: AgentKind, models: &Models) -> RowKind {
    let value = defaults_of(config, kind).model.clone().unwrap_or_default();
    let options = models.of(kind).to_vec();
    let shown = if value.is_empty() {
        MODEL_DEFAULT.to_owned()
    } else {
        options
            .iter()
            .find(|option| option.id == value)
            .map_or_else(|| value.clone(), |option| option.name.clone())
    };
    RowKind::Model {
        value,
        shown,
        options,
        harness: match kind {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        },
    }
}

/// Picks the default model of `kind`: `None` is Harness default, else option `index` of the
/// catalogue. An index past the end is ignored.
pub fn select_model(config: &mut Config, kind: AgentKind, index: Option<usize>, models: &Models) {
    let model = match index {
        None => None,
        Some(index) => match models.of(kind).get(index) {
            Some(option) => Some(option.id.clone()),
            None => return,
        },
    };
    defaults_of_mut(config, kind).model = model;
}

/// `←` / `→` on a Default model row: Harness default, then the catalogue in order, never
/// wrapping. A typed id the catalogue lacks steps from Harness default.
pub fn cycle_model(config: &mut Config, kind: AgentKind, delta: isize, models: &Models) {
    let offered = models.of(kind);
    if offered.is_empty() {
        return;
    }
    // Position 0 is Harness default; the catalogue follows from 1.
    let current = defaults_of(config, kind)
        .model
        .as_deref()
        .and_then(|model| offered.iter().position(|option| option.id == model))
        .map_or(0, |index| index + 1);
    let next = step(current, delta, offered.len() + 1);
    select_model(config, kind, next.checked_sub(1), models);
}

/// A closed choice as the row draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// Every option, as the user reads it, in cycling order.
    pub labels: Vec<String>,
    /// The option the configuration holds, or `None` when its value is off the grid.
    pub index: Option<usize>,
    /// The configured value as the user reads it, for an off-grid value.
    pub current: String,
}

impl Choice {
    fn new(labels: Vec<String>, index: Option<usize>, current: impl Into<String>) -> Self {
        Self {
            labels,
            index,
            current: current.into(),
        }
    }

    /// The row kind that draws this choice.
    #[must_use]
    pub fn kind(self) -> RowKind {
        let len = self.labels.len();
        RowKind::Choice {
            value: self
                .index
                .and_then(|index| self.labels.get(index).cloned())
                .unwrap_or(self.current),
            has_prev: self.index.is_some_and(|index| index > 0),
            has_next: self.index.is_some_and(|index| index + 1 < len),
            off_grid: self.index.is_none(),
            options: self.labels,
        }
    }
}

/// `claude` → `Claude`: an option label in sentence case.
fn sentence(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

const fn defaults_of(config: &Config, kind: AgentKind) -> &fleet_core::config::NativeAgentDefaults {
    match kind {
        AgentKind::Claude => &config.native_agents.claude,
        AgentKind::Codex => &config.native_agents.codex,
    }
}

fn defaults_of_mut(
    config: &mut Config,
    kind: AgentKind,
) -> &mut fleet_core::config::NativeAgentDefaults {
    match kind {
        AgentKind::Claude => &mut config.native_agents.claude,
        AgentKind::Codex => &mut config.native_agents.codex,
    }
}

/// The choice row `id` offers, or `None` when `id` is not a choice.
#[must_use]
pub fn choice_of(config: &Config, id: &RowId, efforts: &Efforts) -> Option<Choice> {
    let choice = match id {
        RowId::Agent => Choice::new(
            AGENTS.iter().map(|agent| sentence(agent)).collect(),
            AGENTS
                .iter()
                .position(|agent| *agent == agent_name(config.agent)),
            agent_name(config.agent),
        ),
        RowId::CloneProtocol => {
            let current = protocol_name(config.github.clone_protocol);
            Choice::new(
                PROTOCOLS.iter().map(|name| (*name).to_owned()).collect(),
                PROTOCOLS.iter().position(|name| *name == current),
                current,
            )
        }
        RowId::KeepFinishedFor => duration(config.jobs.keep_finished_for),
        RowId::TrashRetention => duration(config.trash.retention_ms),
        RowId::HotPoolSize => Choice::new(
            POOL_SIZES.iter().map(u64::to_string).collect(),
            POOL_SIZES
                .iter()
                .position(|size| *size == config.hot_pool_size),
            config.hot_pool_size.to_string(),
        ),
        RowId::ClaudeDefaultMode => mode(config, AgentKind::Claude),
        RowId::CodexDefaultMode => mode(config, AgentKind::Codex),
        RowId::ClaudeDefaultEffort => effort(config, AgentKind::Claude, efforts),
        RowId::CodexDefaultEffort => effort(config, AgentKind::Codex, efforts),
        _ => return None,
    };
    Some(choice)
}

fn duration(value: u64) -> Choice {
    Choice::new(
        DURATIONS
            .iter()
            .map(|step| format_cycler_duration(*step))
            .collect(),
        DURATIONS.iter().position(|step| *step == value),
        format_cycler_duration(value),
    )
}

fn mode(config: &Config, kind: AgentKind) -> Choice {
    let current = defaults_of(config, kind).mode;
    let modes = kind.supported_modes();
    Choice::new(
        modes
            .iter()
            .map(|mode| crate::screens::agent_thread::presentation::mode_label(*mode).to_owned())
            .collect(),
        modes.iter().position(|mode| *mode == current),
        crate::screens::agent_thread::presentation::mode_label(current),
    )
}

/// `Default` first — no effort configured, so the harness picks — then the offered efforts.
fn effort(config: &Config, kind: AgentKind, efforts: &Efforts) -> Choice {
    let offered = efforts.of(kind);
    let mut labels = vec![EFFORT_DEFAULT.to_owned()];
    labels.extend(offered.iter().map(|effort| sentence(effort)));
    match defaults_of(config, kind).effort.as_deref() {
        None => Choice::new(labels, Some(0), EFFORT_DEFAULT),
        Some(current) => Choice::new(
            labels,
            offered
                .iter()
                .position(|effort| effort == current)
                .map(|index| index + 1),
            current,
        ),
    }
}

/// The label of the effort option that leaves the effort to the harness.
pub(super) const EFFORT_DEFAULT: &str = "Default";

/// Writes option `index` of the choice row `id` into the draft. An index past the end is ignored.
pub fn select(config: &mut Config, id: &RowId, index: usize, efforts: &Efforts) {
    match id {
        RowId::Agent => {
            config.agent = match index {
                0 => Agent::Claude,
                1 => Agent::Codex,
                _ => return,
            };
        }
        RowId::CloneProtocol => {
            config.github.clone_protocol = match index {
                0 => CloneProtocol::Ssh,
                1 => CloneProtocol::Https,
                _ => return,
            };
        }
        RowId::KeepFinishedFor => {
            if let Some(step) = DURATIONS.get(index) {
                config.jobs.keep_finished_for = *step;
            }
        }
        RowId::TrashRetention => {
            if let Some(step) = DURATIONS.get(index) {
                config.trash.retention_ms = *step;
            }
        }
        RowId::HotPoolSize => {
            if let Some(size) = POOL_SIZES.get(index) {
                config.hot_pool_size = *size;
            }
        }
        RowId::ClaudeDefaultMode => select_mode(config, AgentKind::Claude, index),
        RowId::CodexDefaultMode => select_mode(config, AgentKind::Codex, index),
        RowId::ClaudeDefaultEffort => select_effort(config, AgentKind::Claude, index, efforts),
        RowId::CodexDefaultEffort => select_effort(config, AgentKind::Codex, index, efforts),
        _ => {}
    }
}

fn select_mode(config: &mut Config, kind: AgentKind, index: usize) {
    if let Some(mode) = kind.supported_modes().get(index) {
        defaults_of_mut(config, kind).mode = *mode;
    }
}

fn select_effort(config: &mut Config, kind: AgentKind, index: usize, efforts: &Efforts) {
    let effort = match index {
        0 => None,
        index => match efforts.of(kind).get(index - 1) {
            Some(effort) => Some(effort.clone()),
            None => return,
        },
    };
    defaults_of_mut(config, kind).effort = effort;
}

/// `←` / `→`: steps the choice `id`, never wrapping. An off-grid value steps from the first option.
pub fn cycle(config: &mut Config, id: &RowId, delta: isize, efforts: &Efforts) {
    let Some(choice) = choice_of(config, id, efforts) else {
        return;
    };
    let index = step(choice.index.unwrap_or(0), delta, choice.labels.len());
    select(config, id, index, efforts);
}

/// Whether the switch row `id` reads as on, or `None` when `id` is not a switch.
#[must_use]
pub fn switch_of(config: &Config, id: &RowId) -> Option<bool> {
    match id {
        RowId::SleepOnSwitch => Some(config.sleep.enabled),
        RowId::WarnBeforeQuit => Some(config.jobs.warn_before_quit),
        RowId::KeepAliveRule(index) => config.sleep.keep_alive.get(*index).map(|rule| rule.enabled),
        _ => None,
    }
}

/// `Space`: flips the switch `id`.
pub fn toggle(config: &mut Config, id: &RowId) {
    if let Some(on) = switch_of(config, id) {
        set_switch(config, id, !on);
    }
}

/// A click on the switch `id`: sets it to `on`.
pub fn set_switch(config: &mut Config, id: &RowId, on: bool) {
    match id {
        RowId::SleepOnSwitch => config.sleep.enabled = on,
        RowId::WarnBeforeQuit => config.jobs.warn_before_quit = on,
        RowId::KeepAliveRule(index) => {
            if let Some(rule) = config.sleep.keep_alive.get_mut(*index) {
                rule.enabled = on;
            }
        }
        _ => {}
    }
}
