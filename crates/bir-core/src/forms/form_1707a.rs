//! BIR Form 1707-A (April 2021) — Annual Capital Gains Tax Return for Onerous
//! Transfer of Shares of Stock Not Traded Through the Local Stock Exchange.
//!
//! Ported from the official `BIR-Form1707Av2021.hta` (eBIRForms 7.9.6.2.1):
//! the amount helpers (`roundElement` / `roundAmount`, which truncate to two
//! decimals and show losses in parentheses, `valForCompute`,
//! `preciseCompute`), the compute chain (`computeSched1*`, `computeSched2*`,
//! `computePart2Item14GainLoss` … `computePart2Item19TotalPayable`),
//! `validateAll()` / `validateSchedules()` with their exact alert texts, and
//! the uploaded file (`saveEncryptedProfile`), which carries the amounts as
//! shown (`1,234.50`, losses `(5.00)`).
//! Background information comes from the taxpayer profile the way
//! `loadBGData()` fills it (including its page-2 TIN3 = TIN2 slip).
//!
//! Schedules 1 and 2 show four rows; past that the "More" pop-up takes row 4
//! and the rest, row 4 then reads "OTHERS" with the pop-up totals, and the
//! pop-up rows are written where the page keeps them (see
//! [`Form1707ADraft::official_layout`]).

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::official_inputs::{
    RowInsertion, digits_only, extend_layout, split_tin, tin_is_well_formed,
};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1707A_FORM_ID: &str = "1707a-v2021";
/// Rows 1–4 of Schedules 1 and 2.
pub const FORM_1707A_SCHEDULE_ROWS: usize = 4;
/// Item 16 applicable tax rate (`txtI16ApplicableTaxRate`).
pub const FORM_1707A_TAX_RATE: f64 = 15.0;

/// Item 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1707AAtc {
    /// II030.
    Individual,
    /// IC110.
    Corporation,
}

/// A Schedule 1 (gain) or Schedule 2 (loss) row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1707ARow {
    /// Column A, `MM/DD/YYYY`.
    pub date: String,
    /// Column B (17 characters).
    pub corporation: String,
    /// Column C.
    pub selling_price: f64,
    /// Column D.
    pub cost: f64,
    /// Column E, computed: gain (Schedule 1) or loss (Schedule 2).
    #[serde(default)]
    pub gain_or_loss: f64,
    /// Column F, Schedule 1 only.
    #[serde(default)]
    pub tax_paid: f64,
}

