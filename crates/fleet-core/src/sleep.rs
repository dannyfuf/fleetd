//! Sleep-policy rule types and pure rule-matching behavior.

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::agents::RECOGNIZED_AGENTS;

/// Kind of process fact inspected by a keep-alive rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeepAliveKind {
    /// Match a case-insensitive regular expression against full command lines.
    Process,
    /// Keep a terminal for every listening TCP port owned by its process tree.
    ListeningPort,
}

/// A configurable terminal keep-alive rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAliveRule {
    /// Stable settings identifier.
    pub id: String,
    /// Label reported for a process match.
    pub label: String,
    /// Fact type inspected by the rule.
    pub kind: KeepAliveKind,
    /// Full-command regular expression; ignored for port rules.
    #[serde(default)]
    pub pattern: String,
    /// Whether this rule participates in matching.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// Returns swarm's four default keep-alive rules.
#[must_use]
pub fn default_keep_alive_rules() -> Vec<KeepAliveRule> {
    RECOGNIZED_AGENTS
        .into_iter()
        .map(|agent| process_rule(agent, agent, &format!(r"(^|/){agent}( |$)")))
        .chain(std::iter::once(KeepAliveRule {
            id: "servers".to_owned(),
            label: "server".to_owned(),
            kind: KeepAliveKind::ListeningPort,
            pattern: String::new(),
            enabled: true,
        }))
        .collect()
}

/// Matches one terminal's process command lines and listening ports against sleep rules.
///
/// Invalid regular expressions are ignored. Process labels retain rule order; dynamic
/// `:<port>` labels are numeric, sorted, and de-duplicated.
#[must_use]
pub fn match_keep_alive(
    rules: &[KeepAliveRule],
    command_lines: &[String],
    listening_ports: &[u16],
) -> Vec<String> {
    let mut labels = Vec::new();
    let ports_enabled = rules
        .iter()
        .any(|rule| rule.enabled && rule.kind == KeepAliveKind::ListeningPort);

    for rule in rules
        .iter()
        .filter(|rule| rule.enabled && rule.kind == KeepAliveKind::Process)
    {
        let Ok(pattern) = RegexBuilder::new(&rule.pattern)
            .case_insensitive(true)
            .build()
        else {
            continue;
        };
        if command_lines
            .iter()
            .any(|command| pattern.is_match(command))
            && !labels.contains(&rule.label)
        {
            labels.push(rule.label.clone());
        }
    }

    if ports_enabled {
        let mut ports = listening_ports.to_vec();
        ports.sort_unstable();
        ports.dedup();
        labels.extend(ports.into_iter().map(|port| format!(":{port}")));
    }
    labels
}

fn process_rule(id: &str, label: &str, pattern: &str) -> KeepAliveRule {
    KeepAliveRule {
        id: id.to_owned(),
        label: label.to_owned(),
        kind: KeepAliveKind::Process,
        pattern: pattern.to_owned(),
        enabled: true,
    }
}

const fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matcher_is_case_insensitive_and_sorts_ports() {
        let labels = match_keep_alive(
            &default_keep_alive_rules(),
            &["/usr/local/bin/CLAUDE --resume".to_owned()],
            &[8080, 3000, 8080],
        );
        assert_eq!(labels, vec!["claude", ":3000", ":8080"]);
    }

    #[test]
    fn matcher_ignores_disabled_and_invalid_rules() {
        let rules = vec![
            KeepAliveRule {
                id: "bad".to_owned(),
                label: "bad".to_owned(),
                kind: KeepAliveKind::Process,
                pattern: "[".to_owned(),
                enabled: true,
            },
            KeepAliveRule {
                id: "off".to_owned(),
                label: "off".to_owned(),
                kind: KeepAliveKind::ListeningPort,
                pattern: String::new(),
                enabled: false,
            },
        ];
        assert!(match_keep_alive(&rules, &["anything".to_owned()], &[80]).is_empty());
    }

    #[test]
    fn rule_schema_defaults_pattern_and_enabled() {
        let rule: KeepAliveRule =
            serde_json::from_str(r#"{"id":"server","label":"server","kind":"listening-port"}"#)
                .unwrap_or_else(|error| panic!("{error}"));
        assert!(rule.pattern.is_empty());
        assert!(rule.enabled);
    }
}
