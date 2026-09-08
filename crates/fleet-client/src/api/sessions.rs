use super::{Result, expect_ack, unexpected};
use crate::Client;
use fleet_core::{
    agents::AttentionKind,
    config::Agent,
    ids::{SessionId, TerminalId, WorktreeId},
    sessions::{AgentActivity, Session},
};
use fleet_proto::{
    request::RequestBody,
    response::{ResponseBody, SleepResult},
};

impl Client {
    /// Sets one terminal's coding-agent activity from an explicit lifecycle hook.
    pub async fn set_agent_activity(
        &self,
        session: SessionId,
        terminal_id: TerminalId,
        activity: AgentActivity,
        attention: Option<AttentionKind>,
    ) -> Result<()> {
        expect_ack(
            "set_agent_activity",
            self.request(RequestBody::SetAgentActivity {
                session,
                terminal_id,
                activity,
                attention,
            })
            .await?,
        )
    }

    /// Ensures a worktree or agent session exists.
    pub async fn ensure_session(
        &self,
        worktree: Option<WorktreeId>,
        agent: Option<Agent>,
        sleep_previous: bool,
    ) -> Result<Session> {
        match self
            .request(RequestBody::EnsureSession {
                worktree,
                agent,
                sleep_previous,
            })
            .await?
        {
            ResponseBody::Session(session) => Ok(session),
            response => Err(unexpected("ensure_session", response)),
        }
    }

    /// Lists every daemon-owned session.
    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        match self.request(RequestBody::ListSessions).await? {
            ResponseBody::Sessions(sessions) => Ok(sessions),
            response => Err(unexpected("list_sessions", response)),
        }
    }

    /// Returns the daemon's active worktree session, when one is selected.
    pub async fn current_session(&self) -> Result<Option<SessionId>> {
        match self.request(RequestBody::CurrentSession).await? {
            ResponseBody::CurrentSession(session) => Ok(session),
            response => Err(unexpected("current_session", response)),
        }
    }

    /// Applies sleep policy to a session.
    pub async fn sleep_session(&self, session: SessionId) -> Result<SleepResult> {
        match self.request(RequestBody::SleepSession { session }).await? {
            ResponseBody::Slept(result) => Ok(result),
            response => Err(unexpected("sleep_session", response)),
        }
    }
}
