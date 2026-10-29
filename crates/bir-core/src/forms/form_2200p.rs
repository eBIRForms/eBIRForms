//! BIR Form 2200P — Excise Tax Return for Petroleum Products (January 2020).
//!
//! Ported from the official `BIR-Form2200Pv2020.hta` (eBIRForms 7.9.6.2.1):
//! the Schedule 1 total (`computeBasicExciseTaxDue`), the Part III chain
//! (`partThreeComputation`), `validateAll` with its exact alert texts
//! (including `validateDate`, `validateEmail` and `checkPartVFields`), and
//! `saveXMLsubmit` through [`crate::official_xml`].
//!
//! Schedule 1 has 27 official rows: rows 1–25 carry fixed ATCs, product
//! descriptions, units and applicable rates (XP010 … XP220); rows 26 and 27
//! are "others" rows where the filer types the ATC digits (after the printed
//! `XP`), description, unit of measure and rate. The page can append rows
//! past 27 (`addRowSched1`); each added row's controls follow the previous
//! row's in the upload (see [`Form2200PDraft::official_layout`]), up to
//! [`FORM_2200P_MAX_ROWS`] rows.
//!
//! Every Schedule 1 cell is free filer input: the official page computes no
//! row arithmetic. Item 16 is the sum of the Basic Excise Tax Due column over
//! all 27 rows, exactly what `computeBasicExciseTaxDue` adds.
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
pub const FORM_2200P_FORM_ID: &str = "2200p-v2020";
/// Schedule 1 rows the page loads with; `addRowSched1` appends more.
pub const FORM_2200P_SCHEDULE_ROWS: usize = 27;
/// Upper bound on rows the editor lets the filer add (the page has none).
pub const FORM_2200P_MAX_ROWS: usize = 200;
/// Controls of a Schedule 1 row in page order; `rowTemplate` appends an
/// added row's controls after the previous row's.
const SCHEDULE_COLUMNS: [&str; 15] = [
    "chkBoxSched1ATC",
    "txtSched1ATC",
    "txtSched1Description",
    "txtSched1UnitOfMeasure",
    "txtSched1ApplicableRate",
    "txtSched1PlaceOfRemoval",
    "txtSched1LocalManuProd",
    "txtSched1Imported",
    "txtSched1TotTaxRemovals",
    "txtSched1TaxPaidImported",
    "txtSched1UnderBond",
    "txtSched1Exports",
    "txtSched1Others",
    "txtSched1TotTaxPaid",
    "txtSched1BasicExciseTaxDue",
];
/// Rows 26 and 27 (index 25 and 26) take filer ATC, description, unit and rate.
pub const FORM_2200P_FIRST_OTHER_ROW: usize = 25;

/// Fixed rows 1–25 of Schedule 1: (ATC, description, unit of measure,
/// applicable rate as printed).
pub const FORM_2200P_FIXED_ROWS: [(&str, &str, &str, &str); 25] = [
    ("XP010", "Lubricating Oils", "Per Liter", "10.00"),
    ("XP020", "Greases", "Per Kg", "10.00"),
    ("XP030", "Processed Gas", "Per Liter", "10.00"),
    ("XP040", "Waxes and Petrolatum", "Per Kg", "10.00"),
    (
        "XP050",
        "Denatured Alcohol used for motive power",
        "Per Liter",
        "10.00",
    ),
    ("XP060", "Unleaded Premium Gasoline", "Per Liter", "10.00"),
    ("XP080", "Regular Gasoline", "Per Liter", "10.00"),
    ("XP085", "Pyrolysis Gasoline", "Per Liter", "10.00"),
    ("XP090", "Naphtha", "Per Liter", "10.00"),
    (
        "XP100",
        "Naphtha to be used for petro-chemicals",
        "Per Liter",
        "0.00",
    ),
    (
        "XP105",
        "Naphtha and Pyrolysis gasoline, when used as a raw material in the production of petrochemical products or in the refining of petroleum products, or as a replacement fuel for natural-gas-fired combined cycle power plant",
        "Per Liter",
        "0.00",
    ),
    ("XP110", "Aviation Gasoline", "Per Liter", "4.00"),
    ("XP120", "Aviation Turbo Jet Fuel", "Per Liter", "4.00"),
    ("XP130", "Kerosene", "Per Liter", "5.00"),
    (
        "XP131",
        "Kerosene used as aviation fuel",
        "Per Liter",
        "4.00",
    ),
    ("XP140", "Diesel Fuel Oil", "Per Liter", "6.00"),
    ("XP150", "LPG used as Motive Power", "Per Kg", "6.00"),
    ("XP160", "LPG", "Per Kg", "3.00"),
    (
        "XP165",
        "LPG used in the production of petro-chemical products",
        "Per Kg",
        "0.00",
    ),
    ("XP170", "Asphalt", "Per Kg", "10.00"),
    (
        "XP180",
        "Bunker Fuel Oil and similar fuel",
        "Per Liter",
        "6.00",
    ),
    (
        "XP190",
        "Base Stocks for tube oils and greases, HVD, aromatic extracts, etc.",
        "Per Liter",
        "10.00",
    ),
    ("XP200", "Petroleum Coke", "Per MT", "6.00"),
    ("XP210", "Petroleum Coke for motive power", "Per MT", "0.00"),
    (
        "XP220",
        "Additives for lubricating oils and greases",
        "Per Liter/Kg",
        "10.00",
    ),
];
/// Printed ATC prefix of rows 26 and 27.
pub const FORM_2200P_OTHER_ATC_PREFIX: &str = "XP";
/// Items 10 and 11: region, province and city codes from the official
/// dropdowns (`xml/region.xml`, `xml/province.xml`, `loadArrayCity`). Blank
/// means the `(Select …)` placeholder, `00`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Form2200PPlace {
    pub region: String,
    pub province: String,
    pub city: String,
}