impl Form1707ARow {
    fn date_blank(&self) -> bool {
        self.date.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1707ADraft {
    #[serde(default)]
    pub id: Option<i64>,
    /// Dashboard year and open-ended key that identify this return.
    pub filing_year: u16,
    pub open_ended_key: u32,

    // Items 1–4
    pub fiscal: bool,
    pub year_end_month: u8,
    pub year_end_day: u8,
    pub year_end_year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,
    pub atc: Option<Form1707AAtc>,

    // Background information (taxpayer profile)
    pub tin: String,
    pub rdo_code: String,
    pub taxpayer_name: String,
    #[serde(default)]
    pub line_of_business: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,
    /// Item 12; `Some` when "Yes", holding Item 12A.
    #[serde(default)]
    pub tax_relief: Option<String>,

    // Schedules
    #[serde(default)]
    pub gains: Vec<Form1707ARow>,
    #[serde(default)]
    pub losses: Vec<Form1707ARow>,
    /// Pop-up totals (selling, cost, gain/loss, tax paid) when a schedule has
    /// more than four rows (`txtS1ModalTotal*` / `txtS2ModalTotal*`).
    #[serde(default)]
    pub s1_modal_totals: [f64; 4],
    #[serde(default)]
    pub s2_modal_totals: [f64; 4],
    #[serde(default)]
    pub s1_total_selling_price: f64,
    #[serde(default)]
    pub s1_total_cost: f64,
    #[serde(default)]
    pub s1_total_gains: f64,
    #[serde(default)]
    pub s1_total_tax_paid: f64,
    #[serde(default)]
    pub s2_total_selling_price: f64,
    #[serde(default)]
    pub s2_total_cost: f64,
    #[serde(default)]
    pub s2_total_loss: f64,

    // Part II
    #[serde(default)]
    pub net_capital_gain: f64,
    /// Item 15; the page writes `0.00` for a negative tax due.
    #[serde(default)]
    pub tax_due: f64,
    /// 16B, amended returns only.
    #[serde(default)]
    pub tax_paid_previous: f64,
    /// 16C.
    #[serde(default)]
    pub total_tax_paid: f64,
    /// 17.
    #[serde(default)]
    pub tax_payable: f64,
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
    /// 19.
    #[serde(default)]
    pub total_amount_payable: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

// ── Official amount helpers ──

/// `Math.round`.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// `preciseCompute`: `Math.round(x * 100) / 100`.
fn precise(value: f64) -> f64 {
    js_round(value * 100.0) / 100.0
}

/// `String(number)` for the amounts these handlers produce.
fn js_number_text(value: f64) -> String {
    if value == 0.0 {
        "0".to_string()
    } else {
        format!("{value}")
    }
}

/// `addCommas(Number(text))`.
fn add_commas_of_number(text: &str) -> String {
    let number: f64 = if text.trim().is_empty() {
        0.0
    } else {
        text.parse().unwrap_or(f64::NAN)
    };
    let plain = js_number_text(number);
    let (sign, digits) = match plain.strip_prefix('-') {
        Some(rest) => ("-", rest.to_string()),
        None => ("", plain.clone()),
    };
    if digits.contains('.') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return plain;
    }
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}{grouped}")
}

/// `roundAmount(text)`: truncates to two decimals, groups thousands and
/// writes negatives as `(1,234.50)`.
pub fn round_amount_text(amount: &str) -> String {
    let int_len = amount.split('.').next().unwrap_or("").len();
    let mut out = if int_len > 12 {
        "0.00".to_string()
    } else if let Some((whole, frac)) = amount.split_once('.') {
        let frac = frac.split('.').next().unwrap_or("");
        match frac.len() {
            0 => format!("{}.00", add_commas_of_number(amount)),
            1 => format!("{}.{frac}0", add_commas_of_number(whole)),
            _ => format!("{}.{}", add_commas_of_number(whole), &frac[..2]),
        }
    } else {
        format!("{}.00", add_commas_of_number(amount))
    };
    if let Some(index) = out.find('-') {
        let after = out[index + 1..].split('-').next().unwrap_or("").to_string();
        out = format!("({after})");
    }
    out
}

/// `valForCompute`: reads a shown amount back, parentheses as a minus.
pub fn value_for_compute(text: &str) -> f64 {
    let negative = text.find(')').is_some_and(|i| i > 0);
    let digits: String = text
        .chars()
        .filter(|c| !matches!(c, '(' | ')' | ','))
        .collect();
    let value: f64 = digits.parse().unwrap_or(f64::NAN);
    if negative { -value } else { value }
}

/// The value a computed field shows, as the next handler reads it.
fn shown(value: f64) -> f64 {
    value_for_compute(&round_amount_text(&js_number_text(value)))
}

/// The text an amount control holds (and the uploaded file carries):
/// `roundAmount` with thousands commas, losses as `(1,234.50)`.
fn submit_text(value: f64) -> String {
    round_amount_text(&js_number_text(value))
}

/// `roundElement` on a typed amount (blank → `0.00`).
fn typed(value: f64) -> f64 {
    if value.is_finite() { shown(value) } else { 0.0 }
}

fn parse_mmddyyyy(text: &str) -> Option<NaiveDate> {
    let parts: Vec<&str> = text.split('/').collect();
    if parts.len() != 3
        || parts[0].len() != 2
        || parts[1].len() != 2
        || parts[2].len() != 4
        || !parts.iter().all(|p| digits_only(p))
    {
        return None;
    }
    let year: i32 = parts[2].parse().ok()?;
    if year < 1800 {
        return None;
    }
    NaiveDate::from_ymd_opt(year, parts[0].parse().ok()?, parts[1].parse().ok()?)
}

impl Form1707ADraft {
    pub const FORM_CODE: &'static str = "1707A";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, open_ended_key: u32) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            filing_year: year,
            open_ended_key,
            fiscal: false,
            year_end_month: 12,
            year_end_day: 31,
            year_end_year: year,
            is_amended: false,
            number_of_attached_sheets: 0,
            atc: None,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            line_of_business: profile.line_of_business.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            tax_relief: None,
            gains: Vec::new(),
            losses: Vec::new(),
            s1_modal_totals: [0.0; 4],
            s2_modal_totals: [0.0; 4],
            s1_total_selling_price: 0.0,
            s1_total_cost: 0.0,
            s1_total_gains: 0.0,
            s1_total_tax_paid: 0.0,
            s2_total_selling_price: 0.0,
            s2_total_cost: 0.0,
            s2_total_loss: 0.0,
            net_capital_gain: 0.0,
            tax_due: 0.0,
            tax_paid_previous: 0.0,
            total_tax_paid: 0.0,
            tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 1 (`checkCalendarFiscal`): a calendar year ends on 12/31.
    pub fn set_fiscal(&mut self, fiscal: bool) {
        self.fiscal = fiscal;
        if fiscal {
            self.year_end_month = 4;
            self.year_end_day = 1;
        } else {
            self.year_end_month = 12;
            self.year_end_day = 31;
        }
        self.recompute();
    }

    /// Item 4 (`atcClick`): individuals file on a calendar year.
    pub fn set_atc(&mut self, atc: Form1707AAtc) {
        self.atc = Some(atc);
        if atc == Form1707AAtc::Individual {
            self.set_fiscal(false);
        }
        self.recompute();
    }

    /// Item 2 (`checkAmendedForItem16A`): "No" resets 16B to `0.00`.
    pub fn set_amended(&mut self, amended: bool) {
        self.is_amended = amended;
        if !amended {
            self.tax_paid_previous = 0.0;
        }
        self.recompute();
    }

