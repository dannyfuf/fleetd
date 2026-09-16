//! Codex's runtime-discovered model and skill catalogues.

use std::{collections::HashSet, path::Path};

use serde_json::{Value, json};

use fleet_core::agents::{ModelDescriptor, ReasoningEffortDescriptor};

use super::{CATALOGUE_DEADLINE, HarnessResult, transport::Transport};

/// Reads every `model/list` page and preserves the complete additive model records.
pub(super) async fn model_catalogue(transport: &Transport) -> HarnessResult<Vec<Value>> {
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    let mut models = Vec::new();
    loop {
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |cursor| json!({"cursor": cursor}));
        let result = transport
            .request("model/list", Some(params), CATALOGUE_DEADLINE)
            .await?;
        let page = result
            .get("data")
            .or_else(|| result.get("models"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        models.extend(page);
        let next_cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let Some(next_cursor) = next_cursor else {
            break;
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            tracing::warn!(
                target: "fleet::agents::codex",
                cursor = %next_cursor,
                "Codex model discovery repeated a cursor; keeping the pages already read"
            );
            break;
        }
        cursor = Some(next_cursor);
    }
    Ok(models)
}

/// Normalizes the additive subset Fleet projects without letting Codex wire types cross the
/// harness boundary. Invalid catalogue entries are skipped rather than narrowing a whole thread.
pub(super) fn model_descriptors(models: &[Value]) -> Vec<ModelDescriptor> {
    models
        .iter()
        .filter_map(|model| {
            let id = model
                .get("id")
                .or_else(|| model.get("model"))
                .and_then(Value::as_str)?
                .to_owned();
            let display_name = model
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&id)
                .to_owned();
            let efforts = model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|effort| {
                    Some(ReasoningEffortDescriptor {
                        id: effort.get("reasoningEffort")?.as_str()?.to_owned(),
                        description: effort
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })
                })
                .collect();
            Some(ModelDescriptor {
                id,
                display_name,
                efforts,
                default_effort: model
                    .get("defaultReasoningEffort")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}

/// Reads skills for the thread cwd. The direct `skills` array remains accepted for older peers.
pub(super) async fn skill_names(
    transport: &Transport,
    cwd: &Path,
    force_reload: bool,
) -> HarnessResult<Vec<String>> {
    let result = transport
        .request(
            "skills/list",
            Some(json!({
                "cwds": [cwd.to_string_lossy()],
                "forceReload": force_reload,
            })),
            CATALOGUE_DEADLINE,
        )
        .await?;
    Ok(parse_skill_names(&result))
}

/// Extracts skill names from the canonical grouped response and the older direct-array shape.
pub(super) fn parse_skill_names(result: &Value) -> Vec<String> {
    let mut skills = result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("skills").and_then(Value::as_array))
        .flatten()
        .filter_map(|skill| skill.get("name").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if skills.is_empty() {
        skills.extend(
            result
                .get("skills")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|skill| {
                    skill
                        .as_str()
                        .or_else(|| skill.get("name").and_then(Value::as_str))
                })
                .map(ToOwned::to_owned),
        );
    }
    skills.sort();
    skills.dedup();
    skills
}
