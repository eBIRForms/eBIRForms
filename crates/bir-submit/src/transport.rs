//! The queue's way out: [`SubmissionTransport`] over [`Submitter`].
//!
//! - Queued → Sending: `open_session` checks the sign-in, the device key
//!   and the server's keys. A failure there leaves the return Queued and
//!   the queue retries with backoff.
//! - Sending → Sent: a verified receipt. It is kept on the return; a
//!   dry-run receipt marks the send as a test.
//! - Not sent (the server says nothing reached BIR): Failed, retried with
//!   backoff.
//! - Unknown (BIR may have it): needs attention, never resent
//!   automatically.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bir_core::background_cron::{StoreFailure, StoreReport, SubmissionTransport, TransportError};

use crate::client::{SendError, Submitter};

pub struct BuwizTransport {
    submitter: Arc<Submitter>,
}

impl BuwizTransport {
    pub fn new(submitter: Arc<Submitter>) -> Self {
        Self { submitter }
    }
}

type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl SubmissionTransport for BuwizTransport {
    type Session = ();

    fn open_session<'a>(
        &'a self,
        _form_type: &'a str,
        _tin: &'a str,
    ) -> Boxed<'a, Result<(), TransportError>> {
        Box::pin(async move {
            self.submitter
                .ready()
                .await
                .map_err(|error| TransportError(error.to_string()))
        })
    }

    fn store_session<'a>(
        &'a self,
        session: (),
        filename: &'a str,
        payload: &'a [u8],
    ) -> Boxed<'a, Result<(), TransportError>> {
        Box::pin(async move {
            self.store_session_reported(session, filename, payload)
                .await
                .map(drop)
                .map_err(|failure| match failure {
                    StoreFailure::NotSent(error) | StoreFailure::MaybeSent(error) => error,
                })
        })
    }

    fn store_session_reported<'a>(
        &'a self,
        _session: (),
        filename: &'a str,
        payload: &'a [u8],
    ) -> Boxed<'a, Result<StoreReport, StoreFailure>> {
        Box::pin(async move {
            match self.submitter.send(filename, payload).await {
                Ok(sent) => Ok(StoreReport {
                    dry_run: sent.receipt.is_test(),
                    receipt: Some(sent.receipt_jws),
                }),
                Err(SendError::NotSent(reason)) => {
                    Err(StoreFailure::NotSent(TransportError(reason)))
                }
                Err(SendError::MaybeSent(reason)) => {
                    Err(StoreFailure::MaybeSent(TransportError(reason)))
                }
            }
        })
    }
}
