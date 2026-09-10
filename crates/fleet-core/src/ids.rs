//! Strongly typed identifiers shared across Fleet's domain model.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The identifier used for the built-in local host.
pub const LOCAL_HOST: &str = "local";

/// An error returned when a domain identifier is malformed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdError {
    /// A string failed the selected identifier's syntax rules.
    #[error("invalid {kind} `{value}`: {reason}")]
    Invalid {
        /// Human-readable identifier family.
        kind: &'static str,
        /// Rejected input.
        value: String,
        /// Expected syntax or violated restriction.
        reason: &'static str,
    },
}

impl IdError {
    fn new(kind: &'static str, value: &str, reason: &'static str) -> Self {
        Self::Invalid {
            kind,
            value: value.to_owned(),
            reason,
        }
    }
}

macro_rules! string_id {
    ($name:ident, $kind:literal, $validate:expr) => {
        #[doc = concat!("A validated ", $kind, ".")]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl $name {
            #[doc = concat!("Returns the string representation of this ", $kind, ".")]
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                ($validate)(&value)?;
                Ok(Self(value))
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                ($validate)(value)?;
                Ok(Self(value.to_owned()))
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::try_from(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
    };
}

fn validate_context_id(value: &str) -> Result<(), IdError> {
    let mut chars = value.chars();
    if !matches!(chars.next(), Some('a'..='z' | '0'..='9'))
        || !chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        || value.ends_with('-')
        || value.contains("--")
    {
        return Err(IdError::new(
            "context id",
            value,
            "expected /^[a-z0-9]+(?:-[a-z0-9]+)*$/",
        ));
    }
    Ok(())
}

fn split_repo(value: &str) -> Option<(&str, &str)> {
    let (owner, name) = value.split_once('/')?;
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || owner.chars().any(char::is_whitespace)
        || name.chars().any(char::is_whitespace)
    {
        None
    } else {
        Some((owner, name))
    }
}

fn validate_repo_id(value: &str) -> Result<(), IdError> {
    split_repo(value)
        .map(|_| ())
        .ok_or_else(|| IdError::new("repository id", value, "expected exactly owner/name"))
}

fn split_worktree(value: &str) -> Option<(&str, &str)> {
    let (repo, slug) = value.split_once('#')?;
    if slug.is_empty() || slug.contains('#') || slug.chars().any(char::is_whitespace) {
        return None;
    }
    split_repo(repo)?;
    Some((repo, slug))
}

fn validate_worktree_id(value: &str) -> Result<(), IdError> {
    split_worktree(value)
        .map(|_| ())
        .ok_or_else(|| IdError::new("worktree id", value, "expected exactly owner/name#slug"))
}

fn validate_host_id(value: &str) -> Result<(), IdError> {
    if value == LOCAL_HOST {
        return Err(IdError::new("host id", value, "`local` is reserved"));
    }
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(IdError::new("host id", value, "expected /^[a-z0-9-]+$/"));
    }
    Ok(())
}

fn validate_session_id(value: &str) -> Result<(), IdError> {
    let is_agent = matches!(value, "swarm-agent-claude" | "swarm-agent-opencode");
    let is_tree_or_proxy = value.split('/').count() >= 2 && !value.split('/').any(str::is_empty);
    if value.chars().any(char::is_whitespace) || (!is_agent && !is_tree_or_proxy) {
        return Err(IdError::new(
            "session id",
            value,
            "expected a worktree/proxy name or reserved agent-session name",
        ));
    }
    Ok(())
}

fn validate_job_id(value: &str) -> Result<(), IdError> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(IdError::new(
            "job id",
            value,
            "expected a non-empty string without whitespace",
        ));
    }
    Ok(())
}

fn validate_slug(value: &str) -> Result<(), IdError> {
    let mut chars = value.chars();
    if !matches!(chars.next(), Some('a'..='z' | '0'..='9'))
        || !chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(IdError::new(
            "context id",
            value,
            "expected /^[a-z0-9][a-z0-9-]*$/",
        ));
    }
    Ok(())
}

