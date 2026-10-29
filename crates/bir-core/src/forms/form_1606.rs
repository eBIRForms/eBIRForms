//! BIR Form 1606 — Withholding Tax Remittance Return for Onerous Transfer of
//! Real Property Other Than Capital Asset (Including Taxable and Exempt).
//!
//! Ported from the official `BIR-Form1606.hta` (eBIRForms 7.9.6.2.1): the
//! taxable-base chain (`compareFMVLI`, `computeTaxableBase`,
//! `computeOfTaxRequired` … `computeOfTotalAmtDue`), the transaction-type
//! handlers that clear and disable fields (`cashSale`, `foreclosure`,
//! `enableExempt`, `enableInstallment`), `validate` and
//! `initialValidateBeforeSave` with their exact alert texts, and
//! `saveXMLsubmit` through [`crate::official_xml`].
//!
//! The filer is the buyer (withholding agent); the profile supplies the
//! buyer's TIN, RDO, name and address. Each return covers one transaction:
//! its filename period is the transaction date plus the TCT/OCT/CCT number
//! (`createXMLFileName`).

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
pub const FORM_1606_FORM_ID: &str = "1606-v2018";

/// `round()` turns anything with more than 12 integer digits into `0.00`.
const MAX_AMOUNT: f64 = 1_000_000_000_000.0;

/// Item 13: the seller's ATC (`optATC13_2` / `optATC13_3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1606SellerType {
    /// WI155 — individual seller.
    Individual,
    /// WC155 — corporate seller.
    Corporation,
}

/// Item 14 (`j_id392`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1606AgentCategory {
    Private,
    Government,
}

/// Item 15 (`j_id393`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1606PropertyClass {
    Residential,
    Commercial,
    CondominiumResidential,
    Agricultural,
    Industrial,
    CondominiumCommercial,
    /// "Others (Specify)" with [`Form1606Draft::property_class_other`].
    Others,
}

impl Form1606PropertyClass {
    pub const ALL: [Self; 7] = [
        Self::Residential,
        Self::Commercial,
        Self::CondominiumResidential,
        Self::Agricultural,
        Self::Industrial,
        Self::CondominiumCommercial,
        Self::Others,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Residential => "Residential",
            Self::Commercial => "Commercial",
            Self::CondominiumResidential => "Condominium Residential",
            Self::Agricultural => "Agricultural",
            Self::Industrial => "Industrial",
            Self::CondominiumCommercial => "Condominium Commercial",
            Self::Others => "Others (Specify)",
        }
    }
}

/// Item 19 (`rdTreaty` + `selTreaty`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1606TaxRelief {
    #[default]
    No,
    /// "Yes" with nothing picked from the list yet.
    YesUnspecified,
    InternationalTaxTreaty,
    SpecialLaw,
}

impl Form1606TaxRelief {
    pub fn is_yes(self) -> bool {
        !matches!(self, Self::No)
    }

    fn select_value(self) -> &'static str {
        match self {
            Self::No | Self::YesUnspecified => "",
            Self::InternationalTaxTreaty => "0",
            Self::SpecialLaw => "1",
        }
    }
}

/// Item 20 (`j_id395`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1606Transaction {
    CashSale,
    ForeclosureSale,
    Exempt,
    Others,
    InstallmentSale,
}

impl Form1606Transaction {
    pub const ALL: [Self; 5] = [
        Self::CashSale,
        Self::ForeclosureSale,
        Self::Exempt,
        Self::Others,
        Self::InstallmentSale,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::CashSale => "Cash Sale",
            Self::ForeclosureSale => "Foreclosure Sale",
            Self::Exempt => "Exempt",
            Self::Others => "Others",
            Self::InstallmentSale => "Installment Sale",
        }
    }

    fn is_exempt_or_others(self) -> bool {
        matches!(self, Self::Exempt | Self::Others)
    }
}

/// The box ticked on Item 28 (`opt28A` … `opt28E`, one radio group). It is
/// informational: the official Item 30 depends on Item 20 only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1606TaxableBaseBox {
    /// 28A Gross Selling Price.
    GrossSellingPrice,
    /// 28B FMV of Land and Improvement.
    FairMarketValue,
    /// 28C Bid Price.
    BidPrice,
    /// 28D Installment Collected.
    InstallmentCollected,
    /// 28E Others.
    Others,
}

/// Item 31 options (`txtTaxRate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form1606TaxRate {
    #[serde(rename = "0")]
    Zero,
    #[serde(rename = "1.5")]
    OnePointFive,
    #[serde(rename = "3")]
    Three,
    #[serde(rename = "5")]
    Five,
    #[serde(rename = "6")]
    Six,
}

impl Form1606TaxRate {
    pub const ALL: [Self; 5] = [
        Self::Zero,
        Self::OnePointFive,
        Self::Three,
        Self::Five,
        Self::Six,
    ];

    /// The option value the select holds.
    pub fn value(self) -> &'static str {
        match self {
            Self::Zero => "0",
            Self::OnePointFive => "1.5",
            Self::Three => "3",
            Self::Five => "5",
            Self::Six => "6",
        }
    }

    pub fn percent(self) -> f64 {
        match self {
            Self::Zero => 0.0,
            Self::OnePointFive => 1.5,
            Self::Three => 3.0,
            Self::Five => 5.0,
            Self::Six => 6.0,
        }
    }
}

