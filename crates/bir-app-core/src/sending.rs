//! Where each return is on its way to BIR, read from the database: the
//! queue worker (`bir_core::background_cron`) records its claim, its
//! failures and the signed receipt on the return, and [`send_stage_for`] turns
//! those fields into what the person sees. [`plain_send_error`] puts the
//! worker's and the relay's reasons in plain words, and [`queue_line`] the
//! smart queue's states (the public contract, `docs/bir-uptime.md` in
//! `buwiz-com/buwiz-com`).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

pub use bir_core::smart_queue::QueueMode;

use crate::{AppError, AppResult, BirApp, TinRefusal, TinStep};

/// A return's place on the way out. [`SendStage::name`] is the stable word
/// for each.
#[derive(Clone, Debug, PartialEq)]
pub enum SendStage {
    /// Waiting for its turn (`next_retry_at`, RFC 3339).
    Queued { next_retry_at: Option<String> },
    /// Its own time has come, but one return goes at a time: another
    /// queued return is tried before it, and it goes right after
    /// (`next_retry_at`, the stored time it waits for). Only when another
    /// really goes first: a return that waits alone for the queue-wide
    /// back-off reads `Failed` or `Queued` with its time.
    Behind { reason: Option<String>, next_retry_at: Option<String> },
    /// Returns get in for others, but this device's last tries all failed:
    /// it waits at least 2 hours and the problem may be the person's own
    /// connection. Still queued; `reason` is the last failure.
    SlowedDown { reason: String, next_retry_at: Option<String> },
    /// Still not sent a day (`notice_after_s`) after it was queued
    /// (`since`). Still queued and still trying by itself.
    StillWaiting { since: String, reason: Option<String>, next_retry_at: Option<String> },
    /// It keeps being refused for the same reason about the return itself
    /// (a refused login, no upload service for its TIN, the same
    /// validation refusal), so its automatic tries stopped. Still queued,
    /// and nothing else is affected; the person's Retry tries it again.
    NeedsYou { reason: String },
    /// The person switched the queue to Paused: nothing goes until Auto.
    Paused,
    /// The worker holds it and is sending it now.
    Sending,
    /// Nothing reached BIR; the queue tries again at `next_retry_at`.
    Failed { reason: String, attempts: u32, next_retry_at: Option<String> },
    /// BIR may have it: never sent again by itself.
    NeedsAttention { reason: String },
    /// Back to Draft with the last reason after five failed sends, under the
    /// rules before the smart queue. No return goes back to Draft for
    /// failed tries any more; rows left so before keep this stage.
    GaveUp { reason: String },
    /// Buwiz refused it for its TIN (`code`): back to Draft at once with
    /// the stored `reason`. Nothing reached BIR.
    Refused { code: TinRefusal, reason: String },
    /// Sent, with the signed receipt when the relay gave one.
    Sent { at: Option<String>, receipt: Option<ReceiptInfo> },
    Confirmed { at: Option<String> },
    Paid,
}

impl SendStage {
    /// The stable word for the stage, for the mini app and tools:
    /// `queued`, `behind`, `slowed_down`, `still_waiting`, `needs_you`, `paused`,
    /// `sending`, `failed`, `needs_attention`, `gave_up`, `refused`, `sent`,
    /// `confirmed`, `paid`.
    pub fn name(&self) -> &'static str {
        match self {
            SendStage::Queued { .. } => "queued",
            SendStage::Behind { .. } => "behind",
            SendStage::NeedsYou { .. } => "needs_you",
            SendStage::SlowedDown { .. } => "slowed_down",
            SendStage::StillWaiting { .. } => "still_waiting",
            SendStage::Paused => "paused",
            SendStage::Sending => "sending",
            SendStage::Failed { .. } => "failed",
            SendStage::NeedsAttention { .. } => "needs_attention",
            SendStage::GaveUp { .. } => "gave_up",
            SendStage::Refused { .. } => "refused",
            SendStage::Sent { .. } => "sent",
            SendStage::Confirmed { .. } => "confirmed",
            SendStage::Paid => "paid",
        }
    }

    /// Still queued and unclaimed: the queue will try it again by itself.
    pub fn is_waiting(&self) -> bool {
        matches!(
            self,
            SendStage::Queued { .. }
                | SendStage::Behind { .. }
                | SendStage::SlowedDown { .. }
                | SendStage::StillWaiting { .. }
                | SendStage::Paused
                | SendStage::Failed { .. }
        )
    }
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

