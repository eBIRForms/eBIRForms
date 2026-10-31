//! Sending what this process queued, beside the desktop app.
//!
//! The database is shared (`bir_core::db::shared`): the desktop app and
//! this process both open it, read and write at once, and see each other's
//! commits. Only one process runs the send queue, the one holding the
//! worker lease. This process never starts the queue worker; after it
//! queues a return it asks for the lease once ([`BirApp::try_queue_lease`]):
//!
//! - another app holds it (the desktop app is open): that app's worker
//!   wakes on this process's commit and sends the return;
//! - nobody holds it: this process runs the queue once through bir-submit's
//!   relay, the same transport the worker uses, on a thread that keeps the
//!   lease until the run ends, then lets it go.
//!
//! A return that fails to go out is retried by whichever app runs the
//! worker next, never by a timer here.

use std::sync::Arc;
use std::thread::JoinHandle;

use bir_app_core::BirApp;
use bir_app_core::bir_submit::SignInStatus;

/// What happened to the queue after a return was queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sending {
    /// This process is running the queue now, through the relay.
    Now,
    /// Another app runs the queue; its worker sends the return.
    ByOtherApp,
    /// This computer is not signed in: the return waits in the queue.
    SignedOut,
}

impl Sending {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::ByOtherApp => "by_other_app",
            Self::SignedOut => "signed_out",
        }
    }
}

/// Whether this computer is signed in to Buwiz for sending (`Some`, with
/// the account's email when known). With `BUWIZ_NO_KEYCHAIN` set the
/// keychain, where the sign-in lives, is not read.
pub(crate) fn sign_in(app: &BirApp) -> Result<Option<Option<String>>, String> {
    if bir_app_core::bir_core::keychain_disabled() {
        return Err("BUWIZ_NO_KEYCHAIN is set, so the sign-in in the keychain is not read".into());
    }
    let submission = app.submission().map_err(|error| error.to_string())?;
    Ok(match submission.sign_in_status() {
        SignInStatus::SignedIn { email } => Some(email),
        _ => None,
    })
}

/// Sends started by this process, joined before it exits so a send is never
/// cut off halfway.
#[derive(Default)]
pub(crate) struct Sends {
    running: Vec<JoinHandle<()>>,
}

impl Sends {
    /// After a return was queued: run the queue once through the relay
    /// when this computer is signed in and no other app runs it.
    pub(crate) fn start(&mut self, app: &Arc<BirApp>) -> Result<Sending, String> {
        self.running.retain(|send| !send.is_finished());
        if bir_app_core::bir_core::keychain_disabled() {
            return Ok(Sending::SignedOut);
        }
        let submission = app.submission().map_err(|error| error.to_string())?;
        if !matches!(submission.sign_in_status(), SignInStatus::SignedIn { .. }) {
            return Ok(Sending::SignedOut);
        }
        let Some(lease) = app.try_queue_lease().map_err(|error| error.to_string())? else {
            return Ok(Sending::ByOtherApp);
        };
        let app = app.clone();
        let send = std::thread::Builder::new()
            .name("ebirdesk-mcp-send".into())
            .spawn(move || {
                app.send_queue_once(&lease, &submission);
                drop(lease);
            })
            .map_err(|error| format!("the send did not start: {error}"))?;
        self.running.push(send);
        Ok(Sending::Now)
    }

    /// Wait for every send to end.
    pub(crate) fn join(&mut self) {
        for send in self.running.drain(..) {
            let _ = send.join();
        }
    }
}
