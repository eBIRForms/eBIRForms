//! BIR Form 1701Q, January 2018 (ENCS).
//!
//! This semantic draft is backed by the locked two-page official PDF and the
//! hash-locked `BIR-Form1701Qv2018.hta` source embedded in eBIRForms 7.9.5.0.
//! The HTA proves its editable-save field contract. Electronic submission
//! remains fail-closed because its external encryption and FTP executables are
//! absent from the reviewed package.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::form_2551q::{AnnualIncomeTaxElection, annual_income_tax_election};
use super::penalty_items::PenaltyState;
use super::queueable::SubmissionLifecycle;
use super::{FilingPeriod, FormValidator, TypedBirForm};
use crate::profile::{TaxpayerProfile, TaxpayerType};

pub const FORM_CODE: &str = "1701Q";
pub const FORM_REVISION: &str = "2018";
pub const FORM_TYPE_ID: &str = "1701Qv2018";
pub const OFFICIAL_FORM_SHA256: &str =
    "c731d3f12556e6f19ab81f6113ca7c4a23f7ed099675c03451ac0074d96b85ed";
pub const OFFICIAL_PACKAGE_SHA256: &str =
    "3d087545564531de1fbe8fb28f086ce6398e18608c54a0ea33353042665917eb";
pub const OFFICIAL_PACKAGE_FILE_VERSION: &str = "7.9.5.0";
pub const OFFICIAL_PACKAGE_PRODUCT_VERSION: &str = "7.9.5.0";
pub const OFFICIAL_PACKAGE_RESOURCE_TYPE: u32 = 23;
pub const OFFICIAL_PACKAGE_MANIFEST_RESOURCE_ID: u32 = 129;
pub const OFFICIAL_PACKAGE_MANIFEST_FILE_OFFSET: usize = 369_216;
pub const OFFICIAL_PACKAGE_MANIFEST_SIZE: usize = 26_828;
pub const OFFICIAL_PACKAGE_MANIFEST_SHA256: &str =
    "c8811837405fd76d8924a1c04a6f283a9ed448e3792753da21aaf6ceea191249";
pub const OFFICIAL_HTA_MANIFEST_INDEX: u32 = 41;
pub const OFFICIAL_HTA_RESOURCE_ID: u32 = 170;
pub const OFFICIAL_HTA_RESOURCE_FILE_OFFSET: usize = 12_963_640;
pub const OFFICIAL_HTA_RESOURCE_DECODED_SIZE: usize = 377_887;
pub const OFFICIAL_HTA_RESOURCE_DECODED_SHA256: &str =
    "42f25e268aefe881a2e1fa1d73ac4c47ef17d3ad236b3cbfb7b62af22949592d";
pub const OFFICIAL_EBIRTOOLS_RESOURCE_ID: u32 = 553;
pub const OFFICIAL_EBIRTOOLS_RESOURCE_FILE_OFFSET: usize = 54_862_324;
pub const OFFICIAL_EBIRTOOLS_RESOURCE_DECODED_SIZE: usize = 6_451;
pub const OFFICIAL_EBIRTOOLS_RESOURCE_DECODED_SHA256: &str =
    "aaf5dbe9593ca81f808540e537353f297f9bd8638e488ea5161673e3985a91bc";
pub const OFFICIAL_ENVIRONMENT_RESOURCE_DECODED_SHA256: &str =
    "01de5f90ad3c5a65af5c1ccdb61a8968d3c61e5d542eb160b6e0eb3432a3be4e";
pub const OFFICIAL_STRING_UTIL_RESOURCE_DECODED_SHA256: &str =
    "8d3f3527e044a5325b1f9019d234717d60c5bb1f72692ea302eb4f9e9cb43d6f";
pub const EXACT_EDITABLE_XML_FIELD_COUNT: usize = 172;
pub const EXACT_OUTBOUND_XML_FIELD_COUNT: usize = 173;
pub const EXACT_RUNTIME_SERIALIZABLE_ELEMENT_COUNT: usize = 173;
/// SHA-256 of the newline-terminated runtime element IDs in DOM order, before
/// the editable serializer combines its two address controls into one field.
pub const EXACT_RUNTIME_ELEMENT_IDS_SHA256: &str =
    "f6ad924c5263f7c543b5ebadc939258bf0e3182f6ff07cd92a0e6fa1195f453e";
/// SHA-256 of the newline-terminated editable-save field IDs in emitted order.
pub const EXACT_EDITABLE_FIELD_IDS_SHA256: &str =
    "a135fd015a3e3c349f4e6baffce52317734cb478e8a3a1d62072901c050acb3d";
