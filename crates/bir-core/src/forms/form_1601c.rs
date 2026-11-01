//! BIR Form 1601C (Monthly Remittance Return of Income Taxes Withheld on Compensation)
//!
//! Data model and reviewed arithmetic for the 1601Cv2018 ENCS offline form.
//!
//! Items 32 through 34 (surcharge, interest, compromise) default to the values
//! the shared penalty engine (`crate::penalties`) computes, and each one can be
//! overridden on its own. The computed value is never filed unseen: the form
//! shows it next to the value used, together with the engine's inputs (the
//! app's resolved deadline for the period, the filing date, which is today for
//! an unfiled return and also stands in for the unknown payment date, the tax
//! still due in Item 31, the profile's EOPT class or the assumed standard
//! class, and the withholding compromise schedule). An override and its
//! optional note (an RDO approval or abatement reference) are saved with the
//! draft. Items 32-34 always hold the used value, so the XML, print and queue
//! fingerprint carry what is filed. Computed values refresh only while the
//! return is a Draft and at queue revalidation, where a change fails closed;
//! a Submitted, Confirmed or Paid return keeps the penalties it was filed with.

use super::penalty_items::{PenaltyItems, PenaltyRounding, PenaltyState};
use super::{FilingStatus, FormValidator};
use crate::calendar_rules::{DeadlineOverride, DeadlinePeriod};
use crate::filing_queue::{QueueAuthSource, QueueAuthorization};
use crate::penalties::{ComputedPenalties, PenaltyComponent, PenaltyProfile, TaxpayerClass};
use crate::profile::TaxpayerProfile;
use crate::validation::{
    OFFICIAL_INVALID_RDO_MESSAGE, OFFICIAL_MIN_FORM_YEAR, OFFICIAL_OLD_VERSION_MESSAGE,
    fits_official_maxlength, official_tax_relief_code, rdo_code_is_official_option, validate_email,
    validate_ph_phone, validate_zip,
};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The 1601-C January 2018 XML contract exposes exactly three Schedule I rows.
pub const MAX_SCHEDULE_1_ROWS: usize = 3;

/// Hash-locked source evidence for the exact January 2018 revision.
pub const OFFICIAL_FORM_SHA256: &str =
    "c8faaa71015337a73b4ceb96bfb265c539589ab5e10eb27899bb81f87f417397";
pub const REVIEWED_EDITABLE_XML_SHA256: &str =
    "794892fc33c0fd7882a91327095f396fb1683d5b3c0d4cb1cb63916f981cad4c";
pub const REVIEWED_ENCRYPTED_XML_SHA256: &str =
    "4501f3514a1883d0137d126101d02b3f0fa94daf7f6e39398b3729c9104c51d3";
pub const EXACT_REVIEWED_PLAIN_XML_FIELD_COUNT: usize = 100;
pub const EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT: usize = 101;
pub const REVIEWED_ENCRYPTED_XML_EXTRA_FIELD: &str = "frm1601c:txtAddress2";

/// Fixed Item 5 ATC for BIR Form 1601-C January 2018.
///
/// Evidence: the pinned official blank PDF
/// `c8faaa71015337a73b4ceb96bfb265c539589ab5e10eb27899bb81f87f417397`
/// prints `WW010`, and the reviewed plain XML sample
/// `794892fc33c0fd7882a91327095f396fb1683d5b3c0d4cb1cb63916f981cad4c`
/// stores `frm1601c:txtATC=WW010`.
pub const FORM_1601C_ATC: &str = "WW010";

/// One official Schedule 1 adjustment row from Part IV of Form 1601-C.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1601CSchedule1Row {
    /// Item 1: previous month in `MM/YYYY` format.
    pub previous_month: String,
    /// Item 2: date paid in `MM/DD/YYYY` format.
    pub date_paid: String,
    /// Item 3: drawee bank/bank code or agency.
    pub drawee_bank_code_or_agency: String,
    /// Item 4: payment reference number.
    pub payment_number: String,
    /// Item 5: tax paid, excluding penalties for the month.
    pub tax_paid: f64,
    /// Item 6: tax that should have been due for the month.
    pub should_be_tax_due: f64,
    /// Item 7: derived adjustment (`Item 6 less Item 5`).
    #[serde(default)]
    pub adjustment: f64,
}

impl Form1601CSchedule1Row {
    /// Recompute the official Item 7 formula using centavo precision.
    pub fn recompute(&mut self) {
        self.adjustment = round_currency(self.should_be_tax_due - self.tax_paid);
    }
}

fn round_currency(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub use super::penalty_items::{PENALTY_MODEL_VERSION, taxpayer_class_of};

/// The 1601-C deadline the app resolves for a month (the same resolver the
/// Calendar uses), with `overrides` applied.
pub fn resolved_due_date(
    year: u16,
    month: u8,
    overrides: &[DeadlineOverride],
) -> Option<NaiveDate> {
    super::penalty_items::resolve_due_date(
        "1601C",
        &DeadlinePeriod::Monthly {
            taxable_year: i32::from(year),
            month,
        },
        overrides,
    )
}

/// Complete draft or filed return for Form 1601C.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Form1601CDraft {
    /// Database row ID (None before first save)
    pub id: Option<i64>,

    // === Filing Period ===
    pub tin: String,
    pub taxable_year: u16,
    pub month: u8, // 1–12

    // === Header Options ===
    pub is_amended: bool,
    pub any_taxes_withheld: bool, // Line 3
    /// Item 3 has no answer yet (`TaxWithheld_1` and `_2` both unchecked, as
    /// the official page opens). `any_taxes_withheld` is ignored while set.
    /// Absent in drafts saved before this field, which load as answered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub any_taxes_withheld_unanswered: bool,
    pub number_of_sheets: u32, // Line 4
    pub atc: String,           // Line 5

    // === Part I — pre-filled from profile ===
    pub rdo_code: String,
    pub line_of_business: String,
    pub taxpayer_name: String,
    pub contact_number: String,
    pub registered_address: String,
    /// Legacy optional second address line retained for backward-compatible
    /// drafts. It is absent from the hash-locked 100-field plaintext sample
    /// and is not part of the exact-source replay claim.
    #[serde(default)]
    pub registered_address_2: String,
    pub zip_code: String,
    /// Item 11: "P" for Private, "G" for Government, "" while unanswered.
    pub category_of_agent: String,
    pub email_address: String,

    // === Item 13 — Tax Relief / Treaty ===
    #[serde(default)]
    pub tax_relief: bool,
    #[serde(default)]
    pub tax_relief_specification: String,

    // === Part II — Computation of Tax ===
    #[serde(default)]
    pub tax_14_total_compensation: f64,

    // Less: Non-Taxable/Exempt Compensation
    #[serde(default)]
    pub tax_15_statutory_minimum_wage: f64,
    #[serde(default)]
    pub tax_16_holiday_pay: f64,
    #[serde(default)]
    pub tax_17_13th_month_pay: f64,
    #[serde(default)]
    pub tax_18_de_minimis: f64,
    #[serde(default)]
    pub tax_19_sss_gsis: f64,

    #[serde(default)]
    pub tax_20_other_name: String,
    #[serde(default)]
    pub tax_20_other_amount: f64,

    // Computed Total Non-Taxable Compensation
    #[serde(default)]
    pub tax_21_total_non_taxable: f64,

    // Computed Total Taxable Compensation
    #[serde(default)]
    pub tax_22_total_taxable: f64,

    #[serde(default)]
    pub tax_23_not_subject: f64,

    // Computed Net Taxable Compensation
    #[serde(default)]
    pub tax_24_net_taxable: f64,

    // Total Taxes Withheld
    #[serde(default)]
    pub tax_25_total_taxes_withheld: f64,

    // Add/Less: Adjustment of Taxes Withheld from Previous Months
    #[serde(default)]
    pub tax_26_adjustment: f64,

    /// Part IV Schedule I rows. The verified XML capacity is three rows.
    #[serde(default)]
    pub schedule_1: Vec<Form1601CSchedule1Row>,

    // Taxes Withheld for Remittance
    #[serde(default)]
    pub tax_27_taxes_withheld_for_remittance: f64,

    // Less: Tax Remitted in Return Previously Filed
    #[serde(default)]
    pub tax_28_tax_remitted_previously: f64,

    #[serde(default)]
    pub tax_29_other_remittances_name: String,
    #[serde(default)]
    pub tax_29_other_remittances_amount: f64,

    // Total Tax Remittances Made
    #[serde(default)]
    pub tax_30_total_tax_remittances: f64,

    // Tax Still Due/(Overremittance)
    #[serde(default)]
    pub tax_31_tax_still_due: f64,

    // === Penalties ===
    /// Legacy persisted switch retained for backward-compatible draft reads.
    /// `compute` always normalizes this to `false`: penalties are always
    /// computed, and the per-component overrides decide what is filed.
    #[serde(default)]
    pub auto_compute_penalties: bool,
    /// Item 32 as filed: the surcharge override when set, else the computed one.
    #[serde(default)]
    pub tax_32_surcharge: f64,
    /// Item 33 as filed: the interest override when set, else the computed one.
    #[serde(default)]
    pub tax_33_interest: f64,
    /// Item 34 as filed: the compromise override when set, else the computed one.
    #[serde(default)]
    pub tax_34_compromise: f64,
    /// Items 32-34's computed values, overrides, note and engine inputs
    /// (the shared model in [`super::penalty_items`]).
    #[serde(flatten)]
    pub penalty: PenaltyState,

    // Computed Total Penalties
    #[serde(default)]
    pub tax_35_total_penalties: f64,

    // Computed Total Amount Payable
    #[serde(default)]
    pub tax_36_total_amount_payable: f64,

    // === Status & Audit ===
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

    /// SHA-256 over the exact reviewed 1601Cv2018 field map at the moment the
    /// user queues the return. Queue retries must reproduce this fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_submission_fingerprint: Option<String>,
    /// Scoped grant recorded at enqueue (form + TIN + period + Confirmed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_authorization: Option<QueueAuthorization>,
    /// Durable claim established immediately before network I/O.
    /// Recoverable only when PUT was never marked started and the lease expired.
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
}

