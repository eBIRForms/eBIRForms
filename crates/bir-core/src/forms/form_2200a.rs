//! BIR Form 2200A — Excise Tax Return for Alcohol Products (January 2020).
//!
//! Ported from the official `BIR-Form2200Av2020.hta` (eBIRForms 7.9.6.2.1):
//! the per-row tax due (`computeField`: applicable tax rate × taxable base,
//! formatted by `amtFormat`, i.e. JavaScript `toFixed(2)`), the Schedule 1
//! total (`calculate_Sched1_TotalDue`), Part III (`calculate_Part3`),
//! `validateForm` with its exact alert texts, the "Others" row checks of
//! `isValidDataOnSched1`, and `saveXMLsubmit` through [`crate::official_xml`].
//!
//! Schedule 1 is a fixed list of 60 product/period rows; each row's rate
//! sits in the page as `<td id="…_ATR" value="…">`, which the official
//! engine (mshta, IE7 document mode) exposes as `td.value`. Three "Others"
//! rows take filer-typed ATC digits, description, bracket, rate and tax due.
//! The page can append more "Others" rows, but those are not part of the
//! official submit layout, so the model stops at three.
//!
//! Profile values (name, address) are submitted as loaded; nothing on the
//! submit path uppercases them.

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
pub const FORM_2200A_FORM_ID: &str = "2200a-v2020";
const PREFIX: &str = "frm2200Av2020";

