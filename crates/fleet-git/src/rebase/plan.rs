use crate::ObjectId;
use serde::{Deserialize, Serialize};

/// Base argument passed to `git rebase --interactive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum RebaseBase {
    /// Rebase every commit from the root.
    Root,
    /// Rebase commits after this revision expression.
    Reference(String),
}

/// Optional behavior for a `fixup` todo line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum FixupFlag {
    /// Use the fixup commit's message (`-C`).
    KeepMessage,
    /// Open the combined message for editing (`-c`).
    EditMessage,
}

/// Action assigned to a rebase todo commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum RebaseAction {
    /// Replay unchanged.
    Pick,
    /// Fold into the previous commit and combine messages.
    Squash,
    /// Fold into the previous commit, normally discarding this message.
    Fixup {
        /// Optional commit-message behavior.
        flag: Option<FixupFlag>,
    },
    /// Ask Git to edit the message.
    Reword,
    /// Pause after applying the commit.
    Edit,
    /// Omit the commit.
    Drop,
}

/// One transformation of Git's todo file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum TodoEdit {
    /// Change one commit's action, matching an OID prefix exactly once.
    Change {
        /// Full or abbreviated object identifier.
        oid: ObjectId,
        /// Replacement action.
        action: RebaseAction,
    },
    /// Move selected todo commits by an actionable-line offset.
    Move {
        /// Full or abbreviated object identifiers.
        oids: Vec<ObjectId>,
        /// Signed offset in chronological todo order.
        offset: isize,
    },
}

/// Complete one-shot interactive-rebase instruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RebasePlan {
    /// Revision before the first commit to rewrite.
    pub(super) base: RebaseBase,
    /// Todo transformations.
    pub(super) edits: Vec<TodoEdit>,
    /// Enable Git's autostash support.
    pub(super) autostash: bool,
    /// Preserve commits that become empty.
    pub(super) keep_empty: bool,
}

/// Direction used by `Repository::move_commit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDirection {
    /// Move toward HEAD/newer commits.
    Up,
    /// Move toward the root/older commits.
    Down,
}
