//! BIR Form 2552 (January 2018 ENCS) — Percentage Tax Return for
//! Transactions Involving Shares of Stock Listed and Traded through the Local
//! Stock Exchange or through Initial and/or Secondary Public Offering.
//!
//! Ported from the official `BIR-Form2552v2018.hta`: the compute chain
//! (`pageTwoComputation` → `totTaxDueComp` → `pageTwoSchedTotalToPageOne` →
//! `pageOneComputation`), the enable/reset handlers (`isAmended`,
//! `availTaxRelief`, `kindOfTransaction`, `enableSchedule`,
//! `handleOverpayment`), `validateAll` with its exact alert texts, and
//! the uploaded file (`saveEncryptedProfile`) through [`crate::official_xml`].
//!
//! Every derived amount on this form goes through `(…).toFixed(0)` before
//! `formatCurrency`, so tax due and every total is a whole peso while the
//! filer's inputs keep centavos.
//!
//! Part V takes more than five transactions the way the official
//! "(Add More...)" popup does: the fifth and later rows go to the popup
//! (`Pg2Pt5Sch<n>PopTable1st/2nd`), row 5 shows `OTHERS` with the popup
//! subtotal, and [`Form2552Draft::official_payload`] splices the popup's
//! controls in ahead of `Pg2Pt5Sch<n>SubTotal`, where they sit in the DOM:
//! the upload (`saveEncryptedProfile`) writes every `frmMain` control in DOM
//! order, and the popup rows exist only once the filer adds them.

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
pub const FORM_2552_FORM_ID: &str = "2552-v2018";
/// Rows in Part IV, each Part V schedule and Part VI.
pub const FORM_2552_ROWS: usize = 5;
/// Part V transactions this model accepts, popup rows included.
pub const FORM_2552_MAX_SCHEDULE_ROWS: usize = 100;
/// Row 5's text once the popup holds two or more rows.
const OTHERS: &str = "OTHERS";
/// Schedule 1 (LSE) rate text and rate: `6/10 of 1%`.
pub const FORM_2552_LSE_RATE_TEXT: &str = "6/10 of 1%";
pub const FORM_2552_LSE_RATE: f64 = 0.6;
/// The page enforces these while typing (`maxlength`).
const TEXT_MAX: usize = 50;
const PART_IV_CLASS_MAX: usize = 80;
const LONG_TEXT_MAX: usize = 100;
const ADDRESS_LINE: usize = 80;

/// Item 11 — Kind of transaction. Picks the Part V schedule in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2552Transaction {
    #[default]
    Unspecified,
    /// 11A — Listed and traded through the LSE (Schedule 1, ATC PT 200).
    LocalStockExchange,
    /// 11B — Initial public offering, primary (Schedule 2, ATC PT 201).
    PrimaryOffering,
    /// 11B — Initial public offering, secondary (Schedule 3, ATC PT 202).
    SecondaryOffering,
}

impl Form2552Transaction {
    /// Part V schedule number (1–3).
    pub fn schedule(self) -> Option<u8> {
        match self {
            Self::Unspecified => None,
            Self::LocalStockExchange => Some(1),
            Self::PrimaryOffering => Some(2),
            Self::SecondaryOffering => Some(3),
        }
    }

    pub fn is_public_offering(self) -> bool {
        matches!(self, Self::PrimaryOffering | Self::SecondaryOffering)
    }
}

/// Overpayment boxes after Item 25.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2552Overpayment {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
}

/// Part IV — a transaction not subject to tax.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2552ExemptRow {
    pub classification: String,
    pub amount: f64,
}

/// Part V — one taxable transaction (columns a–h).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2552ShareRow {
    /// `MM/DD/YYYY`.
    pub date: String,
    pub seller: String,
    pub buyer: String,
    pub issuing_corporation: String,
    pub number_of_shares: f64,
    pub tax_base: f64,
    /// Percent. Schedule 1 is fixed at 0.6 (`6/10 of 1%`).
    pub tax_rate: f64,
    pub tax_due: f64,
}

/// Part VI — a transaction not subject to percentage tax.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2552NonTaxableRow {
    /// `MM/DD/YYYY`.
    pub date: String,
    pub seller: String,
    pub buyer: String,
    pub issuing_corporation: String,
    pub number_of_shares: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2552Draft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–3
    /// Item 1 — date of transaction (or of listing).
    pub transaction_month: u8,
    pub transaction_day: u8,
    pub transaction_year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub rdo_code: String,
    pub taxpayer_name: String,
    /// Up to 160 characters; the page splits it into two 80-character lines.
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,
    /// The profile's line of business (hidden `txtLOB`).
    #[serde(default)]
    pub line_of_business: String,
    /// Item 10 and 10A.
    #[serde(default)]
    pub tax_relief: bool,
    #[serde(default)]
    pub tax_relief_specification: String,
    /// Item 11.
    #[serde(default)]
    pub transaction: Form2552Transaction,
    /// Item 12A / 12B (public offerings only).
    #[serde(default)]
    pub shares_sold: String,
    #[serde(default)]
    pub outstanding_shares: String,

    // Part II
    /// Items 13–15.
    #[serde(default)]
    pub tax_due_lse: f64,
    #[serde(default)]
    pub tax_due_primary: f64,
    #[serde(default)]
    pub tax_due_secondary: f64,
    /// Item 16.
    #[serde(default)]
    pub total_tax_due: f64,
    /// Item 17, amended returns only.
    #[serde(default)]
    pub tax_paid_previous: f64,
    /// Item 18.
    #[serde(default)]
    pub creditable_tax_withheld: f64,
    /// Item 19.
    #[serde(default)]
    pub total_tax_credits: f64,
    /// Item 20.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 21–24.
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
    /// Item 25.
    #[serde(default)]
    pub total_amount_payable: f64,
    #[serde(default)]
    pub overpayment: Form2552Overpayment,

    // Page 2
    /// Part IV rows in order.
    #[serde(default)]
    pub exempt_transactions: Vec<Form2552ExemptRow>,
    /// Part V rows of the schedule Item 11 selects.
    #[serde(default)]
    pub schedule: Vec<Form2552ShareRow>,
    /// Part V Item 6 of that schedule.
    #[serde(default)]
    pub schedule_total: f64,
    /// Part VI rows in order.
    #[serde(default)]
    pub non_taxable_transactions: Vec<Form2552NonTaxableRow>,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `round(this)` / `formatCurrency`: the value an amount field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// `formatCurrency((x).toFixed(0))`: `toFixed(0)` rounds the exact double
/// half away from zero, as `f64::round` does.
fn pesos(value: f64) -> f64 {
    cents(value.round())
}

fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// `capitalize()`: trimmed and upper-cased.
fn caps(value: &str) -> String {
    value.trim().to_uppercase()
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

fn is_leap(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn today() -> (u32, u32, u32) {
    use chrono::Datelike;
    let now = chrono::Local::now().date_naive();
    (now.year() as u32, now.month(), now.day())
}

/// Problems `validateDate` alerts for a Part V/VI `MM/DD/YYYY` date.
fn schedule_date_error(value: &str, today: (u32, u32, u32)) -> Option<&'static str> {
    let parts: Vec<&str> = value.split('/').collect();
    let valid = parts.len() == 3
        && parts[0].len() == 2
        && parts[1].len() == 2
        && parts[2].len() == 4
        && parts.iter().all(|p| digits_only(p));
    let (month, day, year) = if valid {
        (
            parts[0].parse::<u32>().unwrap_or(0),
            parts[1].parse::<u32>().unwrap_or(0),
            parts[2].parse::<u32>().unwrap_or(0),
        )
    } else {
        (0, 0, 0)
    };
    if !valid || !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Some("Please provide a valid date. (MM/DD/YYYY format)");
    }
    if (year, month, day) > today {
        return Some("This date cannot be a future date.");
    }
    if year < 2018 {
        return Some("This date cannot be prior to 2018.");
    }
    None
}

impl Form2552ShareRow {
    fn has_text(&self) -> bool {
        !(self.date.trim().is_empty()
            && self.seller.trim().is_empty()
            && self.buyer.trim().is_empty()
            && self.issuing_corporation.trim().is_empty())
    }

    fn text_complete(&self) -> bool {
        !(self.date.trim().is_empty()
            || self.seller.trim().is_empty()
            || self.buyer.trim().is_empty()
            || self.issuing_corporation.trim().is_empty())
    }
}

impl Form2552NonTaxableRow {
    fn has_text(&self) -> bool {
        !(self.date.trim().is_empty()
            && self.seller.trim().is_empty()
            && self.buyer.trim().is_empty()
            && self.issuing_corporation.trim().is_empty())
    }

    fn text_complete(&self) -> bool {
        !(self.date.trim().is_empty()
            || self.seller.trim().is_empty()
            || self.buyer.trim().is_empty()
            || self.issuing_corporation.trim().is_empty())
    }
}

impl Form2552Draft {
    pub const FORM_CODE: &'static str = "2552";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, month: u8, day: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            transaction_month: month,
            transaction_day: day,
            transaction_year: year,
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
            tax_relief: false,
            tax_relief_specification: String::new(),
            transaction: Form2552Transaction::Unspecified,
            shares_sold: String::new(),
            outstanding_shares: String::new(),
            tax_due_lse: 0.0,
            tax_due_primary: 0.0,
            tax_due_secondary: 0.0,
            total_tax_due: 0.0,
            tax_paid_previous: 0.0,
            creditable_tax_withheld: 0.0,
            total_tax_credits: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            overpayment: Form2552Overpayment::None,
            exempt_transactions: Vec::new(),
            schedule: Vec::new(),
            schedule_total: 0.0,
            non_taxable_transactions: Vec::new(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 11 radio: `kindOfTransaction(); enableSchedule(n); resetArr(...)`.
    /// The other schedules are cleared, so switching drops the rows entered.
    pub fn set_transaction(&mut self, transaction: Form2552Transaction) {
        if transaction != self.transaction {
            self.schedule.clear();
        }
        self.transaction = transaction;
        self.recompute();
    }

    /// The official handlers that reset disabled fields, then the compute chain.
    pub fn recompute(&mut self) {
        // isAmended / availTaxRelief / kindOfTransaction reset what they disable.
        if !self.is_amended {
            self.tax_paid_previous = 0.0;
        }
        if !self.tax_relief {
            self.tax_relief_specification.clear();
        }
        if !self.transaction.is_public_offering() {
            self.shares_sold.clear();
            self.outstanding_shares.clear();
        }
        if self.transaction == Form2552Transaction::Unspecified {
            self.schedule.clear();
        }

        for row in &mut self.exempt_transactions {
            row.amount = cents(row.amount);
        }
        for row in &mut self.non_taxable_transactions {
            row.number_of_shares = cents(row.number_of_shares);
        }

        // pageTwoComputation + totTaxDueComp.
        let lse = self.transaction == Form2552Transaction::LocalStockExchange;
        let folded = self.schedule_is_folded();
        let mut total = 0.0;
        for (index, row) in self.schedule.iter_mut().enumerate() {
            // Popup rows typed in the popup pass toComma(), which drops the
            // centavos before round() (`FormatValue`).
            let typed_in_popup = folded && index >= FORM_2552_ROWS;
            let whole = |value: f64| if typed_in_popup { value.trunc() } else { value };
            row.number_of_shares = cents(whole(row.number_of_shares));
            row.tax_base = cents(whole(row.tax_base));
            row.tax_rate = if lse {
                FORM_2552_LSE_RATE
            } else {
                cents(whole(row.tax_rate))
            };
            row.tax_due = if folded && index >= FORM_2552_ROWS - 1 {
                // Sum_Pg2Pt5AddMore reads the base with removeCommaParenthesis:
                // parseInt once the text has a thousands comma.
                let base = if row.tax_base >= 1000.0 {
                    row.tax_base.trunc()
                } else {
                    row.tax_base
                };
                pesos(base * row.tax_rate / 100.0)
            } else {
                pesos(row.tax_base * row.tax_rate / 100.0)
            };
            total = pesos(total + row.tax_due);
        }
        self.schedule_total = total;

        // pageTwoSchedTotalToPageOne.
        self.tax_due_lse = 0.0;
        self.tax_due_primary = 0.0;
        self.tax_due_secondary = 0.0;
        match self.transaction {
            Form2552Transaction::LocalStockExchange => self.tax_due_lse = total,
            Form2552Transaction::PrimaryOffering => self.tax_due_primary = total,
            Form2552Transaction::SecondaryOffering => self.tax_due_secondary = total,
            Form2552Transaction::Unspecified => {}
        }

        // pageOneComputation.
        self.tax_paid_previous = cents(self.tax_paid_previous);
        self.creditable_tax_withheld = cents(self.creditable_tax_withheld);
        self.total_tax_due =
            pesos(self.tax_due_lse + self.tax_due_primary + self.tax_due_secondary);
        self.total_tax_credits = pesos(self.tax_paid_previous + self.creditable_tax_withheld);
        self.tax_still_due = pesos(self.total_tax_due - self.total_tax_credits);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = pesos(self.surcharge + self.interest + self.compromise);
        // Item 25: with an overpayment, only the penalties are payable.
        self.total_amount_payable = if self.total_penalties > 0.0 && self.tax_still_due < 0.0 {
            pesos(self.total_penalties)
        } else {
            pesos(self.tax_still_due + self.total_penalties)
        };
        // handleOverpayment('disable') clears both boxes.
        if self.tax_still_due >= 0.0 {
            self.overpayment = Form2552Overpayment::None;
        }
    }

    /// More than five Part V rows: the fifth and later ones are popup rows.
    pub fn schedule_is_folded(&self) -> bool {
        self.schedule.len() > FORM_2552_ROWS
    }

    /// The popup subtotal row 5 shows (`Pg2Pt5Sch<n>SubTotal`).
    pub fn popup_subtotal(&self) -> f64 {
        if !self.schedule_is_folded() {
            return 0.0;
        }
        pesos(
            self.schedule[FORM_2552_ROWS - 1..]
                .iter()
                .map(|row| row.tax_due)
                .sum::<f64>(),
        )
    }

    /// The popup's controls in page order: every row's columns 1–3
    /// (`PopTable1st`), then every row's columns 4–8 (`PopTable2nd`).
    fn popup_controls(&self) -> Vec<(String, String)> {
        let Some(schedule) = self.transaction.schedule() else {
            return Vec::new();
        };
        if !self.schedule_is_folded() {
            return Vec::new();
        }
        let rows = &self.schedule[FORM_2552_ROWS - 1..];
        let key = |n: usize, col: u8| format!("frm2552:txtPg2Pt5Sch{schedule}_{n}Col{col}");
        let mut controls = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let n = index + 1;
            controls.push((key(n, 1), row.date.trim().to_string()));
            controls.push((key(n, 2), caps(&row.seller)));
            controls.push((key(n, 3), caps(&row.buyer)));
        }
        for (index, row) in rows.iter().enumerate() {
            let n = index + 1;
            controls.push((key(n, 4), caps(&row.issuing_corporation)));
            controls.push((key(n, 5), official_amount(row.number_of_shares)));
            controls.push((key(n, 6), official_amount(row.tax_base)));
            controls.push((
                key(n, 7),
                if schedule == 1 {
                    FORM_2552_LSE_RATE_TEXT.to_string()
                } else {
                    official_amount(row.tax_rate)
                },
            ));
            controls.push((key(n, 8), official_amount(row.tax_due)));
        }
        // saveEncryptedProfile writes every text control upper-cased.
        for (_, value) in &mut controls {
            *value = value.to_uppercase();
        }
        controls
    }