    /// The official compute chain, run as if every changed field was blurred.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_paid_previous = 0.0;
        }
        if !self.fiscal {
            self.year_end_month = 12;
            self.year_end_day = 31;
        }
        for row in &mut self.gains {
            row.selling_price = typed(row.selling_price);
            row.cost = typed(row.cost);
            row.tax_paid = typed(row.tax_paid);
            row.gain_or_loss = shown(precise(row.selling_price - row.cost));
        }
        for row in &mut self.losses {
            row.selling_price = typed(row.selling_price);
            row.cost = typed(row.cost);
            row.tax_paid = 0.0;
            row.gain_or_loss = shown(precise(row.cost - row.selling_price));
        }
        // modalSched1Subtotal / modalSched2Subtotal over rows 4 onwards.
        let modal = |rows: &[Form1707ARow]| -> [f64; 4] {
            let mut sums = [0.0; 4];
            if rows.len() > FORM_1707A_SCHEDULE_ROWS {
                for row in &rows[FORM_1707A_SCHEDULE_ROWS - 1..] {
                    for (sum, value) in sums.iter_mut().zip([
                        row.selling_price,
                        row.cost,
                        row.gain_or_loss,
                        row.tax_paid,
                    ]) {
                        *sum = precise(*sum + value);
                    }
                }
            }
            sums.map(shown)
        };
        self.s1_modal_totals = modal(&self.gains);
        self.s2_modal_totals = modal(&self.losses);
        let gains: Vec<Form1707ARow> = (0..self.gains.len().min(FORM_1707A_SCHEDULE_ROWS))
            .map(|i| self.main_row(true, i))
            .collect();
        let losses: Vec<Form1707ARow> = (0..self.losses.len().min(FORM_1707A_SCHEDULE_ROWS))
            .map(|i| self.main_row(false, i))
            .collect();
        let total = |rows: &[Form1707ARow], pick: fn(&Form1707ARow) -> f64| {
            let mut sum = 0.0;
            for row in rows {
                sum = precise(sum + pick(row));
            }
            shown(sum)
        };
        self.s1_total_selling_price = total(&gains, |r| r.selling_price);
        self.s1_total_cost = total(&gains, |r| r.cost);
        self.s1_total_gains = total(&gains, |r| r.gain_or_loss);
        self.s1_total_tax_paid = total(&gains, |r| r.tax_paid);
        self.s2_total_selling_price = total(&losses, |r| r.selling_price);
        self.s2_total_cost = total(&losses, |r| r.cost);
        self.s2_total_loss = total(&losses, |r| r.gain_or_loss);

