//! BIR Form 1702-EX (January 2018) — Annual Income Tax Return for
//! Corporations, Partnerships and Other Non-Individual Taxpayers Exempt from
//! Income Tax.
//!
//! Ported from the official `BIR-Form1702EXv2018C.hta` (eBIRForms
//! 7.9.6.2.1): the compute chain (`FormatAmount`, `amountComputation`,
//! `compTaxReliefAvail`), the radio rules (`checkFilingYear`,
//! `Select_rdoPg1I5ATC`, `disableForOSD` / `enableForItemized`),
//! `validateAll()` with its exact alert texts, and `saveXMLsubmit` through
//! [`crate::official_xml`].
//!
//! Five "(more...)" popups extend the last line of a schedule: Schedule 1
//! Item 17I, Schedule 2 Item 4 and Schedule 3 Items 3, 6 and 8. Their rows
//! are text boxes inside `frmMain` (`frm1702EX:txtPg3Pt6<popup>_<n>Col<k>`),
//! so the upload loop writes them where the DOM has them: ahead of
//! `Pg3Pt6<popup>SubTotal`. Once the popup holds two or more rows the fixed
//! line reads `OTHERS` with the popup subtotal, `Pg3Pt6<popup>PopLength`
//! holds the row count, and every popup amount is whole pesos (`toComma`
//! drops the centavos). A row list longer than its fixed lines is filed
//! that way: the last fixed line and every later row are popup rows.
//!
//! Amount entries keep centavos (`round(this)`); computed items are
//! `toFixed(0)` whole pesos. The submit loop strips commas from (and turns
//! parentheses into a minus sign in) controls with `maxLength` 12 or 15; on
//! this page that is only the two signatory title controls, which the page
//! sets to `0` at load.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::form_2551q::TaxPeriodBasis;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1702EX_FORM_ID: &str = "1702ex-v2018c";
/// Schedule 1 Items 17D–17I ("others").
pub const FORM_1702EX_OTHER_DEDUCTION_ROWS: usize = 6;
/// Schedule 2 Items 1–4.
pub const FORM_1702EX_SPECIAL_DEDUCTION_ROWS: usize = 4;
/// Schedule 3 lines per group (Items 2–3, 5–6, 7–8).
pub const FORM_1702EX_RECONCILIATION_ROWS: usize = 2;
/// Rows this model accepts per list, popup rows included.
pub const FORM_1702EX_MAX_ROWS: usize = 100;
/// The fixed line's text once its popup holds two or more rows.
const OTHERS: &str = "OTHERS";

/// One "(more...)" popup: the `Pg3Pt6<id>` controls, the item prefix of
/// its row numbers (`CheckEmptyDesc`), the fixed lines of its list and
/// whether its amount boxes have `maxLength` 12 (the upload loop strips
/// their commas).
struct Popup {
    id: &'static str,
    number: &'static str,
    fixed: usize,
    plain_amounts: bool,
}

const POPUP_S1I17: Popup = Popup {
    id: "S1I17",
    number: "17.",
    fixed: FORM_1702EX_OTHER_DEDUCTION_ROWS,
    plain_amounts: false,
};
const POPUP_S2I4: Popup = Popup {
    id: "S2I4",
    number: "4.",
    fixed: FORM_1702EX_SPECIAL_DEDUCTION_ROWS,
    plain_amounts: false,
};
const POPUP_S3I3: Popup = Popup {
    id: "S3I3",
    number: "3.",
    fixed: FORM_1702EX_RECONCILIATION_ROWS,
    plain_amounts: true,
};
const POPUP_S3I6: Popup = Popup {
    id: "S3I6",
    number: "6.",
    fixed: FORM_1702EX_RECONCILIATION_ROWS,
    plain_amounts: false,
};
const POPUP_S3I8: Popup = Popup {
    id: "S3I8",
    number: "8.",
    fixed: FORM_1702EX_RECONCILIATION_ROWS,
    plain_amounts: false,
};

/// The rows a popup holds: the last fixed line and every later row, once
/// the list is longer than its fixed lines.
fn popup_slice<T>(rows: &[T], fixed: usize) -> &[T] {
    if rows.len() > fixed {
        &rows[fixed - 1..]
    } else {
        &[]
    }
}

/// The rows on fixed lines before the popup line (all rows when unfolded).
fn fixed_slice<T>(rows: &[T], fixed: usize) -> &[T] {
    if rows.len() > fixed {
        &rows[..fixed - 1]
    } else {
        rows
    }
}

/// A popup amount as `toComma` leaves it: centavos dropped, a negative
/// entry becomes 0.
fn popup_amount(amount: f64) -> f64 {
    if amount < 0.0 { 0.0 } else { amount.trunc() }
}

/// Item 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1702ExAtc {
    #[default]
    Unanswered,
    /// IC 011 Exempt corporation on exempt activities (itemized only).
    IC011,
    /// IC 021 General professional partnership.
    IC021,
}

/// Item 13.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1702ExDeduction {
    #[default]
    Unanswered,
    Itemized,
    Osd,
}

/// The overpayment boxes after Item 22.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1702ExOverpayment {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
    CarryOver,
}

/// A description and amount row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1702ExRow {
    pub description: String,
    pub amount: f64,
}

/// A Schedule 2 row: description, legal basis and amount.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1702ExSpecialRow {
    pub description: String,
    pub legal_basis: String,
    pub amount: f64,
}

