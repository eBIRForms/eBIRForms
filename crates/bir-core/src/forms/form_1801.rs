//! BIR Form 1801 (January 2018) — Estate Tax Return.
//!
//! Ported from the official `BIR-Form1801v2018.hta` (eBIRForms 7.9.6.2.1):
//! the page's own `formatCurrency` (`toFixed(2)` with thousands commas) and
//! `NumWithComma`, the schedule totals (`get*_totals`), `calculate_Part4`,
//! `computeNo18` and `calculate_Part2`, `validateForm()` with its exact alert
//! texts, and the uploaded file (`saveEncryptedProfile`) through
//! [`crate::official_xml`]. Background
//! information comes from the taxpayer profile the way `loadBGData()` and
//! `sleeptime()` fill it.
//!
//! The page starts every schedule with two rows; this model covers exactly
//! those rows. Item 17's rate (6%) comes from `xml/taxRate.xml` through
//! `js/tax-rate-helper.js`; `init()` shows it as `6.0%`.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::official_inputs::{
    RowInsertion, cents, digits_only, extend_layout, group_thousands, has_cent_precision,
    split_tin, tin_is_well_formed, to_fixed_2, to_fixed_text, within_round_limit,
};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::official_amount;
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1801_FORM_ID: &str = "1801-v2018";
/// Rows the page creates for every Part V schedule.
pub const FORM_1801_SCHEDULE_ROWS: usize = 2;

/// Column keys of each Part V table, in page order, with `{}` for the row
/// number (`addRow_*` builds `<select id= frm1801v2018:sched1Class_N'`, so
/// the classification ids really end in an apostrophe).
const SCHEDULE_TABLES: [&[&str]; 7] = [
    &[
        "sched1Oct_{}",
        "sched1Td_{}",
        "sched1Loc_{}",
        "sched1Lot_{}",
        "sched1Area_{}",
        "sched1Class_{}'",
    ],
    &[
        "sched1Fmv_{}",
        "sched1Zonal_{}",
        "sched1Exc_{}",
        "sched1Conj_{}",
    ],
    &[
        "sched1AOct_{}",
        "sched1ATd_{}",
        "sched1ALoc_{}",
        "sched1AArea_{}",
        "sched1AClass_{}'",
        "sched1AFmv_{}",
        "sched1AZonal_{}",
        "sched1AExc_{}",
        "sched1AConj_{}",
    ],
    &[
        "sched2Corp_{}",
        "sched2Class_{}",
        "sched2Stock_{}",
        "sched2Shares_{}",
        "sched2FmvPerShare_{}",
        "sched2Exc_{}",
        "sched2Conj_{}",
    ],
    &["sched2AParticulars_{}", "sched2AExc_{}", "sched2AConj_{}"],
    &["sched3Particulars_{}", "sched3Exc_{}", "sched3Conj_{}"],
    &[
        "sched4Name_{}",
        "sched4Address_{}",
        "sched4Rdo_{}",
        "sched4Exc_{}",
        "sched4Conj_{}",
    ],
];
/// Item 17, percent: `xml/taxRate.xml` holds `0.06`, which `init()` shows as
/// `6.0%` and `computeNo18` reads back as 6.
pub const FORM_1801_TAX_RATE_PERCENT: f64 = 6.0;

/// Item 15D frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1801Frequency {
    Monthly,
    Quarterly,
    SemiAnnual,
    Others,
}

/// Exclusive / conjugal amounts of a schedule row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1801Split {
    pub exclusive: f64,
    pub conjugal: f64,
}

/// Schedule 1 (real property) and 1A (family home).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1801RealProperty {
    pub title_number: String,
    pub tax_declaration_number: String,
    pub location: String,
    /// Schedule 1 only.
    #[serde(default)]
    pub lot_or_improvement: String,
    pub area: String,
    /// `RR`, `CR`, `CL`, … or blank.
    pub classification: String,
    pub fmv_per_tax_declaration: String,
    pub fmv_per_zonal_value: String,
    pub value: Form1801Split,
}

/// Schedule 2 (shares of stock).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1801Shares {
    pub corporation: String,
    /// `Listed`, `Not Listed` or blank.
    pub listing: String,
    pub certificate_number: String,
    pub number_of_shares: String,
    pub value_per_share: String,
    pub value: Form1801Split,
}

/// Schedules 2A and 3.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1801Particular {
    pub particulars: String,
    pub value: Form1801Split,
}

/// Schedule 4 (business interest).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1801Business {
    pub name: String,
    pub address: String,
    pub rdo_code: String,
    pub value: Form1801Split,
}

/// Schedule 5 ordinary deductions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1801OrdinaryDeductions {
    pub claims_against_estate: Form1801Split,
    pub claims_against_insolvent: Form1801Split,
    pub unpaid_mortgages_taxes_losses: Form1801Split,
    pub settlement_losses: Form1801Split,
    pub vanishing_deduction: Form1801Split,
    pub transfer_for_public_use: Form1801Split,
    pub others_description: String,
    pub others: Form1801Split,
}

impl Form1801OrdinaryDeductions {
    fn rows(&self) -> [&Form1801Split; 7] {
        [
            &self.claims_against_estate,
            &self.claims_against_insolvent,
            &self.unpaid_mortgages_taxes_losses,
            &self.settlement_losses,
            &self.vanishing_deduction,
            &self.transfer_for_public_use,
            &self.others,
        ]
    }

    fn rows_mut(&mut self) -> [&mut Form1801Split; 7] {
        [
            &mut self.claims_against_estate,
            &mut self.claims_against_insolvent,
            &mut self.unpaid_mortgages_taxes_losses,
            &mut self.settlement_losses,
            &mut self.vanishing_deduction,
            &mut self.transfer_for_public_use,
            &mut self.others,
        ]
    }
}