/// Item 37 boxes (`opt37:_1` / `opt37:_2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1606Overremittance {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1606Draft {
    #[serde(default)]
    pub id: Option<i64>,
    /// Dashboard year the return is filed under (the draft key's year).
    pub filing_year: u16,
    /// The dashboard's open-ended slot: one draft per transaction.
    pub open_ended_key: u32,

    // Items 1–4
    pub transaction_month: u8,
    pub transaction_day: u8,
    pub transaction_year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,
    /// Item 4 "Any taxes withheld?"; `None` until answered.
    #[serde(default)]
    pub taxes_withheld: Option<bool>,

    // Part I — buyer (the filer) and seller
    /// Item 5: 14 digits, 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    /// Item 6.
    pub rdo_code: String,
    /// Item 7: 14 digits like [`Self::tin`].
    #[serde(default)]
    pub seller_tin: String,
    /// Item 8.
    #[serde(default)]
    pub seller_rdo_code: String,
    /// Item 9.
    pub buyer_name: String,
    /// Item 10.
    #[serde(default)]
    pub seller_name: String,
    /// Item 11.
    pub buyer_address: String,
    /// Item 12.
    #[serde(default)]
    pub seller_address: String,
    #[serde(default)]
    pub seller_type: Option<Form1606SellerType>,
    #[serde(default)]
    pub agent_category: Option<Form1606AgentCategory>,
    #[serde(default)]
    pub property_class: Option<Form1606PropertyClass>,
    #[serde(default)]
    pub property_class_other: String,
    /// Item 16.
    #[serde(default)]
    pub property_location: String,
    /// Item 16A.
    #[serde(default)]
    pub property_rdo_code: String,
    /// Item 17 TCT/OCT/CCT No.
    #[serde(default)]
    pub tct_number: String,
    /// Item 17 Area sold (sq. m), digits only.
    #[serde(default)]
    pub area_sold: String,
    /// Item 17 Tax Dec. No., digits only.
    #[serde(default)]
    pub tax_declaration_number: String,
    /// Item 17 Others.
    #[serde(default)]
    pub other_description: String,
    /// Item 18.
    #[serde(default)]
    pub covers_more_than_one_property: Option<bool>,
    #[serde(default)]
    pub tax_relief: Form1606TaxRelief,
    /// Item 20.
    #[serde(default)]
    pub transaction: Option<Form1606Transaction>,
    /// Item 20 "If Exempt, or Others, specify".
    #[serde(default)]
    pub transaction_other: String,

    // Items 22–26, installment sale only (Item 21 is the Item 28A amount).
    #[serde(default)]
    pub cost_and_expenses: f64,
    #[serde(default)]
    pub mortgage_assumed: f64,
    #[serde(default)]
    pub initial_year_payments: f64,
    #[serde(default)]
    pub installment_this_month: f64,
    #[serde(default)]
    pub number_of_installments: f64,

    // Item 27: `None` is a blank field (box unticked); a typed value ticks it.
    #[serde(default)]
    pub fmv_land_tax_declaration: Option<f64>,
    #[serde(default)]
    pub fmv_land_zonal: Option<f64>,
    #[serde(default)]
    pub fmv_improvements_tax_declaration: Option<f64>,
    #[serde(default)]
    pub fmv_improvements_bir: Option<f64>,

    // Item 28
    #[serde(default)]
    pub taxable_base_box: Option<Form1606TaxableBaseBox>,
    /// 28A, also Item 21 on an installment sale.
    #[serde(default)]
    pub gross_selling_price: f64,
    /// 28D.
    #[serde(default)]
    pub installment_collected: f64,
    /// 28B, computed.
    #[serde(default)]
    pub fmv_land_and_improvement: f64,
    /// 28E description.
    #[serde(default)]
    pub others_base_description: String,
    /// 28E.
    #[serde(default)]
    pub others_base: f64,
    /// 28C.
    #[serde(default)]
    pub bid_price: f64,
    /// Item 29.
    #[serde(default)]
    pub seller_habitual: Option<bool>,

    // Part II
    /// Item 30.
    #[serde(default)]
    pub taxable_base: f64,
    /// Item 31.
    #[serde(default)]
    pub tax_rate: Option<Form1606TaxRate>,
    /// Item 32.
    #[serde(default)]
    pub tax_required: f64,
    /// Item 33, amended returns only.
    #[serde(default)]
    pub tax_remitted_previous: f64,
    /// Item 34.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 35A–35D.
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
    /// Item 36.
    #[serde(default)]
    pub total_amount_due: f64,
    #[serde(default)]
    pub overremittance: Form1606Overremittance,

    pub email: String,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

/// A field the filer may leave blank: blank stays `""`, anything typed is
/// formatted by `round(this,2)`.
fn blank_if_zero(value: f64) -> String {
    if value == 0.0 {
        String::new()
    } else {
        official_amount(value)
    }
}

fn optional_amount(value: Option<f64>) -> String {
    value.map(official_amount).unwrap_or_default()
}

/// `NumWithComma(a) + NumWithComma(b)`: a blank field is `NaN`, so the sum is
/// only defined when both fields hold a value.
fn sum_both(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    Some(cents(a?) + cents(b?))
}

/// The TCT/OCT/CCT number as `createXMLFileName` puts it in the filename:
/// `capital()` uppercased, dashes removed.
fn tct_for_filename(tct: &str) -> String {
    tct.trim().to_uppercase().replace('-', "")
}

impl Form1606Draft {
    pub const FORM_CODE: &'static str = "1606";

