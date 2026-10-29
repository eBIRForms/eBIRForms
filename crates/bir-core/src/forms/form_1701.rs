//! BIR Form 1701, January 2018 (ENCS).
//!
//! The semantic model is limited to the four-page official return and the
//! exact reviewed editable saves in `a local folder`: the plain
//! save has 837 fields and its encrypted companion has one additional second
//! address-line field. The separate Part X/attachment worksheets are retained
//! losslessly when an exact save is imported, but are not interpreted as tax
//! evidence here.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::form_2551q::{AnnualIncomeTaxElection, annual_income_tax_election};
use super::penalty_items::PenaltyState;
use super::queueable::SubmissionLifecycle;
use super::{FilingPeriod, FormValidator, TypedBirForm};
use crate::profile::{IncomeTaxElection, TaxClassification, TaxpayerProfile, TaxpayerType};

pub const FORM_CODE: &str = "1701";
pub const FORM_REVISION: &str = "2018";
pub const FORM_TYPE_ID: &str = "1701v2018";
pub const EXACT_REVIEWED_XML_FIELD_COUNT: usize = 837;
pub const EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT: usize = 838;
pub const REVIEWED_ENCRYPTED_XML_EXTRA_FIELD: &str = "frm1701:txtPg1I9Address2";
pub const EXACT_REVIEWED_XML_VERSION: &str = "051414";
pub const QUEUE_SUBMISSION_SUPPORTED: bool = false;
pub const OFFICIAL_FORM_SHA256: &str =
    "19be91d78258eb7c255f2615610db2739f10c378f8ac97adc0887c1bf40d1b2e";
pub const REVIEWED_EDITABLE_XML_SHA256: &str =
    "b168c7b3273d30a10f28f4653847519b876d5a88e77ed82911718a80f65c7827";
pub const REVIEWED_ENCRYPTED_XML_SHA256: &str =
    "3771c99c191ef5e84b1b5e4c51499911bfbec6002febc3c53dca3f08730e92e3";
pub const REVIEWED_ATTACHMENT_PDF_SHA256: &str =
    "e71799dc613c08d4c383fcd66bed83032b182ab43721c8665d7b608047766cad";
