use super::{Result, expect_ack, unexpected};
use crate::{Client, connection::schedules_capability_error};
use fleet_core::{
    ids::{BoardId, ScheduleId},
    schedule::{Schedule, ScheduleDraft, SchedulePatch},
};
use fleet_proto::{
    request::RequestBody,
    response::{ResponseBody, SCHEDULES_CAPABILITY},
};

impl Client {
    /// Lists the schedules of one board, or of every board.
    pub async fn list_schedules(&self, board_id: Option<BoardId>) -> Result<Vec<Schedule>> {
        self.require_schedules_capability()?;
        match self
            .request(RequestBody::ListSchedules { board_id })
            .await?
        {
            ResponseBody::Schedules(value) => Ok(value),
            response => Err(unexpected("list_schedules", response)),
        }
    }

    /// Creates a schedule.
    pub async fn create_schedule(&self, draft: ScheduleDraft) -> Result<Schedule> {
        self.require_schedules_capability()?;
        match self.request(RequestBody::CreateSchedule { draft }).await? {
            ResponseBody::Schedule(value) => Ok(value),
            response => Err(unexpected("create_schedule", response)),
        }
    }

    /// Applies a patch to one schedule.
    pub async fn update_schedule(&self, id: ScheduleId, patch: SchedulePatch) -> Result<Schedule> {
        self.require_schedules_capability()?;
        match self
            .request(RequestBody::UpdateSchedule { id, patch })
            .await?
        {
            ResponseBody::Schedule(value) => Ok(value),
            response => Err(unexpected("update_schedule", response)),
        }
    }

    /// Deletes one schedule and its run logs.
    pub async fn delete_schedule(&self, id: ScheduleId) -> Result<()> {
        self.require_schedules_capability()?;
        expect_ack(
            "delete_schedule",
            self.request(RequestBody::DeleteSchedule { id }).await?,
        )
    }

    /// Fires one schedule now, answering it with the run recorded as started.
    pub async fn run_schedule_now(&self, id: ScheduleId) -> Result<Schedule> {
        self.require_schedules_capability()?;
        match self.request(RequestBody::RunScheduleNow { id }).await? {
            ResponseBody::Schedule(value) => Ok(value),
            response => Err(unexpected("run_schedule_now", response)),
        }
    }

    fn require_schedules_capability(&self) -> Result<()> {
        if self.supports_capability(SCHEDULES_CAPABILITY) {
            Ok(())
        } else {
            Err(schedules_capability_error())
        }
    }
}
