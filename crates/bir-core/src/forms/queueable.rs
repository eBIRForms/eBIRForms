//! The submission lifecycle shared by every queueable form.
//!
//! 1601C and 2551Q carry these fields and transitions inline, with their own
//! storage and worker code. A form that implements [`QueueableForm`] instead
//! embeds one [`SubmissionLifecycle`] (`#[serde(flatten)]`, so the stored JSON
//! keeps the same field names) and gets queueing, revalidation, the exact
//! official payload, storage (`db::queueable`) and the background worker
//! (`background_cron`) from this module.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::penalty_items::PenaltyItems;
use super::{FilingPeriod, FilingStatus};
use crate::filing_queue::{QueueAuthSource, QueueAuthorization};
use crate::penalties::PenaltyComponent;

/// Status, queue authorization, network claim and retry state of one return.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmissionLifecycle {
    pub status: FilingStatus,
    pub created_at: String,
    pub updated_at: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<i64>,
    /// The exact plaintext XML of this generation: written at queue time and
    /// replaced with the bytes actually encrypted for PUT at submission. Kept
    /// so a later amendment can snapshot what BIR received. Absent on returns
    /// queued before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_payload: Option<String>,
    /// The submission service's signed receipt for the send that made this
    /// return Sent (a compact JWS; `bir-submit` verifies it). Absent on
    /// returns sent before receipts existed and on transports without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_receipt: Option<String>,

    /// SHA-256 over the exact reviewed field map when the user queued the
    /// return. Every later step must reproduce it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_submission_fingerprint: Option<String>,
    /// Scoped grant recorded at enqueue (form + TIN + period + Confirmed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_authorization: Option<QueueAuthorization>,
    /// Durable claim taken immediately before network I/O. Recoverable only
    /// when PUT never started and the lease expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_claim_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_claimed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_claim_lease_until: Option<String>,
    /// Set immediately before PUT. Presence means bytes may have reached BIR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_put_started_at: Option<String>,

    #[serde(default)]
    pub submission_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
    /// Profile boxes the filer edited on this return (their on-screen
    /// keys). They keep the edit when the tax profile changes; every other
    /// profile box of a Draft follows the profile. Never filed or printed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_edits: Vec<String>,
    /// The tax-profile values the draft's profile boxes were built from
    /// (draft path -> value), recorded when the page opens or saves it. A
    /// profile box follows a later profile change only while it still
    /// holds this value. Never filed or printed.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub profile_baseline: std::collections::BTreeMap<String, serde_json::Value>,
    /// The filer entered this as-needed return's trigger date (its removal,
    /// withholding or transaction date) on the page, so it is no longer the
    /// date the form pre-fills (decision D8c; see
    /// [`crate::forms::event_trigger`]). Absent (false) on every draft saved
    /// before it existed, so their stored bytes are unchanged. Never filed
    /// or printed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trigger_date_entered: bool,
    /// The trigger-date fields the filer has typed on this return so far
    /// (top-level draft fields, sorted), kept so a date typed over two
    /// sessions, or one part edited after a restart, still counts. Written
    /// by the host only, from the stored row and its own typed writes;
    /// absent (empty) on every draft saved before it existed. Never filed
    /// or printed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger_parts_typed: Vec<String>,
    /// This draft was created under the typed-trigger model (decision
    /// P5-7): while it is an undated Draft of a pre-filled form it computes
    /// no penalties. Written once, when the draft is created, by a builder
    /// that records the filer's typed trigger date (the shell page; the
    /// desktop 1606 and 1706 views). Absent (false) on every row stored
    /// before, which keep the behaviour they had: penalties from the date
    /// they hold. Never filed or printed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trigger_date_model: bool,
}

impl Default for SubmissionLifecycle {
    fn default() -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            status: FilingStatus::Draft,
            created_at: now.clone(),
            updated_at: now,
            submitted_at: None,
            confirmed_at: None,
            submission_filename: None,
            receipt_id: None,
            submitted_payload: None,
            submission_receipt: None,
            queued_submission_fingerprint: None,
            queue_authorization: None,
            submission_claim_token: None,
            submission_claimed_at: None,
            submission_claim_lease_until: None,
            submission_put_started_at: None,
            submission_attempts: 0,
            submission_error: None,
            next_retry_at: None,
            profile_edits: Vec::new(),
            profile_baseline: std::collections::BTreeMap::new(),
            trigger_date_entered: false,
            trigger_parts_typed: Vec::new(),
            trigger_date_model: false,
        }
    }
}

