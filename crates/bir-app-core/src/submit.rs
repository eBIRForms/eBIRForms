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
