//! BIR Form 2200T — Excise Tax Return for Tobacco, Heated Tobacco and Vapor
//! Products (January 2020).
//!
//! Ported from the official `BIR-Form2200Tv2020.hta` (eBIRForms 7.9.6.2.1):
//! the Schedule 1 total (`calculate_Sched1_TotalDue`), Part III
//! (`calculate_Part3`), `validateForm` with its exact alert texts and
//! `saveXMLsubmit` through [`crate::official_xml`].
//!
//! Schedule 1 is a fixed list of 59 product/period rows (ATC, description,
//! bracket and rate are printed by the page) plus three "Others" rows where
//! the filer types the ATC digits, description, bracket and rate. Every row
//! holds Export/Exempt and Taxable tax bases and a filer-entered Basic Excise
//! Tax Due; the page computes no row arithmetic. Item 16 is the sum of every
//! row's tax due.
//!
//! Unlike 2200M, nothing on this page uppercases text: the profile values
//! (name, address) are submitted as loaded, so the field map keeps them as
//! stored.

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
pub const FORM_2200T_FORM_ID: &str = "2200t-v2020";
const PREFIX: &str = "frm2200Tv2020";

/// Fixed Schedule 1 rows in page order: (control key between `txtSched1_`
/// and `_Export`/`_Taxable`/`_Due`, ATC, product, bracket/period, rate).
pub const FORM_2200T_FIXED_ROWS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "1A_2020",
        "XT010",
        "1.) Tobacco Products a.) Tobacco twisted by hand or reduced into a condition to be consumed in any manner other than the ordinary mode of drying and curing Per Kilogram",
        "Effective January 1, 2020",
        "2.31",
    ),
    (
        "1A_2021",
        "XT010",
        "1.) Tobacco Products a.) Tobacco twisted by hand or reduced into a condition to be consumed in any manner other than the ordinary mode of drying and curing Per Kilogram",
        "Effective January 1, 2021",
        "2.40",
    ),
    (
        "1A_2022",
        "XT010",
        "1.) Tobacco Products a.) Tobacco twisted by hand or reduced into a condition to be consumed in any manner other than the ordinary mode of drying and curing Per Kilogram",
        "Effective January 1, 2022",
        "2.50",
    ),
    (
        "1A_2023",
        "XT010",
        "1.) Tobacco Products a.) Tobacco twisted by hand or reduced into a condition to be consumed in any manner other than the ordinary mode of drying and curing Per Kilogram",
        "Effective January 1, 2023",
        "2.60",
    ),
    (
        "1B_2020",
        "XT010",
        "1.) Tobacco Products b.) Tobacco prepared or partially prepared with or without the use of any machine or instrument or without being pressed or sweetened Per Kilogram",
        "Effective January 1, 2020",
        "2.31",
    ),
    (
        "1B_2021",
        "XT010",
        "1.) Tobacco Products b.) Tobacco prepared or partially prepared with or without the use of any machine or instrument or without being pressed or sweetened Per Kilogram",
        "Effective January 1, 2021",
        "2.40",
    ),
    (
        "1B_2022",
        "XT010",
        "1.) Tobacco Products b.) Tobacco prepared or partially prepared with or without the use of any machine or instrument or without being pressed or sweetened Per Kilogram",
        "Effective January 1, 2022",
        "2.50",
    ),
    (
        "1B_2023",
        "XT010",
        "1.) Tobacco Products b.) Tobacco prepared or partially prepared with or without the use of any machine or instrument or without being pressed or sweetened Per Kilogram",
        "Effective January 1, 2023",
        "2.60",
    ),
    (
        "1C_2020",
        "XT010",
        "1.) Tobacco Products c.) Fine-cut shorts and refuse, scraps, slippings, cuttings, stems, midribs and sweepings of tobacco Per Kilogram",
        "Effective January 1, 2020",
        "2.31",
    ),
    (
        "1C_2021",
        "XT010",
        "1.) Tobacco Products c.) Fine-cut shorts and refuse, scraps, slippings, cuttings, stems, midribs and sweepings of tobacco Per Kilogram",
        "Effective January 1, 2021",
        "2.40",
    ),
    (
        "1C_2022",
        "XT010",
        "1.) Tobacco Products c.) Fine-cut shorts and refuse, scraps, slippings, cuttings, stems, midribs and sweepings of tobacco Per Kilogram",
        "Effective January 1, 2022",
        "2.50",
    ),
    (
        "1C_2023",
        "XT010",
        "1.) Tobacco Products c.) Fine-cut shorts and refuse, scraps, slippings, cuttings, stems, midribs and sweepings of tobacco Per Kilogram",
        "Effective January 1, 2023",
        "2.60",
    ),
    (
        "2_2020",
        "XT020",
        "2.) Chewing tobacco unsuitable for use in any other manner Per Kilogram",
        "Effective January 1, 2020",
        "1.97",
    ),
    (
        "2_2021",
        "XT020",
        "2.) Chewing tobacco unsuitable for use in any other manner Per Kilogram",
        "Effective January 1, 2021",
        "2.05",
    ),
    (
        "2_2022",
        "XT020",
        "2.) Chewing tobacco unsuitable for use in any other manner Per Kilogram",
        "Effective January 1, 2022",
        "2.13",
    ),
    (
        "2_2023",
        "XT020",
        "2.) Chewing tobacco unsuitable for use in any other manner Per Kilogram",
        "Effective January 1, 2023",
        "2.22",
    ),
    (
        "3A_2020",
        "XT035",
        "3.) Cigars a.) Ad Valorem Tax Based on the Net Retail Price (NRP) per Cigar [excluding the excise and value-added tax (VAT)] NRP Per Cigar",
        "Effective January 1, 2020",
        "20%",
    ),
    (
        "3A_2021",
        "XT035",
        "3.) Cigars a.) Ad Valorem Tax Based on the Net Retail Price (NRP) per Cigar [excluding the excise and value-added tax (VAT)] NRP Per Cigar",
        "Effective January 1, 2021",
        "20%",
    ),
    (
        "3A_2022",
        "XT035",
        "3.) Cigars a.) Ad Valorem Tax Based on the Net Retail Price (NRP) per Cigar [excluding the excise and value-added tax (VAT)] NRP Per Cigar",
        "Effective January 1, 2022",
        "20%",
    ),
    (
        "3A_2023",
        "XT035",
        "3.) Cigars a.) Ad Valorem Tax Based on the Net Retail Price (NRP) per Cigar [excluding the excise and value-added tax (VAT)] NRP Per Cigar",
        "Effective January 1, 2023",
        "20%",
    ),
    (
        "3B_2020",
        "XT036",
        "3.) Cigars b.) Specific Tax per Cigar, in addition to Ad Valorem Tax Per Cigar",
        "Effective January 1, 2020",
        "6.57",
    ),
    (
        "3B_2021",
        "XT036",
        "3.) Cigars b.) Specific Tax per Cigar, in addition to Ad Valorem Tax Per Cigar",
        "Effective January 1, 2021",
        "6.83",
    ),
    (
        "3B_2022",
        "XT036",
        "3.) Cigars b.) Specific Tax per Cigar, in addition to Ad Valorem Tax Per Cigar",
        "Effective January 1, 2022",
        "7.10",
    ),
    (
        "3B_2023",
        "XT036",
        "3.) Cigars b.) Specific Tax per Cigar, in addition to Ad Valorem Tax Per Cigar",
        "Effective January 1, 2023",
        "7.38",
    ),
    (
        "4A_2020",
        "XT040",
        "4.) Cigarettes a.) Cigarettes packed by hand Per Pack",
        "Effective January 1, 2020",
        "45.00",
    ),
    (
        "4A_2021",
        "XT040",
        "4.) Cigarettes a.) Cigarettes packed by hand Per Pack",
        "Effective January 1, 2021",
        "50.00",
    ),
    (
        "4A_2022",
        "XT040",
        "4.) Cigarettes a.) Cigarettes packed by hand Per Pack",
        "Effective January 1, 2022",
        "55.00",
    ),
    (
        "4A_2023",
        "XT040",
        "4.) Cigarettes a.) Cigarettes packed by hand Per Pack",
        "Effective January 1, 2023",
        "60.00",
    ),
    (
        "4B_2020",
        "XT155",
        "4.) Cigarettes b.) Cigarettes packed by machine (RA No. 11346) Per Pack",
        "Effective January 1, 2020",
        "45.00",
    ),
    (
        "4B_2021",
        "XT155",
        "4.) Cigarettes b.) Cigarettes packed by machine (RA No. 11346) Per Pack",
        "Effective January 1, 2021",
        "50.00",
    ),
    (
        "4B_2022",
        "XT155",
        "4.) Cigarettes b.) Cigarettes packed by machine (RA No. 11346) Per Pack",
        "Effective January 1, 2022",
        "55.00",
    ),
    (
        "4B_2023",
        "XT155",
        "4.) Cigarettes b.) Cigarettes packed by machine (RA No. 11346) Per Pack",
        "Effective January 1, 2023",
        "60.00",
    ),
    (
        "5_2020",
        "XT160",
        "5.) Heated Tobacco Products (RA No. 11346 and 11467) Per Pack",
        "Effective January 1, 2020",
        "10.00",
    ),
    (
        "5_1.23.2020",
        "XT160",
        "5.) Heated Tobacco Products (RA No. 11346 and 11467) Per Pack",
        "Effective January 23, 2020",
        "25.00",
    ),
    (
        "5_2021",
        "XT160",
        "5.) Heated Tobacco Products (RA No. 11346 and 11467) Per Pack",
        "Effective January 1, 2021",
        "27.50",
    ),
    (
        "5_2022",
        "XT160",
        "5.) Heated Tobacco Products (RA No. 11346 and 11467) Per Pack",
        "Effective January 1, 2022",
        "30.00",
    ),
    (
        "5_2023",
        "XT160",
        "5.) Heated Tobacco Products (RA No. 11346 and 11467) Per Pack",
        "Effective January 1, 2023",
        "32.50",
    ),
    (
        "6_10ml",
        "XT165",
        "6.) Vapor Products (RA No. 11346) Effective January 1-22, 2020",
        "0.00 ml to 10.00 ml",
        "10.00",
    ),
    (
        "6_20ml",
        "XT165",
        "6.) Vapor Products (RA No. 11346) Effective January 1-22, 2020",
        "10.01 ml to 20.00 ml",
        "20.00",
    ),
    (
        "6_30ml",
        "XT165",
        "6.) Vapor Products (RA No. 11346) Effective January 1-22, 2020",
        "20.01 ml to 30.00 ml",
        "30.00",
    ),
    (
        "6_40ml",
        "XT165",
        "6.) Vapor Products (RA No. 11346) Effective January 1-22, 2020",
        "30.01 ml to 40.00 ml",
        "40.00",
    ),
    (
        "6_50ml",
        "XT165",
        "6.) Vapor Products (RA No. 11346) Effective January 1-22, 2020",
        "40.01 ml to 50.00 ml",
        "50.00",
    ),
    (
        "6_above_50ml",
        "XT165",
        "6.) Vapor Products (RA No. 11346) Effective January 1-22, 2020",
        "More than 50.00 ml",
        "50.00 plus 10.00 for every additional 10.00 ml",
    ),
    (
        "7A_2020",
        "XT170",
        "7.) Vapor Products (RA No. 11467) a.) Nicotine Salt or Salt Nicotine Per Milliliter",
        "Effective January 23, 2020",
        "37.00",
    ),
    (
        "7A_2021",
        "XT170",
        "7.) Vapor Products (RA No. 11467) a.) Nicotine Salt or Salt Nicotine Per Milliliter",
        "Effective January 1, 2021",
        "42.00",
    ),
    (
        "7A_2022",
        "XT170",
        "7.) Vapor Products (RA No. 11467) a.) Nicotine Salt or Salt Nicotine Per Milliliter",
        "Effective January 1, 2022",
        "47.00",
    ),
    (
        "7A_2023",
        "XT170",
        "7.) Vapor Products (RA No. 11467) a.) Nicotine Salt or Salt Nicotine Per Milliliter",
        "Effective January 1, 2023",
        "52.00",
    ),
    (
        "7B_2020",
        "XT180",
        "7.) Vapor Products (RA No. 11467) b.) Conventional 'Freebase' or 'Classic' Nicotine Per Ten (10) Milliliters",
        "Effective January 1, 2020",
        "45.00",
    ),
    (
        "7B_2021",
        "XT180",
        "7.) Vapor Products (RA No. 11467) b.) Conventional 'Freebase' or 'Classic' Nicotine Per Ten (10) Milliliters",
        "Effective January 1, 2021",
        "50.00",
    ),
    (
        "7B_2022",
        "XT180",
        "7.) Vapor Products (RA No. 11467) b.) Conventional 'Freebase' or 'Classic' Nicotine Per Ten (10) Milliliters",
        "Effective January 1, 2022",
        "55.00",
    ),
    (
        "7B_2023",
        "XT180",
        "7.) Vapor Products (RA No. 11467) b.) Conventional 'Freebase' or 'Classic' Nicotine Per Ten (10) Milliliters",
        "Effective January 1, 2023",
        "60.00",
    ),
    (
        "xt080",
        "XT080",
        "1.) For Cigars Per 1,000 Cigars",
        "",
        "P 0.50",
    ),
    (
        "xt090",
        "XT090",
        "2.) For Cigarettes Per 1,000 Stick",
        "",
        "P 0.10",
    ),
    (
        "xt100",
        "XT100",
        "3.) For Leaf Tobacco Per Kilogram",
        "",
        "P 0.02",
    ),
    (
        "xt110",
        "XT110",
        "4.) For scraps and other manufactured tobacco products Per Kilogram",
        "",
        "P 0.03",
    ),
    (
        "xt120_leaf",
        "XT120",
        "5.) Additional import blending tobacco inspection and monitoring fees - leaf Per Kilogram",
        "",
        "P 0.02",
    ),
    (
        "xt120_scraps",
        "XT120",
        "5.) Additional import blending tobacco inspection and monitoring fees - partially manufactured (scraps and strips) Per Kilogram",
        "",
        "P 0.03",
    ),
    (
        "xt190",
        "XT190",
        "6.) Heated Tobacco Products Per 1,000 Units",
        "",
        "P 0.10",
    ),
    (
        "xt200",
        "XT200",
        "7.) Vapor Products Per Milliliter",
        "",
        "P 0.01",
    ),
];
/// Number of fixed rows (59 on the official page).
pub const FORM_2200T_FIXED_ROW_COUNT: usize = FORM_2200T_FIXED_ROWS.len();
/// "Others (specify)" rows at the end of Schedule 1.
pub const FORM_2200T_OTHER_ROWS: usize = 3;