impl SubmissionLifecycle {
    pub fn is_editable(&self) -> bool {
        matches!(self.status, FilingStatus::Draft)
    }

    /// True when no network claim is held.
    pub fn is_unclaimed(&self) -> bool {
        self.submission_claim_token.is_none() && self.submission_claimed_at.is_none()
    }

    fn clear_claim(&mut self) {
        self.submission_claim_token = None;
        self.submission_claimed_at = None;
        self.submission_claim_lease_until = None;
        self.submission_put_started_at = None;
    }

    fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    fn mark_queued(&mut self, fingerprint: String, authorization: QueueAuthorization) {
        self.queued_submission_fingerprint = Some(fingerprint);
        self.queue_authorization = Some(authorization);
        self.clear_claim();
        self.status = FilingStatus::Queued;
        self.submission_attempts = 0;
        self.submission_error = None;
        self.next_retry_at = Some(chrono::Utc::now().to_rfc3339());
        self.touch();
    }

    pub fn set_queue_auth_source(&mut self, source: QueueAuthSource) {
        if let Some(auth) = &mut self.queue_authorization {
            auth.source = source;
        }
    }

    /// Queued -> Submitted after a successful PUT.
    pub fn transition_to_submitted(&mut self, filename: String) {
        assert!(
            matches!(self.status, FilingStatus::Queued),
            "Cannot submit form in {:?} status - must be Queued",
            self.status
        );
        let now = chrono::Utc::now().to_rfc3339();
        self.status = FilingStatus::Sent;
        self.submitted_at = Some(now.clone());
        self.submission_filename = Some(filename);
        self.submission_attempts = 0;
        self.submission_error = None;
        self.next_retry_at = None;
        self.clear_claim();
        self.updated_at = now;
    }

    /// Submitted -> Confirmed when the BIR receipt matches.
    pub fn transition_to_confirmed(
        &mut self,
        confirmed_at: String,
        receipt_id: Option<i64>,
        filename: Option<String>,
    ) {
        assert!(
            matches!(self.status, FilingStatus::Sent),
            "Cannot confirm form in {:?} status - must be Sent",
            self.status
        );
        self.status = FilingStatus::Confirmed;
        self.confirmed_at = Some(confirmed_at);
        self.receipt_id = receipt_id;
        if let Some(filename) = filename {
            self.submission_filename = Some(filename);
        }
        self.touch();
    }

    pub fn revert_to_draft(&mut self) {
        assert!(
            !matches!(self.status, FilingStatus::Paid),
            "Cannot revert a Paid form to Draft"
        );
        self.status = FilingStatus::Draft;
        // Back to Draft (an amended copy, a cancelled queue, a refused
        // submission) its boxes were not built from the profile as it is
        // now: no baseline, so a later profile change moves nothing until
        // the filer chooses it (the page offers "Use profile values").
        self.profile_baseline.clear();
        self.submitted_at = None;
        self.confirmed_at = None;
        self.receipt_id = None;
        self.submission_filename = None;
        self.submitted_payload = None;
        self.queued_submission_fingerprint = None;
        self.queue_authorization = None;
        self.clear_claim();
        self.submission_attempts = 0;
        self.submission_error = None;
        self.next_retry_at = None;
        self.touch();
    }

    /// Release a claim whose lease expired before PUT started. Nothing reached
    /// BIR, so the attempt budget and authorization stay as they are.
    pub fn release_expired_unstarted_claim(&mut self, note: String) -> bool {
        if !matches!(self.status, FilingStatus::Queued) || self.is_unclaimed() {
            return false;
        }
        self.submission_error = Some(note);
        self.clear_claim();
        self.next_retry_at = Some(chrono::Utc::now().to_rfc3339());
        self.touch();
        true
    }

    /// A failure before PUT: back off 1, 2, 4, 8 minutes; the fifth failure
    /// returns the return to Draft.
    pub fn record_submission_failure(&mut self, error_msg: String) {
        assert!(
            matches!(self.status, FilingStatus::Queued),
            "Cannot record submission failure in {:?} status - must be Queued",
            self.status
        );
        self.submission_attempts += 1;
        self.submission_error = Some(error_msg);
        self.clear_claim();
        if self.submission_attempts >= 5 {
            self.status = FilingStatus::Draft;
            self.next_retry_at = None;
            self.queued_submission_fingerprint = None;
            self.queue_authorization = None;
            self.submitted_payload = None;
        } else {
            let delay_minutes = 2i64.pow(self.submission_attempts - 1);
            self.next_retry_at =
                Some((chrono::Utc::now() + chrono::Duration::minutes(delay_minutes)).to_rfc3339());
        }
        self.touch();
    }