    /// Item 1 as the filename and Item 1 controls write it.
    fn date_parts(&self) -> (String, String, String) {
        (
            format!("{:02}", self.transaction_month),
            format!("{:02}", self.transaction_day),
            format!("{:04}", self.transaction_year),
        )
    }

    /// Registered address split the way `loadBGData` fills Item 7.
    fn address_lines(&self) -> (String, String) {
        let address: Vec<char> = caps(&self.registered_address).chars().collect();
        let first: String = address.iter().take(ADDRESS_LINE).collect();
        let second: String = address
            .iter()
            .skip(ADDRESS_LINE)
            .take(ADDRESS_LINE)
            .collect();
        (first, second)
    }

    /// The official field values the upload (`saveEncryptedProfile`) reads, keyed by element id,
    /// popup rows included.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.layout_fields();
        fields.extend(self.popup_controls());
        fields
    }

    /// The controls of the fixed official layout.
    fn layout_fields(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2552:{key}"), value);
        };
        let flag = |on: bool| on.to_string();

        let (month, day, year) = self.date_parts();
        put("txtPg1I1Month", month);
        put("txtPg1I1Day", day);
        put("txtPg1I1Year", year);
        put("rdoPg1I2AmendedYes", flag(self.is_amended));
        put("rdoPg1I2AmendedNo", flag(!self.is_amended));
        put(
            "txtPg1I3NoOfSheets",
            self.number_of_attached_sheets.to_string(),
        );

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtPg1TIN1", tin1);
        put("txtPg1TIN2", tin2);
        put("txtPg1TIN3", tin3);
        put("txtPg1TIN4", branch);
        let rdo = self.rdo_code.trim().to_string();
        put("txtPg1I5RDO", rdo.clone());
        // loadBGData selects the profile RDO in the injected select.
        put("rdoPg1Pt1I5RDO", rdo);
        put("txtPg1Pt1I6Name", caps(&self.taxpayer_name));
        let (address1, address2) = self.address_lines();
        put("txtPg1Pt1I7RegisteredAddress", address1);
        put("txtPg1Pt1I7RegisteredAddress2", address2);
        put("txPg1I7ZipCode", self.zip_code.trim().to_string());
        put(
            "txtPg1Pt1I8ContactNumber",
            self.contact_number.trim().to_string(),
        );
        // The email keeps its casing.
        put("txtPg1Pt1I9Email", self.email.trim().to_string());
        put("rdoPg1I10TaxReliefYes", flag(self.tax_relief));
        put("rdoPg1I10TaxReliefNo", flag(!self.tax_relief));
        put(
            "txtPg1Pt1I10TaxReliefSpec",
            caps(&self.tax_relief_specification),
        );
        put(
            "rdoPg1I11TransactionLSE",
            flag(self.transaction == Form2552Transaction::LocalStockExchange),
        );
        put(
            "rdoPg1I11TransactionPrimary",
            flag(self.transaction == Form2552Transaction::PrimaryOffering),
        );
        put(
            "rdoPg1I11TransactionSecondary",
            flag(self.transaction == Form2552Transaction::SecondaryOffering),
        );
        put("txtPg1Pt1I12NumOfShare", caps(&self.shares_sold));
        put("txtPg1Pt1I12TotShare", caps(&self.outstanding_shares));

        put("txtPg1P2I13TaxATC", "PT 200".to_string());
        put("txtPg1P2I13TaxDue", official_amount(self.tax_due_lse));
        put("txtPg1P2I14TaxATC", "PT 201".to_string());
        put("txtPg1P2I14TaxDue", official_amount(self.tax_due_primary));
        put("txtPg1P2I15TaxATC", "PT 202".to_string());
        put("txtPg1P2I15TaxDue", official_amount(self.tax_due_secondary));
        put("txtPg1P2I16TotTaxDue", official_amount(self.total_tax_due));
        put(
            "txtPg1P2I17TaxPaidInPrevRtrn",
            official_amount(self.tax_paid_previous),
        );
        put(
            "txtPg1P2I18CredTaxWthhld",
            official_amount(self.creditable_tax_withheld),
        );
        put(
            "txtPg1P2I19TotTaxCredPmnt",
            official_amount(self.total_tax_credits),
        );
        put(
            "txtPg1P2I20TotTaxStllDueOvrPmnt",
            official_amount(self.tax_still_due),
        );
        put("txtPg1P2I21Surcharge", official_amount(self.surcharge));
        put("txtPg1P2I22Interest", official_amount(self.interest));
        put("txtPg1P2I23Compromise", official_amount(self.compromise));
        put(
            "txtPg1P2I24TotPenalties",
            official_amount(self.total_penalties),
        );
        put(
            "txtPg1P2I25TotAmntPyable",
            official_amount(self.total_amount_payable),
        );
        put(
            "rdoPg1OverpaymentRefund",
            flag(self.overpayment == Form2552Overpayment::Refund),
        );
        put(
            "rdoPg1OverpaymentTaxCredCert",
            flag(self.overpayment == Form2552Overpayment::TaxCreditCertificate),
        );

        for index in 0..FORM_2552_ROWS {
            let n = index + 1;
            let row = self
                .exempt_transactions
                .get(index)
                .cloned()
                .unwrap_or_default();
            put(
                &format!("txtPg2P4I{n}TransClass"),
                caps(&row.classification),
            );
            put(&format!("txtPg2P4I{n}Amount"), official_amount(row.amount));
        }

        let active = self.transaction.schedule();
        for schedule in 1..=3u8 {
            for index in 0..FORM_2552_ROWS {
                let n = index + 1;
                let row = if active == Some(schedule) {
                    self.schedule.get(index).cloned().unwrap_or_default()
                } else {
                    Form2552ShareRow::default()
                };
                let key = |column: &str| format!("txtPg2Pt5Sch{schedule}{column}{n}");
                if active == Some(schedule)
                    && index == FORM_2552_ROWS - 1
                    && self.schedule_is_folded()
                {
                    // Save_Pg2Pt5AddMorePopTable with two or more popup rows.
                    for column in ["Date", "Seller", "Buyer", "Corp", "NumShares", "TaxBase"] {
                        put(&key(column), OTHERS.to_string());
                    }
                    put(
                        &key("TaxRate"),
                        if schedule == 1 {
                            FORM_2552_LSE_RATE_TEXT
                        } else {
                            OTHERS
                        }
                        .to_string(),
                    );
                    put(&key("TaxDue"), official_amount(self.popup_subtotal()));
                    continue;
                }
                put(&key("Date"), row.date.trim().to_string());
                put(&key("Seller"), caps(&row.seller));
                put(&key("Buyer"), caps(&row.buyer));
                put(&key("Corp"), caps(&row.issuing_corporation));
                put(&key("NumShares"), official_amount(row.number_of_shares));
                put(&key("TaxBase"), official_amount(row.tax_base));
                put(
                    &key("TaxRate"),
                    if schedule == 1 {
                        FORM_2552_LSE_RATE_TEXT.to_string()
                    } else {
                        official_amount(row.tax_rate)
                    },
                );
                put(&key("TaxDue"), official_amount(row.tax_due));
            }
            let total = if active == Some(schedule) {
                self.schedule_total
            } else {
                0.0
            };
            put(
                &format!("txtPg2Pt5Sch{schedule}TaxDueTot"),
                official_amount(total),
            );
        }

        for index in 0..FORM_2552_ROWS {
            let n = index + 1;
            let row = self
                .non_taxable_transactions
                .get(index)
                .cloned()
                .unwrap_or_default();
            put(&format!("txtPg2Pt6Date{n}"), row.date.trim().to_string());
            put(&format!("txtPg2Pt6Seller{n}"), caps(&row.seller));
            put(&format!("txtPg2Pt6Buyer{n}"), caps(&row.buyer));
            put(&format!("txtPg2Pt6Corp{n}"), caps(&row.issuing_corporation));
            put(
                &format!("txtPg2Pt6NumShares{n}"),
                official_amount(row.number_of_shares),
            );
        }

        put("txtLOB", self.line_of_business.trim().to_string());
        // init() sets the pager to page 1.
        put("txtCurrentPage", "1".to_string());
        if let Some(schedule) = self.transaction.schedule()
            && self.schedule_is_folded()
        {
            fields.insert(
                format!("Pg2Pt5Sch{schedule}SubTotal"),
                official_amount(self.popup_subtotal()),
            );
            fields.insert(
                format!("Pg2Pt5Sch{schedule}PopLength"),
                (self.schedule.len() - (FORM_2552_ROWS - 1)).to_string(),
            );
        }
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `checkDatePage1`, `dateInPage1Item1` and `checkYear`.
    fn validate_item_1(&self, today: (u32, u32, u32), err: &mut impl FnMut(&str, &str)) {
        let month = u32::from(self.transaction_month);
        let day = u32::from(self.transaction_day);
        let year = u32::from(self.transaction_year);
        if month == 0 {
            err(
                "transaction_month",
                "Month field on Page 1 Item 1 is required.",
            );
            return;
        }
        if day == 0 {
            err("transaction_day", "Day field on Page 1 Item 1 is required.");
            return;
        }
        if year == 0 {
            err(
                "transaction_year",
                "Year field on Page 1 Item 1 is required.",
            );
            return;
        }
        if month > 12 || !(1800..=9999).contains(&year) || day > days_in_month(year, month) {
            err(
                "transaction_day",
                "Please provide a valid date. (MM/DD/YYYY format) in Page 1 Item 1.",
            );
            return;
        }
        if (year, month, day) > today {
            err(
                "transaction_day",
                "Page 1 Item 1 Date cannot be a future date ",
            );
            return;
        }
        // checkYear (on blur) clears a year outside 2018 to the present.
        if year < 2018 || year > today.0 {
            err(
                "transaction_year",
                "Year shall not be greater than the present year and not earlier than 2018.",
            );
        }
    }

    fn validate_on(&self, today: (u32, u32, u32)) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        self.validate_item_1(today, &mut err);
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most two digits.",
            );
        }

        // Item 4 comes from the profile; the page has no check, so require
        // the official TIN shape and check digit.
        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        let tin_digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        if tin1.len() != 3
            || tin2.len() != 3
            || tin3.len() != 3
            || tin_digits.len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN on Page 1 Item 4.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }

        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err(
                "rdo_code",
                "Please enter a valid RDO Code on Page 1 Item 5.",
            );
        }
        if self.taxpayer_name.trim().is_empty() {
            err("taxpayer_name", "Name field on Page 1 Item 6 is required.");
        }
        let address = self.registered_address.trim();
        if address.is_empty() {
            err(
                "registered_address",
                "Registered Address field on Page 1 Item 7 is required.",
            );
        } else if address.chars().count() > 2 * ADDRESS_LINE {
            err(
                "registered_address",
                "Item 7 holds at most 160 characters (two lines of 80).",
            );
        }
        if self.zip_code.trim().is_empty() {
            err("zip_code", "Zip Code field on Page 1 Item 7A is required.");
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() {
            err(
                "contact_number",
                "Contact Number field on Page 1 Item 8 is required.",
            );
        } else if phone.len() > 50 || !digits_only(phone) {
            err(
                "contact_number",
                "Item 8 Contact Number takes up to 50 digits only.",
            );
        }
        let email = self.email.trim();
        if email.is_empty() {
            err("email", "E-mail address on page 1 item 9 is required.");
        } else if email.chars().count() > 50 || !email_is_valid(email) {
            err(
                "email",
                "Please enter a valid e-mail address on page 1 item 10",
            );
        }

        if self.tax_relief && self.tax_relief_specification.trim().is_empty() {
            err(
                "tax_relief_specification",
                "Specify Tax Relief field on Page 1 Item 10A is required.",
            );
        }
        if self.tax_relief_specification.trim().chars().count() > LONG_TEXT_MAX {
            err(
                "tax_relief_specification",
                "Item 10A holds at most 100 characters.",
            );
        }

        if self.transaction == Form2552Transaction::Unspecified {
            err(
                "transaction",
                "Select an option for Page 1 Item 11 Kind of Transaction.",
            );
        } else if self.transaction.is_public_offering()
            && (self.shares_sold.trim().is_empty() || self.outstanding_shares.trim().is_empty())
        {
            err(
                "shares_sold",
                "Page 1 Item 12A and 12B cannot not be empty.",
            );
        }
        for (field, value) in [
            ("shares_sold", &self.shares_sold),
            ("outstanding_shares", &self.outstanding_shares),
        ] {
            if value.trim().chars().count() > LONG_TEXT_MAX {
                err(field, "Items 12A and 12B hold at most 100 characters.");
            }
        }

        // The page compares the formatted Item 20 after NumWithComma.
        if self.tax_still_due < 0.0 && self.overpayment == Form2552Overpayment::None {
            err(
                "overpayment",
                "Select an Overpayment option in Page 1 after Item 25.",
            );
        }

        // checkPartIVFields.
        if self.exempt_transactions.len() > FORM_2552_ROWS {
            err("exempt_transactions", "Part IV has 5 rows.");
        }
        for (index, row) in self
            .exempt_transactions
            .iter()
            .enumerate()
            .take(FORM_2552_ROWS)
        {
            let n = index + 1;
            let class = row.classification.trim();
            if (row.amount == 0.0) != class.is_empty() {
                err(
                    &format!("exempt_transactions[{index}]"),
                    &format!("Please complete Item #{n} in Part IV Page 2."),
                );
                break;
            }
        }

        // checkPartVSchFields for the schedule in use (the others are blank).
        if let Some(schedule) = self.transaction.schedule() {
            if self.schedule.len() > FORM_2552_MAX_SCHEDULE_ROWS {
                err(
                    "schedule",
                    "Part V holds at most 100 transactions per return in this editor.",
                );
            }
            let folded = self.schedule_is_folded();
            // Row 5 shows OTHERS once folded (NaN amounts pass the check).
            let main_rows = if folded {
                FORM_2552_ROWS - 1
            } else {
                FORM_2552_ROWS
            };
            for (index, row) in self.schedule.iter().enumerate().take(main_rows) {
                let n = index + 1;
                let numbers_zero = if schedule == 1 {
                    row.number_of_shares == 0.0 || row.tax_base == 0.0
                } else {
                    row.number_of_shares == 0.0 || row.tax_base == 0.0 || row.tax_rate == 0.0
                };
                let any_number = if schedule == 1 {
                    row.number_of_shares != 0.0 || row.tax_base != 0.0
                } else {
                    row.number_of_shares != 0.0 || row.tax_base != 0.0 || row.tax_rate != 0.0
                };
                if (!row.text_complete() && any_number) || (row.has_text() && numbers_zero) {
                    err(
                        &format!("schedule[{index}]"),
                        &format!("Please complete Item #{n} in Page 2 Part V Schedule {schedule}."),
                    );
                    break;
                }
            }
        }

        // CheckEmptyDesc when the popup is saved.
        if self.schedule_is_folded() {
            let lse = self.transaction == Form2552Transaction::LocalStockExchange;
            for (index, row) in self.schedule.iter().enumerate().skip(FORM_2552_ROWS - 1) {
                let popup_row = index - (FORM_2552_ROWS - 2);
                if !row.text_complete()
                    || row.number_of_shares <= 0.0
                    || row.tax_base <= 0.0
                    || (!lse && row.tax_rate <= 0.0)
                {
                    err(
                        &format!("schedule[{index}]"),
                        &format!("Cannot save. You have an empty data in 5.{popup_row}"),
                    );
                    break;
                }
            }
        }

        // checkPartVIFields.
        if self.non_taxable_transactions.len() > FORM_2552_ROWS {
            err("non_taxable_transactions", "Part VI has 5 rows.");
        }
        for (index, row) in self
            .non_taxable_transactions
            .iter()
            .enumerate()
            .take(FORM_2552_ROWS)
        {
            let n = index + 1;
            if (!row.text_complete() && row.number_of_shares != 0.0)
                || (row.has_text() && row.number_of_shares == 0.0)
            {
                err(
                    &format!("non_taxable_transactions[{index}]"),
                    &format!("Please complete Item #{n} in Page 2 Part VI."),
                );
                break;
            }
        }

        // Limits the page enforces while typing.
        for (index, row) in self.exempt_transactions.iter().enumerate() {
            if row.classification.trim().chars().count() > PART_IV_CLASS_MAX {
                err(
                    &format!("exempt_transactions[{index}].classification"),
                    "Part IV Transaction Classification holds at most 80 characters.",
                );
            }
            amount_limits(
                &mut err,
                &format!("exempt_transactions[{index}].amount"),
                row.amount,
            );
        }
        for (index, row) in self.schedule.iter().enumerate() {
            let field = |column: &str| format!("schedule[{index}].{column}");
            if !row.date.trim().is_empty()
                && let Some(message) = schedule_date_error(row.date.trim(), today)
            {
                err(&field("date"), message);
            }
            for (column, value) in [
                ("seller", &row.seller),
                ("buyer", &row.buyer),
                ("issuing_corporation", &row.issuing_corporation),
            ] {
                if value.trim().chars().count() > TEXT_MAX {
                    err(&field(column), "Part V names hold at most 50 characters.");
                }
            }
            amount_limits(&mut err, &field("number_of_shares"), row.number_of_shares);
            amount_limits(&mut err, &field("tax_base"), row.tax_base);
            if self.transaction != Form2552Transaction::LocalStockExchange {
                if row.tax_rate >= 100.0 {
                    err(&field("tax_rate"), "Tax rate should be below 100.");
                } else {
                    amount_limits(&mut err, &field("tax_rate"), row.tax_rate);
                }
            }
        }
        for (index, row) in self.non_taxable_transactions.iter().enumerate() {
            let field = |column: &str| format!("non_taxable_transactions[{index}].{column}");
            if !row.date.trim().is_empty()
                && let Some(message) = schedule_date_error(row.date.trim(), today)
            {
                err(&field("date"), message);
            }
            for (column, value) in [
                ("seller", &row.seller),
                ("buyer", &row.buyer),
                ("issuing_corporation", &row.issuing_corporation),
            ] {
                if value.trim().chars().count() > TEXT_MAX {
                    err(&field(column), "Part VI names hold at most 50 characters.");
                }
            }
            amount_limits(&mut err, &field("number_of_shares"), row.number_of_shares);
        }
        for (field, value) in [
            ("tax_paid_previous", self.tax_paid_previous),
            ("creditable_tax_withheld", self.creditable_tax_withheld),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ] {
            amount_limits(&mut err, field, value);
        }
        if !self.is_amended && self.tax_paid_previous != 0.0 {
            err(
                "tax_paid_previous",
                "Item 17 applies only to an amended return.",
            );
        }

        // Derived items must be what the official compute chain produces.
        let mut expected = self.clone();
        expected.recompute();
        if expected.total_amount_payable != self.total_amount_payable
            || expected.total_tax_due != self.total_tax_due
            || expected.schedule != self.schedule
            || expected.schedule_total != self.schedule_total
        {
            err(
                "total_amount_payable",
                "Totals are out of date. Recompute the return.",
            );
        }

        errors
    }
}

