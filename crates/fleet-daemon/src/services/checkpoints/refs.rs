//! The `refs/fleet/checkpoints/` namespace, and what one checkpoint commit records.
//!
//! **Why a ref namespace is the store.** A checkpoint has to survive a projection rebuild, and
//! the `checkpoints` table does not: `store::project::rebuild_thread` deletes every row of it and
//! replays the log, because that table is the projector's read model of `AgentEvent::Compacted` —
//! a compaction boundary, not a Git tree. A checkpoint index kept there would be erased by boot
//! repair while the refs it named lived on, which is the worst of the three possible states: a
//! `[u]` that is not drawn over a tree that is still recoverable, and a ref the garbage collector
//! can no longer attribute to a thread.
//!
//! The refs themselves have neither problem. They are keyed by thread, they are the only thing a
//! revert actually needs, and `git for-each-ref` answers "which checkpoints does this thread
//! have?" in one call — so the namespace is the index, and there is exactly one truth.
//!
//! **Why this namespace.** `refs/fleet/…` is outside `refs/heads`, `refs/tags` and
//! `refs/remotes`, so a checkpoint never appears in `git branch`, never appears in Fleet's own
//! ref listings (`fleet_git::read::refs` reads those three namespaces), and is never pushed —
//! `git push` without a refspec pushes `refs/heads` only, and Fleet never runs `--mirror`.
//! Nothing else in Fleet or in the user's own workflow writes under `refs/fleet/`.

use chrono::{DateTime, TimeZone, Utc};
use fleet_core::agents::{ThreadId, TurnId};
use fleet_proto::agents::{CheckpointId, CheckpointScope, TurnCheckpoint};
use serde::{Deserialize, Serialize};

use super::error::{CheckpointError, Result};

/// The namespace every Fleet checkpoint ref lives under.
pub(super) const NAMESPACE: &str = "refs/fleet/checkpoints";

/// The metadata schema this build writes and can read.
const METADATA_VERSION: u32 = 1;

/// The ref prefix holding one thread's checkpoints, with its trailing slash.
pub(super) fn thread_prefix(thread: ThreadId) -> String {
    format!("{NAMESPACE}/{thread}/")
}

/// The full ref name for one checkpoint of one thread.
pub(super) fn ref_name(thread: ThreadId, checkpoint: &CheckpointId) -> String {
    format!("{}{checkpoint}", thread_prefix(thread))
}

/// Splits a full ref name into the thread that owns it and the checkpoint leaf.
///
/// Returns `None` for anything that is not shaped like a checkpoint ref, which is what makes the
/// garbage collector safe: a ref it cannot attribute to a thread is left alone rather than
/// guessed at.
pub(super) fn parse_ref(name: &str) -> Option<(ThreadId, CheckpointId)> {
    let rest = name.strip_prefix(NAMESPACE)?.strip_prefix('/')?;
    let (thread, leaf) = rest.split_once('/')?;
    let thread = thread.parse::<ThreadId>().ok()?;
    let parsed = CheckpointId::parse(leaf).ok()?;
    Some((thread, parsed.id))
}

/// What a checkpoint commit body records, beside the tree it points at.
///
/// The tree is the recoverable state and the ref name carries the identity, so this exists for
/// the one thing neither can express: which paths a `file`-scoped checkpoint covered. A file
/// that did not exist yet is checkpointed as an *empty* tree, and reverting it means deleting the
/// file — impossible to derive from a tree that records nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Metadata {
    /// Schema version; a body from a newer build is refused, never guessed at.
    pub version: u32,
    /// Owning thread, so a ref moved by hand is still attributable.
    pub thread: ThreadId,
    /// The turn the capture preceded.
    pub turn: TurnId,
    /// What the capture covered.
    pub scope: CheckpointScope,
    /// Capture order within the thread, one-based.
    pub ordinal: u32,
    /// Worktree-relative paths a `file` capture covered, sorted; empty for a `turn` capture.
    #[serde(default)]
    pub paths: Vec<String>,
}

impl Metadata {
    /// Builds the body for a capture that is about to be committed.
    pub(super) fn new(
        thread: ThreadId,
        turn: TurnId,
        scope: CheckpointScope,
        ordinal: u32,
        paths: Vec<String>,
    ) -> Self {
        Self {
            version: METADATA_VERSION,
            thread,
            turn,
            scope,
            ordinal,
            paths,
        }
    }

    /// Renders the commit message: one human line, then the body this type decodes from.
    ///
    /// `git show` on a checkpoint is a supported way to inspect one, which is why the subject is
    /// prose and the machine-readable half is below the blank line.
    pub(super) fn message(&self) -> Result<String> {
        let scope = self.scope.as_token();
        let body = serde_json::to_string(self).map_err(|error| CheckpointError::Metadata {
            checkpoint: CheckpointId::from_parts(self.ordinal, self.scope, self.turn),
            message: error.to_string(),
        })?;
        Ok(format!(
            "fleet checkpoint {ordinal} ({scope}) before turn {turn}\n\n{body}\n",
            ordinal = self.ordinal,
            turn = self.turn
        ))
    }

