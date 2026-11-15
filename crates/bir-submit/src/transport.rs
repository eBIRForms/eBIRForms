//! The queue's way out: [`SubmissionTransport`] over [`Submitter`].
//!
//! - Queued → Sending: `open_session` checks the sign-in, the device key
//!   and the server's keys. A failure there leaves the return Queued; it
//!   talks to Buwiz only, so it is not a try (`TryClass::NotATry`).
//!   One the server ANSWERED with a refusal no retry changes by itself (a
//!   4xx that is not a hold, a 429, a timeout, a lapsed token, being signed
//!   out or a TIN refusal) is [`TryClass::Answered`]: still not a try, but
//!   the same one three times in a row stops the return (Needs you).
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
use std::sync::{Arc, Mutex};

use bir_core::background_cron::{StoreFailure, StoreReport, SubmissionTransport, TransportError};
use bir_core::bir_sign::SeenSign;
use bir_core::smart_queue::TryClass;

use crate::client::{ClientError, SendError, Submitter};

pub struct BuwizTransport {
    submitter: Arc<Submitter>,
    /// The text of the last `open_session` failure the server answered with
    /// a refusal ([`answered_refusal`]). `session_failure_class` sees only
    /// the text, so it finds out by this.
    answered: Mutex<Option<String>>,
}

impl BuwizTransport {
    pub fn new(submitter: Arc<Submitter>) -> Self {
        Self { submitter, answered: Mutex::new(None) }
    }

    fn remember(&self, text: Option<String>) {
        *self.answered.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = text;
    }
}

/// Whether the server answered with a refusal no retry changes by itself: a
/// 4xx that is not a hold (those are [`ClientError::NotATry`]), a 429, a
/// timeout (408), a lapsed token, or a TIN refusal (the queue reads those
/// first). Being signed out and not reaching Buwiz are other errors.
fn answered_refusal(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Server { status, code, .. }
            if (400..500).contains(status)
                && !matches!(*status, 408 | 429)
                && code != "invalid_token"
    ) && error.tin_refusal().is_none()
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
            self.remember(None);
            self.submitter.ready().await.map_err(|error| {
                let text = error.to_string();
                if answered_refusal(&error) {
                    self.remember(Some(text.clone()));
                }
                TransportError(text)
            })
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
                    | StoreFailure::NotATry { error, .. }
                    | StoreFailure::Counted { error, .. } => error,
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
                Err(SendError::Counted { reason, blame }) => Err(StoreFailure::Counted {
                    error: TransportError(reason),
                    blame,
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
    /// from its text first, by the queue). One the server answered with a
    /// refusal is [`TryClass::Answered`]: not a try either, but three in a
    /// row stop the return.
    fn session_failure_class(&self, error: &TransportError) -> TryClass {
        let answered = self.answered.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if answered.as_deref() == Some(error.0.as_str()) {
            TryClass::Answered
        } else {
            TryClass::NotATry
        }
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
        let header = fixture("contract-compact-sign-header.txt").trim().to_owned();
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
            "/api/tickets" => Answer::json(503, &fixture_json("contract-hold-bir-offline.json")),
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

    /// The full sign object of the public contract reads, queue numbers and
    /// all.
    #[test]
    fn the_contract_sign_object_reads_with_its_queue_numbers() {
        let sign =
            bir_core::bir_sign::Sign::from_json(&fixture_json("contract-sign-object.json")).unwrap();
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

    /// A 4xx the server answered on the way in is `Answered`; a hold, a 429,
    /// a timeout, a 5xx, being signed out and an unreachable server stay
    /// plain not-a-try.
    #[tokio::test]
    async fn only_a_server_answered_refusal_is_an_answered_session() {
        let (public, jws, _) = trust_documents(&["127.0.0.1:9"]);
        for (status, body, retry_after, want) in [
            (403, json!({"error": {"code": "device_unknown", "message": "This device is not allowed."}}), None, TryClass::Answered),
            (404, json!({"error": {"code": "not_found", "message": "x"}}), None, TryClass::Answered),
            (503, json!({"error": {"code": "bir_offline", "message": "x"}}), Some("60"), TryClass::NotATry),
            (429, json!({"error": {"code": "rate_limited", "message": "x"}}), Some("60"), TryClass::NotATry),
            (429, json!({"error": {"code": "slow_down", "message": "x"}}), None, TryClass::NotATry),
            (408, json!({"error": {"code": "timeout", "message": "x"}}), None, TryClass::NotATry),
            (401, json!({"error": {"code": "invalid_token", "message": "x"}}), None, TryClass::NotATry),
            (500, json!({"error": {"code": "internal", "class": "bir", "message": "x"}}), None, TryClass::NotATry),
            (403, json!({"error": {"code": "tin_not_active", "message": "x"}}), None, TryClass::NotATry),
        ] {
            let (jws, answer_body) = (jws.clone(), body.clone());
            let (url, _) = serve(move |request| match request.path.as_str() {
                "/.well-known/buwiz-trust.json" => Answer { status: 200, headers: Vec::new(), body: jws.clone() },
                "/.well-known/buwiz-hpke.json" => {
                    let answer = Answer::json(status, &answer_body);
                    match retry_after {
                        Some(seconds) => answer.with("retry-after", seconds),
                        None => answer,
                    }
                }
                _ => Answer::json(404, &json!({})),
            });
            let transport = BuwizTransport::new(Arc::new(submitter(&url, true, vec![public.clone()])));
            let error = transport.open_session("1601Cv2018", "000000000000").await.unwrap_err();
            assert_eq!(transport.session_failure_class(&error), want, "{status} {body}: {error}");
        }
        // Signed out.
        let (url, _) = serve(|_| Answer::json(404, &json!({})));
        let transport = BuwizTransport::new(Arc::new(submitter(&url, false, vec![public.clone()])));
        let error = transport.open_session("1601Cv2018", "000000000000").await.unwrap_err();
        assert_eq!(transport.session_failure_class(&error), TryClass::NotATry);
        // A text the transport did not see answered is never one.
        assert_eq!(transport.session_failure_class(&TransportError("x (device_unknown)".into())), TryClass::NotATry);
    }
}