pub const REVIEWED_CONSOLIDATED_PDF_SHA256: &str =
    "eac0ce426cc57c473e24638accb14a978ddd54f8cf795cc4303f527088416871";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Form1701Party {
    Taxpayer,
    Spouse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701TaxpayerType {
    SingleProprietor,
    Professional,
    Estate,
    Trust,
    CompensationEarner,
}

impl Form1701TaxpayerType {
    pub const ALL: [Self; 5] = [
        Self::SingleProprietor,
        Self::Professional,
        Self::Estate,
        Self::Trust,
        Self::CompensationEarner,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SingleProprietor => "Single Proprietor",
            Self::Professional => "Professional",
            Self::Estate => "Estate",
            Self::Trust => "Trust",
            Self::CompensationEarner => "Compensation Earner",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701SpouseType {
    SingleProprietor,
    Professional,
    CompensationEarner,
}

impl Form1701SpouseType {
    pub const ALL: [Self; 3] = [
        Self::SingleProprietor,
        Self::Professional,
        Self::CompensationEarner,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SingleProprietor => "Single Proprietor",
            Self::Professional => "Professional",
            Self::CompensationEarner => "Compensation Earner",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Form1701Atc {
    Ii011,
    Ii012,
    Ii013,
    Ii014,
    Ii015,
    Ii016,
    Ii017,
}

impl Form1701Atc {
    pub const ALL: [Self; 7] = [
        Self::Ii011,
        Self::Ii012,
        Self::Ii013,
        Self::Ii014,
        Self::Ii015,
        Self::Ii016,
        Self::Ii017,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Ii011 => "II011",
            Self::Ii012 => "II012",
            Self::Ii013 => "II013",
            Self::Ii014 => "II014",
            Self::Ii015 => "II015",
            Self::Ii016 => "II016",
            Self::Ii017 => "II017",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ii011 => "Compensation Income",
            Self::Ii012 => "Business Income - Graduated IT Rates",
            Self::Ii013 => "Mixed Income - Graduated IT Rates",
            Self::Ii014 => "Income from Profession - Graduated IT Rates",
            Self::Ii015 => "Business Income - 8% IT Rate",
            Self::Ii016 => "Mixed Income - 8% IT Rate",
            Self::Ii017 => "Income from Profession - 8% IT Rate",
        }
    }

    pub const fn tax_rate(self) -> Option<Form1701TaxRate> {
        match self {
            Self::Ii011 => None,
            Self::Ii012 | Self::Ii013 | Self::Ii014 => Some(Form1701TaxRate::Graduated),
            Self::Ii015 | Self::Ii016 | Self::Ii017 => Some(Form1701TaxRate::EightPercent),
        }
    }

    pub const fn gets_eight_percent_reduction(self) -> bool {
        matches!(self, Self::Ii015 | Self::Ii017)
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim().to_ascii_uppercase().as_str() {
            "II011" => Some(Self::Ii011),
            "II012" => Some(Self::Ii012),
            "II013" => Some(Self::Ii013),
            "II014" => Some(Self::Ii014),
            "II015" => Some(Self::Ii015),
            "II016" => Some(Self::Ii016),
            "II017" => Some(Self::Ii017),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701TaxRate {
    Graduated,
    EightPercent,
}

impl Form1701TaxRate {
    pub const ALL: [Self; 2] = [Self::Graduated, Self::EightPercent];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Graduated => "Graduated Rates",
            Self::EightPercent => "8% IT Rate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701DeductionMethod {
    Itemized,
    Osd,
}

impl Form1701DeductionMethod {
    pub const ALL: [Self; 2] = [Self::Itemized, Self::Osd];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Itemized => "Itemized Deduction",
            Self::Osd => "Optional Standard Deduction (OSD)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701CivilStatus {
    Single,
    Married,
    LegallySeparated,
    Widowed,
}

impl Form1701CivilStatus {
    pub const ALL: [Self; 4] = [
        Self::Single,
        Self::Married,
        Self::LegallySeparated,
        Self::Widowed,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Married => "Married",
            Self::LegallySeparated => "Legally Separated",
            Self::Widowed => "Widow/er",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701JointFilingStatus {
    Joint,
    Separate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Form1701OverpaymentDisposition {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
    CarryOver,
}

/// An amount pair preserves an officially blank cell (`None`) separately from
/// an explicitly entered zero (`Some(0.0)`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701AmountPair {
    pub taxpayer: Option<f64>,
    pub spouse: Option<f64>,
}

impl Form1701AmountPair {
    pub const fn value(&self, party: Form1701Party) -> Option<f64> {
        match party {
            Form1701Party::Taxpayer => self.taxpayer,
            Form1701Party::Spouse => self.spouse,
        }
    }

    pub fn set(&mut self, party: Form1701Party, value: Option<f64>) {
        match party {
            Form1701Party::Taxpayer => self.taxpayer = value,
            Form1701Party::Spouse => self.spouse = value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701EmployerRow {
    pub owner: Option<Form1701Party>,
    pub employer_name: String,
    pub employer_tin: String,
    pub compensation_income: Option<f64>,
    pub tax_withheld: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701SpecialDeductionRow {
    pub description: String,
    pub legal_basis: String,
    pub amount: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701NolcoRow {
    pub year_incurred: String,
    pub amount: Option<f64>,
    pub applied_previous_years: Option<f64>,
    pub expired: Option<f64>,
    pub applied_current_year: Option<f64>,
    pub unapplied: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701PaymentRow {
    pub drawee_bank_or_agency: String,
    pub number: String,
    pub date: String,
    pub amount: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701PaymentDetails {
    pub item_34_cash_or_bank_debit_memo: Form1701PaymentRow,
    pub item_35_check: Form1701PaymentRow,
    pub item_36_tax_debit_memo: Form1701PaymentRow,
    pub item_37_others: Form1701PaymentRow,
    pub item_37_others_description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701Spouse {
    pub enabled: bool,
    pub tin: String,
    pub rdo_code: String,
    pub filer_type: Option<Form1701SpouseType>,
    pub atc: Option<Form1701Atc>,
    pub name: String,
    pub contact_number: String,
    pub citizenship: String,
    pub claims_foreign_tax_credits: Option<bool>,
    pub foreign_tax_number: String,
    pub has_exempt_income: Option<bool>,
    pub has_special_rate_income: Option<bool>,
    pub tax_rate: Option<Form1701TaxRate>,
    pub deduction_method: Option<Form1701DeductionMethod>,
}

/// Main-return computation tables. The official item numbers are the keys.
/// Maps allow the UI/renderer to iterate exact printed lines without keeping
/// hundreds of transport-derived Rust identifiers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701Computations {
    pub part_ii: BTreeMap<u8, Form1701AmountPair>,
    pub part_ii_item_32_aggregate: Option<f64>,
    pub schedule_2: BTreeMap<u8, Form1701AmountPair>,
    pub schedule_3: BTreeMap<u8, Form1701AmountPair>,
    pub schedule_3_descriptions: BTreeMap<u8, String>,
    pub schedule_4: BTreeMap<u8, Form1701AmountPair>,
    /// Schedule 4 Item 17a through 17d.
    pub schedule_4_item_17: [Form1701AmountPair; 4],
    pub schedule_4_item_17d_description: String,
    pub schedule_5_taxpayer: [Form1701SpecialDeductionRow; 2],
    pub schedule_5_spouse: [Form1701SpecialDeductionRow; 2],
    pub schedule_5_total_taxpayer: Option<f64>,
    pub schedule_5_total_spouse: Option<f64>,
    pub schedule_6_summary: BTreeMap<u8, Form1701AmountPair>,
    pub schedule_6_taxpayer_nolco: [Form1701NolcoRow; 4],
    pub schedule_6_spouse_nolco: [Form1701NolcoRow; 4],
    pub schedule_6_total_taxpayer: Option<f64>,
    pub schedule_6_total_spouse: Option<f64>,
    pub part_vi: BTreeMap<u8, Form1701AmountPair>,
    pub part_vii: BTreeMap<u8, Form1701AmountPair>,
    pub part_vii_item_9_description: String,
    pub part_viii: BTreeMap<u8, Form1701AmountPair>,
    pub part_ix: BTreeMap<u8, Form1701AmountPair>,
    pub part_ix_descriptions: BTreeMap<u8, String>,
}

/// Complete local draft for exact identity `1701v2018`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1701Draft {
    pub id: Option<i64>,

    // Filing identity.
    pub tin: String,
    pub taxable_year: u16,
    /// The save schema carries an end month. A normal annual return must use
    /// December; another month requires the Short Period choice.
    #[serde(alias = "month")]
    pub period_end_month: u8,
    pub is_amended: bool,
    pub is_short_period: bool,

    // Part I.
    pub rdo_code: String,
    pub taxpayer_type: Option<Form1701TaxpayerType>,
    pub atc: Option<Form1701Atc>,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub date_of_birth: String,
    pub email: String,
    pub citizenship: String,
    pub claims_foreign_tax_credits: Option<bool>,
    pub foreign_tax_number: String,
    pub contact_number: String,
    pub civil_status: Option<Form1701CivilStatus>,
    pub spouse_has_income: Option<bool>,
    pub joint_filing_status: Option<Form1701JointFilingStatus>,
    pub has_exempt_income: Option<bool>,
    pub has_special_rate_income: Option<bool>,
    pub tax_rate: Option<Form1701TaxRate>,
    pub deduction_method: Option<Form1701DeductionMethod>,

    // Page 1 remainder and page 2 spouse background.
    pub number_of_attachments: Option<u8>,
    pub overpayment_disposition: Form1701OverpaymentDisposition,
    pub spouse: Form1701Spouse,
    pub employers: [Form1701EmployerRow; 2],
    pub computations: Form1701Computations,
    pub payment_details: Form1701PaymentDetails,
    pub machine_validation_or_receipt_details: String,

    /// The complete raw field map from an exact imported save. Modeled values
    /// overwrite their corresponding keys on export; all attachment and
    /// unknown fields remain byte-value equivalent after parse/generate.
    pub preserved_xml_fields: BTreeMap<String, String>,
    pub has_exact_xml_snapshot: bool,

    /// Item 6 is a set of checkboxes: a Single Proprietor or Professional
    /// with compensation income also ticks Compensation Earner (mixed
    /// income, ATC II013/II016).
    pub taxpayer_also_compensation_earner: bool,
    /// The same second Item 3 tick for the spouse (page 2).
    pub spouse_also_compensation_earner: bool,
    /// Line of business from the taxpayer profile (`txtLineBus`, filled by
    /// the official page from the background information).
    pub line_of_business: String,
    /// Spouse RDO (`txtPg2I2SpouseRDOCode`); blank keeps the page's `000`.
    /// Kept outside `Form1701Spouse` so earlier stored JSON keeps loading.
    pub spouse_rdo_code: String,

    /// Pre-queue builds stored their own error text here. Kept so old JSON
    /// round-trips; the generic lifecycle reports `submission_error`.
    pub last_error: Option<String>,

    /// Each column's Interest, Surcharge and Compromise (Items 27-29, A and
    /// B) on the shared penalty model ([`super::penalty_items`]): computed
    /// values, overrides and engine inputs, one state per column. A column at
    /// `penalty_model_version` 0 (a draft stored before PR 7, or an imported
    /// save file) still holds the filer's own typed amounts. The stored shape
    /// is kept: a draft stored with no `penalty` key is written back without
    /// it while it is the default, and one stored with the key (even at its
    /// default) is written back with it ([`PenaltySlot`]).
    #[serde(default, skip_serializing_if = "PenaltySlot::is_absent")]
    pub penalty: PenaltySlot,
    #[serde(default, skip_serializing_if = "PenaltySlot::is_absent")]
    pub spouse_penalty: PenaltySlot,

    /// Status, queue authorization and retry state. Flattened so stored JSON
    /// keeps the keys earlier builds wrote inline (`status`, `created_at`,
    /// `submission_attempts`, ...).
    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

impl Default for Form1701Draft {
    fn default() -> Self {
        Self {
            id: None,
            tin: String::new(),
            taxable_year: 2018,
            period_end_month: 12,
            is_amended: false,
            is_short_period: false,
            rdo_code: String::new(),
            taxpayer_type: None,
            atc: None,
            taxpayer_name: String::new(),
            registered_address: String::new(),
            zip_code: String::new(),
            date_of_birth: String::new(),
            email: String::new(),
            citizenship: String::new(),
            claims_foreign_tax_credits: None,
            foreign_tax_number: String::new(),
            contact_number: String::new(),
            civil_status: None,
            spouse_has_income: None,
            joint_filing_status: None,
            has_exempt_income: None,
            has_special_rate_income: None,
            tax_rate: None,
            deduction_method: None,
            number_of_attachments: None,
            overpayment_disposition: Form1701OverpaymentDisposition::None,
            spouse: Form1701Spouse::default(),
            employers: std::array::from_fn(|_| Form1701EmployerRow::default()),
            computations: Form1701Computations::default(),
            payment_details: Form1701PaymentDetails::default(),
            machine_validation_or_receipt_details: String::new(),
            preserved_xml_fields: BTreeMap::new(),
            has_exact_xml_snapshot: false,
            taxpayer_also_compensation_earner: false,
            spouse_also_compensation_earner: false,
            line_of_business: String::new(),
            spouse_rdo_code: String::new(),
            last_error: None,
            penalty: PenaltySlot::default(),
            spouse_penalty: PenaltySlot::default(),
            lifecycle: SubmissionLifecycle::default(),
        }
    }
}

/// One column's [`PenaltyState`], remembering whether the stored draft had
/// the key, so a save writes back the shape it read: present stays present
/// (even at the default), absent stays absent until the state is used.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PenaltySlot {
    pub state: PenaltyState,
    /// The stored draft carried this key.
    present: bool,
}

impl PenaltySlot {
    pub fn new(state: PenaltyState) -> Self {
        Self {
            state,
            present: false,
        }
    }

    fn is_absent(&self) -> bool {
        !self.present && self.state == PenaltyState::default()
    }
}

impl std::ops::Deref for PenaltySlot {
    type Target = PenaltyState;
    fn deref(&self) -> &PenaltyState {
        &self.state
    }
}

impl std::ops::DerefMut for PenaltySlot {
    fn deref_mut(&mut self) -> &mut PenaltyState {
        &mut self.state
    }
}

impl Serialize for PenaltySlot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.state.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PenaltySlot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            state: PenaltyState::deserialize(deserializer)?,
            present: true,
        })
    }
}

impl Form1701Draft {
    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the nine TIN digits of the page 2-4 header
    ///   combs (the sheet preprints the branch 00000 after them).
    /// - `derived:attachments_tens` / `derived:attachments_units`: Item 33
    ///   over its two digit boxes.
    ///
    /// The page 2-4 header names are the last name the sheet prints
    /// ([`Self::page_header_name`]), not the upload map's full name.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm1701:txtPg2TIN1",
                "frm1701:txtPg2TIN2",
                "frm1701:txtPg2TIN3",
            ],
        );
        let attachments: Vec<char> = fields
            .get("frm1701:txtPg1I33NumberOfAttachments")
            .map(|count| format!("{:0>2}", count.trim()).chars().collect())
            .unwrap_or_default();
        let digit = |at: usize| {
            attachments
                .get(at)
                .filter(|_| attachments.len() == 2)
                .map(char::to_string)
                .unwrap_or_default()
        };
        let (tens, units) = (digit(0), digit(1));
        for (key, value) in [
            ("tin_digits", tin_digits),
            ("attachments_tens", tens),
            ("attachments_units", units),
        ] {
            fields.insert(format!("derived:{key}"), value);
        }
        // Item 6 (and the spouse's Part IV Item 3) and Item 7 print what the
        // page shows: the official map ticks Compensation Earner beside a
        // business type for mixed income, which the upload map cannot.
        for (key, value) in super::queueable::QueueableForm::field_map(self) {
            let tail = key.rsplit(':').next().unwrap_or(&key);
            if tail.starts_with("rdoPg1I6TaxpayerType")
                || tail.starts_with("rdoPg2I3SpouseType")
                || tail.starts_with("rdoPg1I7ATC_")
            {
                let key = if key.contains(':') {
                    key.clone()
                } else {
                    format!("frm1701:{key}")
                };
                fields.insert(key, value);
            }
        }
        fields.insert(
            "frm1701:txtPg2I2SpouseRDOCode".to_string(),
            self.spouse_rdo_shown(),
        );
        // The page 2-4 headers print "Taxpayer/Filer's Last Name", as the
        // official page fills them; the XML map keeps its full name.
        for page in [2, 3, 4] {
            fields.insert(
                format!("frm1701:txtPg{page}TaxpayerName"),
                self.page_header_name(),
            );
        }
        fields
    }

    /// Part IV Item 2 as the page shows and the sheet prints it (PR 7,
    /// preview.0): the spouse's RDO the upload files (`spouse_rdo_code`,
    /// which the page writes and an imported save file now sets), blank
    /// while there is none (the upload's `000`) or the spouse column is
    /// closed. The XML-shaped map's `spouse.rdo_code` is not read, so page,
    /// print and upload agree.
    pub fn spouse_rdo_shown(&self) -> String {
        if !self.spouse_column_active() {
            return String::new();
        }
        self.spouse_rdo_code.trim().to_string()
    }

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, _legacy_month: u8) -> Self {
        let mut draft = Self {
            tin: profile.tin.full(),
            taxable_year: year,
            period_end_month: 12,
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            date_of_birth: profile
                .birth_date
                .map(|date| date.format("%m/%d/%Y").to_string())
                .unwrap_or_default(),
            email: profile.email.clone(),
            contact_number: profile.phone.clone(),
            line_of_business: profile.line_of_business.clone(),
            penalty: PenaltySlot::new(PenaltyState::for_profile(profile)),
            // The profile's EOPT class is the filer's, not the spouse's.
            spouse_penalty: PenaltySlot::new(PenaltyState::new(None)),
            ..Self::default()
        };

        let recognized_atcs = profile
            .atc_codes
            .iter()
            .filter_map(|code| Form1701Atc::from_code(code))
            .collect::<BTreeSet<_>>();
        draft.atc = (recognized_atcs.len() == 1)
            .then(|| recognized_atcs.iter().next().copied())
            .flatten();
        draft.taxpayer_type = profile_taxpayer_type(profile, draft.atc);
        let annual_election = annual_income_tax_election(profile, year);
        draft.tax_rate = match annual_election {
            AnnualIncomeTaxElection::Graduated => Some(Form1701TaxRate::Graduated),
            AnnualIncomeTaxElection::EightPercent => Some(Form1701TaxRate::EightPercent),
            AnnualIncomeTaxElection::Unrecorded | AnnualIncomeTaxElection::Conflicting => None,
        };
        let has_osd = profile.tax_elections.iter().any(|election| {
            election.taxable_year == year && election.election == IncomeTaxElection::GraduatedOsd
        });
        let has_itemized = profile.tax_elections.iter().any(|election| {
            election.taxable_year == year
                && election.election == IncomeTaxElection::GraduatedItemized
        });
        draft.deduction_method = match (has_osd, has_itemized) {
            (true, false) => Some(Form1701DeductionMethod::Osd),
            (false, true) => Some(Form1701DeductionMethod::Itemized),
            (false, false) | (true, true) => None,
        };
        if draft.tax_rate == Some(Form1701TaxRate::EightPercent) {
            draft.deduction_method = None;
        }
        draft
    }

    pub fn is_editable(&self) -> bool {
        self.lifecycle.is_editable()
    }

    pub const fn can_queue_for_submission(&self) -> bool {
        QUEUE_SUBMISSION_SUPPORTED
    }

    pub fn xml_evidence_warnings(&self) -> Vec<String> {
        let mut warnings = vec![
            "The reviewed source proves editable-save XML round-trip, not electronic submission semantics; queueing remains disabled."
                .to_string(),
            "The encrypted companion payload is opaque and is not treated as formula or final-flag evidence."
                .to_string(),
            "Part X and attachment worksheet fields are preserved losslessly but are not editable or calculated by this four-page model."
                .to_string(),
        ];
        if !self.has_exact_xml_snapshot {
            warnings.push(
                "This locally-created draft has no imported 837-field exact XML snapshot, so checked XML export is unavailable."
                    .to_string(),
            );
        }
        warnings
    }

    pub fn amount(
        &self,
        section: Form1701AmountSection,
        item: u8,
        party: Form1701Party,
    ) -> Option<f64> {
        self.amount_table(section)
            .get(&item)
            .and_then(|pair| pair.value(party))
    }

    pub fn set_amount(
        &mut self,
        section: Form1701AmountSection,
        item: u8,
        party: Form1701Party,
        value: Option<f64>,
    ) {
        self.amount_table_mut(section)
            .entry(item)
            .or_default()
            .set(party, value);
    }

    fn amount_table(&self, section: Form1701AmountSection) -> &BTreeMap<u8, Form1701AmountPair> {
        match section {
            Form1701AmountSection::PartIi => &self.computations.part_ii,
            Form1701AmountSection::Schedule2 => &self.computations.schedule_2,
            Form1701AmountSection::Schedule3 => &self.computations.schedule_3,
            Form1701AmountSection::Schedule4 => &self.computations.schedule_4,
            Form1701AmountSection::Schedule6 => &self.computations.schedule_6_summary,
            Form1701AmountSection::PartVi => &self.computations.part_vi,
            Form1701AmountSection::PartVii => &self.computations.part_vii,
            Form1701AmountSection::PartViii => &self.computations.part_viii,
            Form1701AmountSection::PartIx => &self.computations.part_ix,
        }
    }

    fn amount_table_mut(
        &mut self,
        section: Form1701AmountSection,
    ) -> &mut BTreeMap<u8, Form1701AmountPair> {
        match section {
            Form1701AmountSection::PartIi => &mut self.computations.part_ii,
            Form1701AmountSection::Schedule2 => &mut self.computations.schedule_2,
            Form1701AmountSection::Schedule3 => &mut self.computations.schedule_3,
            Form1701AmountSection::Schedule4 => &mut self.computations.schedule_4,
            Form1701AmountSection::Schedule6 => &mut self.computations.schedule_6_summary,
            Form1701AmountSection::PartVi => &mut self.computations.part_vi,
            Form1701AmountSection::PartVii => &mut self.computations.part_vii,
            Form1701AmountSection::PartViii => &mut self.computations.part_viii,
            Form1701AmountSection::PartIx => &mut self.computations.part_ix,
        }
    }

    /// The official compute chain (see `form_1701_official`).
    pub fn recompute(&mut self) {
        self.official_recompute();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Form1701AmountSection {
    PartIi,
    Schedule2,
    Schedule3,
    Schedule4,
    Schedule6,
    PartVi,
    PartVii,
    PartViii,
    PartIx,
}

impl FormValidator for Form1701Draft {
    /// The official `validate()` port; see `form_1701_official`.
    fn validate(&self) -> Vec<(String, String)> {
        self.official_errors()
    }
}

impl TypedBirForm for Form1701Draft {
    fn form_code(&self) -> &'static str {
        FORM_CODE
    }

    fn form_type_id(&self) -> &'static str {
        FORM_TYPE_ID
    }

    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Annual
    }

    fn recompute(&mut self) {
        Form1701Draft::recompute(self);
    }

    fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        Form1701Draft::to_bir_field_map(self)
    }
}

fn profile_taxpayer_type(
    profile: &TaxpayerProfile,
    atc: Option<Form1701Atc>,
) -> Option<Form1701TaxpayerType> {
    match profile.taxpayer_type {
        TaxpayerType::Estate => Some(Form1701TaxpayerType::Estate),
        TaxpayerType::Trust => Some(Form1701TaxpayerType::Trust),
        TaxpayerType::Individual => match atc {
            Some(Form1701Atc::Ii011) => Some(Form1701TaxpayerType::CompensationEarner),
            Some(Form1701Atc::Ii012 | Form1701Atc::Ii015) => {
                Some(Form1701TaxpayerType::SingleProprietor)
            }
            Some(Form1701Atc::Ii014 | Form1701Atc::Ii017) => {
                Some(Form1701TaxpayerType::Professional)
            }
            Some(Form1701Atc::Ii013 | Form1701Atc::Ii016) | None => {
                match profile.effective_classification() {
                    Some(TaxClassification::PurelyCompensation) => {
                        Some(Form1701TaxpayerType::CompensationEarner)
                    }
                    _ => None,
                }
            }
        },
        TaxpayerType::Corporation | TaxpayerType::Partnership | TaxpayerType::Cooperative => None,
    }
}

/// Printed January 2018 Form 1701 Tables 1 and 2.
pub fn graduated_income_tax(taxable_year: u16, taxable_income: f64) -> f64 {
    let income = taxable_income.max(0.0);
    if taxable_year <= 2022 {
        if income <= 250_000.0 {
            0.0
        } else if income <= 400_000.0 {
            (income - 250_000.0) * 0.20
        } else if income <= 800_000.0 {
            30_000.0 + (income - 400_000.0) * 0.25
        } else if income <= 2_000_000.0 {
            130_000.0 + (income - 800_000.0) * 0.30
        } else if income <= 8_000_000.0 {
            490_000.0 + (income - 2_000_000.0) * 0.32
        } else {
            2_410_000.0 + (income - 8_000_000.0) * 0.35
        }
    } else if income <= 250_000.0 {
        0.0
    } else if income <= 400_000.0 {
        (income - 250_000.0) * 0.15
    } else if income <= 800_000.0 {
        22_500.0 + (income - 400_000.0) * 0.20
    } else if income <= 2_000_000.0 {
        102_500.0 + (income - 800_000.0) * 0.25
    } else if income <= 8_000_000.0 {
        402_500.0 + (income - 2_000_000.0) * 0.30
    } else {
        2_202_500.0 + (income - 8_000_000.0) * 0.35
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_tax_tables_change_after_2022() {
        assert_eq!(graduated_income_tax(2022, 600_000.0), 80_000.0);
        assert_eq!(graduated_income_tax(2023, 600_000.0), 62_500.0);
        assert_eq!(graduated_income_tax(2025, -1.0), 0.0);
    }

    #[test]
    fn printed_graduated_osd_arithmetic_preserves_overpayment_sign() {
        let mut draft = Form1701Draft {
            taxable_year: 2025,
            atc: Some(Form1701Atc::Ii012),
            tax_rate: Some(Form1701TaxRate::Graduated),
            deduction_method: Some(Form1701DeductionMethod::Osd),
            ..Form1701Draft::default()
        };
        draft.set_amount(
            Form1701AmountSection::Schedule3,
            8,
            Form1701Party::Taxpayer,
            Some(1_000_000.0),
        );
        draft.set_amount(
            Form1701AmountSection::PartVii,
            1,
            Form1701Party::Taxpayer,
            Some(200_000.0),
        );
        draft.recompute();

        assert_eq!(
            draft.amount(
                Form1701AmountSection::Schedule3,
                17,
                Form1701Party::Taxpayer
            ),
            Some(400_000.0)
        );
        assert_eq!(
            draft.amount(
                Form1701AmountSection::Schedule3,
                25,
                Form1701Party::Taxpayer
            ),
            Some(62_500.0)
        );
        assert_eq!(
            draft.amount(Form1701AmountSection::PartIi, 24, Form1701Party::Taxpayer),
            Some(-137_500.0)
        );
        assert_eq!(
            draft.computations.part_ii_item_32_aggregate,
            Some(-137_500.0)
        );
    }

    #[test]
    fn blank_and_explicit_zero_are_distinct() {
        let mut draft = Form1701Draft::default();
        assert_eq!(
            draft.amount(Form1701AmountSection::PartVii, 1, Form1701Party::Taxpayer),
            None
        );
        draft.set_amount(
            Form1701AmountSection::PartVii,
            1,
            Form1701Party::Taxpayer,
            Some(0.0),
        );
        assert_eq!(
            draft.amount(Form1701AmountSection::PartVii, 1, Form1701Party::Taxpayer),
            Some(0.0)
        );
    }
}
