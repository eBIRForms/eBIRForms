//! BIR Form 2550M (February 2007 ENCS) — Monthly Value-Added Tax Declaration.
//!
//! Ported from the official `BIR-Form2550M.hta`: the compute chain
//! (`compute13B` … `compute24`, `getRequiredWithheld`,
//! `totalAmountandOutputTax`, `computeInputTaxSched4`,
//! `computeInputTaxSched5`), `validate` with its exact alert texts, and
//! the uploaded file (`saveEncryptedProfile`) through [`crate::official_xml`].
//!
//! Schedules 1, 2, 3 (Parts A and B), 6, 7 and 8 write one row of controls per
//! entry inside their modals, ahead of each modal's totals;
//! [`Form2550MDraft::official_payload`] splices those rows into the fixed
//! layout where they sit in `frmMain`: the upload (`saveEncryptedProfile`)
//! writes every control in DOM order.

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
pub const FORM_2550M_FORM_ID: &str = "2550m-v2007";
/// VAT rate the page applies (`* 0.12`).
pub const FORM_2550M_VAT_RATE: f64 = 0.12;
/// The modal control Schedule 1 rows are written ahead of.
#[cfg(test)]
const SCHEDULE_1_ANCHOR: &str = "<div>frm2550M:txtmodaltxtTotal12A=";
/// Schedule 2 aggregate cap and Schedule 3A floor (`getSched2Modal`,
/// `checkifEmptyFieldSched3`).
const ONE_MILLION: f64 = 1_000_000.0;

/// The ATC popup (`ATCList`): every `xml/atcCodes.xml` entry tagged 2550M,
/// in file order. Checkbox `AtcCode<n>` is entry `n` (1-based).
pub const FORM_2550M_ATCS: &[(&str, &str)] = &[
    ("VB010", "VAT-ON BUSINESS SERVICES-IN GENERAL"),
    ("VB100", "VAT-ON BUS SERV-HOTEL, MOTELS, ETC"),
    ("VB101", "VAT -ON BUSINESS SERVICES -RESTAURANTS, CATERERS"),
    (
        "VB102",
        "VAT -ON BUS. SERVICES -DEALERS ON SEC/LENDING INVESTORS",
    ),
    ("VB105", "VAT -COMMON CARRIERS -LAND BASED (ROAD FREIGHT)"),
    ("VB106", "VAT -COMMON CARRIERS -DOMESTIC OCEAN-GOING VESSEL"),
    (
        "VB107",
        "VAT -COMMON CARRIERS -INTER-ISLAND SHIPPING VESSEL",
    ),
    ("VB108", "VAT -COMMON CARRIERS -AIRCRAFT"),
    ("VB109", "VAT -FRANCHISE HOLDERS -TELEPHONE"),
    ("VB111", "VAT -FRANCHISE HOLDERS -RADIO/TELEPHONE BROAD."),
    ("VB112", "VAT -FRANCHISE HOLDERS -OTHERS"),
    ("VB113", "VAT -NON-LIFE INSURANCE COMPANIES"),
    ("VC010", "VAT -ON CONSTRUCTION"),
    (
        "VD010",
        "COMPULSORY SOCIAL SECURITY PUBLIC ADMIN AND DEFENSE",
    ),
    (
        "VH010",
        "VAT -ON COMMUNITY, PERSONAL AND HOUSEHOLD SERVICES",
    ),
    ("VI010", "VAT -ON IMPORTATION OF GOODS"),
    ("VM010", "VAT -MANUFACTURING IN GEN."),
    (
        "VM020",
        "VAT ON MANUFACTURING - FOOD, PRODUCTS AND BEVERAGES",
    ),
    ("VM030", "VAT ON MANUFACTURING - CEMENT"),
    ("VM040", "VAT ON MANUFACTURING - TOBACCO"),
    ("VM050", "VAT ON MANUFACTURING - FLOUR"),
    ("VM100", "VAT -MANUFACTURING -PESTICIDES"),
    ("VM110", "VAT ON MANUFACTURING - ALCOHOL"),
    ("VM120", "VAT ON MANUFACTURING - PETROLEUM"),
    ("VM130", "VAT ON MANUFACTURING - AUTOMOBILES"),
    ("VM140", "VAT ON MANUFACTURING - NON ESSENTIALS"),
    ("VM150", "VAT ON MANUFACTURING - PHARMACUETICALS"),
    ("VM160", "VAT ON MANUFACTURING - SUGAR"),
    ("VP100", "VAT - SALE OF REAL PROPERTY"),
    ("VP101", "VAT - LEASE OF REAL PROPERTY"),
    ("VP102", "VAT - SALE/LEASE OF INTANGIBLE PROPERTY"),
    ("VQ010", "VAT ON MINING AND QUARYING"),
    ("VS010", "VAT -ON STORAGE AND WAREHOUSING"),
    ("VT010", "VAT -ON WHOLESALE AND RETAIL TRADE"),
    (
        "VS062",
        "ON SERVICES RENDERED BY PERSONS ENGAGED IN THE PRACTICE OF PROFESSION OR CALLING AND PROFESSIONAL SERVICES RENDERED BY GENERAL PROFESSIONAL PARTNERSHIPS",
    ),
    (
        "VS210",
        "ON SERVICES RENDERED BY STOCK, REAL ESTATE, COMMERCIAL, CUSTOMS, INSURANCE AND IMMIGRATION BROKERS",
    ),
];

/// Position of an ATC in the official popup.
pub fn form_2550m_atc_index(code: &str) -> Option<usize> {
    FORM_2550M_ATCS.iter().position(|(atc, _)| *atc == code)
}

/// Item 11 (`OptSpecialTax` + `lstSpecialTax`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2550MTaxRelief {
    #[default]
    No,
    /// "Yes" with nothing picked from the list yet.
    YesUnspecified,
    SpecialRate,
    InternationalTaxTreaty,
}

impl Form2550MTaxRelief {
    pub fn is_yes(self) -> bool {
        !matches!(self, Self::No)
    }

    fn list_value(self) -> &'static str {
        match self {
            Self::No | Self::YesUnspecified => "0",
            Self::SpecialRate => "1",
            Self::InternationalTaxTreaty => "2",
        }
    }
}

/// Item 18 purchases whose input tax the page fills at 12%
/// (`compute18TransactionbyRow`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form2550MPurchase {
    /// 18E/F — domestic purchases of goods other than capital goods.
    DomesticGoods,
    /// 18G/H — importation of goods other than capital goods.
    ImportedGoods,
    /// 18I/J — domestic purchase of services.
    DomesticServices,
    /// 18K/L — services rendered by non-residents.
    NonResidentServices,
    /// 18N/O — others.
    Others,
}

/// Schedule 1 — one ATC's vatable sales to private buyers.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2550MSalesRow {
    pub atc_code: String,
    pub amount: f64,
    /// `getRequiredWithheld`: 12% of the amount.
    pub output_tax: f64,
}

/// Schedule 4 / 5 inputs: input tax directly attributable, and the input tax
/// not directly attributable that the page allocates by sales.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2550MAllocation {
    pub direct_input_tax: f64,
    pub not_direct_input_tax: f64,
}

/// Schedule 2 — purchase of capital goods not exceeding P1 million (18A/18B).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2550MCapitalGoodsRow {
    /// `MM/DD/YYYY`, within the return period.
    pub date_purchased: String,
    pub description: String,
    /// Amount net of VAT.
    pub amount: f64,
    /// Starts at 12% of the amount (`getInputTaxCompute`) and stays editable.
    pub input_tax: f64,
}

/// Schedule 3 — capital goods exceeding P1 million, amortized.
/// Part A (current purchases, 18C/18D) and Part B (purchases of previous
/// periods) share this row; in Part B `input_tax` is the balance of input
/// tax from the previous period (`txtBalInputTaxPrevious3B`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2550MAmortizedRow {
    pub date_purchased: String,
    pub description: String,
    pub amount: f64,
    pub input_tax: f64,
    /// Estimated life in months (1–999).
    pub estimated_life: u16,
    /// Recognized life in months (1–60).
    pub recognized_life: u16,
    /// Allowable input tax for the period: `input_tax / recognized_life`.
    #[serde(default)]
    pub allowable_input_tax: f64,
    /// Balance of input tax carried to the next period.
    #[serde(default)]
    pub balance: f64,
}

impl Default for Form2550MAmortizedRow {
    /// A new row as `Schedule3()` makes it: estimated life 0, recognized 1.
    fn default() -> Self {
        Self {
            date_purchased: String::new(),
            description: String::new(),
            amount: 0.0,
            input_tax: 0.0,
            estimated_life: 0,
            recognized_life: 1,
            allowable_input_tax: 0.0,
            balance: 0.0,
        }
    }
}

/// Schedule 6 (creditable VAT withheld, 23A) and Schedule 8 (VAT withheld on
/// sales to government, 23C).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2550MWithholdingRow {
    /// `MM/DD/YYYY`, within the return period.
    pub period_covered: String,
    pub withholding_agent: String,
    pub income_payment: f64,
    pub total_withheld: f64,
    pub applied_current_month: f64,
}

/// Schedule 7 — advance VAT payments (23B).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2550MAdvancePaymentRow {
    /// `MM/DD/YYYY`, within the return period.
    pub period_covered: String,
    pub miller: String,
    pub taxpayer_name: String,
    pub or_number: String,
    pub amount_paid: f64,
    pub applied_current_month: f64,
}

