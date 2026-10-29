//! BIR Form 1702Q (January 2018) — Quarterly Income Tax Return for
//! Corporations, Partnerships and Other Non-Individual Taxpayers.
//!
//! Ported from the official `BIR-Form1702Qv2018C.hta` (eBIRForms 7.9.6.2.1):
//! the ATC rules (`ATCEnableDisableSchedules`, `getATCRate`,
//! `Sched2Item10TaxRate`, `Sched3Item5TaxRate`, `enableOptionalDeduction`,
//! `ProcessAmended`, `ProcessTaxableIncomePrevQtr`), the compute chain
//! (`Sched1_Computation` … `Part2_Computation`), `validate()` with its exact
//! alert texts, and `saveXMLsubmit` through [`crate::official_xml`].
//!
//! Every amount entry passes through `roundupdown()` (`Math.round`), so the
//! model holds whole pesos; computed items are `formatCurrency` values.
//!
//! Engine note: `Sched1_Computation` formats with
//! `toLocaleString('en-US', {style: 'currency', …})`. The legacy JScript
//! engine eBIRForms runs on ignores those arguments and returns the locale
//! number (`1,234.00`); a modern engine returns `$1,234`, which the page's own
//! `NumWithComma` cannot parse, wiping out Items 17 and 18. The model follows
//! the legacy engine, as `validateFiscalMonth`'s `getYear()` also assumes.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::form_2551q::TaxPeriodBasis;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::official_amount;
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1702Q_FORM_ID: &str = "1702q-v2018c";
/// Rows of "Other Tax Credits/Payments" on the official page (6a, 6b).
pub const FORM_1702Q_OTHER_CREDIT_ROWS: usize = 2;

/// How Item 5's ATC drives the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form1702qAtcGroup {
    /// 30/25/20% ATCs: Schedule 2 rate comes from the ATC.
    Regular,
    /// 0% / exempt ATCs (IC011, IC021, IC200 0%).
    Exempt,
    /// Special-rate ATCs: the filer enters the Schedule 2 rate, if any.
    Special,
}

/// One option of the official `cbATC_2` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form1702qAtcOption {
    /// The option value the page submits, e.g. `"IC010_25%"`.
    pub value: &'static str,
    pub label: &'static str,
    /// `getATCRate` text, e.g. `"25.00"` (`"2.5"` for IC080).
    pub rate_text: &'static str,
    pub group: Form1702qAtcGroup,
}

const fn atc(
    value: &'static str,
    label: &'static str,
    rate_text: &'static str,
    group: Form1702qAtcGroup,
) -> Form1702qAtcOption {
    Form1702qAtcOption {
        value,
        label,
        rate_text,
        group,
    }
}

use Form1702qAtcGroup::{Exempt as ATC_EXEMPT, Regular as ATC_REGULAR, Special as ATC_SPECIAL};

/// The official list, in page order.
pub const FORM_1702Q_ATC_OPTIONS: &[Form1702qAtcOption] = &[
    atc(
        "IC010_30%",
        "IC 010 Domestic Corporation in General - 30%",
        "30.00",
        ATC_REGULAR,
    ),
    atc(
        "IC010_25%",
        "IC 010 Domestic Corporation in General - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC010_20%",
        "IC 010 Domestic Corporation in General - 20%",
        "20.00",
        ATC_REGULAR,
    ),
    atc(
        "IC011_0%",
        "IC 011 Exempt Corporation on Exempt Activities - 0%",
        "0.00",
        ATC_EXEMPT,
    ),
    atc(
        "IC020_30%",
        "IC 020 Taxable Partnership - 30%",
        "30.00",
        ATC_REGULAR,
    ),
    atc(
        "IC020_25%",
        "IC 020 Taxable Partnership - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC020_20%",
        "IC 020 Taxable Partnership - 20%",
        "20.00",
        ATC_REGULAR,
    ),
    atc(
        "IC021_Exempt",
        "IC 021 General Professional Partnership - Exempt",
        "0.00",
        ATC_EXEMPT,
    ),
    atc(
        "IC030_10%",
        "IC 030 Proprietary Educational Institutions - 10%",
        "10.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC030_1%",
        "IC 030 Proprietary Educational Institutions - 1%",
        "1.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC030_30%",
        "IC 030 Proprietary Educational Institutions - 30%",
        "30.00",
        ATC_REGULAR,
    ),
    atc(
        "IC030_25%",
        "IC 030 Proprietary Educational Institutions - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC030_20%",
        "IC 030 Proprietary Educational Institutions - 20%",
        "20.00",
        ATC_REGULAR,
    ),
    atc(
        "IC031_10%",
        "IC 031 Non-Stock, Non-Profit Hospitals - 10%",
        "10.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC031_1%",
        "IC 031 Non-Stock, Non-Profit Hospitals - 1%",
        "1.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC031_30%",
        "IC 031 Non-Stock, Non-Profit Hospitals - 30%",
        "30.00",
        ATC_REGULAR,
    ),
    atc(
        "IC031_25%",
        "IC 031 Non-Stock, Non-Profit Hospitals - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC031_20%",
        "IC 031 Non-Stock, Non-Profit Hospitals - 20%",
        "20.00",
        ATC_REGULAR,
    ),
    atc(
        "IC040_30%",
        "IC 040 GOCC, Agencies and Instrumentalities - 30%",
        "30.00",
        ATC_REGULAR,
    ),
    atc(
        "IC040_25%",
        "IC 040 GOCC, Agencies and Instrumentalities - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC040_20%",
        "IC 040 GOCC, Agencies and Instrumentalities - 20%",
        "20.00",
        ATC_REGULAR,
    ),
    atc(
        "IC041_30%",
        "IC 041 National Gov't. & LGUs - 30%",
        "30.00",
        ATC_REGULAR,
    ),
    atc(
        "IC041_25%",
        "IC 041 National Gov't. & LGUs - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC041_20%",
        "IC 041 National Gov't. & LGUs - 20%",
        "20.00",
        ATC_REGULAR,
    ),
    atc(
        "IC200_0%",
        "IC 200 PEZA Free Port Zones - 0%",
        "0.00",
        ATC_EXEMPT,
    ),
    atc(
        "IC200_5%",
        "IC 200 PEZA Free Port Zones - 5%",
        "5.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC210_2%",
        "IC 210 Microfinance Non-Government Organizations (NGOs) - 2%",
        "2.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC070_30%",
        "IC 070 Resident Foreign Corporation in General - 30%",
        "30.00",
        ATC_REGULAR,
    ),
    atc(
        "IC070_25%",
        "IC 070 Resident Foreign Corporation in General - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC080_2.5%",
        "IC 080 International Carriers - 2.5%",
        "2.5",
        ATC_SPECIAL,
    ),
    atc(
        "IC101_10%",
        "IC 101 Regional Operating Headquarters - 10%",
        "10.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC101_25%",
        "IC 101 Regional Operating Headquarters - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC190_10%_NotSubjectToFinalTax",
        "IC 190 OBUs - Foreign Currency Transaction Not Subjected to Final Tax - 10%",
        "10.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC190_25%_NotSubjectToFinalTax_New",
        "IC 190 OBUs - Foreign Currency Transaction Not Subjected to Final Tax (NEW) - 25%",
        "25.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC190_30%_OtherThanForeignCurrencyTransaction",
        "IC 190 OBUs - Other than Foreign Currency Transaction - 30%",
        "30.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC190_25%_OtherThanForeignCurrencyTransaction_New",
        "IC 190 OBUs - Other than Foreign Currency Transaction (NEW) - 25%",
        "25.00",
        ATC_SPECIAL,
    ),
    atc(
        "OBU_25%",
        "Offshore Banking Units - 25%",
        "25.00",
        ATC_REGULAR,
    ),
    atc(
        "IC191_10%",
        "IC 191 Foreign Currency Transaction Not Subjected to Final Tax - 10%",
        "10.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC191_25%_NotSubjectToFinalTax_New",
        "IC 191 Foreign Currency Transaction Not Subjected to Final Tax (NEW) - 25%",
        "25.00",
        ATC_SPECIAL,
    ),
    atc(
        "IC191_30%",
        "IC 191 FCDUs - Other Than Foreign Currency - 30%",
        "30.00",
        ATC_REGULAR,
    ),
    atc(
        "IC191_25%_OtherThanForeignCurrencyTransaction_New",
        "IC 191 FCDUs - Other Than Foreign Currency (NEW) - 25%",
        "25.00",
        ATC_REGULAR,
    ),
];

