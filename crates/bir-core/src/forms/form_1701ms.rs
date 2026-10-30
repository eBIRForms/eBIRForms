//! BIR Form 1701MS — Annual Income Tax Return for Individuals Earning
//! Income from Business/Profession (Micro and Small taxpayers, 2024).
//!
//! Ported from the official `BIR-Form1701MS.hta` (eBIRForms 7.9.6.2.1):
//! the per-column compute chain (`calculateNo3TP` … `computeAggregateAmountNo30`,
//! Schedules A/B1/B2, Part V credits and Part VI tax relief), the radio rules
//! (`sourceOfIncome*`, `incomeIsSubjectTo*`, `marriedJointly`, `toggleLine5`,
//! `checkButtons*`), `validate()` with its exact alert texts, and
//! `saveXMLsubmit` through [`crate::official_xml`].
//!
//! The official submit loop of this form writes only the radio buttons and
//! check boxes plus the taxpayer name (`txtTaxpayerNo8a`, `escape()`d). Every
//! amount is computed and validated here exactly like the page, but none of
//! them is part of the submitted plaintext.
//!
//! The "View Details" / "(Add more...)" popups (Schedule IV 10A/10B, 13A/13B,
//! 20A/20B, Part V Item 9) sit after `</form>`, outside `frmMain`, so their
//! rows reach neither `saveXMLsubmit` nor `saveEncryptedProfile`; only the
//! totals they write back into the page do (and Item 9's description, which
//! `closeModalSched5No9` sets to `VARIOUS` for two or more rows). Holding
//! the popup totals as plain amounts is therefore exact.
//! Schedule 10A also keeps its popup rows ([`Form1701MsScheduleRow`]) for the
//! `validationSpecify10A` check; only their total is sent.
//!
//! Every amount entry on this form passes through `removeDecimal()`
//! (`Math.round`) or a `Math.round` in the compute chain, so the model holds
//! whole pesos; only the Item 31 amounts keep centavos (`round(this,2)`).

use super::penalty_items::{PenaltyItems, PenaltyState};
use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1701MS_FORM_ID: &str = "1701ms-v2024";
/// Item 22 of Schedule B2: the ₱250,000 reduction for pure business or
/// profession income under the 8% option (`creditableTaxWithheldForm2316`).
pub const FORM_1701MS_EIGHT_PERCENT_EXEMPTION: f64 = 250_000.0;
/// `checkNumberTP` / `checkNumberSP`: the VAT threshold for the 8% option.
pub const FORM_1701MS_EIGHT_PERCENT_THRESHOLD: f64 = 3_000_000.0;

const EIGHT_PERCENT_THRESHOLD_ALERT: &str = " Your Gross Sales/Receipts and Other Non-Operating Income exceeds VAT Threshold (P3M), thus, not qualified to 8% tax rate and shall be subjected to graduated rates. Please choose Graduated Income Tax Rates in Part I Item 12 and fill out Part IV.B1.";
const ITEM_26_ALERT: &str = "The amount exceeds the allowed input based on the requirements";
const ITEM_17_ALERT: &str = "Item 17 is mandatory if Item 11 is Income from Business, Mixed Income and Income from Profession and Item 12 is Graduated Income Tax Rates.";

/// Item 4 (`civilStatus` option values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1701MsCivilStatus {
    #[default]
    Single,
    Married,
    Widowed,
    Separated,
    NotApplicable,
}

impl Form1701MsCivilStatus {
    pub const ALL: [Self; 5] = [
        Self::Single,
        Self::Married,
        Self::Widowed,
        Self::Separated,
        Self::NotApplicable,
    ];

    /// The `civilStatus` option value.
    pub fn value(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Married => "Married",
            Self::Widowed => "Widowed",
            Self::Separated => "Separated",
            Self::NotApplicable => "Not Applicable",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Married => "Married",
            Self::Widowed => "Widow/er",
            Self::Separated => "Legally Separated",
            Self::NotApplicable => "Not Applicable",
        }
    }
}

/// Item 5 (`txtIfMarried*No5`), only while Item 4 is Married.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1701MsFiling {
    #[default]
    Unanswered,
    Jointly,
    Separately,
    NotApplicable,
}

/// Item 11, source of income.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1701MsSource {
    #[default]
    Unanswered,
    Business,
    Mixed,
    Profession,
    /// Spouse column only (`txtTaxpayerNo11d1`).
    Compensation,
}

impl Form1701MsSource {
    fn has_business(self) -> bool {
        matches!(self, Self::Business | Self::Mixed | Self::Profession)
    }

    fn has_compensation(self) -> bool {
        matches!(self, Self::Mixed | Self::Compensation)
    }
}

/// Item 12, income tax option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1701MsTaxOption {
    #[default]
    Unanswered,
    Graduated,
    EightPercent,
    Exempt,
    Special,
}

impl Form1701MsTaxOption {
    fn is_exempt_or_special(self) -> bool {
        matches!(self, Self::Exempt | Self::Special)
    }
}

/// Item 17, method of deduction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1701MsDeduction {
    #[default]
    Unanswered,
    Itemized,
    Osd,
}

/// Spouse identity (Items 6–10, spouse column).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1701MsSpouseInfo {
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub rdo_code: String,
    pub name: String,
    pub email: String,
    pub contact_number: String,
}

/// One row of a Schedule IV "View Details" popup (`sched410A`): a
/// description and a whole-peso amount. The rows stay outside `frmMain`, so
/// only their total (Item 10A) reaches the upload.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1701MsScheduleRow {
    pub description: String,
    pub amount: f64,
}

/// One column (taxpayer or spouse) of Part I Items 11–17, Part II, Part IV,
/// Part V and Part VI. Amounts are whole pesos.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1701MsColumn {
    // Part I
    pub source: Form1701MsSource,
    pub tax_option: Form1701MsTaxOption,
    /// Item 13 — Legal basis of tax relief/exemption.
    pub legal_basis: String,
    /// Item 14 — Investment Promotion Agency / Government Agency.
    pub promotion_agency: String,
    /// Item 15 — Registered activity/program (registration number).
    pub registered_activity: String,
    /// Item 16 — Effectivity of the relief, `MM/DD/YYYY`.
    pub effectivity_from: String,
    pub effectivity_to: String,
    pub deduction: Form1701MsDeduction,

    // Part IV Schedule A — compensation
    pub gross_compensation: f64,
    pub non_taxable_compensation: f64,
    pub taxable_compensation: f64,
    pub tax_on_compensation: f64,

    // Part IV Schedule B1 — graduated / exempt / special
    pub sales: f64,
    pub sales_returns: f64,
    pub net_sales: f64,
    pub cost_of_sales: f64,
    pub gross_income: f64,
    /// 10A — total of the itemized deduction schedule.
    pub itemized_deductions: f64,
    /// 10A "View Details" rows; when present, Item 10A is their total
    /// (`computeOtherSched410A`).
    pub itemized_rows: Vec<Form1701MsScheduleRow>,
    /// 10B — total of the special allowable itemized deduction schedule.
    pub special_allowable_deductions: f64,
    /// 10C — NOLCO.
    pub nolco: f64,
    pub total_deductions: f64,
    pub osd: f64,
    pub net_income: f64,
    /// 13 — total of the non-operating income schedule.
    pub non_operating_income: f64,
    pub taxable_business_income: f64,
    pub taxable_income: f64,
    /// 16 — special or preferential rate in percent.
    pub special_rate: f64,
    pub special_tax_due: f64,
    pub tax_due_18a: f64,
    pub tax_due_18b: f64,

    // Part IV Schedule B2 — 8% option
    pub eight_sales: f64,
    /// 20 — total of the other non-operating income schedule.
    pub eight_other_income: f64,
    pub eight_total_income: f64,
    pub eight_exemption: f64,
    pub eight_taxable_income: f64,
    pub eight_tax_due: f64,
    pub eight_total_tax_due: f64,

    // Part V — tax credits/payments
    pub prior_year_excess: f64,
    pub quarterly_payments: f64,
    pub cwt_q1_q3: f64,
    pub cwt_q4: f64,
    pub cwt_2316: f64,
    pub previously_filed: f64,
    pub foreign_tax_credits: f64,
    pub special_tax_credits: f64,
    pub other_credits: f64,
    pub total_credits: f64,

    // Part II
    pub income_tax_due: f64,
    pub share_of_other_agencies: f64,
    pub net_special_tax: f64,
    pub regular_income_tax: f64,
    pub total_income_tax_due: f64,
    pub tax_credits: f64,
    pub tax_payable: f64,
    /// 26 — portion of tax due allowed for the second installment.
    pub second_installment: f64,
    pub amount_payable: f64,
    pub surcharge: f64,
    pub interest: f64,
    pub compromise: f64,
    /// This column's penalty items' computed values, overrides, note and
    /// engine inputs (the shared model in [`super::penalty_items`]).
    #[serde(flatten)]
    pub penalty: PenaltyState,
    pub total_penalties: f64,
    pub total_amount_payable: f64,

    // Part VI
    pub tax_relief: f64,
}

/// `Math.round` for the amounts this form handles.
fn js_round(value: f64) -> f64 {
    if value.is_finite() {
        (value + 0.5).floor()
    } else {
        0.0
    }
}

/// `round(this,2)`: the value an Item 31 amount holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// `computeRegularIncomeTax`: the graduated table (TRAIN, 2023 onwards).
pub fn form_1701ms_graduated_tax(taxable_income: f64) -> f64 {
    let x = taxable_income;
    if x <= 250_000.0 {
        0.0
    } else if x <= 400_000.0 {
        0.15 * (x - 250_000.0)
    } else if x <= 800_000.0 {
        22_500.0 + 0.20 * (x - 400_000.0)
    } else if x <= 2_000_000.0 {
        102_500.0 + 0.25 * (x - 800_000.0)
    } else if x <= 8_000_000.0 {
        402_500.0 + 0.30 * (x - 2_000_000.0)
    } else {
        2_202_500.0 + 0.35 * (x - 8_000_000.0)
    }
}

/// `formatCurrencyWithComma` of a whole number, as the official alerts show it.
fn whole_with_commas(value: f64) -> String {
    let text = official_amount(value);
    text.strip_suffix(".00").map(str::to_string).unwrap_or(text)
}

fn has_whole_pesos(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

fn digits(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

/// `MM/DD/YYYY` (also accepts `M/D/YYYY`), as `new Date(...)` reads it.
fn parse_us_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%m/%d/%Y").ok()
}

/// A whole-peso amount as the page's controls hold it: `toString()` with
/// thousands commas (`formatCurrencyWithComma`, `removeDecimal`).
fn page_amount(value: f64) -> String {
    let text = if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    };
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.as_str()),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((int, frac)) => (int, Some(frac)),
        None => (rest, None),
    };
    let mut grouped = String::new();
    for (index, ch) in int.chars().enumerate() {
        if index > 0 && (int.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(frac) => format!("{sign}{grouped}.{frac}"),
        None => format!("{sign}{grouped}"),
    }
}

/// Item 16's rate control: `0.0%` until a rate is typed, then the typed
/// number with `appendPercentSymbol`'s `%`.
fn page_rate(rate: f64) -> String {
    if rate == 0.0 {
        "0.0%".to_string()
    } else {
        format!("{}%", page_amount(rate).replace(',', ""))
    }
}