/// Fixed Schedule 1 rows in page order: (control key between `txtSched1_`
/// and `_Export`/`_Taxable`/`_Due`, ATC, product, period, printed rate, the
/// rate `computeField` multiplies by: the `_ATR` cell's `value` after
/// `js/tax-rate-helper.js` applies `xml/taxRate.xml` at load).
pub const FORM_2200A_FIXED_ROWS: &[(&str, &str, &str, &str, &str, f64)] = &[
    (
        "1A_2020",
        "XA035",
        "1.) Distilled Spirits a.) Ad Valorem Tax Rate based on the Net Retail Price (NRP) per proof [excluding the excise and value-added taxes (VAT)] NRP Per Proof",
        "Effective January 1, 2020",
        "20%",
        0.2,
    ),
    (
        "1A_2020B",
        "XA035",
        "1.) Distilled Spirits a.) Ad Valorem Tax Rate based on the Net Retail Price (NRP) per proof [excluding the excise and value-added taxes (VAT)] NRP Per Proof",
        "Effective January 23, 2020",
        // xml/taxRate.xml (js/tax-rate-helper.js, window.onload) resets this
        // cell from the page's 22% to 21%.
        "21%",
        0.21,
    ),
    (
        "1A_2021",
        "XA035",
        "1.) Distilled Spirits a.) Ad Valorem Tax Rate based on the Net Retail Price (NRP) per proof [excluding the excise and value-added taxes (VAT)] NRP Per Proof",
        "Effective January 1, 2021",
        "22%",
        0.22,
    ),
    (
        "1A_2022",
        "XA035",
        "1.) Distilled Spirits a.) Ad Valorem Tax Rate based on the Net Retail Price (NRP) per proof [excluding the excise and value-added taxes (VAT)] NRP Per Proof",
        "Effective January 1, 2022",
        "22%",
        0.22,
    ),
    (
        "1A_2023",
        "XA035",
        "1.) Distilled Spirits a.) Ad Valorem Tax Rate based on the Net Retail Price (NRP) per proof [excluding the excise and value-added taxes (VAT)] NRP Per Proof",
        "Effective January 1, 2023",
        "22%",
        0.22,
    ),
    (
        "1A_2024",
        "XA035",
        "1.) Distilled Spirits a.) Ad Valorem Tax Rate based on the Net Retail Price (NRP) per proof [excluding the excise and value-added taxes (VAT)] NRP Per Proof",
        "Effective January 1, 2024",
        "22%",
        0.22,
    ),
    (
        "1B_2020",
        "XA036",
        "1.) Distilled Spirits b.) In addition to Ad Valorem Tax, a Specific Tax per proof of liter Per Proof Liter",
        "Effective January 1, 2020",
        "24.34",
        24.34,
    ),
    (
        "1B_2020B",
        "XA036",
        "1.) Distilled Spirits b.) In addition to Ad Valorem Tax, a Specific Tax per proof of liter Per Proof Liter",
        "Effective January 23, 2020",
        "42.00",
        42.0,
    ),
    (
        "1B_2021",
        "XA036",
        "1.) Distilled Spirits b.) In addition to Ad Valorem Tax, a Specific Tax per proof of liter Per Proof Liter",
        "Effective January 1, 2021",
        "47.00",
        47.0,
    ),
    (
        "1B_2022",
        "XA036",
        "1.) Distilled Spirits b.) In addition to Ad Valorem Tax, a Specific Tax per proof of liter Per Proof Liter",
        "Effective January 1, 2022",
        "52.00",
        52.0,
    ),
    (
        "1B_2023",
        "XA036",
        "1.) Distilled Spirits b.) In addition to Ad Valorem Tax, a Specific Tax per proof of liter Per Proof Liter",
        "Effective January 1, 2023",
        "59.00",
        59.0,
    ),
    (
        "1B_2024",
        "XA036",
        "1.) Distilled Spirits b.) In addition to Ad Valorem Tax, a Specific Tax per proof of liter Per Proof Liter",
        "Effective January 1, 2024",
        "66.00",
        66.0,
    ),
    (
        "2A_2020",
        "XA061",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: Php 500.00 or less Per Liter",
        "Effective January 1, 2020",
        "328.98",
        328.98,
    ),
    (
        "2A_2020B",
        "XA061",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: Php 500.00 or less Per Liter",
        "Effective January 23, 2020",
        "50.00",
        50.0,
    ),
    (
        "2A_2021",
        "XA061",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: Php 500.00 or less Per Liter",
        "Effective January 1, 2021",
        "53.00",
        53.0,
    ),
    (
        "2A_2022",
        "XA061",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: Php 500.00 or less Per Liter",
        "Effective January 1, 2022",
        "56.18",
        56.18,
    ),
    (
        "2A_2023",
        "XA061",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: Php 500.00 or less Per Liter",
        "Effective January 1, 2023",
        "59.55",
        59.55,
    ),
    (
        "2A_2024",
        "XA061",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: Php 500.00 or less Per Liter",
        "Effective January 1, 2024",
        "63.12",
        63.12,
    ),
    (
        "2A_2020A",
        "XA062",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: More than Php 500.00 Per Liter",
        "Effective January 1, 2020",
        "921.15",
        921.15,
    ),
    (
        "2A_2020B2",
        "XA062",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: More than Php 500.00 Per Liter",
        "Effective January 23, 2020",
        "50.00",
        50.0,
    ),
    (
        "2A_2021B",
        "XA062",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: More than Php 500.00 Per Liter",
        "Effective January 1, 2021",
        "53.00",
        53.0,
    ),
    (
        "2A_2022B",
        "XA062",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: More than Php 500.00 Per Liter",
        "Effective January 1, 2022",
        "56.18",
        56.18,
    ),
    (
        "2A_2023B",
        "XA062",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: More than Php 500.00 Per Liter",
        "Effective January 1, 2023",
        "59.55",
        59.55,
    ),
    (
        "2A_2024B",
        "XA062",
        "2.) Wines a.) Sparkling wines/champagnes where the NRP (excluding the excise and VAT) per bottle of 750 ml volume capacity, regardless of proof is: More than Php 500.00 Per Liter",
        "Effective January 1, 2024",
        "63.12",
        63.12,
    ),
    (
        "2B_2020",
        "XA070",
        "2.) Wines b.) Still wines and carbonated wines containing 14% of alcohol by volume or less Per Liter",
        "Effective January 1, 2020",
        "39.48",
        39.48,
    ),
    (
        "2B_2020B",
        "XA070",
        "2.) Wines b.) Still wines and carbonated wines containing 14% of alcohol by volume or less Per Liter",
        "Effective January 23, 2020",
        "50.00",
        50.0,
    ),
    (
        "2B_2021",
        "XA070",
        "2.) Wines b.) Still wines and carbonated wines containing 14% of alcohol by volume or less Per Liter",
        "Effective January 1, 2021",
        "53.00",
        53.0,
    ),
    (
        "2B_2022",
        "XA070",
        "2.) Wines b.) Still wines and carbonated wines containing 14% of alcohol by volume or less Per Liter",
        "Effective January 1, 2022",
        "56.18",
        56.18,
    ),
    (
        "2B_2023",
        "XA070",
        "2.) Wines b.) Still wines and carbonated wines containing 14% of alcohol by volume or less Per Liter",
        "Effective January 1, 2023",
        "59.55",
        59.55,
    ),
    (
        "2B_2024",
        "XA070",
        "2.) Wines b.) Still wines and carbonated wines containing 14% of alcohol by volume or less Per Liter",
        "Effective January 1, 2024",
        "63.12",
        63.12,
    ),
    (
        "2C_2020",
        "XA080",
        "2.) Wines c.) Still wines and carbonated wines containing more than 14% of alcohol by volume but not more than 25% of alcohol by volume Per Liter",
        "Effective January 1, 2020",
        "78.96",
        78.96,
    ),
    (
        "2C_2020B",
        "XA080",
        "2.) Wines c.) Still wines and carbonated wines containing more than 14% of alcohol by volume but not more than 25% of alcohol by volume Per Liter",
        "Effective January 23, 2020",
        "50.00",
        50.0,
    ),
    (
        "2C_2021",
        "XA080",
        "2.) Wines c.) Still wines and carbonated wines containing more than 14% of alcohol by volume but not more than 25% of alcohol by volume Per Liter",
        "Effective January 1, 2021",
        "53.00",
        53.0,
    ),
    (
        "2C_2022",
        "XA080",
        "2.) Wines c.) Still wines and carbonated wines containing more than 14% of alcohol by volume but not more than 25% of alcohol by volume Per Liter",
        "Effective January 1, 2022",
        "56.18",
        56.18,
    ),
    (
        "2C_2023",
        "XA080",
        "2.) Wines c.) Still wines and carbonated wines containing more than 14% of alcohol by volume but not more than 25% of alcohol by volume Per Liter",
        "Effective January 1, 2023",
        "59.55",
        59.55,
    ),
    (
        "2C_2024",
        "XA080",
        "2.) Wines c.) Still wines and carbonated wines containing more than 14% of alcohol by volume but not more than 25% of alcohol by volume Per Liter",
        "Effective January 1, 2024",
        "63.12",
        63.12,
    ),
    (
        "2D_2020",
        "XA090",
        "2.) Wines d.) Fortified wines containing more than 25% of alcohol by volume Per Proof Liter",
        "Effective January 1, 2020",
        "Taxed as distilled spirits",
        0.0,
    ),
    (
        "2D_2020B",
        "XA090",
        "2.) Wines d.) Fortified wines containing more than 25% of alcohol by volume Per Proof Liter",
        "Effective January 23, 2020",
        "50.00",
        50.0,
    ),
    (
        "2D_2021",
        "XA090",
        "2.) Wines d.) Fortified wines containing more than 25% of alcohol by volume Per Proof Liter",
        "Effective January 1, 2021",
        "53.00",
        53.0,
    ),
    (
        "2D_2022",
        "XA090",
        "2.) Wines d.) Fortified wines containing more than 25% of alcohol by volume Per Proof Liter",
        "Effective January 1, 2022",
        "56.18",
        56.18,
    ),
    (
        "2D_2023",
        "XA090",
        "2.) Wines d.) Fortified wines containing more than 25% of alcohol by volume Per Proof Liter",
        "Effective January 1, 2023",
        "59.55",
        59.55,
    ),
    (
        "2D_2024",
        "XA090",
        "2.) Wines d.) Fortified wines containing more than 25% of alcohol by volume Per Proof Liter",
        "Effective January 1, 2024",
        "63.12",
        63.12,
    ),
    (
        "3A_2020",
        "XA055",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2020",
        "26.43",
        26.43,
    ),
    (
        "3A_2020B",
        "XA055",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 23, 2020",
        "35.00",
        35.0,
    ),
    (
        "3A_2021",
        "XA055",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2021",
        "37.00",
        37.0,
    ),
    (
        "3A_2022",
        "XA055",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2022",
        "39.00",
        39.0,
    ),
    (
        "3A_2023",
        "XA055",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2023",
        "41.00",
        41.0,
    ),
    (
        "3A_2024",
        "XA055",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2024",
        "43.00",
        43.0,
    ),
    (
        "3A_2020A",
        "XA056",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2020",
        "26.43",
        26.43,
    ),
    (
        "3B_2020B2",
        "XA056",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 23, 2020",
        "35.00",
        35.0,
    ),
    (
        "3A_2021B",
        "XA056",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2021",
        "37.00",
        37.0,
    ),
    (
        "3A_2022B",
        "XA056",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2022",
        "39.00",
        39.0,
    ),
    (
        "3A_2023B",
        "XA056",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2023",
        "41.00",
        41.0,
    ),
    (
        "3A_2024B",
        "XA056",
        "3.) Fermented Liquors a.) If the NRP (excluding the excise and VAT) per liter of volume capacity is: Php 50.60 and below Per Liter",
        "Effective January 1, 2024",
        "43.00",
        43.0,
    ),
    (
        "3B_2020",
        "XA057",
        "3.) Fermented Liquors b.) If brewed and sold at microbreweries or small establishments such as pubs and restaurants, regardless of the NRP Per Liter",
        "Effective January 1, 2020",
        "36.85",
        36.85,
    ),
    (
        "3B_2020B",
        "XA057",
        "3.) Fermented Liquors b.) If brewed and sold at microbreweries or small establishments such as pubs and restaurants, regardless of the NRP Per Liter",
        "Effective January 23, 2020",
        "35.00",
        35.0,
    ),
    (
        "3B_2021",
        "XA057",
        "3.) Fermented Liquors b.) If brewed and sold at microbreweries or small establishments such as pubs and restaurants, regardless of the NRP Per Liter",
        "Effective January 1, 2021",
        "37.00",
        37.0,
    ),
    (
        "3B_2022",
        "XA057",
        "3.) Fermented Liquors b.) If brewed and sold at microbreweries or small establishments such as pubs and restaurants, regardless of the NRP Per Liter",
        "Effective January 1, 2022",
        "39.00",
        39.0,
    ),
    (
        "3B_2023",
        "XA057",
        "3.) Fermented Liquors b.) If brewed and sold at microbreweries or small establishments such as pubs and restaurants, regardless of the NRP Per Liter",
        "Effective January 1, 2023",
        "41.00",
        41.0,
    ),
    (
        "3B_2024",
        "XA057",
        "3.) Fermented Liquors b.) If brewed and sold at microbreweries or small establishments such as pubs and restaurants, regardless of the NRP Per Liter",
        "Effective January 1, 2024",
        "43.00",
        43.0,
    ),
];
/// Number of fixed rows (60 on the official page).
pub const FORM_2200A_FIXED_ROW_COUNT: usize = FORM_2200A_FIXED_ROWS.len();
/// "Others (specify)" rows at the end of Schedule 1.
pub const FORM_2200A_OTHER_ROWS: usize = 3;