pub const XML_ROUND_TRIP_SUPPORTED: bool = true;
pub const QUEUE_SUBMISSION_SUPPORTED: bool = false;

/// Item 7 on page 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701QFilerType {
    SingleProprietor,
    Professional,
    Estate,
    Trust,
}

impl Form1701QFilerType {
    pub const ALL: [Self; 4] = [
        Self::SingleProprietor,
        Self::Professional,
        Self::Estate,
        Self::Trust,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SingleProprietor => "Single Proprietor",
            Self::Professional => "Professional",
            Self::Estate => "Estate",
            Self::Trust => "Trust",
        }
    }
}

/// Item 19 on page 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701QSpouseType {
    SingleProprietor,
    Professional,
    CompensationEarner,
}

impl Form1701QSpouseType {
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

/// The exact ATC choices printed in Items 8 and 20.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701QAtc {
    Ii011,
    Ii012,
    Ii013,
    Ii014,
    Ii015,
    Ii016,
    Ii017,
}

impl Form1701QAtc {
    pub const TAXPAYER_CHOICES: [Self; 6] = [
        Self::Ii012,
        Self::Ii014,
        Self::Ii013,
        Self::Ii015,
        Self::Ii017,
        Self::Ii016,
    ];
    pub const SPOUSE_CHOICES: [Self; 7] = [
        Self::Ii012,
        Self::Ii014,
        Self::Ii013,
        Self::Ii011,
        Self::Ii015,
        Self::Ii017,
        Self::Ii016,
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

    pub const fn tax_rate(self) -> Option<Form1701QTaxRate> {
        match self {
            Self::Ii012 | Self::Ii013 | Self::Ii014 => Some(Form1701QTaxRate::Graduated),
            Self::Ii015 | Self::Ii016 | Self::Ii017 => Some(Form1701QTaxRate::EightPercent),
            Self::Ii011 => None,
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

/// Items 16 and 25.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701QTaxRate {
    Graduated,
    EightPercent,
}

impl Form1701QTaxRate {
    pub const ALL: [Self; 2] = [Self::Graduated, Self::EightPercent];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Graduated => "Graduated Rates",
            Self::EightPercent => "8% IT Rate",
        }
    }
}

/// Items 16A and 25A. It applies only when the corresponding rate is graduated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701QDeductionMethod {
    Itemized,
    Osd,
}

impl Form1701QDeductionMethod {
    pub const ALL: [Self; 2] = [Self::Itemized, Self::Osd];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Itemized => "Itemized Deduction",
            Self::Osd => "Optional Standard Deduction (OSD)",
        }
    }
}

/// The filer's civil status. The January 2018 sheet prints no such item (and
/// the upload carries none); it is the page's answer that opens Part II: the
/// spouse part applies only to a married filer (`has_spouse`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1701QCivilStatus {
    Single,
    Married,
    LegallySeparated,
    Widowed,
}

impl Form1701QCivilStatus {
    pub const ALL: [Self; 4] = [
        Self::Single,
        Self::Married,
        Self::LegallySeparated,
        Self::Widowed,
    ];