    /// A new transaction in the dashboard's `year` and open-ended slot,
    /// dated today when `year` is the current year, else January 1.
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
            open_ended_key: open_ended_key.max(1),
            transaction_month: month,
            transaction_day: day,
            transaction_year: year,
            is_amended: false,
            number_of_attached_sheets: 0,
            taxes_withheld: Some(true),
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            seller_tin: String::new(),
            seller_rdo_code: String::new(),
            buyer_name: profile.full_name.clone(),
            seller_name: String::new(),
            buyer_address: profile.registered_address.clone(),
            seller_address: String::new(),
            seller_type: None,
            agent_category: None,
            property_class: None,
            property_class_other: String::new(),
            property_location: String::new(),
            property_rdo_code: String::new(),
            tct_number: String::new(),
            area_sold: String::new(),
            tax_declaration_number: String::new(),
            other_description: String::new(),
            covers_more_than_one_property: None,
            tax_relief: Form1606TaxRelief::No,
            transaction: None,
            transaction_other: String::new(),
            cost_and_expenses: 0.0,
            mortgage_assumed: 0.0,
            initial_year_payments: 0.0,
            installment_this_month: 0.0,
            number_of_installments: 0.0,
            fmv_land_tax_declaration: None,
            fmv_land_zonal: None,
            fmv_improvements_tax_declaration: None,
            fmv_improvements_bir: None,
            taxable_base_box: None,
            gross_selling_price: 0.0,
            installment_collected: 0.0,
            fmv_land_and_improvement: 0.0,
            others_base_description: String::new(),
            others_base: 0.0,
            bid_price: 0.0,
            seller_habitual: None,
            taxable_base: 0.0,
            tax_rate: None,
            tax_required: 0.0,
            tax_remitted_previous: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_due: 0.0,
            overremittance: Form1606Overremittance::None,
            email: profile.email.clone(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 20: pick the transaction type and clear what its official handler
    /// clears (`cashSale`, `foreclosure`, `enableExempt`, `enableInstallment`).
    pub fn set_transaction(&mut self, transaction: Form1606Transaction) {
        self.transaction = Some(transaction);
        let clear_installment = |d: &mut Self| {
            d.cost_and_expenses = 0.0;
            d.mortgage_assumed = 0.0;
            d.initial_year_payments = 0.0;
            d.installment_this_month = 0.0;
            d.number_of_installments = 0.0;
        };
        match transaction {
            Form1606Transaction::CashSale => {
                clear_installment(self);
                self.installment_collected = 0.0;
                self.bid_price = 0.0;
                self.others_base = 0.0;
                self.others_base_description.clear();
                self.transaction_other.clear();
            }
            Form1606Transaction::ForeclosureSale => {
                clear_installment(self);
                self.installment_collected = 0.0;
                self.others_base = 0.0;
                self.others_base_description.clear();
                self.gross_selling_price = 0.0;
                self.fmv_land_tax_declaration = None;
                self.fmv_land_zonal = None;
                self.fmv_improvements_tax_declaration = None;
                self.fmv_improvements_bir = None;
                self.transaction_other.clear();
            }
            Form1606Transaction::Exempt | Form1606Transaction::Others => {
                clear_installment(self);
            }
            Form1606Transaction::InstallmentSale => {
                self.transaction_other.clear();
            }
        }
        self.recompute();
    }

    /// The official compute chain, run the way the Validate button runs it
    /// (`computeFMVLI` … `computeOfTotalAmtDue`). Every input is held at
    /// cents the way `round(this,2)` leaves it.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_remitted_previous = 0.0;
        }
        for value in [
            &mut self.cost_and_expenses,
            &mut self.mortgage_assumed,
            &mut self.initial_year_payments,
            &mut self.installment_this_month,
            &mut self.number_of_installments,
            &mut self.gross_selling_price,
            &mut self.installment_collected,
            &mut self.others_base,
            &mut self.bid_price,
            &mut self.tax_remitted_previous,
            &mut self.surcharge,
            &mut self.interest,
            &mut self.compromise,
        ] {
            *value = cents(*value);
        }
        for value in [
            &mut self.fmv_land_tax_declaration,
            &mut self.fmv_land_zonal,
            &mut self.fmv_improvements_tax_declaration,
            &mut self.fmv_improvements_bir,
        ] {
            *value = value.map(cents);
        }

        // compareFMVLI: (27A + 27B) when it is higher, else (27C + 27D); a
        // blank field makes its pair NaN, and formatCurrency(NaN) is 0.00.
        let land_and_improvements = sum_both(
            self.fmv_land_tax_declaration,
            self.fmv_improvements_tax_declaration,
        );
        let zonal_and_bir = sum_both(self.fmv_land_zonal, self.fmv_improvements_bir);
        self.fmv_land_and_improvement = match (land_and_improvements, zonal_and_bir) {
            (Some(ab), Some(cd)) if ab > cd => cents(ab),
            (_, Some(cd)) => cents(cd),
            _ => 0.0,
        };

        // computeTaxableBase.
        let fmv = self.fmv_land_and_improvement;
        self.taxable_base = cents(match self.transaction {
            Some(Form1606Transaction::ForeclosureSale) => {
                if self.bid_price >= fmv {
                    self.bid_price
                } else {
                    fmv
                }
            }
            Some(Form1606Transaction::InstallmentSale) => self.installment_collected,
            Some(Form1606Transaction::Exempt | Form1606Transaction::Others) => self.others_base,
            Some(Form1606Transaction::CashSale) | None => {
                if fmv >= self.gross_selling_price {
                    fmv
                } else {
                    self.gross_selling_price
                }
            }
        });

