//! Pure validation helpers for user input and persisted data.

use thiserror::Error;

use crate::slug::slugify;

/// A validation failure for a user-provided slug or branch.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    /// A slug is empty, unsafe, reserved, or non-canonical.
    #[error("invalid slug `{value}`: {reason}")]
    Slug {
        /// Rejected slug.
        value: String,
        /// Violated slug restriction.
        reason: &'static str,
    },
    /// A branch violates Git reference naming rules.
    #[error("invalid branch `{value}`: {reason}")]
    Branch {
        /// Rejected branch.
        value: String,
        /// Violated Git reference restriction.
        reason: &'static str,
    },
}

/// Validates that a slug is canonical and safe as a worktree path component.
pub fn validate_slug(slug: &str) -> Result<(), ValidationError> {
    if slug.is_empty() {
        return Err(slug_error(slug, "must not be empty"));
    }
    if matches!(slug, "." | "..") {
        return Err(slug_error(slug, "must not be `.` or `..`"));
    }
    if slug.starts_with(".hot") {
        return Err(slug_error(slug, "the `.hot` prefix is reserved"));
    }
    if !matches!(slug.as_bytes().first(), Some(b'a'..=b'z' | b'0'..=b'9')) {
        return Err(slug_error(
            slug,
            "must start with an ASCII lowercase letter or digit",
        ));
    }
    if slugify(slug) != slug {
        return Err(slug_error(slug, "must already be in canonical slug form"));
    }
    Ok(())
}

/// Validates a branch using swarm's `validateBranch` rejection rules.
pub fn validate_branch(branch: &str) -> Result<(), ValidationError> {
    if branch.is_empty() {
        return Err(branch_error(branch, "must not be empty"));
    }
    if branch == "@" {
        return Err(branch_error(branch, "must not be `@`"));
    }
    if branch.starts_with('-') {
        return Err(branch_error(branch, "must not start with `-`"));
    }
    if branch.ends_with('/') || branch.ends_with('.') {
        return Err(branch_error(branch, "must not end with `/` or `.`"));
    }
    if branch.contains("..") {
        return Err(branch_error(branch, "must not contain `..`"));
    }
    if branch.contains("@{") {
        return Err(branch_error(branch, "must not contain `@{`"));
    }
    if branch.chars().any(|character| {
        character.is_control()
            || character.is_whitespace()
            || matches!(character, '[' | '\\' | '~' | '^' | ':' | '?' | '*')
    }) {
        return Err(branch_error(branch, "contains a forbidden character"));
    }
    if branch.split('/').any(|component| {
        component.is_empty()
            || component.starts_with('.')
            || component.to_ascii_lowercase().ends_with(".lock")
    }) {
        return Err(branch_error(
            branch,
            "contains an empty, dot-leading, or `.lock` component",
        ));
    }
    Ok(())
}

fn slug_error(value: &str, reason: &'static str) -> ValidationError {
    ValidationError::Slug {
        value: value.to_owned(),
        reason,
    }
}

fn branch_error(value: &str, reason: &'static str) -> ValidationError {
    ValidationError::Branch {
        value: value.to_owned(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_slugs() {
        for valid in ["feature", "f.1", "0_test", "a-b"] {
            assert!(validate_slug(valid).is_ok(), "{valid}");
        }
        for invalid in ["", ".", "..", ".hot", ".hot.2", "Feature", "a/b", "-bad"] {
            assert!(validate_slug(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn validates_all_branch_rejections() {
        for valid in ["main", "feature/foo", "heads-v2", "release@2"] {
            assert!(validate_branch(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "@",
            "-bad",
            "bad/",
            "bad.",
            "bad//name",
            ".hidden",
            "a/.b",
            "a.lock",
            "a/b.LOCK",
            "a b",
            "a\t",
            "a..b",
            "a@{b",
            "a[b",
            "a\\b",
            "a~b",
            "a^b",
            "a:b",
            "a?b",
            "a*b",
        ] {
            assert!(validate_branch(invalid).is_err(), "{invalid}");
        }
    }
}
