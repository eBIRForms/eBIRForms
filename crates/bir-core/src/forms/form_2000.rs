//! BIR Form 2000 (v2018) — Monthly Documentary Stamp Tax Declaration/Return.
//!
//! Ported from the official `BIR-Form2000v2018.hta` (eBIRForms 7.9.6.2.1):
//! the Schedule 1 ATC computations (`getATCdrpTaxRate`, `computeSched1TaxDue`,
//! the DS106 term question `ds106CloseModal`), Schedules 2–4, the Part II
//! chain (`computeTax15D`, `computeTax16`, `computePenalties`,
//! `computeTotalAmtPayable`), `validate()` with its exact alert texts and
//! `saveXMLsubmit` through [`crate::official_xml`].
//!
//! The official page starts with three rows per schedule; each "Add" button
//! re-renders the table with one more row, so the upload carries every DOM
//! row in order. Rows beyond the third extend the layout the same way.
//!
//! ATCs the official page offers but cannot compute reliably are not offered
//! here: DS125 (`computeSched1TaxDue` throws a ReferenceError on a stray `S`,
//! so no tax due is ever set) and DS130–DS132 (their modals read the DS130
//! month field for every charter ATC and DS131/DS132 throw on
//! `newds130rate`).

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_2000_FORM_ID: &str = "2000-v2018";
/// Rows each schedule starts with on the official page (and in its layout).
/// "Add" appends DOM rows after them ([`Form2000Draft::official_layout`]).
pub const FORM_2000_ROWS: usize = 3;
/// Rows a schedule may hold here (the page itself has no limit).
pub const FORM_2000_MAX_ROWS: usize = 50;

/// One Schedule 1 ATC from the official dropdown (`loadATCDropDown`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Form2000AtcOption {
    pub code: &'static str,
    pub description: &'static str,
    /// What `getATCdrpTaxRate` writes into the rate column.
    pub rate_text: &'static str,
}

const fn atc(
    code: &'static str,
    description: &'static str,
    rate_text: &'static str,
) -> Form2000AtcOption {
    Form2000AtcOption {
        code,
        description,
        rate_text,
    }
}

/// Dropdown order. `DS010 - GENERAL` is the page's own first entry; the rest
/// are the `atcCodes.xml` lines tagged `2000^`, minus DS125 and DS130–DS132
/// (see the module docs).
pub const FORM_2000_ATC_OPTIONS: &[Form2000AtcOption] = &[
    atc("DS010", "GENERAL", ""),
    atc("DS101", "ORIGINAL ISSUE OF SHARES OF STOCKS", "P2.00/P200"),
    atc(
        "DS102",
        "SALES, AGREEMENT TO SELL > MEMORANDA OF SALES DELIVERIES OR TRANSFER OF DUE BILL CERTIFICATE OF OBLIGATION",
        "P1.50/P200",
    ),
    atc(
        "DS103",
        "BONDS, DEBENTURE AND CERTIFICATE OF STOCKS OR INDEBTEDNESS ISSUED IN FOREIGN COUNTRIES",
        "75%",
    ),
    atc(
        "DS104",
        "CERTIFICATE OF PROFITS OR INTEREST IN PROPERTY OR ACCUMULATIONS",
        "P1.00/P200",
    ),
    atc(
        "DS105",
        "BANK CHECKS, DRAFTS, CERTL OF DEPOSIT NOT BEARING INTEREST AND OTHER INSTRUMENTS",
        "P3.00/PIECE OF CHECK",
    ),
    atc(
        "DS106",
        "ORIGINAL ISSUE OF ALL DEBT INSTRUMENTS",
        "P1.50/P200",
    ),
    atc(
        "DS107",
        "ACCEPTANCE OF BILLS OF EXCHANGE OR ORDER DRAWN IN A FOREIGN COUNTRY BUT PAYABLE IN THE PHILS",
        "P0.60/P200",
    ),
    atc(
        "DS108",
        "FOREIGN BILLS OF EXCHANGE AND LETTERS OF CREDIT",
        "P0.60/P200",
    ),
    atc(
        "DS109",
        "LIFE INSURANCE POLICIES",
        "Exempt, P20, P50, P100, P150, P200",
    ),
    atc("DS110", "POLICIES OF INSURANCE UPON PROPERTY", "P0.50/P4"),
    atc(
        "DS111",
        "FIDELITY BONDS AND OTHER INSURANCE POLICIES",
        "P0.50/P4",
    ),
    atc("DS112", "CAPITAL OF THE ANNUITIES", "P1.00/P200"),
    atc("DS112", "ON PRE-NEED PLANS", "P0.40/P200"),
    atc("DS113", "INDEMNITY BONDS", "P0.30/P4.00"),
    atc(
        "DS114",
        "CERTIFICATES-SECTION 188 OF THE TAX CODE",
        "P30.0 PER CERTIFICATE ISSUED",
    ),
    atc("DS115", "WAREHOUSE RECEIPTS", "P30.0 W/ VALUE ABOVE 200.01"),
    atc(
        "DS116",
        "JAI-ALAI, HORSE RACE TICKETS, LOTTO ETC.",
        "P0.20/P1.00",
    ),
    atc(
        "DS117",
        "BILLS OF LADING OR RECEIPTS",
        "P100 TO P1000 = P2.0 ABOVE P1000 = P20.0",
    ),
    atc(
        "DS118",
        "PROXIES FOR VOTING AT ANY ELECTION",
        "P30.0 PER ISSUED OF PROXY VOTING",
    ),
    atc(
        "DS119",
        "POWERS OF ATTORNEY",
        "P10.0 PER ISSUED OF POWER OF ATTORNEY",
    ),
    atc(
        "DS120",
        "LEASES AND OTHER HIRING AGREEMENTS",
        "1ST P2000.0 = P6.0 IN EXCESS P2.0/P1000.0",
    ),
    atc(
        "DS121",
        "MORTGAGES, PLEDGES AND DEED OF TRUST",
        "1ST P5000.0 = P40.0 IN EXCESS P20.0/P5000.0",
    ),
    atc(
        "DS124",
        "ON ASSIGNMENT AND RENEWALS OF CERTAIN INSTRUMENTS.",
        "15%",
    ),
    atc("DS126", "BILLS OF EXCHANGE OF DRAFTS", "P0.60/P200"),
];

/// The official option for an ATC code and description.
pub fn form_2000_atc_option(code: &str, description: &str) -> Option<&'static Form2000AtcOption> {
    FORM_2000_ATC_OPTIONS
        .iter()
        .find(|option| option.code == code && option.description == description)
}