/// Items 10 and 11: region, province and city codes from the official
/// dropdowns. Blank means the `(Select …)` placeholder, `00`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Form2200TPlace {
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
pub enum Form2200TPayment {
    #[default]
    None,
    /// Item 13 — Payment on actual performance of service / removal.
    ActualRemoval,
    /// Item 14 — Prepayment / Advance Deposit.
    Prepayment,
    /// Item 15 — Other Similar Schemes.
    OtherScheme,
}

/// Tax bases and tax due of one Schedule 1 row (`round(this,2)` values).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200TAmounts {
    #[serde(default)]
    pub export_exempt: f64,
    #[serde(default)]
    pub taxable: f64,
    #[serde(default)]
    pub tax_due: f64,
}

impl Form2200TAmounts {
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

/// One "Others (specify)" row: free-text ATC digits (after the printed
/// `XT`, max 3), description, bracket and rate.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200TOtherRow {
    #[serde(default)]
    pub atc_digits: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub bracket: String,
    #[serde(default)]
    pub rate: String,
    #[serde(default)]
    pub amounts: Form2200TAmounts,
}

impl Form2200TOtherRow {
    pub fn is_blank(&self) -> bool {
        self.atc_digits.trim().is_empty()
            && self.description.trim().is_empty()
            && self.bracket.trim().is_empty()
            && self.rate.trim().is_empty()
            && self.amounts.is_zero()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2200TDraft {
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
    pub place_of_production: Form2200TPlace,
    #[serde(default)]
    pub place_of_removal: Form2200TPlace,
    /// Item 12 (the official page starts on "No").
    #[serde(default)]
    pub tax_relief: bool,
    #[serde(default)]
    pub tax_relief_specify: String,

    // Part II
    #[serde(default)]
    pub manner_of_payment: Form2200TPayment,
    #[serde(default)]
    pub other_scheme_description: String,

    // Part V — Schedule 1
    /// Fixed rows in [`FORM_2200T_FIXED_ROWS`] order; missing rows are zero.
    #[serde(default)]
    pub schedule: Vec<Form2200TAmounts>,
    /// "Others (specify)" rows, at most three.
    #[serde(default)]
    pub others: Vec<Form2200TOtherRow>,
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
fn place_is_official(place: &Form2200TPlace) -> bool {
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

impl Form2200TDraft {
    pub const FORM_CODE: &'static str = "2200T";

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
            place_of_production: Form2200TPlace::default(),
            place_of_removal: Form2200TPlace::default(),
            tax_relief: false,
            tax_relief_specify: String::new(),
            manner_of_payment: Form2200TPayment::None,
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
    pub fn row_mut(&mut self, index: usize) -> Option<&mut Form2200TAmounts> {
        if index >= FORM_2200T_FIXED_ROW_COUNT {
            return None;
        }
        if self.schedule.len() <= index {
            self.schedule
                .resize_with(index + 1, Form2200TAmounts::default);
        }
        self.schedule.get_mut(index)
    }

    /// Fixed row by its control key (e.g. `"4B_2023"`).
    pub fn row_by_key_mut(&mut self, key: &str) -> Option<&mut Form2200TAmounts> {
        let index = FORM_2200T_FIXED_ROWS.iter().position(|row| row.0 == key)?;
        self.row_mut(index)
    }

    pub fn row(&self, index: usize) -> Form2200TAmounts {
        self.schedule.get(index).copied().unwrap_or_default()
    }

    /// "Others" row `index`, growing the list to reach it.
    pub fn other_mut(&mut self, index: usize) -> Option<&mut Form2200TOtherRow> {
        if index >= FORM_2200T_OTHER_ROWS {
            return None;
        }
        if self.others.len() <= index {
            self.others
                .resize_with(index + 1, Form2200TOtherRow::default);
        }
        self.others.get_mut(index)
    }

    pub fn other(&self, index: usize) -> Form2200TOtherRow {
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
        if self.manner_of_payment != Form2200TPayment::OtherScheme {
            self.other_scheme_description.clear();
        }
        while self.schedule.last().is_some_and(Form2200TAmounts::is_zero) {
            self.schedule.pop();
        }
        while self.others.last().is_some_and(Form2200TOtherRow::is_blank) {
            self.others.pop();
        }
        let mut total = 0.0;
        for row in &mut self.schedule {
            row.round();
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

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the page 2 and 3 header TIN, one 14-digit
    ///   comb each.
    /// - `derived:address_line1` / `derived:line:address_line2`: Item 7
    ///   over its 40-slot first line, broken at a space, and the second.
    /// - `derived:line:item{10,11}_{region,province,city}`: Items 10 and 11
    ///   as place names (the upload carries the PSGC codes); a name longer
    ///   than its comb prints whole in smaller type.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm2200Tv2020:tinA_2",
                "frm2200Tv2020:tinB_2",
                "frm2200Tv2020:tinC_2",
                "frm2200Tv2020:branchCode_2",
            ],
        );
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        for (key, value) in [
            ("tin_digits", tin_digits),
            ("address_line1", line1),
            ("line:address_line2", line2),
        ] {
            fields.insert(format!("derived:{key}"), value);
        }
        for (item, place) in [
            (10, &self.place_of_production),
            (11, &self.place_of_removal),
        ] {
            let names = super::print_map::place_names(&place.region, &place.province, &place.city);
            for (level, name) in ["region", "province", "city"].into_iter().zip(names) {
                fields.insert(format!("derived:line:item{item}_{level}"), name);
            }
        }
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
        // sleeptime() copies TIN and name onto pages 2 and 3.
        for suffix in ["", "_2", "_3"] {
            put(&format!("tinA{suffix}"), tin1.clone());
            put(&format!("tinB{suffix}"), tin2.clone());
            put(&format!("tinC{suffix}"), tin3.clone());
            put(&format!("branchCode{suffix}"), branch.clone());
            put(&format!("registeredName{suffix}"), name.clone());
        }
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
        put("optPayment_1", flag(mop == Form2200TPayment::ActualRemoval));
        put("optPayment_2", flag(mop == Form2200TPayment::Prepayment));
        put("optPayment_3", flag(mop == Form2200TPayment::OtherScheme));
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

        for (index, (key, ..)) in FORM_2200T_FIXED_ROWS.iter().enumerate() {
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
        for index in 0..FORM_2200T_OTHER_ROWS {
            let row = self.other(index);
            let base = format!("txtSched1_xt_others{}", index + 1);
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
                .controls("frm2200Tv2020", "frm2200Tv2020:taxNum"),
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
        } else if self.year < 1904 {
            err(
                "year",
                "Invalid date entry on Item 1. Entry should not be lower than 1904.",
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
            Form2200TPayment::None => err(
                "manner_of_payment",
                "Please enter a Manner of Payment on Part II.",
            ),
            Form2200TPayment::OtherScheme if self.other_scheme_description.trim().is_empty() => {
                err(
                    "other_scheme_description",
                    "Please specify a Scheme on Item 15.",
                )
            }
            _ => {}
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
        if self.schedule.len() > FORM_2200T_FIXED_ROW_COUNT {
            err("schedule", "Schedule 1 has 59 fixed rows.");
        }
        if self.others.len() > FORM_2200T_OTHER_ROWS {
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
                        FORM_2200T_FIXED_ROWS.get(index).map(|r| r.1).unwrap_or("?"),
                        FORM_2200T_FIXED_ROWS.get(index).map(|r| r.3).unwrap_or("")
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
            if !row.is_blank() {
                let atc = row.atc_digits.trim();
                if atc.is_empty() || atc.chars().count() > 3 {
                    err(
                        &format!("others[{index}].atc_digits"),
                        &format!("Others row {n}: enter the ATC (up to 3 characters after XT)."),
                    );
                }
                if row.description.trim().is_empty() {
                    err(
                        &format!("others[{index}].description"),
                        &format!("Others row {n}: enter a description."),
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

impl FormValidator for Form2200TDraft {
    /// `validateForm` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form2200TDraft {
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
                "frm2200Tv2020:txtPen_Surcharge",
                "frm2200Tv2020:txtPen_Interest",
                "frm2200Tv2020:txtPen_Compromise",
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

impl QueueableForm for Form2200TDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2200T";
    /// Official `formType`, filename segment and upload folder
    /// (`2200Tv2020`).
    const FORM_TYPE: &'static str = "2200Tv2020";
    const LAYOUT_ID: &'static str = FORM_2200T_FORM_ID;

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

    pub(crate) fn sample() -> Form2200TDraft {
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Sample Tobacco Corp", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Tobacco", "registered_address": "123 Sample St Quezon City",
            "zip_code": "1100", "phone": "0281234567", "email": "sample.taxpayer@example.com",
            "default_form_type": "2200T", "taxpayer_type": "Corporation"
        }))
        .unwrap();
        let mut draft = Form2200TDraft::new_from_profile(&profile, 2025, 3);
        draft.day = 15;
        draft.place_of_production = Form2200TPlace {
            region: "130000000".into(),
            province: "137400000".into(),
            city: "137403000".into(),
        };
        draft.place_of_removal = draft.place_of_production.clone();
        draft.manner_of_payment = Form2200TPayment::ActualRemoval;
        let row = draft.row_by_key_mut("4B_2023").unwrap();
        row.taxable = 1_000.0;
        row.tax_due = 60_000.005;
        draft.surcharge = 25.5;
        draft.keep_entered_penalties();
        draft.tax_deposit = 50_000.0;
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2200TDraft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn fixed_rows_match_the_official_layout() {
        assert_eq!(FORM_2200T_FIXED_ROW_COUNT, 59);
        let layout = crate::official_xml::layout(FORM_2200T_FORM_ID).unwrap();
        let keys = layout.keys();
        for (key, ..) in FORM_2200T_FIXED_ROWS {
            for column in ["Export", "Taxable", "Due"] {
                let id = format!("{PREFIX}:txtSched1_{key}_{column}");
                assert!(keys.contains(id.as_str()), "{id}");
            }
        }
        let fields = sample().to_bir_field_map();
        assert!(fields.keys().all(|key| keys.contains(key.as_str())));
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert_eq!(draft.row_by_key_mut_clone("4B_2023").tax_due, 60_000.01);
        assert_eq!(draft.schedule_total, 60_000.01);
        assert_eq!(draft.excise_tax_due, 60_000.01);
        assert_eq!(draft.amount_payable, 60_025.51);
        assert_eq!(draft.total_payment, 50_025.5);
        assert_eq!(draft.balance_to_carry_over, 10_000.01);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    impl Form2200TDraft {
        fn row_by_key_mut_clone(&self, key: &str) -> Form2200TAmounts {
            let index = FORM_2200T_FIXED_ROWS
                .iter()
                .position(|row| row.0 == key)
                .unwrap();
            self.row(index)
        }
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        assert_eq!(fields["frm2200Tv2020:txtDateMonth"], "03");
        assert_eq!(
            fields["frm2200Tv2020:registeredName"],
            "Sample Tobacco Corp"
        );
        assert_eq!(
            fields["frm2200Tv2020:registeredName_3"],
            "Sample Tobacco Corp"
        );
        assert_eq!(fields["frm2200Tv2020:tinC_2"], "788");
        assert_eq!(fields["frm2200Tv2020:rdoCode"], "039");
        assert_eq!(fields["frm2200Tv2020:txtSched1_4B_2023_Due"], "60,000.01");
        assert_eq!(fields["frm2200Tv2020:txtSched1_4B_2023_Export"], "0.00");
        assert_eq!(fields["frm2200Tv2020:txtSched1_xt_others1_Atc"], "");
        assert_eq!(fields["frm2200Tv2020:txtSched1_TotalDue"], "60,000.01");
        assert_eq!(fields["frm2200Tv2020:optTreaty_2"], "true");
        assert_eq!(fields["frm2200Tv2020:prodCity"], "137403000");
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2200Tv2020-03152025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        assert_eq!(sample().period_code(), "03152025");
        assert_eq!(
            Form2200TDraft::parse_period_code("03152025"),
            Some((2025, FilingPeriod::Monthly(3)))
        );
        assert_eq!(Form2200TDraft::parse_period_code("13152025"), None);
        assert_eq!(Form2200TDraft::parse_period_code("0315202"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2200TDraft), expected: &str| {
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
        check(&|d| d.day = 32, "Please enter a valid day on Item 1.");
        check(&|d| d.year = 0, "Please enter a valid year on Item 1.");
        check(
            &|d| d.year = 1903,
            "Invalid date entry on Item 1. Entry should not be lower than 1904.",
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
            &|d| d.manner_of_payment = Form2200TPayment::None,
            "Please enter a Manner of Payment on Part II.",
        );
        check(
            &|d| d.manner_of_payment = Form2200TPayment::OtherScheme,
            "Please specify a Scheme on Item 15.",
        );
        check(
            &|d| {
                d.other_mut(0).unwrap().amounts.tax_due = 5.0;
                d.recompute();
            },
            "Others row 1: enter a description.",
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
            <Form2200TDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2200TDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