    /// The words BIR prints for the same answer on Form 1701 (item 16).
    pub const fn label(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Married => "Married",
            Self::LegallySeparated => "Legally Separated",
            Self::Widowed => "Widow/er",
        }
    }

    pub fn from_label(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.label().eq_ignore_ascii_case(text.trim()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form1701QParty {
    Taxpayer,
    Spouse,
}

/// A paired amount cell. `None` preserves an officially blank cell and is not
/// equivalent to an entered zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701QAmountPair {
    pub taxpayer: Option<f64>,
    pub spouse: Option<f64>,
}

impl Form1701QAmountPair {
    pub const fn value(&self, party: Form1701QParty) -> Option<f64> {
        match party {
            Form1701QParty::Taxpayer => self.taxpayer,
            Form1701QParty::Spouse => self.spouse,
        }
    }

    pub fn set(&mut self, party: Form1701QParty, value: Option<f64>) {
        match party {
            Form1701QParty::Taxpayer => self.taxpayer = value,
            Form1701QParty::Spouse => self.spouse = value,
        }
    }
}

/// Every paired amount line printed in Parts III and V.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701QAmounts {
    pub item_26: Form1701QAmountPair,
    pub item_27: Form1701QAmountPair,
    pub item_28: Form1701QAmountPair,
    pub item_29: Form1701QAmountPair,
    pub item_30: Form1701QAmountPair,
    pub item_36: Form1701QAmountPair,
    pub item_37: Form1701QAmountPair,
    pub item_38: Form1701QAmountPair,
    pub item_39: Form1701QAmountPair,
    pub item_40: Form1701QAmountPair,
    pub item_41: Form1701QAmountPair,
    pub item_42: Form1701QAmountPair,
    pub item_43: Form1701QAmountPair,
    pub item_44: Form1701QAmountPair,
    pub item_45: Form1701QAmountPair,
    pub item_46: Form1701QAmountPair,
    pub item_47: Form1701QAmountPair,
    pub item_48: Form1701QAmountPair,
    pub item_49: Form1701QAmountPair,
    pub item_50: Form1701QAmountPair,
    pub item_51: Form1701QAmountPair,
    pub item_52: Form1701QAmountPair,
    pub item_53: Form1701QAmountPair,
    pub item_54: Form1701QAmountPair,
    pub item_55: Form1701QAmountPair,
    pub item_56: Form1701QAmountPair,
    pub item_57: Form1701QAmountPair,
    pub item_58: Form1701QAmountPair,
    pub item_59: Form1701QAmountPair,
    pub item_60: Form1701QAmountPair,
    pub item_61: Form1701QAmountPair,
    pub item_62: Form1701QAmountPair,
    pub item_63: Form1701QAmountPair,
    pub item_64: Form1701QAmountPair,
    pub item_65: Form1701QAmountPair,
    pub item_66: Form1701QAmountPair,
    pub item_67: Form1701QAmountPair,
    pub item_68: Form1701QAmountPair,
}

impl Form1701QAmounts {
    pub fn get(&self, item: u8) -> Option<&Form1701QAmountPair> {
        Some(match item {
            26 => &self.item_26,
            27 => &self.item_27,
            28 => &self.item_28,
            29 => &self.item_29,
            30 => &self.item_30,
            36 => &self.item_36,
            37 => &self.item_37,
            38 => &self.item_38,
            39 => &self.item_39,
            40 => &self.item_40,
            41 => &self.item_41,
            42 => &self.item_42,
            43 => &self.item_43,
            44 => &self.item_44,
            45 => &self.item_45,
            46 => &self.item_46,
            47 => &self.item_47,
            48 => &self.item_48,
            49 => &self.item_49,
            50 => &self.item_50,
            51 => &self.item_51,
            52 => &self.item_52,
            53 => &self.item_53,
            54 => &self.item_54,
            55 => &self.item_55,
            56 => &self.item_56,
            57 => &self.item_57,
            58 => &self.item_58,
            59 => &self.item_59,
            60 => &self.item_60,
            61 => &self.item_61,
            62 => &self.item_62,
            63 => &self.item_63,
            64 => &self.item_64,
            65 => &self.item_65,
            66 => &self.item_66,
            67 => &self.item_67,
            68 => &self.item_68,
            _ => return None,
        })
    }

    pub fn get_mut(&mut self, item: u8) -> Option<&mut Form1701QAmountPair> {
        Some(match item {
            26 => &mut self.item_26,
            27 => &mut self.item_27,
            28 => &mut self.item_28,
            29 => &mut self.item_29,
            30 => &mut self.item_30,
            36 => &mut self.item_36,
            37 => &mut self.item_37,
            38 => &mut self.item_38,
            39 => &mut self.item_39,
            40 => &mut self.item_40,
            41 => &mut self.item_41,
            42 => &mut self.item_42,
            43 => &mut self.item_43,
            44 => &mut self.item_44,
            45 => &mut self.item_45,
            46 => &mut self.item_46,
            47 => &mut self.item_47,
            48 => &mut self.item_48,
            49 => &mut self.item_49,
            50 => &mut self.item_50,
            51 => &mut self.item_51,
            52 => &mut self.item_52,
            53 => &mut self.item_53,
            54 => &mut self.item_54,
            55 => &mut self.item_55,
            56 => &mut self.item_56,
            57 => &mut self.item_57,
            58 => &mut self.item_58,
            59 => &mut self.item_59,
            60 => &mut self.item_60,
            61 => &mut self.item_61,
            62 => &mut self.item_62,
            63 => &mut self.item_63,
            64 => &mut self.item_64,
            65 => &mut self.item_65,
            66 => &mut self.item_66,
            67 => &mut self.item_67,
            68 => &mut self.item_68,
            _ => return None,
        })
    }
}

/// One row in Part IV and its exact editable-save transport fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701QPaymentRow {
    pub drawee_bank_or_agency: String,
    pub number: String,
    pub date: String,
    pub amount: Option<f64>,
}