impl Form2200PPlace {
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
pub enum Form2200PPayment {
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
pub struct Form2200PRow {
    /// Rows 26–27 only: ATC digits after the printed `XP` (`wholenumber`).
    #[serde(default)]
    pub atc_digits: String,
    /// Rows 26–27 only (max 20 characters).
    #[serde(default)]
    pub description: String,
    /// Rows 26–27 only (max 20 characters).
    #[serde(default)]
    pub unit_of_measure: String,
    /// Rows 26–27 only; rows 1–25 print their fixed rate.
    #[serde(default)]
    pub applicable_rate: Option<f64>,
    #[serde(default)]
    pub place_of_removal: String,
    /// Taxable removals: locally manufactured/produced and imported.
    #[serde(default)]
    pub local_manufactured: Option<f64>,
    #[serde(default)]
    pub imported: Option<f64>,
    #[serde(default)]
    pub total_taxable_removals: Option<f64>,
    /// Tax-paid / exempt removals: imported, under bond, exports, others.
    #[serde(default)]
    pub tax_paid_imported: Option<f64>,
    #[serde(default)]
    pub under_bond: Option<f64>,
    #[serde(default)]
    pub exports: Option<f64>,
    #[serde(default)]
    pub others: Option<f64>,
    #[serde(default)]
    pub total_tax_paid: Option<f64>,
    /// Basic excise tax due (summed into Item 16).
    #[serde(default)]
    pub basic_excise_tax_due: Option<f64>,
}

impl Form2200PRow {
    /// The cells `checkPartVRow` tests after the place of removal.
    fn amount_cells(&self) -> [Option<f64>; 9] {
        [
            self.local_manufactured,
            self.imported,
            self.total_taxable_removals,
            self.tax_paid_imported,
            self.under_bond,
            self.exports,
            self.others,
            self.total_tax_paid,
            self.basic_excise_tax_due,
        ]
    }

    fn cells_mut(&mut self) -> [&mut Option<f64>; 10] {
        [
            &mut self.applicable_rate,
            &mut self.local_manufactured,
            &mut self.imported,
            &mut self.total_taxable_removals,
            &mut self.tax_paid_imported,
            &mut self.under_bond,
            &mut self.exports,
            &mut self.others,
            &mut self.total_tax_paid,
            &mut self.basic_excise_tax_due,
        ]
    }

    /// The "others" identification cells (`checkPartVReqRow`).
    fn has_identity(&self) -> bool {
        !self.atc_digits.trim().is_empty()
            || !self.description.trim().is_empty()
            || !self.unit_of_measure.trim().is_empty()
            || self.applicable_rate.is_some()
    }