/// The `select-one` controls, which `capital()` leaves alone.
const SELECT_CONTROLS: [&str; 4] = [
    "frm1701MS:txtMonthNo1",
    "frm1701MS:civilStatus",
    "taxCodeDropdown",
    "taxCodeDropdownSpouse",
];

/// Width of the printed sheet's amount combs.
const PRINT_AMOUNT_SLOTS: usize = 9;

/// Right-aligns `text` in a printed amount comb.
fn print_right_aligned(text: String) -> String {
    format!("{text:>PRINT_AMOUNT_SLOTS$}")
}

/// A whole-peso amount as the printed comb shows it: digits only, an
/// overpayment in parentheses.
fn print_whole_amount(value: f64) -> String {
    let whole = js_round(value.abs()) as u64;
    if value < 0.0 && whole != 0 {
        print_right_aligned(format!("({whole})"))
    } else {
        print_right_aligned(whole.to_string())
    }
}

/// An Item 31 amount (centavos kept), without thousands separators.
fn print_cents_amount(value: f64) -> String {
    print_right_aligned(official_amount(value.abs()).replace(',', ""))
}

/// Item 16 as the sheet prints it: whole percent in the 2-slot comb, the
/// decimals (if any) in the box after the printed point.
fn print_rate(rate: f64) -> (String, String) {
    let text = official_amount(rate).replace(',', "");
    let (whole, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let fraction = fraction.trim_end_matches('0');
    (
        whole.to_string(),
        if fraction.is_empty() { "0" } else { fraction }.to_string(),
    )
}

/// The Item 18 select's option text after the code.
pub fn form_1701ms_atc_description(atc: &str) -> &'static str {
    match atc {
        "II011" => "Compensation Income",
        "II012" => "Business Income- Graduated IT Rates",
        "II013" => "Mixed Income- Graduated IT Rates",
        "II014" => "Income from Profession- Graduated IT Rates",
        "II015" => "Business Income- 8% IT Rate",
        "II016" => "Mixed Income-8% IT Rate",
        "II017" => "Income from Profession - 8% IT Rate",
        _ => "",
    }
}

impl Form1701MsColumn {
    /// Every amount the printed sheet shows for this column, keyed by the
    /// print item (`p2_*` Part II, `p4_*` Part IV, `p5_*` Part V, `p6_*`
    /// Part VI).
    fn print_amounts(&self) -> Vec<(&'static str, f64)> {
        vec![
            ("p2_19", self.income_tax_due),
            ("p2_20", self.share_of_other_agencies),
            ("p2_21", self.net_special_tax),
            ("p2_22", self.regular_income_tax),
            ("p2_23", self.total_income_tax_due),
            ("p2_24", self.tax_credits),
            ("p2_25", self.tax_payable),
            ("p2_26", self.second_installment),
            ("p2_27", self.amount_payable),
            ("p2_28a", self.surcharge),
            ("p2_28b", self.interest),
            ("p2_28c", self.compromise),
            ("p2_28d", self.total_penalties),
            ("p2_29", self.total_amount_payable),
            ("p4_1", self.gross_compensation),
            ("p4_2", self.non_taxable_compensation),
            ("p4_3", self.taxable_compensation),
            ("p4_4", self.tax_on_compensation),
            ("p4_5", self.sales),
            ("p4_6", self.sales_returns),
            ("p4_7", self.net_sales),
            ("p4_8", self.cost_of_sales),
            ("p4_9", self.gross_income),
            ("p4_10a", self.itemized_deductions),
            ("p4_10b", self.special_allowable_deductions),
            ("p4_10c", self.nolco),
            ("p4_10d", self.total_deductions),
            ("p4_11", self.osd),
            ("p4_12", self.net_income),
            ("p4_13", self.non_operating_income),
            ("p4_14", self.taxable_business_income),
            ("p4_15", self.taxable_income),
            ("p4_17", self.special_tax_due),
            ("p4_18a", self.tax_due_18a),
            ("p4_18b", self.tax_due_18b),
            ("p4_19", self.eight_sales),
            ("p4_20", self.eight_other_income),
            ("p4_21", self.eight_total_income),
            ("p4_22", self.eight_exemption),
            ("p4_23", self.eight_taxable_income),
            ("p4_24", self.eight_tax_due),
            ("p4_25", self.eight_total_tax_due),
            ("p5_1", self.prior_year_excess),
            ("p5_2", self.quarterly_payments),
            ("p5_3", self.cwt_q1_q3),
            ("p5_4", self.cwt_q4),
            ("p5_5", self.cwt_2316),
            ("p5_6", self.previously_filed),
            ("p5_7", self.foreign_tax_credits),
            ("p5_8", self.special_tax_credits),
            ("p5_9", self.other_credits),
            ("p5_10", self.total_credits),
            ("p6_1", self.tax_relief),
        ]
    }

    /// Every amount control of this column with its taxpayer and spouse
    /// element ids.
    fn page_amounts(&self) -> Vec<([&'static str; 2], f64)> {
        vec![
            (
                ["txtIncomeTaxDueNo19a", "txtIncomeTaxDueNo19b"],
                self.income_tax_due,
            ),
            (
                ["txtShareOfOtherNo20a", "txtShareOfOtherNo20b"],
                self.share_of_other_agencies,
            ),
            (
                ["txtNetSpecialRateNo21a", "txtNetSpecialRateNo21b"],
                self.net_special_tax,
            ),
            (
                ["txtIncomeTaxRegularNo22a", "txtIncomeTaxRegularNo22b"],
                self.regular_income_tax,
            ),
            (
                ["txtTotalIncomeTaxDueNo23a", "txtTotalIncomeTaxDueNo23b"],
                self.total_income_tax_due,
            ),
            (
                [
                    "txtTotalTaxCreditsPartIINo24a",
                    "txtTotalTaxCreditsPartIINo24b",
                ],
                self.tax_credits,
            ),
            (
                ["txtTaxPayableNo25a", "txtTaxPayableNo25b"],
                self.tax_payable,
            ),
            (
                ["txtTaxDueAllowedNo26a", "txtTaxDueAllowedNo26b"],
                self.second_installment,
            ),
            (
                ["txtAmountPayableNo27a", "txtAmountPayableNo27b"],
                self.amount_payable,
            ),
            (["txtSurchangeNo28a", "txtSurchangeNo28b"], self.surcharge),
            (["txtInterestNo28a", "txtInterestNo28b"], self.interest),
            (["txtCompromise28a", "txtCompromise28b"], self.compromise),
            (
                ["txtTotalPenaltiesNo28a", "txtTotalPenaltiesNo28b"],
                self.total_penalties,
            ),
            (
                ["txtTotalAmountPayableNo29a", "txtTotalAmountPayableNo29b"],
                self.total_amount_payable,
            ),
            (["txtGrossa", "txtGrossb"], self.gross_compensation),
            (
                ["txtLessNonTaxablea", "txtLessNonTaxableb"],
                self.non_taxable_compensation,
            ),
            (
                [
                    "txtTaxableCompensationIncomea",
                    "txtTaxableCompensationIncomeb",
                ],
                self.taxable_compensation,
            ),
            (
                ["txtTaxpayerNo4a", "txtTaxpayerNo4b"],
                self.tax_on_compensation,
            ),
            (["txtSalesRevenueNo5a", "txtSalesRevenueNo5b"], self.sales),
            (
                ["txtSalesReturnsNo6a", "txtSalesReturnsNo6b"],
                self.sales_returns,
            ),
            (
                [
                    "txtTaxableCompensationIncomeNo7a",
                    "txtTaxableCompensationIncomeNo7b",
                ],
                self.net_sales,
            ),
            (
                ["txtCostofSalesNo8a", "txtCostofSalesNo8b"],
                self.cost_of_sales,
            ),
            (
                ["txtGrossIncomeNo9a", "txtGrossIncomeNo9b"],
                self.gross_income,
            ),
            (
                [
                    "txtAllowableItemizedDeductionsNo10a",
                    "txtAllowableItemizedDeductionsNo10aSpouse",
                ],
                self.itemized_deductions,
            ),
            (
                ["txtSpecialAllowableNo10a", "txtSpecialAllowableNo10b"],
                self.special_allowable_deductions,
            ),
            (
                ["txtNetOperatingLossNo10a", "txtNetOperatingLossNo10b"],
                self.nolco,
            ),
            (
                [
                    "txtTotalAllowableItemixedDeductionsNo10a",
                    "txtTotalAllowableItemixedDeductionsNo10b",
                ],
                self.total_deductions,
            ),
            (
                ["txtOptionalDeductionNo11a", "txtOptionalDeductionNo11b"],
                self.osd,
            ),
            (
                ["txtNetIncomeLossNo12a", "txtNetIncomeLossNo12b"],
                self.net_income,
            ),
            (
                ["txtNonOperatingIncomeNo13a", "txtNonOperatingIncomeNo13b"],
                self.non_operating_income,
            ),
            (
                [
                    "txtTaxableIncomeBusinesNo14a",
                    "txtTaxableIncomeBusinesNo14b",
                ],
                self.taxable_business_income,
            ),
            (
                [
                    "txtCompensationBusinessNo15a",
                    "txtCompensationBusinessNo15b",
                ],
                self.taxable_income,
            ),
            (
                ["txtTaxDueSpeciala", "txtTaxDueSpecialb"],
                self.special_tax_due,
            ),
            (
                ["TotalTaxDueCompensationATP", "TotalTaxDueCompensationASP"],
                self.tax_due_18a,
            ),
            (
                ["TotalTaxDueCompensationBTP", "TotalTaxDueCompensationBSP"],
                self.tax_due_18b,
            ),
            (
                ["txtSalesRevenuesFeesANo19a", "txtSalesRevenuesFeesANo19b"],
                self.eight_sales,
            ),
            (
                [
                    "txtOtherNonOperatingIncomeNo20a",
                    "txtOtherNonOperatingIncomeNo20b",
                ],
                self.eight_other_income,
            ),
            (
                ["txtTotalIncomeNo21a", "txtTotalIncomeNo21b"],
                self.eight_total_income,
            ),
            (["txtLess22A", "txtLess22B"], self.eight_exemption),
            (
                ["txtTaxableIncomeLoss23A", "txtTaxableIncomeLoss23B"],
                self.eight_taxable_income,
            ),
            (
                ["txtTaxDueBusinessIncome24A", "txtTaxDueBusinessIncome24B"],
                self.eight_tax_due,
            ),
            (
                [
                    "txtCompensationBusinessNo25a",
                    "txtCompensationBusinessNo25b",
                ],
                self.eight_total_tax_due,
            ),
            (
                ["txtPriorYearsNo1a", "txtPriorYearsNo1b"],
                self.prior_year_excess,
            ),
            (
                ["txtTaxPaymentsNo2a", "txtTaxPaymentsNo2b"],
                self.quarterly_payments,
            ),
            (
                ["txtCreditableTaxNo3a", "txtCreditableTaxNo3b"],
                self.cwt_q1_q3,
            ),
            (
                [
                    "txtCreditableFourthQuarterNo4a",
                    "txtCreditableFourthQuarterNo4b",
                ],
                self.cwt_q4,
            ),
            (
                ["txtCreditableForm2316No5a", "txtCreditableForm2316No5b"],
                self.cwt_2316,
            ),
            (
                [
                    "txtTaxPaidPreviouslyFiledNo6a",
                    "txtTaxPaidPreviouslyFiledNo6b",
                ],
                self.previously_filed,
            ),
            (
                ["txtForeignTaxCreditsNo7a", "txtForeignTaxCreditsNo7b"],
                self.foreign_tax_credits,
            ),
            (
                ["txtSpecialTaxCreditsNo8a", "txtSpecialTaxCreditsNo8b"],
                self.special_tax_credits,
            ),
            (
                ["txtOtherCreditsPaymentsNo9a", "txtOtherCreditsPaymentsNo9b"],
                self.other_credits,
            ),
            (
                [
                    "txtTotalTaxCreditsPaymentsPartVNo10a",
                    "txtTotalTaxCreditsPaymentsPartVNo10b",
                ],
                self.total_credits,
            ),
            (
                ["totalTaxReliefAvailmentA", "totalTaxReliefAvailmentB"],
                self.tax_relief,
            ),
        ]
    }

    /// The ATC `checkButtons` / `checkButtonsSP` select for this column.
    pub fn atc(&self) -> Option<&'static str> {
        use Form1701MsSource as S;
        use Form1701MsTaxOption as T;
        match (self.source, self.tax_option) {
            (S::Compensation, _) => Some("II011"),
            (S::Business, T::Graduated | T::Exempt | T::Special) => Some("II012"),
            (S::Business, T::EightPercent) => Some("II015"),
            (S::Profession, T::Graduated | T::Exempt | T::Special) => Some("II014"),
            (S::Profession, T::EightPercent) => Some("II017"),
            (S::Mixed, T::Graduated | T::Exempt | T::Special) => Some("II013"),
            (S::Mixed, T::EightPercent) => Some("II016"),
            _ => None,
        }
    }