        // computeOfTaxRequired: an unselected rate is NaN, shown as 0.00.
        self.tax_required = match self.tax_rate {
            Some(rate) => cents(self.taxable_base * rate.percent() / 100.0),
            None => 0.0,
        };
        self.tax_still_due = cents(self.tax_required - self.tax_remitted_previous);
        // The penalty items file their override, else the computed value,
        // held at cents like every typed amount.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_due = cents(self.total_penalties + self.tax_still_due);
        if self.total_amount_due >= 0.0 {
            self.overremittance = Form1606Overremittance::None;
        }
    }

    /// The filename period: `MMDDYYYY_` + the TCT/OCT/CCT number.
    fn transaction_date_code(&self) -> String {
        format!(
            "{:02}{:02}{}",
            self.transaction_month, self.transaction_day, self.transaction_year
        )
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1606:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        // capital() (run by computeOfTotalAmtDue) uppercases every text
        // input except txtEmail.
        let text = |value: &str| value.trim().to_uppercase();
        let transaction = self.transaction;
        let is = |t: Form1606Transaction| transaction == Some(t);
        let installment = is(Form1606Transaction::InstallmentSale);

        put("txtDateMonth", format!("{:02}", self.transaction_month));
        put("txtDateDay", format!("{:02}", self.transaction_day));
        put("txtDateYear", self.transaction_year.to_string());
        put("j_id217:_1", flag(self.is_amended));
        put("j_id217:_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());
        put("j_id252:_1", flag(self.taxes_withheld == Some(true)));
        put("j_id252:_2", flag(self.taxes_withheld == Some(false)));

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtTIN1", tin1);
        put("txtTIN2", tin2);
        put("txtTIN3", tin3);
        put("txtBranchCode", branch);
        put("txtRDOCode", self.rdo_code.trim().to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.seller_tin);
        put("txtTINS1", tin1);
        put("txtTINS2", tin2);
        put("txtTINS3", tin3);
        put("txtBranchCodeS", branch);
        put("txtRDOCodeS", self.seller_rdo_code.trim().to_string());
        put("txtBuyerName", text(&self.buyer_name));
        put("txtSellerName", text(&self.seller_name));
        put("txtBuyerAddress", text(&self.buyer_address));
        put("txtSellerAddress", text(&self.seller_address));

        put("txt13", "WI155".to_string());
        put(
            "optATC13_2",
            flag(self.seller_type == Some(Form1606SellerType::Individual)),
        );
        put("txt13C", "WC155".to_string());
        put(
            "optATC13_3",
            flag(self.seller_type == Some(Form1606SellerType::Corporation)),
        );
        put(
            "j_id392:_1",
            flag(self.agent_category == Some(Form1606AgentCategory::Private)),
        );
        put(
            "j_id392:_2",
            flag(self.agent_category == Some(Form1606AgentCategory::Government)),
        );
        let class = |c: Form1606PropertyClass| flag(self.property_class == Some(c));
        put("j_id393:_1", class(Form1606PropertyClass::Residential));
        put("j_id393:_3", class(Form1606PropertyClass::Commercial));
        put(
            "j_id393:_5",
            class(Form1606PropertyClass::CondominiumResidential),
        );
        put("j_id393_8", class(Form1606PropertyClass::Others));
        put("j_id393_7", text(&self.property_class_other));
        put("j_id393:_2", class(Form1606PropertyClass::Agricultural));
        put("j_id393:_4", class(Form1606PropertyClass::Industrial));
        put(
            "j_id393:_6",
            class(Form1606PropertyClass::CondominiumCommercial),
        );
        put("txtLocation", text(&self.property_location));
        put("txtRDOCode16A", self.property_rdo_code.trim().to_string());
        put("txtTCT", text(&self.tct_number));
        put("txtArea", self.area_sold.trim().to_string());
        put("txtTaxDC", self.tax_declaration_number.trim().to_string());
        put("txtOthers", text(&self.other_description));
        put(
            "j_id394:_1",
            flag(self.covers_more_than_one_property == Some(true)),
        );
        put(
            "j_id394:_2",
            flag(self.covers_more_than_one_property == Some(false)),
        );
        put("rdTreaty:_1", flag(self.tax_relief.is_yes()));
        put("rdTreaty:_2", flag(!self.tax_relief.is_yes()));
        put("selTreaty", self.tax_relief.select_value().to_string());

        put("j_id395:_1", flag(is(Form1606Transaction::CashSale)));
        put("j_id395:_4", flag(is(Form1606Transaction::ForeclosureSale)));
        put("j_id395:_2", flag(is(Form1606Transaction::Exempt)));
        put("j_id395:_5", flag(is(Form1606Transaction::Others)));
        put("j_id395:_3", flag(installment));
        put("txtOthers20", text(&self.transaction_other));

        // Items 21–26 are blank unless this is an installment sale, where
        // computeTaxableBase copies 28A into Item 21.
        put(
            "txtSelling",
            if installment {
                official_amount(self.gross_selling_price)
            } else {
                String::new()
            },
        );
        for (key, value) in [
            ("txtCost", self.cost_and_expenses),
            ("txtMortgage", self.mortgage_assumed),
            ("txtTotalP", self.initial_year_payments),
            ("txtAmount", self.installment_this_month),
            ("txtTotalN", self.number_of_installments),
        ] {
            put(
                key,
                if installment {
                    blank_if_zero(value)
                } else {
                    String::new()
                },
            );
        }

        put("opt27A", flag(self.fmv_land_tax_declaration.is_some()));
        put("txtFMVLand", optional_amount(self.fmv_land_tax_declaration));
        put("opt27C", flag(self.fmv_land_zonal.is_some()));
        put("txtFMVZonal", optional_amount(self.fmv_land_zonal));
        put(
            "opt27B",
            flag(self.fmv_improvements_tax_declaration.is_some()),
        );
        put(
            "txtFMVImprovements",
            optional_amount(self.fmv_improvements_tax_declaration),
        );
        put("opt27D", flag(self.fmv_improvements_bir.is_some()));
        put("txtFMVBIR", optional_amount(self.fmv_improvements_bir));

        // Item 28: a field its transaction handler cleared stays blank until
        // typed into; the others keep the page's 0.00.
        let cash = is(Form1606Transaction::CashSale);
        let foreclosure = is(Form1606Transaction::ForeclosureSale);
        let amount_or_cleared = |value: f64, cleared: bool| {
            if cleared {
                blank_if_zero(value)
            } else {
                official_amount(value)
            }
        };
        let tick = |b: Form1606TaxableBaseBox| flag(self.taxable_base_box == Some(b));
        put("opt28A", tick(Form1606TaxableBaseBox::GrossSellingPrice));
        put(
            "txtGross",
            amount_or_cleared(self.gross_selling_price, foreclosure),
        );
        put("opt28D", tick(Form1606TaxableBaseBox::InstallmentCollected));
        put(
            "txtInstallment",
            amount_or_cleared(self.installment_collected, cash || foreclosure),
        );
        put("opt28B", tick(Form1606TaxableBaseBox::FairMarketValue));
        put("txtFMVLI", official_amount(self.fmv_land_and_improvement));
        put("opt28E", tick(Form1606TaxableBaseBox::Others));
        put("txtOtherss28E", text(&self.others_base_description));
        put(
            "txtOthers28E",
            amount_or_cleared(self.others_base, cash || foreclosure),
        );
        put("opt28C", tick(Form1606TaxableBaseBox::BidPrice));
        put("txtBid", amount_or_cleared(self.bid_price, cash));
        put("Habitual_1", flag(self.seller_habitual == Some(true)));
        put("Habitual_2", flag(self.seller_habitual == Some(false)));

        put("txtTax", official_amount(self.taxable_base));
        put(
            "txtTaxRate",
            self.tax_rate.map(|r| r.value()).unwrap_or("").to_string(),
        );
        put("txtTaxR", official_amount(self.tax_required));
        put("txtLess", official_amount(self.tax_remitted_previous));
        put("txtTaxDue", official_amount(self.tax_still_due));
        put("txtSurcharge", official_amount(self.surcharge));
        put("txtInterest", official_amount(self.interest));
        put("txtCompromise", official_amount(self.compromise));
        put("txtTotalPenalties", official_amount(self.total_penalties));
        put("txtTotal", official_amount(self.total_amount_due));
        put(
            "opt37:_1",
            flag(self.overremittance == Form1606Overremittance::Refund),
        );
        put(
            "opt37:_2",
            flag(self.overremittance == Form1606Overremittance::TaxCreditCertificate),
        );

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The field map plus print-only values the frozen 2018 sheet needs
    /// (`derived:` keys, never submitted): whole TINs, the ATC box for the
    /// seller type and rate, the rate digits, Schedule 3 Item 1A, the
    /// Schedule 1 lot/improvement rows (kind, higher FMV, total) and Item 11
    /// "If yes, specify".
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let dashed = |tin: &str| {
            let (a, b, c, d) = split_tin(tin);
            format!("{a}-{b}-{c}-{d}")
        };
        let (a, b, c, d) = split_tin(&self.tin);
        let mut put = |key: &str, value: String| {
            fields.insert(format!("derived:{key}"), value);
        };
        put("buyer_tin", dashed(&self.tin));
        put("seller_tin", dashed(&self.seller_tin));
        put("buyer_tin_digits", format!("{a}{b}{c}{d}"));
        put("buyer_name", self.buyer_name.trim().to_uppercase());
        if let Some(rate) = self.tax_rate {
            let tenths = (rate.percent() * 10.0).round() as u32;
            put("rate_whole", (tenths / 10).to_string());
            put("rate_tenths", (tenths % 10).to_string());
        }
        for (kind, label) in [
            (Form1606SellerType::Individual, "individual"),
            (Form1606SellerType::Corporation, "corporate"),
        ] {
            for rate in [
                Form1606TaxRate::OnePointFive,
                Form1606TaxRate::Three,
                Form1606TaxRate::Five,
                Form1606TaxRate::Six,
            ] {
                put(
                    &format!("atc_{label}_{}", rate.value()),
                    (self.seller_type == Some(kind) && self.tax_rate == Some(rate)).to_string(),
                );
            }
        }
        // Schedule 3 Item 1A: gross selling price, or the bid price on a
        // foreclosure sale.
        put(
            "sched3_1a",
            if self.transaction == Some(Form1606Transaction::ForeclosureSale) {
                official_amount(self.bid_price)
            } else {
                official_amount(self.gross_selling_price)
            },
        );
        // Schedule 1 continuation: the lot on row 1, the improvement on row
        // 2, each with the higher of its two FMVs, and the total (Item 1B).
        let higher = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (None, None) => String::new(),
            (a, b) => official_amount(a.unwrap_or(0.0).max(b.unwrap_or(0.0))),
        };
        let land = (self.fmv_land_tax_declaration, self.fmv_land_zonal);
        let improvement = (
            self.fmv_improvements_tax_declaration,
            self.fmv_improvements_bir,
        );
        for (row, kind, (a, b)) in [(1, "LOT", land), (2, "IMPROVEMENT", improvement)] {
            let shown = a.is_some() || b.is_some();
            put(
                &format!("sched1_kind{row}"),
                if shown { kind } else { "" }.to_string(),
            );
            put(&format!("sched1_fmv{row}"), higher(a, b));
        }
        put(
            "sched1_total",
            official_amount(self.fmv_land_and_improvement),
        );
        put(
            "line:tax_relief_specify",
            match self.tax_relief {
                Form1606TaxRelief::InternationalTaxTreaty => "INTERNATIONAL TAX TREATY",
                Form1606TaxRelief::SpecialLaw => "SPECIAL LAW",
                Form1606TaxRelief::No | Form1606TaxRelief::YesUnspecified => "",
            }
            .to_string(),
        );
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

fn days_in_month(month: u8, year: u16) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

impl FormValidator for Form1606Draft {
    /// `validate` in order, with its alert texts, then
    /// `initialValidateBeforeSave`, plus the input limits the official page
    /// enforces while typing (`maxlength`, `wholenumber`, `numbersonly`,
    /// `letternumber`, the option lists) and a few stricter checks where the
    /// official page lets an inconsistent return through (noted inline).
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // Item 1.
        let (month, day, year) = (
            self.transaction_month,
            self.transaction_day,
            self.transaction_year,
        );
        let leap = days_in_month(2, year) == 29;
        if month == 2 && day == 29 && !leap {
            err("transaction_day", "Filing year is not a leap year.");
        } else if (1..=12).contains(&month) && day > days_in_month(month, year) {
            err("transaction_day", "Invalid date entry on item 1.");
        }
        if month > 12 {
            err(
                "transaction_month",
                "Invalid month entry on Item no.1. Please enter a valid month.",
            );
        }
        // The official check is for a blank field; month/day 00 is as invalid.
        if month == 0 {
            err("transaction_month", "Please enter a valid month on Item 1.");
        }
        if day == 0 || day > 31 {
            err("transaction_day", "Please enter a valid day on Item 1.");
        }
        if year == 0 {
            err("transaction_year", "Please enter a valid year on Item 1.");
        } else if !(1904..=9999).contains(&year) {
            err(
                "transaction_year",
                "Invalid year entry on Item no.1. Year should not be lower than 1904.",
            );
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most two digits.",
            );
        }
        if self.taxes_withheld.is_none() {
            err("taxes_withheld", "Please select an option for Item 4.");
        }

        // Items 5–8.
        let tin_parts = |tin: &str| {
            let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
            let valid = digits.len() >= 9
                && digits.len() <= 14
                && tin.chars().all(|c| c.is_ascii_digit() || c == '-');
            (valid, digits)
        };
        let (buyer_ok, buyer_digits) = tin_parts(&self.tin);
        if !buyer_ok {
            err("tin", "Please enter the Buyer's TIN.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&buyer_digits[..9]) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter the Buyer's RDO Code.");
        }
        let (seller_ok, seller_digits) = tin_parts(&self.seller_tin);
        if !seller_ok {
            err("seller_tin", "Please enter the Seller's TIN.");
        } else if buyer_ok && split_tin(&self.tin) == split_tin(&self.seller_tin) {
            err(
                "seller_tin",
                "TIN for Buyer and Seller should be different.",
            );
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&seller_digits[..9]) != 0
        {
            // Stricter than the page, which never checks the seller's TIN.
            err("seller_tin", "You have entered an incorrect Seller's TIN.");
        }
        if !crate::validation::rdo_code_is_official_option(self.seller_rdo_code.trim()) {
            err("seller_rdo_code", "Please enter the Seller's RDO Code.");
        }

        // Items 9–12 (maxlength 50 / 70).
        let buyer_name = self.buyer_name.trim();
        if buyer_name.is_empty() {
            err("buyer_name", "Please enter the Buyer's Name.");
        } else if buyer_name.chars().count() > 50 {
            err("buyer_name", "Item 9 holds at most 50 characters.");
        }
        let seller_name = self.seller_name.trim();
        if seller_name.is_empty() {
            err("seller_name", "Please enter the Seller's Name.");
        } else if seller_name.chars().count() > 50 {
            err("seller_name", "Item 10 holds at most 50 characters.");
        }
        let buyer_address = self.buyer_address.trim();
        if buyer_address.is_empty() {
            err("buyer_address", "Please enter the Buyer's Address.");
        } else if buyer_address.chars().count() > 70 {
            err("buyer_address", "Item 11 holds at most 70 characters.");
        }
        if self.seller_address.trim().chars().count() > 70 {
            err("seller_address", "Item 12 holds at most 70 characters.");
        }

        // Items 13–15.
        if self.seller_type.is_none() {
            err("seller_type", "Please select an option for Item 13.");
        }
        if self.agent_category.is_none() {
            err("agent_category", "Please select an option for Item 14.");
        }
        if self.property_class.is_none() {
            err("property_class", "Please select an option for Item 15.");
        }
        let class_other = self.property_class_other.trim();
        if self.property_class == Some(Form1606PropertyClass::Others) {
            // The page leaves "Others (Specify)" optional.
            if class_other.is_empty() {
                err(
                    "property_class_other",
                    "Specify the classification of the property on Item 15.",
                );
            }
        } else if !class_other.is_empty() {
            err(
                "property_class_other",
                "Item 15 \"Others\" is specified only when Others is marked.",
            );
        }
        if class_other.chars().count() > 60 {
            err(
                "property_class_other",
                "Item 15 \"Others\" holds at most 60 characters.",
            );
        }

        // Items 16–17.
        let location = self.property_location.trim();
        if location.is_empty() {
            err(
                "property_location",
                "Please enter the Location of the Property on Item 16.",
            );
        } else if location.chars().count() > 125 {
            err("property_location", "Item 16 holds at most 125 characters.");
        }
        if !crate::validation::rdo_code_is_official_option(self.property_rdo_code.trim()) {
            err(
                "property_rdo_code",
                "Please enter the RDO Code on Item 16A.",
            );
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
                "Item 17 TCT/OCT/CCT No. holds at most 20 letters, digits or dashes.",
            );
        }
        let area = self.area_sold.trim();
        if area.len() > 20 || !digits_only(area) {
            err("area_sold", "Item 17 Area Sold holds at most 20 digits.");
        }
        let tax_dec = self.tax_declaration_number.trim();
        if tax_dec.len() > 25 || !digits_only(tax_dec) {
            err(
                "tax_declaration_number",
                "Item 17 Tax Dec. No. holds at most 25 digits.",
            );
        }
        if self.other_description.trim().chars().count() > 25 {
            err(
                "other_description",
                "Item 17 Others holds at most 25 characters.",
            );
        }

        // Item 19.
        if self.tax_relief == Form1606TaxRelief::YesUnspecified {
            err("tax_relief", "Please select a tax relief for Item 19.");
        }

        // Item 20: the page computes Item 30 from it but never requires it.
        let transaction_other = self.transaction_other.trim();
        match self.transaction {
            None => err("transaction", "Please select an option for Item 20."),
            Some(t) if t.is_exempt_or_others() => {
                if transaction_other.is_empty() {
                    err(
                        "transaction_other",
                        "Specify the Exempt or Others transaction on Item 20.",
                    );
                }
            }
            Some(_) => {
                if !transaction_other.is_empty() {
                    err(
                        "transaction_other",
                        "Item 20 is specified only for an Exempt or Others transaction.",
                    );
                }
            }
        }
        if transaction_other.chars().count() > 60 {
            err(
                "transaction_other",
                "Item 20 specification holds at most 60 characters.",
            );
        }
        let installment = self.transaction == Some(Form1606Transaction::InstallmentSale);
        for (field, value) in [
            ("cost_and_expenses", self.cost_and_expenses),
            ("mortgage_assumed", self.mortgage_assumed),
            ("initial_year_payments", self.initial_year_payments),
            ("installment_this_month", self.installment_this_month),
            ("number_of_installments", self.number_of_installments),
        ] {
            if !installment && value != 0.0 {
                err(field, "Items 21 to 26 apply only to an installment sale.");
            }
        }
        let cash = self.transaction == Some(Form1606Transaction::CashSale);
        if cash
            && (self.installment_collected != 0.0
                || self.bid_price != 0.0
                || self.others_base != 0.0
                || !self.others_base_description.trim().is_empty())
        {
            err(
                "transaction",
                "Items 28C, 28D and 28E are disabled for a cash sale.",
            );
        }

        // Item 27: a blank field makes the official 28B 0.00 (NaN pair).
        let fmv = [
            self.fmv_land_tax_declaration,
            self.fmv_land_zonal,
            self.fmv_improvements_tax_declaration,
            self.fmv_improvements_bir,
        ];
        if fmv.iter().any(Option::is_some) && fmv.iter().any(Option::is_none) {
            err(
                "fmv_land_tax_declaration",
                "Enter all four Item 27 values (0 where there is none) so Item 28B is computed.",
            );
        }
        if self.others_base_description.trim().chars().count() > 40 {
            err(
                "others_base_description",
                "Item 28E description holds at most 40 characters.",
            );
        }

        // Amounts: round(this,2) keeps 12 integer digits.
        let amounts = [
            ("cost_and_expenses", self.cost_and_expenses),
            ("mortgage_assumed", self.mortgage_assumed),
            ("initial_year_payments", self.initial_year_payments),
            ("installment_this_month", self.installment_this_month),
            ("number_of_installments", self.number_of_installments),
            (
                "fmv_land_tax_declaration",
                self.fmv_land_tax_declaration.unwrap_or(0.0),
            ),
            ("fmv_land_zonal", self.fmv_land_zonal.unwrap_or(0.0)),
            (
                "fmv_improvements_tax_declaration",
                self.fmv_improvements_tax_declaration.unwrap_or(0.0),
            ),
            (
                "fmv_improvements_bir",
                self.fmv_improvements_bir.unwrap_or(0.0),
            ),
            ("gross_selling_price", self.gross_selling_price),
            ("installment_collected", self.installment_collected),
            ("others_base", self.others_base),
            ("bid_price", self.bid_price),
            ("tax_remitted_previous", self.tax_remitted_previous),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ];
        for (field, value) in amounts {
            if value < 0.0 || value >= MAX_AMOUNT || !has_cent_precision(value) {
                err(
                    field,
                    "Enter a non-negative amount in pesos and centavos (at most 12 digits before the decimal point).",
                );
            }
        }

        // Part II.
        match self.taxes_withheld {
            Some(true) if self.tax_rate.is_none() => {
                // The page lets Item 31 stay "Select Tax Rate" (0.00 due).
                err("tax_rate", "Please select the tax rate on Item 31.");
            }
            Some(false)
                if self.tax_rate.is_some()
                    || self.surcharge != 0.0
                    || self.interest != 0.0
                    || self.compromise != 0.0 =>
            {
                // disablePart2 disables Items 31 and 35 when no tax was withheld.
                err(
                    "tax_rate",
                    "Items 31 and 35 are disabled when Item 4 is No.",
                );
            }
            _ => {}
        }
        if !self.is_amended && self.tax_remitted_previous != 0.0 {
            err(
                "tax_remitted_previous",
                "Item 33 applies only to an amended return.",
            );
        }
        if self.total_amount_due < 0.0 && self.overremittance == Form1606Overremittance::None {
            err(
                "overremittance",
                "Please indicate refund type for overremittance in Item 37.",
            );
        }

        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
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
                "total_amount_due",
                "Totals are out of date. Recompute the return.",
            );
        }

        errors
    }
}