    pub fn is_blank(&self) -> bool {
        !self.has_identity()
            && self.place_of_removal.trim().is_empty()
            && self.amount_cells().iter().all(Option::is_none)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2200PDraft {
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
    pub place_of_production: Form2200PPlace,
    #[serde(default)]
    pub place_of_removal: Form2200PPlace,
    /// Item 12: `None` until the filer answers.
    #[serde(default)]
    pub tax_relief: Option<bool>,
    #[serde(default)]
    pub tax_relief_specify: String,

    // Part II
    #[serde(default)]
    pub manner_of_payment: Form2200PPayment,
    #[serde(default)]
    pub other_scheme_description: String,

    // Part V — Schedule 1 (rows 1–10)
    #[serde(default)]
    pub schedule: Vec<Form2200PRow>,
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

impl Form2200PDraft {
    pub const FORM_CODE: &'static str = "2200P";

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
            place_of_production: Form2200PPlace::default(),
            place_of_removal: Form2200PPlace::default(),
            tax_relief: None,
            tax_relief_specify: String::new(),
            manner_of_payment: Form2200PPayment::None,
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
    pub fn row_mut(&mut self, index: usize) -> Option<&mut Form2200PRow> {
        if index >= FORM_2200P_MAX_ROWS {
            return None;
        }
        if self.schedule.len() <= index {
            self.schedule.resize_with(index + 1, Form2200PRow::default);
        }
        self.schedule.get_mut(index)
    }

    pub fn row(&self, index: usize) -> Form2200PRow {
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
        if self.manner_of_payment != Form2200PPayment::OtherScheme {
            self.other_scheme_description.clear();
        }
        while self.schedule.last().is_some_and(Form2200PRow::is_blank) {
            self.schedule.pop();
        }
        let mut total = 0.0;
        for (index, row) in self.schedule.iter_mut().enumerate() {
            if index < FORM_2200P_FIRST_OTHER_ROW {
                // Fixed rows have no editable identification controls.
                row.atc_digits.clear();
                row.description.clear();
                row.unit_of_measure.clear();
                row.applicable_rate = None;
            }
            for cell in row.cells_mut() {
                *cell = cell.map(cents);
            }
            total += row.basic_excise_tax_due.unwrap_or(0.0);
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

    /// The print preview's map: the submit map plus `derived:` keys for the
    /// printed combs that Items 8-9, the three Item 10 levels and the three
    /// Item 11 levels share (one 40-slot comb each, values placed under their
    /// own captions), and `derived:page_header`, the page 2 header comb (TIN
    /// then name, one 40-slot comb on this sheet).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let mut put = |key: &str, value: String| {
            if !value.trim().is_empty() {
                fields.insert(format!("derived:{key}"), value);
            }
        };
        put(
            "contact_email",
            super::excise_places::contact_email_comb_line(
                &self.contact_number,
                &self.email,
                12,
                40,
            ),
        );
        for (key, place) in [
            ("place_of_production", &self.place_of_production),
            ("place_of_removal", &self.place_of_removal),
        ] {
            put(
                key,
                super::excise_places::shared_comb_line(
                    &[
                        (0, place.region.as_str()),
                        (10, place.province.as_str()),
                        (24, place.city.as_str()),
                    ],
                    40,
                ),
            );
        }
        put(
            "page_header",
            super::print_map::tin_name_header(&self.tin, &self.taxpayer_name),
        );
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2200P:{key}"), value);
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
                Form2200PPlace::code(&place.region),
            );
            put(
                &format!("txtPg1Pt1I{item}Province"),
                Form2200PPlace::code(&place.province),
            );
            put(
                &format!("txtPg1Pt1I{item}City"),
                Form2200PPlace::code(&place.city),
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
            flag(mop == Form2200PPayment::ActualRemoval),
        );
        put(
            "rdoPg1Pt2MOPPrepayment",
            flag(mop == Form2200PPayment::Prepayment),
        );
        put(
            "rdoPg1Pt2MOPOther",
            flag(mop == Form2200PPayment::OtherScheme),
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
                &format!("txtSched1PlaceOfRemoval{n}"),
                capitalize(&row.place_of_removal),
            );
            for (column, value) in [
                ("LocalManuProd", row.local_manufactured),
                ("Imported", row.imported),
                ("TotTaxRemovals", row.total_taxable_removals),
                ("TaxPaidImported", row.tax_paid_imported),
                ("UnderBond", row.under_bond),
                ("Exports", row.exports),
                ("Others", row.others),
                ("TotTaxPaid", row.total_tax_paid),
                ("BasicExciseTaxDue", row.basic_excise_tax_due),
            ] {
                put(&format!("txtSched1{column}{n}"), optional_amount(value));
            }
            if index >= FORM_2200P_FIRST_OTHER_ROW {
                put(&format!("chkBoxSched1ATC{n}"), flag(false));
                put(
                    &format!("txtSched1ATC{n}"),
                    row.atc_digits.trim().to_string(),
                );
                put(
                    &format!("txtSched1Description{n}"),
                    capitalize(&row.description),
                );
                put(
                    &format!("txtSched1UnitOfMeasure{n}"),
                    capitalize(&row.unit_of_measure),
                );
                put(
                    &format!("txtSched1ApplicableRate{n}"),
                    optional_amount(row.applicable_rate),
                );
            }
        }
        put(
            "txtSched1TotalTaxDue",
            if self.schedule_total != 0.0 {
                official_amount(self.schedule_total)
            } else {
                String::new()
            },
        );
        // addRowSched1 counts the rows it adds.
        put(
            "sched1AddedRowCount",
            (self.row_count() - FORM_2200P_SCHEDULE_ROWS).to_string(),
        );
        put("txtLOB", self.line_of_business.clone());
        // sleeptime() sets the pager to page 1 at load.
        put("txtCurrentPage", "1".to_string());
        fields
    }

    /// Rows on the page: the 27 it loads with plus any the filer added.
    pub fn row_count(&self) -> usize {
        self.schedule.len().max(FORM_2200P_SCHEDULE_ROWS)
    }

    /// The generated layout with every added row's controls spliced in after
    /// the previous row's last control, as `rowTemplate` appends them.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        use super::official_inputs::{RowInsertion, extend_layout};
        let base = crate::official_xml::layout(FORM_2200P_FORM_ID).map_err(|e| e.to_string())?;
        let key = |column: &str, n: usize| format!("frm2200P:{column}{n}");
        let last = FORM_2200P_SCHEDULE_ROWS;
        let insertions: Vec<RowInsertion> = ((last + 1)..=self.row_count())
            .map(|n| RowInsertion {
                after: key("txtSched1BasicExciseTaxDue", n - 1),
                copies: SCHEDULE_COLUMNS
                    .iter()
                    .map(|column| (key(column, last), key(column, n)))
                    .collect(),
            })
            .collect();
        extend_layout(base, &insertions)
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    fn place_errors(place: &Form2200PPlace, item: &str, label: &str) -> Option<String> {
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
        if date < NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date") {
            return Some("Page 1 Item 1 Date cannot be earlier than January 2020");
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
            Form2200PPayment::None => err(
                "manner_of_payment",
                "Manner of Payment on Page 1 Part II is required.",
            ),
            Form2200PPayment::OtherScheme if self.other_scheme_description.trim().is_empty() => {
                err(
                    "other_scheme_description",
                    "Specfy Manner of Payment field on Page 1 Part II Item 15 is required.",
                )
            }
            Form2200PPayment::OtherScheme
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
        if self.schedule.len() > FORM_2200P_MAX_ROWS {
            err(
                "schedule",
                "Schedule 1 takes at most 200 rows in this editor.",
            );
        }
        for (index, row) in self.schedule.iter().enumerate() {
            let n = index + 1;
            let other_row = index >= FORM_2200P_FIRST_OTHER_ROW;
            // checkPartVFields checks every added row (28 onwards) whole.
            let added_row = index >= FORM_2200P_SCHEDULE_ROWS;
            let used = added_row
                || !row.place_of_removal.trim().is_empty()
                || row.amount_cells().iter().any(Option::is_some);
            if !used && !(other_row && row.has_identity()) {
                continue;
            }
            if other_row {
                // checkPartVReqRow.
                let missing = if row.atc_digits.trim().is_empty() {
                    Some(("atc_digits", "ATC"))
                } else if row.description.trim().is_empty() {
                    Some(("description", "Description"))
                } else if row.unit_of_measure.trim().is_empty() {
                    Some(("unit_of_measure", "Unit of Measure"))
                } else if row.applicable_rate.is_none() {
                    Some(("applicable_rate", "Applicable Rate"))
                } else {
                    None
                };
                if let Some((field, label)) = missing {
                    err(
                        &format!("schedule[{index}].{field}"),
                        &format!("{label} in row #{n} is required."),
                    );
                    continue;
                }
                if !digits_only(row.atc_digits.trim()) || row.atc_digits.trim().len() > 20 {
                    err(
                        &format!("schedule[{index}].atc_digits"),
                        &format!("Row #{n}: the ATC takes up to 20 digits after XP."),
                    );
                }
            }
            // checkPartVRow.
            if row.place_of_removal.trim().is_empty() {
                err(
                    &format!("schedule[{index}].place_of_removal"),
                    &format!("Place of Removal in row #{n} is required."),
                );
                continue;
            }
            if !row.amount_cells().iter().any(Option::is_some) {
                err(
                    &format!("schedule[{index}]"),
                    &format!("Input at least 1 more column in row #{n}"),
                );
            }
            // maxlength="20" on the text cells.
            if row.place_of_removal.trim().chars().count() > 20
                || row.description.trim().chars().count() > 20
                || row.unit_of_measure.trim().chars().count() > 20
            {
                err(
                    &format!("schedule[{index}]"),
                    &format!(
                        "Row #{n}: description, unit of measure and place of removal hold 20 characters."
                    ),
                );
            }
        }
        for (index, row) in self.schedule.iter().enumerate() {
            if row
                .amount_cells()
                .iter()
                .chain([row.applicable_rate].iter())
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

impl FormValidator for Form2200PDraft {
    /// `validateAll` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form2200PDraft {
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
                "frm2200P:txtPg1P3I21ASurcharge",
                "frm2200P:txtPg1P3I21BInterest",
                "frm2200P:txtPg1P3I21CCompromise",
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

impl QueueableForm for Form2200PDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2200P";
    /// Official `formType` and filename segment. `ftpTargetFolder.PROD` has
    /// no `2200Pv2020` key (only `2200P`); the official page passes
    /// `undefined` as the SFTP folder in PROD, so queueing stays off in
    /// support_level until the real folder is confirmed.
    const FORM_TYPE: &'static str = "2200Pv2020";
    const LAYOUT_ID: &'static str = FORM_2200P_FORM_ID;

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
    /// [`Form2200PDraft::official_layout`].
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

    pub(crate) fn sample() -> Form2200PDraft {
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Sample Petroleum Corp", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Refining", "registered_address": "123 Sample St Quezon City",
            "zip_code": "1100", "phone": "0281234567", "email": "sample.taxpayer@example.com",
            "default_form_type": "2200P", "taxpayer_type": "Corporation"
        }))
        .unwrap();
        let mut draft = Form2200PDraft::new_from_profile(&profile, 2025, 3);
        draft.day = 15;
        draft.place_of_production = Form2200PPlace {
            region: "130000000".into(),
            province: "137400000".into(),
            city: "137403000".into(),
        };
        draft.place_of_removal = draft.place_of_production.clone();
        draft.tax_relief = Some(false);
        draft.manner_of_payment = Form2200PPayment::ActualRemoval;
        let row = draft.row_mut(15).unwrap();
        row.place_of_removal = "Batangas depot".into();
        row.local_manufactured = Some(1_000.005);
        row.basic_excise_tax_due = Some(6_000.03);
        let row = draft.row_mut(25).unwrap();
        row.atc_digits = "999".into();
        row.description = "Other fuel".into();
        row.unit_of_measure = "Per liter".into();
        row.applicable_rate = Some(2.5);
        row.place_of_removal = "Batangas".into();
        row.basic_excise_tax_due = Some(5.005);
        draft.surcharge = 25.5;
        draft.keep_entered_penalties();
        draft.recompute();
        draft.tax_deposit = draft.tax_still_due;
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2200PDraft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn print_map_adds_only_the_shared_comb_lines() {
        let draft = sample();
        let submit = draft.to_bir_field_map();
        let print = draft.to_print_field_map();
        for (key, value) in &submit {
            assert_eq!(print.get(key), Some(value), "{key}");
        }
        assert!(
            print
                .keys()
                .filter(|k| !submit.contains_key(*k))
                .all(|k| k.starts_with("derived:"))
        );
        let line = &print["derived:contact_email"];
        assert!(line.starts_with(draft.contact_number.trim()));
        assert_eq!(&line[12..], draft.email.trim());
        let place = &print["derived:place_of_production"];
        assert!(place.starts_with(draft.place_of_production.region.as_str()));
        assert_eq!(&place[24..], draft.place_of_production.city.as_str());
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert_eq!(draft.schedule[15].local_manufactured, Some(1_000.01));
        assert_eq!(draft.schedule[25].basic_excise_tax_due, Some(5.01));
        assert_eq!(draft.schedule_total, 6_005.04);
        assert_eq!(draft.excise_tax_due, 6_005.04);
        assert_eq!(draft.amount_payable, 6_030.54);
        assert_eq!(draft.balance_to_carry_over, 0.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        let layout = crate::official_xml::layout(FORM_2200P_FORM_ID).unwrap();
        let keys = layout.keys();
        assert!(fields.keys().all(|key| keys.contains(key.as_str())));
        assert_eq!(fields["frm2200P:txtPg1I1Month"], "03");
        assert_eq!(
            fields["frm2200P:txtPg1Pt1I6RegisteredName"],
            "SAMPLE PETROLEUM CORP"
        );
        assert_eq!(fields["frm2200P:rdoPg1Pt1I6RDO"], "039");
        assert_eq!(
            fields["frm2200P:txtSched1PlaceOfRemoval16"],
            "BATANGAS DEPOT"
        );
        assert_eq!(fields["frm2200P:txtSched1LocalManuProd16"], "1,000.01");
        assert_eq!(fields["frm2200P:txtSched1Imported16"], "");
        assert_eq!(fields["frm2200P:txtSched1ATC26"], "999");
        assert_eq!(fields["frm2200P:txtSched1Description26"], "OTHER FUEL");
        assert_eq!(fields["frm2200P:txtSched1ApplicableRate26"], "2.50");
        assert_eq!(fields["frm2200P:txtSched1TotalTaxDue"], "6,005.04");
        assert!(!fields.contains_key("frm2200P:txtSched1ATC1"));
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2200Pv2020-03152025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        assert_eq!(sample().period_code(), "03152025");
        assert_eq!(
            Form2200PDraft::parse_period_code("03152025"),
            Some((2025, FilingPeriod::Monthly(3)))
        );
        assert_eq!(Form2200PDraft::parse_period_code("02302025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2200PDraft), expected: &str| {
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
            &|d| d.year = 2019,
            "Page 1 Item 1 Date cannot be earlier than January 2020",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Page 1 Item 6.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
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
            &|d| d.email = "x@y".into(),
            "Please enter a valid e-mail address on page 1 item 10",
        );
        check(
            &|d| d.place_of_production.region.clear(),
            "Region field on Page 1 Item 10 (Place of Production) is required.",
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
            &|d| d.manner_of_payment = Form2200PPayment::None,
            "Manner of Payment on Page 1 Part II is required.",
        );
        check(
            &|d| d.manner_of_payment = Form2200PPayment::OtherScheme,
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
            &|d| d.schedule[25].atc_digits.clear(),
            "ATC in row #26 is required.",
        );
        check(
            &|d| d.schedule[25].description.clear(),
            "Description in row #26 is required.",
        );
        check(
            &|d| d.schedule[25].unit_of_measure.clear(),
            "Unit of Measure in row #26 is required.",
        );
        check(
            &|d| d.schedule[25].applicable_rate = None,
            "Applicable Rate in row #26 is required.",
        );
        check(
            &|d| d.schedule[15].place_of_removal.clear(),
            "Place of Removal in row #16 is required.",
        );
        check(
            &|d| d.row_mut(2).unwrap().place_of_removal = "Bataan".into(),
            "Input at least 1 more column in row #3",
        );
        // An added row (28 onwards) is checked whole, even when blank.
        check(
            &|d| {
                d.row_mut(27).unwrap().atc_digits.clear();
            },
            "ATC in row #28 is required.",
        );
        check(
            &|d| {
                let row = d.row_mut(27).unwrap();
                row.atc_digits = "990".into();
                row.description = "Added".into();
                row.unit_of_measure = "Per kg".into();
                row.applicable_rate = Some(1.0);
            },
            "Place of Removal in row #28 is required.",
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
            <Form2200PDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2200PDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