    /// Schedule A is open for mixed income and (spouse) compensation income.
    pub fn schedule_a_open(&self) -> bool {
        self.source.has_compensation()
    }

    /// Schedule B1 is open for business income under graduated rates, exempt
    /// or special rates (`setField*Part4B1Enabled`).
    pub fn schedule_b1_open(&self) -> bool {
        self.source.has_business()
            && matches!(
                self.tax_option,
                Form1701MsTaxOption::Graduated
                    | Form1701MsTaxOption::Exempt
                    | Form1701MsTaxOption::Special
            )
    }

    /// Items 8 and 10A–10C follow Itemized Deductions.
    pub fn itemized_open(&self) -> bool {
        self.schedule_b1_open() && self.deduction == Form1701MsDeduction::Itemized
    }

    /// Schedule B2 is open for business income under the 8% option.
    pub fn schedule_b2_open(&self) -> bool {
        self.source.has_business() && self.tax_option == Form1701MsTaxOption::EightPercent
    }

    /// Part V Item 5 (Form 2316) is open only for ATC II011 and II013.
    pub fn form_2316_open(&self) -> bool {
        matches!(self.atc(), Some("II011" | "II013"))
    }

    /// Items 13–16 apply to exempt and special-rate filers.
    pub fn relief_items_open(&self) -> bool {
        self.tax_option.is_exempt_or_special()
    }

    /// Item 17 is open for business income under graduated rates.
    pub fn deduction_open(&self) -> bool {
        self.source.has_business() && self.tax_option == Form1701MsTaxOption::Graduated
    }

    /// Item 20 opens once Item 19 is positive (`toggleShareOfOtherNo20*`).
    pub fn share_open(&self) -> bool {
        self.income_tax_due > 0.0
    }

    /// Item 26 closes while Item 25 is negative (`setP2No25*Status`).
    pub fn installment_open(&self) -> bool {
        self.tax_payable >= 0.0
    }

    /// The radio rules: what each Item 11/12 click forces and clears.
    fn normalize(&mut self, spouse: bool, previously_filed_open: bool) {
        if !spouse && self.source == Form1701MsSource::Compensation {
            self.source = Form1701MsSource::Unanswered;
        }
        if self.source == Form1701MsSource::Compensation {
            // sourceOfIncomeSP auto-selects Graduated and closes Item 17.
            self.tax_option = Form1701MsTaxOption::Graduated;
            self.deduction = Form1701MsDeduction::Unanswered;
        }
        match self.tax_option {
            Form1701MsTaxOption::Exempt | Form1701MsTaxOption::Special => {
                // incomeIsSubjectTo* ticks Itemized and locks Item 17.
                self.deduction = Form1701MsDeduction::Itemized;
            }
            Form1701MsTaxOption::EightPercent | Form1701MsTaxOption::Unanswered => {
                self.deduction = Form1701MsDeduction::Unanswered;
            }
            Form1701MsTaxOption::Graduated => {
                if !self.source.has_business() {
                    self.deduction = Form1701MsDeduction::Unanswered;
                }
            }
        }
        if !self.relief_items_open() {
            self.legal_basis.clear();
            self.promotion_agency.clear();
            self.registered_activity.clear();
            self.effectivity_from.clear();
            self.effectivity_to.clear();
            self.special_rate = 0.0;
        }
        if !self.schedule_a_open() {
            self.gross_compensation = 0.0;
            self.non_taxable_compensation = 0.0;
        }
        if !self.schedule_b1_open() {
            self.sales = 0.0;
            self.sales_returns = 0.0;
            self.non_operating_income = 0.0;
        }
        if !self.itemized_open() {
            self.cost_of_sales = 0.0;
            self.itemized_deductions = 0.0;
            self.itemized_rows.clear();
            self.special_allowable_deductions = 0.0;
            self.nolco = 0.0;
        }
        if !self.schedule_b2_open() {
            self.eight_sales = 0.0;
            self.eight_other_income = 0.0;
        }
        if !self.form_2316_open() {
            self.cwt_2316 = 0.0;
        }
        if !previously_filed_open {
            self.previously_filed = 0.0;
        }
    }

    /// Whole pesos the way `removeDecimal` leaves each entry.
    fn round_inputs(&mut self) {
        for value in [
            &mut self.gross_compensation,
            &mut self.non_taxable_compensation,
            &mut self.sales,
            &mut self.sales_returns,
            &mut self.cost_of_sales,
            &mut self.itemized_deductions,
            &mut self.special_allowable_deductions,
            &mut self.nolco,
            &mut self.non_operating_income,
            &mut self.eight_sales,
            &mut self.eight_other_income,
            &mut self.prior_year_excess,
            &mut self.quarterly_payments,
            &mut self.cwt_q1_q3,
            &mut self.cwt_q4,
            &mut self.cwt_2316,
            &mut self.previously_filed,
            &mut self.foreign_tax_credits,
            &mut self.special_tax_credits,
            &mut self.other_credits,
            &mut self.share_of_other_agencies,
            &mut self.second_installment,
            &mut self.surcharge,
            &mut self.interest,
            &mut self.compromise,
        ] {
            *value = js_round(*value);
        }
        // removeDecimal on each popup amount, then
        // computeOtherSched410A writes Math.round(total) into Item 10A.
        for row in &mut self.itemized_rows {
            row.amount = js_round(row.amount);
        }
        if !self.itemized_rows.is_empty() {
            self.itemized_deductions =
                js_round(self.itemized_rows.iter().map(|row| row.amount).sum());
        }
    }