/// The official option for an ATC value.
pub fn form_1702q_atc_option(value: &str) -> Option<&'static Form1702qAtcOption> {
    FORM_1702Q_ATC_OPTIONS.iter().find(|o| o.value == value)
}

/// Item 12 (`rbMthdOfDdctns_1` itemized, `_2` OSD).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1702qDeduction {
    #[default]
    Unanswered,
    Itemized,
    Osd,
}

/// One column of Schedule 1 (A exempt, B special).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1702qSchedule1Column {
    pub sales: f64,
    pub cost_of_sales: f64,
    pub gross_income: f64,
    pub other_income: f64,
    pub total_gross_income: f64,
    pub deductions: f64,
    pub taxable_income: f64,
    /// Item 8, previous quarters (2nd and 3rd quarter returns only).
    pub previous_quarters: f64,
    pub total_taxable_income: f64,
    /// Item 10 in percent. Column A is always 0 (disabled on the page).
    pub rate: f64,
    pub tax_due: f64,
}

/// One "Other Tax Credits/Payments" row (Schedule 4 Items 6a, 6b).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1702qOtherCredit {
    pub description: String,
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1702qDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–5
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub tax_period_basis: TaxPeriodBasis,
    /// Item 2 month (1–12). Calendar filers use 12.
    pub year_end_month: u8,
    /// Item 2 year (four digits; the page shows `20YY`).
    pub taxable_year: u16,
    /// Item 3 (1–3).
    pub quarter: u8,
    pub is_amended: bool,
    /// Item 5 — IC055 MCIT box.
    #[serde(default)]
    pub atc_mcit: bool,
    /// Item 5 — the `cbATC_2` option value; empty when none.
    #[serde(default)]
    pub atc: String,

    // Part I
    pub rdo_code: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,
    pub line_of_business: String,
    #[serde(default)]
    pub deduction: Form1702qDeduction,
    #[serde(default)]
    pub tax_relief: bool,
    #[serde(default)]
    pub tax_relief_specify: String,

    // Schedule 1
    #[serde(default)]
    pub exempt: Form1702qSchedule1Column,
    #[serde(default)]
    pub special: Form1702qSchedule1Column,
    /// Schedule 1 Item 12B.
    #[serde(default)]
    pub share_of_other_agencies: f64,
    /// Schedule 1 Item 13B.
    #[serde(default)]
    pub special_net_tax_due: f64,

    // Schedule 2
    #[serde(default)]
    pub sales: f64,
    #[serde(default)]
    pub cost_of_sales: f64,
    #[serde(default)]
    pub gross_income: f64,
    #[serde(default)]
    pub other_income: f64,
    #[serde(default)]
    pub total_gross_income: f64,
    /// Item 6; computed (40% of Item 5) under OSD.
    #[serde(default)]
    pub deductions: f64,
    #[serde(default)]
    pub taxable_income: f64,
    #[serde(default)]
    pub previous_quarters_taxable_income: f64,
    #[serde(default)]
    pub total_taxable_income: f64,
    /// Item 10 text the page holds for ATCs whose rate the filer types.
    #[serde(default = "default_rate_text")]
    pub regular_rate_text: String,
    #[serde(default)]
    pub regular_tax_due: f64,
    #[serde(default)]
    pub mcit: f64,
    #[serde(default)]
    pub income_tax_due: f64,

    // Schedule 3
    #[serde(default)]
    pub mcit_gross_income_q1: f64,
    #[serde(default)]
    pub mcit_gross_income_q2: f64,
    #[serde(default)]
    pub mcit_gross_income_q3: f64,
    #[serde(default)]
    pub mcit_total_gross_income: f64,
    /// Item 5 text the filer types when the page leaves the MCIT rate open.
    #[serde(default = "default_rate_text")]
    pub mcit_rate_text: String,

    // Schedule 4
    #[serde(default)]
    pub prior_year_excess_credits: f64,
    #[serde(default)]
    pub previous_quarters_payments: f64,
    #[serde(default)]
    pub previous_quarters_mcit_payments: f64,
    #[serde(default)]
    pub previous_quarters_cwt: f64,
    #[serde(default)]
    pub cwt_this_quarter: f64,
    #[serde(default)]
    pub previously_filed: f64,
    #[serde(default)]
    pub other_credits: Vec<Form1702qOtherCredit>,
    #[serde(default)]
    pub total_credits: f64,

    // Part II
    /// Item 15.
    #[serde(default)]
    pub prior_year_mcit_excess: f64,
    #[serde(default)]
    pub regular_tax_still_due: f64,
    #[serde(default)]
    pub aggregate_tax_due: f64,
    #[serde(default)]
    pub net_tax_payable: f64,
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
    #[serde(default)]
    pub total_amount_payable: f64,
    /// Item 26.
    #[serde(default)]
    pub number_of_attachments: u8,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

fn default_rate_text() -> String {
    "0.00".to_string()
}

/// `Math.round`.
fn js_round(value: f64) -> f64 {
    if value.is_finite() {
        (value + 0.5).floor()
    } else {
        0.0
    }
}

