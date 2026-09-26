//! The model and effort vocabulary the column and schedule forms offer (§3.8.6, §5.4).
//!
//! A harness declares its models on the threads it runs, so the vocabulary is whatever this
//! client has seen each provider declare — the same source the card picker's Model rows and the
//! Settings Agents section read: every opened thread's projection, first thread first. It is read
//! once, when the dialog is seeded, so the forms' prepared rows can offer it without the render
//! reading the app (`docs/APP-CONTRACTS.md`, "render prepares nothing"). An empty catalogue is
//! the common case on a fresh client, and the Model row is then a text box, exactly as before.

use super::*;

/// The effort vocabulary every harness accepts, offered before any harness has declared its own.
const BASE_EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// One model a provider declared: its id, and the name the harness gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelChoice {
    /// The harness-native id, which is what a column or a schedule stores.
    pub(super) id: String,
    /// The harness-authored display name.
    pub(super) name: String,
}

/// The models and efforts each provider has declared on this client.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Catalogue {
    /// Claude's models, first thread first.
    claude: Vec<ModelChoice>,
    /// Codex's models, first thread first.
    codex: Vec<ModelChoice>,
    /// Claude's efforts beyond the base vocabulary.
    claude_efforts: Vec<String>,
    /// Codex's efforts beyond the base vocabulary.
    codex_efforts: Vec<String>,
}

impl Catalogue {
    /// Reads what every provider has declared on this client's threads.
    #[must_use]
    pub(super) fn read(app: &AppState) -> Self {
        let models = |provider| {
            crate::dialogs::card_picker::declared_models(app, Some(provider))
                .into_iter()
                .map(|model| ModelChoice {
                    id: model.id,
                    name: model.display_name,
                })
                .collect()
        };
        let efforts = |provider| {
            crate::dialogs::card_picker::declared_effort_ids(app, provider)
                .into_iter()
                .filter(|effort| !BASE_EFFORTS.contains(&effort.as_str()))
                .collect()
        };
        Self {
            claude: models(AgentKind::Claude),
            codex: models(AgentKind::Codex),
            claude_efforts: efforts(AgentKind::Claude),
            codex_efforts: efforts(AgentKind::Codex),
        }
    }

    /// The models `provider` declared; every provider's, deduplicated, for "column default".
    #[must_use]
    pub(super) fn models(&self, provider: Option<AgentKind>) -> Vec<&ModelChoice> {
        let lists: &[&Vec<ModelChoice>] = match provider {
            Some(AgentKind::Claude) => &[&self.claude],
            Some(AgentKind::Codex) => &[&self.codex],
            None => &[&self.claude, &self.codex],
        };
        let mut models: Vec<&ModelChoice> = Vec::new();
        for model in lists.iter().flat_map(|list| list.iter()) {
            if !models.iter().any(|seen| seen.id == model.id) {
                models.push(model);
            }
        }
        models
    }

    /// The efforts `provider` accepts, lowest first: the base vocabulary, then whatever it
    /// declared beyond it.
    #[must_use]
    pub(super) fn efforts(&self, provider: Option<AgentKind>) -> Vec<String> {
        let extra: &[&Vec<String>] = match provider {
            Some(AgentKind::Claude) => &[&self.claude_efforts],
            Some(AgentKind::Codex) => &[&self.codex_efforts],
            None => &[&self.claude_efforts, &self.codex_efforts],
        };
        let mut efforts: Vec<String> = BASE_EFFORTS
            .iter()
            .map(|effort| (*effort).to_owned())
            .collect();
        for effort in extra.iter().flat_map(|list| list.iter()) {
            if !efforts.contains(effort) {
                efforts.push(effort.clone());
            }
        }
        efforts
    }

    /// A catalogue holding exactly these models, for tests.
    #[cfg(test)]
    #[must_use]
    pub(super) fn with_models(provider: AgentKind, models: &[(&str, &str)]) -> Self {
        let list = models
            .iter()
            .map(|(id, name)| ModelChoice {
                id: (*id).to_owned(),
                name: (*name).to_owned(),
            })
            .collect();
        match provider {
            AgentKind::Claude => Self {
                claude: list,
                ..Self::default()
            },
            AgentKind::Codex => Self {
                codex: list,
                ..Self::default()
            },
        }
    }
}

