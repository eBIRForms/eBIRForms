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

    /// [`Self::submission`], naming the calling app and its version in the
    /// `buwiz-client` header of every request it makes
    /// (`submission_as("ebirdesk-mcp", env!("CARGO_PKG_VERSION"))`). The
    /// desktop app and `ebirdesk-mcp` say their own; the mini app, which
    /// builds its own `Submission`, must call
    /// `bir_submit::Submission::set_client("mini", <its version>)` at start.
    /// A name or version with odd characters is cleaned, never an error.
    pub fn submission_as(&self, app: &str, version: &str) -> AppResult<Submission> {
        let submission = self.submission()?;
        submission.set_client(app, version);
        Ok(submission)
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
    /// sending through it. Until someone signs in the queue stays paused, so
    /// it does not ask Buwiz to send while signed out; signing in resumes
    /// it. (A signed-out send is not a try in any case.)
    pub fn launch_sending(&self) -> AppResult<Submission> {
        self.launch_sending_as("desktop", env!("CARGO_PKG_VERSION"))
    }

    /// [`Self::launch_sending`] for the app `app` at `version`, which the
    /// BIR sign's server sees in every request (`launch_sending_as("desktop",
    /// env!("CARGO_PKG_VERSION"))` in the desktop app's own crate).
    pub fn launch_sending_as(&self, app: &str, version: &str) -> AppResult<Submission> {
        let submission = self.submission_as(app, version)?;
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
    /// at once. Like every tick it sends at most one return, the head of the
    /// line (due soonest, across every TIN), and only when its own time has
    /// come; it returns when that send ended.
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