// ── Helpers shared with the other DST / excise ports on this branch ──

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
pub(crate) fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// JavaScript `Number.prototype.toFixed(2)` read back as a number: the exact
/// binary value rounded half away from zero (ties pick the larger `n`).
pub(crate) fn js_to_fixed2(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let exact = format!("{:.80}", value.abs());
    let (whole, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits: Vec<u8> = whole.bytes().chain(frac.bytes().take(2)).collect();
    if frac.as_bytes().get(2).is_some_and(|d| *d >= b'5') {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let split = digits.len() - 2;
    let text = format!(
        "{}.{}",
        std::str::from_utf8(&digits[..split]).unwrap_or("0"),
        std::str::from_utf8(&digits[split..]).unwrap_or("00")
    );
    let magnitude: f64 = text.parse().unwrap_or(0.0);
    if value < 0.0 { -magnitude } else { magnitude }
}

/// `formatCurrency(NumWithComma(x.toFixed(2)))`, the shape of most official
/// tax-due assignments.
pub(crate) fn fixed_cents(value: f64) -> f64 {
    cents(js_to_fixed2(value))
}

/// `round()` turns an amount with more than 12 integer digits into `0.00`.
pub(crate) fn amount_in_official_range(value: f64) -> bool {
    value.is_finite() && value.abs() < 1e12
}

pub(crate) fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
pub(crate) fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

/// Item TIN checks shared by these forms: three 3-digit groups, a branch of
/// at most five digits, then the official check digit.
pub(crate) fn tin_error(tin: &str, format_message: &str) -> Option<String> {
    let (tin1, tin2, tin3, _) = split_tin(tin);
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    if tin1.len() != 3
        || tin2.len() != 3
        || tin3.len() != 3
        || digits.len() > 14
        || !tin.chars().all(|c| c.is_ascii_digit() || c == '-')
    {
        return Some(format_message.to_string());
    }
    if !crate::validation::relaxed_dev_mode()
        && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
    {
        return Some(crate::validation::OFFICIAL_INVALID_TIN_MESSAGE.to_string());
    }
    None
}

pub(crate) fn email_is_plausible(email: &str) -> bool {
    let email = email.trim();
    !email.is_empty() && email.contains('@') && !email.contains(char::is_whitespace)
}

/// The official `checkDateSched*` rules on an `MM/DD/YYYY` entry, stricter
/// in requiring the exact masked shape. `Err(true)` is a format error,
/// `Err(false)` an impossible or future-year date.
pub(crate) fn check_mmddyyyy(value: &str, current_year: i32) -> Result<(), bool> {
    let parts: Vec<&str> = value.split('/').collect();
    if parts.len() != 3
        || parts[0].len() != 2
        || parts[1].len() != 2
        || parts[2].len() != 4
        || !parts.iter().all(|p| digits_only(p))
    {
        return Err(true);
    }
    let month: u32 = parts[0].parse().map_err(|_| true)?;
    let day: u32 = parts[1].parse().map_err(|_| true)?;
    let year: i32 = parts[2].parse().map_err(|_| true)?;
    if year > current_year || chrono::NaiveDate::from_ymd_opt(year, month, day).is_none() {
        return Err(false);
    }
    Ok(())
}

/// One "Add"-able table: the id prefixes of a row's controls (each id is
/// prefix + row index), the rows the page starts with, and the rows wanted.
pub(crate) struct RowGroup<'a> {
    pub prefixes: &'a [&'a str],
    pub base_rows: usize,
    pub rows: usize,
}

/// The official layout with the rows an "Add" button writes into the DOM:
/// each extra row repeats the last built-in row's controls (ids renumbered)
/// right after it, which is where the upload loop meets them.
pub(crate) fn layout_with_rows(
    layout: &crate::official_xml::OfficialLayout,
    groups: &[RowGroup<'_>],
) -> crate::official_xml::OfficialLayout {
    use crate::official_xml::{Entry, Part};
    let mut out = layout.clone();
    for group in groups {
        if group.rows <= group.base_rows || group.base_rows == 0 {
            continue;
        }
        let last = group.base_rows - 1;
        let renumber = |key: &str, row: usize| -> Option<String> {
            group
                .prefixes
                .iter()
                .find(|prefix| key == format!("{prefix}{last}"))
                .map(|prefix| format!("{prefix}{row}"))
        };
        let positions: Vec<usize> = out
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                let key = match entry {
                    Entry::Bool { key, .. } | Entry::Value { key, .. } => key,
                };
                renumber(key, last).is_some()
            })
            .map(|(index, _)| index)
            .collect();
        let Some(&end) = positions.last() else {
            continue;
        };
        let template: Vec<Entry> = positions.iter().map(|&i| out.entries[i].clone()).collect();
        let mut added = Vec::new();
        for row in group.base_rows..group.rows {
            for entry in &template {
                let mut entry = entry.clone();
                match &mut entry {
                    Entry::Bool { key, .. } => *key = renumber(key, row).unwrap_or_default(),
                    Entry::Value { key, parts, .. } => {
                        *key = renumber(key, row).unwrap_or_default();
                        for part in parts {
                            if let Part::Source { source, .. } = part
                                && let Some(renamed) = renumber(source, row)
                            {
                                *source = renamed;
                            }
                        }
                    }
                }
                added.push(entry);
            }
        }
        out.entries.splice(end + 1..end + 1, added);
    }
    out
}

pub(crate) fn current_year() -> i32 {
    chrono::Datelike::year(&chrono::Local::now().date_naive())
}

// ── Model ──

/// Item 10 — Other party to the transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2000OtherParty {
    #[default]
    Unanswered,
    /// Creditor/Mortgagor/etc.
    Creditor,
    /// Debtor/Mortgagee/etc.
    Debtor,
    None,
}

impl Form2000OtherParty {
    pub fn has_other_party(self) -> bool {
        matches!(self, Self::Creditor | Self::Debtor)
    }
}

/// Item 13 — Mode of affixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2000AffixtureMode {
    #[default]
    Unanswered,
    /// eDST System: Schedule 3 (advance payments, Item 15C).
    Edst,
    /// Constructive Affixture: Schedule 2 (Item 15B) and Schedule 4.
    Constructive,
    /// Loose Stamps: Schedule 4.
    LooseStamps,
}

impl Form2000AffixtureMode {
    /// `processModeAffixture`: which of Schedules 2, 3 and 4 are enabled.
    pub fn schedules(self) -> (bool, bool, bool) {
        match self {
            Self::Edst => (false, true, false),
            Self::Constructive => (true, false, true),
            Self::LooseStamps => (false, false, true),
            Self::Unanswered => (false, false, false),
        }
    }
}

/// One Schedule 1 row: ATC, tax base, rate text (derived) and tax due.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2000AtcRow {
    pub atc_code: String,
    pub description: String,
    pub tax_base: f64,
    /// Computed, except for DS010 where the filer enters it.
    pub tax_due: f64,
}

impl Form2000AtcRow {
    pub fn is_empty(&self) -> bool {
        self.atc_code.is_empty()
    }

    pub fn option(&self) -> Option<&'static Form2000AtcOption> {
        form_2000_atc_option(&self.atc_code, &self.description)
    }

    pub fn tax_due_is_manual(&self) -> bool {
        self.atc_code == "DS010"
    }
}

