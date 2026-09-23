//! New threads this window asked for, selected the moment the daemon first shows them.
//!
//! `AgentThreadCreate` is answered twice: the daemon broadcasts the new thread's `AgentSummary`
//! and then replies with the same summary. The two reach the foreground on different paths —
//! the summary through the event pump, the reply through the request's own channel — so either
//! may land first. Selecting only on the reply left a window, wide on a loaded machine, in which
//! the new tab was already in the strip but not selected: `^s a` visibly did nothing, and
//! anything typed went to the tab the user had just left. A press is therefore recorded here,
//! and whichever answer shows its thread first selects it (`docs/NATIVE-AGENTS.md` §9.2).

use super::*;

use fleet_core::agents::AgentKind;

/// One `^s a` / `^s A` still waiting for its thread.
#[derive(Debug)]
pub(super) struct PendingCreate {
    token: CreateToken,
    worktree: WorktreeId,
    provider: AgentKind,
    /// The worktree's threads at the press; none of them can be the answer.
    before: HashSet<ThreadId>,
    answer: Answer,
}

/// How far first sight got with one pending create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// No listed thread answers it yet.
    Waiting,
    /// First sight selected this thread; the reply must not select it a second time.
    Selected(ThreadId),
    /// First sight could not select it, so the reply decides, as it always has.
    LeftToReply,
}

/// Names one pending create, so its reply retires exactly the press that sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateToken(u64);

impl AgentThreads {
    /// Records a create about to be sent for `worktree`.
    pub fn begin_create(&mut self, worktree: WorktreeId, provider: AgentKind) -> CreateToken {
        self.next_create = self.next_create.wrapping_add(1);
        let token = CreateToken(self.next_create);
        let before = self
            .summaries
            .iter()
            .filter(|summary| summary.worktree == worktree)
            .map(|summary| summary.thread)
            .collect();
        self.pending_creates.push(PendingCreate {
            token,
            worktree,
            provider,
            before,
            answer: Answer::Waiting,
        });
        token
    }

    /// Retires a create once its reply is handled, returning the thread first sight selected.
    pub fn finish_create(&mut self, token: CreateToken) -> Option<ThreadId> {
        let index = self
            .pending_creates
            .iter()
            .position(|pending| pending.token == token)?;
        match self.pending_creates.remove(index).answer {
            Answer::Selected(thread) => Some(thread),
            Answer::Waiting | Answer::LeftToReply => None,
        }
    }

    /// The oldest unanswered create a listed thread answers, and that thread.
    ///
    /// The answer is a top-level thread of the requested provider that appeared in the
    /// requested worktree after the press. A child is a delegation's, never a `^s a`'s.
    fn next_answer(&self) -> Option<(CreateToken, ThreadId)> {
        let taken: HashSet<ThreadId> = self
            .pending_creates
            .iter()
            .filter_map(|pending| match pending.answer {
                Answer::Selected(thread) => Some(thread),
                Answer::Waiting | Answer::LeftToReply => None,
            })
            .collect();
        self.pending_creates
            .iter()
            .filter(|pending| pending.answer == Answer::Waiting)
            .find_map(|pending| {
                self.summaries
                    .iter()
                    .find(|summary| {
                        summary.worktree == pending.worktree
                            && summary.provider == pending.provider
                            && summary.parent.is_none()
                            && !pending.before.contains(&summary.thread)
                            && !taken.contains(&summary.thread)
                    })
                    .map(|summary| (pending.token, summary.thread))
            })
    }

    fn settle_answer(&mut self, token: CreateToken, answer: Answer) {
        if let Some(pending) = self
            .pending_creates
            .iter_mut()
            .find(|pending| pending.token == token)
        {
            pending.answer = answer;
        }
    }
}

impl AppState {
    /// Selects every thread that answers a pending create, as soon as the mirror lists it.
    ///
    /// Runs after each summary and snapshot is applied. A selection refused here — the strip
    /// is full — is left to the reply, which reports it the way it always has, so the refusal
    /// is not repeated on every later summary.
    pub(crate) fn adopt_created_threads(&mut self) {
        while let Some((token, thread)) = self.agents.next_answer() {
            let answer = if self.select_agent_thread(thread) {
                Answer::Selected(thread)
            } else {
                Answer::LeftToReply
            };
            self.agents.settle_answer(token, answer);
        }
    }
}