        self.net_capital_gain = shown(precise(self.s1_total_gains - self.s2_total_loss));
        let tax_due = precise(self.net_capital_gain * (FORM_1707A_TAX_RATE / 100.0));
        self.tax_due = if tax_due < 0.0 { 0.0 } else { shown(tax_due) };
        self.tax_paid_previous = typed(self.tax_paid_previous);
        self.total_tax_paid = shown(precise(self.s1_total_tax_paid + self.tax_paid_previous));
        self.tax_payable = shown(precise(self.tax_due - self.total_tax_paid));
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = typed(self.surcharge);
        self.interest = typed(self.interest);
        self.compromise = typed(self.compromise);
        self.total_penalties = shown(precise(self.surcharge + self.interest + self.compromise));
        let total = if self.tax_payable < 0.0 && self.total_penalties > 0.0 {
            self.total_penalties
        } else {
            precise(self.tax_payable + self.total_penalties)
        };
        self.total_amount_payable = shown(total);
    }

    fn year_end(&self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(
            i32::from(self.year_end_year),
            u32::from(self.year_end_month),
            u32::from(self.year_end_day),
        )
    }

    fn popup(rows: &[Form1707ARow]) -> bool {
        rows.len() > FORM_1707A_SCHEDULE_ROWS
    }

    /// Main-table row `index` (0..4) as the page shows it: row 4 reads
    /// "OTHERS" with the pop-up totals once the pop-up holds the rest.
    fn main_row(&self, gains: bool, index: usize) -> Form1707ARow {
        let (rows, totals) = if gains {
            (&self.gains, self.s1_modal_totals)
        } else {
            (&self.losses, self.s2_modal_totals)
        };
        if index == FORM_1707A_SCHEDULE_ROWS - 1 && Self::popup(rows) {
            return Form1707ARow {
                date: String::new(),
                corporation: "OTHERS".into(),
                selling_price: totals[0],
                cost: totals[1],
                gain_or_loss: totals[2],
                tax_paid: if gains { totals[3] } else { 0.0 },
            };
        }
        rows.get(index).cloned().unwrap_or_default()
    }

    /// The generated layout with the "More" pop-up rows spliced in where the
    /// page keeps them: Schedule 1's rows after `ModalSched2Length`,
    /// Schedule 2's after `txtS1ModalTotalTaxPaid`.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        let base = crate::official_xml::layout(FORM_1707A_FORM_ID).map_err(|e| e.to_string())?;
        let mut insertions = Vec::new();
        for (rows, after, columns) in [
            (
                &self.gains,
                "ModalSched2Length",
                &[
                    "txtS1C1DateOfTransaction_",
                    "txtS1C2NameOfCorporateStock_",
                    "txtS1C3SellingPrice_",
                    "txtS1C4Cost_",
                    "txtS1C5CapitalGains_",
                    "txtS1C6TaxPaid_",
                ][..],
            ),
            (
                &self.losses,
                "frm1707Av2021:txtS1ModalTotalTaxPaid",
                &[
                    "txtS2C1DateOfTransaction_",
                    "txtS2C2NameOfCorporateStock_",
                    "txtS2C3SellingPrice_",
                    "txtS2C4Cost_",
                    "txtS2C5CapitalLoss_",
                ][..],
            ),
        ] {
            if !Self::popup(rows) {
                continue;
            }
            let count = rows.len() - FORM_1707A_SCHEDULE_ROWS + 1;
            let copies = (1..=count)
                .flat_map(|k| {
                    columns.iter().map(move |column| {
                        (
                            "ModalSched2Length".to_string(),
                            format!("frm1707Av2021:{column}{k}"),
                        )
                    })
                })
                .collect();
            insertions.push(RowInsertion {
                after: after.to_string(),
                copies,
            });
        }
        extend_layout(base, &insertions)
    }

    /// Print preview fields: the upload map plus Items 10 and 11, which the
    /// sheet prints in one 40-slot comb (p1c39): the contact number from
    /// slot 0 and the email, as typed, from slot 12 under its caption. An
    /// email longer than the rest of the comb is kept whole so the filler
    /// prints the line in full instead of cutting it. `derived:tin_digits`
    /// is the page 2 header TIN (one 14-digit comb) and
    /// `derived:address_line1` / `derived:line:address_line2` put Item 9
    /// over both its lines.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        const EMAIL_SLOT: usize = 12;
        const WIDTH: usize = 40;
        let mut fields = self.to_bir_field_map();
        let line = super::excise_places::contact_email_comb_line(
            &self.contact_number,
            &self.email,
            EMAIL_SLOT,
            WIDTH,
        );
        if !line.trim().is_empty() {
            fields.insert("derived:contact_email".to_string(), line);
        }
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        for (key, value) in [
            ("tin_digits", super::print_map::tin_digits(&self.tin)),
            ("address_line1", line1),
            ("line:address_line2", line2),
        ] {
            fields.insert(format!("derived:{key}"), value);
        }
        fields
    }

    /// The official control values the upload loop writes, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1707Av2021:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let amount = submit_text;

        put("rdoI1Calendar", flag(!self.fiscal));
        put("rdoI1Fiscal", flag(self.fiscal));
        put("txtI1YearEndMonth", format!("{:02}", self.year_end_month));
        put("txtI1YearEndDay", format!("{:02}", self.year_end_day));
        put("txtI1YearEndYear", self.year_end_year.to_string());
        put("rdoI2AmemdedYes", flag(self.is_amended));
        put("rdoI2AmemdedNo", flag(!self.is_amended));
        put("txtI3Sheets", self.number_of_attached_sheets.to_string());
        put(
            "rdoI4ATCII030",
            flag(self.atc == Some(Form1707AAtc::Individual)),
        );
        put("txtI4TCII030", "II030".to_string());
        put(
            "rdoI4ATCIC110",
            flag(self.atc == Some(Form1707AAtc::Corporation)),
        );
        put("txtI4ATCIC110", "IC110".to_string());

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtI5TIN1", tin1.clone());
        put("txtI5TIN2", tin2.clone());
        put("txtI5TIN3", tin3);
        put("txtI5BranchMask", branch.clone());
        put("hdnI5TIN4", branch.clone());
        let rdo = if self.rdo_code.trim().is_empty() {
            "000".to_string()
        } else {
            self.rdo_code.trim().to_string()
        };
        put("hdnRDO", rdo.clone());
        put("rdoI6RDO", rdo);
        put("txtI8RegisteredName", self.taxpayer_name.to_uppercase());
        put("txtLineOfBusiness", self.line_of_business.clone());
        put(
            "txtI9RegisteredAddress",
            self.registered_address.to_uppercase(),
        );
        put("txtI10ZipCode", self.zip_code.clone());
        put("txtI7TelNo", self.contact_number.clone());
        put("txtI11Email", self.email.clone());
        put("rdoI11TaxTreatyYes", flag(self.tax_relief.is_some()));
        put("rdoI11TaxTreatyNo", flag(self.tax_relief.is_none()));
        put(
            "txtI11ASpecify",
            self.tax_relief
                .as_deref()
                .map(|s| s.trim().to_uppercase())
                .unwrap_or_default(),
        );

        put("txtI12TotalCapitalGains", amount(self.s1_total_gains));
        put("txtI13TotalCapitalLoss", amount(self.s2_total_loss));
        put("txtI14NetCapitalGainLoss", amount(self.net_capital_gain));
        put("txtI16ApplicableTaxRate", "15".to_string());
        put("txtI15TaxDue", amount(self.tax_due));
        put("txtI16ATotalTaxPaid", amount(self.s1_total_tax_paid));
        put("txtI16BTotalTaxPaid", amount(self.tax_paid_previous));
        put("txtI16CTotalTaxPaid", amount(self.total_tax_paid));
        put("txtI17TaxStillPayable", amount(self.tax_payable));
        put("txtI18ASurcharge", amount(self.surcharge));
        put("txtI18BInterest", amount(self.interest));
        put("txtI18CCompromise", amount(self.compromise));
        put("txtI18DPenalties", amount(self.total_penalties));
        put(
            "txtI19TotalAmountPayable",
            amount(self.total_amount_payable),
        );

        // Page 2 header, as loadBGData fills it (TIN3 gets TIN2).
        put("txtI5TIN1P2", tin1);
        put("txtI5TIN2P2", tin2.clone());
        put("txtI5TIN3P2", tin2);
        put("txtI5BranchMaskP2", branch.clone());
        put("hdnI5TIN4P2", branch);
        put("txtI8RegisteredNameP2", self.taxpayer_name.clone());

        for index in 0..FORM_1707A_SCHEDULE_ROWS {
            let n = index + 1;
            let gain = self.main_row(true, index);
            put(
                &format!("txtS1C1DateOfTransactionI{n}"),
                gain.date.trim().to_string(),
            );
            put(
                &format!("txtS1C2NameOfCorporateStockI{n}"),
                gain.corporation.trim().to_uppercase(),
            );
            put(
                &format!("txtS1C3SellingPriceI{n}"),
                amount(gain.selling_price),
            );
            put(&format!("txtS1C4CostI{n}"), amount(gain.cost));
            put(
                &format!("txtS1C5CapitalGainsI{n}"),
                amount(gain.gain_or_loss),
            );
            put(&format!("txtS1C6TaxPaidI{n}"), amount(gain.tax_paid));
            let loss = self.main_row(false, index);
            put(
                &format!("txtS2C1DateOfTransactionI{n}"),
                loss.date.trim().to_string(),
            );
            put(
                &format!("txtS2C2NameOfCorporateStockI{n}"),
                loss.corporation.trim().to_uppercase(),
            );
            put(
                &format!("txtS2C3SellingPriceI{n}"),
                amount(loss.selling_price),
            );
            put(&format!("txtS2C4CostI{n}"), amount(loss.cost));
            put(
                &format!("txtS2C5CapitalLossI{n}"),
                amount(loss.gain_or_loss),
            );
        }
        put("txtS1I20TotalCost", amount(self.s1_total_cost));
        put(
            "txtS1I20TotalSellingPrice",
            amount(self.s1_total_selling_price),
        );
        put("txtS1I20TotalCapitalGains", amount(self.s1_total_gains));
        put("txtS1I20TotalTaxPaid", amount(self.s1_total_tax_paid));
        put(
            "txtS2I21TotalSellingPrice",
            amount(self.s2_total_selling_price),
        );
        put("txtS2I21TotalCost", amount(self.s2_total_cost));
        put("txtS2I21TotalCapitalLoss", amount(self.s2_total_loss));
        for (prefix, rows, totals, names) in [
            (
                "S1",
                &self.gains,
                self.s1_modal_totals,
                &["SellingPrice", "Cost", "CapitalGains", "TaxPaid"][..],
            ),
            (
                "S2",
                &self.losses,
                self.s2_modal_totals,
                &["SellingPrice", "Cost", "CapitalLoss"][..],
            ),
        ] {
            if !Self::popup(rows) {
                continue;
            }
            for (name, total) in names.iter().zip(totals) {
                put(&format!("txt{prefix}ModalTotal{name}"), amount(total));
            }
            for (k, row) in rows[FORM_1707A_SCHEDULE_ROWS - 1..].iter().enumerate() {
                let k = k + 1;
                put(
                    &format!("txt{prefix}C1DateOfTransaction_{k}"),
                    row.date.trim().to_string(),
                );
                put(
                    &format!("txt{prefix}C2NameOfCorporateStock_{k}"),
                    row.corporation.trim().to_uppercase(),
                );
                put(
                    &format!("txt{prefix}C3SellingPrice_{k}"),
                    amount(row.selling_price),
                );
                put(&format!("txt{prefix}C4Cost_{k}"), amount(row.cost));
                if prefix == "S1" {
                    put(
                        &format!("txtS1C5CapitalGains_{k}"),
                        amount(row.gain_or_loss),
                    );
                    put(&format!("txtS1C6TaxPaid_{k}"), amount(row.tax_paid));
                } else {
                    put(&format!("txtS2C5CapitalLoss_{k}"), amount(row.gain_or_loss));
                }
            }
        }
        for (key, rows) in [
            ("ModalSched1Length", &self.gains),
            ("ModalSched2Length", &self.losses),
        ] {
            if Self::popup(rows) {
                fields.insert(
                    key.to_string(),
                    (rows.len() - FORM_1707A_SCHEDULE_ROWS + 1).to_string(),
                );
            }
        }
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `validateAll()` against a given "today" (the page uses the clock).
    pub fn validate_on(&self, today: NaiveDate) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // validateYearEnd, then checkVersion.
        match self.year_end() {
            None => err(
                "year_end",
                "Please provide a valid date (MM/DD/YYYY format) on Item 1",
            ),
            Some(year_end) => {
                if year_end < NaiveDate::from_ymd_opt(2001, 7, 1).unwrap_or(year_end) {
                    err(
                        "year_end",
                        "Valid input for the Month and Year is June 2001 onwards on Item 1.",
                    );
                } else if year_end > today {
                    err(
                        "year_end",
                        "Year Ended should not be a future date on Item 1.",
                    );
                } else if self.fiscal && self.year_end_month == 12 {
                    err("year_end", "Month cannot be equal to December on Item 1.");
                } else if year_end <= NaiveDate::from_ymd_opt(2021, 3, 31).unwrap_or(year_end) {
                    err(
                        "year_end",
                        "Year shall not be greater than the present year and not earlier than April 2021.",
                    );
                }
            }
        }
        match self.atc {
            None => err("atc", "Please select an ATC Code on Item 5."),
            Some(Form1707AAtc::Individual) if self.fiscal => err(
                "atc",
                "If you are filing as Individual, please select Calendar on item 1.",
            ),
            Some(_) => {}
        }
        let rdo = self.rdo_code.trim();
        if rdo.is_empty() || rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err("rdo_code", "Please provide a valid RDO Code on Item 7.");
        }
        let name_limit = if self.atc == Some(Form1707AAtc::Corporation) {
            50
        } else {
            90
        };
        if self.taxpayer_name.trim().is_empty() {
            err(
                "taxpayer_name",
                "Please provide a valid Line of Business/Occupation on Item 8.",
            );
        } else if self.taxpayer_name.trim().chars().count() > name_limit {
            err(
                "taxpayer_name",
                &format!("Item 8 holds at most {name_limit} characters."),
            );
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Please provide a valid Registered Address on Item 9.",
            );
        } else if self.registered_address.trim().chars().count() > 60 {
            err("registered_address", "Item 9 holds at most 60 characters.");
        }
        // The page only warns here; a submission needs the zip code.
        if self.zip_code.trim().is_empty() {
            err(
                "zip_code",
                "Zip Code is required, please validate your registration information and update accordingly using BIR Form 1905, if necessary.",
            );
        }
        if let Some(spec) = &self.tax_relief {
            if spec.trim().is_empty() {
                err("tax_relief", "Please specify the tax relief on item 12A.");
            } else if spec.trim().chars().count() > 20 {
                err("tax_relief", "Item 12A holds at most 20 characters.");
            }
        }
        if self.s1_total_selling_price == 0.0
            && self.s1_total_cost == 0.0
            && self.s2_total_selling_price == 0.0
            && self.s2_total_cost == 0.0
        {
            err(
                "gains",
                "You need to have at least one value on either Schedule 1 or Schedule 2.",
            );
        }

        // validateSchedules for Schedule 1, then Schedule 2.
        let year_end = self.year_end();
        let year_from = year_end.and_then(|d| d.with_year(d.year() - 1));
        let main = |rows: &Vec<Form1707ARow>| {
            if Self::popup(rows) {
                rows[..FORM_1707A_SCHEDULE_ROWS - 1].to_vec()
            } else {
                rows.clone()
            }
        };
        let modal = |rows: &Vec<Form1707ARow>| {
            if Self::popup(rows) {
                rows[FORM_1707A_SCHEDULE_ROWS - 1..].to_vec()
            } else {
                Vec::new()
            }
        };
        for (schedule, field, rows, is_sched1, is_modal, offset) in [
            (" Schedule 1", "gains", main(&self.gains), true, false, 0),
            (" Schedule 2", "losses", main(&self.losses), false, false, 0),
            (
                " Schedule 1 Additional Items",
                "gains",
                modal(&self.gains),
                true,
                true,
                FORM_1707A_SCHEDULE_ROWS - 1,
            ),
            (
                " Schedule 2 Additional Items",
                "losses",
                modal(&self.losses),
                false,
                true,
                FORM_1707A_SCHEDULE_ROWS - 1,
            ),
        ] {
            for (index, row) in rows.iter().enumerate() {
                let n = index + 1;
                let index = index + offset;
                let key = format!("{field}[{index}]");
                let mut empty = 0;
                if row.date_blank() {
                    empty += 1;
                } else {
                    match parse_mmddyyyy(row.date.trim()) {
                        None => {
                            err(
                                &key,
                                &format!(
                                    "Please provide a valid date. (MM/DD/YYYY format) in Date of Transaction on row {n}{schedule}"
                                ),
                            );
                            continue;
                        }
                        Some(date) => {
                            if date.year() < 2001 {
                                err(
                                    &key,
                                    &format!(
                                        "The year should not be less than 2001 in Date of Transaction on row {n}{schedule}"
                                    ),
                                );
                                continue;
                            } else if year_end.is_some_and(|end| date > end) {
                                err(
                                    &key,
                                    &format!(
                                        "The Date of Transaction should not be greater than the Taxable Period on row {n}{schedule}"
                                    ),
                                );
                                continue;
                            } else if year_from.is_some_and(|from| date < from) {
                                err(
                                    &key,
                                    &format!(
                                        "The Date of Transaction should be in the range of up to one year until the Taxable Period on row {n}{schedule}"
                                    ),
                                );
                                continue;
                            }
                        }
                    }
                }
                let corporation = row.corporation.trim();
                if corporation.is_empty() {
                    empty += 1;
                } else if corporation.parse::<f64>().is_ok() {
                    err(
                        &key,
                        &format!(
                            "Name of Corporate Stock should not be all numeric in row {n}{schedule}"
                        ),
                    );
                    continue;
                }
                if row.selling_price == 0.0 {
                    empty += 1;
                }
                if row.cost == 0.0 {
                    empty += 1;
                }
                if empty != 0 && empty != 4 {
                    err(
                        &key,
                        &format!(
                            "There is an empty item on {schedule}. Please fill in all items in row {n}"
                        ),
                    );
                    continue;
                }
                if is_sched1 {
                    if row.tax_paid != 0.0 && empty > 0 {
                        let message = if is_modal {
                            format!(" Please fill in all items in row {n}{schedule}")
                        } else {
                            format!(
                                "There is an empty item on {schedule}. Please fill in all items in row {n}"
                            )
                        };
                        err(&key, &message);
                        continue;
                    }
                    if row.gain_or_loss < 0.0 {
                        err(
                            &key,
                            &format!(
                                "Your Capital Gains is negative in row {n}{schedule}\n Please move and encode this in Schedule 2"
                            ),
                        );
                    }
                } else if row.gain_or_loss < 0.0 {
                    err(
                        &key,
                        &format!(
                            "Your Capital Loss is negative in row {n}{schedule}\n Please move and encode this in Schedule 1"
                        ),
                    );
                }
                if is_modal && empty != 0 {
                    err(
                        &key,
                        &format!(
                            "Row {n} is empty. Please remove row or fill in all items first in{schedule}"
                        ),
                    );
                }
                // `.txtCorporate` keeps 17 (pop-up: 15) of [a-zA-Z 0-9.,#@'()_-].
                if corporation.chars().count() > if is_modal { 15 } else { 17 }
                    || !corporation
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || " .,#@'()_-".contains(c))
                {
                    err(
                        &key,
                        "Name of Corporate Stock holds 17 letters, digits, spaces or .,#@'()_- only.",
                    );
                }
            }
        }

        // Typing limits.
        let mut amounts: Vec<(String, f64)> = vec![
            ("tax_paid_previous".into(), self.tax_paid_previous),
            ("surcharge".into(), self.surcharge),
            ("interest".into(), self.interest),
            ("compromise".into(), self.compromise),
        ];
        for (field, rows) in [("gains", &self.gains), ("losses", &self.losses)] {
            for (index, row) in rows.iter().enumerate() {
                amounts.push((format!("{field}[{index}].selling_price"), row.selling_price));
                amounts.push((format!("{field}[{index}].cost"), row.cost));
                amounts.push((format!("{field}[{index}].tax_paid"), row.tax_paid));
            }
        }
        for (field, value) in amounts {
            if value < 0.0 || value.abs() >= 1_000_000_000_000.0 {
                err(
                    &field,
                    "Enter a non-negative amount below 1,000,000,000,000.",
                );
            }
        }
        if !self.is_amended && self.tax_paid_previous != 0.0 {
            err(
                "tax_paid_previous",
                "Item 16B applies only to an amended return.",
            );
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most two digits.",
            );
        }
        let email = self.email.trim();
        if email.is_empty()
            || !email.contains('@')
            || email.contains(char::is_whitespace)
            || email.contains('#')
            || email.chars().count() > 60
        {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }
        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        if !tin_is_well_formed(&self.tin) {
            err("tin", "The taxpayer profile needs a valid TIN.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
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

impl FormValidator for Form1707ADraft {
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1707ADraft {
    /// Items 20A-20C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "20A Surcharge",
                interest: "20B Interest",
                compromise: "20C Compromise",
            ],
            controls: [
                "frm1707Av2021:txtI18ASurcharge",
                "frm1707Av2021:txtI18BInterest",
                "frm1707Av2021:txtI18CCompromise",
            ],
            state: penalty,
            basic_tax_due: "Item 19", |d| d.tax_payable,
            // Calendar-year returns (year ended 31 December) use the
            // calendar's annual 1707-A deadline; a fiscal year has none there.
            deadline_period: |d: &Self| {
                (!d.fiscal && d.year_end_month == 12 && d.year_end_day == 31).then(|| {
                    crate::calendar_rules::DeadlinePeriod::Annual {
                        taxable_year: i32::from(d.year_end_year),
                    }
                })
            },
        )
    }
}