/// A closed choice over a catalogue: the model row's dropdown, or an effort's segments.
///
/// `inherit` is the first option (what "no value" reads as); the stored value is matched by id,
/// and a value the catalogue does not hold sits off the grid, shown as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CatalogueChoice {
    /// What the control shows: an option's label, or the typed value off the grid.
    pub(super) value: String,
    /// Every option's label, in cycle order; for a model, then `Other model id…`.
    pub(super) options: Vec<String>,
    /// Every option's detail (a model id beside its name), aligned with `options`.
    pub(super) details: Vec<String>,
    /// The stored value of each steppable option: `None` for the inherited one.
    pub(super) values: Vec<Option<String>>,
    /// Where the value sits among the steppable options; `None` when it is off the grid.
    pub(super) at: Option<usize>,
    /// Whether the last option is `Other model id…`, which opens the editor.
    pub(super) other: bool,
}

/// The label of the option that opens the editor on a model row.
pub(super) const OTHER_MODEL: &str = "Other model id\u{2026}";
/// The detail beside [`OTHER_MODEL`].
const OTHER_MODEL_DETAIL: &str = "type it";

impl CatalogueChoice {
    /// The model row: `inherit`, each declared model, then `Other model id…`.
    ///
    /// `None` when the catalogue is empty, which leaves the row a text box.
    #[must_use]
    pub(super) fn model(
        catalogue: &Catalogue,
        provider: Option<AgentKind>,
        current: Option<&str>,
        inherit: &str,
    ) -> Option<Self> {
        let models = catalogue.models(provider);
        if models.is_empty() {
            return None;
        }
        let mut options = vec![inherit.to_owned()];
        let mut details = vec![String::new()];
        let mut values = vec![None];
        for model in models {
            let named = !model.name.trim().is_empty() && model.name != model.id;
            options.push(if named {
                model.name.clone()
            } else {
                model.id.clone()
            });
            details.push(if named {
                model.id.clone()
            } else {
                String::new()
            });
            values.push(Some(model.id.clone()));
        }
        options.push(OTHER_MODEL.to_owned());
        details.push(OTHER_MODEL_DETAIL.to_owned());
        Some(Self::positioned(options, details, values, current, true))
    }

    /// An effort row: `inherit`, then the provider's efforts.
    #[must_use]
    pub(super) fn effort(
        catalogue: &Catalogue,
        provider: Option<AgentKind>,
        current: Option<&str>,
        inherit: &str,
    ) -> Self {
        let efforts = catalogue.efforts(provider);
        let mut options = vec![inherit.to_owned()];
        let mut values = vec![None];
        for effort in efforts {
            options.push(effort.clone());
            values.push(Some(effort));
        }
        let details = vec![String::new(); options.len()];
        Self::positioned(options, details, values, current, false)
    }

    fn positioned(
        options: Vec<String>,
        details: Vec<String>,
        values: Vec<Option<String>>,
        current: Option<&str>,
        other: bool,
    ) -> Self {
        let current = current.map(str::trim).filter(|value| !value.is_empty());
        let at = values.iter().position(|value| value.as_deref() == current);
        let value = match (at, current) {
            (Some(at), _) => options.get(at).cloned().unwrap_or_default(),
            (None, Some(typed)) => typed.to_owned(),
            (None, None) => options.first().cloned().unwrap_or_default(),
        };
        Self {
            value,
            options,
            details,
            values,
            at,
            other,
        }
    }

    /// How many options `h` / `l` step through: every one but `Other model id…`.
    #[must_use]
    pub(super) fn steps(&self) -> usize {
        self.values.len()
    }

    /// The stored value `delta` steps away from the current one, clamped.
    ///
    /// From off the grid the step lands on the neighbour the arrow names: `l` on the first
    /// declared value, `h` on the inherited one.
    #[must_use]
    pub(super) fn stepped(&self, delta: isize) -> Option<String> {
        let next = match self.at {
            Some(at) => step(at, delta, self.steps()),
            None if delta > 0 => 1.min(self.steps().saturating_sub(1)),
            None => 0,
        };
        self.values.get(next).cloned().flatten()
    }
}