/// Items 10 and 11: region, province and city codes from the official
/// dropdowns. Blank means the `(Select …)` placeholder, `00`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Form2200APlace {
    pub region: String,
    pub province: String,
    pub city: String,
}

fn place_code(value: &str) -> String {
    if value.trim().is_empty() {
        "00".to_string()
    } else {
        value.trim().to_string()
    }
}

/// Part II — Manner of Payment (Items 13–15, one radio group).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2200APayment {
    #[default]
    None,
    /// Item 13 — Payment on actual removal.
    ActualRemoval,
    /// Item 14 — Prepayment / Advance Deposit.
    Prepayment,
    /// Item 15 — Other Similar Schemes (enabled on the official page only
    /// after Item 14).
    OtherScheme,
}

/// Tax bases and tax due of one Schedule 1 row (`round(this,2)` values).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200AAmounts {
    #[serde(default)]
    pub export_exempt: f64,
    #[serde(default)]
    pub taxable: f64,
    #[serde(default)]
    pub tax_due: f64,
}

impl Form2200AAmounts {
    fn is_zero(&self) -> bool {
        self.export_exempt == 0.0 && self.taxable == 0.0 && self.tax_due == 0.0
    }

    fn round(&mut self) {
        self.export_exempt = cents(self.export_exempt);
        self.taxable = cents(self.taxable);
        self.tax_due = cents(self.tax_due);
    }

