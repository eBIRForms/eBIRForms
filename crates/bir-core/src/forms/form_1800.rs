//! BIR Form 1800 (January 2018) — Donor's Tax Return.
//!
//! Ported from the official `BIR-Form1800v2018.hta` (eBIRForms 7.9.6.2.1):
//! the compute chain (`computeScheduleA`, `computeScheduleB2`, `compute27` …
//! `compute38`, `compute16`, `compute17D`, `compute18`, `computePenalties`,
//! `computeTotalAmtPayable`), `validate()` / `checkDate1()` /
//! `initialValidateBeforeSave()` with their exact alert texts, and
//! the uploaded file (`saveEncryptedProfile`) through [`crate::official_xml`]. Background
//! information comes from the taxpayer profile the way `loadBGData()` fills
//! it.
//!
//! Two official defects are surfaced instead of filed silently:
//! `compute33()` reads controls that do not exist, so Items 28–32 never reach
//! Item 33; and Item 38 goes negative (a negative tax due) when net gifts are
//! within the ₱250,000 exemption.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::official_inputs::{
    RowInsertion, cents, digits_only, extend_layout, has_cent_precision, split_tin,
    tin_is_well_formed, within_round_limit,
};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{js_escape, js_unescape, official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1800_FORM_ID: &str = "1800-v2018";
/// Donees A–E (Item 12).
pub const FORM_1800_DONEES: usize = 5;
/// Rows Schedules A and B open with; `addScheduleA` / `addScheduleB1` /
/// `addScheduleB2` append more without a limit.
pub const FORM_1800_SCHEDULE_ROWS: usize = 5;
/// Deductions, Items 28–32.
pub const FORM_1800_DEDUCTIONS: usize = 5;
/// Item 37 exempt gift.
pub const FORM_1800_EXEMPT_GIFT: f64 = 250_000.0;
/// Item 15 rate.
pub const FORM_1800_TAX_RATE: f64 = 0.06;

/// Item 12 row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1800Donee {
    pub name: String,
    /// Up to 14 digits.
    pub tin: String,
}

/// Schedule A row: donated personal property.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1800PersonalProperty {
    pub particulars: String,
    pub fair_market_value: f64,
}

/// Schedule B row: donated real property (both halves of the table).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1800RealProperty {
    pub title_number: String,
    pub tax_declaration_number: String,
    pub location: String,
    pub lot_or_improvement: String,
    pub classification: String,
    pub area: String,
    pub fmv_per_tax_declaration: String,
    pub fmv_per_zonal_value: String,
    pub fair_market_value: f64,
}