impl QueueableForm for Form1707ADraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1707A";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1707Av2021']`).
    const FORM_TYPE: &'static str = "1707Av2021";
    const LAYOUT_ID: &'static str = FORM_1707A_FORM_ID;

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
        self.set_amended(true);
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
        self.filing_year
    }
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::OpenEnded(self.open_ended_key)
    }
    /// Year-end `MM + DD + YYYY`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!(
            "{:02}{:02}{:04}",
            self.year_end_month, self.year_end_day, self.year_end_year
        )
    }
    /// The filename names the year end, not the dashboard's open-ended key,
    /// so a receipt cannot be mapped back to a draft row.
    fn parse_period_code(_code: &str) -> Option<(u16, FilingPeriod)> {
        None
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
    /// The "More" pop-ups add rows at run time, so the plaintext follows
    /// [`Form1707ADraft::official_layout`] rather than the fixed layout.
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
        NaiveDate::from_ymd_opt(2026, 6, 30).unwrap()
    }

    fn row(date: &str, name: &str, selling: f64, cost: f64, tax_paid: f64) -> Form1707ARow {
        Form1707ARow {
            date: date.into(),
            corporation: name.into(),
            selling_price: selling,
            cost,
            gain_or_loss: 0.0,
            tax_paid,
        }
    }

    #[test]
    fn print_map_shares_the_item_10_11_comb() {
        let mut draft = sample();
        draft.contact_number = "09171234567".into();
        draft.email = "Me@Example.ph".into();
        let fields = draft.to_print_field_map();
        assert_eq!(fields["derived:contact_email"], "09171234567 Me@Example.ph");
        draft.email = "Marita.Joy.Torres.Accounting@Example-Company.com.ph".into();
        let fields = draft.to_print_field_map();
        assert_eq!(
            fields["derived:contact_email"],
            "09171234567 Marita.Joy.Torres.Accounting@Example-Company.com.ph"
        );
    }

    pub(crate) fn sample() -> Form1707ADraft {
        let mut draft = Form1707ADraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            filing_year: 2025,
            open_ended_key: 1,
            fiscal: false,
            year_end_month: 12,
            year_end_day: 31,
            year_end_year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            atc: Some(Form1707AAtc::Individual),
            tin: "12345678800000".into(),
            rdo_code: "039".into(),
            taxpayer_name: "Sample Taxpayer".into(),
            line_of_business: "Investor".into(),
            registered_address: "1 Sample St".into(),
            zip_code: "1100".into(),
            contact_number: "09170000000".into(),
            email: "sample.taxpayer@example.com".into(),
            tax_relief: None,
            gains: vec![
                row(
                    "02/15/2025",
                    "Sample Corp",
                    500_000.009,
                    120_000.0,
                    15_000.5,
                ),
                row(
                    "06/30/2025",
                    "Example Mining",
                    1_234_567.891,
                    1_000_000.105,
                    0.0,
                ),
            ],
            losses: vec![row("09/01/2025", "Loss Co", 50_000.0, 80_000.55, 0.0)],
            s1_modal_totals: [0.0; 4],
            s2_modal_totals: [0.0; 4],
            s1_total_selling_price: 0.0,
            s1_total_cost: 0.0,
            s1_total_gains: 0.0,
            s1_total_tax_paid: 0.0,
            s2_total_selling_price: 0.0,
            s2_total_cost: 0.0,
            s2_total_loss: 0.0,
            net_capital_gain: 0.0,
            tax_due: 0.0,
            tax_paid_previous: 0.0,
            total_tax_paid: 0.0,
            tax_payable: 0.0,
            surcharge: 25.5,
            interest: 10.009,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        // This sample enters its penalties by hand.
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1707ADraft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, m)| m)
            .collect()
    }

    #[test]
    fn amount_helpers_match_the_page() {
        assert_eq!(round_amount_text("10.009"), "10.00");
        assert_eq!(round_amount_text("1234567.8"), "1,234,567.80");
        assert_eq!(round_amount_text("-12.5"), "(12.50)");
        assert_eq!(round_amount_text("-0.5"), "0.50");
        assert_eq!(round_amount_text(""), "0.00");
        assert_eq!(value_for_compute("(1,234.50)"), -1234.5);
        assert_eq!(submit_text(-1234.5), "(1,234.50)");
        assert_eq!(submit_text(1234.5), "1,234.50");
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let d = sample();
        assert_eq!(d.gains[0].selling_price, 500_000.0);
        assert_eq!(d.gains[1].cost, 1_000_000.1);
        assert_eq!(d.s1_total_gains, 614_567.79);
        assert_eq!(d.s2_total_loss, 30_000.55);
        assert_eq!(d.net_capital_gain, 584_567.24);
        assert_eq!(d.tax_due, 87_685.09);
        assert_eq!(d.tax_payable, 72_684.59);
        assert_eq!(d.total_penalties, 35.5);
        assert_eq!(d.total_amount_payable, 72_720.09);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let d = sample();
        let f = d.to_bir_field_map();
        assert_eq!(f["frm1707Av2021:txtI12TotalCapitalGains"], "614,567.79");
        assert_eq!(f["frm1707Av2021:txtI5TIN3P2"], "456");
        assert_eq!(f["frm1707Av2021:txtI8RegisteredName"], "SAMPLE TAXPAYER");
        assert_eq!(f["frm1707Av2021:txtI8RegisteredNameP2"], "Sample Taxpayer");
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1707Av2021-12312025#sample.taxpayer@example.com#.xml"
        );
    }

    #[test]
    fn overpayment_with_penalties_pays_penalties_only() {
        let mut d = sample();
        d.set_amended(true);
        d.tax_paid_previous = 100_000.0;
        d.recompute();
        assert_eq!(d.tax_payable, -27_315.41);
        assert_eq!(d.total_amount_payable, 35.5);
        assert_eq!(
            d.to_bir_field_map()["frm1707Av2021:txtI17TaxStillPayable"],
            "(27,315.41)"
        );
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1707ADraft), expected: &str| {
            let mut d = sample();
            mutate(&mut d);
            d.recompute();
            let found = messages(&d);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.year_end_year = 2026,
            "Year Ended should not be a future date on Item 1.",
        );
        check(
            &|d| d.year_end_year = 2000,
            "Valid input for the Month and Year is June 2001 onwards on Item 1.",
        );
        check(
            &|d| d.year_end_year = 2020,
            "Year shall not be greater than the present year and not earlier than April 2021.",
        );
        check(
            &|d| {
                d.atc = Some(Form1707AAtc::Corporation);
                d.fiscal = true;
                d.year_end_month = 2;
                d.year_end_day = 30;
            },
            "Please provide a valid date (MM/DD/YYYY format) on Item 1",
        );
        check(
            &|d| {
                d.fiscal = true;
                d.year_end_month = 6;
                d.year_end_day = 30;
            },
            "If you are filing as Individual, please select Calendar on item 1.",
        );
        check(&|d| d.atc = None, "Please select an ATC Code on Item 5.");
        check(
            &|d| d.rdo_code = "000".into(),
            "Please provide a valid RDO Code on Item 7.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please provide a valid Line of Business/Occupation on Item 8.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please provide a valid Registered Address on Item 9.",
        );
        check(
            &|d| d.tax_relief = Some(" ".into()),
            "Please specify the tax relief on item 12A.",
        );
        check(
            &|d| {
                d.gains.clear();
                d.losses.clear();
            },
            "You need to have at least one value on either Schedule 1 or Schedule 2.",
        );
        check(
            &|d| d.gains[0].date = "13/01/2025".into(),
            "Please provide a valid date. (MM/DD/YYYY format) in Date of Transaction on row 1 Schedule 1",
        );
        check(
            &|d| d.gains[0].date = "12/01/2024".into(),
            "The Date of Transaction should be in the range of up to one year until the Taxable Period on row 1 Schedule 1",
        );
        check(
            &|d| d.gains[0].date = "01/02/2026".into(),
            "The Date of Transaction should not be greater than the Taxable Period on row 1 Schedule 1",
        );
        check(
            &|d| d.gains[0].corporation = "12345".into(),
            "Name of Corporate Stock should not be all numeric in row 1 Schedule 1",
        );
        check(
            &|d| d.gains[0].corporation.clear(),
            "There is an empty item on  Schedule 1. Please fill in all items in row 1",
        );
        check(
            &|d| d.gains[0].cost = 600_000.0,
            "Your Capital Gains is negative in row 1 Schedule 1\n Please move and encode this in Schedule 2",
        );
        check(
            &|d| d.losses[0].selling_price = 90_000.0,
            "Your Capital Loss is negative in row 1 Schedule 2\n Please move and encode this in Schedule 1",
        );
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut d = sample();
        d.queue(crate::filing_queue::QueueAuthSource::Gui).unwrap();
        assert!(d.revalidate_queued_before_submission().is_ok());
        d.surcharge = 99.0;
        d.keep_entered_penalties();
        assert!(d.revalidate_queued_before_submission().is_err());
    }
}
