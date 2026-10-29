//! BIR Form 0605, July 1999 (ENCS).
//!
//! This model is intentionally limited to behavior established by the pinned
//! two-page official form and two reviewed 235-field editable saves. Form 0605
//! remains manual/external: the reviewed saves prove an editable persistence
//! contract, not an electronic-submission contract.

use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::queueable::SubmissionLifecycle;
use super::{FilingPeriod, FormValidator, TypedBirForm};
use crate::profile::{TaxpayerProfile, TaxpayerType};

pub const FORM_CODE: &str = "0605";
pub const FORM_REVISION: &str = "1999";
pub const FORM_TYPE_ID: &str = "0605v1999";
pub const FORM_VERSION_LABEL: &str = "July 1999 (ENCS)";
pub const QUEUE_SUBMISSION_SUPPORTED: bool = true;

/// Item 1. The two XML flags are derived from this single choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Form0605FilingBasis {
    #[default]
    Calendar,
    Fiscal,
}

/// Item 11. The official form defines only Individual and Non-Individual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Form0605TaxpayerClassification {
    #[default]
    Individual,
    NonIndividual,
}

/// Item 17. The five voluntary-payment and two audit/delinquency XML flags
/// are one semantic choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form0605MannerOfPayment {
    SelfAssessment,
    TaxDepositOrAdvancePayment,
    IncomeTaxSecondInstallmentIndividual,
    Penalties,
    Others,
    PreliminaryOrFinalAssessmentOrDeficiencyTax,
    AccountsReceivableOrDelinquentAccount,
}

impl Form0605MannerOfPayment {
    pub const ALL: [Self; 7] = [
        Self::SelfAssessment,
        Self::TaxDepositOrAdvancePayment,
        Self::IncomeTaxSecondInstallmentIndividual,
        Self::Penalties,
        Self::Others,
        Self::PreliminaryOrFinalAssessmentOrDeficiencyTax,
        Self::AccountsReceivableOrDelinquentAccount,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SelfAssessment => "Self-Assessment",
            Self::TaxDepositOrAdvancePayment => "Tax Deposit / Advance Payment",
            Self::IncomeTaxSecondInstallmentIndividual => {
                "Income Tax Second Installment (Individual)"
            }
            Self::Penalties => "Penalties",
            Self::Others => "Others (Specify)",
            Self::PreliminaryOrFinalAssessmentOrDeficiencyTax => {
                "Preliminary / Final Assessment / Deficiency Tax"
            }
            Self::AccountsReceivableOrDelinquentAccount => {
                "Accounts Receivable / Delinquent Account"
            }
        }
    }
}

/// Item 18. The source samples prove XML option 1 is Installment and option 3
/// is Full; the intervening official option is Partial Payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form0605TypeOfPayment {
    Installment,
    PartialPayment,
    FullPayment,
}

impl Form0605TypeOfPayment {
    pub const ALL: [Self; 3] = [Self::Installment, Self::PartialPayment, Self::FullPayment];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Installment => "Installment",
            Self::PartialPayment => "Partial Payment",
            Self::FullPayment => "Full Payment",
        }
    }
}

/// Item 17 approval radio `frm0605:itemApprovedYN` (BIR-Form0605.hta:944-947).
/// Filer-settable: `enableDefTax()` enables it when Item 17 is Preliminary/Final
/// Assessment/Deficiency Tax or Accounts Receivable/Delinquent Account
/// (`itemMannerOfPaymentB:_1/_2`); `disableDefTax()` clears and disables it otherwise.
/// Variant names are kept for stored-draft compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Form0605ApprovalSelection {
    #[default]
    None,
    /// `frm0605:itemApprovedYN:_1` (value `Y`): "Pre-approved by Investigating Office".
    XmlOption1,
    /// `frm0605:itemApprovedYN:_2` (value `N`): "Not approved by Investigating Office".
    XmlOption2,
}

/// Signature lines printed in Item 22 of the locked July 1999 form.
///
/// The reviewed editable saves do not contain keys for these lines. They are
/// persisted in the app draft for semantic HTML output, but are deliberately
/// omitted from the 235-field editable-save payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Form0605SignatureDetails {
    pub taxpayer_or_authorized_representative: String,
    pub title_or_position: String,
    pub head_of_office: String,
}