/// Items 28–32.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1800Deduction {
    pub title: String,
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1800Draft {
    #[serde(default)]
    pub id: Option<i64>,
    /// Dashboard year and open-ended key that identify this return.
    pub filing_year: u16,
    pub open_ended_key: u32,

    // Items 1–4
    pub donation_month: u8,
    pub donation_day: u8,
    pub donation_year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I (taxpayer profile)
    pub tin: String,
    pub rdo_code: String,
    pub donor_name: String,
    pub registered_address: String,
    pub zip_code: String,
    /// Item 9; the page copies the registered address.
    pub residence_address: String,
    pub residence_zip_code: String,
    pub contact_number: String,
    pub email: String,
    #[serde(default)]
    pub line_of_business: String,
    #[serde(default)]
    pub donees: Vec<Form1800Donee>,
    /// Item 13; `Some` when "Yes", holding Item 13A.
    #[serde(default)]
    pub tax_relief: Option<String>,

    // Part IV / V
    #[serde(default)]
    pub personal_properties: Vec<Form1800PersonalProperty>,
    #[serde(default)]
    pub real_properties: Vec<Form1800RealProperty>,
    #[serde(default)]
    pub deductions: Vec<Form1800Deduction>,
    /// Item 35.
    #[serde(default)]
    pub prior_net_gifts: f64,
    #[serde(default)]
    pub total_personal: f64,
    #[serde(default)]
    pub total_real: f64,
    /// Item 27.
    #[serde(default)]
    pub total_gifts: f64,
    /// Item 33 (stays 0.00 on the official page).
    #[serde(default)]
    pub total_deductions: f64,
    /// Item 34.
    #[serde(default)]
    pub net_gifts_this_return: f64,
    /// Item 36.
    #[serde(default)]
    pub total_net_gifts: f64,
    /// Items 38 and 14.
    #[serde(default)]
    pub net_gifts_subject_to_tax: f64,

    // Part II
    /// 16.
    #[serde(default)]
    pub tax_due: f64,
    /// 17A–17D.
    #[serde(default)]
    pub prior_gift_payments: f64,
    #[serde(default)]
    pub foreign_tax_paid: f64,
    #[serde(default)]
    pub tax_paid_previous: f64,
    #[serde(default)]
    pub total_credits: f64,
    /// 18.
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
    /// 20.
    #[serde(default)]
    pub total_amount_payable: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `loadBGData` splits the *escaped* profile address at 100 characters.
fn split_address(address: &str) -> (String, String) {
    let escaped = js_escape(address);
    if escaped.len() <= 100 {
        return (address.to_string(), String::new());
    }
    let (first, second) = escaped.split_at(100);
    (
        js_unescape(first).unwrap_or_else(|| first.to_string()),
        js_unescape(second).unwrap_or_else(|| second.to_string()),
    )
}

impl Form1800Draft {
    pub const FORM_CODE: &'static str = "1800";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, open_ended_key: u32) -> Self {
        let mut draft = Self {
            // Maintainer decision (1800/1801 penalties run from the due date):
            // a new return's penalties run from its due date,
            // 30 days after the donation date it carries (Item 1).
            penalty: crate::forms::penalty_items::PenaltyState::for_profile_typed_due_date(profile),
            id: None,
            filing_year: year,
            open_ended_key,
            // Item 1 starts blank like the official txtMonth/txtDate/txtYear.
            donation_month: 0,
            donation_day: 0,
            donation_year: 0,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            donor_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            residence_address: profile.registered_address.clone(),
            residence_zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            line_of_business: profile.line_of_business.clone(),
            donees: Vec::new(),
            tax_relief: None,
            personal_properties: Vec::new(),
            real_properties: Vec::new(),
            deductions: Vec::new(),
            prior_net_gifts: 0.0,
            total_personal: 0.0,
            total_real: 0.0,
            total_gifts: 0.0,
            total_deductions: 0.0,
            net_gifts_this_return: 0.0,
            total_net_gifts: 0.0,
            net_gifts_subject_to_tax: 0.0,
            tax_due: 0.0,
            prior_gift_payments: 0.0,
            foreign_tax_paid: 0.0,
            tax_paid_previous: 0.0,
            total_credits: 0.0,
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

    /// Item 2 (`processAmended`): "No" resets 17C to `0.00`.
    pub fn set_amended(&mut self, amended: bool) {
        self.is_amended = amended;
        if !amended {
            self.tax_paid_previous = 0.0;
        }
        self.recompute();
    }

    /// Tax due (Item 38) is zero or less because of Items 28-32 or the
    /// P250,000 exemption: the case that needs an in-app confirm before
    /// Submit. Recompute first.
    pub fn zero_tax_due_needs_confirm(&self) -> bool {
        let has_deductions = self.deductions.iter().any(|row| row.amount != 0.0);
        let within_exemption = self.total_net_gifts <= FORM_1800_EXEMPT_GIFT;
        self.tax_due <= 0.0 && (has_deductions || within_exemption)
    }

    /// The official compute chain, run as if every changed field was blurred.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_paid_previous = 0.0;
        }
        let mut total = 0.0;
        for row in &mut self.personal_properties {
            row.fair_market_value = cents(row.fair_market_value);
            total = cents(total + row.fair_market_value);
        }
        self.total_personal = total;
        let mut total = 0.0;
        for row in &mut self.real_properties {
            row.fair_market_value = cents(row.fair_market_value);
            total = cents(total + row.fair_market_value);
        }
        self.total_real = total;
        for row in &mut self.deductions {
            row.amount = cents(row.amount);
        }
        self.total_gifts = cents(self.total_personal + self.total_real);
        // compute33() throws on controls the page does not have; Item 33
        // keeps its initial 0.00.
        self.total_deductions = 0.0;
        self.net_gifts_this_return = cents(self.total_gifts - self.total_deductions);
        self.prior_net_gifts = cents(self.prior_net_gifts);
        self.total_net_gifts = cents(self.net_gifts_this_return + self.prior_net_gifts);
        self.net_gifts_subject_to_tax = cents(self.total_net_gifts - FORM_1800_EXEMPT_GIFT);
        self.tax_due = cents(self.net_gifts_subject_to_tax * FORM_1800_TAX_RATE);
        self.prior_gift_payments = cents(self.prior_gift_payments);
        self.foreign_tax_paid = cents(self.foreign_tax_paid);
        self.tax_paid_previous = cents(self.tax_paid_previous);
        self.total_credits =
            cents(self.prior_gift_payments + self.foreign_tax_paid + self.tax_paid_previous);
        self.tax_payable = cents(self.tax_due - self.total_credits);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_payable = cents(self.tax_payable + self.total_penalties);
    }

    fn donation_date(&self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(
            i32::from(self.donation_year),
            u32::from(self.donation_month),
            u32::from(self.donation_day),
        )
    }

    /// The official control values the upload loop writes, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1800v2018:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let amount = official_amount;
        // Every `onblur="capital(this, event)"` calls the later, argument-less
        // `capital()` in js/string-util.js, which uppercases every text control
        // on the page (the e-mail included); a filer always passes one (the
        // donee names).
        let up = |value: &str| value.trim().to_uppercase();

        put("txtMonth", format!("{:02}", self.donation_month));
        put("txtDate", format!("{:02}", self.donation_day));
        put("txtYear", self.donation_year.to_string());
        put("AmendedRtn_1", flag(self.is_amended));
        put("AmendedRtn_2", flag(!self.is_amended));
        put(
            "txtSheets",
            format!("{:02}", self.number_of_attached_sheets),
        );
        put("txtATC", "DN 010".to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        for (page1, page2, value) in [
            ("txtTIN1", "txtPg2TIN1", &tin1),
            ("txtTIN2", "txtPg2TIN2", &tin2),
            ("txtTIN3", "txtPg2TIN3", &tin3),
            ("txtBranchCode", "txtPg2BranchCode", &branch),
        ] {
            put(page1, value.clone());
            put(page2, value.clone());
        }
        put(
            "txtRDOCode",
            if self.rdo_code.trim().is_empty() {
                "000".to_string()
            } else {
                self.rdo_code.trim().to_string()
            },
        );
        put("txtDonorName", up(&self.donor_name));
        put("txtPg2TaxpayerName", up(&self.donor_name));
        let (reg1, reg2) = split_address(&up(&self.registered_address));
        put("txtRegAddress", reg1.clone());
        put("txtRegAddress2", reg2.clone());
        put("txtZipCode", self.zip_code.clone());
        // Item 9 starts as a copy of Item 8.
        let (res1, res2) = split_address(&up(&self.residence_address));
        put("txtResAddress", res1);
        put("txtResAddress2", res2);
        put("txtZipCode2", self.residence_zip_code.trim().to_string());
        put("txtContact", self.contact_number.clone());
        put("txtEmail", up(&self.email));
        for (index, letter) in ["A", "B", "C", "D", "E"].iter().enumerate() {
            let donee = self.donees.get(index).cloned().unwrap_or_default();
            put(&format!("txtDoneeName{letter}"), up(&donee.name));
            put(
                &format!("txtDoneeTIN{letter}"),
                donee.tin.trim().to_string(),
            );
        }
        put("TaxTreatyYN_Y", flag(self.tax_relief.is_some()));
        put("TaxTreatyYN_N", flag(self.tax_relief.is_none()));
        put(
            "TaxTreaty",
            self.tax_relief.as_deref().map(up).unwrap_or_default(),
        );
        put("TotalNetGifts", amount(self.net_gifts_subject_to_tax));
        put("DonorTaxDue", amount(self.tax_due));
        put("TaxCredit17A", amount(self.prior_gift_payments));
        put("TaxCredit17B", amount(self.foreign_tax_paid));
        put("TaxCredit17C", amount(self.tax_paid_previous));
        put("TaxCredit17D", amount(self.total_credits));
        put("txtTaxPayable", amount(self.tax_payable));
        put("Surcharge", amount(self.surcharge));
        put("Interest", amount(self.interest));
        put("Compromise", amount(self.compromise));
        put("TotalPenalties", amount(self.total_penalties));
        put("TotalAmountPayable", amount(self.total_amount_payable));
        put("PersonalProperties", amount(self.total_personal));
        put("RealProperties", amount(self.total_real));
        put("TotalGifts", amount(self.total_gifts));
        for index in 0..FORM_1800_DEDUCTIONS {
            let item = 28 + index;
            let row = self.deductions.get(index).cloned().unwrap_or_default();
            put(
                &format!("txtDeductionTitle{item}"),
                row.title.trim().to_string(),
            );
            put(&format!("txtDeductionAmount{item}"), amount(row.amount));
        }
        put("TotalDeductionsAllowed", amount(self.total_deductions));
        put("TotalReturnNetGifts", amount(self.net_gifts_this_return));
        put("TotalPriorNetGifts", amount(self.prior_net_gifts));
        put("TotalNetGifts36", amount(self.total_net_gifts));
        put(
            "TotalNetGiftsSubjectToTax",
            amount(self.net_gifts_subject_to_tax),
        );
        for index in 0..Self::table_rows(&self.personal_properties) {
            let a = self
                .personal_properties
                .get(index)
                .cloned()
                .unwrap_or_default();
            put(&format!("schedA:txtParticulars{index}"), up(&a.particulars));
            put(
                &format!("schedA:txtFairMarketValue{index}"),
                amount(a.fair_market_value),
            );
        }
        for index in 0..Self::table_rows(&self.real_properties) {
            let b = self.real_properties.get(index).cloned().unwrap_or_default();
            for (key, value) in [
                ("schedB1:txtOCT", &b.title_number),
                ("schedB1:txtTaxDecNo", &b.tax_declaration_number),
                ("schedB1:txtLocation", &b.location),
                ("schedB1:txtLotImprovement", &b.lot_or_improvement),
                ("schedB1:txtClassification", &b.classification),
                ("schedB2:txtArea", &b.area),
                ("schedB2:txtFTD", &b.fmv_per_tax_declaration),
                (
                    "schedB2:txtFMVperTDperBIRZonalValue",
                    &b.fmv_per_zonal_value,
                ),
            ] {
                put(&format!("{key}{index}"), up(value));
            }
            put(
                &format!("schedB2:txtFairMarketValue{index}"),
                amount(b.fair_market_value),
            );
        }
        put("schedA:txtTotalPayment1", amount(self.total_personal));
        put("schedB2:txtTotal", amount(self.total_real));
        put("txtLineBus", up(&self.line_of_business));
        put(
            "modLabel",
            "IS THE TERM OF DEBT INSTRUMENT LESS THAN A YEAR?".to_string(),
        );
        fields
    }

    /// Rows a schedule table writes: the page opens with five and the Add
    /// buttons append more.
    fn table_rows<T>(rows: &[T]) -> usize {
        rows.len().max(FORM_1800_SCHEDULE_ROWS)
    }

    /// The generated layout (five rows per table) with the rows the Add
    /// buttons append spliced in after the last row of their table.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        let base = crate::official_xml::layout(FORM_1800_FORM_ID).map_err(|e| e.to_string())?;
        let tables: [(usize, &[&str]); 3] = [
            (
                Self::table_rows(&self.personal_properties),
                &[
                    "chkscheduleADelete",
                    "frm1800v2018:schedA:txtParticulars",
                    "frm1800v2018:schedA:txtFairMarketValue",
                ],
            ),
            (
                Self::table_rows(&self.real_properties),
                &[
                    "chkScheduleB1Delete",
                    "frm1800v2018:schedB1:txtOCT",
                    "frm1800v2018:schedB1:txtTaxDecNo",
                    "frm1800v2018:schedB1:txtLocation",
                    "frm1800v2018:schedB1:txtLotImprovement",
                    "frm1800v2018:schedB1:txtClassification",
                ],
            ),
            (
                Self::table_rows(&self.real_properties),
                &[
                    "chkScheduleB2Delete",
                    "frm1800v2018:schedB2:txtArea",
                    "frm1800v2018:schedB2:txtFTD",
                    "frm1800v2018:schedB2:txtFMVperTDperBIRZonalValue",
                    "frm1800v2018:schedB2:txtFairMarketValue",
                ],
            ),
        ];
        let last = FORM_1800_SCHEDULE_ROWS - 1;
        let mut insertions = Vec::new();
        for (rows, columns) in tables {
            for index in FORM_1800_SCHEDULE_ROWS..rows {
                insertions.push(RowInsertion {
                    after: format!("{}{}", columns[columns.len() - 1], index - 1),
                    copies: columns
                        .iter()
                        .map(|column| (format!("{column}{last}"), format!("{column}{index}")))
                        .collect(),
                });
            }
        }
        extend_layout(base, &insertions)
    }

    /// The field map for the print preview: the upload's map, except the
    /// e-mail is printed as typed (the upload keeps it in capitals), plus
    /// `derived:tin_digits`, the nine TIN digits of the page 2 header.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        fields.insert(
            "frm1800v2018:txtEmail".to_string(),
            self.email.trim().to_string(),
        );
        // The page 2 header comb has nine inputs (the branch is preprinted).
        let mut tin_digits = super::print_map::tin_digits(&self.tin);
        tin_digits.truncate(9);
        fields.insert("derived:tin_digits".to_string(), tin_digits);
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `validate()` and `initialValidateBeforeSave()` against a given "today".
    pub fn validate_on(&self, today: NaiveDate) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        if let Some(spec) = &self.tax_relief {
            if spec.trim().is_empty() {
                err(
                    "tax_relief",
                    "Please specify the Special Treaty or International Law the payee is availing in item 11A.",
                );
            } else if spec.trim().chars().count() > 60 {
                err("tax_relief", "Item 13A holds at most 60 characters.");
            }
        }
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        if tin1.is_empty() || tin2.is_empty() || tin3.is_empty() || branch.is_empty() {
            err("tin", "Please enter a valid TIN number on Item 4.");
        } else if !tin_is_well_formed(&self.tin) {
            err("tin", "Please enter a valid TIN number on Item 5.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        let rdo = self.rdo_code.trim();
        if rdo.is_empty() || rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        // checkDate1.
        if self.donation_month == 0 || self.donation_day == 0 || self.donation_year == 0 {
            err("donation_date", "Please enter a valid date on Item 1.");
        } else if self.donation_year < 1000 {
            err("donation_date", "Please enter a valid Year(YYYY)");
        } else {
            match self.donation_date() {
                None => err("donation_date", "Please enter a valid date on Item 1."),
                Some(date) if date > today => err(
                    "donation_date",
                    "Date of donation should not be later than the current date.",
                ),
                Some(_) => {}
            }
        }
        if self.donor_name.trim().is_empty() {
            err(
                "donor_name",
                "Please enter a valid Taxpayer's Name on Item 8.",
            );
        }

        // Items 28-32 and net gifts within the P250,000 exemption are not
        // refused: the official page validates and files them (no alert in
        // `validateForm()`/`saveXML()`), with Item 33 left at 0.00 because
        // `compute33()` throws on controls the page lacks, and Item 38 not
        // floored, so tax due can be zero or negative. The desktop view asks
        // for confirmation first (`zero_tax_due_needs_confirm`).

        if self.donees.iter().all(|d| d.name.trim().is_empty()) {
            err("donees", "Enter at least one donee on Item 12.");
        }

        // Typing limits.
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 3 holds at most two digits.",
            );
        }
        if self.donees.len() > FORM_1800_DONEES {
            err("donees", "Item 12 holds donees A to E.");
        }
        for (index, donee) in self.donees.iter().enumerate() {
            let tin = donee.tin.trim();
            if donee.name.trim().chars().count() > 80 || tin.len() > 14 || !digits_only(tin) {
                err(
                    &format!("donees[{index}]"),
                    "Donee names hold 80 characters and TINs up to 14 digits.",
                );
            }
        }
        if self.deductions.len() > FORM_1800_DEDUCTIONS {
            err("deductions", "Items 28 to 32 hold five deductions.");
        }
        let mut amounts: Vec<(String, f64)> = vec![
            ("prior_net_gifts".into(), self.prior_net_gifts),
            ("prior_gift_payments".into(), self.prior_gift_payments),
            ("foreign_tax_paid".into(), self.foreign_tax_paid),
            ("tax_paid_previous".into(), self.tax_paid_previous),
            ("surcharge".into(), self.surcharge),
            ("interest".into(), self.interest),
            ("compromise".into(), self.compromise),
        ];
        for (index, row) in self.personal_properties.iter().enumerate() {
            amounts.push((
                format!("personal_properties[{index}].fair_market_value"),
                row.fair_market_value,
            ));
        }
        for (index, row) in self.real_properties.iter().enumerate() {
            amounts.push((
                format!("real_properties[{index}].fair_market_value"),
                row.fair_market_value,
            ));
        }
        for (field, value) in amounts {
            if value < 0.0 || !has_cent_precision(value) || !within_round_limit(value) {
                err(
                    &field,
                    "Enter a non-negative amount in pesos and centavos below 1,000,000,000,000.",
                );
            }
        }
        if !self.is_amended && self.tax_paid_previous != 0.0 {
            err(
                "tax_paid_previous",
                "Item 17C applies only to an amended return.",
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

impl FormValidator for Form1800Draft {
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1800Draft {
    /// Items 19A-19C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "19A Surcharge",
                interest: "19B Interest",
                compromise: "19C Compromise",
            ],
            controls: [
                "frm1800v2018:Surcharge",
                "frm1800v2018:Interest",
                "frm1800v2018:Compromise",
            ],
            state: penalty,
            basic_tax_due: "Item 18", |d| d.tax_payable,
            // Maintainer decision (1800/1801 penalties run from the due date)
            // (2026-10-19): the return is due within thirty
            // (30) days after the donation (html-frozen/1800-2018/guide.html,
            // "When and Where to File and Pay"), so the penalties run from
            // that day, moved to the next business day on the app's
            // deadline calendar.
            typed_due_date: Some(|d: &Self| d.due_date()),
        )
    }

    /// The day this return is due: 30 days after the donation date (Item 1),
    /// or the next business day on the app's deadline calendar when that is
    /// not one. `None` until Item 1 is a date.
    pub fn due_date(&self) -> Option<NaiveDate> {
        let due = self
            .donation_date()?
            .checked_add_days(chrono::Days::new(FORM_1800_DAYS_TO_FILE))?;
        Some(super::penalty_items::next_business_day(due))
    }
}

