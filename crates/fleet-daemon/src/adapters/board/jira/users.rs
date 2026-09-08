//! Display name → account id, learned from every pull.
//!
//! `search` and `view` hand back `assignee.displayName`, and `edit --assignee` wants an account
//! id or an email. The cache is what closes that gap without a user-search call per push; it is
//! in-memory per daemon and refilled by every pull, so a cold daemon simply learns again.

use super::settings::JiraSettings;
use std::{
    collections::HashMap,
    sync::{PoisonError, RwLock},
};

/// The scope one board's accounts are cached under.
///
/// Two boards on the same site share what either of them learned; two sites never do, because
/// the same display name is a different person on each.
#[must_use]
pub fn scope(settings: &JiraSettings) -> String {
    settings
        .site
        .as_deref()
        .map(str::trim)
        .filter(|site| !site.is_empty())
        .unwrap_or(settings.project.trim())
        .to_lowercase()
}

/// One Jira account, as much of it as a pull revealed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraUser {
    /// Atlassian account id, which is what writes are addressed to.
    pub account_id: String,
    /// The account's email, when the site exposes one.
    pub email: Option<String>,
    /// The name every issue payload shows.
    pub display_name: String,
}

/// Display-name lookups, scoped so two boards on different sites never mix accounts.
#[derive(Debug, Default)]
pub struct UserCache {
    inner: RwLock<HashMap<String, HashMap<String, JiraUser>>>,
}

impl UserCache {
    /// Remembers one account under a scope (the board's site, else its project).
    pub fn remember(&self, scope: &str, user: JiraUser) {
        self.inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(scope.to_owned())
            .or_default()
            .insert(user.display_name.clone(), user);
    }

    /// Looks one up by the display name an issue payload showed.
    #[must_use]
    pub fn resolve(&self, scope: &str, display_name: &str) -> Option<JiraUser> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(scope)?
            .get(display_name)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_without_a_site_is_scoped_to_its_project() {
        let with_site =
            JiraSettings::parse(&serde_json::json!({"project": "SP", "site": "BUK.atlassian.net"}))
                .unwrap();
        assert_eq!(scope(&with_site), "buk.atlassian.net");
        let without = JiraSettings::parse(&serde_json::json!({"project": "SP"})).unwrap();
        assert_eq!(scope(&without), "sp");
    }

    #[test]
    fn a_remembered_user_is_found_in_its_own_scope_only() {
        let cache = UserCache::default();
        cache.remember(
            "buk.atlassian.net",
            JiraUser {
                account_id: "5b10a".into(),
                email: Some("danny@example.com".into()),
                display_name: "Danny Fuentes".into(),
            },
        );
        assert_eq!(
            cache
                .resolve("buk.atlassian.net", "Danny Fuentes")
                .map(|user| user.account_id),
            Some("5b10a".to_owned())
        );
        // Two sites can hold the same display name over two different accounts.
        assert_eq!(cache.resolve("other.atlassian.net", "Danny Fuentes"), None);
        assert_eq!(cache.resolve("buk.atlassian.net", "Nobody"), None);
    }
}
