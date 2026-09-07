//! Sleep-policy rule types and pure rule-matching behavior.

use regex::{Regex, RegexBuilder};
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

/// Compiled rules for one effective sleep configuration, independent of its serialized schema.
///
/// Retain this with the configuration owner and call [`Self::update`] when the resolved
/// `sleep.keep_alive` rules change. Matching never compiles regular expressions.
#[derive(Debug, Clone)]
pub struct CompiledSleepPolicy {
    rules: Vec<KeepAliveRule>,
    processes: Vec<(usize, Regex)>,
    ports_enabled: bool,
    diagnostics: Vec<KeepAliveDiagnostic>,
}

/// A skipped enabled process rule and its regex parser diagnostic.
#[derive(Debug, Clone)]
pub struct KeepAliveDiagnostic {
    /// Position in the configured rule list, disambiguating repeated identifiers.
    pub rule_index: usize,
    /// Configured identifier of the skipped rule.
    pub rule_id: String,
    /// Why the pattern failed to compile.
    pub error: regex::Error,
}

impl CompiledSleepPolicy {
    /// Compiles every enabled process pattern, retaining parser failures as diagnostics.
    #[must_use]
    pub fn new(rules: &[KeepAliveRule]) -> Self {
        let mut processes = Vec::new();
        let mut diagnostics = Vec::new();
        let mut ports_enabled = false;
        for (rule_index, rule) in rules.iter().enumerate().filter(|(_, rule)| rule.enabled) {
            match rule.kind {
                KeepAliveKind::ListeningPort => ports_enabled = true,
                KeepAliveKind::Process => {
                    match RegexBuilder::new(&rule.pattern)
                        .case_insensitive(true)
                        .build()
                    {
                        Ok(pattern) => processes.push((rule_index, pattern)),
                        Err(error) => diagnostics.push(KeepAliveDiagnostic {
                            rule_index,
                            rule_id: rule.id.clone(),
                            error,
                        }),
                    }
                }
            }
        }
        Self {
            rules: rules.to_vec(),
            processes,
            ports_enabled,
            diagnostics,
        }
    }

    /// Replaces compiled storage only when the effective rule configuration changes.
    /// Returns whether a replacement was needed, including diagnostic-only changes.
    pub fn update(&mut self, rules: &[KeepAliveRule]) -> bool {
        if self.rules == rules {
            return false;
        }
        *self = Self::new(rules);
        true
    }

    /// Invalid enabled process rules, in configuration order. Disabled and port rules
    /// are not parsed, matching the keep-alive policy's treatment of their patterns.
    #[must_use]
    pub fn diagnostics(&self) -> &[KeepAliveDiagnostic] {
        &self.diagnostics
    }

