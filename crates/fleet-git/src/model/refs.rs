use serde::{Deserialize, Serialize};
use std::fmt;

/// A hexadecimal Git object identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectId(pub String);

impl ObjectId {
    /// Returns the hexadecimal identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<String> for ObjectId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ObjectId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Tracking relationship for a local branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// Short remote ref name.
    pub name: String,
    /// Commits local is ahead of upstream.
    pub ahead: usize,
    /// Commits local is behind upstream.
    pub behind: usize,
}

/// Local branch summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Short branch name.
    pub name: String,
    /// Tip object.
    pub oid: ObjectId,
    /// Whether this is the checked-out branch.
    pub is_head: bool,
    /// Tracking relationship.
    pub upstream: Option<Upstream>,
    /// Tip subject.
    pub subject: String,
    /// Committer timestamp in Unix seconds.
    pub committed_at: i64,
    /// When the branch was last checked out, from the HEAD reflog, when it appears there.
    ///
    /// Preferred over `committed_at` for checkout-recency presentation.
    pub checked_out_at: Option<i64>,
}

/// Remote-tracking branch summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranch {
    /// Full short name, such as `origin/main`.
    pub name: String,
    /// Name without the remote prefix.
    pub branch: String,
    /// Tip object.
    pub oid: ObjectId,
    /// Tip subject.
    pub subject: String,
    /// Committer timestamp in Unix seconds.
    pub committed_at: i64,
}

/// Remote branches grouped by remote name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranchGroup {
    /// Remote name.
    pub remote: String,
    /// Branches belonging to the remote.
    pub branches: Vec<RemoteBranch>,
}

/// A configured remote and its separate fetch/push URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Remote name.
    pub name: String,
    /// Fetch URL, if configured.
    pub fetch_url: Option<String>,
    /// Push URL, if configured.
    pub push_url: Option<String>,
}

/// Tag summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Tag name.
    pub name: String,
    /// Peeled commit or tagged object identifier.
    pub oid: ObjectId,
    /// Creator timestamp in Unix seconds, or zero when unavailable.
    pub created_at: i64,
    /// Tag or target subject.
    pub subject: String,
}

/// Commit or reflog summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Commit identifier.
    pub oid: ObjectId,
    /// Parent commit identifiers.
    pub parents: Vec<ObjectId>,
    /// Author display name.
    pub author_name: String,
    /// Author email address.
    pub author_email: String,
    /// Author timestamp in Unix seconds.
    pub authored_at: i64,
    /// Committer timestamp in Unix seconds.
    pub committed_at: i64,
    /// First line of the message.
    pub subject: String,
    /// Remaining message body.
    pub body: String,
    /// Ref decorations emitted by Git.
    pub decorations: Vec<String>,
    /// Whether the commit is contained in the branch's upstream. Always `false`
    /// when the checked-out branch has no upstream configured.
    pub pushed: bool,
}

/// Reflog record with selector and action text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    /// Commit identifier at this entry.
    pub oid: ObjectId,
    /// Parent identifiers.
    pub parents: Vec<ObjectId>,
    /// Reflog selector such as `HEAD@{0}`.
    pub selector: String,
    /// Reflog action/subject.
    pub subject: String,
    /// Committer timestamp in Unix seconds.
    pub committed_at: i64,
}

/// Stash-list record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// Zero-based stash index.
    pub index: usize,
    /// Stash commit object.
    pub oid: ObjectId,
    /// Creation timestamp in Unix seconds.
    pub created_at: i64,
    /// Stash subject.
    pub subject: String,
}

/// An explicitly typed checkout target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref(pub String);

impl From<String> for Ref {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Ref {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