    /// The official chain for one column, in dependency order.
    fn compute(&mut self) {
        use Form1701MsTaxOption as T;
        let option = self.tax_option;
        let mixed = self.source == Form1701MsSource::Mixed;

        // Schedule A (calculateNo3*).
        self.taxable_compensation = self.gross_compensation - self.non_taxable_compensation;
        self.tax_on_compensation = js_round(form_1701ms_graduated_tax(self.taxable_compensation));

        // Schedule B1.
        self.net_sales = self.sales - self.sales_returns;
        self.gross_income = self.net_sales - self.cost_of_sales;
        self.total_deductions =
            self.itemized_deductions + self.special_allowable_deductions + self.nolco;
        self.osd = if self.deduction == Form1701MsDeduction::Osd {
            js_round(self.net_sales * 0.40)
        } else {
            0.0
        };
        self.net_income = match self.deduction {
            Form1701MsDeduction::Itemized => self.gross_income - self.total_deductions,
            Form1701MsDeduction::Osd => self.gross_income - self.osd,
            Form1701MsDeduction::Unanswered => 0.0,
        };
        self.taxable_business_income = self.net_income + self.non_operating_income;
        let positive = |v: f64| if v > 0.0 { v } else { 0.0 };
        self.taxable_income = if self.source == Form1701MsSource::Compensation {
            self.taxable_business_income
        } else if option == T::Graduated || (mixed && option.is_exempt_or_special()) {
            positive(self.taxable_compensation) + positive(self.taxable_business_income)
        } else {
            0.0
        };
        self.special_tax_due = match option {
            T::Exempt => js_round(self.taxable_business_income * (self.special_rate / 100.0)),
            T::Special => js_round(positive(self.gross_income) * (self.special_rate / 100.0)),
            _ => 0.0,
        };
        self.tax_due_18a = if option.is_exempt_or_special() && !(mixed && option == T::Special) {
            js_round(self.special_tax_due + self.tax_on_compensation)
        } else {
            0.0
        };
        self.tax_due_18b = if option == T::Graduated && self.taxable_income >= 0.0 {
            js_round(form_1701ms_graduated_tax(self.taxable_income))
        } else {
            0.0
        };

        // Schedule B2.
        self.eight_total_income = self.eight_sales + self.eight_other_income;
        self.eight_exemption = if self.schedule_b2_open() && !mixed {
            FORM_1701MS_EIGHT_PERCENT_EXEMPTION
        } else {
            0.0
        };
        self.eight_taxable_income = self.eight_total_income - self.eight_exemption;
        self.eight_tax_due = if self.eight_taxable_income >= 1.0 {
            js_round(self.eight_taxable_income * 0.08)
        } else {
            0.0
        };
        self.eight_total_tax_due = if option == T::EightPercent {
            self.tax_on_compensation + self.eight_tax_due
        } else {
            0.0
        };

        // Part V.
        self.total_credits = self.prior_year_excess
            + self.quarterly_payments
            + self.cwt_q1_q3
            + self.cwt_q4
            + self.cwt_2316
            + self.previously_filed
            + self.foreign_tax_credits
            + self.special_tax_credits
            + self.other_credits;

        // Part II.
        self.income_tax_due = match option {
            T::Special => self.special_tax_due,
            T::Exempt => 0.0,
            _ => self.tax_due_18a,
        };
        if !self.share_open() {
            self.share_of_other_agencies = 0.0;
        }
        self.net_special_tax = self.income_tax_due - self.share_of_other_agencies;
        self.regular_income_tax = match option {
            T::Graduated if self.source == Form1701MsSource::Compensation => {
                self.tax_on_compensation
            }
            T::Graduated => self.tax_due_18b,
            T::Exempt | T::Special => self.tax_on_compensation,
            T::EightPercent => self.eight_total_tax_due,
            T::Unanswered => 0.0,
        };
        self.total_income_tax_due = self.net_special_tax + self.regular_income_tax;
        self.tax_credits = self.total_credits;
        self.tax_payable = self.total_income_tax_due - self.tax_credits;
        if !self.installment_open() {
            self.second_installment = 0.0;
        }
        self.amount_payable = self.tax_payable - self.second_installment;
        self.total_penalties = self.surcharge + self.interest + self.compromise;
        self.total_amount_payable = if self.amount_payable < 0.0 && self.total_penalties > 0.0 {
            self.total_penalties
        } else {
            self.amount_payable + self.total_penalties
        };

        // Part VI (computeTaxAvailment*). The page refreshes this only from
        // some handlers, so its value can lag Item 17 when the rate is typed
        // last; this is the value it shows once every input is current.
        self.tax_relief = if option.is_exempt_or_special() {
            let computed = form_1701ms_graduated_tax(
                self.special_allowable_deductions + self.taxable_business_income,
            );
            js_round(positive(
                computed - self.special_tax_due + self.special_tax_credits,
            ))
        } else {
            0.0
        };
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1701MsDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–5
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub taxable_year: u16,
    /// Item 1 month: 12, or 1–11 on a short period return.
    pub month: u8,
    pub is_amended: bool,
    pub is_short_period: bool,
    #[serde(default)]
    pub civil_status: Form1701MsCivilStatus,
    #[serde(default)]
    pub filing: Form1701MsFiling,

    // Items 6–10, taxpayer
    pub rdo_code: String,
    pub taxpayer_name: String,
    pub email: String,
    pub contact_number: String,
    #[serde(default)]
    pub spouse_info: Form1701MsSpouseInfo,

    #[serde(default)]
    pub taxpayer: Form1701MsColumn,
    #[serde(default)]
    pub spouse: Form1701MsColumn,

    /// Part V Item 7 description (`addForeignTaxCredits`).
    #[serde(default)]
    pub foreign_tax_credits_description: String,
    /// Part V Item 9 description (`addOtherCreditsPayments`).
    #[serde(default)]
    pub other_credits_description: String,

    /// Item 30.
    #[serde(default)]
    pub aggregate_amount_payable: f64,
    /// Item 31 boxes and amounts (centavos).
    #[serde(default)]
    pub to_be_refunded: bool,
    #[serde(default)]
    pub refund_amount: f64,
    #[serde(default)]
    pub to_be_issued_tcc: bool,
    #[serde(default)]
    pub tcc_amount: f64,
    #[serde(default)]
    pub to_be_carried_over: bool,
    #[serde(default)]
    pub carry_over_amount: f64,

    /// The perjury declaration check box.
    #[serde(default)]
    pub perjury_agreed: bool,

    /// The signature block's printed name and authorized representative
    /// (`printedName`, `authorizeRepresentative`), uploaded as typed.
    #[serde(default)]
    pub signatory_name: String,
    #[serde(default)]
    pub authorized_representative: String,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits = digits(tin);
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

impl Form1701MsDraft {
    pub const FORM_CODE: &'static str = "1701MS";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16) -> Self {
        let mut draft = Self {
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            month: 12,
            is_amended: false,
            is_short_period: false,
            civil_status: Form1701MsCivilStatus::Single,
            filing: Form1701MsFiling::Unanswered,
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            email: profile.email.clone(),
            contact_number: profile.phone.clone(),
            spouse_info: Form1701MsSpouseInfo::default(),
            taxpayer: Form1701MsColumn::default(),
            spouse: Form1701MsColumn::default(),
            foreign_tax_credits_description: String::new(),
            other_credits_description: String::new(),
            aggregate_amount_payable: 0.0,
            to_be_refunded: false,
            refund_amount: 0.0,
            to_be_issued_tcc: false,
            tcc_amount: 0.0,
            to_be_carried_over: false,
            carry_over_amount: 0.0,
            perjury_agreed: false,
            signatory_name: String::new(),
            authorized_representative: String::new(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Whether the filer typed into a Part V control whose `onblur` calls
    /// `capital(this, event)` (Items 7 and 9: `addForeignTaxCredits`,
    /// `txtForeignTaxCreditsNo7a/b`, `addOtherCreditsPayments`,
    /// `txtOtherCreditsPaymentsNo9b`). The last `capital` definition in
    /// string-util.js ignores its arguments and upper-cases every text
    /// control of `frmMain`; the model assumes Part V is filled after Items
    /// 6–16, as the page's order has it.
    pub fn part_v_capitalizes(&self) -> bool {
        !self.foreign_tax_credits_description.trim().is_empty()
            || !self.other_credits_description.trim().is_empty()
            || self.taxpayer.foreign_tax_credits != 0.0
            || self.spouse.foreign_tax_credits != 0.0
            || self.spouse.other_credits != 0.0
    }

    /// Spouse columns are open only for a joint return (`marriedJointly`).
    pub fn is_joint(&self) -> bool {
        self.civil_status == Form1701MsCivilStatus::Married
            && self.filing == Form1701MsFiling::Jointly
    }

    /// Part V Item 6 opens for amended and short period returns; the spouse
    /// column only on a joint return.
    fn previously_filed_open(&self, spouse: bool) -> bool {
        (self.is_amended || self.is_short_period) && (!spouse || self.is_joint())
    }

    /// Item 31 is open only while Item 30 is negative.
    pub fn overpayment_open(&self) -> bool {
        self.aggregate_amount_payable < 0.0
    }

    /// The official compute chain over both columns.
    pub fn recompute(&mut self) {
        if !self.is_short_period {
            self.month = 12;
        }
        if self.civil_status != Form1701MsCivilStatus::Married {
            self.filing = Form1701MsFiling::Unanswered;
        }
        if !self.is_joint() {
            // marriedJointly / toggleLine5 reset every spouse control.
            self.spouse = Form1701MsColumn::default();
            self.spouse_info = Form1701MsSpouseInfo::default();
        }
        let taxpayer_previous = self.previously_filed_open(false);
        let spouse_previous = self.previously_filed_open(true);
        self.taxpayer.round_inputs();
        self.spouse.round_inputs();
        self.taxpayer.normalize(false, taxpayer_previous);
        self.spouse.normalize(true, spouse_previous);
        self.taxpayer.compute();
        self.spouse.compute();
        // Each column's penalty items file their override, else the
        // computed value; the column totals are then summed with them.
        for items in Self::penalty_declarations() {
            items.apply(self);
        }
        self.taxpayer.round_inputs();
        self.spouse.round_inputs();
        self.taxpayer.compute();
        self.spouse.compute();

        // computeAggregateAmountNo30.
        let a = self.taxpayer.total_amount_payable;
        let b = self.spouse.total_amount_payable;
        self.aggregate_amount_payable = if a < 0.0 && b > 0.0 {
            b
        } else if b < 0.0 && a > 0.0 {
            a
        } else {
            a + b
        };
        if !self.overpayment_open() {
            self.to_be_refunded = false;
            self.to_be_issued_tcc = false;
            self.to_be_carried_over = false;
        }
        // An unchecked box clears its amount (the check box onclick handlers).
        self.refund_amount = if self.to_be_refunded {
            cents(self.refund_amount)
        } else {
            0.0
        };
        self.tcc_amount = if self.to_be_issued_tcc {
            cents(self.tcc_amount)
        } else {
            0.0
        };
        self.carry_over_amount = if self.to_be_carried_over {
            cents(self.carry_over_amount)
        } else {
            0.0
        };
    }

    /// The official field values `saveEncryptedProfile` reads, keyed by
    /// element id: every `frmMain` control in page order.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut text = |key: &str, value: String| {
            fields.insert(format!("frm1701MS:{key}"), value);
        };
        let joint = self.is_joint();

        // Items 1–10.
        text("txtMonthNo1", format!("{:02}", self.month));
        text("txtYearNo1", self.taxable_year.to_string());
        text("civilStatus", self.civil_status.value().to_string());
        let (a, b, c, branch) = split_tin(&self.tin);
        // loadBGData copies the profile TIN to both pages.
        for page in ["Pg1", "Pg2"] {
            text(&format!("txt{page}TIN1"), a.clone());
            text(&format!("txt{page}TIN2"), b.clone());
            text(&format!("txt{page}TIN3"), c.clone());
            text(&format!("txt{page}BranchCode"), branch.clone());
        }
        text("txtRDOCodea", self.rdo_code.trim().to_string());
        let name = self.taxpayer_name.trim().to_uppercase();
        // loadBGData: page 2 shows the name up to its first comma.
        text(
            "Pg2TaxPayer",
            name.split(',').next().unwrap_or("").to_string(),
        );
        text("txtTaxpayerNo8a", name);
        text("txtEmaila", self.email.trim().to_string());
        text("txtTelNum10a", self.contact_number.trim().to_string());
        let spouse = &self.spouse_info;
        let (sa, sb, sc, sbranch) = if joint {
            split_tin(&spouse.tin)
        } else {
            Default::default()
        };
        text("txtPg2TIN1Spouse", sa);
        text("txtPg2TIN2Spouse", sb);
        text("txtPg2TIN3Spouse", sc);
        text("txtPg2BranchCodeSpouse", sbranch);
        let spouse_text = |value: &str| {
            if joint {
                value.trim().to_string()
            } else {
                String::new()
            }
        };
        text("txtRDOCodeb", spouse_text(&spouse.rdo_code));
        // capitalize(this) on blur.
        text("txtTaxpayerNo8b", spouse_text(&spouse.name).to_uppercase());
        text("txtEmailb", spouse_text(&spouse.email));
        text("txtTelNum10b", spouse_text(&spouse.contact_number));

        // Items 13–16 (capitalize on blur; dates as typed).
        for (suffix, column) in [("a", &self.taxpayer), ("b", &self.spouse)] {
            text(
                &format!("txtTaxpayerNo13{suffix}"),
                column.legal_basis.trim().to_uppercase(),
            );
            text(
                &format!("txtTaxpayerNo14{suffix}"),
                column.promotion_agency.trim().to_uppercase(),
            );
            text(
                &format!("txtTaxpayerNo15{suffix}"),
                column.registered_activity.trim().to_uppercase(),
            );
            text(
                &format!("txtTaxpayerNo16{suffix}"),
                column.effectivity_from.trim().to_string(),
            );
            text(
                &format!("txtTaxpayerNo16{suffix}1"),
                column.effectivity_to.trim().to_string(),
            );
        }

        // Part II, Part IV and Part V amounts, both columns.
        let columns = [(&self.taxpayer, 0usize), (&self.spouse, 1usize)];
        for (column, side) in columns {
            for (keys, value) in column.page_amounts() {
                text(keys[side], page_amount(value));
            }
            let rate = if side == 0 {
                "txtTaxRatea"
            } else {
                "txtTaxRateb"
            };
            text(rate, page_rate(column.special_rate));
        }
        text(
            "txtAggregateAmountPayable30a",
            page_amount(self.aggregate_amount_payable),
        );
        // Item 31: an unchecked box clears its amount; round(this,2).
        let item31 = |on: bool, value: f64| {
            if on {
                official_amount(value)
            } else {
                String::new()
            }
        };
        text(
            "txtToBeRefunded1",
            item31(self.to_be_refunded, self.refund_amount),
        );
        text(
            "txtToBeIssued1",
            item31(self.to_be_issued_tcc, self.tcc_amount),
        );
        text(
            "txtToBeCarried1",
            item31(self.to_be_carried_over, self.carry_over_amount),
        );
        text(
            "addForeignTaxCredits",
            self.foreign_tax_credits_description.trim().to_uppercase(),
        );
        text(
            "addOtherCreditsPayments",
            self.other_credits_description.trim().to_uppercase(),
        );
        // Items 32–35 and the payment details are not modelled; they stay as
        // the page leaves them.
        text("txtPg1I33NumberOfAttachments", "00".to_string());
        // init() opens page 1 of 2.
        text("txtCurrentPage", "1".to_string());
        text("txtMaxPage", "2".to_string());

        // ATC selects (checkButtons / checkButtonsSP).
        fields.insert(
            "taxCodeDropdown".to_string(),
            self.taxpayer.atc().unwrap_or("").to_string(),
        );
        fields.insert(
            "taxCodeDropdownSpouse".to_string(),
            if joint { self.spouse.atc() } else { None }
                .unwrap_or("")
                .to_string(),
        );

        // Part V Items 7 and 9 blur through string-util's `capital()`, which
        // upper-cases every text control of the form (emails included).
        if self.part_v_capitalizes() {
            for (key, value) in fields.iter_mut() {
                if !SELECT_CONTROLS.contains(&key.as_str()) {
                    *value = value.to_uppercase();
                }
            }
        }

        let mut put = |key: &str, on: bool| {
            fields.insert(format!("frm1701MS:{key}"), on.to_string());
        };
        put("txtAmendedYesNo2", self.is_amended);
        put("txtAmendedNoNo2", !self.is_amended);
        put("txtReturnPeriodYesNo3", self.is_short_period);
        put("txtReturnPeriodNoNo3", !self.is_short_period);
        put(
            "txtIfMarriedJointlyNo5",
            self.filing == Form1701MsFiling::Jointly,
        );
        put(
            "txtIfMarriedSeparatelyNo5",
            self.filing == Form1701MsFiling::Separately,
        );
        put(
            "txtIfMarriedNotApplicableNo5",
            self.filing == Form1701MsFiling::NotApplicable,
        );

        use Form1701MsDeduction as D;
        use Form1701MsSource as S;
        use Form1701MsTaxOption as T;
        let tp = &self.taxpayer;
        let sp = &self.spouse;
        put("txtTaxpayerNo11a", tp.source == S::Business);
        put("txtTaxpayerNo11b", tp.source == S::Mixed);
        put("txtTaxpayerNo11c", tp.source == S::Profession);
        put("txtTaxpayerNo11a1", sp.source == S::Business);
        put("txtTaxpayerNo11b1", sp.source == S::Mixed);
        put("txtTaxpayerNo11c1", sp.source == S::Profession);
        put("txtTaxpayerNo11d1", sp.source == S::Compensation);
        put("txtTaxpayerNo12b", tp.tax_option == T::Graduated);
        put("txtTaxpayerNo12c", tp.tax_option == T::EightPercent);
        put("txtTaxpayerNo12d", tp.tax_option == T::Exempt);
        put("txtTaxpayerNo12e", tp.tax_option == T::Special);
        put("txtTaxpayerNo12b1", sp.tax_option == T::Graduated);
        put("txtTaxpayerNo12c1", sp.tax_option == T::EightPercent);
        put("txtTaxpayerNo12d1", sp.tax_option == T::Exempt);
        put("txtTaxpayerNo12e1", sp.tax_option == T::Special);
        put("txtTaxpayerNo17a", tp.deduction == D::Itemized);
        put("txtTaxpayerNo17b", tp.deduction == D::Osd);
        put("txtSpouseNo17a", sp.deduction == D::Itemized);
        put("txtSpouseNo17b", sp.deduction == D::Osd);
        put("txtToBeRefunded", self.to_be_refunded);
        put("txtToBeIssued", self.to_be_issued_tcc);
        put("txtToBeCarried", self.to_be_carried_over);
        put("perjuryClause", self.perjury_agreed);
        fields.insert(
            "frm1701MS:printedName".to_string(),
            self.signatory_name.trim().to_string(),
        );
        fields.insert(
            "frm1701MS:authorizeRepresentative".to_string(),
            self.authorized_representative.trim().to_string(),
        );
        fields
    }

    /// The field map plus the `derived:` values the printed 1701-MS sheet
    /// shows (`html-frozen/1701ms-2024/writer-cells.json`): identity, Items
    /// 13–18, every Part II, IV, V and VI amount in both columns and the
    /// Item 31 amounts. Print only; none of the `derived:` keys is submitted.
    ///
    /// The sheet's amount boxes are 9-slot whole-peso combs, so amounts print
    /// as digits without thousands separators, right-aligned (left-padded
    /// with blanks to the comb width); an overpayment prints in parentheses.
    /// The spouse column prints only on a joint return.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let mut put = |key: &str, value: String| {
            if !value.is_empty() {
                fields.insert(format!("derived:{key}"), value);
            }
        };
        put("month", format!("{:02}", self.month));
        put("year", self.taxable_year.to_string());
        put(
            "civil_status",
            self.civil_status.label().to_ascii_uppercase(),
        );
        let (a, b, c, branch) = split_tin(&self.tin);
        // Page 2's TIN comb has nine inputs; the branch `00000` is preprinted.
        put("tin_digits", format!("{a}{b}{c}"));
        put("page2_name", self.taxpayer_name.trim().to_uppercase());
        put("tp_tin", format!("{a}-{b}-{c}-{branch}"));
        put("tp_rdo", self.rdo_code.trim().to_string());
        put("tp_email", self.email.trim().to_string());
        put("tp_contact", self.contact_number.trim().to_string());
        if self.is_joint() {
            let spouse = &self.spouse_info;
            let (a, b, c, branch) = split_tin(&spouse.tin);
            put("sp_tin", format!("{a}-{b}-{c}-{branch}"));
            put("sp_rdo", spouse.rdo_code.trim().to_string());
            put("sp_name", spouse.name.trim().to_uppercase());
            put("sp_email", spouse.email.trim().to_string());
            put("sp_contact", spouse.contact_number.trim().to_string());
        }
        let columns: &[(&str, &Form1701MsColumn)] = if self.is_joint() {
            &[("tp", &self.taxpayer), ("sp", &self.spouse)]
        } else {
            &[("tp", &self.taxpayer)]
        };
        for &(side, column) in columns {
            let mut col = |key: &str, value: String| put(&format!("{side}_{key}"), value);
            col("legal_basis", column.legal_basis.trim().to_string());
            col("agency", column.promotion_agency.trim().to_string());
            col("activity", column.registered_activity.trim().to_string());
            col(
                "effectivity_from",
                column.effectivity_from.trim().to_string(),
            );
            col("effectivity_to", column.effectivity_to.trim().to_string());
            if let Some(atc) = column.atc() {
                col("atc", atc.to_string());
                col(
                    "atc_description",
                    form_1701ms_atc_description(atc).to_string(),
                );
            }
            if column.tax_option.is_exempt_or_special() {
                let (whole, fraction) = print_rate(column.special_rate);
                col("rate_whole", whole);
                col("rate_fraction", fraction);
            }
            for (key, value) in column.print_amounts() {
                col(key, print_whole_amount(value));
            }
        }
        put(
            "aggregate",
            print_whole_amount(self.aggregate_amount_payable),
        );
        put(
            "foreign_tax_credits_description",
            self.foreign_tax_credits_description.trim().to_string(),
        );
        put(
            "other_credits_description",
            self.other_credits_description.trim().to_string(),
        );
        if self.to_be_refunded {
            put("refund_amount", print_cents_amount(self.refund_amount));
        }
        if self.to_be_issued_tcc {
            put("tcc_amount", print_cents_amount(self.tcc_amount));
        }
        if self.to_be_carried_over {
            put(
                "carry_over_amount",
                print_cents_amount(self.carry_over_amount),
            );
        }
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    fn validate_effectivity(
        &self,
        column: &Form1701MsColumn,
        who: &str,
        field: &str,
        err: &mut dyn FnMut(&str, &str),
    ) -> bool {
        // validateEffectivityDate*16: From before the last day of the return
        // month; To on or after it.
        let Some(last_day) = NaiveDate::from_ymd_opt(
            i32::from(self.taxable_year),
            u32::from(self.month.clamp(1, 12)),
            1,
        )
        .and_then(|first| first.checked_add_months(chrono::Months::new(1)))
        .and_then(|next| next.pred_opt()) else {
            return true;
        };
        let stamp = format!(
            "{}/{}/{}",
            format_args!("{:02}", self.month),
            last_day.day(),
            self.taxable_year
        );
        match parse_us_date(&column.effectivity_from) {
            Some(from) if from >= last_day => {
                err(
                    &format!("{field}.effectivity_from"),
                    &format!("Invalid date. {who} Item 16 From date must be earlier than {stamp}."),
                );
                return false;
            }
            None if !column.effectivity_from.trim().is_empty() => {
                err(
                    &format!("{field}.effectivity_from"),
                    &format!("Invalid date. {who} Item 16 From date must be earlier than {stamp}."),
                );
                return false;
            }
            _ => {}
        }
        match parse_us_date(&column.effectivity_to) {
            Some(to) if to < last_day => {
                err(
                    &format!("{field}.effectivity_to"),
                    &format!("Invalid date. {who} Item 16 To date must be {stamp} and onwards."),
                );
                false
            }
            None if !column.effectivity_to.trim().is_empty() => {
                err(
                    &format!("{field}.effectivity_to"),
                    &format!("Invalid date. {who} Item 16 To date must be {stamp} and onwards."),
                );
                false
            }
            _ => true,
        }
    }

    fn validate_on(&self, today: NaiveDate) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        use Form1701MsSource as S;
        use Form1701MsTaxOption as T;
        let tp = &self.taxpayer;
        let sp = &self.spouse;
        let current_year = today.year();
        let current_month = today.month();
        let year = i32::from(self.taxable_year);
        let month = u32::from(self.month);

        // Item 1.
        if !self.is_short_period {
            if year >= current_year {
                err(
                    "taxable_year",
                    "Future filing is not allowed. Please input correct year on Item 1",
                );
            }
        } else if month > current_month && year >= current_year {
            err(
                "taxable_year",
                "Future filing is not allowed. Please input correct year on Item 1",
            );
        }

        if self.civil_status == Form1701MsCivilStatus::Married
            && self.filing == Form1701MsFiling::Unanswered
        {
            err(
                "filing",
                "Item 5 is required if Item 4 is marked as Married.",
            );
        }

        if self.is_joint() {
            let info = &self.spouse_info;
            let (t1, t2, t3, _) = split_tin(&info.tin);
            let spouse_digits = digits(&info.tin);
            if t1.len() != 3
                || t2.len() != 3
                || t3.len() != 3
                || spouse_digits.len() > 14
                || !info.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
            {
                err(
                    "spouse_info.tin",
                    "Please enter a valid TIN number on Item 6 for Spouse",
                );
            } else if !crate::validation::relaxed_dev_mode()
                && crate::validation::official_tin_check_code(&format!("{t1}{t2}{t3}")) != 0
            {
                err(
                    "spouse_info.tin",
                    crate::validation::OFFICIAL_INVALID_TIN_MESSAGE,
                );
            }
            if !crate::validation::rdo_code_is_official_option(info.rdo_code.trim()) {
                err(
                    "spouse_info.rdo_code",
                    "Please enter a valid RDO code for Spouse on Item 7",
                );
            }
            if info.name.trim().is_empty() || info.name.trim().chars().count() > 60 {
                err(
                    "spouse_info.name",
                    "Please enter a valid Name for Spouse on Item 8",
                );
            }
            let email = info.email.trim();
            if email.is_empty() || email.len() > 60 || !email.contains('@') {
                err(
                    "spouse_info.email",
                    "Please enter a valid Email Address for Spouse on Item 9",
                );
            }
            let phone = info.contact_number.trim();
            if phone.is_empty() || phone.len() > 20 || !phone.bytes().all(|b| b.is_ascii_digit()) {
                err(
                    "spouse_info.contact_number",
                    "Please enter a valid Contact Number for Spouse on Item 10",
                );
            }
            if tp.source == S::Unanswered {
                err(
                    "taxpayer.source",
                    "Please select an option on Item 11 for Taxpayer",
                );
            }
            if sp.source == S::Unanswered {
                err(
                    "spouse.source",
                    "Please select an option on Item 11 for Spouse",
                );
            }
            if sp.tax_option == T::Unanswered {
                err(
                    "spouse.tax_option",
                    "Item 12 is required if Item 5 is marked as Jointly.",
                );
            }
            // The official check compares the Item 12 Special radio element
            // itself with `true`, so it only ever runs for Exempt; check both.
            if sp.tax_option.is_exempt_or_special() {
                self.validate_effectivity(sp, "Spouse", "spouse", &mut err);
            }
            if sp.deduction_open() && sp.deduction == Form1701MsDeduction::Unanswered {
                err("spouse.deduction", ITEM_17_ALERT);
            }
        } else if tp.source == S::Unanswered {
            // Official: only checked on a joint return; a return without a
            // source of income has no tax base, so require it always.
            err(
                "taxpayer.source",
                "Please select an option on Item 11 for Taxpayer",
            );
        }

        // Item 31 (official: inside the joint branch only; checked always).
        if self.overpayment_open() {
            if !self.to_be_refunded && !self.to_be_issued_tcc && !self.to_be_carried_over {
                err(
                    "overpayment",
                    "Item 31 is mandatory because Item 30 is has negative value.",
                );
            } else {
                let item27_total = (tp.amount_payable + sp.amount_payable).abs();
                let total31 = cents(self.refund_amount + self.tcc_amount + self.carry_over_amount);
                if (total31 - item27_total).abs() > 0.004 {
                    err(
                        "overpayment",
                        &format!(
                            "The total amount in Item 31 must equal the sum of Item 27A and Item 27B ({}).",
                            whole_with_commas(item27_total)
                        ),
                    );
                }
            }
        }

        if tp.tax_option.is_exempt_or_special() {
            self.validate_effectivity(tp, "Taxpayer", "taxpayer", &mut err);
        }

        // Part V Items 7 and 9 need their description and amount together.
        let foreign = tp.foreign_tax_credits != 0.0 || sp.foreign_tax_credits != 0.0;
        let foreign_text = !self.foreign_tax_credits_description.trim().is_empty();
        if foreign && !foreign_text {
            err(
                "foreign_tax_credits_description",
                "Part V - Item 7 Foreign Tax Credits is mandatory if the amount column has value.",
            );
        } else if foreign_text && !foreign {
            err(
                "taxpayer.foreign_tax_credits",
                "Part V - Item 7 Foreign Tax Amount is mandatory if Foreign Tax Credits field has value.",
            );
        }
        let other = tp.other_credits != 0.0 || sp.other_credits != 0.0;
        let other_text = !self.other_credits_description.trim().is_empty();
        if other && !other_text {
            err(
                "other_credits_description",
                "Part V - Item 9 Other Credits/Payments is mandatory if the Amount column has value.",
            );
        } else if other_text && !other {
            err(
                "taxpayer.other_credits",
                "Part V - Item 9 Amount is mandatory if the Other Credits/Payments field has value.",
            );
        }

        if self.taxable_year == 0 || !(1..=12).contains(&self.month) {
            err("taxable_year", "Please enter a valid year and month.");
        }
        if year > current_year {
            err(
                "taxable_year",
                "Future filing is not allowed. Please enter a valid year.",
            );
        }
        if year == current_year && month > current_month {
            err(
                "month",
                "Future filing is not allowed. Please enter a valid month.",
            );
        }
        if self.is_short_period && self.month == 12 {
            // limitMonthOptions removes December on a short period return.
            err("month", "Please enter a valid month and year for Item 1.");
        }

        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        if tin1.len() != 3
            || tin2.len() != 3
            || tin3.len() != 3
            || digits(&self.tin).len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 6");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if self.taxable_year == 0 {
            err("taxable_year", "Item 1 is a required field.");
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO code on Item 7");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 8",
            );
        }
        let email = self.email.trim();
        if email.is_empty()
            || email.len() > 60
            || !email.contains('@')
            || email.contains(char::is_whitespace)
        {
            err("email", "Please enter a valid Email Address on Item 9");
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 50 {
            err(
                "contact_number",
                "Please enter a valid Contact Number on Item 10",
            );
        }

        if tp.tax_option == T::Unanswered {
            err(
                "taxpayer.tax_option",
                "Taxpayer Item 12 is a required field ",
            );
        }
        let relief_missing = |c: &Form1701MsColumn| {
            c.legal_basis.trim().is_empty()
                || c.promotion_agency.trim().is_empty()
                || c.registered_activity.trim().is_empty()
                || c.effectivity_from.trim().is_empty()
                || c.effectivity_to.trim().is_empty()
        };
        if tp.relief_items_open() && relief_missing(tp) {
            err(
                "taxpayer.legal_basis",
                "Please make sure to enter details on Item 13 to 16 on Taxpayer",
            );
        }
        if sp.relief_items_open() && relief_missing(sp) {
            err(
                "spouse.legal_basis",
                "Please make sure to enter details on Item 13 to 16 on Spouse",
            );
        }
        if tp.deduction_open() && tp.deduction == Form1701MsDeduction::Unanswered {
            err("taxpayer.deduction", ITEM_17_ALERT);
        }

        if !self.perjury_agreed {
            err("perjury_agreed", "You need to agree to the Perjury Clause.");
        }

        // Limits the page enforces while typing.
        for (prefix, column) in [("taxpayer", tp), ("spouse", sp)] {
            if column.tax_option == T::EightPercent
                && column.eight_total_income > FORM_1701MS_EIGHT_PERCENT_THRESHOLD
            {
                err(
                    &format!("{prefix}.eight_sales"),
                    EIGHT_PERCENT_THRESHOLD_ALERT,
                );
            }
            if column.installment_open()
                && (column.second_installment > column.total_income_tax_due / 2.0
                    || column.second_installment > column.tax_payable)
            {
                err(&format!("{prefix}.second_installment"), ITEM_26_ALERT);
            }
            if column.relief_items_open()
                && !(column.special_rate >= 0.0 && column.special_rate < 100.0)
            {
                err(
                    &format!("{prefix}.special_rate"),
                    "Enter the Item 16 rate in percent (0 to below 100).",
                );
            }
            for (field, value) in [
                ("gross_compensation", column.gross_compensation),
                ("non_taxable_compensation", column.non_taxable_compensation),
                ("sales", column.sales),
                ("sales_returns", column.sales_returns),
                ("cost_of_sales", column.cost_of_sales),
                ("itemized_deductions", column.itemized_deductions),
                (
                    "special_allowable_deductions",
                    column.special_allowable_deductions,
                ),
                ("nolco", column.nolco),
                ("non_operating_income", column.non_operating_income),
                ("eight_sales", column.eight_sales),
                ("eight_other_income", column.eight_other_income),
                ("prior_year_excess", column.prior_year_excess),
                ("quarterly_payments", column.quarterly_payments),
                ("cwt_q1_q3", column.cwt_q1_q3),
                ("cwt_q4", column.cwt_q4),
                ("cwt_2316", column.cwt_2316),
                ("previously_filed", column.previously_filed),
                ("foreign_tax_credits", column.foreign_tax_credits),
                ("special_tax_credits", column.special_tax_credits),
                ("other_credits", column.other_credits),
                ("share_of_other_agencies", column.share_of_other_agencies),
                ("second_installment", column.second_installment),
                ("surcharge", column.surcharge),
                ("interest", column.interest),
                ("compromise", column.compromise),
            ] {
                // wholenumber() lets only digits through; maxlength 12.
                if value < 0.0 || !has_whole_pesos(value) || value >= 1e12 {
                    err(
                        &format!("{prefix}.{field}"),
                        "Enter a whole, non-negative peso amount (at most 12 digits).",
                    );
                }
            }
        }
        if self.to_be_refunded && self.to_be_issued_tcc {
            err(
                "overpayment",
                "You cannot select BOTH 'To be refunded' and 'To be issued a Tax Credit Certificate'.",
            );
        }
        if self.to_be_carried_over {
            let max_prior = tp.prior_year_excess.max(sp.prior_year_excess);
            if self.carry_over_amount > max_prior {
                err(
                    "carry_over_amount",
                    "The value of 'To Be Carried' cannot be higher than the maximum of 'Prior Years No 1a' and 'Prior Years No 1b'.",
                );
            }
        }
        for (field, value) in [
            ("refund_amount", self.refund_amount),
            ("tcc_amount", self.tcc_amount),
            ("carry_over_amount", self.carry_over_amount),
        ] {
            if value < 0.0 || !has_cent_precision(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }

        // validationSpecify10A: every popup row needs a description and a
        // non-zero amount.
        for (prefix, column) in [("taxpayer", tp), ("spouse", sp)] {
            for (index, row) in column.itemized_rows.iter().enumerate() {
                if row.description.trim().is_empty() || row.amount == 0.0 {
                    err(
                        &format!("{prefix}.itemized_rows[{index}]"),
                        "Empty fields are not allowed",
                    );
                }
            }
        }

        // Derived items must be what the official compute chain produces.
        let mut expected = self.clone();
        expected.recompute();
        if expected.taxpayer != self.taxpayer
            || expected.spouse != self.spouse
            || expected.aggregate_amount_payable != self.aggregate_amount_payable
        {
            err(
                "aggregate_amount_payable",
                "Totals are out of date. Recompute the return.",
            );
        }

        errors
    }
}

