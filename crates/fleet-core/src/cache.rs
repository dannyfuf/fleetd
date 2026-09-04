//! Persisted GitHub cache envelopes shared by adapters and protocol responses.

use serde::{Deserialize, Serialize};

use crate::github::{PullRequest, RemoteRepo};

/// Cached repository-discovery results for one GitHub owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCache {
    /// ISO-8601 time at which the repositories were fetched.
    pub fetched_at: String,
    /// Cached repository records.
    pub repos: Vec<RemoteRepo>,
}

/// Cached pull requests for one repository and tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrCache {
    /// ISO-8601 time at which the pull requests were fetched.
    pub fetched_at: String,
    /// Cached pull-request records.
    pub prs: Vec<PullRequest>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cache_envelopes_round_trip() {
        let repos = RepoCache {
            fetched_at: "2026-09-04T12:00:00Z".to_owned(),
            repos: Vec::new(),
        };
        let encoded = serde_json::to_string(&repos).unwrap_or_else(|error| panic!("{error}"));
        let decoded: RepoCache =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, repos);

        let prs = PrCache {
            fetched_at: "2026-09-04T12:00:00Z".to_owned(),
            prs: Vec::new(),
        };
        let encoded = serde_json::to_string(&prs).unwrap_or_else(|error| panic!("{error}"));
        let decoded: PrCache =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, prs);
    }
}
