//! Sending returns: the submission client and the queue worker, over
//! `bir-submit`. The UI calls [`BirApp::submission`] once, then the
//! client's `sign_in_status`, `start_sign_in` / `poll_sign_in` and
//! `send_now`.

use bir_submit::Submission;

use crate::{AppResult, BirApp};

/// The setting that holds the server URL. `BUWIZ_SERVER_URL` overrides it.
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