fn validate_opaque_id(value: &str) -> Result<(), IdError> {
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_whitespace) {
        return Err(IdError::new(
            "card",
            value,
            "expected 1..=64 bytes without whitespace",
        ));
    }
    Ok(())
}

string_id!(BoardId, "board", validate_slug);
string_id!(CardId, "card", validate_opaque_id);
string_id!(StatusId, "status", validate_slug);
string_id!(LabelId, "label", validate_slug);

impl From<ContextId> for BoardId {
    fn from(id: ContextId) -> Self {
        Self(id.0)
    }
}

string_id!(ContextId, "context id", validate_context_id);
string_id!(RepoId, "repository id", validate_repo_id);
string_id!(WorktreeId, "worktree id", validate_worktree_id);
string_id!(HostId, "remote host id", validate_host_id);
string_id!(SessionId, "session id", validate_session_id);
string_id!(JobId, "job id", validate_job_id);

impl RepoId {
    /// Returns the repository owner.
    #[must_use]
    pub fn owner(&self) -> &str {
        split_repo(&self.0).map_or("", |(owner, _)| owner)
    }

    /// Returns the repository name.
    #[must_use]
    pub fn name(&self) -> &str {
        split_repo(&self.0).map_or("", |(_, name)| name)
    }
}

impl WorktreeId {
    /// Returns the repository portion of the worktree identifier.
    #[must_use]
    pub fn repo(&self) -> &str {
        split_worktree(&self.0).map_or("", |(repo, _)| repo)
    }

    /// Returns the worktree slug.
    #[must_use]
    pub fn slug(&self) -> &str {
        split_worktree(&self.0).map_or("", |(_, slug)| slug)
    }
}

impl SessionId {
    /// Constructs the canonical local session name from a repository name and slug.
    pub fn local(repo_name: &str, slug: &str) -> Result<Self, IdError> {
        Self::try_from(format!(
            "{}/{}",
            session_component(repo_name),
            session_component(slug)
        ))
    }
}

fn session_component(value: &str) -> String {
    value.replace(['.', ':'], "-")
}

/// A stable numeric identifier for a daemon-owned terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerminalId(pub u64);

impl fmt::Display for TerminalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for TerminalId {
    type Err = std::num::ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl TryFrom<String> for TerminalId {
    type Error = std::num::ParseIntError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<&str> for TerminalId {
    type Error = std::num::ParseIntError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_context_ids() {
        assert!(ContextId::try_from("alpha-2").is_ok());
        assert!(ContextId::try_from("-alpha").is_err());
        assert!(ContextId::try_from("alpha-").is_err());
        assert!(ContextId::try_from("alpha--2").is_err());
        assert!(ContextId::try_from("Alpha").is_err());
    }

    #[test]
    fn validates_repo_ids() {
        let id = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(id.owner(), "acme");
        assert_eq!(id.name(), "api");
        assert!(RepoId::try_from("acme/api/more").is_err());
        assert!(RepoId::try_from("acme /api").is_err());
    }

    #[test]
    fn validates_worktree_ids() {
        let id = WorktreeId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(id.repo(), "acme/api");
        assert_eq!(id.slug(), "feature");
        assert!(WorktreeId::try_from("acme/api#bad#slug").is_err());
    }

    #[test]
    fn validates_host_ids_and_reserves_local() {
        assert!(HostId::try_from("dev-box-2").is_ok());
        assert!(HostId::try_from("local").is_err());
        assert!(HostId::try_from("DevBox").is_err());
    }

    #[test]
    fn local_session_names_replace_dots_and_colons() {
        let local =
            SessionId::local("pay.roll:api", "feat:one").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(local.as_str(), "pay-roll-api/feat-one");
        assert!(SessionId::local("repo", "").is_err());
    }

    #[test]
    fn validates_job_and_terminal_ids() {
        assert!(JobId::try_from("job-123").is_ok());
        assert!(JobId::try_from("job 123").is_err());
        assert_eq!("42".parse::<TerminalId>().ok(), Some(TerminalId(42)));
        assert_eq!(TerminalId::try_from("43").ok(), Some(TerminalId(43)));
    }
}
