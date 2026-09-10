//! Stable, filesystem-safe slug generation helpers.

/// Converts arbitrary text to the canonical worktree slug representation.
#[must_use]
pub fn slugify(value: &str) -> String {
    normalize(value, true)
}

/// Converts a context name to its persisted context identifier.
#[must_use]
pub fn normalize_context_id(value: &str) -> String {
    normalize(value, false)
}

fn normalize(value: &str, allow_dot_and_underscore: bool) -> String {
    let mut output = String::new();
    let mut pending_dash = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        let allowed = character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || (allow_dot_and_underscore && matches!(character, '.' | '_'));
        if allowed {
            if pending_dash && !output.is_empty() {
                output.push('-');
            }
            pending_dash = false;
            output.push(character);
        } else {
            pending_dash = true;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ContextId;

    #[test]
    fn slugifies_inventory_cases() {
        assert_eq!(slugify("Feature/Foo Bar"), "feature-foo-bar");
        assert_eq!(slugify("--Foo...bar__baz--"), "foo...bar__baz");
        assert_eq!(slugify("A:::B///C"), "a-b-c");
        assert_eq!(slugify(" héllo "), "h-llo");
        assert_eq!(slugify("..."), "...");
        assert_eq!(slugify("---"), "");
    }

    #[test]
    fn normalizes_context_names() {
        assert_eq!(normalize_context_id(" Platform / API "), "platform-api");
        assert_eq!(normalize_context_id("One___Two..."), "one-two");
    }

    #[test]
    fn every_accepted_context_id_case_is_normalization_idempotent() {
        for value in ["personal", "platform-api-2"] {
            assert!(ContextId::try_from(value).is_ok());
            assert_eq!(normalize_context_id(value), value);
        }

        for value in ["personal-", "personal--"] {
            assert!(ContextId::try_from(value).is_err());
            assert_eq!(normalize_context_id(value), "personal");
        }
    }
}
