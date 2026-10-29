//! BIR Form 2550Q, April 2024 (ENCS).
//!
//! The model is bounded by the locked two-page official form, its official
//! guidelines, and the reviewed 160-field plain/encrypted editable-save pair.
//! The payloads establish editable persistence, not electronic submission.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use bir_rules::{RawValue, StableInstanceId};
use serde::{Deserialize, Deserializer, Serialize};

use super::queueable::SubmissionLifecycle;
use super::{FilingPeriod, FilingStatus, FormValidator};
use crate::filing_queue::QueueAuthSource;
use crate::form_rules::validate_form_2550q_raw_bindings;
use crate::profile::{EoptTier, TaxpayerProfile};
use crate::validation::{validate_email, validate_ph_phone, validate_zip};

pub const FORM_CODE: &str = "2550Q";
pub const FORM_REVISION: &str = "2024";
pub const FORM_TYPE_ID: &str = "2550Qv2024";
pub const FORM_VERSION_LABEL: &str = "April 2024 (ENCS)";
pub const QUEUE_SUBMISSION_SUPPORTED: bool = true;
pub const OFFICIAL_FORM_SHA256: &str =
    "18eb16925010fdda820cef958221ba2c0d073066efa93a898113e39b31135a25";
pub const OFFICIAL_GUIDELINES_SHA256: &str =
    "b6ee4f090cb48963a44b1ef58fd6cdb4b5865ba4674963c3661c7f164895b120";
pub const REVIEWED_EDITABLE_XML_SHA256: &str =
    "43577fdd70b8959b16dbada9ff7d8418a1fdc5d18e61302c8cbfc8e9bbab4520";
pub const REVIEWED_ENCRYPTED_XML_SHA256: &str =
    "57ccf9d8132c490d54bceaf5c55fc2b4bec01b780951a63600402c61a595cdbe";

const VAT_RATE: f64 = 0.12;
/// `computeSched1`: "Only dates on or before December 31, 2021 are allowed."
fn is_after_schedule_1_cutoff(date: Form2550QDate) -> bool {
    date.as_naive()
        .is_some_and(|date| date > chrono::NaiveDate::from_ymd_opt(2021, 12, 31).expect("date"))
}
const FIRST_REPEATED_ROW_ORDINAL: u64 = 1;
const LEGACY_FLAT_SHAPE_ANCHORS: [&str; 11] = [
    "opt_short_prd1",
    "opt_short_prd2",
    "pg2tax_payer",
    "rtn_period_from_no4",
    "rtn_period_to_no4",
    "calendar_no1",
    "fiscal_no1",
    "amended_return_no5",
    "amended_return_yes_no5",
    "txt_current_page",
    "txt_max_page",
];
pub(super) const PRESERVED_UNMODELED_XML_KEYS: [&str; 4] = [
    "resultOtherCreditsNo19",
    "resultOtherCreditsNo42",
    "resultOtherCreditsNo47",
    "resultOtherCreditsNo56",
];

pub(super) fn is_preserved_unmodeled_xml_key(key: &str) -> bool {
    PRESERVED_UNMODELED_XML_KEYS.contains(&key)
}

const fn default_next_repeated_row_ordinal() -> u64 {
    FIRST_REPEATED_ROW_ORDINAL
}

const fn default_raw_editor_state_version() -> u8 {
    1
}

const fn default_year_end_month() -> u8 {
    12
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Form2550QFilingBasis {
    #[default]
    Calendar,
    Fiscal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form2550QQuarter {
    First,
    Second,
    Third,
    Fourth,
    /// Retains invalid legacy/caller data so validation can fail closed.
    Unresolved(u8),
}

impl Default for Form2550QQuarter {
    fn default() -> Self {
        Self::Unresolved(0)
    }
}

impl Form2550QQuarter {
    pub const ALL: [Self; 4] = [Self::First, Self::Second, Self::Third, Self::Fourth];

    pub const fn from_number(value: u8) -> Self {
        match value {
            1 => Self::First,
            2 => Self::Second,
            3 => Self::Third,
            4 => Self::Fourth,
            other => Self::Unresolved(other),
        }
    }

    pub const fn number(self) -> Option<u8> {
        match self {
            Self::First => Some(1),
            Self::Second => Some(2),
            Self::Third => Some(3),
            Self::Fourth => Some(4),
            Self::Unresolved(_) => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::First => "1st",
            Self::Second => "2nd",
            Self::Third => "3rd",
            Self::Fourth => "4th",
            Self::Unresolved(_) => "Needs review",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form2550QTaxpayerClassification {
    Micro,
    Small,
    Medium,
    Large,
}

impl Form2550QTaxpayerClassification {
    pub const ALL: [Self; 4] = [Self::Micro, Self::Small, Self::Medium, Self::Large];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Micro => "Micro",
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
        }
    }
}

impl From<&EoptTier> for Form2550QTaxpayerClassification {
    fn from(value: &EoptTier) -> Self {
        match value {
            EoptTier::Micro => Self::Micro,
            EoptTier::Small => Self::Small,
            EoptTier::Medium => Self::Medium,
            EoptTier::Large => Self::Large,
        }
    }
}

/// The reviewed editable payload uses `1`; its encrypted companion uses `0`.
/// Neither value is promoted to lifecycle truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Form2550QXmlFinalFlag {
    Zero,
    #[default]
    One,
    Missing,
    Unknown(String),
}

impl Form2550QXmlFinalFlag {
    pub fn as_xml_value(&self) -> &str {
        match self {
            Self::Zero => "0",
            Self::One => "1",
            Self::Missing => "",
            Self::Unknown(value) => value,
        }
    }

    pub fn requires_review(&self) -> bool {
        matches!(self, Self::Missing | Self::Unknown(_))
    }
}

/// A checked Gregorian date with the editable-save formatting used by 2550Q.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form2550QDate {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl Form2550QDate {
    pub fn new(year: u16, month: u8, day: u8) -> Result<Self, String> {
        let value = Self { year, month, day };
        value.validate()?;
        Ok(value)
    }

    pub fn parse_return_period(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        let date = ["%m/%d/%Y", "%-m/%d/%Y", "%-m/%-d/%Y"]
            .iter()
            .find_map(|format| chrono::NaiveDate::parse_from_str(trimmed, format).ok())
            .ok_or_else(|| "Date must use MM/DD/YYYY and be a real calendar date".to_string())?;
        Self::from_naive(date)
    }

    pub fn parse_filed_date(value: &str) -> Result<Self, String> {
        let date = chrono::NaiveDate::parse_from_str(value.trim(), "%Y/%m/%d")
            .map_err(|_| "Filed date must use YYYY/MM/DD".to_string())?;
        Self::from_naive(date)
    }

    fn from_naive(date: chrono::NaiveDate) -> Result<Self, String> {
        use chrono::Datelike;
        Ok(Self {
            year: u16::try_from(date.year())
                .map_err(|_| "Date year is outside the supported range".to_string())?,
            month: u8::try_from(date.month())
                .map_err(|_| "Date month is outside the supported range".to_string())?,
            day: u8::try_from(date.day())
                .map_err(|_| "Date day is outside the supported range".to_string())?,
        })
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

    pub fn as_naive(self) -> Option<chrono::NaiveDate> {
        chrono::NaiveDate::from_ymd_opt(
            i32::from(self.year),
            u32::from(self.month),
            u32::from(self.day),
        )
    }

    pub fn to_filed_date(self) -> String {
        format!("{:04}/{:02}/{:02}", self.year, self.month, self.day)
    }
}

impl fmt::Display for Form2550QDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The reviewed app save uses an unpadded month and a padded day.
        write!(formatter, "{}/{:02}/{:04}", self.month, self.day, self.year)
    }
}

fn deserialize_optional_date_compat<'de, D>(
    deserializer: D,
) -> Result<Option<Form2550QDate>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DateCompat {
        Typed(Form2550QDate),
        LegacyText(String),
    }

    match Option::<DateCompat>::deserialize(deserializer)? {
        None => Ok(None),
        Some(DateCompat::Typed(value)) => {
            value.validate().map_err(serde::de::Error::custom)?;
            Ok(Some(value))
        }
        Some(DateCompat::LegacyText(value)) if value.trim().is_empty() => Ok(None),
        Some(DateCompat::LegacyText(value)) => Form2550QDate::parse_filed_date(&value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

/// Persisted family identity for the seven logical 2550Q repeating groups.
///
/// This is editor/draft identity only. It deliberately does not select a v2
/// rule-set group ID or authorize executable rule behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Form2550QRowFamily {
    #[serde(rename = "schedule-1")]
    Schedule1,
    #[serde(rename = "schedule-3")]
    Schedule3,
    #[serde(rename = "schedule-4")]
    Schedule4,
    #[serde(rename = "item-19-additional")]
    Item19Additional,
    #[serde(rename = "item-42-additional")]
    Item42Additional,
    #[serde(rename = "item-47-additional")]
    Item47Additional,
    #[serde(rename = "item-56-additional")]
    Item56Additional,
}

impl Form2550QRowFamily {
    pub const ALL: [Self; 7] = [
        Self::Schedule1,
        Self::Schedule3,
        Self::Schedule4,
        Self::Item19Additional,
        Self::Item42Additional,
        Self::Item47Additional,
        Self::Item56Additional,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Schedule1 => "Schedule 1",
            Self::Schedule3 => "Schedule 3",
            Self::Schedule4 => "Schedule 4",
            Self::Item19Additional => "Item 19 additional rows",
            Self::Item42Additional => "Item 42 additional rows",
            Self::Item47Additional => "Item 47 additional rows",
            Self::Item56Additional => "Item 56 additional rows",
        }
    }
}

/// Allocation cursor for stable repeated-row IDs.
///
/// IDs are fixed-width and monotonically increasing so lexical runtime order
/// matches creation order. Assigned IDs are never inferred from a transient
/// vector index after the one-time legacy seeding pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form2550QRowIdentityState {
    #[serde(default = "default_next_repeated_row_ordinal")]
    next_ordinal: u64,
}

impl Default for Form2550QRowIdentityState {
    fn default() -> Self {
        Self {
            next_ordinal: FIRST_REPEATED_ROW_ORDINAL,
        }
    }
}

impl Form2550QRowIdentityState {
    pub const fn next_ordinal(&self) -> u64 {
        self.next_ordinal
    }
}

/// Raw editor buffers persisted independently from parsed/typed draft values.
///
/// Missing map entries mean the adapter has not captured that buffer. An
/// explicit `RawValue::Absent` is a captured absence, while
/// `RawValue::Text("")` is a materialized blank control. Malformed text is
/// retained verbatim and its parse error is recorded separately so checked
/// XML paths cannot use a stale typed value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form2550QRawEditorState {
    #[serde(default = "default_raw_editor_state_version")]
    version: u8,
    #[serde(default)]
    singleton_fields: BTreeMap<String, RawValue>,
    #[serde(default)]
    repeated_fields:
        BTreeMap<Form2550QRowFamily, BTreeMap<StableInstanceId, BTreeMap<String, RawValue>>>,
    #[serde(default)]
    malformed_fields: BTreeMap<String, String>,
}

impl Default for Form2550QRawEditorState {
    fn default() -> Self {
        Self {
            version: default_raw_editor_state_version(),
            singleton_fields: BTreeMap::new(),
            repeated_fields: Form2550QRowFamily::ALL
                .into_iter()
                .map(|family| (family, BTreeMap::new()))
                .collect(),
            malformed_fields: BTreeMap::new(),
        }
    }
}

impl Form2550QRawEditorState {
    pub const fn version(&self) -> u8 {
        self.version
    }

    pub fn singleton_fields(&self) -> &BTreeMap<String, RawValue> {
        &self.singleton_fields
    }

    pub fn repeated_fields(
        &self,
    ) -> &BTreeMap<Form2550QRowFamily, BTreeMap<StableInstanceId, BTreeMap<String, RawValue>>> {
        &self.repeated_fields
    }

    pub fn malformed_fields(&self) -> &BTreeMap<String, String> {
        &self.malformed_fields
    }

    pub fn singleton_value(&self, field_key: &str) -> Option<&RawValue> {
        self.singleton_fields.get(field_key)
    }

    pub fn repeated_value(
        &self,
        family: Form2550QRowFamily,
        instance_id: &StableInstanceId,
        field_key: &str,
    ) -> Option<&RawValue> {
        self.repeated_fields
            .get(&family)?
            .get(instance_id)?
            .get(field_key)
    }

    pub fn set_singleton(&mut self, field_key: impl Into<String>, value: RawValue) {
        self.singleton_fields.insert(field_key.into(), value);
    }

    pub fn set_repeated(
        &mut self,
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
        field_key: impl Into<String>,
        value: RawValue,
    ) {
        self.repeated_fields
            .entry(family)
            .or_default()
            .entry(instance_id)
            .or_default()
            .insert(field_key.into(), value);
    }

    /// Forget a removed row's buffers and malformed notes, so a row taken
    /// out of Schedule 1, 3 or 4 leaves no orphan raw state behind.
    pub fn remove_repeated_row(
        &mut self,
        family: Form2550QRowFamily,
        instance_id: &StableInstanceId,
    ) {
        if let Some(instances) = self.repeated_fields.get_mut(&family) {
            instances.remove(instance_id);
        }
        let marker = format!("/{instance_id}/");
        self.malformed_fields
            .retain(|path, _| !path.contains(&marker));
    }

    pub(crate) fn mark_malformed(
        &mut self,
        stable_path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.malformed_fields
            .insert(stable_path.into(), message.into());
    }

    pub(crate) fn clear_malformed(&mut self, stable_path: &str) {
        self.malformed_fields.remove(stable_path);
    }

    pub fn clear_all_malformed(&mut self) {
        self.malformed_fields.clear();
    }

    pub fn has_malformed_fields(&self) -> bool {
        !self.malformed_fields.is_empty()
    }

    pub fn validate_version(&self) -> Result<(), Form2550QRawEditorStateError> {
        if self.version == default_raw_editor_state_version() {
            Ok(())
        } else {
            Err(Form2550QRawEditorStateError::UnsupportedVersion {
                actual: self.version,
            })
        }
    }

    fn ensure_group_slots(&mut self) {
        for family in Form2550QRowFamily::ALL {
            self.repeated_fields.entry(family).or_default();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form2550QRawEditorStateError {
    UnsupportedVersion { actual: u8 },
}

impl fmt::Display for Form2550QRawEditorStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { actual } => write!(
                formatter,
                "unsupported 2550Q raw editor state version {actual}; only version 1 is understood"
            ),
        }
    }
}

impl std::error::Error for Form2550QRawEditorStateError {}

/// Why persisted repeated-row identity could not be made safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form2550QRowIdentityError {
    Missing {
        family: Form2550QRowFamily,
    },
    Duplicate {
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
    },
    UnsupportedFormat {
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
    },
    Exhausted,
}

impl fmt::Display for Form2550QRowIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { family } => {
                write!(
                    formatter,
                    "{} contains a row without stable identity",
                    family.label()
                )
            }
            Self::Duplicate {
                family,
                instance_id,
            } => write!(
                formatter,
                "{} contains duplicate stable row ID {instance_id}",
                family.label()
            ),
            Self::UnsupportedFormat {
                family,
                instance_id,
            } => write!(
                formatter,
                "{} contains unsupported stable row ID {instance_id}; expected row- followed by exactly 20 digits with ordinal at least 1",
                family.label()
            ),
            Self::Exhausted => formatter.write_str("2550Q stable row ID allocator is exhausted"),
        }
    }
}

impl std::error::Error for Form2550QRowIdentityError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form2550QLegacyDraftMigrationError {
    IncompleteLegacyShape { missing_fields: Vec<&'static str> },
    AmbiguousHybridShape,
    InvalidQuarter { raw_value: String },
    RowIdentity(Form2550QRowIdentityError),
}

impl fmt::Display for Form2550QLegacyDraftMigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompleteLegacyShape { missing_fields } => write!(
                formatter,
                "flattened month is present without the complete scaffold-era 2550Q fingerprint; missing {}",
                missing_fields.join(", ")
            ),
            Self::AmbiguousHybridShape => formatter.write_str(
                "draft contains both scaffold-era flat fields and meaningful current 2550Q state",
            ),
            Self::InvalidQuarter { raw_value } => write!(
                formatter,
                "scaffold-era month must identify quarter 1 through 4, got {raw_value}"
            ),
            Self::RowIdentity(error) => write!(
                formatter,
                "scaffold-era repeated rows could not receive safe identities: {error}"
            ),
        }
    }
}

impl std::error::Error for Form2550QLegacyDraftMigrationError {}