/// Schedule totals the modals show and copy into Items 18, 20A and 23.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2550MScheduleTotals {
    /// Schedule 2 (18A, 18B).
    pub schedule_2_amount: f64,
    pub schedule_2_input_tax: f64,
    /// Schedule 3A (18C, 18D) and the balances (20A).
    pub schedule_3_amount: f64,
    pub schedule_3_input_tax: f64,
    pub schedule_3a_balance: f64,
    pub schedule_3b_balance: f64,
    pub schedule_3_deferred: f64,
    /// Schedules 6–8: withheld/paid and applied this month.
    pub schedule_6_withheld: f64,
    pub schedule_6_applied: f64,
    pub schedule_7_paid: f64,
    pub schedule_7_applied: f64,
    pub schedule_8_withheld: f64,
    pub schedule_8_applied: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2550MDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–3
    pub month: u8,
    pub taxable_year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub rdo_code: String,
    pub line_of_business: String,
    pub taxpayer_name: String,
    pub contact_number: String,
    pub registered_address: String,
    pub zip_code: String,
    /// `txtEmail`, never upper-cased.
    pub email: String,
    #[serde(default)]
    pub tax_relief: Form2550MTaxRelief,

    // Part II — sales
    /// Schedule 1 rows in popup order.
    #[serde(default)]
    pub sales_schedule: Vec<Form2550MSalesRow>,
    /// 12A / 12B.
    #[serde(default)]
    pub vatable_sales_private: f64,
    #[serde(default)]
    pub output_tax_private: f64,
    /// 13A / 13B (13B starts at 12% of 13A and stays editable).
    #[serde(default)]
    pub sales_to_government: f64,
    #[serde(default)]
    pub output_tax_government: f64,
    /// 14, 15.
    #[serde(default)]
    pub zero_rated_sales: f64,
    #[serde(default)]
    pub exempt_sales: f64,
    /// 16A / 16B.
    #[serde(default)]
    pub total_sales: f64,
    #[serde(default)]
    pub total_output_tax: f64,

    // Item 17
    #[serde(default)]
    pub input_tax_carried_over: f64,
    #[serde(default)]
    pub input_tax_deferred_capital_goods: f64,
    #[serde(default)]
    pub transitional_input_tax: f64,
    #[serde(default)]
    pub presumptive_input_tax: f64,
    #[serde(default)]
    pub other_input_tax: f64,
    #[serde(default)]
    pub total_input_tax_17f: f64,

    // Item 18 (purchase, input tax)
    #[serde(default)]
    pub domestic_goods: f64,
    #[serde(default)]
    pub domestic_goods_input_tax: f64,
    #[serde(default)]
    pub imported_goods: f64,
    #[serde(default)]
    pub imported_goods_input_tax: f64,
    #[serde(default)]
    pub domestic_services: f64,
    #[serde(default)]
    pub domestic_services_input_tax: f64,
    #[serde(default)]
    pub non_resident_services: f64,
    #[serde(default)]
    pub non_resident_services_input_tax: f64,
    /// 18M.
    #[serde(default)]
    pub purchases_not_qualified: f64,
    #[serde(default)]
    pub other_purchases: f64,
    #[serde(default)]
    pub other_purchases_input_tax: f64,
    /// 18P, 19.
    #[serde(default)]
    pub total_current_purchases: f64,
    #[serde(default)]
    pub total_available_input_tax: f64,

    // Item 20
    /// Schedule 4 (20B), opened only with sales to government.
    #[serde(default)]
    pub schedule_4: Option<Form2550MAllocation>,
    /// Schedule 4 "Less: standard input tax" (`txtlessStdTaxSched4`): 5.00
    /// on a fresh page, then 7% of 13A whenever 13A is entered.
    #[serde(default = "default_standard_input_tax")]
    pub standard_input_tax_government: f64,
    #[serde(default)]
    pub schedule_4_attributed: f64,
    #[serde(default)]
    pub schedule_4_total: f64,
    /// Schedule 5 (20C), opened only with exempt sales.
    #[serde(default)]
    pub schedule_5: Option<Form2550MAllocation>,
    #[serde(default)]
    pub schedule_5_attributed: f64,
    /// Schedule rows.
    #[serde(default)]
    pub schedule_2: Vec<Form2550MCapitalGoodsRow>,
    #[serde(default)]
    pub schedule_3a: Vec<Form2550MAmortizedRow>,
    #[serde(default)]
    pub schedule_3b: Vec<Form2550MAmortizedRow>,
    #[serde(default)]
    pub schedule_6: Vec<Form2550MWithholdingRow>,
    #[serde(default)]
    pub schedule_7: Vec<Form2550MAdvancePaymentRow>,
    #[serde(default)]
    pub schedule_8: Vec<Form2550MWithholdingRow>,
    #[serde(default)]
    pub schedule_totals: Form2550MScheduleTotals,
    /// 20B, 20C, 20D, 20E, 20F.
    #[serde(default)]
    pub input_tax_sales_to_government: f64,
    #[serde(default)]
    pub input_tax_exempt_sales: f64,
    #[serde(default)]
    pub vat_refund_claimed: f64,
    #[serde(default)]
    pub other_deductions: f64,
    #[serde(default)]
    pub total_deductions: f64,
    /// 21, 22.
    #[serde(default)]
    pub total_allowable_input_tax: f64,
    #[serde(default)]
    pub net_vat_payable: f64,

    // Item 23
    /// 23D, amended returns only.
    #[serde(default)]
    pub vat_paid_previous: f64,
    /// 23E, 23F, 23G.
    #[serde(default)]
    pub advance_payments: f64,
    #[serde(default)]
    pub other_credits: f64,
    #[serde(default)]
    pub total_credits: f64,
    /// 24.
    #[serde(default)]
    pub tax_still_payable: f64,
    /// 25A–25D, 26.
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

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

fn default_standard_input_tax() -> f64 {
    5.0
}