impl Form1701QPaymentRow {
    pub fn is_empty(&self) -> bool {
        self.drawee_bank_or_agency.trim().is_empty()
            && self.number.trim().is_empty()
            && self.date.trim().is_empty()
            && self.amount.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701QPaymentDetails {
    pub item_32_cash_or_bank_debit_memo: Form1701QPaymentRow,
    pub item_33_check: Form1701QPaymentRow,
    pub item_34_tax_debit_memo: Form1701QPaymentRow,
    pub item_35_others: Form1701QPaymentRow,
    pub item_35_others_description: String,
    pub machine_validation_or_receipt_details: String,
}

/// The amounts the filer types, in both columns. Item 52 (the P250,000
/// allowable reduction) is typed too (`item52Validate` checks what is
/// entered), so a save file restores it.
pub const USER_ENTERED_AMOUNT_ITEMS: &[u8] = &[
    36, 37, 39, 42, 43, 44, 47, 48, 50, 52, 55, 56, 57, 58, 59, 60, 61, 64, 65, 66,
];

/// App-owned semantic draft for the exact January 2018 revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form1701QDraft {
    pub id: Option<i64>,

    // Items 1-4.
    pub taxable_year: u16,
    pub quarter: u8,
    #[serde(default)]
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_sheets: u8,

    // Items 5-16A.
    pub tin: String,
    pub rdo_code: String,
    #[serde(default)]
    pub filer_type: Option<Form1701QFilerType>,
    #[serde(default)]
    pub atc: Option<Form1701QAtc>,
    pub taxpayer_name: String,
    /// Page 2 prints this separately from Item 9's full name. It remains blank
    /// until explicitly supplied because the profile owns only an unstructured
    /// full-name string.
    #[serde(default)]
    pub taxpayer_last_name: String,
    pub registered_address: String,
    #[serde(default)]
    pub registered_address_2: String,
    pub zip_code: String,
    #[serde(default)]
    pub date_of_birth: String,
    pub email: String,
    #[serde(default)]
    pub citizenship: String,
    #[serde(default)]
    pub foreign_tax_number: String,
    #[serde(default)]
    pub claims_foreign_tax_credits: Option<bool>,
    #[serde(default)]
    pub tax_rate: Option<Form1701QTaxRate>,
    #[serde(default)]
    pub deduction_method: Option<Form1701QDeductionMethod>,
    /// Profile metadata retained for the generic render envelope. The official
    /// January 2018 Form 1701Q does not print a contact-number field.
    pub contact_number: String,
    /// Submission/profile metadata serialized by the official HTA but not
    /// printed in the two-page January 2018 form.
    #[serde(default)]
    pub line_of_business: String,

    /// The page's Civil Status answer for a status other than Married (a
    /// married filer is `has_spouse`). `None` until the filer answers; a
    /// draft stored before the answer existed has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub civil_status: Option<Form1701QCivilStatus>,

    // Items 17-25A.
    #[serde(default)]
    pub has_spouse: bool,
    #[serde(default)]
    pub spouse_tin: String,
    #[serde(default)]
    pub spouse_rdo_code: String,
    #[serde(default)]
    pub spouse_type: Option<Form1701QSpouseType>,
    #[serde(default)]
    pub spouse_atc: Option<Form1701QAtc>,
    #[serde(default)]
    pub spouse_name: String,
    #[serde(default)]
    pub spouse_citizenship: String,
    #[serde(default)]
    pub spouse_foreign_tax_number: String,
    #[serde(default)]
    pub spouse_claims_foreign_tax_credits: Option<bool>,
    #[serde(default)]
    pub spouse_tax_rate: Option<Form1701QTaxRate>,
    #[serde(default)]
    pub spouse_deduction_method: Option<Form1701QDeductionMethod>,

    // Parts III and V.
    #[serde(default)]
    pub amounts: Form1701QAmounts,
    #[serde(default)]
    pub item_31_aggregate_amount_payable: Option<f64>,
    #[serde(default)]
    pub item_43_non_operating_income_description: String,
    #[serde(default)]
    pub item_48_non_operating_income_description: String,
    #[serde(default)]
    pub item_61_other_tax_credit_description: String,

    /// Each party's Surcharge, Interest and Compromise (Items 64-66, columns
    /// A and B) on the shared penalty model ([`super::penalty_items`]):
    /// computed values, overrides and engine inputs, one state per column
    /// (the draft is flat, so they are not flattened into it). A column at
    /// `penalty_model_version` 0 still holds the filer's own typed amounts.
    /// The stored shape is kept: a draft stored with no `penalty` key is
    /// written back without it while it is the default, and one stored with
    /// the key (even at its default) is written back with it
    /// ([`PenaltySlot`]).
    #[serde(default, skip_serializing_if = "PenaltySlot::is_absent")]
    pub penalty: PenaltySlot,
    #[serde(default, skip_serializing_if = "PenaltySlot::is_absent")]
    pub spouse_penalty: PenaltySlot,

    // Part IV.
    #[serde(default)]
    pub payment_details: Form1701QPaymentDetails,

    // Compatibility aggregates used by existing dashboard/preview callers.
    #[serde(default)]
    pub total_tax_due: f64,
    #[serde(default)]
    pub total_amount_payable: f64,

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

impl Form1701QDraft {
    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the later pages' header TIN, one 14-digit
    ///   comb (TIN then branch).
    /// - `derived:page2_name`: the page 2 header's last name. A new return
    ///   leaves `taxpayer_last_name` blank, so the header falls back to the
    ///   part of the taxpayer name before its first comma (Last Name, First
    ///   Name), or the whole name when it has none.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let last_name = if self.taxpayer_last_name.trim().is_empty() {
            self.taxpayer_name.split(',').next().unwrap_or_default()
        } else {
            self.taxpayer_last_name.as_str()
        };
        fields.insert(
            "derived:page2_name".to_string(),
            last_name.trim().to_uppercase(),
        );
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm1701q:txtPg2TIN1",
                "frm1701q:txtPg2TIN2",
                "frm1701q:txtPg2TIN3",
                "frm1701q:txtPg2BranchCode",
            ],
        );
        fields.insert("derived:tin_digits".to_string(), tin_digits);
        fields
    }

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let (tax_rate, deduction_method) = match annual_income_tax_election(profile, year) {
            AnnualIncomeTaxElection::EightPercent => (Some(Form1701QTaxRate::EightPercent), None),
            AnnualIncomeTaxElection::Graduated => {
                let method = profile
                    .tax_elections
                    .iter()
                    .rev()
                    .find(|entry| entry.taxable_year == year)
                    .and_then(|entry| match entry.election {
                        crate::profile::IncomeTaxElection::GraduatedOsd => {
                            Some(Form1701QDeductionMethod::Osd)
                        }
                        crate::profile::IncomeTaxElection::GraduatedItemized => {
                            Some(Form1701QDeductionMethod::Itemized)
                        }
                        crate::profile::IncomeTaxElection::GraduatedUnspecified
                        | crate::profile::IncomeTaxElection::EightPercent => None,
                    });
                (Some(Form1701QTaxRate::Graduated), method)
            }
            AnnualIncomeTaxElection::Unrecorded | AnnualIncomeTaxElection::Conflicting => {
                (None, None)
            }
        };
        let atc = profile
            .atc_codes
            .iter()
            .filter_map(|code| Form1701QAtc::from_code(code))
            .find(|candidate| *candidate != Form1701QAtc::Ii011);
        let filer_type = match profile.taxpayer_type {
            TaxpayerType::Estate => Some(Form1701QFilerType::Estate),
            TaxpayerType::Trust => Some(Form1701QFilerType::Trust),
            TaxpayerType::Individual
            | TaxpayerType::Corporation
            | TaxpayerType::Partnership
            | TaxpayerType::Cooperative => None,
        };

        Self {
            id: None,
            taxable_year: year,
            quarter,
            is_amended: false,
            number_of_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            filer_type,
            atc,
            taxpayer_name: profile.full_name.clone(),
            taxpayer_last_name: String::new(),
            registered_address: profile.registered_address.clone(),
            registered_address_2: String::new(),
            zip_code: profile.zip_code.clone(),
            date_of_birth: profile
                .birth_date
                .map(|date| date.format("%m/%d/%Y").to_string())
                .unwrap_or_default(),
            email: profile.email.clone(),
            citizenship: String::new(),
            foreign_tax_number: String::new(),
            claims_foreign_tax_credits: None,
            tax_rate,
            deduction_method,
            contact_number: profile.phone.clone(),
            line_of_business: profile.line_of_business.clone(),
            civil_status: None,
            has_spouse: false,
            spouse_tin: String::new(),
            spouse_rdo_code: String::new(),
            spouse_type: None,
            spouse_atc: None,
            spouse_name: String::new(),
            spouse_citizenship: String::new(),
            spouse_foreign_tax_number: String::new(),
            spouse_claims_foreign_tax_credits: None,
            spouse_tax_rate: None,
            spouse_deduction_method: None,
            amounts: Form1701QAmounts::default(),
            item_31_aggregate_amount_payable: None,
            item_43_non_operating_income_description: String::new(),
            item_48_non_operating_income_description: String::new(),
            item_61_other_tax_credit_description: String::new(),
            penalty: PenaltySlot::new(PenaltyState::for_profile(profile)),
            // The profile's EOPT class is the filer's, not the spouse's.
            spouse_penalty: PenaltySlot::new(PenaltyState::new(None)),
            payment_details: Form1701QPaymentDetails::default(),
            total_tax_due: 0.0,
            total_amount_payable: 0.0,
            last_error: None,
            lifecycle: SubmissionLifecycle::default(),
        }
    }

    pub const fn form_code(&self) -> &'static str {
        FORM_CODE
    }

    pub const fn form_type_id(&self) -> &'static str {
        FORM_TYPE_ID
    }

    pub const fn taxable_year_u16(&self) -> u16 {
        self.taxable_year
    }

    pub const fn quarter_u8(&self) -> u8 {
        self.quarter
    }

    pub fn period_code(&self) -> String {
        format!("{}Q{}", self.taxable_year, self.quarter)
    }

    pub fn default_submission_filename(&self) -> String {
        format!(
            "{}-{FORM_TYPE_ID}-{}.xml",
            self.tin.replace('-', ""),
            self.period_code()
        )
    }

    pub fn amount(&self, item: u8, party: Form1701QParty) -> Option<f64> {
        self.amounts.get(item).and_then(|pair| pair.value(party))
    }

    pub fn set_amount(&mut self, item: u8, party: Form1701QParty, value: Option<f64>) {
        if let Some(pair) = self.amounts.get_mut(item) {
            pair.set(party, value);
        }
    }

    /// The Civil Status the page shows: Married for a draft that reports a
    /// spouse (`has_spouse`, which a draft stored before the answer existed
    /// may already carry), else the other status answered, if any.
    pub fn shown_civil_status(&self) -> Option<Form1701QCivilStatus> {
        if self.has_spouse {
            Some(Form1701QCivilStatus::Married)
        } else {
            self.civil_status
                .filter(|status| *status != Form1701QCivilStatus::Married)
        }
    }

    /// Answer Civil Status. Married opens Part II and the spouse's column
    /// (`has_spouse`); any other answer, or none, closes them and clears
    /// everything entered there (items 17-25A, the B column, the spouse's
    /// penalty overrides), so nothing of it is filed or printed. Recomputes.
    pub fn set_civil_status(&mut self, status: Option<Form1701QCivilStatus>) {
        let married = status == Some(Form1701QCivilStatus::Married);
        self.civil_status = status.filter(|_| !married);
        if married {
            // A draft stored while the spouse's penalties were typed boxes
            // had no spouse here: there is nothing of theirs to keep.
            if !self.has_spouse
                && self.spouse_penalty.penalty_model_version
                    < super::penalty_items::PENALTY_MODEL_VERSION
            {
                self.spouse_penalty.state = PenaltyState::new(None);
            }
            self.has_spouse = true;
        } else {
            self.has_spouse = false;
            self.clear_spouse_part();
        }
        self.recompute();
    }

    /// Whether Part II or the spouse's column holds anything entered.
    pub fn spouse_part_has_entries(&self) -> bool {
        let text = [
            &self.spouse_tin,
            &self.spouse_rdo_code,
            &self.spouse_name,
            &self.spouse_citizenship,
            &self.spouse_foreign_tax_number,
        ]
        .iter()
        .any(|value| !value.trim().is_empty());
        let chosen = self.spouse_type.is_some()
            || self.spouse_atc.is_some()
            || self.spouse_claims_foreign_tax_credits.is_some()
            || self.spouse_tax_rate.is_some()
            || self.spouse_deduction_method.is_some();
        let typed = USER_ENTERED_AMOUNT_ITEMS.iter().any(|item| {
            self.amount(*item, Form1701QParty::Spouse)
                .is_some_and(|v| v != 0.0)
        });
        let overridden = crate::penalties::PenaltyComponent::ALL
            .iter()
            .any(|component| {
                self.spouse_penalty
                    .penalty_overrides
                    .get(*component)
                    .is_some_and(|value| value != 0.0)
            });
        text || chosen || typed || overridden
    }

    /// Blank Part II and the spouse's column.
    fn clear_spouse_part(&mut self) {
        self.spouse_tin.clear();
        self.spouse_rdo_code.clear();
        self.spouse_type = None;
        self.spouse_atc = None;
        self.spouse_name.clear();
        self.spouse_citizenship.clear();
        self.spouse_foreign_tax_number.clear();
        self.spouse_claims_foreign_tax_credits = None;
        self.spouse_tax_rate = None;
        self.spouse_deduction_method = None;
        for item in [
            26, 27, 28, 29, 30, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52,
            53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68,
        ] {
            self.set_amount(item, Form1701QParty::Spouse, None);
        }
        self.spouse_penalty.clear_overrides();
        self.spouse_penalty.computed_penalties = None;
    }

    /// The official compute chain (see `form_1701q_official`).
    pub fn recompute(&mut self) {
        self.official_recompute();
    }

    pub fn is_editable(&self) -> bool {
        self.lifecycle.is_editable()
    }
}

