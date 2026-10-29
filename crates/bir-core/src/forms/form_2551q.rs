//! BIR Form 2551Q (January 2018 ENCS) — Quarterly Percentage Tax Return
//!
//! Data model, carry-forward logic, and auto-computation.

use super::FilingStatus;
use super::official_inputs::has_cent_precision;
use crate::filing_queue::{QueueAuthSource, QueueAuthorization};
use crate::forms::atc::{AtcRateResolution, find_atc, resolve_2551q_atc_rate};
use crate::penalties::{
    PenaltyComponent, PenaltyConfig, PenaltyContext, PenaltyEngine, PenaltyOverrides,
    PenaltyProfile, TaxpayerClass,
};
use crate::profile::{IncomeTaxElection, TaxProfileVersionStatus, TaxpayerProfile, TaxpayerType};
use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

fn default_true() -> bool {
    true
}

fn default_year_end_month() -> u8 {
    12
}

fn fits_decimal_comb(value: f64, integer_cells: usize) -> bool {
    if !value.is_finite() {
        return false;
    }
    format!("{value:.2}")
        .split('.')
        .next()
        .is_some_and(|whole| whole.chars().count() <= integer_cells)
}

/// Values less than half a cent from Rust's cent-rounded result serialize to
/// the same two-decimal amount on the official return.
const TWO_DECIMAL_TOLERANCE: f64 = 0.005;
const ATC_RATE_TOLERANCE: f64 = 1e-12;

/// The reviewed eBIRForms XML contract exposes only `drpATC1..6` and their
/// matching amount/rate/due fields. Printable continuation sheets must never
/// be mistaken for additional XML capacity.
pub const FORM_2551Q_XML_SCHEDULE_ROW_CAPACITY: usize = 6;

/// Application-owned Schedule 1 continuation sheets retain twelve lines after
/// the repeated identity and column header. These sheets are printable
/// attachments only; they are not part of the reviewed eBIRForms XML payload.
pub const FORM_2551Q_SCHEDULE_CONTINUATION_ROW_CAPACITY: usize = 12;

fn round_to_cents(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn matches_cent_rounded(actual: f64, expected: f64) -> bool {
    actual.is_finite()
        && expected.is_finite()
        && (actual - round_to_cents(expected)).abs() < TWO_DECIMAL_TOLERANCE
}

/// Taxable-period basis selected in Item 1 of Form 2551Q.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaxPeriodBasis {
    #[default]
    Calendar,
    Fiscal,
}

/// Income-tax-rate election printed in Item 13 of Form 2551Q.
///
/// Older drafts deserialize to `Unanswered` so they cannot silently queue with
/// both official election boxes clear. Rust must not infer a legal election
/// from a taxpayer profile or from renderer layout.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Item13Election {
    #[default]
    Unanswered,
    NotApplicable,
    Graduated,
    EightPercent,
}

/// Annual income-tax election snapshot used to keep the quarterly Item 13
/// answer and Section 116 treatment consistent for the whole taxable year.
/// `None` on the draft means legacy JSON did not own a trustworthy snapshot.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnnualIncomeTaxElection {
    Unrecorded,
    Graduated,
    EightPercent,
    /// The profile contains both an 8% and a graduated election for the same
    /// taxable year. No return may choose between contradictory legal records.
    Conflicting,
}

pub(crate) fn annual_income_tax_election(
    profile: &TaxpayerProfile,
    year: u16,
) -> AnnualIncomeTaxElection {
    let elections = profile
        .tax_elections
        .iter()
        .filter(|history| history.taxable_year == year);
    let mut eight_percent = false;
    let mut graduated = false;
    for history in elections {
        match history.election {
            IncomeTaxElection::EightPercent => eight_percent = true,
            IncomeTaxElection::GraduatedUnspecified
            | IncomeTaxElection::GraduatedOsd
            | IncomeTaxElection::GraduatedItemized => {
                graduated = true;
            }
        }
    }
    match (eight_percent, graduated) {
        (true, true) => AnnualIncomeTaxElection::Conflicting,
        (true, false) => AnnualIncomeTaxElection::EightPercent,
        (false, true) => AnnualIncomeTaxElection::Graduated,
        (false, false) => AnnualIncomeTaxElection::Unrecorded,
    }
}

/// Requested disposition when Item 24 is an overpayment.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OverpaymentDisposition {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
}

/// One row in Schedule 1 — a single ATC category with its taxable amount.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule1Row {
    /// Alphanumeric Tax Code, e.g. "PT010"
    pub atc: String,
    /// Human-readable description, e.g. "Persons exempt from VAT [Sec. 116]"
    pub atc_description: String,
    /// User-entered gross receipts for this ATC category
    pub taxable_amount: f64,
    /// Tax rate, auto-filled from ATC table (e.g. 0.03 for 3%)
    pub tax_rate: f64,
    /// Computed: taxable_amount × tax_rate
    pub tax_due: f64,
}

impl Schedule1Row {
    /// Create a new row for a given ATC code. Returns None if code not in ATC table.
    pub fn new(atc_code: &str) -> Option<Self> {
        let entry = find_atc(atc_code)?;
        Some(Self {
            atc: entry.code.to_string(),
            atc_description: entry.description.to_string(),
            taxable_amount: 0.0,
            tax_rate: entry.rate,
            tax_due: 0.0,
        })
    }

    /// Create a default PT010 row.
    pub fn default_pt010() -> Self {
        Self::new("PT010").expect("PT010 must exist in ATC table")
    }

    /// Recompute tax_due from taxable_amount and tax_rate.
    pub fn recompute(&mut self) {
        self.tax_due = round_to_cents(round_to_cents(self.taxable_amount) * self.tax_rate);
    }
}

/// Complete draft or filed return for Form 2551Q.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Form2551QDraft {
    /// Database row ID (None before first save)
    pub id: Option<i64>,

    // === Filing Period ===
    pub tin: String,
    /// Taxpayer-type snapshot owned by this return while it is editable.
    ///
    /// This is optional only for backward-compatible deserialization. Older
    /// persisted JSON has no trustworthy value, so `None` must fail validation
    /// instead of inheriting `TaxpayerType::default()` (`Individual`).
    #[serde(default)]
    pub taxpayer_type: Option<TaxpayerType>,
    /// Business-commencement snapshot used to identify a new registrant's
    /// initial quarterly return. Older profiles/drafts may not own this date;
    /// after Q1 that ambiguity must fail closed instead of denying the taxpayer
    /// the election allowed on the first return after commencement.
    #[serde(default)]
    pub business_start_date: Option<chrono::NaiveDate>,
    pub taxable_year: u16,
    pub quarter: u8, // 1–4
    #[serde(default)]
    pub tax_period_basis: TaxPeriodBasis,
    /// Month in which the taxable year ends (1–12). Calendar filers use 12.
    #[serde(default = "default_year_end_month")]
    pub year_end_month: u8,
    pub eopt_tier: Option<crate::profile::EoptTier>,

    // === Header Options ===
    pub is_amended: bool,
    #[serde(default)]
    pub original_return_filed_and_paid_on_time: bool,
    /// Number entered in Item 5 (maximum two printed digits).
    #[serde(default)]
    pub number_of_attached_sheets: u16,
    pub tax_relief: bool,
    #[serde(default)]
    pub tax_relief_specification: String,
    #[serde(default)]
    pub item_13_election: Item13Election,
    /// Snapshot of the profile's annual income-tax election. Older drafts
    /// deserialize to `None` and fail closed until refreshed from the profile.
    #[serde(default)]
    pub annual_income_tax_election: Option<AnnualIncomeTaxElection>,
    /// Item 13 as the filed (Submitted, Confirmed or Paid) Q1 return of this
    /// TIN and taxable year in this app answered it, recorded on a later
    /// quarter's editable draft when it is opened
    /// ([`Self::adopt_filed_q1_item_13`]). Item 13 is answered once a year,
    /// on the initial quarter: while this is set the later quarter's Item 13
    /// is that answer and is locked. `None` (no filed Q1 here, e.g. Q1 was
    /// filed before migrating) leaves Item 13 the filer's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filed_q1_item_13: Option<Item13Election>,

    // === Part I — pre-filled from profile, read-only in UI ===
    pub rdo_code: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,

    // === Schedule 1 — user editable ===
    pub schedule_1: Vec<Schedule1Row>,

    // === Part II — computed from Schedule 1 ===
    /// Sum of all schedule_1[].tax_due
    pub total_tax_due: f64,
    /// From BIR Form 2307 — user-entered
    pub creditable_tax_withheld: f64,
    /// Only applicable when is_amended = true — LOCKED otherwise
    pub tax_paid_previous: f64,
    /// Line 17: Other Tax Credit/Payment — user-entered
    #[serde(default)]
    pub other_tax_credit: f64,
    /// Line 17 `(specify)` description paired with `other_tax_credit`.
    #[serde(default)]
    pub other_tax_credit_description: String,
    /// Line 18: Total Tax Credits/Payments = sum of Lines 15, 16, 17
    #[serde(default)]
    pub total_tax_credits: f64,
    /// Line 19: Tax Still Payable/(Overpayment) = Line 14 Less Line 18
    /// Can be negative (overpayment)
    pub tax_payable: f64,

    // === Penalties ===
    #[serde(default = "default_true")]
    pub auto_compute_penalties: bool,
    /// Reviewed ERP/historical sales basis used by the anomaly surcharge rule.
    /// Persist it so queue-boundary recomputation cannot silently clear a
    /// previously detected under-declaration.
    #[serde(default)]
    pub expected_sales_for_penalties: Option<f64>,
    #[serde(default)]
    pub surcharge: f64,
    #[serde(default)]
    pub interest: f64,
    #[serde(default)]
    pub compromise: f64,
    #[serde(default)]
    pub total_penalties: f64,
    /// Items 20-22 the filer entered in place of the computed ones (rule 8,
    /// the shared model's per-component override): each set component files
    /// instead of what [`Self::recompute`] computes, the others keep
    /// computing. Stored under the same JSON keys as every other form's
    /// (`penalty_overrides/{surcharge,interest,compromise}`).
    #[serde(default)]
    pub penalty_overrides: PenaltyOverrides,
    /// The deadline the Calendar resolves for this quarter (weekends, the
    /// stored deadline overrides and the profile's own applied), recorded by
    /// [`Self::apply_penalty_profile`] while the return is a Draft, so its
    /// penalties run from the date the Calendar shows. `None` (a fiscal
    /// quarter, which the Calendar has no row for, or a draft no profile was
    /// applied to) falls back to [`Self::filing_deadline`]'s own date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub penalty_due_date: Option<NaiveDate>,
    #[serde(default)]
    pub total_amount_payable: f64,
    #[serde(default)]
    pub overpayment_disposition: OverpaymentDisposition,

    // === Status & Audit ===
    pub status: FilingStatus,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub confirmed_at: Option<String>,
    #[serde(default)]
    pub submission_filename: Option<String>,
    #[serde(default)]
    pub receipt_id: Option<i64>,
    /// The exact plaintext XML of this generation: written at queue time and
    /// replaced with the bytes actually encrypted for PUT at submission. Kept
    /// so a later amendment can snapshot what BIR received. Absent on returns
    /// queued before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_payload: Option<String>,
    /// SHA-256 over the exact BIR field map plus internal calculation inputs at
    /// the moment the user queues the return for submission.
    #[serde(default)]
    pub queued_submission_fingerprint: Option<String>,
    /// Short-lived database claim established immediately before network I/O.
    /// Recoverable only when PUT was never marked started and the lease expired.
    #[serde(default)]
    pub submission_claim_token: Option<String>,
    #[serde(default)]
    pub submission_claimed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_authorization: Option<QueueAuthorization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_claim_lease_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_put_started_at: Option<String>,
    /// A filed/queued snapshot is immutable. When the taxpayer profile later
    /// changes, we mark the return for an explicit revert/amendment workflow
    /// instead of silently rewriting the reviewed submission fields.
    #[serde(default)]
    pub profile_snapshot_stale: bool,
    #[serde(default)]
    pub profile_snapshot_stale_reason: Option<String>,
    /// Confirmed effective-dated profile version used to prefill this return.
    /// `None` on older drafts is upgraded when the draft is next opened.
    #[serde(default)]
    pub effective_profile_version_id: Option<String>,
    /// Persisted, user-visible reason an exact filing-period profile segment
    /// could not be selected. A draft with this error cannot be queued.
    #[serde(default)]
    pub profile_resolution_error: Option<String>,

    // === Background Retry Logic ===
    #[serde(default)]
    pub submission_attempts: u32,
    #[serde(default)]
    pub next_retry_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
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

    /// Set to true when this draft was pre-filled from a previous quarter.
    /// UI shows a "Pre-filled from Q{n} {year}" banner when true.
    pub carried_forward_from: Option<(u16, u8)>, // (year, quarter)

    // === Payment / Attachments ===
    #[serde(default)]
    pub payment_receipt_path: Option<String>,
}

/// The page's 2551Q profile boxes (their keys) and the draft field each
/// writes: what `profile_edits` names.
const PROFILE_BOXES: [(&str, &str); 5] = [
    ("frm2551Qv2018:txtRDOCode", "/rdo_code"),
    ("frm2551Qv2018:registeredName", "/taxpayer_name"),
    ("frm2551Qv2018:registeredAddress", "/registered_address"),
    ("frm2551Qv2018:zipCode", "/zip_code"),
    ("frm2551Qv2018:telNo", "/contact_number"),
];

impl Form2551QDraft {
    /// Minimum Item 5 count required by application-generated Schedule 1
    /// continuation sheets. The user's Item 5 value may be larger because it
    /// can include other supporting attachments.
    pub fn required_schedule_attachment_sheets(&self) -> u16 {
        let overflow_rows = self
            .schedule_1
            .len()
            .saturating_sub(FORM_2551Q_XML_SCHEDULE_ROW_CAPACITY);
        if overflow_rows == 0 {
            return 0;
        }

        let continuation_pages = overflow_rows
            .saturating_add(FORM_2551Q_SCHEDULE_CONTINUATION_ROW_CAPACITY - 1)
            / FORM_2551Q_SCHEDULE_CONTINUATION_ROW_CAPACITY;
        u16::try_from(continuation_pages)
            .expect("the canonical 2551Q ATC registry cannot exceed Item 5 capacity")
    }

    /// Raise Item 5 to the generated-continuation minimum without erasing any
    /// user-owned count for other attachments. Removing Schedule rows does not
    /// call this helper, so the count is never silently lowered.
    pub fn ensure_required_schedule_attachment_sheets(&mut self) -> bool {
        let required = self.required_schedule_attachment_sheets();
        if self.number_of_attached_sheets >= required {
            return false;
        }
        self.number_of_attached_sheets = required;
        true
    }