    /// Take the network claim. Callers hold the storage transaction.
    pub(crate) fn claim(&mut self, token: String, claimed_at: chrono::DateTime<chrono::Utc>) {
        self.submission_claim_token = Some(token);
        self.submission_claimed_at = Some(claimed_at.to_rfc3339());
        self.submission_claim_lease_until =
            Some(crate::filing_queue::claim_lease_until(claimed_at));
        self.submission_put_started_at = None;
        self.submission_error = Some(
            "Submission outcome pending. Automatic retry is disabled unless the claim lease expires before PUT starts; keep any BIR confirmation or receipt and contact support for manual reconciliation before taking another submission action."
                .to_string(),
        );
    }
}

/// save-009, verbatim from the official `saveXML()` (e.g. 1800v2018 L3249).
pub const AMENDED_RETURN_ALERT: &str = "If your intention is to make an amended return, kindly tick Amended Return to 'Yes' before hitting 'Save' or 'Final Copy' or 'Submit'.";

/// Status-box text for a failed queue: the save-009 alert alone, otherwise
/// the usual "Could not queue" line with the error.
pub fn queue_failure_message(form_label: &str, error: &str) -> String {
    if error == AMENDED_RETURN_ALERT {
        AMENDED_RETURN_ALERT.to_string()
    } else {
        format!("Could not queue {form_label}. No submission was started: {error}")
    }
}