impl FormValidator for Form1701QDraft {
    /// The official `validate()` port; see `form_1701q_official`.
    fn validate(&self) -> Vec<(String, String)> {
        self.official_errors()
    }
}

/// Applies only the two rate tables printed on page 2 of the locked form.
pub fn graduated_tax_due(taxable_year: u16, taxable_income: f64) -> f64 {
    let income = taxable_income;
    if income <= 250_000.0 {
        return 0.0;
    }
    if taxable_year <= 2022 {
        match income {
            value if value <= 400_000.0 => (value - 250_000.0) * 0.20,
            value if value <= 800_000.0 => 30_000.0 + (value - 400_000.0) * 0.25,
            value if value <= 2_000_000.0 => 130_000.0 + (value - 800_000.0) * 0.30,
            value if value <= 8_000_000.0 => 490_000.0 + (value - 2_000_000.0) * 0.32,
            value => 2_410_000.0 + (value - 8_000_000.0) * 0.35,
        }
    } else {
        match income {
            value if value <= 400_000.0 => (value - 250_000.0) * 0.15,
            value if value <= 800_000.0 => 22_500.0 + (value - 400_000.0) * 0.20,
            value if value <= 2_000_000.0 => 102_500.0 + (value - 800_000.0) * 0.25,
            value if value <= 8_000_000.0 => 402_500.0 + (value - 2_000_000.0) * 0.30,
            value => 2_202_500.0 + (value - 8_000_000.0) * 0.35,
        }
    }
}