/// Days after the donation within which the return is filed
/// (html-frozen/1800-2018/guide.html: "within thirty (30) days after the date
/// the gift (donation) is made").
pub const FORM_1800_DAYS_TO_FILE: u64 = 30;

impl QueueableForm for Form1800Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1800";
    /// Official `formType` (`1800v2018`).
    const FORM_TYPE: &'static str = "1800v2018";
    const LAYOUT_ID: &'static str = FORM_1800_FORM_ID;

    /// Total Amount Payable.
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
    /// `MM + DD + YYYY` of the donation, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!(
            "{:02}{:02}{:04}",
            self.donation_month, self.donation_day, self.donation_year
        )
    }
    /// The filename names the donation date, not the dashboard's open-ended
    /// key, so a receipt cannot be mapped back to a draft row.
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
    /// The upload follows [`Form1800Draft::official_layout`].
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
        NaiveDate::from_ymd_opt(2026, 1, 31).unwrap()
    }

    pub(crate) fn sample() -> Form1800Draft {
        let mut d = Form1800Draft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            filing_year: 2025,
            open_ended_key: 1,
            donation_month: 3,
            donation_day: 10,
            donation_year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: "12345678800000".into(),
            rdo_code: "039".into(),
            donor_name: "Sample Donor".into(),
            registered_address: "1 Sample St".into(),
            zip_code: "1100".into(),
            residence_address: "1 Sample St".into(),
            residence_zip_code: "1100".into(),
            contact_number: "09170000000".into(),
            email: "sample.taxpayer@example.com".into(),
            line_of_business: "Consulting".into(),
            donees: vec![Form1800Donee {
                name: "Sample Donee".into(),
                tin: "111222333000".into(),
            }],
            tax_relief: None,
            personal_properties: vec![Form1800PersonalProperty {
                particulars: "Car".into(),
                fair_market_value: 500_000.005,
            }],
            real_properties: Vec::new(),
            deductions: Vec::new(),
            prior_net_gifts: 0.0,
            total_personal: 0.0,
            total_real: 0.0,
            total_gifts: 0.0,
            total_deductions: 0.0,
            net_gifts_this_return: 0.0,
            total_net_gifts: 0.0,
            net_gifts_subject_to_tax: 0.0,
            tax_due: 0.0,
            prior_gift_payments: 0.0,
            foreign_tax_paid: 0.0,
            tax_paid_previous: 0.0,
            total_credits: 0.0,
            tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        d.recompute();
        d
    }

    fn messages(d: &Form1800Draft) -> Vec<String> {
        d.validate_on(today()).into_iter().map(|(_, m)| m).collect()
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// A return on the maintainer decision's (1800/1801 penalties run from the
    /// due date) model (a new return), donated on `on`.
    fn new_return(on: NaiveDate) -> Form1800Draft {
        let mut d = sample();
        d.penalty.penalty_model_version = crate::forms::penalty_items::TYPED_DUE_DATE_MODEL_VERSION;
        d.donation_year = u16::try_from(chrono::Datelike::year(&on)).unwrap();
        d.donation_month = u8::try_from(chrono::Datelike::month(&on)).unwrap();
        d.donation_day = u8::try_from(chrono::Datelike::day(&on)).unwrap();
        d
    }

    /// maintainer decision (1800/1801 penalties run from the due date): due 30
    /// days after the donation, moved past a weekend to the
    /// next business day; no due date until Item 1 is a date.
    #[test]
    fn due_thirty_days_after_the_donation() {
        let due = |on| new_return(on).due_date();
        assert_eq!(due(date(2025, 3, 10)), Some(date(2025, 4, 9)));
        // Sunday 2025-03-30 and Saturday 2024-03-30 move to Monday.
        assert_eq!(due(date(2025, 2, 28)), Some(date(2025, 3, 31)));
        assert_eq!(due(date(2024, 2, 29)), Some(date(2024, 4, 1)));
        // An official holiday (maintainer decision: roll past official
        // holidays): 2026-04-09 is Araw
        // ng Kagitingan, one of the app's seeded holidays, so a donation on
        // 2026-03-10 is due Friday, April 10.
        assert_eq!(due(date(2026, 3, 10)), Some(date(2026, 4, 10)));
        // Maundy Thursday and Good Friday 2026 both move to Monday, April 6.
        assert_eq!(due(date(2026, 3, 3)), Some(date(2026, 4, 6)));
        assert_eq!(due(date(2026, 3, 4)), Some(date(2026, 4, 6)));
        // Official holidays are known only for the years whose proclamation
        // ships (2026 today); other years roll off weekends only until
        // theirs is added (maintainer decision: ship 2026, add later years):
        // Friday 2027-04-09 does not roll
        // yet.
        assert_eq!(due(date(2027, 3, 10)), Some(date(2027, 4, 9)));
        let mut blank = sample();
        (
            blank.donation_month,
            blank.donation_day,
            blank.donation_year,
        ) = (0, 0, 0);
        assert_eq!(blank.due_date(), None);
        let items = Form1800Draft::penalty_declaration();
        assert_eq!(
            items.due_date(&new_return(date(2025, 3, 10))),
            Some(date(2025, 4, 9))
        );
    }

    /// maintainer decision (1800/1801 penalties run from the due date) over a
    /// holiday: the 2026-03-10 donation, filed on 2026-04-10
    /// (the working day after Araw ng Kagitingan), is on time; a day later
    /// it is late.
    #[test]
    fn a_return_filed_the_day_after_a_holiday_due_date_is_on_time() {
        use crate::forms::penalty_items::with_clock;
        let mut on_time = new_return(date(2026, 3, 10));
        with_clock(date(2026, 4, 10), || on_time.recompute());
        assert_eq!(
            (on_time.surcharge, on_time.interest, on_time.compromise),
            (0.0, 0.0, 0.0)
        );
        let mut late = new_return(date(2026, 3, 10));
        with_clock(date(2026, 4, 13), || late.recompute());
        assert!(late.surcharge > 0.0);
    }

    /// maintainer decision (1800/1801 penalties run from the due date): a late
    /// return computes the 25% surcharge, 12% a year interest
    /// and the compromise on Item 18; one filed on its due date computes
    /// none; a return stored before maintainer decision (1800/1801 penalties
    /// run from the due date) computes none until it joins.
    #[test]
    fn penalties_run_from_the_due_date() {
        use crate::forms::penalty_items::{PENALTY_MODEL_VERSION, with_clock};
        use crate::penalties::PenaltyProfile;
        use crate::penalties::compromise::lookup_compromise;
        let on = date(2026, 1, 5);
        let due = date(2026, 2, 4);
        let filed = date(2026, 3, 6);
        let mut late = new_return(on);
        with_clock(filed, || late.recompute());
        let tax = late.tax_payable;
        assert_eq!(tax, 15_000.0);
        let days = (filed - due).num_days() as f64;
        let cents = |value: f64| (value * 100.0).round() / 100.0;
        assert_eq!(late.surcharge, cents(tax * 0.25));
        assert_eq!(late.interest, cents(tax * 0.12 * days / 365.0));
        assert_eq!(
            late.compromise,
            cents(lookup_compromise(tax, 0.0, PenaltyProfile::StandardFiling))
        );
        assert!(late.compromise > 0.0);
        assert_eq!(
            late.total_amount_payable,
            cents(tax + late.surcharge + late.interest + late.compromise)
        );
        let mut on_time = new_return(on);
        with_clock(due, || on_time.recompute());
        assert_eq!(
            (on_time.surcharge, on_time.interest, on_time.compromise),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(on_time.total_amount_payable, tax);
        let mut stored = new_return(on);
        stored.penalty.penalty_model_version = PENALTY_MODEL_VERSION;
        with_clock(filed, || stored.recompute());
        assert_eq!(
            (stored.surcharge, stored.interest, stored.compromise),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(stored.penalty.computed_penalties, None);
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let d = sample();
        assert_eq!(d.total_personal, 500_000.01);
        assert_eq!(d.net_gifts_subject_to_tax, 250_000.01);
        assert_eq!(d.tax_due, 15_000.0);
        assert_eq!(d.total_amount_payable, 15_000.0);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    /// Maintainer decision (two-decimal amounts of any size pass): Check takes
    /// every two-decimal amount below
    /// 1,000,000,000,000 in the money boxes, decided from the amount's
    /// decimal form; 8816764.47 and 1234567890.12 were refused before.
    #[test]
    fn check_takes_large_two_decimal_amounts() {
        const REFUSED: &str =
            "Enter a non-negative amount in pesos and centavos below 1,000,000,000,000.";
        for amount in [8_816_764.47, 1_234_567_890.12, 999_999_999_999.99] {
            let mut d = sample();
            d.prior_gift_payments = amount;
            d.foreign_tax_paid = amount;
            d.prior_net_gifts = amount;
            d.personal_properties[0].fair_market_value = amount;
            d.recompute();
            assert_eq!(d.prior_gift_payments, amount);
            let refused: Vec<_> = d
                .validate_on(today())
                .into_iter()
                .filter(|(_, message)| message == REFUSED)
                .collect();
            assert!(refused.is_empty(), "{amount}: {refused:?}");
        }
        for amount in [10.005, 1_000_000_000_000.0] {
            let mut d = sample();
            d.prior_gift_payments = amount;
            assert!(
                d.validate_on(today())
                    .iter()
                    .any(|(field, message)| field == "prior_gift_payments" && message == REFUSED),
                "{amount}"
            );
        }
    }

    #[test]
    fn deductions_and_exemption_are_filed_like_the_official_page() {
        let mut d = sample();
        assert!(!d.zero_tax_due_needs_confirm());
        d.deductions = vec![Form1800Deduction {
            title: "Dowry".into(),
            amount: 10_000.0,
        }];
        d.recompute();
        assert_eq!(d.total_deductions, 0.0);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
        let mut d = sample();
        d.personal_properties[0].fair_market_value = 100_000.0;
        d.recompute();
        assert_eq!(d.tax_due, -9_000.0);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
        assert!(d.zero_tax_due_needs_confirm());
    }

    #[test]
    fn field_map_and_filename() {
        let d = sample();
        let f = d.to_bir_field_map();
        assert_eq!(f["frm1800v2018:txtSheets"], "00");
        assert_eq!(f["frm1800v2018:txtDoneeNameA"], "SAMPLE DONEE");
        assert_eq!(f["frm1800v2018:txtDonorName"], "SAMPLE DONOR");
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1800v2018-03102025#sample.taxpayer@example.com#.xml"
        );
        assert_eq!(split_address(&"x".repeat(120)).1, "x".repeat(20));
    }

    #[test]
    fn print_map_shows_the_email_as_typed_and_the_upload_keeps_capitals() {
        let d = sample();
        let key = "frm1800v2018:txtEmail";
        assert_eq!(d.to_bir_field_map()[key], "SAMPLE.TAXPAYER@EXAMPLE.COM");
        assert_eq!(d.to_print_field_map()[key], "sample.taxpayer@example.com");
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1800Draft), expected: &str| {
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
            &|d| d.tax_relief = Some(String::new()),
            "Please specify the Special Treaty or International Law the payee is availing in item 11A.",
        );
        check(
            &|d| d.tin.clear(),
            "Please enter a valid TIN number on Item 4.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 5.",
        );
        check(
            &|d| d.donation_month = 0,
            "Please enter a valid date on Item 1.",
        );
        check(
            &|d| d.donation_year = 2026,
            "Date of donation should not be later than the current date.",
        );
        check(&|d| d.donation_year = 25, "Please enter a valid Year(YYYY)");
        check(
            &|d| d.donor_name.clear(),
            "Please enter a valid Taxpayer's Name on Item 8.",
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

    /// save-009 (BIR-Form1800v2018.hta saveXML L3222/L3249).
    mod save_009 {
        use super::*;
        use crate::db::{Database, DbError};
        use crate::filing_queue::QueueAuthSource;
        use crate::forms::queueable::{AMENDED_RETURN_ALERT, queue_failure_message};

        const ALERT: &str = "If your intention is to make an amended return, kindly tick Amended Return to 'Yes' before hitting 'Save' or 'Final Copy' or 'Submit'.";

        /// An original BIR already has, imported under a legacy slot.
        fn file_original(db: &Database, d: &Form1800Draft) {
            db.conn
                .execute(
                    "INSERT INTO form_drafts
                        (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                     VALUES (?1, '1800', ?2, 9001, ?3, 'Submitted', '{}')",
                    rusqlite::params![
                        d.tin,
                        i64::from(d.filing_year),
                        d.filing_period().to_period_key()
                    ],
                )
                .unwrap();
        }

        /// Save the draft, then try to queue it the way Submit does.
        fn submit(db: &Database, d: &Form1800Draft) -> Result<i64, DbError> {
            let mut d = d.clone();
            d.id = Some(db.save_queueable_draft(&d).unwrap());
            d.queue(QueueAuthSource::Gui).unwrap();
            db.save_queued_queueable(&d)
        }

        #[test]
        fn amended_no_with_a_filed_original_is_refused_with_the_exact_text() {
            let db = Database::open_in_memory_for_tests().unwrap();
            let d = sample();
            file_original(&db, &d);
            let error = submit(&db, &d).unwrap_err();
            assert!(matches!(error, DbError::AmendedReturnRequired));
            assert_eq!(AMENDED_RETURN_ALERT, ALERT);
            assert_eq!(error.to_string(), ALERT);
            assert_eq!(
                queue_failure_message("Form 1800", &error.to_string()),
                ALERT
            );
            let stored = db
                .get_queueable_draft::<Form1800Draft>(&d.tin, 2025, 1)
                .unwrap()
                .unwrap();
            assert!(stored.lifecycle.is_editable());
        }

        #[test]
        fn amended_no_without_an_original_is_allowed() {
            let db = Database::open_in_memory_for_tests().unwrap();
            assert!(submit(&db, &sample()).is_ok());
        }

        #[test]
        fn a_draft_or_queued_return_is_not_an_original() {
            let db = Database::open_in_memory_for_tests().unwrap();
            let d = sample();
            submit(&db, &d).unwrap();
            assert!(
                !db.has_filed_original(&d.tin, "1800", 2025, "O1", None)
                    .unwrap()
            );
        }

        #[test]
        fn amended_yes_with_a_filed_original_is_allowed() {
            let db = Database::open_in_memory_for_tests().unwrap();
            let mut d = sample();
            file_original(&db, &d);
            d.set_amended(true);
            assert!(submit(&db, &d).is_ok());
        }

        #[test]
        fn after_revert_as_amendment_yes_is_allowed_and_no_is_refused() {
            let db = Database::open_in_memory_for_tests().unwrap();
            let d = sample();
            let id = submit(&db, &d).unwrap();
            let mut filed = db
                .get_queueable_draft::<Form1800Draft>(&d.tin, 2025, 1)
                .unwrap()
                .unwrap();
            let filename = filed.submission_filename();
            filed.lifecycle.transition_to_submitted(filename);
            db.conn
                .execute(
                    "UPDATE form_drafts SET status = 'Submitted', data_json = ?1 WHERE id = ?2",
                    rusqlite::params![serde_json::to_string(&filed).unwrap(), id],
                )
                .unwrap();
            // The filed record itself is not its own original.
            assert!(!db.amended_return_required(&filed).unwrap());

            let reverted = db
                .revert_queueable_for_amendment::<Form1800Draft>(&d.tin, 2025, 1)
                .unwrap();
            assert!(reverted.snapshot_id.is_some());
            assert!(reverted.draft.is_amended);

            let mut flipped = reverted.draft.clone();
            flipped.set_amended(false);
            let error = submit(&db, &flipped).unwrap_err();
            assert_eq!(error.to_string(), ALERT);

            assert!(submit(&db, &reverted.draft).is_ok());
        }
    }
}