/// Schedule 1 — ordinary allowable itemized deductions, Items 1–17C.
pub const FORM_1702EX_ORDINARY_ITEMS: [(&str, &str); 19] = [
    ("txtPg3Pt6S1I1Ammortization", "1 — Amortizations"),
    ("txtPg3Pt6S1I2BadDebts", "2 — Bad debts"),
    (
        "txtPg3Pt6S1I3CharitableContrib",
        "3 — Charitable and other contributions",
    ),
    ("txtPg3Pt6S1I4Depletion", "4 — Depletion"),
    ("txtPg3Pt6S1I5Depreciation", "5 — Depreciation"),
    (
        "txtPg3Pt6S1I6EntertainmentAmuseRecreate",
        "6 — Entertainment, amusement and recreation",
    ),
    ("txtPg3Pt6S1I7FringeBenefits", "7 — Fringe benefits"),
    ("txtPg3Pt6S1I8Interest", "8 — Interest"),
    ("txtPg3Pt6S1I9Losses", "9 — Losses"),
    ("txtPg3Pt6S1I10PensionTrust", "10 — Pension trusts"),
    ("txtPg3Pt6S1I11Rental", "11 — Rental"),
    ("txtPg3Pt6S1I12RnD", "12 — Research and development"),
    (
        "txtPg3Pt6S1I13SalaryWageAllowance",
        "13 — Salaries, wages and allowances",
    ),
    (
        "txtPg3Pt6S1I14SssGsisPHealthHDMFOthers",
        "14 — SSS, GSIS, PhilHealth, HDMF and other contributions",
    ),
    ("txtPg3Pt6S1I15TaxAndLicense", "15 — Taxes and licenses"),
    (
        "txtPg3Pt6S1I16TranspoAndTravel",
        "16 — Transportation and travel",
    ),
    (
        "txtPg3Pt6S1I17aJanitorAndMessengerServ",
        "17A — Janitorial and messengerial services",
    ),
    ("txtPg3Pt6S1I17bProfessionalFee", "17B — Professional fees"),
    ("txtPg3Pt6S1I17cSecurityServices", "17C — Security services"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1702ExDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–5
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub tax_period_basis: TaxPeriodBasis,
    /// Item 2 month (1–12). Calendar filers use 12 unless a short period.
    pub year_end_month: u8,
    /// Item 2 year (four digits; the page shows `20YY`).
    pub taxable_year: u16,
    pub is_amended: bool,
    pub is_short_period: bool,
    #[serde(default)]
    pub atc: Form1702ExAtc,

    // Part I
    pub rdo_code: String,
    pub registered_name: String,
    pub registered_address: String,
    pub zip_code: String,
    /// Item 10, `MM/DD/YYYY`.
    #[serde(default)]
    pub date_of_incorporation: String,
    pub contact_number: String,
    pub email: String,
    #[serde(default)]
    pub deduction: Form1702ExDeduction,
    /// Items 14–17.
    #[serde(default)]
    pub legal_basis: String,
    #[serde(default)]
    pub promotion_agency: String,
    #[serde(default)]
    pub registered_activity: String,
    #[serde(default)]
    pub effectivity_from: String,
    #[serde(default)]
    pub effectivity_to: String,
    pub line_of_business: String,

    // Part II
    #[serde(default)]
    pub tax_due: f64,
    #[serde(default)]
    pub total_credits: f64,
    #[serde(default)]
    pub net_payable: f64,
    /// Item 21.
    #[serde(default)]
    pub penalties: f64,
    #[serde(default)]
    pub total_amount_payable: f64,
    #[serde(default)]
    pub overpayment: Form1702ExOverpayment,
    /// Item 23.
    #[serde(default)]
    pub number_of_attachments: u8,

    // Part IV
    #[serde(default)]
    pub sales: f64,
    #[serde(default)]
    pub sales_returns: f64,
    #[serde(default)]
    pub net_sales: f64,
    #[serde(default)]
    pub cost_of_sales: f64,
    #[serde(default)]
    pub gross_income: f64,
    #[serde(default)]
    pub other_income: f64,
    #[serde(default)]
    pub total_gross_income: f64,
    #[serde(default)]
    pub ordinary_deductions: f64,
    #[serde(default)]
    pub special_deductions: f64,
    #[serde(default)]
    pub total_itemized: f64,
    #[serde(default)]
    pub osd: f64,
    #[serde(default)]
    pub net_taxable_income: f64,
    /// Item 40 in percent.
    #[serde(default)]
    pub tax_rate: f64,
    #[serde(default)]
    pub prior_year_excess: f64,
    #[serde(default)]
    pub quarterly_payments: f64,
    #[serde(default)]
    pub cwt_previous_quarters: f64,
    #[serde(default)]
    pub cwt_q4: f64,
    #[serde(default)]
    pub foreign_tax_credits: f64,
    #[serde(default)]
    pub previously_filed: f64,
    /// Items 48 and 49.
    #[serde(default)]
    pub other_credits: Vec<Form1702ExRow>,

    // Part V
    #[serde(default)]
    pub regular_income_tax: f64,
    #[serde(default)]
    pub special_allowable_relief: f64,
    #[serde(default)]
    pub tax_relief: f64,

    // Part VI
    /// Schedule 1 Items 1–17C in [`FORM_1702EX_ORDINARY_ITEMS`] order.
    #[serde(default)]
    pub ordinary_items: [f64; 19],
    /// Schedule 1 Items 17D–17I.
    #[serde(default)]
    pub other_deductions: Vec<Form1702ExRow>,
    /// Schedule 2 Items 1–4.
    #[serde(default)]
    pub special_rows: Vec<Form1702ExSpecialRow>,
    /// Schedule 3.
    #[serde(default)]
    pub net_income_per_books: f64,
    #[serde(default)]
    pub non_deductible: Vec<Form1702ExRow>,
    #[serde(default)]
    pub reconciliation_total: f64,
    #[serde(default)]
    pub non_taxable_income: Vec<Form1702ExRow>,
    #[serde(default)]
    pub special_deductions_s3: Vec<Form1702ExRow>,
    #[serde(default)]
    pub reconciliation_less: f64,
    #[serde(default)]
    pub reconciled_taxable_income: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `toFixed(0)`.
fn fixed0(value: f64) -> f64 {
    if value.is_finite() {
        value.round()
    } else {
        0.0
    }
}

/// `round(this)` / `formatCurrency`: the value an entry holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
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

/// `capitalize()`: upper case, trimmed.
fn capitalize(value: &str) -> String {
    value.trim().to_uppercase()
}

/// Split `text` into pieces of at most `width` characters, as `loadBGData`
/// fills the name (60) and address (80) lines.
fn split_lines(text: &str, width: usize) -> [String; 3] {
    let chars: Vec<char> = text.chars().collect();
    let piece = |from: usize, to: usize| -> String {
        chars
            .get(from.min(chars.len())..to.min(chars.len()))
            .map(|c| c.iter().collect())
            .unwrap_or_default()
    };
    if chars.len() > width * 2 {
        [
            piece(0, width),
            piece(width, width * 2),
            piece(width * 2, chars.len()),
        ]
    } else if chars.len() > width {
        [piece(0, width), piece(width, chars.len()), String::new()]
    } else {
        [text.to_string(), String::new(), String::new()]
    }
}

/// `validateDate`: an `MM/DD/YYYY` date, year 1800 or later.
fn parse_form_date(text: &str) -> Option<NaiveDate> {
    let parts: Vec<&str> = text.split('/').collect();
    if parts.len() != 3 || parts[0].len() != 2 || parts[1].len() != 2 || parts[2].len() != 4 {
        return None;
    }
    let date = NaiveDate::parse_from_str(text, "%m/%d/%Y").ok()?;
    (date.year() >= 1800).then_some(date)
}

/// Split `text` over the sheet's comb lines of the given widths.
fn print_lines(text: &str, widths: &[usize]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut from = 0;
    widths
        .iter()
        .map(|width| {
            let to = (from + width).min(chars.len());
            let line: String = chars[from.min(chars.len())..to].iter().collect();
            from = to;
            line
        })
        .collect()
}

/// A whole-peso amount (50 centavos or more round up) right-aligned in a
/// 12-slot comb; a negative amount prints in parentheses.
fn print_whole_pesos(amount: f64) -> String {
    let pesos = (amount.abs() + 0.5 + 1e-9).floor();
    let text = if amount < 0.0 && pesos > 0.0 {
        format!("({pesos:.0})")
    } else {
        format!("{pesos:.0}")
    };
    format!("{text:>12}")
}

/// The amount controls the frozen sheet prints, each with the description
/// control of its row (a row with neither prints blank). Items 18 and 41 are
/// left out: the freeze preprints `000` in the last three slots of both.
fn print_amount_keys() -> Vec<(String, Option<String>)> {
    let fixed = [
        "txtPg1Pt2I19TotalTaxCrPmt",
        "txtPg1Pt2I20TotalOverpmt",
        "txtPg1Pt2I21PenaltyCompromise",
        "txtPg1Pt2I22TotalAmtPayable",
        "txtPg2Pt4I28SalesReceiptsRevFees",
        "txtPg2Pt4I29SalesRetAllowanceDisc",
        "txtPg2Pt4I30NetSalesReceiptsRevFees",
        "txtPg2Pt4I31CostOfSalesServ",
        "txtPg2Pt4I32GrossIncomeFromOper",
        "txtPg2Pt4I33AddOther",
        "txtPg2Pt4I34TotalGross",
        "txtPg2Pt4I35OrdinaryAllowable",
        "txtPg2Pt4I36SpecialAllowable",
        "txtPg2Pt4I37TotalItemized",
        "txtPg2Pt4I38OptionalStandardDeduc",
        "txtPg2Pt4I39NetTaxable",
        "txtPg2Pt4I42PriorYearExcessCr",
        "txtPg2Pt4I43IncomeTaxPmtFromPreviousQrt",
        "txtPg2Pt4I44CreditableTaxWithheldFromPrevQrt",
        "txtPg2Pt4I45CreditableTaxWithheldFor4thQrt",
        "txtPg2Pt4I46ForeignTaxCr",
        "txtPg2Pt4I47TaxPaidInReturnPrevFiled",
        "txtPg2Pt4I50TotalTaxCrPmt",
        "txtPg2Pt4I51TotalOverpayment",
        "txtPg2Pt5I52RegularIncomeOtherwiseDue",
        "txtPg2Pt5I53SpecialAllowableItemizedDeduc",
        "txtPg2Pt5I54TotalTaxReliefAvailment",
        "txtPg3Pt6S1I18TotOrdinaryAllowableItemDeduc",
        "txtPg3Pt6S2I5TotSpecialAllowedItemDeduc",
        "txtPg3Pt6S3I1NetIncomePerBook",
        "txtPg3Pt6S3I4Total",
        "txtPg3Pt6S3I9Total",
        "txtPg3Pt6S3I10NetTaxableIncome",
    ];
    let mut keys: Vec<(String, Option<String>)> = fixed
        .iter()
        .chain(FORM_1702EX_ORDINARY_ITEMS.iter().map(|(key, _)| key))
        .map(|key| (key.to_string(), None))
        .collect();
    for item in ["48", "49"] {
        keys.push((
            format!("txtPg2Pt4I{item}OtherTaxCrPmtAmt"),
            Some(format!("txtPg2Pt4I{item}OtherTaxCrPmtDesc")),
        ));
    }
    for letter in ["D", "E", "F", "G", "H", "I"] {
        keys.push((
            format!("txtPg3Pt6I17Others{letter}Amt"),
            Some(format!("txtPg3Pt6I17Others{letter}Desc")),
        ));
    }
    for n in 1..=FORM_1702EX_SPECIAL_DEDUCTION_ROWS {
        keys.push((
            format!("txtPg3Pt6S2I{n}Amount"),
            Some(format!("txtPg3Pt6S2I{n}Description")),
        ));
    }
    for (item, stem) in [
        (2, "NonDeductExpenseOtherIncome"),
        (3, "NonDeductExpenseOtherIncome"),
        (5, "NonTaxIncomeAndIncomeSubjectToFinTax"),
        (6, "NonTaxIncomeAndIncomeSubjectToFinTax"),
        (7, "SpecialDeduct"),
        (8, "SpecialDeduct"),
    ] {
        keys.push((
            format!("txtPg3Pt6S3I{item}{stem}Amt"),
            Some(format!("txtPg3Pt6S3I{item}{stem}Desc")),
        ));
    }
    keys
}

/// `true` when a row has content.
fn row_used(row: &Form1702ExRow) -> bool {
    !row.description.trim().is_empty() || row.amount != 0.0
}

fn trim_rows(rows: &mut Vec<Form1702ExRow>, limit: usize) {
    rows.truncate(limit);
    for row in rows.iter_mut() {
        row.description = capitalize(&row.description);
        row.amount = cents(row.amount);
    }
    while rows.last().is_some_and(|row| !row_used(row)) {
        rows.pop();
    }
}

/// Popup rows keep whole pesos (`FormatValue` on load, `toComma` on entry).
fn whole_popup_rows(rows: &mut [Form1702ExRow], fixed: usize) {
    if rows.len() > fixed {
        for row in &mut rows[fixed - 1..] {
            row.amount = popup_amount(row.amount);
        }
    }
}

impl Form1702ExDraft {
    pub const FORM_CODE: &'static str = "1702EX";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16) -> Self {
        let mut draft = Self {
            id: None,
            tin: profile.tin.full(),
            tax_period_basis: TaxPeriodBasis::Calendar,
            year_end_month: 12,
            taxable_year: year,
            is_amended: false,
            is_short_period: false,
            atc: Form1702ExAtc::Unanswered,
            rdo_code: profile.rdo_code.clone(),
            registered_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            date_of_incorporation: profile
                .business_start_date
                .map(|d| d.format("%m/%d/%Y").to_string())
                .unwrap_or_default(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            deduction: Form1702ExDeduction::Unanswered,
            legal_basis: String::new(),
            promotion_agency: String::new(),
            registered_activity: String::new(),
            effectivity_from: String::new(),
            effectivity_to: String::new(),
            line_of_business: profile.line_of_business.clone(),
            tax_due: 0.0,
            total_credits: 0.0,
            net_payable: 0.0,
            penalties: 0.0,
            total_amount_payable: 0.0,
            overpayment: Form1702ExOverpayment::None,
            number_of_attachments: 0,
            sales: 0.0,
            sales_returns: 0.0,
            net_sales: 0.0,
            cost_of_sales: 0.0,
            gross_income: 0.0,
            other_income: 0.0,
            total_gross_income: 0.0,
            ordinary_deductions: 0.0,
            special_deductions: 0.0,
            total_itemized: 0.0,
            osd: 0.0,
            net_taxable_income: 0.0,
            tax_rate: 0.0,
            prior_year_excess: 0.0,
            quarterly_payments: 0.0,
            cwt_previous_quarters: 0.0,
            cwt_q4: 0.0,
            foreign_tax_credits: 0.0,
            previously_filed: 0.0,
            other_credits: Vec::new(),
            regular_income_tax: 0.0,
            special_allowable_relief: 0.0,
            tax_relief: 0.0,
            ordinary_items: [0.0; 19],
            other_deductions: Vec::new(),
            special_rows: Vec::new(),
            net_income_per_books: 0.0,
            non_deductible: Vec::new(),
            reconciliation_total: 0.0,
            non_taxable_income: Vec::new(),
            special_deductions_s3: Vec::new(),
            reconciliation_less: 0.0,
            reconciled_taxable_income: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 1: calendar years end in December unless a short period.
    pub fn set_tax_period_basis(&mut self, basis: TaxPeriodBasis) {
        self.tax_period_basis = basis;
        if basis == TaxPeriodBasis::Calendar && !self.is_short_period {
            self.year_end_month = 12;
        }
    }

    /// Items 47 only on an amended return.
    pub fn previously_filed_open(&self) -> bool {
        self.is_amended
    }

    /// Schedules 1 and 2 are open only under itemized deductions.
    pub fn itemized_open(&self) -> bool {
        self.deduction != Form1702ExDeduction::Osd
    }

    /// The overpayment boxes are open only while Item 20 is negative.
    pub fn overpayment_open(&self) -> bool {
        self.net_payable < 0.0
    }

    /// The radio rules, then the official compute chain.
    pub fn recompute(&mut self) {
        if self.tax_period_basis == TaxPeriodBasis::Calendar && !self.is_short_period {
            self.year_end_month = 12;
        }
        if self.atc == Form1702ExAtc::IC011 {
            // Select_rdoPg1I5ATC: IC 011 allows itemized deductions only.
            self.deduction = Form1702ExDeduction::Itemized;
        }
        if !self.is_amended {
            self.previously_filed = 0.0;
        }
        self.legal_basis = capitalize(&self.legal_basis);
        self.promotion_agency = capitalize(&self.promotion_agency);
        self.registered_activity = capitalize(&self.registered_activity);
        for value in [
            &mut self.sales,
            &mut self.sales_returns,
            &mut self.cost_of_sales,
            &mut self.other_income,
            &mut self.tax_rate,
            &mut self.prior_year_excess,
            &mut self.quarterly_payments,
            &mut self.cwt_previous_quarters,
            &mut self.cwt_q4,
            &mut self.foreign_tax_credits,
            &mut self.previously_filed,
            &mut self.regular_income_tax,
            &mut self.special_allowable_relief,
            &mut self.penalties,
            &mut self.net_income_per_books,
        ] {
            *value = cents(*value);
        }
        for value in &mut self.ordinary_items {
            *value = cents(*value);
        }
        trim_rows(&mut self.other_credits, 2);
        for (rows, fixed) in [
            (&mut self.other_deductions, FORM_1702EX_OTHER_DEDUCTION_ROWS),
            (&mut self.non_deductible, FORM_1702EX_RECONCILIATION_ROWS),
            (
                &mut self.non_taxable_income,
                FORM_1702EX_RECONCILIATION_ROWS,
            ),
            (
                &mut self.special_deductions_s3,
                FORM_1702EX_RECONCILIATION_ROWS,
            ),
        ] {
            trim_rows(rows, FORM_1702EX_MAX_ROWS);
            whole_popup_rows(rows, fixed);
        }
        self.special_rows.truncate(FORM_1702EX_MAX_ROWS);
        for row in &mut self.special_rows {
            row.description = capitalize(&row.description);
            row.legal_basis = capitalize(&row.legal_basis);
            row.amount = cents(row.amount);
        }
        while self.special_rows.last().is_some_and(|row| {
            row.description.is_empty() && row.legal_basis.is_empty() && row.amount == 0.0
        }) {
            self.special_rows.pop();
        }
        if self.special_rows.len() > FORM_1702EX_SPECIAL_DEDUCTION_ROWS {
            for row in &mut self.special_rows[FORM_1702EX_SPECIAL_DEDUCTION_ROWS - 1..] {
                row.amount = popup_amount(row.amount);
            }
        }
        if !self.itemized_open() {
            // disableForOSD resets Schedules 1 and 2.
            self.ordinary_items = [0.0; 19];
            self.other_deductions.clear();
            self.special_rows.clear();
        }

        // Part IV ("totalGrossIncome").
        self.net_sales = fixed0(self.sales - self.sales_returns);
        self.gross_income = fixed0(self.net_sales - self.cost_of_sales);
        self.total_gross_income = fixed0(self.gross_income + self.other_income);
        // Schedules 1 and 2.
        let ordinary: f64 = self.ordinary_items.iter().sum::<f64>()
            + self.other_deductions.iter().map(|r| r.amount).sum::<f64>();
        let schedule1 = fixed0(ordinary);
        let schedule2 = fixed0(self.special_rows.iter().map(|r| r.amount).sum::<f64>());
        match self.deduction {
            Form1702ExDeduction::Osd => {
                self.ordinary_deductions = 0.0;
                self.special_deductions = 0.0;
                self.osd = fixed0(self.total_gross_income * 0.4);
                self.net_taxable_income = fixed0(self.total_gross_income - self.osd);
            }
            Form1702ExDeduction::Itemized => {
                self.ordinary_deductions = schedule1;
                self.special_deductions = schedule2;
                self.osd = 0.0;
                self.net_taxable_income =
                    fixed0(self.total_gross_income - fixed0(schedule1 + schedule2));
            }
            Form1702ExDeduction::Unanswered => {
                self.ordinary_deductions = schedule1;
                self.special_deductions = schedule2;
                self.osd = fixed0(self.total_gross_income * 0.4);
                self.net_taxable_income = 0.0;
            }
        }
        self.total_itemized = fixed0(self.ordinary_deductions + self.special_deductions);
        self.tax_due = fixed0(self.net_taxable_income * self.tax_rate / 100.0);
        self.total_credits = fixed0(
            self.prior_year_excess
                + self.quarterly_payments
                + self.cwt_previous_quarters
                + self.cwt_q4
                + self.foreign_tax_credits
                + self.previously_filed
                + self.other_credits.iter().map(|r| r.amount).sum::<f64>(),
        );
        self.net_payable = fixed0(self.tax_due - self.total_credits);
        // Part V.
        self.tax_relief = fixed0(self.regular_income_tax + self.special_allowable_relief);
        // Part II.
        self.total_amount_payable = if self.net_payable > -1.0 {
            fixed0(self.net_payable + self.penalties)
        } else if self.penalties > 0.0 {
            self.penalties
        } else {
            self.net_payable
        };
        if !self.overpayment_open() {
            self.overpayment = Form1702ExOverpayment::None;
        }
        // Schedule 3.
        self.reconciliation_total = cents(
            self.net_income_per_books + self.non_deductible.iter().map(|r| r.amount).sum::<f64>(),
        );
        self.reconciliation_less = fixed0(
            self.non_taxable_income
                .iter()
                .map(|r| r.amount)
                .sum::<f64>()
                + self
                    .special_deductions_s3
                    .iter()
                    .map(|r| r.amount)
                    .sum::<f64>(),
        );
        self.reconciled_taxable_income =
            fixed0(self.reconciliation_total - self.reconciliation_less);
    }

    /// The official field values the upload loop reads, keyed by element
    /// id, popup rows included.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.layout_fields();
        for popup in Self::POPUPS {
            fields.extend(self.popup_controls(popup));
        }
        fields
    }

    /// The generated layout with each popup's row controls spliced in where
    /// the DOM has them: ahead of `Pg3Pt6<id>SubTotal`.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        use super::official_inputs::{RowInsertion, extend_layout};
        use crate::official_xml::Entry;
        let base = crate::official_xml::layout(FORM_1702EX_FORM_ID).map_err(|e| e.to_string())?;
        let key_of = |entry: &Entry| match entry {
            Entry::Bool { key, .. } | Entry::Value { key, .. } => key.clone(),
        };
        // A plain upper-cased text control with the page's separator.
        let template = "frm1702EX:txtMaxPage".to_string();
        let mut insertions = Vec::new();
        for popup in Self::POPUPS {
            let controls = self.popup_controls(popup);
            if controls.is_empty() {
                continue;
            }
            let anchor = format!("Pg3Pt6{}SubTotal", popup.id);
            let at = base
                .entries
                .iter()
                .position(|entry| key_of(entry) == anchor)
                .filter(|at| *at > 0)
                .ok_or_else(|| format!("no anchor entry {anchor}"))?;
            insertions.push(RowInsertion {
                after: key_of(&base.entries[at - 1]),
                copies: controls
                    .into_iter()
                    .map(|(key, _)| (template.clone(), key))
                    .collect(),
            });
        }
        extend_layout(base, &insertions)
    }

    /// The controls of the fixed official layout.
    fn layout_fields(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1702EX:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let money = official_amount;

        let calendar = self.tax_period_basis == TaxPeriodBasis::Calendar;
        put("rdoPg1I1Calendar", flag(calendar));
        put("rdoPg1I1Fiscal", flag(!calendar));
        put("ddlPg1I2Date", format!("{:02}", self.year_end_month));
        put("txtPg1I2YearEnd", format!("{:02}", self.taxable_year % 100));
        put("rdoPg1I3AmendedYes", flag(self.is_amended));
        put("rdoPg1I3AmendedNo", flag(!self.is_amended));
        put("rdoPg1I4ShortPeriodYes", flag(self.is_short_period));
        put("rdoPg1I4ShortPeriodNo", flag(!self.is_short_period));
        put("rdoPg1I5ATCR1C3", flag(self.atc == Form1702ExAtc::IC011));
        put("rdoPg1I5ATCR2C3", flag(self.atc == Form1702ExAtc::IC021));

        let (t1, t2, t3, branch) = split_tin(&self.tin);
        for (a, b, c, br) in [
            (
                "txtPg1Pt1I6TINC1",
                "txtPg1Pt1I6TINC2",
                "txtPg1Pt1I6TINC3",
                "txtPg1Pt1I6TINC4",
            ),
            ("txtPg2TinC1", "txtPg2TinC2", "txtPg2TinC3", "txtPg2TinC4"),
            ("txtPg3TinC1", "txtPg3TinC2", "txtPg3TinC3", "txtPg3TinC4"),
        ] {
            put(a, t1.clone());
            put(b, t2.clone());
            put(c, t3.clone());
            put(br, branch.clone());
        }
        put("hdnPg1Pt1I7RDO", self.rdo_code.trim().to_string());
        put("rdoPg1Pt1I7RDO", self.rdo_code.trim().to_string());
        // loadBGData: the name in capitals over 60-character lines, the
        // address over 80-character lines (upper-cased at load); pages 2
        // and 3 repeat the first name line.
        let name = split_lines(&self.registered_name.trim().to_uppercase(), 60);
        put("txtPg1Pt1I8RegisteredName", name[0].clone());
        put("txtPg1Pt1I8RegisteredName2", name[1].clone());
        put("txtPg1Pt1I8RegisteredName3", name[2].clone());
        put("txtPg2RegisteredName", name[0].clone());
        put("txtPg3RegisteredName", name[0].clone());
        let address = split_lines(&self.registered_address.trim().to_uppercase(), 80);
        put("txtPg1Pt1I9RegisteredAddress", address[0].clone());
        put("txtPg1Pt1I9RegisteredAddress2", address[1].clone());
        put("txtPg1Pt1I9RegisteredAddress3", address[2].clone());
        put("txPg1I9ZipCode", self.zip_code.trim().to_string());
        put(
            "txtPg1Pt1I10DateofIncorporation",
            self.date_of_incorporation.trim().to_string(),
        );
        put(
            "txtPg1Pt1I11ContactNumber",
            self.contact_number.trim().to_string(),
        );
        put("txtPg1Pt1I12Email", self.email.trim().to_string());
        put(
            "rdoPg1Pt1I13MethodOfDeducItemized",
            flag(self.deduction == Form1702ExDeduction::Itemized),
        );
        put(
            "rdoPg1Pt1I13MethodOfDeducOptional",
            flag(self.deduction == Form1702ExDeduction::Osd),
        );
        put("txtPg1Pt1I14LegalBasis", self.legal_basis.clone());
        put("txtPg1Pt1I15Investment", self.promotion_agency.clone());
        put(
            "txtPg1Pt1I16RegisteredActivity",
            self.registered_activity.clone(),
        );
        put(
            "txtPg1Pt1I17EffectivityFrom",
            self.effectivity_from.trim().to_string(),
        );
        put(
            "txtPg1Pt1I17EffectivityTo",
            self.effectivity_to.trim().to_string(),
        );

        put("txtPg1Pt2I18TaxDue", money(self.tax_due));
        put("txtPg1Pt2I19TotalTaxCrPmt", money(self.total_credits));
        put("txtPg1Pt2I20TotalOverpmt", money(self.net_payable));
        put("txtPg1Pt2I21PenaltyCompromise", money(self.penalties));
        put(
            "txtPg1Pt2I22TotalAmtPayable",
            money(self.total_amount_payable),
        );
        use Form1702ExOverpayment as O;
        put(
            "rdoPg1OverpaymentRefund",
            flag(self.overpayment == O::Refund),
        );
        put(
            "rdoPg1OverpaymentTCC",
            flag(self.overpayment == O::TaxCreditCertificate),
        );
        put(
            "rdoPg1OverpaymentCarryOver",
            flag(self.overpayment == O::CarryOver),
        );
        put(
            "txtPg1P2I23NumOfAttachments",
            format!("{:02}", self.number_of_attachments),
        );
        // sleeptime() writes FormatValue("") = "0" into the maxLength-12
        // signatory titles; the submit loop strips commas from them.
        put("txtPg1Pt2TitleofSignatory", "0".to_string());
        put("txtPg1Pt2TitleofSignatory2", "0".to_string());

        put("txtPg2Pt4I28SalesReceiptsRevFees", money(self.sales));
        put(
            "txtPg2Pt4I29SalesRetAllowanceDisc",
            money(self.sales_returns),
        );
        put("txtPg2Pt4I30NetSalesReceiptsRevFees", money(self.net_sales));
        put("txtPg2Pt4I31CostOfSalesServ", money(self.cost_of_sales));
        put("txtPg2Pt4I32GrossIncomeFromOper", money(self.gross_income));
        put("txtPg2Pt4I33AddOther", money(self.other_income));
        put("txtPg2Pt4I34TotalGross", money(self.total_gross_income));
        put(
            "txtPg2Pt4I35OrdinaryAllowable",
            money(self.ordinary_deductions),
        );
        put(
            "txtPg2Pt4I36SpecialAllowable",
            money(self.special_deductions),
        );
        put("txtPg2Pt4I37TotalItemized", money(self.total_itemized));
        put("txtPg2Pt4I38OptionalStandardDeduc", money(self.osd));
        put("txtPg2Pt4I39NetTaxable", money(self.net_taxable_income));
        // The rate keeps the page's initial "0" until the filer types one.
        put(
            "txtPg2Pt4I40TaxRate",
            if self.tax_rate == 0.0 {
                "0".to_string()
            } else {
                money(self.tax_rate)
            },
        );
        put("txtPg2Pt4I41TaxDue", money(self.tax_due));
        put(
            "txtPg2Pt4I42PriorYearExcessCr",
            money(self.prior_year_excess),
        );
        put(
            "txtPg2Pt4I43IncomeTaxPmtFromPreviousQrt",
            money(self.quarterly_payments),
        );
        put(
            "txtPg2Pt4I44CreditableTaxWithheldFromPrevQrt",
            money(self.cwt_previous_quarters),
        );
        put(
            "txtPg2Pt4I45CreditableTaxWithheldFor4thQrt",
            money(self.cwt_q4),
        );
        put("txtPg2Pt4I46ForeignTaxCr", money(self.foreign_tax_credits));
        // checkFilingYear (Items 1, 3, 4) writes a bare 0 when not amended.
        put(
            "txtPg2Pt4I47TaxPaidInReturnPrevFiled",
            if self.is_amended {
                money(self.previously_filed)
            } else {
                "0".to_string()
            },
        );
        for (index, item) in ["48", "49"].iter().enumerate() {
            let row = self.other_credits.get(index).cloned().unwrap_or_default();
            put(
                &format!("txtPg2Pt4I{item}OtherTaxCrPmtDesc"),
                row.description,
            );
            put(
                &format!("txtPg2Pt4I{item}OtherTaxCrPmtAmt"),
                money(row.amount),
            );
        }
        put("txtPg2Pt4I50TotalTaxCrPmt", money(self.total_credits));
        put("txtPg2Pt4I51TotalOverpayment", money(self.net_payable));
        put(
            "txtPg2Pt5I52RegularIncomeOtherwiseDue",
            money(self.regular_income_tax),
        );
        put(
            "txtPg2Pt5I53SpecialAllowableItemizedDeduc",
            money(self.special_allowable_relief),
        );
        put(
            "txtPg2Pt5I54TotalTaxReliefAvailment",
            money(self.tax_relief),
        );

        for ((key, _), value) in FORM_1702EX_ORDINARY_ITEMS.iter().zip(self.ordinary_items) {
            put(key, money(value));
        }
        let s1_folded = self.other_deductions.len() > FORM_1702EX_OTHER_DEDUCTION_ROWS;
        for (index, letter) in ["D", "E", "F", "G", "H", "I"].iter().enumerate() {
            let mut row = self
                .other_deductions
                .get(index)
                .cloned()
                .unwrap_or_default();
            if s1_folded && index == FORM_1702EX_OTHER_DEDUCTION_ROWS - 1 {
                // Save_Pg3Pt6S1I17PopTable with two or more rows.
                row = Form1702ExRow {
                    description: OTHERS.to_string(),
                    amount: self.popup_subtotal(&POPUP_S1I17),
                };
            }
            put(&format!("txtPg3Pt6I17Others{letter}Desc"), row.description);
            put(&format!("txtPg3Pt6I17Others{letter}Amt"), money(row.amount));
        }
        put(
            "txtPg3Pt6S1I18TotOrdinaryAllowableItemDeduc",
            money(fixed0(
                self.ordinary_items.iter().sum::<f64>()
                    + self.other_deductions.iter().map(|r| r.amount).sum::<f64>(),
            )),
        );
        let s2_folded = self.special_rows.len() > FORM_1702EX_SPECIAL_DEDUCTION_ROWS;
        for index in 0..FORM_1702EX_SPECIAL_DEDUCTION_ROWS {
            let n = index + 1;
            let mut row = self.special_rows.get(index).cloned().unwrap_or_default();
            if s2_folded && n == FORM_1702EX_SPECIAL_DEDUCTION_ROWS {
                // Save_Pg3Pt6S2I4PopTable keeps the line's legal basis.
                row.description = OTHERS.to_string();
                row.amount = self.popup_subtotal(&POPUP_S2I4);
            }
            put(&format!("txtPg3Pt6S2I{n}Description"), row.description);
            put(&format!("txtPg3Pt6S2I{n}LegalBasis"), row.legal_basis);
            put(&format!("txtPg3Pt6S2I{n}Amount"), money(row.amount));
        }
        put(
            "txtPg3Pt6S2I5TotSpecialAllowedItemDeduc",
            money(fixed0(
                self.special_rows.iter().map(|r| r.amount).sum::<f64>(),
            )),
        );
        put(
            "txtPg3Pt6S3I1NetIncomePerBook",
            money(self.net_income_per_books),
        );
        for (rows, items, stem) in [
            (
                &self.non_deductible,
                ["2", "3"],
                "NonDeductExpenseOtherIncome",
            ),
            (
                &self.non_taxable_income,
                ["5", "6"],
                "NonTaxIncomeAndIncomeSubjectToFinTax",
            ),
            (&self.special_deductions_s3, ["7", "8"], "SpecialDeduct"),
        ] {
            let folded = rows.len() > FORM_1702EX_RECONCILIATION_ROWS;
            let subtotal = fixed0(
                popup_slice(rows, FORM_1702EX_RECONCILIATION_ROWS)
                    .iter()
                    .map(|r| r.amount)
                    .sum::<f64>(),
            );
            for (index, item) in items.iter().enumerate() {
                let mut row = rows.get(index).cloned().unwrap_or_default();
                if folded && index == FORM_1702EX_RECONCILIATION_ROWS - 1 {
                    row = Form1702ExRow {
                        description: OTHERS.to_string(),
                        amount: subtotal,
                    };
                }
                put(&format!("txtPg3Pt6S3I{item}{stem}Desc"), row.description);
                put(&format!("txtPg3Pt6S3I{item}{stem}Amt"), money(row.amount));
            }
        }
        put("txtPg3Pt6S3I4Total", money(self.reconciliation_total));
        put("txtPg3Pt6S3I9Total", money(self.reconciliation_less));
        put(
            "txtPg3Pt6S3I10NetTaxableIncome",
            money(self.reconciled_taxable_income),
        );
        put("txtLOB", self.line_of_business.trim().to_uppercase());
        put("txtCurrentPage", "1".to_string());
        for popup in Self::POPUPS {
            let count = self.popup_len(popup);
            if count > 0 {
                fields.insert(
                    format!("Pg3Pt6{}SubTotal", popup.id),
                    money(self.popup_subtotal(popup)),
                );
                fields.insert(format!("Pg3Pt6{}PopLength", popup.id), count.to_string());
            }
        }
        fields
    }

    const POPUPS: [&'static Popup; 5] = [
        &POPUP_S1I17,
        &POPUP_S2I4,
        &POPUP_S3I3,
        &POPUP_S3I6,
        &POPUP_S3I8,
    ];

    /// A plain-row popup's list.
    fn popup_list(&self, popup: &Popup) -> &[Form1702ExRow] {
        match popup.id {
            "S1I17" => &self.other_deductions,
            "S3I3" => &self.non_deductible,
            "S3I6" => &self.non_taxable_income,
            "S3I8" => &self.special_deductions_s3,
            _ => &[],
        }
    }

    /// Rows the popup holds (0 when the list fits its fixed lines).
    pub fn popup_len_of(&self, list: &str) -> usize {
        Self::POPUPS
            .iter()
            .find(|popup| popup.id == list)
            .map_or(0, |popup| self.popup_len(popup))
    }

    fn popup_len(&self, popup: &Popup) -> usize {
        if popup.id == "S2I4" {
            popup_slice(&self.special_rows, popup.fixed).len()
        } else {
            popup_slice(self.popup_list(popup), popup.fixed).len()
        }
    }

    /// `Pg3Pt6<id>SubTotal`: the popup's whole-peso amounts.
    fn popup_subtotal(&self, popup: &Popup) -> f64 {
        let amounts: Vec<f64> = if popup.id == "S2I4" {
            popup_slice(&self.special_rows, popup.fixed)
                .iter()
                .map(|r| r.amount)
                .collect()
        } else {
            popup_slice(self.popup_list(popup), popup.fixed)
                .iter()
                .map(|r| r.amount)
                .collect()
        };
        fixed0(amounts.iter().sum())
    }

    /// The popup's row controls in DOM order, as `Load_Pg3Pt6<id>PopTable`
    /// leaves them: upper-case text and `formatCurrency` whole pesos (no
    /// commas in a `maxLength` 12 box).
    fn popup_controls(&self, popup: &Popup) -> Vec<(String, String)> {
        let amount = |value: f64| {
            let text = official_amount(value);
            if popup.plain_amounts {
                text.replace(',', "")
            } else {
                text
            }
        };
        let key = |n: usize, col: u8| format!("frm1702EX:txtPg3Pt6{}_{n}Col{col}", popup.id);
        let mut controls = Vec::new();
        if popup.id == "S2I4" {
            for (index, row) in popup_slice(&self.special_rows, popup.fixed)
                .iter()
                .enumerate()
            {
                let n = index + 1;
                controls.push((key(n, 1), capitalize(&row.description)));
                controls.push((key(n, 2), capitalize(&row.legal_basis)));
                controls.push((key(n, 3), amount(row.amount)));
            }
        } else {
            for (index, row) in popup_slice(self.popup_list(popup), popup.fixed)
                .iter()
                .enumerate()
            {
                let n = index + 1;
                controls.push((key(n, 1), capitalize(&row.description)));
                controls.push((key(n, 2), amount(row.amount)));
            }
        }
        controls
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// The field map plus print-only values the frozen 2018 sheet needs
    /// (`derived:` keys, never submitted): every amount in whole pesos
    /// right-aligned in its 12-slot comb (`derived:amount:<control>`; rows
    /// nobody filled stay blank), the Item 2 and date digits, the name and
    /// address over the sheet's 38-slot lines, the attachment count and the
    /// nine TIN digits for the page 2 and 3 headers (the sheet preprints the
    /// branch code as 00000).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let mut derived = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            derived.insert(format!("derived:{key}"), value);
        };
        let month = format!("{:02}", self.year_end_month);
        let year = format!("{:02}", self.taxable_year % 100);
        for (key, value) in [
            ("year_end_mm1", &month[..1]),
            ("year_end_mm2", &month[1..2]),
            ("year_end_yy1", &year[..1]),
            ("year_end_yy2", &year[1..2]),
        ] {
            put(key, value.to_string());
        }
        let name = self.registered_name.trim().to_uppercase();
        for (index, line) in print_lines(&name, &[38, 38, 38]).into_iter().enumerate() {
            put(&format!("name_line{}", index + 1), line);
        }
        let address = self.registered_address.trim().to_uppercase();
        for (index, line) in print_lines(&address, &[38, 38, 30]).into_iter().enumerate() {
            put(&format!("address_line{}", index + 1), line);
        }
        for (stem, date) in [
            ("incorporation", &self.date_of_incorporation),
            ("effectivity_from", &self.effectivity_from),
            ("effectivity_to", &self.effectivity_to),
        ] {
            let parts: Vec<&str> = date.trim().split('/').collect();
            let [mm, dd, yyyy] = match parts.as_slice() {
                [mm, dd, yyyy] => [*mm, *dd, *yyyy],
                _ => ["", "", ""],
            };
            put(&format!("{stem}_mm"), mm.to_string());
            put(&format!("{stem}_dd"), dd.to_string());
            put(&format!("{stem}_yyyy"), yyyy.to_string());
        }
        put("attachments", format!("{:>3}", self.number_of_attachments));
        // Pages 2 and 3 have nine TIN boxes; the branch prints as 00000.
        let (a, b, c, _) = split_tin(&self.tin);
        put("tin_digits", format!("{a}{b}{c}"));
        put("tin_digits_p3", format!("{a}{b}{c}"));
        for (amount_key, description_key) in print_amount_keys() {
            let key = format!("frm1702EX:{amount_key}");
            let value = fields.get(&key).cloned().unwrap_or_default();
            let amount = parse_official_amount(&value).unwrap_or(0.0);
            let unused_row = description_key.is_some_and(|description| {
                fields
                    .get(&format!("frm1702EX:{description}"))
                    .is_none_or(|text| text.trim().is_empty())
                    && amount == 0.0
            });
            let text = if unused_row {
                String::new()
            } else {
                print_whole_pesos(amount)
            };
            put(&format!("amount:{amount_key}"), text);
        }
        fields.extend(derived);
        fields
    }

    /// The "(more...)" popups: the button opens only once every fixed line
    /// before it is complete (`Check_Pg3Pt6<id>`), and `CheckEmptyDesc`
    /// guards each save with its alerts.
    fn validate_popups(&self, err: &mut dyn FnMut(&str, &str)) {
        let lists: [(&Popup, &str, &str); 5] = [
            (
                &POPUP_S1I17,
                "other_deductions",
                "Schedule 1 Items 17D to 17H",
            ),
            (&POPUP_S2I4, "special_rows", "Schedule 2 Items 1 to 3"),
            (&POPUP_S3I3, "non_deductible", "Schedule 3 Item 2"),
            (&POPUP_S3I6, "non_taxable_income", "Schedule 3 Item 5"),
            (&POPUP_S3I8, "special_deductions_s3", "Schedule 3 Item 7"),
        ];
        for (popup, field, lines) in lists {
            if self.popup_len(popup) == 0 {
                continue;
            }
            let fixed_complete = if popup.id == "S2I4" {
                fixed_slice(&self.special_rows, popup.fixed)
                    .iter()
                    .all(|row| {
                        !row.description.trim().is_empty()
                            && !row.legal_basis.trim().is_empty()
                            && row.amount != 0.0
                    })
            } else {
                fixed_slice(self.popup_list(popup), popup.fixed)
                    .iter()
                    .all(|row| !row.description.trim().is_empty() && row.amount != 0.0)
            };
            if !fixed_complete {
                err(
                    field,
                    &format!(
                        "Complete Page 3 Part VI {lines} before adding rows: the (more...) button stays disabled until then."
                    ),
                );
                continue;
            }
            // CheckEmptyDesc(table, number, "save"), first failing row only.
            let first = popup.fixed - 1;
            let rows: Vec<(String, String, f64)> = if popup.id == "S2I4" {
                popup_slice(&self.special_rows, popup.fixed)
                    .iter()
                    .map(|r| (r.description.clone(), r.legal_basis.clone(), r.amount))
                    .collect()
            } else {
                popup_slice(self.popup_list(popup), popup.fixed)
                    .iter()
                    .map(|r| (r.description.clone(), String::new(), r.amount))
                    .collect()
            };
            for (x, (description, legal_basis, amount)) in rows.iter().enumerate() {
                let item = format!("{}{}", popup.number, x + 1);
                let at = format!("{field}[{}]", first + x);
                if description.trim().is_empty() {
                    err(
                        &at,
                        &format!("Cannot save. You have an empty description on item {item}"),
                    );
                    break;
                }
                if popup.id == "S2I4" && legal_basis.trim().is_empty() {
                    err(
                        &at,
                        &format!("Cannot save. Your legal basis is empty on item {item}"),
                    );
                    break;
                }
                if *amount == 0.0 {
                    err(
                        &at,
                        &format!("Cannot save.  Amount should not be zero on item {item}"),
                    );
                    break;
                }
            }
        }
    }

    fn validate_on(&self, today: NaiveDate) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let year = i32::from(self.taxable_year);
        let month = u32::from(self.year_end_month);
        let current_year = today.year();
        let current_month = today.month();
        let fiscal = self.tax_period_basis == TaxPeriodBasis::Fiscal;

        // validateAll(), in order.
        if self.taxable_year == 0 {
            err(
                "taxable_year",
                "Please enter a valid Year End on Page 1 Item 2",
            );
        } else if fiscal && (year > current_year || (month > current_month && year == current_year))
        {
            err(
                "taxable_year",
                "Date (Page 1 Item 2) cannot be greater than current date when filing for Fiscal Year.",
            );
        } else if fiscal && month == 12 {
            err(
                "year_end_month",
                "Date (Page 1 Item 2) Month cannot be equal to December.",
            );
        } else if year < 2018 {
            err(
                "taxable_year",
                "Invalid Year. Year should not be earlier than 2018.",
            );
        } else if !fiscal {
            if !self.is_short_period && year >= current_year {
                err(
                    "taxable_year",
                    "Year (Page 1 Item 2) cannot be greater than or equal to current year when filing for Calendar Year.",
                );
            } else if self.is_short_period {
                if year > current_year {
                    err(
                        "taxable_year",
                        "Year (Page 1 Item 2) cannot be greater than the current year when filing for Calendar Year.",
                    );
                } else if year == current_year && month > current_month {
                    err(
                        "year_end_month",
                        "Month (Page 1 Item 2) cannot be greater than  current month date when filing for Calendar Year  and  Short Period Return.",
                    );
                } else if year == current_year && month == 12 {
                    err(
                        "year_end_month",
                        "Month (Page 1 Item 2) cannot be equal to december  when filing for Calendar Year and  Short Period Return.",
                    );
                }
            }
        }
        if !(1..=12).contains(&self.year_end_month) {
            err("year_end_month", "(Page 1 Item 2) Month is invalid.");
        }
        if self.atc == Form1702ExAtc::Unanswered {
            err("atc", "Please select an ATC on Page 1 Item 5");
        }
        let (t1, t2, t3, _) = split_tin(&self.tin);
        if t1.len() != 3
            || t2.len() != 3
            || t3.len() != 3
            || digits(&self.tin).len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN on Page 1 Item 6.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{t1}{t2}{t3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err(
                "rdo_code",
                "Please enter a valid RDO Code on Page 1 Item 7.",
            );
        }
        let name = self.registered_name.trim();
        if name.is_empty() || name.chars().count() > 180 {
            err(
                "registered_name",
                "Please enter a valid name on Page 1 Item 8",
            );
        }
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > 240 {
            err(
                "registered_address",
                "Please enter a valid Registered Address on Page 1 Item 9.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || !zip.bytes().all(|b| b.is_ascii_digit()) {
            err(
                "zip_code",
                "Please enter a valid Zip Code on Page 1 Item 9A.",
            );
        }
        let incorporated = self.date_of_incorporation.trim();
        if incorporated.is_empty() {
            err(
                "date_of_incorporation",
                "Please enter a valid Date of Incorporation on Page 1 Item 10",
            );
        } else {
            match parse_form_date(incorporated) {
                None => err(
                    "date_of_incorporation",
                    "Please provide a valid date. (MM/DD/YYYY format)",
                ),
                Some(date) if date > today => err(
                    "date_of_incorporation",
                    "This date cannot be a future date.",
                ),
                Some(date) => {
                    let after_year = date.year() > year;
                    let after_month = date.year() == year && date.month() > month;
                    if after_year || after_month {
                        err(
                            "date_of_incorporation",
                            "The date of incorporation should not be more than the Filing date ",
                        );
                    }
                }
            }
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 25 || !phone.bytes().all(|b| b.is_ascii_digit()) {
            err(
                "contact_number",
                "Please enter a valid Contact Number on Page 1 Item 11",
            );
        }
        let email = self.email.trim();
        let email_ok = {
            let mut parts = email.splitn(2, '@');
            let local = parts.next().unwrap_or("");
            let domain = parts.next().unwrap_or("");
            let labels: Vec<&str> = domain.split('.').collect();
            !local.is_empty()
                && local
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c))
                && labels.len() >= 2
                && labels.iter().all(|l| {
                    !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
                && labels.last().is_some_and(|tld| {
                    tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
                })
        };
        if !email_ok {
            err(
                "email",
                "Please enter a valid e-mail address on page 1 item 12",
            );
        }
        if self.deduction == Form1702ExDeduction::Unanswered {
            err(
                "deduction",
                "Please select a Method of Deduction in page 1 Item 13.",
            );
        }
        if self.legal_basis.is_empty() {
            err(
                "legal_basis",
                "Please enter a value Legal Bases of Tax Relief/Exemption on Page 1 Item 14",
            );
        }
        if self.promotion_agency.is_empty() {
            err(
                "promotion_agency",
                "Please enter a value for Investment Promotion Agency (IPA) Government Agency on Page 1 Item 15",
            );
        }
        if self.registered_activity.is_empty() {
            err(
                "registered_activity",
                "Please enter a value for Registered Activity/Program (Reg. No.) on Page 1 Item 16",
            );
        }
        let from_text = self.effectivity_from.trim();
        let to_text = self.effectivity_to.trim();
        if from_text.is_empty() {
            err(
                "effectivity_from",
                "Please enter a valid date for Effectivity Date of Tax Relief/Exemption(FROM) on Page 1 Item 17",
            );
        }
        if to_text.is_empty() {
            err(
                "effectivity_to",
                "Please enter a valid date for Effectivity Date of Tax Relief/Exemption(TO) on Page 1 Item 17",
            );
        }
        let from = parse_form_date(from_text);
        let to = parse_form_date(to_text);
        if !from_text.is_empty() && from.is_none() {
            err(
                "effectivity_from",
                "Please provide a valid date. (MM/DD/YYYY format)",
            );
        }
        if !to_text.is_empty() && to.is_none() {
            err(
                "effectivity_to",
                "Please provide a valid date. (MM/DD/YYYY format)",
            );
        }
        if let Some(from) = from {
            if from > today {
                err("effectivity_from", "This date cannot be a future date.");
            }
            // chkDtTaxRliefFrom.
            if from.year() > year || (from.year() == year && from.month() > month) {
                err(
                    "effectivity_from",
                    "Effective Tax Relief/Exemption date should not be greater than Taxable Year in Page 1 Item 2.",
                );
            }
        }
        if let (Some(from), Some(to)) = (from, to) {
            // DateCompare_Page1_Item17.
            if from >= to {
                err(
                    "effectivity_to",
                    "Effective Tax Relief/Exemption TO date should be greater than FROM date.",
                );
            } else if year > to.year() || (year >= to.year() && month > to.month()) {
                err(
                    "effectivity_to",
                    "Expired exemption not allowed. Effective Tax Relief/Exemption date should be greater than the Taxable Year in Page 1 Item 2.",
                );
            }
        }
        if self.overpayment_open() && self.overpayment == Form1702ExOverpayment::None {
            err(
                "overpayment",
                "Please select an Overpayment option in Page 1 after Item 22.",
            );
        }
        if self.net_taxable_income != self.reconciled_taxable_income {
            err(
                "reconciled_taxable_income",
                "Page 2 Part IV Item 39 should be equal to Page 3 Schedule 3 Item 10.",
            );
        }
        // validateAmountDescription: Items 48/49 and Schedule 1 Item 17D/I.
        let described = |rows: &[Form1702ExRow], label: &str, err: &mut dyn FnMut(&str, &str)| {
            for row in rows {
                if row.description.is_empty() && row.amount > 0.01 {
                    err(label, &format!("You have an empty data on {label}."));
                } else if !row.description.is_empty() && row.amount == 0.0 {
                    err(
                        label,
                        &format!("You entered a data on {label}. The amount should not be zero."),
                    );
                }
            }
        };
        for (index, row) in self.other_credits.iter().enumerate() {
            let label = format!("Page 2 Part 4 Item {}", 48 + index);
            described(std::slice::from_ref(row), &label, &mut err);
        }
        described(
            fixed_slice(&self.other_deductions, FORM_1702EX_OTHER_DEDUCTION_ROWS),
            "Page 3 Part 6 Item 17",
            &mut err,
        );
        // validateAmountDescription_pt2.
        for (index, row) in fixed_slice(&self.special_rows, FORM_1702EX_SPECIAL_DEDUCTION_ROWS)
            .iter()
            .enumerate()
        {
            let n = index + 1;
            let desc = !row.description.is_empty();
            let basis = !row.legal_basis.is_empty();
            let positive = row.amount > 0.01;
            if (!(desc && basis) && positive) || (desc != basis && !positive) {
                err(
                    &format!("special_rows[{index}]"),
                    &format!("You have an empty data on Page 3 Part VI Schedule 2 Item {n}."),
                );
            } else if desc && basis && !positive {
                err(
                    &format!("special_rows[{index}]"),
                    &format!(
                        "You entered a data on Page 3 Part VI Schedule 2 Item {n}. Amount should not be zero"
                    ),
                );
            }
        }
        for (rows, items, field) in [
            (&self.non_deductible, [2, 3], "non_deductible"),
            (&self.non_taxable_income, [5, 6], "non_taxable_income"),
            (&self.special_deductions_s3, [7, 8], "special_deductions_s3"),
        ] {
            for (index, row) in fixed_slice(rows, FORM_1702EX_RECONCILIATION_ROWS)
                .iter()
                .enumerate()
            {
                let item = items[index];
                if row.description.is_empty() && row.amount > 0.01 {
                    err(
                        &format!("{field}[{index}]"),
                        &format!(
                            "You have an empty data on Page 3 Part VI Schedule 3 Item {item}."
                        ),
                    );
                } else if !row.description.is_empty() && row.amount < 0.01 {
                    err(
                        &format!("{field}[{index}]"),
                        &format!(
                            "You entered a data on Page 3 Part VI Schedule 3 Item {item}. Amount should not be zero"
                        ),
                    );
                }
            }
        }
        self.validate_popups(&mut err);
        if self.special_deductions > 0.0 && self.special_allowable_relief == 0.0 {
            err(
                "special_allowable_relief",
                "Please provide value on Page 2 Item 53.",
            );
        }
        if self.net_taxable_income > 0.0 && self.regular_income_tax == 0.0 {
            err(
                "regular_income_tax",
                "Please provide value on Page 2 Item 52.",
            );
        }

        // Limits applied while typing.
        if self.atc == Form1702ExAtc::IC011 && self.deduction == Form1702ExDeduction::Osd {
            err(
                "deduction",
                "Itemized is the only allowed Method of Deduction in IC 011 ATC (Alphanumeric Tax Code).",
            );
        }
        if !(0.0..100.0).contains(&self.tax_rate) {
            err(
                "tax_rate",
                "Enter the Item 40 rate in percent (0 to below 100).",
            );
        }
        for (field, value) in [
            ("sales", self.sales),
            ("sales_returns", self.sales_returns),
            ("cost_of_sales", self.cost_of_sales),
            ("other_income", self.other_income),
            ("prior_year_excess", self.prior_year_excess),
            ("quarterly_payments", self.quarterly_payments),
            ("cwt_previous_quarters", self.cwt_previous_quarters),
            ("cwt_q4", self.cwt_q4),
            ("foreign_tax_credits", self.foreign_tax_credits),
            ("previously_filed", self.previously_filed),
            ("regular_income_tax", self.regular_income_tax),
            ("special_allowable_relief", self.special_allowable_relief),
            ("penalties", self.penalties),
        ] {
            if value < 0.0 {
                err(field, "Enter a non-negative amount.");
            }
        }
        if self.ordinary_items.iter().any(|v| *v < 0.0) {
            err("ordinary_items", "Enter a non-negative amount.");
        }
        for (field, text, limit) in [
            ("legal_basis", &self.legal_basis, 31),
            ("promotion_agency", &self.promotion_agency, 47),
            ("registered_activity", &self.registered_activity, 30),
            ("line_of_business", &self.line_of_business, 100),
        ] {
            if text.chars().count() > limit {
                err(field, &format!("At most {limit} characters."));
            }
        }
        if self.number_of_attachments > 99 {
            err("number_of_attachments", "Item 23 holds at most two digits.");
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

impl FormValidator for Form1702ExDraft {
    /// `validateAll()` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl QueueableForm for Form1702ExDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    const FORM_CODE: &'static str = "1702EX";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1702EXv2018C']`).
    const FORM_TYPE: &'static str = "1702EXv2018C";
    const LAYOUT_ID: &'static str = FORM_1702EX_FORM_ID;

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
        FilingPeriod::Annual
    }
    /// `ddlPg1I2Date + "20" + txtPg1I2YearEnd`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}{}", self.year_end_month, self.taxable_year)
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

    /// Written through [`Form1702ExDraft::official_layout`], so popup rows
    /// land where the DOM has them.
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as QueueableForm>::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let layout = self
            .official_layout()
            .map_err(|error| vec![("xml".to_string(), error)])?;
        crate::official_xml::write(&layout, &self.to_bir_field_map())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()
    }

    pub(crate) fn sample() -> Form1702ExDraft {
        let mut d = Form1702ExDraft {
            id: None,
            tin: "12345678800000".to_string(),
            tax_period_basis: TaxPeriodBasis::Calendar,
            year_end_month: 12,
            taxable_year: 2025,
            is_amended: false,
            is_short_period: false,
            atc: Form1702ExAtc::IC011,
            rdo_code: "039".to_string(),
            registered_name: "Sample Dummy Foundation Inc".to_string(),
            registered_address: "123 Sample Street, Barangay Example, Quezon City".to_string(),
            zip_code: "1100".to_string(),
            date_of_incorporation: "03/01/2010".to_string(),
            contact_number: "09170000000".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            deduction: Form1702ExDeduction::Itemized,
            legal_basis: "Sec 30 NIRC".to_string(),
            promotion_agency: "Sample Agency".to_string(),
            registered_activity: "REG-0001".to_string(),
            effectivity_from: "01/01/2010".to_string(),
            effectivity_to: "12/31/2030".to_string(),
            line_of_business: "Sample Charitable Activities".to_string(),
            tax_due: 0.0,
            total_credits: 0.0,
            net_payable: 0.0,
            penalties: 0.0,
            total_amount_payable: 0.0,
            overpayment: Form1702ExOverpayment::Refund,
            number_of_attachments: 0,
            sales: 5_000_000.5,
            sales_returns: 100_000.0,
            net_sales: 0.0,
            cost_of_sales: 2_000_000.0,
            gross_income: 0.0,
            other_income: 50_000.0,
            total_gross_income: 0.0,
            ordinary_deductions: 0.0,
            special_deductions: 0.0,
            total_itemized: 0.0,
            osd: 0.0,
            net_taxable_income: 0.0,
            tax_rate: 0.0,
            prior_year_excess: 0.0,
            quarterly_payments: 0.0,
            cwt_previous_quarters: 0.0,
            cwt_q4: 1_000.0,
            foreign_tax_credits: 0.0,
            previously_filed: 0.0,
            other_credits: Vec::new(),
            regular_income_tax: 600_000.0,
            special_allowable_relief: 15_000.0,
            tax_relief: 0.0,
            ordinary_items: {
                let mut items = [0.0; 19];
                items[1] = 10_000.0;
                items[12] = 500_000.0;
                items[16] = 20_000.0;
                items
            },
            other_deductions: vec![Form1702ExRow {
                description: "Sample expense".to_string(),
                amount: 5_000.0,
            }],
            special_rows: vec![Form1702ExSpecialRow {
                description: "Sample deduction".to_string(),
                legal_basis: "RA 0000".to_string(),
                amount: 15_000.0,
            }],
            net_income_per_books: 2_300_000.0,
            non_deductible: vec![Form1702ExRow {
                description: "Nondeductible expense".to_string(),
                amount: 120_001.0,
            }],
            reconciliation_total: 0.0,
            non_taxable_income: vec![Form1702ExRow {
                description: "Interest income".to_string(),
                amount: 20_000.0,
            }],
            special_deductions_s3: Vec::new(),
            reconciliation_less: 0.0,
            reconciled_taxable_income: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        d.recompute();
        d
    }

    fn messages(d: &Form1702ExDraft) -> Vec<String> {
        d.validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let d = sample();
        assert_eq!(d.net_sales, 4_900_001.0);
        assert_eq!(d.gross_income, 2_900_001.0);
        assert_eq!(d.total_gross_income, 2_950_001.0);
        assert_eq!(d.ordinary_deductions, 535_000.0);
        assert_eq!(d.special_deductions, 15_000.0);
        assert_eq!(d.total_itemized, 550_000.0);
        assert_eq!(d.net_taxable_income, 2_400_001.0);
        assert_eq!(d.tax_due, 0.0);
        assert_eq!(d.total_credits, 1_000.0);
        assert_eq!(d.net_payable, -1_000.0);
        assert_eq!(d.total_amount_payable, -1_000.0);
        assert_eq!(d.tax_relief, 615_000.0);
        assert_eq!(d.reconciliation_total, 2_420_001.0);
        assert_eq!(d.reconciled_taxable_income, 2_400_001.0);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let d = sample();
        let f = d.to_bir_field_map();
        assert_eq!(f["frm1702EX:txtPg1I2YearEnd"], "25");
        assert_eq!(
            f["frm1702EX:txtPg1Pt1I8RegisteredName"],
            "SAMPLE DUMMY FOUNDATION INC"
        );
        assert_eq!(f["frm1702EX:txtPg1Pt1I14LegalBasis"], "SEC 30 NIRC");
        assert_eq!(f["frm1702EX:txtPg1Pt2TitleofSignatory"], "0");
        assert_eq!(f["frm1702EX:txtPg2Pt4I40TaxRate"], "0");
        assert_eq!(f["frm1702EX:txtPg2Pt4I47TaxPaidInReturnPrevFiled"], "0");
        assert_eq!(f["frm1702EX:txtPg3Pt6I17OthersDDesc"], "SAMPLE EXPENSE");
        assert_eq!(f["frm1702EX:txtPg1P2I23NumOfAttachments"], "00");
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1702EXv2018C-122025#sample.taxpayer@example.com#.xml"
        );
        assert!(d.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn print_map_adds_whole_peso_and_layout_values_without_touching_the_submit_map() {
        let d = sample();
        let print = d.to_print_field_map();
        let submit = d.to_bir_field_map();
        for (key, value) in &submit {
            assert_eq!(print.get(key), Some(value), "{key}");
        }
        assert!(
            print
                .keys()
                .filter(|k| !submit.contains_key(*k))
                .all(|k| k.starts_with("derived:"))
        );
        let get = |key: &str| print[&format!("derived:{key}")].clone();
        // 5,000,000.50 rounds up to whole pesos; right-aligned in 12 slots.
        assert_eq!(
            get("amount:txtPg2Pt4I28SalesReceiptsRevFees"),
            "     5000001"
        );
        assert_eq!(get("amount:txtPg1Pt2I22TotalAmtPayable"), "      (1000)");
        assert_eq!(
            get("amount:txtPg2Pt4I38OptionalStandardDeduc"),
            "           0"
        );
        // Unused description rows print blank.
        assert_eq!(get("amount:txtPg3Pt6I17OthersEAmt"), "");
        assert_eq!(get("amount:txtPg3Pt6I17OthersDAmt"), "        5000");
        assert_eq!(get("year_end_mm1"), "1");
        assert_eq!(get("year_end_mm2"), "2");
        assert_eq!(get("year_end_yy1"), "2");
        assert_eq!(get("year_end_yy2"), "5");
        assert_eq!(get("incorporation_yyyy"), "2010");
        assert_eq!(get("effectivity_to_mm"), "12");
        assert_eq!(get("tin_digits"), "123456788");
        assert_eq!(get("attachments"), "  0");
        assert_eq!(get("name_line1"), "SAMPLE DUMMY FOUNDATION INC");
        assert_eq!(get("name_line2"), "");
        assert_eq!(
            get("address_line1"),
            "123 SAMPLE STREET, BARANGAY EXAMPLE, Q"
        );
        assert_eq!(get("address_line2"), "UEZON CITY");
    }

    fn popup_rows(d: &mut Form1702ExDraft) {
        let row = |description: &str, amount: f64| Form1702ExRow {
            description: description.to_string(),
            amount,
        };
        d.other_deductions = vec![
            row("A", 1_000.0),
            row("B", 1_000.0),
            row("C", 1_000.0),
            row("D", 1_000.0),
            row("E", 1_000.0),
            row("Transport", 3_000.6),
            row("Courier", 2_500.75),
        ];
        d.non_deductible = vec![
            row("Nondeductible", 120_001.0),
            row("Entertainment", 30_000.3),
            row("Penalties", 1_234_567.89),
        ];
        d.recompute();
    }

    #[test]
    fn popup_rows_fold_into_others_with_whole_peso_subtotals() {
        let mut d = sample();
        popup_rows(&mut d);
        let map = d.to_bir_field_map();
        let get = |key: &str| map[key].clone();
        // Save_Pg3Pt6S1I17PopTable: OTHERS and the whole-peso subtotal.
        assert_eq!(get("frm1702EX:txtPg3Pt6I17OthersIDesc"), "OTHERS");
        assert_eq!(get("frm1702EX:txtPg3Pt6I17OthersIAmt"), "5,500.00");
        assert_eq!(get("Pg3Pt6S1I17SubTotal"), "5,500.00");
        assert_eq!(get("Pg3Pt6S1I17PopLength"), "2");
        assert_eq!(get("frm1702EX:txtPg3Pt6S1I17_1Col1"), "TRANSPORT");
        assert_eq!(get("frm1702EX:txtPg3Pt6S1I17_1Col2"), "3,000.00");
        assert_eq!(get("frm1702EX:txtPg3Pt6S1I17_2Col2"), "2,500.00");
        // The Schedule 3 Item 3 popup boxes have maxLength 12: no commas.
        assert_eq!(get("frm1702EX:txtPg3Pt6S3I3_2Col2"), "1234567.00");
        assert_eq!(
            get("frm1702EX:txtPg3Pt6S3I3NonDeductExpenseOtherIncomeAmt"),
            "1,264,567.00"
        );
        // Totals add the subtotal, not the typed centavos.
        assert_eq!(
            d.ordinary_deductions,
            d.ordinary_items.iter().sum::<f64>() + 5_000.0 + 5_500.0
        );
        // Unused popups stay at their defaults.
        assert!(!map.contains_key("Pg3Pt6S2I4PopLength"));
        // The rows sit ahead of the subtotal, where the DOM has them.
        let layout = d.official_layout().unwrap();
        let keys: Vec<String> = layout
            .entries
            .iter()
            .map(|entry| match entry {
                crate::official_xml::Entry::Bool { key, .. }
                | crate::official_xml::Entry::Value { key, .. } => key.clone(),
            })
            .collect();
        let at = |key: &str| keys.iter().position(|k| k == key).unwrap();
        assert_eq!(
            at("frm1702EX:txtPg3Pt6S1I17_2Col2") + 1,
            at("Pg3Pt6S1I17SubTotal")
        );
        assert_eq!(
            at("frm1702EX:txtPg3Pt6S3I3_2Col2") + 1,
            at("Pg3Pt6S3I3SubTotal")
        );
        // The fixed line prints OTHERS with the subtotal.
        let print = d.to_print_field_map();
        assert_eq!(print["frm1702EX:txtPg3Pt6I17OthersIDesc"], "OTHERS");
        assert_eq!(
            print["derived:amount:txtPg3Pt6I17OthersIAmt"],
            "        5500"
        );
    }

    #[test]
    fn popup_rows_take_the_check_empty_desc_alerts() {
        let mut d = sample();
        popup_rows(&mut d);
        d.other_deductions[6].description.clear();
        assert!(
            messages(&d)
                .contains(&"Cannot save. You have an empty description on item 17.2".to_string())
        );
        popup_rows(&mut d);
        d.other_deductions[6].amount = 0.4;
        d.recompute();
        assert!(
            messages(&d)
                .contains(&"Cannot save.  Amount should not be zero on item 17.2".to_string())
        );
        let mut d = sample();
        let special = |legal_basis: &str| Form1702ExSpecialRow {
            description: "Deduction".to_string(),
            legal_basis: legal_basis.to_string(),
            amount: 1_000.0,
        };
        d.special_rows = vec![
            special("RA 1"),
            special("RA 2"),
            special("RA 3"),
            special("RA 4"),
            special(""),
        ];
        d.recompute();
        assert!(
            messages(&d)
                .contains(&"Cannot save. Your legal basis is empty on item 4.2".to_string())
        );
        // The (more...) button needs every fixed line before the popup line.
        let mut d = sample();
        popup_rows(&mut d);
        d.other_deductions[2].amount = 0.0;
        d.recompute();
        assert!(messages(&d).iter().any(|m| m.contains("(more...)")));
    }

    #[test]
    fn osd_and_rates() {
        let mut d = sample();
        d.atc = Form1702ExAtc::IC021;
        d.deduction = Form1702ExDeduction::Osd;
        d.tax_rate = 10.0;
        d.recompute();
        assert!(d.special_rows.is_empty());
        assert_eq!(d.osd, 1_180_000.0);
        assert_eq!(d.net_taxable_income, 1_770_001.0);
        assert_eq!(d.tax_due, 177_000.0);
        assert_eq!(
            d.to_bir_field_map()["frm1702EX:txtPg2Pt4I40TaxRate"],
            "10.00"
        );
        // IC 011 forces itemized deductions.
        d.atc = Form1702ExAtc::IC011;
        d.recompute();
        assert_eq!(d.deduction, Form1702ExDeduction::Itemized);
    }

    #[test]
    fn period_codes_round_trip() {
        let d = sample();
        assert_eq!(d.period_code(), "122025");
        assert_eq!(
            Form1702ExDraft::parse_period_code("122025"),
            Some((2025, FilingPeriod::Annual))
        );
        assert_eq!(Form1702ExDraft::parse_period_code("2025Q1"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1702ExDraft), expected: &str| {
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
            "Please enter a valid Year End on Page 1 Item 2",
        );
        check(
            &|d| d.taxable_year = 2026,
            "Year (Page 1 Item 2) cannot be greater than or equal to current year when filing for Calendar Year.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Invalid Year. Year should not be earlier than 2018.",
        );
        check(
            &|d| {
                d.set_tax_period_basis(TaxPeriodBasis::Fiscal);
                d.recompute();
            },
            "Date (Page 1 Item 2) Month cannot be equal to December.",
        );
        check(
            &|d| {
                d.set_tax_period_basis(TaxPeriodBasis::Fiscal);
                d.year_end_month = 6;
                d.taxable_year = 2026;
                d.recompute();
            },
            "Date (Page 1 Item 2) cannot be greater than current date when filing for Fiscal Year.",
        );
        check(
            &|d| {
                d.atc = Form1702ExAtc::Unanswered;
                d.recompute();
            },
            "Please select an ATC on Page 1 Item 5",
        );
        check(
            &|d| d.rdo_code.clear(),
            "Please enter a valid RDO Code on Page 1 Item 7.",
        );
        check(
            &|d| d.registered_name.clear(),
            "Please enter a valid name on Page 1 Item 8",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter a valid Registered Address on Page 1 Item 9.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter a valid Zip Code on Page 1 Item 9A.",
        );
        check(
            &|d| d.date_of_incorporation.clear(),
            "Please enter a valid Date of Incorporation on Page 1 Item 10",
        );
        check(
            &|d| d.date_of_incorporation = "03/01/2026".into(),
            "The date of incorporation should not be more than the Filing date ",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Contact Number on Page 1 Item 11",
        );
        check(
            &|d| d.email = "not-an-email".into(),
            "Please enter a valid e-mail address on page 1 item 12",
        );
        check(
            &|d| {
                d.atc = Form1702ExAtc::IC021;
                d.deduction = Form1702ExDeduction::Unanswered;
                d.recompute();
            },
            "Please select a Method of Deduction in page 1 Item 13.",
        );
        check(
            &|d| {
                d.legal_basis.clear();
                d.recompute();
            },
            "Please enter a value Legal Bases of Tax Relief/Exemption on Page 1 Item 14",
        );
        check(
            &|d| {
                d.promotion_agency.clear();
                d.recompute();
            },
            "Please enter a value for Investment Promotion Agency (IPA) Government Agency on Page 1 Item 15",
        );
        check(
            &|d| {
                d.registered_activity.clear();
                d.recompute();
            },
            "Please enter a value for Registered Activity/Program (Reg. No.) on Page 1 Item 16",
        );
        check(
            &|d| d.effectivity_from.clear(),
            "Please enter a valid date for Effectivity Date of Tax Relief/Exemption(FROM) on Page 1 Item 17",
        );
        check(
            &|d| d.effectivity_to.clear(),
            "Please enter a valid date for Effectivity Date of Tax Relief/Exemption(TO) on Page 1 Item 17",
        );
        check(
            &|d| d.effectivity_to = "01/01/2009".into(),
            "Effective Tax Relief/Exemption TO date should be greater than FROM date.",
        );
        check(
            &|d| d.effectivity_to = "06/30/2025".into(),
            "Expired exemption not allowed. Effective Tax Relief/Exemption date should be greater than the Taxable Year in Page 1 Item 2.",
        );
        check(
            &|d| {
                d.overpayment = Form1702ExOverpayment::None;
                d.recompute();
            },
            "Please select an Overpayment option in Page 1 after Item 22.",
        );
        check(
            &|d| {
                d.net_income_per_books = 2_000_000.0;
                d.recompute();
            },
            "Page 2 Part IV Item 39 should be equal to Page 3 Schedule 3 Item 10.",
        );
        check(
            &|d| {
                d.other_deductions[0].description.clear();
                d.recompute();
            },
            "You have an empty data on Page 3 Part 6 Item 17.",
        );
        check(
            &|d| {
                d.special_rows[0].legal_basis.clear();
                d.recompute();
            },
            "You have an empty data on Page 3 Part VI Schedule 2 Item 1.",
        );
        check(
            &|d| {
                d.non_taxable_income[0].description.clear();
                d.recompute();
            },
            "You have an empty data on Page 3 Part VI Schedule 3 Item 5.",
        );
        check(
            &|d| {
                d.special_allowable_relief = 0.0;
                d.recompute();
            },
            "Please provide value on Page 2 Item 53.",
        );
        check(
            &|d| {
                d.regular_income_tax = 0.0;
                d.recompute();
            },
            "Please provide value on Page 2 Item 52.",
        );
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut d = sample();
        d.queue(crate::filing_queue::QueueAuthSource::Gui).unwrap();
        assert!(d.revalidate_queued_before_submission().is_ok());
        d.penalties = 99.0;
        assert!(d.revalidate_queued_before_submission().is_err());
    }
}