/// A form the background worker can validate, serialize exactly like the
/// official app, upload and confirm.
pub trait QueueableForm: Clone + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Short code used in `form_drafts.form_code`, e.g. `"2550Q"`.
    const FORM_CODE: &'static str;
    /// Official form type in filenames and transport, e.g. `"2550Qv2024"`.
    const FORM_TYPE: &'static str;
    /// Rule-package id of the official layout, e.g. `"2550q-v2024"`.
    const LAYOUT_ID: &'static str;

    fn lifecycle(&self) -> &SubmissionLifecycle;
    fn lifecycle_mut(&mut self) -> &mut SubmissionLifecycle;

    /// `form_drafts.id` of the row this draft was loaded from or last saved
    /// to. Lets a save under a changed period key move that row instead of
    /// leaving a stale duplicate behind.
    fn draft_id(&self) -> Option<i64>;
    fn set_draft_id(&mut self, id: Option<i64>);

    /// TIN as stored on the draft (dashed or compact, with branch).
    fn tin(&self) -> &str;
    fn taxable_year(&self) -> u16;
    fn filing_period(&self) -> FilingPeriod;
    /// The period segment of the official filename, as the form's own
    /// `createXMLFileName()` writes it (e.g. `"062025"`, `"122025Q1"`).
    fn period_code(&self) -> String;
    /// Inverse of [`Self::period_code`] for a receipt: the taxable year and
    /// filing period, or `None` when the code is not this form's format.
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)>;
    /// The email embedded in the IAF filename (`#email#`).
    fn submission_email(&self) -> &str;

    /// Recompute every derived amount.
    fn compute(&mut self);
    /// Every rule of the official form, with its exact message.
    fn validate(&self) -> Vec<(String, String)>;
    /// Official control ids and values (see [`crate::official_xml::write`]).
    fn field_map(&self) -> BTreeMap<String, String>;

    /// The field map the form's Print preview fills its frozen sheet with —
    /// `to_print_field_map` where the draft has print-only `derived:` keys,
    /// otherwise `to_bir_field_map` (the same map each desktop view's
    /// `print_fields` builds for `bir_print::frozen_html::filled_document`).
    fn print_field_map(&self) -> BTreeMap<String, String>;

    /// Set the form's "Amended Return?" item to Yes the way its own editor
    /// does. Returns false for a form that has no such item.
    fn mark_amended(&mut self) -> bool {
        false
    }

    /// The "Amended Return?" answer for a form whose official `saveXML()`
    /// runs the save-009 guard (refuse Amended = No once a final original or
    /// a V-numbered version exists). `None` for a form without that guard.
    fn amended_return_answer(&self) -> Option<bool> {
        None
    }

    /// The return's own computed total amount payable (its final "Total
    /// Amount Payable/Still Due" item). Zero or negative means nothing is to
    /// be paid. `None` when the form has no single such total; the Calendar
    /// then keeps asking for payment once the return is filed.
    fn amount_payable(&self) -> Option<f64> {
        None
    }

    /// The form's Surcharge, Interest and Compromise items on the shared
    /// computed-with-override model ([`super::penalty_items`]): one set per
    /// party that carries its own (the filer, then the spouse on a joint
    /// individual return). Empty for a form without such items, or one not
    /// wired to the model.
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        Vec::new()
    }

    /// Set (`Some`) or clear (`None`) the override of one of the filer's
    /// (first declared) penalty items. Returns false for a form without wired
    /// penalty items.
    fn set_penalty_override(&mut self, component: PenaltyComponent, value: Option<f64>) -> bool {
        match Self::penalty_items().first() {
            Some(items) => {
                items.set_override(self, component, value);
                true
            }
            None => false,
        }
    }

    /// Resolve each party's penalty inputs from the profile: the period's
    /// deadline as the Calendar resolves it (global and profile deadline
    /// overrides applied) and the profile's EOPT class. A return that has
    /// left Draft keeps the inputs it was reviewed with.
    fn apply_penalty_profile(
        &mut self,
        profile: &crate::profile::TaxpayerProfile,
        global_overrides: &[crate::calendar_rules::DeadlineOverride],
    ) {
        for items in Self::penalty_items() {
            items.apply_profile(self, profile, global_overrides);
        }
    }

    /// Treat every applicable penalty item's current amount as entered by
    /// hand: each one becomes that component's override, so it files as it
    /// stands whatever the engine computes (what an import does).
    fn keep_entered_penalties(&mut self) {
        for items in Self::penalty_items() {
            if (items.applies)(self) {
                items.adopt_imported(self);
            }
        }
    }

    /// `form_drafts.quarter` value: month, quarter, 0 for annual returns.
    fn period_column(&self) -> i64 {
        period_column(&self.filing_period())
    }

    fn submission_filename(&self) -> String {
        format!(
            "{}-{}-{}#{}#.xml",
            self.tin().replace('-', ""),
            Self::FORM_TYPE,
            self.period_code(),
            self.submission_email()
        )
    }

    fn submission_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("ebirforms:{}:queued-submission:v1\0", Self::FORM_TYPE).as_bytes());
        hasher.update(
            serde_json::to_vec(&self.field_map())
                .expect("a BTreeMap<String, String> always serializes to JSON"),
        );
        hex::encode(hasher.finalize())
    }

    /// The official submit plaintext, only for a draft that validates.
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = self.validate();
        if !errors.is_empty() {
            return Err(errors);
        }
        let layout = crate::official_xml::layout(Self::LAYOUT_ID)
            .map_err(|error| vec![("xml".to_string(), error.to_string())])?;
        crate::official_xml::write(layout, &self.field_map())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])
    }

    /// Draft -> Queued: freeze the reviewed field map behind a fingerprint and
    /// a scoped authorization.
    fn queue(&mut self, source: QueueAuthSource) -> Result<(), Vec<(String, String)>> {
        if !self.lifecycle().is_editable() {
            return Err(vec![(
                "status".to_string(),
                format!(
                    "Only a Draft can be queued; this return is {:?}",
                    self.lifecycle().status
                ),
            )]);
        }
        self.compute();
        let payload = self.official_payload()?;
        let fingerprint = self.submission_fingerprint();
        let authorization = QueueAuthorization::new(
            Self::FORM_CODE,
            self.tin().to_string(),
            self.period_code(),
            source,
        );
        self.lifecycle_mut().mark_queued(fingerprint, authorization);
        self.lifecycle_mut().submitted_payload = Some(payload);
        Ok(())
    }

    /// Recompute and revalidate an immutable queued snapshot right before XML
    /// generation or a claim. Any drift returns it to Draft with the reason.
    fn revalidate_queued_before_submission(&mut self) -> Result<(), Vec<(String, String)>> {
        if !matches!(self.lifecycle().status, FilingStatus::Queued) {
            return Err(vec![(
                "status".to_string(),
                "Only a Queued return can be revalidated for submission".to_string(),
            )]);
        }
        let reviewed = self.lifecycle().queued_submission_fingerprint.clone();
        // Computed penalties refresh here (and only here) once queued: a
        // change since the return was reviewed fails closed.
        let penalty_items = Self::penalty_items();
        let reviewed_penalties: Vec<[f64; 3]> = penalty_items
            .iter()
            .map(|items| items.filed(self))
            .collect();
        let today = super::penalty_items::today();
        match penalty_items.first() {
            // One compute chain refreshes every party's items.
            Some(items) => items.compute_on(self, today, true),
            None => self.compute(),
        }
        let refreshed = self.submission_fingerprint();
        let mut errors = match self.official_payload() {
            Ok(_) => Vec::new(),
            Err(errors) => errors,
        };
        for (items, reviewed) in penalty_items.iter().zip(reviewed_penalties) {
            if items.filed(self) != reviewed {
                errors.push(items.changed_error(self, today));
            }
        }
        match reviewed {
            Some(fingerprint) if fingerprint == refreshed => {}
            Some(_) => errors.push((
                "queued_submission_fingerprint".to_string(),
                "Submission fields changed after the return was queued; review the return and queue it again"
                    .to_string(),
            )),
            None => errors.push((
                "queued_submission_fingerprint".to_string(),
                "Queued return has no review fingerprint; reopen and queue it again before submission"
                    .to_string(),
            )),
        }
        if let Some(error) = crate::filing_queue::scoped_authorization_error(
            self.lifecycle().queue_authorization.as_ref(),
            Self::FORM_CODE,
            self.tin(),
            &self.period_code(),
        ) {
            errors.push(error);
        }
        if errors.is_empty() {
            return Ok(());
        }
        let summary = errors
            .iter()
            .map(|(field, message)| format!("{field}: {message}"))
            .collect::<Vec<_>>()
            .join("; ");
        let lifecycle = self.lifecycle_mut();
        lifecycle.revert_to_draft();
        lifecycle.submission_error = Some(format!(
            "Submission blocked by queue revalidation: {summary}"
        ));
        Err(errors)
    }
}

