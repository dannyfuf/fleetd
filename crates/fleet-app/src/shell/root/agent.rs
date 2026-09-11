use super::Shell;
use crate::{actions::fleet, state::AgentPopupTransition};
use fleet_core::{config::Agent, ids::WorktreeId};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use gpui::{Context, Window};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct AgentEnsureKey {
    agent: Agent,
    /// The worktree the popup is scoped to, or `None` for the repository-level popup.
    worktree: Option<WorktreeId>,
    generation: u64,
}

impl Hash for AgentEnsureKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        AgentEnsureFlights::agent_index(self.agent).hash(state);
        self.worktree.as_ref().map(WorktreeId::as_str).hash(state);
        self.generation.hash(state);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AgentEnsureClaim {
    key: AgentEnsureKey,
    nonce: u64,
}

#[derive(Debug, Default)]
pub(super) struct AgentEnsureFlights {
    in_flight: HashMap<AgentEnsureKey, u64>,
    current: [Option<u64>; 2],
    next_nonce: u64,
}

impl AgentEnsureFlights {
    /// The popup slot one agent occupies. The legacy OpenCode value shares the second slot
    /// with Codex, which is safe because `config.agent` names exactly one of them at a time.
    const fn agent_index(agent: Agent) -> usize {
        match agent {
            Agent::Claude => 0,
            Agent::Codex | Agent::Opencode => 1,
        }
    }

    fn claim(&mut self, key: AgentEnsureKey) -> Option<AgentEnsureClaim> {
        if self.in_flight.contains_key(&key) {
            return None;
        }
        self.next_nonce = self.next_nonce.wrapping_add(1);
        let claim = AgentEnsureClaim {
            key: key.clone(),
            nonce: self.next_nonce,
        };
        let agent = Self::agent_index(key.agent);
        self.in_flight.insert(key, claim.nonce);
        self.current[agent] = Some(claim.nonce);
        Some(claim)
    }

    /// Finishes an exact flight and reports whether its response still owns the agent claim.
    fn finish(&mut self, claim: &AgentEnsureClaim, current_generation: u64) -> bool {
        if self.in_flight.get(&claim.key) != Some(&claim.nonce) {
            return false;
        }
        self.in_flight.remove(&claim.key);
        let current = &mut self.current[Self::agent_index(claim.key.agent)];
        if *current != Some(claim.nonce) {
            return false;
        }
        *current = None;
        claim.key.generation == current_generation
    }
}

impl Shell {
    pub(super) fn open_agent_claude(
        &mut self,
        _: &fleet::OpenAgentClaude,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_agent(Agent::Claude, cx);
    }

    pub(super) fn open_agent_codex(
        &mut self,
        _: &fleet::OpenAgentCodex,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_agent(Agent::Codex, cx);
    }

    /// Opens or switches the independent popup and ensures its fixed daemon session exists.
    fn toggle_agent(&mut self, agent: Agent, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let hiding_current = state
            .agent_popup
            .as_ref()
            .is_some_and(|popup| popup.agent == agent && popup.worktree.is_none());
        let preserve = state
            .active_session()
            .and_then(|session| session.active_terminal);
        if state.refuses_mutations() && !hiding_current {
            return;
        }
        let transition = self.state.update(cx, |state, cx| {
            let transition = state.toggle_agent_popup(agent, None);
            cx.notify();
            transition
        });
        if matches!(
            transition,
            AgentPopupTransition::Hidden | AgentPopupTransition::Switched
        ) {
            self.agent_popup.detach(&self.bridge, preserve);
        }
        if transition == AgentPopupTransition::Hidden {
            return;
        }
        self.reconcile_agent_session(cx);
    }