/// Item 24, Check, has all four payment-detail columns on the official form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form0605CheckPayment {
    pub drawee_bank_or_agency: String,
    pub number: String,
    /// Manual `MM/DD/YYYY` value. No payment-channel date rule is inferred.
    pub date: String,
    /// `None` is an officially blank amount; `Some(0.0)` is an entered zero.
    pub amount: Option<f64>,
}

/// Item 25, Tax Debit Memo, has Number, Date, and Amount fields only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form0605TaxDebitMemoPayment {
    pub number: String,
    /// Manual `MM/DD/YYYY` value. No payment-channel date rule is inferred.
    pub date: String,
    /// `None` is an officially blank amount; `Some(0.0)` is an entered zero.
    pub amount: Option<f64>,
}

/// Item 26, Others, has all four payment-detail columns on the official form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form0605OtherPayment {
    pub drawee_bank_or_agency: String,
    pub number: String,
    /// Manual `MM/DD/YYYY` value. No payment-channel date rule is inferred.
    pub date: String,
    /// `None` is an officially blank amount; `Some(0.0)` is an entered zero.
    pub amount: Option<f64>,
}

/// The four fixed Part III rows on page 1 of the official form.
///
/// These values are PDF-backed draft/renderer data. The two reviewed editable
/// saves have no corresponding XML keys, so they never expand the exact
/// 235-field save contract or imply an electronic-submission contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form0605PaymentDetails {
    pub cash_or_bank_debit_memo_amount: Option<f64>,
    pub check: Form0605CheckPayment,
    pub tax_debit_memo: Form0605TaxDebitMemoPayment,
    pub others: Form0605OtherPayment,
    pub machine_validation_or_receipt_details: String,
}

/// A complete Gregorian date retained as three exact XML components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Form0605Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl Form0605Date {
    pub fn new(year: u16, month: u8, day: u8) -> Result<Self, String> {
        let value = Self { year, month, day };
        value.validate()?;
        Ok(value)
    }

    pub fn parse_mm_dd_yyyy(value: &str) -> Result<Self, String> {
        let date = chrono::NaiveDate::parse_from_str(value.trim(), "%m/%d/%Y")
            .map_err(|_| "Date must use MM/DD/YYYY and be a real calendar date".to_string())?;
        let year = u16::try_from(chrono::Datelike::year(&date))
            .map_err(|_| "Date year is outside the supported range".to_string())?;
        let month = u8::try_from(chrono::Datelike::month(&date))
            .map_err(|_| "Date month is outside the supported range".to_string())?;
        let day = u8::try_from(chrono::Datelike::day(&date))
            .map_err(|_| "Date day is outside the supported range".to_string())?;
        Ok(Self { year, month, day })
    }

    pub fn validate(self) -> Result<(), String> {
        chrono::NaiveDate::from_ymd_opt(
            i32::from(self.year),
            u32::from(self.month),
            u32::from(self.day),
        )
        .map(|_| ())
        .ok_or_else(|| "Date is not a real calendar date".to_string())
    }
}

impl fmt::Display for Form0605Date {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:02}/{:02}/{:04}",
            self.month, self.day, self.year
        )
    }
}

/// Source-proven ATC choices. Their XML indexes come from the two reviewed
/// editable saves, not from the visual ordering of the official table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form0605ReviewedAtc {
    Fp010,
    Ii011,
}

impl Form0605ReviewedAtc {
    pub const ALL: [Self; 2] = [Self::Fp010, Self::Ii011];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Fp010 => "FP010",
            Self::Ii011 => "II011",
        }
    }

    pub const fn xml_index(self) -> u16 {
        match self {
            Self::Fp010 => 1,
            Self::Ii011 => 24,
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Fp010 => "Fines and Penalties",
            Self::Ii011 => "Pure Compensation Income",
        }
    }
}

/// Source-proven Tax Type choices and indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form0605ReviewedTaxType {
    Do,
    It,
}