/// Part IV column A / B / C.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1801Columns {
    pub exclusive: f64,
    pub conjugal: f64,
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1801Draft {
    #[serde(default)]
    pub id: Option<i64>,
    /// Dashboard year and open-ended key that identify this return.
    pub filing_year: u16,
    pub open_ended_key: u32,

    // Items 1–4
    pub death_month: u8,
    pub death_day: u8,
    pub death_year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
    pub tin: String,
    pub rdo_code: String,
    /// Item 7 (after the printed "ESTATE OF").
    pub estate_name: String,
    /// Item 8.
    pub residence_address: String,
    /// Item 9.
    pub non_resident_alien: Option<bool>,
    pub administrator_name: String,
    #[serde(default)]
    pub administrator_tin: String,
    #[serde(default)]
    pub administrator_branch_code: String,
    pub contact_number: String,
    pub email: String,
    /// Item 14; `Some` when "Yes", holding Item 14A.
    #[serde(default)]
    pub tax_relief: Option<String>,
    #[serde(default)]
    pub extension_to_file: bool,
    #[serde(default)]
    pub settled_judicially: bool,
    #[serde(default)]
    pub extension_to_pay: bool,
    #[serde(default)]
    pub installment_granted: bool,
    #[serde(default)]
    pub installment_frequency: Option<Form1801Frequency>,
    #[serde(default)]
    pub installment_frequency_other: String,

    // Part V
    #[serde(default)]
    pub real_properties: Vec<Form1801RealProperty>,
    #[serde(default)]
    pub family_homes: Vec<Form1801RealProperty>,
    #[serde(default)]
    pub shares: Vec<Form1801Shares>,
    #[serde(default)]
    pub other_personal: Vec<Form1801Particular>,
    #[serde(default)]
    pub taxable_transfers: Vec<Form1801Particular>,
    #[serde(default)]
    pub business_interests: Vec<Form1801Business>,
    #[serde(default)]
    pub ordinary_deductions: Form1801OrdinaryDeductions,

    // Part IV
    #[serde(default)]
    pub standard_deduction: f64,
    #[serde(default)]
    pub family_home_deduction: f64,
    #[serde(default)]
    pub other_special_deduction_description: String,
    #[serde(default)]
    pub other_special_deduction: f64,
    #[serde(default)]
    pub schedule_totals: BTreeMap<String, Form1801Split>,
    #[serde(default)]
    pub real_property: Form1801Columns,
    #[serde(default)]
    pub family_home: Form1801Columns,
    #[serde(default)]
    pub personal_property: Form1801Columns,
    #[serde(default)]
    pub taxable_transfer: Form1801Columns,
    #[serde(default)]
    pub business_interest: Form1801Columns,
    #[serde(default)]
    pub gross_estate: Form1801Columns,
    #[serde(default)]
    pub ordinary_deduction: Form1801Columns,
    #[serde(default)]
    pub estate_after_deductions: Form1801Columns,
    #[serde(default)]
    pub total_special_deductions: f64,
    #[serde(default)]
    pub net_estate: f64,
    #[serde(default)]
    pub spouse_share: f64,
    /// Items 40 and 16.
    #[serde(default)]
    pub net_taxable_estate: f64,

    // Part II
    #[serde(default)]
    pub foreign_estate_tax: f64,
    #[serde(default)]
    pub tax_paid_previous: f64,
    #[serde(default)]
    pub total_credits: f64,
    /// Item 21 year, as typed.
    #[serde(default)]
    pub installment_year: String,
    #[serde(default)]
    pub installment_portion: f64,
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

    /// Details of Payment, as typed (`cashBank` ... `othersAmt`).
    #[serde(default)]
    pub payment: super::payment_cells::PaymentCells,
    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// The page's `formatCurrency`: `toFixed(2)` with thousands commas.
fn fc_text(value: f64) -> String {
    group_thousands(&to_fixed_text(value, 2))
}

/// The value `NumWithComma(formatCurrency(x))` reads back.
fn fc(value: f64) -> f64 {
    if value.is_nan() {
        f64::NAN
    } else {
        to_fixed_2(value)
    }
}

fn sum_split<'a>(rows: impl Iterator<Item = &'a Form1801Split>) -> Form1801Split {
    let (mut exclusive, mut conjugal) = (0.0, 0.0);
    for row in rows {
        exclusive += row.exclusive;
        conjugal += row.conjugal;
    }
    Form1801Split {
        exclusive: fc(exclusive),
        conjugal: fc(conjugal),
    }
}

fn columns(exclusive: f64, conjugal: f64) -> Form1801Columns {
    Form1801Columns {
        exclusive,
        conjugal,
        total: fc(exclusive + conjugal),
    }
}

fn round_split(split: &mut Form1801Split) {
    split.exclusive = cents(split.exclusive).max(0.0);
    split.conjugal = cents(split.conjugal).max(0.0);
}

