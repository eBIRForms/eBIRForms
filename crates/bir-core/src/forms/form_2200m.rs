//! BIR Form 2200M — Excise Tax Return for Mineral Products (v2018).
//!
//! Ported from the official `BIR-Form2200Mv2018.hta` (eBIRForms 7.9.6.2.1):
//! the Schedule 1 total (`computeTotalTaxDue`), the Part III chain
//! (`partThreeComputation`), `validateAll` with its exact alert texts
//! (including `validateDate`, `validateEmail` and `checkPartVFields`), and
//! `saveXMLsubmit` through [`crate::official_xml`].
//!
//! Schedule 1 has ten official rows: rows 1–8 carry fixed ATCs, descriptions
//! and rates (XM010 ×3, XM020, XM030, XM040, XM050, XM051); rows 9 and 10 are
//! XM060 "others" rows where the filer types the description and both rates.
//! "Add row" (`addRowSched1`) appends more XM060 rows; each of their cells
//! lands at the end of its own column table in the page's DOM, so the upload
//! carries them there (see [`Form2200MDraft::official_layout`]).
//!
//! Every Schedule 1 cell is free filer input: the official page computes no
//! row arithmetic. Item 16 is the sum of column F (locally extracted tax due)
//! over all ten rows — exactly what `computeTotalTaxDue` adds; the imported
//! and "Total Tax Due" columns are carried but not totalled by the official
//! page either.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_2200M_FORM_ID: &str = "2200m-v2018";
/// Official Schedule 1 rows in the submit layout.
pub const FORM_2200M_SCHEDULE_ROWS: usize = 10;
/// Upper bound on rows the editor lets the filer add (the page has none).
pub const FORM_2200M_MAX_ROWS: usize = 200;
/// Column controls of a Schedule 1 row, in their own column tables; an added
/// row's control follows the previous row's in the same table.
const SCHEDULE_COLUMNS: [&str; 15] = [
    "txtSched1_PlaceOfRemoval",
    "txtSched1_VOMRITaxableA",
    "txtSched1_VOMRIExemptB",
    "txtSched1_LocallyExtractedRateC",
    "txtSched1_LocallyExtractedRateD",
    "txtSched1_LocallyExtractedTaxRateE",
    "txtSched1_LocallyExtractedTaxDueF",
    "txtSched1_descriptionCont",
    "txtSched1_ImportedTaxableG",
    "txtSched1_ImportedExemptH",
    "txtSched1_ImportedTaxRate",
    "txtSched1_ImportedTaxDue",
    "txtSched1_TaxDueAdjustment",
    "txtSched1_TotalTaxDue",
    "txtSched1ATC",
];
/// Rows 9 and 10 (index 8 and 9) take a filer description and rates.
pub const FORM_2200M_FIRST_OTHER_ROW: usize = 8;

/// Fixed rows 1–8 of Schedule 1: (ATC, description, rate text).
pub const FORM_2200M_FIXED_ROWS: [(&str, &str, &str); 8] = [
    ("XM010", "Coal and Coke - January 1, 2018", "P 50/MT"),
    ("XM010", "Coal and Coke - January 1, 2019", "P 100/MT"),
    (
        "XM010",
        "Coal and Coke - January 1, 2020 and onwards",
        "P 150/MT",
    ),
    ("XM020", "Non-metallic minerals and quarry resources", "4%"),
    ("XM030", "Copper and other Metallic Minerals", "4%"),
    ("XM040", "Gold and Chromite", "4%"),
    ("XM050", "Indigenous Petroleum", "6%"),
    (
        "XM051",
        "Natural Gas or Liquefied Natural Gas (Locally extracted)",
        "0%",
    ),
];
/// ATC of rows 9 and 10.
pub const FORM_2200M_OTHER_ATC: &str = "XM060";

/// Items 10 and 11: region, province and city codes from the official
/// dropdowns (`xml/region.xml`, `xml/province.xml`, `loadArrayCity`). Blank
/// means the `(Select …)` placeholder, `00`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Form2200MPlace {
    pub region: String,
    pub province: String,
    pub city: String,
}

impl Form2200MPlace {
    fn code(value: &str) -> String {
        if value.trim().is_empty() {
            "00".to_string()
        } else {
            value.trim().to_string()
        }
    }
}

/// Part II — Manner of Payment (Items 13–15, one radio group).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2200MPayment {
    #[default]
    None,
    /// Item 13 — Payment on Actual Removal.
    ActualRemoval,
    /// Item 14 — Prepayment / Advance Deposit.
    Prepayment,
    /// Item 15 — Other Similar Schemes (only reachable after Item 14 on the
    /// official page, which enables the radio).
    OtherScheme,
}

/// One Schedule 1 row. Blank cells stay blank in the submit plaintext; a
/// typed cell holds the `round(this)` text (`1,234.50`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200MRow {
    /// Rows 9–10 only (`txtSched1_description9/10`, max 20 characters).
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub place_of_removal: String,
    /// Volume removed/imported (MT): A taxable, B exempt.
    #[serde(default)]
    pub volume_taxable: Option<f64>,
    #[serde(default)]
    pub volume_exempt: Option<f64>,
    /// Locally extracted, actual/fair market value: C taxable, D exempt.
    #[serde(default)]
    pub local_value_taxable: Option<f64>,
    #[serde(default)]
    pub local_value_exempt: Option<f64>,
    /// E — rows 9–10 only; rows 1–8 print their fixed rate.
    #[serde(default)]
    pub local_rate: Option<f64>,
    /// F — locally extracted tax due (summed into Item 16).
    #[serde(default)]
    pub local_tax_due: Option<f64>,
    /// Imported, value used by BOC: G taxable, H exempt.
    #[serde(default)]
    pub imported_taxable: Option<f64>,
    #[serde(default)]
    pub imported_exempt: Option<f64>,
    /// I — rows 9–10 only.
    #[serde(default)]
    pub imported_rate: Option<f64>,
    /// J — imported tax due.
    #[serde(default)]
    pub imported_tax_due: Option<f64>,
    /// K — tax due adjustment per final value.
    #[serde(default)]
    pub adjustment: Option<f64>,
    #[serde(default)]
    pub total_tax_due: Option<f64>,
}