impl Form1601CDraft {
    /// A new return as the official page opens it: pre-filled from the
    /// profile, with Item 3 (Any Taxes Withheld) and Item 11 (Category of
    /// Withholding Agent) unanswered, so validate() raises "Please select an
    /// option for Item 3." / "... Item 11." until the filer picks.
    pub fn new_unanswered_from_profile(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        let mut draft = Self::new_from_profile(profile, year, month);
        draft.any_taxes_withheld = false;
        draft.any_taxes_withheld_unanswered = true;
        draft.category_of_agent.clear();
        draft
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the page-2 header TIN, one 14-digit comb
    ///   (TIN then branch).
    /// - `derived:address_line1` / `derived:line:address_line2`: Item 9 over
    ///   its 40-slot first line, broken at a space, and the 31-slot second
    ///   line (the rest, then Address 2).
    /// - `derived:line:tax_relief_specify`: Item 13A, the relief named on a Yes.
    ///
    /// Schedule I rows past the draft's rows print blank, not `0.00`.
    pub fn to_print_field_map(&self) -> std::collections::BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm1601c:txtPg2TIN1",
                "frm1601c:txtPg2TIN2",
                "frm1601c:txtPg2TIN3",
                "frm1601c:txtPg2BranchCode",
            ],
        );
        let address = self.registered_address.trim().to_uppercase();
        let (line1, rest) = super::print_map::split_print_line(&address, 40);
        let line2 = [rest.as_str(), self.registered_address_2.trim()]
            .iter()
            .filter(|part| !part.is_empty())
            .map(|part| part.to_uppercase())
            .collect::<Vec<_>>()
            .join(" ");
        let specify = if self.tax_relief {
            self.tax_relief_specification.trim().to_string()
        } else {
            String::new()
        };
        // Schedule I rows past the draft's print blank, not 0.00.
        for row in self.schedule_1.len()..MAX_SCHEDULE_1_ROWS {
            for key in ["txtTaxPaid", "txtShouldTaxDue", "txtAdjustments"] {
                fields.insert(format!("frm1601c:sched1:{key}{row}"), String::new());
            }
        }
        for (key, value) in [
            ("tin_digits", tin_digits),
            ("address_line1", line1),
            ("line:address_line2", line2),
            ("line:tax_relief_specify", specify),
        ] {
            fields.insert(format!("derived:{key}"), value);
        }
        fields
    }

    /// Item 3's answer: `None` while unanswered.
    pub fn taxes_withheld_answer(&self) -> Option<bool> {
        (!self.any_taxes_withheld_unanswered).then_some(self.any_taxes_withheld)
    }

    /// Answer Item 3.
    pub fn set_any_taxes_withheld(&mut self, value: bool) {
        self.any_taxes_withheld = value;
        self.any_taxes_withheld_unanswered = false;
    }

    /// Create a new draft pre-filled from a profile, with Item 3 answered
    /// Yes and Item 11 answered Private (fixtures and tests; new returns in
    /// the app use [`Self::new_unanswered_from_profile`]).
    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            month,
            is_amended: false,
            any_taxes_withheld: true,
            any_taxes_withheld_unanswered: false,
            number_of_sheets: 0,
            atc: FORM_1601C_ATC.to_string(),
            rdo_code: profile.rdo_code.clone(),
            line_of_business: profile.line_of_business.clone(),
            taxpayer_name: profile.full_name.clone(),
            contact_number: profile.phone.clone(),
            registered_address: profile.registered_address.clone(),
            registered_address_2: String::new(),
            zip_code: profile.zip_code.clone(),
            category_of_agent: "P".to_string(), // Default Private
            email_address: profile.email.clone(),
            tax_relief: false,
            tax_relief_specification: String::new(),

            tax_14_total_compensation: 0.0,
            tax_15_statutory_minimum_wage: 0.0,
            tax_16_holiday_pay: 0.0,
            tax_17_13th_month_pay: 0.0,
            tax_18_de_minimis: 0.0,
            tax_19_sss_gsis: 0.0,
            tax_20_other_name: String::new(),
            tax_20_other_amount: 0.0,
            tax_21_total_non_taxable: 0.0,
            tax_22_total_taxable: 0.0,
            tax_23_not_subject: 0.0,
            tax_24_net_taxable: 0.0,
            tax_25_total_taxes_withheld: 0.0,
            tax_26_adjustment: 0.0,
            schedule_1: Vec::new(),
            tax_27_taxes_withheld_for_remittance: 0.0,
            tax_28_tax_remitted_previously: 0.0,
            tax_29_other_remittances_name: String::new(),
            tax_29_other_remittances_amount: 0.0,
            tax_30_total_tax_remittances: 0.0,
            tax_31_tax_still_due: 0.0,

            auto_compute_penalties: false,
            tax_32_surcharge: 0.0,
            tax_33_interest: 0.0,
            tax_34_compromise: 0.0,
            penalty: PenaltyState::for_profile(profile),
            tax_35_total_penalties: 0.0,
            tax_36_total_amount_payable: 0.0,

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
        }
    }

    pub fn period_code(&self) -> String {
        format!("{:02}{:04}", self.month, self.taxable_year)
    }

    pub fn default_submission_filename(&self) -> String {
        format!(
            "{}-1601Cv2018-{}#{}#.xml",
            self.tin.replace("-", ""),
            self.period_code(),
            self.email_address
        )
    }

    fn submission_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"ebirforms:1601Cv2018:queued-submission:v1\0");
        hasher.update(
            serde_json::to_vec(&self.to_bir_field_map())
                .expect("a BTreeMap<String, String> always serializes to JSON"),
        );
        hex::encode(hasher.finalize())
    }

    pub fn is_editable(&self) -> bool {
        matches!(self.status, FilingStatus::Draft)
    }

    /// Transition an editable, validated snapshot into the background queue.
    /// The used Items 32-34 (override or computed) are frozen into the
    /// field-map fingerprint.
    pub fn transition_to_queued(&mut self) -> Result<(), Vec<(String, String)>> {
        self.transition_to_queued_on(chrono::Local::now().date_naive())
    }

    fn transition_to_queued_on(&mut self, today: NaiveDate) -> Result<(), Vec<(String, String)>> {
        assert!(
            matches!(self.status, FilingStatus::Draft),
            "Cannot queue form in {:?} status - must be Draft",
            self.status
        );
        self.compute_on(today, true);
        let errors = self.validate();
        if !errors.is_empty() {
            return Err(errors);
        }

        // Keep the reviewed plaintext so a later amendment can snapshot it.
        self.submitted_payload = Some(self.to_bir_xml_payload());
        self.queued_submission_fingerprint = Some(self.submission_fingerprint());
        self.queue_authorization = Some(QueueAuthorization::new(
            "1601C",
            self.tin.clone(),
            self.period_code(),
            QueueAuthSource::Gui,
        ));
        self.submission_claim_token = None;
        self.submission_claimed_at = None;
        self.submission_claim_lease_until = None;
        self.submission_put_started_at = None;
        self.status = FilingStatus::Queued;
        self.submission_attempts = 0;
        self.submission_error = None;
        self.next_retry_at = Some(chrono::Utc::now().to_rfc3339());
        self.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    }

    pub fn set_queue_auth_source(&mut self, source: QueueAuthSource) {
        if let Some(auth) = &mut self.queue_authorization {
            auth.source = source;
        }
    }

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
        self.submission_claim_token = None;
        self.submission_claimed_at = None;
        self.submission_claim_lease_until = None;
        self.submission_put_started_at = None;
        self.updated_at = now;
    }

    /// Transition: Submitted → Confirmed (BIR receipt matched).
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
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Set "Amended Return?" to Yes the way the editor does, and recompute.
    pub fn mark_amended(&mut self) {
        self.is_amended = true;
        self.compute();
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
        self.submission_claim_token = None;
        self.submission_claimed_at = None;
        self.submission_claim_lease_until = None;
        self.submission_put_started_at = None;
        self.submission_attempts = 0;
        self.submission_error = None;
        self.next_retry_at = None;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Recompute and validate an immutable queued snapshot immediately before
    /// XML generation or a database claim. Any drift returns it to Draft.
    pub fn revalidate_queued_before_submission(&mut self) -> Result<(), Vec<(String, String)>> {
        self.revalidate_queued_before_submission_on(chrono::Local::now().date_naive())
    }

    fn revalidate_queued_before_submission_on(
        &mut self,
        today: NaiveDate,
    ) -> Result<(), Vec<(String, String)>> {
        assert!(
            matches!(self.status, FilingStatus::Queued),
            "Cannot revalidate a form in {:?} status - must be Queued",
            self.status
        );
        let reviewed_fingerprint = self.queued_submission_fingerprint.clone();
        let reviewed_penalties = self.filed_penalties();
        self.compute_on(today, true);
        let refreshed_fingerprint = self.submission_fingerprint();
        let mut errors = self.validate();
        if self.filed_penalties() != reviewed_penalties {
            errors.push(Self::penalty_items().changed_error(self, today));
        }
        match reviewed_fingerprint {
            Some(fingerprint) if fingerprint == refreshed_fingerprint => {}
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
            self.queue_authorization.as_ref(),
            "1601C",
            &self.tin,
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
        self.revert_to_draft();
        self.submission_error = Some(format!(
            "Submission blocked by queue revalidation: {summary}"
        ));
        Err(errors)
    }

    /// Release a claim whose lease expired before PUT started.
    ///
    /// Nothing reached BIR, so this is not a submission failure: the attempt
    /// counter, retry back-off and `queue_authorization` are untouched and the
    /// same authorized task is retried immediately. Returns `false` (no change)
    /// unless the draft is `Queued` and carries a claim.
    pub fn release_expired_unstarted_claim(&mut self, note: String) -> bool {
        if !matches!(self.status, FilingStatus::Queued)
            || (self.submission_claim_token.is_none() && self.submission_claimed_at.is_none())
        {
            return false;
        }
        self.submission_error = Some(note);
        self.submission_claim_token = None;
        self.submission_claimed_at = None;
        self.submission_claim_lease_until = None;
        self.submission_put_started_at = None;
        self.next_retry_at = Some(chrono::Utc::now().to_rfc3339());
        self.updated_at = chrono::Utc::now().to_rfc3339();
        true
    }

    pub fn record_submission_failure(&mut self, error_msg: String) {
        assert!(
            matches!(self.status, FilingStatus::Queued),
            "Cannot record submission failure in {:?} status - must be Queued",
            self.status
        );
        self.submission_attempts += 1;
        self.submission_error = Some(error_msg);
        self.submission_claim_token = None;
        self.submission_claimed_at = None;
        self.submission_claim_lease_until = None;
        self.submission_put_started_at = None;
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
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Items 32-34 on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_items() -> PenaltyItems<Self> {
        use PenaltyComponent as C;
        PenaltyItems {
            form_code: "1601C",
            party: None,
            applies: |_| true,
            schedule: PenaltyProfile::Withholding,
            fields: ["tax_32_surcharge", "tax_33_interest", "tax_34_compromise"],
            controls: [
                "frm1601c:txtTax32",
                "frm1601c:txtTax33",
                "frm1601c:txtTax34",
            ],
            labels: ["32 Surcharge", "33 Interest", "34 Compromise"],
            basic_tax_due_label: "Item 31",
            rounding: PenaltyRounding::Cents,
            get: |d, component| match component {
                C::Surcharge => d.tax_32_surcharge,
                C::Interest => d.tax_33_interest,
                C::Compromise => d.tax_34_compromise,
            },
            set: |d, component, value| match component {
                C::Surcharge => d.tax_32_surcharge = value,
                C::Interest => d.tax_33_interest = value,
                C::Compromise => d.tax_34_compromise = value,
            },
            basic_tax_due: |d| d.tax_31_tax_still_due,
            // A compensation-withholding return has no gross-sales basis for
            // the engine's no-tax-due compromise tier.
            gross_sales: None,
            state: |d| &d.penalty,
            state_mut: |d| &mut d.penalty,
            status: |d| d.status.clone(),
            trigger_date_pending: |_| false,
            deadline_period: |d| {
                Some(DeadlinePeriod::Monthly {
                    taxable_year: i32::from(d.taxable_year),
                    month: d.month,
                })
            },
            typed_due_date: None,
            period_code: |d| d.period_code(),
            is_amended: |d| d.is_amended,
            recompute: |d| d.compute(),
        }
    }

    /// The deadline penalties are measured from.
    pub fn penalty_due_date(&self) -> Option<NaiveDate> {
        Self::penalty_items().due_date(self)
    }

    /// Set (`Some`) or clear (`None`) one penalty component's override.
    pub fn set_penalty_override(&mut self, component: PenaltyComponent, value: Option<f64>) {
        Self::penalty_items().set_override(self, component, value);
    }

    /// Record the profile-derived penalty inputs. A return that has left
    /// Draft keeps the inputs it was reviewed with.
    pub fn set_penalty_context(
        &mut self,
        due_date: Option<NaiveDate>,
        taxpayer_class: Option<TaxpayerClass>,
    ) {
        Self::penalty_items().set_context(self, due_date, taxpayer_class);
    }

    /// Resolve the penalty inputs from the profile: the period's deadline as
    /// the Calendar resolves it (global and profile deadline overrides
    /// applied) and the profile's EOPT class.
    pub fn apply_penalty_profile(
        &mut self,
        profile: &TaxpayerProfile,
        global_overrides: &[DeadlineOverride],
    ) {
        Self::penalty_items().apply_profile(self, profile, global_overrides);
    }

    /// What the engine computes for this return if it is filed (and paid) on
    /// `filing_date`. `None` when no deadline is known for the period.
    pub fn penalties_if_filed_on(&self, filing_date: NaiveDate) -> Option<ComputedPenalties> {
        Self::penalty_items().if_filed_on(self, filing_date)
    }

    /// Item 35 as it would stand if this unfiled return were filed on
    /// `filing_date` (overrides honoured). `None` for a filed return or when
    /// nothing can be computed.
    pub fn penalties_payable_if_filed_on(&self, filing_date: NaiveDate) -> Option<f64> {
        Self::penalty_items().payable_if_filed_on(self, filing_date)
    }

    /// Items 32-34 as filed.
    fn filed_penalties(&self) -> [f64; 3] {
        Self::penalty_items().filed(self)
    }

    /// The used value of each component: the override, else the computed one.
    fn used_penalties(&self) -> [f64; 3] {
        Self::penalty_items().used(self).map(round_currency)
    }

    /// Run the compute chain as of `today`; `refresh_penalties` recomputes
    /// the engine's values even for a Queued return.
    fn compute_on(&mut self, today: NaiveDate, refresh_penalties: bool) {
        Self::penalty_items().compute_on(self, today, refresh_penalties);
    }

    /// Recalculate computed fields based on user inputs.
    pub fn compute(&mut self) {
        // Official Schedule I Item 7 = Item 6 less Item 5. The Schedule I
        // total feeds Part II Item 26.
        for row in &mut self.schedule_1 {
            row.recompute();
        }
        self.tax_26_adjustment =
            round_currency(self.schedule_1.iter().map(|row| row.adjustment).sum());

        // Line 21 = 15 + 16 + 17 + 18 + 19 + 20
        self.tax_21_total_non_taxable = round_currency(
            self.tax_15_statutory_minimum_wage
                + self.tax_16_holiday_pay
                + self.tax_17_13th_month_pay
                + self.tax_18_de_minimis
                + self.tax_19_sss_gsis
                + self.tax_20_other_amount,
        );

        // Line 22 = 14 - 21
        self.tax_22_total_taxable =
            round_currency(self.tax_14_total_compensation - self.tax_21_total_non_taxable);

        // Line 24 = 22 - 23
        self.tax_24_net_taxable =
            round_currency(self.tax_22_total_taxable - self.tax_23_not_subject);

        // Line 27 = 25 + 26
        self.tax_27_taxes_withheld_for_remittance =
            round_currency(self.tax_25_total_taxes_withheld + self.tax_26_adjustment);

        // Line 30 = 28 + 29
        self.tax_30_total_tax_remittances = round_currency(
            self.tax_28_tax_remitted_previously + self.tax_29_other_remittances_amount,
        );

        // Line 31 = 27 - 30
        self.tax_31_tax_still_due = round_currency(
            self.tax_27_taxes_withheld_for_remittance - self.tax_30_total_tax_remittances,
        );

        // Legacy persisted switch: penalties are always computed now and the
        // per-component overrides decide what is filed.
        self.auto_compute_penalties = false;
        // Submitted, Confirmed and Paid returns keep the Items 32-34 they
        // were filed with. A Draft refreshes the computed values; a Queued
        // return refreshes them only at revalidation, where a change fails
        // closed.
        Self::penalty_items().apply(self);
        self.tax_32_surcharge = round_currency(self.tax_32_surcharge);
        self.tax_33_interest = round_currency(self.tax_33_interest);
        self.tax_34_compromise = round_currency(self.tax_34_compromise);

        // Line 35 = 32 + 33 + 34
        self.tax_35_total_penalties =
            round_currency(self.tax_32_surcharge + self.tax_33_interest + self.tax_34_compromise);

        // Line 36 = 31 + 35
        self.tax_36_total_amount_payable =
            round_currency(self.tax_31_tax_still_due + self.tax_35_total_penalties);

        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
}

impl FormValidator for Form1601CDraft {
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();

        let today = chrono::Local::now().date_naive();
        let current_year = today.year();
        let current_month = today.month();

        if !(1900..=9999).contains(&self.taxable_year) {
            errors.push((
                "taxable_year".to_string(),
                "Taxable year must be a 4-digit year".to_string(),
            ));
        } else if self.taxable_year < OFFICIAL_MIN_FORM_YEAR {
            errors.push((
                "taxable_year".to_string(),
                OFFICIAL_OLD_VERSION_MESSAGE.to_string(),
            ));
        } else if self.taxable_year as i32 > current_year {
            errors.push((
                "taxable_year".to_string(),
                "Invalid year. Year should not be later than the current year.".to_string(),
            ));
        }

        if !(1..=12).contains(&self.month) {
            errors.push((
                "month".to_string(),
                "Month must be between 1 and 12".to_string(),
            ));
        } else if self.taxable_year as i32 == current_year && self.month as u32 > current_month {
            errors.push((
                "month".to_string(),
                "Invalid month. Month should not be later than the current month.".to_string(),
            ));
        }

        // Official validate(): Item 3 is checked right after the period.
        if self.any_taxes_withheld_unanswered {
            errors.push((
                "any_taxes_withheld".to_string(),
                "Please select an option for Item 3.".to_string(),
            ));
        }

        if !valid_submission_tin(&self.tin) {
            errors.push((
                "tin".to_string(),
                "TIN must use 12 to 14 digits in either compact form or the reviewed 3-3-3-branch format"
                    .to_string(),
            ));
        }

        if self.email_address.trim().is_empty() {
            errors.push((
                "email_address".to_string(),
                "Email address is required for the reviewed IAF filename".to_string(),
            ));
        } else if !validate_email(&self.email_address) {
            errors.push((
                "email_address".to_string(),
                "Email address is invalid".to_string(),
            ));
        }

        if self.rdo_code.trim().is_empty() {
            errors.push(("rdo_code".to_string(), "RDO is required".to_string()));
        } else if !rdo_code_is_official_option(&self.rdo_code) {
            // `1601c-validate-027`: official rejects the `'000'` placeholder
            // (`selectedIndex == 0`); the dropdown itself limits every other
            // value to rdo.xml, which free text must enforce here.
            errors.push((
                "rdo_code".to_string(),
                OFFICIAL_INVALID_RDO_MESSAGE.to_string(),
            ));
        }

        if self.taxpayer_name.trim().is_empty() {
            errors.push(("taxpayer_name".to_string(), "Name is required".to_string()));
        } else if !fits_official_maxlength(&self.taxpayer_name, 50) {
            errors.push((
                "taxpayer_name".to_string(),
                "Name exceeds the official 50-character field".to_string(),
            ));
        }

        if self.registered_address.trim().is_empty() {
            errors.push((
                "registered_address".to_string(),
                "Address is required".to_string(),
            ));
        } else if !fits_official_maxlength(&self.registered_address, 100) {
            errors.push((
                "registered_address".to_string(),
                "Address exceeds the official 100-character field".to_string(),
            ));
        }
        if !fits_official_maxlength(&self.registered_address_2, 50) {
            errors.push((
                "registered_address_2".to_string(),
                "Address (line 2) exceeds the official 50-character field".to_string(),
            ));
        }

        if !validate_zip(&self.zip_code) {
            errors.push((
                "zip_code".to_string(),
                "Valid ZIP Code required".to_string(),
            ));
        }

        if !validate_ph_phone(&self.contact_number) {
            errors.push((
                "contact_number".to_string(),
                "Valid Philippine phone number required".to_string(),
            ));
        } else if !fits_official_maxlength(&self.contact_number, 20) {
            errors.push((
                "contact_number".to_string(),
                "Telephone number exceeds the official 20-character field".to_string(),
            ));
        }

        if !fits_official_maxlength(&self.line_of_business, 60) {
            errors.push((
                "line_of_business".to_string(),
                "Line of business exceeds the official 60-character field".to_string(),
            ));
        }

        if !fits_official_maxlength(&self.email_address, 60) {
            errors.push((
                "email_address".to_string(),
                "Email address exceeds the official 60-character field".to_string(),
            ));
        }

        // eFPS Parity Validations
        if self.category_of_agent != "P" && self.category_of_agent != "G" {
            errors.push((
                "category_of_agent".to_string(),
                "Please select an option for Item 11.".to_string(),
            ));
        }

        if self.atc != FORM_1601C_ATC {
            errors.push((
                "atc".to_string(),
                format!("Item 5 ATC must be {FORM_1601C_ATC} for BIR Form 1601-C January 2018"),
            ));
        }

        if self.number_of_sheets > 99 {
            errors.push((
                "number_of_sheets".to_string(),
                "Number of sheets must fit the official two-digit field".to_string(),
            ));
        }

        if self.tax_relief {
            if self.tax_relief_specification.trim().is_empty() {
                errors.push((
                    "tax_relief_specification".to_string(),
                    "Item 13A is required when payees avail of tax relief".to_string(),
                ));
            } else if official_tax_relief_code(&self.tax_relief_specification, true).is_none() {
                errors.push((
                    "tax_relief_specification".to_string(),
                    "Item 13A must be one of the official options: 1 (Special Rate), 2 (International Tax Treaty), or 3 (Both)".to_string(),
                ));
            }
        }

        // `1601c-input-011`. Profile: app-stricter — official disables Item 28
        // and silently resets it to 0.00 on a non-amended return; we flag the
        // stale value instead of discarding it.
        if !self.is_amended && self.tax_28_tax_remitted_previously.abs() >= 0.005 {
            errors.push((
                "tax_28_tax_remitted_previously".to_string(),
                "Item 28 accepts entries only on an amended return".to_string(),
            ));
        }

        if self.schedule_1.len() > MAX_SCHEDULE_1_ROWS {
            errors.push((
                "schedule_1".to_string(),
                format!(
                    "Schedule I supports at most {MAX_SCHEDULE_1_ROWS} rows in the verified 1601-C XML contract"
                ),
            ));
        }

        for (index, row) in self.schedule_1.iter().enumerate() {
            // `1601c-validate-047`: the official validate inspects a Schedule I
            // date only when that field is nonblank and never requires row
            // completeness. The recommended app behavior requires every column
            // once a row carries any value, so a fully blank row is skipped.
            // Profile: app-stricter — diverges from official, which accepts
            // partially filled rows.
            let row_has_data = !row.previous_month.trim().is_empty()
                || !row.date_paid.trim().is_empty()
                || !row.drawee_bank_code_or_agency.trim().is_empty()
                || !row.payment_number.trim().is_empty()
                || row.tax_paid != 0.0
                || row.should_be_tax_due != 0.0
                || row.adjustment != 0.0;
            if !row_has_data {
                continue;
            }
            let field = format!("schedule_1_row_{}", index + 1);
            if !valid_month_year(&row.previous_month) {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule I row {} requires month in MM/YYYY format",
                        index + 1
                    ),
                ));
            } else if month_year_in_future(&row.previous_month, today) {
                errors.push((
                    field.clone(),
                    format!("Schedule I row {} month cannot be a future date", index + 1),
                ));
            }
            if !valid_calendar_date(&row.date_paid) {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule I row {} requires a valid MM/DD/YYYY date paid",
                        index + 1
                    ),
                ));
            } else if calendar_date_in_future(&row.date_paid, today) {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule I row {} date paid cannot be a future date",
                        index + 1
                    ),
                ));
            }
            if row.drawee_bank_code_or_agency.trim().is_empty() {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule I row {} requires a drawee bank/code or agency",
                        index + 1
                    ),
                ));
            }
            if row.payment_number.trim().is_empty() {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule I row {} requires a payment reference number",
                        index + 1
                    ),
                ));
            }
            if !row.tax_paid.is_finite() || row.tax_paid < 0.0 {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule I row {} tax paid must be a non-negative amount",
                        index + 1
                    ),
                ));
            }
            if !row.should_be_tax_due.is_finite() || row.should_be_tax_due < 0.0 {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule I row {} tax that should be due must be a non-negative amount",
                        index + 1
                    ),
                ));
            }
            let expected_adjustment = round_currency(row.should_be_tax_due - row.tax_paid);
            validate_computed_money(
                &mut errors,
                &field,
                row.adjustment,
                expected_adjustment,
                &format!(
                    "Schedule I row {} adjustment must equal Item 6 less Item 5",
                    index + 1
                ),
            );
        }

        let expected_schedule_total =
            round_currency(self.schedule_1.iter().map(|row| row.adjustment).sum());
        validate_computed_money(
            &mut errors,
            "tax_26_adjustment",
            self.tax_26_adjustment,
            expected_schedule_total,
            "Item 26 must equal the total of Schedule I adjustments",
        );

        for (field, value) in [
            ("tax_14_total_compensation", self.tax_14_total_compensation),
            (
                "tax_15_statutory_minimum_wage",
                self.tax_15_statutory_minimum_wage,
            ),
            ("tax_16_holiday_pay", self.tax_16_holiday_pay),
            ("tax_17_13th_month_pay", self.tax_17_13th_month_pay),
            ("tax_18_de_minimis", self.tax_18_de_minimis),
            ("tax_19_sss_gsis", self.tax_19_sss_gsis),
            ("tax_20_other_amount", self.tax_20_other_amount),
            ("tax_23_not_subject", self.tax_23_not_subject),
            (
                "tax_25_total_taxes_withheld",
                self.tax_25_total_taxes_withheld,
            ),
            (
                "tax_28_tax_remitted_previously",
                self.tax_28_tax_remitted_previously,
            ),
            (
                "tax_29_other_remittances_amount",
                self.tax_29_other_remittances_amount,
            ),
            ("tax_32_surcharge", self.tax_32_surcharge),
            ("tax_33_interest", self.tax_33_interest),
            ("tax_34_compromise", self.tax_34_compromise),
        ] {
            validate_non_negative_money(&mut errors, field, value);
        }

        // Items 32-34 must be the used values: the override when set, else the
        // computed value shown to the user.
        if self.penalty.penalty_model_version >= PENALTY_MODEL_VERSION
            && matches!(self.status, FilingStatus::Draft | FilingStatus::Queued)
        {
            for ((field, actual), expected) in [
                ("tax_32_surcharge", self.tax_32_surcharge),
                ("tax_33_interest", self.tax_33_interest),
                ("tax_34_compromise", self.tax_34_compromise),
            ]
            .into_iter()
            .zip(self.used_penalties())
            {
                validate_computed_money(
                    &mut errors,
                    field,
                    actual,
                    expected,
                    "Penalty must equal its override, or the computed value when not overridden",
                );
            }
        }

        let expected_21 = round_currency(
            self.tax_15_statutory_minimum_wage
                + self.tax_16_holiday_pay
                + self.tax_17_13th_month_pay
                + self.tax_18_de_minimis
                + self.tax_19_sss_gsis
                + self.tax_20_other_amount,
        );
        let expected_22 = round_currency(self.tax_14_total_compensation - expected_21);
        let expected_24 = round_currency(expected_22 - self.tax_23_not_subject);
        let expected_27 =
            round_currency(self.tax_25_total_taxes_withheld + expected_schedule_total);
        let expected_30 = round_currency(
            self.tax_28_tax_remitted_previously + self.tax_29_other_remittances_amount,
        );
        let expected_31 = round_currency(expected_27 - expected_30);
        let expected_35 =
            round_currency(self.tax_32_surcharge + self.tax_33_interest + self.tax_34_compromise);
        let expected_36 = round_currency(expected_31 + expected_35);

        for (field, actual, expected, message) in [
            (
                "tax_21_total_non_taxable",
                self.tax_21_total_non_taxable,
                expected_21,
                "Item 21 must equal the sum of Items 15 through 20",
            ),
            (
                "tax_22_total_taxable",
                self.tax_22_total_taxable,
                expected_22,
                "Item 22 must equal Item 14 less Item 21",
            ),
            (
                "tax_24_net_taxable",
                self.tax_24_net_taxable,
                expected_24,
                "Item 24 must equal Item 22 less Item 23",
            ),
            (
                "tax_27_taxes_withheld_for_remittance",
                self.tax_27_taxes_withheld_for_remittance,
                expected_27,
                "Item 27 must equal Item 25 plus Item 26",
            ),
            (
                "tax_30_total_tax_remittances",
                self.tax_30_total_tax_remittances,
                expected_30,
                "Item 30 must equal Item 28 plus Item 29",
            ),
            (
                "tax_31_tax_still_due",
                self.tax_31_tax_still_due,
                expected_31,
                "Item 31 must equal Item 27 less Item 30",
            ),
            (
                "tax_35_total_penalties",
                self.tax_35_total_penalties,
                expected_35,
                "Item 35 must equal the sum of Items 32 through 34",
            ),
            (
                "tax_36_total_amount_payable",
                self.tax_36_total_amount_payable,
                expected_36,
                "Item 36 must equal Item 31 plus Item 35",
            ),
        ] {
            validate_computed_money(&mut errors, field, actual, expected, message);
        }

        if self.taxes_withheld_answer() == Some(true) {
            if self.tax_14_total_compensation <= 0.0 {
                errors.push((
                    "tax_14_total_compensation".to_string(),
                    "Invalid amount in Item no.14. Value must be greater than zero(0).".to_string(),
                ));
            }
            if self.tax_25_total_taxes_withheld <= 0.0 {
                errors.push((
                    "tax_25_total_taxes_withheld".to_string(),
                    "Invalid amount in Item no.25. Value must be greater than zero(0).".to_string(),
                ));
            }
        }

        errors
    }
}