    /// Keeps a visible fixed agent session present, with one request per agent/link generation.
    pub(super) fn reconcile_agent_session(&mut self, cx: &mut Context<Self>) {
        if !self.state.read(cx).daemon.is_connected() {
            self.agent_popup.discard_pending();
            return;
        }
        let Some((agent, worktree)) = self.state.read(cx).missing_agent_popup_session() else {
            return;
        };
        let key = AgentEnsureKey {
            agent,
            worktree: worktree.clone(),
            generation: self.state.read(cx).link_generation,
        };
        let Some(claim) = self.agent_ensures.claim(key) else {
            return;
        };
        // §1/§2: `^s F` is the same-worktree fallback, so the ensured session is the worktree's
        // own; the Hub's repository-level popup passes no worktree and keeps `repos_dir`.
        let reply = self.bridge.request(RequestBody::EnsureSession {
            worktree,
            agent: Some(agent),
            sleep_previous: false,
        });
        cx.spawn(async move |shell, cx| {
            let answer = reply.recv().await;
            let _ = shell.update(cx, |shell, cx| {
                let generation = shell.state.read(cx).link_generation;
                if !shell.agent_ensures.finish(&claim, generation) {
                    return;
                }
                match answer {
                    Ok(Ok(ResponseBody::Session(session))) => {
                        shell.state.update(cx, |app, cx| {
                            app.apply_session(session);
                            cx.notify();
                        });
                    }
                    Ok(Err(error)) => shell.fail_agent_ensure(&claim.key, error.message, cx),
                    Ok(Ok(_)) => shell.fail_agent_ensure(
                        &claim.key,
                        "daemon returned an unexpected response; press a/A to retry".to_owned(),
                        cx,
                    ),
                    // A lost reply accompanies a bridge disconnect. The reconnect event advances
                    // the generation and re-enters this single-flight gate.
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    fn fail_agent_ensure(
        &mut self,
        claim: &AgentEnsureKey,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let still_selected = self
            .state
            .read(cx)
            .agent_popup
            .as_ref()
            .is_some_and(|popup| {
                popup.agent == claim.agent
                    && popup.worktree == claim.worktree
                    && self.state.read(cx).link_generation == claim.generation
            });
        if !still_selected {
            return;
        }
        let preserve = self
            .state
            .read(cx)
            .active_session()
            .and_then(|session| session.active_terminal);
        self.agent_popup.detach(&self.bridge, preserve);
        self.state.update(cx, |app, cx| {
            app.hide_agent_popup();
            app.sticky_error = Some(crate::state::StickyError {
                text: format!("agent popup could not start: {message}"),
                job: None,
                retryable: false,
            });
            cx.notify();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ensure_ignores_previous_link_generation() {
        let mut flights = AgentEnsureFlights::default();
        let old_key = AgentEnsureKey {
            agent: Agent::Claude,
            worktree: None,
            generation: 3,
        };
        let old = flights.claim(old_key.clone()).expect("first claim");
        assert!(
            flights.claim(old_key.clone()).is_none(),
            "same key stays single-flight"
        );

        assert!(
            !flights.finish(&old, 4),
            "a reply from the previous link is stale even before a replacement claim exists"
        );

        let superseded = flights
            .claim(old_key)
            .expect("the completed old flight may be retried");
        let current = flights
            .claim(AgentEnsureKey {
                agent: Agent::Claude,
                worktree: None,
                generation: 4,
            })
            .expect("new link generation gets a distinct flight");
        assert!(
            !flights.finish(&superseded, 4),
            "a newer flight supersedes the old nonce before its reply arrives"
        );
        assert!(flights.finish(&current, 4));
    }

    #[test]
    fn the_same_agent_in_two_worktrees_is_two_flights() {
        let mut flights = AgentEnsureFlights::default();
        let worktree: WorktreeId = "acme/api#feature"
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        let repos = AgentEnsureKey {
            agent: Agent::Claude,
            worktree: None,
            generation: 1,
        };
        let scoped = AgentEnsureKey {
            agent: Agent::Claude,
            worktree: Some(worktree),
            generation: 1,
        };
        let first = flights.claim(repos).expect("the repository-level popup");
        // §1/§2: `^s F` ensures the worktree's own fallback session, which is a different
        // session from the Hub's — one in flight must not block the other.
        let second = flights.claim(scoped).expect("the worktree fallback");
        assert!(
            !flights.finish(&first, 1),
            "the second claim owns the agent"
        );
        assert!(flights.finish(&second, 1));
    }
}