/// `round(this,2)` / `formatCurrency`: the value an amount field holds.
fn cents(value: f64) -> f64 {
    if !value.is_finite() {
        // formatCurrency(NaN) writes 0.00 (Schedule 4/5 with no sales).
        return 0.0;
    }
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// `capital()`: every text control except `txtEmail` is upper-cased.
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

/// Element-id stems of the schedule row controls the official page builds
/// in script (`'<stem>' + row`). Each stem appears quoted in
/// `BIR-Form2550M.hta`; the row keys are not in the generated layout, so
/// `official_payload` checks them against this list instead.
const SCHEDULE_ROW_KEY_STEMS: &[&str] = &[
    "frm2550m:txtAtcCde",
    "frm2550m:txtAmountSales",
    "frm2550m:txtOutputTax",
    "chxSched2",
    "txtDatePurchased",
    "txtDescription",
    "txtAmt",
    "txtInputTax",
    "chxSched3A",
    "chxSched3B",
    "txtDatePurchased3A",
    "txtDatePurchased3B",
    "txtDescription3A",
    "txtDescription3B",
    "txtAmt3A",
    "txtAmt3B",
    "txtInputTax3A",
    "txtBalInputTaxPrevious3B",
    "txtEstLife3A",
    "txtEstLife3B",
    "txtRecogLife3A",
    "txtRecogLife3B",
    "txtAllowInputTax3A",
    "txtAllowInputTax3B",
    "txtBalInputTax3A",
    "txtBalInputTax3B",
    "chxSched6",
    "chxSched8",
    "txtPeriodCovered",
    "txtPeriodCoveredSch8",
    "txtNameAgent",
    "txtNameAgentSch8",
    "txtIncomePayment",
    "txtIncomePaymentSch8",
    "txtTotalWithheld",
    "txtTotalWithheldSch8",
    "txtAppliedCurr",
    "txtAppliedCurrSch8",
    "chxSched7",
    "txtPeriodCoveredSch7",
    "txtNameMillerSch7",
    "txtNameTaxPayerSch7",
    "txtORNumSch7",
    "txtAmountPaidSch7",
    "txtAppliedCurrSch7",
];

/// True when `id` is a known row stem followed by the row number.
fn is_schedule_row_key(id: &str) -> bool {
    SCHEDULE_ROW_KEY_STEMS.iter().any(|stem| {
        id.strip_prefix(stem)
            .is_some_and(|row| !row.is_empty() && row.bytes().all(|b| b.is_ascii_digit()))
    })
}

impl Form2550MDraft {
    pub const FORM_CODE: &'static str = "2550M";

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
            line_of_business: profile.line_of_business.clone(),
            taxpayer_name: profile.full_name.clone(),
            contact_number: profile.phone.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            email: profile.email.clone(),
            tax_relief: Form2550MTaxRelief::No,
            sales_schedule: Vec::new(),
            vatable_sales_private: 0.0,
            output_tax_private: 0.0,
            sales_to_government: 0.0,
            output_tax_government: 0.0,
            zero_rated_sales: 0.0,
            exempt_sales: 0.0,
            total_sales: 0.0,
            total_output_tax: 0.0,
            input_tax_carried_over: 0.0,
            input_tax_deferred_capital_goods: 0.0,
            transitional_input_tax: 0.0,
            presumptive_input_tax: 0.0,
            other_input_tax: 0.0,
            total_input_tax_17f: 0.0,
            domestic_goods: 0.0,
            domestic_goods_input_tax: 0.0,
            imported_goods: 0.0,
            imported_goods_input_tax: 0.0,
            domestic_services: 0.0,
            domestic_services_input_tax: 0.0,
            non_resident_services: 0.0,
            non_resident_services_input_tax: 0.0,
            purchases_not_qualified: 0.0,
            other_purchases: 0.0,
            other_purchases_input_tax: 0.0,
            total_current_purchases: 0.0,
            total_available_input_tax: 0.0,
            schedule_4: None,
            standard_input_tax_government: default_standard_input_tax(),
            schedule_4_attributed: 0.0,
            schedule_4_total: 0.0,
            schedule_5: None,
            schedule_5_attributed: 0.0,
            schedule_2: Vec::new(),
            schedule_3a: Vec::new(),
            schedule_3b: Vec::new(),
            schedule_6: Vec::new(),
            schedule_7: Vec::new(),
            schedule_8: Vec::new(),
            schedule_totals: Form2550MScheduleTotals::default(),
            input_tax_sales_to_government: 0.0,
            input_tax_exempt_sales: 0.0,
            vat_refund_claimed: 0.0,
            other_deductions: 0.0,
            total_deductions: 0.0,
            total_allowable_input_tax: 0.0,
            net_vat_payable: 0.0,
            vat_paid_previous: 0.0,
            advance_payments: 0.0,
            other_credits: 0.0,
            total_credits: 0.0,
            tax_still_payable: 0.0,
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

    /// Item 13A on blur: `round`, `compute13B` (12%) and `changedTxtTax13A`
    /// (Schedule 4 standard input tax, 7%).
    pub fn set_sales_to_government(&mut self, amount: f64) {
        self.sales_to_government = cents(amount);
        self.output_tax_government = cents(self.sales_to_government * FORM_2550M_VAT_RATE);
        self.standard_input_tax_government = cents(self.sales_to_government * 7.0 / 100.0);
        self.recompute();
    }

    /// An Item 18 purchase on blur: `round` and `compute18TransactionbyRow`,
    /// which fills its input tax at 12%. The input tax stays editable.
    pub fn set_purchase(&mut self, purchase: Form2550MPurchase, amount: f64) {
        let amount = cents(amount);
        let tax = cents(amount * FORM_2550M_VAT_RATE);
        let (base, input) = self.purchase_fields(purchase);
        *base = amount;
        *input = tax;
        self.recompute();
    }

    /// A Schedule 2 amount on blur: `round` and `getInputTaxCompute` (12%).
    pub fn set_schedule_2_amount(&mut self, row: usize, amount: f64) {
        if let Some(entry) = self.schedule_2.get_mut(row) {
            entry.amount = cents(amount);
            entry.input_tax = cents(entry.amount * FORM_2550M_VAT_RATE);
        }
        self.recompute();
    }

    /// A Schedule 3A amount on blur: `round` and `getInputTaxCompute3A` (12%).
    pub fn set_schedule_3a_amount(&mut self, row: usize, amount: f64) {
        if let Some(entry) = self.schedule_3a.get_mut(row) {
            entry.amount = cents(amount);
            entry.input_tax = cents(entry.amount * FORM_2550M_VAT_RATE);
        }
        self.recompute();
    }

    fn purchase_fields(&mut self, purchase: Form2550MPurchase) -> (&mut f64, &mut f64) {
        match purchase {
            Form2550MPurchase::DomesticGoods => {
                (&mut self.domestic_goods, &mut self.domestic_goods_input_tax)
            }
            Form2550MPurchase::ImportedGoods => {
                (&mut self.imported_goods, &mut self.imported_goods_input_tax)
            }
            Form2550MPurchase::DomesticServices => (
                &mut self.domestic_services,
                &mut self.domestic_services_input_tax,
            ),
            Form2550MPurchase::NonResidentServices => (
                &mut self.non_resident_services,
                &mut self.non_resident_services_input_tax,
            ),
            Form2550MPurchase::Others => (
                &mut self.other_purchases,
                &mut self.other_purchases_input_tax,
            ),
        }
    }

    /// Tick an ATC in the Schedule 1 popup (`getATCCode`). Rows follow the
    /// popup order; an ATC appears once.
    pub fn add_sales_atc(&mut self, code: &str) -> Result<(), String> {
        if form_2550m_atc_index(code).is_none() {
            return Err(format!("{code} is not a 2550M ATC."));
        }
        if self.sales_schedule.iter().any(|row| row.atc_code == code) {
            return Err(format!("{code} is already in Schedule 1."));
        }
        self.sales_schedule.push(Form2550MSalesRow {
            atc_code: code.to_string(),
            ..Default::default()
        });
        self.recompute();
        Ok(())
    }

    /// Untick an ATC.
    pub fn remove_sales_atc(&mut self, code: &str) {
        self.sales_schedule.retain(|row| row.atc_code != code);
        self.recompute();
    }

    /// The official compute chain; every amount is the formatted value of
    /// the items it reads.
    pub fn recompute(&mut self) {
        // optAmendFunc clears 23D on an original return.
        if !self.is_amended {
            self.vat_paid_previous = 0.0;
        }

        // Schedule 1 (getRequiredWithheld, totalAmountandOutputTax) in popup order.
        self.sales_schedule
            .sort_by_key(|row| form_2550m_atc_index(&row.atc_code).unwrap_or(usize::MAX));
        let mut sales = 0.0;
        let mut output = 0.0;
        for row in &mut self.sales_schedule {
            row.amount = cents(row.amount);
            row.output_tax = cents(row.amount * FORM_2550M_VAT_RATE);
            sales += row.amount;
            output += row.output_tax;
        }
        self.vatable_sales_private = cents(sales);
        self.output_tax_private = cents(output);

        self.sales_to_government = cents(self.sales_to_government);
        self.output_tax_government = cents(self.output_tax_government);
        self.zero_rated_sales = cents(self.zero_rated_sales);
        self.exempt_sales = cents(self.exempt_sales);
        self.total_sales = cents(
            self.vatable_sales_private
                + self.sales_to_government
                + self.zero_rated_sales
                + self.exempt_sales,
        );
        self.total_output_tax = cents(self.output_tax_private + self.output_tax_government);

        for value in [
            &mut self.input_tax_carried_over,
            &mut self.input_tax_deferred_capital_goods,
            &mut self.transitional_input_tax,
            &mut self.presumptive_input_tax,
            &mut self.other_input_tax,
            &mut self.domestic_goods,
            &mut self.domestic_goods_input_tax,
            &mut self.imported_goods,
            &mut self.imported_goods_input_tax,
            &mut self.domestic_services,
            &mut self.domestic_services_input_tax,
            &mut self.non_resident_services,
            &mut self.non_resident_services_input_tax,
            &mut self.purchases_not_qualified,
            &mut self.other_purchases,
            &mut self.other_purchases_input_tax,
            &mut self.standard_input_tax_government,
            &mut self.vat_refund_claimed,
            &mut self.other_deductions,
            &mut self.vat_paid_previous,
            &mut self.advance_payments,
            &mut self.other_credits,
            &mut self.surcharge,
            &mut self.interest,
            &mut self.compromise,
        ] {
            *value = cents(*value);
        }
        self.total_input_tax_17f = cents(
            self.input_tax_carried_over
                + self.input_tax_deferred_capital_goods
                + self.transitional_input_tax
                + self.presumptive_input_tax
                + self.other_input_tax,
        );
        self.compute_schedules();
        let t = self.schedule_totals;
        // compute18P.
        self.total_current_purchases = cents(
            t.schedule_2_amount
                + t.schedule_3_amount
                + self.domestic_goods
                + self.imported_goods
                + self.domestic_services
                + self.non_resident_services
                + self.purchases_not_qualified
                + self.other_purchases,
        );
        // compute19.
        self.total_available_input_tax = cents(
            self.total_input_tax_17f
                + t.schedule_2_input_tax
                + t.schedule_3_input_tax
                + self.domestic_goods_input_tax
                + self.imported_goods_input_tax
                + self.domestic_services_input_tax
                + self.non_resident_services_input_tax
                + self.other_purchases_input_tax,
        );

        // computeInputTaxSched4 with Item 13A and 16A copied in on opening.
        match &mut self.schedule_4 {
            Some(schedule) => {
                schedule.direct_input_tax = cents(schedule.direct_input_tax);
                schedule.not_direct_input_tax = cents(schedule.not_direct_input_tax);
                self.schedule_4_attributed = cents(
                    self.sales_to_government / self.total_sales * schedule.not_direct_input_tax,
                );
                self.schedule_4_total =
                    cents(schedule.direct_input_tax + self.schedule_4_attributed);
                // getSched4Modal copies the Schedule 4 total into 20B.
                self.input_tax_sales_to_government =
                    cents(self.schedule_4_total - self.standard_input_tax_government);
            }
            None => {
                self.schedule_4_attributed = 0.0;
                self.schedule_4_total = 0.0;
                self.input_tax_sales_to_government = 0.0;
            }
        }
        // computeInputTaxSched5 with Item 15 and 16A copied in on opening.
        match &mut self.schedule_5 {
            Some(schedule) => {
                schedule.direct_input_tax = cents(schedule.direct_input_tax);
                schedule.not_direct_input_tax = cents(schedule.not_direct_input_tax);
                self.schedule_5_attributed =
                    cents(self.exempt_sales / self.total_sales * schedule.not_direct_input_tax);
                self.input_tax_exempt_sales =
                    cents(schedule.direct_input_tax + self.schedule_5_attributed);
            }
            None => {
                self.schedule_5_attributed = 0.0;
                self.input_tax_exempt_sales = 0.0;
            }
        }

        // compute20F … compute24.
        self.total_deductions = cents(
            t.schedule_3_deferred
                + self.input_tax_sales_to_government
                + self.input_tax_exempt_sales
                + self.vat_refund_claimed
                + self.other_deductions,
        );
        self.total_allowable_input_tax =
            cents(self.total_available_input_tax - self.total_deductions);
        self.net_vat_payable = cents(self.total_output_tax - self.total_allowable_input_tax);
        // compute23G.
        self.total_credits = cents(
            t.schedule_6_applied
                + t.schedule_7_applied
                + t.schedule_8_applied
                + self.vat_paid_previous
                + self.advance_payments
                + self.other_credits,
        );
        self.tax_still_payable = cents(self.net_vat_payable - self.total_credits);
        // The penalty items file their override, else the computed value,
        // held at cents like every typed amount.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_payable = cents(self.tax_still_payable + self.total_penalties);
    }

    /// Schedules 2, 3, 6, 7 and 8: `computeSumTax`, `computeSumTax3A`,
    /// `computeSumTax3B`, `computeSumModal20A`, `computeSumWithheldApp*`.
    fn compute_schedules(&mut self) {
        let mut t = Form2550MScheduleTotals::default();
        let (mut amount, mut input) = (0.0, 0.0);
        for row in &mut self.schedule_2 {
            row.amount = cents(row.amount);
            row.input_tax = cents(row.input_tax);
            amount += row.amount;
            input += row.input_tax;
        }
        t.schedule_2_amount = cents(amount);
        t.schedule_2_input_tax = cents(input);

        let (mut amount, mut input, mut balance) = (0.0, 0.0, 0.0);
        for row in &mut self.schedule_3a {
            row.amount = cents(row.amount);
            row.input_tax = cents(row.input_tax);
            row.allowable_input_tax = cents(row.input_tax / f64::from(row.recognized_life));
            row.balance = cents(row.input_tax - row.allowable_input_tax);
            amount += row.amount;
            input += row.input_tax;
            balance += row.balance;
        }
        t.schedule_3_amount = cents(amount);
        t.schedule_3_input_tax = cents(input);
        t.schedule_3a_balance = cents(balance);
        let mut balance = 0.0;
        for row in &mut self.schedule_3b {
            row.amount = cents(row.amount);
            row.input_tax = cents(row.input_tax);
            row.allowable_input_tax = cents(row.input_tax / f64::from(row.recognized_life));
            row.balance = cents(row.input_tax - row.allowable_input_tax);
            balance += row.balance;
        }
        t.schedule_3b_balance = cents(balance);
        t.schedule_3_deferred = cents(t.schedule_3a_balance + t.schedule_3b_balance);

        let sum_withholding = |rows: &mut Vec<Form2550MWithholdingRow>| {
            let (mut withheld, mut applied) = (0.0, 0.0);
            for row in rows.iter_mut() {
                row.income_payment = cents(row.income_payment);
                row.total_withheld = cents(row.total_withheld);
                row.applied_current_month = cents(row.applied_current_month);
                withheld += row.total_withheld;
                applied += row.applied_current_month;
            }
            (cents(withheld), cents(applied))
        };
        (t.schedule_6_withheld, t.schedule_6_applied) = sum_withholding(&mut self.schedule_6);
        (t.schedule_8_withheld, t.schedule_8_applied) = sum_withholding(&mut self.schedule_8);
        let (mut paid, mut applied) = (0.0, 0.0);
        for row in &mut self.schedule_7 {
            row.amount_paid = cents(row.amount_paid);
            row.applied_current_month = cents(row.applied_current_month);
            paid += row.amount_paid;
            applied += row.applied_current_month;
        }
        t.schedule_7_paid = cents(paid);
        t.schedule_7_applied = cents(applied);
        self.schedule_totals = t;
    }

    /// The row controls of every schedule, grouped by the modal control the
    /// page writes them ahead of, in page order.
    fn schedule_row_controls(&self) -> Vec<(&'static str, Vec<(String, String)>)> {
        let money = official_amount;
        let mut groups = Vec::new();
        let mut rows = Vec::new();
        for (n, row) in self
            .sales_schedule
            .iter()
            .enumerate()
            .map(|(i, r)| (i + 1, r))
        {
            rows.push((format!("frm2550m:txtAtcCde{n}"), row.atc_code.clone()));
            rows.push((format!("frm2550m:txtAmountSales{n}"), money(row.amount)));
            rows.push((format!("frm2550m:txtOutputTax{n}"), money(row.output_tax)));
        }
        groups.push(("frm2550M:txtmodaltxtTotal12A", rows));

        let mut rows = Vec::new();
        for (i, row) in self.schedule_2.iter().enumerate() {
            rows.push((format!("chxSched2{i}"), "false".to_string()));
            rows.push((
                format!("txtDatePurchased{i}"),
                row.date_purchased.trim().to_string(),
            ));
            rows.push((format!("txtDescription{i}"), caps(&row.description)));
            rows.push((format!("txtAmt{i}"), money(row.amount)));
            rows.push((format!("txtInputTax{i}"), money(row.input_tax)));
        }
        groups.push(("txtmodalTotalAmt", rows));

        for (part, schedule, anchor, previous) in [
            (
                "3A",
                &self.schedule_3a,
                "txtmodalTotalAmountSched3",
                "txtInputTax3A",
            ),
            (
                "3B",
                &self.schedule_3b,
                "txtmodalTotalBalanceSched3B",
                "txtBalInputTaxPrevious3B",
            ),
        ] {
            let mut rows = Vec::new();
            for (i, row) in schedule.iter().enumerate() {
                rows.push((format!("chxSched{part}{i}"), "false".to_string()));
                rows.push((
                    format!("txtDatePurchased{part}{i}"),
                    row.date_purchased.trim().to_string(),
                ));
                rows.push((format!("txtDescription{part}{i}"), caps(&row.description)));
                rows.push((format!("txtAmt{part}{i}"), money(row.amount)));
                rows.push((format!("{previous}{i}"), money(row.input_tax)));
                rows.push((
                    format!("txtEstLife{part}{i}"),
                    row.estimated_life.to_string(),
                ));
                rows.push((
                    format!("txtRecogLife{part}{i}"),
                    row.recognized_life.to_string(),
                ));
                rows.push((
                    format!("txtAllowInputTax{part}{i}"),
                    money(row.allowable_input_tax),
                ));
                rows.push((format!("txtBalInputTax{part}{i}"), money(row.balance)));
            }
            groups.push((anchor, rows));
        }

        for (schedule, rows_in, suffix, anchor) in [
            (6, &self.schedule_6, "", "txtmodalTotal23A"),
            (8, &self.schedule_8, "Sch8", "txtmodalTotal23C"),
        ] {
            let mut rows = Vec::new();
            for (i, row) in rows_in.iter().enumerate() {
                rows.push((format!("chxSched{schedule}{i}"), "false".to_string()));
                rows.push((
                    format!("txtPeriodCovered{suffix}{i}"),
                    row.period_covered.trim().to_string(),
                ));
                rows.push((
                    format!("txtNameAgent{suffix}{i}"),
                    caps(&row.withholding_agent),
                ));
                rows.push((
                    format!("txtIncomePayment{suffix}{i}"),
                    money(row.income_payment),
                ));
                rows.push((
                    format!("txtTotalWithheld{suffix}{i}"),
                    money(row.total_withheld),
                ));
                rows.push((
                    format!("txtAppliedCurr{suffix}{i}"),
                    money(row.applied_current_month),
                ));
            }
            groups.push((anchor, rows));
        }
        let mut rows = Vec::new();
        for (i, row) in self.schedule_7.iter().enumerate() {
            rows.push((format!("chxSched7{i}"), "false".to_string()));
            rows.push((
                format!("txtPeriodCoveredSch7{i}"),
                row.period_covered.trim().to_string(),
            ));
            rows.push((format!("txtNameMillerSch7{i}"), caps(&row.miller)));
            rows.push((format!("txtNameTaxPayerSch7{i}"), caps(&row.taxpayer_name)));
            rows.push((format!("txtORNumSch7{i}"), caps(&row.or_number)));
            rows.push((format!("txtAmountPaidSch7{i}"), money(row.amount_paid)));
            rows.push((
                format!("txtAppliedCurrSch7{i}"),
                money(row.applied_current_month),
            ));
        }
        groups.push(("txtmodalTotal23B", rows));
        // Page order: Schedule 1, 2, 3A, 3B, then 6, 7, 8.
        let order = [0, 1, 2, 3, 4, 6, 5];
        order.iter().map(|&i| groups[i].clone()).collect()
    }

    /// The official field values the upload (`saveEncryptedProfile`) reads, keyed by element id,
    /// including the Schedule 1 row controls.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.layout_fields();
        for (_, rows) in self.schedule_row_controls() {
            fields.extend(rows);
        }
        fields
    }

    /// The controls of the fixed official layout.
    fn layout_fields(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(key.to_string(), value);
        };
        let flag = |on: bool| on.to_string();
        let money = official_amount;

        put("frm2550m:RtnYear", format!("{:02}", self.month));
        put("frm2550m:txtYear", self.taxable_year.to_string());
        put("frm2550m:OptAmendedYN1", flag(self.is_amended));
        put("frm2550m:OptAmendedYN2", flag(!self.is_amended));
        put(
            "frm2550m:txtSheets",
            self.number_of_attached_sheets.to_string(),
        );
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("frm2550m:txtTIN1", tin1);
        put("frm2550m:txtTIN2", tin2);
        put("frm2550m:txtTIN3", tin3);
        put("frm2550m:txtBranchCode", branch);
        put("frm2550m:txtRDOCode", self.rdo_code.trim().to_string());
        put("frm2550m:txtLineBus", caps(&self.line_of_business));
        put("frm2550m:txtTaxPayerName", caps(&self.taxpayer_name));
        put(
            "frm2550m:txtTelephoneNum",
            self.contact_number.trim().to_string(),
        );
        put("frm2550m:txtAddress", caps(&self.registered_address));
        put("frm2550m:txtZipCode", self.zip_code.trim().to_string());
        put("frm2550m:OptSpecialTax1", flag(self.tax_relief.is_yes()));
        put("frm2550m:OptSpecialTax2", flag(!self.tax_relief.is_yes()));
        put(
            "frm2550m:lstSpecialTax",
            self.tax_relief.list_value().to_string(),
        );

        let t = self.schedule_totals;
        let items: [(&str, f64); 46] = [
            ("12A", self.vatable_sales_private),
            ("12B", self.output_tax_private),
            ("13A", self.sales_to_government),
            ("13B", self.output_tax_government),
            ("14", self.zero_rated_sales),
            ("15", self.exempt_sales),
            ("16A", self.total_sales),
            ("16B", self.total_output_tax),
            ("17A", self.input_tax_carried_over),
            ("17B", self.input_tax_deferred_capital_goods),
            ("17C", self.transitional_input_tax),
            ("17D", self.presumptive_input_tax),
            ("17E", self.other_input_tax),
            ("17F", self.total_input_tax_17f),
            ("18A", t.schedule_2_amount),
            ("18B", t.schedule_2_input_tax),
            ("18C", t.schedule_3_amount),
            ("18D", t.schedule_3_input_tax),
            ("18E", self.domestic_goods),
            ("18F", self.domestic_goods_input_tax),
            ("18G", self.imported_goods),
            ("18H", self.imported_goods_input_tax),
            ("18I", self.domestic_services),
            ("18J", self.domestic_services_input_tax),
            ("18K", self.non_resident_services),
            ("18L", self.non_resident_services_input_tax),
            ("18M", self.purchases_not_qualified),
            ("18N", self.other_purchases),
            ("18O", self.other_purchases_input_tax),
            ("18P", self.total_current_purchases),
            ("19", self.total_available_input_tax),
            ("20A", t.schedule_3_deferred),
            ("20B", self.input_tax_sales_to_government),
            ("20C", self.input_tax_exempt_sales),
            ("20D", self.vat_refund_claimed),
            ("20E", self.other_deductions),
            ("20F", self.total_deductions),
            ("21", self.total_allowable_input_tax),
            ("22", self.net_vat_payable),
            ("23A", t.schedule_6_applied),
            ("23B", t.schedule_7_applied),
            ("23C", t.schedule_8_applied),
            ("23D", self.vat_paid_previous),
            ("23E", self.advance_payments),
            ("23F", self.other_credits),
            ("23G", self.total_credits),
        ];
        for (item, value) in items {
            put(&format!("frm2550m:txtTax{item}"), money(value));
        }
        put("frm2550m:txtTax24", money(self.tax_still_payable));
        put("frm2550m:txtTax25A", money(self.surcharge));
        put("frm2550m:txtTax25B", money(self.interest));
        put("frm2550m:txtTax25C", money(self.compromise));
        put("frm2550m:txtTax25D", money(self.total_penalties));
        put("frm2550m:txtTax26", money(self.total_amount_payable));

        // Schedule 1 modal totals and the popup's checkboxes.
        put(
            "frm2550M:txtmodaltxtTotal12A",
            money(self.vatable_sales_private),
        );
        put(
            "frm2550M:txtmodaltxtTotal12B",
            money(self.output_tax_private),
        );
        for (index, (code, _)) in FORM_2550M_ATCS.iter().enumerate() {
            let ticked = self.sales_schedule.iter().any(|row| row.atc_code == *code);
            put(&format!("AtcCode{}", index + 1), flag(ticked));
        }

        // Schedule 2, 3, 6, 7 and 8 modal totals.
        for (key, value) in [
            ("txtmodalTotalAmt", t.schedule_2_amount),
            ("txtmodalTotalInputTax", t.schedule_2_input_tax),
            ("txtmodalTotalAmountSched3", t.schedule_3_amount),
            ("txtmodalTotalInputTaxSched3", t.schedule_3_input_tax),
            ("txtmodalTotalBalanceSched3A", t.schedule_3a_balance),
            ("txtmodalTotalBalanceSched3B", t.schedule_3b_balance),
            ("txtmodalTotalInputTax20ASched3", t.schedule_3_deferred),
            ("txtmodalTotal23A", t.schedule_6_withheld),
            ("txtmodalTotalSched6AppliedCurrent", t.schedule_6_applied),
            ("txtmodalTotal23B", t.schedule_7_paid),
            ("txtmodalTotalSched7AppliedCurrent", t.schedule_7_applied),
            ("txtmodalTotal23C", t.schedule_8_withheld),
            ("txtmodalTotalSched8AppliedCurrent", t.schedule_8_applied),
        ] {
            put(key, money(value));
        }

        // Schedule 4 and 5 modal controls.
        let schedule_4 = self.schedule_4.unwrap_or_default();
        let opened_4 = self.schedule_4.is_some();
        put("txtinputtaxSched4", money(schedule_4.direct_input_tax));
        put(
            "txtTaxableSaleSched4",
            money(if opened_4 {
                self.sales_to_government
            } else {
                0.0
            }),
        );
        put(
            "txtInputTaxnotDirectSched4",
            money(schedule_4.not_direct_input_tax),
        );
        put(
            "txtTotalInputTaxnotDirectSched4",
            money(self.schedule_4_attributed),
        );
        put(
            "txtTotalSaleSched4",
            money(if opened_4 { self.total_sales } else { 0.0 }),
        );
        put(
            "txtTotalInputSaletoGovernmentSched4",
            money(self.schedule_4_total),
        );
        put(
            "txtlessStdTaxSched4",
            money(self.standard_input_tax_government),
        );
        put(
            "txtTotal20BSched4",
            money(self.input_tax_sales_to_government),
        );
        let schedule_5 = self.schedule_5.unwrap_or_default();
        let opened_5 = self.schedule_5.is_some();
        put("txtinputtaxSched5", money(schedule_5.direct_input_tax));
        put(
            "txtTotSaleSched5",
            money(if opened_5 { self.exempt_sales } else { 0.0 }),
        );
        put(
            "txtAmountInputnotDirectSched5",
            money(schedule_5.not_direct_input_tax),
        );
        put(
            "txtProductnotDirectSched5",
            money(self.schedule_5_attributed),
        );
        put(
            "txtSumTotalSaleSched5",
            money(if opened_5 { self.total_sales } else { 0.0 }),
        );
        put("txtTotal20CSched5", money(self.input_tax_exempt_sales));

        put("txtEmail", self.email.trim().to_string());
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// The checks each schedule's OK button runs (`checkifEmptyFieldSched2`,
    /// `checkifEmptyFieldSched3`, `checkifEmptyFieldSched6/7/8`), in order,
    /// stopping at a schedule's first problem as the page does. The page only
    /// checks the return period for dates outside February; every date is
    /// checked here.
    fn validate_schedules(&self, err: &mut impl FnMut(&str, &str)) {
        let any_rows = !(self.schedule_2.is_empty()
            && self.schedule_3a.is_empty()
            && self.schedule_3b.is_empty()
            && self.schedule_6.is_empty()
            && self.schedule_7.is_empty()
            && self.schedule_8.is_empty());
        if any_rows && self.taxable_year <= 2000 {
            err(
                "taxable_year",
                "Please input a valid Return period year. Please enter 2000 above.",
            );
        }
        let period = (u32::from(self.taxable_year), u32::from(self.month));
        let text_limit = |value: &str, max: usize| value.trim().chars().count() > max;

        // Schedule 2.
        'schedule_2: {
            for (i, row) in self.schedule_2.iter().enumerate() {
                let n = i + 1;
                let field = format!("schedule_2[{i}]");
                if row.date_purchased.trim().is_empty() || row.description.trim().is_empty() {
                    err(
                        &field,
                        &format!(
                            "Please enter valid row {n} data for Schedule 2.\nEmpty fields are not allowed."
                        ),
                    );
                    break 'schedule_2;
                }
                if row.amount > ONE_MILLION {
                    err(
                        &field,
                        "Please enter valid data.\n Aggregate amount should not exceed P1 Million.",
                    );
                    break 'schedule_2;
                }
                if let Some(message) = date_problem(&row.date_purchased, n, period, false) {
                    err(&field, &message);
                    break 'schedule_2;
                }
                if row.amount <= 0.0 {
                    err(
                        &field,
                        &format!(
                            "Please enter amount for Net of VAT on row {n}.\nValue must be greater than 0."
                        ),
                    );
                    break 'schedule_2;
                }
                if text_limit(&row.description, 50) {
                    err(
                        &field,
                        "Schedule 2 descriptions hold at most 50 characters.",
                    );
                    break 'schedule_2;
                }
            }
            if self.schedule_totals.schedule_2_amount > ONE_MILLION {
                err(
                    "schedule_2",
                    "The total aggregate amount should not exceed 1 Million.\n Please re-enter the values of Schedule 2.",
                );
            }
        }

        // Schedule 3, Part A then Part B.
        'schedule_3: {
            for (i, row) in self.schedule_3a.iter().enumerate() {
                let n = i + 1;
                let field = format!("schedule_3a[{i}]");
                if row.date_purchased.trim().is_empty()
                    || row.description.trim().is_empty()
                    || row.allowable_input_tax == 0.0
                {
                    err(
                        &field,
                        &format!(
                            "Please enter valid row {n} data for Schedule 3A.\nEmpty fields are not allowed."
                        ),
                    );
                    break 'schedule_3;
                }
                if let Some(message) = date_problem(&row.date_purchased, n, period, false) {
                    err(&field, &message);
                    break 'schedule_3;
                }
                if row.amount <= 0.0 {
                    err(
                        &field,
                        &format!(
                            "Please enter amount for Net of VAT on row {n}.\nValue must be greater than 0."
                        ),
                    );
                    break 'schedule_3;
                }
                if !(1..=999).contains(&row.estimated_life) {
                    err(
                        &field,
                        &format!(
                            "Please enter a value 1 to 999 for Estimated Life on row {n} Schedule 3Part A."
                        ),
                    );
                    break 'schedule_3;
                }
                if !(1..=60).contains(&row.recognized_life) {
                    err(
                        &field,
                        &format!(
                            "Please enter a value 1 to 60 for Recognized Life on row {n}Part A."
                        ),
                    );
                    break 'schedule_3;
                }
                if text_limit(&row.description, 50) {
                    err(
                        &field,
                        "Schedule 3 descriptions hold at most 50 characters.",
                    );
                    break 'schedule_3;
                }
            }
            if !self.schedule_3a.is_empty() && self.schedule_totals.schedule_3_amount <= ONE_MILLION
            {
                err(
                    "schedule_3a",
                    "The total aggregate amount does not exceed 1 Million.\n Please re-enter the values of Schedule 3.",
                );
                break 'schedule_3;
            }
            for (i, row) in self.schedule_3b.iter().enumerate() {
                let n = i + 1;
                let field = format!("schedule_3b[{i}]");
                if row.date_purchased.trim().is_empty() || row.description.trim().is_empty() {
                    err(
                        &field,
                        "If you don't have any entries for Schedule 3b,\nplease delete the row on this schedule if not applicable.",
                    );
                    break 'schedule_3;
                }
                if let Some(message) = date_problem(&row.date_purchased, n, period, true) {
                    err(&field, &message);
                    break 'schedule_3;
                }
                if row.amount <= 0.0 {
                    err(
                        &field,
                        &format!(
                            "Please enter amount for Net of VAT on row {n}.\nValue must be greater than 0 in Part B."
                        ),
                    );
                    break 'schedule_3;
                }
                if !(1..=999).contains(&row.estimated_life) {
                    err(
                        &field,
                        &format!(
                            "Please enter a value 1 to 999 for Estimated Life on row {n} Schedule 3Part B."
                        ),
                    );
                    break 'schedule_3;
                }
                if !(1..=60).contains(&row.recognized_life) {
                    err(
                        &field,
                        &format!(
                            "Please enter a value 1 to 60 for Recognized Life on row {n}Part B."
                        ),
                    );
                    break 'schedule_3;
                }
                if text_limit(&row.description, 50) {
                    err(
                        &field,
                        "Schedule 3 descriptions hold at most 50 characters.",
                    );
                    break 'schedule_3;
                }
            }
        }

        // Schedules 6 and 8.
        for (schedule, rows, name) in [
            (6, &self.schedule_6, "schedule_6"),
            (8, &self.schedule_8, "schedule_8"),
        ] {
            if schedule == 8 && !rows.is_empty() && self.sales_to_government <= 0.0 {
                err(
                    name,
                    "Please enter a valid value on Item 13A to be able to load the Schedule 8.",
                );
            }
            for (i, row) in rows.iter().enumerate() {
                let n = i + 1;
                let field = format!("{name}[{i}]");
                let problem = if row.period_covered.trim().is_empty()
                    || row.withholding_agent.trim().is_empty()
                {
                    Some(format!(
                        "Please enter valid row {n} data for Schedule {schedule}.\nEmpty fields are not allowed."
                    ))
                } else if let Some(message) = date_problem(&row.period_covered, n, period, false) {
                    Some(message)
                } else if row.income_payment <= 0.0 {
                    Some(format!(
                        "Please enter a valid amount for Income Payment on row {n}.\nValue must be greater than 0."
                    ))
                } else if row.total_withheld <= 0.0 {
                    Some(format!(
                        "Please enter a valid amount for Total Tax Withheld on row {n}.\nValue must be greater than 0."
                    ))
                } else if row.applied_current_month <= 0.0 {
                    Some(format!(
                        "Please enter a valid amount for Applied Current Month on row {n}.\nValue must be greater than 0."
                    ))
                } else if row.applied_current_month > row.total_withheld {
                    // Schedule 6's check; Schedule 8's is commented out on the
                    // page, but applying more than was withheld is never right.
                    Some(format!(
                        "The Applied Current Month on row {n} should not be greater than the Total Tax Withheld."
                    ))
                } else if text_limit(&row.withholding_agent, 50) {
                    Some("Withholding agent names hold at most 50 characters.".to_string())
                } else {
                    None
                };
                if let Some(message) = problem {
                    err(&field, &message);
                    break;
                }
            }
        }

        // Schedule 7.
        for (i, row) in self.schedule_7.iter().enumerate() {
            let n = i + 1;
            let field = format!("schedule_7[{i}]");
            let problem = if row.period_covered.trim().is_empty()
                || row.miller.trim().is_empty()
                || row.taxpayer_name.trim().is_empty()
                || row.or_number.trim().is_empty()
            {
                Some(format!(
                    "Please enter valid row {n} data for Schedule 7.\nEmpty fields are not allowed."
                ))
            } else if let Some(message) = date_problem(&row.period_covered, n, period, false) {
                Some(message)
            } else if row.amount_paid <= 0.0 {
                Some(format!(
                    "Please enter a valid amount for Amount Paid on row {n}.\nValue must be greater than 0."
                ))
            } else if row.applied_current_month <= 0.0 {
                Some(format!(
                    "Please enter a valid amount for Applied Current Month on row {n}.\nValue must be greater than 0."
                ))
            } else if text_limit(&row.miller, 50)
                || text_limit(&row.taxpayer_name, 17)
                || text_limit(&row.or_number, 20)
            {
                Some("Schedule 7 holds at most 50 characters for the miller, 17 for the taxpayer and 20 for the OR number.".to_string())
            } else {
                None
            };
            if let Some(message) = problem {
                err(&field, &message);
                break;
            }
        }
    }

    fn validate_inner(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // `validate` in order.
        if self.tax_relief == Form2550MTaxRelief::YesUnspecified {
            err(
                "tax_relief",
                "Please select an option on item no. 11. Entry must not be empty.",
            );
        }
        if self.taxable_year == 0 {
            err("taxable_year", "Please indicate a valid Year.");
        } else if self.taxable_year < 2000 {
            err(
                "taxable_year",
                "Invalid date entry on Item no.1. Entry should not be lower than 2000.",
            );
        } else if self.taxable_year > 9999 {
            err("taxable_year", "Item 1 year holds four digits.");
        }
        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        let tin_digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        if tin1.len() != 3
            || tin2.len() != 3
            || tin3.len() != 3
            || tin_digits.len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 4.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        let lob = self.line_of_business.trim();
        if lob.is_empty() || lob.chars().count() > 60 {
            err(
                "line_of_business",
                "Please enter a valid Line of Business on Item 6.",
            );
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 7.",
            );
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() {
            err(
                "contact_number",
                "Please enter Taxpayer's telephone number on Item 8.",
            );
        } else if phone.len() > 15 || !digits_only(phone) {
            err(
                "contact_number",
                "Item 8 Telephone No. takes up to 15 digits only.",
            );
        }
        let address = self.registered_address.trim();
        if address.is_empty() {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 9.",
            );
        } else if address.chars().count() > 150 {
            err(
                "registered_address",
                "Item 9 Registered Address holds at most 150 characters.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() {
            err("zip_code", "Please enter Taxpayer's Zip code on Item 10.");
        } else if zip.len() > 8 || !digits_only(zip) {
            err("zip_code", "Item 10 Zip Code takes up to 8 digits only.");
        }

        // The page leaves these to its controls; the filename needs them.
        if !(1..=12).contains(&self.month) {
            err("month", "Select the month on Item 1.");
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most two digits.",
            );
        }
        // App rule, not an official alert: BIR-Form2550M.hta has no email
        // check, but the confirmation email needs a usable address.
        let email = self.email.trim();
        if email.is_empty()
            || !email.contains('@')
            || email.contains(char::is_whitespace)
            || email.chars().count() > 60
        {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }

        // Schedule 1.
        for (index, row) in self.sales_schedule.iter().enumerate() {
            let field = format!("sales_schedule[{index}]");
            if form_2550m_atc_index(&row.atc_code).is_none() {
                err(
                    &field,
                    &format!("{} is not an ATC the official form offers.", row.atc_code),
                );
            }
            if self.sales_schedule[..index]
                .iter()
                .any(|other| other.atc_code == row.atc_code)
            {
                err(&field, "Each ATC appears once in Schedule 1.");
            }
            if row.amount <= 0.0 {
                err(
                    &field,
                    &format!(
                        "Please enter valid value for the Amount of Sales/Receipt for the period on row {}",
                        index + 1
                    ),
                );
            }
        }
        self.validate_schedules(&mut err);

        // showSched4 / showSched5 refuse to open without the sales they allocate.
        if self.schedule_4.is_some() && self.sales_to_government <= 0.0 {
            err(
                "schedule_4",
                "Please enter a valid value on Item 13A to be able to load the Schedule 4.",
            );
        }
        if self.schedule_5.is_some() && self.exempt_sales <= 0.0 {
            err(
                "schedule_5",
                "Please enter a valid value on Item 15 to be able to load the Schedule 5.",
            );
        }

        // `round` keeps 12 integer digits; `numbersonly` takes no minus sign.
        let mut amounts: Vec<(String, f64)> = vec![
            ("sales_to_government".into(), self.sales_to_government),
            ("output_tax_government".into(), self.output_tax_government),
            ("zero_rated_sales".into(), self.zero_rated_sales),
            ("exempt_sales".into(), self.exempt_sales),
            ("input_tax_carried_over".into(), self.input_tax_carried_over),
            (
                "input_tax_deferred_capital_goods".into(),
                self.input_tax_deferred_capital_goods,
            ),
            ("transitional_input_tax".into(), self.transitional_input_tax),
            ("presumptive_input_tax".into(), self.presumptive_input_tax),
            ("other_input_tax".into(), self.other_input_tax),
            ("domestic_goods".into(), self.domestic_goods),
            (
                "domestic_goods_input_tax".into(),
                self.domestic_goods_input_tax,
            ),
            ("imported_goods".into(), self.imported_goods),
            (
                "imported_goods_input_tax".into(),
                self.imported_goods_input_tax,
            ),
            ("domestic_services".into(), self.domestic_services),
            (
                "domestic_services_input_tax".into(),
                self.domestic_services_input_tax,
            ),
            ("non_resident_services".into(), self.non_resident_services),
            (
                "non_resident_services_input_tax".into(),
                self.non_resident_services_input_tax,
            ),
            (
                "purchases_not_qualified".into(),
                self.purchases_not_qualified,
            ),
            ("other_purchases".into(), self.other_purchases),
            (
                "other_purchases_input_tax".into(),
                self.other_purchases_input_tax,
            ),
            (
                "standard_input_tax_government".into(),
                self.standard_input_tax_government,
            ),
            ("vat_refund_claimed".into(), self.vat_refund_claimed),
            ("other_deductions".into(), self.other_deductions),
            ("vat_paid_previous".into(), self.vat_paid_previous),
            ("advance_payments".into(), self.advance_payments),
            ("other_credits".into(), self.other_credits),
            ("surcharge".into(), self.surcharge),
            ("interest".into(), self.interest),
            ("compromise".into(), self.compromise),
        ];
        for (index, row) in self.sales_schedule.iter().enumerate() {
            amounts.push((format!("sales_schedule[{index}].amount"), row.amount));
        }
        for (i, row) in self.schedule_2.iter().enumerate() {
            amounts.push((format!("schedule_2[{i}].amount"), row.amount));
            amounts.push((format!("schedule_2[{i}].input_tax"), row.input_tax));
        }
        for (name, rows) in [
            ("schedule_3a", &self.schedule_3a),
            ("schedule_3b", &self.schedule_3b),
        ] {
            for (i, row) in rows.iter().enumerate() {
                amounts.push((format!("{name}[{i}].amount"), row.amount));
                amounts.push((format!("{name}[{i}].input_tax"), row.input_tax));
            }
        }
        for (name, rows) in [
            ("schedule_6", &self.schedule_6),
            ("schedule_8", &self.schedule_8),
        ] {
            for (i, row) in rows.iter().enumerate() {
                amounts.push((format!("{name}[{i}].income_payment"), row.income_payment));
                amounts.push((format!("{name}[{i}].total_withheld"), row.total_withheld));
                amounts.push((
                    format!("{name}[{i}].applied_current_month"),
                    row.applied_current_month,
                ));
            }
        }
        for (i, row) in self.schedule_7.iter().enumerate() {
            amounts.push((format!("schedule_7[{i}].amount_paid"), row.amount_paid));
            amounts.push((
                format!("schedule_7[{i}].applied_current_month"),
                row.applied_current_month,
            ));
        }
        for (name, schedule) in [
            ("schedule_4", self.schedule_4),
            ("schedule_5", self.schedule_5),
        ] {
            if let Some(schedule) = schedule {
                amounts.push((
                    format!("{name}.direct_input_tax"),
                    schedule.direct_input_tax,
                ));
                amounts.push((
                    format!("{name}.not_direct_input_tax"),
                    schedule.not_direct_input_tax,
                ));
            }
        }
        for (field, value) in amounts {
            if !value.is_finite() || !(0.0..1e12).contains(&value) || !has_cent_precision(value) {
                err(
                    &field,
                    "Enter a non-negative amount below 1,000,000,000,000 in pesos and centavos.",
                );
            }
        }
        if !self.is_amended && self.vat_paid_previous != 0.0 {
            err(
                "vat_paid_previous",
                "Item 23D applies only to an amended return.",
            );
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

/// A schedule date (`MM/DD/YYYY`) the way the schedule checks read it:
/// within the return period, or before it for Schedule 3B.
fn date_problem(
    value: &str,
    row: usize,
    period: (u32, u32),
    before_period: bool,
) -> Option<String> {
    let format_error = || {
        Some(format!(
            "Please enter a valid date for Date of Purchase on row {row}.\nPlease enter a date in the MM/DD/YYYY format."
        ))
    };
    let parts: Vec<&str> = value.trim().split('/').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !digits_only(p)) {
        return format_error();
    }
    let (Ok(month), Ok(day), Ok(year)) = (
        parts[0].parse::<u32>(),
        parts[1].parse::<u32>(),
        parts[2].parse::<u32>(),
    ) else {
        return format_error();
    };
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let outside = if before_period {
        (year, month) >= period
    } else {
        (year, month) != period
    };
    if !(1..=12).contains(&month) || day < 1 || day > days || outside {
        return Some(format!(
            "Invalid entry on row {row}. Date of Purchase must be within the Return Period only."
        ));
    }
    None
}

