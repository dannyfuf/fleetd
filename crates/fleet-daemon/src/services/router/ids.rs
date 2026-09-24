//! Bidirectional local/remote identifier mappings.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use fleet_core::{
    agents::{AgentThreadSummary, ThreadId},
    board::BoardSummary,
    ids::{BoardId, CardId, HostId, JobId, TerminalId, WorktreeId},
    model::Worktree,
};

/// Identifiers removed when a host link goes down.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClearedIds {
    pub terminals: Vec<TerminalId>,
    pub sessions: Vec<String>,
    pub jobs: Vec<JobId>,
    pub worktrees: Vec<WorktreeId>,
    pub threads: Vec<ThreadId>,
    /// Worktree boards a host owned. Card→board rows are deliberately kept: see
    /// [`RemoteIds::clear_host`].
    pub boards: Vec<BoardId>,
}

#[derive(Default)]
struct Maps {
    terminal_local: BTreeMap<(HostId, TerminalId), TerminalId>,
    terminal_remote: BTreeMap<TerminalId, (HostId, TerminalId)>,
    job_local: BTreeMap<(HostId, JobId), JobId>,
    job_remote: BTreeMap<JobId, (HostId, JobId)>,
    worktrees: BTreeMap<WorktreeId, HostId>,
    threads: BTreeMap<ThreadId, HostId>,
    /// Only *worktree* boards. A host's context boards share this daemon's own context board
    /// ids by construction (`personal`, …), so registering one would hijack a local board.
    boards: BTreeMap<BoardId, HostId>,
    cards: BTreeMap<CardId, BoardId>,
    sessions: BTreeSet<String>,
}

/// Shared remote-id table used by request, response, event, and snapshot translation.
pub struct RemoteIds {
    next: Arc<AtomicU64>,
    maps: Arc<Mutex<Maps>>,
}

impl Clone for RemoteIds {
    fn clone(&self) -> Self {
        Self {
            next: Arc::clone(&self.next),
            maps: Arc::clone(&self.maps),
        }
    }
}

impl Default for RemoteIds {
    fn default() -> Self {
        // Production composition replaces this with the session runtime's allocator. Keeping the
        // standalone default in the upper half prevents collisions before that wiring is present.
        Self::new(Arc::new(AtomicU64::new(1 << 63)))
    }
}

impl RemoteIds {
    #[must_use]
    pub fn new(next: Arc<AtomicU64>) -> Self {
        Self {
            next,
            maps: Arc::new(Mutex::new(Maps::default())),
        }
    }

    pub fn local_terminal(&self, host: &HostId, remote: TerminalId) -> TerminalId {
        let mut maps = lock(&self.maps);
        if let Some(local) = maps.terminal_local.get(&(host.clone(), remote)) {
            return *local;
        }
        let local = TerminalId(self.next.fetch_add(1, Ordering::Relaxed));
        maps.terminal_local.insert((host.clone(), remote), local);
        maps.terminal_remote.insert(local, (host.clone(), remote));
        local
    }

    /// Restores a stable local terminal id after a remote daemon/link restart.
    pub fn restore_terminal(&self, host: &HostId, remote: TerminalId, local: TerminalId) {
        let mut maps = lock(&self.maps);
        if let Some(replaced) = maps.terminal_local.insert((host.clone(), remote), local)
            && replaced != local
        {
            maps.terminal_remote.remove(&replaced);
        }
        if let Some((old_host, old_remote)) =
            maps.terminal_remote.insert(local, (host.clone(), remote))
            && (old_host != *host || old_remote != remote)
        {
            maps.terminal_local.remove(&(old_host, old_remote));
        }
    }

    #[must_use]
    pub fn existing_local_terminal(&self, host: &HostId, remote: TerminalId) -> Option<TerminalId> {
        lock(&self.maps)
            .terminal_local
            .get(&(host.clone(), remote))
            .copied()
    }

    #[must_use]
    pub fn remote_terminal(&self, local: TerminalId) -> Option<(HostId, TerminalId)> {
        lock(&self.maps).terminal_remote.get(&local).cloned()
    }