impl Form1606Draft {
    /// Items 35A-35C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: Withholding,
            items: [
                surcharge: "35A Surcharge",
                interest: "35B Interest",
                compromise: "35C Compromise",
            ],
            controls: [
                "frm1606:txtSurcharge",
                "frm1606:txtInterest",
                "frm1606:txtCompromise",
            ],
            state: penalty,
            basic_tax_due: "Item 34", |d| d.tax_still_due,
            // Item 1 is the date of the transaction: the calendar's
            // monthly 1606 deadline is the one for that month.
            deadline_period: |d: &Self| {
                Some(crate::calendar_rules::DeadlinePeriod::Monthly {
                    taxable_year: i32::from(d.transaction_year),
                    month: d.transaction_month,
                })
            },
        )
    }
}

impl QueueableForm for Form1606Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1606";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1606']`).
    const FORM_TYPE: &'static str = "1606";
    const LAYOUT_ID: &'static str = FORM_1606_FORM_ID;

    /// Total Amount Still Due.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.total_amount_due)
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
        self.filing_year
    }
    /// One return per transaction, keyed by the dashboard's open-ended slot.
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::OpenEnded(self.open_ended_key)
    }
    /// `MM + DD + YYYY + "_" + TCT` (dashes removed), as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!(
            "{}_{}",
            self.transaction_date_code(),
            tct_for_filename(&self.tct_number)
        )
    }
    /// The filename names the transaction (date and TCT), not the
    /// dashboard slot, so a receipt cannot be mapped back by period; the
    /// queue confirms it by the uploaded filename instead.
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

    pub(crate) fn sample() -> Form1606Draft {
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Sample Buyer Corp",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Real estate",
            "registered_address": "123 Sample St Quezon City",
            "zip_code": "1100",
            "phone": "0281234567",
            "email": "sample.buyer@example.com",
            "default_form_type": "1606",
            "taxpayer_type": "Corporation"
        }))
        .unwrap();
        let mut draft = Form1606Draft::new_from_profile(&profile, 2025, 1);
        draft.transaction_month = 6;
        draft.transaction_day = 15;
        draft.seller_tin = "98765432100000".into();
        draft.seller_rdo_code = "040".into();
        draft.seller_name = "Sample Seller Inc".into();
        draft.seller_address = "456 Example Ave Makati".into();
        draft.seller_type = Some(Form1606SellerType::Corporation);
        draft.agent_category = Some(Form1606AgentCategory::Private);
        draft.property_class = Some(Form1606PropertyClass::Commercial);
        draft.property_location = "Lot 1 Block 2 Sample Subdivision".into();
        draft.property_rdo_code = "039".into();
        draft.tct_number = "T-12345".into();
        draft.set_transaction(Form1606Transaction::CashSale);
        draft.gross_selling_price = 5_000_000.005;
        draft.fmv_land_tax_declaration = Some(3_000_000.0);
        draft.fmv_improvements_tax_declaration = Some(1_500_000.0);
        draft.fmv_land_zonal = Some(4_200_000.0);
        draft.fmv_improvements_bir = Some(1_000_000.0);
        draft.taxable_base_box = Some(Form1606TaxableBaseBox::FairMarketValue);
        draft.tax_rate = Some(Form1606TaxRate::Six);
        draft.surcharge = 100.0;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1606Draft) -> Vec<String> {
        <Form1606Draft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        assert_eq!(draft.gross_selling_price, 5_000_000.01);
        assert_eq!(draft.fmv_land_and_improvement, 5_200_000.0);
        assert_eq!(draft.taxable_base, 5_200_000.0);
        assert_eq!(draft.tax_required, 312_000.0);
        assert_eq!(draft.tax_still_due, 312_000.0);
        assert_eq!(draft.total_penalties, 100.0);
        assert_eq!(draft.total_amount_due, 312_100.0);

        // A blank Item 27 field makes its pair NaN: 28B falls to the other pair.
        let mut blank = sample();
        blank.fmv_improvements_bir = None;
        blank.recompute();
        assert_eq!(blank.fmv_land_and_improvement, 0.0);
        blank.fmv_improvements_bir = Some(1_000_000.0);
        blank.fmv_land_zonal = None;
        blank.recompute();
        assert_eq!(blank.fmv_land_and_improvement, 0.0);

        let mut foreclosure = sample();
        foreclosure.set_transaction(Form1606Transaction::ForeclosureSale);
        assert_eq!(foreclosure.fmv_land_tax_declaration, None);
        foreclosure.bid_price = 2_000_000.0;
        foreclosure.recompute();
        assert_eq!(foreclosure.taxable_base, 2_000_000.0);
        assert_eq!(
            foreclosure.to_bir_field_map()["frm1606:txtGross"],
            String::new()
        );

        let mut installment = sample();
        installment.set_transaction(Form1606Transaction::InstallmentSale);
        installment.installment_collected = 250_000.0;
        installment.recompute();
        assert_eq!(installment.taxable_base, 250_000.0);
        assert_eq!(installment.tax_required, 15_000.0);
        assert_eq!(
            installment.to_bir_field_map()["frm1606:txtSelling"],
            "5,000,000.01"
        );
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        assert_eq!(fields["frm1606:txtDateMonth"], "06");
        assert_eq!(fields["frm1606:txtDateDay"], "15");
        assert_eq!(fields["frm1606:txtBuyerName"], "SAMPLE BUYER CORP");
        assert_eq!(fields["frm1606:txtTCT"], "T-12345");
        assert_eq!(fields["frm1606:txtGross"], "5,000,000.01");
        assert_eq!(fields["frm1606:txtInstallment"], "");
        assert_eq!(fields["frm1606:txtBid"], "");
        assert_eq!(fields["frm1606:txtFMVLI"], "5,200,000.00");
        assert_eq!(fields["frm1606:txtTaxRate"], "6");
        assert_eq!(fields["frm1606:txtTaxR"], "312,000.00");
        assert_eq!(fields["frm1606:selTreaty"], "");
        assert_eq!(fields["frm1606:optATC13_3"], "true");
        assert_eq!(fields["frm1606:txtSelling"], "");
        assert_eq!(fields["txtEmail"], "sample.buyer@example.com");
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1606-06152025_T12345#sample.buyer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_name_the_transaction_not_the_slot() {
        let draft = sample();
        assert_eq!(draft.period_code(), "06152025_T12345");
        assert_eq!(draft.filing_period(), FilingPeriod::OpenEnded(1));
        assert_eq!(draft.taxable_year(), 2025);
        assert_eq!(Form1606Draft::parse_period_code("06152025_T12345"), None);
    }

    #[test]
    fn two_transactions_in_one_month_persist_separately() {
        let db = crate::db::Database::open_in_memory_for_tests().unwrap();
        let first = sample();
        let mut second = sample();
        second.open_ended_key = 2;
        second.transaction_day = 20;
        second.tct_number = "T-67890".into();
        second.recompute();
        db.save_queueable_draft(&first).unwrap();
        db.save_queueable_draft(&second).unwrap();
        let load = |key: i64| {
            db.get_queueable_draft::<Form1606Draft>(&first.tin, 2025, key)
                .unwrap()
                .unwrap()
        };
        assert_eq!(load(1).tct_number, "T-12345");
        assert_eq!(load(2).tct_number, "T-67890");
        assert_eq!(load(2).transaction_month, load(1).transaction_month);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1606Draft), expected: &str| {
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
                d.transaction_month = 2;
                d.transaction_day = 29;
            },
            "Filing year is not a leap year.",
        );
        check(&|d| d.transaction_day = 31, "Invalid date entry on item 1.");
        check(
            &|d| d.transaction_month = 13,
            "Invalid month entry on Item no.1. Please enter a valid month.",
        );
        check(
            &|d| d.transaction_month = 0,
            "Please enter a valid month on Item 1.",
        );
        check(
            &|d| d.transaction_day = 0,
            "Please enter a valid day on Item 1.",
        );
        check(
            &|d| d.transaction_year = 0,
            "Please enter a valid year on Item 1.",
        );
        check(
            &|d| d.transaction_year = 1903,
            "Invalid year entry on Item no.1. Year should not be lower than 1904.",
        );
        check(
            &|d| d.taxes_withheld = None,
            "Please select an option for Item 4.",
        );
        check(&|d| d.tin = "123".into(), "Please enter the Buyer's TIN.");
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter the Buyer's RDO Code.",
        );
        check(&|d| d.seller_tin.clear(), "Please enter the Seller's TIN.");
        check(
            &|d| d.seller_tin = d.tin.clone(),
            "TIN for Buyer and Seller should be different.",
        );
        check(
            &|d| d.seller_rdo_code.clear(),
            "Please enter the Seller's RDO Code.",
        );
        check(&|d| d.buyer_name.clear(), "Please enter the Buyer's Name.");
        check(
            &|d| d.seller_name = " ".into(),
            "Please enter the Seller's Name.",
        );
        check(
            &|d| d.buyer_address.clear(),
            "Please enter the Buyer's Address.",
        );
        check(
            &|d| d.seller_type = None,
            "Please select an option for Item 13.",
        );
        check(
            &|d| d.agent_category = None,
            "Please select an option for Item 14.",
        );
        check(
            &|d| d.property_class = None,
            "Please select an option for Item 15.",
        );
        check(
            &|d| d.property_location.clear(),
            "Please enter the Location of the Property on Item 16.",
        );
        check(
            &|d| d.property_rdo_code = "000".into(),
            "Please enter the RDO Code on Item 16A.",
        );
        check(
            &|d| d.tct_number.clear(),
            "Please enter the TCT/OCT/CCT No.",
        );
        check(
            &|d| d.tax_relief = Form1606TaxRelief::YesUnspecified,
            "Please select a tax relief for Item 19.",
        );
        check(
            &|d| {
                d.is_amended = true;
                d.tax_remitted_previous = 400_000.0;
                d.recompute();
            },
            "Please indicate refund type for overremittance in Item 37.",
        );
        check(
            &|d| d.fmv_improvements_bir = None,
            "Enter all four Item 27 values (0 where there is none) so Item 28B is computed.",
        );
        check(
            &|d| d.transaction = None,
            "Please select an option for Item 20.",
        );
        check(
            &|d| d.tct_number = "T/123".into(),
            "Item 17 TCT/OCT/CCT No. holds at most 20 letters, digits or dashes.",
        );
        check(
            &|d| d.surcharge = 1.0,
            "Totals are out of date. Recompute the return.",
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
            <Form1606Draft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form1606Draft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