impl FormValidator for Form1701MsDraft {
    /// `validate()` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1701MsDraft {
    /// The penalty items of each column on the shared penalty model
    /// ([`super::penalty_items`]): the taxpayer/filer's, then the spouse's,
    /// which apply only to a joint return.
    pub fn penalty_declarations() -> Vec<PenaltyItems<Self>> {
        let filer = super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                taxpayer.surcharge: "28A Surcharge",
                taxpayer.interest: "28B Interest",
                taxpayer.compromise: "28C Compromise",
            ],
            controls: [
                "frm1701MS:txtSurchangeNo28a",
                "frm1701MS:txtInterestNo28a",
                "frm1701MS:txtCompromise28a",
            ],
            state: taxpayer.penalty,
            basic_tax_due: "Item 27A", |d| d.taxpayer.amount_payable,
            party: Some("Taxpayer/Filer"),
            rounding: super::penalty_items::PenaltyRounding::WholePesos,
            deadline_period: |d: &Self| (!d.is_short_period && d.month == 12).then(|| super::penalty_items::calendar_period(d)).flatten(),
        );
        let spouse = super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                spouse.surcharge: "28A Surcharge",
                spouse.interest: "28B Interest",
                spouse.compromise: "28C Compromise",
            ],
            controls: [
                "frm1701MS:txtSurchangeNo28b",
                "frm1701MS:txtInterestNo28b",
                "frm1701MS:txtCompromise28b",
            ],
            state: spouse.penalty,
            basic_tax_due: "Item 27B", |d| d.spouse.amount_payable,
            party: Some("Spouse"),
            applies: |d: &Self| d.is_joint(),
            // The profile's EOPT class is the filer's, not the spouse's.
            rounding: super::penalty_items::PenaltyRounding::WholePesos,
            deadline_period: |d: &Self| (!d.is_short_period && d.month == 12).then(|| super::penalty_items::calendar_period(d)).flatten(),
        );
        vec![filer, spouse]
    }
}