impl Form1801Draft {
    pub const FORM_CODE: &'static str = "1801";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, open_ended_key: u32) -> Self {
        let mut draft = Self {
            // Maintainer decision (1800/1801 penalties run from the due date):
            // a new return's penalties run from its due date,
            // one year after the date of death it carries (Item 1).
            penalty: crate::forms::penalty_items::PenaltyState::for_profile_typed_due_date(profile),
            id: None,
            filing_year: year,
            open_ended_key,
            // Item 1 starts blank like the official txtDateMonth/Day/Year.
            death_month: 0,
            death_day: 0,
            death_year: 0,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            estate_name: profile.full_name.clone(),
            residence_address: profile.registered_address.clone(),
            non_resident_alien: None,
            administrator_name: String::new(),
            administrator_tin: String::new(),
            administrator_branch_code: String::new(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            tax_relief: None,
            extension_to_file: false,
            settled_judicially: false,
            extension_to_pay: false,
            installment_granted: false,
            installment_frequency: None,
            installment_frequency_other: String::new(),
            real_properties: Vec::new(),
            family_homes: Vec::new(),
            shares: Vec::new(),
            other_personal: Vec::new(),
            taxable_transfers: Vec::new(),
            business_interests: Vec::new(),
            ordinary_deductions: Form1801OrdinaryDeductions::default(),
            standard_deduction: 0.0,
            family_home_deduction: 0.0,
            other_special_deduction_description: String::new(),
            other_special_deduction: 0.0,
            schedule_totals: BTreeMap::new(),
            real_property: Form1801Columns::default(),
            family_home: Form1801Columns::default(),
            personal_property: Form1801Columns::default(),
            taxable_transfer: Form1801Columns::default(),
            business_interest: Form1801Columns::default(),
            gross_estate: Form1801Columns::default(),
            ordinary_deduction: Form1801Columns::default(),
            estate_after_deductions: Form1801Columns::default(),
            total_special_deductions: 0.0,
            net_estate: 0.0,
            spouse_share: 0.0,
            net_taxable_estate: 0.0,
            foreign_estate_tax: 0.0,
            tax_paid_previous: 0.0,
            total_credits: 0.0,
            installment_year: String::new(),
            installment_portion: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            payment: Default::default(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 2 (`setCredits_TaxPrev`): "No" leaves 19B disabled; this model
    /// also clears it.
    pub fn set_amended(&mut self, amended: bool) {
        self.is_amended = amended;
        if !amended {
            self.tax_paid_previous = 0.0;
        }
        self.recompute();
    }

    /// Item 15D (`installmentGranted`).
    pub fn set_installment_granted(&mut self, granted: bool) {
        self.installment_granted = granted;
        if !granted {
            self.installment_frequency = None;
            self.installment_frequency_other.clear();
        }
        self.recompute();
    }

    /// The official compute chain, run as if every changed field was blurred.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_paid_previous = 0.0;
        }
        for row in self
            .real_properties
            .iter_mut()
            .chain(self.family_homes.iter_mut())
        {
            round_split(&mut row.value);
        }
        for row in &mut self.shares {
            round_split(&mut row.value);
        }
        for row in self
            .other_personal
            .iter_mut()
            .chain(self.taxable_transfers.iter_mut())
        {
            round_split(&mut row.value);
        }
        for row in &mut self.business_interests {
            round_split(&mut row.value);
        }
        for row in self.ordinary_deductions.rows_mut() {
            round_split(row);
        }
        let s1 = sum_split(self.real_properties.iter().map(|r| &r.value));
        let s1a = sum_split(self.family_homes.iter().map(|r| &r.value));
        let s2 = sum_split(self.shares.iter().map(|r| &r.value));
        let s2a = sum_split(self.other_personal.iter().map(|r| &r.value));
        let s3 = sum_split(self.taxable_transfers.iter().map(|r| &r.value));
        let s4 = sum_split(self.business_interests.iter().map(|r| &r.value));
        let s5 = sum_split(self.ordinary_deductions.rows().into_iter());

        self.real_property = columns(s1.exclusive, s1.conjugal);
        self.family_home = columns(s1a.exclusive, s1a.conjugal);
        let personal_a = fc(s2.exclusive + s2a.exclusive);
        let personal_b = fc(s2.conjugal + s2a.conjugal);
        self.personal_property = columns(personal_a, personal_b);
        self.taxable_transfer = columns(s3.exclusive, s3.conjugal);
        self.business_interest = columns(s4.exclusive, s4.conjugal);
        let gross_a = fc(self.real_property.exclusive
            + self.family_home.exclusive
            + self.personal_property.exclusive
            + self.taxable_transfer.exclusive
            + self.business_interest.exclusive);
        let gross_b = fc(self.real_property.conjugal
            + self.family_home.conjugal
            + self.personal_property.conjugal
            + self.taxable_transfer.conjugal
            + self.business_interest.conjugal);
        self.gross_estate = columns(gross_a, gross_b);
        self.ordinary_deduction = columns(s5.exclusive, s5.conjugal);
        let after_a = fc(self.gross_estate.exclusive - self.ordinary_deduction.exclusive);
        let after_b = fc(self.gross_estate.conjugal - self.ordinary_deduction.conjugal);
        self.estate_after_deductions = columns(after_a, after_b);

        self.standard_deduction = cents(self.standard_deduction).max(0.0);
        self.family_home_deduction = cents(self.family_home_deduction).max(0.0);
        self.other_special_deduction = cents(self.other_special_deduction).max(0.0);
        self.total_special_deductions =
            fc(self.standard_deduction + self.family_home_deduction + self.other_special_deduction);
        self.net_estate = fc(self.estate_after_deductions.total - self.total_special_deductions);
        self.spouse_share = fc(self.estate_after_deductions.conjugal / 2.0);
        self.net_taxable_estate = fc(self.net_estate - self.spouse_share);

        self.schedule_totals = [
            ("sched1", s1),
            ("sched1A", s1a),
            ("sched2", s2),
            ("sched2A", s2a),
            ("sched3", s3),
            ("sched4", s4),
            ("sched5", s5),
        ]
        .into_iter()
        .map(|(name, split)| (name.to_string(), split))
        .collect();

        self.foreign_estate_tax = cents(self.foreign_estate_tax).max(0.0);
        self.tax_paid_previous = cents(self.tax_paid_previous).max(0.0);
        self.installment_portion = cents(self.installment_portion).max(0.0);
        self.total_credits = fc(self.foreign_estate_tax + self.tax_paid_previous);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge).max(0.0);
        self.interest = cents(self.interest).max(0.0);
        self.compromise = cents(self.compromise).max(0.0);
        self.total_penalties = fc(self.surcharge + self.interest + self.compromise);
    }

    /// Item 18 (`computeNo18`): Item 16 × Item 17 for a positive estate,
    /// `0.00` otherwise.
    pub fn estate_tax_due(&self) -> f64 {
        if self.net_taxable_estate > 0.0 {
            fc(self.net_taxable_estate * (FORM_1801_TAX_RATE_PERCENT / 100.0))
        } else {
            0.0
        }
    }

    /// Items 20, 22 and 24 (`calculate_Part2`).
    pub fn payables(&self) -> (f64, f64, f64) {
        let payable = fc(self.estate_tax_due() - self.total_credits);
        let first = fc(payable - self.installment_portion);
        let first_or_zero = if first.is_nan() { 0.0 } else { first };
        let total = if first_or_zero < 0.0 && self.total_penalties > 0.0 {
            fc(self.total_penalties)
        } else {
            fc(first_or_zero + self.total_penalties)
        };
        (payable, first, total)
    }

    fn death_date(&self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(
            i32::from(self.death_year),
            u32::from(self.death_month),
            u32::from(self.death_day),
        )
    }

    fn row<T: Clone + Default>(rows: &[T], index: usize) -> T {
        rows.get(index).cloned().unwrap_or_default()
    }

    /// The official control values the upload loop writes, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1801v2018:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        // `capital(this, event)` resolves to the argument-less `capital()` in
        // js/string-util.js and uppercases every text control (e-mail too);
        // a filer always passes one (Item 10).
        let up = |value: &str| value.trim().to_uppercase();
        let amount = fc_text;
        let typed = official_amount;