impl Form2200MRow {
    /// The cells `checkPartVFields` tests for "row used" (all but the rates).
    fn amount_cells(&self) -> [Option<f64>; 10] {
        [
            self.volume_taxable,
            self.volume_exempt,
            self.local_value_taxable,
            self.local_value_exempt,
            self.local_tax_due,
            self.imported_taxable,
            self.imported_exempt,
            self.imported_tax_due,
            self.adjustment,
            self.total_tax_due,
        ]
    }

    fn cells_mut(&mut self) -> [&mut Option<f64>; 12] {
        [
            &mut self.volume_taxable,
            &mut self.volume_exempt,
            &mut self.local_value_taxable,
            &mut self.local_value_exempt,
            &mut self.local_rate,
            &mut self.local_tax_due,
            &mut self.imported_taxable,
            &mut self.imported_exempt,
            &mut self.imported_rate,
            &mut self.imported_tax_due,
            &mut self.adjustment,
            &mut self.total_tax_due,
        ]
    }

    pub fn is_blank(&self) -> bool {
        self.description.trim().is_empty()
            && self.place_of_removal.trim().is_empty()
            && self.amount_cells().iter().all(Option::is_none)
            && self.local_rate.is_none()
            && self.imported_rate.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2200MDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–3
    /// Item 1 date (MM/DD/YYYY).
    pub month: u8,
    pub day: u8,
    pub year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I — Background Information (loaded from the taxpayer profile)
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub rdo_code: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,
    /// `txtLOB`: the profile's main line of business, sent as stored.
    #[serde(default)]
    pub line_of_business: String,
    #[serde(default)]
    pub place_of_production: Form2200MPlace,
    #[serde(default)]
    pub place_of_removal: Form2200MPlace,
    /// Item 12: `None` until the filer answers.
    #[serde(default)]
    pub tax_relief: Option<bool>,
    #[serde(default)]
    pub tax_relief_specify: String,

    // Part II
    #[serde(default)]
    pub manner_of_payment: Form2200MPayment,
    #[serde(default)]
    pub other_scheme_description: String,

    // Part V — Schedule 1 (rows 1–10)
    #[serde(default)]
    pub schedule: Vec<Form2200MRow>,
    /// `TotalExciseTaxDue`: sum of column F.
    #[serde(default)]
    pub schedule_total: f64,

    // Part III
    /// Item 16.
    #[serde(default)]
    pub excise_tax_due: f64,
    /// Item 17A.
    #[serde(default)]
    pub balance_carried_over: f64,
    /// Item 17B.
    #[serde(default)]
    pub creditable_excise_tax: f64,
    /// Item 17C.
    #[serde(default)]
    pub total_credits: f64,
    /// Item 18.
    #[serde(default)]
    pub net_tax_due: f64,
    /// Item 19, amended returns only.
    #[serde(default)]
    pub previous_payment: f64,
    /// Item 20.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 21A–21D.
    #[serde(default)]
    pub surcharge: f64,
    #[serde(default)]
    pub interest: f64,
    #[serde(default)]
    pub compromise: f64,
    /// The penalty items' computed values, overrides, note and engine
    /// inputs (the shared model in [`super::penalty_items`]).
    #[serde(flatten)]
    pub penalty: PenaltyState,
    #[serde(default)]
    pub total_penalties: f64,
    /// Item 22.
    #[serde(default)]
    pub amount_payable: f64,
    /// Item 23A — tax payment/deposit made today.
    #[serde(default)]
    pub tax_deposit: f64,
    /// Item 23B (= Item 21D).
    #[serde(default)]
    pub penalties_paid: f64,
    /// Item 23C.
    #[serde(default)]
    pub total_payment: f64,
    /// Item 24.
    #[serde(default)]
    pub balance_to_carry_over: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `round(this)` / `formatCurrency`: the value the official field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// `isAmountWithinAllowedPrecision`: at most 12 integer digits, else the
/// official field resets to `0.00`.
fn within_official_precision(value: f64) -> bool {
    value.is_finite() && value.abs() < 1e12
}

fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// `capitalize()`: trimmed and uppercased.
fn capitalize(value: &str) -> String {
    value.trim().to_uppercase()
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

fn optional_amount(value: Option<f64>) -> String {
    value.map(official_amount).unwrap_or_default()
}

fn last_day_of_month(year: u16, month: u8) -> u8 {
    let (next_year, next_month) = if month >= 12 {
        (i32::from(year) + 1, 1)
    } else {
        (i32::from(year), u32::from(month) + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first| first.pred_opt())
        .map(|last| last.day() as u8)
        .unwrap_or(28)
}

/// `validateEmail`'s pattern, searched (not anchored) like `String.match`.
fn email_matches_official_pattern(email: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN
        .get_or_init(|| {
            regex::Regex::new(r"\b[a-zA-Z0-9._%+-]+@(?:[a-zA-Z0-9-]+\.)+[a-zA-Z]{2,4}\b")
                .expect("official email pattern")
        })
        .is_match(email)
}

impl Form2200MDraft {
    pub const FORM_CODE: &'static str = "2200M";

    /// A new return for the dashboard month. Item 1 defaults to the last day
    /// of that month, or today when the month is the current one.
    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        let month = month.clamp(1, 12);
        let today = chrono::Local::now().date_naive();
        let day = if i32::from(year) == today.year() && u32::from(month) == today.month() {
            today.day() as u8
        } else {
            last_day_of_month(year, month)
        };
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile_typed_due_date(profile),
            id: None,
            month,
            day,
            year,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            line_of_business: profile.line_of_business.clone(),
            place_of_production: Form2200MPlace::default(),
            place_of_removal: Form2200MPlace::default(),
            tax_relief: None,
            tax_relief_specify: String::new(),
            manner_of_payment: Form2200MPayment::None,
            other_scheme_description: String::new(),
            schedule: Vec::new(),
            schedule_total: 0.0,
            excise_tax_due: 0.0,
            balance_carried_over: 0.0,
            creditable_excise_tax: 0.0,
            total_credits: 0.0,
            net_tax_due: 0.0,
            previous_payment: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            amount_payable: 0.0,
            tax_deposit: 0.0,
            penalties_paid: 0.0,
            total_payment: 0.0,
            balance_to_carry_over: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Row `index` (0-based), growing the schedule to reach it.
    pub fn row_mut(&mut self, index: usize) -> Option<&mut Form2200MRow> {
        if index >= FORM_2200M_MAX_ROWS {
            return None;
        }
        if self.schedule.len() <= index {
            self.schedule.resize_with(index + 1, Form2200MRow::default);
        }
        self.schedule.get_mut(index)
    }

    pub fn row(&self, index: usize) -> Form2200MRow {
        self.schedule.get(index).cloned().unwrap_or_default()
    }

    /// The official compute chain: every typed amount held at cents the way
    /// `round(this)` leaves it, Item 16 from `computeTotalTaxDue`, then
    /// `partThreeComputation` over the formatted values.
    pub fn recompute(&mut self) {
        // isAmended(): "No" resets and disables Item 19.
        if !self.is_amended {
            self.previous_payment = 0.0;
        }
        // availTaxRelief() / modeOfPayment() clear the disabled text fields.
        if self.tax_relief != Some(true) {
            self.tax_relief_specify.clear();
        }
        if self.manner_of_payment != Form2200MPayment::OtherScheme {
            self.other_scheme_description.clear();
        }
        while self.schedule.last().is_some_and(Form2200MRow::is_blank) {
            self.schedule.pop();
        }
        let mut total = 0.0;
        for (index, row) in self.schedule.iter_mut().enumerate() {
            if index < FORM_2200M_FIRST_OTHER_ROW {
                // Fixed rows have no description or editable rate controls.
                row.description.clear();
                row.local_rate = None;
                row.imported_rate = None;
            }
            for cell in row.cells_mut() {
                *cell = cell.map(cents);
            }
            total += row.local_tax_due.unwrap_or(0.0);
        }
        self.schedule_total = cents(total);
        self.excise_tax_due = self.schedule_total;

        self.balance_carried_over = cents(self.balance_carried_over);
        self.creditable_excise_tax = cents(self.creditable_excise_tax);
        self.total_credits = cents(self.balance_carried_over + self.creditable_excise_tax);
        self.net_tax_due = cents(self.excise_tax_due - self.total_credits);
        self.previous_payment = cents(self.previous_payment);
        self.tax_still_due = cents(self.net_tax_due - self.previous_payment);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.amount_payable = cents(self.tax_still_due + self.total_penalties);
        self.tax_deposit = cents(self.tax_deposit);
        self.penalties_paid = self.total_penalties;
        self.total_payment = cents(self.tax_deposit + self.penalties_paid);
        self.balance_to_carry_over = cents(self.amount_payable - self.total_payment);
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the page 2 header TIN, one 14-digit comb.
    /// - `derived:line:item{10,11}_{region,province,city}`: Items 10 and 11
    ///   as place names (the upload carries the PSGC codes); a name longer
    ///   than its comb prints whole in smaller type.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm2200M:txtPg2TinC1",
                "frm2200M:txtPg2TinC2",
                "frm2200M:txtPg2TinC3",
                "frm2200M:txtPg2TinC4",
            ],
        );
        fields.insert("derived:tin_digits".to_string(), tin_digits);
        for (item, place) in [
            (10, &self.place_of_production),
            (11, &self.place_of_removal),
        ] {
            let names = super::print_map::place_names(&place.region, &place.province, &place.city);
            for (level, name) in ["region", "province", "city"].into_iter().zip(names) {
                fields.insert(format!("derived:line:item{item}_{level}"), name);
            }
        }
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2200M:{key}"), value);
        };
        let flag = |on: bool| on.to_string();

        put("txtPg1I1Month", format!("{:02}", self.month));
        put("txtPg1I1Day", format!("{:02}", self.day));
        put("txtPg1I1Year", self.year.to_string());
        put("rdoPg1I2AmendedYes", flag(self.is_amended));
        put("rdoPg1I2AmendedNo", flag(!self.is_amended));
        put(
            "txtPg1I3NoSheets",
            self.number_of_attached_sheets.to_string(),
        );

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        for (page1, page2, value) in [
            ("txtPg1TIN1", "txtPg2TinC1", &tin1),
            ("txtPg1TIN2", "txtPg2TinC2", &tin2),
            ("txtPg1TIN3", "txtPg2TinC3", &tin3),
            ("txtPg1TIN4", "txtPg2TinC4", &branch),
        ] {
            put(page1, value.clone());
            put(page2, value.clone());
        }
        let rdo = self.rdo_code.trim().to_string();
        put("txtPg1I5RDO", rdo.clone());
        put("rdoPg1Pt1I6RDO", rdo);
        // loadBGData uppercases the profile name and address and splits the
        // address at 80 characters into Item 7 and its second line.
        let name = capitalize(&self.taxpayer_name);
        put("txtPg1Pt1I6RegisteredName", name.clone());
        put("txtPg2RegisteredName", name);
        let address: Vec<char> = capitalize(&self.registered_address).chars().collect();
        let line = |range: std::ops::Range<usize>| -> String {
            address
                .get(range.start.min(address.len())..range.end.min(address.len()))
                .map(|chars| chars.iter().collect())
                .unwrap_or_default()
        };
        put("txtPg1Pt1I7RegisteredAddress", line(0..80));
        put("txtPg1Pt1I7RegisteredAddress2", line(80..160));
        put("txPg1I7ZipCode", self.zip_code.trim().to_string());
        put(
            "txtPg1Pt1I8ContactNumber",
            self.contact_number.trim().to_string(),
        );
        put("txtPg1Pt1I9Email", self.email.trim().to_string());
        for (item, place) in [
            ("10", &self.place_of_production),
            ("11", &self.place_of_removal),
        ] {
            put(
                &format!("txtPg1Pt1I{item}Region"),
                Form2200MPlace::code(&place.region),
            );
            put(
                &format!("txtPg1Pt1I{item}Province"),
                Form2200MPlace::code(&place.province),
            );
            put(
                &format!("txtPg1Pt1I{item}City"),
                Form2200MPlace::code(&place.city),
            );
        }
        put("rdoPg1I12TaxReliefYes", flag(self.tax_relief == Some(true)));
        put("rdoPg1I12TaxReliefNo", flag(self.tax_relief == Some(false)));
        put(
            "txtPg1I12TaxReliefSpecify",
            capitalize(&self.tax_relief_specify),
        );
        let mop = self.manner_of_payment;
        put(
            "rdoPg1Pt2MOPPaymentActual",
            flag(mop == Form2200MPayment::ActualRemoval),
        );
        put(
            "rdoPg1Pt2MOPPrepayment",
            flag(mop == Form2200MPayment::Prepayment),
        );
        put(
            "rdoPg1Pt2MOPOther",
            flag(mop == Form2200MPayment::OtherScheme),
        );
        put(
            "txtPg1Pt2MOPOtherDesc",
            capitalize(&self.other_scheme_description),
        );

        for (key, value) in [
            ("txtPg1P3I16ExciseTaxDue", self.excise_tax_due),
            ("txtPg1P3I17ABalCarriedOver", self.balance_carried_over),
            ("txtPg1P3I17BCredExciseTax", self.creditable_excise_tax),
            ("txtPg1P3I17CTotal", self.total_credits),
            ("txtPg1P3I18NetTaxDue", self.net_tax_due),
            ("txtPg1P3I19PmntOnRtrnPrevFiled", self.previous_payment),
            ("txtPg1P3I20TaxStillDue", self.tax_still_due),
            ("txtPg1P3I21ASurcharge", self.surcharge),
            ("txtPg1P3I21BInterest", self.interest),
            ("txtPg1P3I21CCompromise", self.compromise),
            ("txtPg1P3I21DTotPenalties", self.total_penalties),
            ("txtPg1P3I22AmountPayable", self.amount_payable),
            ("txtPg1P3I23ATaxPmntDposit", self.tax_deposit),
            ("txtPg1P3I23BPenalties", self.penalties_paid),
            ("txtPg1P3I23CTotPmntMade", self.total_payment),
            ("txtPg1P3I24BalToCarryOver", self.balance_to_carry_over),
        ] {
            put(key, official_amount(value));
        }

        for index in 0..self.row_count() {
            let n = index + 1;
            let row = self.row(index);
            put(
                &format!("txtSched1_PlaceOfRemoval{n}"),
                capitalize(&row.place_of_removal),
            );
            put(
                &format!("txtSched1_VOMRITaxableA{n}"),
                optional_amount(row.volume_taxable),
            );
            put(
                &format!("txtSched1_VOMRIExemptB{n}"),
                optional_amount(row.volume_exempt),
            );
            // Row 1's column C control has the id `LocallyExtractedRate1`
            // (its name is `txtSched1_LocallyExtractedRateC1`).
            let column_c = if n == 1 {
                "LocallyExtractedRate1".to_string()
            } else {
                format!("txtSched1_LocallyExtractedRateC{n}")
            };
            put(&column_c, optional_amount(row.local_value_taxable));
            put(
                &format!("txtSched1_LocallyExtractedRateD{n}"),
                optional_amount(row.local_value_exempt),
            );
            put(
                &format!("txtSched1_LocallyExtractedTaxDueF{n}"),
                optional_amount(row.local_tax_due),
            );
            put(
                &format!("txtSched1_ImportedTaxableG{n}"),
                optional_amount(row.imported_taxable),
            );
            put(
                &format!("txtSched1_ImportedExemptH{n}"),
                optional_amount(row.imported_exempt),
            );
            put(
                &format!("txtSched1_ImportedTaxDue{n}"),
                optional_amount(row.imported_tax_due),
            );
            put(
                &format!("txtSched1_TaxDueAdjustment{n}"),
                optional_amount(row.adjustment),
            );
            put(
                &format!("txtSched1_TotalTaxDue{n}"),
                optional_amount(row.total_tax_due),
            );
            if index >= FORM_2200M_FIRST_OTHER_ROW {
                let description = capitalize(&row.description);
                put(&format!("txtSched1_description{n}"), description.clone());
                // populateTable2Desc copies it to the continuation table.
                put(&format!("txtSched1_descriptionCont{n}"), description);
                put(&format!("chkBoxSched1Description{n}"), flag(false));
                if index >= FORM_2200M_SCHEDULE_ROWS {
                    // sched1RowTemplate gives every added row the XM060 ATC.
                    put(
                        &format!("txtSched1ATC{n}"),
                        FORM_2200M_OTHER_ATC.to_string(),
                    );
                }
                put(
                    &format!("txtSched1_LocallyExtractedTaxRateE{n}"),
                    optional_amount(row.local_rate),
                );
                put(
                    &format!("txtSched1_ImportedTaxRate{n}"),
                    optional_amount(row.imported_rate),
                );
            }
        }
        put(
            "TotalExciseTaxDue",
            if self.schedule_total != 0.0 {
                official_amount(self.schedule_total)
            } else {
                String::new()
            },
        );
        // addRowSched1 counts the rows it adds.
        put(
            "sched1AddedRowCount",
            (self.row_count() - FORM_2200M_SCHEDULE_ROWS).to_string(),
        );
        put("txtLOB", self.line_of_business.clone());
        // sleeptime() sets the pager to page 1 at load.
        put("txtCurrentPage", "1".to_string());
        fields
    }

    /// Rows on the page: the ten it loads with plus any the filer added.
    pub fn row_count(&self) -> usize {
        self.schedule.len().max(FORM_2200M_SCHEDULE_ROWS)
    }

    /// The generated layout with every added row's controls spliced in after
    /// the previous row's control of the same column table (the description
    /// table also holds each row's delete checkbox), as `sched1RowTemplate`
    /// appends them.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        use super::official_inputs::{RowInsertion, extend_layout};
        let base = crate::official_xml::layout(FORM_2200M_FORM_ID).map_err(|e| e.to_string())?;
        let key = |column: &str, n: usize| format!("frm2200M:{column}{n}");
        let last = FORM_2200M_SCHEDULE_ROWS;
        let mut insertions = Vec::new();
        for n in (last + 1)..=self.row_count() {
            insertions.push(RowInsertion {
                after: key("txtSched1_description", n - 1),
                copies: vec![
                    (
                        key("chkBoxSched1Description", last),
                        key("chkBoxSched1Description", n),
                    ),
                    (
                        key("txtSched1_description", last),
                        key("txtSched1_description", n),
                    ),
                ],
            });
            for column in SCHEDULE_COLUMNS {
                insertions.push(RowInsertion {
                    after: key(column, n - 1),
                    copies: vec![(key(column, last), key(column, n))],
                });
            }
        }
        extend_layout(base, &insertions)
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    fn place_errors(place: &Form2200MPlace, item: &str, label: &str) -> Option<String> {
        let region = place.region.trim();
        let province = place.province.trim();
        let city = place.city.trim();
        let region_ok = crate::reference::get_all_regions()
            .iter()
            .any(|r| r.code == region);
        if !region_ok {
            return Some(format!(
                "Region field on Page 1 Item {item} ({label}) is required."
            ));
        }
        let province_ok = crate::reference::get_provinces_for_region(region)
            .iter()
            .any(|p| p.code == province);
        if !province_ok {
            return Some(format!(
                "Province field on Page 1 Item {item} ({label}) is required."
            ));
        }
        let city_ok = crate::reference::get_cities_for_province(province)
            .iter()
            // The official city list (`loadArrayCity`) holds 9-digit codes only.
            .any(|c| c.code == city && c.region_code == region && c.code.len() == 9);
        if !city_ok {
            return Some(format!(
                "City field on Page 1 Item {item} ({label}) is required."
            ));
        }
        None
    }

    /// `validateDate()` against `today`.
    fn date_error(&self, today: NaiveDate) -> Option<&'static str> {
        let date = NaiveDate::from_ymd_opt(
            i32::from(self.year),
            u32::from(self.month),
            u32::from(self.day),
        );
        let Some(date) = date.filter(|_| self.year >= 1800) else {
            return Some("Please provide a valid date. (MM/DD/YYYY format) in Page 1 Item 1.");
        };
        if date > today {
            return Some("Page 1 Item 1 Date cannot be a future date ");
        }
        if date < NaiveDate::from_ymd_opt(2018, 1, 1).expect("valid date") {
            return Some("Page 1 Item 1 Date cannot be earlier than January 2018");
        }
        None
    }