    fn values(&self) -> [f64; 3] {
        [self.export_exempt, self.taxable, self.tax_due]
    }
}

/// One "Others (specify)" row: ATC digits typed after the printed `XA`
/// (digits only, exactly 3 when given), description, bracket, rate (digits
/// and `.`), tax bases and the tax due the filer computes. The row's
/// checkbox (`_Check`) is ticked whenever the row is used.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200AOtherRow {
    #[serde(default)]
    pub atc_digits: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub bracket: String,
    #[serde(default)]
    pub rate: String,
    #[serde(default)]
    pub amounts: Form2200AAmounts,
}

impl Form2200AOtherRow {
    pub fn is_blank(&self) -> bool {
        self.atc_digits.trim().is_empty()
            && self.description.trim().is_empty()
            && self.bracket.trim().is_empty()
            && self.rate.trim().is_empty()
            && self.amounts.is_zero()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2200ADraft {
    #[serde(default)]
    pub id: Option<i64>,

    /// Item 1 date (MM/DD/YYYY).
    pub month: u8,
    pub day: u8,
    pub year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I (loaded from the taxpayer profile)
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub rdo_code: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,
    #[serde(default)]
    pub place_of_production: Form2200APlace,
    #[serde(default)]
    pub place_of_removal: Form2200APlace,
    /// Item 12 (the official page starts on "No").
    #[serde(default)]
    pub tax_relief: bool,
    #[serde(default)]
    pub tax_relief_specify: String,

    // Part II
    #[serde(default)]
    pub manner_of_payment: Form2200APayment,
    #[serde(default)]
    pub other_scheme_description: String,

    // Part V — Schedule 1
    /// Fixed rows in [`FORM_2200A_FIXED_ROWS`] order; missing rows are zero.
    #[serde(default)]
    pub schedule: Vec<Form2200AAmounts>,
    /// "Others (specify)" rows, at most three.
    #[serde(default)]
    pub others: Vec<Form2200AOtherRow>,
    /// `txtSched1_TotalDue`.
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
    /// Item 23A.
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

    /// Details of Payment, as typed (`cashBank` ... `othersAmt`).
    #[serde(default)]
    pub payment: super::payment_cells::PaymentCells,
    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `round(this,2)` / `amtFormat`: the value the official field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// JavaScript `x.toFixed(2)` as `amtFormat` uses it: the exact binary value
/// rounded half up at the cent (not the decimal-text rounding of
/// `round()`), e.g. `(0.2 * 1.005).toFixed(2)` is `0.20`.
fn js_to_fixed2(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    // Rust prints the exact decimal expansion of an f64 at this precision.
    let text = format!("{:.1100}", value.abs());
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let digits = format!("{whole}{}", &fraction[..2]);
    let mut cents: u128 = digits.parse().unwrap_or(0);
    if fraction.as_bytes().get(2).is_some_and(|d| *d >= b'5') {
        cents += 1;
    }
    let magnitude = cents as f64 / 100.0;
    if value < 0.0 { -magnitude } else { magnitude }
}

/// `isAmountWithinAllowedPrecision`: at most 12 integer digits.
fn within_official_precision(value: f64) -> bool {
    value.is_finite() && value.abs() < 1e12
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

/// Whether a region → province → city choice is one the official cascading
/// dropdowns can hold (9-digit codes from the official lists).
fn place_is_official(place: &Form2200APlace) -> bool {
    let (region, province, city) = (
        place.region.trim(),
        place.province.trim(),
        place.city.trim(),
    );
    crate::reference::get_all_regions()
        .iter()
        .any(|r| r.code == region)
        && crate::reference::get_provinces_for_region(region)
            .iter()
            .any(|p| p.code == province)
        && crate::reference::get_cities_for_province(province)
            .iter()
            .any(|c| c.code == city && c.region_code == region && c.code.len() == 9)
}

impl Form2200ADraft {
    pub const FORM_CODE: &'static str = "2200A";

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
            place_of_production: Form2200APlace::default(),
            place_of_removal: Form2200APlace::default(),
            tax_relief: false,
            tax_relief_specify: String::new(),
            manner_of_payment: Form2200APayment::None,
            other_scheme_description: String::new(),
            schedule: Vec::new(),
            others: Vec::new(),
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
            payment: Default::default(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Fixed row `index`, growing the schedule to reach it.
    pub fn row_mut(&mut self, index: usize) -> Option<&mut Form2200AAmounts> {
        if index >= FORM_2200A_FIXED_ROW_COUNT {
            return None;
        }
        if self.schedule.len() <= index {
            self.schedule
                .resize_with(index + 1, Form2200AAmounts::default);
        }
        self.schedule.get_mut(index)
    }

    /// Fixed row by its control key (e.g. `"4B_2023"`).
    pub fn row_by_key_mut(&mut self, key: &str) -> Option<&mut Form2200AAmounts> {
        let index = FORM_2200A_FIXED_ROWS.iter().position(|row| row.0 == key)?;
        self.row_mut(index)
    }

    pub fn row(&self, index: usize) -> Form2200AAmounts {
        self.schedule.get(index).copied().unwrap_or_default()
    }

    /// "Others" row `index`, growing the list to reach it.
    pub fn other_mut(&mut self, index: usize) -> Option<&mut Form2200AOtherRow> {
        if index >= FORM_2200A_OTHER_ROWS {
            return None;
        }
        if self.others.len() <= index {
            self.others
                .resize_with(index + 1, Form2200AOtherRow::default);
        }
        self.others.get_mut(index)
    }

    pub fn other(&self, index: usize) -> Form2200AOtherRow {
        self.others.get(index).cloned().unwrap_or_default()
    }

    /// The official compute chain over values held at cents.
    pub fn recompute(&mut self) {
        // onAmendedClick(): "No" resets Item 19 to 0.00.
        if !self.is_amended {
            self.previous_payment = 0.0;
        }
        if !self.tax_relief {
            self.tax_relief_specify.clear();
        }
        // The tax due of a fixed row is computed, not typed.
        if self.manner_of_payment != Form2200APayment::OtherScheme {
            self.other_scheme_description.clear();
        }
        while self.schedule.last().is_some_and(Form2200AAmounts::is_zero) {
            self.schedule.pop();
        }
        while self.others.last().is_some_and(Form2200AOtherRow::is_blank) {
            self.others.pop();
        }
        self.schedule.truncate(FORM_2200A_FIXED_ROW_COUNT);
        let mut total = 0.0;
        for (row, fixed) in self.schedule.iter_mut().zip(FORM_2200A_FIXED_ROWS) {
            row.round();
            // computeField: Due = amtFormat(ATR × Taxable).
            row.tax_due = js_to_fixed2(fixed.5 * row.taxable);
            total += row.tax_due;
        }
        for row in &mut self.others {
            row.amounts.round();
            total += row.amounts.tax_due;
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
    /// own captions). The email prints as typed. `derived:page_header` is the
    /// page 2 and 3 header comb (TIN then name, one 40-slot comb on this
    /// sheet); `derived:address_line1` / `derived:line:address_line2` put
    /// Item 7 over both its lines.
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
                        (0, place.region.trim()),
                        (10, place.province.trim()),
                        (24, place.city.trim()),
                    ],
                    40,
                ),
            );
        }
        put(
            "page_header",
            super::print_map::tin_name_header(&self.tin, &self.taxpayer_name),
        );
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        put("address_line1", line1);
        put("line:address_line2", line2);
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("{PREFIX}:{key}"), value);
        };
        let flag = |on: bool| on.to_string();

        // addZeroPadding() pads one-digit month and day.
        put("txtDateMonth", format!("{:02}", self.month));
        put("txtDateDay", format!("{:02}", self.day));
        put("txtDateYear", self.year.to_string());
        put("amendedRtn_1", flag(self.is_amended));
        put("amendedRtn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        let name = self.taxpayer_name.trim().to_string();
        // Pages 2 and 3 repeat TIN and name under the same element ids
        // (sleeptime() fills them by name), so one value covers all three.
        put("tinA", tin1);
        put("tinB", tin2);
        put("tinC", tin3);
        put("branchCode", branch);
        put("registeredName", name);
        put("rdoCode", self.rdo_code.trim().to_string());
        put(
            "registeredAddress",
            self.registered_address.trim().to_string(),
        );
        put("zipCode", self.zip_code.trim().to_string());
        put("phoneNumber", self.contact_number.trim().to_string());
        put("txtEmail", self.email.trim().to_string());
        for (prefix, place) in [
            ("prod", &self.place_of_production),
            ("rem", &self.place_of_removal),
        ] {
            put(&format!("{prefix}Region"), place_code(&place.region));
            put(&format!("{prefix}Province"), place_code(&place.province));
            put(&format!("{prefix}City"), place_code(&place.city));
        }
        put("optTreaty_1", flag(self.tax_relief));
        put("optTreaty_2", flag(!self.tax_relief));
        put("treatyY", self.tax_relief_specify.trim().to_string());
        let mop = self.manner_of_payment;
        put("optPayment_1", flag(mop == Form2200APayment::ActualRemoval));
        put("optPayment_2", flag(mop == Form2200APayment::Prepayment));
        put("optPayment_3", flag(mop == Form2200APayment::OtherScheme));
        put(
            "paymentOther",
            self.other_scheme_description.trim().to_string(),
        );

        for (key, value) in [
            ("txtExciseDue", self.excise_tax_due),
            ("txtLess_Balance", self.balance_carried_over),
            ("txtLess_Excise", self.creditable_excise_tax),
            ("txtLess_Tot", self.total_credits),
            ("txtNetTaxDue", self.net_tax_due),
            ("txtPrevReturn", self.previous_payment),
            ("txtStillDue", self.tax_still_due),
            ("txtPen_Surcharge", self.surcharge),
            ("txtPen_Interest", self.interest),
            ("txtPen_Compromise", self.compromise),
            ("txtPen_Tot", self.total_penalties),
            ("txtAmtPayable", self.amount_payable),
            ("txtPay_TaxPayment", self.tax_deposit),
            ("txtPay_Penalties", self.penalties_paid),
            ("txtPay_Tot", self.total_payment),
            ("txtBalance", self.balance_to_carry_over),
        ] {
            put(key, official_amount(value));
        }

        for (index, (key, ..)) in FORM_2200A_FIXED_ROWS.iter().enumerate() {
            let row = self.row(index);
            put(
                &format!("txtSched1_{key}_Export"),
                official_amount(row.export_exempt),
            );
            put(
                &format!("txtSched1_{key}_Taxable"),
                official_amount(row.taxable),
            );
            put(
                &format!("txtSched1_{key}_Due"),
                official_amount(row.tax_due),
            );
        }
        for index in 0..FORM_2200A_OTHER_ROWS {
            let row = self.other(index);
            let base = format!("txtSched1_xa_others{}", index + 1);
            put(&format!("{base}_Check"), flag(!row.is_blank()));
            put(&format!("{base}_Atc"), row.atc_digits.trim().to_string());
            put(&format!("{base}_Desc"), row.description.trim().to_string());
            put(&format!("{base}_Bracket"), row.bracket.trim().to_string());
            put(&format!("{base}_Rate"), row.rate.trim().to_string());
            put(
                &format!("{base}_Export"),
                official_amount(row.amounts.export_exempt),
            );
            put(
                &format!("{base}_Taxable"),
                official_amount(row.amounts.taxable),
            );
            put(&format!("{base}_Due"), official_amount(row.amounts.tax_due));
        }
        put("txtSched1_TotalDue", official_amount(self.schedule_total));
        // sleeptime() sets the pager to page 1 at load.
        put("txtCurrentPage", "1".to_string());
        fields.extend(
            self.payment
                .controls("frm2200Av2020", "frm2200Av2020:taxNum"),
        );
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

        // validateForm, in order.
        let year = i32::from(self.year);
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let max_day = match self.month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if !(1..=12).contains(&self.month) {
            err("month", "Please enter a valid month on Item 1.");
        } else if max_day == 28 && self.day > 28 {
            err(
                "day",
                "Please enter a valid date on Item 1. Filing year is not a leap year.",
            );
        } else if self.day < 1 || self.day > max_day {
            err("day", "Please enter a valid day on Item 1.");
        } else if self.year == 0 {
            err("year", "Please enter a valid year on Item 1.");
        } else if self.year < 2020 {
            err(
                "year",
                "Returning old forms is restricted to the last version up to the effectivity of the new form version.",
            );
        } else if NaiveDate::from_ymd_opt(year, u32::from(self.month), u32::from(self.day))
            .is_some_and(|date| date > today)
        {
            err(
                "day",
                "Invalid date entry on Item 1. Date cannot be after the current date.",
            );
        }
        if self.number_of_attached_sheets > 99 {
            // maxlength="2" on txtSheets.
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most two digits.",
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
            err("tin", "Please enter a valid TIN number on Item 4.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 70 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer's Name on Item 6.",
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
        if self.contact_number.trim().is_empty() {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 8.",
            );
        }
        if !place_is_official(&self.place_of_production) {
            err(
                "place_of_production",
                "Please enter a valid Place of Production on Item 10.",
            );
        }
        if !place_is_official(&self.place_of_removal) {
            err(
                "place_of_removal",
                "Please enter a valid Place of Removal on Item 11.",
            );
        }
        if self.tax_relief && self.tax_relief_specify.trim().is_empty() {
            err(
                "tax_relief_specify",
                "Please specify a Tax Relief on Item 12A.",
            );
        }
        match self.manner_of_payment {
            Form2200APayment::None => err(
                "manner_of_payment",
                "Please enter a Manner of Payment on Part II.",
            ),
            Form2200APayment::OtherScheme if self.other_scheme_description.trim().is_empty() => {
                err(
                    "other_scheme_description",
                    "Please specify a Scheme on Item 15.",
                )
            }
            _ => {}
        }

        // validateForm's Item 24 check (Item 22 less Item 23C).
        if self.amount_payable - self.total_payment > 0.0 {
            err(
                "tax_deposit",
                "YOU HAVE INSUFFICIENT FUND. PLEASE APPLY DEPOSIT TO PROCEED",
            );
        }

        // Not checked by validateForm, but needed for a meaningful return
        // and for the submission filename.
        let email = self.email.trim();
        if email.is_empty()
            || !email.contains('@')
            || email.contains(char::is_whitespace)
            || email.chars().count() > 60
        {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to (Item 9).",
            );
        }
        if self.schedule.len() > FORM_2200A_FIXED_ROW_COUNT {
            err("schedule", "Schedule 1 has 60 fixed rows.");
        }
        if self.others.len() > FORM_2200A_OTHER_ROWS {
            err("others", "Schedule 1 has 3 \"Others\" rows.");
        }
        for (index, row) in self.schedule.iter().enumerate() {
            if row
                .values()
                .iter()
                .any(|v| *v < 0.0 || !has_cent_precision(*v) || !within_official_precision(*v))
            {
                err(
                    &format!("schedule[{index}]"),
                    &format!(
                        "Schedule 1 {} ({}): amounts must be non-negative pesos and centavos below 1,000,000,000,000.",
                        FORM_2200A_FIXED_ROWS.get(index).map(|r| r.1).unwrap_or("?"),
                        FORM_2200A_FIXED_ROWS.get(index).map(|r| r.3).unwrap_or("")
                    ),
                );
            }
        }
        for (index, row) in self.others.iter().enumerate() {
            let n = index + 1;
            if row
                .amounts
                .values()
                .iter()
                .any(|v| *v < 0.0 || !has_cent_precision(*v) || !within_official_precision(*v))
            {
                err(
                    &format!("others[{index}]"),
                    &format!(
                        "Others row {n}: amounts must be non-negative pesos and centavos below 1,000,000,000,000."
                    ),
                );
            }
            // isValidDataOnSched1, for a used row; the official check runs
            // when a row is added, so a used row with no ATC is also refused.
            if !row.is_blank() {
                let atc = row.atc_digits.trim();
                if atc.len() != 3 || !digits_only(atc) {
                    err(
                        &format!("others[{index}].atc_digits"),
                        &format!("Please supply valid ATC Code for row #{n}"),
                    );
                } else if row.description.trim().is_empty() {
                    err(
                        &format!("others[{index}].description"),
                        &format!("Description is required in row #{n}"),
                    );
                } else if row.bracket.trim().is_empty() {
                    err(
                        &format!("others[{index}].bracket"),
                        &format!("Tax Bracket/Unit of Measure is required in row #{n}"),
                    );
                } else if row.rate.trim().is_empty() {
                    err(
                        &format!("others[{index}].rate"),
                        &format!("Applicable Tax Rate is required in row #{n}"),
                    );
                } else if !row
                    .rate
                    .trim()
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'.')
                {
                    // numbersonly() while typing.
                    err(
                        &format!("others[{index}].rate"),
                        &format!(
                            "Others row {n}: the tax rate takes digits and a decimal point only."
                        ),
                    );
                }
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

impl FormValidator for Form2200ADraft {
    /// `validateForm` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form2200ADraft {
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
                "frm2200Av2020:txtPen_Surcharge",
                "frm2200Av2020:txtPen_Interest",
                "frm2200Av2020:txtPen_Compromise",
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

impl QueueableForm for Form2200ADraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2200A";
    /// Official `formType` and filename segment. eBIRForms has
    /// no upload folder for `2200Av2020` (only `2200A` → `2200Av2013`); the official page
    /// passes `undefined` as the upload folder in PROD, so queueing stays off in
    /// support_level until the real folder is confirmed.
    const FORM_TYPE: &'static str = "2200Av2020";
    const LAYOUT_ID: &'static str = FORM_2200A_FORM_ID;

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
    /// `txtDateMonth + txtDateDay + txtDateYear`, as in `createXMLFileName`.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
    }

    pub(crate) fn sample() -> Form2200ADraft {
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Sample Distillery Corp", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Distilling", "registered_address": "123 Sample St Quezon City",
            "zip_code": "1100", "phone": "0281234567", "email": "sample.taxpayer@example.com",
            "default_form_type": "2200A", "taxpayer_type": "Corporation"
        }))
        .unwrap();
        let mut draft = Form2200ADraft::new_from_profile(&profile, 2025, 3);
        draft.day = 15;
        draft.place_of_production = Form2200APlace {
            region: "130000000".into(),
            province: "137400000".into(),
            city: "137403000".into(),
        };
        draft.place_of_removal = draft.place_of_production.clone();
        draft.manner_of_payment = Form2200APayment::ActualRemoval;
        draft.row_by_key_mut("1B_2024").unwrap().taxable = 1_000.005;
        draft.surcharge = 25.5;
        draft.keep_entered_penalties();
        draft.recompute();
        draft.tax_deposit = draft.tax_still_due;
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2200ADraft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    fn row_by_key(draft: &Form2200ADraft, key: &str) -> Form2200AAmounts {
        let index = FORM_2200A_FIXED_ROWS
            .iter()
            .position(|row| row.0 == key)
            .unwrap();
        draft.row(index)
    }

    #[test]
    fn print_map_adds_only_the_shared_comb_lines() {
        let mut draft = sample();
        draft.contact_number = "09171234567".into();
        draft.email = "Me@Example.ph".into();
        draft.place_of_removal = Form2200APlace {
            region: "010000000".into(),
            province: "012800000".into(),
            city: "012801000".into(),
        };
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
        assert!(line.starts_with("09171234567"));
        assert_eq!(&line[12..], "Me@Example.ph");
        assert_eq!(
            print["derived:place_of_removal"],
            "010000000 012800000     012801000"
        );
    }

    #[test]
    fn fixed_rows_match_the_official_layout() {
        assert_eq!(FORM_2200A_FIXED_ROW_COUNT, 60);
        let layout = crate::official_xml::layout(FORM_2200A_FORM_ID).unwrap();
        let keys = layout.keys();
        for (key, ..) in FORM_2200A_FIXED_ROWS {
            for column in ["Export", "Taxable", "Due"] {
                let id = format!("{PREFIX}:txtSched1_{key}_{column}");
                assert!(keys.contains(id.as_str()), "{id}");
            }
        }
        let fields = sample().to_bir_field_map();
        assert!(fields.keys().all(|key| keys.contains(key.as_str())));
    }

    #[test]
    fn rates_follow_tax_rate_xml() {
        // taxRate.xml overrides the page's 22% for 1A_2020B with 21%.
        let row = FORM_2200A_FIXED_ROWS
            .iter()
            .find(|row| row.0 == "1A_2020B")
            .unwrap();
        assert_eq!((row.4, row.5), ("21%", 0.21));
    }

    #[test]
    fn to_fixed_matches_javascript() {
        // node: (0.2*1.005).toFixed(2), (1.005).toFixed(2), (2.675).toFixed(2), (1.125).toFixed(2)
        assert_eq!(js_to_fixed2(0.2 * 1.005), 0.2);
        assert_eq!(js_to_fixed2(1.005), 1.0);
        assert_eq!(js_to_fixed2(2.675), 2.67);
        assert_eq!(js_to_fixed2(1.125), 1.13);
        assert_eq!(js_to_fixed2(66.0 * 1000.01), 66_000.66);
        assert_eq!(js_to_fixed2(-1.125), -1.13);
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        let row = row_by_key(&draft, "1B_2024");
        assert_eq!(row.taxable, 1_000.01);
        assert_eq!(row.tax_due, 66_000.66);
        assert_eq!(draft.schedule_total, 66_000.66);
        assert_eq!(draft.amount_payable, 66_026.16);
        assert_eq!(draft.balance_to_carry_over, 0.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        assert_eq!(fields["frm2200Av2020:txtDateMonth"], "03");
        assert_eq!(
            fields["frm2200Av2020:registeredName"],
            "Sample Distillery Corp"
        );
        assert_eq!(fields["frm2200Av2020:tinC"], "788");
        assert_eq!(fields["frm2200Av2020:rdoCode"], "039");
        assert_eq!(fields["frm2200Av2020:txtSched1_1B_2024_Due"], "66,000.66");
        assert_eq!(fields["frm2200Av2020:txtSched1_1B_2024_Export"], "0.00");
        assert_eq!(fields["frm2200Av2020:txtSched1_xa_others1_Check"], "false");
        assert_eq!(fields["frm2200Av2020:txtSched1_TotalDue"], "66,000.66");
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2200Av2020-03152025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        assert_eq!(sample().period_code(), "03152025");
        assert_eq!(
            Form2200ADraft::parse_period_code("03152025"),
            Some((2025, FilingPeriod::Monthly(3)))
        );
        assert_eq!(Form2200ADraft::parse_period_code("13152025"), None);
        assert_eq!(Form2200ADraft::parse_period_code("0315202"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2200ADraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(&|d| d.month = 0, "Please enter a valid month on Item 1.");
        check(
            &|d| {
                d.month = 2;
                d.day = 29;
            },
            "Please enter a valid date on Item 1. Filing year is not a leap year.",
        );
        check(&|d| d.day = 0, "Please enter a valid day on Item 1.");
        check(&|d| d.year = 0, "Please enter a valid year on Item 1.");
        check(
            &|d| d.year = 2019,
            "Returning old forms is restricted to the last version up to the effectivity of the new form version.",
        );
        check(
            &|d| d.year = 2026,
            "Invalid date entry on Item 1. Date cannot be after the current date.",
        );
        check(
            &|d| d.tin = "123".into(),
            "Please enter a valid TIN number on Item 4.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 5.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer's Name on Item 6.",
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
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number on Item 8.",
        );
        check(
            &|d| d.place_of_production.city.clear(),
            "Please enter a valid Place of Production on Item 10.",
        );
        check(
            &|d| d.place_of_removal.city = "00".into(),
            "Please enter a valid Place of Removal on Item 11.",
        );
        check(
            &|d| d.tax_relief = true,
            "Please specify a Tax Relief on Item 12A.",
        );
        check(
            &|d| d.manner_of_payment = Form2200APayment::None,
            "Please enter a Manner of Payment on Part II.",
        );
        check(
            &|d| d.manner_of_payment = Form2200APayment::OtherScheme,
            "Please specify a Scheme on Item 15.",
        );
        check(
            &|d| {
                d.tax_deposit = 0.0;
                d.recompute();
            },
            "YOU HAVE INSUFFICIENT FUND. PLEASE APPLY DEPOSIT TO PROCEED",
        );
        let other =
            |atc: &'static str, desc: &'static str, bracket: &'static str, rate: &'static str| {
                move |d: &mut Form2200ADraft| {
                    let row = d.other_mut(0).unwrap();
                    row.atc_digits = atc.into();
                    row.description = desc.into();
                    row.bracket = bracket.into();
                    row.rate = rate.into();
                    d.recompute();
                }
            };
        check(
            &other("12", "Gin", "Per Liter", "5"),
            "Please supply valid ATC Code for row #1",
        );
        check(
            &other("123", "", "Per Liter", "5"),
            "Description is required in row #1",
        );
        check(
            &other("123", "Gin", "", "5"),
            "Tax Bracket/Unit of Measure is required in row #1",
        );
        check(
            &other("123", "Gin", "Per Liter", ""),
            "Applicable Tax Rate is required in row #1",
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
            <Form2200ADraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2200ADraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