        put("txtDateMonth", format!("{:02}", self.death_month));
        put("txtDateDay", format!("{:02}", self.death_day));
        put("txtDateYear", format!("{:02}", self.death_year));
        put("amendedRtn_1", flag(self.is_amended));
        put("amendedRtn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());
        put("txtAtc", "ES 010".to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        for suffix in ["_1", "_2"] {
            put(&format!("tinA{suffix}"), tin1.clone());
            put(&format!("tinB{suffix}"), tin2.clone());
            put(&format!("tinC{suffix}"), tin3.clone());
            put(&format!("branchCode{suffix}"), branch.clone());
            put(&format!("registeredName{suffix}"), up(&self.estate_name));
        }
        put(
            "txtRDOCode_1",
            if self.rdo_code.trim().is_empty() {
                "000".to_string()
            } else {
                self.rdo_code.trim().to_string()
            },
        );
        put("registeredAddress_1", up(&self.residence_address));
        put(
            "categoryNonResident_1",
            flag(self.non_resident_alien == Some(true)),
        );
        put(
            "categoryNonResident_2",
            flag(self.non_resident_alien == Some(false)),
        );
        put("adminName", up(&self.administrator_name));
        let (e1, e2, e3, _) = split_tin(&self.administrator_tin);
        put("txtTINE1", e1);
        put("txtTINE2", e2);
        put("txtTINE3", e3);
        put("txtBranchCodeE", up(&self.administrator_branch_code));
        put("telephoneNumber", self.contact_number.trim().to_string());
        put("txtEmail", up(&self.email));
        put("optTreaty_1", flag(self.tax_relief.is_some()));
        put("optTreaty_2", flag(self.tax_relief.is_none()));
        put(
            "treatyY",
            self.tax_relief.as_deref().map(up).unwrap_or_default(),
        );
        for (id, on) in [
            ("fileGranted", self.extension_to_file),
            ("instGranted", self.installment_granted),
            ("settled", self.settled_judicially),
            ("extGranted", self.extension_to_pay),
        ] {
            put(&format!("{id}_1"), flag(on));
            put(&format!("{id}_2"), flag(!on));
        }
        let freq = self.installment_frequency;
        put("freq_Mon", flag(freq == Some(Form1801Frequency::Monthly)));
        put("freq_Qtr", flag(freq == Some(Form1801Frequency::Quarterly)));
        put(
            "freq_Semi",
            flag(freq == Some(Form1801Frequency::SemiAnnual)),
        );
        put("freq_Other", flag(freq == Some(Form1801Frequency::Others)));
        put(
            "freqOtherInput",
            if freq == Some(Form1801Frequency::Others) {
                up(&self.installment_frequency_other)
            } else {
                String::new()
            },
        );

        let (payable, first, total) = self.payables();
        put("txtTaxableEstate", amount(self.net_taxable_estate));
        put("txtTaxRate", "6.0%".to_string());
        put("txtEstTaxDue", amount(self.estate_tax_due()));
        put("txtCredits_ForeignEst", typed(self.foreign_estate_tax));
        put("txtCredits_TaxPrev", typed(self.tax_paid_previous));
        put("txtCredits_Tot", amount(self.total_credits));
        put("txtTaxPayable", amount(payable));
        put(
            "txtInstallmentYear",
            self.installment_year.trim().to_string(),
        );
        put("txtInstallment", typed(self.installment_portion));
        put("txtTaxPayable_1st", amount(first));
        put("txtPen_Surcharge", typed(self.surcharge));
        put("txtPen_Interest", typed(self.interest));
        put("txtPen_Compromise", typed(self.compromise));
        put("txtPen_Tot", amount(self.total_penalties));
        put("txtTotPayable", amount(total));

        for (name, cols) in [
            ("RealProp", self.real_property),
            ("Fam", self.family_home),
            ("PersonalProp", self.personal_property),
            ("Taxable", self.taxable_transfer),
            ("Business", self.business_interest),
            ("GrossEst", self.gross_estate),
            ("OrdDed", self.ordinary_deduction),
            ("EstAfterDed", self.estate_after_deductions),
        ] {
            put(&format!("txt{name}_A"), amount(cols.exclusive));
            put(&format!("txt{name}_B"), amount(cols.conjugal));
            put(&format!("txt{name}_C"), amount(cols.total));
        }
        put("txtStandardDed_C", typed(self.standard_deduction));
        put("txtFamDed_C", typed(self.family_home_deduction));
        put(
            "txtOtherDed_Specify",
            up(&self.other_special_deduction_description),
        );
        put("txtOtherDed_C", typed(self.other_special_deduction));
        put("txtTotalDed_C", amount(self.total_special_deductions));
        put("txtNetEst_C", amount(self.net_estate));
        put("txtShareSpouse_C", amount(self.spouse_share));
        put("txtNetTaxEst_C", amount(self.net_taxable_estate));

        let rows = self.table_rows();
        let most = rows
            .iter()
            .copied()
            .max()
            .unwrap_or(FORM_1801_SCHEDULE_ROWS);
        for index in 0..most {
            let n = index + 1;
            let mut put = |table: usize, key: &str, value: String| {
                if index < rows[table] {
                    put(key, value);
                }
            };
            let r = Self::row(&self.real_properties, index);
            put(0, &format!("sched1Oct_{n}"), up(&r.title_number));
            put(0, &format!("sched1Td_{n}"), up(&r.tax_declaration_number));
            put(0, &format!("sched1Loc_{n}"), up(&r.location));
            put(0, &format!("sched1Lot_{n}"), up(&r.lot_or_improvement));
            put(0, &format!("sched1Area_{n}"), up(&r.area));
            put(
                0,
                &format!("sched1Class_{n}'"),
                r.classification.trim().to_string(),
            );
            put(1, &format!("sched1Fmv_{n}"), up(&r.fmv_per_tax_declaration));
            put(1, &format!("sched1Zonal_{n}"), up(&r.fmv_per_zonal_value));
            put(1, &format!("sched1Exc_{n}"), typed(r.value.exclusive));
            put(1, &format!("sched1Conj_{n}"), typed(r.value.conjugal));
            let f = Self::row(&self.family_homes, index);
            put(2, &format!("sched1AOct_{n}"), up(&f.title_number));
            put(2, &format!("sched1ATd_{n}"), up(&f.tax_declaration_number));
            put(2, &format!("sched1ALoc_{n}"), up(&f.location));
            put(2, &format!("sched1AArea_{n}"), up(&f.area));
            put(
                2,
                &format!("sched1AClass_{n}'"),
                f.classification.trim().to_string(),
            );
            put(
                2,
                &format!("sched1AFmv_{n}"),
                up(&f.fmv_per_tax_declaration),
            );
            put(2, &format!("sched1AZonal_{n}"), up(&f.fmv_per_zonal_value));
            put(2, &format!("sched1AExc_{n}"), typed(f.value.exclusive));
            put(2, &format!("sched1AConj_{n}"), typed(f.value.conjugal));
            let s = Self::row(&self.shares, index);
            put(3, &format!("sched2Corp_{n}"), up(&s.corporation));
            put(3, &format!("sched2Class_{n}"), s.listing.trim().to_string());
            put(3, &format!("sched2Stock_{n}"), up(&s.certificate_number));
            put(3, &format!("sched2Shares_{n}"), up(&s.number_of_shares));
            put(3, &format!("sched2FmvPerShare_{n}"), up(&s.value_per_share));
            put(3, &format!("sched2Exc_{n}"), typed(s.value.exclusive));
            put(3, &format!("sched2Conj_{n}"), typed(s.value.conjugal));
            let o = Self::row(&self.other_personal, index);
            put(4, &format!("sched2AParticulars_{n}"), up(&o.particulars));
            put(4, &format!("sched2AExc_{n}"), typed(o.value.exclusive));
            put(4, &format!("sched2AConj_{n}"), typed(o.value.conjugal));
            let t = Self::row(&self.taxable_transfers, index);
            put(5, &format!("sched3Particulars_{n}"), up(&t.particulars));
            put(5, &format!("sched3Exc_{n}"), typed(t.value.exclusive));
            put(5, &format!("sched3Conj_{n}"), typed(t.value.conjugal));
            let b = Self::row(&self.business_interests, index);
            put(6, &format!("sched4Name_{n}"), up(&b.name));
            put(6, &format!("sched4Address_{n}"), up(&b.address));
            put(
                6,
                &format!("sched4Rdo_{n}"),
                if b.rdo_code.trim().is_empty() {
                    "000".to_string()
                } else {
                    b.rdo_code.trim().to_string()
                },
            );
            put(6, &format!("sched4Exc_{n}"), typed(b.value.exclusive));
            put(6, &format!("sched4Conj_{n}"), typed(b.value.conjugal));
        }
        for name in [
            "sched1", "sched1A", "sched2", "sched2A", "sched3", "sched4", "sched5",
        ] {
            let split = self.schedule_totals.get(name).cloned().unwrap_or_default();
            put(&format!("{name}Exc_Total"), amount(split.exclusive));
            put(&format!("{name}Conj_Total"), amount(split.conjugal));
        }
        let ded = &self.ordinary_deductions;
        for (name, split) in [
            ("Estate", &ded.claims_against_estate),
            ("Insolvent", &ded.claims_against_insolvent),
            ("Unpaid", &ded.unpaid_mortgages_taxes_losses),
            ("Losses", &ded.settlement_losses),
            ("Vanishing", &ded.vanishing_deduction),
            ("Transfer", &ded.transfer_for_public_use),
            ("Others", &ded.others),
        ] {
            put(&format!("sched5{name}Exc"), typed(split.exclusive));
            put(&format!("sched5{name}Conj"), typed(split.conjugal));
        }
        put("sched5Others_Specify", up(&ded.others_description));
        fields.extend(self.payment.controls("frm1801v2018", "frm1801v2018taxkNum"));
        fields
    }

    /// Rows each Part V table writes: the page opens with two and "Add row"
    /// appends more.
    fn table_rows(&self) -> [usize; 7] {
        let rows = |n: usize| n.max(FORM_1801_SCHEDULE_ROWS);
        let real = rows(self.real_properties.len());
        [
            real,
            real,
            rows(self.family_homes.len()),
            rows(self.shares.len()),
            rows(self.other_personal.len()),
            rows(self.taxable_transfers.len()),
            rows(self.business_interests.len()),
        ]
    }

    /// The generated layout (two rows per table) with the rows "Add row"
    /// appends spliced in after the last row of their table.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        let base = crate::official_xml::layout(FORM_1801_FORM_ID).map_err(|e| e.to_string())?;
        let rows = self.table_rows();
        let mut insertions = Vec::new();
        for (table, columns) in SCHEDULE_TABLES.iter().enumerate() {
            for n in (FORM_1801_SCHEDULE_ROWS + 1)..=rows[table] {
                let key = |column: &str, row: usize| {
                    format!("frm1801v2018:{}", column.replace("{}", &row.to_string()))
                };
                insertions.push(RowInsertion {
                    after: key(columns[columns.len() - 1], n - 1),
                    copies: columns
                        .iter()
                        .map(|column| (key(column, FORM_1801_SCHEDULE_ROWS), key(column, n)))
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
            "frm1801v2018:txtEmail".to_string(),
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

    /// `validateForm()` against a given "today".
    pub fn validate_on(&self, today: NaiveDate) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // Item 1.
        let leap =
            |y: u16| (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
        let max_day = match self.death_month {
            2 => {
                if leap(self.death_year) {
                    29
                } else {
                    28
                }
            }
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if self.death_month == 0 && self.death_day == 0 && self.death_year == 0 {
            err("death_date", "Please enter a valid Return Date");
        } else if !(1..=12).contains(&self.death_month) {
            err("death_date", "Please enter a valid month on Item 1.");
        } else if max_day == 28 && self.death_day > 28 {
            err(
                "death_date",
                "Please enter a valid date on Item 1. Filing year is not a leap year.",
            );
        } else if self.death_day < 1 || self.death_day > max_day {
            err("death_date", "Please enter a valid day on Item 1.");
        } else if self.death_year == 0 {
            err("death_date", "Please enter a valid year on Item 1.");
        } else if self.death_year < 1904 {
            err(
                "death_date",
                "Invalid date entry on Item 1. Entry should not be lower than 1904.",
            );
        } else if self.death_date().is_some_and(|d| d > today) {
            err(
                "death_date",
                "Invalid date entry on Item 1. Date cannot be after the current date.",
            );
        }
        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        if tin1.is_empty() || tin2.is_empty() || tin3.is_empty() || !tin_is_well_formed(&self.tin) {
            err("tin", "Please enter a valid TIN number on Item 5.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        let rdo = self.rdo_code.trim();
        if rdo.is_empty() || rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err("rdo_code", "Please enter a valid RDO Code on Item 6.");
        }
        if self.estate_name.trim().is_empty() {
            err(
                "estate_name",
                "Please enter a valid Taxpayer's Name on Item 7.",
            );
        }
        if self.residence_address.trim().is_empty() {
            err(
                "residence_address",
                "Please enter Taxpayer's Resident of Decedent on Item 8.",
            );
        }
        if self.non_resident_alien.is_none() {
            err("non_resident_alien", "Please select an option for Item 9.");
        }
        if self.contact_number.trim().is_empty() {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 12.",
            );
        }
        if self.installment_granted {
            match self.installment_frequency {
                None => err(
                    "installment_frequency",
                    "Please select a frequency of payment on Item 15D.",
                ),
                Some(Form1801Frequency::Others)
                    if self.installment_frequency_other.trim().is_empty() =>
                {
                    err(
                        "installment_frequency_other",
                        "Please specify a frequency of payment on Item 15D - Others.",
                    )
                }
                Some(_) => {}
            }
        }
        if self.non_resident_alien == Some(true) && self.standard_deduction > 500_000.0 {
            err(
                "standard_deduction",
                "If Yes is selected on Item 9, value should not exceed Php500 Thousand on Item 37A.",
            );
        } else if self.non_resident_alien == Some(false) && self.standard_deduction > 5_000_000.0 {
            err(
                "standard_deduction",
                "If No is selected on Item 9, value should not exceed Php5 Million on Item 37A.",
            );
        }
        if self.family_home_deduction > 10_000_000.0 {
            err(
                "family_home_deduction",
                "Value should not exceed Php10 Million on Item 37B.",
            );
        }

        // Schedule rows: every filled cell (amounts other than 0.00) or none.
        let complete = |cells: &[String]| {
            let filled: Vec<bool> = cells
                .iter()
                .filter(|v| v.as_str() != "0.00")
                .map(|v| !v.trim().is_empty())
                .collect();
            filled.iter().all(|f| *f) || filled.iter().all(|f| !*f)
        };
        let money = |v: f64| official_amount(v);
        for (index, r) in self.real_properties.iter().enumerate() {
            let first = [
                r.title_number.clone(),
                r.tax_declaration_number.clone(),
                r.location.clone(),
                r.lot_or_improvement.clone(),
                r.area.clone(),
                r.classification.clone(),
            ];
            let mut all = first.to_vec();
            if !complete(&first) {
                err(
                    &format!("real_properties[{index}]"),
                    &format!("Incomplete values on Schedule 1, Row {}.", index + 1),
                );
                continue;
            }
            all.extend([
                r.fmv_per_tax_declaration.clone(),
                r.fmv_per_zonal_value.clone(),
                money(r.value.exclusive),
                money(r.value.conjugal),
            ]);
            if !complete(&all) {
                err(
                    &format!("real_properties[{index}]"),
                    &format!(
                        "Incomplete values on Schedule 1 (Continuation), Row {}.",
                        index + 1
                    ),
                );
            }
        }
        let checks: Vec<(&str, &str, Vec<Vec<String>>)> = vec![
            (
                "family_homes",
                "Schedule 1A",
                self.family_homes
                    .iter()
                    .map(|r| {
                        vec![
                            r.title_number.clone(),
                            r.tax_declaration_number.clone(),
                            r.location.clone(),
                            r.area.clone(),
                            r.classification.clone(),
                            r.fmv_per_tax_declaration.clone(),
                            r.fmv_per_zonal_value.clone(),
                            money(r.value.exclusive),
                            money(r.value.conjugal),
                        ]
                    })
                    .collect(),
            ),
            (
                "shares",
                "Schedule 2",
                self.shares
                    .iter()
                    .map(|r| {
                        vec![
                            r.corporation.clone(),
                            r.listing.clone(),
                            r.certificate_number.clone(),
                            r.number_of_shares.clone(),
                            r.value_per_share.clone(),
                            money(r.value.exclusive),
                            money(r.value.conjugal),
                        ]
                    })
                    .collect(),
            ),
            (
                "other_personal",
                "Schedule 2A",
                self.other_personal
                    .iter()
                    .map(|r| {
                        vec![
                            r.particulars.clone(),
                            money(r.value.exclusive),
                            money(r.value.conjugal),
                        ]
                    })
                    .collect(),
            ),
            (
                "taxable_transfers",
                "Schedule 3",
                self.taxable_transfers
                    .iter()
                    .map(|r| {
                        vec![
                            r.particulars.clone(),
                            money(r.value.exclusive),
                            money(r.value.conjugal),
                        ]
                    })
                    .collect(),
            ),
            (
                "business_interests",
                "Schedule 4",
                self.business_interests
                    .iter()
                    .map(|r| {
                        let rdo = r.rdo_code.trim();
                        vec![
                            r.name.clone(),
                            r.address.clone(),
                            if rdo == "000" {
                                String::new()
                            } else {
                                rdo.to_string()
                            },
                            money(r.value.exclusive),
                            money(r.value.conjugal),
                        ]
                    })
                    .collect(),
            ),
        ];
        for (field, schedule, rows) in checks {
            for (index, cells) in rows.iter().enumerate() {
                if !complete(cells) {
                    err(
                        &format!("{field}[{index}]"),
                        &format!("Incomplete values on {schedule}, Row {}.", index + 1),
                    );
                }
            }
        }

        // A capital() blur happens on Item 10; the uppercase rule relies on it.
        if self.administrator_name.trim().is_empty() {
            err(
                "administrator_name",
                "Enter the executor / administrator on Item 10.",
            );
        }
        if let Some(spec) = &self.tax_relief
            && spec.trim().is_empty()
        {
            err("tax_relief", "Specify the tax relief on Item 14A.");
        }

        // Typing limits. The Part V tables have no row limit (`addRow_*`).
        for value in self
            .real_properties
            .iter()
            .chain(self.family_homes.iter())
            .map(|r| r.classification.trim())
        {
            if !value.is_empty()
                && ![
                    "RR", "CR", "CL", "GL", "A", "X", "RC", "CC", "PS", "GP", "I", "APD",
                ]
                .contains(&value)
            {
                err(
                    "classification",
                    "Pick a classification from the official list.",
                );
            }
        }
        for value in self.shares.iter().map(|r| r.listing.trim()) {
            if !value.is_empty() && value != "Listed" && value != "Not Listed" {
                err(
                    "shares",
                    "Schedule 2 classification is Listed or Not Listed.",
                );
            }
        }
        let admin_digits: String = self
            .administrator_tin
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        if !self.administrator_tin.trim().is_empty() && admin_digits.len() != 9 {
            err("administrator_tin", "Item 11 TIN has nine digits.");
        }
        if !self.installment_year.trim().is_empty() && !digits_only(self.installment_year.trim()) {
            err("installment_year", "Item 21 year holds digits only.");
        }
        let mut amounts = vec![
            self.foreign_estate_tax,
            self.tax_paid_previous,
            self.installment_portion,
            self.surcharge,
            self.interest,
            self.compromise,
            self.standard_deduction,
            self.family_home_deduction,
            self.other_special_deduction,
        ];
        for row in self.ordinary_deductions.rows() {
            amounts.extend([row.exclusive, row.conjugal]);
        }
        if amounts
            .iter()
            .any(|v| *v < 0.0 || !has_cent_precision(*v) || !within_round_limit(*v))
        {
            err(
                "amounts",
                "Enter non-negative amounts in pesos and centavos below 1,000,000,000,000.",
            );
        }
        if !self.is_amended && self.tax_paid_previous != 0.0 {
            err(
                "tax_paid_previous",
                "Item 19B applies only to an amended return.",
            );
        }
        let email = self.email.trim();
        if email.is_empty()
            || !email.contains('@')
            || email.contains(char::is_whitespace)
            || email.contains('#')
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

impl FormValidator for Form1801Draft {
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1801Draft {
    /// Items 23A-23C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "23A Surcharge",
                interest: "23B Interest",
                compromise: "23C Compromise",
            ],
            controls: [
                "frm1801v2018:txtPen_Surcharge",
                "frm1801v2018:txtPen_Interest",
                "frm1801v2018:txtPen_Compromise",
            ],
            state: penalty,
            basic_tax_due: "Item 22", |d| d.payables().1,
            // Maintainer decision (1800/1801 penalties run from the due date)
            // (2026-10-19): the return is filed within one (1)
            // year from the decedent's death (html-frozen/1801-2018/guide.html,
            // "When and Where to File and Pay"), so the penalties run from
            // that day, moved to the next business day on the app's deadline
            // calendar. The guide's extension of up to thirty (30) days is
            // one the Commissioner may grant in meritorious cases, not a
            // later due date for every return, so it is not added.
            typed_due_date: Some(|d: &Self| d.due_date()),
        )
    }

    /// The day this return is due: one year after the date of death (Item 1;
    /// a death on February 29 is due on February 28), or the next business
    /// day on the app's deadline calendar when that is not one. `None` until
    /// Item 1 is a date.
    pub fn due_date(&self) -> Option<NaiveDate> {
        let due = self
            .death_date()?
            .checked_add_months(chrono::Months::new(12))?;
        Some(super::penalty_items::next_business_day(due))
    }
}

impl QueueableForm for Form1801Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1801";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1801v2018']`).
    const FORM_TYPE: &'static str = "1801v2018";
    const LAYOUT_ID: &'static str = FORM_1801_FORM_ID;

    /// Item 24 Total Amount Payable.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.payables().2)
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
    /// Date of death `MM + DD + YYYY`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!(
            "{:02}{:02}{:04}",
            self.death_month, self.death_day, self.death_year
        )
    }
    /// The filename names the date of death, not the dashboard's open-ended
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
    /// The page grows its tables at run time, so the plaintext follows
    /// [`Form1801Draft::official_layout`] rather than the fixed layout.
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

    fn split(exclusive: f64, conjugal: f64) -> Form1801Split {
        Form1801Split {
            exclusive,
            conjugal,
        }
    }

    pub(crate) fn sample() -> Form1801Draft {
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Estate of Sample Decedent",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Estate",
            "registered_address": "1 Sample St",
            "zip_code": "1100",
            "phone": "09170000000",
            "email": "sample.taxpayer@example.com",
            "default_form_type": "1801",
            "taxpayer_type": "Estate",
            "business_start_date": "2020-01-15",
            "tax_elections": []
        }))
        .unwrap();
        let mut d = Form1801Draft::new_from_profile(&profile, 2025, 1);
        d.death_month = 6;
        d.death_day = 5;
        d.death_year = 2025;
        d.non_resident_alien = Some(false);
        d.administrator_name = "Sample Executor".into();
        d.other_personal = vec![Form1801Particular {
            particulars: "Car".into(),
            value: split(1_000_000.005, 500_000.0),
        }];
        d.ordinary_deductions.claims_against_estate = split(100_000.0, 0.0);
        d.standard_deduction = 5_000_000.0;
        d.recompute();
        d
    }