    pub fn local_job(&self, host: &HostId, remote: &JobId) -> JobId {
        let mut maps = lock(&self.maps);
        if let Some(local) = maps.job_local.get(&(host.clone(), remote.clone())) {
            return local.clone();
        }
        let local = JobId::try_from(format!(
            "job-remote-{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        ))
        .expect("generated job id is valid");
        maps.job_local
            .insert((host.clone(), remote.clone()), local.clone());
        maps.job_remote
            .insert(local.clone(), (host.clone(), remote.clone()));
        local
    }

    #[must_use]
    pub fn remote_job(&self, local: &JobId) -> Option<(HostId, JobId)> {
        lock(&self.maps).job_remote.get(local).cloned()
    }

    #[must_use]
    pub fn local_session(&self, host: &HostId, remote: &str) -> String {
        let local = format!("{host}/{remote}");
        lock(&self.maps).sessions.insert(local.clone());
        local
    }

    #[must_use]
    pub fn remote_session(&self, local: &str) -> Option<(HostId, String)> {
        let (host, remote) = local.split_once('/')?;
        Some((HostId::try_from(host).ok()?, remote.to_owned()))
    }

    pub fn register_worktree(&self, host: &HostId, worktree: WorktreeId) {
        lock(&self.maps).worktrees.insert(worktree, host.clone());
    }

    pub fn register_thread(&self, host: &HostId, thread: ThreadId) {
        lock(&self.maps).threads.insert(thread, host.clone());
    }

    /// Records that `host` owns the worktree board `board`.
    pub fn register_board(&self, host: &HostId, board: BoardId) {
        lock(&self.maps).boards.insert(board, host.clone());
    }

    /// Records that `card` lives on `board`, the stable fact a card id alone cannot express.
    pub fn register_card(&self, board: &BoardId, card: CardId) {
        lock(&self.maps).cards.insert(card, board.clone());
    }

    /// Replaces the card set of one board, so a card the owner no longer lists is forgotten.
    pub fn replace_board_cards(&self, board: &BoardId, cards: impl IntoIterator<Item = CardId>) {
        let mut maps = lock(&self.maps);
        maps.cards.retain(|_, item_board| item_board != board);
        maps.cards
            .extend(cards.into_iter().map(|card| (card, board.clone())));
    }

    /// Adds the worktree boards a host's summary listing names, never removing one.
    ///
    /// A listing is only *one* source of board ownership, and a stale one races the answer to the
    /// request that created a board: a snapshot fragment older than a forwarded `Board(view)`
    /// would forget the new board, and a card mutation on it would then be answered locally with
    /// "card not found". Forgetting a board the host deleted buys nothing either — forwarding a
    /// request for it gets the owner's `not found`, which is the right answer.
    /// [`Self::clear_host`] is the only place board ownership is removed.
    pub fn register_host_boards(&self, host: &HostId, boards: &[BoardSummary]) {
        lock(&self.maps)
            .boards
            .extend(worktree_boards(boards, host));
    }

    /// Atomically replaces the worktree and thread ownership inventory for one host, and adds the
    /// worktree boards it lists (see [`Self::register_host_boards`] for why boards only grow).
    pub fn replace_host_inventory(
        &self,
        host: &HostId,
        worktrees: &[Worktree],
        threads: &[AgentThreadSummary],
        boards: &[BoardSummary],
    ) {
        let mut maps = lock(&self.maps);
        maps.worktrees.retain(|_, item_host| item_host != host);
        maps.threads.retain(|_, item_host| item_host != host);
        maps.worktrees.extend(
            worktrees
                .iter()
                .map(|worktree| (worktree.id.clone(), host.clone())),
        );
        maps.threads
            .extend(threads.iter().map(|summary| (summary.thread, host.clone())));
        maps.boards.extend(worktree_boards(boards, host));
    }

    pub fn forget_terminal(&self, local: TerminalId) {
        let mut maps = lock(&self.maps);
        if let Some(key) = maps.terminal_remote.remove(&local) {
            maps.terminal_local.remove(&key);
        }
    }

    pub fn forget_job(&self, local: &JobId) {
        let mut maps = lock(&self.maps);
        if let Some(key) = maps.job_remote.remove(local) {
            maps.job_local.remove(&key);
        }
    }

    pub fn forget_session(&self, local: &str) {
        lock(&self.maps).sessions.remove(local);
    }

    pub fn forget_worktree(&self, worktree: &WorktreeId) {
        lock(&self.maps).worktrees.remove(worktree);
    }

    pub fn forget_thread(&self, thread: ThreadId) {
        lock(&self.maps).threads.remove(&thread);
    }

    pub fn clear_host(&self, host: &HostId) -> ClearedIds {
        let mut maps = lock(&self.maps);
        let terminals = maps
            .terminal_remote
            .iter()
            .filter(|(_, (item_host, _))| item_host == host)
            .map(|(local, _)| *local)
            .collect::<Vec<_>>();
        let jobs = maps
            .job_remote
            .iter()
            .filter(|(_, (item_host, _))| item_host == host)
            .map(|(local, _)| local.clone())
            .collect::<Vec<_>>();
        let threads = maps
            .threads
            .iter()
            .filter(|(_, item_host)| *item_host == host)
            .map(|(thread, _)| *thread)
            .collect::<Vec<_>>();
        let worktrees = maps
            .worktrees
            .iter()
            .filter(|(_, item_host)| *item_host == host)
            .map(|(worktree, _)| worktree.clone())
            .collect::<Vec<_>>();
        let boards = maps
            .boards
            .iter()
            .filter(|(_, item_host)| *item_host == host)
            .map(|(board, _)| board.clone())
            .collect::<Vec<_>>();
        let sessions = maps
            .sessions
            .iter()
            .filter(|session| session.starts_with(&format!("{host}/")))
            .cloned()
            .collect::<Vec<_>>();
        maps.terminal_remote
            .retain(|_, (item_host, _)| item_host != host);
        maps.terminal_local
            .retain(|(item_host, _), _| item_host != host);
        maps.job_remote
            .retain(|_, (item_host, _)| item_host != host);
        maps.job_local.retain(|(item_host, _), _| item_host != host);
        maps.worktrees.retain(|_, item_host| item_host != host);
        maps.threads.retain(|_, item_host| item_host != host);
        maps.boards.retain(|_, item_host| item_host != host);
        // Card→board rows survive on purpose: which board a card belongs to is a fact about the
        // card, not about the link, and keeping it lets the mirror fallback answer `host_of_card`
        // for the whole window in which a host is down.
        maps.sessions
            .retain(|session| !session.starts_with(&format!("{host}/")));
        ClearedIds {
            terminals,
            sessions,
            jobs,
            worktrees,
            threads,
            boards,
        }
    }

    #[must_use]
    pub fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId> {
        lock(&self.maps).worktrees.get(id).cloned()
    }
    #[must_use]
    pub fn host_of_session(&self, id: &str) -> Option<HostId> {
        lock(&self.maps)
            .sessions
            .contains(id)
            .then(|| self.remote_session(id))
            .flatten()
            .map(|(host, _)| host)
    }
    #[must_use]
    pub fn host_of_terminal(&self, id: TerminalId) -> Option<HostId> {
        self.remote_terminal(id).map(|(host, _)| host)
    }
    #[must_use]
    pub fn host_of_job(&self, id: &JobId) -> Option<HostId> {
        self.remote_job(id).map(|(host, _)| host)
    }
    #[must_use]
    pub fn host_of_thread(&self, id: &ThreadId) -> Option<HostId> {
        lock(&self.maps).threads.get(id).cloned()
    }
    #[must_use]
    pub fn host_of_board(&self, id: &BoardId) -> Option<HostId> {
        lock(&self.maps).boards.get(id).cloned()
    }
    #[must_use]
    pub fn board_of_card(&self, id: &CardId) -> Option<BoardId> {
        lock(&self.maps).cards.get(id).cloned()
    }
}

/// The worktree-scoped subset of a host's board listing, paired with its owner.
///
/// Context boards are filtered out here rather than at every call site: their ids are derived
/// from context ids, which every daemon shares, so a hosted `personal` board would shadow this
/// daemon's own.
fn worktree_boards<'a>(
    boards: &'a [BoardSummary],
    host: &'a HostId,
) -> impl Iterator<Item = (BoardId, HostId)> + 'a {
    boards
        .iter()
        .filter(|summary| summary.worktree_id.is_some())
        .map(|summary| (summary.id.clone(), host.clone()))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_adds_boards_and_never_removes_one_only_clear_host_does() {
        let ids = RemoteIds::default();
        let owner = host("dev-box");
        let other = host("laptop-two");
        let owned = board("wt-acme-api-feature");
        let elsewhere = board("wt-acme-api-other");
        ids.register_board(&owner, owned.clone());
        ids.register_board(&other, elsewhere.clone());

        assert_eq!(ids.host_of_board(&owned), Some(owner.clone()));

        // A listing that predates a board's creation must not un-register it: the snapshot this
        // daemon holds is routinely older than the answer that registered the newest board.
        ids.register_host_boards(&owner, &[worktree_summary("wt-acme-api-later")]);
        assert_eq!(ids.host_of_board(&owned), Some(owner.clone()));
        assert_eq!(
            ids.host_of_board(&board("wt-acme-api-later")),
            Some(owner.clone())
        );
        assert_eq!(ids.host_of_board(&elsewhere), Some(other));

        let cleared = ids.clear_host(&owner);
        assert_eq!(
            cleared.boards,
            vec![owned.clone(), board("wt-acme-api-later")]
        );
        assert_eq!(ids.host_of_board(&owned), None);
        assert_eq!(ids.host_of_board(&board("wt-acme-api-later")), None);
    }