/// `round(this)` keeps at most 12 integer digits (`isAmountWithinAllowedPrecision`)
/// and `wholenumber` admits no minus sign.
fn amount_limits(err: &mut impl FnMut(&str, &str), field: &str, value: f64) {
    if !value.is_finite() || !(0.0..1e12).contains(&value) || !has_cent_precision(value) {
        err(
            field,
            "Enter a non-negative amount below 1,000,000,000,000 in pesos and centavos.",
        );
    }
}

/// `validateEmail`'s pattern: `\b[a-zA-Z0-9._%+-]+@(?:[a-zA-Z0-9-]+\.)+[a-zA-Z]{2,4}\b`,
/// applied to the whole value.
fn email_is_valid(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'));
    let labels: Vec<&str> = domain.split('.').collect();
    let domain_ok = labels.len() >= 2
        && labels[..labels.len() - 1].iter().all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        && {
            let tld = labels[labels.len() - 1];
            tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
        };
    local_ok && domain_ok
}

impl FormValidator for Form2552Draft {
    /// `validateAll` in order, with its alert texts, plus the limits the
    /// page enforces while typing (`maxlength`, `wholenumber`, `validateDate`,
    /// `checkRate`, `checkYear`).
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(today())
    }
}

impl Form2552Draft {
    /// Items 21-23 on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "21 Surcharge",
                interest: "22 Interest",
                compromise: "23 Compromise",
            ],
            controls: [
                "frm2552:txtPg1P2I21Surcharge",
                "frm2552:txtPg1P2I22Interest",
                "frm2552:txtPg1P2I23Compromise",
            ],
            state: penalty,
            basic_tax_due: "Item 20", |d| d.tax_still_due,
        )
    }
}