    /// Decodes the body out of `git cat-file commit` output.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Metadata`] when the commit carries no JSON body, a body this
    /// build cannot decode, or a schema version it does not implement.
    pub(super) fn decode(checkpoint: &CheckpointId, commit: &str) -> Result<Self> {
        let refuse = |message: String| CheckpointError::Metadata {
            checkpoint: checkpoint.clone(),
            message,
        };
        let body = commit
            .split_once("\n\n")
            .map(|(_, body)| body)
            .ok_or_else(|| refuse("the commit has no message body".to_owned()))?;
        let json = body
            .lines()
            .find(|line| line.starts_with('{'))
            .ok_or_else(|| refuse("the message body carries no checkpoint record".to_owned()))?;
        let metadata: Self =
            serde_json::from_str(json).map_err(|error| refuse(error.to_string()))?;
        if metadata.version != METADATA_VERSION {
            return Err(refuse(format!(
                "record version {} is not version {METADATA_VERSION}",
                metadata.version
            )));
        }
        Ok(metadata)
    }
}

/// One checkpoint as the daemon holds it: the ref's identity plus the body it records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// Identity inside the thread.
    pub id: CheckpointId,
    /// What it covers.
    pub scope: CheckpointScope,
    /// The turn it was captured for.
    pub turn: TurnId,
    /// Capture order within the thread, one-based.
    pub ordinal: u32,
    /// When it was captured, from the commit's own creator date.
    pub at: DateTime<Utc>,
}

impl Checkpoint {
    /// Builds a listing entry from a `for-each-ref` row.
    pub(super) fn from_ref(id: CheckpointId, created_at: i64) -> Option<Self> {
        let parsed = CheckpointId::parse(id.as_str()).ok()?;
        Some(Self {
            id: parsed.id,
            scope: parsed.scope,
            turn: parsed.turn,
            ordinal: parsed.ordinal,
            at: Utc
                .timestamp_opt(created_at, 0)
                .single()
                .unwrap_or_else(Utc::now),
        })
    }

    /// The wire shape a client lists and draws `[u]` from.
    #[must_use]
    pub fn to_wire(&self) -> TurnCheckpoint {
        TurnCheckpoint {
            id: self.id.clone(),
            scope: self.scope,
            turn: self.turn,
            ordinal: self.ordinal,
            at: self.at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkpoint_ref_lives_outside_every_namespace_git_lists_or_pushes() {
        let thread = ThreadId::new();
        let id = CheckpointId::from_parts(1, CheckpointScope::Turn, TurnId::new());
        let name = ref_name(thread, &id);

        assert!(name.starts_with("refs/fleet/checkpoints/"));
        for listed in ["refs/heads/", "refs/tags/", "refs/remotes/", "refs/notes/"] {
            assert!(
                !name.starts_with(listed),
                "{name} must not live under {listed}"
            );
        }
        assert_eq!(parse_ref(&name), Some((thread, id)));
    }

    #[test]
    fn a_ref_that_is_not_a_checkpoint_is_never_attributed_to_a_thread() {
        for hostile in [
            "refs/heads/main",
            "refs/fleet/checkpoints",
            "refs/fleet/checkpoints/not-a-uuid/00001-turn-11111111-2222-4333-8444-555555555555",
            "refs/fleet/checkpoints/11111111-2222-4333-8444-555555555555",
            "refs/fleet/checkpoints/11111111-2222-4333-8444-555555555555/bogus",
        ] {
            assert_eq!(parse_ref(hostile), None, "{hostile} must not parse");
        }
    }

    #[test]
    fn a_metadata_body_round_trips_through_a_commit_message() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let metadata = Metadata::new(
            thread,
            turn,
            CheckpointScope::File,
            2,
            vec!["src/lib.rs".to_owned()],
        );
        let message = metadata.message().unwrap_or_else(|error| panic!("{error}"));
        let id = CheckpointId::from_parts(2, CheckpointScope::File, turn);

        assert!(message.starts_with(&format!("fleet checkpoint 2 (file) before turn {turn}")));
        let commit = format!("tree 4b825dc\nauthor Fleet <fleet@localhost> 0 +0000\n\n{message}");
        assert_eq!(
            Metadata::decode(&id, &commit).unwrap_or_else(|error| panic!("{error}")),
            metadata
        );
    }

    #[test]
    fn a_record_from_a_newer_build_is_refused_rather_than_guessed_at() {
        let id = CheckpointId::from_parts(1, CheckpointScope::Turn, TurnId::new());
        let commit = format!(
            "tree 4b825dc\n\nfleet checkpoint\n\n{}\n",
            serde_json::json!({
                "version": METADATA_VERSION + 1,
                "thread": ThreadId::new(),
                "turn": TurnId::new(),
                "scope": "turn",
                "ordinal": 1,
            })
        );

        let error = Metadata::decode(&id, &commit).expect_err("a newer record must be refused");
        assert!(matches!(error, CheckpointError::Metadata { .. }));
    }

    #[test]
    fn a_commit_with_no_record_is_refused() {
        let id = CheckpointId::from_parts(1, CheckpointScope::Turn, TurnId::new());

        assert!(Metadata::decode(&id, "tree 4b825dc").is_err());
        assert!(Metadata::decode(&id, "tree 4b825dc\n\njust prose\n").is_err());
    }

    #[test]
    fn a_listing_row_recovers_every_field_from_the_ref_name() {
        let turn = TurnId::new();
        let id = CheckpointId::from_parts(12, CheckpointScope::File, turn);
        let checkpoint = Checkpoint::from_ref(id.clone(), 1_760_000_000)
            .expect("a canonical identifier describes itself");

        assert_eq!(checkpoint.id, id);
        assert_eq!(checkpoint.ordinal, 12);
        assert_eq!(checkpoint.scope, CheckpointScope::File);
        assert_eq!(checkpoint.turn, turn);
        assert_eq!(checkpoint.at.timestamp(), 1_760_000_000);
        assert_eq!(checkpoint.to_wire().id, id);
    }
}