/// Whether the user may "Revert to Draft" a return in `status` (D7A):
/// Queued, Submitted and Confirmed may be reverted; Draft has nothing to
/// revert and Paid never reverts.
pub fn can_user_revert(status: &FilingStatus) -> bool {
    matches!(
        status,
        FilingStatus::Queued | FilingStatus::Sent | FilingStatus::Confirmed
    )
}

/// `form_drafts.quarter` value for a filing period.
pub fn period_column(period: &FilingPeriod) -> i64 {
    match period {
        FilingPeriod::Monthly(month) => i64::from(*month),
        FilingPeriod::Quarterly(quarter) => i64::from(*quarter),
        FilingPeriod::Annual => 0,
        FilingPeriod::OpenEnded(n) => i64::from(*n),
    }
}

/// Every form on the generic submission path. When a form joins, add its
/// variant here, to [`QueueableKind::ALL`] and an arm in
/// [`crate::with_queueable_kind`]; lookups follow from its trait constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueableKind {
    Form2553,
    Form1604F,
    Form1604C,
    Form1604E,
    Form1606,
    Form1600VT,
    Form1600PT,
    Form1600WP,
    Form1601EQ,
    Form1601FQ,
    Form1603Q,
    Form1602Q,
    Form2552,
    Form2550M,
    Form1706,
    Form1707,
    Form1707A,
    Form1800,
    Form1801,
    Form2000,
    Form2000OT,
    Form2200S,
    Form2200C,
    Form2200AN,
    Form2200M,
    Form2200T,
    Form2200A,
    Form2200P,
    Form1701MS,
    Form1702Q,
    Form1701A,
    Form1700,
    Form1702EX,
    Form0605,
    Form0619E,
    Form0619F,
    Form1701Q,
    Form1702RT,
    Form1701,
    Form1702MX,
    Form2550Q,
    #[cfg(test)]
    Test,
}