/// Paused stops everything that is still waiting by itself: such a stage
/// reads [`SendStage::Paused`] while the queue is paused. A return that
/// needs the person, or may already be at the BIR, keeps its own.
pub(crate) fn paused_stage(stage: SendStage, paused: bool) -> SendStage {
    if paused && stage.is_waiting() { SendStage::Paused } else { stage }
}

/// Where a stage's stored next try is, for the stages that wait for one.
fn stored_time(stage: &SendStage) -> Option<chrono::DateTime<chrono::Utc>> {
    let at = match stage {
        SendStage::Queued { next_retry_at }
        | SendStage::Behind { next_retry_at, .. }
        | SendStage::SlowedDown { next_retry_at, .. }
        | SendStage::StillWaiting { next_retry_at, .. }
        | SendStage::Failed { next_retry_at, .. } => next_retry_at.as_deref()?,
        _ => return None,
    };
    bir_core::smart_queue::parse_time(at)
}

/// A row's stage reads `Behind` when its stored time is later than its own,
/// which also happens when it only waits for the queue-wide back-off. This
/// keeps `Behind` for a return that has another one ahead of it (one being
/// sent, or waiting with an earlier stored time, ties by id) and lets the
/// rest read as what they are: `Failed` with the reason, or `Queued`, each
/// with its time. `rows` are each return's id, stage and stored attempts.
pub(crate) fn settle_behind(rows: Vec<(i64, SendStage, u32)>) -> BTreeMap<i64, SendStage> {
    let goes_first = |id: i64, at: Option<chrono::DateTime<chrono::Utc>>| {
        rows.iter().any(|(other, stage, _)| {
            *other != id
                && match stage {
                    SendStage::Sending => true,
                    SendStage::Queued { .. }
                    | SendStage::Behind { .. }
                    | SendStage::SlowedDown { .. }
                    | SendStage::StillWaiting { .. }
                    | SendStage::Failed { .. } => match (stored_time(stage), at) {
                        (Some(theirs), Some(ours)) => (theirs, *other) < (ours, id),
                        _ => false,
                    },
                    _ => false,
                }
        })
    };
    let ahead: BTreeSet<i64> = rows
        .iter()
        .filter(|(id, stage, _)| matches!(stage, SendStage::Behind { .. }) && goes_first(*id, stored_time(stage)))
        .map(|(id, _, _)| *id)
        .collect();
    rows.into_iter()
        .map(|(id, stage, attempts)| {
            let stage = match stage {
                SendStage::Behind { reason, next_retry_at } if !ahead.contains(&id) => match reason {
                    Some(reason) => SendStage::Failed { reason, attempts, next_retry_at },
                    None => SendStage::Queued { next_retry_at },
                },
                stage => stage,
            };
            (id, stage)
        })
        .collect()
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
    let queue: bir_core::smart_queue::QueueState =
        data.get("smart_queue").and_then(|value| serde_json::from_value(value.clone()).ok()).unwrap_or_default();
    match status {
        "Queued" if claimed => Some(match error {
            Some(reason) => SendStage::NeedsAttention { reason },
            None => SendStage::Sending,
        }),
        // The queue raised its "still waiting" notice for this queueing.
        // Stopped: three refusals of the same kind about the return itself.
        "Queued" if queue.is_stuck() => Some(SendStage::NeedsYou {
            reason: error.or_else(|| queue.stuck.clone()).unwrap_or_default(),
        }),
        "Queued" if queue.notices > 0 => Some(SendStage::StillWaiting {
            since: data
                .get("queue_authorization")
                .and_then(|auth| text(auth, "authorized_at"))
                .or_else(|| text(data, "created_at"))
                .unwrap_or_default(),
            reason: error,
            next_retry_at,
        }),
        "Queued" if queue.slowed && error.is_some() => {
            Some(SendStage::SlowedDown { reason: error.unwrap_or_default(), next_retry_at })
        }
        // The stored time is later than its own: it waits behind another.
        "Queued"
            if queue.next_try().is_some_and(|own| {
                next_retry_at.as_deref().and_then(bir_core::smart_queue::parse_time).is_some_and(|stored| stored > own)
            }) =>
        {
            Some(SendStage::Behind { reason: error, next_retry_at })
        }
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

/// "about 6 min", "about 30 min", "about 2 h": the wait until `at`, for
/// people. `None` when `at` is not a time; "a moment" when it has passed.
pub fn about(at: &str, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let at = bir_core::smart_queue::parse_time(at)?;
    let minutes = ((at - now).num_seconds() + 59) / 60;
    Some(match minutes {
        ..=0 => "a moment".to_owned(),
        1..=9 => format!("about {minutes} min"),
        10..=59 => format!("about {} min", ((minutes + 2) / 5 * 5).max(10)),
        _ => {
            let hours = (minutes + 30) / 60;
            format!("about {hours} h")
        }
    })
}

fn tries_again(next_retry_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> String {
    match next_retry_at.and_then(|at| about(at, now)) {
        Some(wait) if wait == "a moment" => "Tries again in a moment.".to_owned(),
        Some(wait) => format!("Tries again in {wait}."),
        None => "It tries again by itself.".to_owned(),
    }
}

/// The queue's words for a stage that is still waiting (the public contract,
/// `docs/bir-uptime.md` in `buwiz-com/buwiz-com`), with the time left until its next try; `None` for every other
/// stage, whose words the screens already have.
pub fn queue_line(stage: &SendStage, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    Some(match stage {
        SendStage::Behind { reason: Some(reason), .. } => {
            format!("{} Goes right after the one above.", plain_send_error(reason).text)
        }
        SendStage::Behind { .. } => "Goes right after the one above.".to_owned(),
        SendStage::NeedsYou { reason } => format!(
            "This return keeps being refused: {} It stopped trying by itself; fix the cause, then press Retry.",
            plain_send_error(reason).text
        ),
        SendStage::SlowedDown { next_retry_at, .. } => format!(
            "Returns are getting in for others. The problem may be your own connection. {}",
            tries_again(next_retry_at.as_deref(), now)
        ),
        SendStage::StillWaiting { since, .. } => {
            let hours = bir_core::smart_queue::parse_time(since)
                .map(|since| ((now - since).num_hours() / 24).max(1) * 24)
                .unwrap_or(24);
            format!("Still waiting after {hours} hours. It keeps trying by itself; you can cancel it or file another way.")
        }
        SendStage::Paused => "Your queue is paused · press Auto to send".to_owned(),
        // Waiting a while for the queue-wide back-off, with nothing to say
        // but when: a time a minute away or less is just "waiting to send".
        SendStage::Queued { next_retry_at: Some(at) }
            if bir_core::smart_queue::parse_time(at).is_some_and(|at| at > now + chrono::Duration::seconds(60)) =>
        {
            tries_again(Some(at.as_str()), now)
        }
        SendStage::Failed { reason, next_retry_at, .. } => {
            let plain = plain_send_error(reason);
            match plain.action {
                ErrorAction::None => format!("{} {}", plain.text, tries_again(next_retry_at.as_deref(), now)),
                _ => plain.text,
            }
        }
        _ => return None,
    })
}

/// What a person's Retry did ([`BirApp::retry`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryAnswer {
    /// It goes as soon as the line lets it: after the queue-wide back-off
    /// (`not_before`) when the way in to the BIR is waiting, else at once.
    Going { not_before: Option<chrono::DateTime<chrono::Utc>> },
    /// Too soon: Retry on this return may be pressed again at this time.
    NotBefore(chrono::DateTime<chrono::Utc>),
}

/// "You can press Retry again in about 20 min", for a refused press.
pub fn retry_wait_words(at: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> String {
    match about(&at.to_rfc3339(), now) {
        Some(wait) if wait != "a moment" => format!("Retry was pressed a while ago. You can press it again in {wait}."),
        _ => "Retry was pressed a moment ago. You can press it again shortly.".to_owned(),
    }
}

/// What to tell the person after [`RetryAnswer::Going`].
pub fn retry_going_words(not_before: Option<chrono::DateTime<chrono::Utc>>, now: chrono::DateTime<chrono::Utc>) -> String {
    match not_before.and_then(|at| about(&at.to_rfc3339(), now)) {
        Some(wait) if wait != "a moment" => {
            format!("Noted. The BIR could not be reached a moment ago, so every return waits; this one goes in {wait}.")
        }
        _ => "Sending again now.".to_owned(),
    }
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
        let paused = QueueMode::from_setting(db.get_setting(bir_core::smart_queue::QUEUE_MODE_SETTING)?.as_deref()) == QueueMode::Paused;
        let mut stages = Vec::new();
        for (id, form_code) in rows {
            if let Some((status, json)) = db.draft_status_and_json(id)?
                && let Ok(data) = serde_json::from_str::<Value>(&json)
                && let Some(stage) = send_stage_for(&form_code, &status, &data)
            {
                let attempts = data.get("submission_attempts").and_then(Value::as_u64).unwrap_or(0) as u32;
                stages.push((id, shown_stage(stage, sending.contains(&id)), attempts));
            }
        }
        // Behind only where another return really goes first.
        let mut out = settle_behind(stages);
        for stage in out.values_mut() {
            *stage = paused_stage(std::mem::replace(stage, SendStage::Paused), paused);
        }
        Ok(out)
    }

    /// The person's switch: the queue sends by itself (Auto) or not at all
    /// (Paused). Separate from the pause while signed out.
    pub fn queue_mode(&self) -> AppResult<QueueMode> {
        Ok(QueueMode::from_setting(self.get_setting(bir_core::smart_queue::QUEUE_MODE_SETTING)?.as_deref()))
    }

    /// Switch the queue to `mode`. Auto wakes it, so a waiting return goes
    /// when its own time has come.
    pub fn set_queue_mode(&self, mode: QueueMode) -> AppResult<()> {
        self.set_setting(bir_core::smart_queue::QUEUE_MODE_SETTING, mode.as_setting())?;
        if mode == QueueMode::Auto {
            if let Ok(db) = self.db.lock() {
                let _ = db.resolve_alert(None, bir_core::db::alert_kinds::QUEUE_PAUSED);
            }
            bir_core::background_cron::wake_quietly();
        }
        Ok(())
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

    /// A person's Retry on one return, at `now`. At most once per 30
    /// minutes and three times in 24 hours for each return
    /// ([`bir_core::smart_queue::QueueState::retry_allowed_at`]): a press
    /// too soon changes nothing and answers when it may be pressed again.
    /// The return then goes as soon as the line lets it, never before the
    /// queue-wide back-off after a way in that is down, unless the person
    /// confirmed `try_anyway`. Only an unclaimed Queued return: one that
    /// needs attention is never resent by itself. Wake the queue after
    /// [`RetryAnswer::Going`] (`Submission::send_now`).
    ///
    /// No tool an AI agent can call reaches this: it is the person's button.
    pub fn retry(&self, id: i64, try_anyway: bool) -> AppResult<RetryAnswer> {
        self.retry_at(id, try_anyway, chrono::Utc::now())
    }

    #[doc(hidden)]
    pub fn retry_at(&self, id: i64, try_anyway: bool, now: chrono::DateTime<chrono::Utc>) -> AppResult<RetryAnswer> {
        let path_floor = {
            let db = self.db.lock().map_err(|_| AppError::Call("the database lock is poisoned".into()))?;
            bir_core::smart_queue::PathBackoff::from_setting(db.get_setting(bir_core::smart_queue::PATH_SETTING)?.as_deref()).floor(now)
        };
        let mut answer = None;
        self.change_lifecycle(id, |status, map| {
            if status != "Queued" {
                return Err(AppError::Call("Only a queued return can be sent again.".into()));
            }
            if map.get("submission_claim_token").is_some_and(|v| !v.is_null()) {
                return Err(AppError::Call("BIR may already have this return. Check for BIR's confirmation email before sending it again.".into()));
            }
            let mut state: bir_core::smart_queue::QueueState =
                map.get("smart_queue").and_then(|value| serde_json::from_value(value.clone()).ok()).unwrap_or_default();
            if let Some(at) = state.retry_allowed_at(now) {
                answer = Some(RetryAnswer::NotBefore(at));
                return Ok(());
            }
            state.press_retry(now, try_anyway);
            map.insert("next_retry_at".into(), Value::String(now.to_rfc3339()));
            map.insert("smart_queue".into(), serde_json::to_value(&state).map_err(call_error)?);
            answer = Some(RetryAnswer::Going { not_before: path_floor.filter(|_| !try_anyway) });
            Ok(())
        })?;
        answer.ok_or_else(|| AppError::Call("The return could not be read.".into()))
    }

    /// [`Self::retry`] without "try anyway", for callers that only need to
    /// know it went: a press that is too soon is an error that says when it
    /// may be pressed again.
    pub fn retry_now(&self, id: i64) -> AppResult<()> {
        match self.retry(id, false)? {
            RetryAnswer::Going { .. } => Ok(()),
            RetryAnswer::NotBefore(at) => Err(AppError::Call(retry_wait_words(at, chrono::Utc::now()))),
        }
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
                SendStage::Behind { next_retry_at, .. } => {
                    let later = chrono::Utc::now() + chrono::Duration::minutes(1);
                    set("submission_error", Value::Null);
                    set("next_retry_at", Value::String(next_retry_at.unwrap_or_else(|| later.to_rfc3339())));
                    set("smart_queue", serde_json::json!({ "next_try_at": now }));
                }
                SendStage::SlowedDown { reason, next_retry_at } => {
                    let later = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
                    let at = next_retry_at.unwrap_or(later);
                    set("submission_error", Value::String(reason));
                    set("submission_attempts", Value::from(attempts.max(3)));
                    set("next_retry_at", Value::String(at.clone()));
                    set("smart_queue", serde_json::json!({ "failures": attempts.max(3), "next_try_at": at, "slowed": true }));
                }
                SendStage::StillWaiting { reason, next_retry_at, .. } => {
                    let later = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
                    let at = next_retry_at.unwrap_or(later);
                    set("submission_error", reason.map_or(Value::Null, Value::String));
                    set("next_retry_at", Value::String(at.clone()));
                    set("smart_queue", serde_json::json!({ "failures": 5, "next_try_at": at, "notices": 1 }));
                }
                SendStage::NeedsYou { reason } => {
                    set("submission_error", Value::String(reason.clone()));
                    set("submission_attempts", Value::from(attempts.max(3)));
                    set("smart_queue", serde_json::json!({ "failures": attempts.max(3), "stuck": reason, "kind": reason, "same_kind": 3 }));
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

    #[test]
    fn the_smart_queue_states_read_from_the_row() {
        let own = "2026-11-11T09:00:00+00:00";
        let later = "2026-11-11T09:02:00+00:00";
        let behind = json!({"next_retry_at": later, "smart_queue": {"next_try_at": own}});
        assert_eq!(send_stage("Queued", &behind), Some(SendStage::Behind { reason: None, next_retry_at: Some(later.into()) }));
        let on_time = json!({"next_retry_at": own, "smart_queue": {"next_try_at": own, "failures": 1}, "submission_error": "x"});
        assert!(matches!(send_stage("Queued", &on_time), Some(SendStage::Failed { .. })));
        let slowed = json!({"next_retry_at": later, "submission_error": "Not sent (nothing reached BIR): x", "smart_queue": {"next_try_at": later, "failures": 3, "slowed": true}});
        assert_eq!(
            send_stage("Queued", &slowed),
            Some(SendStage::SlowedDown { reason: "Not sent (nothing reached BIR): x".into(), next_retry_at: Some(later.into()) })
        );
        let waiting = json!({"next_retry_at": later, "submission_error": "e", "queue_authorization": {"authorized_at": "2026-11-10T08:00:00Z"}, "smart_queue": {"notices": 1, "slowed": true}});
        assert_eq!(
            send_stage("Queued", &waiting),
            Some(SendStage::StillWaiting { since: "2026-11-10T08:00:00Z".into(), reason: Some("e".into()), next_retry_at: Some(later.into()) })
        );
        // A claim wins over all of them: BIR may have it.
        let mut claimed = waiting.clone();
        claimed["submission_claim_token"] = json!("t");
        assert!(matches!(send_stage("Queued", &claimed), Some(SendStage::NeedsAttention { .. })));
        // A row from before the smart queue reads as it did.
        assert_eq!(send_stage("Queued", &json!({"next_retry_at": later})), Some(SendStage::Queued { next_retry_at: Some(later.into()) }));
    }

    /// Rows as the worker leaves them: `own` is the return's own next try,
    /// `stored` the time on its row (later when it waits behind another or
    /// for the queue-wide back-off).
    fn stage_of_row(id: i64, own: &str, stored: &str, error: Option<&str>) -> (i64, SendStage, u32) {
        let mut data = json!({"next_retry_at": stored, "submission_attempts": 2, "smart_queue": {"next_try_at": own, "failures": 1}});
        if let Some(error) = error {
            data["submission_error"] = json!(error);
        }
        (id, send_stage("Queued", &data).unwrap(), 2)
    }

    #[test]
    fn behind_needs_another_return_that_goes_first() {
        let own = "2026-11-11T09:00:00+00:00";
        let floor = "2026-11-11T09:30:00+00:00";
        let reason = "Not sent (nothing reached BIR): the BIR could not be reached";
        // Alone, waiting only for the queue-wide back-off: no one is ahead.
        let alone = settle_behind(vec![stage_of_row(1, own, floor, Some(reason))]);
        assert_eq!(
            alone[&1],
            SendStage::Failed { reason: reason.into(), attempts: 2, next_retry_at: Some(floor.into()) }
        );
        let fresh = settle_behind(vec![stage_of_row(1, own, floor, None)]);
        assert_eq!(fresh[&1], SendStage::Queued { next_retry_at: Some(floor.into()) });
        // Two waiting on the same back-off: the earlier stored time is the
        // head and the other goes right after it.
        let line_up = settle_behind(vec![
            stage_of_row(1, own, floor, Some(reason)),
            stage_of_row(2, own, "2026-11-11T09:31:00+00:00", Some(reason)),
        ]);
        assert!(matches!(line_up[&1], SendStage::Failed { .. }), "{:?}", line_up[&1]);
        assert_eq!(
            line_up[&2],
            SendStage::Behind { reason: Some(reason.into()), next_retry_at: Some("2026-11-11T09:31:00+00:00".into()) }
        );
        // One being sent goes first.
        let sending = settle_behind(vec![stage_of_row(1, own, floor, None), (2, SendStage::Sending, 0)]);
        assert!(matches!(sending[&1], SendStage::Behind { .. }));
        // One stopped, or that may be at the BIR, is not tried before it.
        let stopped = settle_behind(vec![
            stage_of_row(1, own, floor, None),
            (2, SendStage::NeedsYou { reason: "x".into() }, 0),
            (3, SendStage::NeedsAttention { reason: "x".into() }, 0),
        ]);
        assert!(matches!(stopped[&1], SendStage::Queued { .. }));
    }

    #[test]
    fn a_return_waiting_for_the_queue_wide_wait_says_when() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-11T09:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let at = |minutes: i64| Some((now + chrono::Duration::minutes(minutes)).to_rfc3339());
        assert_eq!(
            queue_line(&SendStage::Failed { reason: "Not sent (nothing reached BIR): the BIR could not be reached".into(), attempts: 2, next_retry_at: at(30) }, now).unwrap(),
            "The BIR could not be reached. Tries again in about 30 min."
        );
        assert_eq!(queue_line(&SendStage::Queued { next_retry_at: at(30) }, now).unwrap(), "Tries again in about 30 min.");
        // Due now or within a minute: just waiting to send.
        assert_eq!(queue_line(&SendStage::Queued { next_retry_at: at(0) }, now), None);
        assert_eq!(queue_line(&SendStage::Queued { next_retry_at: None }, now), None);
    }

    #[test]
    fn every_stage_has_a_stable_name() {
        let stages = [
            SendStage::Queued { next_retry_at: None },
            SendStage::Behind { reason: None, next_retry_at: None },
            SendStage::NeedsYou { reason: String::new() },
            SendStage::SlowedDown { reason: String::new(), next_retry_at: None },
            SendStage::StillWaiting { since: String::new(), reason: None, next_retry_at: None },
            SendStage::Paused,
            SendStage::Sending,
            SendStage::Failed { reason: String::new(), attempts: 1, next_retry_at: None },
            SendStage::NeedsAttention { reason: String::new() },
            SendStage::GaveUp { reason: String::new() },
            SendStage::Refused { code: TinRefusal::NotActive, reason: String::new() },
            SendStage::Sent { at: None, receipt: None },
            SendStage::Confirmed { at: None },
            SendStage::Paid,
        ];
        let names: Vec<&str> = stages.iter().map(SendStage::name).collect();
        assert_eq!(
            names,
            ["queued", "behind", "needs_you", "slowed_down", "still_waiting", "paused", "sending", "failed", "needs_attention", "gave_up", "refused", "sent", "confirmed", "paid"]
        );
        let waiting: Vec<bool> = stages.iter().map(SendStage::is_waiting).collect();
        assert_eq!(waiting, [true, true, false, true, true, true, false, true, false, false, false, false, false, false]);
    }

    #[test]
    fn the_queue_lines_are_api_10s_words() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-11T09:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let at = |minutes: i64| Some((now + chrono::Duration::minutes(minutes)).to_rfc3339());
        assert_eq!(queue_line(&SendStage::Behind { reason: None, next_retry_at: at(1) }, now).unwrap(), "Goes right after the one above.");
        assert_eq!(
            queue_line(&SendStage::SlowedDown { reason: "x".into(), next_retry_at: at(118) }, now).unwrap(),
            "Returns are getting in for others. The problem may be your own connection. Tries again in about 2 h."
        );
        assert_eq!(
            queue_line(&SendStage::StillWaiting { since: "2026-11-10T08:00:00Z".into(), reason: None, next_retry_at: None }, now).unwrap(),
            "Still waiting after 24 hours. It keeps trying by itself; you can cancel it or file another way."
        );
        assert_eq!(
            queue_line(&SendStage::StillWaiting { since: "2026-11-09T08:00:00Z".into(), reason: None, next_retry_at: None }, now).unwrap(),
            "Still waiting after 48 hours. It keeps trying by itself; you can cancel it or file another way."
        );
        assert_eq!(queue_line(&SendStage::Paused, now).unwrap(), "Your queue is paused · press Auto to send");
        assert_eq!(
            queue_line(&SendStage::Failed { reason: "Not sent (nothing reached BIR): the relay closed early (relay_closed)".into(), attempts: 2, next_retry_at: at(29) }, now).unwrap(),
            "The relay closed early. Tries again in about 30 min."
        );
        // A step for the person (sign in) is the whole line.
        assert_eq!(
            queue_line(&SendStage::Failed { reason: "Sign in to send returns.".into(), attempts: 0, next_retry_at: at(5) }, now).unwrap(),
            "Sign in to send this return."
        );
        assert_eq!(queue_line(&SendStage::Sending, now), None);
        assert_eq!(queue_line(&SendStage::Queued { next_retry_at: None }, now), None);
        let about = |minutes: i64| about(&(now + chrono::Duration::seconds(minutes * 60 - 30)).to_rfc3339(), now).unwrap();
        assert_eq!([about(0), about(6), about(12), about(30), about(61), about(150)], ["a moment", "about 6 min", "about 10 min", "about 30 min", "about 1 h", "about 3 h"]);
    }
}