impl FormValidator for Form2550MDraft {
    /// `validate` in order with its alert texts, plus the limits the page
    /// enforces while typing (`maxlength`, `wholenumber`, `numbersonly`) and
    /// the Schedule 1, 4 and 5 rules.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_inner()
    }
}

impl Form2550MDraft {
    /// Items 25A-25C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "25A Surcharge",
                interest: "25B Interest",
                compromise: "25C Compromise",
            ],
            controls: [
                "frm2550m:txtTax25A",
                "frm2550m:txtTax25B",
                "frm2550m:txtTax25C",
            ],
            state: penalty,
            basic_tax_due: "Item 24", |d| d.tax_still_payable,
        )
    }
}

impl QueueableForm for Form2550MDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_bir_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2550M";
    /// Official `formType` (`2550M`).
    const FORM_TYPE: &'static str = "2550M";
    const LAYOUT_ID: &'static str = FORM_2550M_FORM_ID;

    /// Item 26 Total Amount Payable.
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
        FilingPeriod::Monthly(self.month)
    }
    /// `RtnYear + txtYear`, as in `createXMLFileName`.
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

    /// The fixed layout, with the schedule row controls the upload
    /// writes between Item 26 and the Schedule 1 totals.
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
        // Every control on this page is followed by the same separator.
        let separator = &layout.lead;
        for (anchor, rows) in self.schedule_row_controls() {
            if rows.is_empty() {
                continue;
            }
            let anchor = format!("<div>{anchor}=");
            let at = payload
                .find(&anchor)
                .ok_or_else(|| xml_error(format!("{anchor} is missing from the layout")))?;
            if let Some((id, _)) = rows.iter().find(|(id, _)| !is_schedule_row_key(id)) {
                return Err(xml_error(format!(
                    "{id} is not an official schedule row control"
                )));
            }
            let controls: String = rows
                .iter()
                .map(|(id, value)| format!("<div>{id}={value}{id}=</div>{separator}"))
                .collect();
            payload.insert_str(at, &controls);
        }
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Form2550MDraft {
        let mut draft = Form2550MDraft {
            id: None,
            month: 6,
            taxable_year: 2022,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            line_of_business: "Sample Consulting Services".to_string(),
            taxpayer_name: "Sample Dummy Taxpayer".to_string(),
            contact_number: "09170000000".to_string(),
            registered_address: "123 Sample Street, Quezon City".to_string(),
            zip_code: "1100".to_string(),
            email: "Sample.Taxpayer@example.com".to_string(),
            tax_relief: Form2550MTaxRelief::No,
            ..Form2550MDraft::blank()
        };
        draft.add_sales_atc("VT010").unwrap();
        draft.add_sales_atc("VB010").unwrap();
        // Rows follow the popup order: VB010, then VT010.
        draft.sales_schedule[0].amount = 2_345.67;
        draft.sales_schedule[1].amount = 100_000.005;
        draft.set_sales_to_government(50_000.0);
        draft.exempt_sales = 10_000.0;
        draft.input_tax_carried_over = 1_000.0;
        draft.set_purchase(Form2550MPurchase::DomesticGoods, 20_000.0);
        draft.set_purchase(Form2550MPurchase::DomesticServices, 3_333.33);
        draft.schedule_4 = Some(Form2550MAllocation {
            direct_input_tax: 500.0,
            not_direct_input_tax: 1_200.0,
        });
        draft.schedule_5 = Some(Form2550MAllocation {
            direct_input_tax: 100.0,
            not_direct_input_tax: 300.0,
        });
        draft.surcharge = 25.5;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    impl Form2550MDraft {
        fn blank() -> Self {
            let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
                "id": null,
                "full_name": "",
                "tin": {"segment1": "", "segment2": "", "segment3": "", "branch": ""},
                "rdo_code": "",
                "line_of_business": "",
                "registered_address": "",
                "zip_code": "",
                "phone": "",
                "email": "",
                "default_form_type": "2550M",
                "taxpayer_type": "Corporation",
                "business_start_date": "2020-01-15",
                "tax_elections": []
            }))
            .expect("blank profile");
            Self::new_from_profile(&profile, 2022, 6)
        }
    }

    fn messages(draft: &Form2550MDraft) -> Vec<String> {
        <Form2550MDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let d = sample();
        // Popup order: VB010 before VT010.
        assert_eq!(d.sales_schedule[0].atc_code, "VB010");
        assert_eq!(d.sales_schedule[1].amount, 100_000.01);
        assert_eq!(d.sales_schedule[1].output_tax, 12_000.0);
        assert_eq!(d.vatable_sales_private, 102_345.68);
        assert_eq!(d.output_tax_private, 12_281.48);
        assert_eq!(d.output_tax_government, 6_000.0);
        assert_eq!(d.standard_input_tax_government, 3_500.0);
        assert_eq!(d.total_sales, 162_345.68);
        assert_eq!(d.total_output_tax, 18_281.48);
        assert_eq!(d.domestic_goods_input_tax, 2_400.0);
        assert_eq!(d.domestic_services_input_tax, 400.0);
        assert_eq!(d.total_current_purchases, 23_333.33);
        assert_eq!(d.total_available_input_tax, 3_800.0);
        assert_eq!(d.schedule_4_attributed, 369.58);
        assert_eq!(d.schedule_4_total, 869.58);
        assert_eq!(d.input_tax_sales_to_government, -2_630.42);
        assert_eq!(d.schedule_5_attributed, 18.48);
        assert_eq!(d.input_tax_exempt_sales, 118.48);
        assert_eq!(d.total_deductions, -2_511.94);
        assert_eq!(d.total_allowable_input_tax, 6_311.94);
        assert_eq!(d.net_vat_payable, 11_969.54);
        assert_eq!(d.total_amount_payable, 11_995.04);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let d = sample();
        let fields = d.to_bir_field_map();
        assert_eq!(fields["frm2550m:RtnYear"], "06");
        assert_eq!(fields["frm2550m:txtTaxPayerName"], "SAMPLE DUMMY TAXPAYER");
        assert_eq!(fields["txtEmail"], "Sample.Taxpayer@example.com");
        assert_eq!(fields["frm2550m:txtBranchCode"], "00000");
        assert_eq!(fields["frm2550m:txtTax12A"], "102,345.68");
        assert_eq!(fields["frm2550m:txtAtcCde2"], "VT010");
        assert_eq!(fields["frm2550m:txtAmountSales2"], "100,000.01");
        assert_eq!(fields["AtcCode1"], "true");
        assert_eq!(fields["AtcCode34"], "true");
        assert_eq!(fields["AtcCode2"], "false");
        assert_eq!(fields["txtlessStdTaxSched4"], "3,500.00");
        assert_eq!(fields["frm2550m:txtTax20B"], "-2,630.42");
        assert_eq!(
            d.submission_filename(),
            "12345678800000-2550M-062022#Sample.Taxpayer@example.com#.xml"
        );
        let payload = d.to_bir_xml_payload().unwrap();
        let rows = payload.find("frm2550m:txtAtcCde1=VB010").unwrap();
        let totals = payload.find(SCHEDULE_1_ANCHOR).unwrap();
        let item_26 = payload.find("<div>frm2550m:txtTax26=").unwrap();
        assert!(item_26 < rows && rows < totals);
    }

    #[test]
    fn fresh_return_keeps_the_page_defaults() {
        let mut d = Form2550MDraft::blank();
        d.recompute();
        assert_eq!(d.standard_input_tax_government, 5.0);
        let fields = d.layout_fields();
        assert_eq!(fields["txtlessStdTaxSched4"], "5.00");
        assert_eq!(fields["txtTotal20BSched4"], "0.00");
    }

    #[test]
    fn period_codes_round_trip() {
        let d = sample();
        assert_eq!(d.period_code(), "062022");
        assert_eq!(
            Form2550MDraft::parse_period_code("062022"),
            Some((2022, FilingPeriod::Monthly(6)))
        );
        assert_eq!(Form2550MDraft::parse_period_code("132022"), None);
        assert_eq!(Form2550MDraft::parse_period_code("62022"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2550MDraft), expected: &str| {
            let mut d = sample();
            mutate(&mut d);
            let found = messages(&d);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.tax_relief = Form2550MTaxRelief::YesUnspecified,
            "Please select an option on item no. 11. Entry must not be empty.",
        );
        check(&|d| d.taxable_year = 0, "Please indicate a valid Year.");
        check(
            &|d| d.taxable_year = 1999,
            "Invalid date entry on Item no.1. Entry should not be lower than 2000.",
        );
        check(
            &|d| d.tin = "1234".into(),
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
            &|d| d.line_of_business.clear(),
            "Please enter a valid Line of Business on Item 6.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 7.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter Taxpayer's telephone number on Item 8.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 9.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip code on Item 10.",
        );
        check(
            &|d| {
                d.sales_schedule[0].amount = 0.0;
                d.recompute();
            },
            "Please enter valid value for the Amount of Sales/Receipt for the period on row 1",
        );
        check(
            &|d| {
                d.set_sales_to_government(0.0);
            },
            "Please enter a valid value on Item 13A to be able to load the Schedule 4.",
        );
        check(
            &|d| {
                d.exempt_sales = 0.0;
                d.recompute();
            },
            "Please enter a valid value on Item 15 to be able to load the Schedule 5.",
        );
        check(&|d| d.month = 0, "Select the month on Item 1.");
    }

    #[test]
    fn schedules_feed_items_18_20a_and_23() {
        let mut d = sample();
        d.schedule_2.push(Form2550MCapitalGoodsRow {
            date_purchased: "06/05/2022".into(),
            description: "Laptop".into(),
            ..Default::default()
        });
        d.set_schedule_2_amount(0, 85_000.55);
        d.schedule_3a.push(Form2550MAmortizedRow {
            date_purchased: "06/10/2022".into(),
            description: "Machine".into(),
            estimated_life: 120,
            recognized_life: 60,
            ..Default::default()
        });
        d.set_schedule_3a_amount(0, 1_500_000.25);
        d.schedule_6.push(Form2550MWithholdingRow {
            period_covered: "06/30/2022".into(),
            withholding_agent: "Agent".into(),
            income_payment: 1_000.0,
            total_withheld: 50.0,
            applied_current_month: 40.0,
        });
        d.recompute();
        let t = d.schedule_totals;
        assert_eq!(d.schedule_2[0].input_tax, 10_200.07);
        assert_eq!(t.schedule_3_input_tax, 180_000.03);
        assert_eq!(d.schedule_3a[0].allowable_input_tax, 3_000.0);
        assert_eq!(t.schedule_3_deferred, 177_000.03);
        assert_eq!(t.schedule_6_applied, 40.0);
        let fields = d.to_bir_field_map();
        assert_eq!(fields["frm2550m:txtTax18A"], "85,000.55");
        assert_eq!(fields["frm2550m:txtTax20A"], "177,000.03");
        assert_eq!(fields["frm2550m:txtTax23A"], "40.00");
        assert_eq!(fields["txtDescription0"], "LAPTOP");
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn schedule_checks_use_the_official_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2550MDraft), expected: &str| {
            let mut d = sample();
            mutate(&mut d);
            d.recompute();
            let found = messages(&d);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        let row2 = |date: &str, amount: f64| Form2550MCapitalGoodsRow {
            date_purchased: date.into(),
            description: "Item".into(),
            amount,
            input_tax: 0.0,
        };
        check(
            &|d| d.schedule_2.push(row2("", 10.0)),
            "Please enter valid row 1 data for Schedule 2.\nEmpty fields are not allowed.",
        );
        check(
            &|d| d.schedule_2.push(row2("07/01/2022", 10.0)),
            "Invalid entry on row 1. Date of Purchase must be within the Return Period only.",
        );
        check(
            &|d| d.schedule_2.push(row2("6/1/22x", 10.0)),
            "Please enter a valid date for Date of Purchase on row 1.\nPlease enter a date in the MM/DD/YYYY format.",
        );
        check(
            &|d| d.schedule_2.push(row2("06/01/2022", 0.0)),
            "Please enter amount for Net of VAT on row 1.\nValue must be greater than 0.",
        );
        check(
            &|d| {
                d.schedule_2.push(row2("06/01/2022", 600_000.0));
                d.schedule_2.push(row2("06/02/2022", 600_000.0));
            },
            "The total aggregate amount should not exceed 1 Million.\n Please re-enter the values of Schedule 2.",
        );
        let row3 = |date: &str, amount: f64, input: f64| Form2550MAmortizedRow {
            date_purchased: date.into(),
            description: "Machine".into(),
            amount,
            input_tax: input,
            estimated_life: 60,
            recognized_life: 60,
            ..Default::default()
        };
        check(
            &|d| d.schedule_3a.push(row3("06/01/2022", 500_000.0, 60_000.0)),
            "The total aggregate amount does not exceed 1 Million.\n Please re-enter the values of Schedule 3.",
        );
        check(
            &|d| {
                let mut row = row3("06/01/2022", 2_000_000.0, 240_000.0);
                row.recognized_life = 61;
                d.schedule_3a.push(row);
            },
            "Please enter a value 1 to 60 for Recognized Life on row 1Part A.",
        );
        check(
            &|d| {
                d.schedule_3b
                    .push(row3("06/01/2022", 2_000_000.0, 100_000.0))
            },
            "Invalid entry on row 1. Date of Purchase must be within the Return Period only.",
        );
        check(
            &|d| {
                d.schedule_6.push(Form2550MWithholdingRow {
                    period_covered: "06/30/2022".into(),
                    withholding_agent: "Agent".into(),
                    income_payment: 1_000.0,
                    total_withheld: 50.0,
                    applied_current_month: 60.0,
                })
            },
            "The Applied Current Month on row 1 should not be greater than the Total Tax Withheld.",
        );
        check(
            &|d| d.schedule_7.push(Form2550MAdvancePaymentRow::default()),
            "Please enter valid row 1 data for Schedule 7.\nEmpty fields are not allowed.",
        );
        check(
            &|d| {
                d.set_sales_to_government(0.0);
                d.schedule_4 = None;
                d.schedule_8.push(Form2550MWithholdingRow {
                    period_covered: "06/30/2022".into(),
                    withholding_agent: "Agency".into(),
                    income_payment: 1_000.0,
                    total_withheld: 50.0,
                    applied_current_month: 50.0,
                })
            },
            "Please enter a valid value on Item 13A to be able to load the Schedule 8.",
        );
    }

    #[test]
    fn schedule_1_popup_rules() {
        let mut d = sample();
        assert!(d.add_sales_atc("VB010").is_err());
        assert!(d.add_sales_atc("XX999").is_err());
        d.remove_sales_atc("VB010");
        assert_eq!(d.sales_schedule.len(), 1);
        assert_eq!(d.to_bir_field_map()["AtcCode1"], "false");
    }

    #[test]
    fn amended_flag_controls_item_23d() {
        let mut d = sample();
        d.vat_paid_previous = 100.0;
        d.recompute();
        assert_eq!(d.vat_paid_previous, 0.0);
        d.is_amended = true;
        d.vat_paid_previous = 100.0;
        d.recompute();
        assert_eq!(d.total_credits, 100.0);
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
    fn every_schedule_row_key_is_an_official_row_control() {
        let mut d = sample();
        d.sales_schedule.push(Form2550MSalesRow::default());
        d.schedule_2.push(Form2550MCapitalGoodsRow::default());
        d.schedule_3a.push(Form2550MAmortizedRow::default());
        d.schedule_3b.push(Form2550MAmortizedRow::default());
        d.schedule_6.push(Form2550MWithholdingRow::default());
        d.schedule_7.push(Form2550MAdvancePaymentRow::default());
        d.schedule_8.push(Form2550MWithholdingRow::default());
        let groups = d.schedule_row_controls();
        assert!(groups.iter().all(|(_, rows)| !rows.is_empty()));
        for (_, rows) in groups {
            for (id, _) in rows {
                assert!(is_schedule_row_key(&id), "{id}");
            }
        }
        assert!(!is_schedule_row_key("txtDescription"));
        assert!(!is_schedule_row_key("txtDescriptionX0"));
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
            <Form2550MDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2550MDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