impl TypedBirForm for Form1701QDraft {
    fn form_code(&self) -> &'static str {
        FORM_CODE
    }

    fn form_type_id(&self) -> &'static str {
        FORM_TYPE_ID
    }

    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Quarterly(self.quarter)
    }

    fn recompute(&mut self) {
        Form1701QDraft::recompute(self);
    }

    fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        Form1701QDraft::to_bir_field_map(self)
    }

    fn to_bir_xml(&self) -> String {
        self.to_bir_xml_payload().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The official compute, validation and payload tests live in
    // `form_1701q_official`.

    #[test]
    fn graduated_tax_due_should_use_2023_onward_printed_table() {
        assert_eq!(graduated_tax_due(2026, 600_000.0), 62_500.0);
    }

    #[test]
    fn amount_registry_should_own_every_official_paired_line() {
        let amounts = Form1701QAmounts::default();
        let expected = [
            26, 27, 28, 29, 30, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52,
            53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68,
        ];

        assert!(expected.into_iter().all(|item| amounts.get(item).is_some()));
    }

    /// A married filer with a spouse on graduated rates and typed amounts.
    fn married() -> Form1701QDraft {
        let mut draft = Form1701QDraft {
            taxable_year: 2025,
            quarter: 2,
            filer_type: Some(Form1701QFilerType::Professional),
            atc: Some(Form1701QAtc::Ii014),
            deduction_method: Some(Form1701QDeductionMethod::Osd),
            penalty: PenaltySlot::new(PenaltyState::new(None)),
            spouse_penalty: PenaltySlot::new(PenaltyState::new(None)),
            ..Default::default()
        };
        draft.set_civil_status(Some(Form1701QCivilStatus::Married));
        draft.spouse_tin = "987654321".into();
        draft.spouse_name = "SAMPLE SPOUSE".into();
        draft.spouse_type = Some(Form1701QSpouseType::Professional);
        draft.spouse_atc = Some(Form1701QAtc::Ii014);
        draft.spouse_deduction_method = Some(Form1701QDeductionMethod::Itemized);
        draft.set_amount(36, Form1701QParty::Spouse, Some(500_000.0));
        draft.set_amount(58, Form1701QParty::Spouse, Some(1_000.0));
        draft.recompute();
        draft
    }

    /// Decision 11: Married opens Part II; another answer closes it and
    /// clears everything entered there, so none of it files.
    #[test]
    fn civil_status_married_opens_part_ii_and_another_answer_clears_it() {
        let draft = married();
        assert!(draft.has_spouse);
        assert_eq!(
            draft.shown_civil_status(),
            Some(Form1701QCivilStatus::Married)
        );
        assert!(draft.spouse_part_has_entries());
        let fields = draft.to_official_field_map();
        assert_eq!(fields["frm1701q:txtSpouseName"], "SAMPLE SPOUSE");
        assert_eq!(fields["frm1701q:txt36B"], "500,000.00");

        for status in [
            Some(Form1701QCivilStatus::Single),
            Some(Form1701QCivilStatus::LegallySeparated),
            Some(Form1701QCivilStatus::Widowed),
            None,
        ] {
            let mut closed = draft.clone();
            closed.set_civil_status(status);
            assert!(!closed.has_spouse);
            assert_eq!(closed.shown_civil_status(), status);
            assert!(!closed.spouse_part_has_entries(), "{status:?}");
            assert_eq!(closed.spouse_name, "");
            assert_eq!(closed.spouse_tin, "");
            assert_eq!(closed.spouse_type, None);
            assert_eq!(closed.spouse_atc, None);
            assert_eq!(closed.amount(36, Form1701QParty::Spouse), Some(0.0));
            let fields = closed.to_official_field_map();
            assert_eq!(fields["frm1701q:txtSpouseName"], "");
            assert_eq!(fields["frm1701q:txt36B"], "0.00");
            assert_eq!(fields["frm1701q:txt26B"], "0.00");
            // The taxpayer's column is untouched.
            assert_eq!(
                fields["frm1701q:txt26A"],
                draft.to_official_field_map()["frm1701q:txt26A"]
            );
        }
    }

    /// The answer is the page's: it is not in the upload, and a draft
    /// stored before it existed reads and writes as it was.
    #[test]
    fn civil_status_is_not_filed_and_legacy_drafts_keep_their_shape() {
        let mut single = married();
        single.set_civil_status(Some(Form1701QCivilStatus::Single));
        let mut unanswered = married();
        unanswered.set_civil_status(None);
        assert_eq!(
            single.to_official_field_map(),
            unanswered.to_official_field_map()
        );
        assert_eq!(
            single.to_official_xml_payload(),
            unanswered.to_official_xml_payload()
        );

        // A stored draft from before: a spouse means Married, none means
        // unanswered, and it serializes without the new key.
        let mut stored = serde_json::to_value(married()).unwrap();
        assert!(stored.get("civil_status").is_none());
        stored.as_object_mut().unwrap().remove("penalty");
        let legacy: Form1701QDraft = serde_json::from_value(stored).unwrap();
        assert_eq!(
            legacy.shown_civil_status(),
            Some(Form1701QCivilStatus::Married)
        );
        let mut alone = serde_json::to_value(Form1701QDraft::default()).unwrap();
        alone.as_object_mut().unwrap().remove("civil_status");
        let alone: Form1701QDraft = serde_json::from_value(alone).unwrap();
        assert_eq!(alone.shown_civil_status(), None);
        assert_eq!(
            Form1701QCivilStatus::from_label("widow/er"),
            Some(Form1701QCivilStatus::Widowed)
        );
    }

    #[test]
    fn json_round_trip_should_preserve_choices_and_signed_amounts() {
        let mut draft = Form1701QDraft {
            taxable_year: 2021,
            quarter: 2,
            filer_type: Some(Form1701QFilerType::SingleProprietor),
            atc: Some(Form1701QAtc::Ii012),
            deduction_method: Some(Form1701QDeductionMethod::Osd),
            has_spouse: true,
            spouse_type: Some(Form1701QSpouseType::CompensationEarner),
            ..Default::default()
        };
        draft.set_amount(42, Form1701QParty::Taxpayer, Some(-25_000.0));
        draft.recompute();

        let json = serde_json::to_string(&draft).expect("1701Q draft should serialize");
        let restored: Form1701QDraft =
            serde_json::from_str(&json).expect("1701Q draft should deserialize");

        assert_eq!(restored, draft);
        assert_eq!(
            restored.amount(42, Form1701QParty::Taxpayer),
            Some(-25_000.0)
        );
    }
}