    fn validate_on(&self, today: NaiveDate) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // validateAll, in order.
        if !(1..=12).contains(&self.month) {
            err("month", "Month field on Page 1 Item 1 is required.");
        } else if self.day == 0 {
            err("day", "Day field on Page 1 Item 1 is required.");
        } else if self.year == 0 {
            err("year", "Year field on Page 1 Item 1 is required.");
        } else if let Some(message) = self.date_error(today) {
            err("day", message);
        }
        if self.number_of_attached_sheets > 999 {
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most three digits.",
            );
        }

        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        let tin_digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        if tin1.len() != 3
            || tin2.len() != 3
            || tin3.len() != 3
            || tin_digits.len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Page 1 Item 4.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }

        // The official check rejects only the '000' placeholder; the
        // dropdown can hold nothing else but a listed code.
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err(
                "rdo_code",
                "Please enter a valid RDO Code on Page 1 Item 6.",
            );
        }
        if self.taxpayer_name.trim().is_empty() {
            err("taxpayer_name", "Name field on Page 1 Item 7 is required.");
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Registered Address field on Page 1 Item 8 is required.",
            );
        } else if self.registered_address.trim().chars().count() > 160 {
            err(
                "registered_address",
                "Item 7 holds at most 160 characters (two lines of 80).",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() {
            err("zip_code", "Zip Code field on Page 1 Item 8A is required.");
        } else if !digits_only(zip) || zip.len() > 10 {
            err("zip_code", "Item 7A must be digits only.");
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() {
            err(
                "contact_number",
                "Contact Number field on Page 1 Item 9 is required.",
            );
        } else if !digits_only(phone) || phone.len() > 45 {
            // wholenumber() and maxlength="45" while typing.
            err("contact_number", "Item 8 accepts digits only, up to 45.");
        }
        let email = self.email.trim();
        if email.is_empty() {
            err("email", "E-mail address on page 1 item 10 is required.");
        } else if !email_matches_official_pattern(email) || email.chars().count() > 50 {
            err(
                "email",
                "Please enter a valid e-mail address on page 1 item 10",
            );
        }
        if let Some(message) =
            Self::place_errors(&self.place_of_production, "10", "Place of Production")
        {
            err("place_of_production", &message);
        }
        if let Some(message) = Self::place_errors(&self.place_of_removal, "11", "Place of Removal")
        {
            err("place_of_removal", &message);
        }

        match self.tax_relief {
            None => err(
                "tax_relief",
                "Availing of Tax Relief field on Page 1 Item 12 is required.",
            ),
            Some(true) if self.tax_relief_specify.trim().is_empty() => err(
                "tax_relief_specify",
                "Specify Tax Relief field on Page 1 Item 12A is required.",
            ),
            Some(true) if self.tax_relief_specify.trim().chars().count() > 50 => err(
                "tax_relief_specify",
                "Item 12A holds at most 50 characters.",
            ),
            _ => {}
        }
        match self.manner_of_payment {
            Form2200MPayment::None => err(
                "manner_of_payment",
                "Manner of Payment on Page 1 Part II is required.",
            ),
            Form2200MPayment::OtherScheme if self.other_scheme_description.trim().is_empty() => {
                err(
                    "other_scheme_description",
                    "Specfy Manner of Payment field on Page 1 Part II Item 15 is required.",
                )
            }
            Form2200MPayment::OtherScheme
                if self.other_scheme_description.trim().chars().count() > 80 =>
            {
                err(
                    "other_scheme_description",
                    "Item 15 holds at most 80 characters.",
                )
            }
            _ => {}
        }

        if self.balance_to_carry_over > 0.0 {
            err(
                "tax_deposit",
                "YOU HAVE INSUFICIENT FUND. PLEASE APPLY DEPOSIT TO PROCEED",
            );
        }

        // checkPartVFields, row by row.
        if self.schedule.len() > FORM_2200M_MAX_ROWS {
            err(
                "schedule",
                "Schedule 1 takes at most 200 rows in this editor.",
            );
        }
        for (index, row) in self.schedule.iter().enumerate() {
            let n = index + 1;
            let other_row = index >= FORM_2200M_FIRST_OTHER_ROW;
            // checkPartVFields checks every added row (11 onwards) whole.
            let added_row = index >= FORM_2200M_SCHEDULE_ROWS;
            let used = added_row
                || !row.place_of_removal.trim().is_empty()
                || row.amount_cells().iter().any(Option::is_some);
            let described = other_row && !row.description.trim().is_empty();
            if !used && !described {
                if !other_row && (row.local_rate.is_some() || row.imported_rate.is_some()) {
                    err(
                        &format!("schedule[{index}]"),
                        &format!("Row #{n} has a fixed tax rate."),
                    );
                }
                if other_row && (row.local_rate.is_some() || row.imported_rate.is_some()) {
                    err(
                        &format!("schedule[{index}].description"),
                        &format!("Description in row #{n} is required."),
                    );
                }
                continue;
            }
            if other_row && !described {
                err(
                    &format!("schedule[{index}].description"),
                    &format!("Description in row #{n} is required."),
                );
                continue;
            }
            if row.place_of_removal.trim().is_empty() {
                err(
                    &format!("schedule[{index}].place_of_removal"),
                    &format!("Place of Removal in row #{n} is required."),
                );
                continue;
            }
            let mut others = row.amount_cells().iter().any(Option::is_some);
            if other_row {
                others = others || row.local_rate.is_some() || row.imported_rate.is_some();
            }
            if !others {
                err(
                    &format!("schedule[{index}]"),
                    &format!("Input at least 1 more column in row #{n}"),
                );
            }
            // maxlength="20" on the text cells.
            if row.place_of_removal.trim().chars().count() > 20
                || row.description.trim().chars().count() > 20
            {
                err(
                    &format!("schedule[{index}]"),
                    &format!("Row #{n}: description and place of removal hold 20 characters."),
                );
            }
        }
        for (index, row) in self.schedule.iter().enumerate() {
            let cells = row.amount_cells();
            if cells
                .iter()
                .chain([row.local_rate, row.imported_rate].iter())
                .flatten()
                .any(|v| *v < 0.0 || !has_cent_precision(*v) || !within_official_precision(*v))
            {
                err(
                    &format!("schedule[{index}]"),
                    &format!(
                        "Row #{}: amounts must be non-negative pesos and centavos below 1,000,000,000,000.",
                        index + 1
                    ),
                );
            }
        }

        for (field, value) in [
            ("balance_carried_over", self.balance_carried_over),
            ("creditable_excise_tax", self.creditable_excise_tax),
            ("previous_payment", self.previous_payment),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
            ("tax_deposit", self.tax_deposit),
        ] {
            if value < 0.0 || !has_cent_precision(value) || !within_official_precision(value) {
                err(
                    field,
                    "Enter a non-negative amount in pesos and centavos (at most 12 digits before the decimal point).",
                );
            }
        }
        if !self.is_amended && self.previous_payment != 0.0 {
            err(
                "previous_payment",
                "Item 19 applies only to an amended return.",
            );
        }

        // Derived items must be what the official compute chain produces.
        let mut expected = self.clone();
        expected.recompute();
        if expected != *self {
            err(
                "balance_to_carry_over",
                "Totals are out of date. Recompute the return.",
            );
        }

        errors
    }
}

impl FormValidator for Form2200MDraft {
    /// `validateAll` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form2200MDraft {
    /// Items 21A-21C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "21A Surcharge",
                interest: "21B Interest",
                compromise: "21C Compromise",
            ],
            controls: [
                "frm2200M:txtPg1P3I21ASurcharge",
                "frm2200M:txtPg1P3I21BInterest",
                "frm2200M:txtPg1P3I21CCompromise",
            ],
            state: penalty,
            basic_tax_due: "Item 20", |d| d.tax_still_due,
            // Decision D8 (2026-10-18): due on removal, so the penalties run
            // from Item 1, the removal date the filer types.
            typed_due_date: Some(|d: &Self| {
                chrono::NaiveDate::from_ymd_opt(
                    i32::from(d.year),
                    u32::from(d.month),
                    u32::from(d.day),
                )
            }),
        )
    }
}