impl QueueableForm for Form1701MsDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        Self::penalty_declarations()
    }
    const FORM_CODE: &'static str = "1701MS";
    /// Official `formType` (`1701MS`).
    const FORM_TYPE: &'static str = "1701MS";
    const LAYOUT_ID: &'static str = FORM_1701MS_FORM_ID;

    /// Aggregate Amount Payable (taxpayer and spouse).
    fn amount_payable(&self) -> Option<f64> {
        Some(self.aggregate_amount_payable)
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
        FilingPeriod::Annual
    }
    /// `txtMonthNo1 + txtYearNo1`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}{}", self.month, self.taxable_year)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let year: u16 = code.get(2..)?.parse().ok()?;
        (1..=12)
            .contains(&month)
            .then_some((year, FilingPeriod::Annual))
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

    /// Married, filing jointly. Taxpayer: mixed income, graduated rates, OSD.
    /// Spouse: compensation income only.
    pub(crate) fn sample() -> Form1701MsDraft {
        let mut draft = Form1701MsDraft {
            id: None,
            tin: "12345678800000".to_string(),
            taxable_year: 2025,
            month: 12,
            is_amended: false,
            is_short_period: false,
            civil_status: Form1701MsCivilStatus::Married,
            filing: Form1701MsFiling::Jointly,
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Dummy Taxpayer".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            contact_number: "09170000000".to_string(),
            spouse_info: Form1701MsSpouseInfo {
                tin: "12345678800000".to_string(),
                rdo_code: "039".to_string(),
                name: "Sample Dummy Spouse".to_string(),
                email: "sample.spouse@example.com".to_string(),
                contact_number: "09170000001".to_string(),
            },
            taxpayer: Form1701MsColumn {
                source: Form1701MsSource::Mixed,
                tax_option: Form1701MsTaxOption::Graduated,
                deduction: Form1701MsDeduction::Osd,
                gross_compensation: 900_000.5,
                non_taxable_compensation: 90_000.0,
                sales: 1_500_000.0,
                sales_returns: 10_000.0,
                cwt_2316: 80_000.0,
                quarterly_payments: 50_000.0,
                ..Form1701MsColumn::default()
            },
            spouse: Form1701MsColumn {
                source: Form1701MsSource::Compensation,
                gross_compensation: 600_000.0,
                non_taxable_compensation: 90_000.0,
                cwt_2316: 44_500.0,
                ..Form1701MsColumn::default()
            },
            foreign_tax_credits_description: String::new(),
            other_credits_description: String::new(),
            aggregate_amount_payable: 0.0,
            to_be_refunded: false,
            refund_amount: 0.0,
            to_be_issued_tcc: false,
            tcc_amount: 0.0,
            to_be_carried_over: false,
            carry_over_amount: 0.0,
            perjury_agreed: true,
            signatory_name: String::new(),
            authorized_representative: String::new(),
            lifecycle: SubmissionLifecycle::default(),
        };
        // This sample enters its penalties by hand.
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1701MsDraft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let d = sample();
        let tp = &d.taxpayer;
        assert_eq!(tp.gross_compensation, 900_001.0);
        assert_eq!(tp.taxable_compensation, 810_001.0);
        assert_eq!(tp.tax_on_compensation, 105_000.0);
        assert_eq!(tp.net_sales, 1_490_000.0);
        assert_eq!(tp.gross_income, 1_490_000.0);
        assert_eq!(tp.osd, 596_000.0);
        assert_eq!(tp.net_income, 894_000.0);
        assert_eq!(tp.taxable_business_income, 894_000.0);
        assert_eq!(tp.taxable_income, 1_704_001.0);
        assert_eq!(tp.tax_due_18b, 328_500.0);
        assert_eq!(tp.regular_income_tax, 328_500.0);
        assert_eq!(tp.total_credits, 130_000.0);
        assert_eq!(tp.tax_payable, 198_500.0);
        assert_eq!(tp.total_amount_payable, 198_500.0);
        let sp = &d.spouse;
        assert_eq!(sp.tax_option, Form1701MsTaxOption::Graduated);
        assert_eq!(sp.taxable_compensation, 510_000.0);
        assert_eq!(sp.tax_on_compensation, 44_500.0);
        assert_eq!(sp.regular_income_tax, 44_500.0);
        assert_eq!(sp.tax_payable, 0.0);
        assert_eq!(d.aggregate_amount_payable, 198_500.0);
        assert_eq!(tp.atc(), Some("II013"));
        assert_eq!(sp.atc(), Some("II011"));
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn eight_percent_and_special_rate_columns() {
        let mut d = sample();
        d.taxpayer = Form1701MsColumn {
            source: Form1701MsSource::Business,
            tax_option: Form1701MsTaxOption::EightPercent,
            eight_sales: 1_200_000.0,
            eight_other_income: 50_000.0,
            ..Form1701MsColumn::default()
        };
        d.recompute();
        let tp = &d.taxpayer;
        assert_eq!(tp.eight_total_income, 1_250_000.0);
        assert_eq!(tp.eight_exemption, 250_000.0);
        assert_eq!(tp.eight_tax_due, 80_000.0);
        assert_eq!(tp.regular_income_tax, 80_000.0);
        assert_eq!(tp.atc(), Some("II015"));
        assert_eq!(tp.deduction, Form1701MsDeduction::Unanswered);

        d.taxpayer = Form1701MsColumn {
            source: Form1701MsSource::Business,
            tax_option: Form1701MsTaxOption::Special,
            deduction: Form1701MsDeduction::Osd,
            sales: 2_000_000.0,
            cost_of_sales: 500_000.0,
            special_rate: 5.0,
            ..Form1701MsColumn::default()
        };
        d.recompute();
        let tp = &d.taxpayer;
        assert_eq!(tp.deduction, Form1701MsDeduction::Itemized);
        assert_eq!(tp.gross_income, 1_500_000.0);
        assert_eq!(tp.special_tax_due, 75_000.0);
        assert_eq!(tp.income_tax_due, 75_000.0);
        assert_eq!(tp.total_income_tax_due, 75_000.0);
        // Part VI: graduated tax on Item 14 less Item 17.
        assert_eq!(tp.tax_relief, 202_500.0);
    }

    #[test]
    fn field_map_uses_official_formats() {
        let d = sample();
        let fields = d.to_bir_field_map();
        assert_eq!(fields["frm1701MS:txtTaxpayerNo8a"], "SAMPLE DUMMY TAXPAYER");
        assert_eq!(fields["frm1701MS:txtTaxpayerNo11b"], "true");
        assert_eq!(fields["frm1701MS:txtTaxpayerNo11d1"], "true");
        assert_eq!(fields["frm1701MS:txtTaxpayerNo12b1"], "true");
        assert_eq!(fields["frm1701MS:txtTaxpayerNo17b"], "true");
        assert_eq!(fields["frm1701MS:txtSpouseNo17b"], "false");
        assert_eq!(fields["frm1701MS:perjuryClause"], "true");
        // 183 return cells plus the printed name and authorized representative.
        assert_eq!(fields.len(), 185);
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1701MS-122025#sample.taxpayer@example.com#.xml"
        );
        assert!(
            official_xml_write_ok(&d),
            "field map must follow the official layout"
        );
    }

    fn official_xml_write_ok(d: &Form1701MsDraft) -> bool {
        let layout = crate::official_xml::layout(FORM_1701MS_FORM_ID).unwrap();
        crate::official_xml::write(layout, &d.to_bir_field_map()).is_ok()
    }

    #[test]
    fn period_codes_round_trip() {
        let mut d = sample();
        assert_eq!(d.period_code(), "122025");
        assert_eq!(
            Form1701MsDraft::parse_period_code("122025"),
            Some((2025, FilingPeriod::Annual))
        );
        d.is_short_period = true;
        d.month = 6;
        d.recompute();
        assert_eq!(d.period_code(), "062025");
        assert_eq!(Form1701MsDraft::parse_period_code("132025"), None);
        assert_eq!(Form1701MsDraft::parse_period_code("122025Q1"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1701MsDraft), expected: &str| {
            let mut d = sample();
            mutate(&mut d);
            let found = messages(&d);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.taxable_year = 2026,
            "Future filing is not allowed. Please input correct year on Item 1",
        );
        check(
            &|d| d.taxable_year = 2027,
            "Future filing is not allowed. Please enter a valid year.",
        );
        check(
            &|d| {
                d.filing = Form1701MsFiling::Unanswered;
                d.recompute();
            },
            "Item 5 is required if Item 4 is marked as Married.",
        );
        check(
            &|d| d.spouse_info.tin = "12".into(),
            "Please enter a valid TIN number on Item 6 for Spouse",
        );
        check(
            &|d| d.spouse_info.rdo_code.clear(),
            "Please enter a valid RDO code for Spouse on Item 7",
        );
        check(
            &|d| d.spouse_info.name.clear(),
            "Please enter a valid Name for Spouse on Item 8",
        );
        check(
            &|d| d.spouse_info.email.clear(),
            "Please enter a valid Email Address for Spouse on Item 9",
        );
        check(
            &|d| d.spouse_info.contact_number.clear(),
            "Please enter a valid Contact Number for Spouse on Item 10",
        );
        check(
            &|d| {
                d.taxpayer.source = Form1701MsSource::Unanswered;
                d.recompute();
            },
            "Please select an option on Item 11 for Taxpayer",
        );
        check(
            &|d| {
                d.spouse.source = Form1701MsSource::Unanswered;
                d.recompute();
            },
            "Please select an option on Item 11 for Spouse",
        );
        check(
            &|d| {
                d.spouse.source = Form1701MsSource::Business;
                d.spouse.tax_option = Form1701MsTaxOption::Unanswered;
                d.recompute();
            },
            "Item 12 is required if Item 5 is marked as Jointly.",
        );
        check(
            &|d| {
                d.spouse.source = Form1701MsSource::Business;
                d.spouse.tax_option = Form1701MsTaxOption::Graduated;
                d.recompute();
            },
            ITEM_17_ALERT,
        );
        check(
            &|d| {
                d.taxpayer.deduction = Form1701MsDeduction::Unanswered;
                d.recompute();
            },
            ITEM_17_ALERT,
        );
        check(
            &|d| {
                d.taxpayer.cwt_2316 = 500_000.0;
                d.recompute();
            },
            "Item 31 is mandatory because Item 30 is has negative value.",
        );
        check(
            &|d| {
                d.taxpayer.cwt_2316 = 500_000.0;
                d.recompute();
                d.to_be_refunded = true;
                d.refund_amount = 10.0;
                d.recompute();
            },
            "The total amount in Item 31 must equal the sum of Item 27A and Item 27B (221,500).",
        );
        check(
            &|d| {
                d.taxpayer.cwt_2316 = 500_000.0;
                d.recompute();
                d.to_be_refunded = true;
                d.to_be_issued_tcc = true;
                d.refund_amount = 221_500.0;
                d.recompute();
            },
            "You cannot select BOTH 'To be refunded' and 'To be issued a Tax Credit Certificate'.",
        );
        check(
            &|d| {
                d.taxpayer.tax_option = Form1701MsTaxOption::Exempt;
                d.recompute();
            },
            "Please make sure to enter details on Item 13 to 16 on Taxpayer",
        );
        check(
            &|d| {
                d.taxpayer.tax_option = Form1701MsTaxOption::Exempt;
                d.taxpayer.legal_basis = "RA 0000".into();
                d.taxpayer.promotion_agency = "Sample Agency".into();
                d.taxpayer.registered_activity = "REG-1".into();
                d.taxpayer.effectivity_from = "12/31/2025".into();
                d.taxpayer.effectivity_to = "12/31/2030".into();
                d.recompute();
            },
            "Invalid date. Taxpayer Item 16 From date must be earlier than 12/31/2025.",
        );
        check(
            &|d| {
                d.spouse.source = Form1701MsSource::Business;
                d.spouse.tax_option = Form1701MsTaxOption::Special;
                d.spouse.legal_basis = "RA 0000".into();
                d.spouse.promotion_agency = "Sample Agency".into();
                d.spouse.registered_activity = "REG-1".into();
                d.spouse.effectivity_from = "01/01/2020".into();
                d.spouse.effectivity_to = "06/30/2025".into();
                d.recompute();
            },
            "Invalid date. Spouse Item 16 To date must be 12/31/2025 and onwards.",
        );
        check(
            &|d| {
                d.taxpayer.foreign_tax_credits = 1_000.0;
                d.recompute();
            },
            "Part V - Item 7 Foreign Tax Credits is mandatory if the amount column has value.",
        );
        check(
            &|d| d.foreign_tax_credits_description = "Sample country".into(),
            "Part V - Item 7 Foreign Tax Amount is mandatory if Foreign Tax Credits field has value.",
        );
        check(
            &|d| {
                d.taxpayer.other_credits = 1_000.0;
                d.recompute();
            },
            "Part V - Item 9 Other Credits/Payments is mandatory if the Amount column has value.",
        );
        check(
            &|d| d.other_credits_description = "Sample credit".into(),
            "Part V - Item 9 Amount is mandatory if the Other Credits/Payments field has value.",
        );
        check(
            &|d| d.tin = "12345".into(),
            "Please enter a valid TIN number on Item 6",
        );
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code.clear(),
            "Please enter a valid RDO code on Item 7",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 8",
        );
        check(
            &|d| d.email.clear(),
            "Please enter a valid Email Address on Item 9",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Contact Number on Item 10",
        );
        check(
            &|d| {
                d.taxpayer.tax_option = Form1701MsTaxOption::Unanswered;
                d.recompute();
            },
            "Taxpayer Item 12 is a required field ",
        );
        check(
            &|d| d.perjury_agreed = false,
            "You need to agree to the Perjury Clause.",
        );
        check(
            &|d| {
                d.taxpayer.second_installment = 170_000.0;
                d.recompute();
            },
            ITEM_26_ALERT,
        );
        check(
            &|d| {
                d.taxpayer = Form1701MsColumn {
                    source: Form1701MsSource::Business,
                    tax_option: Form1701MsTaxOption::EightPercent,
                    eight_sales: 3_000_001.0,
                    ..Form1701MsColumn::default()
                };
                d.recompute();
            },
            EIGHT_PERCENT_THRESHOLD_ALERT,
        );
    }

    #[test]
    fn print_map_adds_derived_values_and_never_touches_the_submit_map() {
        let draft = sample();
        let submit = draft.to_bir_field_map();
        let print = draft.to_print_field_map();
        for (key, value) in &submit {
            assert_eq!(print.get(key), Some(value), "{key}");
        }
        assert!(
            print
                .keys()
                .filter(|key| !submit.contains_key(*key))
                .all(|key| key.starts_with("derived:"))
        );
        assert_eq!(print["derived:tp_tin"], "123-456-788-00000");
        assert_eq!(print["derived:tin_digits"], "123456788");
        assert_eq!(print["derived:sp_name"], "SAMPLE DUMMY SPOUSE");
        assert_eq!(print["derived:tp_atc"], "II013");
        assert_eq!(
            print["derived:tp_atc_description"],
            "Mixed Income- Graduated IT Rates"
        );
        assert_eq!(print["derived:tp_p4_1"], "   900001");
        assert_eq!(print["derived:sp_p4_1"], "   600000");
        assert_eq!(print["derived:tp_p4_5"], "  1500000");
        assert!(!print.contains_key("derived:tp_rate_whole"));
    }

    #[test]
    fn print_amounts_right_align_digits_and_bracket_overpayments() {
        assert_eq!(print_whole_amount(0.0), "        0");
        assert_eq!(print_whole_amount(123_456_789.0), "123456789");
        assert_eq!(print_whole_amount(-4_500.0), "   (4500)");
        assert_eq!(print_cents_amount(-1_234.5), "  1234.50");
        assert_eq!(print_rate(5.0), ("5".to_string(), "0".to_string()));
        assert_eq!(print_rate(7.5), ("7".to_string(), "5".to_string()));
    }

    #[test]
    fn print_map_omits_the_spouse_column_on_a_separate_return() {
        let mut draft = sample();
        draft.filing = Form1701MsFiling::Separately;
        draft.recompute();
        let print = draft.to_print_field_map();
        assert!(!print.keys().any(|key| key.starts_with("derived:sp_")));
        assert!(print.contains_key("derived:tp_p2_29"));
    }

    #[test]
    fn spouse_columns_follow_item_5() {
        let mut d = sample();
        d.filing = Form1701MsFiling::Separately;
        d.recompute();
        assert_eq!(d.spouse, Form1701MsColumn::default());
        assert_eq!(d.aggregate_amount_payable, d.taxpayer.total_amount_payable);
        let fields = d.to_bir_field_map();
        assert_eq!(fields["frm1701MS:txtTaxpayerNo11d1"], "false");
        assert_eq!(fields["frm1701MS:txtIfMarriedSeparatelyNo5"], "true");
        d.civil_status = Form1701MsCivilStatus::Single;
        d.recompute();
        assert_eq!(d.filing, Form1701MsFiling::Unanswered);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        // Queueing validates against today's date; the sample is tax year 2025.
        let mut d = sample();
        d.queue(crate::filing_queue::QueueAuthSource::Gui).unwrap();
        assert!(d.revalidate_queued_before_submission().is_ok());
        d.perjury_agreed = false;
        assert!(d.revalidate_queued_before_submission().is_err());
    }

    #[test]
    fn schedule_10a_rows_total_into_item_10a_and_need_both_fields() {
        let mut d = sample();
        d.taxpayer.deduction = Form1701MsDeduction::Itemized;
        d.taxpayer.itemized_rows = vec![
            Form1701MsScheduleRow {
                description: "Rent".into(),
                amount: 100_000.4,
            },
            Form1701MsScheduleRow {
                description: "Utilities".into(),
                amount: 20_000.5,
            },
        ];
        d.recompute();
        assert_eq!(d.taxpayer.itemized_deductions, 120_001.0);
        assert!(
            !messages(&d)
                .iter()
                .any(|m| m == "Empty fields are not allowed")
        );
        d.taxpayer.itemized_rows[1].description.clear();
        assert!(
            messages(&d)
                .iter()
                .any(|m| m == "Empty fields are not allowed")
        );
        d.taxpayer.itemized_rows[1].description = "Utilities".into();
        d.taxpayer.itemized_rows[0].amount = 0.0;
        d.recompute();
        assert!(
            messages(&d)
                .iter()
                .any(|m| m == "Empty fields are not allowed")
        );
        // OSD closes Item 10A and drops the rows.
        d.taxpayer.deduction = Form1701MsDeduction::Osd;
        d.recompute();
        assert!(d.taxpayer.itemized_rows.is_empty());
    }

    /// N07: the Item 31 amounts are the filer's entries. The page writes them
    /// through the agent bindings, they reach the draft and the official box,
    /// and `compute` still clears them once the overpayment is gone.
    #[test]
    fn item_31_amounts_are_typed_by_the_filer_and_cleared_without_overpayment() {
        use crate::forms::agent::{self, AgentFormKind};
        let kind = AgentFormKind::from_code("1701MS").unwrap();
        let mut d = sample();
        d.taxpayer.cwt_2316 = 500_000.0;
        d.recompute();
        d.to_be_refunded = true;
        let mut value = serde_json::to_value(&d).unwrap();
        agent::fill_as_user(
            kind,
            &mut value,
            "frm1701MS:txtToBeRefunded1",
            &serde_json::json!("221,500.00"),
        )
        .unwrap();
        let mut d: Form1701MsDraft = serde_json::from_value(value).unwrap();
        d.recompute();
        assert_eq!(d.refund_amount, 221_500.0);
        assert_eq!(
            d.to_bir_field_map()["frm1701MS:txtToBeRefunded1"],
            "221,500.00"
        );
        assert!(
            !d.validate_on(today())
                .iter()
                .any(|(_, m)| m.contains("Item 31")),
            "{:?}",
            d.validate_on(today())
        );
        d.taxpayer.cwt_2316 = 0.0;
        d.recompute();
        assert_eq!(d.refund_amount, 0.0);
    }

    /// P5-1: the earlier float-tolerance check refused two-decimal
    /// amounts above about 2,000,000 (`8_816_764.47 * 100` lands 1.2e-7
    /// off a whole number); the shared `official_inputs` check accepts
    /// them again, and a three-decimal amount is still refused.
    #[test]
    fn large_two_decimal_amounts_pass_cent_precision() {
        let mut draft = sample();
        draft.refund_amount = 8_816_764.47;
        assert!(
            <Form1701MsDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "refund_amount"),
            "a large two-decimal amount must be accepted"
        );
        draft.refund_amount = 1.005;
        assert!(
            <Form1701MsDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "refund_amount"),
            "a three-decimal amount must still be refused"
        );
    }
}