fn validate_non_negative_money(errors: &mut Vec<(String, String)>, field: &str, value: f64) {
    if !value.is_finite() || value < 0.0 {
        errors.push((
            field.to_string(),
            format!("{field} must be a finite, non-negative amount"),
        ));
    }
}

fn validate_computed_money(
    errors: &mut Vec<(String, String)>,
    field: &str,
    actual: f64,
    expected: f64,
    mismatch_message: &str,
) {
    if !actual.is_finite() || !expected.is_finite() {
        errors.push((
            field.to_string(),
            format!("{field} must be a finite computed amount"),
        ));
    } else if (actual - expected).abs() > 0.001 {
        errors.push((field.to_string(), mismatch_message.to_string()));
    }
}

fn valid_month_year(value: &str) -> bool {
    value.len() == 7
        && value.as_bytes().get(2) == Some(&b'/')
        && value
            .chars()
            .enumerate()
            .all(|(index, ch)| index == 2 || ch.is_ascii_digit())
        && chrono::NaiveDate::parse_from_str(&format!("01/{value}"), "%d/%m/%Y").is_ok()
}

fn valid_calendar_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes().get(2) == Some(&b'/')
        && value.as_bytes().get(5) == Some(&b'/')
        && value
            .chars()
            .enumerate()
            .all(|(index, ch)| matches!(index, 2 | 5) || ch.is_ascii_digit())
        && chrono::NaiveDate::parse_from_str(value, "%m/%d/%Y").is_ok()
}