/// One Schedule 2 / Schedule 3 payment row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2000PaymentRow {
    /// `MM/DD/YYYY`.
    pub date: String,
    pub receipt_number: String,
    pub amount: f64,
}

impl Form2000PaymentRow {
    fn is_blank(&self) -> bool {
        self.date.trim().is_empty() && self.receipt_number.trim().is_empty() && self.amount == 0.0
    }
}

/// One Schedule 4 remittance row (documentary stamps sold).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2000RemittanceRow {
    pub rco_code: String,
    /// `MM/DD/YYYY`.
    pub date: String,
    pub bank: String,
    pub amount: f64,
    pub number_from: String,
    pub number_to: String,
}

impl Form2000RemittanceRow {
    fn is_blank(&self) -> bool {
        self.rco_code.trim().is_empty()
            && self.date.trim().is_empty()
            && self.bank.trim().is_empty()
            && self.amount == 0.0
            && self.number_from.trim().is_empty()
            && self.number_to.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2000Draft {
    #[serde(default)]
    pub id: Option<i64>,

    // Part I
    /// Item 1 month (1–12).
    pub month: u8,
    /// Item 1 year.
    pub taxable_year: u16,
    /// Item 2.
    pub is_amended: bool,
    /// Item 3.
    #[serde(default)]
    pub number_of_attached_sheets: u16,
    /// Item 4: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    /// Item 5.
    pub rdo_code: String,
    /// Item 6.
    pub taxpayer_name: String,
    /// Item 7.
    pub registered_address: String,
    /// Item 7A.
    pub zip_code: String,
    /// Item 8.
    pub contact_number: String,
    /// Item 9.
    pub email: String,
    /// Hidden `txtLineBus`, loaded from the profile.
    #[serde(default)]
    pub line_of_business: String,
    /// Item 10.
    #[serde(default)]
    pub other_party: Form2000OtherParty,
    /// Item 11.
    #[serde(default)]
    pub other_party_name: String,
    /// Item 12.
    #[serde(default)]
    pub other_party_tin: String,
    /// Item 13.
    #[serde(default)]
    pub affixture_mode: Form2000AffixtureMode,

    // Schedules
    #[serde(default)]
    pub schedule1: Vec<Form2000AtcRow>,
    /// DS106 question: is the debt instrument's term less than a year?
    #[serde(default)]
    pub ds106_term_under_a_year: Option<bool>,
    /// DS106 term in days when it is under a year.
    #[serde(default)]
    pub ds106_term_days: u16,
    #[serde(default)]
    pub schedule2: Vec<Form2000PaymentRow>,
    #[serde(default)]
    pub schedule3: Vec<Form2000PaymentRow>,
    #[serde(default)]
    pub schedule4: Vec<Form2000RemittanceRow>,

    // Part II
    /// Item 14 (Schedule 1 total).
    #[serde(default)]
    pub tax_due: f64,
    /// Item 15A, enabled only on an amended return.
    #[serde(default)]
    pub balance_carried_over: f64,
    /// Item 15B (Schedule 2 total).
    #[serde(default)]
    pub constructive_affixture_payments: f64,
    /// Item 15C (Schedule 3 total).
    #[serde(default)]
    pub advance_payments: f64,
    /// Item 15D.
    #[serde(default)]
    pub total_credits: f64,
    /// Item 16.
    #[serde(default)]
    pub net_tax_payable: f64,
    /// Items 17A–17D.
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
    /// Item 18.
    #[serde(default)]
    pub total_amount_payable: f64,
    /// Item 19 (Schedule 4 total).
    #[serde(default)]
    pub stamps_sold: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// The four `frm2000:modLabel` texts as the page holds them, in layout order.
const MOD_LABELS: [&str; 4] = [
    "Is the term of Debt Instrument less than a year?",
    "Is the duration of the charter/contract is 6 months or less?",
    "Is the duration of the charter/contract is 6 months or less?",
    "Is the duration of the charter/contract is 6 months or less?            ",
];

impl Form2000Draft {
    pub const FORM_CODE: &'static str = "2000";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            month,
            taxable_year: year,
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
            other_party: Form2000OtherParty::Unanswered,
            other_party_name: String::new(),
            other_party_tin: String::new(),
            affixture_mode: Form2000AffixtureMode::Unanswered,
            schedule1: Vec::new(),
            ds106_term_under_a_year: None,
            ds106_term_days: 0,
            schedule2: Vec::new(),
            schedule3: Vec::new(),
            schedule4: Vec::new(),
            tax_due: 0.0,
            balance_carried_over: 0.0,
            constructive_affixture_payments: 0.0,
            advance_payments: 0.0,
            total_credits: 0.0,
            net_tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            stamps_sold: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Pick an ATC for a Schedule 1 row from the official dropdown.
    pub fn set_atc(&mut self, row: usize, option: &Form2000AtcOption) -> Result<(), String> {
        if row >= FORM_2000_MAX_ROWS {
            return Err(format!("Schedule 1 holds up to {FORM_2000_MAX_ROWS} rows."));
        }
        if option.code == "DS106"
            && self
                .schedule1
                .iter()
                .enumerate()
                .any(|(index, other)| index != row && other.atc_code == "DS106")
        {
            return Err(
                "DS106 can be used on one row only; its term question is shared.".to_string(),
            );
        }
        if self.schedule1.len() <= row {
            self.schedule1.resize_with(row + 1, Form2000AtcRow::default);
        }
        let base = self.schedule1[row].tax_base;
        self.schedule1[row] = Form2000AtcRow {
            atc_code: option.code.to_string(),
            description: option.description.to_string(),
            tax_base: base,
            tax_due: 0.0,
        };
        self.recompute();
        Ok(())
    }

    /// Clear a Schedule 1 row back to the blank official state.
    pub fn clear_atc(&mut self, row: usize) {
        if let Some(entry) = self.schedule1.get_mut(row) {
            *entry = Form2000AtcRow::default();
        }
        while self.schedule1.last().is_some_and(Form2000AtcRow::is_empty) {
            self.schedule1.pop();
        }
        self.recompute();
    }

    /// `capital()` (string-util.js) uppercases every text input except
    /// `txtEmail`. On this page only Item 11 (enabled for a creditor or
    /// debtor) calls it; the profile fields are disabled and loaded as stored.
    pub fn capital_applies(&self) -> bool {
        self.other_party.has_other_party()
    }

    fn has_ds106(&self) -> bool {
        self.schedule1.iter().any(|row| row.atc_code == "DS106")
    }

    /// Rate column text for a row, as `getATCdrpTaxRate` / `ds106CloseModal`
    /// leave it.
    pub fn rate_text(&self, row: &Form2000AtcRow) -> String {
        if row.is_empty() {
            return String::new();
        }
        if row.atc_code == "DS106"
            && self.ds106_term_under_a_year == Some(true)
            && self.ds106_term_days != 0
        {
            return format!("P1.5/200 * {}/365 days", self.ds106_term_days);
        }
        row.option()
            .map(|option| option.rate_text.to_string())
            .unwrap_or_default()
    }

    /// `computeSched1TaxDue` for one row; `None` for DS010 (filer-entered).
    fn computed_tax_due(&self, row: &Form2000AtcRow) -> Option<f64> {
        let base = row.tax_base;
        let fixed = |value: f64| Some(fixed_cents(value));
        match (row.atc_code.as_str(), row.description.as_str()) {
            ("DS010", _) => None,
            ("DS101", _) => fixed((base / 200.0) * 2.0),
            ("DS102", _) => fixed((base / 200.0) * 1.50),
            ("DS103", _) => fixed(base * 0.75),
            ("DS104", _) => fixed((base / 200.0) * 1.0),
            ("DS105", _) => fixed(base * 3.0),
            ("DS106", _) => {
                if self.ds106_term_under_a_year == Some(true) {
                    let days = f64::from(self.ds106_term_days);
                    fixed((base / 200.0) * 1.50 * (days / 365.0))
                } else {
                    fixed((base / 200.0) * 1.50)
                }
            }
            ("DS107" | "DS108" | "DS126", _) => fixed((base / 200.0) * 0.60),
            ("DS109", _) => fixed(if base <= 100_000.0 {
                0.0
            } else if base <= 300_000.0 {
                20.0
            } else if base <= 500_000.0 {
                50.0
            } else if base <= 750_000.0 {
                100.0
            } else if base <= 1_000_000.0 {
                150.0
            } else {
                200.0
            }),
            ("DS110" | "DS111", _) => fixed((base / 4.0) * 0.50),
            ("DS112", "ON PRE-NEED PLANS") => fixed((base / 200.0) * 0.40),
            ("DS112", _) => fixed((base / 200.0) * 1.0),
            ("DS113", _) => fixed((base / 4.0) * 0.30),
            ("DS114" | "DS118", _) => fixed(base * 30.0),
            ("DS115", _) => fixed(if base <= 200.0 { 0.0 } else { 30.0 }),
            ("DS116", _) => fixed((base / 1.0) * 0.20),
            ("DS117", _) => fixed(if base < 100.0 {
                0.0
            } else if base <= 1000.0 {
                2.0
            } else {
                20.0
            }),
            ("DS119", _) => fixed(base * 10.0),
            ("DS120", _) => fixed(if (1.0..=2000.0).contains(&base) {
                6.0
            } else if base > 2000.0 {
                6.0 + (((base - 2000.0) / 1000.0) * 2.0)
            } else {
                0.0
            }),
            ("DS121", _) => fixed(if (1.0..=5000.0).contains(&base) {
                40.0
            } else if base > 5000.0 {
                40.0 + (((base - 5000.0) / 5000.0) * 20.0)
            } else {
                0.0
            }),
            ("DS124", _) => fixed(base * 0.15),
            _ => Some(0.0),
        }
    }

    /// The official compute chain. Every input is held at cents the way
    /// `round(this,2)` leaves it; each derived item is the formatted value of
    /// the items it reads.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.balance_carried_over = 0.0;
        }
        if !self.other_party.has_other_party() {
            // processOtherParty clears Items 11 and 12.
            self.other_party_name.clear();
            self.other_party_tin.clear();
        }
        if !self.has_ds106() {
            self.ds106_term_under_a_year = None;
            self.ds106_term_days = 0;
        } else if self.ds106_term_under_a_year != Some(true) {
            // ds106BtnFunc resets the day count when "No" is picked.
            self.ds106_term_days = 0;
        }
        let (sched2, sched3, sched4) = self.affixture_mode.schedules();
        // disableSchedule* clears a schedule its mode does not use.
        if !sched2 {
            self.schedule2.clear();
        }
        if !sched3 {
            self.schedule3.clear();
        }
        if !sched4 {
            self.schedule4.clear();
        }

        // Schedule 1 → Item 14: a float sum, formatted once.
        let mut rows = std::mem::take(&mut self.schedule1);
        let mut total = 0.0;
        for row in &mut rows {
            row.tax_base = cents(row.tax_base);
            if row.is_empty() {
                row.tax_due = 0.0;
            } else if let Some(due) = self.computed_tax_due(row) {
                row.tax_due = due;
            } else {
                row.tax_due = cents(row.tax_due);
            }
            total += row.tax_due;
        }
        self.schedule1 = rows;
        self.tax_due = cents(total);

        // Schedules 2–4: the running total is formatted at every row.
        let running = |amounts: &mut dyn Iterator<Item = &mut f64>| {
            let mut total = 0.0;
            for amount in amounts {
                *amount = cents(*amount);
                total = cents(total + *amount);
            }
            total
        };
        self.constructive_affixture_payments =
            running(&mut self.schedule2.iter_mut().map(|row| &mut row.amount));
        self.advance_payments = running(&mut self.schedule3.iter_mut().map(|row| &mut row.amount));
        self.stamps_sold = running(&mut self.schedule4.iter_mut().map(|row| &mut row.amount));

        self.balance_carried_over = cents(self.balance_carried_over);
        self.total_credits = cents(
            self.balance_carried_over
                + self.constructive_affixture_payments
                + self.advance_payments,
        );
        // computeTax16: Constructive Affixture never goes below zero; the
        // other modes always show the difference as a negative amount.
        let net = if self.affixture_mode == Form2000AffixtureMode::Constructive {
            (self.tax_due - (self.balance_carried_over + self.constructive_affixture_payments))
                .max(0.0)
        } else {
            let value = self.tax_due
                - self.balance_carried_over
                - self.constructive_affixture_payments
                - self.advance_payments;
            if value > 0.0 { -value } else { value }
        };
        self.net_tax_payable = cents(net);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_payable = cents(self.net_tax_payable + self.total_penalties);
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:page_header`: the page 2 header, one 40-slot comb under the
    ///   TIN and Taxpayer's Name captions (14 TIN digits, then the name).
    /// - `derived:contact_email`: Items 8 and 9 on their one shared 40-slot
    ///   comb (the number from slot 0, the email as typed from slot 13).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        fields.insert(
            "derived:page_header".to_string(),
            super::print_map::tin_name_header(&self.tin, &self.taxpayer_name),
        );
        let contact = super::excise_places::contact_email_comb_line(
            &self.contact_number,
            &self.email,
            13,
            40,
        );
        fields.insert("derived:contact_email".to_string(), contact);
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let upper = self.capital_applies();
        let text = |value: &str| {
            let value = value.trim();
            if upper {
                value.to_uppercase()
            } else {
                value.to_string()
            }
        };
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2000:{key}"), value);
        };
        let flag = |on: bool| on.to_string();

        put("txtMonth", format!("{:02}", self.month));
        put("txtYear", self.taxable_year.to_string());
        put("AmendedRtn_1", flag(self.is_amended));
        put("AmendedRtn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        for (key, value) in [
            ("txtTIN1", &tin1),
            ("txtTIN2", &tin2),
            ("txtTIN3", &tin3),
            ("txtBranchCode", &branch),
            ("txtPg2TIN1", &tin1),
            ("txtPg2TIN2", &tin2),
            ("txtPg2TIN3", &tin3),
            ("txtPg2BranchCode", &branch),
        ] {
            put(key, value.clone());
        }
        put("txtRDOCode", self.rdo_code.trim().to_string());
        put("txtTaxpayerName", text(&self.taxpayer_name));
        put("txtPg2TaxpayerName", text(&self.taxpayer_name));
        // The two address boxes are written as one escaped value.
        put("txtAddress", text(&self.registered_address));
        put("txtAddress2", String::new());
        put("txtZipCode", self.zip_code.trim().to_string());
        put("txtTelNum", self.contact_number.trim().to_string());
        put("txtLineBus", text(&self.line_of_business));
        // The page the filer is on; the return is validated from page 1.
        put("txtCurrentPage", "1".to_string());
        put(
            "optParty_1",
            flag(self.other_party == Form2000OtherParty::Creditor),
        );
        put(
            "optParty_2",
            flag(self.other_party == Form2000OtherParty::Debtor),
        );
        put(
            "optParty_3",
            flag(self.other_party == Form2000OtherParty::None),
        );
        put("txtOtherName", text(&self.other_party_name));
        put("txtOtherName2", String::new());
        put("txtOtherTIN", self.other_party_tin.trim().to_string());
        put(
            "optMode_1",
            flag(self.affixture_mode == Form2000AffixtureMode::Edst),
        );
        put(
            "optMode_2",
            flag(self.affixture_mode == Form2000AffixtureMode::Constructive),
        );
        put(
            "optMode_3",
            flag(self.affixture_mode == Form2000AffixtureMode::LooseStamps),
        );

        put("txtTax14", official_amount(self.tax_due));
        put("txtTax15A", official_amount(self.balance_carried_over));
        put(
            "txtTax15B",
            official_amount(self.constructive_affixture_payments),
        );
        put("txtTax15C", official_amount(self.advance_payments));
        put("txtTax15D", official_amount(self.total_credits));
        put("txtTax16", official_amount(self.net_tax_payable));
        put("txtTax17A", official_amount(self.surcharge));
        put("txtTax17B", official_amount(self.interest));
        put("txtTax17C", official_amount(self.compromise));
        put("txtTax17D", official_amount(self.total_penalties));
        put("txtTax18", official_amount(self.total_amount_payable));
        put("txtTax19", official_amount(self.stamps_sold));

        for index in 0..self.schedule1.len().max(FORM_2000_ROWS) {
            let row = self.schedule1.get(index).cloned().unwrap_or_default();
            let code = if row.is_empty() {
                "-".to_string()
            } else {
                row.atc_code.clone()
            };
            fields.insert(format!("drpATCCode{index}"), code);
            let mut put = |key: &str, value: String| {
                fields.insert(format!("frm2000:sched1:{key}{index}"), value);
            };
            put("txtTaxBase", official_amount(row.tax_base));
            put("txtTaxRate", text(&self.rate_text(&row)));
            put("txtTaxDue", official_amount(row.tax_due));
        }
        fields.insert(
            "frm2000:sched1:txtTotalDue1".to_string(),
            official_amount(self.tax_due),
        );
        for (sched, rows, total) in [
            (
                "sched2",
                &self.schedule2,
                self.constructive_affixture_payments,
            ),
            ("sched3", &self.schedule3, self.advance_payments),
        ] {
            for index in 0..rows.len().max(FORM_2000_ROWS) {
                let row = rows.get(index).cloned().unwrap_or_default();
                let mut put = |key: &str, value: String| {
                    fields.insert(format!("frm2000:{sched}:{key}{index}"), value);
                };
                put("txtPaymentDate", text(&row.date));
                put("txtReceipt", text(&row.receipt_number));
                put("txtAmountPaid", official_amount(row.amount));
            }
            fields.insert(
                format!("frm2000:{sched}:txtTotalPayment1"),
                official_amount(total),
            );
        }
        for index in 0..self.schedule4.len().max(FORM_2000_ROWS) {
            let row = self.schedule4.get(index).cloned().unwrap_or_default();
            let mut put = |key: &str, value: String| {
                fields.insert(format!("frm2000:sched4:{key}{index}"), value);
            };
            put("txtRCOCode", text(&row.rco_code));
            put("txtRemittanceDate", text(&row.date));
            put("txtBank", text(&row.bank));
            put("txtAmountRemitted", official_amount(row.amount));
            put("txtNumberFrom", row.number_from.trim().to_string());
            put("txtNumberTo", row.number_to.trim().to_string());
        }
        fields.insert(
            "frm2000:sched4:txtTotalRemittance1".to_string(),
            official_amount(self.stamps_sold),
        );

        // The DS106 term question (its modal's controls are in the form).
        fields.insert(
            "ds106modYes".to_string(),
            flag(self.ds106_term_under_a_year == Some(true)),
        );
        fields.insert(
            "ds106modNo".to_string(),
            flag(self.ds106_term_under_a_year == Some(false)),
        );
        fields.insert(
            "frm2000:numOfDays".to_string(),
            self.ds106_term_days.to_string(),
        );

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The official layout with this return's added schedule rows.
    pub fn official_layout(
        &self,
    ) -> Result<crate::official_xml::OfficialLayout, Vec<(String, String)>> {
        let base = crate::official_xml::layout(FORM_2000_FORM_ID)
            .map_err(|error| vec![("xml".to_string(), error.to_string())])?;
        let payment = |n: u8| {
            [
                format!("chkSchedule{n}Delete"),
                format!("frm2000:sched{n}:txtPaymentDate"),
                format!("frm2000:sched{n}:txtReceipt"),
                format!("frm2000:sched{n}:txtAmountPaid"),
            ]
        };
        let (p2, p3) = (payment(2), payment(3));
        let p2: Vec<&str> = p2.iter().map(String::as_str).collect();
        let p3: Vec<&str> = p3.iter().map(String::as_str).collect();
        let groups = [
            RowGroup {
                prefixes: &[
                    "chkSchedule1Delete",
                    "drpATCCode",
                    "frm2000:sched1:txtTaxBase",
                    "frm2000:sched1:txtTaxRate",
                    "frm2000:sched1:txtTaxDue",
                ],
                base_rows: FORM_2000_ROWS,
                rows: self.schedule1.len(),
            },
            RowGroup {
                prefixes: &p2,
                base_rows: FORM_2000_ROWS,
                rows: self.schedule2.len(),
            },
            RowGroup {
                prefixes: &p3,
                base_rows: FORM_2000_ROWS,
                rows: self.schedule3.len(),
            },
            RowGroup {
                prefixes: &[
                    "chkSchedule4Delete",
                    "frm2000:sched4:txtRCOCode",
                    "frm2000:sched4:txtRemittanceDate",
                    "frm2000:sched4:txtBank",
                    "frm2000:sched4:txtAmountRemitted",
                    "frm2000:sched4:txtNumberFrom",
                    "frm2000:sched4:txtNumberTo",
                ],
                base_rows: FORM_2000_ROWS,
                rows: self.schedule4.len(),
            },
        ];
        Ok(layout_with_rows(base, &groups))
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

impl FormValidator for Form2000Draft {
    /// `validate()` in order with its alert texts, then the limits the page
    /// enforces while typing and the schedule rules it leaves implicit.
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let this_year = current_year();

        if !(1..=12).contains(&self.month) {
            err("month", "Please select a valid month in Item 1.");
        }
        if self.taxable_year == 0 {
            err("taxable_year", "Please enter a valid year in Item 1.");
        } else if self.taxable_year < 2018 {
            err(
                "taxable_year",
                "Please file using the old version of the form.",
            );
        } else if i32::from(self.taxable_year) > this_year {
            err(
                "taxable_year",
                "Year (Item 1) cannot be greater than or equal to current year.",
            );
        }
        if let Some(message) = tin_error(&self.tin, "Please enter a valid TIN number on Item 4.") {
            err("tin", &message);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        if self.taxpayer_name.trim().is_empty() {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 6.",
            );
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 20 || !digits_only(phone) {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 8.",
            );
        }
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > 150 {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 7.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 7A.");
        }
        if self.other_party == Form2000OtherParty::Unanswered {
            err("other_party", "Please select an option in Item 10.");
        }
        if self.other_party.has_other_party() {
            let tin = self.other_party_tin.trim();
            if tin.is_empty() || tin.len() > 14 || !digits_only(tin) {
                err("other_party_tin", "Please enter a valid TIN in Item 12.");
            }
            // Item 11 is what applies capital() to the page; ask for it.
            let name = self.other_party_name.trim();
            if name.is_empty() || name.chars().count() > 130 {
                err(
                    "other_party_name",
                    "Enter the other party's name in Item 11.",
                );
            }
        }
        if self.affixture_mode == Form2000AffixtureMode::Unanswered {
            err("affixture_mode", "Please select an option in Item 13.");
        }
        // The official "eDST System and Loose Stamps" zero-payment check needs
        // two Item 13 radios checked at once, which a radio group never has.

        let (sched2, sched3, sched4) = self.affixture_mode.schedules();
        let date_errors = |rows: Vec<&str>, schedule: u8, column: u8| -> Vec<(String, String)> {
            rows.iter()
                .enumerate()
                .filter(|(_, date)| !date.trim().is_empty())
                .find_map(|(index, date)| {
                    check_mmddyyyy(date.trim(), this_year).err().map(|format| {
                        let message = if format {
                            format!(
                                "Invalid date entry in Schedule {schedule}, column {column} row {index}.Format should be MM/DD/YYYY"
                            )
                        } else {
                            format!(
                                "Please enter a valid date in Schedule {schedule}, column {column} row {index}."
                            )
                        };
                        (format!("schedule{schedule}[{index}].date"), message)
                    })
                })
                .into_iter()
                .collect()
        };
        if sched2 {
            for (field, message) in date_errors(
                self.schedule2.iter().map(|r| r.date.as_str()).collect(),
                2,
                1,
            ) {
                err(&field, &message);
            }
        }
        if sched3 {
            for (field, message) in date_errors(
                self.schedule3.iter().map(|r| r.date.as_str()).collect(),
                3,
                1,
            ) {
                err(&field, &message);
            }
        }
        // The official page checks Schedule 4 dates only for Loose Stamps
        // although Constructive Affixture enables it too; check both.
        if sched4 {
            for (field, message) in date_errors(
                self.schedule4.iter().map(|r| r.date.as_str()).collect(),
                4,
                2,
            ) {
                err(&field, &message);
            }
        }

        // ── Limits the page enforces while typing, and implicit rules ──
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most two digits.",
            );
        }
        if self.schedule1.len() > FORM_2000_MAX_ROWS {
            err("schedule1", "Schedule 1 holds up to 50 rows here.");
        }
        let mut ds106_rows = 0;
        for (index, row) in self.schedule1.iter().enumerate() {
            let label = index + 1;
            if row.is_empty() {
                if row.tax_base != 0.0 {
                    err(
                        &format!("schedule1[{index}].atc_code"),
                        &format!(
                            "Schedule 1 row {label}: select an ATC before entering a tax base."
                        ),
                    );
                }
                continue;
            }
            if row.option().is_none() {
                err(
                    &format!("schedule1[{index}].atc_code"),
                    &format!(
                        "Schedule 1 row {label}: {} is not an ATC this form can compute.",
                        row.atc_code
                    ),
                );
                continue;
            }
            if row.atc_code == "DS106" {
                ds106_rows += 1;
            }
            if row.tax_base <= 0.0
                || !has_cent_precision(row.tax_base)
                || !amount_in_official_range(row.tax_base)
            {
                err(
                    &format!("schedule1[{index}].tax_base"),
                    &format!("Schedule 1 row {label}: enter the tax base."),
                );
            }
            if row.tax_due_is_manual()
                && (row.tax_due <= 0.0
                    || !has_cent_precision(row.tax_due)
                    || !amount_in_official_range(row.tax_due))
            {
                err(
                    &format!("schedule1[{index}].tax_due"),
                    &format!("Schedule 1 row {label}: enter the tax due for DS010."),
                );
            }
        }
        if ds106_rows > 1 {
            err(
                "schedule1",
                "DS106 can be used on one row only; its term question is shared.",
            );
        }
        if ds106_rows > 0 {
            match self.ds106_term_under_a_year {
                None => err(
                    "ds106_term_under_a_year",
                    "Please select an option for the Debt Instrument.",
                ),
                Some(true) if !(1..=365).contains(&self.ds106_term_days) => err(
                    "ds106_term_days",
                    "Number of days should be from 1 to 365 only.",
                ),
                _ => {}
            }
        }
        for (schedule, rows) in [(2u8, &self.schedule2), (3u8, &self.schedule3)] {
            if rows.len() > FORM_2000_MAX_ROWS {
                err(
                    &format!("schedule{schedule}"),
                    &format!("Schedule {schedule} holds up to 50 rows here."),
                );
            }
            for (index, row) in rows.iter().enumerate() {
                let label = index + 1;
                if row.is_blank() {
                    continue;
                }
                if row.date.trim().is_empty()
                    || row.amount <= 0.0
                    || !has_cent_precision(row.amount)
                    || !amount_in_official_range(row.amount)
                {
                    err(
                        &format!("schedule{schedule}[{index}].amount"),
                        &format!(
                            "Schedule {schedule} row {label}: enter the payment date and amount."
                        ),
                    );
                }
                let receipt = row.receipt_number.trim();
                if receipt.chars().count() > 20
                    || !receipt.chars().all(|c| c.is_ascii_alphanumeric())
                {
                    err(
                        &format!("schedule{schedule}[{index}].receipt_number"),
                        &format!(
                            "Schedule {schedule} row {label}: the receipt number takes up to 20 letters and digits."
                        ),
                    );
                }
            }
        }
        if self.schedule4.len() > FORM_2000_MAX_ROWS {
            err("schedule4", "Schedule 4 holds up to 50 rows here.");
        }
        for (index, row) in self.schedule4.iter().enumerate() {
            let label = index + 1;
            if row.is_blank() {
                continue;
            }
            if row.date.trim().is_empty()
                || row.amount <= 0.0
                || !has_cent_precision(row.amount)
                || !amount_in_official_range(row.amount)
            {
                err(
                    &format!("schedule4[{index}].amount"),
                    &format!("Schedule 4 row {label}: enter the remittance date and amount."),
                );
            }
            if row.rco_code.trim().chars().count() > 10 || row.bank.trim().chars().count() > 20 {
                err(
                    &format!("schedule4[{index}].bank"),
                    &format!(
                        "Schedule 4 row {label}: the RCO code takes up to 10 characters and the bank up to 20."
                    ),
                );
            }
            for number in [&row.number_from, &row.number_to] {
                let number = number.trim();
                if number.len() > 10 || !digits_only(number) {
                    err(
                        &format!("schedule4[{index}].number_from"),
                        &format!(
                            "Schedule 4 row {label}: loose stamp numbers take up to 10 digits."
                        ),
                    );
                    break;
                }
            }
        }

        for (field, value) in [
            ("balance_carried_over", self.balance_carried_over),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ] {
            if value < 0.0 || !has_cent_precision(value) || !amount_in_official_range(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }
        if !self.is_amended && self.balance_carried_over != 0.0 {
            err(
                "balance_carried_over",
                "Item 15A is open only on an amended return.",
            );
        }
        if !email_is_plausible(&self.email) {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }

        let mut expected = self.clone();
        expected.recompute();
        if expected != *self {
            err(
                "total_amount_payable",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }
}

impl Form2000Draft {
    /// Items 17A-17C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "17A Surcharge",
                interest: "17B Interest",
                compromise: "17C Compromise",
            ],
            controls: [
                "frm2000:txtTax17A",
                "frm2000:txtTax17B",
                "frm2000:txtTax17C",
            ],
            state: penalty,
            basic_tax_due: "Item 16", |d| d.net_tax_payable,
        )
    }
}

impl QueueableForm for Form2000Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2000";
    /// Official `formType` (`2000v2018`).
    const FORM_TYPE: &'static str = "2000v2018";
    const LAYOUT_ID: &'static str = FORM_2000_FORM_ID;

    /// Total Amount Payable.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.total_amount_payable)
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
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Monthly(self.month)
    }
    /// `txtMonth + txtYear`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}{}", self.month, self.taxable_year)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 6 || !digits_only(code) {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let year: u16 = code.get(2..)?.parse().ok()?;
        (1..=12)
            .contains(&month)
            .then_some((year, FilingPeriod::Monthly(month)))
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

    /// The layout writer plus one page quirk: the four read-only
    /// `frm2000:modLabel` boxes share an id but hold different texts, and
    /// `capital()` uppercases them with every other text box.
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as QueueableForm>::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let layout = self.official_layout()?;
        let mut text = crate::official_xml::write(&layout, &self.field_map())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])?;
        if self.capital_applies() {
            for label in MOD_LABELS {
                let key = "frm2000:modLabel";
                text = text.replace(
                    &format!("<div>{key}={label}{key}=</div>"),
                    &format!("<div>{key}={}{key}=</div>", label.to_uppercase()),
                );
            }
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(code: &str, description: &str) -> &'static Form2000AtcOption {
        form_2000_atc_option(code, description).unwrap()
    }

    pub(crate) fn sample() -> Form2000Draft {
        let mut draft = Form2000Draft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            month: 6,
            taxable_year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Taxpayer Inc".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            contact_number: "0281234567".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            line_of_business: "Lending".to_string(),
            other_party: Form2000OtherParty::None,
            other_party_name: String::new(),
            other_party_tin: String::new(),
            affixture_mode: Form2000AffixtureMode::Edst,
            schedule1: Vec::new(),
            ds106_term_under_a_year: None,
            ds106_term_days: 0,
            schedule2: Vec::new(),
            schedule3: Vec::new(),
            schedule4: Vec::new(),
            tax_due: 0.0,
            balance_carried_over: 0.0,
            constructive_affixture_payments: 0.0,
            advance_payments: 0.0,
            total_credits: 0.0,
            net_tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            stamps_sold: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft
            .set_atc(0, option("DS101", "ORIGINAL ISSUE OF SHARES OF STOCKS"))
            .unwrap();
        draft.schedule1[0].tax_base = 123_456.789;
        draft.schedule3.push(Form2000PaymentRow {
            date: "06/10/2025".into(),
            receipt_number: "ADV001".into(),
            amount: 500.0,
        });
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2000Draft) -> Vec<String> {
        <Form2000Draft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn js_to_fixed_matches_javascript() {
        assert_eq!(js_to_fixed2(1.005), 1.0); // 1.00499999…
        assert_eq!(js_to_fixed2(0.125), 0.13); // exact tie → larger
        assert_eq!(js_to_fixed2(1234.5679), 1234.57);
        assert_eq!(js_to_fixed2(-2.345), -2.35); // -2.34500000000000019…
        assert_eq!(js_to_fixed2(9.999), 10.0);
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert_eq!(draft.schedule1[0].tax_base, 123_456.79);
        assert_eq!(draft.schedule1[0].tax_due, 1_234.57);
        assert_eq!(draft.tax_due, 1_234.57);
        assert_eq!(draft.advance_payments, 500.0);
        assert_eq!(draft.total_credits, 500.0);
        // eDST: the Item 14 less 15D difference is shown negative.
        assert_eq!(draft.net_tax_payable, -734.57);
        assert_eq!(draft.total_amount_payable, -734.57);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn constructive_affixture_floors_item_16_at_zero() {
        let mut draft = sample();
        draft.affixture_mode = Form2000AffixtureMode::Constructive;
        draft.schedule2.push(Form2000PaymentRow {
            date: "06/11/2025".into(),
            receipt_number: "R1".into(),
            amount: 2_000.0,
        });
        draft.recompute();
        assert!(draft.schedule3.is_empty());
        assert_eq!(draft.constructive_affixture_payments, 2_000.0);
        assert_eq!(draft.net_tax_payable, 0.0);
        draft.schedule2[0].amount = 1_000.0;
        draft.surcharge = 25.5;
        draft.keep_entered_penalties();
        draft.recompute();
        assert_eq!(draft.net_tax_payable, 234.57);
        assert_eq!(draft.total_amount_payable, 260.07);
    }

    #[test]
    fn atc_computations_follow_compute_sched1_tax_due() {
        let cases: &[(&str, &str, f64, f64)] = &[
            ("DS102", FORM_2000_ATC_OPTIONS[2].description, 1_000.0, 7.5),
            ("DS103", FORM_2000_ATC_OPTIONS[3].description, 100.0, 75.0),
            ("DS105", FORM_2000_ATC_OPTIONS[5].description, 4.0, 12.0),
            ("DS109", "LIFE INSURANCE POLICIES", 300_000.0, 20.0),
            ("DS109", "LIFE INSURANCE POLICIES", 300_000.01, 50.0),
            ("DS110", "POLICIES OF INSURANCE UPON PROPERTY", 10.0, 1.25),
            ("DS112", "CAPITAL OF THE ANNUITIES", 1_000.0, 5.0),
            ("DS112", "ON PRE-NEED PLANS", 1_000.0, 2.0),
            ("DS115", "WAREHOUSE RECEIPTS", 200.0, 0.0),
            ("DS117", "BILLS OF LADING OR RECEIPTS", 1_000.0, 2.0),
            ("DS117", "BILLS OF LADING OR RECEIPTS", 1_000.01, 20.0),
            ("DS120", "LEASES AND OTHER HIRING AGREEMENTS", 5_000.0, 12.0),
            (
                "DS121",
                "MORTGAGES, PLEDGES AND DEED OF TRUST",
                7_500.0,
                50.0,
            ),
        ];
        for (code, description, base, due) in cases {
            let mut draft = sample();
            draft.set_atc(0, option(code, description)).unwrap();
            draft.schedule1[0].tax_base = *base;
            draft.recompute();
            assert_eq!(
                draft.schedule1[0].tax_due, *due,
                "{code} {description} {base}"
            );
        }
    }

    #[test]
    fn ds106_term_question() {
        let mut draft = sample();
        draft
            .set_atc(1, option("DS106", "ORIGINAL ISSUE OF ALL DEBT INSTRUMENTS"))
            .unwrap();
        draft.schedule1[1].tax_base = 2_500_000.0;
        draft.recompute();
        assert!(
            messages(&draft)
                .contains(&"Please select an option for the Debt Instrument.".to_string())
        );
        draft.ds106_term_under_a_year = Some(true);
        draft.recompute();
        assert!(
            messages(&draft).contains(&"Number of days should be from 1 to 365 only.".to_string())
        );
        draft.ds106_term_days = 90;
        draft.recompute();
        assert_eq!(draft.schedule1[1].tax_due, 4_623.29);
        assert_eq!(
            draft.rate_text(&draft.schedule1[1]),
            "P1.5/200 * 90/365 days"
        );
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["ds106modYes"], "true");
        assert_eq!(fields["frm2000:numOfDays"], "90");
        draft.ds106_term_under_a_year = Some(false);
        draft.recompute();
        assert_eq!(draft.ds106_term_days, 0);
        assert_eq!(draft.schedule1[1].tax_due, 18_750.0);
        assert_eq!(
            draft.set_atc(2, option("DS106", "ORIGINAL ISSUE OF ALL DEBT INSTRUMENTS")),
            Err("DS106 can be used on one row only; its term question is shared.".to_string())
        );
    }

    #[test]
    fn field_map_uses_official_formats_and_capital_rule() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm2000:txtMonth"], "06");
        assert_eq!(fields["drpATCCode0"], "DS101");
        assert_eq!(fields["drpATCCode1"], "-");
        assert_eq!(fields["frm2000:sched1:txtTaxBase0"], "123,456.79");
        assert_eq!(fields["frm2000:sched1:txtTaxRate0"], "P2.00/P200");
        assert_eq!(fields["frm2000:txtTax16"], "-734.57");
        // No other party: capital() never runs, profile text stays as stored.
        assert_eq!(fields["frm2000:txtTaxpayerName"], "Sample Taxpayer Inc");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        let payload = draft.to_bir_xml_payload().unwrap();
        assert!(payload.contains("Is the term of Debt Instrument less than a year?"));

        let mut creditor = draft.clone();
        creditor.other_party = Form2000OtherParty::Creditor;
        creditor.other_party_name = "Sample Creditor Corp".into();
        creditor.other_party_tin = "987654321".into();
        creditor.recompute();
        let fields = creditor.to_bir_field_map();
        assert_eq!(fields["frm2000:txtTaxpayerName"], "SAMPLE TAXPAYER INC");
        assert_eq!(fields["frm2000:txtOtherName"], "SAMPLE CREDITOR CORP");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        let payload = creditor.to_bir_xml_payload().unwrap();
        assert!(payload.contains("IS THE TERM OF DEBT INSTRUMENT LESS THAN A YEAR?"));
        assert_eq!(
            creditor.submission_filename(),
            "12345678800000-2000v2018-062025#sample.taxpayer@example.com#.xml"
        );
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "062025");
        assert_eq!(
            Form2000Draft::parse_period_code("062025"),
            Some((2025, FilingPeriod::Monthly(6)))
        );
        assert_eq!(Form2000Draft::parse_period_code("132025"), None);
        assert_eq!(Form2000Draft::parse_period_code("06202"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2000Draft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            draft.recompute();
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(&|d| d.month = 0, "Please select a valid month in Item 1.");
        check(
            &|d| d.taxable_year = 0,
            "Please enter a valid year in Item 1.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Please file using the old version of the form.",
        );
        check(
            &|d| d.taxable_year = 9999,
            "Year (Item 1) cannot be greater than or equal to current year.",
        );
        check(
            &|d| d.tin = "123".into(),
            "Please enter a valid TIN number on Item 4.",
        );
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 5.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 6.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number on Item 8.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 7.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 7A.",
        );
        check(
            &|d| d.other_party = Form2000OtherParty::Unanswered,
            "Please select an option in Item 10.",
        );
        check(
            &|d| {
                d.other_party = Form2000OtherParty::Debtor;
                d.other_party_name = "x".into();
            },
            "Please enter a valid TIN in Item 12.",
        );
        check(
            &|d| d.affixture_mode = Form2000AffixtureMode::Unanswered,
            "Please select an option in Item 13.",
        );
        check(
            &|d| d.schedule3[0].date = "6/10/2025".into(),
            "Invalid date entry in Schedule 3, column 1 row 0.Format should be MM/DD/YYYY",
        );
        check(
            &|d| d.schedule3[0].date = "02/29/2025".into(),
            "Please enter a valid date in Schedule 3, column 1 row 0.",
        );
        check(
            &|d| {
                d.affixture_mode = Form2000AffixtureMode::LooseStamps;
                d.schedule4.push(Form2000RemittanceRow {
                    rco_code: "1".into(),
                    date: "13/01/2025".into(),
                    bank: "B".into(),
                    amount: 1.0,
                    number_from: "1".into(),
                    number_to: "2".into(),
                });
            },
            "Please enter a valid date in Schedule 4, column 2 row 0.",
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
            <Form2000Draft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2000Draft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