impl From<Form2550QRowIdentityError> for Form2550QLegacyDraftMigrationError {
    fn from(error: Form2550QRowIdentityError) -> Self {
        Self::RowIdentity(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form2550QDraftShape {
    Current,
    LegacyFlat,
}

/// One of the two fixed Schedule 1 source rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Form2550QCapitalGoodRow {
    #[serde(default)]
    pub instance_id: Option<StableInstanceId>,
    pub purchase_or_import_date: Option<Form2550QDate>,
    /// Official source code: `D` for domestic purchase or `I` for importation.
    pub source_code: String,
    pub description: String,
    pub purchase_or_import_amount: Option<f64>,
    pub input_tax: Option<f64>,
    pub estimated_life_months: Option<u16>,
    pub recognized_life_months: Option<u16>,
    /// Preserved from the reviewed save. The available evidence does not expose
    /// the number-of-months-in-use input needed to derive this safely.
    pub allowable_input_tax_for_period: Option<f64>,
    pub balance_to_next_period: Option<f64>,
}

impl Form2550QCapitalGoodRow {
    pub fn is_empty(&self) -> bool {
        self.purchase_or_import_date.is_none()
            && self.source_code.trim().is_empty()
            && self.description.trim().is_empty()
            && self
                .purchase_or_import_amount
                .is_none_or(|value| value == 0.0)
            && self.input_tax.is_none_or(|value| value == 0.0)
            && self.estimated_life_months.is_none_or(|value| value == 0)
            && self.recognized_life_months.is_none_or(|value| value == 0)
            && self
                .allowable_input_tax_for_period
                .is_none_or(|value| value == 0.0)
            && self.balance_to_next_period.is_none_or(|value| value == 0.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Form2550QSchedule2 {
    pub input_tax_directly_attributable_to_exempt_sales: Option<f64>,
    pub vat_exempt_sales: Option<f64>,
    pub input_tax_not_directly_attributable: Option<f64>,
    pub total_sales: Option<f64>,
    pub ratable_input_tax: Option<f64>,
    pub total_input_tax_attributable_to_exempt_sales: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Form2550QCreditableVatRow {
    #[serde(default)]
    pub instance_id: Option<StableInstanceId>,
    pub period_from: Option<Form2550QDate>,
    pub period_to: Option<Form2550QDate>,
    pub withholding_agent_name: String,
    pub income_payment: Option<f64>,
    pub tax_withheld: Option<f64>,
}

impl Form2550QCreditableVatRow {
    pub fn is_empty(&self) -> bool {
        self.period_from.is_none()
            && self.period_to.is_none()
            && self.withholding_agent_name.trim().is_empty()
            && self.income_payment.is_none_or(|value| value == 0.0)
            && self.tax_withheld.is_none_or(|value| value == 0.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Form2550QAdvanceVatRow {
    #[serde(default)]
    pub instance_id: Option<StableInstanceId>,
    pub period_from: Option<Form2550QDate>,
    pub period_to: Option<Form2550QDate>,
    pub miller_name: String,
    pub taxpayer_name: String,
    /// Official receipt numbers are identifiers, never money.
    pub official_receipt_number: String,
    pub amount_paid: Option<f64>,
}

impl Form2550QAdvanceVatRow {
    pub fn is_empty(&self) -> bool {
        self.period_from.is_none()
            && self.period_to.is_none()
            && self.miller_name.trim().is_empty()
            && self.taxpayer_name.trim().is_empty()
            && matches!(self.official_receipt_number.trim(), "" | "0.00")
            && self.amount_paid.is_none_or(|value| value == 0.0)
    }
}

/// One line of an item 19, 42, 47 or 56 "(Add more...)" popup
/// (`addTblSpecifyNo19/42/47/56`): rows 19.1, 19.2, ... with a description
/// and an amount. When an item has popup rows, its static line shows
/// `VARIOUS` (more than one row) or the single row's description, and the
/// rows' total, as `closeModalNo19/42/47/56` sets them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Form2550QAdditionalItemRow {
    #[serde(default)]
    pub instance_id: Option<StableInstanceId>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub amount: Option<f64>,
}

trait Form2550QRepeatedRow {
    fn instance_id(&self) -> Option<&StableInstanceId>;
    fn instance_id_mut(&mut self) -> &mut Option<StableInstanceId>;
}

macro_rules! impl_repeated_row {
    ($row:ty) => {
        impl Form2550QRepeatedRow for $row {
            fn instance_id(&self) -> Option<&StableInstanceId> {
                self.instance_id.as_ref()
            }

            fn instance_id_mut(&mut self) -> &mut Option<StableInstanceId> {
                &mut self.instance_id
            }
        }
    };
}

impl_repeated_row!(Form2550QCapitalGoodRow);
impl_repeated_row!(Form2550QCreditableVatRow);
impl_repeated_row!(Form2550QAdvanceVatRow);
impl_repeated_row!(Form2550QAdditionalItemRow);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Form2550QPartIV {
    pub item_31a_vatable_sales: Option<f64>,
    pub item_31b_output_tax: Option<f64>,
    pub item_32a_zero_rated_sales: Option<f64>,
    pub item_33a_exempt_sales: Option<f64>,
    pub item_34a_total_sales: Option<f64>,
    pub item_34b_output_tax_due: Option<f64>,
    pub item_35b_less_output_vat_uncollected: Option<f64>,
    pub item_36b_add_output_vat_recovered: Option<f64>,
    pub item_37b_adjusted_output_tax_due: Option<f64>,
    pub item_38b_input_tax_carried: Option<f64>,
    pub item_39b_input_tax_deferred: Option<f64>,
    pub item_40b_transitional_input_tax: Option<f64>,
    pub item_41b_presumptive_input_tax: Option<f64>,
    pub item_42_description: String,
    pub item_42b_other_input_tax: Option<f64>,
    pub item_43b_total_prior_input_tax: Option<f64>,
    pub item_44a_domestic_purchases: Option<f64>,
    pub item_44b_domestic_input_tax: Option<f64>,
    pub item_45a_nonresident_services: Option<f64>,
    pub item_45b_nonresident_service_input_tax: Option<f64>,
    pub item_46a_importations: Option<f64>,
    pub item_46b_import_input_tax: Option<f64>,
    pub item_47_description: String,
    pub item_47a_other_purchases: Option<f64>,
    pub item_47b_other_input_tax: Option<f64>,
    pub item_48a_domestic_purchases_no_input_tax: Option<f64>,
    pub item_49a_vat_exempt_importations: Option<f64>,
    pub item_50a_total_current_purchases: Option<f64>,
    pub item_50b_total_current_input_tax: Option<f64>,
    pub item_51b_total_available_input_tax: Option<f64>,
    pub item_52b_deferred_capital_goods_input_tax: Option<f64>,
    pub item_53b_input_tax_attributable_to_exempt_sales: Option<f64>,
    pub item_54b_vat_refund_or_tcc_claimed: Option<f64>,
    pub item_55b_input_vat_on_unpaid_payables: Option<f64>,
    pub item_56_description: String,
    pub item_56b_other_deduction: Option<f64>,
    pub item_57b_total_deductions: Option<f64>,
    pub item_58b_input_vat_on_settled_payables: Option<f64>,
    pub item_59b_adjusted_deductions: Option<f64>,
    pub item_60b_total_allowable_input_tax: Option<f64>,
    pub item_61b_net_vat_payable_or_excess: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Form2550QPartII {
    pub item_15_net_vat_payable_or_excess: Option<f64>,
    pub item_16_creditable_vat_withheld: Option<f64>,
    pub item_17_advance_vat_payments: Option<f64>,
    pub item_18_paid_on_previous_return: Option<f64>,
    pub item_19_description: String,
    pub item_19_other_credit_or_payment: Option<f64>,
    pub item_20_total_credits_or_payments: Option<f64>,
    pub item_21_tax_payable_or_excess_credits: Option<f64>,
    pub item_22_surcharge: Option<f64>,
    pub item_23_interest: Option<f64>,
    pub item_24_compromise: Option<f64>,
    pub item_25_total_penalties: Option<f64>,
    pub item_26_total_amount_payable_or_excess: Option<f64>,
}

/// Page-one fields not present in either reviewed XML payload. They are local
/// draft/HTML values and never expand the exact 160-field save contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Form2550QLocalPrintFields {
    pub taxpayer_or_authorized_representative: String,
    pub representative_title: String,
    pub non_individual_authorized_officer: String,
    pub tax_agent_accreditation_or_roll_number: String,
    pub tax_agent_date_of_issue: String,
    pub tax_agent_date_of_expiry: String,
    pub cash_or_bank_debit_advice_amount: Option<f64>,
    pub check_bank: String,
    pub check_number: String,
    pub check_date: String,
    pub check_amount: Option<f64>,
    pub tax_debit_memo_number: String,
    pub tax_debit_memo_date: String,
    pub tax_debit_memo_amount: Option<f64>,
    pub other_payment_description: String,
    pub other_payment_bank: String,
    pub other_payment_number: String,
    pub other_payment_date: String,
    pub other_payment_amount: Option<f64>,
    pub machine_validation_or_receipt_details: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2550QDraft {
    pub id: Option<i64>,

    // Filing period.
    pub tin: String,
    pub taxable_year: u16,
    #[serde(default)]
    pub filing_basis: Form2550QFilingBasis,
    #[serde(default = "default_year_end_month")]
    pub year_end_month: u8,
    #[serde(default)]
    pub quarter: Form2550QQuarter,
    #[serde(default)]
    pub return_period_from: Option<Form2550QDate>,
    #[serde(default)]
    pub return_period_to: Option<Form2550QDate>,
    pub is_amended: bool,
    #[serde(default)]
    pub is_short_period_return: bool,

    // Profile-prefilled values.
    pub rdo_code: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,
    #[serde(default)]
    pub taxpayer_classification: Option<Form2550QTaxpayerClassification>,
    #[serde(default)]
    pub is_availing_tax_relief: bool,
    #[serde(default)]
    pub tax_relief_details: String,

    #[serde(default)]
    pub part_ii: Form2550QPartII,
    #[serde(default)]
    pub part_iv: Form2550QPartIV,
    #[serde(default)]
    pub schedule_1: Vec<Form2550QCapitalGoodRow>,
    #[serde(default)]
    pub schedule_2: Form2550QSchedule2,
    #[serde(default)]
    pub schedule_3: Vec<Form2550QCreditableVatRow>,
    #[serde(default)]
    pub schedule_4: Vec<Form2550QAdvanceVatRow>,
    #[serde(default)]
    pub item_19_additional_rows: Vec<Form2550QAdditionalItemRow>,
    #[serde(default)]
    pub item_42_additional_rows: Vec<Form2550QAdditionalItemRow>,
    #[serde(default)]
    pub item_47_additional_rows: Vec<Form2550QAdditionalItemRow>,
    #[serde(default)]
    pub item_56_additional_rows: Vec<Form2550QAdditionalItemRow>,
    #[serde(default)]
    pub row_identity_state: Form2550QRowIdentityState,
    #[serde(default)]
    pub raw_editor_state: Form2550QRawEditorState,
    #[serde(default)]
    pub local_print_fields: Form2550QLocalPrintFields,

    // Editable-save metadata. Online credential fields are deliberately not
    // persisted because both reviewed sources contain them only as blanks.
    #[serde(default)]
    pub xml_final_flag: Form2550QXmlFinalFlag,
    #[serde(default)]
    pub xml_contact_email: String,
    #[serde(default, deserialize_with = "deserialize_optional_date_compat")]
    pub date_filed: Option<Form2550QDate>,
    #[serde(default)]
    pub preserved_unmodeled_xml_fields: BTreeMap<String, String>,
    #[serde(default)]
    pub migration_review_items: Vec<String>,

    /// Status, queue authorization, network claim and retry state, shared
    /// with every form on the generic submission path. Declared before the
    /// legacy catch-all so serde routes lifecycle keys here first.
    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,

    /// Unknown top-level JSON properties are retained for forward
    /// compatibility. A scaffold-era `month` is migrated only when the full
    /// legacy fingerprint is present and the current nested model is empty;
    /// all other flattened values remain preserved.
    #[serde(default, flatten, skip_serializing_if = "BTreeMap::is_empty")]
    pub legacy_flat_draft_fields: BTreeMap<String, serde_json::Value>,
}

impl Form2550QDraft {
    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let quarter = Form2550QQuarter::from_number(quarter);
        let (return_period_from, return_period_to) = default_calendar_period(year, quarter)
            .map_or((None, None), |(from, to)| (Some(from), Some(to)));
        let zero = Some(0.0);
        let mut draft = Self {
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            filing_basis: Form2550QFilingBasis::Calendar,
            year_end_month: 12,
            quarter,
            return_period_from,
            return_period_to,
            is_amended: false,
            is_short_period_return: false,
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            taxpayer_classification: profile.eopt_tier.as_ref().map(Into::into),
            is_availing_tax_relief: false,
            tax_relief_details: String::new(),
            part_ii: Form2550QPartII {
                item_18_paid_on_previous_return: zero,
                item_19_other_credit_or_payment: zero,
                item_22_surcharge: zero,
                item_23_interest: zero,
                item_24_compromise: zero,
                ..Form2550QPartII::default()
            },
            part_iv: Form2550QPartIV {
                item_31a_vatable_sales: zero,
                item_32a_zero_rated_sales: zero,
                item_33a_exempt_sales: zero,
                item_35b_less_output_vat_uncollected: zero,
                item_36b_add_output_vat_recovered: zero,
                item_38b_input_tax_carried: zero,
                item_40b_transitional_input_tax: zero,
                item_41b_presumptive_input_tax: zero,
                item_42b_other_input_tax: zero,
                item_44a_domestic_purchases: zero,
                item_44b_domestic_input_tax: zero,
                item_45a_nonresident_services: zero,
                item_45b_nonresident_service_input_tax: zero,
                item_46a_importations: zero,
                item_46b_import_input_tax: zero,
                item_47a_other_purchases: zero,
                item_47b_other_input_tax: zero,
                item_48a_domestic_purchases_no_input_tax: zero,
                item_49a_vat_exempt_importations: zero,
                item_54b_vat_refund_or_tcc_claimed: zero,
                item_55b_input_vat_on_unpaid_payables: zero,
                item_56b_other_deduction: zero,
                item_58b_input_vat_on_settled_payables: zero,
                ..Form2550QPartIV::default()
            },
            schedule_1: vec![zero_capital_good_row(), zero_capital_good_row()],
            schedule_2: Form2550QSchedule2 {
                input_tax_directly_attributable_to_exempt_sales: zero,
                vat_exempt_sales: zero,
                input_tax_not_directly_attributable: zero,
                ..Form2550QSchedule2::default()
            },
            schedule_3: vec![zero_creditable_vat_row(), zero_creditable_vat_row()],
            schedule_4: vec![zero_advance_vat_row(), zero_advance_vat_row()],
            item_19_additional_rows: Vec::new(),
            item_42_additional_rows: Vec::new(),
            item_47_additional_rows: Vec::new(),
            item_56_additional_rows: Vec::new(),
            row_identity_state: Form2550QRowIdentityState::default(),
            raw_editor_state: Form2550QRawEditorState::default(),
            local_print_fields: Form2550QLocalPrintFields::default(),
            xml_final_flag: Form2550QXmlFinalFlag::One,
            xml_contact_email: String::new(),
            date_filed: None,
            preserved_unmodeled_xml_fields: BTreeMap::new(),
            migration_review_items: Vec::new(),
            legacy_flat_draft_fields: BTreeMap::new(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft
            .ensure_repeating_row_ids()
            .expect("new 2550Q row identity allocation");
        draft.recompute();
        draft
    }

    /// Lazily assign stable identities to rows loaded from older JSON.
    ///
    /// Existing IDs are validated and retained. Missing IDs are assigned once
    /// in the draft's current visible order using a persisted monotonic
    /// allocator. The method never derives an ID from the vector index after
    /// that initial migration.
    pub fn ensure_repeating_row_ids(&mut self) -> Result<bool, Form2550QRowIdentityError> {
        let mut next_ordinal = self
            .row_identity_state
            .next_ordinal
            .max(FIRST_REPEATED_ROW_ORDINAL);

        observe_row_ids(
            Form2550QRowFamily::Schedule1,
            &self.schedule_1,
            &mut next_ordinal,
        )?;
        observe_row_ids(
            Form2550QRowFamily::Schedule3,
            &self.schedule_3,
            &mut next_ordinal,
        )?;
        observe_row_ids(
            Form2550QRowFamily::Schedule4,
            &self.schedule_4,
            &mut next_ordinal,
        )?;
        observe_row_ids(
            Form2550QRowFamily::Item19Additional,
            &self.item_19_additional_rows,
            &mut next_ordinal,
        )?;
        observe_row_ids(
            Form2550QRowFamily::Item42Additional,
            &self.item_42_additional_rows,
            &mut next_ordinal,
        )?;
        observe_row_ids(
            Form2550QRowFamily::Item47Additional,
            &self.item_47_additional_rows,
            &mut next_ordinal,
        )?;
        observe_row_ids(
            Form2550QRowFamily::Item56Additional,
            &self.item_56_additional_rows,
            &mut next_ordinal,
        )?;

        let mut changed = false;
        changed |= assign_missing_row_ids(&mut self.schedule_1, &mut next_ordinal)?;
        changed |= assign_missing_row_ids(&mut self.schedule_3, &mut next_ordinal)?;
        changed |= assign_missing_row_ids(&mut self.schedule_4, &mut next_ordinal)?;
        changed |= assign_missing_row_ids(&mut self.item_19_additional_rows, &mut next_ordinal)?;
        changed |= assign_missing_row_ids(&mut self.item_42_additional_rows, &mut next_ordinal)?;
        changed |= assign_missing_row_ids(&mut self.item_47_additional_rows, &mut next_ordinal)?;
        changed |= assign_missing_row_ids(&mut self.item_56_additional_rows, &mut next_ordinal)?;

        if self.row_identity_state.next_ordinal != next_ordinal {
            self.row_identity_state.next_ordinal = next_ordinal;
            changed = true;
        }
        self.raw_editor_state.ensure_group_slots();
        Ok(changed)
    }

    /// Validate that every materialized row has a unique stable ID within its
    /// family. Malformed IDs fail earlier during serde deserialization.
    pub fn validate_repeating_row_ids(&self) -> Result<(), Form2550QRowIdentityError> {
        validate_complete_row_ids(Form2550QRowFamily::Schedule1, &self.schedule_1)?;
        validate_complete_row_ids(Form2550QRowFamily::Schedule3, &self.schedule_3)?;
        validate_complete_row_ids(Form2550QRowFamily::Schedule4, &self.schedule_4)?;
        validate_complete_row_ids(
            Form2550QRowFamily::Item19Additional,
            &self.item_19_additional_rows,
        )?;
        validate_complete_row_ids(
            Form2550QRowFamily::Item42Additional,
            &self.item_42_additional_rows,
        )?;
        validate_complete_row_ids(
            Form2550QRowFamily::Item47Additional,
            &self.item_47_additional_rows,
        )?;
        validate_complete_row_ids(
            Form2550QRowFamily::Item56Additional,
            &self.item_56_additional_rows,
        )?;
        Ok(())
    }

    fn legacy_flat_draft_shape(
        &self,
    ) -> Result<Form2550QDraftShape, Form2550QLegacyDraftMigrationError> {
        if !self.legacy_flat_draft_fields.contains_key("month") {
            return Ok(Form2550QDraftShape::Current);
        }

        let missing_fields = LEGACY_FLAT_SHAPE_ANCHORS
            .into_iter()
            .filter(|field| !self.legacy_flat_draft_fields.contains_key(*field))
            .collect::<Vec<_>>();
        if !missing_fields.is_empty() {
            return Err(Form2550QLegacyDraftMigrationError::IncompleteLegacyShape {
                missing_fields,
            });
        }
        if self.has_meaningful_current_shape_state() {
            return Err(Form2550QLegacyDraftMigrationError::AmbiguousHybridShape);
        }
        Ok(Form2550QDraftShape::LegacyFlat)
    }

    fn has_meaningful_current_shape_state(&self) -> bool {
        self.filing_basis != Form2550QFilingBasis::Calendar
            || self.year_end_month != default_year_end_month()
            || self.quarter != Form2550QQuarter::Unresolved(0)
            || self.return_period_from.is_some()
            || self.return_period_to.is_some()
            || self.is_short_period_return
            || self.taxpayer_classification.is_some()
            || self.is_availing_tax_relief
            || !self.tax_relief_details.is_empty()
            || self.part_ii != Form2550QPartII::default()
            || self.part_iv != Form2550QPartIV::default()
            || !self.schedule_1.is_empty()
            || self.schedule_2 != Form2550QSchedule2::default()
            || !self.schedule_3.is_empty()
            || !self.schedule_4.is_empty()
            || !self.item_19_additional_rows.is_empty()
            || !self.item_42_additional_rows.is_empty()
            || !self.item_47_additional_rows.is_empty()
            || !self.item_56_additional_rows.is_empty()
            || self.row_identity_state != Form2550QRowIdentityState::default()
            || self.raw_editor_state != Form2550QRawEditorState::default()
            || self.local_print_fields != Form2550QLocalPrintFields::default()
            || self.xml_final_flag != Form2550QXmlFinalFlag::One
            || !self.xml_contact_email.is_empty()
            || !self.preserved_unmodeled_xml_fields.is_empty()
            || !self.migration_review_items.is_empty()
    }

    /// Upgrade the old scaffold-only flat JSON shape into the reviewed
    /// semantic model. Migration is transactional: an incomplete fingerprint,
    /// a hybrid carrying meaningful current state, an invalid quarter, or
    /// unsafe row identity leaves the original draft byte-for-byte equivalent.
    ///
    /// Only the migrated `month` discriminator is removed. Other flattened
    /// fields remain persisted so unmodeled scaffold data and future extension
    /// data are never silently discarded.
    pub fn migrate_legacy_flat_draft(
        &mut self,
    ) -> Result<bool, Form2550QLegacyDraftMigrationError> {
        if self.legacy_flat_draft_shape()? == Form2550QDraftShape::Current {
            return Ok(false);
        }

        let legacy = self.legacy_flat_draft_fields.clone();
        let quarter = legacy_u8(&legacy, "month")
            .filter(|quarter| (1..=4).contains(quarter))
            .ok_or_else(|| Form2550QLegacyDraftMigrationError::InvalidQuarter {
                raw_value: legacy
                    .get("month")
                    .map_or_else(|| "<missing>".to_string(), serde_json::Value::to_string),
            })?;
        let mut migrated = self.clone();
        migrated.legacy_flat_draft_fields.remove("month");
        migrated.apply_legacy_flat_draft(&legacy, quarter)?;
        *self = migrated;
        Ok(true)
    }

    fn apply_legacy_flat_draft(
        &mut self,
        legacy: &BTreeMap<String, serde_json::Value>,
        quarter: u8,
    ) -> Result<(), Form2550QLegacyDraftMigrationError> {
        self.migration_review_items.clear();
        self.filing_basis = if legacy_bool(legacy, "fiscal_no1").unwrap_or(false) {
            Form2550QFilingBasis::Fiscal
        } else {
            Form2550QFilingBasis::Calendar
        };
        self.year_end_month = 12;
        self.quarter = Form2550QQuarter::from_number(quarter);
        let default_period = default_calendar_period(self.taxable_year, self.quarter);
        self.return_period_from = legacy_return_date(legacy, "rtn_period_from_no4")
            .or_else(|| default_period.map(|(from, _)| from));
        self.return_period_to = legacy_return_date(legacy, "rtn_period_to_no4")
            .or_else(|| default_period.map(|(_, to)| to));
        self.is_short_period_return = legacy_bool(legacy, "opt_short_prd1").unwrap_or(false);
        self.taxpayer_classification = [
            (
                "tax_payer_classification1",
                Form2550QTaxpayerClassification::Micro,
            ),
            (
                "tax_payer_classification2",
                Form2550QTaxpayerClassification::Small,
            ),
            (
                "tax_payer_classification3",
                Form2550QTaxpayerClassification::Medium,
            ),
            (
                "tax_payer_classification4",
                Form2550QTaxpayerClassification::Large,
            ),
        ]
        .into_iter()
        .find_map(|(key, classification)| {
            legacy_bool(legacy, key)
                .unwrap_or(false)
                .then_some(classification)
        });
        self.is_availing_tax_relief =
            legacy_bool(legacy, "international_treaty_yn").unwrap_or(false);
        self.tax_relief_details = legacy_text(legacy, "specify_international_treaty");

        self.part_ii = Form2550QPartII {
            item_18_paid_on_previous_return: legacy_money(legacy, "vat_paid_return"),
            item_19_description: legacy_text(legacy, "add_specify_no19"),
            item_19_other_credit_or_payment: legacy_money(legacy, "other_credits_no19"),
            item_22_surcharge: legacy_money(legacy, "surcharge"),
            item_23_interest: legacy_money(legacy, "interest"),
            item_24_compromise: legacy_money(legacy, "compromise"),
            ..Form2550QPartII::default()
        };
        self.part_iv = Form2550QPartIV {
            item_31a_vatable_sales: legacy_money(legacy, "vatable_sales"),
            item_32a_zero_rated_sales: legacy_money(legacy, "zero_rated_sales"),
            item_33a_exempt_sales: legacy_money(legacy, "exempt_sales"),
            item_35b_less_output_vat_uncollected: legacy_money(legacy, "less_output_vat"),
            item_36b_add_output_vat_recovered: legacy_money(legacy, "add_output_vat"),
            item_38b_input_tax_carried: legacy_money(legacy, "input_tax_carried"),
            item_40b_transitional_input_tax: legacy_money(legacy, "transitional_input_tax"),
            item_41b_presumptive_input_tax: legacy_money(legacy, "presumptive_input_tax"),
            item_42_description: legacy_text(legacy, "add_specify_no42"),
            item_42b_other_input_tax: legacy_money(legacy, "other_specify42"),
            item_44a_domestic_purchases: legacy_money(legacy, "domestic_purchase"),
            item_44b_domestic_input_tax: legacy_money(legacy, "domestic_input_tax"),
            item_45a_nonresident_services: legacy_money(legacy, "services_purchase"),
            item_45b_nonresident_service_input_tax: legacy_money(legacy, "service_input_tax"),
            item_46a_importations: legacy_money(legacy, "import_purchase"),
            item_46b_import_input_tax: legacy_money(legacy, "import_input_tax"),
            item_47_description: legacy_text(legacy, "add_specify_no47"),
            item_47a_other_purchases: legacy_money(legacy, "other_specify47"),
            item_47b_other_input_tax: legacy_money(legacy, "other_specify47b"),
            item_48a_domestic_purchases_no_input_tax: legacy_money(
                legacy,
                "domestic_purchase_no_tax",
            ),
            item_49a_vat_exempt_importations: legacy_money(legacy, "vat_exempt_imports"),
            item_54b_vat_refund_or_tcc_claimed: legacy_money(legacy, "vat_refund"),
            item_55b_input_vat_on_unpaid_payables: legacy_money(legacy, "input_vat_unpaid"),
            item_56_description: legacy_text(legacy, "add_specify_no56"),
            item_56b_other_deduction: legacy_money(legacy, "other_specify56"),
            item_58b_input_vat_on_settled_payables: legacy_money(legacy, "add_input_vat"),
            ..Form2550QPartIV::default()
        };
        self.schedule_1 = [10, 11]
            .into_iter()
            .map(|suffix| legacy_capital_good_row(legacy, suffix))
            .collect();
        self.schedule_2 = Form2550QSchedule2 {
            input_tax_directly_attributable_to_exempt_sales: legacy_money(
                legacy,
                "sched2input_tax_direct",
            ),
            vat_exempt_sales: legacy_money(legacy, "sched2vat_exempt_sale"),
            input_tax_not_directly_attributable: legacy_money(legacy, "sched2amount_input_tax"),
            ..Form2550QSchedule2::default()
        };
        self.schedule_3 = [30, 31]
            .into_iter()
            .map(|suffix| legacy_creditable_vat_row(legacy, suffix))
            .collect();
        self.schedule_4 = [40, 41]
            .into_iter()
            .map(|suffix| legacy_advance_vat_row(legacy, suffix))
            .collect();

        self.ensure_repeating_row_ids()?;
        self.recompute();
        let comparisons = [
            ("output_vat_sales", self.part_iv.item_31b_output_tax),
            ("total_sales", self.part_iv.item_34a_total_sales),
            ("output_tax_due", self.part_iv.item_34b_output_tax_due),
            (
                "total_adj_output",
                self.part_iv.item_37b_adjusted_output_tax_due,
            ),
            (
                "input_tax_deferred",
                self.part_iv.item_39b_input_tax_deferred,
            ),
            ("total43", self.part_iv.item_43b_total_prior_input_tax),
            (
                "total_cur_purchase",
                self.part_iv.item_50a_total_current_purchases,
            ),
            (
                "total_cur_input_tax",
                self.part_iv.item_50b_total_current_input_tax,
            ),
            (
                "total_avail_input_tax",
                self.part_iv.item_51b_total_available_input_tax,
            ),
            (
                "import_capital_input_tax",
                self.part_iv.item_52b_deferred_capital_goods_input_tax,
            ),
            (
                "input_tax_attr",
                self.part_iv.item_53b_input_tax_attributable_to_exempt_sales,
            ),
            ("total_deductions", self.part_iv.item_57b_total_deductions),
            ("adj_deductions", self.part_iv.item_59b_adjusted_deductions),
            (
                "total_allow_input_tax",
                self.part_iv.item_60b_total_allowable_input_tax,
            ),
            (
                "net_vat_payable",
                self.part_iv.item_61b_net_vat_payable_or_excess,
            ),
            (
                "excess_input_tax",
                self.part_ii.item_15_net_vat_payable_or_excess,
            ),
            (
                "creditable_vat",
                self.part_ii.item_16_creditable_vat_withheld,
            ),
            ("adv_vat_payment", self.part_ii.item_17_advance_vat_payments),
            (
                "total_tax_credits",
                self.part_ii.item_20_total_credits_or_payments,
            ),
            (
                "excess_credits",
                self.part_ii.item_21_tax_payable_or_excess_credits,
            ),
            ("penalties", self.part_ii.item_25_total_penalties),
            (
                "total_payable",
                self.part_ii.item_26_total_amount_payable_or_excess,
            ),
        ];
        for (legacy_key, recomputed) in comparisons {
            if let (Some(old), Some(new)) = (legacy_money(legacy, legacy_key), recomputed)
                && (old - new).abs() > 0.005
            {
                self.migration_review_items.push(format!(
                    "Legacy {legacy_key} was {old:.2}; the reviewed formula recomputes {new:.2}"
                ));
            }
        }
        if matches!(self.filing_basis, Form2550QFilingBasis::Fiscal) {
            self.migration_review_items.push(
                "The scaffold-era draft did not retain a reliable fiscal year-end month; review Item 2"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Recompute only formulas stated on the official April 2024 form or
    /// corroborated by the reviewed editable-save pair.
    pub fn recompute(&mut self) {
        match self.migrate_legacy_flat_draft() {
            Ok(false) => {}
            Ok(true) | Err(_) => return,
        }
        self.apply_specify_popup_rows();
        let p4 = &mut self.part_iv;
        p4.item_31b_output_tax = p4
            .item_31a_vatable_sales
            .map(|value| money(value * VAT_RATE));
        p4.item_34a_total_sales = sum_money(&[
            p4.item_31a_vatable_sales,
            p4.item_32a_zero_rated_sales,
            p4.item_33a_exempt_sales,
        ]);
        p4.item_34b_output_tax_due = p4.item_31b_output_tax;
        p4.item_37b_adjusted_output_tax_due = add_subtract_money(
            p4.item_34b_output_tax_due,
            p4.item_36b_add_output_vat_recovered,
            p4.item_35b_less_output_vat_uncollected,
        );

        // Official `computeSched1`: each row's allowable input tax is
        // `formatCurrency(input tax / recognized life * 3)` and its balance
        // `formatCurrency(input tax - allowable)`, for purchase dates on or
        // before December 31, 2021.
        for row in &mut self.schedule_1 {
            if row.is_empty() {
                continue;
            }
            let (Some(date), Some(input_tax), Some(life)) = (
                row.purchase_or_import_date,
                row.input_tax,
                row.recognized_life_months,
            ) else {
                continue;
            };
            if life == 0 || is_after_schedule_1_cutoff(date) {
                continue;
            }
            let allowable = format_currency_cents(input_tax / f64::from(life) * 3.0);
            row.allowable_input_tax_for_period = Some(allowable);
            row.balance_to_next_period = Some(format_currency_cents(input_tax - allowable));
        }
        let p4 = &mut self.part_iv;

        let schedule_1_previous = sum_money(
            &self
                .schedule_1
                .iter()
                .map(|row| row.input_tax)
                .collect::<Vec<_>>(),
        );
        let schedule_1_next = sum_money(
            &self
                .schedule_1
                .iter()
                .map(|row| row.balance_to_next_period)
                .collect::<Vec<_>>(),
        );
        p4.item_39b_input_tax_deferred = schedule_1_previous;
        p4.item_43b_total_prior_input_tax = sum_money(&[
            p4.item_38b_input_tax_carried,
            p4.item_39b_input_tax_deferred,
            p4.item_40b_transitional_input_tax,
            p4.item_41b_presumptive_input_tax,
            p4.item_42b_other_input_tax,
        ]);

        // Official compute44AB/45AB/46AB: the disabled Part IV input-tax
        // controls are always 12% of their purchase amounts.
        p4.item_44b_domestic_input_tax = p4
            .item_44a_domestic_purchases
            .map(|value| money(value * VAT_RATE));
        p4.item_45b_nonresident_service_input_tax = p4
            .item_45a_nonresident_services
            .map(|value| money(value * VAT_RATE));
        p4.item_46b_import_input_tax = p4
            .item_46a_importations
            .map(|value| money(value * VAT_RATE));

        p4.item_50a_total_current_purchases = sum_money(&[
            p4.item_44a_domestic_purchases,
            p4.item_45a_nonresident_services,
            p4.item_46a_importations,
            p4.item_47a_other_purchases,
            p4.item_48a_domestic_purchases_no_input_tax,
            p4.item_49a_vat_exempt_importations,
        ]);
        p4.item_50b_total_current_input_tax = sum_money(&[
            p4.item_44b_domestic_input_tax,
            p4.item_45b_nonresident_service_input_tax,
            p4.item_46b_import_input_tax,
            p4.item_47b_other_input_tax,
        ]);
        p4.item_51b_total_available_input_tax = sum_money(&[
            p4.item_43b_total_prior_input_tax,
            p4.item_50b_total_current_input_tax,
        ]);
        p4.item_52b_deferred_capital_goods_input_tax = schedule_1_next;

        // Official computeVATExemptSale/computeTotalSales copy Part IV.
        self.schedule_2.vat_exempt_sales = p4.item_33a_exempt_sales;
        self.schedule_2.total_sales = p4.item_34a_total_sales;
        self.schedule_2.ratable_input_tax = compute_ratable_input_tax(&self.schedule_2);
        self.schedule_2.total_input_tax_attributable_to_exempt_sales = sum_money(&[
            self.schedule_2
                .input_tax_directly_attributable_to_exempt_sales,
            self.schedule_2.ratable_input_tax,
        ]);
        p4.item_53b_input_tax_attributable_to_exempt_sales =
            self.schedule_2.total_input_tax_attributable_to_exempt_sales;

        p4.item_57b_total_deductions = sum_money(&[
            p4.item_52b_deferred_capital_goods_input_tax,
            p4.item_53b_input_tax_attributable_to_exempt_sales,
            p4.item_54b_vat_refund_or_tcc_claimed,
            p4.item_55b_input_vat_on_unpaid_payables,
            p4.item_56b_other_deduction,
        ]);
        p4.item_59b_adjusted_deductions = sum_money(&[
            p4.item_57b_total_deductions,
            p4.item_58b_input_vat_on_settled_payables,
        ]);
        p4.item_60b_total_allowable_input_tax = subtract_money(
            p4.item_51b_total_available_input_tax,
            p4.item_59b_adjusted_deductions,
        );
        p4.item_61b_net_vat_payable_or_excess = subtract_money(
            p4.item_37b_adjusted_output_tax_due,
            p4.item_60b_total_allowable_input_tax,
        );

        let schedule_3_tax = sum_money(
            &self
                .schedule_3
                .iter()
                .map(|row| row.tax_withheld)
                .collect::<Vec<_>>(),
        );
        let schedule_4_amount = sum_money(
            &self
                .schedule_4
                .iter()
                .map(|row| row.amount_paid)
                .collect::<Vec<_>>(),
        );

        let p2 = &mut self.part_ii;
        p2.item_15_net_vat_payable_or_excess = p4.item_61b_net_vat_payable_or_excess;
        p2.item_16_creditable_vat_withheld = schedule_3_tax;
        p2.item_17_advance_vat_payments = schedule_4_amount;
        p2.item_20_total_credits_or_payments = sum_money(&[
            p2.item_16_creditable_vat_withheld,
            p2.item_17_advance_vat_payments,
            p2.item_18_paid_on_previous_return,
            p2.item_19_other_credit_or_payment,
        ]);
        p2.item_21_tax_payable_or_excess_credits = subtract_money(
            p2.item_15_net_vat_payable_or_excess,
            p2.item_20_total_credits_or_payments,
        );
        p2.item_25_total_penalties = sum_money(&[
            p2.item_22_surcharge,
            p2.item_23_interest,
            p2.item_24_compromise,
        ]);
        // The form labels Item 26 as payable/(excess credits). The reviewed
        // save corroborates that negative excess credits do not offset cash
        // penalties: Item 21 = -2,440 and Item 25 = 1,300 produce Item 26 =
        // 1,300. Preserve the negative credit in Item 21 and add only a
        // positive payable component to penalties.
        p2.item_26_total_amount_payable_or_excess = match (
            p2.item_21_tax_payable_or_excess_credits,
            p2.item_25_total_penalties,
        ) {
            (Some(payable_or_credit), Some(penalties)) => {
                Some(money(payable_or_credit.max(0.0) + penalties))
            }
            _ => None,
        };

        self.lifecycle.updated_at = chrono::Utc::now().to_rfc3339();
    }

    pub fn is_editable(&self) -> bool {
        self.lifecycle.is_editable()
    }

    pub fn quarter_number(&self) -> Option<u8> {
        self.quarter.number()
    }

    /// Draft -> Queued through the generic submission lifecycle: recompute,
    /// require the exact official upload payload, then freeze the reviewed
    /// field map behind a fingerprint and a scoped GUI authorization.
    pub fn transition_to_queued(&mut self) -> Result<(), Vec<(String, String)>> {
        super::queueable::QueueableForm::queue(self, QueueAuthSource::Gui)
    }

    pub fn revert_to_draft(&mut self) -> Result<(), String> {
        if matches!(self.lifecycle.status, FilingStatus::Paid) {
            return Err("A paid 2550Q cannot be reverted to draft".to_string());
        }
        self.lifecycle.revert_to_draft();
        Ok(())
    }

    pub(crate) fn schedule_1_previous_total(&self) -> Option<f64> {
        sum_money(
            &self
                .schedule_1
                .iter()
                .map(|row| row.input_tax)
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn schedule_1_next_total(&self) -> Option<f64> {
        sum_money(
            &self
                .schedule_1
                .iter()
                .map(|row| row.balance_to_next_period)
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn schedule_3_income_total(&self) -> Option<f64> {
        sum_money(
            &self
                .schedule_3
                .iter()
                .map(|row| row.income_payment)
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn schedule_3_tax_total(&self) -> Option<f64> {
        sum_money(
            &self
                .schedule_3
                .iter()
                .map(|row| row.tax_withheld)
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn schedule_4_amount_total(&self) -> Option<f64> {
        sum_money(
            &self
                .schedule_4
                .iter()
                .map(|row| row.amount_paid)
                .collect::<Vec<_>>(),
        )
    }
}

impl FormValidator for Form2550QDraft {
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        match self.legacy_flat_draft_shape() {
            Ok(Form2550QDraftShape::Current) => {}
            Ok(Form2550QDraftShape::LegacyFlat) => errors.push((
                "legacy_flat_draft_fields".to_string(),
                "Scaffold-era 2550Q fields require checked migration before use".to_string(),
            )),
            Err(error) => errors.push((
                "legacy_flat_draft_fields".to_string(),
                format!("Flattened 2550Q draft shape is unsafe: {error}"),
            )),
        }
        if let Err(error) = self.validate_repeating_row_ids() {
            errors.push((
                "repeated_row_identity".to_string(),
                format!("Repeated-row identity is invalid: {error}"),
            ));
        }
        for key in self.preserved_unmodeled_xml_fields.keys() {
            if !is_preserved_unmodeled_xml_key(key) {
                errors.push((
                    format!("preserved_unmodeled_xml_fields.{key}"),
                    format!("Unreviewed 2550Q XML field {key} cannot be preserved"),
                ));
            }
        }
        if let Err(error) = self.raw_editor_state.validate_version() {
            errors.push((
                "raw_editor_state.version".to_string(),
                format!("Raw editor state is invalid: {error}"),
            ));
        }
        if let Err(error) = validate_form_2550q_raw_bindings(self) {
            errors.push((
                "raw_editor_state.bindings".to_string(),
                format!("Raw editor bindings are invalid: {error}"),
            ));
        }
        for (field, message) in self.raw_editor_state.malformed_fields() {
            errors.push((
                format!("raw_editor.{field}"),
                format!("Visible editor text is malformed and has not been reconciled: {message}"),
            ));
        }
        errors.extend(self.raw_editor_coherence_errors());
        validate_specify_popup_rows(self, &mut errors);
        validate_identity(self, &mut errors);
        validate_period(self, &mut errors);

        if self.taxpayer_classification.is_none() {
            errors.push((
                "taxpayer_classification".to_string(),
                "Item 13 taxpayer classification is required".to_string(),
            ));
        }
        if self.is_availing_tax_relief && self.tax_relief_details.trim().is_empty() {
            errors.push((
                "tax_relief_details".to_string(),
                "Item 14A must identify the Special Law or International Tax Treaty".to_string(),
            ));
        }
        if self.xml_final_flag.requires_review() {
            errors.push((
                "xml_final_flag".to_string(),
                "Imported txtFinalFlag is outside the reviewed editable/encrypted values 0 and 1"
                    .to_string(),
            ));
        }
        for (index, message) in self.migration_review_items.iter().enumerate() {
            errors.push((format!("legacy_migration[{index}]"), message.clone()));
        }

        for (field, value) in user_money_fields(self) {
            validate_required_non_negative_money(field, value, &mut errors);
        }
        for (field, description, value) in [
            (
                "item_19_description",
                self.part_ii.item_19_description.as_str(),
                self.part_ii.item_19_other_credit_or_payment,
            ),
            (
                "item_42_description",
                self.part_iv.item_42_description.as_str(),
                self.part_iv.item_42b_other_input_tax,
            ),
            (
                "item_56_description",
                self.part_iv.item_56_description.as_str(),
                self.part_iv.item_56b_other_deduction,
            ),
        ] {
            if value.is_some_and(|amount| amount != 0.0) && description.trim().is_empty() {
                errors.push((
                    field.to_string(),
                    "An Others amount requires a description".to_string(),
                ));
            }
        }
        if (self
            .part_iv
            .item_47a_other_purchases
            .is_some_and(|amount| amount != 0.0)
            || self
                .part_iv
                .item_47b_other_input_tax
                .is_some_and(|amount| amount != 0.0))
            && self.part_iv.item_47_description.trim().is_empty()
        {
            errors.push((
                "item_47_description".to_string(),
                "An Item 47A or 47B amount requires an Others description".to_string(),
            ));
        }
        if !self.is_amended
            && self
                .part_ii
                .item_18_paid_on_previous_return
                .is_some_and(|value| value != 0.0)
        {
            errors.push((
                "item_18_paid_on_previous_return".to_string(),
                "Item 18 applies only to an amended return".to_string(),
            ));
        }

        validate_schedule_rows(self, &mut errors);
        validate_local_print_fields(&self.local_print_fields, &mut errors);
        if self.schedule_2.total_sales == Some(0.0)
            && self
                .schedule_2
                .vat_exempt_sales
                .is_some_and(|value| value != 0.0)
            && self
                .schedule_2
                .input_tax_not_directly_attributable
                .is_some_and(|value| value != 0.0)
        {
            errors.push((
                "schedule_2.total_sales".to_string(),
                "Schedule 2 cannot allocate ratable input tax when total sales is zero".to_string(),
            ));
        }

        for (field, value) in computed_money_fields(self) {
            if value.is_none() {
                errors.push((
                    field.to_string(),
                    "Computed amount is unresolved because an input is blank or invalid"
                        .to_string(),
                ));
            } else if value.is_some_and(|amount| !amount.is_finite()) {
                errors.push((
                    field.to_string(),
                    "Computed amount is not finite".to_string(),
                ));
            }
        }
        errors
    }
}

fn observe_row_ids<T: Form2550QRepeatedRow>(
    family: Form2550QRowFamily,
    rows: &[T],
    next_ordinal: &mut u64,
) -> Result<(), Form2550QRowIdentityError> {
    let mut seen = BTreeSet::new();
    for row in rows {
        let Some(instance_id) = row.instance_id() else {
            continue;
        };
        if !seen.insert(instance_id.clone()) {
            return Err(Form2550QRowIdentityError::Duplicate {
                family,
                instance_id: instance_id.clone(),
            });
        }
        let Some(ordinal) = persisted_row_ordinal(instance_id) else {
            return Err(Form2550QRowIdentityError::UnsupportedFormat {
                family,
                instance_id: instance_id.clone(),
            });
        };
        let candidate = ordinal
            .checked_add(1)
            .ok_or(Form2550QRowIdentityError::Exhausted)?;
        *next_ordinal = (*next_ordinal).max(candidate);
    }
    Ok(())
}

fn validate_complete_row_ids<T: Form2550QRepeatedRow>(
    family: Form2550QRowFamily,
    rows: &[T],
) -> Result<(), Form2550QRowIdentityError> {
    for row in rows {
        if row.instance_id().is_none() {
            return Err(Form2550QRowIdentityError::Missing { family });
        }
    }
    let mut ignored_next = FIRST_REPEATED_ROW_ORDINAL;
    observe_row_ids(family, rows, &mut ignored_next)
}

fn assign_missing_row_ids<T: Form2550QRepeatedRow>(
    rows: &mut [T],
    next_ordinal: &mut u64,
) -> Result<bool, Form2550QRowIdentityError> {
    let mut changed = false;
    for row in rows {
        if row.instance_id().is_some() {
            continue;
        }
        let ordinal = *next_ordinal;
        *next_ordinal = (*next_ordinal)
            .checked_add(1)
            .ok_or(Form2550QRowIdentityError::Exhausted)?;
        let instance_id = StableInstanceId::parse(format!("row-{ordinal:020}"))
            .expect("fixed-width row ID is valid");
        *row.instance_id_mut() = Some(instance_id);
        changed = true;
    }
    Ok(changed)
}

fn persisted_row_ordinal(instance_id: &StableInstanceId) -> Option<u64> {
    let digits = instance_id.as_str().strip_prefix("row-")?;
    if digits.len() != 20 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|ordinal| *ordinal >= 1)
}

fn zero_capital_good_row() -> Form2550QCapitalGoodRow {
    Form2550QCapitalGoodRow {
        purchase_or_import_amount: Some(0.0),
        input_tax: Some(0.0),
        estimated_life_months: Some(0),
        recognized_life_months: Some(0),
        allowable_input_tax_for_period: Some(0.0),
        balance_to_next_period: Some(0.0),
        ..Form2550QCapitalGoodRow::default()
    }
}

fn zero_creditable_vat_row() -> Form2550QCreditableVatRow {
    Form2550QCreditableVatRow {
        income_payment: Some(0.0),
        tax_withheld: Some(0.0),
        ..Form2550QCreditableVatRow::default()
    }
}

fn zero_advance_vat_row() -> Form2550QAdvanceVatRow {
    Form2550QAdvanceVatRow {
        official_receipt_number: "0.00".to_string(),
        amount_paid: Some(0.0),
        ..Form2550QAdvanceVatRow::default()
    }
}

fn default_calendar_period(
    taxable_year: u16,
    quarter: Form2550QQuarter,
) -> Option<(Form2550QDate, Form2550QDate)> {
    let quarter = quarter.number()?;
    let start_month = quarter.checked_sub(1)?.checked_mul(3)?.checked_add(1)?;
    let end_month = start_month.checked_add(2)?;
    let next_month = if end_month == 12 { 1 } else { end_month + 1 };
    let next_year = if end_month == 12 {
        taxable_year.checked_add(1)?
    } else {
        taxable_year
    };
    let next_start =
        chrono::NaiveDate::from_ymd_opt(i32::from(next_year), u32::from(next_month), 1)?;
    let end = next_start.pred_opt()?;
    Some((
        Form2550QDate::new(taxable_year, start_month, 1).ok()?,
        Form2550QDate::from_naive(end).ok()?,
    ))
}

fn legacy_bool(fields: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<bool> {
    fields.get(key)?.as_bool()
}

fn legacy_money(fields: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<f64> {
    let value = fields.get(key)?;
    let parsed = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().replace(',', "").parse::<f64>().ok())?;
    parsed.is_finite().then_some(parsed)
}

fn legacy_text(fields: &BTreeMap<String, serde_json::Value>, key: &str) -> String {
    fields
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn legacy_u8(fields: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<u8> {
    let value = fields.get(key)?;
    value
        .as_u64()
        .and_then(|number| u8::try_from(number).ok())
        .or_else(|| value.as_str()?.trim().parse::<u8>().ok())
}

fn legacy_u16(fields: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<u16> {
    let value = legacy_money(fields, key)?;
    if value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    u16::try_from(value as u64).ok()
}

fn legacy_return_date(
    fields: &BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Option<Form2550QDate> {
    let value = legacy_text(fields, key);
    if value.trim().is_empty() {
        None
    } else {
        Form2550QDate::parse_return_period(&value).ok()
    }
}

fn legacy_identifier(fields: &BTreeMap<String, serde_json::Value>, key: &str) -> String {
    let Some(value) = fields.get(key) else {
        return String::new();
    };
    if let Some(text) = value.as_str() {
        text.to_string()
    } else if let Some(number) = value.as_f64().filter(|number| number.is_finite()) {
        format!("{number:.2}")
    } else {
        String::new()
    }
}

fn legacy_capital_good_row(
    fields: &BTreeMap<String, serde_json::Value>,
    suffix: u8,
) -> Form2550QCapitalGoodRow {
    Form2550QCapitalGoodRow {
        instance_id: None,
        purchase_or_import_date: legacy_return_date(fields, &format!("txt_date_purchase{suffix}")),
        source_code: legacy_text(fields, &format!("txt_source_code{suffix}")),
        description: legacy_text(fields, &format!("txt_description{suffix}")),
        purchase_or_import_amount: legacy_money(fields, &format!("txt_amount_purchase{suffix}")),
        input_tax: legacy_money(fields, &format!("txt_input_tax{suffix}")),
        estimated_life_months: legacy_u16(fields, &format!("txt_estimated_life{suffix}")),
        recognized_life_months: legacy_u16(fields, &format!("txt_recognized_life{suffix}")),
        allowable_input_tax_for_period: legacy_money(
            fields,
            &format!("txt_allowed_input_tax{suffix}"),
        ),
        balance_to_next_period: legacy_money(fields, &format!("txt_balance_input_tax{suffix}")),
    }
}

fn legacy_creditable_vat_row(
    fields: &BTreeMap<String, serde_json::Value>,
    suffix: u8,
) -> Form2550QCreditableVatRow {
    let row_index = suffix.saturating_sub(30);
    Form2550QCreditableVatRow {
        instance_id: None,
        period_from: legacy_return_date(fields, &format!("txt_date_covered{suffix}")),
        period_to: legacy_return_date(fields, &format!("txt_date_covered3to{row_index}")),
        withholding_agent_name: legacy_text(
            fields,
            &format!("txt_name_with_holding_agent{suffix}"),
        ),
        income_payment: legacy_money(fields, &format!("txt_income_payment{suffix}")),
        tax_withheld: legacy_money(fields, &format!("txt_total_tax_with_held{suffix}")),
    }
}

fn legacy_advance_vat_row(
    fields: &BTreeMap<String, serde_json::Value>,
    suffix: u8,
) -> Form2550QAdvanceVatRow {
    let row_index = suffix.saturating_sub(40);
    Form2550QAdvanceVatRow {
        instance_id: None,
        period_from: legacy_return_date(fields, &format!("txt_date{suffix}")),
        period_to: legacy_return_date(fields, &format!("txt_date4to{row_index}")),
        miller_name: legacy_text(fields, &format!("txt_name_of_miller{suffix}")),
        taxpayer_name: legacy_text(fields, &format!("txt_name_of_taxpayer{suffix}")),
        official_receipt_number: legacy_identifier(
            fields,
            &format!("txt_official_receipt_number{suffix}"),
        ),
        amount_paid: legacy_money(fields, &format!("txt_amount_paid{suffix}")),
    }
}

/// The official `formatCurrency` rounding: `Math.floor(|x| * 100 +
/// 0.50000000001)` cents, sign restored.
fn format_currency_cents(value: f64) -> f64 {
    let cents = (value.abs() * 100.0 + 0.500_000_000_01).floor() / 100.0;
    if value < 0.0 { -cents } else { cents }
}

/// `saveAndCloseNo19/42/47/56`: every popup row needs a description and an
/// amount greater than zero.
pub const SPECIFY_ROW_SAVE_ALERT: &str =
    "Please fill up all required fields and ensure amounts are greater than zero.";
/// `validationSpecify19/42/47/56`, checked by "Add Row" (`addColNo19/42/47/56`).
pub const SPECIFY_ROW_ADD_ALERT: &str = "Empty fields are not allowed";
/// The popup row description box's `maxlength`.
pub const SPECIFY_ROW_DESCRIPTION_MAX: usize = 50;

/// The official `validationSpecify19/42/47/56` before "Add Row": no row may
/// have an empty description or a `0.00` amount.
pub fn specify_add_row_alert(rows: &[Form2550QAdditionalItemRow]) -> Option<&'static str> {
    rows.iter()
        .any(|row| {
            row.description.is_empty() || row.amount.is_none_or(|amount| money(amount) == 0.0)
        })
        .then_some(SPECIFY_ROW_ADD_ALERT)
}

fn validate_specify_popup_rows(draft: &Form2550QDraft, errors: &mut Vec<(String, String)>) {
    for (field, rows) in [
        ("item_19_additional_rows", &draft.item_19_additional_rows),
        ("item_42_additional_rows", &draft.item_42_additional_rows),
        ("item_47_additional_rows", &draft.item_47_additional_rows),
        ("item_56_additional_rows", &draft.item_56_additional_rows),
    ] {
        for (index, row) in rows.iter().enumerate() {
            // `manualTrim`, then `description === "" || amount <= 0`.
            if row.description.trim().is_empty() || row.amount.is_none_or(|amount| amount <= 0.0) {
                errors.push((
                    format!("{field}[{index}]"),
                    SPECIFY_ROW_SAVE_ALERT.to_string(),
                ));
            }
            if row.description.chars().count() > SPECIFY_ROW_DESCRIPTION_MAX {
                errors.push((
                    format!("{field}[{index}].description"),
                    format!("Use at most {SPECIFY_ROW_DESCRIPTION_MAX} characters."),
                ));
            }
        }
    }
}

impl Form2550QDraft {
    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the nine TIN digits of the page 2 header
    ///   comb (its branch slots are preprinted).
    /// - `derived:address_line1` / `derived:line:address_line2`: Item 9 over
    ///   its 40-slot first line, broken at a space, and the 31-slot second.
    /// - `derived:sched3_period{0,1}` / `derived:sched4_period{0,1}`: the
    ///   Schedule 3 and 4 "Period Covered" cell, `MM/DD/YY-MM/DD/YY`.
    ///
    /// Schedule 4 rows past the draft's print blank (the upload writes
    /// `0.00` into an empty official receipt number).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm2550qv2024:txtPg2TIN1",
                "frm2550qv2024:txtPg2TIN2",
                "frm2550qv2024:txtPg2TIN3",
            ],
        );
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        let period = |from: Option<Form2550QDate>, to: Option<Form2550QDate>| {
            let short = |date: Form2550QDate| {
                format!("{:02}/{:02}/{:02}", date.month, date.day, date.year % 100)
            };
            match (from, to) {
                (Some(from), Some(to)) => format!("{}-{}", short(from), short(to)),
                (Some(date), None) | (None, Some(date)) => short(date),
                (None, None) => String::new(),
            }
        };
        let mut derived = vec![
            ("tin_digits".to_string(), tin_digits),
            ("address_line1".to_string(), line1),
            ("line:address_line2".to_string(), line2),
        ];
        for offset in 0..2 {
            let row3 = self.schedule_3.get(offset);
            derived.push((
                format!("sched3_period{offset}"),
                row3.map(|row| period(row.period_from, row.period_to))
                    .unwrap_or_default(),
            ));
            let Some(row4) = self.schedule_4.get(offset) else {
                for key in ["txtOfficialReceiptNumber4", "txtAmountPaid4"] {
                    fields.insert(format!("{key}{offset}"), String::new());
                }
                derived.push((format!("sched4_period{offset}"), String::new()));
                continue;
            };
            derived.push((
                format!("sched4_period{offset}"),
                period(row4.period_from, row4.period_to),
            ));
        }
        for (key, value) in derived {
            fields.insert(format!("derived:{key}"), value);
        }
        fields
    }

    /// `closeModalNo19/42/47/56`: an item with popup rows shows `VARIOUS`
    /// (or the one row's description) and `formatCurrency` of the rows' sum.
    fn apply_specify_popup_rows(&mut self) {
        fn summary(rows: &[Form2550QAdditionalItemRow]) -> Option<(String, Option<f64>)> {
            let first = rows.first()?;
            let description = if rows.len() > 1 {
                "VARIOUS".to_string()
            } else {
                first.description.clone()
            };
            let total: f64 = rows
                .iter()
                .map(|row| row.amount.map_or(0.0, format_currency_cents))
                .sum();
            Some((description, Some(format_currency_cents(total))))
        }
        if let Some((description, total)) = summary(&self.item_19_additional_rows) {
            self.part_ii.item_19_description = description;
            self.part_ii.item_19_other_credit_or_payment = total;
        }
        let p4 = &mut self.part_iv;
        if let Some((description, total)) = summary(&self.item_42_additional_rows) {
            p4.item_42_description = description;
            p4.item_42b_other_input_tax = total;
        }
        if let Some((description, total)) = summary(&self.item_47_additional_rows) {
            p4.item_47_description = description;
            p4.item_47a_other_purchases = total;
            // A popup row amount's blur also runs `compute47AB`: 47B is
            // `formatCurrency(47A * 0.12)`.
            p4.item_47b_other_input_tax =
                total.map(|total| format_currency_cents(total * VAT_RATE));
        }
        if let Some((description, total)) = summary(&self.item_56_additional_rows) {
            p4.item_56_description = description;
            p4.item_56b_other_deduction = total;
        }
    }
}

fn money(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn sum_money(values: &[Option<f64>]) -> Option<f64> {
    values
        .iter()
        .try_fold(0.0, |total, value| value.map(|amount| total + amount))
        .map(money)
}

fn subtract_money(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    Some(money(left? - right?))
}

fn add_subtract_money(
    base: Option<f64>,
    addition: Option<f64>,
    deduction: Option<f64>,
) -> Option<f64> {
    Some(money(base? + addition? - deduction?))
}

fn compute_ratable_input_tax(schedule: &Form2550QSchedule2) -> Option<f64> {
    let exempt_sales = schedule.vat_exempt_sales?;
    let total_sales = schedule.total_sales?;
    let indirect_input_tax = schedule.input_tax_not_directly_attributable?;
    if total_sales == 0.0 {
        if exempt_sales == 0.0 || indirect_input_tax == 0.0 {
            Some(0.0)
        } else {
            None
        }
    } else {
        Some(money(exempt_sales / total_sales * indirect_input_tax))
    }
}

fn validate_identity(draft: &Form2550QDraft, errors: &mut Vec<(String, String)>) {
    let tin_digits: String = draft
        .tin
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    if !matches!(tin_digits.len(), 12..=14)
        || draft
            .tin
            .chars()
            .any(|character| !character.is_ascii_digit() && character != '-')
    {
        errors.push((
            "tin".to_string(),
            "TIN must contain 12 to 14 digits, with optional dashes".to_string(),
        ));
    }
    for (field, label, value) in [
        ("rdo_code", "RDO code", draft.rdo_code.as_str()),
        (
            "taxpayer_name",
            "Taxpayer name",
            draft.taxpayer_name.as_str(),
        ),
        (
            "registered_address",
            "Registered address",
            draft.registered_address.as_str(),
        ),
        ("zip_code", "ZIP code", draft.zip_code.as_str()),
        (
            "contact_number",
            "Contact number",
            draft.contact_number.as_str(),
        ),
        ("email", "Email address", draft.email.as_str()),
    ] {
        if value.trim().is_empty() {
            errors.push((field.to_string(), format!("{label} is required")));
        }
    }
    if !draft.rdo_code.trim().is_empty()
        && (draft.rdo_code.len() != 3
            || !draft
                .rdo_code
                .chars()
                .all(|character| character.is_ascii_digit()))
    {
        errors.push((
            "rdo_code".to_string(),
            "RDO code must be 3 digits".to_string(),
        ));
    }
    if !draft.zip_code.trim().is_empty() && !validate_zip(draft.zip_code.trim()) {
        errors.push((
            "zip_code".to_string(),
            "ZIP code must be 4 digits".to_string(),
        ));
    }
    if !draft.contact_number.trim().is_empty() && !validate_ph_phone(&draft.contact_number) {
        errors.push((
            "contact_number".to_string(),
            "Contact number must be a valid Philippine mobile or landline number".to_string(),
        ));
    }
    if !draft.email.trim().is_empty() && !validate_email(&draft.email) {
        errors.push(("email".to_string(), "Email address is invalid".to_string()));
    }
}

fn validate_period(draft: &Form2550QDraft, errors: &mut Vec<(String, String)>) {
    if !(1900..=9999).contains(&draft.taxable_year) {
        errors.push((
            "taxable_year".to_string(),
            "Taxable year must contain four digits".to_string(),
        ));
    }
    if !(1..=12).contains(&draft.year_end_month) {
        errors.push((
            "year_end_month".to_string(),
            "Year-end month must be from 1 to 12".to_string(),
        ));
    }
    if draft.quarter.number().is_none() {
        errors.push((
            "quarter".to_string(),
            "Quarter must be from 1 to 4".to_string(),
        ));
    }
    match (draft.return_period_from, draft.return_period_to) {
        (Some(from), Some(to)) => {
            if from.validate().is_err() || to.validate().is_err() {
                errors.push((
                    "return_period".to_string(),
                    "Return period dates must be real calendar dates".to_string(),
                ));
            } else if from.as_naive() > to.as_naive() {
                errors.push((
                    "return_period".to_string(),
                    "Return period From date cannot be after the To date".to_string(),
                ));
            }
        }
        _ => errors.push((
            "return_period".to_string(),
            "Both return period dates are required".to_string(),
        )),
    }
}

fn validate_required_non_negative_money(
    field: &str,
    value: Option<f64>,
    errors: &mut Vec<(String, String)>,
) {
    match value {
        None => errors.push((
            field.to_string(),
            "Amount is blank; enter 0.00 explicitly when there is no amount".to_string(),
        )),
        Some(amount) if !amount.is_finite() || amount < 0.0 => errors.push((
            field.to_string(),
            "Amount must be a finite, non-negative number".to_string(),
        )),
        Some(_) => {}
    }
}

/// Schedule 1/3/4 `checkInputField*`: any blank field in a used row (the
/// official text ends with a space).
const OFFICIAL_EMPTY_FIELDS_MESSAGE: &str = "Empty fields are not allowed ";
/// Schedule 1 date blur.
const OFFICIAL_DATE_PURCHASE_EMPTY_MESSAGE: &str = "Date Purchase cannot be an empty field.";
/// `validateSourceCode`: two alerts, "Invalid input" then this one.
const OFFICIAL_SOURCE_CODE_MESSAGE: &str = "Invalid input\nAccept only I and D ";
/// `showNewSched3` (the double space is official).
const OFFICIAL_RETURN_PERIOD_YEAR_MESSAGE: &str = "Please  enter a valid year for the Return Period. Only values 2000 and beyond will be accepted.";
/// Schedules 3 and 4: "From" after "To".
const OFFICIAL_FROM_AFTER_TO_MESSAGE: &str = "'From' date cannot be after 'To' date.";

/// Schedules 3 and 4 date checks: both dates inside Items 4 (return period),
/// then From not after To, in the official order.
fn validate_row_period(
    draft: &Form2550QDraft,
    field: &str,
    from: Option<Form2550QDate>,
    to: Option<Form2550QDate>,
    errors: &mut Vec<(String, String)>,
) {
    let (Some(from), Some(to)) = (from, to) else {
        return;
    };
    if let (Some(start), Some(end)) = (draft.return_period_from, draft.return_period_to) {
        let inside = |date: Form2550QDate| {
            matches!(
                (date.as_naive(), start.as_naive(), end.as_naive()),
                (Some(date), Some(start), Some(end)) if date >= start && date <= end
            )
        };
        if !inside(from) || !inside(to) {
            errors.push((
                format!("{field}.period"),
                format!("Dates must fall within the Return Period: {start} to {end}"),
            ));
            return;
        }
    }
    if from.as_naive() > to.as_naive() {
        errors.push((
            format!("{field}.period"),
            OFFICIAL_FROM_AFTER_TO_MESSAGE.to_string(),
        ));
    }
}

fn validate_schedule_rows(draft: &Form2550QDraft, errors: &mut Vec<(String, String)>) {
    // Schedules 1, 3 and 4 are the official `addlistSched1/3/4` lists: any
    // number of rows, no limit on the official page.
    for (index, row) in draft.schedule_1.iter().enumerate() {
        if row.is_empty() {
            continue;
        }
        let field = format!("schedule_1[{index}]");
        if row.purchase_or_import_date.is_none() {
            errors.push((
                format!("{field}.purchase_or_import_date"),
                OFFICIAL_DATE_PURCHASE_EMPTY_MESSAGE.to_string(),
            ));
        }
        if row.purchase_or_import_date.is_none()
            || row.source_code.trim().is_empty()
            || row.description.trim().is_empty()
            || row.estimated_life_months.is_none()
            || row.recognized_life_months.is_none()
        {
            errors.push((field.clone(), OFFICIAL_EMPTY_FIELDS_MESSAGE.to_string()));
        }
        // Official `computeSched1` / `validateEstimatedLife` alerts.
        if row
            .purchase_or_import_date
            .is_some_and(is_after_schedule_1_cutoff)
        {
            errors.push((
                format!("{field}.purchase_or_import_date"),
                "Only dates on or before December 31, 2021 are allowed.".to_string(),
            ));
        }
        if row.estimated_life_months.is_some_and(|months| months > 60) {
            errors.push((
                format!("{field}.estimated_life_months"),
                "Invalid Input it should be 1 to 60".to_string(),
            ));
        }
        if !row.source_code.trim().is_empty()
            && !matches!(
                row.source_code.trim().to_ascii_uppercase().as_str(),
                "D" | "I"
            )
        {
            errors.push((
                format!("{field}.source_code"),
                OFFICIAL_SOURCE_CODE_MESSAGE.to_string(),
            ));
        }
        for (suffix, value) in [
            ("purchase_or_import_amount", row.purchase_or_import_amount),
            ("input_tax", row.input_tax),
            (
                "allowable_input_tax_for_period",
                row.allowable_input_tax_for_period,
            ),
            ("balance_to_next_period", row.balance_to_next_period),
        ] {
            validate_required_non_negative_money(&format!("{field}.{suffix}"), value, errors);
        }
    }
    // `showNewSched3`: the Schedule 3 popup refuses a return year of 2000
    // or earlier (`year <= 2000 && year <= 2020` reduces to `year <= 2000`).
    if draft.taxable_year <= 2000 && draft.schedule_3.iter().any(|row| !row.is_empty()) {
        errors.push((
            "schedule_3".to_string(),
            OFFICIAL_RETURN_PERIOD_YEAR_MESSAGE.to_string(),
        ));
    }
    for (index, row) in draft.schedule_3.iter().enumerate() {
        if row.is_empty() {
            continue;
        }
        let field = format!("schedule_3[{index}]");
        if row.period_from.is_none()
            || row.period_to.is_none()
            || row.withholding_agent_name.trim().is_empty()
        {
            errors.push((field.clone(), OFFICIAL_EMPTY_FIELDS_MESSAGE.to_string()));
        }
        validate_row_period(draft, &field, row.period_from, row.period_to, errors);
        validate_required_non_negative_money(
            &format!("{field}.income_payment"),
            row.income_payment,
            errors,
        );
        validate_required_non_negative_money(
            &format!("{field}.tax_withheld"),
            row.tax_withheld,
            errors,
        );
    }
    for (index, row) in draft.schedule_4.iter().enumerate() {
        if row.is_empty() {
            continue;
        }
        let field = format!("schedule_4[{index}]");
        if row.period_from.is_none()
            || row.period_to.is_none()
            || row.miller_name.trim().is_empty()
            || row.taxpayer_name.trim().is_empty()
            || matches!(row.official_receipt_number.trim(), "" | "0.00")
        {
            errors.push((field.clone(), OFFICIAL_EMPTY_FIELDS_MESSAGE.to_string()));
        }
        validate_row_period(draft, &field, row.period_from, row.period_to, errors);
        validate_required_non_negative_money(
            &format!("{field}.amount_paid"),
            row.amount_paid,
            errors,
        );
    }
}

fn validate_local_print_fields(
    fields: &Form2550QLocalPrintFields,
    errors: &mut Vec<(String, String)>,
) {
    for (field, value) in [
        (
            "tax_agent_date_of_issue",
            fields.tax_agent_date_of_issue.as_str(),
        ),
        (
            "tax_agent_date_of_expiry",
            fields.tax_agent_date_of_expiry.as_str(),
        ),
        ("check_date", fields.check_date.as_str()),
        ("tax_debit_memo_date", fields.tax_debit_memo_date.as_str()),
        ("other_payment_date", fields.other_payment_date.as_str()),
    ] {
        if !value.trim().is_empty()
            && chrono::NaiveDate::parse_from_str(value.trim(), "%m/%d/%Y").is_err()
        {
            errors.push((
                format!("local_print_fields.{field}"),
                "Date must use MM/DD/YYYY".to_string(),
            ));
        }
    }
    for (field, value) in [
        (
            "cash_or_bank_debit_advice_amount",
            fields.cash_or_bank_debit_advice_amount,
        ),
        ("check_amount", fields.check_amount),
        ("tax_debit_memo_amount", fields.tax_debit_memo_amount),
        ("other_payment_amount", fields.other_payment_amount),
    ] {
        if value.is_some_and(|amount| !amount.is_finite() || amount < 0.0) {
            errors.push((
                format!("local_print_fields.{field}"),
                "Payment amount must be finite and non-negative".to_string(),
            ));
        }
    }
}

fn user_money_fields(draft: &Form2550QDraft) -> Vec<(&'static str, Option<f64>)> {
    let p2 = &draft.part_ii;
    let p4 = &draft.part_iv;
    vec![
        (
            "item_18_paid_on_previous_return",
            p2.item_18_paid_on_previous_return,
        ),
        (
            "item_19_other_credit_or_payment",
            p2.item_19_other_credit_or_payment,
        ),
        ("item_22_surcharge", p2.item_22_surcharge),
        ("item_23_interest", p2.item_23_interest),
        ("item_24_compromise", p2.item_24_compromise),
        ("item_31a_vatable_sales", p4.item_31a_vatable_sales),
        ("item_32a_zero_rated_sales", p4.item_32a_zero_rated_sales),
        ("item_33a_exempt_sales", p4.item_33a_exempt_sales),
        (
            "item_35b_less_output_vat_uncollected",
            p4.item_35b_less_output_vat_uncollected,
        ),
        (
            "item_36b_add_output_vat_recovered",
            p4.item_36b_add_output_vat_recovered,
        ),
        ("item_38b_input_tax_carried", p4.item_38b_input_tax_carried),
        (
            "item_40b_transitional_input_tax",
            p4.item_40b_transitional_input_tax,
        ),
        (
            "item_41b_presumptive_input_tax",
            p4.item_41b_presumptive_input_tax,
        ),
        ("item_42b_other_input_tax", p4.item_42b_other_input_tax),
        (
            "item_44a_domestic_purchases",
            p4.item_44a_domestic_purchases,
        ),
        (
            "item_44b_domestic_input_tax",
            p4.item_44b_domestic_input_tax,
        ),
        (
            "item_45a_nonresident_services",
            p4.item_45a_nonresident_services,
        ),
        (
            "item_45b_nonresident_service_input_tax",
            p4.item_45b_nonresident_service_input_tax,
        ),
        ("item_46a_importations", p4.item_46a_importations),
        ("item_46b_import_input_tax", p4.item_46b_import_input_tax),
        ("item_47a_other_purchases", p4.item_47a_other_purchases),
        ("item_47b_other_input_tax", p4.item_47b_other_input_tax),
        (
            "item_48a_domestic_purchases_no_input_tax",
            p4.item_48a_domestic_purchases_no_input_tax,
        ),
        (
            "item_49a_vat_exempt_importations",
            p4.item_49a_vat_exempt_importations,
        ),
        (
            "item_54b_vat_refund_or_tcc_claimed",
            p4.item_54b_vat_refund_or_tcc_claimed,
        ),
        (
            "item_55b_input_vat_on_unpaid_payables",
            p4.item_55b_input_vat_on_unpaid_payables,
        ),
        ("item_56b_other_deduction", p4.item_56b_other_deduction),
        (
            "item_58b_input_vat_on_settled_payables",
            p4.item_58b_input_vat_on_settled_payables,
        ),
        (
            "schedule_2.input_tax_directly_attributable",
            draft
                .schedule_2
                .input_tax_directly_attributable_to_exempt_sales,
        ),
        (
            "schedule_2.vat_exempt_sales",
            draft.schedule_2.vat_exempt_sales,
        ),
        (
            "schedule_2.input_tax_not_directly_attributable",
            draft.schedule_2.input_tax_not_directly_attributable,
        ),
    ]
}

fn computed_money_fields(draft: &Form2550QDraft) -> Vec<(&'static str, Option<f64>)> {
    let p2 = &draft.part_ii;
    let p4 = &draft.part_iv;
    vec![
        ("item_15", p2.item_15_net_vat_payable_or_excess),
        ("item_16", p2.item_16_creditable_vat_withheld),
        ("item_17", p2.item_17_advance_vat_payments),
        ("item_20", p2.item_20_total_credits_or_payments),
        ("item_21", p2.item_21_tax_payable_or_excess_credits),
        ("item_25", p2.item_25_total_penalties),
        ("item_26", p2.item_26_total_amount_payable_or_excess),
        ("item_31b", p4.item_31b_output_tax),
        ("item_34a", p4.item_34a_total_sales),
        ("item_34b", p4.item_34b_output_tax_due),
        ("item_37b", p4.item_37b_adjusted_output_tax_due),
        ("item_39b", p4.item_39b_input_tax_deferred),
        ("item_43b", p4.item_43b_total_prior_input_tax),
        ("item_50a", p4.item_50a_total_current_purchases),
        ("item_50b", p4.item_50b_total_current_input_tax),
        ("item_51b", p4.item_51b_total_available_input_tax),
        ("item_52b", p4.item_52b_deferred_capital_goods_input_tax),
        (
            "item_53b",
            p4.item_53b_input_tax_attributable_to_exempt_sales,
        ),
        ("item_57b", p4.item_57b_total_deductions),
        ("item_59b", p4.item_59b_adjusted_deductions),
        ("item_60b", p4.item_60b_total_allowable_input_tax),
        ("item_61b", p4.item_61b_net_vat_payable_or_excess),
    ]
}

/// Rule-package id of the official upload layout.
pub const FORM_2550Q_LAYOUT_ID: &str = "2550q-v2024";

impl super::queueable::QueueableForm for Form2550QDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    const FORM_CODE: &'static str = FORM_CODE;
    /// Official `formType`, as `createXMLFileName()` writes it.
    const FORM_TYPE: &'static str = FORM_TYPE_ID;
    const LAYOUT_ID: &'static str = FORM_2550Q_LAYOUT_ID;

    fn lifecycle(&self) -> &SubmissionLifecycle {
        &self.lifecycle
    }
    fn lifecycle_mut(&mut self) -> &mut SubmissionLifecycle {
        &mut self.lifecycle
    }

    /// Item "Amended Return?" = Yes, as the editor sets it.
    fn mark_amended(&mut self) -> bool {
        self.is_amended = true;
        self.compute();
        true
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
    fn tin(&self) -> &str {
        &self.tin
    }
    fn taxable_year(&self) -> u16 {
        self.taxable_year
    }
    /// An unresolved imported quarter maps to Q0, which never validates.
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Quarterly(self.quarter.number().unwrap_or(0))
    }
    /// `selectedMonthNo2 + txtYearNo2 + "Q" + n`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!(
            "{:02}{}Q{}",
            self.year_end_month,
            self.taxable_year,
            self.quarter.number().unwrap_or(0)
        )
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 8 || code.get(6..7)? != "Q" {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let year: u16 = code.get(2..6)?.parse().ok()?;
        let quarter: u8 = code.get(7..)?.parse().ok()?;
        ((1..=12).contains(&month) && (1..=4).contains(&quarter))
            .then_some((year, FilingPeriod::Quarterly(quarter)))
    }
    fn submission_email(&self) -> &str {
        self.email.trim()
    }
    fn compute(&mut self) {
        self.recompute();
    }
    /// Every form rule, including raw-buffer coherence for any raw value the
    /// draft carries. A control without a raw buffer (a page-typed draft) is
    /// written from its typed value, as the official page writes its controls.
    fn validate(&self) -> Vec<(String, String)> {
        <Self as FormValidator>::validate(self)
    }
    /// The official controls `saveEncryptedProfile` walks. `dateFiled` is not
    /// a control: it is appended after the trailer by [`Self::official_payload`].
    /// A schedule with rows is written as its official modal rows
    /// (`addlistSched1/3/4`), see [`Self::official_layout`].
    fn field_map(&self) -> BTreeMap<String, String> {
        self.official_control_map()
    }
    /// `saveEncryptedProfile(true)`: the control loop, the trailer, then
    /// `<dateFiled>yyyy/mm/dd</dateFiled>` and a newline.
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = super::queueable::QueueableForm::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let layout = self
            .official_layout()
            .map_err(|error| vec![("xml".to_string(), error)])?;
        let mut payload = crate::official_xml::write(&layout, &self.field_map())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])?;
        let date_filed = self.date_filed.map_or_else(
            || chrono::Local::now().format("%Y/%m/%d").to_string(),
            Form2550QDate::to_filed_date,
        );
        payload.push_str("<dateFiled>");
        payload.push_str(&date_filed);
        payload.push_str("</dateFiled>\n");
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form_rules::{
        Form2550QCaptureError, Form2550QCaptureGap, Form2550QFieldValueSource,
        form_2550q_group_bindings, form_2550q_local_print_raw_field_keys,
        form_2550q_singleton_field_ids,
    };

    fn profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "JUAN DELA CRUZ",
            "tin": {
                "segment1": "000",
                "segment2": "000",
                "segment3": "000",
                "branch": "00000"
            },
            "rdo_code": "018",
            "line_of_business": "SOFTWARE DEVELOPMENT",
            "registered_address": "OLONGAPO",
            "zip_code": "2200",
            "phone": "09123456789",
            "email": "taxpayer@example.com",
            "default_form_type": "2550Qv2024",
            "taxpayer_type": "Individual",
            "tax_classification": "SelfEmployed",
            "is_vat_registered": true,
            "eopt_tier": "Medium",
            "withholding_obligations": [],
            "excise_tax_liabilities": [],
            "atc_codes": [],
            "business_start_date": null,
            "birth_date": null,
            "registration_activity_status": "Active",
            "is_dormant_entity": false,
            "is_government_withholding_entity": false,
            "is_gpp_partner": false,
            "is_top_withholding_agent": false,
            "income_tax_elections": { "elections": {} },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .expect("valid test profile")
    }

    fn add_legacy_flat_shape_fingerprint(json: &mut serde_json::Value, quarter: u8) {
        let object = json.as_object_mut().expect("draft JSON is an object");
        object.insert("month".to_string(), serde_json::json!(quarter));
        for (key, value) in [
            ("opt_short_prd1", serde_json::json!(false)),
            ("opt_short_prd2", serde_json::json!(true)),
            ("pg2tax_payer", serde_json::json!("JUAN DELA CRUZ")),
            ("rtn_period_from_no4", serde_json::json!("1/01/2026")),
            ("rtn_period_to_no4", serde_json::json!("3/31/2026")),
            ("calendar_no1", serde_json::json!(true)),
            ("fiscal_no1", serde_json::json!(false)),
            ("amended_return_no5", serde_json::json!(true)),
            ("amended_return_yes_no5", serde_json::json!(false)),
            ("txt_current_page", serde_json::json!(2)),
            ("txt_max_page", serde_json::json!(2)),
        ] {
            object.insert(key.to_string(), value);
        }
    }

    #[test]
    fn new_draft_uses_quarter_not_year_end_month() {
        let draft = Form2550QDraft::new_from_profile(&profile(), 2026, 2);
        assert_eq!(draft.quarter, Form2550QQuarter::Second);
        assert_eq!(draft.year_end_month, 12);
        assert_eq!(
            draft.return_period_from.expect("from").to_string(),
            "4/01/2026"
        );
        assert_eq!(draft.return_period_to.expect("to").to_string(), "6/30/2026");
    }

    #[test]
    fn scaffold_era_flat_json_is_migrated_without_losing_user_amounts() {
        let json = serde_json::json!({
            "id": null,
            "tin": "00000000000000",
            "taxable_year": 2026,
            "month": 2,
            "is_amended": false,
            "opt_short_prd1": false,
            "opt_short_prd2": true,
            "pg2tax_payer": "JUAN DELA CRUZ",
            "rtn_period_from_no4": "4/01/2026",
            "rtn_period_to_no4": "6/30/2026",
            "calendar_no1": true,
            "fiscal_no1": false,
            "amended_return_no5": true,
            "amended_return_yes_no5": false,
            "txt_current_page": 2,
            "txt_max_page": 2,
            "rdo_code": "018",
            "taxpayer_name": "JUAN DELA CRUZ",
            "registered_address": "OLONGAPO",
            "zip_code": "2200",
            "contact_number": "09123456789",
            "email": "taxpayer@example.com",
            "date_filed": "",
            "vatable_sales": 1000.0,
            "zero_rated_sales": 200.0,
            "exempt_sales": 300.0,
            "less_output_vat": 0.0,
            "add_output_vat": 0.0,
            "tax_payer_classification1": true,
            "future_flat_extension": {
                "must_survive": true
            },
            "status": "Draft",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        });
        let mut draft: Form2550QDraft =
            serde_json::from_value(json).expect("legacy JSON must deserialize");

        assert!(
            draft
                .migrate_legacy_flat_draft()
                .expect("complete legacy shape migrates")
        );
        assert_eq!(draft.quarter, Form2550QQuarter::Second);
        assert_eq!(draft.part_iv.item_31a_vatable_sales, Some(1_000.0));
        assert_eq!(draft.part_iv.item_31b_output_tax, Some(120.0));
        assert_eq!(
            draft.taxpayer_classification,
            Some(Form2550QTaxpayerClassification::Micro)
        );
        assert_eq!(
            draft.legacy_flat_draft_fields.get("future_flat_extension"),
            Some(&serde_json::json!({ "must_survive": true }))
        );
        assert!(!draft.legacy_flat_draft_fields.contains_key("month"));
        assert!(
            !serde_json::to_value(&draft)
                .expect("current JSON serializes")
                .as_object()
                .expect("draft is an object")
                .contains_key("month")
        );
    }

    #[test]
    fn month_alone_and_current_legacy_hybrids_refuse_migration_without_mutation() {
        let mut month_only_json =
            serde_json::to_value(Form2550QDraft::new_from_profile(&profile(), 2026, 1))
                .expect("current draft serializes");
        month_only_json
            .as_object_mut()
            .expect("draft JSON is an object")
            .insert("month".to_string(), serde_json::json!(2));
        let mut month_only: Form2550QDraft =
            serde_json::from_value(month_only_json).expect("flattened month is preserved");
        let month_only_before = month_only.clone();
        assert!(matches!(
            month_only.migrate_legacy_flat_draft(),
            Err(Form2550QLegacyDraftMigrationError::IncompleteLegacyShape { .. })
        ));
        assert_eq!(month_only, month_only_before);
        month_only.recompute();
        assert_eq!(
            month_only, month_only_before,
            "recompute must not mutate an ambiguous flattened draft"
        );

        let mut hybrid_json =
            serde_json::to_value(Form2550QDraft::new_from_profile(&profile(), 2026, 1))
                .expect("current draft serializes");
        hybrid_json["part_iv"]["item_31a_vatable_sales"] = serde_json::json!(777.0);
        add_legacy_flat_shape_fingerprint(&mut hybrid_json, 2);
        hybrid_json
            .as_object_mut()
            .expect("draft JSON is an object")
            .insert(
                "future_flat_extension".to_string(),
                serde_json::json!({ "must_survive": true }),
            );
        let mut hybrid: Form2550QDraft =
            serde_json::from_value(hybrid_json).expect("known hybrid fields deserialize");
        let hybrid_before = hybrid.clone();
        assert!(matches!(
            hybrid.migrate_legacy_flat_draft(),
            Err(Form2550QLegacyDraftMigrationError::AmbiguousHybridShape)
        ));
        assert_eq!(hybrid, hybrid_before);
        assert_eq!(hybrid.part_iv.item_31a_vatable_sales, Some(777.0));
        assert_eq!(
            hybrid.legacy_flat_draft_fields.get("future_flat_extension"),
            Some(&serde_json::json!({ "must_survive": true }))
        );
    }

    #[test]
    fn nested_unknown_2550q_properties_fail_deserialization_visibly() {
        let base = serde_json::to_value(Form2550QDraft::new_from_profile(&profile(), 2026, 1))
            .expect("current draft serializes");
        let mut cases = Vec::new();

        let mut part_ii = base.clone();
        part_ii["part_ii"]["future_field"] = serde_json::json!("preserve-or-fail");
        cases.push(("part_ii", part_ii));

        let mut part_iv = base.clone();
        part_iv["part_iv"]["future_field"] = serde_json::json!("preserve-or-fail");
        cases.push(("part_iv", part_iv));

        let mut schedule_1 = base.clone();
        schedule_1["schedule_1"][0]["future_field"] = serde_json::json!("preserve-or-fail");
        cases.push(("schedule_1 row", schedule_1));

        let mut schedule_2 = base.clone();
        schedule_2["schedule_2"]["future_field"] = serde_json::json!("preserve-or-fail");
        cases.push(("schedule_2", schedule_2));

        let mut schedule_3 = base.clone();
        schedule_3["schedule_3"][0]["future_field"] = serde_json::json!("preserve-or-fail");
        cases.push(("schedule_3 row", schedule_3));

        let mut schedule_4 = base.clone();
        schedule_4["schedule_4"][0]["future_field"] = serde_json::json!("preserve-or-fail");
        cases.push(("schedule_4 row", schedule_4));

        let mut additional = base.clone();
        additional["item_19_additional_rows"] = serde_json::json!([{
            "instance_id": null,
            "description": "",
            "amount": null,
            "future_field": "preserve-or-fail"
        }]);
        cases.push(("additional row", additional));

        let mut local_print = base.clone();
        local_print["local_print_fields"]["future_field"] = serde_json::json!("preserve-or-fail");
        cases.push(("local print", local_print));

        let mut date = base;
        date["return_period_from"]["future_field"] = serde_json::json!("preserve-or-fail");
        cases.push(("date", date));

        for (location, json) in cases {
            let error = serde_json::from_value::<Form2550QDraft>(json)
                .expect_err("unknown nested data must not be silently dropped");
            assert!(
                error.to_string().contains("unknown field"),
                "{location} should report its unknown field, got {error}"
            );
        }
    }

    #[test]
    fn current_json_round_trips_typed_filed_date() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.date_filed = Some(Form2550QDate::new(2026, 4, 25).expect("valid date"));
        let json = serde_json::to_value(&draft).expect("current JSON serializes");
        let restored: Form2550QDraft =
            serde_json::from_value(json).expect("current JSON deserializes");

        assert_eq!(restored.date_filed, draft.date_filed);
        assert!(restored.legacy_flat_draft_fields.is_empty());
    }

    #[test]
    fn stable_row_ids_are_seeded_once_and_survive_json_round_trip() {
        let draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let expected = (1..=6)
            .map(|ordinal| format!("row-{ordinal:020}"))
            .collect::<Vec<_>>();
        let actual = draft
            .schedule_1
            .iter()
            .map(|row| row.instance_id.as_ref())
            .chain(draft.schedule_3.iter().map(|row| row.instance_id.as_ref()))
            .chain(draft.schedule_4.iter().map(|row| row.instance_id.as_ref()))
            .map(|instance_id| instance_id.expect("new rows have IDs").to_string())
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);
        assert_eq!(draft.row_identity_state.next_ordinal(), 7);

        let restored: Form2550QDraft =
            serde_json::from_value(serde_json::to_value(&draft).expect("current draft serializes"))
                .expect("current draft deserializes");
        assert_eq!(
            restored
                .schedule_1
                .iter()
                .map(|row| row.instance_id.as_ref())
                .chain(
                    restored
                        .schedule_3
                        .iter()
                        .map(|row| row.instance_id.as_ref())
                )
                .chain(
                    restored
                        .schedule_4
                        .iter()
                        .map(|row| row.instance_id.as_ref())
                )
                .collect::<Vec<_>>(),
            draft
                .schedule_1
                .iter()
                .map(|row| row.instance_id.as_ref())
                .chain(draft.schedule_3.iter().map(|row| row.instance_id.as_ref()))
                .chain(draft.schedule_4.iter().map(|row| row.instance_id.as_ref()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn legacy_rows_without_ids_are_lazily_seeded_monotonically() {
        let draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let mut json = serde_json::to_value(&draft).expect("current draft serializes");
        let object = json.as_object_mut().expect("draft JSON is an object");
        object.remove("row_identity_state");
        object.remove("raw_editor_state");
        object.remove("item_19_additional_rows");
        object.remove("item_42_additional_rows");
        object.remove("item_47_additional_rows");
        object.remove("item_56_additional_rows");
        for family in ["schedule_1", "schedule_3", "schedule_4"] {
            for row in object
                .get_mut(family)
                .and_then(serde_json::Value::as_array_mut)
                .expect("schedule is an array")
            {
                row.as_object_mut()
                    .expect("row is an object")
                    .remove("instance_id");
            }
        }

        let mut restored: Form2550QDraft =
            serde_json::from_value(json).expect("legacy current-shape JSON deserializes");
        assert!(restored.schedule_1[0].instance_id.is_none());
        assert!(
            restored
                .ensure_repeating_row_ids()
                .expect("legacy row IDs seed")
        );
        let first_ids = restored
            .schedule_1
            .iter()
            .map(|row| row.instance_id.clone())
            .chain(
                restored
                    .schedule_3
                    .iter()
                    .map(|row| row.instance_id.clone()),
            )
            .chain(
                restored
                    .schedule_4
                    .iter()
                    .map(|row| row.instance_id.clone()),
            )
            .collect::<Vec<_>>();
        assert!(!restored.ensure_repeating_row_ids().expect("IDs revalidate"));
        assert_eq!(
            first_ids,
            restored
                .schedule_1
                .iter()
                .map(|row| row.instance_id.clone())
                .chain(
                    restored
                        .schedule_3
                        .iter()
                        .map(|row| row.instance_id.clone())
                )
                .chain(
                    restored
                        .schedule_4
                        .iter()
                        .map(|row| row.instance_id.clone())
                )
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_or_unsupported_persisted_row_ids_fail_closed() {
        let draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let mut duplicate = draft.clone();
        duplicate.schedule_1[1].instance_id = duplicate.schedule_1[0].instance_id.clone();
        assert!(matches!(
            duplicate.ensure_repeating_row_ids(),
            Err(Form2550QRowIdentityError::Duplicate {
                family: Form2550QRowFamily::Schedule1,
                ..
            })
        ));

        let mut json = serde_json::to_value(&draft).expect("current draft serializes");
        json["schedule_1"][0]["instance_id"] = serde_json::json!("legacy-row-id");
        let mut unsupported: Form2550QDraft =
            serde_json::from_value(json).expect("syntactically stable ID deserializes");
        assert!(matches!(
            unsupported.ensure_repeating_row_ids(),
            Err(Form2550QRowIdentityError::UnsupportedFormat {
                family: Form2550QRowFamily::Schedule1,
                ..
            })
        ));

        let mut zero_ordinal_json =
            serde_json::to_value(&draft).expect("current draft serializes for zero ordinal");
        zero_ordinal_json["schedule_1"][0]["instance_id"] =
            serde_json::json!("row-00000000000000000000");
        let mut zero_ordinal: Form2550QDraft = serde_json::from_value(zero_ordinal_json)
            .expect("zero ordinal is syntactically stable");
        assert!(matches!(
            zero_ordinal.ensure_repeating_row_ids(),
            Err(Form2550QRowIdentityError::UnsupportedFormat {
                family: Form2550QRowFamily::Schedule1,
                ..
            })
        ));

        let mut malformed_json =
            serde_json::to_value(&draft).expect("current draft serializes again");
        malformed_json["schedule_1"][0]["instance_id"] = serde_json::json!("not a stable id");
        assert!(
            serde_json::from_value::<Form2550QDraft>(malformed_json).is_err(),
            "malformed StableInstanceId must fail during deserialization"
        );
    }

    #[test]
    fn raw_editor_json_preserves_absent_blank_malformed_and_family_wire_names() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft
            .raw_editor_state
            .set_singleton("frm2550qv2024:vatPaidReturn", RawValue::Absent);
        draft
            .raw_editor_state
            .set_singleton("frm2550qv2024:surcharge", RawValue::Text(String::new()));
        let schedule_id = draft.schedule_1[0]
            .instance_id
            .clone()
            .expect("new row has identity");
        draft.raw_editor_state.set_repeated(
            Form2550QRowFamily::Schedule1,
            schedule_id.clone(),
            "txtInputTax1",
            RawValue::Text("not-a-number".to_string()),
        );
        draft.raw_editor_state.mark_malformed(
            format!("schedule-1/{schedule_id}/txtInputTax1"),
            "not a number",
        );

        let json = serde_json::to_value(&draft).expect("raw draft serializes");
        let repeated = json["raw_editor_state"]["repeated_fields"]
            .as_object()
            .expect("repeated fields are a JSON object");
        // Compare as a set. `serde_json::Map` is a `BTreeMap` (alphabetical) or
        // an `IndexMap` (insertion order) depending on whether the
        // `preserve_order` feature is on, and feature unification turns it on
        // only for the full-workspace build. This assertion is about which
        // family wire names are emitted, not about the JSON object's key order.
        let mut repeated_keys = repeated.keys().map(String::as_str).collect::<Vec<_>>();
        repeated_keys.sort_unstable();
        assert_eq!(
            repeated_keys,
            vec![
                "item-19-additional",
                "item-42-additional",
                "item-47-additional",
                "item-56-additional",
                "schedule-1",
                "schedule-3",
                "schedule-4",
            ]
        );
        assert_eq!(
            json["raw_editor_state"]["singleton_fields"]["frm2550qv2024:vatPaidReturn"]["state"],
            "absent"
        );
        assert_eq!(
            json["raw_editor_state"]["singleton_fields"]["frm2550qv2024:surcharge"]["text"],
            ""
        );

        let restored: Form2550QDraft =
            serde_json::from_value(json).expect("raw draft deserializes");
        assert_eq!(restored.raw_editor_state, draft.raw_editor_state);
        assert!(restored.raw_editor_state.has_malformed_fields());
    }

    #[test]
    fn versioned_row_and_raw_state_reject_unknown_json_properties() {
        let draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);

        let mut unknown_raw = serde_json::to_value(&draft).expect("current draft serializes");
        unknown_raw["raw_editor_state"]["future_state"] = serde_json::json!({
            "must_not_be_silently_dropped": true
        });
        assert!(
            serde_json::from_value::<Form2550QDraft>(unknown_raw).is_err(),
            "unknown raw-state properties must fail closed"
        );

        let mut unknown_allocator =
            serde_json::to_value(&draft).expect("current draft serializes again");
        unknown_allocator["row_identity_state"]["future_allocator_metadata"] = serde_json::json!(1);
        assert!(
            serde_json::from_value::<Form2550QDraft>(unknown_allocator).is_err(),
            "unknown row-identity properties must fail closed"
        );
    }

    #[test]
    fn unknown_raw_editor_version_and_additional_rows_block_validation() {
        let draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let mut json = serde_json::to_value(&draft).expect("current draft serializes");
        json["raw_editor_state"]["version"] = serde_json::json!(2);
        let mut restored: Form2550QDraft =
            serde_json::from_value(json).expect("unknown version remains inspectable");
        restored
            .item_19_additional_rows
            .push(Form2550QAdditionalItemRow {
                instance_id: None,
                description: "PRESERVED ONLY".to_string(),
                amount: Some(10.0),
            });
        restored
            .ensure_repeating_row_ids()
            .expect("additional row receives identity");

        let errors = restored.validate();
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "raw_editor_state.version")
        );
        // A complete popup row is a valid item 19 line now.
        assert!(
            !errors
                .iter()
                .any(|(field, _)| field.starts_with("item_19_additional_rows"))
        );
    }

    #[test]
    fn inert_adapter_inventory_is_exactly_seven_groups_and_twenty_eight_members() {
        let groups = form_2550q_group_bindings();
        assert_eq!(groups.len(), 7);
        assert_eq!(
            groups
                .iter()
                .map(|binding| binding.group_id())
                .collect::<Vec<_>>(),
            vec![
                "schedule-1-capital-good-row",
                "schedule-3-creditable-vat-row",
                "schedule-4-advance-vat-row",
                "item-19-additional-row",
                "item-42-additional-row",
                "item-47-additional-row",
                "item-56-additional-row",
            ]
        );
        assert_eq!(
            groups
                .iter()
                .map(|binding| binding.members().len())
                .collect::<Vec<_>>(),
            vec![9, 5, 6, 2, 2, 2, 2]
        );
        assert_eq!(
            groups.iter().flat_map(|binding| binding.members()).count(),
            28
        );
        assert_eq!(
            groups
                .iter()
                .flat_map(|binding| binding.members())
                .map(|member| member.field_id())
                .collect::<Vec<_>>(),
            vec![
                "txtDatePurchase1",
                "txtSourceCode1",
                "txtDescription1",
                "txtAmountPurchase1",
                "txtInputTax1",
                "txtEstimatedLife1",
                "txtRecognizedLife1",
                "txtAllowedInputTax1",
                "txtBalanceInputTax1",
                "txtDateCovered3",
                "txtDateCovered3To",
                "txtNameWithHoldingAgent3",
                "txtIncomePayment3",
                "txtTotalTaxWithHeld3",
                "txtDate4",
                "txtDate4To",
                "txtNameOfMiller4",
                "txtNameOfTaxpayer4",
                "txtOfficialReceiptNumber4",
                "txtAmountPaid4",
                "frm2550qv2024:totalTaxPayableNo19Description",
                "frm2550qv2024:totalTaxPayableNo19Amount",
                "frm2550qv2024:totalTaxPayableNo42Description",
                "frm2550qv2024:totalTaxPayableNo42Amount",
                "frm2550qv2024:totalTaxPayableNo47Description",
                "frm2550qv2024:totalTaxPayableNo47Amount",
                "frm2550qv2024:totalTaxPayableNo56Description",
                "frm2550qv2024:totalTaxPayableNo56Amount",
            ]
        );
        assert_eq!(form_2550q_singleton_field_ids().len(), 66);
        for field_id in [
            "frm2550qv2024:txtTIN1",
            "frm2550qv2024:txtTIN2",
            "frm2550qv2024:txtTIN3",
            "frm2550qv2024:branchCode",
            "frm2550qv2024:txtRDOCode",
            "frm2550qv2024:taxpayerName",
            "frm2550qv2024:amendedReturnYesNo5",
            "frm2550qv2024:amendedReturnNo5",
            "frm2550qv2024:OptShortPrd1",
            "frm2550qv2024:OptShortPrd2",
        ] {
            assert!(
                form_2550q_singleton_field_ids().contains(&field_id),
                "candidate identity field {field_id} must have a closed raw binding"
            );
        }
        assert_eq!(form_2550q_local_print_raw_field_keys().len(), 20);
        assert_eq!(
            groups
                .iter()
                .filter(|binding| !binding.has_live_editor_controls())
                .map(|binding| binding.family())
                .collect::<Vec<_>>(),
            Vec::<Form2550QRowFamily>::new()
        );
    }

    #[test]
    fn inert_capture_omits_uncaptured_buffers_instead_of_formatting_typed_values() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.part_iv.item_31a_vatable_sales = Some(123.45);
        let source =
            Form2550QFieldValueSource::try_capture(&mut draft).expect("empty raw state captures");

        assert_eq!(source.snapshot().repeated_group_instances().len(), 6);
        assert!(
            source.snapshot().fields().is_empty(),
            "typed 123.45 must not be formatted into a raw buffer"
        );
        assert_eq!(
            source
                .gaps()
                .iter()
                .filter(|gap| matches!(gap, Form2550QCaptureGap::MissingRawBuffer { .. }))
                .count(),
            106
        );
        assert_eq!(
            source
                .gaps()
                .iter()
                .filter(|gap| matches!(gap, Form2550QCaptureGap::NoLiveEditorControls { .. }))
                .count(),
            0
        );
        assert_eq!(source.excluded_local_print_buffer_count(), 0);
    }

    #[test]
    fn inert_capture_preserves_raw_states_and_excludes_local_print_buffers() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft
            .raw_editor_state
            .set_singleton("frm2550qv2024:vatPaidReturn", RawValue::Absent);
        draft
            .raw_editor_state
            .set_singleton("frm2550qv2024:surcharge", RawValue::Text(String::new()));
        draft.raw_editor_state.set_singleton(
            "signatory",
            RawValue::Text("AUTHORIZED REPRESENTATIVE".to_string()),
        );
        let schedule_id = draft.schedule_1[0]
            .instance_id
            .clone()
            .expect("new row has identity");
        draft.raw_editor_state.set_repeated(
            Form2550QRowFamily::Schedule1,
            schedule_id.clone(),
            "txtInputTax1",
            RawValue::Text("not-a-number".to_string()),
        );
        draft.raw_editor_state.mark_malformed(
            format!("schedule-1/{schedule_id}/txtInputTax1"),
            "not a number",
        );

        let source =
            Form2550QFieldValueSource::try_capture(&mut draft).expect("raw state captures");
        assert_eq!(source.snapshot().fields().len(), 3);
        assert_eq!(source.excluded_local_print_buffer_count(), 1);
        assert_eq!(source.excluded_local_print_buffer_keys(), ["signatory"]);
        assert!(source.gaps().iter().any(|gap| matches!(
            gap,
            Form2550QCaptureGap::ExcludedLocalPrintRawBuffer {
                field_key: "signatory"
            }
        )));
        assert!(
            source
                .snapshot()
                .fields()
                .iter()
                .all(|raw| raw.field().field_id().as_str() != "signatory")
        );
        assert!(
            source
                .snapshot()
                .fields()
                .iter()
                .any(|raw| raw.value() == &RawValue::Absent)
        );
        assert!(
            source
                .snapshot()
                .fields()
                .iter()
                .any(|raw| raw.value() == &RawValue::Text(String::new()))
        );
        assert!(
            source
                .snapshot()
                .fields()
                .iter()
                .any(|raw| raw.value() == &RawValue::Text("not-a-number".to_string()))
        );

        let restored: Form2550QDraft = serde_json::from_value(
            serde_json::to_value(&draft).expect("draft with local print raw state serializes"),
        )
        .expect("draft with local print raw state deserializes");
        assert_eq!(
            restored.raw_editor_state.singleton_value("signatory"),
            Some(&RawValue::Text("AUTHORIZED REPRESENTATIVE".to_string()))
        );
    }

    #[test]
    fn inert_capture_is_stable_when_vectors_are_reordered() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let schedule_id = draft.schedule_4[1]
            .instance_id
            .clone()
            .expect("new row has identity");
        draft.raw_editor_state.set_repeated(
            Form2550QRowFamily::Schedule4,
            schedule_id,
            "txtOfficialReceiptNumber4",
            RawValue::Text("OR-123".to_string()),
        );
        let first =
            Form2550QFieldValueSource::try_capture(&mut draft).expect("first capture succeeds");

        draft.schedule_1.reverse();
        draft.schedule_3.reverse();
        draft.schedule_4.reverse();
        let reordered =
            Form2550QFieldValueSource::try_capture(&mut draft).expect("reordered capture succeeds");

        assert_eq!(first.snapshot(), reordered.snapshot());
        assert_eq!(first.gaps(), reordered.gaps());
    }

    #[test]
    fn inert_capture_rejects_unknown_raw_keys_versions_and_orphan_rows() {
        let mut unknown = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        unknown
            .raw_editor_state
            .set_singleton("future_unreviewed_field", RawValue::Text("x".to_string()));
        assert!(matches!(
            Form2550QFieldValueSource::try_capture(&mut unknown),
            Err(Form2550QCaptureError::UnexpectedSingletonField { .. })
        ));

        let versioned = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let mut json = serde_json::to_value(versioned).expect("draft serializes");
        json["raw_editor_state"]["version"] = serde_json::json!(2);
        let mut versioned: Form2550QDraft =
            serde_json::from_value(json).expect("unknown version stays inspectable");
        assert!(matches!(
            Form2550QFieldValueSource::try_capture(&mut versioned),
            Err(Form2550QCaptureError::RawEditorState(
                Form2550QRawEditorStateError::UnsupportedVersion { actual: 2 }
            ))
        ));

        let mut orphan = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        orphan.raw_editor_state.set_repeated(
            Form2550QRowFamily::Schedule1,
            StableInstanceId::parse("row-00000000000000000999").expect("valid row ID"),
            "txtInputTax1",
            RawValue::Text("10.00".to_string()),
        );
        assert!(matches!(
            Form2550QFieldValueSource::try_capture(&mut orphan),
            Err(Form2550QCaptureError::OrphanRepeatedInstance {
                family: Form2550QRowFamily::Schedule1,
                ..
            })
        ));
    }

    #[test]
    fn reviewed_sample_formula_chain_is_reproduced_without_guessed_penalties() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2025, 1);
        let p4 = &mut draft.part_iv;
        p4.item_31a_vatable_sales = Some(1_000.0);
        p4.item_32a_zero_rated_sales = Some(1_000.0);
        p4.item_33a_exempt_sales = Some(1_000.0);
        p4.item_35b_less_output_vat_uncollected = Some(1_000.0);
        p4.item_36b_add_output_vat_recovered = Some(1_000.0);
        p4.item_38b_input_tax_carried = Some(1_000.0);
        p4.item_40b_transitional_input_tax = Some(1_000.0);
        p4.item_41b_presumptive_input_tax = Some(1_000.0);
        p4.item_42b_other_input_tax = Some(10_000.0);
        p4.item_44a_domestic_purchases = Some(1_000.0);
        p4.item_44b_domestic_input_tax = Some(120.0);
        p4.item_45a_nonresident_services = Some(1_000.0);
        p4.item_45b_nonresident_service_input_tax = Some(120.0);
        p4.item_46a_importations = Some(10_000.0);
        p4.item_46b_import_input_tax = Some(1_200.0);
        p4.item_47a_other_purchases = Some(1_000.0);
        p4.item_47b_other_input_tax = Some(120.0);
        p4.item_48a_domestic_purchases_no_input_tax = Some(1_000.0);
        p4.item_49a_vat_exempt_importations = Some(1_000.0);
        p4.item_54b_vat_refund_or_tcc_claimed = Some(1_000.0);
        p4.item_55b_input_vat_on_unpaid_payables = Some(1_000.0);
        p4.item_56b_other_deduction = Some(1_000.0);
        p4.item_58b_input_vat_on_settled_payables = Some(10_000.0);
        draft.part_ii.item_19_other_credit_or_payment = Some(1_000.0);
        draft.part_ii.item_22_surcharge = Some(1_000.0);
        draft.part_ii.item_23_interest = Some(100.0);
        draft.part_ii.item_24_compromise = Some(200.0);
        draft.recompute();

        assert_eq!(
            draft.part_iv.item_50a_total_current_purchases,
            Some(15_000.0)
        );
        assert_eq!(
            draft.part_iv.item_61b_net_vat_payable_or_excess,
            Some(-1_440.0)
        );
        assert_eq!(
            draft.part_ii.item_21_tax_payable_or_excess_credits,
            Some(-2_440.0)
        );
        assert_eq!(
            draft.part_ii.item_26_total_amount_payable_or_excess,
            Some(1_300.0)
        );
    }

    #[test]
    fn invalid_or_blank_money_is_not_coerced_to_zero() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.part_iv.item_31a_vatable_sales = None;
        draft.recompute();
        let errors = draft.validate();
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "item_31a_vatable_sales")
        );
        assert_eq!(draft.part_iv.item_31b_output_tax, None);
    }

    #[test]
    fn item_47_description_is_required_for_either_column() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.part_iv.item_47a_other_purchases = Some(0.0);
        draft.part_iv.item_47b_other_input_tax = Some(10.0);
        draft.part_iv.item_47_description.clear();
        draft.recompute();

        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "item_47_description")
        );
    }

    #[test]
    fn schedule_4_placeholder_is_not_accepted_as_a_receipt_number() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.schedule_4[0].period_from = Some(Form2550QDate::new(2026, 1, 1).expect("valid date"));
        draft.schedule_4[0].period_to = Some(Form2550QDate::new(2026, 3, 31).expect("valid date"));
        draft.schedule_4[0].miller_name = "MILLER".to_string();
        draft.schedule_4[0].taxpayer_name = "TAXPAYER".to_string();
        draft.schedule_4[0].amount_paid = Some(10.0);
        draft.recompute();

        assert!(
            draft
                .validate()
                .iter()
                .any(|(field, _)| field == "schedule_4[0]")
        );
    }

    #[test]
    fn queue_transition_runs_the_generic_lifecycle_gate() {
        assert!(QUEUE_SUBMISSION_SUPPORTED);
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.tin = "not-a-tin".to_string();
        let result = draft.transition_to_queued();
        assert!(result.is_err(), "an invalid draft must not queue");
        assert_eq!(draft.lifecycle.status, FilingStatus::Draft);
        assert!(draft.lifecycle.queued_submission_fingerprint.is_none());
        assert!(draft.lifecycle.queue_authorization.is_none());
    }

    #[test]
    fn stored_lifecycle_json_keeps_its_field_names() {
        let draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let json = serde_json::to_value(&draft).expect("serializes");
        let object = json.as_object().expect("object");
        for key in ["status", "created_at", "updated_at", "submission_attempts"] {
            assert!(object.contains_key(key), "{key} stays top-level");
        }
        assert!(!object.contains_key("lifecycle"));
        let back: Form2550QDraft = serde_json::from_value(json).expect("deserializes");
        assert_eq!(back.lifecycle, draft.lifecycle);
        assert!(back.legacy_flat_draft_fields.is_empty());
    }

    #[test]
    fn queueable_period_code_matches_create_xml_file_name() {
        use crate::forms::queueable::QueueableForm;
        let draft = Form2550QDraft::new_from_profile(&profile(), 2026, 2);
        assert_eq!(draft.period_code(), "122026Q2");
        assert_eq!(
            Form2550QDraft::parse_period_code("122026Q2"),
            Some((2026, FilingPeriod::Quarterly(2)))
        );
        assert!(Form2550QDraft::parse_period_code("122026Q5").is_none());
    }

    fn capital_row(year: u16, input_tax: f64, life: u16) -> Form2550QCapitalGoodRow {
        Form2550QCapitalGoodRow {
            purchase_or_import_date: Form2550QDate::new(year, 1, 15).ok(),
            source_code: "D".to_string(),
            description: "EQUIPMENT".to_string(),
            purchase_or_import_amount: Some(input_tax * 10.0),
            input_tax: Some(input_tax),
            estimated_life_months: Some(life),
            recognized_life_months: Some(life),
            ..Form2550QCapitalGoodRow::default()
        }
    }

    fn errors_for(draft: &Form2550QDraft, field: &str) -> Vec<String> {
        draft
            .validate()
            .into_iter()
            .filter(|(key, _)| key == field)
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn schedule_1_purchase_after_2021_uses_the_official_alert() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.schedule_1 = vec![capital_row(2022, 1_200.0, 60)];
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert_eq!(
            errors_for(&draft, "schedule_1[0].purchase_or_import_date"),
            vec!["Only dates on or before December 31, 2021 are allowed.".to_string()]
        );
    }

    #[test]
    fn schedule_1_estimated_life_over_60_uses_the_official_alert() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.schedule_1 = vec![capital_row(2021, 1_200.0, 61)];
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert_eq!(
            errors_for(&draft, "schedule_1[0].estimated_life_months"),
            vec!["Invalid Input it should be 1 to 60".to_string()]
        );
    }

    #[test]
    fn schedule_1_blank_date_uses_the_official_alerts() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let mut row = capital_row(2021, 1_200.0, 60);
        row.purchase_or_import_date = None;
        draft.schedule_1 = vec![row];
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert_eq!(
            errors_for(&draft, "schedule_1[0].purchase_or_import_date"),
            vec!["Date Purchase cannot be an empty field.".to_string()]
        );
        assert_eq!(
            errors_for(&draft, "schedule_1[0]"),
            vec!["Empty fields are not allowed ".to_string()]
        );
    }

    #[test]
    fn schedule_1_source_code_uses_the_official_alert() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let mut row = capital_row(2021, 1_200.0, 60);
        row.source_code = "X".to_string();
        draft.schedule_1 = vec![row];
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert_eq!(
            errors_for(&draft, "schedule_1[0].source_code"),
            vec!["Invalid input\nAccept only I and D ".to_string()]
        );
    }

    fn creditable_row(from: Form2550QDate, to: Form2550QDate) -> Form2550QCreditableVatRow {
        Form2550QCreditableVatRow {
            period_from: Some(from),
            period_to: Some(to),
            withholding_agent_name: "AGENT".to_string(),
            income_payment: Some(1_000.0),
            tax_withheld: Some(50.0),
            ..Form2550QCreditableVatRow::default()
        }
    }

    #[test]
    fn schedule_3_dates_outside_the_return_period_use_the_official_alert() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let start = draft.return_period_from.unwrap();
        let end = draft.return_period_to.unwrap();
        draft.schedule_3 = vec![creditable_row(
            Form2550QDate::new(2025, 12, 1).unwrap(),
            end,
        )];
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert_eq!(
            errors_for(&draft, "schedule_3[0].period"),
            vec![format!(
                "Dates must fall within the Return Period: {start} to {end}"
            )]
        );
    }