impl QueueableKind {
    /// Every production form on the generic path.
    pub const ALL: &'static [QueueableKind] = &[
        QueueableKind::Form2553,
        QueueableKind::Form1604F,
        QueueableKind::Form1604C,
        QueueableKind::Form1604E,
        QueueableKind::Form1606,
        QueueableKind::Form1600VT,
        QueueableKind::Form1600PT,
        QueueableKind::Form1600WP,
        QueueableKind::Form1601EQ,
        QueueableKind::Form1601FQ,
        QueueableKind::Form1603Q,
        QueueableKind::Form1602Q,
        QueueableKind::Form2552,
        QueueableKind::Form2550M,
        QueueableKind::Form1706,
        QueueableKind::Form1707,
        QueueableKind::Form1707A,
        QueueableKind::Form1800,
        QueueableKind::Form1801,
        QueueableKind::Form2000,
        QueueableKind::Form2000OT,
        QueueableKind::Form2200S,
        QueueableKind::Form2200C,
        QueueableKind::Form2200AN,
        QueueableKind::Form2200M,
        QueueableKind::Form2200T,
        QueueableKind::Form2200A,
        QueueableKind::Form2200P,
        QueueableKind::Form1701MS,
        QueueableKind::Form1702Q,
        QueueableKind::Form1701A,
        QueueableKind::Form1700,
        QueueableKind::Form1702EX,
        QueueableKind::Form0605,
        QueueableKind::Form0619E,
        QueueableKind::Form0619F,
        QueueableKind::Form1701Q,
        QueueableKind::Form1702RT,
        QueueableKind::Form1701,
        QueueableKind::Form1702MX,
        QueueableKind::Form2550Q,
    ];

    fn candidates() -> impl Iterator<Item = QueueableKind> {
        let all = Self::ALL.iter().copied();
        #[cfg(test)]
        let all = all.chain(std::iter::once(Self::Test));
        all
    }

    /// `form_drafts.form_code`, e.g. `"2553"`.
    pub fn form_code(self) -> &'static str {
        crate::with_queueable_kind!(self, F => <F as QueueableForm>::FORM_CODE)
    }

    /// Official form type in filenames and transport, e.g. `"2553"`.
    pub fn form_type(self) -> &'static str {
        crate::with_queueable_kind!(self, F => <F as QueueableForm>::FORM_TYPE)
    }

    /// The compromise schedule of each party's penalty items; empty for a
    /// form without items on the shared penalty model.
    pub fn penalty_schedules(self) -> Vec<crate::penalties::PenaltyProfile> {
        crate::with_queueable_kind!(self, F => <F as QueueableForm>::penalty_items()
            .iter()
            .map(|items| items.schedule)
            .collect())
    }

    /// The draft fields of each party's penalty items (surcharge, interest,
    /// compromise); empty for a form without items on the shared model.
    pub fn penalty_fields(self) -> Vec<[&'static str; 3]> {
        crate::with_queueable_kind!(self, F => <F as QueueableForm>::penalty_items()
            .iter()
            .map(|items| items.fields)
            .collect())
    }

    /// From `form_drafts.form_code`.
    pub fn from_form_code(code: &str) -> Option<Self> {
        Self::candidates().find(|kind| kind.form_code() == code)
    }

    /// From a receipt's official form type.
    pub fn from_form_type(form_type: &str) -> Option<Self> {
        Self::candidates().find(|kind| kind.form_type() == form_type)
    }
}

/// Run `$body` with `$ty` bound to the draft type of a [`QueueableKind`].
#[cfg(test)]
#[doc(hidden)]
#[macro_export]
macro_rules! __queueable_kind_match {
    ($kind:expr, $ty:ident => $body:expr; $($arms:tt)*) => {
        match $kind {
            $($arms)*
            $crate::forms::queueable::QueueableKind::Test => {
                type $ty = $crate::forms::queueable::test_support::TestForm;
                $body
            }
        }
    };
}

#[cfg(not(test))]
#[doc(hidden)]
#[macro_export]
macro_rules! __queueable_kind_match {
    ($kind:expr, $ty:ident => $body:expr; $($arms:tt)*) => {
        match $kind {
            $($arms)*
        }
    };
}