impl QueueableForm for Form2200MDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2200M";
    /// Official `formType` and filename segment. eBIRForms has
    /// no upload folder for `2200Mv2018` (the official page passes `undefined` as the
    /// upload folder in PROD), so queueing stays off in support_level until the
    /// real folder is confirmed.
    const FORM_TYPE: &'static str = "2200Mv2018";
    const LAYOUT_ID: &'static str = FORM_2200M_FORM_ID;

    /// Item 24 Amount Payable.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.amount_payable)
    }

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
        self.year
    }
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Monthly(self.month)
    }
    /// `MM + DD + YYYY`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}{:02}{}", self.month, self.day, self.year)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 8 || !digits_only(code) {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let day: u8 = code.get(2..4)?.parse().ok()?;
        let year: u16 = code.get(4..)?.parse().ok()?;
        NaiveDate::from_ymd_opt(i32::from(year), u32::from(month), u32::from(day))
            .map(|_| (year, FilingPeriod::Monthly(month)))
    }
    fn submission_email(&self) -> &str {
        self.email.trim()
    }
    fn compute(&mut self) {
        self.recompute();
    }
    fn validate(&self) -> Vec<(String, String)> {
        <Self as FormValidator>::validate(self)
    }
    fn field_map(&self) -> BTreeMap<String, String> {
        self.to_bir_field_map()
    }
    /// Added rows extend the page, so the plaintext follows
    /// [`Form2200MDraft::official_layout`].
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as FormValidator>::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let layout = self
            .official_layout()
            .map_err(|error| vec![("xml".to_string(), error)])?;
        crate::official_xml::write(&layout, &self.field_map())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
    }

    pub(crate) fn sample() -> Form2200MDraft {
        let mut draft = Form2200MDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            month: 3,
            day: 15,
            year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Mining Corp".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            contact_number: "0281234567".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            line_of_business: "Mining".to_string(),
            place_of_production: Form2200MPlace {
                region: "130000000".into(),
                province: "137400000".into(),
                city: "137403000".into(),
            },
            place_of_removal: Form2200MPlace {
                region: "130000000".into(),
                province: "137400000".into(),
                city: "137401000".into(),
            },
            tax_relief: Some(false),
            tax_relief_specify: String::new(),
            manner_of_payment: Form2200MPayment::ActualRemoval,
            other_scheme_description: String::new(),
            schedule: Vec::new(),
            schedule_total: 0.0,
            excise_tax_due: 0.0,
            balance_carried_over: 0.0,
            creditable_excise_tax: 0.0,
            total_credits: 0.0,
            net_tax_due: 0.0,
            previous_payment: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            amount_payable: 0.0,
            tax_deposit: 0.0,
            penalties_paid: 0.0,
            total_payment: 0.0,
            balance_to_carry_over: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        let row = draft.row_mut(0).unwrap();
        row.place_of_removal = "Pasig yard".into();
        row.volume_taxable = Some(1_234.567);
        row.local_tax_due = Some(1_000.005);
        let row = draft.row_mut(8).unwrap();
        row.description = "Sand x".into();
        row.place_of_removal = "Pasig".into();
        row.local_rate = Some(4.0);
        row.local_tax_due = Some(5.0);
        draft.surcharge = 25.5;
        draft.keep_entered_penalties();
        draft.recompute();
        draft.tax_deposit = draft.tax_still_due;
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2200MDraft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.schedule[0].volume_taxable, Some(1_234.57));
        assert_eq!(draft.schedule[0].local_tax_due, Some(1_000.01));
        assert_eq!(draft.schedule_total, 1_005.01);
        assert_eq!(draft.excise_tax_due, 1_005.01);
        assert_eq!(draft.tax_still_due, 1_005.01);
        assert_eq!(draft.total_penalties, 25.5);
        assert_eq!(draft.amount_payable, 1_030.51);
        assert_eq!(draft.penalties_paid, 25.5);
        assert_eq!(draft.total_payment, 1_030.51);
        assert_eq!(draft.balance_to_carry_over, 0.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        assert_eq!(fields["frm2200M:txtPg1I1Month"], "03");
        assert_eq!(fields["frm2200M:txtPg1I1Day"], "15");
        assert_eq!(
            fields["frm2200M:txtPg1Pt1I6RegisteredName"],
            "SAMPLE MINING CORP"
        );
        assert_eq!(
            fields["frm2200M:txtPg2RegisteredName"],
            "SAMPLE MINING CORP"
        );
        assert_eq!(fields["frm2200M:txtPg1TIN4"], "00000");
        assert_eq!(fields["frm2200M:rdoPg1Pt1I6RDO"], "039");
        assert_eq!(fields["frm2200M:txtSched1_VOMRITaxableA1"], "1,234.57");
        assert_eq!(fields["frm2200M:txtSched1_VOMRIExemptB1"], "");
        assert_eq!(fields["frm2200M:LocallyExtractedRate1"], "");
        assert_eq!(fields["frm2200M:txtSched1_PlaceOfRemoval1"], "PASIG YARD");
        assert_eq!(fields["frm2200M:txtSched1_description9"], "SAND X");
        assert_eq!(fields["frm2200M:txtSched1_descriptionCont9"], "SAND X");
        assert_eq!(
            fields["frm2200M:txtSched1_LocallyExtractedTaxRateE9"],
            "4.00"
        );
        assert_eq!(fields["frm2200M:TotalExciseTaxDue"], "1,005.01");
        assert_eq!(fields["frm2200M:txtPg1Pt1I11Region"], "130000000");
        assert_eq!(
            fields["frm2200M:txtPg1Pt1I9Email"],
            "sample.taxpayer@example.com"
        );
        assert!(!fields.contains_key("frm2200M:txtSched1_LocallyExtractedTaxRateE1"));
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2200Mv2018-03152025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn long_addresses_split_like_the_profile_loader() {
        let mut draft = sample();
        draft.registered_address = "a".repeat(85);
        let fields = draft.to_bir_field_map();
        assert_eq!(
            fields["frm2200M:txtPg1Pt1I7RegisteredAddress"],
            "A".repeat(80)
        );
        assert_eq!(fields["frm2200M:txtPg1Pt1I7RegisteredAddress2"], "AAAAA");
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "03152025");
        assert_eq!(
            Form2200MDraft::parse_period_code("03152025"),
            Some((2025, FilingPeriod::Monthly(3)))
        );
        assert_eq!(Form2200MDraft::parse_period_code("02302025"), None);
        assert_eq!(Form2200MDraft::parse_period_code("0315202"), None);
        assert_eq!(Form2200MDraft::parse_period_code("032025Q1"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2200MDraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.month = 0,
            "Month field on Page 1 Item 1 is required.",
        );
        check(&|d| d.day = 0, "Day field on Page 1 Item 1 is required.");
        check(&|d| d.year = 0, "Year field on Page 1 Item 1 is required.");
        check(
            &|d| {
                d.month = 2;
                d.day = 30;
            },
            "Please provide a valid date. (MM/DD/YYYY format) in Page 1 Item 1.",
        );
        check(
            &|d| d.year = 2026,
            "Page 1 Item 1 Date cannot be a future date ",
        );
        check(
            &|d| d.year = 2017,
            "Page 1 Item 1 Date cannot be earlier than January 2018",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Page 1 Item 6.",
        );
        check(
            &|d| d.taxpayer_name = " ".into(),
            "Name field on Page 1 Item 7 is required.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Registered Address field on Page 1 Item 8 is required.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Zip Code field on Page 1 Item 8A is required.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Contact Number field on Page 1 Item 9 is required.",
        );
        check(
            &|d| d.email.clear(),
            "E-mail address on page 1 item 10 is required.",
        );
        check(
            &|d| d.email = "not-an-email".into(),
            "Please enter a valid e-mail address on page 1 item 10",
        );
        check(
            &|d| d.place_of_production.region.clear(),
            "Region field on Page 1 Item 10 (Place of Production) is required.",
        );
        check(
            &|d| d.place_of_production.province.clear(),
            "Province field on Page 1 Item 10 (Place of Production) is required.",
        );
        check(
            &|d| d.place_of_production.city = "012801000".into(),
            "City field on Page 1 Item 10 (Place of Production) is required.",
        );
        check(
            &|d| d.place_of_removal.region = "00".into(),
            "Region field on Page 1 Item 11 (Place of Removal) is required.",
        );
        check(
            &|d| d.place_of_removal.province = "00".into(),
            "Province field on Page 1 Item 11 (Place of Removal) is required.",
        );
        check(
            &|d| d.place_of_removal.city.clear(),
            "City field on Page 1 Item 11 (Place of Removal) is required.",
        );
        check(
            &|d| d.tax_relief = None,
            "Availing of Tax Relief field on Page 1 Item 12 is required.",
        );
        check(
            &|d| d.tax_relief = Some(true),
            "Specify Tax Relief field on Page 1 Item 12A is required.",
        );
        check(
            &|d| d.manner_of_payment = Form2200MPayment::None,
            "Manner of Payment on Page 1 Part II is required.",
        );
        check(
            &|d| d.manner_of_payment = Form2200MPayment::OtherScheme,
            "Specfy Manner of Payment field on Page 1 Part II Item 15 is required.",
        );
        check(
            &|d| {
                d.tax_deposit = 0.0;
                d.recompute();
            },
            "YOU HAVE INSUFICIENT FUND. PLEASE APPLY DEPOSIT TO PROCEED",
        );
        check(
            &|d| d.schedule[8].description.clear(),
            "Description in row #9 is required.",
        );
        check(
            &|d| d.schedule[0].place_of_removal.clear(),
            "Place of Removal in row #1 is required.",
        );
        check(
            &|d| {
                let row = d.row_mut(1).unwrap();
                row.place_of_removal = "Taguig".into();
            },
            "Input at least 1 more column in row #2",
        );
        check(
            &|d| {
                let row = d.row_mut(9).unwrap();
                row.description = "Gravel".into();
                row.place_of_removal = "Taguig".into();
            },
            "Input at least 1 more column in row #10",
        );
        // An added row (11 onwards) is checked whole, even when blank.
        check(
            &|d| {
                d.row_mut(10).unwrap();
            },
            "Description in row #11 is required.",
        );
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN",
        );
    }

    #[test]
    fn deposits_can_exceed_the_amount_payable() {
        let mut draft = sample();
        draft.tax_deposit += 100.0;
        draft.recompute();
        assert_eq!(draft.balance_to_carry_over, -100.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        assert_eq!(
            draft.to_bir_field_map()["frm2200M:txtPg1P3I24BalToCarryOver"],
            "-100.00"
        );
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        assert!(draft.revalidate_queued_before_submission().is_ok());
        draft.surcharge = 99.0;
        draft.keep_entered_penalties();
        assert!(draft.revalidate_queued_before_submission().is_err());
    }

    /// P5-1: the earlier float-tolerance check refused two-decimal
    /// amounts above about 2,000,000 (`8_816_764.47 * 100` lands 1.2e-7
    /// off a whole number); the shared `official_inputs` check accepts
    /// them again, and a three-decimal amount is still refused.
    #[test]
    fn large_two_decimal_amounts_pass_cent_precision() {
        let mut draft = sample();
        draft.surcharge = 8_816_764.47;
        assert!(
            <Form2200MDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2200MDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