    #[test]
    fn schedule_3_from_after_to_uses_the_official_alert() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let start = draft.return_period_from.unwrap();
        let end = draft.return_period_to.unwrap();
        draft.schedule_3 = vec![creditable_row(end, start)];
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert_eq!(
            errors_for(&draft, "schedule_3[0].period"),
            vec!["'From' date cannot be after 'To' date.".to_string()]
        );
    }

    #[test]
    fn schedule_3_year_2000_or_earlier_uses_the_official_alert() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        let (start, end) = (
            draft.return_period_from.unwrap(),
            draft.return_period_to.unwrap(),
        );
        draft.schedule_3 = vec![creditable_row(start, end)];
        draft.taxable_year = 2000;
        assert_eq!(
            errors_for(&draft, "schedule_3"),
            vec![
                "Please  enter a valid year for the Return Period. Only values 2000 and beyond will be accepted."
                    .to_string()
            ]
        );
    }

    #[test]
    fn schedule_1_allowable_and_balance_follow_compute_sched1() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.schedule_1 = vec![capital_row(2019, 30_000.06, 40)];
        draft.recompute();
        let row = &draft.schedule_1[0];
        assert_eq!(row.allowable_input_tax_for_period, Some(2_250.0));
        assert_eq!(row.balance_to_next_period, Some(27_750.06));
    }

    #[test]
    fn schedules_hold_any_number_of_rows_and_write_the_modal_layout() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.schedule_1 = (0..5).map(|_| capital_row(2021, 1_200.0, 60)).collect();
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert!(
            draft
                .validate()
                .iter()
                .all(|(field, _)| field != "schedules" && !field.starts_with("schedule_1")),
            "{:?}",
            draft.validate()
        );
        let fields = draft.official_control_map();
        assert_eq!(fields["txtDatePurchase14"], "1/15/2021");
        assert_eq!(fields["txtEstimatedLife14"], "60");
        assert_eq!(fields["txtAllowedInputTax14"], "60.00");
        // Unused schedules keep the disabled static rows at page defaults.
        assert!(!fields.contains_key("txtDate40"));
        let layout = draft.official_layout().unwrap();
        let keys: Vec<String> = layout
            .entries
            .iter()
            .map(|entry| match entry {
                crate::official_xml::Entry::Bool { key, .. }
                | crate::official_xml::Entry::Value { key, .. } => key.clone(),
            })
            .collect();
        let anchor = keys
            .iter()
            .position(|key| key == "txtTotalAmountOfBalanceofInputTaxFromPrevious")
            .unwrap();
        assert_eq!(keys[anchor - 1], "txtBalanceInputTax14");
        assert_eq!(
            keys.iter()
                .filter(|key| key.starts_with("txtDatePurchase1"))
                .count(),
            5
        );
        assert!(keys.iter().any(|key| key == "txtDate40"));
        crate::official_xml::write(&layout, &fields).unwrap();
    }

    #[test]
    fn removing_a_row_forgets_its_raw_buffers() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.schedule_3.push(Form2550QCreditableVatRow::default());
        draft.ensure_repeating_row_ids().unwrap();
        let id = draft.schedule_3[2].instance_id.clone().unwrap();
        draft.raw_editor_state.set_repeated(
            Form2550QRowFamily::Schedule3,
            id.clone(),
            "txtNameWithHoldingAgent3",
            RawValue::Text("AGENT".to_string()),
        );
        draft.schedule_3.pop();
        draft
            .raw_editor_state
            .remove_repeated_row(Form2550QRowFamily::Schedule3, &id);
        assert!(errors_for(&draft, "raw_editor_state.bindings").is_empty());
    }

    fn specify_row(description: &str, amount: f64) -> Form2550QAdditionalItemRow {
        Form2550QAdditionalItemRow {
            instance_id: None,
            description: description.to_string(),
            amount: Some(amount),
        }
    }

    #[test]
    fn specify_popup_save_with_an_incomplete_row_uses_the_official_alert() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.item_42_additional_rows = vec![specify_row("INPUT A", 10.0), specify_row("", 5.0)];
        draft.item_56_additional_rows = vec![specify_row("DEDUCTION", 0.0)];
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert_eq!(
            errors_for(&draft, "item_42_additional_rows[1]"),
            vec![SPECIFY_ROW_SAVE_ALERT.to_string()]
        );
        assert_eq!(
            errors_for(&draft, "item_56_additional_rows[0]"),
            vec![
                "Please fill up all required fields and ensure amounts are greater than zero."
                    .to_string()
            ]
        );
        assert!(errors_for(&draft, "item_42_additional_rows[0]").is_empty());
    }

    #[test]
    fn specify_popup_add_row_with_an_empty_field_uses_the_official_alert() {
        assert_eq!(
            specify_add_row_alert(&[specify_row("CREDIT", 1.0), specify_row("", 2.0)]),
            Some("Empty fields are not allowed")
        );
        assert_eq!(
            specify_add_row_alert(&[specify_row("CREDIT", 0.0)]),
            Some("Empty fields are not allowed")
        );
        assert_eq!(specify_add_row_alert(&[specify_row("CREDIT", 1.0)]), None);
        assert_eq!(specify_add_row_alert(&[]), None);
    }

    #[test]
    fn specify_popup_rows_set_various_total_and_47b_like_close_modal() {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2026, 1);
        draft.item_19_additional_rows = vec![specify_row("A", 100.0), specify_row("B", 2_200.5)];
        draft.item_47_additional_rows = vec![
            specify_row("P1", 5_000.0),
            specify_row("P2", 1_234.5),
            specify_row("P3", 10.05),
        ];
        draft.item_56_additional_rows = vec![specify_row("ONLY", 45.67)];
        draft.ensure_repeating_row_ids().unwrap();
        draft.recompute();
        assert_eq!(draft.part_ii.item_19_description, "VARIOUS");
        assert_eq!(draft.part_ii.item_19_other_credit_or_payment, Some(2_300.5));
        assert_eq!(draft.part_iv.item_47a_other_purchases, Some(6_244.55));
        assert_eq!(draft.part_iv.item_47b_other_input_tax, Some(749.35));
        assert_eq!(draft.part_iv.item_56_description, "ONLY");
        assert_eq!(draft.part_iv.item_56b_other_deduction, Some(45.67));

        let fields = draft.official_control_map();
        assert_eq!(fields["frm2550qv2024:totalTaxPayableNo19Description1"], "B");
        assert_eq!(
            fields["frm2550qv2024:totalTaxPayableNo19Amount1"],
            "2,200.50"
        );
        assert_eq!(fields["resultOtherCreditsNo19"], "2,300.50");
        assert_eq!(fields["frm2550qv2024:totalTaxPayableNo56Amount0"], "45.67");
        assert_eq!(fields["resultOtherCreditsNo42"], "0.00");
        let layout = draft.official_layout().unwrap();
        let keys: Vec<String> = layout
            .entries
            .iter()
            .map(|entry| match entry {
                crate::official_xml::Entry::Bool { key, .. }
                | crate::official_xml::Entry::Value { key, .. } => key.clone(),
            })
            .collect();
        let anchor = keys
            .iter()
            .position(|key| key == "resultOtherCreditsNo47")
            .unwrap();
        assert_eq!(keys[anchor - 1], "frm2550qv2024:totalTaxPayableNo47Amount2");
        assert!(!keys.iter().any(|key| key.contains("totalTaxPayableNo42")));
        crate::official_xml::write(&layout, &fields).unwrap();
    }
}