impl Form0605ReviewedTaxType {
    pub const ALL: [Self; 2] = [Self::Do, Self::It];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Do => "DO",
            Self::It => "IT",
        }
    }

    pub const fn xml_index(self) -> u16 {
        match self {
            Self::Do => 4,
            Self::It => 9,
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Do => "Documentary Stamp Tax - One Time",
            Self::It => "Income Tax",
        }
    }
}

/// Distinguishes app-selectable reviewed mappings from exact imported pairs
/// that can be retained but cannot be treated as certified choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form0605CodeEvidence {
    ReviewedPair,
    ImportedExact,
}

/// Semantic code plus the one selected checkbox index in the 142/37-field
/// legacy matrix. Unknown imported pairs survive round-trip without becoming
/// app-authorized mappings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Form0605IndexedCode {
    code: String,
    xml_index: u16,
    evidence: Form0605CodeEvidence,
}

impl Form0605IndexedCode {
    pub fn reviewed_atc(value: Form0605ReviewedAtc) -> Self {
        Self {
            code: value.code().to_string(),
            xml_index: value.xml_index(),
            evidence: Form0605CodeEvidence::ReviewedPair,
        }
    }

    pub fn reviewed_tax_type(value: Form0605ReviewedTaxType) -> Self {
        Self {
            code: value.code().to_string(),
            xml_index: value.xml_index(),
            evidence: Form0605CodeEvidence::ReviewedPair,
        }
    }

    /// An ATC or tax type picked from the official popup list.
    pub(crate) fn official(code: &str, xml_index: u16) -> Self {
        Self {
            code: code.to_string(),
            xml_index,
            evidence: Form0605CodeEvidence::ReviewedPair,
        }
    }

    pub(crate) fn imported_atc(code: String, xml_index: u16) -> Self {
        let is_reviewed = Form0605ReviewedAtc::ALL
            .iter()
            .copied()
            .any(|candidate| candidate.code() == code && candidate.xml_index() == xml_index);
        let evidence = if is_reviewed {
            Form0605CodeEvidence::ReviewedPair
        } else {
            Form0605CodeEvidence::ImportedExact
        };
        Self {
            code,
            xml_index,
            evidence,
        }
    }