    fn messages(d: &Form1801Draft) -> Vec<String> {
        d.validate_on(today()).into_iter().map(|(_, m)| m).collect()
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// A new return (the maintainer decision's (1800/1801 penalties run from
    /// the due date) model) whose decedent died on `on`,
    /// with estate tax due.
    fn new_return(on: NaiveDate) -> Form1801Draft {
        let mut d = sample();
        assert_eq!(
            d.penalty.penalty_model_version,
            crate::forms::penalty_items::TYPED_DUE_DATE_MODEL_VERSION,
            "a new return is on P4-4"
        );
        d.standard_deduction = 0.0;
        d.death_year = u16::try_from(chrono::Datelike::year(&on)).unwrap();
        d.death_month = u8::try_from(chrono::Datelike::month(&on)).unwrap();
        d.death_day = u8::try_from(chrono::Datelike::day(&on)).unwrap();
        d
    }

    /// maintainer decision (1800/1801 penalties run from the due date): due one
    /// year after the death, moved past a weekend to the next
    /// business day; a death on February 29 is due on February 28; no due
    /// date until Item 1 is a date.
    #[test]
    fn due_one_year_after_the_death() {
        let due = |on| new_return(on).due_date();
        assert_eq!(due(date(2025, 6, 5)), Some(date(2026, 6, 5)));
        // Saturday 2025-10-04 moves to Monday.
        assert_eq!(due(date(2024, 10, 4)), Some(date(2025, 10, 6)));
        assert_eq!(due(date(2024, 2, 29)), Some(date(2025, 2, 28)));
        // An official holiday (maintainer decision: roll past official
        // holidays): 2026-04-09, Araw ng Kagitingan, moves
        // to Friday, April 10.
        assert_eq!(due(date(2025, 4, 9)), Some(date(2026, 4, 10)));
        let mut blank = sample();
        (blank.death_month, blank.death_day, blank.death_year) = (0, 0, 0);
        assert_eq!(blank.due_date(), None);
        assert_eq!(
            Form1801Draft::penalty_declaration().due_date(&new_return(date(2025, 6, 5))),
            Some(date(2026, 6, 5))
        );
    }

    /// maintainer decision (1800/1801 penalties run from the due date): filed
    /// after its due date the return computes the 25%
    /// surcharge, 12% a year interest and the compromise on Item 22; filed
    /// on the due date it computes none; a return stored before maintainer
    /// decision (1800/1801 penalties run from the due date)
    /// computes none until it joins.
    #[test]
    fn penalties_run_from_the_due_date() {
        use crate::forms::penalty_items::{PENALTY_MODEL_VERSION, with_clock};
        use crate::penalties::PenaltyProfile;
        use crate::penalties::compromise::lookup_compromise;
        let on = date(2025, 6, 5);
        let due = date(2026, 6, 5);
        let filed = date(2026, 10, 13);
        let mut late = new_return(on);
        with_clock(filed, || late.recompute());
        let tax = late.payables().1;
        assert_eq!(tax, 69_000.0);
        let days = (filed - due).num_days() as f64;
        let cents = |value: f64| (value * 100.0).round() / 100.0;
        assert_eq!(late.surcharge, cents(tax * 0.25));
        assert_eq!(late.interest, cents(tax * 0.12 * days / 365.0));
        assert_eq!(
            late.compromise,
            cents(lookup_compromise(tax, 0.0, PenaltyProfile::StandardFiling))
        );
        assert!(late.compromise > 0.0);
        let mut on_time = new_return(on);
        with_clock(due, || on_time.recompute());
        assert_eq!(
            (on_time.surcharge, on_time.interest, on_time.compromise),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(on_time.payables().2, tax);
        let mut stored = new_return(on);
        stored.penalty.penalty_model_version = PENALTY_MODEL_VERSION;
        stored.penalty.computed_penalties = None;
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
        assert_eq!(d.personal_property.total, 1_500_000.01);
        assert_eq!(d.gross_estate.exclusive, 1_000_000.01);
        assert_eq!(d.estate_after_deductions.total, 1_400_000.01);
        assert_eq!(d.spouse_share, 250_000.0);
        assert_eq!(d.net_taxable_estate, -3_849_999.99);
        let fields = d.to_bir_field_map();
        assert_eq!(fields["frm1801v2018:txtNetTaxEst_C"], "-3,849,999.99");
        assert_eq!(fields["frm1801v2018:txtEstTaxDue"], "0.00");
        assert_eq!(fields["frm1801v2018:txtTaxRate"], "6.0%");
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn a_positive_estate_pays_six_percent() {
        let mut d = sample();
        d.standard_deduction = 0.0;
        d.surcharge = 10.0;
        d.keep_entered_penalties();
        d.recompute();
        assert_eq!(d.net_taxable_estate, 1_150_000.01);
        assert_eq!(d.estate_tax_due(), 69_000.0);
        let (payable, first, total) = d.payables();
        assert_eq!((payable, first, total), (69_000.0, 69_000.0, 69_010.0));
        assert_eq!(
            d.to_bir_field_map()["frm1801v2018:txtEstTaxDue"],
            "69,000.00"
        );
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn print_map_shows_the_email_as_typed_and_the_upload_keeps_capitals() {
        let d = sample();
        let key = "frm1801v2018:txtEmail";
        assert_eq!(d.to_bir_field_map()[key], "SAMPLE.TAXPAYER@EXAMPLE.COM");
        assert_eq!(d.to_print_field_map()[key], "sample.taxpayer@example.com");
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1801Draft), expected: &str| {
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
            &|d| d.death_month = 13,
            "Please enter a valid month on Item 1.",
        );
        check(
            &|d| {
                d.death_month = 2;
                d.death_day = 29;
            },
            "Please enter a valid date on Item 1. Filing year is not a leap year.",
        );
        check(&|d| d.death_day = 31, "Please enter a valid day on Item 1.");
        check(
            &|d| d.death_year = 1900,
            "Invalid date entry on Item 1. Entry should not be lower than 1904.",
        );
        check(
            &|d| d.death_year = 2026,
            "Invalid date entry on Item 1. Date cannot be after the current date.",
        );
        check(
            &|d| d.tin.clear(),
            "Please enter a valid TIN number on Item 5.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 6.",
        );
        check(
            &|d| d.estate_name.clear(),
            "Please enter a valid Taxpayer's Name on Item 7.",
        );
        check(
            &|d| d.residence_address.clear(),
            "Please enter Taxpayer's Resident of Decedent on Item 8.",
        );
        check(
            &|d| d.non_resident_alien = None,
            "Please select an option for Item 9.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number on Item 12.",
        );
        check(
            &|d| d.installment_granted = true,
            "Please select a frequency of payment on Item 15D.",
        );
        check(
            &|d| {
                d.installment_granted = true;
                d.installment_frequency = Some(Form1801Frequency::Others);
            },
            "Please specify a frequency of payment on Item 15D - Others.",
        );
        check(
            &|d| d.standard_deduction = 5_000_000.01,
            "If No is selected on Item 9, value should not exceed Php5 Million on Item 37A.",
        );
        check(
            &|d| {
                d.non_resident_alien = Some(true);
                d.standard_deduction = 600_000.0;
            },
            "If Yes is selected on Item 9, value should not exceed Php500 Thousand on Item 37A.",
        );
        check(
            &|d| d.family_home_deduction = 10_000_000.01,
            "Value should not exceed Php10 Million on Item 37B.",
        );
        check(
            &|d| d.other_personal[0].particulars.clear(),
            "Incomplete values on Schedule 2A, Row 1.",
        );
        check(
            &|d| {
                d.real_properties = vec![Form1801RealProperty {
                    title_number: "T-1".into(),
                    ..Default::default()
                }]
            },
            "Incomplete values on Schedule 1, Row 1.",
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
