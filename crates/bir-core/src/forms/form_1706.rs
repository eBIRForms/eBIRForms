//! BIR Form 1706 — Capital Gains Tax Return for Onerous Transfer of Real
//! Property Classified as Capital Asset (both Taxable and Exempt).
//!
//! Ported from the official `BIR-Form1706.hta` (eBIRForms 7.9.6.2.1): the
//! transaction-type handlers (`cashSale`, `foreclosure`, `enableExempt`,
//! `enableInstallment`), the compute chain (`compareFMVLI` →
//! `computeTaxableBase` → `computeOfTaxDue` → `computeTaxPayable` →
//! `computeOfTotalAmtDue`, `computePenalties`), `validate()` with its exact
//! alert texts, and the uploaded file (`saveEncryptedProfile`) through [`crate::official_xml`].
//!
//! 1706 is event-based: one return per sale. The dashboard's open-ended key
//! and year identify the draft; the official filename carries the date of
//! transaction and the TCT/OCT/CCT number instead
//! (`TIN-1706-MMDDYYYY_TCT#email#.xml`).

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::official_inputs::{
    amount_text, capital, cents, cents_opt, has_cent_precision, is_calendar_date, js_ge, js_num,
    letters_and_digits, numbers_only, split_tin, tin_is_well_formed, to_fixed_2,
    within_round_limit,
};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::official_amount;
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1706_FORM_ID: &str = "1706-v2018";
/// Schedule 1 (unutilized portion of sales proceeds): 10 rows × 2 cells.
pub const FORM_1706_SCHEDULE_1_CELLS: usize = 20;
/// Item 32 rate: `computeOfTaxDue` multiplies Item 31 by `6 / 100`.
pub const FORM_1706_TAX_RATE: f64 = 6.0 / 100.0;

/// Item 4: the ATC follows the seller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1706SellerType {
    /// `opt4`, ATC II420.
    Individual,
    /// `opt4C`, ATC IC420.
    Corporation,
}

/// Item 15 (`j_id391`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1706PropertyClass {
    Residential,
    Agricultural,
    Commercial,
    Industrial,
    CondominiumResidential,
    CondominiumCommercial,
    Others,
}

impl Form1706PropertyClass {
    pub const ALL: [Self; 7] = [
        Self::Residential,
        Self::Commercial,
        Self::CondominiumResidential,
        Self::Agricultural,
        Self::Industrial,
        Self::CondominiumCommercial,
        Self::Others,
    ];

    /// The radio's element id suffix.
    fn radio(self) -> &'static str {
        match self {
            Self::Residential => "_1",
            Self::Agricultural => "_2",
            Self::Commercial => "_3",
            Self::Industrial => "_4",
            Self::CondominiumResidential => "_5",
            Self::CondominiumCommercial => "_6",
            Self::Others => "_8",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Residential => "Residential",
            Self::Agricultural => "Agricultural",
            Self::Commercial => "Commercial",
            Self::Industrial => "Industrial",
            Self::CondominiumResidential => "Condominium Residential",
            Self::CondominiumCommercial => "Condominium Commercial",
            Self::Others => "Others (specify)",
        }
    }
}

/// Item 20 (`rdTreaty` + `selTreaty`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1706TaxRelief {
    #[default]
    No,
    /// "Yes" with nothing picked from the list yet.
    YesUnspecified,
    InternationalTaxTreaty,
    SpecialLaw,
}

impl Form1706TaxRelief {
    pub fn is_yes(self) -> bool {
        !matches!(self, Self::No)
    }

    fn list_value(self) -> &'static str {
        match self {
            Self::No | Self::YesUnspecified => "0",
            Self::InternationalTaxTreaty => "1",
            Self::SpecialLaw => "2",
        }
    }
}

/// Item 21 (`j_id395`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1706TransactionType {
    CashSale,
    ForeclosureSale,
    Exempt,
    Others,
    InstallmentSale,
}

impl Form1706TransactionType {
    pub const ALL: [Self; 5] = [
        Self::CashSale,
        Self::ForeclosureSale,
        Self::Exempt,
        Self::Others,
        Self::InstallmentSale,
    ];

    fn radio(self) -> &'static str {
        match self {
            Self::CashSale => "_1",
            Self::Exempt => "_2",
            Self::InstallmentSale => "_3",
            Self::ForeclosureSale => "_4",
            Self::Others => "_5",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CashSale => "Cash Sale",
            Self::ForeclosureSale => "Foreclosure Sale",
            Self::Exempt => "Exempt",
            Self::Others => "Others",
            Self::InstallmentSale => "Installment Sale",
        }
    }

    /// Exempt and Others take a description and use Item 30F.
    pub fn needs_description(self) -> bool {
        matches!(self, Self::Exempt | Self::Others)
    }
}

/// Item 30 (`opt30A` … `opt30F`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1706TaxableBase {
    /// 30A Gross selling price.
    GrossSellingPrice,
    /// 30B Bid price (foreclosure sale).
    BidPrice,
    /// 30C Fair market value of land and improvement.
    FairMarketValue,
    /// 30D Taxable installment collected.
    TaxableInstallment,
    /// 30E Unutilized portion of sales proceeds (Items 17 and 18).
    UnutilizedProceeds,
    /// 30F Others.
    Others,
}

impl Form1706TaxableBase {
    pub const ALL: [Self; 6] = [
        Self::GrossSellingPrice,
        Self::BidPrice,
        Self::FairMarketValue,
        Self::TaxableInstallment,
        Self::UnutilizedProceeds,
        Self::Others,
    ];

    fn radio(self) -> &'static str {
        match self {
            Self::GrossSellingPrice => "opt30A",
            Self::BidPrice => "opt30B",
            Self::FairMarketValue => "opt30C",
            Self::TaxableInstallment => "opt30D",
            Self::UnutilizedProceeds => "opt30E",
            Self::Others => "opt30F",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::GrossSellingPrice => "30A Gross selling price",
            Self::BidPrice => "30B Bid price (foreclosure sale)",
            Self::FairMarketValue => "30C FMV of land and improvement",
            Self::TaxableInstallment => "30D Taxable installment collected",
            Self::UnutilizedProceeds => "30E Unutilized portion of sales proceeds",
            Self::Others => "30F Others",
        }
    }
}