    pub(crate) fn imported_tax_type(code: String, xml_index: u16) -> Self {
        let is_reviewed = Form0605ReviewedTaxType::ALL
            .iter()
            .copied()
            .any(|candidate| candidate.code() == code && candidate.xml_index() == xml_index);
        let evidence = if is_reviewed {
            Form0605CodeEvidence::ReviewedPair
        } else {
            Form0605CodeEvidence::ImportedExact
        };
        Self {
            code,
            xml_index,
            evidence,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub const fn xml_index(&self) -> u16 {
        self.xml_index
    }

    pub const fn evidence(&self) -> Form0605CodeEvidence {
        self.evidence
    }

    pub const fn requires_review(&self) -> bool {
        matches!(self.evidence, Form0605CodeEvidence::ImportedExact)
    }
}

/// Complete editable draft for exact identity `0605v1999`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form0605Draft {
    pub id: Option<i64>,

    // Database/open-ended filing slot: the payment number in the year (1, 2,
    // ...; not capped at 12). It is not used to derive official dates,
    // quarter, or year-ended fields.
    pub tin: String,
    pub taxable_year: u16,
    pub month: u8,

    // Items 1-8.
    #[serde(default)]
    pub filing_basis: Form0605FilingBasis,
    #[serde(default = "default_quarter")]
    pub quarter: u8,
    #[serde(default = "default_year_end_month")]
    pub year_end_month: u8,
    #[serde(default)]
    pub due_date: Option<Form0605Date>,
    #[serde(default)]
    pub return_period: Option<Form0605Date>,
    #[serde(default)]
    pub number_of_sheets: u16,
    #[serde(default)]
    pub atc: Option<Form0605IndexedCode>,
    #[serde(default)]
    pub tax_type: Option<Form0605IndexedCode>,

    // Items 9-16.
    pub rdo_code: String,
    pub taxpayer_name: String,
    #[serde(default)]
    pub classification: Form0605TaxpayerClassification,
    #[serde(default, alias = "txt_line_bus")]
    pub line_of_business: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,

    // Items 17-18.
    #[serde(default)]
    pub manner_of_payment: Option<Form0605MannerOfPayment>,
    #[serde(default)]
    pub other_manner_description: String,
    #[serde(default)]
    pub type_of_payment: Option<Form0605TypeOfPayment>,
    #[serde(default)]
    pub number_of_installments: Option<u16>,

    // Items 19-21. The aliases retain amount data from the generated scaffold.
    #[serde(alias = "txt_tax19")]
    pub item_19_basic_tax_or_payment: f64,
    #[serde(alias = "txt_tax20a")]
    pub item_20a_surcharge: f64,
    #[serde(alias = "txt_tax20b")]
    pub item_20b_interest: f64,
    #[serde(alias = "txt_tax20c")]
    pub item_20c_compromise: f64,
    /// The penalty items' computed values, overrides, note and engine
    /// inputs (the shared model in [`super::penalty_items`]).
    #[serde(flatten)]
    pub penalty: PenaltyState,
    #[serde(alias = "txt_tax20d")]
    pub item_20d_total_penalties: f64,
    #[serde(alias = "txt_tax21")]
    pub item_21_total_amount_payable: f64,

    // The only approval-related fields present in either exact XML source.
    #[serde(default)]
    pub approval_selection: Form0605ApprovalSelection,

    // Item 22 and Part III. These are official-PDF-backed app fields, but are
    // absent from both reviewed 235-field editable saves.
    #[serde(default)]
    pub signatures: Form0605SignatureDetails,
    #[serde(default)]
    pub payment_details: Form0605PaymentDetails,

    // Unknown future/source transport keys are retained, while every modeled
    // value overwrites the same key during export.
    #[serde(default)]
    pub preserved_unmodeled_xml_fields: BTreeMap<String, String>,

    /// Pre-queue builds stored their own error text here. Kept so old JSON
    /// round-trips; the generic lifecycle reports `submission_error`.
    #[serde(default)]
    pub last_error: Option<String>,

    /// Status, queue authorization and retry state. Flattened so stored JSON
    /// keeps the keys earlier builds wrote inline (`status`, `created_at`,
    /// `submission_attempts`, ...).
    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

const fn default_quarter() -> u8 {
    1
}

const fn default_year_end_month() -> u8 {
    12
}

impl Form0605Draft {
    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, period_slot: u8) -> Self {
        Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            month: period_slot.max(1),
            filing_basis: Form0605FilingBasis::Calendar,
            // The official page opens with no quarter ticked.
            quarter: 0,
            year_end_month: 12,
            due_date: None,
            return_period: None,
            number_of_sheets: 0,
            atc: None,
            tax_type: None,
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            classification: if matches!(profile.taxpayer_type, TaxpayerType::Individual) {
                Form0605TaxpayerClassification::Individual
            } else {
                Form0605TaxpayerClassification::NonIndividual
            },
            line_of_business: profile.line_of_business.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            manner_of_payment: None,
            other_manner_description: String::new(),
            type_of_payment: None,
            number_of_installments: None,
            item_19_basic_tax_or_payment: 0.0,
            item_20a_surcharge: 0.0,
            item_20b_interest: 0.0,
            item_20c_compromise: 0.0,
            item_20d_total_penalties: 0.0,
            item_21_total_amount_payable: 0.0,
            approval_selection: Form0605ApprovalSelection::None,
            signatures: Form0605SignatureDetails::default(),
            payment_details: Form0605PaymentDetails::default(),
            preserved_unmodeled_xml_fields: BTreeMap::new(),
            last_error: None,
            lifecycle: SubmissionLifecycle::default(),
        }
    }

    pub fn select_reviewed_atc(&mut self, value: Form0605ReviewedAtc) {
        self.atc = Some(Form0605IndexedCode::reviewed_atc(value));
    }

    pub fn select_reviewed_tax_type(&mut self, value: Form0605ReviewedTaxType) {
        self.tax_type = Some(Form0605IndexedCode::reviewed_tax_type(value));
    }

    /// The official compute chain (see `form_0605_official`).
    pub fn recompute(&mut self) {
        self.official_recompute();
    }

    pub fn is_editable(&self) -> bool {
        self.lifecycle.is_editable()
    }

