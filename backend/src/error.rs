use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("reservation failed: {0}")]
    ReservationFailed(String),

    #[error("commit failed: {0}")]
    CommitFailed(String),

    #[error("scheduler invariant violated: {0}")]
    SchedulerInvariant(String),
}
