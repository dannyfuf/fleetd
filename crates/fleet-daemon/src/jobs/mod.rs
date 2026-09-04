//! Detached background job execution and tracking.

pub mod job;
pub mod manager;

pub use job::JobCtx;
pub use manager::JobManager;