/// Overpayment boxes under Item 36 (`opt37`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1706Overpayment {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
}

/// Items 22–28, for an installment sale.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1706Installment {
    /// 22 Selling price.
    pub selling_price: Option<f64>,
    /// 23 Cost and other expenses.
    pub cost_and_expenses: Option<f64>,
    /// 24 Mortgage assumed.
    pub mortgage_assumed: Option<f64>,
    /// 25 Total payments during the initial year.
    pub initial_year_payments: Option<f64>,
    /// 26 Amount of installment this month.
    pub installment_this_month: Option<f64>,
    /// 27 No. of installments in the contract (the page formats it like an
    /// amount, `round(this,2)`).
    pub number_of_installments: Option<f64>,
    /// 28 Date of installment, as typed (`numbersonly`).
    pub date_month: String,
    pub date_day: String,
    pub date_year: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1706Draft {
    #[serde(default)]
    pub id: Option<i64>,

    /// Dashboard year and open-ended key that identify this return.
    pub filing_year: u16,
    pub open_ended_key: u32,

    // Items 1–4
    pub transaction_month: u8,
    pub transaction_day: u8,
    pub transaction_year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,
    pub seller_type: Option<Form1706SellerType>,

    // Part I
    /// Item 5: the seller (this taxpayer), 14 digits with branch.
    pub tin: String,
    /// Item 6.
    pub rdo_code: String,
    /// Item 7: buyer TIN (9 digits) and branch code.
    pub buyer_tin: String,
    pub buyer_branch_code: String,
    /// Item 8.
    pub buyer_rdo_code: String,
    pub seller_name: String,
    pub buyer_name: String,
    pub seller_address: String,
    pub buyer_address: String,
    /// Item 13 (individual sellers).
    #[serde(default)]
    pub seller_residence_address: String,
    pub property_location: String,
    /// Item 14A.
    pub property_rdo_code: String,
    pub property_class: Option<Form1706PropertyClass>,
    #[serde(default)]
    pub property_class_other: String,
    /// Item 16.
    pub tct_number: String,
    #[serde(default)]
    pub area_sold: String,
    #[serde(default)]
    pub tax_declaration_number: String,
    #[serde(default)]
    pub property_other_description: String,
    /// Items 17–19.
    pub principal_residence: Option<bool>,
    pub new_residence_within_18_months: Option<bool>,
    pub covers_multiple_properties: Option<bool>,
    #[serde(default)]
    pub tax_relief: Form1706TaxRelief,
    /// Item 21.
    pub transaction_type: Option<Form1706TransactionType>,
    #[serde(default)]
    pub transaction_description: String,
    #[serde(default)]
    pub installment: Form1706Installment,

    // Item 29
    pub fmv_land_tax_declaration: Option<f64>,
    pub fmv_land_zonal: Option<f64>,
    pub fmv_improvements_tax_declaration: Option<f64>,
    pub fmv_improvements_bir: Option<f64>,

    // Item 30
    pub taxable_base_option: Option<Form1706TaxableBase>,
    pub gross_selling_price: Option<f64>,
    pub bid_price: Option<f64>,
    /// 30C, computed by `compareFMVLI`.
    pub fair_market_value: Option<f64>,
    pub taxable_installment: Option<f64>,
    pub unutilized_proceeds: Option<f64>,
    #[serde(default)]
    pub others_description: String,
    pub others_amount: Option<f64>,

    // Part II
    /// 31.
    #[serde(default)]
    pub taxable_base: f64,
    /// 32.
    #[serde(default)]
    pub tax_due: f64,
    /// 33, amended returns only.
    #[serde(default)]
    pub tax_paid_previous: f64,
    /// 34.
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
    /// 36.
    #[serde(default)]
    pub total_amount_payable: f64,
    #[serde(default)]
    pub overpayment: Form1706Overpayment,

    /// Schedule 1 cells in page order (free text).
    #[serde(default)]
    pub schedule_1: Vec<String>,

    pub email: String,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// The four Item 29 sums `compareFMVLI` compares, blank fields as `NaN`.
fn fmv_strict_max(a: Option<f64>, b: Option<f64>, c: Option<f64>, d: Option<f64>) -> Option<f64> {
    let (a, b, c, d) = (js_num(a), js_num(b), js_num(c), js_num(d));
    let ab = a + b;
    let cd = c + d;
    let bc = b + c;
    let ad = a + d;
    if ab > cd && ab > bc && ab > ad {
        Some(ab)
    } else if cd > bc && cd > ad && cd > ab {
        Some(cd)
    } else if bc > ad && bc > ab && bc > cd {
        Some(bc)
    } else if ad > ab && ad > cd && ad > bc {
        Some(ad)
    } else {
        None
    }
}

/// `formatCurrency` of a value that may be `NaN` (`"0.00"`).
fn format_nan(value: f64) -> f64 {
    if value.is_nan() { 0.0 } else { cents(value) }
}

impl Form1706Draft {
    pub const FORM_CODE: &'static str = "1706";

    /// Dated today when `year` is the current year (the official `init()`
    /// fills today's date), else January 1 -- same rule as 1606.
    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, open_ended_key: u32) -> Self {
        use chrono::Datelike;
        let today = chrono::Local::now().date_naive();
        let (month, day) = if i32::from(year) == today.year() {
            (today.month() as u8, today.day() as u8)
        } else {
            (1, 1)
        };
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            filing_year: year,
            open_ended_key,
            transaction_month: month,
            transaction_day: day,
            transaction_year: year,
            is_amended: false,
            number_of_attached_sheets: 0,
            seller_type: None,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            buyer_tin: String::new(),
            buyer_branch_code: "00000".to_string(),
            buyer_rdo_code: String::new(),
            seller_name: profile.full_name.clone(),
            buyer_name: String::new(),
            seller_address: profile.registered_address.clone(),
            buyer_address: String::new(),
            seller_residence_address: String::new(),
            property_location: String::new(),
            property_rdo_code: String::new(),
            property_class: None,
            property_class_other: String::new(),
            tct_number: String::new(),
            area_sold: String::new(),
            tax_declaration_number: String::new(),
            property_other_description: String::new(),
            principal_residence: None,
            new_residence_within_18_months: None,
            covers_multiple_properties: None,
            tax_relief: Form1706TaxRelief::No,
            transaction_type: None,
            transaction_description: String::new(),
            installment: Form1706Installment::default(),
            fmv_land_tax_declaration: None,
            fmv_land_zonal: None,
            fmv_improvements_tax_declaration: None,
            fmv_improvements_bir: None,
            taxable_base_option: None,
            // The page's initial values: `0.00` in 30A–30D, blank in 30E/30F.
            gross_selling_price: Some(0.0),
            bid_price: Some(0.0),
            fair_market_value: Some(0.0),
            taxable_installment: Some(0.0),
            unutilized_proceeds: None,
            others_description: String::new(),
            others_amount: None,
            taxable_base: 0.0,
            tax_due: 0.0,
            tax_paid_previous: 0.0,
            tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            overpayment: Form1706Overpayment::None,
            schedule_1: Vec::new(),
            email: profile.email.clone(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 4. A corporation clears Items 17 and 18 (`disable17and18`).
    pub fn set_seller_type(&mut self, seller_type: Form1706SellerType) {
        self.seller_type = Some(seller_type);
        if seller_type == Form1706SellerType::Corporation {
            self.principal_residence = None;
            self.new_residence_within_18_months = None;
            if self.taxable_base_option == Some(Form1706TaxableBase::UnutilizedProceeds) {
                self.taxable_base_option = None;
            }
        }
        self.recompute();
    }

    /// Items 17 / 18. A "No" unticks 30E (`disableIndividual`).
    pub fn set_individual_answers(&mut self, principal: Option<bool>, new_residence: Option<bool>) {
        self.principal_residence = principal;
        self.new_residence_within_18_months = new_residence;
        if (principal == Some(false) || new_residence == Some(false))
            && self.taxable_base_option == Some(Form1706TaxableBase::UnutilizedProceeds)
        {
            self.taxable_base_option = None;
        }
        self.recompute();
    }

    /// True when Items 17 and 18 are both "Yes" (`enableIndividual`).
    pub fn unutilized_proceeds_apply(&self) -> bool {
        self.principal_residence == Some(true) && self.new_residence_within_18_months == Some(true)
    }

    /// Item 2. "No" resets Item 33 to `0.00`.
    pub fn set_amended(&mut self, amended: bool) {
        self.is_amended = amended;
        if !amended {
            self.tax_paid_previous = 0.0;
        }
        self.recompute();
    }

    /// Item 21, clearing what the official handler clears.
    pub fn set_transaction_type(&mut self, kind: Form1706TransactionType) {
        self.transaction_type = Some(kind);
        match kind {
            Form1706TransactionType::CashSale => {
                self.installment = Form1706Installment::default();
                self.transaction_description.clear();
                self.taxable_installment = None;
                self.bid_price = None;
                self.others_amount = None;
                self.others_description.clear();
            }
            Form1706TransactionType::ForeclosureSale => {
                self.installment = Form1706Installment::default();
                self.transaction_description.clear();
                self.taxable_installment = None;
                self.others_amount = None;
                self.others_description.clear();
                self.fair_market_value = None;
                self.gross_selling_price = None;
                self.fmv_land_tax_declaration = None;
                self.fmv_land_zonal = None;
                self.fmv_improvements_tax_declaration = None;
                self.fmv_improvements_bir = None;
            }
            Form1706TransactionType::Exempt | Form1706TransactionType::Others => {
                self.installment = Form1706Installment::default();
                self.taxable_installment = None;
                self.bid_price = None;
                self.gross_selling_price = None;
            }
            Form1706TransactionType::InstallmentSale => {
                self.transaction_description.clear();
            }
        }
        self.recompute();
    }

    /// Item 20.
    pub fn set_tax_relief(&mut self, relief: Form1706TaxRelief) {
        self.tax_relief = relief;
    }

    /// The official compute chain, run as if every changed field was blurred.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_paid_previous = 0.0;
        }
        if self.seller_type == Some(Form1706SellerType::Corporation) {
            self.principal_residence = None;
            self.new_residence_within_18_months = None;
        }
        // Item 29: blockletter (parseFloat().toFixed(2)) then round(this,2).
        for value in [
            &mut self.fmv_land_tax_declaration,
            &mut self.fmv_land_zonal,
            &mut self.fmv_improvements_tax_declaration,
            &mut self.fmv_improvements_bir,
        ] {
            *value = value.map(|v| cents(to_fixed_2(v)));
        }
        let installment = &mut self.installment;
        for value in [
            &mut installment.selling_price,
            &mut installment.cost_and_expenses,
            &mut installment.mortgage_assumed,
            &mut installment.initial_year_payments,
            &mut installment.installment_this_month,
            &mut installment.number_of_installments,
        ] {
            *value = cents_opt(*value);
        }
        self.gross_selling_price = cents_opt(self.gross_selling_price);
        self.bid_price = cents_opt(self.bid_price);
        self.taxable_installment = cents_opt(self.taxable_installment);
        self.unutilized_proceeds = cents_opt(self.unutilized_proceeds);
        self.others_amount = cents_opt(self.others_amount);

        // compareFMVLI: only a strict maximum replaces Item 30C.
        if let Some(max) = fmv_strict_max(
            self.fmv_land_tax_declaration,
            self.fmv_improvements_tax_declaration,
            self.fmv_land_zonal,
            self.fmv_improvements_bir,
        ) {
            self.fair_market_value = Some(cents(max));
        }
        self.fair_market_value = cents_opt(self.fair_market_value);

        if let Some(base) = self.official_taxable_base() {
            self.taxable_base = base;
        }
        self.taxable_base = cents(self.taxable_base);
        self.tax_due = cents(self.taxable_base * FORM_1706_TAX_RATE);
        self.tax_paid_previous = cents(self.tax_paid_previous);
        self.tax_payable = cents(self.tax_due - self.tax_paid_previous);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_payable = cents(self.total_penalties + self.tax_payable);
        if self.total_amount_payable >= 0.0 {
            self.overpayment = Form1706Overpayment::None;
        }
    }

    /// `computeTaxableBase`: the Item 31 its rules set, or `None` when no
    /// rule fires (the page then keeps the previous value).
    fn official_taxable_base(&self) -> Option<f64> {
        let gross = js_num(self.gross_selling_price);
        let fmv = js_num(self.fair_market_value);
        let bid = js_num(self.bid_price);
        let foreclosure = self.transaction_type == Some(Form1706TransactionType::ForeclosureSale);
        let mut base = None;
        if js_ge(gross, fmv) {
            base = Some(cents(gross));
        }
        if js_ge(fmv, gross) {
            base = Some(cents(fmv));
        }
        if foreclosure && js_ge(fmv, bid) && js_ge(fmv, gross) {
            base = Some(cents(fmv));
        }
        if foreclosure && js_ge(bid, fmv) && js_ge(bid, gross) {
            base = Some(cents(bid));
        }
        if foreclosure && js_ge(gross, fmv) && js_ge(gross, bid) {
            base = Some(cents(gross));
        }
        match self.transaction_type {
            Some(Form1706TransactionType::InstallmentSale) => {
                base = Some(format_nan(js_num(self.taxable_installment)));
            }
            Some(Form1706TransactionType::Exempt | Form1706TransactionType::Others) => {
                base = Some(format_nan(js_num(self.others_amount)));
            }
            _ => {}
        }
        if self.taxable_base_option == Some(Form1706TaxableBase::UnutilizedProceeds) {
            base = Some(format_nan(js_num(self.unutilized_proceeds)));
        }
        base
    }

    fn date_code(&self) -> String {
        format!(
            "{:02}{:02}{:04}",
            self.transaction_month, self.transaction_day, self.transaction_year
        )
    }

    /// The TCT/OCT/CCT number as the filename carries it (dashes removed).
    fn tct_for_filename(&self) -> String {
        capital(&self.tct_number).replace('-', "")
    }

    fn schedule_cell(&self, index: usize) -> String {
        self.schedule_1
            .get(index)
            .map(|cell| capital(cell))
            .unwrap_or_default()
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// the 2018 sheet needs, never submitted.
    ///
    /// - `derived:seller_tin` / `derived:buyer_tin`: the item 6 and 7 TIN
    ///   cells, one dashed TIN each; `derived:buyer_tin_digits` the page 2
    ///   header comb.
    /// - `derived:line:tax_relief_specify`: Item 11 "If yes, specify".
    /// - `derived:sched1_kind{1,2}` / `derived:sched1_fmv{1,2}` /
    ///   `derived:sched1_total`: the lot (row 1) and the improvement (row 2)
    ///   of the Schedule 1 continuation, the higher FMV of each and the total.
    /// - `derived:sched4_a`: Schedule 4 A, the gross selling price or, on a
    ///   foreclosure, the bid price.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let dashed = |tin: &str| {
            let (a, b, c, d) = split_tin(tin);
            format!("{a}-{b}-{c}-{d}")
        };
        let (b1, b2, b3, _) = split_tin(&self.buyer_tin);
        let buyer_branch = format!("{:0>5}", self.buyer_branch_code.trim());
        let higher = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (None, None) => String::new(),
            (a, b) => official_amount(a.unwrap_or(0.0).max(b.unwrap_or(0.0))),
        };
        let land = (self.fmv_land_tax_declaration, self.fmv_land_zonal);
        let improvement = (
            self.fmv_improvements_tax_declaration,
            self.fmv_improvements_bir,
        );
        let mut put = |key: &str, value: String| {
            fields.insert(format!("derived:{key}"), value);
        };
        put("seller_tin", dashed(&self.tin));
        put("buyer_tin", format!("{b1}-{b2}-{b3}-{buyer_branch}"));
        put("buyer_tin_digits", format!("{b1}{b2}{b3}{buyer_branch}"));
        put(
            "line:tax_relief_specify",
            match self.tax_relief {
                Form1706TaxRelief::InternationalTaxTreaty => "INTERNATIONAL TAX TREATY",
                Form1706TaxRelief::SpecialLaw => "SPECIAL LAW",
                Form1706TaxRelief::No | Form1706TaxRelief::YesUnspecified => "",
            }
            .to_string(),
        );
        for (row, kind, (a, b)) in [(1, "LOT", land), (2, "IMPROVEMENT", improvement)] {
            let shown = a.is_some() || b.is_some();
            put(
                &format!("sched1_kind{row}"),
                if shown { kind } else { "" }.to_string(),
            );
            put(&format!("sched1_fmv{row}"), higher(a, b));
        }
        put("sched1_total", amount_text(self.fair_market_value));
        put(
            "sched4_a",
            amount_text(
                if self.transaction_type == Some(Form1706TransactionType::ForeclosureSale) {
                    self.bid_price
                } else {
                    self.gross_selling_price
                },
            ),
        );
        fields
    }

    /// The official control values the upload loop writes, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1706:{key}"), value);
        };
        let flag = |on: bool| on.to_string();

        put("txtDateMonth", format!("{:02}", self.transaction_month));
        put("txtDateDay", format!("{:02}", self.transaction_day));
        put("txtDateYear", self.transaction_year.to_string());
        put("j_id217:_1", flag(self.is_amended));
        put("j_id217:_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());
        put("txt4", "II420".to_string());
        put(
            "opt4",
            flag(self.seller_type == Some(Form1706SellerType::Individual)),
        );
        put("txt4C", "IC420".to_string());
        put(
            "opt4C",
            flag(self.seller_type == Some(Form1706SellerType::Corporation)),
        );

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtTIN1", tin1);
        put("txtTIN2", tin2);
        put("txtTIN3", tin3);
        put("txtBranchCode", branch);
        put("txtRDOCode", rdo_value(&self.rdo_code));
        let (b1, b2, b3, _) = split_tin(&self.buyer_tin);
        put("txtTINB1", b1);
        put("txtTINB2", b2);
        put("txtTINB3", b3);
        put("txtBranchCodeB", capital(&self.buyer_branch_code));
        put("txtRDOCodeB", rdo_value(&self.buyer_rdo_code));
        put("txtSellerName", capital(&self.seller_name));
        put("txtBuyerName", capital(&self.buyer_name));
        put("txtSellerAddress", capital(&self.seller_address));
        put("txtBuyerAddress", capital(&self.buyer_address));
        put("txtSellerRAddress", capital(&self.seller_residence_address));
        put("txtLocation", capital(&self.property_location));
        put("txtRDOCode14A", rdo_value(&self.property_rdo_code));
        for class in Form1706PropertyClass::ALL {
            put(
                &format!("j_id391:{}", class.radio()),
                flag(self.property_class == Some(class)),
            );
        }
        put(
            "j_id391:_7",
            if self.property_class == Some(Form1706PropertyClass::Others) {
                capital(&self.property_class_other)
            } else {
                String::new()
            },
        );
        put("txtTCT", capital(&self.tct_number));
        put("txtArea", self.area_sold.trim().to_string());
        put("txtTaxDC", self.tax_declaration_number.trim().to_string());
        put("txtOthers", capital(&self.property_other_description));
        let yes_no = |put: &mut dyn FnMut(&str, String), id: &str, answer: Option<bool>| {
            put(&format!("{id}:_1"), flag(answer == Some(true)));
            put(&format!("{id}:_2"), flag(answer == Some(false)));
        };
        yes_no(&mut put, "j_id392", self.principal_residence);
        yes_no(&mut put, "j_id393", self.new_residence_within_18_months);
        yes_no(&mut put, "j_id394", self.covers_multiple_properties);
        put("rdTreaty:_1", flag(self.tax_relief.is_yes()));
        put("rdTreaty:_2", flag(!self.tax_relief.is_yes()));
        put("selTreaty", self.tax_relief.list_value().to_string());
        for kind in Form1706TransactionType::ALL {
            put(
                &format!("j_id395:{}", kind.radio()),
                flag(self.transaction_type == Some(kind)),
            );
        }
        put("txtOthers21", capital(&self.transaction_description));
        let inst = &self.installment;
        put("txtSelling", amount_text(inst.selling_price));
        put("txtCost", amount_text(inst.cost_and_expenses));
        put("txtMortgage", amount_text(inst.mortgage_assumed));
        put("txtTotalP", amount_text(inst.initial_year_payments));
        put("txtAmount", amount_text(inst.installment_this_month));
        put("txtTotalN", amount_text(inst.number_of_installments));
        put("txtDateMonthI", inst.date_month.trim().to_string());
        put("txtDateDayI", inst.date_day.trim().to_string());
        put("txtDateYearI", inst.date_year.trim().to_string());

        // tickCheckboxWithValue ticks a box once its amount is above zero.
        let tick = |value: Option<f64>| flag(value.is_some_and(|v| v > 0.0));
        put("opt29A", tick(self.fmv_land_tax_declaration));
        put("txtFMVLand", amount_text(self.fmv_land_tax_declaration));
        put("opt29C", tick(self.fmv_land_zonal));
        put("txtFMVZonal", amount_text(self.fmv_land_zonal));
        put("opt29B", tick(self.fmv_improvements_tax_declaration));
        put(
            "txtFMVImprovements",
            amount_text(self.fmv_improvements_tax_declaration),
        );
        put("opt29D", tick(self.fmv_improvements_bir));
        put("txtFMVBIR", amount_text(self.fmv_improvements_bir));

        for option in Form1706TaxableBase::ALL {
            put(
                option.radio(),
                flag(self.taxable_base_option == Some(option)),
            );
        }
        put("txtGross", amount_text(self.gross_selling_price));
        put("txtBid", amount_text(self.bid_price));
        put("txtFMVLI", amount_text(self.fair_market_value));
        put("txtInstallment", amount_text(self.taxable_installment));
        put("txtOthers30E", amount_text(self.unutilized_proceeds));
        put("txtOtherss30F", capital(&self.others_description));
        put("txtOthers30F", amount_text(self.others_amount));

        put("txtTax", official_amount(self.taxable_base));
        put("txtRate", official_amount(self.tax_due));
        put("txtLess", official_amount(self.tax_paid_previous));
        put("txtTaxDue", official_amount(self.tax_payable));
        put("txtSurcharge", official_amount(self.surcharge));
        put("txtInterest", official_amount(self.interest));
        put("txtCompromise", official_amount(self.compromise));
        put("txtTotalPenalties", official_amount(self.total_penalties));
        put("txtTotal", official_amount(self.total_amount_payable));
        put(
            "opt37:_1",
            flag(self.overpayment == Form1706Overpayment::Refund),
        );
        put(
            "opt37:_2",
            flag(self.overpayment == Form1706Overpayment::TaxCreditCertificate),
        );
        for index in 0..FORM_1706_SCHEDULE_1_CELLS {
            put(
                &format!("txtSchedModal{}", index + 1),
                self.schedule_cell(index),
            );
        }

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

/// A select value for an RDO control: `000` (the blank option) when unset.
fn rdo_value(code: &str) -> String {
    let code = code.trim();
    if code.is_empty() {
        "000".to_string()
    } else {
        code.to_string()
    }
}

fn rdo_selected(code: &str) -> bool {
    let code = code.trim();
    !code.is_empty() && code != "000"
}

impl FormValidator for Form1706Draft {
    /// `validate()` in order with its alert texts, then `initialValidateBeforeSave`,
    /// plus the limits the page enforces while typing (`maxlength`,
    /// `wholenumber`, `numbersonly`, `letternumber`, `round()`).
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // Item 1. The official check lets month 00 through; a real date is required here.
        if !is_calendar_date(
            self.transaction_year,
            self.transaction_month,
            self.transaction_day,
        ) {
            err("transaction_date", "Invalid date entry on item 1.");
        } else if self.transaction_year < 1904 {
            err(
                "transaction_date",
                "Invalid date entry on Item no.1. Entry should not be lower than 1904.",
            );
        }
        if self.seller_type.is_none() {
            err("seller_type", "Please select an option for Item 4.");
        }
        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        if tin1.is_empty() || tin2.is_empty() || tin3.is_empty() {
            err("tin", "Please enter the Seller's TIN.");
        } else if !tin_is_well_formed(&self.tin) || tin3.len() != 3 {
            err("tin", "Please enter a valid TIN number on Item 5.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !rdo_selected(&self.rdo_code) {
            err("rdo_code", "Please enter the Seller's RDO Code.");
        } else if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err(
                "rdo_code",
                "Please enter a valid Seller's RDO Code on Item 6.",
            );
        }
        let buyer_digits: String = self
            .buyer_tin
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        let buyer_branch = self.buyer_branch_code.trim();
        if buyer_digits.is_empty()
            || buyer_branch.is_empty()
            || buyer_digits.len() != 9
            || !self
                .buyer_tin
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-')
            || buyer_branch.len() > 5
            || !letters_and_digits(buyer_branch)
        {
            err("buyer_tin", "Please enter the Buyer's TIN.");
        } else {
            let seller = split_tin(&self.tin);
            let seller_full = format!("{}{}{}{}", seller.0, seller.1, seller.2, seller.3);
            if seller_full == format!("{buyer_digits}{}", buyer_branch.to_uppercase()) {
                err("buyer_tin", "TIN for Buyer and Seller should be different.");
            }
        }
        if !rdo_selected(&self.buyer_rdo_code)
            || !crate::validation::rdo_code_is_official_option(self.buyer_rdo_code.trim())
        {
            err("buyer_rdo_code", "Please enter the Buyer's RDO Code.");
        }
        let text_limits: [(&str, &str, usize, &str); 5] = [
            (
                "seller_name",
                &self.seller_name,
                50,
                "Please enter the Seller's Name.",
            ),
            (
                "buyer_name",
                &self.buyer_name,
                50,
                "Please enter the Buyer's Name.",
            ),
            (
                "seller_address",
                &self.seller_address,
                70,
                "Please enter the Seller's Address.",
            ),
            (
                "buyer_address",
                &self.buyer_address,
                70,
                "Please enter the Buyer's address.",
            ),
            (
                "property_location",
                &self.property_location,
                70,
                "Please enter the Location of the Property.",
            ),
        ];
        for (field, value, max, message) in text_limits {
            let value = value.trim();
            if value.is_empty() {
                err(field, message);
            } else if value.chars().count() > max {
                err(field, &format!("Item text holds at most {max} characters."));
            }
        }
        if self.seller_residence_address.trim().chars().count() > 70 {
            err(
                "seller_residence_address",
                "Item text holds at most 70 characters.",
            );
        }
        if !rdo_selected(&self.property_rdo_code)
            || !crate::validation::rdo_code_is_official_option(self.property_rdo_code.trim())
        {
            err(
                "property_rdo_code",
                "Please enter the RDO Code on Item 14A.",
            );
        }
        match self.property_class {
            None => err("property_class", "Please select an option for Item 15."),
            Some(Form1706PropertyClass::Others) => {
                let other = self.property_class_other.trim();
                if other.is_empty() || other.chars().count() > 60 {
                    err(
                        "property_class_other",
                        "Please specify the classification of the property on Item 15.",
                    );
                }
            }
            Some(_) => {}
        }
        let tct = self.tct_number.trim();
        if tct.is_empty() {
            err("tct_number", "Please enter the TCT/OCT/CCT No.");
        } else if tct.chars().count() > 20
            || !tct.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            // The number becomes part of the official filename.
            err(
                "tct_number",
                "TCT/OCT/CCT No. holds at most 20 letters, digits or dashes.",
            );
        }
        if self.area_sold.trim().len() > 20 || !numbers_only(self.area_sold.trim()) {
            err(
                "area_sold",
                "Area sold holds at most 20 digits and decimal points.",
            );
        }
        if self.tax_declaration_number.trim().len() > 25
            || !numbers_only(self.tax_declaration_number.trim())
        {
            err(
                "tax_declaration_number",
                "Tax Dec. No. holds at most 25 digits and decimal points.",
            );
        }
        if self.property_other_description.trim().chars().count() > 25 {
            err(
                "property_other_description",
                "Item 16 Others holds at most 25 characters.",
            );
        }
        if self.seller_type == Some(Form1706SellerType::Individual) {
            if self.principal_residence.is_none() {
                err(
                    "principal_residence",
                    "Please select an option for Item 17.",
                );
            }
            if self.new_residence_within_18_months.is_none() {
                err(
                    "new_residence_within_18_months",
                    "Please select an option for Item 18.",
                );
            }
        }
        if self.covers_multiple_properties.is_none() {
            err(
                "covers_multiple_properties",
                "Please select an option for Item 19.",
            );
        }
        if self.transaction_type.is_none() {
            err("transaction_type", "Please select an option for Item 21.");
        }
        if self.tax_relief == Form1706TaxRelief::YesUnspecified {
            err(
                "tax_relief",
                "Please specify tax relief you are availing for Item 20.",
            );
        }
        if self.transaction_type.is_some_and(|t| t.needs_description()) {
            let description = self.transaction_description.trim();
            if description.is_empty() {
                err(
                    "transaction_description",
                    "Please specify a valid description of transaction in item 21.",
                );
            } else if description.chars().count() > 60 {
                err(
                    "transaction_description",
                    "Item 21 description holds at most 60 characters.",
                );
            }
        }
        match self.taxable_base_option {
            None => err(
                "taxable_base_option",
                "Please choose Determination of taxable base in Item 30",
            ),
            Some(Form1706TaxableBase::UnutilizedProceeds) if !self.unutilized_proceeds_apply() => {
                // The page only enables 30E after "Yes" on Items 17 and 18.
                err(
                    "taxable_base_option",
                    "Item 30E applies only when Items 17 and 18 are both Yes.",
                );
            }
            Some(_) => {}
        }
        if self.total_amount_payable < 0.0 && self.overpayment == Form1706Overpayment::None {
            err(
                "overpayment",
                "Please choose if to be refunded or to be issued a Tax Credit Certificate.",
            );
        }

        // Limits the page enforces while typing.
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most two digits.",
            );
        }
        if self.others_description.trim().chars().count() > 40 {
            err(
                "others_description",
                "Item 30F description holds at most 40 characters.",
            );
        }
        for (index, cell) in self.schedule_1.iter().enumerate() {
            if cell.trim().chars().count() > 17 {
                err(
                    &format!("schedule_1[{index}]"),
                    "Each Schedule 1 entry holds at most 17 characters.",
                );
            }
        }
        if self.schedule_1.len() > FORM_1706_SCHEDULE_1_CELLS {
            err("schedule_1", "Schedule 1 has 20 entries.");
        }
        let inst = &self.installment;
        for (field, value, max) in [
            ("installment.date_month", &inst.date_month, 2),
            ("installment.date_day", &inst.date_day, 2),
            ("installment.date_year", &inst.date_year, 4),
        ] {
            let value = value.trim();
            if value.len() > max || !numbers_only(value) {
                err(field, "Item 28 takes the installment date as MM/DD/YYYY.");
            }
        }
        let amounts = [
            ("installment.selling_price", inst.selling_price),
            ("installment.cost_and_expenses", inst.cost_and_expenses),
            ("installment.mortgage_assumed", inst.mortgage_assumed),
            (
                "installment.initial_year_payments",
                inst.initial_year_payments,
            ),
            (
                "installment.installment_this_month",
                inst.installment_this_month,
            ),
            (
                "installment.number_of_installments",
                inst.number_of_installments,
            ),
            ("fmv_land_tax_declaration", self.fmv_land_tax_declaration),
            ("fmv_land_zonal", self.fmv_land_zonal),
            (
                "fmv_improvements_tax_declaration",
                self.fmv_improvements_tax_declaration,
            ),
            ("fmv_improvements_bir", self.fmv_improvements_bir),
            ("gross_selling_price", self.gross_selling_price),
            ("bid_price", self.bid_price),
            ("taxable_installment", self.taxable_installment),
            ("unutilized_proceeds", self.unutilized_proceeds),
            ("others_amount", self.others_amount),
            ("tax_paid_previous", Some(self.tax_paid_previous)),
            ("surcharge", Some(self.surcharge)),
            ("interest", Some(self.interest)),
            ("compromise", Some(self.compromise)),
        ];
        for (field, value) in amounts {
            if let Some(value) = value
                && (value < 0.0 || !has_cent_precision(value) || !within_round_limit(value))
            {
                err(
                    field,
                    "Enter a non-negative amount in pesos and centavos below 1,000,000,000,000.",
                );
            }
        }
        if !self.is_amended && self.tax_paid_previous != 0.0 {
            err(
                "tax_paid_previous",
                "Item 33 applies only to an amended return.",
            );
        }
        if self.transaction_type.is_some() && self.official_taxable_base().is_none() {
            err(
                "taxable_base",
                "Item 31 cannot be determined yet. Complete Items 29 and 30 for this transaction.",
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

impl Form1706Draft {
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
                "frm1706:txtSurcharge",
                "frm1706:txtInterest",
                "frm1706:txtCompromise",
            ],
            state: penalty,
            basic_tax_due: "Item 16", |d| d.tax_payable,
        )
    }
}

