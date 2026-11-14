//! Where each return is on its way to BIR, read from the database: the
//! queue worker (`bir_core::background_cron`) records its claim, its
//! failures and the signed receipt on the return, and [`send_stage_for`] turns
//! those fields into what the person sees. [`plain_send_error`] puts the
//! worker's and the relay's reasons in plain words.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{AppError, AppResult, BirApp, TinRefusal, TinStep};

/// A return's place on the way out.
#[derive(Clone, Debug, PartialEq)]
pub enum SendStage {
    /// Waiting for its turn (`next_retry_at`, RFC 3339).
    Queued { next_retry_at: Option<String> },
    /// The worker holds it and is sending it now.
    Sending,
    /// Nothing reached BIR; the queue tries again at `next_retry_at`.
    Failed { reason: String, attempts: u32, next_retry_at: Option<String> },
    /// BIR may have it: never sent again by itself.
    NeedsAttention { reason: String },
    /// Five sends failed: back to Draft with the last reason.
    GaveUp { reason: String },
    /// Buwiz refused it for its TIN (`code`): back to Draft at once with
    /// the stored `reason`. Nothing reached BIR.
    Refused { code: TinRefusal, reason: String },
    /// Sent, with the signed receipt when the relay gave one.
    Sent { at: Option<String>, receipt: Option<ReceiptInfo> },
    Confirmed { at: Option<String> },
    Paid,
}

/// The readable claims of a send receipt (a compact JWS; bir-submit
/// verified its signature when it arrived).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptInfo {
    pub jti: Option<String>,
    /// `live`, `lab` or `dry_run`.
    pub mode: String,
    /// Seconds since the epoch.
    pub iat: Option<i64>,
}

impl ReceiptInfo {
    /// A dry-run receipt: the server only simulated the send.
    pub fn is_test(&self) -> bool {
        self.mode == "dry_run"
    }
}