    /// Compatibility constructor for an already-resolved profile projection.
    /// Defaults to a PT010 row with zero amounts.
    ///
    /// Production profile-owned creation must use
    /// [`Self::new_from_effective_profile`] so flat compatibility fields never
    /// bypass the confirmed effective-dated ledger. XML/import adapters and
    /// focused model tests may use this constructor when they already own the
    /// authoritative snapshot.
    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        let annual_election = annual_income_tax_election(profile, year);
        let mut draft = Self {
            id: None,
            tin: profile.tin.full(),
            taxpayer_type: Some(profile.taxpayer_type.clone()),
            business_start_date: profile.business_start_date,
            taxable_year: year,
            quarter,
            tax_period_basis: TaxPeriodBasis::Calendar,
            year_end_month: 12,
            eopt_tier: profile.eopt_tier.clone(),
            is_amended: false,
            original_return_filed_and_paid_on_time: false,
            number_of_attached_sheets: 0,
            tax_relief: false,
            tax_relief_specification: String::new(),
            item_13_election: Item13Election::Unanswered,
            annual_income_tax_election: Some(annual_election),
            filed_q1_item_13: None,
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            schedule_1: vec![Schedule1Row::default_pt010()],
            total_tax_due: 0.0,
            creditable_tax_withheld: 0.0,
            tax_paid_previous: 0.0,
            other_tax_credit: 0.0,
            other_tax_credit_description: String::new(),
            total_tax_credits: 0.0,
            tax_payable: 0.0,
            auto_compute_penalties: true,
            expected_sales_for_penalties: None,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            penalty_overrides: PenaltyOverrides::default(),
            penalty_due_date: None,
            total_amount_payable: 0.0,
            overpayment_disposition: OverpaymentDisposition::None,
            status: FilingStatus::Draft,
            created_at: now.clone(),
            updated_at: now,
            submitted_at: None,
            confirmed_at: None,
            submission_filename: None,
            receipt_id: None,
            submitted_payload: None,
            queued_submission_fingerprint: None,
            submission_claim_token: None,
            submission_claimed_at: None,
            queue_authorization: None,
            submission_claim_lease_until: None,
            submission_put_started_at: None,
            profile_snapshot_stale: false,
            profile_snapshot_stale_reason: None,
            effective_profile_version_id: None,
            profile_resolution_error: None,
            submission_attempts: 0,
            next_retry_at: None,
            last_error: None,
            profile_edits: Vec::new(),
            profile_baseline: std::collections::BTreeMap::new(),
            carried_forward_from: None,
            payment_receipt_path: None,
        };
        if draft.item_13_is_applicable() == Some(true) {
            draft.item_13_election = match annual_election {
                AnnualIncomeTaxElection::EightPercent => Item13Election::EightPercent,
                AnnualIncomeTaxElection::Graduated => Item13Election::Graduated,
                AnnualIncomeTaxElection::Unrecorded | AnnualIncomeTaxElection::Conflicting => {
                    Item13Election::Unanswered
                }
            };
        }
        // A draft can be previewed before the first input event. Canonicalize
        // period-owned ATC rates and every derived amount immediately.
        draft.recompute(None);
        draft
    }

    /// Create a production draft from the single confirmed profile segment
    /// that covers its complete filing period.
    ///
    /// The compatibility fields on `TaxpayerProfile` are never used as a
    /// fallback. When resolution fails, only the stable TIN is retained for
    /// persistence and the blocking reason is stored on the draft.
    pub fn new_from_effective_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let mut draft = Self::new_from_profile(profile, year, quarter);
        draft.clear_profile_owned_snapshot();
        let _ = draft.reconcile_with_effective_profile(profile);
        draft.recompute(None);
        draft
    }

    /// Carry-forward: clone previous quarter's Schedule 1 rows as editable defaults.
    /// Preserves ATCs and amounts as starting point — user adjusts them.
    pub fn with_carried_forward(mut self, previous: &Form2551QDraft) -> Self {
        self.schedule_1 = previous.schedule_1.clone();
        self.carried_forward_from = Some((previous.taxable_year, previous.quarter));
        self.recompute(None);
        self
    }

    /// Compatibility sync for callers that already own a resolved projection.
    ///
    /// Production drafts carrying effective-profile audit state are always
    /// reconciled through the effective-dated ledger. Legacy/internal callers
    /// without that state retain the former raw projection behavior.
    pub fn sync_with_profile(&mut self, profile: &TaxpayerProfile) {
        if self.effective_profile_version_id.is_some() || self.profile_resolution_error.is_some() {
            let _ = self.reconcile_with_effective_profile(profile);
            return;
        }
        self.sync_with_profile_snapshot(profile, None);
    }

    /// Reconcile this return against the confirmed profile segment for the
    /// return's exact calendar/fiscal quarter.
    ///
    /// Editable drafts receive refreshed prefills. Queued and later snapshots
    /// remain immutable; only their audit/staleness markers may change.
    pub fn reconcile_with_effective_profile(
        &mut self,
        profile: &TaxpayerProfile,
    ) -> Result<(), String> {
        let Some((period_start, period_end)) = self.filing_period_bounds() else {
            let error =
                "The 2551Q filing period is invalid, so an effective taxpayer-profile version cannot be selected"
                    .to_string();
            self.record_profile_resolution_failure(error.clone());
            return Err(error);
        };
        // A first-time registrant can begin business inside a quarter. In that
        // one narrow case the legally relevant filing period starts on the
        // confirmed registration/effective date, not before the taxpayer
        // existed. A mid-quarter replacement version is deliberately not
        // treated this way: any prior confirmed version keeps the full-quarter
        // resolver active so a profile change inside the period fails closed.
        let touching_confirmed = profile
            .profile_versions
            .iter()
            .filter(|version| {
                version.status == TaxProfileVersionStatus::Confirmed
                    && version.overlaps_period(period_start, period_end)
            })
            .collect::<Vec<_>>();
        let effective_period_start = touching_confirmed
            .as_slice()
            .first()
            .filter(|_| touching_confirmed.len() == 1)
            .and_then(|version| {
                let effective_from = version.effective_from?;
                let is_initial_registration = effective_from > period_start
                    && effective_from <= period_end
                    && version.cor.registration_date == Some(effective_from)
                    && !profile.profile_versions.iter().any(|other| {
                        other.status == TaxProfileVersionStatus::Confirmed
                            && other.id != version.id
                            && other
                                .effective_from
                                .is_some_and(|start| start < effective_from)
                    });
                is_initial_registration.then_some(effective_from)
            })
            .unwrap_or(period_start);
        let resolved = profile.resolve_tax_profile_for_period(effective_period_start, period_end);
        if resolved.has_blocking_issues() {
            let details = resolved
                .issues
                .iter()
                .map(|issue| issue.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            let error = if details.is_empty() {
                format!(
                    "No confirmed taxpayer-profile version covers the 2551Q filing period {period_start} through {period_end}"
                )
            } else {
                format!(
                    "The taxpayer profile cannot be resolved for the 2551Q filing period {period_start} through {period_end}: {details}"
                )
            };
            self.record_profile_resolution_failure(error.clone());
            return Err(error);
        }

        let version = resolved
            .effective_segment
            .as_ref()
            .expect("a resolution without blocking issues owns one segment");
        let projected = profile.projection_for_version(version);
        self.sync_with_profile_snapshot(&projected, Some(version.id.as_str()));
        Ok(())
    }

    fn clear_profile_owned_snapshot(&mut self) {
        self.taxpayer_type = None;
        self.business_start_date = None;
        self.eopt_tier = None;
        self.annual_income_tax_election = None;
        self.item_13_election = Item13Election::Unanswered;
        self.rdo_code.clear();
        self.taxpayer_name.clear();
        self.registered_address.clear();
        self.zip_code.clear();
        self.contact_number.clear();
        self.email.clear();
        self.effective_profile_version_id = None;
        self.profile_resolution_error = None;
    }

    fn record_profile_resolution_failure(&mut self, error: String) {
        if matches!(self.status, FilingStatus::Draft) {
            // Editable returns must not keep displaying a previously resolved
            // profile snapshot after the effective-dated ledger stops
            // resolving. Clear profile-owned prefills and Item 13 so preview
            // and validation fail closed instead of presenting stale facts as
            // current. Queue-bound and later snapshots remain immutable below.
            self.clear_profile_owned_snapshot();
            self.profile_snapshot_stale = false;
            self.profile_snapshot_stale_reason = None;
        }
        self.profile_resolution_error = Some(error.clone());
        if !matches!(self.status, FilingStatus::Draft) {
            self.profile_snapshot_stale = true;
            self.profile_snapshot_stale_reason = Some(format!(
                "The immutable return snapshot could not be reconciled with the effective-dated taxpayer profile: {error}"
            ));
        }
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Whether the filer edited the page's profile box writing `path` on
    /// this return (`profile_edits` holds the boxes' keys).
    fn edited_on_return(&self, path: &str) -> bool {
        PROFILE_BOXES
            .iter()
            .any(|(key, field)| *field == path && self.profile_edits.iter().any(|edit| edit == key))
    }

    fn profile_field_mut(&mut self, path: &str) -> &mut String {
        match path {
            "/rdo_code" => &mut self.rdo_code,
            "/taxpayer_name" => &mut self.taxpayer_name,
            "/registered_address" => &mut self.registered_address,
            "/zip_code" => &mut self.zip_code,
            _ => &mut self.contact_number,
        }
    }

    fn sync_with_profile_snapshot(
        &mut self,
        profile: &TaxpayerProfile,
        effective_profile_version_id: Option<&str>,
    ) {
        let annual_election = annual_income_tax_election(profile, self.taxable_year);
        if !matches!(self.status, FilingStatus::Draft) {
            let snapshot_matches = self.tin == profile.tin.full()
                && self.taxpayer_type.as_ref() == Some(&profile.taxpayer_type)
                && self.business_start_date == profile.business_start_date
                && self.eopt_tier == profile.eopt_tier
                && self.annual_income_tax_election == Some(annual_election)
                && (self.rdo_code == profile.rdo_code || self.edited_on_return("/rdo_code"))
                && (self.taxpayer_name == profile.full_name
                    || self.edited_on_return("/taxpayer_name"))
                && (self.registered_address == profile.registered_address
                    || self.edited_on_return("/registered_address"))
                && (self.zip_code == profile.zip_code || self.edited_on_return("/zip_code"))
                && (self.contact_number == profile.phone
                    || self.edited_on_return("/contact_number"))
                && self.email == profile.email
                && effective_profile_version_id.is_none_or(|version_id| {
                    self.effective_profile_version_id.as_deref() == Some(version_id)
                });
            self.profile_snapshot_stale = !snapshot_matches;
            self.profile_snapshot_stale_reason = (!snapshot_matches).then(|| {
                "The taxpayer profile changed after this return was reviewed. Revert or amend the return to review refreshed profile values before filing."
                    .to_string()
            });
            self.profile_resolution_error = None;
            self.updated_at = chrono::Utc::now().to_rfc3339();
            return;
        }

        self.tin = profile.tin.full();
        self.taxpayer_type = Some(profile.taxpayer_type.clone());
        self.business_start_date = profile.business_start_date;
        self.eopt_tier = profile.eopt_tier.clone();
        self.annual_income_tax_election = Some(annual_election);
        // A profile box the filer edited on this return keeps the edit; the
        // others take the profile, and their baseline with them, so the
        // page's profile follow sees them as following.
        for (path, value) in [
            ("/rdo_code", &profile.rdo_code),
            ("/taxpayer_name", &profile.full_name),
            ("/registered_address", &profile.registered_address),
            ("/zip_code", &profile.zip_code),
            ("/contact_number", &profile.phone),
        ] {
            if self.edited_on_return(path) {
                continue;
            }
            *self.profile_field_mut(path) = value.clone();
            if let Some(baseline) = self.profile_baseline.get_mut(path) {
                *baseline = serde_json::Value::String(value.clone());
            }
        }
        self.email = profile.email.clone();
        self.item_13_election = match self.item_13_is_applicable() {
            // The filed Q1's answer stands for the rest of its year.
            _ if self.filed_q1_item_13().is_some() => {
                self.filed_q1_item_13().unwrap_or(self.item_13_election)
            }
            Some(false)
                if self.later_period_requires_recorded_annual_election()
                    && matches!(
                        annual_election,
                        AnnualIncomeTaxElection::Unrecorded | AnnualIncomeTaxElection::Conflicting
                    ) =>
            {
                Item13Election::Unanswered
            }
            Some(false) => Item13Election::NotApplicable,
            Some(true) => match annual_election {
                AnnualIncomeTaxElection::Graduated => Item13Election::Graduated,
                AnnualIncomeTaxElection::EightPercent => Item13Election::EightPercent,
                AnnualIncomeTaxElection::Conflicting => Item13Election::Unanswered,
                AnnualIncomeTaxElection::Unrecorded => match self.item_13_election {
                    Item13Election::Graduated | Item13Election::EightPercent => {
                        self.item_13_election
                    }
                    Item13Election::Unanswered | Item13Election::NotApplicable => {
                        Item13Election::Unanswered
                    }
                },
            },
            None => Item13Election::Unanswered,
        };
        self.profile_snapshot_stale = false;
        self.profile_snapshot_stale_reason = None;
        self.effective_profile_version_id = effective_profile_version_id.map(ToOwned::to_owned);
        self.profile_resolution_error = None;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Record the Item 13 answer of this TIN's filed Q1 return of the same
    /// taxable year (`filed`, `None` when this app holds none) on an editable
    /// later-quarter draft, and take it as this return's Item 13. A Q1 draft,
    /// or a return that left Draft, is not changed.
    pub fn adopt_filed_q1_item_13(&mut self, filed: Option<Item13Election>) {
        if !matches!(self.status, FilingStatus::Draft) {
            return;
        }
        self.filed_q1_item_13 = filed
            .filter(|election| *election != Item13Election::Unanswered)
            .filter(|_| self.quarter != 1);
        if let Some(election) = self.filed_q1_item_13 {
            self.item_13_election = election;
        }
    }

    /// The filed Q1's Item 13 this later-quarter return is locked to.
    pub fn filed_q1_item_13(&self) -> Option<Item13Election> {
        self.filed_q1_item_13
            .filter(|election| *election != Item13Election::Unanswered)
            .filter(|_| self.quarter != 1)
    }

    /// Whether the official Item 13 election applies to this return.
    ///
    /// The question is only applicable to an Individual taxpayer's initial
    /// quarterly return when Schedule 1 contains the Sec. 116 activity
    /// represented by canonical ATC `PT010`. For an existing taxpayer this is
    /// Q1; for a new registrant it is the quarter containing commencement of
    /// business. Presence of PT010 is the persisted activity signal even on a
    /// NIL return, so a zero taxable amount does not make the question
    /// disappear. `None` means the draft lacks a trustworthy profile snapshot
    /// needed to decide applicability and must fail closed at validation.
    pub fn item_13_is_applicable(&self) -> Option<bool> {
        match self.taxpayer_type.as_ref()? {
            TaxpayerType::Individual => {}
            _ => return Some(false),
        }
        if !self.schedule_1.iter().any(|row| row.atc.trim() == "PT010") {
            return Some(false);
        }
        if !(1..=4).contains(&self.quarter) || !(1..=12).contains(&self.year_end_month) {
            return None;
        }

        let initial_quarter = if let Some(start_date) = self.business_start_date {
            let fiscal_year_end =
                i32::from(self.taxable_year) * 12 + i32::from(self.year_end_month) - 1;
            let fiscal_year_start = fiscal_year_end - 11;
            let business_start = start_date.year() * 12
                + i32::try_from(start_date.month()).expect("month fits i32")
                - 1;
            if business_start < fiscal_year_start {
                1
            } else if business_start > fiscal_year_end {
                return Some(false);
            } else {
                u8::try_from((business_start - fiscal_year_start) / 3 + 1)
                    .expect("a fiscal-year month maps to quarter 1 through 4")
            }
        } else if self.quarter == 1 {
            // Q1 is the initial return for an existing taxpayer even when a
            // legacy profile does not own its historical commencement date.
            1
        } else {
            // The redesigned tax profile records no commencement date, so a
            // later quarter without one cannot be classified: the filer's own
            // Item 13 answer stands, as on the official form.
            return None;
        };

        Some(self.quarter == initial_quarter)
    }

    /// A later return with Section 116 activity must inherit the taxpayer's
    /// already-recorded annual election. It is not legally safe to infer that
    /// election from the current quarter or let the renderer silently leave
    /// Item 13 unresolved.
    fn later_period_requires_recorded_annual_election(&self) -> bool {
        matches!(self.taxpayer_type, Some(TaxpayerType::Individual))
            && self.schedule_1.iter().any(|row| row.atc.trim() == "PT010")
            && self.item_13_is_applicable() != Some(true)
    }

    /// Recompute all derived values (call after any field change).
    /// `expected_sales` can be optionally provided by the ERP system to detect under-declaration fraud.
    #[allow(clippy::collapsible_if)]
    pub fn recompute(&mut self, expected_sales: Option<f64>) {
        if let Some(expected_sales) = expected_sales {
            self.expected_sales_for_penalties = Some(expected_sales);
        }
        let refresh_auto_penalties = matches!(self.status, FilingStatus::Draft);
        self.recompute_internal(self.expected_sales_for_penalties, refresh_auto_penalties);
    }

    #[allow(clippy::collapsible_if)]
    fn recompute_internal(&mut self, expected_sales: Option<f64>, refresh_auto_penalties: bool) {
        let taxable_year = self.taxable_year;
        let quarter = self.quarter;
        let year_end_month = self.year_end_month;
        for row in &mut self.schedule_1 {
            if let Some(AtcRateResolution::Single(rate)) =
                resolve_2551q_atc_rate(row.atc.trim(), taxable_year, quarter, year_end_month)
            {
                row.tax_rate = rate;
            }
            row.recompute();
        }
        // Line 14: Total Tax Due = sum of Schedule 1 rows
        self.total_tax_due =
            (self.schedule_1.iter().map(|r| r.tax_due).sum::<f64>() * 100.0).round() / 100.0;

        // Line 18: Total Tax Credits = Line 15 + Line 16 (if amended) + Line 17
        let previous_credit = if self.is_amended {
            round_to_cents(self.tax_paid_previous)
        } else {
            0.0
        };
        self.total_tax_credits = round_to_cents(
            round_to_cents(self.creditable_tax_withheld)
                + previous_credit
                + round_to_cents(self.other_tax_credit),
        );

        // Line 19: Tax Still Payable/(Overpayment) = Line 14 - Line 18
        // NOTE: Can be negative (overpayment). Do NOT clamp to zero.
        self.tax_payable = ((self.total_tax_due - self.total_tax_credits) * 100.0).round() / 100.0;

        // Compute deadline and penalties.
        // The penalty engine handles all three cases:
        //   1. Filed on time → all penalties = 0 (engine's own on-time check)
        //   2. Filed late, tax due → surcharge + interest + compromise
        //   3. Filed late, no tax due (overpayment) → surcharge=0, interest=0,
        //      compromise from gross_sales table (engine's unpaid_tax<=0 branch)
        //
        // We pass max(tax_payable, 0) as basic_tax_due so surcharge/interest
        // are computed on the positive amount only. Line 19 itself stays unclamped.
        self.adopt_legacy_manual_penalties();
        if self.auto_compute_penalties && refresh_auto_penalties {
            if let Some(deadline) = self.filing_deadline() {
                let today = super::penalty_items::today();

                let config = PenaltyConfig::default_rules();

                let gross_sales = self
                    .schedule_1
                    .iter()
                    .map(|r| round_to_cents(r.taxable_amount))
                    .sum::<f64>();

                // Penalty base: clamp to 0 for surcharge/interest calc only.
                // Line 19 (self.tax_payable) is NOT clamped — it preserves overpayment.
                let penalty_tax_base = self.tax_payable.max(0.0);

                let taxpayer_class = match self.eopt_tier {
                    Some(crate::profile::EoptTier::Micro) => TaxpayerClass::Micro,
                    Some(crate::profile::EoptTier::Small) => TaxpayerClass::Small,
                    Some(crate::profile::EoptTier::Medium) => TaxpayerClass::Medium,
                    Some(crate::profile::EoptTier::Large) => TaxpayerClass::Large,
                    None => TaxpayerClass::Regular,
                };

                let mut is_fraud = false;
                if let Some(expected) = expected_sales {
                    if crate::integration::fraud::detect_under_declaration(expected, gross_sales) {
                        is_fraud = true;
                    }
                }

                let ctx = PenaltyContext {
                    form_code: "2551Qv2018".to_string(),
                    tax_type: PenaltyProfile::StandardFiling,
                    taxpayer_class,
                    taxable_period: format!(
                        "Q{} year ended {:02}/{}",
                        self.quarter, self.year_end_month, self.taxable_year
                    ),
                    is_amended_return: self.is_amended,
                    original_was_on_time: self.original_return_filed_and_paid_on_time,
                    is_fraud_or_willful_neglect: is_fraud,
                    basic_tax_due: penalty_tax_base,
                    amount_paid_before_deadline: 0.0,
                    gross_sales_or_receipts: gross_sales,
                    due_date: deadline,
                    filing_date: today,
                    payment_date: None,
                };

                let penalties = PenaltyEngine::calculate(&ctx, &config);
                self.surcharge = penalties.surcharge;
                self.interest = penalties.interest;
                self.compromise = penalties.compromise;
            }
        }

        // Rule 8: an entered amount files instead of the computed one.
        if self.auto_compute_penalties {
            for (component, slot) in [
                (PenaltyComponent::Surcharge, &mut self.surcharge),
                (PenaltyComponent::Interest, &mut self.interest),
                (PenaltyComponent::Compromise, &mut self.compromise),
            ] {
                if let Some(value) = self.penalty_overrides.get(component) {
                    *slot = round_to_cents(value);
                }
            }
        }

        // `2551q-tax-rate-eight-percent`: Item 13 = 8% on this return zeroes
        // Items 20/21/22 along with the rest of the figures, and the
        // official page disables them, so they take no override either.
        // Profile: official-match. A legacy manual draft (never migrated,
        // see `adopt_legacy_manual_penalties`) keeps the amounts it was
        // saved with.
        if matches!(self.item_13_election, Item13Election::EightPercent) {
            for component in PenaltyComponent::ALL {
                self.penalty_overrides.set(component, None);
            }
            if self.auto_compute_penalties {
                self.surcharge = 0.0;
                self.interest = 0.0;
                self.compromise = 0.0;
            }
        }

        // Line 23: Total Penalties
        self.total_penalties =
            ((self.surcharge + self.interest + self.compromise) * 100.0).round() / 100.0;
        // Line 24: Total Amount Payable/(Overpayment) = Line 19 + Line 23
        self.total_amount_payable =
            ((self.tax_payable + self.total_penalties) * 100.0).round() / 100.0;

        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Once, for a Draft saved with the manual switch (all three typed):
    /// each typed amount becomes that component's override and the switch
    /// turns off for good (it is the one-time flag), so the return files
    /// what it filed before and a later blank computes again. A manual draft
    /// with Item 13 = 8% stays manual (the 8% option takes no override),
    /// keeping its amounts until its election changes. Returns whether the
    /// draft changed. Runs before any write
    /// ([`crate::forms::agent::prepare_for_writes`]) and in
    /// [`Self::recompute`].
    pub fn adopt_legacy_manual_penalties(&mut self) -> bool {
        if self.auto_compute_penalties
            || !matches!(self.status, FilingStatus::Draft)
            || matches!(self.item_13_election, Item13Election::EightPercent)
        {
            return false;
        }
        for (component, value) in [
            (PenaltyComponent::Surcharge, self.surcharge),
            (PenaltyComponent::Interest, self.interest),
            (PenaltyComponent::Compromise, self.compromise),
        ] {
            self.penalty_overrides.set(component, Some(value));
        }
        self.auto_compute_penalties = true;
        true
    }

    fn submission_fingerprint(&self) -> String {
        let field_map = self.to_bir_field_map();
        let internal_inputs = (
            &self.taxpayer_type,
            &self.business_start_date,
            &self.annual_income_tax_election,
            &self.eopt_tier,
            self.auto_compute_penalties,
            self.expected_sales_for_penalties.map(f64::to_bits),
            self.original_return_filed_and_paid_on_time,
        );
        let mut hasher = Sha256::new();
        hasher.update(b"ebirforms:2551Qv2018:queued-submission:v1\0");
        hasher.update(
            serde_json::to_vec(&field_map)
                .expect("a BTreeMap<String, String> always serializes to JSON"),
        );
        hasher.update([0]);
        hasher.update(
            serde_json::to_vec(&internal_inputs)
                .expect("owned 2551Q fingerprint inputs always serialize to JSON"),
        );
        hex::encode(hasher.finalize())
    }

    /// Returns a friendly label for the carry-forward banner.
    pub fn carry_forward_label(&self) -> Option<String> {
        self.carried_forward_from
            .map(|(year, q)| format!("Pre-filled from Q{} {} - adjust amounts as needed", q, year))
    }

    pub fn period_code(&self) -> String {
        format!(
            "{:02}{}Q{}",
            self.year_end_month, self.taxable_year, self.quarter
        )
    }

    /// Inclusive calendar dates covered by this return's selected quarter.
    ///
    /// `taxable_year` is the year in which the calendar/fiscal year ends. A
    /// fiscal year ending June 2026 therefore begins July 2025.
    pub fn filing_period_bounds(&self) -> Option<(NaiveDate, NaiveDate)> {
        if !(1..=4).contains(&self.quarter)
            || !(1..=12).contains(&self.year_end_month)
            || (matches!(self.tax_period_basis, TaxPeriodBasis::Calendar)
                && self.year_end_month != 12)
        {
            return None;
        }

        let fiscal_end_month_index =
            i32::from(self.taxable_year) * 12 + i32::from(self.year_end_month) - 1;
        let quarter_start_month_index =
            fiscal_end_month_index - 11 + (i32::from(self.quarter) - 1) * 3;
        let period_start_year = quarter_start_month_index.div_euclid(12);
        let period_start_month = quarter_start_month_index.rem_euclid(12) + 1;
        let period_start = NaiveDate::from_ymd_opt(
            period_start_year,
            u32::try_from(period_start_month).ok()?,
            1,
        )?;

        let month_after_period_index = quarter_start_month_index + 3;
        let month_after_period_year = month_after_period_index.div_euclid(12);
        let month_after_period = month_after_period_index.rem_euclid(12) + 1;
        let period_end = NaiveDate::from_ymd_opt(
            month_after_period_year,
            u32::try_from(month_after_period).ok()?,
            1,
        )? - Duration::days(1);

        Some((period_start, period_end))
    }

    /// Resolve the deadline penalties run from as the Calendar resolves it
    /// for this quarter: `global_overrides` (the stored deadline overrides,
    /// shipped holidays included) and the profile's own, applied the way
    /// every other form's [`super::penalty_items::PenaltyItems::apply_profile`]
    /// applies them, so a deadline moved for everyone moves this return's
    /// penalties too. A fiscal quarter has no Calendar row and keeps its own
    /// date. A return that has left Draft keeps the date it was reviewed with.
    pub fn apply_penalty_profile(
        &mut self,
        profile: &TaxpayerProfile,
        global_overrides: &[crate::calendar_rules::DeadlineOverride],
    ) {
        if !matches!(self.status, FilingStatus::Draft) {
            return;
        }
        self.penalty_due_date = (self.year_end_month == 12 && (1..=4).contains(&self.quarter))
            .then(|| {
                let overrides = crate::forms::event_trigger::calendar_overrides(
                    global_overrides,
                    profile,
                    "2551Q",
                    Some(self.taxable_year),
                );
                super::penalty_items::resolve_due_date(
                    "2551Q",
                    &crate::calendar_rules::DeadlinePeriod::Quarterly {
                        taxable_year: i32::from(self.taxable_year),
                        quarter: self.quarter,
                    },
                    &overrides,
                )
            })
            .flatten();
    }

    /// The deadline penalties run from: the Calendar's
    /// ([`Self::apply_penalty_profile`]), else this quarter's own.
    ///
    /// Its own is the 25th day after the quarter closes, moved to the next
    /// working day when it falls on a weekend or an official holiday, as the
    /// Calendar moves it (`penalty_items::next_business_day`): Q3 2026's
    /// Sunday, Oct 25, is due Monday, Oct 26, and filing then carries no
    /// penalty. `taxable_year` is the year in which the selected taxable
    /// year ends. A fiscal year ending in June 2026 therefore places Q1/Q2
    /// in calendar 2025 and Q3/Q4 in calendar 2026.
    fn filing_deadline(&self) -> Option<chrono::NaiveDate> {
        if let Some(due) = self.penalty_due_date {
            return Some(due);
        }
        if !(1..=12).contains(&self.year_end_month) || !(1..=4).contains(&self.quarter) {
            return None;
        }

        let quarter_end_month =
            ((u16::from(self.year_end_month) - 1 + u16::from(self.quarter) * 3) % 12 + 1) as u8;
        let mut quarter_end_year = i32::from(self.taxable_year);
        if quarter_end_month > self.year_end_month {
            quarter_end_year -= 1;
        }

        let (deadline_year, deadline_month) = if quarter_end_month == 12 {
            (quarter_end_year + 1, 1)
        } else {
            (quarter_end_year, u32::from(quarter_end_month) + 1)
        };

        chrono::NaiveDate::from_ymd_opt(deadline_year, deadline_month, 25)
            .map(super::penalty_items::next_business_day)
    }

    pub fn default_submission_filename(&self) -> String {
        format!(
            "{}-2551Qv2018-{}#{}#.xml",
            self.tin,
            self.period_code(),
            self.email
        )
    }

    // ── State Transition Methods ──
    // These centralize all status mutations with precondition checks.
    // Callers should use these instead of directly assigning `self.status`.

    /// Returns true if the form fields should be editable (only in Draft status).
    pub fn is_editable(&self) -> bool {
        matches!(self.status, FilingStatus::Draft)
    }

    /// Transition: Draft → Queued.
    /// Validates the form first. Returns Err with validation errors if invalid.
    pub fn transition_to_queued(&mut self) -> Result<(), Vec<(String, String)>> {
        assert!(
            matches!(self.status, FilingStatus::Draft),
            "Cannot queue form in {:?} status — must be Draft",
            self.status
        );
        self.recompute(None);
        // Same checks as the page's Validate (`agent::validate` → `filing_errors`).
        let errors = self.filing_errors();
        if !errors.is_empty() {
            return Err(errors);
        }
        if self.item_13_is_applicable() == Some(true)
            && matches!(
                self.annual_income_tax_election,
                Some(AnnualIncomeTaxElection::Unrecorded)
            )
        {
            self.annual_income_tax_election = match self.item_13_election {
                Item13Election::EightPercent => Some(AnnualIncomeTaxElection::EightPercent),
                Item13Election::Graduated => Some(AnnualIncomeTaxElection::Graduated),
                Item13Election::Unanswered | Item13Election::NotApplicable => {
                    self.annual_income_tax_election
                }
            };
        }
        // Keep the reviewed plaintext so a later amendment can snapshot it.
        self.submitted_payload = Some(self.to_bir_xml_payload()?);
        self.queued_submission_fingerprint = Some(self.submission_fingerprint());
        self.queue_authorization = Some(QueueAuthorization::new(
            "2551Q",
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
        self.next_retry_at = Some(chrono::Utc::now().to_rfc3339());
        self.last_error = None;
        self.profile_snapshot_stale = false;
        self.profile_snapshot_stale_reason = None;
        self.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    }

    pub fn set_queue_auth_source(&mut self, source: QueueAuthSource) {
        if let Some(auth) = &mut self.queue_authorization {
            auth.source = source;
        }
    }

    /// Transition: Queued → Submitted (called by background cron after successful upload).
    pub fn transition_to_submitted(&mut self, filename: String) {
        assert!(
            matches!(self.status, FilingStatus::Queued),
            "Cannot submit form in {:?} status — must be Queued",
            self.status
        );
        let now = chrono::Utc::now();
        self.status = FilingStatus::Sent;
        self.submitted_at = Some(now.to_rfc3339());
        self.submission_filename = Some(filename);
        self.submission_attempts = 0;
        self.next_retry_at = None;
        self.last_error = None;
        self.submission_claim_token = None;
        self.submission_claimed_at = None;
        self.submission_claim_lease_until = None;
        self.submission_put_started_at = None;
        self.updated_at = now.to_rfc3339();
    }

    /// Transition: Submitted → Confirmed (called when BIR confirmation email is matched).
    pub fn transition_to_confirmed(
        &mut self,
        confirmed_at: String,
        receipt_id: Option<i64>,
        filename: Option<String>,
    ) {
        assert!(
            matches!(self.status, FilingStatus::Sent),
            "Cannot confirm form in {:?} status — must be Sent",
            self.status
        );
        self.status = FilingStatus::Confirmed;
        self.confirmed_at = Some(confirmed_at);
        self.receipt_id = receipt_id;
        if let Some(f) = filename {
            self.submission_filename = Some(f);
        }
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Transition: Confirmed → Paid (called by user action after bank payment).
    pub fn transition_to_paid(&mut self) {
        assert!(
            matches!(self.status, FilingStatus::Confirmed),
            "Cannot mark as paid in {:?} status — must be Confirmed",
            self.status
        );
        self.status = FilingStatus::Paid;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// The editor's Amended toggle: Yes keeps Items 16 and the on-time
    /// answer as entered and recomputes.
    pub fn mark_amended(&mut self) {
        self.is_amended = true;
        self.recompute(None);
    }

    /// Transition: Any non-terminal → Draft (revert). Clears submission metadata.
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
        self.next_retry_at = None;
        self.last_error = None;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Revalidate a queued draft immediately before XML generation/network I/O.
    ///
    /// This protects queued JSON created by older application versions when a
    /// newly introduced legal field deserializes to an unanswered default. It
    /// also refreshes automatic penalties using the current submission date.
    /// If those user-visible amounts changed while the return was waiting, the
    /// return is sent back to Draft for review instead of being silently filed
    /// with either stale or newly changed liability.
    pub fn revalidate_queued_before_submission(&mut self) -> Result<(), Vec<(String, String)>> {
        assert!(
            matches!(self.status, FilingStatus::Queued),
            "Cannot revalidate a form in {:?} status — must be Queued",
            self.status
        );
        let reviewed_values = FilingCalculationSnapshot::from(&*self);
        let reviewed_fingerprint = self.queued_submission_fingerprint.clone();
        self.recompute_internal(self.expected_sales_for_penalties, true);
        let refreshed_values = FilingCalculationSnapshot::from(&*self);
        let refreshed_fingerprint = self.submission_fingerprint();
        // The page's Validate, then the queue-drift checks below.
        let mut errors = self.filing_errors();
        if reviewed_values != refreshed_values {
            errors.push((
                "calculated_values".to_string(),
                "Calculated rates or amounts changed while queued; review the refreshed return and queue it again"
                    .to_string(),
            ));
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
            "2551Q",
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
        self.last_error = Some(format!(
            "Submission blocked by queue revalidation: {summary}"
        ));
        Err(errors)
    }

    /// Record a failed submission attempt with exponential backoff.
    /// After 5 failures, automatically reverts to Draft.
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
        self.last_error = Some(note);
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
            "Cannot record submission failure in {:?} status — must be Queued",
            self.status
        );
        self.submission_attempts += 1;
        self.last_error = Some(error_msg);
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
            let delay_mins = 2i64.pow(self.submission_attempts - 1);
            let next_time = chrono::Utc::now() + chrono::Duration::minutes(delay_mins);
            self.next_retry_at = Some(next_time.to_rfc3339());
        }
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
}

fn cents(value: f64) -> i64 {
    (value * 100.0).round() as i64
}

#[derive(Debug, PartialEq, Eq)]
struct FilingCalculationSnapshot {
    schedule_rates_and_due: Vec<(u64, i64)>,
    total_tax_due: i64,
    total_tax_credits: i64,
    tax_payable: i64,
    surcharge: i64,
    interest: i64,
    compromise: i64,
    total_penalties: i64,
    total_amount_payable: i64,
}

impl From<&Form2551QDraft> for FilingCalculationSnapshot {
    fn from(draft: &Form2551QDraft) -> Self {
        Self {
            schedule_rates_and_due: draft
                .schedule_1
                .iter()
                .map(|row| (row.tax_rate.to_bits(), cents(row.tax_due)))
                .collect(),
            total_tax_due: cents(draft.total_tax_due),
            total_tax_credits: cents(draft.total_tax_credits),
            tax_payable: cents(draft.tax_payable),
            surcharge: cents(draft.surcharge),
            interest: cents(draft.interest),
            compromise: cents(draft.compromise),
            total_penalties: cents(draft.total_penalties),
            total_amount_payable: cents(draft.total_amount_payable),
        }
    }
}

use super::FormValidator;
use crate::validation::{
    OFFICIAL_INVALID_RDO_MESSAGE, OFFICIAL_MIN_FORM_YEAR, OFFICIAL_OLD_VERSION_MESSAGE,
    fits_official_maxlength, rdo_code_is_official_option, validate_email, validate_ph_phone,
    validate_zip,
};

/// `2551q-validate-month` / `2551q-save-month` (official text, verbatim).
pub const OFFICIAL_YEAR_ENDED_MONTH_MESSAGE: &str =
    "Please enter valid Year Ended month on item 2.";
/// `2551q-validate-overpayment-formatted` (official text, verbatim).
pub const OFFICIAL_OVERPAYMENT_OPTION_MESSAGE: &str = "Please select overpayment option.";
/// `2551q-validate-telephone` (official text, verbatim).
pub const OFFICIAL_TELEPHONE_MESSAGE: &str = "Please enter Telephone Number on Item 10.";
/// `2551q-validate-zero-atc` (official text, verbatim).
pub const OFFICIAL_ZERO_ATC_MESSAGE: &str =
    "If you are filing zero transaction, please select the applicable ATC in Page 2 Schedule 1.";

/// What the 2551Q page's text boxes hold for the entries it checks on top of
/// [`FormValidator::validate`] (blank or unparseable text). The page reads
/// them from its inputs; the agent uses [`Self::from_draft`], which is how
/// the page fills those inputs when it opens a draft.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form2551QEntries {
    pub year_end_month: String,
    pub attached_sheets: String,
    /// Schedule 1 taxable amount, one per row.
    pub taxable_amounts: Vec<String>,
    pub creditable_withheld: String,
    pub tax_paid_previous: String,
    pub other_tax_credit: String,
}

impl Form2551QEntries {
    /// The text each input shows for `draft` (a negative amount is a blank box).
    pub fn from_draft(draft: &Form2551QDraft) -> Self {
        let amount = |value: f64| {
            if value >= 0.0 {
                format!("{value:.2}")
            } else {
                String::new()
            }
        };
        Self {
            year_end_month: draft.year_end_month.to_string(),
            attached_sheets: draft.number_of_attached_sheets.to_string(),
            taxable_amounts: draft
                .schedule_1
                .iter()
                .map(|row| amount(row.taxable_amount))
                .collect(),
            creditable_withheld: amount(draft.creditable_tax_withheld),
            tax_paid_previous: amount(draft.tax_paid_previous),
            other_tax_credit: amount(draft.other_tax_credit),
        }
    }
}

impl Form2551QDraft {
    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the page-2 header TIN, one 14-digit comb
    ///   (TIN then branch).
    /// - `derived:address_line1` / `derived:line:address_line2`: Item 9 over
    ///   its 40-slot first line, broken at a space, and the 31-slot second.
    /// - `derived:line:tax_relief_specify`: Item 12A, the relief named on a Yes.
    /// - `derived:sched1_atc{n}` / `derived:sched1_rate{n}`: Schedule 1 row
    ///   `n`'s ATC code and its rate in percent (`3`, `.5`); the upload
    ///   carries the dropdown position instead. Rows past the schedule print
    ///   blank, not `0.00`.
    pub fn to_print_field_map(&self) -> std::collections::BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm2551Qv2018:txtPg2TIN1",
                "frm2551Qv2018:txtPg2TIN2",
                "frm2551Qv2018:txtPg2TIN3",
                "frm2551Qv2018:txtPg2BranchCode",
            ],
        );
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        let specify = if self.tax_relief {
            self.tax_relief_specification.trim().to_string()
        } else {
            String::new()
        };
        let mut derived = vec![
            ("tin_digits".to_string(), tin_digits),
            ("address_line1".to_string(), line1),
            ("line:address_line2".to_string(), line2),
            ("line:tax_relief_specify".to_string(), specify),
        ];
        for n in 1..=6 {
            let Some(row) = self.schedule_1.get(n - 1) else {
                for key in ["txtATCAmt", "txtATCRate", "txtATCDue"] {
                    fields.insert(format!("{key}{n}"), String::new());
                }
                continue;
            };
            let rate = fields
                .get(&format!("txtATCRate{n}"))
                .and_then(|rate| rate.parse::<f64>().ok())
                .filter(|rate| *rate > 0.0)
                .map(|rate| {
                    let text = format!("{rate}");
                    text.strip_prefix('0').unwrap_or(&text).to_string()
                })
                .unwrap_or_default();
            derived.push((format!("sched1_atc{n}"), row.atc.trim().to_uppercase()));
            derived.push((format!("sched1_rate{n}"), rate));
        }
        for (key, value) in derived {
            fields.insert(format!("derived:{key}"), value);
        }
        fields
    }

    /// The page's checks on its entries, run after [`FormValidator::validate`]
    /// (whose errors are passed in so a month error is not repeated).
    pub fn entry_errors(
        &self,
        entries: &Form2551QEntries,
        validate_errors: &[(String, String)],
    ) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        if matches!(self.tax_period_basis, TaxPeriodBasis::Fiscal)
            && entries.year_end_month.trim().parse::<u8>().is_err()
            && !validate_errors
                .iter()
                .any(|(field, _)| field == "year_end_month")
        {
            errors.push((
                "year_end_month".to_string(),
                OFFICIAL_YEAR_ENDED_MONTH_MESSAGE.to_string(),
            ));
        }
        let sheets = entries.attached_sheets.trim();
        if sheets.is_empty() || sheets.parse::<u16>().is_err() {
            errors.push((
                "number_of_attached_sheets".to_string(),
                "Number of attached sheets must be a whole number from 0 to 99".to_string(),
            ));
        }
        for (i, text) in entries.taxable_amounts.iter().enumerate() {
            if text.trim().is_empty() {
                errors.push((
                    format!("schedule_1_row_{}", i + 1),
                    format!("Schedule 1 row {} taxable amount is required", i + 1),
                ));
            } else if text.trim().parse::<f64>().is_err() {
                errors.push((
                    format!("schedule_1_row_{}", i + 1),
                    format!(
                        "Schedule 1 row {} taxable amount must be a valid number",
                        i + 1
                    ),
                ));
            }
        }
        let creditable = entries.creditable_withheld.trim();
        if creditable.is_empty() {
            errors.push((
                "creditable_withheld".to_string(),
                "Creditable percentage tax withheld is required".to_string(),
            ));
        } else if creditable.parse::<f64>().is_err() {
            errors.push((
                "creditable_withheld".to_string(),
                "Creditable percentage tax withheld must be a valid number".to_string(),
            ));
        }
        if self.is_amended {
            let previous = entries.tax_paid_previous.trim();
            if previous.is_empty() {
                errors.push((
                    "tax_paid_previous".to_string(),
                    "Tax paid in return previously filed is required".to_string(),
                ));
            } else if previous.parse::<f64>().is_err() {
                errors.push((
                    "tax_paid_previous".to_string(),
                    "Tax paid in return previously filed must be a valid number".to_string(),
                ));
            }
        }
        let other_credit = entries.other_tax_credit.trim();
        if !other_credit.is_empty() {
            match other_credit.parse::<f64>() {
                Ok(value) if value >= 0.0 => {}
                _ => errors.push((
                    "other_tax_credit".to_string(),
                    "Other tax credit/payment must be a non-negative number".to_string(),
                )),
            }
        }
        errors
    }

    /// The page's Validate: [`FormValidator::validate`] plus the page's entry
    /// checks. `bir-mcp`'s `form.validate`, the shell page, Queue, and the
    /// pre-upload revalidate all use this list. There is no third checker.
    pub fn filing_errors(&self) -> Vec<(String, String)> {
        let mut errors = <Self as super::FormValidator>::validate(self);
        errors.extend(self.entry_errors(&Form2551QEntries::from_draft(self), &errors));
        errors
    }
}

impl FormValidator for Form2551QDraft {
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        if let Some(error) = &self.profile_resolution_error {
            errors.push(("profile_resolution".to_string(), error.clone()));
        }
        if self.profile_snapshot_stale {
            errors.push((
                "profile_snapshot".to_string(),
                self.profile_snapshot_stale_reason.clone().unwrap_or_else(|| {
                    "The taxpayer profile changed after this return was queued; revert or amend it before filing"
                        .to_string()
                }),
            ));
        }
        let today = chrono::Local::now().date_naive();
        if !(1900..=9999).contains(&self.taxable_year) {
            errors.push((
                "taxable_year".to_string(),
                "Taxable year must be a 4-digit year".to_string(),
            ));
        } else if self.taxable_year < OFFICIAL_MIN_FORM_YEAR {
            // `2551q-input-year-revision`: official rewrites <2018 to 2018 at
            // input, so a <2018 period can never reach filing; the gate's
            // rejection preserves that boundary (`2551q-validate-year-min`
            // accepts 1900+ only because input made lower years unreachable).
            errors.push((
                "taxable_year".to_string(),
                OFFICIAL_OLD_VERSION_MESSAGE.to_string(),
            ));
        } else if self
            .filing_period_bounds()
            .is_some_and(|(period_start, _)| period_start > today)
        {
            // Profile: app-stricter. Official `validateItem2()` only alerts on
            // blur when the year-END is after today and keeps the value;
            // `validateForm()` has no future check. Item 2 is the year the
            // calendar/fiscal year ends, so comparing it to today would block
            // Q1–Q3 of a fiscal year ending next calendar year. We reject only
            // a quarter that has not started yet.
            errors.push((
                "taxable_year".to_string(),
                "The selected quarter has not started yet; Items 2 and 3 cannot be a future period."
                    .to_string(),
            ));
        }

        if !(1..=4).contains(&self.quarter) {
            errors.push(("quarter".to_string(), "Quarter is required".to_string()));
        }

        if !(1..=12).contains(&self.year_end_month) {
            errors.push((
                "year_end_month".to_string(),
                OFFICIAL_YEAR_ENDED_MONTH_MESSAGE.to_string(),
            ));
        } else if matches!(self.tax_period_basis, TaxPeriodBasis::Calendar)
            && self.year_end_month != 12
        {
            errors.push((
                "year_end_month".to_string(),
                "Calendar filers must use December as the year-end month".to_string(),
            ));
        }

        if self.number_of_attached_sheets > 99 {
            errors.push((
                "number_of_attached_sheets".to_string(),
                "Number of attached sheets must fit the two-digit Item 5 field".to_string(),
            ));
        }
        let required_schedule_attachments = self.required_schedule_attachment_sheets();
        if self.number_of_attached_sheets < required_schedule_attachments {
            errors.push((
                "number_of_attached_sheets".to_string(),
                format!(
                    "Item 5 must include at least {required_schedule_attachments} Schedule 1 continuation sheet(s) for {} ATC rows",
                    self.schedule_1.len()
                ),
            ));
        }

        // `2551q-validate-treaty`: official rejects relief-Yes with the
        // selector at index zero. `txtTaxReliefSpecify` is a select whose
        // options are `0` (blank), `1` Special Rate and `2` International Tax
        // Treaty (HTA L874–879; no "Both", unlike 1601C), so anything that
        // does not map to 1 or 2 is a value official cannot submit.
        if self.tax_relief
            && crate::validation::official_tax_relief_code(&self.tax_relief_specification, false)
                .is_none()
        {
            errors.push((
                "tax_relief_specification".to_string(),
                "Please specify Tax Relief on Item 12.".to_string(),
            ));
        }

        if !fits_official_maxlength(&self.other_tax_credit_description, 50) {
            errors.push((
                "other_tax_credit_description".to_string(),
                "Other tax credit description exceeds the official 50-character field".to_string(),
            ));
        }

        if self
            .expected_sales_for_penalties
            .is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            errors.push((
                "expected_sales_for_penalties".to_string(),
                "Expected-sales penalty basis must be a finite non-negative amount".to_string(),
            ));
        }

        let item_13_is_applicable = self.item_13_is_applicable();
        match self.annual_income_tax_election {
            None => errors.push((
                "annual_income_tax_election".to_string(),
                "Annual income-tax election snapshot is required; reopen this draft from its taxpayer profile"
                    .to_string(),
            )),
            Some(AnnualIncomeTaxElection::Conflicting) => errors.push((
                "annual_income_tax_election".to_string(),
                "Taxpayer profile contains conflicting 8% and graduated income-tax elections for this taxable year; resolve the profile ledger before filing"
                    .to_string(),
            )),
            Some(AnnualIncomeTaxElection::Unrecorded)
                if self.later_period_requires_recorded_annual_election() =>
            {
                errors.push((
                    "annual_income_tax_election".to_string(),
                    "This later-quarter Section 116 return needs the taxpayer profile's recorded annual income-tax election; save the election for this taxable year before filing"
                        .to_string(),
                ));
            }
            _ => {}
        }
        if self.taxpayer_type.is_none() {
            errors.push((
                "taxpayer_type".to_string(),
                "Taxpayer type snapshot is required to determine Item 13 applicability; reopen this draft from its taxpayer profile"
                    .to_string(),
            ));
        }
        match (item_13_is_applicable, self.item_13_election) {
            (_, Item13Election::Unanswered) => errors.push((
                "item_13_election".to_string(),
                "Choose the applicable Item 13 income-tax-rate election or explicitly select Not applicable"
                    .to_string(),
            )),
            (Some(true), Item13Election::NotApplicable) => errors.push((
                "item_13_election".to_string(),
                "Item 13 applies to an Individual taxpayer with PT010 activity on the initial-quarter return; choose Graduated or Eight percent"
                    .to_string(),
            )),
            (
                Some(false),
                Item13Election::Graduated | Item13Election::EightPercent,
            ) => errors.push((
                "item_13_election".to_string(),
                "Item 13 must be Not applicable unless this is an Individual taxpayer's initial-quarter return with PT010 activity"
                    .to_string(),
            )),
            _ => {}
        }
        if item_13_is_applicable == Some(true) {
            match (self.annual_income_tax_election, self.item_13_election) {
                (
                    Some(AnnualIncomeTaxElection::EightPercent),
                    election,
                ) if election != Item13Election::EightPercent => errors.push((
                    "item_13_election".to_string(),
                    "Item 13 must match the taxpayer profile's recorded 8% election for this taxable year"
                        .to_string(),
                )),
                (
                    Some(AnnualIncomeTaxElection::Graduated),
                    election,
                ) if election != Item13Election::Graduated => errors.push((
                    "item_13_election".to_string(),
                    "Item 13 must match the taxpayer profile's recorded graduated-rate election for this taxable year"
                        .to_string(),
                )),
                _ => {}
            }
        }
        // A later quarter whose filed Q1 is in this app files Q1's answer:
        // Item 13 is answered once a year, on the initial quarter.
        if let Some(filed) = self.filed_q1_item_13() {
            errors.retain(|(field, _)| field != "item_13_election");
            if self.item_13_election != filed {
                errors.push((
                    "item_13_election".to_string(),
                    format!(
                        "Item 13 must match your filed Q1 return for {}",
                        self.taxable_year
                    ),
                ));
            }
        }

        for (key, label, value) in [
            ("tin", "TIN", self.tin.as_str()),
            ("rdo_code", "RDO Code", self.rdo_code.as_str()),
            (
                "taxpayer_name",
                "Taxpayer Name",
                self.taxpayer_name.as_str(),
            ),
            (
                "registered_address",
                "Registered Address",
                self.registered_address.as_str(),
            ),
            ("zip_code", "ZIP Code", self.zip_code.as_str()),
            (
                "contact_number",
                "Contact Number",
                self.contact_number.as_str(),
            ),
            ("email", "Email Address", self.email.as_str()),
        ] {
            if value.trim().is_empty() {
                let message = if key == "contact_number" {
                    // `2551q-validate-telephone`
                    OFFICIAL_TELEPHONE_MESSAGE.to_string()
                } else {
                    format!("{label} is required")
                };
                errors.push((key.to_string(), message));
            }
        }

        // `2551q-validate-rdo` rejects only a blank RDO (handled by the
        // required loop above); its `"000"` check is commented out (HTA
        // L4915), so official accepts the placeholder. Profile: app-stricter —
        // we reject the placeholder and, since our field is free text, any
        // code the rdo.xml-built dropdown could not hold.
        if !self.rdo_code.trim().is_empty() && !rdo_code_is_official_option(&self.rdo_code) {
            errors.push((
                "rdo_code".to_string(),
                OFFICIAL_INVALID_RDO_MESSAGE.to_string(),
            ));
        }

        if !self.zip_code.trim().is_empty() && !validate_zip(&self.zip_code) {
            errors.push((
                "zip_code".to_string(),
                "Zip Code must be 4 digits".to_string(),
            ));
        }

        if !self.contact_number.trim().is_empty() && !validate_ph_phone(&self.contact_number) {
            errors.push((
                "contact_number".to_string(),
                "Contact Number must be valid".to_string(),
            ));
        }

        if !self.email.trim().is_empty() && !validate_email(&self.email) {
            errors.push((
                "email".to_string(),
                "Email Address must be a valid email".to_string(),
            ));
        }

        // Official input caps (maxlength on the official form). The submitted
        // XML cannot represent values the official app cannot produce, so the
        // caps are enforced here even though the frozen HTML renderer can lay
        // out longer text in its plain-box fallback.
        for (field, label, value, capacity) in [
            (
                "taxpayer_name",
                "Taxpayer name",
                self.taxpayer_name.as_str(),
                50,
            ),
            (
                "registered_address",
                "Registered address",
                self.registered_address.as_str(),
                100,
            ),
            ("email", "Email address", self.email.as_str(), 50),
            (
                "contact_number",
                "Contact number",
                self.contact_number.as_str(),
                20,
            ),
        ] {
            if !fits_official_maxlength(value, capacity) {
                errors.push((
                    field.to_string(),
                    format!("{label} exceeds the official {capacity}-character submission limit"),
                ));
            }
        }

        if self.schedule_1.is_empty() {
            errors.push((
                "schedule_1".to_string(),
                // `2551q-validate-zero-atc`
                OFFICIAL_ZERO_ATC_MESSAGE.to_string(),
            ));
        }
        if self.schedule_1.len() > FORM_2551Q_XML_SCHEDULE_ROW_CAPACITY {
            errors.push((
                "schedule_1".to_string(),
                format!(
                    "BIR Form 2551Q XML accepts at most {FORM_2551Q_XML_SCHEDULE_ROW_CAPACITY} Schedule 1 rows; additional rows remain saved and printable as continuation attachments but cannot be queued or submitted until the official XML attachment protocol is verified"
                ),
            ));
        }
        let mut seen_atc_codes = HashSet::new();
        // Item 13 = 8% can only be ticked on the initial-quarter return
        // (`item_13_is_applicable`), which is where official zeroes figures.
        let eight_percent_on_this_return =
            matches!(self.item_13_election, Item13Election::EightPercent);
        for (i, row) in self.schedule_1.iter().enumerate() {
            let field = format!("schedule_1_row_{}", i + 1);
            let normalized_atc = row.atc.trim().to_ascii_uppercase();
            if !normalized_atc.is_empty() && !seen_atc_codes.insert(normalized_atc.clone()) {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} repeats ATC {}; combine taxable amounts into one ATC line",
                        i + 1,
                        normalized_atc
                    ),
                ));
            }
            if !row.taxable_amount.is_finite() || row.taxable_amount < 0.0 {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} taxable amount must be finite and non-negative",
                        i + 1
                    ),
                ));
            } else if !has_cent_precision(row.taxable_amount) {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} taxable amount must have at most two decimal places",
                        i + 1
                    ),
                ));
            }
            if !fits_decimal_comb(row.taxable_amount, 11) {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} taxable amount does not fit the official 11-cell integer field",
                        i + 1
                    ),
                ));
            }
            if !fits_decimal_comb(row.tax_due, 7) {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} tax due does not fit the official 7-cell integer field",
                        i + 1
                    ),
                ));
            }

            // A row with no ATC is the official blank option ("0" in the
            // XML, as for an absent row). It is unused only while it holds
            // no amount: an amount there has no rate and no classification.
            if normalized_atc.is_empty() {
                if row.taxable_amount != 0.0 || row.tax_due != 0.0 {
                    errors.push((
                        field,
                        format!(
                            "Schedule 1 row {} has an amount but no ATC; choose its ATC or clear the amount",
                            i + 1
                        ),
                    ));
                }
                continue;
            }
            let Some(entry) = find_atc(row.atc.trim()) else {
                errors.push((
                    field,
                    format!(
                        "Schedule 1 row {} uses unknown ATC code {} for BIR Form 2551Q January 2018",
                        i + 1,
                        row.atc
                    ),
                ));
                continue;
            };

            if row.atc != entry.code {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} ATC code must use canonical value {}",
                        i + 1,
                        entry.code
                    ),
                ));
            }
            if row.atc_description != entry.description {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} description does not match official ATC {}",
                        i + 1,
                        entry.code
                    ),
                ));
            }
            let Some(rate_resolution) = resolve_2551q_atc_rate(
                entry.code,
                self.taxable_year,
                self.quarter,
                self.year_end_month,
            ) else {
                errors.push((
                    field,
                    format!(
                        "Schedule 1 row {} ATC {} has no rate resolution for this return period",
                        i + 1,
                        entry.code
                    ),
                ));
                continue;
            };
            let AtcRateResolution::Single(expected_rate) = rate_resolution else {
                errors.push((
                    field,
                    format!(
                        "Schedule 1 row {} PT010 spans the July statutory rate boundary; split-period receipts are not safely representable by this draft",
                        i + 1
                    ),
                ));
                continue;
            };

            // The official dropdown submits a list position; a code and rate
            // with no official option cannot be written.
            if super::form_2551q_xml::official_atc_option(entry.code, expected_rate).is_none() {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} ATC {} at {:.1}% is not an option on the official form",
                        i + 1,
                        entry.code,
                        expected_rate * 100.0
                    ),
                ));
            }

            if !row.tax_rate.is_finite()
                || (row.tax_rate - expected_rate).abs() > ATC_RATE_TOLERANCE
            {
                errors.push((
                    field.clone(),
                    format!(
                        "Schedule 1 row {} tax rate does not match official ATC {} rate of {:.2}%",
                        i + 1,
                        entry.code,
                        expected_rate * 100.0
                    ),
                ));
            }

            let expected_tax_due = round_to_cents(row.taxable_amount * expected_rate);
            if !expected_tax_due.is_finite()
                || !row.tax_due.is_finite()
                || (row.tax_due - expected_tax_due).abs() >= TWO_DECIMAL_TOLERANCE
            {
                errors.push((
                    field,
                    format!(
                        "Schedule 1 row {} tax due must equal taxable amount times the official ATC {} rate, rounded to two decimals ({expected_tax_due:.2})",
                        i + 1,
                        entry.code
                    ),
                ));
            }

            let row_has_amounts = row.taxable_amount.abs() >= TWO_DECIMAL_TOLERANCE
                || row.tax_due.abs() >= TWO_DECIMAL_TOLERANCE;
            if eight_percent_on_this_return && row_has_amounts {
                // `2551q-tax-rate-eight-percent`: ticking Item 13 = 8% on this
                // (initial-quarter) return zeroes and disables every ATC row.
                // Profile: app-stricter — official zeroes the rows silently;
                // we reject so no entered amount is discarded unseen.
                errors.push((
                    format!("schedule_1_row_{}", i + 1),
                    format!(
                        "Schedule 1 row {} must be NIL when Item 13 is the 8% option on this return; the official option zeroes every ATC row",
                        i + 1
                    ),
                ));
            } else if matches!(
                self.annual_income_tax_election,
                Some(AnnualIncomeTaxElection::EightPercent)
            ) && entry.code == "PT010"
                && row_has_amounts
            {
                // Later quarters of an 8% year: official has no Item 13 tick
                // to zero anything, but the 8% option is in lieu of Section
                // 116 percentage tax, so PT010 stays NIL. Other ATCs are
                // independently taxable and are left alone.
                errors.push((
                    format!("schedule_1_row_{}", i + 1),
                    "PT010 must be a NIL row for every quarter of a taxable year covered by the 8% income-tax election because that option is in lieu of Section 116 percentage tax"
                        .to_string(),
                ));
            }
        }

        // Items 15/16/17 and 20/21/22 under the 8% option ticked on this
        // return: official `taxRateOption()` zeroes and disables all six.
        // Profile: app-stricter for 15/16/17 — official zeroes them silently;
        // we reject a nonzero entry instead. 20/21/22 are zeroed by
        // `recompute` (official-match) and rejected here only if overridden.
        if eight_percent_on_this_return {
            for (field, label, value) in [
                (
                    "creditable_withheld",
                    "Item 15 creditable tax withheld",
                    self.creditable_tax_withheld,
                ),
                (
                    "tax_paid_previous",
                    "Item 16 tax paid previously",
                    self.tax_paid_previous,
                ),
                (
                    "other_tax_credit",
                    "Item 17 other tax credit/payment",
                    self.other_tax_credit,
                ),
                ("surcharge", "Item 20 surcharge", self.surcharge),
                ("interest", "Item 21 interest", self.interest),
                ("compromise", "Item 22 compromise", self.compromise),
            ] {
                if value.abs() >= TWO_DECIMAL_TOLERANCE {
                    errors.push((
                        field.to_string(),
                        format!(
                            "{label} must be zero under the 8% election; the official option zeroes and disables every listed figure"
                        ),
                    ));
                }
            }
        }

        for (field, label, value) in [
            (
                "creditable_withheld",
                "Creditable percentage tax withheld",
                self.creditable_tax_withheld,
            ),
            (
                "tax_paid_previous",
                "Tax paid in return previously filed",
                self.tax_paid_previous,
            ),
            (
                "other_tax_credit",
                "Other tax credit/payment",
                self.other_tax_credit,
            ),
        ] {
            if !value.is_finite() || value < 0.0 {
                errors.push((
                    field.to_string(),
                    format!("{label} must be finite and non-negative"),
                ));
            } else if !has_cent_precision(value) {
                errors.push((
                    field.to_string(),
                    format!("{label} must have at most two decimal places"),
                ));
            }
        }

        if !self.is_amended && self.tax_paid_previous.abs() >= 0.005 {
            errors.push((
                "tax_paid_previous".to_string(),
                "Tax paid in a previously filed return must be zero unless Amended Return is selected"
                    .to_string(),
            ));
        }

        if self.other_tax_credit > 0.0 && self.other_tax_credit_description.trim().is_empty() {
            errors.push((
                "other_tax_credit_description".to_string(),
                "Item 17 description is required when an other tax credit/payment is entered"
                    .to_string(),
            ));
        }

        if self.total_amount_payable < 0.0 {
            if matches!(self.overpayment_disposition, OverpaymentDisposition::None) {
                errors.push((
                    "overpayment_disposition".to_string(),
                    OFFICIAL_OVERPAYMENT_OPTION_MESSAGE.to_string(),
                ));
            }
        } else if !matches!(self.overpayment_disposition, OverpaymentDisposition::None) {
            errors.push((
                "overpayment_disposition".to_string(),
                "Overpayment disposition is only allowed when Item 24 is an overpayment"
                    .to_string(),
            ));
        }

        for (field, label, value) in [
            ("surcharge", "Item 20 surcharge", self.surcharge),
            ("interest", "Item 21 interest", self.interest),
            ("compromise", "Item 22 compromise", self.compromise),
        ] {
            if !value.is_finite() {
                errors.push((
                    field.to_string(),
                    format!("{label} must be a finite amount"),
                ));
            } else if value < 0.0 {
                errors.push((field.to_string(), format!("{label} must be non-negative")));
            } else if !has_cent_precision(value) {
                errors.push((
                    field.to_string(),
                    format!("{label} must have at most two decimal places"),
                ));
            } else if !fits_decimal_comb(value, 11) {
                errors.push((
                    field.to_string(),
                    format!("{label} does not fit the official 11-cell integer field"),
                ));
            }
        }

        for (field, label, value) in [
            ("total_tax_due", "Item 14 total tax due", self.total_tax_due),
            (
                "creditable_withheld",
                "Item 15 creditable tax withheld",
                self.creditable_tax_withheld,
            ),
            (
                "tax_paid_previous",
                "Item 16 tax paid previously",
                self.tax_paid_previous,
            ),
            (
                "other_tax_credit",
                "Item 17 other tax credit/payment",
                self.other_tax_credit,
            ),
            (
                "total_tax_credits",
                "Item 18 total tax credits/payments",
                self.total_tax_credits,
            ),
            ("tax_payable", "Item 19 tax payable", self.tax_payable),
            (
                "total_penalties",
                "Item 23 total penalties",
                self.total_penalties,
            ),
            (
                "total_amount_payable",
                "Item 24 total amount payable",
                self.total_amount_payable,
            ),
        ] {
            if !fits_decimal_comb(value, 11) {
                errors.push((
                    field.to_string(),
                    format!("{label} does not fit the official 11-cell integer field"),
                ));
            }
        }

        // Derived amounts are persisted for UI/queue state, but none of them
        // may become an independent source of truth at the XML boundary.
        // Validate the complete formula chain against its owned inputs so a
        // stale or tampered draft cannot serialize internally inconsistent
        // Items 14, 18, 19, 23, or 24.
        let expected_total_tax_due = round_to_cents(
            self.schedule_1
                .iter()
                .map(|row| round_to_cents(row.tax_due))
                .sum::<f64>(),
        );
        let expected_total_tax_credits = round_to_cents(
            round_to_cents(self.creditable_tax_withheld)
                + if self.is_amended {
                    round_to_cents(self.tax_paid_previous)
                } else {
                    0.0
                }
                + round_to_cents(self.other_tax_credit),
        );
        let expected_tax_payable =
            round_to_cents(expected_total_tax_due - expected_total_tax_credits);
        let expected_total_penalties = round_to_cents(
            round_to_cents(self.surcharge)
                + round_to_cents(self.interest)
                + round_to_cents(self.compromise),
        );
        let expected_total_amount_payable =
            round_to_cents(expected_tax_payable + expected_total_penalties);

        for (field, label, actual, expected) in [
            (
                "total_tax_due",
                "Item 14 total tax due",
                self.total_tax_due,
                expected_total_tax_due,
            ),
            (
                "total_tax_credits",
                "Item 18 total tax credits/payments",
                self.total_tax_credits,
                expected_total_tax_credits,
            ),
            (
                "tax_payable",
                "Item 19 tax payable/(overpayment)",
                self.tax_payable,
                expected_tax_payable,
            ),
            (
                "total_penalties",
                "Item 23 total penalties",
                self.total_penalties,
                expected_total_penalties,
            ),
            (
                "total_amount_payable",
                "Item 24 total amount payable/(overpayment)",
                self.total_amount_payable,
                expected_total_amount_payable,
            ),
        ] {
            if !matches_cent_rounded(actual, expected) {
                errors.push((
                    field.to_string(),
                    format!(
                        "{label} must equal its Rust-derived value ({:.2})",
                        round_to_cents(expected)
                    ),
                ));
            }
        }

        if self.auto_compute_penalties {
            let mut refreshed = self.clone();
            refreshed.recompute_internal(refreshed.expected_sales_for_penalties, true);
            for (field, label, actual, expected) in [
                (
                    "surcharge",
                    "Item 20 automatic surcharge",
                    self.surcharge,
                    refreshed.surcharge,
                ),
                (
                    "interest",
                    "Item 21 automatic interest",
                    self.interest,
                    refreshed.interest,
                ),
                (
                    "compromise",
                    "Item 22 automatic compromise",
                    self.compromise,
                    refreshed.compromise,
                ),
            ] {
                if !matches_cent_rounded(actual, expected) {
                    errors.push((
                        field.to_string(),
                        format!(
                            "{label} is stale or inconsistent; recompute the return ({expected:.2})"
                        ),
                    ));
                }
            }
        }

        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::Tin;
    use crate::profile::{
        TaxProfileVersion, TaxProfileVersionSource, TaxProfileVersionStatus, TaxpayerProfile,
        TaxpayerType,
    };

    /// A deadline on a weekend moves to the next working day, as the
    /// Calendar's does: Q3 2026's Oct 25 is a Sunday, so the return is due
    /// (and penalty-free) on Monday, Oct 26; Q2 2026's Jul 25 is a Saturday.
    #[test]
    fn a_weekend_deadline_moves_to_the_next_working_day() {
        let day = |m, d| chrono::NaiveDate::from_ymd_opt(2026, m, d);
        let q3 = Form2551QDraft::new_from_profile(&test_profile(), 2026, 3);
        assert_eq!(q3.filing_deadline(), day(10, 26));
        let q2 = Form2551QDraft::new_from_profile(&test_profile(), 2026, 2);
        assert_eq!(q2.filing_deadline(), day(7, 27));
        let q1 = Form2551QDraft::new_from_profile(&test_profile(), 2026, 1);
        assert_eq!(q1.filing_deadline(), day(4, 27));
    }

    /// Penalties run from the deadline the Calendar resolves, global
    /// deadline overrides included: Q3 2026 is due Monday, Oct 26 (Oct 25
    /// is a Sunday), and an override that moves it to Nov 3 makes a return
    /// paid on Nov 3 penalty-free and one paid on Nov 4 late. A queued
    /// return keeps the date it was reviewed with.
    #[test]
    fn penalties_run_from_the_calendars_deadline_with_its_overrides() {
        use crate::forms::penalty_items::with_clock;
        let day = |m, d| chrono::NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        let penalties_on = |draft: &Form2551QDraft, on: NaiveDate| {
            let mut copy = draft.clone();
            with_clock(on, || copy.recompute(None));
            copy.total_penalties
        };
        let mut draft = make_draft(500_000.0, 0.0, 2026, 3);
        draft.apply_penalty_profile(&test_profile(), &[]);
        assert_eq!(draft.penalty_due_date, Some(day(10, 26)));
        assert_eq!(
            penalties_on(&draft, day(10, 26)),
            0.0,
            "paid on the deadline"
        );
        assert!(penalties_on(&draft, day(10, 27)) > 0.0, "a day late");

        let moved = crate::calendar_rules::DeadlineOverride {
            id: "override-test".into(),
            title: "2551Q Q3 2026 deadline".into(),
            source_reference: "Set by the user".into(),
            affected_form_codes: vec!["2551Q".into()],
            original_deadline: day(10, 25),
            adjusted_deadline: day(11, 3),
            affected_regions: Vec::new(),
            affected_taxpayer_types: Vec::new(),
            effective_from: None,
            effective_until: None,
            expires_at: None,
        };
        draft.apply_penalty_profile(&test_profile(), std::slice::from_ref(&moved));
        assert_eq!(draft.penalty_due_date, Some(day(11, 3)));
        assert_eq!(
            penalties_on(&draft, day(11, 3)),
            0.0,
            "paid on the moved deadline"
        );
        assert!(penalties_on(&draft, day(11, 4)) > 0.0, "a day after it");

        // A fiscal quarter has no Calendar row: its own date.
        let mut fiscal = draft.clone();
        fiscal.tax_period_basis = TaxPeriodBasis::Fiscal;
        fiscal.year_end_month = 6;
        fiscal.apply_penalty_profile(&test_profile(), std::slice::from_ref(&moved));
        assert_eq!(fiscal.penalty_due_date, None);
        assert_eq!(
            fiscal.filing_deadline(),
            Some(day(4, 27)),
            "Q3 of a June year ends in March"
        );

        // Past Draft, the reviewed date stays.
        let mut queued = draft.clone();
        queued.status = FilingStatus::Queued;
        queued.apply_penalty_profile(&test_profile(), &[]);
        assert_eq!(queued.penalty_due_date, Some(day(11, 3)));
    }

    fn test_profile() -> TaxpayerProfile {
        TaxpayerProfile {
            id: None,
            full_name: "Test Taxpayer".into(),
            tin: Tin {
                segment1: "123".into(),
                segment2: "456".into(),
                segment3: "789".into(),
                branch: "000".into(),
            },
            rdo_code: "018".into(),
            line_of_business: "Retail".into(),
            registered_address: "Manila".into(),
            zip_code: "1000".into(),
            phone: "09123456789".into(),
            email: "test@example.com".into(),
            default_form_type: "2551Qv2018".into(),
            taxpayer_type: TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: chrono::NaiveDate::from_ymd_opt(2010, 1, 1),
            birth_date: None,
            is_archived: false,
            email_tracking_enabled: false,
            test_notification_enabled: false,
            profile_versions: vec![],
            compliance_source_mode: Default::default(),
            per_year_forms: Default::default(),
            tax_classification: None,
            eopt_tier: None,
            is_bmbe: false,
            is_gpp_partner: false,
            is_create_msme: false,
            is_expanded_withholding_agent: false,
            atc_codes: vec![],
            excise_tax_categories: vec![],
            tax_elections: vec![],
            has_employees: false,
            is_dormant: false,
            has_single_employer: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            registration_activity_status: Default::default(),
        }
    }

    fn confirmed_profile_version(
        profile: &TaxpayerProfile,
        id: &str,
        name: &str,
        rdo_code: &str,
        effective_from: NaiveDate,
        effective_until: Option<NaiveDate>,
    ) -> TaxProfileVersion {
        let mut version = TaxProfileVersion::from_profile_backfill(profile);
        version.id = id.to_string();
        version.label = name.to_string();
        version.source = TaxProfileVersionSource::ManualCor;
        version.status = TaxProfileVersionStatus::Confirmed;
        version.effective_from = Some(effective_from);
        version.effective_until = effective_until;
        version.needs_effective_date_review = false;
        version.cor.registered_name = name.to_string();
        version.cor.rdo_code = rdo_code.to_string();
        version
    }

    /// Helper: create a draft with given taxable_amount, creditable_tax_withheld,
    /// and quarter/year that determines if it's filed on time or late.
    fn make_draft(
        taxable_amount: f64,
        creditable_withheld: f64,
        year: u16,
        quarter: u8,
    ) -> Form2551QDraft {
        let mut draft = Form2551QDraft::new_from_profile(&test_profile(), year, quarter);
        draft.item_13_election = Item13Election::Graduated;
        draft.schedule_1[0].taxable_amount = taxable_amount;
        draft.creditable_tax_withheld = creditable_withheld;
        draft.recompute(None);
        draft
    }

    #[test]
    fn new_draft_uses_safe_explicit_print_defaults() {
        let draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 2);

        assert_eq!(draft.taxpayer_type, Some(TaxpayerType::Individual));
        assert_eq!(
            draft.business_start_date,
            chrono::NaiveDate::from_ymd_opt(2010, 1, 1)
        );
        assert_eq!(draft.tax_period_basis, TaxPeriodBasis::Calendar);
        assert_eq!(draft.year_end_month, 12);
        assert_eq!(draft.number_of_attached_sheets, 0);
        assert!(draft.tax_relief_specification.is_empty());
        assert_eq!(draft.item_13_election, Item13Election::Unanswered);
        assert!(draft.other_tax_credit_description.is_empty());
        assert_eq!(draft.overpayment_disposition, OverpaymentDisposition::None);
        assert_eq!(draft.period_code(), "122026Q2");
        assert_eq!(
            serde_json::to_value(&draft).expect("new draft must serialize")["taxpayer_type"],
            serde_json::Value::String("Individual".to_string())
        );
    }

    #[test]
    fn pre_contract_json_deserializes_with_safe_defaults() {
        let draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 2);
        let mut value = serde_json::to_value(draft).expect("draft must serialize");
        let object = value.as_object_mut().expect("draft must be an object");
        for key in [
            "taxpayer_type",
            "business_start_date",
            "tax_period_basis",
            "year_end_month",
            "number_of_attached_sheets",
            "tax_relief_specification",
            "item_13_election",
            "other_tax_credit_description",
            "overpayment_disposition",
            "expected_sales_for_penalties",
            "annual_income_tax_election",
            "queued_submission_fingerprint",
            "submission_claim_token",
            "submission_claimed_at",
            "effective_profile_version_id",
            "profile_resolution_error",
        ] {
            object.remove(key);
        }

        let restored: Form2551QDraft =
            serde_json::from_value(value).expect("older draft JSON must remain readable");

        assert_eq!(restored.tax_period_basis, TaxPeriodBasis::Calendar);
        assert_eq!(restored.business_start_date, None);
        assert_eq!(restored.taxpayer_type, None);
        assert_eq!(restored.year_end_month, 12);
        assert_eq!(restored.number_of_attached_sheets, 0);
        assert_eq!(restored.item_13_election, Item13Election::Unanswered);
        assert_eq!(restored.expected_sales_for_penalties, None);
        assert_eq!(restored.annual_income_tax_election, None);
        assert_eq!(restored.queued_submission_fingerprint, None);
        assert_eq!(restored.submission_claim_token, None);
        assert_eq!(restored.submission_claimed_at, None);
        assert_eq!(restored.effective_profile_version_id, None);
        assert_eq!(restored.profile_resolution_error, None);
        assert_eq!(
            restored.overpayment_disposition,
            OverpaymentDisposition::None
        );
        let errors = restored.validate();
        assert!(errors.iter().any(|(field, message)| {
            field == "taxpayer_type" && message.contains("snapshot is required")
        }));
        assert!(errors.iter().any(|(field, message)| {
            field == "annual_income_tax_election" && message.contains("snapshot is required")
        }));

        let mut unknown = serde_json::to_value(restored).expect("old draft must serialize");
        unknown["taxpayer_type"] = serde_json::Value::String("UnknownEntity".to_string());
        assert!(
            serde_json::from_value::<Form2551QDraft>(unknown).is_err(),
            "an unrecognized persisted taxpayer type must fail closed"
        );
    }

    #[test]
    fn effective_profile_creation_selects_the_segment_for_q1_and_q3() {
        let mut profile = test_profile();
        profile.profile_versions = vec![
            confirmed_profile_version(
                &profile,
                "first-half",
                "First Half Name",
                "018",
                NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 6, 30),
            ),
            confirmed_profile_version(
                &profile,
                "second-half",
                "Second Half Name",
                "019",
                NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
                None,
            ),
        ];

        let q1 = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);
        let q3 = Form2551QDraft::new_from_effective_profile(&profile, 2026, 3);

        assert_eq!(
            q1.effective_profile_version_id.as_deref(),
            Some("first-half")
        );
        assert_eq!(q1.taxpayer_name, "First Half Name");
        assert_eq!(q1.rdo_code, "018");
        assert!(q1.profile_resolution_error.is_none());
        assert_eq!(
            q3.effective_profile_version_id.as_deref(),
            Some("second-half")
        );
        assert_eq!(q3.taxpayer_name, "Second Half Name");
        assert_eq!(q3.rdo_code, "019");
        assert!(q3.profile_resolution_error.is_none());
    }

    #[test]
    fn first_registration_inside_a_quarter_uses_the_confirmed_registration_date() {
        let mut profile = test_profile();
        let registration_date = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        profile.business_start_date = Some(registration_date);
        profile.profile_versions = vec![confirmed_profile_version(
            &profile,
            "initial-registration",
            "New Registrant",
            "018",
            registration_date,
            None,
        )];

        let draft = Form2551QDraft::new_from_effective_profile(&profile, 2026, 3);

        assert_eq!(
            draft.effective_profile_version_id.as_deref(),
            Some("initial-registration")
        );
        assert_eq!(draft.business_start_date, Some(registration_date));
        assert_eq!(draft.item_13_is_applicable(), Some(true));
        assert!(draft.profile_resolution_error.is_none());
    }

    #[test]
    fn effective_profile_projection_preserves_the_annual_election_ledger() {
        let mut profile = test_profile();
        profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::GraduatedOsd,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "profile_manager".into(),
            });
        profile.profile_versions = vec![confirmed_profile_version(
            &profile,
            "effective",
            "Effective Name",
            "018",
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            None,
        )];

        let draft = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);

        assert_eq!(
            draft.annual_income_tax_election,
            Some(AnnualIncomeTaxElection::Graduated)
        );
        assert_eq!(draft.item_13_election, Item13Election::Graduated);
    }

    #[test]
    fn unresolved_effective_profile_creation_never_uses_flat_prefills() {
        let profile = test_profile();

        let draft = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);

        assert_eq!(draft.tin, profile.tin.full());
        assert!(draft.taxpayer_name.is_empty());
        assert!(draft.rdo_code.is_empty());
        assert!(draft.registered_address.is_empty());
        assert_eq!(draft.taxpayer_type, None);
        assert_eq!(draft.annual_income_tax_election, None);
        assert!(draft.profile_resolution_error.is_some());
        assert!(draft.validate().iter().any(|(field, message)| {
            field == "profile_resolution" && message.contains("No confirmed")
        }));

        let restored: Form2551QDraft = serde_json::from_str(
            &serde_json::to_string(&draft).expect("unresolved draft must serialize"),
        )
        .expect("unresolved draft must deserialize");
        assert_eq!(
            restored.profile_resolution_error,
            draft.profile_resolution_error
        );
        assert_eq!(restored.effective_profile_version_id, None);
    }

    #[test]
    fn unresolved_profile_refresh_clears_an_editable_profile_snapshot() {
        let mut profile = test_profile();
        profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::GraduatedOsd,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "profile_manager".into(),
            });
        profile.profile_versions = vec![confirmed_profile_version(
            &profile,
            "reviewed",
            "Reviewed Snapshot",
            "018",
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            None,
        )];
        let mut draft = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);
        assert_eq!(draft.item_13_election, Item13Election::Graduated);
        assert_eq!(draft.taxpayer_name, "Reviewed Snapshot");

        profile.profile_versions.clear();
        let error = draft
            .reconcile_with_effective_profile(&profile)
            .expect_err("an unresolved confirmed timeline must fail closed");

        assert!(error.contains("No confirmed"));
        assert_eq!(draft.tin, profile.tin.full());
        assert!(draft.taxpayer_name.is_empty());
        assert!(draft.rdo_code.is_empty());
        assert!(draft.registered_address.is_empty());
        assert_eq!(draft.taxpayer_type, None);
        assert_eq!(draft.annual_income_tax_election, None);
        assert_eq!(draft.item_13_election, Item13Election::Unanswered);
        assert_eq!(draft.effective_profile_version_id, None);
        assert_eq!(
            draft.profile_resolution_error.as_deref(),
            Some(error.as_str())
        );
        assert!(!draft.profile_snapshot_stale);
        assert!(draft.profile_snapshot_stale_reason.is_none());
    }

    #[test]
    fn unresolved_profile_refresh_keeps_a_queued_snapshot_immutable() {
        let mut profile = test_profile();
        profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::EightPercent,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "profile_manager".into(),
            });
        profile.profile_versions = vec![confirmed_profile_version(
            &profile,
            "reviewed",
            "Reviewed Snapshot",
            "018",
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            None,
        )];
        let mut draft = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);
        draft.status = FilingStatus::Queued;

        profile.profile_versions.clear();
        let error = draft
            .reconcile_with_effective_profile(&profile)
            .expect_err("an unresolved confirmed timeline must fail closed");

        assert_eq!(draft.taxpayer_name, "Reviewed Snapshot");
        assert_eq!(draft.rdo_code, "018");
        assert_eq!(draft.taxpayer_type, Some(profile.taxpayer_type.clone()));
        assert_eq!(
            draft.annual_income_tax_election,
            Some(AnnualIncomeTaxElection::EightPercent)
        );
        assert_eq!(draft.item_13_election, Item13Election::EightPercent);
        assert_eq!(
            draft.effective_profile_version_id.as_deref(),
            Some("reviewed")
        );
        assert_eq!(
            draft.profile_resolution_error.as_deref(),
            Some(error.as_str())
        );
        assert!(draft.profile_snapshot_stale);
        assert!(
            draft
                .profile_snapshot_stale_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("immutable return snapshot"))
        );
    }

    #[test]
    fn effective_profile_reconciliation_keeps_queued_snapshot_immutable() {
        let mut profile = test_profile();
        profile.profile_versions = vec![confirmed_profile_version(
            &profile,
            "reviewed",
            "Reviewed Snapshot",
            "018",
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            None,
        )];
        let mut draft = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);
        draft.status = FilingStatus::Queued;

        profile.profile_versions = vec![confirmed_profile_version(
            &profile,
            "replacement",
            "Replacement Profile",
            "019",
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            None,
        )];
        // Queue-boundary callers still use the compatibility method. Once a
        // production draft owns version audit state it must delegate back to
        // effective-dated reconciliation without mutating the snapshot.
        draft.sync_with_profile(&profile);

        assert_eq!(draft.taxpayer_name, "Reviewed Snapshot");
        assert_eq!(draft.rdo_code, "018");
        assert_eq!(
            draft.effective_profile_version_id.as_deref(),
            Some("reviewed")
        );
        assert!(draft.profile_snapshot_stale);
        assert!(draft.profile_resolution_error.is_none());
    }

    #[test]
    fn fiscal_quarters_resolve_the_correct_calendar_year_segment() {
        let mut profile = test_profile();
        profile.profile_versions = vec![
            confirmed_profile_version(
                &profile,
                "calendar-2025",
                "Calendar 2025",
                "018",
                NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
                NaiveDate::from_ymd_opt(2025, 12, 31),
            ),
            confirmed_profile_version(
                &profile,
                "calendar-2026",
                "Calendar 2026",
                "019",
                NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                None,
            ),
        ];
        let mut draft = Form2551QDraft::new_from_profile(&profile, 2026, 1);
        draft.tax_period_basis = TaxPeriodBasis::Fiscal;
        draft.year_end_month = 6;

        assert_eq!(
            draft.filing_period_bounds(),
            Some((
                NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
                NaiveDate::from_ymd_opt(2025, 9, 30).unwrap(),
            ))
        );
        draft
            .reconcile_with_effective_profile(&profile)
            .expect("fiscal Q1 belongs entirely to the 2025 segment");
        assert_eq!(
            draft.effective_profile_version_id.as_deref(),
            Some("calendar-2025")
        );

        draft.quarter = 3;
        assert_eq!(
            draft.filing_period_bounds(),
            Some((
                NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            ))
        );
        draft
            .reconcile_with_effective_profile(&profile)
            .expect("fiscal Q3 belongs entirely to the 2026 segment");
        assert_eq!(
            draft.effective_profile_version_id.as_deref(),
            Some("calendar-2026")
        );
    }

    #[test]
    fn profile_sync_refreshes_the_editable_taxpayer_type_snapshot() {
        let mut draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 1);
        let mut updated_profile = test_profile();
        updated_profile.taxpayer_type = TaxpayerType::Corporation;

        draft.sync_with_profile(&updated_profile);

        assert_eq!(draft.taxpayer_type, Some(TaxpayerType::Corporation));
        assert_eq!(draft.item_13_is_applicable(), Some(false));
    }

    #[test]
    fn profile_sync_maps_saved_osd_and_itemized_elections_to_item_13_graduated() {
        for election in [
            IncomeTaxElection::GraduatedOsd,
            IncomeTaxElection::GraduatedItemized,
        ] {
            let mut draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 1);
            let mut updated_profile = test_profile();
            updated_profile
                .tax_elections
                .push(crate::profile::TaxElectionHistory {
                    taxable_year: 2026,
                    election,
                    elected_at: chrono::NaiveDateTime::default(),
                    source_form: "profile_manager".into(),
                });

            draft.sync_with_profile(&updated_profile);

            assert_eq!(draft.item_13_election, Item13Election::Graduated);
            assert_eq!(
                draft.annual_income_tax_election,
                Some(AnnualIncomeTaxElection::Graduated)
            );
        }
    }

    /// The profile reconciliation and the page's profile follow agree: a
    /// box the filer edited on the return (`profile_edits`) keeps the edit
    /// through a sync, the others take the profile and their baseline with
    /// them, and a queued return whose only difference is such an edit is
    /// not marked stale.
    #[test]
    fn profile_sync_keeps_the_returns_own_edits_and_moves_the_baseline() {
        let mut draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 1);
        draft.profile_edits = vec!["frm2551Qv2018:txtRDOCode".to_string()];
        draft.rdo_code = "040".to_string();
        draft.profile_baseline = std::collections::BTreeMap::from([
            (
                "/rdo_code".to_string(),
                serde_json::json!(test_profile().rdo_code),
            ),
            (
                "/zip_code".to_string(),
                serde_json::json!(test_profile().zip_code),
            ),
        ]);
        let mut changed = test_profile();
        changed.rdo_code = "043".to_string();
        changed.zip_code = "1200".to_string();
        draft.sync_with_profile(&changed);
        assert_eq!(draft.rdo_code, "040", "the edit on this return is kept");
        assert_eq!(draft.zip_code, "1200", "an unedited box follows");
        assert_eq!(draft.profile_baseline["/zip_code"], "1200");
        assert_eq!(
            draft.profile_baseline["/rdo_code"],
            serde_json::json!(test_profile().rdo_code),
            "the edited box's baseline is left alone"
        );
        // Queued with that edit: the snapshot still matches the profile.
        draft.status = FilingStatus::Queued;
        draft.sync_with_profile(&changed);
        assert!(
            !draft.profile_snapshot_stale,
            "{:?}",
            draft.profile_snapshot_stale_reason
        );
        assert_eq!(draft.rdo_code, "040");
        // An unedited box that differs still marks it stale.
        changed.zip_code = "1300".to_string();
        draft.sync_with_profile(&changed);
        assert!(draft.profile_snapshot_stale);
    }

    #[test]
    fn profile_sync_applies_a_saved_eight_percent_election_to_item_13() {
        let mut draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 1);
        let mut updated_profile = test_profile();
        updated_profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::EightPercent,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "profile_manager".into(),
            });

        draft.sync_with_profile(&updated_profile);

        assert_eq!(draft.item_13_election, Item13Election::EightPercent);
    }

    #[test]
    fn profile_sync_clears_item_13_when_later_quarter_context_is_unknown() {
        let mut draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 1);
        draft.quarter = 2;
        draft.business_start_date = None;
        draft.item_13_election = Item13Election::Graduated;

        let mut updated_profile = test_profile();
        updated_profile.business_start_date = None;
        draft.sync_with_profile(&updated_profile);

        assert_eq!(draft.item_13_election, Item13Election::Unanswered);
    }

    #[test]
    fn profile_sync_keeps_later_quarter_item_13_blank_until_election_is_recorded() {
        let profile = test_profile();
        let mut draft = Form2551QDraft::new_from_profile(&profile, 2026, 2);
        draft.item_13_election = Item13Election::NotApplicable;

        draft.sync_with_profile(&profile);

        assert_eq!(draft.item_13_election, Item13Election::Unanswered);
        assert!(draft.validate().iter().any(|(field, message)| {
            field == "annual_income_tax_election"
                && message.contains("later-quarter Section 116 return")
        }));
    }

    #[test]
    fn profile_sync_marks_later_quarter_item_13_not_applicable_after_recorded_election() {
        let mut profile = test_profile();
        profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::GraduatedOsd,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "profile_manager".into(),
            });
        let mut draft = Form2551QDraft::new_from_profile(&profile, 2026, 2);

        draft.sync_with_profile(&profile);

        assert_eq!(draft.item_13_election, Item13Election::NotApplicable);
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "annual_income_tax_election")
        );
    }

    #[test]
    fn fiscal_year_end_drives_period_code_and_quarter_deadline() {
        let mut draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 1);
        draft.tax_period_basis = TaxPeriodBasis::Fiscal;
        draft.year_end_month = 6;

        assert_eq!(draft.period_code(), "062026Q1");
        // Oct 25, 2025 is a Saturday: due the Monday after
        assert_eq!(
            draft.filing_deadline(),
            chrono::NaiveDate::from_ymd_opt(2025, 10, 27)
        );

        draft.quarter = 4;
        // Jul 25, 2026 is a Saturday too
        assert_eq!(
            draft.filing_deadline(),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 27)
        );
    }

    #[test]
    fn validation_requires_conditional_descriptions() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.tax_relief = true;
        draft.other_tax_credit = 10.0;
        draft.recompute(None);

        let errors = draft.validate();
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "tax_relief_specification")
        );
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "other_tax_credit_description")
        );

        draft.tax_relief_specification = "Special Rate".to_string();
        draft.other_tax_credit_description = "Prior payment".to_string();
        let errors = draft.validate();
        assert!(
            errors
                .iter()
                .all(|(field, _)| field != "tax_relief_specification"
                    && field != "other_tax_credit_description")
        );
    }

    #[test]
    fn validation_accepts_values_that_use_adaptive_print_text_boxes() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.tax_relief = true;
        draft.tax_relief_specification = "2".to_string();
        draft.taxpayer_name = "N".repeat(41);
        draft.registered_address = "A".repeat(72);
        draft.email = "abcdefghijklmnopqrst@example.com".to_string();

        let errors = draft.validate();
        for field in [
            "tax_relief_specification",
            "taxpayer_name",
            "registered_address",
            "email",
        ] {
            assert!(
                errors.iter().all(|(error_field, _)| error_field != field),
                "adaptive print text must accept {field}: {errors:?}"
            );
        }
    }

    #[test]
    fn validation_enforces_reviewed_submission_limits_not_comb_capacities() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.tax_relief = true;
        draft.tax_relief_specification = "X".repeat(101);
        draft.taxpayer_name = "N".repeat(101);
        draft.registered_address = "A".repeat(201);
        draft.email = format!("{}@example.com", "e".repeat(89));
        draft.contact_number = "9".repeat(21);

        let errors = draft.validate();
        // `txtTaxReliefSpecify` records no official maxlength, so it is
        // deliberately absent from this list.
        for field in [
            "taxpayer_name",
            "registered_address",
            "email",
            "contact_number",
        ] {
            assert!(
                errors.iter().any(|(error_field, message)| {
                    error_field == field && message.contains("official")
                }),
                "expected an official submission-limit error for {field}: {errors:?}"
            );
        }
    }

    #[test]
    fn item_13_applicability_uses_taxpayer_type_quarter_and_pt010_activity() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        assert_eq!(draft.item_13_is_applicable(), Some(true));

        // A NIL PT010 return is still a Sec. 116 activity return.
        draft.schedule_1[0].taxable_amount = 0.0;
        draft.recompute(None);
        assert_eq!(draft.item_13_is_applicable(), Some(true));

        draft.quarter = 2;
        assert_eq!(draft.item_13_is_applicable(), Some(false));

        draft.quarter = 1;
        draft.schedule_1 = vec![Schedule1Row::new("PT040").expect("PT040 must exist")];
        assert_eq!(draft.item_13_is_applicable(), Some(false));

        draft.schedule_1 = vec![Schedule1Row::default_pt010()];
        draft.taxpayer_type = Some(TaxpayerType::Corporation);
        assert_eq!(draft.item_13_is_applicable(), Some(false));

        draft.taxpayer_type = None;
        assert_eq!(draft.item_13_is_applicable(), None);
    }

    #[test]
    fn new_registrant_can_make_item_13_election_on_the_commencement_quarter() {
        let mut profile = test_profile();
        profile.business_start_date = chrono::NaiveDate::from_ymd_opt(2026, 8, 15);

        for (quarter, expected) in [(1, false), (2, false), (3, true), (4, false)] {
            let draft = Form2551QDraft::new_from_profile(&profile, 2026, quarter);
            assert_eq!(
                draft.item_13_is_applicable(),
                Some(expected),
                "unexpected Item 13 applicability for Q{quarter}"
            );
        }

        let mut initial_return = Form2551QDraft::new_from_profile(&profile, 2026, 3);
        initial_return.item_13_election = Item13Election::Graduated;
        initial_return.schedule_1[0].taxable_amount = 50_000.0;
        initial_return.recompute(None);
        assert!(
            initial_return
                .validate()
                .iter()
                .all(|(field, _)| field != "item_13_election" && field != "business_start_date")
        );
    }

    #[test]
    fn later_quarter_without_business_start_accepts_the_filers_item_13() {
        // The redesigned tax profile records no commencement date; a later
        // quarter without one is an existing registrant's return and must not
        // block filing on a box the official form never asks for.
        let mut profile = test_profile();
        profile.business_start_date = None;
        let mut draft = Form2551QDraft::new_from_profile(&profile, 2026, 2);
        draft.item_13_election = Item13Election::NotApplicable;

        assert_eq!(draft.item_13_is_applicable(), None);
        for election in [
            Item13Election::NotApplicable,
            Item13Election::Graduated,
            Item13Election::EightPercent,
        ] {
            draft.item_13_election = election;
            assert!(
                draft
                    .validate()
                    .iter()
                    .all(|(field, _)| field != "business_start_date" && field != "item_13_election"),
                "{election:?} must be accepted when the start date is unknown"
            );
        }
    }

    #[test]
    fn validation_enforces_the_exact_item_13_election_matrix() {
        let mut applicable = make_draft(50_000.0, 0.0, 2026, 1);
        for election in [Item13Election::Graduated, Item13Election::EightPercent] {
            applicable.item_13_election = election;
            assert!(
                applicable
                    .validate()
                    .iter()
                    .all(|(field, _)| field != "item_13_election"),
                "{election:?} must be accepted when Item 13 applies"
            );
        }
        for election in [Item13Election::NotApplicable, Item13Election::Unanswered] {
            applicable.item_13_election = election;
            assert!(
                applicable
                    .validate()
                    .iter()
                    .any(|(field, _)| field == "item_13_election"),
                "{election:?} must be rejected when Item 13 applies"
            );
        }

        let mut inapplicable_cases = Vec::new();

        let mut later_quarter = make_draft(50_000.0, 0.0, 2026, 1);
        later_quarter.quarter = 2;
        later_quarter.annual_income_tax_election = Some(AnnualIncomeTaxElection::Graduated);
        inapplicable_cases.push(("later quarter".to_string(), later_quarter));

        let mut without_pt010 = make_draft(50_000.0, 0.0, 2026, 1);
        without_pt010.schedule_1 = vec![Schedule1Row::new("PT040").expect("PT040 must exist")];
        without_pt010.recompute(None);
        inapplicable_cases.push(("no PT010 activity".to_string(), without_pt010));

        for taxpayer_type in [
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
            TaxpayerType::Cooperative,
            TaxpayerType::Estate,
            TaxpayerType::Trust,
        ] {
            let mut non_individual = make_draft(50_000.0, 0.0, 2026, 1);
            non_individual.taxpayer_type = Some(taxpayer_type.clone());
            inapplicable_cases.push((format!("{taxpayer_type:?} taxpayer"), non_individual));
        }

        for (case, mut draft) in inapplicable_cases {
            draft.item_13_election = Item13Election::NotApplicable;
            assert!(
                draft
                    .validate()
                    .iter()
                    .all(|(field, _)| field != "item_13_election"),
                "NotApplicable must be accepted for {case}"
            );

            for election in [
                Item13Election::Unanswered,
                Item13Election::Graduated,
                Item13Election::EightPercent,
            ] {
                draft.item_13_election = election;
                assert!(
                    draft
                        .validate()
                        .iter()
                        .any(|(field, _)| field == "item_13_election"),
                    "{election:?} must be rejected for {case}"
                );
            }
        }
    }

    #[test]
    fn eight_percent_election_requires_all_atc_rows_and_figures_nil() {
        // `2551q-tax-rate-eight-percent`: the official option zeroes and
        // disables every ATC row and every listed figure; the return is
        // compliance-only, so non-PT010 activity is rejected too.
        let mut positive_pt010 = make_draft(50_000.0, 0.0, 2026, 1);
        positive_pt010.item_13_election = Item13Election::EightPercent;
        let errors = positive_pt010.validate();
        assert!(errors.iter().any(|(field, message)| {
            field == "schedule_1_row_1" && message.contains("must be NIL")
        }));

        let mut other_activity = make_draft(0.0, 0.0, 2026, 1);
        other_activity.item_13_election = Item13Election::EightPercent;
        let mut row = Schedule1Row::new("PT040").expect("PT040 must exist");
        row.taxable_amount = 50_000.0;
        other_activity.schedule_1.push(row);
        other_activity.recompute(None);
        let errors = other_activity.validate();
        assert!(
            errors.iter().any(|(field, _)| field == "schedule_1_row_2"),
            "non-PT010 activity is also zeroed by the official 8% option: {errors:?}"
        );

        let mut credited = make_draft(0.0, 0.0, 2026, 1);
        credited.item_13_election = Item13Election::EightPercent;
        credited.creditable_tax_withheld = 100.0;
        credited.auto_compute_penalties = false;
        credited.compromise = 1_000.0;
        let errors = credited.validate();
        for field in ["creditable_withheld", "compromise"] {
            assert!(
                errors.iter().any(|(error_field, _)| error_field == field),
                "{field} is zeroed by the official 8% option: {errors:?}"
            );
        }

        // Automatic penalties are zeroed like official `taxRateOption()`.
        let mut late = make_draft(0.0, 0.0, 2024, 1);
        late.item_13_election = Item13Election::EightPercent;
        late.recompute(None);
        assert_eq!(
            (late.surcharge, late.interest, late.compromise),
            (0.0, 0.0, 0.0)
        );

        let mut valid = make_draft(0.0, 0.0, 2026, 1);
        valid.item_13_election = Item13Election::EightPercent;
        let errors = valid.validate();
        assert!(
            errors
                .iter()
                .all(|(field, _)| !field.starts_with("schedule_1_row_")),
            "an all-NIL Schedule 1 must validate under the 8% election: {errors:?}"
        );
    }

    #[test]
    fn schedule_1_row_with_an_amount_needs_an_atc() {
        fn blank_row(taxable_amount: f64) -> Schedule1Row {
            Schedule1Row {
                atc: String::new(),
                atc_description: String::new(),
                taxable_amount,
                tax_rate: 0.0,
                tax_due: 0.0,
            }
        }
        let message =
            "Schedule 1 row 2 has an amount but no ATC; choose its ATC or clear the amount";

        // An unclassified amount has no rate: it must not validate, queue,
        // or survive the pre-submission revalidation.
        let mut unclassified = make_draft(50_000.0, 0.0, 2026, 1);
        unclassified.schedule_1.push(blank_row(5_000.0));
        unclassified.recompute(None);
        let errors = unclassified.validate();
        assert!(
            errors
                .iter()
                .any(|(field, text)| field == "schedule_1_row_2" && text == message),
            "{errors:?}"
        );
        let errors = unclassified
            .clone()
            .transition_to_queued()
            .expect_err("an amount without an ATC must not queue");
        assert!(errors.iter().any(|(field, _)| field == "schedule_1_row_2"));

        let mut queued = make_draft(50_000.0, 0.0, 2026, 1);
        queued.schedule_1.push(blank_row(0.0));
        queued.recompute(None);
        queued
            .transition_to_queued()
            .expect("a blank row with no amount should queue");
        queued.schedule_1[1].taxable_amount = 5_000.0;
        let errors = queued
            .revalidate_queued_before_submission()
            .expect_err("an amount without an ATC must fail queue revalidation");
        assert!(
            errors
                .iter()
                .any(|(field, text)| field == "schedule_1_row_2" && text == message),
            "{errors:?}"
        );

        // A row with neither an ATC nor an amount is simply unused.
        let mut unused = make_draft(50_000.0, 0.0, 2026, 1);
        unused.schedule_1.push(blank_row(0.0));
        unused.recompute(None);
        let errors = unused.validate();
        assert!(
            errors.iter().all(|(field, _)| field != "schedule_1_row_2"),
            "{errors:?}"
        );
    }

    #[test]
    fn pt010_recompute_uses_the_period_specific_statutory_rate() {
        for (year, quarter, expected_rate) in [
            (2020, 2, 0.03),
            (2020, 3, 0.01),
            (2023, 2, 0.01),
            (2023, 3, 0.03),
        ] {
            let mut profile = test_profile();
            profile
                .tax_elections
                .push(crate::profile::TaxElectionHistory {
                    taxable_year: year,
                    election: IncomeTaxElection::GraduatedUnspecified,
                    elected_at: chrono::NaiveDateTime::default(),
                    source_form: "test_fixture".into(),
                });
            let mut draft = Form2551QDraft::new_from_profile(&profile, year, quarter);
            draft.item_13_election = if quarter == 1 {
                Item13Election::Graduated
            } else {
                Item13Election::NotApplicable
            };
            draft.schedule_1[0].taxable_amount = 100_000.0;
            draft.recompute(None);

            assert_eq!(draft.schedule_1[0].tax_rate, expected_rate);
            assert_eq!(draft.schedule_1[0].tax_due, 100_000.0 * expected_rate);
            assert!(
                draft.validate().is_empty(),
                "{year} Q{quarter} must validate"
            );
        }
    }

    #[test]
    fn constructors_canonicalize_the_temporary_pt010_rate_before_preview() {
        let mut fresh = Form2551QDraft::new_from_profile(&test_profile(), 2021, 3);
        assert_eq!(fresh.schedule_1[0].tax_rate, 0.01);
        fresh.schedule_1[0].taxable_amount = 100_000.0;
        fresh.recompute(None);
        assert_eq!(fresh.schedule_1[0].tax_rate, 0.01);
        assert_eq!(fresh.schedule_1[0].tax_due, 1_000.0);

        let mut q2 = Form2551QDraft::new_from_profile(&test_profile(), 2021, 2);
        q2.schedule_1[0].taxable_amount = 100_000.0;
        q2.recompute(None);
        assert_eq!(q2.schedule_1[0].tax_rate, 0.01);
        q2.schedule_1[0].tax_rate = 0.03;
        q2.schedule_1[0].tax_due = 3_000.0;

        let carried =
            Form2551QDraft::new_from_profile(&test_profile(), 2021, 3).with_carried_forward(&q2);
        assert_eq!(carried.schedule_1[0].tax_rate, 0.01);
        assert_eq!(carried.schedule_1[0].tax_due, 1_000.0);
    }

    #[test]
    fn recorded_annual_election_controls_item_13_and_later_pt010() {
        let mut eight_percent_profile = test_profile();
        eight_percent_profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::EightPercent,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "2551Qv2018".into(),
            });

        let q1 = Form2551QDraft::new_from_profile(&eight_percent_profile, 2026, 1);
        assert_eq!(q1.item_13_election, Item13Election::EightPercent);
        assert_eq!(
            q1.annual_income_tax_election,
            Some(AnnualIncomeTaxElection::EightPercent)
        );

        let mut q2 = Form2551QDraft::new_from_profile(&eight_percent_profile, 2026, 2);
        q2.item_13_election = Item13Election::NotApplicable;
        q2.schedule_1[0].taxable_amount = 100.0;
        q2.recompute(None);
        assert!(q2.validate().iter().any(|(field, message)| {
            field == "schedule_1_row_1" && message.contains("NIL row")
        }));

        // Later quarters keep independently taxable non-PT010 activity: the
        // official 8% tick that zeroes every row exists only on Q1.
        let mut q2_other = Form2551QDraft::new_from_profile(&eight_percent_profile, 2026, 2);
        q2_other.item_13_election = Item13Election::NotApplicable;
        let mut row = Schedule1Row::new("PT040").expect("PT040 must exist");
        row.taxable_amount = 50_000.0;
        q2_other.schedule_1.push(row);
        q2_other.recompute(None);
        let errors = q2_other.validate();
        assert!(
            errors.iter().all(|(field, _)| field != "schedule_1_row_2"),
            "a later-quarter 8% filer's non-PT010 activity must not be rejected: {errors:?}"
        );
    }

    #[test]
    fn conflicting_annual_election_ledger_fails_closed_in_either_order() {
        for reverse in [false, true] {
            let mut profile = test_profile();
            let mut elections = vec![
                crate::profile::TaxElectionHistory {
                    taxable_year: 2026,
                    election: IncomeTaxElection::EightPercent,
                    elected_at: chrono::NaiveDateTime::default(),
                    source_form: "2551Qv2018".into(),
                },
                crate::profile::TaxElectionHistory {
                    taxable_year: 2026,
                    election: IncomeTaxElection::GraduatedOsd,
                    elected_at: chrono::NaiveDateTime::default(),
                    source_form: "1701Q".into(),
                },
            ];
            if reverse {
                elections.reverse();
            }
            profile.tax_elections = elections;

            let draft = Form2551QDraft::new_from_profile(&profile, 2026, 1);
            assert_eq!(
                draft.annual_income_tax_election,
                Some(AnnualIncomeTaxElection::Conflicting)
            );
            assert!(draft.validate().iter().any(|(field, message)| {
                field == "annual_income_tax_election" && message.contains("conflicting")
            }));
        }
    }

    #[test]
    fn fiscal_pt010_quarter_crossing_a_rate_boundary_fails_closed() {
        let mut draft = Form2551QDraft::new_from_profile(&test_profile(), 2020, 4);
        draft.tax_period_basis = TaxPeriodBasis::Fiscal;
        draft.year_end_month = 8;
        draft.item_13_election = Item13Election::NotApplicable;
        draft.schedule_1[0].taxable_amount = 100_000.0;
        draft.recompute(None);

        let errors = draft.validate();
        assert!(errors.iter().any(|(field, message)| {
            field == "schedule_1_row_1" && message.contains("split-period")
        }));
    }

    #[test]
    fn validation_fails_closed_without_a_taxpayer_type_snapshot() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.taxpayer_type = None;

        for election in [
            Item13Election::Unanswered,
            Item13Election::NotApplicable,
            Item13Election::Graduated,
            Item13Election::EightPercent,
        ] {
            draft.item_13_election = election;
            let errors = draft.validate();
            assert!(
                errors.iter().any(|(field, message)| {
                    field == "taxpayer_type" && message.contains("snapshot is required")
                }),
                "missing taxpayer type must fail closed for {election:?}"
            );
        }
    }

    #[test]
    fn queue_boundary_rejects_item_13_choices_that_conflict_with_applicability() {
        let mut applicable = make_draft(50_000.0, 0.0, 2026, 1);
        applicable.item_13_election = Item13Election::NotApplicable;
        let errors = applicable
            .transition_to_queued()
            .expect_err("NotApplicable must not queue when Item 13 applies");
        assert!(errors.iter().any(|(field, _)| field == "item_13_election"));

        let mut inapplicable = make_draft(50_000.0, 0.0, 2026, 1);
        inapplicable.quarter = 2;
        let errors = inapplicable
            .transition_to_queued()
            .expect_err("Graduated must not queue when Item 13 is inapplicable");
        assert!(errors.iter().any(|(field, _)| field == "item_13_election"));
    }

    #[test]
    fn queue_revalidation_reverts_an_upgraded_unanswered_draft() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft
            .transition_to_queued()
            .expect("the explicit graduated election should queue");
        draft.item_13_election = Item13Election::Unanswered;

        let errors = draft
            .revalidate_queued_before_submission()
            .expect_err("an upgraded unanswered draft must fail closed");

        assert!(errors.iter().any(|(field, _)| field == "item_13_election"));
        assert_eq!(draft.status, FilingStatus::Draft);
        assert!(
            draft
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("queue revalidation"))
        );
        assert_eq!(draft.submission_attempts, 0);
        assert!(draft.next_retry_at.is_none());
    }

    #[test]
    fn queued_snapshot_is_immutable_when_current_profile_changes() {
        let mut profile = test_profile();
        profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::GraduatedUnspecified,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "2551Qv2018".into(),
            });
        let mut draft = Form2551QDraft::new_from_profile(&profile, 2026, 1);
        draft.schedule_1[0].taxable_amount = 50_000.0;
        draft.recompute(None);
        assert_eq!(
            draft.annual_income_tax_election,
            Some(AnnualIncomeTaxElection::Graduated)
        );

        draft
            .transition_to_queued()
            .expect("the reviewed graduated choice should queue");
        let original_name = draft.taxpayer_name.clone();
        let original_tier = draft.eopt_tier.clone();
        let original_election = draft.annual_income_tax_election;

        let mut changed_profile = profile;
        changed_profile.full_name = "Changed After Queue".to_string();
        changed_profile.eopt_tier = Some(crate::profile::EoptTier::Micro);
        changed_profile.tax_elections.clear();
        changed_profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::EightPercent,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "profile_manager".into(),
            });
        draft.sync_with_profile(&changed_profile);

        assert_eq!(draft.taxpayer_name, original_name);
        assert_eq!(draft.eopt_tier, original_tier);
        assert_eq!(draft.annual_income_tax_election, original_election);
        assert!(draft.profile_snapshot_stale);
        assert_eq!(draft.status, FilingStatus::Queued);
    }

    #[test]
    fn stale_queued_profile_snapshot_fails_validation_without_rewriting_values() {
        let mut profile = test_profile();
        profile.eopt_tier = Some(crate::profile::EoptTier::Medium);
        profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::GraduatedUnspecified,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "2551Qv2018".into(),
            });
        let mut draft = Form2551QDraft::new_from_profile(&profile, 2026, 1);
        draft
            .transition_to_queued()
            .expect("the reviewed return should queue");

        let mut changed_profile = profile;
        changed_profile.eopt_tier = Some(crate::profile::EoptTier::Micro);
        draft.sync_with_profile(&changed_profile);
        assert_eq!(draft.eopt_tier, Some(crate::profile::EoptTier::Medium));
        assert!(draft.profile_snapshot_stale);

        let errors = draft
            .revalidate_queued_before_submission()
            .expect_err("a changed EOPT tier must require another review");
        assert!(errors.iter().any(|(field, _)| field == "profile_snapshot"));
        assert_eq!(draft.status, FilingStatus::Draft);
        assert!(draft.profile_snapshot_stale);
    }

    #[test]
    fn queue_revalidation_reverts_a_legacy_draft_missing_taxpayer_type() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft
            .transition_to_queued()
            .expect("the fully owned Item 13 election should queue");
        draft.taxpayer_type = None;

        let errors = draft
            .revalidate_queued_before_submission()
            .expect_err("a queued legacy draft without taxpayer type must fail closed");

        assert!(errors.iter().any(|(field, message)| {
            field == "taxpayer_type" && message.contains("snapshot is required")
        }));
        assert_eq!(draft.status, FilingStatus::Draft);
        assert!(
            draft
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("taxpayer_type"))
        );
    }

    #[test]
    fn queue_revalidation_fails_closed_when_auto_penalties_change() {
        let mut draft = make_draft(50_000.0, 0.0, 2020, 1);
        draft
            .transition_to_queued()
            .expect("the late return should queue with computed penalties");
        assert!(draft.total_penalties > 0.0);

        // Model a queued record whose automatic penalties became stale while
        // waiting to submit. Revalidation must refresh the values, but must not
        // transmit the changed liability without another user review.
        draft.surcharge = 0.0;
        draft.interest = 0.0;
        draft.compromise = 0.0;
        draft.total_penalties = 0.0;
        draft.total_amount_payable = draft.tax_payable;

        let errors = draft
            .revalidate_queued_before_submission()
            .expect_err("changed automatic penalties must require review");

        assert!(errors.iter().any(|(field, _)| field == "calculated_values"));
        assert_eq!(draft.status, FilingStatus::Draft);
        assert!(draft.total_penalties > 0.0);
        assert!(
            draft
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("Calculated rates or amounts changed"))
        );
    }

    #[test]
    fn queue_revalidation_preserves_the_persisted_fraud_penalty_basis() {
        let mut draft = make_draft(50_000.0, 0.0, 2020, 1);
        draft.recompute(Some(100_000.0));
        let fraud_surcharge = draft.surcharge;
        assert_eq!(draft.expected_sales_for_penalties, Some(100_000.0));
        assert!(fraud_surcharge > 0.0);

        draft
            .transition_to_queued()
            .expect("fraud-reviewed values should queue with their persisted basis");
        assert_eq!(draft.surcharge, fraud_surcharge);
        draft
            .revalidate_queued_before_submission()
            .expect("revalidation must reuse the persisted expected-sales basis");
        assert_eq!(draft.status, FilingStatus::Queued);
        assert_eq!(draft.surcharge, fraud_surcharge);
    }

    #[test]
    fn queue_revalidation_detects_tampered_totals_and_rates() {
        for tamper in ["total", "rate"] {
            let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
            draft
                .transition_to_queued()
                .expect("the reviewed return should queue");

            match tamper {
                "total" => draft.total_tax_due += 1.0,
                "rate" => draft.schedule_1[0].tax_rate = 0.01,
                _ => unreachable!(),
            }

            let errors = draft
                .revalidate_queued_before_submission()
                .expect_err("any changed reviewed calculation must return to Draft");
            assert!(
                errors.iter().any(|(field, _)| field == "calculated_values"),
                "{tamper} tamper was not detected: {errors:?}"
            );
            assert_eq!(draft.status, FilingStatus::Draft);
        }
    }

    #[test]
    fn queue_fingerprint_binds_inputs_even_when_totals_do_not_change() {
        for tamper in [
            "zero_rate_amount",
            "same_rate_atc",
            "credit_split",
            "penalty_mode",
            "business_start",
        ] {
            let mut draft = make_draft(50_000.0, 300.0, 2026, 1);
            draft.other_tax_credit = 200.0;
            draft.other_tax_credit_description = "Adjustment".into();
            if tamper == "zero_rate_amount" {
                draft.schedule_1 = vec![Schedule1Row::new("PT102").expect("PT102 must exist")];
                draft.schedule_1[0].taxable_amount = 1_000.0;
                draft.item_13_election = Item13Election::NotApplicable;
                draft.creditable_tax_withheld = 0.0;
                draft.other_tax_credit = 0.0;
                draft.other_tax_credit_description.clear();
            }
            draft.recompute(None);
            draft
                .transition_to_queued()
                .expect("the reviewed return should queue");

            match tamper {
                "zero_rate_amount" => draft.schedule_1[0].taxable_amount = 2_000.0,
                "same_rate_atc" => {
                    let amount = draft.schedule_1[0].taxable_amount;
                    draft.schedule_1[0] = Schedule1Row::new("PT040").expect("PT040 must exist");
                    draft.schedule_1[0].taxable_amount = amount;
                    draft.schedule_1[0].recompute();
                }
                "credit_split" => {
                    draft.creditable_tax_withheld = 400.0;
                    draft.other_tax_credit = 100.0;
                }
                "penalty_mode" => draft.auto_compute_penalties = false,
                "business_start" => {
                    draft.business_start_date = chrono::NaiveDate::from_ymd_opt(2009, 1, 1)
                }
                _ => unreachable!(),
            }

            let errors = draft
                .revalidate_queued_before_submission()
                .expect_err("changed reviewed inputs must return to Draft");
            assert!(
                errors
                    .iter()
                    .any(|(field, _)| field == "queued_submission_fingerprint"),
                "{tamper} was not bound by the queue fingerprint: {errors:?}"
            );
            assert_eq!(draft.status, FilingStatus::Draft);
        }
    }

    #[test]
    fn queue_revalidation_rejects_legacy_records_without_a_fingerprint() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft
            .transition_to_queued()
            .expect("the reviewed return should queue");
        draft.queued_submission_fingerprint = None;

        let errors = draft
            .revalidate_queued_before_submission()
            .expect_err("an unbound queued record must fail closed");
        assert!(errors.iter().any(|(field, message)| {
            field == "queued_submission_fingerprint" && message.contains("no review fingerprint")
        }));
        assert_eq!(draft.status, FilingStatus::Draft);
    }

    #[test]
    fn queue_revalidation_catches_a_new_profile_election_before_submission() {
        let mut original_profile = test_profile();
        original_profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::GraduatedUnspecified,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "2551Qv2018".into(),
            });
        let mut draft = Form2551QDraft::new_from_profile(&original_profile, 2026, 2);
        draft.item_13_election = Item13Election::NotApplicable;
        draft.schedule_1[0].taxable_amount = 50_000.0;
        draft.recompute(None);
        draft
            .transition_to_queued()
            .expect("the reviewed Q2 PT010 return should initially queue");

        let mut changed_profile = test_profile();
        changed_profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: IncomeTaxElection::EightPercent,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "2551Qv2018".into(),
            });
        draft.sync_with_profile(&changed_profile);

        let errors = draft
            .revalidate_queued_before_submission()
            .expect_err("a later annual 8% election must stop queued PT010 submission");
        assert!(errors.iter().any(|(field, _)| field == "profile_snapshot"));
        assert_eq!(draft.status, FilingStatus::Draft);
    }

    #[test]
    fn validation_rejects_sub_cent_monetary_inputs() {
        for field in ["schedule", "creditable", "previous", "other", "surcharge"] {
            let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
            match field {
                "schedule" => draft.schedule_1[0].taxable_amount = 50_000.004,
                "creditable" => draft.creditable_tax_withheld = 0.004,
                "previous" => {
                    draft.is_amended = true;
                    draft.tax_paid_previous = 0.004;
                }
                "other" => {
                    draft.other_tax_credit = 0.004;
                    draft.other_tax_credit_description = "Adjustment".into();
                }
                "surcharge" => {
                    draft.auto_compute_penalties = false;
                    draft.surcharge = 0.004;
                }
                _ => unreachable!(),
            }
            draft.recompute(None);
            if field == "surcharge" {
                draft.surcharge = 0.004;
            }

            assert!(
                draft
                    .validate()
                    .iter()
                    .any(|(_, message)| message.contains("at most two decimal places")),
                "{field} sub-cent input was accepted"
            );
        }
    }

    #[test]
    fn validation_rejects_negative_other_tax_credit() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.other_tax_credit = -1.0;
        draft.recompute(None);

        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "other_tax_credit")
        );
    }

    #[test]
    fn queue_rejects_negative_or_non_finite_manual_penalties() {
        let cases = [
            ("surcharge", -1.0),
            ("interest", -1.0),
            ("compromise", -1.0),
            ("surcharge", f64::NAN),
            ("interest", f64::INFINITY),
            ("compromise", f64::NEG_INFINITY),
        ];

        for (field, value) in cases {
            let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
            draft.auto_compute_penalties = false;
            match field {
                "surcharge" => draft.surcharge = value,
                "interest" => draft.interest = value,
                "compromise" => draft.compromise = value,
                _ => unreachable!("test case uses a known penalty field"),
            }
            if value.is_finite() {
                let persisted = serde_json::to_string(&draft)
                    .expect("a finite manual-penalty draft should serialize");
                draft = serde_json::from_str(&persisted)
                    .expect("a persisted manual-penalty draft should deserialize");
            }

            let errors = draft.transition_to_queued().expect_err(
                "manual negative and non-finite penalties must fail the queue boundary",
            );

            assert!(
                errors.iter().any(|(error_field, _)| error_field == field),
                "expected {field} to be rejected for {value:?}; got {errors:?}"
            );
            assert_eq!(draft.status, FilingStatus::Draft);
        }
    }

    #[test]
    fn queue_boundary_recomputes_stale_derived_values() {
        let mut draft = make_draft(10_000.0, 0.0, 2026, 1);
        draft.auto_compute_penalties = false;
        draft.surcharge = 0.0;
        draft.interest = 0.0;
        draft.compromise = 0.0;
        draft.schedule_1[0].taxable_amount = 50_000.0;
        draft.schedule_1[0].tax_due = 1.0;
        draft.total_tax_due = 1.0;
        draft.tax_payable = 1.0;
        draft.total_amount_payable = 1.0;

        draft
            .transition_to_queued()
            .expect("queue boundary should repair Rust-derived values");

        assert_eq!(draft.schedule_1[0].tax_due, 1_500.0);
        assert_eq!(draft.total_tax_due, 1_500.0);
        assert_eq!(draft.tax_payable, 1_500.0);
        assert_eq!(draft.total_amount_payable, 1_500.0);
    }

    #[test]
    fn non_amended_item_16_must_be_zero() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.is_amended = false;
        draft.tax_paid_previous = 500.0;

        let errors = draft
            .transition_to_queued()
            .expect_err("a non-amended Item 16 value must not queue");

        assert!(errors.iter().any(|(field, _)| field == "tax_paid_previous"));
    }

    #[test]
    fn validation_rejects_values_that_overflow_official_amount_combs() {
        let draft = make_draft(100_000_000_000_000.0, 0.0, 2026, 1);

        let errors = draft.validate();

        assert!(errors.iter().any(|(field, message)| {
            field == "schedule_1_row_1" && message.contains("11-cell")
        }));
        assert!(
            errors
                .iter()
                .any(|(field, message)| field == "total_tax_due" && message.contains("11-cell"))
        );
    }

    #[test]
    fn validation_enforces_overpayment_disposition_at_queue_boundary() {
        let mut draft = make_draft(50_000.0, 4_000.0, 2026, 1);
        assert!(draft.total_amount_payable < 0.0);

        let errors = draft
            .transition_to_queued()
            .expect_err("overpayment without a disposition must not queue");
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "overpayment_disposition")
        );

        draft.overpayment_disposition = OverpaymentDisposition::TaxCreditCertificate;
        draft
            .transition_to_queued()
            .expect("one overpayment disposition should satisfy the queue gate");
    }

    fn messages(draft: &Form2551QDraft, field: &str) -> Vec<String> {
        draft
            .validate()
            .into_iter()
            .filter(|(key, _)| key == field)
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn overpayment_without_option_uses_the_official_alert() {
        let draft = make_draft(50_000.0, 4_000.0, 2026, 1);
        assert_eq!(
            messages(&draft, "overpayment_disposition"),
            vec!["Please select overpayment option.".to_string()]
        );
    }

    #[test]
    fn invalid_year_ended_month_uses_the_official_alert() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.year_end_month = 0;
        assert_eq!(
            messages(&draft, "year_end_month"),
            vec!["Please enter valid Year Ended month on item 2.".to_string()]
        );
    }

    #[test]
    fn blank_telephone_uses_the_official_alert() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.contact_number.clear();
        assert_eq!(
            messages(&draft, "contact_number"),
            vec!["Please enter Telephone Number on Item 10.".to_string()]
        );
    }

    #[test]
    fn no_atc_uses_the_official_zero_transaction_alert() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.schedule_1.clear();
        assert!(messages(&draft, "schedule_1").contains(
            &"If you are filing zero transaction, please select the applicable ATC in Page 2 Schedule 1."
                .to_string()
        ));
    }

    #[test]
    fn validation_forbids_disposition_without_an_overpayment() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        assert!(draft.total_amount_payable > 0.0);
        draft.overpayment_disposition = OverpaymentDisposition::Refund;

        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "overpayment_disposition")
        );
    }

    #[test]
    fn validation_bounds_period_and_attached_sheet_fields() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.year_end_month = 6;
        draft.number_of_attached_sheets = 100;

        let errors = draft.validate();
        assert!(errors.iter().any(|(field, _)| field == "year_end_month"));
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "number_of_attached_sheets")
        );

        draft.tax_period_basis = TaxPeriodBasis::Fiscal;
        draft.number_of_attached_sheets = 99;
        let errors = draft.validate();
        assert!(errors
            .iter()
            .all(|(field, _)| field != "year_end_month" && field != "number_of_attached_sheets"));
    }

    #[test]
    fn scenario_1_filed_on_time_with_overpayment() {
        // Automatic penalties are disabled so this exercises only the
        // credit/payable arithmetic of an on-time filing.
        let mut draft = make_draft(50_000.0, 4_000.0, 2026, 1);
        draft.auto_compute_penalties = false;
        draft.surcharge = 0.0;
        draft.interest = 0.0;
        draft.compromise = 0.0;
        draft.recompute(None);

        // Line 14: 50000 * 3% = 1500
        assert_eq!(draft.total_tax_due, 1500.0);
        // Line 18: 4000 (creditable) + 0 (previous) + 0 (other)
        assert_eq!(draft.total_tax_credits, 4000.0);
        // Line 19: 1500 - 4000 = -2500 (overpayment, NOT clamped)
        assert_eq!(draft.tax_payable, -2500.0);
        // Filed on time → all penalties = 0
        assert_eq!(draft.surcharge, 0.0);
        assert_eq!(draft.interest, 0.0);
        assert_eq!(draft.compromise, 0.0);
        assert_eq!(draft.total_penalties, 0.0);
        // Line 24: -2500 + 0 = -2500
        assert_eq!(draft.total_amount_payable, -2500.0);
    }

    #[test]
    fn scenario_2_filed_late_with_overpayment() {
        // Use a past quarter to guarantee late filing
        let mut draft = make_draft(50_000.0, 4_000.0, 2020, 1);
        draft.recompute(None);

        // Line 14: 1500
        assert_eq!(draft.total_tax_due, 1500.0);
        // Line 19: -2500 (overpayment)
        assert_eq!(draft.tax_payable, -2500.0);
        // Filed late but no unpaid tax → surcharge=0, interest=0
        assert_eq!(draft.surcharge, 0.0);
        assert_eq!(draft.interest, 0.0);
        // Compromise from "no amount due" tier: gross_sales=50000 ≤ 100000 → 1000
        assert_eq!(draft.compromise, 1000.0);
        assert_eq!(draft.total_penalties, 1000.0);
        // Line 24: -2500 + 1000 = -1500 (net overpayment)
        assert_eq!(draft.total_amount_payable, -1500.0);
    }

    #[test]
    fn scenario_3_filed_late_with_tax_due() {
        // Credits < tax due, past quarter
        let mut draft = make_draft(50_000.0, 400.0, 2020, 1);
        draft.recompute(None);

        // Line 14: 1500
        assert_eq!(draft.total_tax_due, 1500.0);
        // Line 18: 400
        assert_eq!(draft.total_tax_credits, 400.0);
        // Line 19: 1500 - 400 = 1100
        assert_eq!(draft.tax_payable, 1100.0);
        // Filed late with unpaid tax → surcharge, interest, and compromise apply
        assert!(
            draft.surcharge > 0.0,
            "surcharge should be positive for late filing with tax due"
        );
        assert!(
            draft.interest > 0.0,
            "interest should be positive for late filing with tax due"
        );
        assert!(
            draft.compromise > 0.0,
            "compromise should be positive for late filing with tax due"
        );
        // Line 24 = Line 19 + Line 23
        let expected_24 = ((draft.tax_payable + draft.total_penalties) * 100.0).round() / 100.0;
        assert_eq!(draft.total_amount_payable, expected_24);
        assert!(draft.total_amount_payable > draft.tax_payable);
    }

    #[test]
    fn zero_tax_filed_on_time_no_penalties() {
        let mut draft = make_draft(0.0, 0.0, 2026, 1);
        draft.auto_compute_penalties = false;
        draft.surcharge = 0.0;
        draft.interest = 0.0;
        draft.compromise = 0.0;
        draft.recompute(None);

        assert_eq!(draft.total_tax_due, 0.0);
        assert_eq!(draft.tax_payable, 0.0);
        assert_eq!(draft.surcharge, 0.0);
        assert_eq!(draft.interest, 0.0);
        assert_eq!(draft.compromise, 0.0);
        assert_eq!(draft.total_amount_payable, 0.0);
    }

    #[test]
    fn multiple_atc_rows_sum_correctly() {
        let mut draft = Form2551QDraft::new_from_profile(&test_profile(), 2026, 1);
        // Row 1: PT010 at 3%
        draft.schedule_1[0].taxable_amount = 100_000.0;
        // Row 2: PT105 at 5%
        if let Some(row) = Schedule1Row::new("PT105") {
            draft.schedule_1.push(row);
        }
        draft.schedule_1[1].taxable_amount = 200_000.0;
        draft.recompute(None);

        // PT010: 100000 * 3% = 3000
        assert_eq!(draft.schedule_1[0].tax_due, 3000.0);
        // PT105: 200000 * 5% = 10000
        assert_eq!(draft.schedule_1[1].tax_due, 10000.0);
        // Line 14: 3000 + 10000 = 13000
        assert_eq!(draft.total_tax_due, 13000.0);
    }

    #[test]
    fn validation_rejects_noncanonical_atc_schedule_data() {
        let mut draft = make_draft(10_000.0, 0.0, 2026, 1);

        draft.schedule_1[0].atc = "PT999".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .any(|(_, message)| message.contains("unknown ATC code PT999"))
        );

        draft.schedule_1[0] = Schedule1Row::default_pt010();
        draft.schedule_1[0].taxable_amount = 10_000.0;
        draft.schedule_1[0].recompute();
        draft.schedule_1[0].atc_description = "Invented description".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .any(|(_, message)| message
                    .contains("description does not match official ATC PT010"))
        );

        draft.schedule_1[0] = Schedule1Row::default_pt010();
        draft.schedule_1[0].taxable_amount = 10_000.0;
        draft.schedule_1[0].recompute();
        draft.schedule_1[0].tax_rate = 0.04;
        assert!(
            draft
                .validate()
                .iter()
                .any(|(_, message)| message.contains("tax rate does not match official ATC PT010"))
        );

        draft.schedule_1[0] = Schedule1Row::default_pt010();
        draft.schedule_1[0].taxable_amount = 10_000.0;
        draft.schedule_1[0].recompute();
        draft.schedule_1[0].tax_due += 0.006;
        assert!(
            draft
                .validate()
                .iter()
                .any(|(_, message)| message.contains("tax due must equal taxable amount"))
        );
    }

    #[test]
    fn validation_accepts_tax_due_within_half_cent_two_decimal_tolerance() {
        let mut draft = make_draft(10_000.0, 0.0, 2026, 1);
        draft.schedule_1[0].tax_due += 0.004;

        assert!(
            draft
                .validate()
                .iter()
                .all(|(_, message)| !message.contains("tax due must equal taxable amount"))
        );
    }

    #[test]
    fn validation_accepts_six_schedule_rows_supported_by_xml() {
        let mut draft = make_draft(100_000.0, 0.0, 2026, 1);
        draft.schedule_1 = ["PT010", "PT040", "PT060", "PT090", "PT140", "PT180"]
            .into_iter()
            .map(|code| Schedule1Row::new(code).expect("test ATC must be canonical"))
            .collect();

        let errors = draft.validate();

        assert!(
            errors.iter().all(|(field, _)| field != "schedule_1"),
            "unexpected schedule validation errors: {errors:?}"
        );
    }

    #[test]
    fn schedule_continuation_sheet_minimum_preserves_user_owned_attachments() {
        let mut draft = make_draft(100_000.0, 0.0, 2026, 1);
        draft.schedule_1 = ["PT010", "PT040", "PT041", "PT060", "PT070", "PT090"]
            .into_iter()
            .map(|code| Schedule1Row::new(code).expect("test ATC must be canonical"))
            .collect();
        assert_eq!(draft.required_schedule_attachment_sheets(), 0);
        assert!(!draft.ensure_required_schedule_attachment_sheets());

        draft
            .schedule_1
            .push(Schedule1Row::new("PT140").expect("test ATC must be canonical"));
        assert_eq!(draft.required_schedule_attachment_sheets(), 1);
        assert!(draft.ensure_required_schedule_attachment_sheets());
        assert_eq!(draft.number_of_attached_sheets, 1);

        draft.number_of_attached_sheets = 4;
        draft
            .schedule_1
            .truncate(FORM_2551Q_XML_SCHEDULE_ROW_CAPACITY);
        assert_eq!(draft.required_schedule_attachment_sheets(), 0);
        assert!(!draft.ensure_required_schedule_attachment_sheets());
        assert_eq!(
            draft.number_of_attached_sheets, 4,
            "removing Schedule rows must not erase unrelated attachments"
        );
    }

    #[test]
    fn validation_rejects_duplicate_schedule_atcs() {
        let mut draft = make_draft(100_000.0, 0.0, 2026, 1);
        draft.schedule_1 = vec![Schedule1Row::default_pt010(), Schedule1Row::default_pt010()];

        let errors = draft.validate();

        assert!(errors.iter().any(|(field, message)| {
            field == "schedule_1_row_2"
                && message.contains("repeats ATC PT010")
                && message.contains("combine taxable amounts")
        }));
    }

    #[test]
    fn validation_rejects_schedule_rows_beyond_verified_xml_capacity() {
        let mut draft = make_draft(100_000.0, 0.0, 2026, 1);
        draft.schedule_1 = [
            "PT010", "PT040", "PT041", "PT060", "PT070", "PT090", "PT140", "PT150", "PT160",
            "PT170",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, code)| {
            let mut row = Schedule1Row::new(code).expect("test ATC must be canonical");
            row.taxable_amount = (index as f64 + 1.0) * 1_000.0;
            row
        })
        .collect();
        assert_eq!(draft.required_schedule_attachment_sheets(), 1);
        draft.ensure_required_schedule_attachment_sheets();
        draft.recompute(None);

        let errors = draft.validate();

        assert!(errors.iter().any(|(field, message)| {
            field == "schedule_1"
                && message.contains("at most 6")
                && message.contains("cannot be queued or submitted")
        }));
        assert_eq!(draft.total_tax_due, 6_110.0);

        let queue_errors = draft
            .transition_to_queued()
            .expect_err("ten printable rows must remain outside the verified XML queue boundary");
        assert!(queue_errors.iter().any(|(field, message)| {
            field == "schedule_1" && message.contains("official XML attachment protocol")
        }));
        assert_eq!(draft.status, FilingStatus::Draft);
    }

    #[test]
    fn validation_requires_item_5_to_count_generated_schedule_attachments() {
        let mut draft = make_draft(100_000.0, 0.0, 2026, 1);
        draft.schedule_1 = [
            "PT010", "PT040", "PT041", "PT060", "PT070", "PT090", "PT140", "PT150", "PT160",
            "PT170",
        ]
        .into_iter()
        .map(|code| Schedule1Row::new(code).expect("test ATC must be canonical"))
        .collect();
        draft.number_of_attached_sheets = 0;

        assert!(draft.validate().iter().any(|(field, message)| {
            field == "number_of_attached_sheets"
                && message.contains("at least 1")
                && message.contains("10 ATC rows")
        }));

        draft.number_of_attached_sheets = 1;
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "number_of_attached_sheets")
        );
    }

    #[test]
    fn other_tax_credit_reduces_payable() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.other_tax_credit = 500.0;
        draft.recompute(None);

        // Line 14: 1500
        assert_eq!(draft.total_tax_due, 1500.0);
        // Line 18: 0 + 0 + 500 = 500
        assert_eq!(draft.total_tax_credits, 500.0);
        // Line 19: 1500 - 500 = 1000
        assert_eq!(draft.tax_payable, 1000.0);
    }

    #[test]
    fn total_tax_credits_includes_all_three_sources() {
        let mut draft = make_draft(50_000.0, 1000.0, 2026, 1);
        draft.is_amended = true;
        draft.tax_paid_previous = 200.0;
        draft.other_tax_credit = 300.0;
        draft.recompute(None);

        // Line 18: 1000 + 200 + 300 = 1500
        assert_eq!(draft.total_tax_credits, 1500.0);
        // Line 19: 1500 - 1500 = 0
        assert_eq!(draft.tax_payable, 0.0);
    }

    #[test]
    fn tax_paid_previous_only_counted_when_amended() {
        let mut draft = make_draft(50_000.0, 1000.0, 2026, 1);
        draft.is_amended = false;
        draft.tax_paid_previous = 500.0; // should be ignored
        draft.recompute(None);

        // Line 18 should NOT include tax_paid_previous
        assert_eq!(draft.total_tax_credits, 1000.0);
        assert_eq!(draft.tax_payable, 500.0); // 1500 - 1000
    }

    #[test]
    fn official_parity_rejects_out_of_range_year_and_placeholder_rdo() {
        let draft = make_draft(50_000.0, 0.0, 2017, 1);
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "taxable_year")
        );

        // A quarter that has not started yet is rejected.
        let mut draft = make_draft(
            50_000.0,
            0.0,
            u16::try_from(chrono::Utc::now().year() + 1).expect("next year fits u16"),
            1,
        );
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "taxable_year")
        );

        draft.rdo_code = "000".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "rdo_code")
        );

        // The official dropdown holds only rdo.xml codes.
        draft.rdo_code = "999".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "rdo_code")
        );

        draft.rdo_code = "018".to_string();
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "rdo_code")
        );
    }

    #[test]
    fn official_parity_rejects_unselected_tax_relief_selector() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.tax_relief = true;

        // `txtTaxReliefSpecify` options: 0 (blank), 1 Special Rate, 2
        // International Tax Treaty (HTA L874–879).
        for spec in ["1", "2", "Special Rate", "International Tax Treaty"] {
            draft.tax_relief_specification = spec.to_string();
            assert!(
                draft
                    .validate()
                    .iter()
                    .all(|(field, _)| field != "tax_relief_specification"),
                "spec {spec:?} should validate"
            );
        }

        draft.tax_relief_specification = String::new();
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "tax_relief_specification"),
            "a relief-Yes return with the selector unselected must be rejected"
        );

        // "0" is the unselected option; 2551Q has no "Both"; free text is not
        // an option official can submit.
        for spec in ["0", "3", "Both", "Special law 123"] {
            draft.tax_relief_specification = spec.to_string();
            assert!(
                draft
                    .validate()
                    .iter()
                    .any(|(field, _)| field == "tax_relief_specification"),
                "spec {spec:?} must be rejected"
            );
        }
    }

    #[test]
    fn fiscal_year_ending_next_year_accepts_started_quarters() {
        // Item 2 is the year the fiscal year ends. A fiscal year ending in
        // twelve months from now has its Q1 already under way.
        let today = chrono::Local::now().date_naive();
        let fiscal_end = today
            .checked_add_months(chrono::Months::new(11))
            .expect("date in range");
        let mut draft = make_draft(
            50_000.0,
            0.0,
            u16::try_from(fiscal_end.year()).expect("year fits u16"),
            1,
        );
        draft.tax_period_basis = TaxPeriodBasis::Fiscal;
        draft.year_end_month = u8::try_from(fiscal_end.month()).expect("month fits u8");
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "taxable_year"),
            "a started fiscal Q1 must pass even when the fiscal year ends next calendar year"
        );

        draft.quarter = 4;
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "taxable_year"),
            "a fiscal quarter that has not started must be rejected"
        );
    }

    #[test]
    fn official_parity_enforces_field_length_caps() {
        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.taxpayer_name = "N".repeat(51);
        draft.registered_address = "A".repeat(101);
        draft.email = format!("{}@example.com", "e".repeat(40));
        let errors = draft.validate();
        for field in ["taxpayer_name", "registered_address", "email"] {
            assert!(
                errors.iter().any(|(error_field, _)| error_field == field),
                "expected an error for {field}: {errors:?}"
            );
        }
    }

    /// Rule 8 on 2551Q: Items 20-22 compute by default; a component the
    /// filer enters files instead and the others keep computing; Items 23
    /// and 24 add up what is filed; clearing it computes again.
    #[test]
    fn an_entered_penalty_files_instead_and_the_others_keep_computing() {
        // Q1 2024 is long past its deadline: every component is due.
        let computed = make_draft(500_000.0, 0.0, 2024, 1);
        assert!(computed.surcharge > 0.0 && computed.interest > 0.0 && computed.compromise > 0.0);
        assert_eq!(computed.penalty_overrides, PenaltyOverrides::default());

        let mut entered = computed.clone();
        entered.penalty_overrides.interest = Some(123.45);
        entered.recompute(None);
        assert_eq!(entered.interest, 123.45);
        assert_eq!(entered.surcharge, computed.surcharge);
        assert_eq!(entered.compromise, computed.compromise);
        assert_eq!(
            entered.total_penalties,
            round_to_cents(computed.surcharge + 123.45 + computed.compromise)
        );
        assert_eq!(
            entered.total_amount_payable,
            round_to_cents(entered.tax_payable + entered.total_penalties)
        );
        assert!(entered.validate().iter().all(|(field, _)| {
            !matches!(field.as_str(), "surcharge" | "interest" | "compromise")
        }));
        // The upload and the print carry what is filed.
        let map = entered.to_bir_field_map();
        assert_eq!(map["frm2551Qv2018:txt21"], "123.45");

        entered.penalty_overrides.interest = None;
        entered.recompute(None);
        assert_eq!(
            (entered.surcharge, entered.interest, entered.compromise),
            (computed.surcharge, computed.interest, computed.compromise)
        );
    }

    /// A return whose deadline has not come computes no penalty.
    #[test]
    fn a_return_that_is_not_late_computes_no_penalty() {
        let next = u16::try_from(chrono::Local::now().year() + 1).unwrap();
        let draft = make_draft(500_000.0, 0.0, next, 4);
        assert_eq!(
            (
                draft.surcharge,
                draft.interest,
                draft.compromise,
                draft.total_penalties
            ),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(draft.total_amount_payable, draft.tax_payable);
    }

    /// A draft saved with the manual switch files the same amounts: each
    /// typed amount becomes that component's override.
    #[test]
    fn a_manual_penalty_draft_keeps_its_amounts_as_overrides() {
        let mut draft = make_draft(500_000.0, 0.0, 2024, 1);
        draft.auto_compute_penalties = false;
        draft.surcharge = 10.0;
        draft.interest = 0.0;
        draft.compromise = 1_000.0;
        draft.recompute(None);
        assert!(draft.auto_compute_penalties);
        assert_eq!(
            (draft.surcharge, draft.interest, draft.compromise),
            (10.0, 0.0, 1_000.0)
        );
        assert_eq!(draft.penalty_overrides.surcharge, Some(10.0));
        assert_eq!(draft.penalty_overrides.interest, Some(0.0));
        assert_eq!(draft.penalty_overrides.compromise, Some(1_000.0));
        assert_eq!(draft.total_penalties, 1_010.0);
    }

    /// A manual draft on Item 13 = 8% files what it filed before: the 8%
    /// option takes no override, so it stays manual until its election
    /// changes, and then adopts its amounts like any manual draft.
    #[test]
    fn a_manual_eight_percent_draft_keeps_its_amounts() {
        let mut draft = make_draft(0.0, 0.0, 2024, 1);
        draft.item_13_election = Item13Election::EightPercent;
        draft.auto_compute_penalties = false;
        draft.surcharge = 10.0;
        draft.interest = 0.0;
        draft.compromise = 5.0;
        draft.recompute(None);
        assert!(!draft.auto_compute_penalties);
        assert_eq!(
            (draft.surcharge, draft.interest, draft.compromise),
            (10.0, 0.0, 5.0)
        );
        assert_eq!(draft.penalty_overrides, PenaltyOverrides::default());
        assert_eq!(draft.to_bir_field_map()["frm2551Qv2018:txt20"], "10.00");
        draft.item_13_election = Item13Election::Graduated;
        draft.recompute(None);
        assert!(draft.auto_compute_penalties);
        assert_eq!(draft.penalty_overrides.surcharge, Some(10.0));
        assert_eq!(draft.penalty_overrides.compromise, Some(5.0));
        assert_eq!((draft.surcharge, draft.compromise), (10.0, 5.0));
    }

    /// The migration runs once: a component blanked afterwards computes
    /// again instead of coming back from the old manual amount.
    #[test]
    fn a_blank_after_the_migration_computes_again() {
        let computed = make_draft(500_000.0, 0.0, 2024, 1);
        let mut draft = computed.clone();
        draft.auto_compute_penalties = false;
        draft.interest = 31.5;
        assert!(draft.adopt_legacy_manual_penalties());
        assert!(!draft.adopt_legacy_manual_penalties(), "only once");
        draft.penalty_overrides.interest = None;
        draft.recompute(None);
        assert_eq!(draft.interest, computed.interest);
        assert_eq!(draft.penalty_overrides.interest, None);
        assert_eq!(draft.surcharge, computed.surcharge);
    }

    /// Item 13 = 8% zeroes and disables Items 20-22 on the official page
    /// (`taxRateOption()`): an override does not survive it.
    #[test]
    fn an_eight_percent_return_takes_no_penalty_override() {
        let mut draft = make_draft(0.0, 0.0, 2024, 1);
        draft.penalty_overrides.surcharge = Some(100.0);
        draft.item_13_election = Item13Election::EightPercent;
        draft.recompute(None);
        assert_eq!(draft.penalty_overrides, PenaltyOverrides::default());
        assert_eq!(
            (
                draft.surcharge,
                draft.interest,
                draft.compromise,
                draft.total_penalties
            ),
            (0.0, 0.0, 0.0, 0.0)
        );
    }

    /// P5-1: the earlier float-tolerance check refused two-decimal
    /// amounts above about 2,000,000 (`8_816_764.47 * 100` lands 1.2e-7
    /// off a whole number); the shared `official_inputs` check accepts
    /// them again, and a three-decimal amount is still refused.
    #[test]
    fn validation_accepts_large_two_decimal_monetary_inputs() {
        for field in ["schedule", "creditable", "previous", "other", "surcharge"] {
            let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
            match field {
                "schedule" => draft.schedule_1[0].taxable_amount = 8_816_764.47,
                "creditable" => draft.creditable_tax_withheld = 8_816_764.47,
                "previous" => {
                    draft.is_amended = true;
                    draft.tax_paid_previous = 8_816_764.47;
                }
                "other" => {
                    draft.other_tax_credit = 8_816_764.47;
                    draft.other_tax_credit_description = "Adjustment".into();
                }
                "surcharge" => {
                    draft.auto_compute_penalties = false;
                    draft.surcharge = 8_816_764.47;
                }
                _ => unreachable!(),
            }
            draft.recompute(None);
            if field == "surcharge" {
                draft.surcharge = 8_816_764.47;
            }

            assert!(
                draft
                    .validate()
                    .iter()
                    .all(|(_, message)| !message.contains("at most two decimal places")),
                "{field} large two-decimal input was refused"
            );
        }

        let mut draft = make_draft(50_000.0, 0.0, 2026, 1);
        draft.auto_compute_penalties = false;
        draft.surcharge = 1.005;
        draft.recompute(None);
        draft.surcharge = 1.005;
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