/// `formatCurrency`: two decimals, the value a computed field holds.
fn cents(value: f64) -> f64 {
    crate::official_xml::parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

fn digits(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits = digits(tin);
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

/// `"IC 010 Domestic Corporation in General - 25%"` as the sheet's code
/// cell (`IC 010`) and description cell.
fn print_atc_parts(label: &str) -> (String, String) {
    let mut words = label.splitn(3, ' ');
    let prefix = words.next().unwrap_or("");
    let number = words.next().unwrap_or("");
    let rest = words.next().unwrap_or("");
    (format!("{prefix} {number}"), rest.trim().to_string())
}

/// A comb text split over two printed lines of `width` slots.
fn print_lines(text: &str, width: usize) -> (String, String) {
    let chars: Vec<char> = text.trim().chars().collect();
    let first: String = chars.iter().take(width).collect();
    let second: String = chars
        .iter()
        .skip(width)
        .collect::<String>()
        .trim()
        .to_string();
    (first.trim_end().to_string(), second)
}

/// An official amount right-aligned in a `slots`-wide comb. Commas go first
/// when it does not fit; `None` (blank on print) when it still does not.
fn print_amount(text: &str, slots: usize) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let fitted = if text.chars().count() <= slots {
        text.to_string()
    } else {
        let bare: String = text.chars().filter(|ch| *ch != ',').collect();
        if bare.chars().count() > slots {
            return None;
        }
        bare
    };
    Some(format!("{fitted:>slots$}"))
}

/// A whole-number rate right-aligned in a 2-slot box; `None` when the rate
/// has a fraction the box cannot show.
fn print_whole_rate(text: &str) -> Option<String> {
    let rate: f64 = text.trim().parse().ok()?;
    if rate.fract() != 0.0 || !(0.0..100.0).contains(&rate) {
        return None;
    }
    Some(format!("{:>2}", rate as u32))
}

/// A rate as the 2-slot whole box and the 1-slot tenths box after the
/// printed decimal point; `None` when it needs more than tenths.
fn print_tenths_rate(text: &str) -> Option<(String, String)> {
    let rate: f64 = text.trim().parse().ok()?;
    let tenths = (rate * 10.0).round();
    if (rate * 10.0 - tenths).abs() > 1e-9 || !(0.0..1000.0).contains(&tenths) {
        return None;
    }
    let tenths = tenths as u32;
    Some((format!("{:>2}", tenths / 10), (tenths % 10).to_string()))
}

/// `setDecimal`'s pattern for typed rates.
fn rate_text_is_valid(text: &str) -> bool {
    let mut parts = text.splitn(2, '.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next();
    !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction
            .is_none_or(|f| (1..=5).contains(&f.len()) && f.bytes().all(|b| b.is_ascii_digit()))
}

fn rate_value(text: &str) -> f64 {
    text.trim().replace(',', "").parse::<f64>().unwrap_or(0.0)
}

/// `Sched3Item5TaxRate`: the MCIT rate text and whether the filer may edit it.
pub fn form_1702q_mcit_rate(year: u16, month: u8) -> (&'static str, bool) {
    match (year, month) {
        (2018 | 2019, _) => ("2.00", false),
        (2020, m) if m <= 6 => ("2.00", false),
        (2020, _) => ("0.00", true),
        (2021..=2023, _) => ("0.00", true),
        (2024, m) if m >= 5 => ("0.00", true),
        (y, m) if y >= 2024 && m >= 7 => ("2.00", false),
        _ => ("0.00", false),
    }
}

/// The last day of `quarter` for a year ending in `month`/`year`, as
/// `validate()` computes the quarter windows (January year-ends start the
/// return period in February).
fn quarter_end(year: u16, month: u8, quarter: u8) -> Option<NaiveDate> {
    let year = i32::from(year);
    let (start_year, start_month, lengths) = if month == 1 {
        (year - 1, 2u32, [2u32, 5, 8])
    } else {
        (year - 1, u32::from(month) + 1, [3u32, 6, 9])
    };
    let months = *lengths.get(usize::from(quarter.checked_sub(1)?))?;
    let start = NaiveDate::from_ymd_opt(start_year, 1, 1)?
        .checked_add_months(chrono::Months::new(start_month - 1))?;
    start
        .checked_add_months(chrono::Months::new(months))?
        .pred_opt()
}

impl Form1702qDraft {
    pub const FORM_CODE: &'static str = "1702Q";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            tin: profile.tin.full(),
            tax_period_basis: TaxPeriodBasis::Calendar,
            year_end_month: 12,
            taxable_year: year,
            quarter,
            is_amended: false,
            atc_mcit: false,
            atc: String::new(),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            line_of_business: profile.line_of_business.clone(),
            deduction: Form1702qDeduction::Unanswered,
            tax_relief: false,
            tax_relief_specify: String::new(),
            exempt: Form1702qSchedule1Column::default(),
            special: Form1702qSchedule1Column::default(),
            share_of_other_agencies: 0.0,
            special_net_tax_due: 0.0,
            sales: 0.0,
            cost_of_sales: 0.0,
            gross_income: 0.0,
            other_income: 0.0,
            total_gross_income: 0.0,
            deductions: 0.0,
            taxable_income: 0.0,
            previous_quarters_taxable_income: 0.0,
            total_taxable_income: 0.0,
            regular_rate_text: default_rate_text(),
            regular_tax_due: 0.0,
            mcit: 0.0,
            income_tax_due: 0.0,
            mcit_gross_income_q1: 0.0,
            mcit_gross_income_q2: 0.0,
            mcit_gross_income_q3: 0.0,
            mcit_total_gross_income: 0.0,
            mcit_rate_text: default_rate_text(),
            prior_year_excess_credits: 0.0,
            previous_quarters_payments: 0.0,
            previous_quarters_mcit_payments: 0.0,
            previous_quarters_cwt: 0.0,
            cwt_this_quarter: 0.0,
            previously_filed: 0.0,
            other_credits: Vec::new(),
            total_credits: 0.0,
            prior_year_mcit_excess: 0.0,
            regular_tax_still_due: 0.0,
            aggregate_tax_due: 0.0,
            net_tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            number_of_attachments: 0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 1: calendar years end in December (`checkFilingYear`).
    pub fn set_tax_period_basis(&mut self, basis: TaxPeriodBasis) {
        self.tax_period_basis = basis;
        if basis == TaxPeriodBasis::Calendar {
            self.year_end_month = 12;
        }
    }

    pub fn atc_option(&self) -> Option<&'static Form1702qAtcOption> {
        form_1702q_atc_option(&self.atc)
    }

    /// `enableOptionalDeduction`: OSD is closed for ATCs other than the
    /// 30/25/20% ones and IC021.
    pub fn osd_allowed(&self) -> bool {
        match self.atc_option() {
            None => true,
            Some(option) => {
                option.value == "IC021_Exempt"
                    || ["30%", "25%", "20%"]
                        .iter()
                        .any(|rate| option.value.contains(rate))
            }
        }
    }

    /// Schedule 3 is open only with both Item 5 boxes ticked.
    pub fn mcit_open(&self) -> bool {
        self.atc_mcit && self.atc_option().is_some()
    }

    /// The Schedule 2 Item 10 text `Sched2Item10TaxRate` leaves, or `None`
    /// when the filer types it.
    pub fn forced_regular_rate(&self) -> Option<&'static str> {
        let option = self.atc_option()?;
        if option.group != Form1702qAtcGroup::Regular {
            return None;
        }
        // Official precedence: `year == 2020 && month == 7 || month == 8`.
        if (self.taxable_year == 2020 && self.year_end_month == 7) || self.year_end_month == 8 {
            return None;
        }
        Some(option.rate_text)
    }

    /// The Schedule 3 Item 5 text, forced or typed.
    pub fn mcit_rate_display(&self) -> String {
        if !self.mcit_open() {
            return "0.0000".to_string();
        }
        match form_1702q_mcit_rate(self.taxable_year, self.year_end_month) {
            (text, false) => text.to_string(),
            (_, true) => self.mcit_rate_text.trim().to_string(),
        }
    }

    /// The Schedule 2 Item 10 text.
    pub fn regular_rate_display(&self) -> String {
        match self.forced_regular_rate() {
            Some(text) => text.to_string(),
            None => self.regular_rate_text.trim().to_string(),
        }
    }

    /// Item 15 is open only while the regular tax exceeds the MCIT.
    pub fn prior_mcit_open(&self) -> bool {
        self.regular_tax_due > self.mcit
    }

    /// Schedule 4 Item 6 only on an amended return.
    pub fn previously_filed_open(&self) -> bool {
        self.is_amended
    }

    /// Items 8 and Schedule 4 Items 2–4 only for the 2nd and 3rd quarters.
    pub fn previous_quarters_open(&self) -> bool {
        self.quarter >= 2
    }

    /// The official compute chain, after the radio and ATC rules.
    pub fn recompute(&mut self) {
        if self.tax_period_basis == TaxPeriodBasis::Calendar {
            self.year_end_month = 12;
        }
        if !self.osd_allowed() && self.atc_option().is_some() {
            self.deduction = Form1702qDeduction::Itemized;
        }
        if !self.tax_relief {
            self.tax_relief_specify.clear();
        }
        let previous = self.previous_quarters_open();
        for value in [
            &mut self.exempt.sales,
            &mut self.exempt.cost_of_sales,
            &mut self.exempt.other_income,
            &mut self.exempt.deductions,
            &mut self.exempt.previous_quarters,
            &mut self.special.sales,
            &mut self.special.cost_of_sales,
            &mut self.special.other_income,
            &mut self.special.deductions,
            &mut self.special.previous_quarters,
            &mut self.share_of_other_agencies,
            &mut self.sales,
            &mut self.cost_of_sales,
            &mut self.other_income,
            &mut self.deductions,
            &mut self.previous_quarters_taxable_income,
            &mut self.mcit_gross_income_q1,
            &mut self.mcit_gross_income_q2,
            &mut self.mcit_gross_income_q3,
            &mut self.prior_year_excess_credits,
            &mut self.previous_quarters_payments,
            &mut self.previous_quarters_mcit_payments,
            &mut self.previous_quarters_cwt,
            &mut self.cwt_this_quarter,
            &mut self.previously_filed,
            &mut self.prior_year_mcit_excess,
            &mut self.surcharge,
            &mut self.interest,
            &mut self.compromise,
        ] {
            *value = js_round(*value);
        }
        for row in &mut self.other_credits {
            row.amount = js_round(row.amount);
        }
        self.other_credits.truncate(FORM_1702Q_OTHER_CREDIT_ROWS);
        while self
            .other_credits
            .last()
            .is_some_and(|row| row.description.is_empty() && row.amount == 0.0)
        {
            self.other_credits.pop();
        }

        // ATCEnableDisableSchedules: no ATC closes Schedules 1, 2 and 4.
        if self.atc_option().is_none() {
            self.exempt = Form1702qSchedule1Column::default();
            self.special = Form1702qSchedule1Column::default();
            self.share_of_other_agencies = 0.0;
            self.sales = 0.0;
            self.cost_of_sales = 0.0;
            self.other_income = 0.0;
            self.deductions = 0.0;
            self.previous_quarters_taxable_income = 0.0;
            self.prior_year_excess_credits = 0.0;
            self.previous_quarters_payments = 0.0;
            self.previous_quarters_mcit_payments = 0.0;
            self.previous_quarters_cwt = 0.0;
            self.cwt_this_quarter = 0.0;
            self.previously_filed = 0.0;
            self.other_credits.clear();
        }
        if !previous {
            self.exempt.previous_quarters = 0.0;
            self.special.previous_quarters = 0.0;
            self.previous_quarters_taxable_income = 0.0;
            self.previous_quarters_payments = 0.0;
            self.previous_quarters_mcit_payments = 0.0;
            self.previous_quarters_cwt = 0.0;
        }
        if !self.previously_filed_open() {
            self.previously_filed = 0.0;
        }
        if !self.mcit_open() {
            self.mcit_gross_income_q1 = 0.0;
            self.mcit_gross_income_q2 = 0.0;
            self.mcit_gross_income_q3 = 0.0;
        }
        if self.quarter < 2 {
            self.mcit_gross_income_q2 = 0.0;
        }
        if self.quarter < 3 {
            self.mcit_gross_income_q3 = 0.0;
        }

        // Schedule 1 (column A's rate field is disabled at 0.00).
        self.exempt.rate = 0.0;
        self.special.rate = rate_value(&official_amount(self.special.rate));
        for column in [&mut self.exempt, &mut self.special] {
            column.gross_income = column.sales - column.cost_of_sales;
            column.total_gross_income = column.other_income + column.gross_income;
            column.taxable_income = column.total_gross_income - column.deductions;
            column.total_taxable_income = column.taxable_income + column.previous_quarters;
        }
        self.exempt.tax_due = js_round(self.exempt.total_taxable_income * self.exempt.rate / 100.0);
        self.special.tax_due = if self.special.total_taxable_income <= 0.0 {
            0.0
        } else {
            js_round(self.special.total_taxable_income * self.special.rate / 100.0)
        };
        self.special_net_tax_due =
            js_round(self.exempt.tax_due + self.special.tax_due - self.share_of_other_agencies);

        // Schedule 3.
        self.mcit_total_gross_income =
            self.mcit_gross_income_q1 + self.mcit_gross_income_q2 + self.mcit_gross_income_q3;
        let mcit_rate = rate_value(&self.mcit_rate_display());
        let mcit = js_round(cents(self.mcit_total_gross_income * mcit_rate / 100.0));
        self.mcit = if mcit < 0.0 { 0.0 } else { mcit };

        // Schedule 2.
        self.gross_income = self.sales - self.cost_of_sales;
        self.total_gross_income = self.gross_income + self.other_income;
        if self.deduction == Form1702qDeduction::Osd {
            self.deductions = js_round(cents(self.total_gross_income * 0.40));
        }
        self.taxable_income = self.total_gross_income - self.deductions;
        self.total_taxable_income = self.taxable_income + self.previous_quarters_taxable_income;
        let rate = rate_value(&self.regular_rate_display());
        self.regular_tax_due = if self.total_taxable_income < 0.0 {
            0.0
        } else {
            js_round(cents(self.total_taxable_income * rate / 100.0))
        };
        self.income_tax_due = js_round(self.regular_tax_due.max(self.mcit));
        if !self.prior_mcit_open() {
            self.prior_year_mcit_excess = 0.0;
        }

        // Schedule 4.
        self.total_credits = self.prior_year_excess_credits
            + self.previous_quarters_payments
            + self.previous_quarters_mcit_payments
            + self.previous_quarters_cwt
            + self.cwt_this_quarter
            + self.previously_filed
            + self.other_credits.iter().map(|row| row.amount).sum::<f64>();

        // Part II.
        self.regular_tax_still_due = self.income_tax_due - self.prior_year_mcit_excess;
        self.aggregate_tax_due = self.regular_tax_still_due + self.special_net_tax_due;
        self.net_tax_payable = self.aggregate_tax_due - self.total_credits;
        // The penalty items file their override, else the computed value,
        // held in whole pesos like every typed amount (`removeDecimal`).
        Self::penalty_declaration().apply(self);
        self.surcharge = js_round(self.surcharge);
        self.interest = js_round(self.interest);
        self.compromise = js_round(self.compromise);
        self.total_penalties = self.surcharge + self.interest + self.compromise;
        self.total_amount_payable = if self.net_tax_payable < 0.0 && self.total_penalties > 0.0 {
            self.total_penalties
        } else {
            self.net_tax_payable + self.total_penalties
        };
    }

    /// Schedule 1 Item 13B as the page holds it: `netIncome()` (run when
    /// Item 12B is left) writes the bare number, floored at zero; otherwise
    /// it is the `Sched1_Computation` amount.
    fn special_net_tax_due_text(&self) -> String {
        if self.share_of_other_agencies != 0.0 {
            let net = self.exempt.tax_due + self.special.tax_due - self.share_of_other_agencies;
            let net = if net < 0.0 { 0.0 } else { net };
            format!("{net}")
        } else {
            official_amount(self.special_net_tax_due)
        }
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1702q:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let money = official_amount;
        // Profile values arrive in capitals (loadBGData from the profile).
        let upper = |value: &str| value.trim().to_uppercase();

        let calendar = self.tax_period_basis == TaxPeriodBasis::Calendar;
        put("rbForClndrFscl_1", flag(calendar));
        put("rbForClndrFscl_2", flag(!calendar));
        put("rbYrEndMonth", format!("{:02}", self.year_end_month));
        put("txtYrEndYear", format!("{:02}", self.taxable_year % 100));
        for quarter in 1..=3u8 {
            put(
                &format!("rbQuarter_{quarter}"),
                flag(self.quarter == quarter),
            );
        }
        put("rbAmendedRtn_1", flag(self.is_amended));
        put("rbAmendedRtn_2", flag(!self.is_amended));
        put("rbATC_1", flag(self.atc_mcit));
        put("cbATC_2", self.atc.clone());
        put("rbATC_2", flag(self.atc_option().is_some()));

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        for prefix in ["txt", "txtPg2"] {
            put(&format!("{prefix}TIN1"), tin1.clone());
            put(&format!("{prefix}TIN2"), tin2.clone());
            put(&format!("{prefix}TIN3"), tin3.clone());
        }
        put("txtBranchCode", branch.clone());
        put("txtPg2BranchCode", branch);
        put("txtRDOCode", self.rdo_code.trim().to_string());
        put("txtTaxpayerName1", upper(&self.taxpayer_name));
        put("txtPg2TaxpayerName", upper(&self.taxpayer_name));
        put("txtAddress", upper(&self.registered_address));
        put("txtZipCode", self.zip_code.trim().to_string());
        // Both txtTelNum controls (Item 10 and the hidden profile copy).
        put("txtTelNum", self.contact_number.trim().to_string());
        put("txtLOB", upper(&self.line_of_business));
        // saveEncryptedProfile writes the pager as the constant "1"
        // ("set page to 1"), whatever page the filer is on.
        put("txtCurrentPage", "1".to_string());
        put(
            "rbMthdOfDdctns_1",
            flag(self.deduction == Form1702qDeduction::Itemized),
        );
        put(
            "rbMthdOfDdctns_2",
            flag(self.deduction == Form1702qDeduction::Osd),
        );
        put("rbTxRlf_1", flag(self.tax_relief));
        put("rbTxRlf_2", flag(!self.tax_relief));
        put("txtTxRlfSpcfy", self.tax_relief_specify.clone());

        put("txtTax14", money(self.income_tax_due));
        put("txtTax15", money(self.prior_year_mcit_excess));
        put("txtTax16", money(self.regular_tax_still_due));
        put("txtTax17", money(self.special_net_tax_due));
        put("txtTax18", money(self.aggregate_tax_due));
        put("txtTax19", money(self.total_credits));
        put("txtTax20", money(self.net_tax_payable));
        put("txtTax21", money(self.surcharge));
        put("txtTax22", money(self.interest));
        put("txtTax23", money(self.compromise));
        put("txtTax24", money(self.total_penalties));
        put("txtTax25", money(self.total_amount_payable));
        put("txtSheets", self.number_of_attachments.to_string());

        for (suffix, column) in [("A", &self.exempt), ("B", &self.special)] {
            let mut item = |n: u8, value: f64| {
                put(&format!("Sched1:txtTax{n}{suffix}"), money(value));
            };
            item(1, column.sales);
            item(2, column.cost_of_sales);
            item(3, column.gross_income);
            item(4, column.other_income);
            item(5, column.total_gross_income);
            item(6, column.deductions);
            item(7, column.taxable_income);
            item(8, column.previous_quarters);
            item(9, column.total_taxable_income);
            item(10, column.rate);
            item(11, column.tax_due);
        }
        put("Sched1:txtTax12B", money(self.share_of_other_agencies));
        put("Sched1:txtTax13B", self.special_net_tax_due_text());

        let mut sched2 = |n: u8, value: String| put(&format!("Sched2:txtTax{n}"), value);
        sched2(1, money(self.sales));
        sched2(2, money(self.cost_of_sales));
        sched2(3, money(self.gross_income));
        sched2(4, money(self.other_income));
        sched2(5, money(self.total_gross_income));
        sched2(6, money(self.deductions));
        sched2(7, money(self.taxable_income));
        sched2(8, money(self.previous_quarters_taxable_income));
        sched2(9, money(self.total_taxable_income));
        sched2(10, self.regular_rate_display());
        sched2(11, money(self.regular_tax_due));
        sched2(12, money(self.mcit));
        sched2(13, money(self.income_tax_due));

        put("Sched3:txtTax1", money(self.mcit_gross_income_q1));
        put("Sched3:txtTax2", money(self.mcit_gross_income_q2));
        put("Sched3:txtTax3", money(self.mcit_gross_income_q3));
        put("Sched3:txtTax4", money(self.mcit_total_gross_income));
        put("Sched3:txtTax5", self.mcit_rate_display());
        put("Sched3:txtTax6", money(self.mcit));

        put("Sched4:txtTax1", money(self.prior_year_excess_credits));
        put("Sched4:txtTax2", money(self.previous_quarters_payments));
        put(
            "Sched4:txtTax3",
            money(self.previous_quarters_mcit_payments),
        );
        put("Sched4:txtTax4", money(self.previous_quarters_cwt));
        put("Sched4:txtTax5", money(self.cwt_this_quarter));
        put("Sched4:txtTax6", money(self.previously_filed));
        for index in 0..FORM_1702Q_OTHER_CREDIT_ROWS {
            let row = self.other_credits.get(index).cloned().unwrap_or_default();
            put(&format!("Sched4:chkOthrTxCrdts{index}"), flag(false));
            put(&format!("Sched4:txtOthrTxCrdts{index}"), row.description);
            put(
                &format!("Sched4:txtOthrTxCrdtAmnt{index}"),
                money(row.amount),
            );
        }
        put("Sched4:txtTax7", money(self.total_credits));

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        // This page never calls getDrives(), so its export drive select has
        // no options and submits an empty value.
        fields.insert("driveSelectTPExport".to_string(), String::new());
        fields
    }

    /// The field map plus print-only values the frozen January 2018 sheet
    /// needs (`derived:` keys, never submitted): Item 2 digit boxes, the
    /// Item 5 ATC code and description, name/address lines, the page 2 TIN
    /// digits,
    /// amounts right-aligned in their 12-slot combs, and rate boxes.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let mut derived = BTreeMap::new();
        let value = |key: &str| {
            fields
                .get(&format!("frm1702q:{key}"))
                .cloned()
                .unwrap_or_default()
        };

        let month = value("rbYrEndMonth");
        let year = value("txtYrEndYear");
        for (name, text) in [("year_end_mm", &month), ("year_end_yy", &year)] {
            for (index, ch) in text.chars().take(2).enumerate() {
                derived.insert(format!("{name}{}", index + 1), ch.to_string());
            }
        }
        if let Some(option) = self.atc_option() {
            let (code, description) = print_atc_parts(option.label);
            derived.insert("atc_code".to_string(), code);
            derived.insert("atc_description".to_string(), description);
        }
        let (name1, name2) = print_lines(&value("txtTaxpayerName1"), 38);
        derived.insert("name_line1".to_string(), name1);
        derived.insert("name_line2".to_string(), name2);
        let (address1, address2) = print_lines(&value("txtAddress"), 38);
        derived.insert("address_line1".to_string(), address1);
        derived.insert("address_line2".to_string(), address2);
        // Page 2's TIN comb preprints the 00000 branch; only nine digits fill.
        derived.insert("tin_digits".to_string(), {
            let (a, b, c, _) = split_tin(&self.tin);
            format!("{a}{b}{c}")
        });

        let mut amount_keys: Vec<String> = (14..=25).map(|n| format!("txtTax{n}")).collect();
        for n in (1..=9).chain([11]) {
            amount_keys.push(format!("Sched1:txtTax{n}A"));
            amount_keys.push(format!("Sched1:txtTax{n}B"));
        }
        amount_keys.push("Sched1:txtTax12B".to_string());
        amount_keys.push("Sched1:txtTax13B".to_string());
        for n in (1..=9).chain([11, 12, 13]) {
            amount_keys.push(format!("Sched2:txtTax{n}"));
        }
        for n in [1, 2, 3, 4, 6] {
            amount_keys.push(format!("Sched3:txtTax{n}"));
        }
        for n in 1..=7 {
            amount_keys.push(format!("Sched4:txtTax{n}"));
        }
        for index in 0..FORM_1702Q_OTHER_CREDIT_ROWS {
            amount_keys.push(format!("Sched4:txtOthrTxCrdtAmnt{index}"));
        }
        for key in amount_keys {
            if let Some(text) = print_amount(&value(&key), 12) {
                derived.insert(key, text);
            }
        }

        if let Some(rate) = print_whole_rate(&value("Sched1:txtTax10A")) {
            derived.insert("sched1_rate_a".to_string(), rate);
        }
        if let Some((whole, tenths)) = print_tenths_rate(&value("Sched1:txtTax10B")) {
            derived.insert("sched1_rate_b_whole".to_string(), whole);
            derived.insert("sched1_rate_b_tenths".to_string(), tenths);
        }
        if let Some(rate) = print_whole_rate(&value("Sched2:txtTax10")) {
            derived.insert("sched2_rate".to_string(), rate);
        }
        if self.mcit_open()
            && let Some(rate) = print_whole_rate(&value("Sched3:txtTax5"))
        {
            derived.insert("sched3_rate".to_string(), rate);
        }

        for (key, text) in derived {
            fields.insert(format!("derived:{key}"), text);
        }
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    fn validate_on(&self, today: NaiveDate) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let year = self.taxable_year;
        let current_year = today.year();

        // validate(), in order.
        if !(2000..=2099).contains(&year) {
            err(
                "taxable_year",
                "Invalid year entered. Please provide a valid year.",
            );
        }
        if !(1..=12).contains(&self.year_end_month) {
            err(
                "year_end_month",
                "Please provide a valid fiscal year-end month.",
            );
        }
        if !(1..=3).contains(&self.quarter) {
            err("quarter", "Please select Quarter in Item 3.");
        }
        if year < 2018 {
            err(
                "taxable_year",
                "Year (Page 1 Item 2) should not be earlier than January 2018.",
            );
        } else if self.tax_period_basis == TaxPeriodBasis::Calendar
            && i32::from(year) > current_year
        {
            err(
                "taxable_year",
                "Year (Page 1 Item 2) cannot be greater than the current year for Calendar Year.",
            );
        }
        // Official: only fiscal returns check the quarter end; a quarter that
        // has not ended cannot be filed on either basis.
        if (1..=12).contains(&self.year_end_month)
            && let Some(end) = quarter_end(year, self.year_end_month, self.quarter)
            && end > today
        {
            err("quarter", "Future filing is not allowed.");
        }
        if self.tax_period_basis == TaxPeriodBasis::Fiscal {
            if self.year_end_month == 12 {
                // FiscalMonth() clears December on a fiscal return.
                err("year_end_month", "Please enter a valid Date on Item 2.");
            }
            // validateFiscalMonth (legacy getYear() returns the full year).
            if i32::from(year) > current_year && u32::from(self.year_end_month) >= today.month() {
                err(
                    "year_end_month",
                    "Date (Page 1 Item 2) cannot be greater than current date when filing for Fiscal Year.",
                );
            }
        }
        if !self.atc_mcit && self.atc.is_empty() {
            err("atc", "Please select ATC on Item 5.");
        } else if self.atc_option().is_none() {
            // Official: an MCIT-only return passes, but every schedule except
            // Schedule 3 stays closed and Schedule 3 needs both boxes.
            err("atc", "Please select ATC on Item 5");
        }

        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        if tin1.len() != 3
            || tin2.len() != 3
            || tin3.len() != 3
            || digits(&self.tin).len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 6.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 7.");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 8.",
            );
        }
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > 130 {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 9.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !zip.bytes().all(|b| b.is_ascii_digit()) {
            err("zip_code", "Please enter Zip Code on Item 9A.");
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 20 || !phone.bytes().all(|b| b.is_ascii_digit()) {
            err("contact_number", "Please enter Contact Number on Item 10.");
        }
        if self.deduction == Form1702qDeduction::Unanswered {
            err(
                "deduction",
                "Please select Method of Deductions on Item 12.",
            );
        }
        if self.tax_relief && self.tax_relief_specify.trim().is_empty() {
            err(
                "tax_relief_specify",
                "Please specify Special Law/International Tax Treaty on Item 13A.",
            );
        }
        for (index, row) in self.other_credits.iter().enumerate() {
            if !row.description.is_empty() && row.amount == 0.0 {
                err(
                    &format!("other_credits[{index}].amount"),
                    "Please input Other Tax Credits/Payments amount on Schedule 4 Item 6",
                );
            }
        }
        for (index, row) in self.other_credits.iter().enumerate() {
            if row.description.is_empty() && row.amount != 0.0 {
                err(
                    &format!("other_credits[{index}].description"),
                    "Please specify Other Tax Credits/Payments on Schedule 4 Item 6",
                );
            }
        }

        // Limits the page enforces while typing.
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }
        if self.line_of_business.trim().chars().count() > 50 {
            err(
                "line_of_business",
                "Line of business holds at most 50 characters.",
            );
        }
        if self.tax_relief_specify.chars().count() > 100 {
            err(
                "tax_relief_specify",
                "Item 13A holds at most 100 characters.",
            );
        }
        if self.number_of_attachments > 99 {
            err("number_of_attachments", "Item 26 holds at most two digits.");
        }
        if self.forced_regular_rate().is_none()
            && !rate_text_is_valid(self.regular_rate_text.trim())
        {
            err(
                "regular_rate_text",
                "Decimal point must be less than 5 places only",
            );
        }
        if !(0.0..100.0).contains(&self.special.rate) {
            err(
                "special.rate",
                "Enter the Schedule 1 Item 10B rate in percent (0 to below 100).",
            );
        }
        if self.mcit_open()
            && form_1702q_mcit_rate(self.taxable_year, self.year_end_month).1
            && !rate_text_is_valid(self.mcit_rate_text.trim())
        {
            err(
                "mcit_rate_text",
                "Enter the Schedule 3 Item 5 MCIT rate in percent.",
            );
        }
        if self.share_of_other_agencies > self.exempt.tax_due + self.special.tax_due {
            // netIncome() floors Item 13B at zero while Item 17 keeps the
            // negative value; keep the two consistent.
            err(
                "share_of_other_agencies",
                "Schedule 1 Item 12B cannot exceed the income tax due in Item 11.",
            );
        }
        for row in &self.other_credits {
            if row.description.chars().count() > 25 {
                err(
                    "other_credits",
                    "Other Tax Credits/Payments descriptions hold at most 25 characters.",
                );
            }
        }
        let non_negative = [
            ("exempt.sales", self.exempt.sales),
            ("exempt.cost_of_sales", self.exempt.cost_of_sales),
            ("exempt.other_income", self.exempt.other_income),
            ("exempt.deductions", self.exempt.deductions),
            ("special.sales", self.special.sales),
            ("special.cost_of_sales", self.special.cost_of_sales),
            ("special.other_income", self.special.other_income),
            ("special.deductions", self.special.deductions),
            ("share_of_other_agencies", self.share_of_other_agencies),
            ("sales", self.sales),
            ("cost_of_sales", self.cost_of_sales),
            ("other_income", self.other_income),
            ("deductions", self.deductions),
            (
                "previous_quarters_taxable_income",
                self.previous_quarters_taxable_income,
            ),
            ("prior_year_excess_credits", self.prior_year_excess_credits),
            (
                "previous_quarters_payments",
                self.previous_quarters_payments,
            ),
            (
                "previous_quarters_mcit_payments",
                self.previous_quarters_mcit_payments,
            ),
            ("previous_quarters_cwt", self.previous_quarters_cwt),
            ("cwt_this_quarter", self.cwt_this_quarter),
            ("previously_filed", self.previously_filed),
            ("prior_year_mcit_excess", self.prior_year_mcit_excess),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ];
        for (field, value) in non_negative {
            // numbersonly() lets no minus sign through.
            if value < 0.0 {
                err(field, "Enter a non-negative amount.");
            }
        }

        // Derived items must be what the official compute chain produces.
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

impl FormValidator for Form1702qDraft {
    /// `validate()` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1702qDraft {
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
                "frm1702q:txtTax21",
                "frm1702q:txtTax22",
                "frm1702q:txtTax23",
            ],
            state: penalty,
            basic_tax_due: "Item 20", |d| d.net_tax_payable,
            // Every amount on this page is whole pesos.
            rounding: super::penalty_items::PenaltyRounding::WholePesos,
            // The calendar's 1702Q deadlines follow calendar quarters; a
            // fiscal-year filer's quarter has none there.
            deadline_period: |d: &Self| {
                (d.tax_period_basis == super::form_2551q::TaxPeriodBasis::Calendar
                    && d.year_end_month == 12)
                    .then(|| super::penalty_items::calendar_period(d))
                    .flatten()
            },
        )
    }
}