fn text(data: &Value, key: &str) -> Option<String> {
    data.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The stage of a stored return from its `submission_error` only: it reads
/// no 2551Q error (a 2551Q's is in `last_error`), so it is kept for this
/// module's tests and callers use [`send_stage_for`].
#[cfg(test)]
fn send_stage(status: &str, data: &Value) -> Option<SendStage> {
    stage_with_error(status, data, text(data, "submission_error"))
}

/// The stage of a stored return of `form_code`: `status` is its status
/// column, `data` its JSON. `None` for a plain draft (one that never failed
/// a send). A 2551Q has no `submission_error`: the queue writes its
/// reasons to `last_error`. Other forms' `last_error` is text from builds
/// before the queue and is not read.
pub fn send_stage_for(form_code: &str, status: &str, data: &Value) -> Option<SendStage> {
    let error = text(data, "submission_error")
        .or_else(|| (form_code == "2551Q").then(|| text(data, "last_error")).flatten());
    stage_with_error(status, data, error)
}

/// The queued returns this process's worker is sending now, by id: those
/// whose job key (from the worker's own list of queued rows) is in its
/// in-flight set. When that set cannot be read, every queued return. An
/// empty set answers without reading the database.
pub(crate) fn sending_now(db: &bir_core::db::Database) -> Result<BTreeSet<i64>, bir_core::db::DbError> {
    let jobs = bir_core::background_cron::get_active_jobs();
    let jobs = match jobs.lock() {
        Ok(jobs) if jobs.is_empty() => return Ok(BTreeSet::new()),
        Ok(jobs) => Some(jobs.clone()),
        Err(_) => None,
    };
    let queued = db.list_all_queued_submissions()?;
    let Some(jobs) = jobs else {
        return Ok(queued.into_iter().map(|summary| summary.id).collect());
    };
    Ok(queued.into_iter().filter(|summary| jobs.contains(&bir_core::background_cron::queue_job_key(summary))).map(|summary| summary.id).collect())
}

/// The stage to show. A claim writes its "outcome pending" note as it is
/// taken, so a claimed return reads `NeedsAttention` from that instant;
/// while this process is still sending it (`sending_now`) it is `Sending`.
/// [`send_stage_for`] stays a pure reading of the row; only callers that know
/// this process's worker apply this.
pub(crate) fn shown_stage(stage: SendStage, sending_now: bool) -> SendStage {
    match stage {
        SendStage::NeedsAttention { .. } if sending_now => SendStage::Sending,
        stage => stage,
    }
}

fn stage_with_error(status: &str, data: &Value, error: Option<String>) -> Option<SendStage> {
    let claimed = text(data, "submission_claim_token").is_some();
    let attempts = data.get("submission_attempts").and_then(Value::as_u64).unwrap_or(0) as u32;
    let next_retry_at = text(data, "next_retry_at");
    match status {
        "Queued" if claimed => Some(match error {
            Some(reason) => SendStage::NeedsAttention { reason },
            None => SendStage::Sending,
        }),
        "Queued" => Some(match error {
            Some(reason) => SendStage::Failed { reason, attempts, next_retry_at },
            None => SendStage::Queued { next_retry_at },
        }),
        "Submitted" | "Sent" => Some(SendStage::Sent {
            at: text(data, "submitted_at"),
            receipt: text(data, "submission_receipt").and_then(|jws| decode_receipt(&jws)),
        }),
        "Confirmed" => Some(SendStage::Confirmed { at: text(data, "confirmed_at") }),
        "Paid" => Some(SendStage::Paid),
        "Draft" if attempts > 0 => error.map(|reason| match TinRefusal::from_reason(&reason) {
            Some(code) => SendStage::Refused { code, reason },
            None => SendStage::GaveUp { reason },
        }),
        _ => None,
    }
}

fn base64url(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in input.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The claims of a compact JWS receipt, unverified (display only).
pub fn decode_receipt(jws: &str) -> Option<ReceiptInfo> {
    let payload = jws.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&base64url(payload)?).ok()?;
    Some(ReceiptInfo {
        jti: text(&claims, "jti"),
        mode: text(&claims, "mode").unwrap_or_else(|| "live".to_owned()),
        iat: claims.get("iat").and_then(Value::as_i64),
    })
}

/// What a send error asks the person to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorAction {
    None,
    SignIn,
    /// Refused for the TIN's state on the Buwiz account: nothing is retried
    /// by itself; the step, when there is one, is the thing to do.
    Tin(Option<TinStep>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlainError {
    pub text: String,
    pub action: ErrorAction,
}

/// The worker's reasons carry where they came from ("Not sent (nothing
/// reached BIR): …"); the person gets one plain sentence and, when there is
/// one, the thing to do.
pub fn plain_send_error(raw: &str) -> PlainError {
    // A refusal for the TIN is read by its code alone, never its English.
    if let Some(code) = TinRefusal::from_reason(raw) {
        let statement = BirApp::tin_statement_for_refusal(code, Some(raw));
        return PlainError { text: statement.line, action: ErrorAction::Tin(statement.step) };
    }
    let lower = raw.to_lowercase();
    if lower.contains("sign in to send") || lower.contains("signed_out") || lower.contains("not signed in") {
        return PlainError { text: "Sign in to send this return.".into(), action: ErrorAction::SignIn };
    }
    if lower.contains("could not reach the buwiz server") || lower.contains("connection refused") || lower.contains("dns error") {
        return PlainError { text: "Couldn't reach the relay. It will try again.".into(), action: ErrorAction::None };
    }
    let mut rest = raw.trim();
    for prefix in [
        "Needs attention: ",
        "Not sent (nothing reached BIR): ",
        "Submission session failed before upload (no PUT attempted): ",
    ] {
        if let Some(r) = rest.strip_prefix(prefix) {
            rest = r.trim();
        }
    }
    // A trailing server code (`message (code)`) is not for people.
    if let Some(open) = rest.rfind(" (")
        && rest.ends_with(')')
        && rest[open + 2..rest.len() - 1].chars().all(|c| c.is_ascii_lowercase() || c == '_')
    {
        rest = &rest[..open];
    }
    let mut chars = rest.chars();
    let mut text: String = match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => "The return was not sent.".to_owned(),
    };
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    PlainError { text, action: ErrorAction::None }
}

fn call_error(error: impl std::fmt::Display) -> AppError {
    AppError::Call(error.to_string())
}

impl BirApp {
    /// The stage of every return that has one, by return id.
    pub fn send_states(&self) -> AppResult<BTreeMap<i64, SendStage>> {
        let rows: Vec<(i64, String)> = self.list_returns(crate::ReturnFilter::All)?.into_iter().map(|r| (r.id, r.form_code)).collect();
        let db = self.db.lock().map_err(|_| AppError::Call("the database lock is poisoned".into()))?;
        let sending = sending_now(&db)?;
        let mut out = BTreeMap::new();
        for (id, form_code) in rows {
            if let Some((status, json)) = db.draft_status_and_json(id)?
                && let Ok(data) = serde_json::from_str::<Value>(&json)
                && let Some(stage) = send_stage_for(&form_code, &status, &data)
            {
                out.insert(id, shown_stage(stage, sending.contains(&id)));
            }
        }
        Ok(out)
    }

    /// Edit the lifecycle fields of a stored return with `change` (the
    /// status column follows the JSON's `status`); refused when the queue
    /// worker changed the row meanwhile.
    fn change_lifecycle(&self, id: i64, change: impl FnOnce(&str, &mut serde_json::Map<String, Value>) -> AppResult<()>) -> AppResult<()> {
        let db = self.db.lock().map_err(|_| AppError::Call("the database lock is poisoned".into()))?;
        let (status, json) = db.draft_status_and_json(id)?.ok_or_else(|| AppError::Call("That return is gone.".into()))?;
        let mut data: Value = serde_json::from_str(&json).map_err(call_error)?;
        let map = data.as_object_mut().ok_or_else(|| AppError::Call("The stored return is not an object.".into()))?;
        change(&status, map)?;
        let new_status = map.get("status").and_then(Value::as_str).unwrap_or(&status).to_owned();
        let new_json = serde_json::to_string(&data).map_err(call_error)?;
        if db.replace_draft_json_if_unchanged(id, &json, &new_status, &new_json)? {
            Ok(())
        } else {
            Err(AppError::Call("The queue is working on that return; try again in a moment.".into()))
        }
    }

    /// Retry a failed send now instead of after its backoff (then wake the
    /// queue with `Submission::send_now`). Only an unclaimed Queued return:
    /// one that needs attention is never resent by itself.
    pub fn retry_now(&self, id: i64) -> AppResult<()> {
        self.change_lifecycle(id, |status, map| {
            if status != "Queued" {
                return Err(AppError::Call("Only a queued return can be sent again.".into()));
            }
            if map.get("submission_claim_token").is_some_and(|v| !v.is_null()) {
                return Err(AppError::Call("BIR may already have this return. Check for BIR's confirmation email before sending it again.".into()));
            }
            map.insert("next_retry_at".into(), Value::String(chrono::Utc::now().to_rfc3339()));
            Ok(())
        })
    }

    /// Demo and test data only: put the queued return `id` in `stage`
    /// (Sending, Failed with `reason`, Needs attention, Sent with a demo
    /// receipt, or Refused: back to Draft with `reason`, which ends with the
    /// code), as the queue worker would leave it. Screenshots and checks use
    /// it where no relay runs.
    #[doc(hidden)]
    pub fn demo_set_send_stage(&self, id: i64, stage: &SendStage) -> AppResult<()> {
        let stage = stage.clone();
        self.change_lifecycle(id, move |status, map| {
            if status != "Queued" {
                return Err(AppError::Call("Only a queued return takes a demo send state.".into()));
            }
            let now = chrono::Utc::now().to_rfc3339();
            let attempts = map.get("submission_attempts").and_then(Value::as_u64).unwrap_or(0);
            let mut set = |k: &str, v: Value| {
                map.insert(k.into(), v);
            };
            match stage {
                SendStage::Sending => {
                    set("submission_claim_token", Value::String("demo-claim".into()));
                    set("submission_claimed_at", Value::String(now.clone()));
                    set("submission_put_started_at", Value::String(now));
                    set("submission_error", Value::Null);
                }
                SendStage::Failed { reason, attempts, next_retry_at } => {
                    set("submission_error", Value::String(reason));
                    set("submission_attempts", Value::from(attempts.max(1)));
                    set("next_retry_at", next_retry_at.map_or(Value::Null, Value::String));
                }
                SendStage::NeedsAttention { reason } => {
                    set("submission_claim_token", Value::String("demo-claim".into()));
                    set("submission_claimed_at", Value::String(now.clone()));
                    set("submission_put_started_at", Value::String(now));
                    set("submission_error", Value::String(reason));
                }
                SendStage::Sent { .. } => {
                    let claims = serde_json::json!({ "jti": "demo-7f3a91c2", "mode": "dry_run", "status": "dry_run", "iat": chrono::Utc::now().timestamp() });
                    let enc = |v: &[u8]| base64url_encode(v);
                    let jws = format!("{}.{}.{}", enc(br#"{"alg":"EdDSA"}"#), enc(claims.to_string().as_bytes()), enc(b"demo"));
                    set("status", Value::String("Submitted".into()));
                    set("submitted_at", Value::String(now));
                    set("submission_receipt", Value::String(jws));
                    set("submission_error", Value::Null);
                    set("next_retry_at", Value::Null);
                }
                SendStage::Refused { reason, .. } => {
                    set("status", Value::String("Draft".into()));
                    set("submission_error", Value::String(reason));
                    set("submission_attempts", Value::from(attempts.max(1)));
                    for key in [
                        "submission_claim_token",
                        "submission_claimed_at",
                        "submission_claim_lease_until",
                        "submission_put_started_at",
                        "next_retry_at",
                        "queued_submission_fingerprint",
                        "queue_authorization",
                        "submitted_payload",
                    ] {
                        set(key, Value::Null);
                    }
                }
                _ => return Err(AppError::Call("That stage has no demo state.".into())),
            }
            Ok(())
        })
    }
}

fn base64url_encode(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        let take = chunk.len() + 1;
        for i in 0..take {
            out.push(ABC[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queued_claimed_and_failed_returns_read_as_their_stage() {
        assert_eq!(send_stage("Queued", &json!({})), Some(SendStage::Queued { next_retry_at: None }));
        assert_eq!(send_stage("Queued", &json!({"submission_claim_token": "t"})), Some(SendStage::Sending));
        assert_eq!(
            send_stage("Queued", &json!({"submission_claim_token": "t", "submission_error": "Needs attention: dropped"})),
            Some(SendStage::NeedsAttention { reason: "Needs attention: dropped".into() })
        );
        assert_eq!(
            send_stage("Queued", &json!({"submission_error": "Not sent (nothing reached BIR): x", "submission_attempts": 2, "next_retry_at": "later"})),
            Some(SendStage::Failed { reason: "Not sent (nothing reached BIR): x".into(), attempts: 2, next_retry_at: Some("later".into()) })
        );
        assert_eq!(send_stage("Draft", &json!({})), None);
        assert_eq!(
            send_stage("Draft", &json!({"submission_error": "gave up", "submission_attempts": 5})),
            Some(SendStage::GaveUp { reason: "gave up".into() })
        );
    }

    #[test]
    fn a_2551q_reads_its_send_error_from_last_error() {
        let claimed = json!({"submission_claim_token": "t", "last_error": "Submission outcome pending."});
        assert_eq!(send_stage_for("2551Q", "Queued", &claimed), Some(SendStage::NeedsAttention { reason: "Submission outcome pending.".into() }));
        let not_sent = json!({"last_error": "Not sent (nothing reached BIR): x", "submission_attempts": 1, "next_retry_at": "later"});
        assert_eq!(
            send_stage_for("2551Q", "Queued", &not_sent),
            Some(SendStage::Failed { reason: "Not sent (nothing reached BIR): x".into(), attempts: 1, next_retry_at: Some("later".into()) })
        );
        let gave_up = json!({"last_error": "Not sent (nothing reached BIR): x", "submission_attempts": 5});
        assert_eq!(send_stage_for("2551Q", "Draft", &gave_up), Some(SendStage::GaveUp { reason: "Not sent (nothing reached BIR): x".into() }));
        // `submission_error` still wins where a row has both.
        let both = json!({"submission_error": "a", "last_error": "b", "submission_attempts": 1});
        assert_eq!(send_stage_for("2551Q", "Queued", &both), Some(SendStage::Failed { reason: "a".into(), attempts: 1, next_retry_at: None }));
    }

    #[test]
    fn another_forms_old_last_error_is_not_a_send_error() {
        let legacy = json!({"last_error": "old", "submission_attempts": 2});
        assert_eq!(send_stage_for("0619E", "Queued", &legacy), Some(SendStage::Queued { next_retry_at: None }));
        assert_eq!(send_stage_for("1601C", "Draft", &legacy), None);
        assert_eq!(send_stage("Queued", &legacy), Some(SendStage::Queued { next_retry_at: None }));
    }

    #[test]
    fn a_sent_return_reads_its_receipt_claims() {
        let claims = br#"{"jti":"abc123","mode":"live","iat":1790000000}"#;
        let jws = format!("e30.{}.c2ln", base64url_encode(claims));
        let stage = send_stage("Submitted", &json!({"submitted_at": "2026-10-30T01:00:00Z", "submission_receipt": jws}));
        assert_eq!(
            stage,
            Some(SendStage::Sent {
                at: Some("2026-10-30T01:00:00Z".into()),
                receipt: Some(ReceiptInfo { jti: Some("abc123".into()), mode: "live".into(), iat: Some(1_790_000_000) })
            })
        );
        assert_eq!(decode_receipt("not-a-jws"), None);
    }

    #[test]
    fn base64url_round_trips() {
        for s in ["", "a", "ab", "abc", "abcd", "{\"jti\":\"x\"}"] {
            assert_eq!(base64url(&base64url_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
    }

    #[test]
    fn send_errors_read_in_plain_words() {
        let tin = plain_send_error("Not sent (nothing reached BIR): TIN 000123453 is not active on this account (tin_not_active)");
        assert_eq!(tin.action, ErrorAction::Tin(Some(TinStep::Add)));
        assert_eq!(tin.text, "This TIN is not on your Buwiz account.");
        let signed_out = plain_send_error("Submission session failed before upload (no PUT attempted): Sign in to send returns.");
        assert_eq!(signed_out.action, ErrorAction::SignIn);
        let net = plain_send_error("Submission session failed before upload (no PUT attempted): could not reach the Buwiz server: connection refused");
        assert_eq!(net.text, "Couldn't reach the relay. It will try again.");
        let other = plain_send_error("Not sent (nothing reached BIR): the relay closed early (relay_closed)");
        assert_eq!(other, PlainError { text: "The relay closed early.".into(), action: ErrorAction::None });
    }

    #[test]
    fn plain_send_error_reads_codes_not_english() {
        let rows = [
            ("tin_not_active", "This TIN is not on your Buwiz account.", Some(TinStep::Add)),
            ("tin_unverified", "This TIN is on your Buwiz account but not verified yet.", Some(TinStep::Verify)),
            ("tin_pending", "Buwiz is reviewing this TIN. You can send it once it is approved.", None),
            ("tin_needs_fix", "Buwiz sent this TIN back for a fix.", Some(TinStep::Fix)),
            ("tin_paused", "Buwiz staff paused this TIN. It can't send until they turn it back on.", None),
            ("account_banned", "This Buwiz account is banned, so nothing can be sent.", None),
        ];
        for (code, line, step) in rows {
            for raw in [
                format!("Not sent (nothing reached BIR): words nobody matches on ({code})"),
                format!("Submission session failed before upload (no PUT attempted): sign in to send, could not reach the buwiz server ({code})"),
            ] {
                assert_eq!(plain_send_error(&raw), PlainError { text: line.into(), action: ErrorAction::Tin(step) }, "{raw}");
            }
        }
        // English alone is no longer read as a TIN state.
        let text = plain_send_error("Not sent (nothing reached BIR): tin_not_active somewhere in the text");
        assert_eq!(text.action, ErrorAction::None);
    }

    #[test]
    fn a_draft_with_attempts_and_a_tin_code_reads_refused() {
        let reason = "Not sent (nothing reached BIR): This TIN is not verified yet. (tin_unverified)";
        assert_eq!(
            send_stage("Draft", &json!({"submission_error": reason, "submission_attempts": 1})),
            Some(SendStage::Refused { code: TinRefusal::Unverified, reason: reason.into() })
        );
        let paused = "Not sent (nothing reached BIR): Sending is paused for this TIN. (tin_paused)";
        assert_eq!(
            send_stage_for("2551Q", "Draft", &json!({"last_error": paused, "submission_attempts": 1})),
            Some(SendStage::Refused { code: TinRefusal::Paused, reason: paused.into() })
        );
        // Queued and unclaimed with a code: a failure the next tick retries.
        assert!(matches!(
            send_stage("Queued", &json!({"submission_error": reason, "submission_attempts": 1})),
            Some(SendStage::Failed { .. })
        ));
        // Claimed: BIR may have it, whatever the text says.
        assert!(matches!(
            send_stage("Queued", &json!({"submission_claim_token": "t", "submission_error": reason})),
            Some(SendStage::NeedsAttention { .. })
        ));
    }

    #[test]
    fn a_legacy_five_try_draft_with_tin_not_active_reads_refused() {
        let reason = "Not sent (nothing reached BIR): this TIN is not active on your account (tin_not_active)";
        assert_eq!(
            send_stage("Draft", &json!({"submission_error": reason, "submission_attempts": 5})),
            Some(SendStage::Refused { code: TinRefusal::NotActive, reason: reason.into() })
        );
    }

    #[test]
    fn a_draft_without_a_code_still_reads_gave_up() {
        let reason = "Not sent (nothing reached BIR): the relay closed early (relay_closed)";
        assert_eq!(
            send_stage("Draft", &json!({"submission_error": reason, "submission_attempts": 5})),
            Some(SendStage::GaveUp { reason: reason.into() })
        );
    }
}