    pub fn evidence_warnings(&self) -> Vec<String> {
        let mut warnings = vec![
            "Only FP010↔AtcCode1, II011↔AtcCode24, DO↔TaxTypeCode4, and IT↔TaxTypeCode9 are source-proven editable mappings. Other imported pairs are retained but not certified."
                .to_string(),
            "Item 22 signatures and Part III payment details are backed by the official PDF and persist in the app draft, but the two reviewed 235-field saves contain no corresponding XML keys; they are omitted from editable-save export."
                .to_string(),
        ];
        if self
            .atc
            .as_ref()
            .is_some_and(Form0605IndexedCode::requires_review)
        {
            warnings.push(
                "The imported ATC/index pair requires review before safe export.".to_string(),
            );
        }
        if self
            .tax_type
            .as_ref()
            .is_some_and(Form0605IndexedCode::requires_review)
        {
            warnings.push(
                "The imported Tax Type/index pair requires review before safe export.".to_string(),
            );
        }
        if !self.preserved_unmodeled_xml_fields.is_empty() {
            warnings.push(format!(
                "Preserved unmodeled XML keys require review: {}",
                self.preserved_unmodeled_xml_fields
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        warnings
    }
}

impl FormValidator for Form0605Draft {
    /// The official `validate()` port; see `form_0605_official`.
    fn validate(&self) -> Vec<(String, String)> {
        self.official_errors()
    }
}

impl TypedBirForm for Form0605Draft {
    fn form_code(&self) -> &'static str {
        FORM_CODE
    }

    fn form_type_id(&self) -> &'static str {
        FORM_TYPE_ID
    }

    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::OpenEnded(u32::from(self.month))
    }

    fn recompute(&mut self) {
        Form0605Draft::recompute(self);
    }

    fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        Form0605Draft::to_bir_field_map(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "JUAN DELA CRUZ",
            "tin": {
                "segment1": "123",
                "segment2": "456",
                "segment3": "788",
                "branch": "00000"
            },
            "rdo_code": "018",
            "line_of_business": "SOFTWARE DEVELOPMENT",
            "registered_address": "OLONGAPO",
            "zip_code": "2200",
            "phone": "09123456789",
            "email": "taxpayer@example.com",
            "default_form_type": "0605v1999",
            "taxpayer_type": "Individual"
        }))
        .expect("test profile must deserialize")
    }

    fn valid_draft() -> Form0605Draft {
        let mut draft = Form0605Draft::new_from_profile(&test_profile(), 2026, 1);
        draft.due_date = Some(Form0605Date::new(2026, 1, 1).unwrap());
        draft.return_period = Some(Form0605Date::new(2026, 1, 31).unwrap());
        draft.select_reviewed_atc(Form0605ReviewedAtc::Fp010);
        draft.select_reviewed_tax_type(Form0605ReviewedTaxType::Do);
        draft.manner_of_payment = Some(Form0605MannerOfPayment::SelfAssessment);
        draft.type_of_payment = Some(Form0605TypeOfPayment::FullPayment);
        draft.item_19_basic_tax_or_payment = 10.0;
        draft.recompute();
        draft
    }

    #[test]
    fn exact_identity_uses_july_1999_revision() {
        let draft = valid_draft();
        assert_eq!(draft.form_type_id(), "0605v1999");
        assert_eq!(FORM_VERSION_LABEL, "July 1999 (ENCS)");
    }

    #[test]
    fn official_formulas_are_deterministic_without_timestamp_mutation() {
        let mut draft = valid_draft();
        draft.item_19_basic_tax_or_payment = 1_000.0;
        draft.item_20a_surcharge = 10.0;
        draft.item_20b_interest = 20.0;
        draft.item_20c_compromise = 1_000.0;
        crate::forms::queueable::QueueableForm::keep_entered_penalties(&mut draft);
        draft.lifecycle.updated_at = "fixed".to_string();
        draft.recompute();
        assert_eq!(draft.item_20d_total_penalties, 1_030.0);
        assert_eq!(draft.item_21_total_amount_payable, 2_030.0);
        assert_eq!(draft.lifecycle.updated_at, "fixed");
    }

    #[test]
    fn thirteenth_payment_gets_its_own_slot() {
        let mut draft = valid_draft();
        draft.month = 13;
        draft.recompute();
        assert_eq!(draft.filing_period(), FilingPeriod::OpenEnded(13));
        assert!(!draft.validate().iter().any(|(field, _)| field == "month"));
    }

    #[test]
    fn quarter_is_not_derived_from_return_period_month() {
        let mut draft = valid_draft();
        draft.quarter = 1;
        draft.return_period = Some(Form0605Date::new(2025, 12, 31).unwrap());
        assert_eq!(draft.quarter, 1);
    }

    #[test]
    fn only_source_proven_code_pairs_are_app_selectable() {
        let mut draft = valid_draft();
        draft.select_reviewed_atc(Form0605ReviewedAtc::Ii011);
        draft.select_reviewed_tax_type(Form0605ReviewedTaxType::It);
        assert_eq!(draft.atc.as_ref().unwrap().xml_index(), 24);
        assert_eq!(draft.tax_type.as_ref().unwrap().xml_index(), 9);
    }

    #[test]
    fn imported_unknown_code_pair_is_preserved_but_fails_closed() {
        let mut draft = valid_draft();
        draft.atc = Some(Form0605IndexedCode::imported_atc(
            "UNREVIEWED".to_string(),
            77,
        ));
        assert_eq!(draft.atc.as_ref().unwrap().code(), "UNREVIEWED");
        assert!(draft.validate().iter().any(|(field, _)| field == "atc"));
    }

    #[test]
    fn forged_reviewed_evidence_cannot_authorize_an_unknown_code_pair() {
        let mut draft = valid_draft();
        draft.atc = Some(Form0605IndexedCode {
            code: "UNREVIEWED".to_string(),
            xml_index: 77,
            evidence: Form0605CodeEvidence::ReviewedPair,
        });

        assert!(draft.validate().iter().any(|(field, message)| {
            field == "atc" && message == "Please enter a valid ATC on Item 6."
        }));
    }

    #[test]
    fn invalid_date_is_rejected_without_panicking() {
        assert!(Form0605Date::new(2026, 2, 30).is_err());
        assert!(Form0605Date::parse_mm_dd_yyyy("02/30/2026").is_err());
    }

    #[test]
    fn installment_requires_positive_count() {
        let mut draft = valid_draft();
        draft.type_of_payment = Some(Form0605TypeOfPayment::Installment);
        draft.number_of_installments = None;
        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "number_of_installments")
        );
    }

    #[test]
    fn json_roundtrip_preserves_semantic_selections_and_dates() {
        let mut draft = valid_draft();
        draft.signatures.taxpayer_or_authorized_representative = "JUAN DELA CRUZ".to_string();
        draft.signatures.title_or_position = "OWNER".to_string();
        draft.payment_details.check.drawee_bank_or_agency = "AAB".to_string();
        draft.payment_details.check.number = "000123".to_string();
        draft.payment_details.check.date = "01/31/2026".to_string();
        draft.payment_details.check.amount = Some(10.0);
        let json = serde_json::to_string(&draft).expect("draft should serialize");
        let reopened: Form0605Draft =
            serde_json::from_str(&json).expect("draft should deserialize");
        assert_eq!(reopened, draft);
    }

    #[test]
    fn pdf_only_signature_and_payment_fields_do_not_expand_editable_xml_contract() {
        let mut draft = valid_draft();
        draft.signatures.taxpayer_or_authorized_representative = "JUAN DELA CRUZ".to_string();
        draft.payment_details.cash_or_bank_debit_memo_amount = Some(10.0);
        draft.payment_details.check.number = "CHECK-1".to_string();
        draft.payment_details.check.date = "01/31/2026".to_string();
        draft.payment_details.check.amount = Some(10.0);

        let fields = draft.to_bir_field_map();

        assert_eq!(fields.len(), 235);
        assert!(
            fields
                .keys()
                .all(|key| !key.contains("Signature") && !key.starts_with("payment_"))
        );
    }

    #[test]
    fn invalid_payment_values_fail_validation_without_panicking() {
        let mut draft = valid_draft();
        draft.payment_details.check.date = "02/30/2026".to_string();
        draft.payment_details.check.amount = Some(f64::NAN);

        let errors = draft.validate();

        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "payment_24_check.date")
        );
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "payment_24_check.amount")
        );
    }
}
