//! Claude's runtime model catalogue.

use fleet_core::agents::{ModelDescriptor, ReasoningEffortDescriptor};
use serde_json::Value;

const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Selectable descriptors plus Claude's selector-to-resolved-model aliases.
#[derive(Debug, Clone, Default)]
pub(super) struct Catalogue {
    pub(super) models: Vec<ModelDescriptor>,
    aliases: Vec<ModelAlias>,
}

#[derive(Debug, Clone)]
struct ModelAlias {
    selector: String,
    resolved: String,
}

impl Catalogue {
    /// Maps `system/init.model` back to the selector users can send to Claude.
    pub(super) fn selection_id(&self, reported: &str, launched: Option<&str>) -> String {
        if self.models.iter().any(|model| model.id == reported) {
            return reported.to_owned();
        }
        if let Some(launched) = launched
            && let Some(alias) = self.aliases.iter().find(|alias| {
                alias.selector == launched && resolved_matches(&alias.resolved, reported)
            })
        {
            return alias.selector.clone();
        }
        self.aliases
            .iter()
            .find(|alias| alias.resolved == reported)
            .or_else(|| {
                self.aliases
                    .iter()
                    .find(|alias| resolved_matches(&alias.resolved, reported))
            })
            .map_or_else(|| reported.to_owned(), |alias| alias.selector.clone())
    }
}

/// Normalizes the `initialize.models` or `list_models.models` payload.
pub(super) fn models(payload: &Value) -> Catalogue {
    let mut aliases = Vec::new();
    let mut models = payload
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model
                .get("value")
                .or_else(|| model.get("model"))
                .or_else(|| model.get("id"))
                .and_then(Value::as_str)?;
            if let Some(resolved) = model
                .get("resolvedModel")
                .or_else(|| model.get("resolved_model"))
                .and_then(Value::as_str)
            {
                aliases.push(ModelAlias {
                    selector: id.to_owned(),
                    resolved: resolved.to_owned(),
                });
            }
            let display_name = model
                .get("displayName")
                .or_else(|| model.get("display_name"))
                .and_then(Value::as_str)
                .unwrap_or(id);
            let efforts = model
                .get("supportedEffortLevels")
                .or_else(|| model.get("supported_effort_levels"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(effort)
                .collect();
            Some(ModelDescriptor {
                id: id.to_owned(),
                display_name: display_name.to_owned(),
                efforts,
                default_effort: model
                    .get("defaultEffort")
                    .or_else(|| model.get("default_effort"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect::<Vec<_>>();
    if !models.is_empty() && models.iter().all(|model| model.efforts.is_empty()) {
        for model in &mut models {
            model.efforts = EFFORTS.into_iter().map(effort).collect();
        }
    }
    Catalogue { models, aliases }
}

/// Conservative catalogue used only when both discovery operations return no models.
pub(super) fn fallback() -> Catalogue {
    let models = [
        ("claude-fable-5-1", "Fable 5.1"),
        ("claude-opus-5", "Opus 5"),
        ("claude-sonnet-5", "Sonnet 5"),
        ("claude-haiku-4-5-20251001", "Haiku 4.5"),
    ]
    .into_iter()
    .map(|(id, display_name)| ModelDescriptor {
        id: id.to_owned(),
        display_name: display_name.to_owned(),
        efforts: EFFORTS.into_iter().map(effort).collect(),
        default_effort: None,
    })
    .collect();
    Catalogue {
        models,
        aliases: Vec::new(),
    }
}

fn resolved_matches(left: &str, right: &str) -> bool {
    left == right || without_context_suffix(left) == without_context_suffix(right)
}

fn without_context_suffix(model: &str) -> &str {
    model
        .rsplit_once('[')
        .filter(|(_, suffix)| suffix.ends_with(']'))
        .map_or(model, |(base, _)| base)
}

fn effort(id: &str) -> ReasoningEffortDescriptor {
    ReasoningEffortDescriptor {
        id: id.to_owned(),
        description: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn normalizes_current_and_scripted_model_shapes() {
        let parsed = models(&json!({"models": [
            {"value": "opus", "displayName": "Opus", "supportedEffortLevels": ["low", "high"]},
            {"model": "scripted", "displayName": "Scripted"}
        ]}));
        assert_eq!(parsed.models[0].id, "opus");
        assert_eq!(parsed.models[0].efforts.len(), 2);
        assert_eq!(parsed.models[1].id, "scripted");
        assert!(parsed.models[1].efforts.is_empty());

        let without_efforts = models(&json!({"models": [
            {"value": "legacy", "displayName": "Legacy"}
        ]}));
        assert_eq!(without_efforts.models[0].efforts.len(), EFFORTS.len());
    }

    #[test]
    fn resolved_models_canonicalize_to_selectable_ids() {
        let catalogue = models(&json!({"models": [
            {"value": "default", "resolvedModel": "claude-opus-5[1m]"},
            {"value": "opus", "resolvedModel": "claude-opus-5"},
            {"value": "fable[1m]", "resolvedModel": "claude-fable-5-1"},
            {"value": "sonnet", "resolvedModel": "claude-sonnet-5"}
        ]}));

        assert_eq!(catalogue.selection_id("claude-opus-5", None), "opus");
        assert_eq!(
            catalogue.selection_id("claude-opus-5[1m]", Some("default")),
            "default"
        );
        assert_eq!(
            catalogue.selection_id("claude-fable-5-1[1m]", Some("fable[1m]")),
            "fable[1m]"
        );
        assert_eq!(catalogue.selection_id("claude-sonnet-5", None), "sonnet");
    }
}
