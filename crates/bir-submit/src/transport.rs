//! The queue's way out: [`SubmissionTransport`] over [`Submitter`].
//!
//! - Queued → Sending: `open_session` checks the sign-in, the device key
//!   and the server's keys. A failure there leaves the return Queued; it
//!   talks to Buwiz only, so it is not a try (`TryClass::NotATry`).
//! - Sending → Sent: a verified receipt. It is kept on the return; a
//!   dry-run receipt marks the send as a test.
//! - Not sent (the server says nothing reached BIR): Failed, retried on the
//!   return's own schedule; when the reason ends with a TIN refusal code
//!   (`bir_core::tin_refusal::TinRefusal`), back to Draft at once.
//! - Not a try (a hold, class `ours`, signed out, Buwiz unreachable):
//!   waits without a count, never before the server's `wait_until`.
//! - Unknown (BIR may have it): needs attention, never resent
//!   automatically.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bir_core::background_cron::{StoreFailure, StoreReport, SubmissionTransport, TransportError};
use bir_core::bir_sign::SeenSign;
use bir_core::smart_queue::TryClass;

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
                    StoreFailure::NotSent(error)
                    | StoreFailure::MaybeSent(error)
                    | StoreFailure::NotATry { error, .. } => error,
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
                Err(SendError::NotATry { reason, wait_until }) => Err(StoreFailure::NotATry {
                    error: TransportError(reason),
                    wait_until,
                }),
                Err(SendError::MaybeSent(reason)) => {
                    Err(StoreFailure::MaybeSent(TransportError(reason)))
                }
            }
        })
    }

    /// `open_session` is `Submitter::ready`: sign-in, the device key, the
    /// trust list and the sealing key, all from Buwiz. Its failure never
    /// touched the BIR, so it is never a try (a TIN refusal there is read
    /// from its text first, by the queue).
    fn session_failure_class(&self, _error: &TransportError) -> TryClass {
        TryClass::NotATry
    }

    fn sign(&self) -> Option<SeenSign> {
        self.submitter.last_sign()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{Answer, fixture, fixture_json, serve, submitter, trust_documents};
    use serde_json::json;

    const FILE: &str = "00000000000000-1601Cv2018-102026#juana@example.test#.xml";

    /// A held ticket reaches the queue as not a try with the hold's time,
    /// and the transport hands the queue the sign it heard.
    #[tokio::test]
    async fn a_held_ticket_is_not_a_try_for_the_queue() {
        let (public, jws, sealing) = trust_documents(&["127.0.0.1:9"]);
        let header = fixture("api-2.3-compact-sign.txt").trim().to_owned();
        let (url, _) = serve(move |request| match request.path.as_str() {
            "/.well-known/buwiz-trust.json" => Answer {
                status: 200,
                headers: vec![("buwiz-bir", header.clone())],
                body: jws.clone(),
            },
            "/.well-known/buwiz-hpke.json" => Answer {
                status: 200,
                headers: Vec::new(),
                body: sealing.clone(),
            },
            "/api/tickets" => Answer::json(503, &fixture_json("api-5.2-hold-bir-offline.json")),
            _ => Answer::json(
                404,
                &json!({"error": {"code": "not_found", "message": "x"}}),
            ),
        });
        let transport = BuwizTransport::new(Arc::new(submitter(&url, true, vec![public])));
        assert_eq!(transport.sign(), None, "no sign heard yet");
        let failure = transport
            .store_session_reported((), FILE, b"<xml/>")
            .await
            .unwrap_err();
        let StoreFailure::NotATry { error, wait_until } = failure else {
            panic!("{failure:?}")
        };
        assert!(error.0.ends_with("(bir_offline)"));
        assert_eq!(error.refusal(), None);
        assert_eq!(
            wait_until,
            bir_core::smart_queue::parse_time("2026-11-11T09:13:41Z")
        );
        let sign = transport.sign().expect("the header's sign");
        assert_eq!(sign.sign.seq, 4182);
        assert_eq!(sign.sign.wait_until, wait_until);
    }

    /// The full sign object of API.md section 2.2 reads, queue numbers and
    /// all.
    #[test]
    fn the_api_2_2_sign_object_reads_with_its_queue_numbers() {
        let sign =
            bir_core::bir_sign::Sign::from_json(&fixture_json("api-2.2-sign-object.json")).unwrap();
        assert_eq!(sign.seq, 4182);
        assert_eq!(
            sign.queue,
            Some(bir_core::smart_queue::QueuePolicy::default())
        );
    }

    /// Every session failure only talked to Buwiz: not a try.
    #[tokio::test]
    async fn a_session_that_fails_before_any_ticket_is_not_a_try() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let transport = BuwizTransport::new(Arc::new(submitter(&url, false, Vec::new())));
        let error = transport
            .open_session("1601Cv2018", "000000000000")
            .await
            .unwrap_err();
        assert_eq!(transport.session_failure_class(&error), TryClass::NotATry);
    }
}
