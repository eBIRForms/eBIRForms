//! Sending returns: the submission client and the queue worker, over
//! `bir-submit`. The UI calls [`BirApp::submission`] once, then the
//! client's `sign_in_status`, `start_sign_in` / `poll_sign_in` and
//! `send_now`.

use bir_submit::Submission;

use crate::{AppResult, BirApp};

/// The setting that holds the server URL. `BUWIZ_API_URL` overrides it.
pub const SERVER_URL_SETTING: &str = "submission_server_url";

impl BirApp {
    /// The submission client for this app's server URL setting.
    pub fn submission(&self) -> AppResult<Submission> {
        Ok(Submission::new(
            self.get_setting(SERVER_URL_SETTING)?.as_deref(),
        ))
    }

    /// Starts the queue worker, sending through `submission`.
    pub fn start_sending(
        &self,
        submission: &Submission,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        self.start_queue_worker(submission.transport())
    }
}

impl BirApp {
    /// What the app runs at launch, off the main thread: the submission
    /// client for this database's server setting, and the queue worker
    /// sending through it. Until someone signs in the queue stays paused,
    /// so a signed-out send is not counted as a failed try (five of those
    /// would put the return back to Draft); signing in resumes it.
    pub fn launch_sending(&self) -> AppResult<Submission> {
        let submission = self.submission()?;
        if !matches!(submission.sign_in_status(), bir_submit::SignInStatus::SignedIn { .. }) {
            bir_core::background_cron::set_submission_paused(true);
        }
        self.start_sending(&submission)
            .map_err(|error| crate::AppError::Call(format!("the queue worker did not start: {error}")))?;
        Ok(submission)
    }
}

/// This process's hold on the send queue: the worker lease
/// ([`bir_core::db::WorkerLease`]), which only one process sharing the
/// database holds at a time. Dropping it lets another process's worker take
/// over.
pub struct QueueLease(#[allow(dead_code)] bir_core::db::WorkerLease);

impl BirApp {
    /// The worker lease, when no other process holds it (`None`: another
    /// app runs the queue, and its worker wakes on this process's commits).
    /// For a process that sends what it just queued without running the
    /// worker, such as `ebirdesk-mcp`.
    pub fn try_queue_lease(&self) -> AppResult<Option<QueueLease>> {
        Ok(bir_core::db::WorkerLease::try_acquire(self.database_path())?.map(QueueLease))
    }

    /// Run the sending queue once, now, on the calling thread, through
    /// `submission`'s relay (the same transport as the queue worker). Only
    /// with the worker lease in hand, so two processes never run the queue
    /// at once. Returns when every due return was tried.
    pub fn send_queue_once(&self, _lease: &QueueLease, submission: &Submission) {
        let transport = submission.transport();
        block_on(bir_core::background_cron::run_queue_tick(self.db.clone(), &transport));
    }
}

/// Run one of the client's async calls to the end on the calling (worker)
/// thread.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}

/// Pause the queue (after signing out) or resume it.
pub fn set_sending_paused(paused: bool) {
    bir_core::background_cron::set_submission_paused(paused);
}