    /// Counts commands matching a configured process rule by its position.
    /// Disabled, invalid, port, and missing rules have no process matches.
    #[must_use]
    pub fn count_process_matches<'a>(
        &self,
        rule_index: usize,
        commands: impl IntoIterator<Item = &'a str>,
    ) -> usize {
        self.processes
            .iter()
            .find(|(index, _)| *index == rule_index)
            .map_or(0, |(_, pattern)| {
                commands
                    .into_iter()
                    .filter(|command| pattern.is_match(command))
                    .count()
            })
    }

    /// Process labels retain rule order and are de-duplicated. Port labels follow them
    /// in numeric order, with repeated ports removed. Invalid process rules are ignored.
    #[must_use]
    pub fn match_keep_alive(
        &self,
        command_lines: &[String],
        listening_ports: &[u16],
    ) -> Vec<String> {
        let mut labels = Vec::new();
        for (rule_index, pattern) in &self.processes {
            let label = &self.rules[*rule_index].label;
            if !labels.contains(label)
                && command_lines
                    .iter()
                    .any(|command| pattern.is_match(command))
            {
                labels.push(label.clone());
            }
        }
        if self.ports_enabled {
            let mut ports = listening_ports.to_vec();
            ports.sort_unstable();
            ports.dedup();
            labels.extend(ports.into_iter().map(|port| format!(":{port}")));
        }
        labels
    }
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
    fn default_rules_match_agents_case_insensitively_and_sort_ports() {
        let policy = CompiledSleepPolicy::new(&default_keep_alive_rules());
        assert_eq!(
            policy.match_keep_alive(
                &["/usr/local/bin/CLAUDE --resume".to_owned()],
                &[8080, 3000, 8080]
            ),
            ["claude", ":3000", ":8080"]
        );
    }

    #[test]
    fn disabled_port_and_invalid_process_rules_contribute_no_labels() {
        let rules = vec![
            KeepAliveRule {
                id: "off".to_owned(),
                label: "off".to_owned(),
                kind: KeepAliveKind::ListeningPort,
                pattern: String::new(),
                enabled: false,
            },
            process_rule("bad", "bad", "["),
        ];
        let policy = CompiledSleepPolicy::new(&rules);
        assert!(
            policy
                .match_keep_alive(&["anything".to_owned()], &[80])
                .is_empty()
        );
    }

    #[test]
    fn compiled_policy_preserves_rule_order_deduplication_and_port_labels() {
        let mut rules = vec![
            process_rule("first", "worker", "cargo"),
            process_rule("second", "worker", "rustc"),
            process_rule("third", ":80", "cargo"),
            process_rule("fourth", "editor", "nvim"),
        ];
        rules.extend(default_keep_alive_rules());
        let policy = CompiledSleepPolicy::new(&rules);
        let commands = vec![
            "NVIM".to_owned(),
            "cargo test".to_owned(),
            "rustc".to_owned(),
        ];
        assert_eq!(
            policy.match_keep_alive(&commands, &[443, 80, 80]),
            ["worker", ":80", "editor", ":80", ":443"]
        );
        assert_eq!(policy.match_keep_alive(&[], &[443, 80]), [":80", ":443"]);
        assert!(policy.match_keep_alive(&[], &[]).is_empty());
    }

    #[test]
    fn compiled_policy_retains_parser_errors_only_for_active_process_rules() {
        let mut rules = vec![
            process_rule("bad", "bad", "["),
            process_rule("bad", "also bad", r"\q"),
            process_rule("off", "off", "("),
        ];
        rules[2].enabled = false;
        rules.push(KeepAliveRule {
            id: "ports".to_owned(),
            label: "ports".to_owned(),
            kind: KeepAliveKind::ListeningPort,
            pattern: "[".to_owned(),
            enabled: true,
        });
        let policy = CompiledSleepPolicy::new(&rules);
        let diagnostics = policy.diagnostics();
        assert_eq!(diagnostics.len(), 2);
        for (index, diagnostic) in diagnostics.iter().enumerate() {
            assert_eq!(diagnostic.rule_index, index);
            assert_eq!(diagnostic.rule_id, "bad");
            let expected = RegexBuilder::new(&rules[index].pattern)
                .case_insensitive(true)
                .build()
                .unwrap_err();
            assert_eq!(diagnostic.error.to_string(), expected.to_string());
        }
        assert_eq!(
            policy.match_keep_alive(&["anything".to_owned()], &[80]),
            [":80"]
        );
    }

    #[test]
    fn process_counts_distinguish_duplicate_rules_and_skip_inactive_entries() {
        let mut rules = vec![
            process_rule("same", "same", "cargo"),
            process_rule("same", "same", "rustc"),
            process_rule("invalid", "invalid", r"\q"),
            process_rule("disabled", "disabled", "cargo"),
        ];
        rules[3].enabled = false;
        rules.push(KeepAliveRule {
            id: "ports".to_owned(),
            label: "ports".to_owned(),
            kind: KeepAliveKind::ListeningPort,
            pattern: "cargo".to_owned(),
            enabled: true,
        });
        let policy = CompiledSleepPolicy::new(&rules);
        let commands = ["CARGO build", "cargo test", "rustc"];
        assert_eq!(policy.count_process_matches(0, commands), 2);
        assert_eq!(policy.count_process_matches(1, commands), 1);
        for index in 2..=rules.len() {
            assert_eq!(policy.count_process_matches(index, commands), 0);
        }
    }

    #[test]
    fn policy_replacement_tracks_effective_rules_and_clears_stale_diagnostics() {
        let mut rules = vec![process_rule("worker", "worker", "cargo")];
        let mut policy = CompiledSleepPolicy::new(&rules);
        let commands = vec!["cargo test".to_owned()];
        let processes = policy.processes.as_ptr();
        for _ in 0..32 {
            assert!(!policy.update(&rules));
            assert_eq!(policy.processes.as_ptr(), processes);
            assert_eq!(policy.match_keep_alive(&commands, &[]), ["worker"]);
        }
        rules[0].label = "build".to_owned();
        assert!(policy.update(&rules));
        assert_eq!(policy.match_keep_alive(&commands, &[]), ["build"]);
        rules[0].pattern = "[".to_owned();
        assert!(policy.update(&rules));
        assert_eq!(policy.diagnostics().len(), 1);
        assert!(policy.match_keep_alive(&commands, &[]).is_empty());
        assert!(!policy.update(&rules));
        rules[0].enabled = false;
        assert!(policy.update(&rules));
        assert!(policy.diagnostics().is_empty());
        rules[0].kind = KeepAliveKind::ListeningPort;
        rules[0].enabled = true;
        assert!(policy.update(&rules));
        assert_eq!(policy.match_keep_alive(&commands, &[80]), [":80"]);
        assert!(policy.update(&[]));
        assert!(policy.match_keep_alive(&commands, &[80]).is_empty());
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