/// Mirrors the official `validateMonthYear` mask: MM/YYYY must not exceed the
/// filing machine's current month.
fn month_year_in_future(value: &str, today: chrono::NaiveDate) -> bool {
    chrono::NaiveDate::parse_from_str(&format!("01/{value}"), "%d/%m/%Y")
        .is_ok_and(|date| (date.year(), date.month()) > (today.year(), today.month()))
}

/// Mirrors the official `validateDate` mask: MM/DD/YYYY must not be a future date.
fn calendar_date_in_future(value: &str, today: chrono::NaiveDate) -> bool {
    chrono::NaiveDate::parse_from_str(value, "%m/%d/%Y").is_ok_and(|date| date > today)
}

fn valid_submission_tin(value: &str) -> bool {
    let value = value.trim();
    if value.chars().all(|ch| ch.is_ascii_digit()) {
        return (12..=14).contains(&value.len());
    }

    let mut segments = value.split('-');
    let first = segments.next();
    let second = segments.next();
    let third = segments.next();
    let branch = segments.next();
    if segments.next().is_some() {
        return false;
    }

    matches!((first, second, third, branch), (Some(first), Some(second), Some(third), Some(branch))
        if [first, second, third]
            .into_iter()
            .all(|segment| segment.len() == 3 && segment.chars().all(|ch| ch.is_ascii_digit()))
            && (3..=5).contains(&branch.len())
            && branch.chars().all(|ch| ch.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{EoptTier, TaxpayerProfile};

    fn test_profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Test Withholding Agent",
            "tin": {
                "segment1": "123",
                "segment2": "456",
                "segment3": "789",
                "branch": "00000"
            },
            "rdo_code": "018",
            "line_of_business": "Software Development",
            "registered_address": "Olongapo",
            "zip_code": "2200",
            "phone": "09123456789",
            "email": "tax@example.com",
            "default_form_type": "1601Cv2018",
            "taxpayer_type": "Corporation"
        }))
        .expect("test profile must deserialize")
    }

    fn schedule_row(month: &str, paid: f64, due: f64) -> Form1601CSchedule1Row {
        Form1601CSchedule1Row {
            previous_month: month.to_string(),
            date_paid: "05/10/2026".to_string(),
            drawee_bank_code_or_agency: "AAB-001".to_string(),
            payment_number: "REF-001".to_string(),
            tax_paid: paid,
            should_be_tax_due: due,
            adjustment: 0.0,
        }
    }

    #[test]
    fn schedule_adjustments_drive_item_26_and_item_27() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.auto_compute_penalties = false;
        draft.tax_25_total_taxes_withheld = 1_000.0;
        draft.schedule_1 = vec![
            schedule_row("04/2026", 900.25, 1_000.5),
            schedule_row("05/2026", 500.0, 450.0),
        ];

        draft.compute();

        assert_eq!(draft.schedule_1[0].adjustment, 100.25);
        assert_eq!(draft.schedule_1[1].adjustment, -50.0);
        assert_eq!(draft.tax_26_adjustment, 50.25);
        assert_eq!(draft.tax_27_taxes_withheld_for_remittance, 1_050.25);
    }

    #[test]
    fn validation_rejects_rows_beyond_verified_schedule_capacity() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        draft.schedule_1 = vec![schedule_row("05/2026", 1.0, 1.0); 4];
        draft.compute();

        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "schedule_1")
        );
    }

    #[test]
    fn schedule_dates_require_exact_official_formats() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        let mut row = schedule_row("5/2026", 1.0, 1.0);
        row.date_paid = "5/1/2026".to_string();
        draft.schedule_1 = vec![row];
        draft.compute();

        let row_errors = draft
            .validate()
            .into_iter()
            .filter(|(field, _)| field == "schedule_1_row_1")
            .count();
        assert_eq!(row_errors, 2);
    }

    #[test]
    fn item_13_requires_specification_only_when_selected() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        draft.tax_relief = true;

        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "tax_relief_specification")
        );

        draft.tax_relief_specification = "International Tax Treaty".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "tax_relief_specification")
        );
    }

    #[test]
    fn validation_rejects_contact_numbers_beyond_the_official_twelve_digit_field() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        draft.contact_number = "6391234567890".to_string();

        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "contact_number")
        );
    }

    #[test]
    fn queue_validation_rejects_malformed_tin_before_fingerprinting() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        draft.tin = "123-456-789-00X00".to_string();

        let errors = draft
            .transition_to_queued()
            .expect_err("a malformed TIN must not enter the submission queue");

        assert!(errors.iter().any(|(field, _)| field == "tin"));
        assert_eq!(draft.status, FilingStatus::Draft);
        assert!(draft.queued_submission_fingerprint.is_none());
    }

    #[test]
    fn queue_validation_accepts_reviewed_compact_and_segmented_tins() {
        for tin in ["123456789000", "12345678900000", "123-456-789-00000"] {
            let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
            draft.any_taxes_withheld = false;
            draft.tin = tin.to_string();

            draft
                .transition_to_queued()
                .unwrap_or_else(|errors| panic!("{tin} should be accepted: {errors:?}"));
        }
    }

    #[test]
    fn new_returns_start_with_items_3_and_11_unanswered_like_the_official_page() {
        let mut draft = Form1601CDraft::new_unanswered_from_profile(&test_profile(), 2026, 6);
        assert_eq!(draft.taxes_withheld_answer(), None);
        assert_eq!(draft.category_of_agent, "");
        draft.compute();
        let errors = draft.validate();
        let item_3 = errors
            .iter()
            .position(|(field, message)| {
                field == "any_taxes_withheld" && message == "Please select an option for Item 3."
            })
            .expect("Item 3 alert");
        let item_11 = errors
            .iter()
            .position(|(field, message)| {
                field == "category_of_agent" && message == "Please select an option for Item 11."
            })
            .expect("Item 11 alert");
        assert!(item_3 < item_11, "validate() checks Item 3 before Item 11");
        // Unanswered is not Yes: the Item 14/25 alerts stay quiet.
        assert!(
            errors
                .iter()
                .all(|(_, message)| !message.contains("Item no.14"))
        );
        // Both radios unchecked in the field map, and it reads back unanswered.
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1601c:TaxWithheld_1"], "false");
        assert_eq!(fields["frm1601c:TaxWithheld_2"], "false");
        assert_eq!(fields["frm1601c:CatAgent_P"], "false");
        assert_eq!(fields["frm1601c:CatAgent_G"], "false");

        draft.set_any_taxes_withheld(false);
        draft.category_of_agent = "P".to_string();
        let errors = draft.validate();
        assert!(
            errors
                .iter()
                .all(|(field, _)| field != "any_taxes_withheld" && field != "category_of_agent")
        );
        assert_eq!(draft.taxes_withheld_answer(), Some(false));
    }

    #[test]
    fn drafts_saved_before_the_unanswered_state_load_answered() {
        let draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        let mut json = serde_json::to_value(&draft).unwrap();
        assert!(json.get("any_taxes_withheld_unanswered").is_none());
        json.as_object_mut()
            .unwrap()
            .remove("any_taxes_withheld_unanswered");
        let loaded: Form1601CDraft = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.taxes_withheld_answer(), Some(true));
        assert_eq!(loaded.category_of_agent, "P");
    }

    #[test]
    fn validation_requires_the_official_fixed_item_5_atc_and_item_11_category() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        assert_eq!(draft.atc, FORM_1601C_ATC);
        assert_eq!(draft.category_of_agent, "P");
        draft.compute();
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "category_of_agent")
        );

        draft.atc = "WC010".to_string();
        draft.category_of_agent.clear();
        let errors = draft.validate();

        assert!(errors.iter().any(|(field, message)| {
            field == "atc" && message.contains("Item 5 ATC must be WW010")
        }));
        assert!(errors.iter().any(|(field, message)| {
            field == "category_of_agent" && message.contains("Item 11")
        }));
        assert!(
            errors
                .iter()
                .all(|(_, message)| !message.contains("Item 12"))
        );
    }

    #[test]
    fn government_category_validates_and_maps_exclusive_xml_xboxes() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        draft.category_of_agent = "G".to_string();
        draft.compute();

        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "category_of_agent")
        );
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1601c:CatAgent_P"], "false");
        assert_eq!(fields["frm1601c:CatAgent_G"], "true");
    }

    #[test]
    fn validation_rejects_stale_totals_and_non_finite_inputs() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        draft.auto_compute_penalties = false;
        draft.tax_14_total_compensation = 10.0;
        draft.tax_15_statutory_minimum_wage = 1.0;
        draft.compute();

        draft.tax_15_statutory_minimum_wage = 2.0;
        draft.tax_29_other_remittances_amount = f64::NAN;

        let errors = draft.validate();
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "tax_21_total_non_taxable")
        );
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "tax_29_other_remittances_amount")
        );
    }

    #[test]
    fn legacy_manual_penalties_load_as_overrides_and_keep_their_totals() {
        // Before the computed-default model, Items 32-34 were typed by hand.
        // A draft saved then (no `penalty_model_version`) keeps every
        // non-zero hand-typed value as an override, so nothing it already
        // carried changes value, even though the period is late.
        let mut current = Form1601CDraft::new_from_profile(&test_profile(), 2020, 1);
        current.auto_compute_penalties = true;
        current.tax_25_total_taxes_withheld = 1_000.0;
        current.tax_32_surcharge = 125.25;
        current.tax_33_interest = 31.5;
        current.tax_34_compromise = 1_000.0;
        let mut json = serde_json::to_value(&current).unwrap();
        let object = json.as_object_mut().unwrap();
        for key in [
            "penalty_model_version",
            "penalty_due_date",
            "penalty_taxpayer_class",
            "computed_penalties",
            "penalty_overrides",
        ] {
            object.remove(key);
        }
        let mut draft: Form1601CDraft = serde_json::from_value(json).unwrap();
        assert_eq!(draft.penalty.penalty_model_version, 0);

        draft.compute_on(day(2026, 10, 13), true);

        assert!(!draft.auto_compute_penalties);
        assert_eq!(draft.penalty.penalty_model_version, PENALTY_MODEL_VERSION);
        assert_eq!(draft.penalty.penalty_overrides.surcharge, Some(125.25));
        assert_eq!(draft.penalty.penalty_overrides.interest, Some(31.5));
        assert_eq!(draft.penalty.penalty_overrides.compromise, Some(1_000.0));
        assert_eq!(draft.tax_32_surcharge, 125.25);
        assert_eq!(draft.tax_33_interest, 31.5);
        assert_eq!(draft.tax_34_compromise, 1_000.0);
        assert_eq!(draft.tax_35_total_penalties, 1_156.75);
        assert_eq!(draft.tax_36_total_amount_payable, 2_156.75);
        assert!(
            draft
                .penalty
                .computed_penalties
                .as_ref()
                .is_some_and(|computed| computed.total() > 0.0),
            "the computed values are still shown next to the kept overrides"
        );

        let before = (
            draft.tax_32_surcharge,
            draft.tax_33_interest,
            draft.tax_34_compromise,
            draft.tax_35_total_penalties,
            draft.tax_36_total_amount_payable,
        );
        draft.compute_on(day(2026, 10, 14), true);
        assert_eq!(
            before,
            (
                draft.tax_32_surcharge,
                draft.tax_33_interest,
                draft.tax_34_compromise,
                draft.tax_35_total_penalties,
                draft.tax_36_total_amount_payable,
            )
        );
    }

    #[test]
    fn legacy_zero_penalties_are_not_overrides() {
        let current = Form1601CDraft::new_from_profile(&test_profile(), 2026, 8);
        let mut json = serde_json::to_value(&current).unwrap();
        json.as_object_mut()
            .unwrap()
            .remove("penalty_model_version");
        let mut draft: Form1601CDraft = serde_json::from_value(json).unwrap();
        draft.tax_25_total_taxes_withheld = 20_000.0;
        draft.tax_32_surcharge = 0.0;
        draft.tax_33_interest = 31.5;

        draft.compute_on(day(2026, 10, 13), true);

        assert_eq!(draft.penalty.penalty_overrides.surcharge, None);
        assert_eq!(draft.penalty.penalty_overrides.interest, Some(31.5));
        assert_eq!(draft.penalty.penalty_overrides.compromise, None);
        assert_eq!(draft.tax_32_surcharge, 5_000.0);
        assert_eq!(draft.tax_33_interest, 31.5);
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// Test taxpayer `111-111-114-00000` (a dummy TIN, never real data).
    fn dummy_tin_profile() -> TaxpayerProfile {
        let mut profile = test_profile();
        profile.tin = serde_json::from_value(serde_json::json!({
            "segment1": "111",
            "segment2": "111",
            "segment3": "114",
            "branch": "00000"
        }))
        .unwrap();
        profile
    }

    /// August 2026 return with Item 31 = PHP 20,000 still due. The app's
    /// resolver puts its deadline on Thursday 10 September 2026.
    fn late_august_2026_draft() -> Form1601CDraft {
        let mut draft = Form1601CDraft::new_from_profile(&dummy_tin_profile(), 2026, 8);
        draft.tax_14_total_compensation = 100_000.0;
        draft.tax_25_total_taxes_withheld = 20_000.0;
        draft
    }

    /// Worked example, filed 13 October 2026 (33 days after the 10 September
    /// 2026 deadline), Item 31 = PHP 20,000, standard (non-micro/small) class:
    ///
    /// * Item 32 surcharge, NIRC Sec. 248(A): 25% x 20,000 = 5,000.00
    /// * Item 33 interest, NIRC Sec. 249(B) as amended by RA 10963 (TRAIN,
    ///   12% a year from 1 January 2018): 20,000 x 12% x 33/365 = 216.99
    /// * Item 34 compromise, RMO 7-2015 Annex A (the schedule `compromise.rs`
    ///   cites): unpaid tax over 10,000 up to 20,000 = 5,000.00
    ///
    /// Item 35 = 10,216.99 and Item 36 = 20,000 + 10,216.99 = 30,216.99.
    #[test]
    fn late_1601c_computes_penalties_from_the_configured_rates() {
        let mut draft = late_august_2026_draft();
        assert_eq!(draft.tin, "11111111400000");
        assert_eq!(draft.penalty_due_date(), Some(day(2026, 9, 10)));

        draft.compute_on(day(2026, 10, 13), true);

        let computed = draft
            .penalty
            .computed_penalties
            .clone()
            .expect("a late return with tax due has computed penalties");
        assert_eq!(computed.inputs.due_date, day(2026, 9, 10));
        assert_eq!(computed.inputs.filing_date, day(2026, 10, 13));
        assert_eq!(computed.inputs.payment_date, day(2026, 10, 13));
        assert_eq!(computed.inputs.days_late, 33);
        assert_eq!(computed.inputs.basic_tax_due, 20_000.0);
        assert_eq!(computed.inputs.taxpayer_class, TaxpayerClass::Regular);
        assert!(!computed.inputs.class_from_profile);
        assert_eq!(computed.inputs.schedule, PenaltyProfile::Withholding);
        assert_eq!(
            (computed.surcharge, computed.interest, computed.compromise),
            (5_000.0, 216.99, 5_000.0)
        );
        assert_eq!(draft.tax_32_surcharge, 5_000.0);
        assert_eq!(draft.tax_33_interest, 216.99);
        assert_eq!(draft.tax_34_compromise, 5_000.0);
        assert_eq!(draft.tax_35_total_penalties, 10_216.99);
        assert_eq!(draft.tax_36_total_amount_payable, 30_216.99);
        assert!(draft.validate().is_empty(), "{:?}", draft.validate());
    }

    /// EOPT (RA 11976) relief for micro and small taxpayers, RR 6-2024, as
    /// `PenaltyConfig` carries it for deadlines from 22 January 2024: 10%
    /// surcharge, 6% interest and half the RMO 7-2015 compromise.
    #[test]
    fn micro_profile_uses_the_eopt_reduced_rates() {
        let mut profile = dummy_tin_profile();
        profile.eopt_tier = Some(EoptTier::Micro);
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 8);
        draft.tax_14_total_compensation = 100_000.0;
        draft.tax_25_total_taxes_withheld = 20_000.0;

        draft.compute_on(day(2026, 10, 13), true);

        let computed = draft.penalty.computed_penalties.clone().unwrap();
        assert_eq!(computed.inputs.taxpayer_class, TaxpayerClass::Micro);
        assert!(computed.inputs.class_from_profile);
        // 20,000 x 10%; 20,000 x 6% x 33/365 = 108.49; 5,000 x 50%.
        assert_eq!(
            (computed.surcharge, computed.interest, computed.compromise),
            (2_000.0, 108.49, 2_500.0)
        );
        assert_eq!(draft.tax_35_total_penalties, 4_608.49);
    }

    #[test]
    fn overriding_one_component_changes_only_that_component() {
        let mut computed_only = late_august_2026_draft();
        computed_only.compute_on(day(2026, 10, 13), true);
        let computed_fields = computed_only.to_bir_field_map();
        let computed_fingerprint = computed_only.submission_fingerprint();

        let mut draft = late_august_2026_draft();
        draft.set_penalty_override(PenaltyComponent::Compromise, Some(500.0));
        draft.penalty.penalty_overrides.note = "RDO 018 approval ref. 2026-0042".to_string();
        draft.compute_on(day(2026, 10, 13), true);

        let computed = draft.penalty.computed_penalties.clone().unwrap();
        assert_eq!(computed.compromise, 5_000.0, "the computed value is kept");
        assert_eq!(draft.tax_32_surcharge, 5_000.0);
        assert_eq!(draft.tax_33_interest, 216.99);
        assert_eq!(draft.tax_34_compromise, 500.0);
        assert_eq!(draft.tax_35_total_penalties, 5_716.99);
        assert_eq!(draft.tax_36_total_amount_payable, 25_716.99);
        assert!(draft.validate().is_empty(), "{:?}", draft.validate());

        let amount = crate::official_xml::official_amount;
        let fields = draft.to_bir_field_map();
        assert_eq!(
            fields["frm1601c:txtTax32"],
            computed_fields["frm1601c:txtTax32"]
        );
        assert_eq!(
            fields["frm1601c:txtTax33"],
            computed_fields["frm1601c:txtTax33"]
        );
        assert_eq!(fields["frm1601c:txtTax34"], amount(500.0));
        assert_eq!(fields["frm1601c:txtTax35"], amount(5_716.99));
        assert_eq!(fields["frm1601c:txtTax36"], amount(25_716.99));
        assert_ne!(draft.submission_fingerprint(), computed_fingerprint);

        let restored: Form1601CDraft =
            serde_json::from_str(&serde_json::to_string(&draft).unwrap()).unwrap();
        assert_eq!(
            restored.penalty.penalty_overrides,
            draft.penalty.penalty_overrides
        );
    }

    #[test]
    fn clearing_an_override_returns_to_the_computed_value() {
        let mut draft = late_august_2026_draft();
        draft.set_penalty_override(PenaltyComponent::Surcharge, Some(1_000.0));
        draft.compute_on(day(2026, 10, 13), true);
        assert_eq!(draft.tax_32_surcharge, 1_000.0);

        draft.set_penalty_override(PenaltyComponent::Surcharge, None);
        draft.compute_on(day(2026, 10, 13), true);

        assert_eq!(draft.tax_32_surcharge, 5_000.0);
        assert_eq!(draft.tax_35_total_penalties, 10_216.99);
    }

    #[test]
    fn on_time_1601c_computes_no_penalties() {
        let mut draft = late_august_2026_draft();
        draft.compute_on(day(2026, 9, 10), true);

        let computed = draft.penalty.computed_penalties.clone().unwrap();
        assert!(computed.filed_on_time());
        assert_eq!(computed.total(), 0.0);
        assert_eq!(draft.tax_35_total_penalties, 0.0);
        assert_eq!(draft.tax_36_total_amount_payable, 20_000.0);
    }

    #[test]
    fn resolved_deadline_from_the_app_drives_the_computation() {
        // A deadline override (e.g. an RMC extension) moves the due date.
        let mut draft = late_august_2026_draft();
        draft.penalty.penalty_due_date = Some(day(2026, 10, 13));
        draft.compute_on(day(2026, 10, 13), true);
        assert_eq!(
            draft
                .penalty
                .computed_penalties
                .as_ref()
                .map(|c| c.inputs.due_date),
            Some(day(2026, 10, 13))
        );
        assert_eq!(draft.tax_35_total_penalties, 0.0);
    }

    #[test]
    fn filed_return_never_recomputes_penalties() {
        let mut draft = late_august_2026_draft();
        draft.compute_on(day(2026, 10, 13), true);
        let filed = (
            draft.tax_32_surcharge,
            draft.tax_33_interest,
            draft.tax_34_compromise,
            draft.tax_35_total_penalties,
            draft.penalty.computed_penalties.clone(),
        );
        assert!(filed.3 > 0.0);
        for status in [
            FilingStatus::Sent,
            FilingStatus::Confirmed,
            FilingStatus::Paid,
        ] {
            draft.status = status;
            draft.compute();
            assert_eq!(
                filed,
                (
                    draft.tax_32_surcharge,
                    draft.tax_33_interest,
                    draft.tax_34_compromise,
                    draft.tax_35_total_penalties,
                    draft.penalty.computed_penalties.clone(),
                )
            );
        }
    }

    #[test]
    fn queue_revalidation_fails_closed_when_computed_penalties_change() {
        let mut draft = late_august_2026_draft();
        draft
            .transition_to_queued_on(day(2026, 10, 13))
            .expect("the late return should queue with computed penalties");
        assert_eq!(draft.tax_33_interest, 216.99);

        // Submission happens a day later: the computed interest moved.
        let errors = draft
            .revalidate_queued_before_submission_on(day(2026, 10, 14))
            .expect_err("changed computed penalties must require review");

        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "computed_penalties")
        );
        assert_eq!(draft.status, FilingStatus::Draft);
    }

    #[test]
    fn queue_revalidation_keeps_overridden_penalties() {
        let mut draft = late_august_2026_draft();
        draft.set_penalty_override(PenaltyComponent::Interest, Some(200.0));
        draft
            .transition_to_queued_on(day(2026, 10, 13))
            .expect("the late return should queue");

        draft
            .revalidate_queued_before_submission_on(day(2026, 10, 14))
            .expect("overridden components do not move with the clock");
        assert_eq!(draft.status, FilingStatus::Queued);
        assert_eq!(draft.tax_33_interest, 200.0);
    }

    #[test]
    fn imported_xml_penalties_are_kept_as_overrides() {
        let mut draft = late_august_2026_draft();
        for component in PenaltyComponent::ALL {
            draft.set_penalty_override(component, Some(0.0));
        }
        draft.compute();
        let fields = draft.to_bir_field_map();

        let imported =
            Form1601CDraft::from_bir_field_map(&fields).expect("a reviewed field map imports");
        assert_eq!(imported.tax_35_total_penalties, 0.0);
        assert_eq!(imported.penalty.penalty_overrides.surcharge, Some(0.0));
        assert_eq!(imported.to_bir_field_map(), fields);
    }

    #[test]
    fn queued_revalidate_requires_matching_authorization() {
        let mut draft = crate::filing_queue::mandatory_lab_1601c_draft();
        draft.transition_to_queued().expect("fixture must queue");
        draft.queue_authorization = None;
        let errors = draft
            .revalidate_queued_before_submission()
            .expect_err("missing grant must fail closed");
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "queue_authorization")
        );
        assert_eq!(draft.status, FilingStatus::Draft);
    }

    #[test]
    fn official_parity_rejects_out_of_range_periods() {
        let today = chrono::Local::now().date_naive();
        let current_year = today.year() as u16;

        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), current_year - 1, 6);
        draft.any_taxes_withheld = false;
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "taxable_year" && field != "month")
        );

        draft.taxable_year = 2017;
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "taxable_year")
        );

        draft.taxable_year = current_year + 1;
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "taxable_year")
        );

        draft.taxable_year = current_year;
        if today.month() < 12 {
            draft.month = today.month() as u8 + 1;
            assert!(draft.validate().iter().any(|(field, _)| field == "month"));
        } else {
            // December leaves no later month this year; the current month must pass.
            draft.month = 12;
            assert!(draft.validate().iter().all(|(field, _)| field != "month"));
        }
    }

    #[test]
    fn official_parity_rejects_placeholder_rdo_and_amended_item28() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;

        draft.rdo_code = "000".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "rdo_code")
        );

        // The official dropdown holds only rdo.xml codes, so free text
        // outside that list is rejected too.
        draft.rdo_code = "999".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "rdo_code")
        );

        draft.rdo_code = "018".to_string();
        draft.is_amended = false;
        draft.tax_28_tax_remitted_previously = 100.0;
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "tax_28_tax_remitted_previously")
        );

        draft.is_amended = true;
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "tax_28_tax_remitted_previously")
        );
    }

    #[test]
    fn official_parity_rejects_future_schedule_dates() {
        let today = chrono::Local::now().date_naive();
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        let mut row = schedule_row("01/2030", 1.0, 1.0);
        row.date_paid = "01/15/2030".to_string();
        draft.schedule_1 = vec![row];
        let errors = draft.validate();
        assert_eq!(
            errors
                .iter()
                .filter(|(field, _)| field == "schedule_1_row_1")
                .count(),
            2
        );

        let past_month = today
            .checked_sub_months(chrono::Months::new(1))
            .expect("one month ago exists");
        draft.schedule_1[0].previous_month = past_month.format("%m/%Y").to_string();
        draft.schedule_1[0].date_paid = today
            .checked_sub_days(chrono::Days::new(1))
            .expect("yesterday exists")
            .format("%m/%d/%Y")
            .to_string();
        assert!(
            draft
                .validate()
                .iter()
                .all(|(_, message)| !message.contains("future date"))
        );
    }

    #[test]
    fn official_parity_enforces_field_length_caps() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        draft.taxpayer_name = "N".repeat(51);
        draft.registered_address = "A".repeat(101);
        draft.registered_address_2 = "B".repeat(51);
        let errors = draft.validate();
        for field in [
            "taxpayer_name",
            "registered_address",
            "registered_address_2",
        ] {
            assert!(
                errors.iter().any(|(error_field, _)| error_field == field),
                "expected an error for {field}"
            );
        }
    }

    #[test]
    fn item_13_accepts_codes_and_labels_official_can_serialize() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = false;
        draft.tax_relief = true;

        for spec in [
            "1",
            "2",
            "3",
            "Special Rate",
            "International Tax Treaty",
            "Both",
        ] {
            draft.tax_relief_specification = spec.to_string();
            assert!(
                draft
                    .validate()
                    .iter()
                    .all(|(field, _)| field != "tax_relief_specification"),
                "spec {spec:?} should validate"
            );
        }

        draft.tax_relief_specification = "Special Law 123".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "tax_relief_specification")
        );
    }

    #[test]
    fn zero_items_14_and_25_use_the_official_alerts() {
        let mut draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 6);
        draft.any_taxes_withheld = true;
        draft.compute();
        let errors = draft.validate();
        let message = |field: &str| {
            errors
                .iter()
                .find(|(f, _)| f == field)
                .map(|(_, m)| m.as_str())
        };
        assert_eq!(
            message("tax_14_total_compensation"),
            Some("Invalid amount in Item no.14. Value must be greater than zero(0).")
        );
        assert_eq!(
            message("tax_25_total_taxes_withheld"),
            Some("Invalid amount in Item no.25. Value must be greater than zero(0).")
        );
    }
}
