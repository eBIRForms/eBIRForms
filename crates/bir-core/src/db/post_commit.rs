//! What a committed write reports about its follow-up: whether the
//! background worker was woken to pick the change up.

use super::{Database, DbError};

/// Result of requesting a worker refresh after a database write has already
/// committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostCommitRefreshStatus {
    /// An active worker was notified (or already had a notification pending).
    Requested,
    /// Immediate worker notification was unavailable. A later worker tick
    /// still sees the committed change.
    Deferred { warning: String },
    /// The refresh request itself failed. The primary write is still
    /// committed and must not be reported as rolled back.
    Failed { warning: String },
}

impl PostCommitRefreshStatus {
    /// Whether the refresh request was accepted (now or for a later tick).
    pub fn request_recorded(&self) -> bool {
        matches!(self, Self::Requested | Self::Deferred { .. })
    }

    /// A non-blocking warning suitable for logs or user notification.
    pub fn warning(&self) -> Option<&str> {
        match self {
            Self::Requested => None,
            Self::Deferred { warning } | Self::Failed { warning } => Some(warning),
        }
    }
}

/// A successful, committed database write plus the independent status of its
/// post-commit refresh request.
#[derive(Debug)]
#[must_use = "inspect refresh_status before presenting the committed write to the user"]
pub struct PostCommitWrite<T> {
    committed: T,
    refresh_status: PostCommitRefreshStatus,
}

impl<T> PostCommitWrite<T> {
    pub fn committed(&self) -> &T {
        &self.committed
    }

    pub fn refresh_status(&self) -> &PostCommitRefreshStatus {
        &self.refresh_status
    }

    pub fn into_parts(self) -> (T, PostCommitRefreshStatus) {
        (self.committed, self.refresh_status)
    }

    pub fn into_committed(self) -> T {
        self.committed
    }
}

impl Database {
    /// Wake the background worker after a committed write. Never fails; a
    /// worker that is not running picks the change up on its next tick.
    pub fn request_post_commit_refresh(&self) -> Result<(), DbError> {
        if let Some(reason) = deferred_wake_reason(crate::background_cron::wake_with_status()) {
            tracing::debug!(reason, "Post-commit worker wake was deferred");
        }
        Ok(())
    }

    pub(crate) fn finish_post_commit_write<T>(
        &self,
        committed: T,
        operation: &'static str,
    ) -> PostCommitWrite<T> {
        let refresh_status = match deferred_wake_reason(crate::background_cron::wake_with_status())
        {
            None => PostCommitRefreshStatus::Requested,
            Some(reason) => PostCommitRefreshStatus::Deferred {
                warning: format!(
                    "{operation} committed, but the immediate worker wake was deferred: {reason}"
                ),
            },
        };
        PostCommitWrite {
            committed,
            refresh_status,
        }
    }
}

fn deferred_wake_reason(
    status: crate::background_cron::WakeDispatchStatus,
) -> Option<&'static str> {
    use crate::background_cron::WakeDispatchStatus;

    match status {
        WakeDispatchStatus::Delivered | WakeDispatchStatus::AlreadyPending => None,
        WakeDispatchStatus::WorkerNotRunning => Some("the background worker is not running yet"),
        WakeDispatchStatus::WorkerClosed => Some("the background worker channel is closed"),
    }
}
