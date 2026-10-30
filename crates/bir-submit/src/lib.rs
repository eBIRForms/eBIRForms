//! The client half of submission. A queued return leaves this crate as
//! plaintext XML sealed for buwiz-server, with a ticket and a device
//! signature, and comes back as a signed receipt.
//!
//! - [`Submitter`]: device-flow sign-in, tokens and the device key in the
//!   OS credential store ([`secrets`]), tickets, the HPKE seal ([`seal`]),
//!   the v2 submission, the relay pipe ([`pipe`]) limited to the hosts of
//!   an offline-signed trust list ([`trust`]), and receipt checks
//!   ([`receipt`]).
//! - [`BuwizTransport`]: the queue's `SubmissionTransport`.
//! - [`Submission`]: the small API the app calls.
//!
//! This crate holds no BIR protocol constant, passphrase, endpoint or SFTP
//! code. Where the pipe may connect comes from the server's signed trust
//! list, trusted only through a pinned public key; the list also carries
//! the only sealing and receipt keys the app accepts.

pub mod client;
pub mod device;
pub mod pipe;
pub mod receipt;
pub mod seal;
pub mod secrets;
pub mod transport;
pub mod trust;

use std::sync::Arc;

pub use client::{
    ClientError, DEFAULT_SERVER_URL, DeviceCode, PollOutcome, SendError, Sent, SignInStatus,
    SubmitConfig, Submitter,
};
pub use transport::BuwizTransport;

/// The app's door into submission: sign-in and "send now". The queue
/// worker runs on its own thread with [`Submission::transport`].
#[derive(Clone)]
pub struct Submission {
    submitter: Arc<Submitter>,
}

impl Submission {
    /// `server_url_setting` is the app's setting; `BUWIZ_SERVER_URL`
    /// overrides it. Secrets go to the platform's credential store.
    pub fn new(server_url_setting: Option<&str>) -> Self {
        Self::with(
            SubmitConfig::new(server_url_setting),
            secrets::platform_store(),
        )
    }

    pub fn with(config: SubmitConfig, store: Box<dyn secrets::SecretStore>) -> Self {
        Self {
            submitter: Arc::new(Submitter::new(config, store)),
        }
    }

    pub fn submitter(&self) -> &Arc<Submitter> {
        &self.submitter
    }

    /// For [`bir_core::background_cron::start_cron_jobs`] (through
    /// `BirApp::start_queue_worker`).
    pub fn transport(&self) -> Arc<BuwizTransport> {
        Arc::new(BuwizTransport::new(self.submitter.clone()))
    }

    pub fn sign_in_status(&self) -> SignInStatus {
        self.submitter.sign_in_status()
    }

    /// Starts sign-in: show `user_code`, open `verification_uri_complete`,
    /// then call [`Self::poll_sign_in`] every `interval` seconds until it is
    /// no longer pending. Signing in also lets the queue run.
    pub async fn start_sign_in(&self) -> Result<DeviceCode, ClientError> {
        self.submitter.start_sign_in().await
    }

    pub async fn poll_sign_in(&self) -> Result<PollOutcome, ClientError> {
        let outcome = self.submitter.poll_sign_in().await?;
        if outcome == PollOutcome::SignedIn {
            bir_core::background_cron::set_submission_paused(false);
        }
        Ok(outcome)
    }

    pub async fn sign_out(&self) -> Result<(), ClientError> {
        self.submitter.sign_out().await
    }

    /// Runs the queue now instead of at its next minute.
    pub fn send_now(&self) {
        bir_core::background_cron::wake();
    }
}