#[macro_export]
macro_rules! with_queueable_kind {
    ($kind:expr, $ty:ident => $body:expr) => {
        $crate::__queueable_kind_match!($kind, $ty => $body;
            $crate::forms::queueable::QueueableKind::Form2553 => {
                type $ty = $crate::forms::form_2553::Form2553Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1604F => {
                type $ty = $crate::forms::form_1604f::Form1604fDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1604C => {
                type $ty = $crate::forms::form_1604c::Form1604cDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1604E => {
                type $ty = $crate::forms::form_1604e::Form1604eDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1606 => {
                type $ty = $crate::forms::form_1606::Form1606Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1600VT => {
                type $ty = $crate::forms::form_1600vt::Form1600VtDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1600PT => {
                type $ty = $crate::forms::form_1600pt::Form1600PtDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1600WP => {
                type $ty = $crate::forms::form_1600wp::Form1600WpDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1601EQ => {
                type $ty = $crate::forms::form_1601eq::Form1601EqDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1601FQ => {
                type $ty = $crate::forms::form_1601fq::Form1601FqDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1603Q => {
                type $ty = $crate::forms::form_1603q::Form1603QDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1602Q => {
                type $ty = $crate::forms::form_1602q::Form1602QDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2552 => {
                type $ty = $crate::forms::form_2552::Form2552Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2550M => {
                type $ty = $crate::forms::form_2550m::Form2550MDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1706 => {
                type $ty = $crate::forms::form_1706::Form1706Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1707 => {
                type $ty = $crate::forms::form_1707::Form1707Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1707A => {
                type $ty = $crate::forms::form_1707a::Form1707ADraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1800 => {
                type $ty = $crate::forms::form_1800::Form1800Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1801 => {
                type $ty = $crate::forms::form_1801::Form1801Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2000 => {
                type $ty = $crate::forms::form_2000::Form2000Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2000OT => {
                type $ty = $crate::forms::form_2000ot::Form2000OTDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2200S => {
                type $ty = $crate::forms::form_2200s::Form2200SDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2200C => {
                type $ty = $crate::forms::form_2200c::Form2200CDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2200AN => {
                type $ty = $crate::forms::form_2200an::Form2200ANDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2200M => {
                type $ty = $crate::forms::form_2200m::Form2200MDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2200T => {
                type $ty = $crate::forms::form_2200t::Form2200TDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2200A => {
                type $ty = $crate::forms::form_2200a::Form2200ADraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2200P => {
                type $ty = $crate::forms::form_2200p::Form2200PDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1701MS => {
                type $ty = $crate::forms::form_1701ms::Form1701MsDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1702Q => {
                type $ty = $crate::forms::form_1702q::Form1702qDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1701A => {
                type $ty = $crate::forms::form_1701a::Form1701ADraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1700 => {
                type $ty = $crate::forms::form_1700::Form1700Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1702EX => {
                type $ty = $crate::forms::form_1702ex::Form1702ExDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form0605 => {
                type $ty = $crate::forms::form_0605::Form0605Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form0619E => {
                type $ty = $crate::forms::form_0619e::Form0619EDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form0619F => {
                type $ty = $crate::forms::form_0619f::Form0619FDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1701Q => {
                type $ty = $crate::forms::form_1701q::Form1701QDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1702RT => {
                type $ty = $crate::forms::form_1702rt::Form1702RTDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1701 => {
                type $ty = $crate::forms::form_1701::Form1701Draft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form1702MX => {
                type $ty = $crate::forms::form_1702mx::Form1702MXDraft;
                $body
            }
            $crate::forms::queueable::QueueableKind::Form2550Q => {
                type $ty = $crate::forms::form_2550q::Form2550QDraft;
                $body
            }
        )
    };
}

/// A minimal queueable form over the 1601C layout, for testing the generic
/// storage and worker without a real form.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct TestForm {
        pub tin: String,
        pub taxable_year: u16,
        pub month: u8,
        pub email: String,
        pub amount: f64,
        #[serde(default)]
        pub doubled: f64,
        #[serde(default)]
        pub id: Option<i64>,
        #[serde(default)]
        pub is_amended: bool,
        #[serde(flatten)]
        pub lifecycle: SubmissionLifecycle,
    }

    impl TestForm {
        pub const CODE: &'static str = "TESTQ";
        pub const TYPE: &'static str = "TESTQv1";

        pub fn new(month: u8, amount: f64) -> Self {
            Self {
                tin: "123-456-788-00000".to_string(),
                taxable_year: 2025,
                month,
                email: "sample@example.com".to_string(),
                amount,
                doubled: 0.0,
                id: None,
                is_amended: false,
                lifecycle: SubmissionLifecycle::default(),
            }
        }
    }

    impl QueueableForm for TestForm {
        fn print_field_map(&self) -> BTreeMap<String, String> {
            self.field_map()
        }
        const FORM_CODE: &'static str = Self::CODE;
        const FORM_TYPE: &'static str = Self::TYPE;
        const LAYOUT_ID: &'static str = "1601c-v2018";

        fn lifecycle(&self) -> &SubmissionLifecycle {
            &self.lifecycle
        }
        fn lifecycle_mut(&mut self) -> &mut SubmissionLifecycle {
            &mut self.lifecycle
        }
        fn amended_return_answer(&self) -> Option<bool> {
            Some(self.is_amended)
        }
        fn draft_id(&self) -> Option<i64> {
            self.id
        }
        fn set_draft_id(&mut self, id: Option<i64>) {
            self.id = id;
        }
        fn mark_amended(&mut self) -> bool {
            self.is_amended = true;
            true
        }
        fn tin(&self) -> &str {
            &self.tin
        }
        fn taxable_year(&self) -> u16 {
            self.taxable_year
        }
        fn filing_period(&self) -> FilingPeriod {
            FilingPeriod::Monthly(self.month)
        }
        fn period_code(&self) -> String {
            format!("{:02}{}", self.month, self.taxable_year)
        }
        fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
            let month: u8 = code.get(..2)?.parse().ok()?;
            let year: u16 = code.get(2..6)?.parse().ok()?;
            (code.len() == 6 && (1..=12).contains(&month))
                .then_some((year, FilingPeriod::Monthly(month)))
        }
        fn submission_email(&self) -> &str {
            &self.email
        }
        fn compute(&mut self) {
            self.doubled = self.amount * 2.0;
        }
        fn validate(&self) -> Vec<(String, String)> {
            if self.amount < 0.0 {
                vec![(
                    "amount".to_string(),
                    "Amount must not be negative".to_string(),
                )]
            } else {
                Vec::new()
            }
        }
        fn field_map(&self) -> BTreeMap<String, String> {
            BTreeMap::from([
                (
                    "frm1601c:txtMonth".to_string(),
                    format!("{:02}", self.month),
                ),
                (
                    "frm1601c:txtYear".to_string(),
                    self.taxable_year.to_string(),
                ),
                (
                    "frm1601c:txtTax14".to_string(),
                    format!("{:.2}", self.amount),
                ),
                (
                    "frm1601c:txtTax22".to_string(),
                    format!("{:.2}", self.doubled),
                ),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TestForm;
    use super::*;

    #[test]
    fn lifecycle_flattens_to_the_same_json_keys_as_1601c() {
        let json = serde_json::to_value(TestForm::new(6, 10.0)).unwrap();
        for key in [
            "status",
            "created_at",
            "updated_at",
            "submission_attempts",
            "tin",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert!(json.get("lifecycle").is_none());
    }

    #[test]
    fn only_queued_submitted_and_confirmed_returns_may_be_reverted_by_the_user() {
        assert!(!can_user_revert(&FilingStatus::Draft));
        assert!(can_user_revert(&FilingStatus::Queued));
        assert!(can_user_revert(&FilingStatus::Sent));
        assert!(can_user_revert(&FilingStatus::Confirmed));
        assert!(!can_user_revert(&FilingStatus::Paid));
    }

    #[test]
    fn queue_keeps_the_exact_payload_and_revert_clears_it() {
        let mut form = TestForm::new(6, 10.0);
        form.queue(QueueAuthSource::Gui).unwrap();
        assert_eq!(
            form.lifecycle.submitted_payload.as_deref(),
            Some(form.official_payload().unwrap().as_str())
        );
        form.lifecycle.revert_to_draft();
        assert!(form.lifecycle.submitted_payload.is_none());
    }

    #[test]
    fn queue_freezes_a_fingerprint_and_revalidation_catches_drift() {
        let mut form = TestForm::new(6, 10.0);
        form.queue(QueueAuthSource::Gui).unwrap();
        assert_eq!(form.lifecycle.status, FilingStatus::Queued);
        assert!(form.lifecycle.queued_submission_fingerprint.is_some());
        assert_eq!(form.doubled, 20.0);
        form.clone().revalidate_queued_before_submission().unwrap();

        form.amount = 11.0;
        let errors = form.revalidate_queued_before_submission().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "queued_submission_fingerprint")
        );
        assert_eq!(form.lifecycle.status, FilingStatus::Draft);
    }

    #[test]
    fn invalid_drafts_cannot_be_queued_and_payloads_follow_the_official_layout() {
        let mut bad = TestForm::new(6, -1.0);
        assert!(bad.queue(QueueAuthSource::Gui).is_err());
        assert_eq!(bad.lifecycle.status, FilingStatus::Draft);

        let mut good = TestForm::new(6, 10.0);
        good.compute();
        let payload = good.official_payload().unwrap();
        assert!(payload.contains("<div>frm1601c:txtTax22=20.00frm1601c:txtTax22=</div>"));
        assert!(payload.ends_with("All Rights Reserved BIR 2012.0"));
        assert_eq!(
            good.submission_filename(),
            "12345678800000-TESTQv1-062025#sample@example.com#.xml"
        );
    }

    #[test]
    fn retry_backoff_returns_to_draft_after_five_failures() {
        let mut form = TestForm::new(6, 10.0);
        form.queue(QueueAuthSource::Gui).unwrap();
        for attempt in 1..=4 {
            form.lifecycle
                .record_submission_failure(format!("try {attempt}"));
            assert_eq!(form.lifecycle.status, FilingStatus::Queued);
        }
        form.lifecycle
            .record_submission_failure("try 5".to_string());
        assert_eq!(form.lifecycle.status, FilingStatus::Draft);
        assert!(form.lifecycle.queue_authorization.is_none());
    }
}