    #[test]
    fn a_context_board_summary_is_never_registered_against_a_host() {
        let ids = RemoteIds::default();
        let owner = host("dev-box");
        let context = context_summary("personal");
        let scoped = worktree_summary("wt-acme-api-feature");

        ids.replace_host_inventory(&owner, &[], &[], &[context.clone(), scoped.clone()]);

        // `personal` names this daemon's own context board too; claiming the host owns it would
        // route every local context-board request off the machine.
        assert_eq!(ids.host_of_board(&context.id), None);
        assert_eq!(ids.host_of_board(&scoped.id), Some(owner.clone()));

        ids.register_host_boards(&owner, std::slice::from_ref(&context));
        assert_eq!(ids.host_of_board(&context.id), None);
    }

    #[test]
    fn card_to_board_rows_survive_clear_host_and_are_replaced_per_board() {
        let ids = RemoteIds::default();
        let owner = host("dev-box");
        let owned = board("wt-acme-api-feature");
        let kept = card("card-kept");
        let dropped = card("card-dropped");
        ids.register_board(&owner, owned.clone());
        ids.register_card(&owned, kept.clone());
        ids.register_card(&owned, dropped.clone());

        ids.replace_board_cards(&owned, [kept.clone()]);
        assert_eq!(ids.board_of_card(&kept), Some(owned.clone()));
        assert_eq!(ids.board_of_card(&dropped), None);

        // A down link forgets who owns the *board*; which board a card is on is not a fact about
        // the link, so the mirror fallback can still resolve the card afterwards.
        ids.clear_host(&owner);
        assert_eq!(ids.host_of_board(&owned), None);
        assert_eq!(ids.board_of_card(&kept), Some(owned));
    }

    fn host(value: &str) -> HostId {
        value.parse().expect("host id")
    }

    fn board(value: &str) -> BoardId {
        value.parse().expect("board id")
    }

    fn card(value: &str) -> CardId {
        value.parse().expect("card id")
    }

    fn worktree_summary(id: &str) -> BoardSummary {
        BoardSummary {
            worktree_id: Some(WorktreeId::try_from("acme/api#feature").expect("worktree id")),
            ..context_summary(id)
        }
    }

    fn context_summary(id: &str) -> BoardSummary {
        BoardSummary {
            id: board(id),
            context_id: "personal".parse().expect("context id"),
            worktree_id: None,
            kind: Default::default(),
            name: id.to_owned(),
            prefix: "FLT".to_owned(),
            backend_kind: "local".to_owned(),
            card_count: 0,
            open_count: 0,
            dirty_count: 0,
            conflict_count: 0,
            working_count: 0,
            attention_count: 0,
            idle_started: 0,
            last_synced_at: None,
            last_error: None,
        }
    }
}