impl QueueableForm for Form2552Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_bir_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2552";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['2552v2018']`).
    const FORM_TYPE: &'static str = "2552v2018";
    const LAYOUT_ID: &'static str = FORM_2552_FORM_ID;

    /// Total Amount Payable/(Overpayment).
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
        self.transaction_year
    }
    /// One return per transaction date: keyed `MMDD` within the year.
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::OpenEnded(
            u32::from(self.transaction_month) * 100 + u32::from(self.transaction_day),
        )
    }
    /// `txtPg1I1Month + txtPg1I1Day + txtPg1I1Year`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        let (month, day, year) = self.date_parts();
        format!("{month}{day}{year}")
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 8 || !digits_only(code) {
            return None;
        }
        let month: u32 = code.get(..2)?.parse().ok()?;
        let day: u32 = code.get(2..4)?.parse().ok()?;
        let year: u16 = code.get(4..)?.parse().ok()?;
        ((1..=12).contains(&month) && (1..=31).contains(&day))
            .then_some((year, FilingPeriod::OpenEnded(month * 100 + day)))
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

    /// The fixed layout, with the Add More popup's controls where the page's
    /// upload writes them: ahead of `Pg2Pt5Sch<n>SubTotal`.
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as QueueableForm>::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let xml_error = |error: String| vec![("xml".to_string(), error)];
        let layout = crate::official_xml::layout(Self::LAYOUT_ID)
            .map_err(|error| xml_error(error.to_string()))?;
        let mut payload = crate::official_xml::write(layout, &self.layout_fields())
            .map_err(|error| xml_error(error.to_string()))?;
        let controls = self.popup_controls();
        if let Some(schedule) = self.transaction.schedule()
            && !controls.is_empty()
        {
            let anchor = format!("<div>Pg2Pt5Sch{schedule}SubTotal=");
            let at = payload
                .find(&anchor)
                .ok_or_else(|| xml_error("the popup subtotal is missing from the layout".into()))?;
            // Every control on this page is followed by the same separator.
            let separator = &layout.lead;
            let rows: String = controls
                .iter()
                .map(|(id, value)| format!("<div>{id}={value}{id}=</div>{separator}"))
                .collect();
            payload.insert_str(at, &rows);
        }
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: (u32, u32, u32) = (2026, 10, 10);

    pub(crate) fn sample() -> Form2552Draft {
        let mut draft = Form2552Draft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            transaction_month: 3,
            transaction_day: 14,
            transaction_year: 2025,
            is_amended: true,
            number_of_attached_sheets: 1,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Dummy Taxpayer".to_string(),
            registered_address: "123 Sample Street, Barangay Example, Quezon City".to_string(),
            zip_code: "1100".to_string(),
            contact_number: "09170000000".to_string(),
            email: "Sample.Taxpayer@example.com".to_string(),
            line_of_business: "Sample Consulting Services".to_string(),
            tax_relief: true,
            tax_relief_specification: "Special law sample".to_string(),
            transaction: Form2552Transaction::Unspecified,
            shares_sold: String::new(),
            outstanding_shares: String::new(),
            tax_due_lse: 0.0,
            tax_due_primary: 0.0,
            tax_due_secondary: 0.0,
            total_tax_due: 0.0,
            tax_paid_previous: 100.5,
            creditable_tax_withheld: 1_000.25,
            total_tax_credits: 0.0,
            tax_still_due: 0.0,
            surcharge: 25.5,
            interest: 10.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            overpayment: Form2552Overpayment::None,
            exempt_transactions: vec![
                Form2552ExemptRow {
                    classification: "Exempt sample one".into(),
                    amount: 1_000.005,
                },
                Form2552ExemptRow {
                    classification: "exempt two".into(),
                    amount: 250.0,
                },
            ],
            schedule: Vec::new(),
            schedule_total: 0.0,
            non_taxable_transactions: vec![Form2552NonTaxableRow {
                date: "03/13/2025".into(),
                seller: "Seller Six".into(),
                buyer: "Buyer Six".into(),
                issuing_corporation: "Other Corp".into(),
                number_of_shares: 42.0,
            }],
            lifecycle: SubmissionLifecycle::default(),
        };
        // This sample enters its penalties by hand.
        draft.keep_entered_penalties();
        draft.set_transaction(Form2552Transaction::LocalStockExchange);
        draft.schedule = vec![
            Form2552ShareRow {
                date: "03/14/2025".into(),
                seller: "Seller One".into(),
                buyer: "Buyer One".into(),
                issuing_corporation: "Issuer Corp".into(),
                number_of_shares: 1_000.0,
                tax_base: 1_234_567.891,
                ..Default::default()
            },
            Form2552ShareRow {
                date: "03/14/2025".into(),
                seller: "Seller Two".into(),
                buyer: "Buyer Two".into(),
                issuing_corporation: "Issuer Corp".into(),
                number_of_shares: 500.5,
                tax_base: 999_999.995,
                ..Default::default()
            },
        ];
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2552Draft) -> Vec<String> {
        draft
            .validate_on(TODAY)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.schedule[0].tax_base, 1_234_567.89);
        assert_eq!(draft.schedule[0].tax_due, 7_407.0);
        assert_eq!(draft.schedule[1].tax_base, 1_000_000.0);
        assert_eq!(draft.schedule[1].tax_due, 6_000.0);
        assert_eq!(draft.schedule_total, 13_407.0);
        assert_eq!(draft.tax_due_lse, 13_407.0);
        assert_eq!(draft.total_tax_due, 13_407.0);
        assert_eq!(draft.total_tax_credits, 1_101.0);
        assert_eq!(draft.tax_still_due, 12_306.0);
        assert_eq!(draft.total_penalties, 36.0);
        assert_eq!(draft.total_amount_payable, 12_342.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn overpayment_pays_only_penalties() {
        let mut draft = sample();
        draft.creditable_tax_withheld = 20_000.0;
        draft.recompute();
        assert_eq!(draft.tax_still_due, -6_694.0);
        assert_eq!(draft.total_amount_payable, 36.0);
        draft.surcharge = 0.0;
        draft.interest = 0.0;
        draft.keep_entered_penalties();
        draft.recompute();
        assert_eq!(draft.total_amount_payable, -6_694.0);
    }

    #[test]
    fn public_offering_uses_the_entered_rate() {
        let mut draft = sample();
        draft.set_transaction(Form2552Transaction::PrimaryOffering);
        assert!(draft.schedule.is_empty());
        draft.shares_sold = "1000000".into();
        draft.outstanding_shares = "5000000".into();
        draft.schedule.push(Form2552ShareRow {
            date: "03/14/2025".into(),
            seller: "Issuer".into(),
            buyer: "Public".into(),
            issuing_corporation: "Issuer Corp".into(),
            number_of_shares: 1_000.0,
            tax_base: 12_345.67,
            tax_rate: 4.0,
            ..Default::default()
        });
        draft.recompute();
        assert_eq!(draft.schedule[0].tax_due, 494.0);
        assert_eq!(draft.tax_due_primary, 494.0);
        assert_eq!(draft.tax_due_lse, 0.0);
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm2552:txtPg2Pt5Sch2TaxRate1"], "4.00");
        assert_eq!(fields["frm2552:txtPg2Pt5Sch1TaxRate1"], "6/10 of 1%");
        assert_eq!(fields["frm2552:txtPg2Pt5Sch1Seller1"], "");
        assert_eq!(fields["frm2552:txtPg1Pt1I12NumOfShare"], "1000000");
        // 494 less 1,101 of credits is an overpayment.
        assert_eq!(draft.tax_still_due, -607.0);
        draft.overpayment = Form2552Overpayment::TaxCreditCertificate;
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm2552:txtPg1I1Month"], "03");
        assert_eq!(fields["frm2552:txtPg1I1Day"], "14");
        assert_eq!(fields["frm2552:txtPg1Pt1I6Name"], "SAMPLE DUMMY TAXPAYER");
        assert_eq!(
            fields["frm2552:txtPg1Pt1I9Email"],
            "Sample.Taxpayer@example.com"
        );
        assert_eq!(
            fields["frm2552:txtPg1Pt1I10TaxReliefSpec"],
            "SPECIAL LAW SAMPLE"
        );
        assert_eq!(fields["frm2552:rdoPg1Pt1I5RDO"], "039");
        assert_eq!(fields["frm2552:txtPg2P4I1Amount"], "1,000.01");
        assert_eq!(fields["frm2552:txtPg2Pt5Sch1TaxBase1"], "1,234,567.89");
        assert_eq!(fields["frm2552:txtPg2Pt5Sch1TaxDue1"], "7,407.00");
        assert_eq!(fields["frm2552:txtPg2Pt5Sch2TaxRate1"], "0.00");
        assert_eq!(fields["frm2552:txtPg1P2I25TotAmntPyable"], "12,342.00");
        assert_eq!(fields["frm2552:txtLOB"], "Sample Consulting Services");
        assert_eq!(fields["frm2552:txtCurrentPage"], "1");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2552v2018-03142025#Sample.Taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn rows_past_five_fold_into_the_add_more_popup() {
        // Same entries as the official page run (runtime.js, Add More saved).
        let mut d = sample();
        d.schedule.clear();
        for i in 1..=5 {
            d.schedule.push(Form2552ShareRow {
                date: "03/14/2025".into(),
                seller: format!("Seller {i}"),
                buyer: format!("Buyer {i}"),
                issuing_corporation: "Listed Corp".into(),
                number_of_shares: 100.0 * i as f64,
                tax_base: 10_000.5 * i as f64,
                ..Default::default()
            });
        }
        for (shares, base) in [(600.0, 12_345.67), (70.0, 999.99)] {
            d.schedule.push(Form2552ShareRow {
                date: "03/13/2025".into(),
                seller: "Seller".into(),
                buyer: "Buyer".into(),
                issuing_corporation: "Other Corp".into(),
                number_of_shares: shares,
                tax_base: base,
                ..Default::default()
            });
        }
        d.recompute();
        assert_eq!(d.schedule[5].tax_base, 12_345.0);
        assert_eq!(d.schedule[6].tax_base, 999.0);
        assert_eq!(d.popup_subtotal(), 380.0);
        assert_eq!(d.schedule_total, 980.0);
        let fields = d.to_bir_field_map();
        assert_eq!(fields["frm2552:txtPg2Pt5Sch1TaxBase5"], "OTHERS");
        assert_eq!(fields["frm2552:txtPg2Pt5Sch1TaxDue5"], "380.00");
        assert_eq!(fields["frm2552:txtPg2Pt5Sch1_1Col6"], "50,002.50");
        assert_eq!(fields["frm2552:txtPg2Pt5Sch1_2Col8"], "74.00");
        assert_eq!(fields["Pg2Pt5Sch1PopLength"], "3");
        d.overpayment = Form2552Overpayment::Refund;
        let payload = d.to_bir_xml_payload().unwrap();
        assert!(payload.contains(
            "<div>frm2552:txtPg2Pt5Sch1_2Col7=6/10 OF 1%frm2552:txtPg2Pt5Sch1_2Col7=</div>"
        ));
        let popup = payload.find("frm2552:txtPg2Pt5Sch1_1Col1=").unwrap();
        assert!(popup < payload.find("<div>Pg2Pt5Sch1SubTotal=").unwrap());
        d.schedule[6].buyer.clear();
        assert!(
            messages(&d)
                .iter()
                .any(|m| m == "Cannot save. You have an empty data in 5.3")
        );
    }

    #[test]
    fn long_addresses_split_into_two_lines() {
        let mut draft = sample();
        draft.registered_address = format!("{}{}", "a".repeat(80), "b".repeat(10));
        let fields = draft.to_bir_field_map();
        assert_eq!(
            fields["frm2552:txtPg1Pt1I7RegisteredAddress"],
            "A".repeat(80)
        );
        assert_eq!(
            fields["frm2552:txtPg1Pt1I7RegisteredAddress2"],
            "B".repeat(10)
        );
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "03142025");
        assert_eq!(draft.filing_period(), FilingPeriod::OpenEnded(314));
        assert_eq!(
            Form2552Draft::parse_period_code("03142025"),
            Some((2025, FilingPeriod::OpenEnded(314)))
        );
        assert_eq!(Form2552Draft::parse_period_code("13142025"), None);
        assert_eq!(Form2552Draft::parse_period_code("03002025"), None);
        assert_eq!(Form2552Draft::parse_period_code("032025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2552Draft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.transaction_month = 0,
            "Month field on Page 1 Item 1 is required.",
        );
        check(
            &|d| d.transaction_day = 0,
            "Day field on Page 1 Item 1 is required.",
        );
        check(
            &|d| d.transaction_year = 0,
            "Year field on Page 1 Item 1 is required.",
        );
        check(
            &|d| {
                d.transaction_month = 2;
                d.transaction_day = 29;
            },
            "Please provide a valid date. (MM/DD/YYYY format) in Page 1 Item 1.",
        );
        check(
            &|d| d.transaction_year = 2027,
            "Page 1 Item 1 Date cannot be a future date ",
        );
        check(
            &|d| d.transaction_year = 2017,
            "Year shall not be greater than the present year and not earlier than 2018.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Page 1 Item 5.",
        );
        check(
            &|d| d.taxpayer_name = " ".into(),
            "Name field on Page 1 Item 6 is required.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Registered Address field on Page 1 Item 7 is required.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Zip Code field on Page 1 Item 7A is required.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Contact Number field on Page 1 Item 8 is required.",
        );
        check(
            &|d| d.email.clear(),
            "E-mail address on page 1 item 9 is required.",
        );
        check(
            &|d| d.email = "not-an-email".into(),
            "Please enter a valid e-mail address on page 1 item 10",
        );
        check(
            &|d| d.tax_relief_specification = " ".into(),
            "Specify Tax Relief field on Page 1 Item 10A is required.",
        );
        check(
            &|d| d.set_transaction(Form2552Transaction::Unspecified),
            "Select an option for Page 1 Item 11 Kind of Transaction.",
        );
        check(
            &|d| d.set_transaction(Form2552Transaction::SecondaryOffering),
            "Page 1 Item 12A and 12B cannot not be empty.",
        );
        check(
            &|d| {
                d.creditable_tax_withheld = 20_000.0;
                d.recompute();
            },
            "Select an Overpayment option in Page 1 after Item 25.",
        );
        check(
            &|d| d.exempt_transactions[1].classification.clear(),
            "Please complete Item #2 in Part IV Page 2.",
        );
        check(
            &|d| {
                d.schedule[1].buyer.clear();
            },
            "Please complete Item #2 in Page 2 Part V Schedule 1.",
        );
        check(
            &|d| {
                d.schedule[0].tax_base = 0.0;
                d.recompute();
            },
            "Please complete Item #1 in Page 2 Part V Schedule 1.",
        );
        check(
            &|d| d.non_taxable_transactions[0].number_of_shares = 0.0,
            "Please complete Item #1 in Page 2 Part VI.",
        );
        check(
            &|d| d.schedule[0].date = "02/30/2025".into(),
            "Please provide a valid date. (MM/DD/YYYY format)",
        );
        check(
            &|d| d.schedule[0].date = "12/31/2026".into(),
            "This date cannot be a future date.",
        );
        check(
            &|d| d.non_taxable_transactions[0].date = "12/31/2017".into(),
            "This date cannot be prior to 2018.",
        );
        check(
            &|d| {
                d.set_transaction(Form2552Transaction::PrimaryOffering);
                d.schedule.push(Form2552ShareRow {
                    tax_rate: 100.0,
                    ..Default::default()
                });
            },
            "Tax rate should be below 100.",
        );
    }

    #[test]
    fn handlers_reset_disabled_fields() {
        let mut draft = sample();
        draft.is_amended = false;
        draft.tax_relief = false;
        draft.recompute();
        assert_eq!(draft.tax_paid_previous, 0.0);
        assert!(draft.tax_relief_specification.is_empty());
        assert_eq!(draft.total_tax_credits, 1_000.0);
        draft.shares_sold = "1".into();
        draft.recompute();
        assert!(draft.shares_sold.is_empty(), "LSE clears Item 12A");
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
            <Form2552Draft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2552Draft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