impl QueueableForm for Form1702qDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1702Q";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1702Qv2018C']`).
    const FORM_TYPE: &'static str = "1702Qv2018C";
    const LAYOUT_ID: &'static str = FORM_1702Q_FORM_ID;

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
        self.taxable_year
    }
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Quarterly(self.quarter)
    }
    /// `(2000 + txtYrEndYear) + "Q" + n`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{}Q{}", self.taxable_year, self.quarter)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 6 || code.get(4..5)? != "Q" {
            return None;
        }
        let year: u16 = code.get(..4)?.parse().ok()?;
        let quarter: u8 = code.get(5..)?.parse().ok()?;
        (1..=3)
            .contains(&quarter)
            .then_some((year, FilingPeriod::Quarterly(quarter)))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()
    }

    pub(crate) fn sample() -> Form1702qDraft {
        let mut d = Form1702qDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            tin: "12345678800000".to_string(),
            tax_period_basis: TaxPeriodBasis::Calendar,
            year_end_month: 12,
            taxable_year: 2025,
            quarter: 2,
            is_amended: false,
            atc_mcit: true,
            atc: "IC010_25%".to_string(),
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Dummy Corporation".to_string(),
            registered_address: "123 Sample Street, Barangay Example, Quezon City".to_string(),
            zip_code: "1100".to_string(),
            contact_number: "09170000000".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            line_of_business: "Sample Consulting Services".to_string(),
            deduction: Form1702qDeduction::Itemized,
            tax_relief: false,
            tax_relief_specify: String::new(),
            exempt: Form1702qSchedule1Column::default(),
            special: Form1702qSchedule1Column {
                sales: 800_000.5,
                cost_of_sales: 300_000.0,
                other_income: 20_000.0,
                deductions: 100_000.0,
                previous_quarters: 150_000.0,
                rate: 10.0,
                ..Form1702qSchedule1Column::default()
            },
            share_of_other_agencies: 0.0,
            special_net_tax_due: 0.0,
            sales: 5_000_000.5,
            cost_of_sales: 1_200_000.0,
            gross_income: 0.0,
            other_income: 100_000.49,
            total_gross_income: 0.0,
            deductions: 900_000.0,
            taxable_income: 0.0,
            previous_quarters_taxable_income: 500_000.0,
            total_taxable_income: 0.0,
            regular_rate_text: "0.00".to_string(),
            regular_tax_due: 0.0,
            mcit: 0.0,
            income_tax_due: 0.0,
            mcit_gross_income_q1: 3_000_000.0,
            mcit_gross_income_q2: 3_900_001.0,
            mcit_gross_income_q3: 0.0,
            mcit_total_gross_income: 0.0,
            mcit_rate_text: "0.00".to_string(),
            prior_year_excess_credits: 10_000.0,
            previous_quarters_payments: 150_000.0,
            previous_quarters_mcit_payments: 20_000.0,
            previous_quarters_cwt: 5_000.0,
            cwt_this_quarter: 7_500.5,
            previously_filed: 0.0,
            other_credits: vec![Form1702qOtherCredit {
                description: "Sample credit".to_string(),
                amount: 1_234.0,
            }],
            total_credits: 0.0,
            prior_year_mcit_excess: 0.0,
            regular_tax_still_due: 0.0,
            aggregate_tax_due: 0.0,
            net_tax_payable: 0.0,
            surcharge: 1_000.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            number_of_attachments: 0,
            lifecycle: SubmissionLifecycle::default(),
        };
        // This sample enters its penalties by hand.
        d.keep_entered_penalties();
        d.recompute();
        d
    }

    fn messages(d: &Form1702qDraft) -> Vec<String> {
        d.validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js
        // with the legacy-engine toLocaleString).
        let d = sample();
        assert_eq!(d.special.gross_income, 500_001.0);
        assert_eq!(d.special.total_taxable_income, 570_001.0);
        assert_eq!(d.special.tax_due, 57_000.0);
        assert_eq!(d.special_net_tax_due, 57_000.0);
        assert_eq!(d.total_taxable_income, 3_500_001.0);
        assert_eq!(d.regular_rate_display(), "25.00");
        assert_eq!(d.regular_tax_due, 875_000.0);
        assert_eq!(d.mcit_total_gross_income, 6_900_001.0);
        assert_eq!(d.mcit_rate_display(), "2.00");
        assert_eq!(d.mcit, 138_000.0);
        assert_eq!(d.income_tax_due, 875_000.0);
        assert_eq!(d.aggregate_tax_due, 932_000.0);
        assert_eq!(d.cwt_this_quarter, 7_501.0);
        assert_eq!(d.total_credits, 193_735.0);
        assert_eq!(d.net_tax_payable, 738_265.0);
        assert_eq!(d.total_amount_payable, 739_265.0);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let d = sample();
        let f = d.to_bir_field_map();
        assert_eq!(f["frm1702q:txtYrEndYear"], "25");
        assert_eq!(f["frm1702q:rbYrEndMonth"], "12");
        assert_eq!(f["frm1702q:cbATC_2"], "IC010_25%");
        assert_eq!(f["frm1702q:Sched1:txtTax10A"], "0.00");
        assert_eq!(f["frm1702q:Sched1:txtTax10B"], "10.00");
        assert_eq!(f["frm1702q:Sched1:txtTax13B"], "57,000.00");
        assert_eq!(f["frm1702q:Sched2:txtTax10"], "25.00");
        assert_eq!(f["frm1702q:Sched3:txtTax5"], "2.00");
        assert_eq!(f["frm1702q:txtTaxpayerName1"], "SAMPLE DUMMY CORPORATION");
        assert_eq!(f["frm1702q:Sched4:txtOthrTxCrdts0"], "Sample credit");
        assert_eq!(f["txtEmail"], "sample.taxpayer@example.com");
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1702Qv2018C-2025Q2#sample.taxpayer@example.com#.xml"
        );
        assert!(d.to_bir_xml_payload().is_ok());

        let mut shared = d.clone();
        shared.share_of_other_agencies = 7_000.0;
        shared.recompute();
        let f = shared.to_bir_field_map();
        assert_eq!(f["frm1702q:Sched1:txtTax13B"], "50000");
        assert_eq!(f["frm1702q:txtTax17"], "50,000.00");
    }

    #[test]
    fn atc_rules() {
        let mut d = sample();
        d.atc = "IC030_10%".into();
        d.deduction = Form1702qDeduction::Osd;
        d.regular_rate_text = "10".into();
        d.recompute();
        // enableOptionalDeduction forces Itemized for special-rate ATCs.
        assert_eq!(d.deduction, Form1702qDeduction::Itemized);
        assert_eq!(d.regular_rate_display(), "10");
        assert_eq!(d.regular_tax_due, 350_000.0);
        d.atc = "IC010_30%".into();
        d.deduction = Form1702qDeduction::Osd;
        d.recompute();
        assert_eq!(d.deductions, 1_560_000.0);
        assert_eq!(d.regular_rate_display(), "30.00");
        // Fiscal years ending in August leave the rate to the filer.
        d.set_tax_period_basis(TaxPeriodBasis::Fiscal);
        d.year_end_month = 8;
        d.recompute();
        assert_eq!(d.forced_regular_rate(), None);
        assert_eq!(form_1702q_mcit_rate(2025, 6), ("0.00", false));
        assert_eq!(form_1702q_mcit_rate(2022, 12), ("0.00", true));
        assert_eq!(form_1702q_mcit_rate(2019, 12), ("2.00", false));
    }

    #[test]
    fn period_codes_round_trip() {
        let d = sample();
        assert_eq!(d.period_code(), "2025Q2");
        assert_eq!(
            Form1702qDraft::parse_period_code("2025Q2"),
            Some((2025, FilingPeriod::Quarterly(2)))
        );
        assert_eq!(Form1702qDraft::parse_period_code("2025Q4"), None);
        assert_eq!(Form1702qDraft::parse_period_code("122025Q2"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1702qDraft), expected: &str| {
            let mut d = sample();
            mutate(&mut d);
            let found = messages(&d);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.taxable_year = 0,
            "Invalid year entered. Please provide a valid year.",
        );
        check(
            &|d| {
                d.set_tax_period_basis(TaxPeriodBasis::Fiscal);
                d.year_end_month = 0;
            },
            "Please provide a valid fiscal year-end month.",
        );
        check(&|d| d.quarter = 0, "Please select Quarter in Item 3.");
        check(
            &|d| d.taxable_year = 2017,
            "Year (Page 1 Item 2) should not be earlier than January 2018.",
        );
        check(
            &|d| d.taxable_year = 2027,
            "Year (Page 1 Item 2) cannot be greater than the current year for Calendar Year.",
        );
        check(
            &|d| {
                d.set_tax_period_basis(TaxPeriodBasis::Fiscal);
                d.year_end_month = 6;
                d.taxable_year = 2027;
                d.quarter = 1;
                d.recompute();
            },
            "Future filing is not allowed.",
        );
        check(
            &|d| {
                d.atc_mcit = false;
                d.atc.clear();
                d.recompute();
            },
            "Please select ATC on Item 5.",
        );
        check(
            &|d| {
                d.atc.clear();
                d.recompute();
            },
            "Please select ATC on Item 5",
        );
        check(
            &|d| d.tin = "12".into(),
            "Please enter a valid TIN number on Item 6.",
        );
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code.clear(),
            "Please enter a valid RDO Code on Item 7.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 8.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 9.",
        );
        check(&|d| d.zip_code.clear(), "Please enter Zip Code on Item 9A.");
        check(
            &|d| d.contact_number.clear(),
            "Please enter Contact Number on Item 10.",
        );
        check(
            &|d| d.deduction = Form1702qDeduction::Unanswered,
            "Please select Method of Deductions on Item 12.",
        );
        check(
            &|d| d.tax_relief = true,
            "Please specify Special Law/International Tax Treaty on Item 13A.",
        );
        check(
            &|d| {
                d.other_credits[0].amount = 0.0;
                d.recompute();
            },
            "Please input Other Tax Credits/Payments amount on Schedule 4 Item 6",
        );
        check(
            &|d| {
                d.other_credits[0].description.clear();
                d.recompute();
            },
            "Please specify Other Tax Credits/Payments on Schedule 4 Item 6",
        );
        check(
            &|d| {
                d.atc = "IC030_10%".into();
                d.regular_rate_text = "10.123456".into();
                d.recompute();
            },
            "Decimal point must be less than 5 places only",
        );
    }

    #[test]
    fn quarter_windows_follow_the_official_validate() {
        assert_eq!(
            quarter_end(2025, 12, 1),
            NaiveDate::from_ymd_opt(2025, 3, 31)
        );
        assert_eq!(
            quarter_end(2025, 6, 3),
            NaiveDate::from_ymd_opt(2025, 3, 31)
        );
        assert_eq!(
            quarter_end(2025, 1, 1),
            NaiveDate::from_ymd_opt(2024, 3, 31)
        );
        assert_eq!(
            quarter_end(2025, 1, 3),
            NaiveDate::from_ymd_opt(2024, 9, 30)
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

    #[test]
    fn print_field_map_adds_only_derived_keys() {
        let draft = sample();
        let submitted = draft.to_bir_field_map();
        let print = draft.to_print_field_map();
        for (key, value) in &submitted {
            assert_eq!(print.get(key), Some(value), "{key}");
        }
        assert!(
            print
                .keys()
                .filter(|key| !submitted.contains_key(*key))
                .all(|key| key.starts_with("derived:"))
        );
        let get = |key: &str| {
            print
                .get(&format!("derived:{key}"))
                .cloned()
                .unwrap_or_default()
        };
        assert_eq!(get("year_end_mm1"), "1");
        assert_eq!(get("year_end_mm2"), "2");
        assert_eq!(get("year_end_yy1"), "2");
        assert_eq!(get("year_end_yy2"), "5");
        assert_eq!(get("atc_code"), "IC 010");
        assert_eq!(
            get("atc_description"),
            "Domestic Corporation in General - 25%"
        );
        assert_eq!(get("name_line1"), "SAMPLE DUMMY CORPORATION");
        assert_eq!(get("name_line2"), "");
        assert_eq!(
            get("address_line1"),
            "123 SAMPLE STREET, BARANGAY EXAMPLE, Q"
        );
        assert_eq!(get("address_line2"), "UEZON CITY");
        assert_eq!(get("tin_digits"), "123456788");
        assert_eq!(get("txtTax25"), "  739,265.00");
        assert_eq!(get("Sched2:txtTax1"), "5,000,001.00");
        assert_eq!(get("Sched1:txtTax1B"), "  800,001.00");
        assert_eq!(get("sched1_rate_a"), " 0");
        assert_eq!(get("sched1_rate_b_whole"), "10");
        assert_eq!(get("sched1_rate_b_tenths"), "0");
        assert_eq!(get("sched2_rate"), "25");
        assert_eq!(get("sched3_rate"), " 2");
    }

    #[test]
    fn print_amounts_fit_the_comb_or_stay_blank() {
        assert_eq!(
            print_amount("1,234.00", 12).as_deref(),
            Some("    1,234.00")
        );
        assert_eq!(
            print_amount("12,345,678.90", 12).as_deref(),
            Some(" 12345678.90")
        );
        assert_eq!(print_amount("1,234,567,890,123.00", 12), None);
        assert_eq!(print_amount("", 12), None);
        assert_eq!(print_whole_rate("2.5"), None);
        assert_eq!(print_tenths_rate("2.5"), Some((" 2".into(), "5".into())));
        assert_eq!(print_tenths_rate("2.25"), None);
    }
}