impl QueueableForm for Form1706Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1706";
    /// Official `formType` (`1706`).
    const FORM_TYPE: &'static str = "1706";
    const LAYOUT_ID: &'static str = FORM_1706_FORM_ID;

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
    /// `MMDDYYYY + "_" + TCT` without dashes, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{}_{}", self.date_code(), self.tct_for_filename())
    }
    /// The filename names the sale (date and title), not the dashboard's
    /// open-ended key, so a receipt cannot be mapped back to a draft row.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Form1706Draft {
        let mut draft = Form1706Draft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            filing_year: 2025,
            open_ended_key: 1,
            transaction_month: 3,
            transaction_day: 15,
            transaction_year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            seller_type: None,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            buyer_tin: "111-222-333".to_string(),
            buyer_branch_code: "00000".to_string(),
            buyer_rdo_code: "040".to_string(),
            seller_name: "Sample Seller, Jr.".to_string(),
            buyer_name: "Sample Buyer".to_string(),
            seller_address: "1 Seller St".to_string(),
            buyer_address: "2 Buyer St".to_string(),
            seller_residence_address: String::new(),
            property_location: "Lot 3 Sample".to_string(),
            property_rdo_code: "039".to_string(),
            property_class: Some(Form1706PropertyClass::Residential),
            property_class_other: String::new(),
            tct_number: "T-12345".to_string(),
            area_sold: String::new(),
            tax_declaration_number: String::new(),
            property_other_description: String::new(),
            principal_residence: None,
            new_residence_within_18_months: None,
            covers_multiple_properties: Some(false),
            tax_relief: Form1706TaxRelief::No,
            transaction_type: None,
            transaction_description: String::new(),
            installment: Form1706Installment::default(),
            fmv_land_tax_declaration: None,
            fmv_land_zonal: None,
            fmv_improvements_tax_declaration: None,
            fmv_improvements_bir: None,
            taxable_base_option: None,
            gross_selling_price: Some(0.0),
            bid_price: Some(0.0),
            fair_market_value: Some(0.0),
            taxable_installment: Some(0.0),
            unutilized_proceeds: None,
            others_description: String::new(),
            others_amount: None,
            taxable_base: 0.0,
            tax_due: 0.0,
            tax_paid_previous: 0.0,
            tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            overpayment: Form1706Overpayment::None,
            schedule_1: Vec::new(),
            email: "sample.taxpayer@example.com".to_string(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.set_seller_type(Form1706SellerType::Individual);
        draft.set_individual_answers(Some(false), Some(false));
        draft.set_transaction_type(Form1706TransactionType::CashSale);
        draft.fmv_land_tax_declaration = Some(1_000_000.005);
        draft.fmv_land_zonal = Some(1_200_000.0);
        draft.fmv_improvements_tax_declaration = Some(500_000.0);
        draft.fmv_improvements_bir = Some(400_000.0);
        draft.taxable_base_option = Some(Form1706TaxableBase::GrossSellingPrice);
        draft.gross_selling_price = Some(1_500_000.0);
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1706Draft) -> Vec<String> {
        <Form1706Draft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.fmv_land_tax_declaration, Some(1_000_000.01));
        assert_eq!(draft.fair_market_value, Some(1_700_000.0));
        assert_eq!(draft.taxable_base, 1_700_000.0);
        assert_eq!(draft.tax_due, 102_000.0);
        assert_eq!(draft.tax_payable, 102_000.0);
        assert_eq!(draft.total_amount_payable, 102_000.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn foreclosure_takes_the_highest_of_bid_gross_and_fmv() {
        let mut draft = sample();
        draft.set_transaction_type(Form1706TransactionType::ForeclosureSale);
        // foreclosure() clears Item 29, 30A and 30C.
        assert_eq!(draft.fmv_land_zonal, None);
        assert_eq!(draft.gross_selling_price, None);
        assert_eq!(draft.fair_market_value, None);
        assert!(
            messages(&draft)
                .iter()
                .any(|m| m.starts_with("Item 31 cannot be determined"))
        );
        draft.fmv_land_tax_declaration = Some(100.0);
        draft.fmv_improvements_tax_declaration = Some(50.0);
        draft.fmv_land_zonal = Some(80.0);
        draft.fmv_improvements_bir = Some(10.0);
        draft.gross_selling_price = Some(120.0);
        draft.bid_price = Some(200.0);
        draft.taxable_base_option = Some(Form1706TaxableBase::BidPrice);
        draft.recompute();
        assert_eq!(draft.fair_market_value, Some(150.0));
        assert_eq!(draft.taxable_base, 200.0);
        assert_eq!(draft.tax_due, 12.0);
    }

    #[test]
    fn installment_exempt_and_unutilized_bases() {
        let mut draft = sample();
        draft.set_transaction_type(Form1706TransactionType::InstallmentSale);
        draft.taxable_installment = Some(10_000.0);
        draft.recompute();
        assert_eq!(draft.taxable_base, 10_000.0);
        draft.set_transaction_type(Form1706TransactionType::Exempt);
        draft.others_amount = Some(5_000.0);
        draft.recompute();
        assert_eq!(draft.taxable_base, 5_000.0);
        assert!(
            messages(&draft)
                .iter()
                .any(|m| m == "Please specify a valid description of transaction in item 21.")
        );
        draft.set_individual_answers(Some(true), Some(true));
        draft.taxable_base_option = Some(Form1706TaxableBase::UnutilizedProceeds);
        draft.unutilized_proceeds = Some(2_500.0);
        draft.recompute();
        assert_eq!(draft.taxable_base, 2_500.0);
        assert_eq!(draft.tax_due, 150.0);
    }

    #[test]
    fn amended_overpayment_needs_a_box() {
        let mut draft = sample();
        draft.set_amended(true);
        draft.tax_paid_previous = 200_000.0;
        draft.recompute();
        assert_eq!(draft.tax_payable, -98_000.0);
        assert!(
            messages(&draft).iter().any(|m| m
                == "Please choose if to be refunded or to be issued a Tax Credit Certificate.")
        );
        draft.overpayment = Form1706Overpayment::Refund;
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        draft.set_amended(false);
        assert_eq!(draft.tax_paid_previous, 0.0);
        assert_eq!(draft.overpayment, Form1706Overpayment::None);
    }

    #[test]
    fn field_map_uses_official_formats() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1706:txtDateMonth"], "03");
        assert_eq!(fields["frm1706:txtSellerName"], "SAMPLE SELLER, JR.");
        assert_eq!(fields["frm1706:txtFMVLand"], "1,000,000.01");
        assert_eq!(fields["frm1706:opt29A"], "true");
        assert_eq!(fields["frm1706:txtBid"], "");
        assert_eq!(fields["frm1706:txtTax"], "1,700,000.00");
        assert_eq!(fields["frm1706:txtRDOCodeB"], "040");
        assert_eq!(fields["frm1706:txt4"], "II420");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1706-03152025_T12345#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_code_names_the_sale() {
        let draft = sample();
        assert_eq!(draft.period_code(), "03152025_T12345");
        assert_eq!(draft.filing_period(), FilingPeriod::OpenEnded(1));
        assert_eq!(Form1706Draft::parse_period_code("03152025_T12345"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1706Draft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| {
                d.transaction_month = 4;
                d.transaction_day = 31;
            },
            "Invalid date entry on item 1.",
        );
        check(
            &|d| {
                d.transaction_month = 2;
                d.transaction_day = 29;
            },
            "Invalid date entry on item 1.",
        );
        check(
            &|d| d.transaction_month = 0,
            "Invalid date entry on item 1.",
        );
        check(
            &|d| d.transaction_year = 1903,
            "Invalid date entry on Item no.1. Entry should not be lower than 1904.",
        );
        check(
            &|d| d.seller_type = None,
            "Please select an option for Item 4.",
        );
        check(&|d| d.tin = String::new(), "Please enter the Seller's TIN.");
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter the Seller's RDO Code.",
        );
        check(&|d| d.buyer_tin.clear(), "Please enter the Buyer's TIN.");
        check(
            &|d| d.buyer_tin = "123-456-788".into(),
            "TIN for Buyer and Seller should be different.",
        );
        check(
            &|d| d.buyer_rdo_code.clear(),
            "Please enter the Buyer's RDO Code.",
        );
        check(
            &|d| d.seller_name.clear(),
            "Please enter the Seller's Name.",
        );
        check(&|d| d.buyer_name.clear(), "Please enter the Buyer's Name.");
        check(
            &|d| d.seller_address.clear(),
            "Please enter the Seller's Address.",
        );
        check(
            &|d| d.buyer_address.clear(),
            "Please enter the Buyer's address.",
        );
        check(
            &|d| d.property_location.clear(),
            "Please enter the Location of the Property.",
        );
        check(
            &|d| d.property_rdo_code.clear(),
            "Please enter the RDO Code on Item 14A.",
        );
        check(
            &|d| d.property_class = None,
            "Please select an option for Item 15.",
        );
        check(
            &|d| d.tct_number.clear(),
            "Please enter the TCT/OCT/CCT No.",
        );
        check(
            &|d| d.principal_residence = None,
            "Please select an option for Item 17.",
        );
        check(
            &|d| d.new_residence_within_18_months = None,
            "Please select an option for Item 18.",
        );
        check(
            &|d| d.covers_multiple_properties = None,
            "Please select an option for Item 19.",
        );
        check(
            &|d| d.transaction_type = None,
            "Please select an option for Item 21.",
        );
        check(
            &|d| d.tax_relief = Form1706TaxRelief::YesUnspecified,
            "Please specify tax relief you are availing for Item 20.",
        );
        check(
            &|d| d.set_transaction_type(Form1706TransactionType::Others),
            "Please specify a valid description of transaction in item 21.",
        );
        check(
            &|d| d.taxable_base_option = None,
            "Please choose Determination of taxable base in Item 30",
        );
    }

    #[test]
    fn corporations_skip_items_17_and_18() {
        let mut draft = sample();
        draft.set_seller_type(Form1706SellerType::Corporation);
        assert_eq!(draft.principal_residence, None);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1706:opt4C"], "true");
        assert_eq!(fields["frm1706:j_id392:_2"], "false");
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
}
