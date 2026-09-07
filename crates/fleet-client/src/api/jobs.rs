use super::{Result, unexpected};
use crate::Client;
use fleet_core::ids::JobId;
use fleet_proto::{job::JobRecord, request::RequestBody, response::ResponseBody};

impl Client {
    /// Lists current and recently completed jobs.
    pub async fn list_jobs(&self) -> Result<Vec<JobRecord>> {
        match self.request(RequestBody::ListJobs).await? {
            ResponseBody::Jobs(jobs) => Ok(jobs),
            response => Err(unexpected("list_jobs", response)),
        }
    }

    /// Cancels a cancellable job.
    pub async fn cancel_job(&self, job: JobId) -> Result<JobId> {
        match self.request(RequestBody::CancelJob { job }).await? {
            ResponseBody::JobCancelled(job) => Ok(job),
            response => Err(unexpected("cancel_job", response)),
        }
    }

    /// Reads trailing lines from a job log.
    pub async fn tail_job(&self, job: JobId, lines: usize) -> Result<Vec<String>> {
        match self.request(RequestBody::TailJob { job, lines }).await? {
            ResponseBody::JobLog(lines) => Ok(lines),
            response => Err(unexpected("tail_job", response)),
        }
    }
}
