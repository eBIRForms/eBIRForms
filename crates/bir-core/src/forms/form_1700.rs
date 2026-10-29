//! BIR Form 1700 (January 2018) — Annual Income Tax Return for Individuals
//! Earning Purely Compensation Income.
//!
//! Ported from the official `BIR-Form1700v2018.hta` (eBIRForms 7.9.6.2.1):
//! the taxpayer and spouse compute chain (`computeSched1I5`,
//! `computePg2I44` … `computePg2I59`, `computePg1I30` … `computePg1I36`), the
//! radio rules (`processTaxpayerType`, `processCivilStatus`,
//! `processSpouseHasIncome`, `processFilingStatus`, the Schedule 1 employer
//! boxes), `validate()` with its exact alert texts, and the `saveXML` submit
//! loop through [`crate::official_xml`].
//!
//! Amount entries keep centavos (`round(this,2)`); computed items are
//! `toFixed(0)` whole pesos.
//!
//! Schedule 1 takes more than four employers the way the official
//! "(add more...)" popup (`modalPage2Part6`) does: the fourth and later rows
//! are popup rows `4_1`, `4_2`, … (`saveSched1Items`), Item 4 then shows
//! `OTHERS` with no TIN and the popup subtotal (`setUpSchedule1TableModal`,
//! `computeSubTotal`, `computeSched1I5`), and each popup row is checked by
//! `validateSched1('4_<n>')` with its own alert texts. Checked on the official
//! page (runtime oracle, with IE's implicit `<tbody>` in the popup's empty
//! tables): the popup rows are form controls inside `frmMain`, so they are
//! written by the upload loop (`saveEncryptedProfile`) where the DOM has
//! them: every row's employer cells, then every row's amounts, between
//! `ebirOnlineSecret` and `txtPg2Pt6I4SubtotalC`
//! ([`Form1700Draft::official_layout`]). Sample `1700-2025-others` is
//! byte-identical to the official page.

use super::penalty_items::{PenaltyItems, PenaltyState};
use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout. Only a lookup key: the layout it
/// names is the January 2018 (ENCS) page (`formType` `1700v2018`), which is
/// the revision the header shows and the upload uses.
pub const FORM_1700_FORM_ID: &str = "1700-v2013";
/// Schedule 1 employer rows (Items 1–4).
pub const FORM_1700_EMPLOYER_ROWS: usize = 4;
/// Employers this model accepts, popup rows included.
pub const FORM_1700_MAX_EMPLOYERS: usize = 100;
/// Item 4's employer name once the popup holds its rows.
pub const FORM_1700_OTHERS: &str = "OTHERS";
/// `init()` fixes Item 3.
pub const FORM_1700_ATC: &str = "II011";

/// Items 6 / 20.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1700TaxpayerType {
    #[default]
    Unanswered,
    /// Employee, graduated rates (Part V.A).
    Employee,
    /// Non-resident alien not engaged in trade or business, 25% (Part V.B).
    Nranetb,
}

/// Item 15.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1700CivilStatus {
    #[default]
    Unanswered,
    Single,
    Married,
    Separated,
    Widow,
}

/// Item 17.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1700FilingStatus {
    #[default]
    Unanswered,
    Joint,
    Separate,
}

/// One column (A taxpayer/filer, B spouse) of Parts III and V.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1700Column {
    // Part V.A
    pub gross_compensation: f64,
    pub non_taxable: f64,
    pub taxable_compensation: f64,
    pub other_income: f64,
    pub taxable_income: f64,
    pub graduated_tax_due: f64,
    // Part V.B
    pub flat_gross_compensation: f64,
    pub flat_non_taxable: f64,
    pub flat_taxable_compensation: f64,
    pub flat_other_income: f64,
    pub flat_taxable_income: f64,
    pub flat_tax_due: f64,
    // Part V.C
    pub tax_withheld: f64,
    pub previously_filed: f64,
    pub foreign_tax_credits: f64,
    pub other_credits: f64,
    pub total_credits: f64,
    pub net_payable: f64,
    // Part III
    pub tax_due: f64,
    /// 29 — portion allowed for the 2nd installment.
    pub second_installment: f64,
    pub amount_payable: f64,
    pub interest: f64,
    pub surcharge: f64,
    pub compromise: f64,
    /// This column's penalty items' computed values, overrides, note and
    /// engine inputs (the shared model in [`super::penalty_items`]).
    #[serde(flatten)]
    pub penalty: PenaltyState,
    pub total_penalties: f64,
    pub total_amount_payable: f64,
}

/// One Schedule 1 employer row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1700Employer {
    /// `true` for the spouse box, `false` for the taxpayer box.
    pub for_spouse: bool,
    pub name: String,
    /// The second name line.
    pub name2: String,
    /// 9 digits plus the branch code.
    pub tin: String,
    /// c — compensation subject to regular rates.
    pub regular: f64,
    /// d — compensation subject to the 25% flat rate.
    pub flat: f64,
    /// e — tax withheld.
    pub withheld: f64,
}

/// Part II — background information on the spouse.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Form1700Spouse {
    pub tin: String,
    pub rdo_code: String,
    pub taxpayer_type: Form1700TaxpayerType,
    pub name: String,
    pub contact_number: String,
    pub citizenship: String,
    pub foreign_tax_credits: Option<bool>,
    pub foreign_tax_number: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1700Draft {
    #[serde(default)]
    pub id: Option<i64>,

    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub taxable_year: u16,
    pub is_amended: bool,

    // Part I
    pub rdo_code: String,
    #[serde(default)]
    pub taxpayer_type: Form1700TaxpayerType,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    /// Item 9, `MM/DD/YYYY`.
    #[serde(default)]
    pub birth_date: String,
    pub email: String,
    #[serde(default)]
    pub citizenship: String,
    #[serde(default)]
    pub foreign_tax_credits: Option<bool>,
    #[serde(default)]
    pub foreign_tax_number: String,
    pub contact_number: String,
    #[serde(default)]
    pub civil_status: Form1700CivilStatus,
    #[serde(default)]
    pub spouse_has_income: Option<bool>,
    #[serde(default)]
    pub filing_status: Form1700FilingStatus,
    pub line_of_business: String,

    #[serde(default)]
    pub spouse: Form1700Spouse,
    #[serde(default)]
    pub taxpayer: Form1700Column,
    #[serde(default)]
    pub spouse_column: Form1700Column,

    /// Items 45, 49, 51 and 57 descriptions (shared by both columns).
    #[serde(default)]
    pub other_income_description: String,
    #[serde(default)]
    pub flat_non_taxable_description: String,
    #[serde(default)]
    pub flat_other_income_description: String,
    #[serde(default)]
    pub other_credits_description: String,

    /// Schedule 1 Items 1–4.
    #[serde(default)]
    pub employers: Vec<Form1700Employer>,
    /// Schedule 1 Item 5A / 5B (c, d, e).
    #[serde(default)]
    pub schedule_totals: [[f64; 3]; 2],

    /// Item 36.
    #[serde(default)]
    pub aggregate_amount_payable: f64,
    /// Item 37.
    #[serde(default)]
    pub number_of_attachments: u8,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `toFixed(0)` (half away from zero on the value).
fn fixed0(value: f64) -> f64 {
    if value.is_finite() {
        value.round()
    } else {
        0.0
    }
}

/// `round(this,2)` / `formatCurrency`: the value an entry holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

fn digits(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes;
/// the branch keeps what was entered (empty when none).
fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits = digits(tin);
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    (
        part(0..3),
        part(3..6),
        part(6..9),
        digits.get(9..).unwrap_or("").to_string(),
    )
}

fn tin_is_valid(tin: &str) -> bool {
    let (a, b, c, branch) = split_tin(tin);
    a.len() == 3
        && b.len() == 3
        && c.len() == 3
        && branch.len() <= 5
        && tin.chars().all(|ch| ch.is_ascii_digit() || ch == '-')
}

fn check_digit_ok(tin: &str) -> bool {
    let (a, b, c, _) = split_tin(tin);
    crate::validation::relaxed_dev_mode()
        || crate::validation::official_tin_check_code(&format!("{a}{b}{c}")) == 0
}

/// `text` over two print combs: the first `width` characters, cut back to
/// the last word break, then the rest.
fn split_print_line(text: &str, width: usize) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return (text.to_string(), String::new());
    }
    let cut = chars[..=width]
        .iter()
        .rposition(|ch| *ch == ' ')
        .filter(|at| *at > 0)
        .unwrap_or(width);
    let first: String = chars[..cut].iter().collect();
    let rest: String = chars[cut..].iter().collect();
    (first.trim_end().to_string(), rest.trim_start().to_string())
}

/// `computeTaxDue`: the table for the taxable year, `toFixed(2)` then the
/// `toFixed(0)` the caller applies.
pub fn form_1700_graduated_tax(year: u16, income: f64) -> f64 {
    let x = income;
    let tax = if year >= 2023 {
        if x >= 8_000_000.0 {
            (x - 8_000_000.0) * (35.0 / 100.0) + 2_202_500.0
        } else if x >= 2_000_000.0 {
            (x - 2_000_000.0) * (30.0 / 100.0) + 402_500.0
        } else if x >= 800_000.0 {
            (x - 800_000.0) * (25.0 / 100.0) + 102_500.0
        } else if x >= 400_000.0 {
            (x - 400_000.0) * (20.0 / 100.0) + 22_500.0
        } else if x >= 250_000.0 {
            (x - 250_000.0) * (15.0 / 100.0)
        } else {
            0.0
        }
    } else if x >= 8_000_000.0 {
        (x - 8_000_000.0) * (35.0 / 100.0) + 2_410_000.0
    } else if x >= 2_000_000.0 {
        (x - 2_000_000.0) * (32.0 / 100.0) + 490_000.0
    } else if x >= 800_000.0 {
        (x - 800_000.0) * (30.0 / 100.0) + 130_000.0
    } else if x >= 400_000.0 {
        (x - 400_000.0) * (25.0 / 100.0) + 30_000.0
    } else if x >= 250_000.0 {
        (x - 250_000.0) * (20.0 / 100.0)
    } else {
        0.0
    };
    fixed0(cents(tax))
}

/// `validateMonthDayYearDate`: `true` when the text is not a valid
/// `MM/DD/YYYY` date.
fn birth_date_invalid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('/').collect();
    if parts.len() != 3 || parts[0].len() != 2 || parts[1].len() != 2 || parts[2].len() != 4 {
        return true;
    }
    let (Ok(month), Ok(day), Ok(year)) = (
        parts[0].parse::<u32>(),
        parts[1].parse::<u32>(),
        parts[2].parse::<i32>(),
    ) else {
        return true;
    };
    NaiveDate::from_ymd_opt(year, month, day).is_none()
}

impl Form1700Column {
    fn round_inputs(&mut self) {
        for value in [
            &mut self.non_taxable,
            &mut self.other_income,
            &mut self.flat_non_taxable,
            &mut self.flat_other_income,
            &mut self.previously_filed,
            &mut self.foreign_tax_credits,
            &mut self.other_credits,
            &mut self.second_installment,
            &mut self.interest,
            &mut self.surcharge,
            &mut self.compromise,
        ] {
            *value = cents(*value);
        }
    }

    /// Part V and Part III for one column. `spouse` follows the page's
    /// slightly different rounding of Item 44B.
    fn compute(&mut self, year: u16, kind: Form1700TaxpayerType, spouse: bool) {
        self.taxable_compensation = if spouse {
            fixed0(self.gross_compensation - self.non_taxable)
        } else {
            self.gross_compensation - fixed0(self.non_taxable)
        };
        self.taxable_income = fixed0(self.taxable_compensation + self.other_income);
        self.graduated_tax_due = form_1700_graduated_tax(year, self.taxable_income);
        self.flat_taxable_compensation =
            fixed0(self.flat_gross_compensation - self.flat_non_taxable);
        self.flat_taxable_income = fixed0(self.flat_taxable_compensation + self.flat_other_income);
        self.flat_tax_due = fixed0(self.flat_taxable_income * (25.0 / 100.0));
        self.total_credits = fixed0(
            self.tax_withheld
                + self.previously_filed
                + self.foreign_tax_credits
                + self.other_credits,
        );
        self.tax_due = match kind {
            Form1700TaxpayerType::Nranetb => self.flat_tax_due,
            _ => self.graduated_tax_due,
        };
        self.net_payable = cents(self.tax_due - self.total_credits);
        let item28 = fixed0(self.net_payable);
        self.amount_payable = cents(item28 - fixed0(self.second_installment));
        self.total_penalties = fixed0(self.interest + self.surcharge + self.compromise);
        self.total_amount_payable = if self.amount_payable >= 0.0 {
            cents(self.amount_payable + self.total_penalties)
        } else if self.total_penalties == 0.0 {
            self.amount_payable
        } else {
            self.total_penalties
        };
    }
}

impl Form1700Draft {
    pub const FORM_CODE: &'static str = "1700";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16) -> Self {
        let mut draft = Self {
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            is_amended: false,
            rdo_code: profile.rdo_code.clone(),
            taxpayer_type: Form1700TaxpayerType::Unanswered,
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            birth_date: profile
                .birth_date
                .map(|d| d.format("%m/%d/%Y").to_string())
                .unwrap_or_default(),
            email: profile.email.clone(),
            citizenship: String::new(),
            foreign_tax_credits: None,
            foreign_tax_number: String::new(),
            contact_number: profile.phone.clone(),
            civil_status: Form1700CivilStatus::Unanswered,
            spouse_has_income: None,
            filing_status: Form1700FilingStatus::Unanswered,
            line_of_business: profile.line_of_business.clone(),
            spouse: Form1700Spouse::default(),
            taxpayer: Form1700Column::default(),
            spouse_column: Form1700Column::default(),
            other_income_description: String::new(),
            flat_non_taxable_description: String::new(),
            flat_other_income_description: String::new(),
            other_credits_description: String::new(),
            employers: Vec::new(),
            schedule_totals: [[0.0; 3]; 2],
            aggregate_amount_payable: 0.0,
            number_of_attachments: 0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Part II and column B are open only on a joint return.
    pub fn is_joint(&self) -> bool {
        self.civil_status == Form1700CivilStatus::Married
            && self.spouse_has_income == Some(true)
            && self.filing_status == Form1700FilingStatus::Joint
    }

    fn spouse_type(&self) -> Form1700TaxpayerType {
        if self.is_joint() {
            self.spouse.taxpayer_type
        } else {
            Form1700TaxpayerType::Unanswered
        }
    }

    fn any_type(&self, kind: Form1700TaxpayerType) -> bool {
        self.taxpayer_type == kind || self.spouse_type() == kind
    }

    /// Schedule 1 column c is open when either filer is an employee.
    pub fn regular_column_open(&self) -> bool {
        self.any_type(Form1700TaxpayerType::Employee)
    }

    /// Schedule 1 column d is open when either filer is an NRANETB.
    pub fn flat_column_open(&self) -> bool {
        self.any_type(Form1700TaxpayerType::Nranetb)
    }

    /// The radio rules, then the official compute chain.
    pub fn recompute(&mut self) {
        if self.civil_status != Form1700CivilStatus::Married {
            self.spouse_has_income = None;
        }
        if self.spouse_has_income != Some(true) {
            self.filing_status = Form1700FilingStatus::Unanswered;
        }
        if !self.is_joint() {
            self.spouse = Form1700Spouse::default();
            self.spouse_column = Form1700Column::default();
            for row in &mut self.employers {
                if row.for_spouse {
                    *row = Form1700Employer::default();
                }
            }
        }
        if self.foreign_tax_credits != Some(true) {
            self.foreign_tax_number.clear();
            self.taxpayer.foreign_tax_credits = 0.0;
        }
        if self.spouse.foreign_tax_credits != Some(true) {
            self.spouse.foreign_tax_number.clear();
            self.spouse_column.foreign_tax_credits = 0.0;
        }
        if !self.is_amended {
            self.taxpayer.previously_filed = 0.0;
            self.spouse_column.previously_filed = 0.0;
        }
        let tp_type = self.taxpayer_type;
        let sp_type = self.spouse_type();
        // disablePartVA*/VB*: the part a filer type does not use is zero.
        for (column, kind) in [
            (&mut self.taxpayer, tp_type),
            (&mut self.spouse_column, sp_type),
        ] {
            if kind != Form1700TaxpayerType::Employee {
                column.non_taxable = 0.0;
                column.other_income = 0.0;
                column.second_installment = 0.0;
            }
            if kind != Form1700TaxpayerType::Nranetb {
                column.flat_non_taxable = 0.0;
                column.flat_other_income = 0.0;
            }
        }
        if !self.any_type(Form1700TaxpayerType::Employee) {
            self.other_income_description.clear();
        }
        if !self.any_type(Form1700TaxpayerType::Nranetb) {
            self.flat_non_taxable_description.clear();
            self.flat_other_income_description.clear();
        }
        let regular_open = self.regular_column_open();
        let flat_open = self.flat_column_open();
        self.employers.truncate(FORM_1700_MAX_EMPLOYERS);
        for row in &mut self.employers {
            if !regular_open {
                row.regular = 0.0;
            }
            if !flat_open {
                row.flat = 0.0;
            }
            row.regular = cents(row.regular);
            row.flat = cents(row.flat);
            row.withheld = cents(row.withheld);
        }
        while self
            .employers
            .last()
            .is_some_and(|row| *row == Form1700Employer::default())
        {
            self.employers.pop();
        }
        self.taxpayer.round_inputs();
        self.spouse_column.round_inputs();

        // computeSched1I5 (an unticked row counts toward the spouse total).
        let mut totals = [[0.0f64; 3]; 2];
        for row in &self.employers {
            let who = usize::from(row.for_spouse);
            totals[who][0] += row.regular;
            totals[who][1] += row.flat;
            totals[who][2] += row.withheld;
        }
        for total in totals.iter_mut().flatten() {
            *total = fixed0(*total);
        }
        self.schedule_totals = totals;
        let year = self.taxable_year;
        self.taxpayer.gross_compensation = 0.0;
        self.taxpayer.flat_gross_compensation = 0.0;
        if tp_type == Form1700TaxpayerType::Employee {
            self.taxpayer.gross_compensation = totals[0][0];
        } else {
            self.taxpayer.flat_gross_compensation = totals[0][1];
        }
        self.taxpayer.tax_withheld = totals[0][2];
        self.spouse_column.gross_compensation = 0.0;
        self.spouse_column.flat_gross_compensation = 0.0;
        match sp_type {
            Form1700TaxpayerType::Employee => self.spouse_column.gross_compensation = totals[1][0],
            Form1700TaxpayerType::Nranetb => {
                self.spouse_column.flat_gross_compensation = totals[1][1]
            }
            Form1700TaxpayerType::Unanswered => {}
        }
        self.spouse_column.tax_withheld = if self.is_joint() { totals[1][2] } else { 0.0 };
        self.taxpayer.compute(year, tp_type, false);
        self.spouse_column.compute(year, sp_type, true);
        // Each column's penalty items file their override, else the
        // computed value; the column totals are then summed with them.
        for items in Self::penalty_declarations() {
            items.apply(self);
        }
        self.taxpayer.round_inputs();
        self.spouse_column.round_inputs();
        self.taxpayer.compute(year, tp_type, false);
        self.spouse_column.compute(year, sp_type, true);
        self.aggregate_amount_payable =
            cents(self.taxpayer.total_amount_payable + self.spouse_column.total_amount_payable);
    }

    /// More than four employers: the fourth and later ones are popup rows.
    pub fn schedule_is_folded(&self) -> bool {
        self.employers.len() > FORM_1700_EMPLOYER_ROWS
    }

    /// The popup rows (`4_1`, `4_2`, …), empty unless folded.
    pub fn popup_rows(&self) -> &[Form1700Employer] {
        if self.schedule_is_folded() {
            &self.employers[FORM_1700_EMPLOYER_ROWS - 1..]
        } else {
            &[]
        }
    }

    /// `computeSubTotal`: the popup's c, d and e subtotals (`formatCurrency`).
    pub fn popup_subtotals(&self) -> [f64; 3] {
        let rows = self.popup_rows();
        [
            cents(rows.iter().map(|row| row.regular).sum()),
            cents(rows.iter().map(|row| row.flat).sum()),
            cents(rows.iter().map(|row| row.withheld).sum()),
        ]
    }

    /// Texts typed before a later `capital()` trigger in form order are
    /// upper-cased: Item 11 (always), Items 21/23 on a joint return, and the
    /// Schedule 1 employer names on page 2.
    fn caps_after(&self, position: u8) -> bool {
        let employer_names = self
            .employers
            .iter()
            .any(|row| !row.name.trim().is_empty() || !row.name2.trim().is_empty());
        let spouse_trigger = self.is_joint()
            && (!self.spouse.name.trim().is_empty() || !self.spouse.citizenship.trim().is_empty());
        (position < 11 && !self.citizenship.trim().is_empty())
            || (position < 23 && spouse_trigger)
            || (position < 60 && employer_names)
    }

    /// The official field values the submit loop reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1700:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let money = official_amount;
        let typed = |value: &str, position: u8| {
            if self.caps_after(position) {
                value.to_uppercase()
            } else {
                value.to_string()
            }
        };

        put("txtPg1I1Year", self.taxable_year.to_string());
        put("txtPg1I3ATC", FORM_1700_ATC.to_string());
        put("rdoPg1I2AmendedYes", flag(self.is_amended));
        put("rdoPg1I2AmendedNo", flag(!self.is_amended));
        let (t1, t2, t3, branch) = split_tin(&self.tin);
        let branch = format!("{branch:0>5}");
        for (a, b, c, br) in [
            (
                "txtPg1I4TIN1",
                "txtPg1I4TIN2",
                "txtPg1I4TIN3",
                "txtPg1I4BranchCode",
            ),
            ("txtPg2TIN1", "txtPg2TIN2", "txtPg2TIN3", "txtPg2BranchCode"),
        ] {
            put(a, t1.clone());
            put(b, t2.clone());
            put(c, t3.clone());
            put(br, branch.clone());
        }
        put("txtRDOCode", self.rdo_code.trim().to_string());
        use Form1700TaxpayerType as T;
        put(
            "rdoPg1I6TaxpayerTypeE",
            flag(self.taxpayer_type == T::Employee),
        );
        put(
            "rdoPg1I6TaxpayerTypeN",
            flag(self.taxpayer_type == T::Nranetb),
        );
        let name = self.taxpayer_name.trim().to_uppercase();
        put("txtPg1I7TaxpayerName", name.clone());
        put(
            "txtPg2TaxpayerName",
            name.split(',').next().unwrap_or("").to_string(),
        );
        // loadBGData splits the address at 127 characters.
        let address = self.registered_address.trim().to_uppercase();
        let split = address
            .char_indices()
            .nth(127)
            .map_or(address.len(), |(at, _)| at);
        put("txtPg1I8Address", address[..split].to_string());
        put("txtPg1I8Address2", address[split..].to_string());
        put("txtPg1I8AZipCode", self.zip_code.trim().to_string());
        put("txtPg1I9BirthDate", self.birth_date.trim().to_string());
        // loadBGData shows Item 10 in capitals; the IAF filename keeps it.
        put("txtPg1I10Email", self.email.trim().to_uppercase());
        put(
            "txtPg1I11Citizenship",
            self.citizenship.trim().to_uppercase(),
        );
        put(
            "rdoPg1I12ForeignTaxCreditsYes",
            flag(self.foreign_tax_credits == Some(true)),
        );
        put(
            "rdoPg1I12ForeignTaxCreditsNo",
            flag(self.foreign_tax_credits == Some(false)),
        );
        put(
            "txtPg1I13ForeignTaxNumber",
            typed(&self.foreign_tax_number, 13),
        );
        put("txtPg1I14TelNum", self.contact_number.trim().to_string());
        use Form1700CivilStatus as C;
        put(
            "rdoPg1I15CivilStatusS",
            flag(self.civil_status == C::Single),
        );
        put(
            "rdoPg1I15CivilStatusM",
            flag(self.civil_status == C::Married),
        );
        put(
            "rdoPg1I15CivilStatusLS",
            flag(self.civil_status == C::Separated),
        );
        put("rdoPg1I15CivilStatusW", flag(self.civil_status == C::Widow));
        put(
            "rdoPg1I16SpouseHasIncomeY",
            flag(self.spouse_has_income == Some(true)),
        );
        put(
            "rdoPg1I16SpouseHasIncomeN",
            flag(self.spouse_has_income == Some(false)),
        );
        put(
            "rdoPg1I17FilingStatusJ",
            flag(self.filing_status == Form1700FilingStatus::Joint),
        );
        put(
            "rdoPg1I17FilingStatusS",
            flag(self.filing_status == Form1700FilingStatus::Separate),
        );

        // Part II: disableSpouseBGInformation empties the spouse TIN boxes,
        // the branch code included, whenever the return is not joint.
        let s = &self.spouse;
        let (s1, s2, s3, sb) = if self.is_joint() {
            split_tin(&s.tin)
        } else {
            Default::default()
        };
        put("txtPg1I18STIN1", s1);
        put("txtPg1I18STIN2", s2);
        put("txtPg1I18STIN3", s3);
        put("txtPg1I18SBranchCode", sb);
        put(
            "txtSpouseRDOCode",
            if self.is_joint() && !s.rdo_code.trim().is_empty() {
                s.rdo_code.trim().to_string()
            } else {
                "000".to_string()
            },
        );
        put(
            "rdoPg1I20SpouseTaxpayerTypeE",
            flag(s.taxpayer_type == T::Employee),
        );
        put(
            "rdoPg1I20SpouseTaxpayerTypeN",
            flag(s.taxpayer_type == T::Nranetb),
        );
        put("txtPg1I21SpouseName", s.name.trim().to_uppercase());
        put(
            "txtPg1I22TSpouseTelNum",
            s.contact_number.trim().to_string(),
        );
        put(
            "txtPg1I23SpouseCitizenship",
            s.citizenship.trim().to_uppercase(),
        );
        put(
            "rdoPg1I24SpouseForeignTaxCreditsYes",
            flag(s.foreign_tax_credits == Some(true)),
        );
        put(
            "rdoPg1I24SpouseForeignTaxCreditsNo",
            flag(s.foreign_tax_credits == Some(false)),
        );
        put(
            "txtPg1I25SpouseForeignTaxNumber",
            typed(&s.foreign_tax_number, 25),
        );

        for (suffix, column) in [("A", &self.taxpayer), ("B", &self.spouse_column)] {
            let mut p1 = |n: u8, value: f64| put(&format!("txtPg1I{n}{suffix}"), money(value));
            p1(26, column.tax_due);
            p1(27, column.total_credits);
            p1(28, fixed0(column.net_payable));
            p1(29, column.second_installment);
            p1(30, column.amount_payable);
            p1(31, column.interest);
            p1(32, column.surcharge);
            p1(33, column.compromise);
            p1(34, column.total_penalties);
            p1(35, column.total_amount_payable);
            let mut p2 = |n: u8, value: f64| put(&format!("txtPg2I{n}{suffix}"), money(value));
            p2(42, column.gross_compensation);
            p2(43, column.non_taxable);
            p2(44, column.taxable_compensation);
            p2(45, column.other_income);
            p2(46, column.taxable_income);
            p2(47, column.graduated_tax_due);
            p2(48, column.flat_gross_compensation);
            p2(49, column.flat_non_taxable);
            p2(50, column.flat_taxable_compensation);
            p2(51, column.flat_other_income);
            p2(52, column.flat_taxable_income);
            p2(53, column.flat_tax_due);
            p2(54, column.tax_withheld);
            p2(55, column.previously_filed);
            p2(56, column.foreign_tax_credits);
            p2(57, column.other_credits);
            p2(58, column.total_credits);
            p2(59, column.net_payable);
        }
        put("txtPg1I36", money(self.aggregate_amount_payable));
        put(
            "txtPg1I37NumberOfAttachments",
            if self.number_of_attachments == 0 {
                "00".to_string()
            } else {
                self.number_of_attachments.to_string()
            },
        );
        put("txtPg2I45Desc", typed(&self.other_income_description, 45));
        put(
            "txtPg2I49Desc",
            typed(&self.flat_non_taxable_description, 49),
        );
        put(
            "txtPg2I51Desc",
            typed(&self.flat_other_income_description, 51),
        );
        put("txtPg2I57Desc", typed(&self.other_credits_description, 57));

        for index in 0..FORM_1700_EMPLOYER_ROWS {
            let n = index + 1;
            let mut row = self.employers.get(index).cloned().unwrap_or_default();
            if n == FORM_1700_EMPLOYER_ROWS && self.schedule_is_folded() {
                // setUpSchedule1TableModal + computeSched1I5: Item 4 keeps
                // its tick, reads OTHERS and carries the popup subtotal.
                let [regular, flat, withheld] = self.popup_subtotals();
                row = Form1700Employer {
                    for_spouse: row.for_spouse,
                    name: FORM_1700_OTHERS.to_string(),
                    name2: String::new(),
                    tin: String::new(),
                    regular,
                    flat,
                    withheld,
                };
            }
            let used = row != Form1700Employer::default();
            put(
                &format!("rdoPg2I{n}PartVIEmployeeT"),
                flag(used && !row.for_spouse),
            );
            put(
                &format!("rdoPg2I{n}PartVIEmployeeS"),
                flag(used && row.for_spouse),
            );
            put(
                &format!("txtPg2I{n}PartVIEmployerName1"),
                row.name.trim().to_uppercase(),
            );
            put(
                &format!("txtPg2I{n}PartVIEmployerName2"),
                row.name2.trim().to_uppercase(),
            );
            let (e1, e2, e3, eb) = split_tin(&row.tin);
            put(&format!("txtPg2I{n}PartVIEmployerTIN1"), e1);
            put(&format!("txtPg2I{n}PartVIEmployerTIN2"), e2);
            put(&format!("txtPg2I{n}PartVIEmployerTIN3"), e3);
            put(&format!("txtPg2I{n}PartVIEmployerBranchCode"), eb);
            put(&format!("txtPg2ISched1c_{n}REG"), money(row.regular));
            put(&format!("txtPg2ISched1d_{n}CIFR"), money(row.flat));
            put(&format!("txtPg2ISched1e_{n}TW"), money(row.withheld));
        }
        for (who, suffix) in [(0usize, "A"), (1, "B")] {
            put(
                &format!("txtPg2ISched1c_5{suffix}REG"),
                money(self.schedule_totals[who][0]),
            );
            put(
                &format!("txtPg2ISched1d_5{suffix}CIFR"),
                money(self.schedule_totals[who][1]),
            );
            put(
                &format!("txtPg2ISched1e_5{suffix}TW"),
                money(self.schedule_totals[who][2]),
            );
        }
        if self.schedule_is_folded() {
            let [c, d, e] = self.popup_subtotals();
            put("txtPg2Pt6I4SubtotalC", money(c));
            put("txtPg2Pt6I4SubtotalD", money(d));
            put("txtPg2Pt6I4SubtotalE", money(e));
            // The popup's own rows (`4_1` is the original Item 4), written
            // where the page keeps them (see [`Form1700Draft::official_layout`]).
            for (index, row) in self.popup_rows().iter().enumerate() {
                let n = format!("4_{}", index + 1);
                put(&format!("rdoPg2I{n}PartVIEmployeeT"), flag(!row.for_spouse));
                put(&format!("rdoPg2I{n}PartVIEmployeeS"), flag(row.for_spouse));
                put(
                    &format!("txtPg2I{n}PartVIEmployerName1"),
                    row.name.trim().to_uppercase(),
                );
                put(
                    &format!("txtPg2I{n}PartVIEmployerName2"),
                    row.name2.trim().to_uppercase(),
                );
                let (e1, e2, e3, eb) = split_tin(&row.tin);
                put(&format!("txtPg2I{n}PartVIEmployerTIN1"), e1);
                put(&format!("txtPg2I{n}PartVIEmployerTIN2"), e2);
                put(&format!("txtPg2I{n}PartVIEmployerTIN3"), e3);
                put(&format!("txtPg2I{n}PartVIEmployerBranchCode"), eb);
                put(&format!("txtPg2ISched1c_{n}REG"), money(row.regular));
                put(&format!("txtPg2ISched1d_{n}CIFR"), money(row.flat));
                put(&format!("txtPg2ISched1e_{n}TW"), money(row.withheld));
            }
        }
        // The submit loop writes "1" for the current page.
        put("txtCurrentPage", "1".to_string());
        put("txtLOB", self.line_of_business.trim().to_uppercase());
        fields
    }

    /// The generated layout with the add-more popup's rows spliced in where
    /// the page keeps them (`addRow_schedule1` inside `frmMain`): every row's
    /// employer cells (`4_1`, `4_2`, …), then every row's amounts, between
    /// `ebirOnlineSecret` and `txtPg2Pt6I4SubtotalC`.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        let base = crate::official_xml::layout(FORM_1700_FORM_ID).map_err(|e| e.to_string())?;
        let rows = self.popup_rows().len();
        if rows == 0 {
            return Ok(base.clone());
        }
        let key =
            |n: usize, field: &str| format!("frm1700:{}", field.replace("{n}", &format!("4_{n}")));
        let template = |field: &str| format!("frm1700:{}", field.replace("{n}", "4"));
        let mut copies = Vec::new();
        for n in 1..=rows {
            for field in [
                "rdoPg2I{n}PartVIEmployeeT",
                "rdoPg2I{n}PartVIEmployeeS",
                "txtPg2I{n}PartVIEmployerName1",
                "txtPg2I{n}PartVIEmployerName2",
                "txtPg2I{n}PartVIEmployerTIN1",
                "txtPg2I{n}PartVIEmployerTIN2",
                "txtPg2I{n}PartVIEmployerTIN3",
                "txtPg2I{n}PartVIEmployerBranchCode",
            ] {
                copies.push((template(field), key(n, field)));
            }
        }
        for n in 1..=rows {
            for field in [
                "txtPg2ISched1c_{n}REG",
                "txtPg2ISched1d_{n}CIFR",
                "txtPg2ISched1e_{n}TW",
            ] {
                copies.push((template(field), key(n, field)));
            }
        }
        super::official_inputs::extend_layout(
            base,
            &[super::official_inputs::RowInsertion {
                after: "ebirOnlineSecret".into(),
                copies,
            }],
        )
    }

    /// The field map plus the `derived:` values the printed January 2018
    /// sheet needs (`html-frozen/1700-2018/writer-cells.json`). Print only;
    /// never submitted.
    ///
    /// - `derived:amount_<item><A|B>` / `derived:amount_36` /
    ///   `derived:amount_sched1_<row><c|d|e>`: whole pesos (`toFixed(0)`; the
    ///   sheet says "DO NOT enter Centavos"), right-aligned in the sheet's
    ///   comb; an overpayment prints in parentheses. Zero, column B of a
    ///   non-joint return, and amounts wider than the comb stay blank. The
    ///   sheet prints Item 31 Surcharge and Item 32 Interest (the page's own
    ///   controls are the other way round).
    /// - `derived:address_line1/2`: the address over the 40- and 32-slot
    ///   combs, split at a word break.
    /// - `derived:birth_mm/dd/yyyy`, `derived:tin_digits` (page 2's nine
    ///   TIN slots),
    ///   `derived:employer<n>_tin`, `derived:attachments`,
    ///   `derived:spouse_rdo` (joint only).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        const AMOUNT_SLOTS: usize = 8;
        const WITHHELD_SLOTS: usize = 7;
        let mut fields = self.to_bir_field_map();
        // The upload keeps the e-mail in capitals; the sheet shows it as typed.
        fields.insert(
            "frm1700:txtPg1I10Email".to_string(),
            self.email.trim().to_string(),
        );
        let mut derived = BTreeMap::new();
        let whole = |value: f64, slots: usize| -> Option<String> {
            let value = fixed0(value);
            if value == 0.0 {
                return None;
            }
            let digits = format!("{:.0}", value.abs());
            let printed = if value < 0.0 {
                format!("({digits})")
            } else {
                digits
            };
            (printed.len() <= slots).then(|| format!("{printed:>slots$}"))
        };
        let mut columns = vec![("A", &self.taxpayer)];
        if self.is_joint() {
            columns.push(("B", &self.spouse_column));
        }
        for (suffix, c) in columns {
            for (item, value) in [
                (26, c.tax_due),
                (27, c.total_credits),
                (28, c.net_payable),
                (29, c.second_installment),
                (30, c.amount_payable),
                (31, c.surcharge),
                (32, c.interest),
                (33, c.compromise),
                (34, c.total_penalties),
                (35, c.total_amount_payable),
                (42, c.gross_compensation),
                (43, c.non_taxable),
                (44, c.taxable_compensation),
                (45, c.other_income),
                (46, c.taxable_income),
                (47, c.graduated_tax_due),
                (48, c.flat_gross_compensation),
                (49, c.flat_non_taxable),
                (50, c.flat_taxable_compensation),
                (51, c.flat_other_income),
                (52, c.flat_taxable_income),
                (53, c.flat_tax_due),
                (54, c.tax_withheld),
                (55, c.previously_filed),
                (56, c.foreign_tax_credits),
                (57, c.other_credits),
                (58, c.total_credits),
                (59, c.net_payable),
            ] {
                if let Some(printed) = whole(value, AMOUNT_SLOTS) {
                    derived.insert(format!("amount_{item}{suffix}"), printed);
                }
            }
        }
        if let Some(printed) = whole(self.aggregate_amount_payable, AMOUNT_SLOTS) {
            derived.insert("amount_36".to_string(), printed);
        }

        // Schedule 1 rows 1-4 as the return files them (Item 4 folded into
        // OTHERS when the popup holds rows), then 5A/5B.
        let mut schedule = Vec::new();
        for n in 1..=FORM_1700_EMPLOYER_ROWS {
            let amount = |column: &str| {
                fields
                    .get(&format!("frm1700:txtPg2ISched1{column}"))
                    .and_then(|text| parse_official_amount(text))
                    .unwrap_or(0.0)
            };
            schedule.push((
                n.to_string(),
                [
                    amount(&format!("c_{n}REG")),
                    amount(&format!("d_{n}CIFR")),
                    amount(&format!("e_{n}TW")),
                ],
            ));
            let tin = [
                format!("frm1700:txtPg2I{n}PartVIEmployerTIN1"),
                format!("frm1700:txtPg2I{n}PartVIEmployerTIN2"),
                format!("frm1700:txtPg2I{n}PartVIEmployerTIN3"),
                format!("frm1700:txtPg2I{n}PartVIEmployerBranchCode"),
            ]
            .iter()
            .filter_map(|key| fields.get(key))
            .cloned()
            .collect::<String>();
            if !tin.is_empty() {
                derived.insert(format!("employer{n}_tin"), tin);
            }
        }
        schedule.push(("5A".to_string(), self.schedule_totals[0]));
        if self.is_joint() {
            schedule.push(("5B".to_string(), self.schedule_totals[1]));
        }
        for (row, values) in schedule {
            for (column, value, slots) in [
                ("c", values[0], AMOUNT_SLOTS),
                ("d", values[1], AMOUNT_SLOTS),
                ("e", values[2], WITHHELD_SLOTS),
            ] {
                if let Some(printed) = whole(value, slots) {
                    derived.insert(format!("amount_sched1_{row}{column}"), printed);
                }
            }
        }

        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = split_print_line(&address, 40);
        derived.insert("address_line1".to_string(), line1);
        derived.insert("address_line2".to_string(), line2);
        let mut parts = self.birth_date.trim().split('/');
        if let (Some(mm), Some(dd), Some(yyyy), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        {
            derived.insert("birth_mm".to_string(), mm.to_string());
            derived.insert("birth_dd".to_string(), dd.to_string());
            derived.insert("birth_yyyy".to_string(), yyyy.to_string());
        }
        let (t1, t2, t3, _) = split_tin(&self.tin);
        derived.insert("tin_digits".to_string(), format!("{t1}{t2}{t3}"));
        if self.number_of_attachments > 0 {
            derived.insert(
                "attachments".to_string(),
                format!("{:02}", self.number_of_attachments % 100),
            );
        }
        if self.is_joint() {
            derived.insert(
                "spouse_rdo".to_string(),
                self.spouse.rdo_code.trim().to_string(),
            );
        }
        fields.extend(
            derived
                .into_iter()
                .map(|(key, value)| (format!("derived:{key}"), value)),
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
        let year = i32::from(self.taxable_year);

        // validate(), in order.
        if self.taxable_year == 0 {
            err("taxable_year", "Please enter a valid year in Item 1.");
        }
        if !tin_is_valid(&self.tin) || split_tin(&self.tin).3.len() < 3 {
            err("tin", "Please enter a valid TIN number on Item 4.");
        } else if !check_digit_ok(&self.tin) {
            // The official getChkTinErrDesc(...) + " on Item N." pattern.
            err(
                "tin",
                &format!(
                    "{} on Item 4.",
                    crate::validation::OFFICIAL_INVALID_TIN_MESSAGE
                ),
            );
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        if self.taxpayer_type == Form1700TaxpayerType::Unanswered {
            err("taxpayer_type", "Please select Taxpayer Type on Item no. 6");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 50 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 7.",
            );
        }
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > 227 {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 8.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !zip.bytes().all(|b| b.is_ascii_digit()) {
            err(
                "zip_code",
                "Please enter Taxpayer's Registered Address on Item 8A.",
            );
        }
        let birth = self.birth_date.trim();
        if birth.is_empty() {
            err(
                "birth_date",
                "Please indicate Birth Date of Taxpayer on item 9.",
            );
        } else if birth_date_invalid(birth) {
            err(
                "birth_date",
                "Invalid birth date on item 9 of Taxpayer.  Please check date format.",
            );
        }
        if self.citizenship.trim().is_empty() {
            err(
                "citizenship",
                "Please enter Taxpayer's Citizenship on Item 11",
            );
        }
        if self.civil_status == Form1700CivilStatus::Unanswered {
            err("civil_status", "Please indicate Civil Status on Item 15");
        }
        if self.civil_status == Form1700CivilStatus::Married {
            match self.spouse_has_income {
                Some(true) => match self.filing_status {
                    Form1700FilingStatus::Joint => {
                        let s = &self.spouse;
                        let (a, b, c, _) = split_tin(&s.tin);
                        if a.is_empty() || b.is_empty() || c.is_empty() {
                            err("spouse.tin", "Please enter Spouse TIN");
                        } else if !tin_is_valid(&s.tin) {
                            err("spouse.tin", "Please enter a valid TIN number on Item 18.");
                        } else if !check_digit_ok(&s.tin) {
                            err(
                                "spouse.tin",
                                &format!(
                                    "{} on Item 18.",
                                    crate::validation::OFFICIAL_INVALID_TIN_MESSAGE
                                ),
                            );
                        }
                        if !crate::validation::rdo_code_is_official_option(s.rdo_code.trim()) {
                            err(
                                "spouse.rdo_code",
                                "Please enter a valid RDO Code on Item 19.",
                            );
                        }
                        if s.name.trim().is_empty() {
                            err("spouse.name", "Please enter Spouse Name on Item 21.");
                        }
                        if s.taxpayer_type == Form1700TaxpayerType::Unanswered {
                            // Not checked by validate(); the spouse column has
                            // no tax base without it.
                            err(
                                "spouse.taxpayer_type",
                                "Select the spouse's taxpayer type (Item 20).",
                            );
                        }
                    }
                    Form1700FilingStatus::Separate => {}
                    Form1700FilingStatus::Unanswered => {
                        err("filing_status", "Please choose Yes or No on Item 17");
                    }
                },
                Some(false) => {}
                None => err("spouse_has_income", "Please choose Yes or No on Item 16"),
            }
        }
        if self.employers.len() > FORM_1700_MAX_EMPLOYERS {
            err(
                "employers",
                "Schedule 1 holds at most 100 employers per return in this editor.",
            );
        }
        let folded = self.schedule_is_folded();
        for (index, row) in self.employers.iter().enumerate() {
            // validateSched1More ('4_<n>') when the popup is saved; validate()
            // skips Item 4 once the popup holds its rows.
            let n = if folded && index + 1 >= FORM_1700_EMPLOYER_ROWS {
                format!("4_{}", index + 2 - FORM_1700_EMPLOYER_ROWS)
            } else {
                (index + 1).to_string()
            };
            if *row == Form1700Employer::default() {
                continue;
            }
            if row.name.trim().is_empty() && row.name2.trim().is_empty() {
                err(
                    &format!("employers[{index}].name"),
                    &format!("Please enter Employer's name on Part VI Item {n}A "),
                );
                continue;
            }
            let (a, b, c, branch) = split_tin(&row.tin);
            if a.is_empty() || b.is_empty() || c.is_empty() {
                err(
                    &format!("employers[{index}].tin"),
                    &format!("Please enter Employer's TIN on Part VI Item {n}B"),
                );
                continue;
            }
            if !tin_is_valid(&row.tin) || branch.len() < 3 {
                err(
                    &format!("employers[{index}].tin"),
                    &format!("Please enter valid TIN on Part VI Item {n}B"),
                );
                continue;
            }
            if !check_digit_ok(&row.tin) {
                err(
                    &format!("employers[{index}].tin"),
                    &format!(
                        "{} Part VI Item {n}B.",
                        crate::validation::OFFICIAL_INVALID_TIN_MESSAGE
                    ),
                );
                continue;
            }
            let zero_message = format!("Page 2 Item {n} Compensation Income should not be zero.");
            if self.regular_column_open() && row.regular == 0.0 {
                err(&format!("employers[{index}].regular"), &zero_message);
                continue;
            }
            if self.flat_column_open() && row.flat == 0.0 {
                err(&format!("employers[{index}].flat"), &zero_message);
            }
        }

        // checkYear and the Item 29 rules applied while typing.
        if year > today.year() {
            err(
                "taxable_year",
                "Invalid year input. Year should not be later to the Current Year.",
            );
        }
        if self.taxable_year != 0 && year < 2018 {
            err(
                "taxable_year",
                "Invalid year input. Year should not be lower than 2018.",
            );
        }
        if let Ok(date) = NaiveDate::parse_from_str(birth, "%m/%d/%Y")
            && date > today
        {
            err("birth_date", "Birth year should not be a future date.");
        }
        for (suffix, column) in [("A", &self.taxpayer), ("B", &self.spouse_column)] {
            let field = if suffix == "A" {
                "taxpayer"
            } else {
                "spouse_column"
            };
            if column.tax_due * 0.5 < column.second_installment {
                err(
                    &format!("{field}.second_installment"),
                    &format!(
                        "Amount in Item 29{suffix} cannot be more than 50% of Item 26{suffix}."
                    ),
                );
            } else if column.total_credits != 0.0
                && column.second_installment != 0.0
                && column.second_installment > fixed0(column.net_payable)
            {
                err(
                    &format!("{field}.second_installment"),
                    &format!("Amount in Item 29{suffix} cannot be greater than Item 28{suffix}."),
                );
            }
            for (name, value) in [
                ("non_taxable", column.non_taxable),
                ("other_income", column.other_income),
                ("flat_non_taxable", column.flat_non_taxable),
                ("flat_other_income", column.flat_other_income),
                ("previously_filed", column.previously_filed),
                ("foreign_tax_credits", column.foreign_tax_credits),
                ("other_credits", column.other_credits),
                ("second_installment", column.second_installment),
                ("interest", column.interest),
                ("surcharge", column.surcharge),
                ("compromise", column.compromise),
            ] {
                if value < 0.0 {
                    err(&format!("{field}.{name}"), "Enter a non-negative amount.");
                }
            }
        }
        for (index, row) in self.employers.iter().enumerate() {
            if row.for_spouse && !self.is_joint() {
                err(
                    &format!("employers[{index}].for_spouse"),
                    "A spouse employer row needs a joint return (Item 17).",
                );
            }
            if row.regular < 0.0 || row.flat < 0.0 || row.withheld < 0.0 {
                err(
                    &format!("employers[{index}]"),
                    "Enter a non-negative amount.",
                );
            }
        }
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }
        for (field, text, limit) in [
            ("citizenship", &self.citizenship, 20),
            ("foreign_tax_number", &self.foreign_tax_number, 20),
            (
                "other_income_description",
                &self.other_income_description,
                25,
            ),
            (
                "flat_non_taxable_description",
                &self.flat_non_taxable_description,
                25,
            ),
            (
                "flat_other_income_description",
                &self.flat_other_income_description,
                25,
            ),
            (
                "other_credits_description",
                &self.other_credits_description,
                25,
            ),
        ] {
            if text.chars().count() > limit {
                err(field, &format!("At most {limit} characters."));
            }
        }
        if self.number_of_attachments > 99 {
            err("number_of_attachments", "Item 37 holds at most two digits.");
        }

        // Derived items must be what the official compute chain produces.
        let mut expected = self.clone();
        expected.recompute();
        if expected != *self {
            err(
                "aggregate_amount_payable",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }
}

impl FormValidator for Form1700Draft {
    /// `validate()` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1700Draft {
    /// The penalty items of each column on the shared penalty model
    /// ([`super::penalty_items`]): the taxpayer/filer's, then the spouse's,
    /// which apply only to a joint return.
    pub fn penalty_declarations() -> Vec<PenaltyItems<Self>> {
        let filer = super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                taxpayer.surcharge: "32 Surcharge",
                taxpayer.interest: "31 Interest",
                taxpayer.compromise: "33 Compromise",
            ],
            controls: [
                "frm1700:txtPg1I32A",
                "frm1700:txtPg1I31A",
                "frm1700:txtPg1I33A",
            ],
            state: taxpayer.penalty,
            basic_tax_due: "Item 30A", |d| d.taxpayer.amount_payable,
            party: Some("Taxpayer/Filer"),
            rounding: super::penalty_items::PenaltyRounding::Cents,
        );
        let spouse = super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                spouse_column.surcharge: "32 Surcharge",
                spouse_column.interest: "31 Interest",
                spouse_column.compromise: "33 Compromise",
            ],
            controls: [
                "frm1700:txtPg1I32B",
                "frm1700:txtPg1I31B",
                "frm1700:txtPg1I33B",
            ],
            state: spouse_column.penalty,
            basic_tax_due: "Item 30B", |d| d.spouse_column.amount_payable,
            party: Some("Spouse"),
            applies: |d: &Self| d.is_joint(),
            // The profile's EOPT class is the filer's, not the spouse's.
            rounding: super::penalty_items::PenaltyRounding::Cents,
        );
        vec![filer, spouse]
    }
}

impl QueueableForm for Form1700Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        Self::penalty_declarations()
    }
    const FORM_CODE: &'static str = "1700";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1700v2018']`).
    const FORM_TYPE: &'static str = "1700v2018";
    const LAYOUT_ID: &'static str = FORM_1700_FORM_ID;

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
    /// `txtPg1I1Year`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        self.taxable_year.to_string()
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        (code.len() == 4 && code.bytes().all(|b| b.is_ascii_digit()))
            .then(|| code.parse().ok())
            .flatten()
            .map(|year| (year, FilingPeriod::Annual))
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
    /// The add-more popup adds rows at run time, so the plaintext follows
    /// [`Form1700Draft::official_layout`] rather than the fixed layout.
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
        NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()
    }

    pub(crate) fn sample() -> Form1700Draft {
        let mut d = Form1700Draft {
            id: None,
            tin: "12345678800000".to_string(),
            taxable_year: 2025,
            is_amended: false,
            rdo_code: "039".to_string(),
            taxpayer_type: Form1700TaxpayerType::Employee,
            taxpayer_name: "Dummy, Sample Taxpayer".to_string(),
            registered_address: "123 Sample Street, Barangay Example, Quezon City".to_string(),
            zip_code: "1100".to_string(),
            birth_date: "01/15/1980".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            citizenship: "Filipino".to_string(),
            foreign_tax_credits: Some(false),
            foreign_tax_number: String::new(),
            contact_number: "09170000000".to_string(),
            civil_status: Form1700CivilStatus::Single,
            spouse_has_income: None,
            filing_status: Form1700FilingStatus::Unanswered,
            line_of_business: "Sample Consulting Services".to_string(),
            spouse: Form1700Spouse::default(),
            taxpayer: Form1700Column {
                non_taxable: 90_000.0,
                other_income: 10_000.49,
                other_credits: 100.0,
                surcharge: 1_000.0,
                ..Form1700Column::default()
            },
            spouse_column: Form1700Column::default(),
            other_income_description: "Prize".to_string(),
            flat_non_taxable_description: String::new(),
            flat_other_income_description: String::new(),
            other_credits_description: "Sample credit".to_string(),
            employers: vec![Form1700Employer {
                for_spouse: false,
                name: "Sample Employer Inc".to_string(),
                name2: String::new(),
                tin: "12345678800000".to_string(),
                regular: 600_000.5,
                flat: 0.0,
                withheld: 40_000.0,
            }],
            schedule_totals: [[0.0; 3]; 2],
            aggregate_amount_payable: 0.0,
            number_of_attachments: 0,
            lifecycle: SubmissionLifecycle::default(),
        };
        // This sample enters its penalties by hand.
        d.keep_entered_penalties();
        d.recompute();
        d
    }

    fn messages(d: &Form1700Draft) -> Vec<String> {
        d.validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let d = sample();
        let tp = &d.taxpayer;
        assert_eq!(d.schedule_totals[0], [600_001.0, 0.0, 40_000.0]);
        assert_eq!(tp.gross_compensation, 600_001.0);
        assert_eq!(tp.taxable_compensation, 510_001.0);
        assert_eq!(tp.taxable_income, 520_001.0);
        assert_eq!(tp.graduated_tax_due, 46_500.0);
        assert_eq!(tp.total_credits, 40_100.0);
        assert_eq!(tp.net_payable, 6_400.0);
        assert_eq!(tp.total_amount_payable, 7_400.0);
        assert_eq!(d.aggregate_amount_payable, 7_400.0);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let d = sample();
        let f = d.to_bir_field_map();
        assert_eq!(f["frm1700:txtPg1I7TaxpayerName"], "DUMMY, SAMPLE TAXPAYER");
        assert_eq!(f["frm1700:txtPg2TaxpayerName"], "DUMMY");
        assert_eq!(f["frm1700:txtPg1I10Email"], "SAMPLE.TAXPAYER@EXAMPLE.COM");
        assert_eq!(f["frm1700:txtPg1I11Citizenship"], "FILIPINO");
        assert_eq!(f["frm1700:txtPg2I45Desc"], "PRIZE");
        assert_eq!(
            f["frm1700:txtPg2I1PartVIEmployerName1"],
            "SAMPLE EMPLOYER INC"
        );
        assert_eq!(f["frm1700:txtPg1I18SBranchCode"], "");
        assert_eq!(f["frm1700:txtSpouseRDOCode"], "000");
        assert_eq!(f["frm1700:txtPg1I37NumberOfAttachments"], "00");
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1700v2018-2025#sample.taxpayer@example.com#.xml"
        );
        assert!(d.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let d = sample();
        assert_eq!(d.period_code(), "2025");
        assert_eq!(
            Form1700Draft::parse_period_code("2025"),
            Some((2025, FilingPeriod::Annual))
        );
        assert_eq!(Form1700Draft::parse_period_code("122025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1700Draft), expected: &str| {
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
            "Please enter a valid year in Item 1.",
        );
        check(
            &|d| d.tin = "1".into(),
            "Please enter a valid TIN number on Item 4.",
        );
        check(
            &|d| d.rdo_code.clear(),
            "Please enter a valid RDO Code on Item 5.",
        );
        check(
            &|d| {
                d.taxpayer_type = Form1700TaxpayerType::Unanswered;
                d.recompute();
            },
            "Please select Taxpayer Type on Item no. 6",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 7.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 8.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Registered Address on Item 8A.",
        );
        check(
            &|d| d.birth_date.clear(),
            "Please indicate Birth Date of Taxpayer on item 9.",
        );
        check(
            &|d| d.birth_date = "1/15/1980".into(),
            "Invalid birth date on item 9 of Taxpayer.  Please check date format.",
        );
        check(
            &|d| d.citizenship.clear(),
            "Please enter Taxpayer's Citizenship on Item 11",
        );
        check(
            &|d| {
                d.civil_status = Form1700CivilStatus::Unanswered;
                d.recompute();
            },
            "Please indicate Civil Status on Item 15",
        );
        check(
            &|d| {
                d.civil_status = Form1700CivilStatus::Married;
                d.recompute();
            },
            "Please choose Yes or No on Item 16",
        );
        check(
            &|d| {
                d.civil_status = Form1700CivilStatus::Married;
                d.spouse_has_income = Some(true);
                d.recompute();
            },
            "Please choose Yes or No on Item 17",
        );
        let joint = |d: &mut Form1700Draft| {
            d.civil_status = Form1700CivilStatus::Married;
            d.spouse_has_income = Some(true);
            d.filing_status = Form1700FilingStatus::Joint;
            d.spouse = Form1700Spouse {
                tin: "12345678800000".into(),
                rdo_code: "039".into(),
                taxpayer_type: Form1700TaxpayerType::Employee,
                name: "Dummy, Sample Spouse".into(),
                ..Form1700Spouse::default()
            };
        };
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN on Item 4.",
        );
        check(
            &|d| {
                joint(d);
                d.spouse.tin.clear();
                d.recompute();
            },
            "Please enter Spouse TIN",
        );
        check(
            &|d| {
                joint(d);
                d.spouse.tin = "12345678900000".into();
                d.recompute();
            },
            "You have entered an incorrect TIN on Item 18.",
        );
        check(
            &|d| {
                joint(d);
                d.spouse.rdo_code.clear();
                d.recompute();
            },
            "Please enter a valid RDO Code on Item 19.",
        );
        check(
            &|d| {
                joint(d);
                d.spouse.name.clear();
                d.recompute();
            },
            "Please enter Spouse Name on Item 21.",
        );
        check(
            &|d| {
                d.employers[0].name.clear();
                d.recompute();
            },
            "Please enter Employer's name on Part VI Item 1A ",
        );
        check(
            &|d| {
                d.employers[0].tin.clear();
                d.recompute();
            },
            "Please enter Employer's TIN on Part VI Item 1B",
        );
        check(
            &|d| {
                d.employers[0].tin = "123456788".into();
                d.recompute();
            },
            "Please enter valid TIN on Part VI Item 1B",
        );
        check(
            &|d| {
                d.employers[0].tin = "12345678900000".into();
                d.recompute();
            },
            "You have entered an incorrect TIN Part VI Item 1B.",
        );
        check(
            &|d| {
                d.employers[0].regular = 0.0;
                d.recompute();
            },
            "Page 2 Item 1 Compensation Income should not be zero.",
        );
        check(
            &|d| d.taxable_year = 2027,
            "Invalid year input. Year should not be later to the Current Year.",
        );
        check(
            &|d| {
                d.taxpayer.second_installment = 30_000.0;
                d.recompute();
            },
            "Amount in Item 29A cannot be more than 50% of Item 26A.",
        );
    }

    #[test]
    fn flat_rate_column() {
        let mut d = sample();
        d.taxpayer_type = Form1700TaxpayerType::Nranetb;
        d.employers[0].regular = 0.0;
        d.employers[0].flat = 400_000.0;
        d.employers[0].withheld = 90_000.0;
        d.recompute();
        let tp = &d.taxpayer;
        assert_eq!(tp.non_taxable, 0.0);
        assert_eq!(tp.flat_gross_compensation, 400_000.0);
        assert_eq!(tp.flat_tax_due, 100_000.0);
        assert_eq!(tp.tax_due, 100_000.0);
        assert!(d.other_income_description.is_empty());
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn joint_returns_add_the_spouse_column() {
        let mut d = sample();
        d.civil_status = Form1700CivilStatus::Married;
        d.spouse_has_income = Some(true);
        d.filing_status = Form1700FilingStatus::Joint;
        d.spouse = Form1700Spouse {
            tin: "12345678800000".into(),
            rdo_code: "039".into(),
            taxpayer_type: Form1700TaxpayerType::Employee,
            name: "Dummy, Sample Spouse".into(),
            ..Form1700Spouse::default()
        };
        d.employers.push(Form1700Employer {
            for_spouse: true,
            name: "Sample Spouse Employer".into(),
            tin: "12345678800000".into(),
            regular: 400_000.0,
            withheld: 20_000.0,
            ..Form1700Employer::default()
        });
        d.recompute();
        let sp = &d.spouse_column;
        assert_eq!(d.schedule_totals[1], [400_000.0, 0.0, 20_000.0]);
        assert_eq!(sp.graduated_tax_due, 22_500.0);
        assert_eq!(sp.net_payable, 2_500.0);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
        d.filing_status = Form1700FilingStatus::Separate;
        d.recompute();
        assert_eq!(d.employers.len(), 1);
        assert_eq!(d.spouse_column, Form1700Column::default());
    }

    fn employer(name: &str, tin: &str, regular: f64, withheld: f64) -> Form1700Employer {
        Form1700Employer {
            for_spouse: false,
            name: name.to_string(),
            name2: String::new(),
            tin: tin.to_string(),
            regular,
            flat: 0.0,
            withheld,
        }
    }

    /// Rows 1 and 4 on the page, then two more through the popup: Item 4 reads
    /// OTHERS with the popup subtotal (values from the official page).
    fn folded() -> Form1700Draft {
        let mut d = sample();
        d.employers.resize_with(3, Form1700Employer::default);
        d.employers.push(employer(
            "Fourth Employer Corp",
            "12345678800000",
            100_000.25,
            5_000.0,
        ));
        d.employers.push(employer(
            "Fifth Employer Corp",
            "12345678800000",
            50_000.4,
            2_500.1,
        ));
        d.employers.push(employer(
            "Sixth Employer",
            "12345678800001",
            25_000.0,
            1_000.0,
        ));
        d.recompute();
        d
    }

    #[test]
    fn add_more_popup_folds_item_4_into_others() {
        let d = folded();
        assert!(d.schedule_is_folded());
        assert_eq!(d.popup_rows().len(), 3);
        assert_eq!(d.popup_subtotals(), [175_000.65, 0.0, 8_500.1]);
        assert_eq!(d.schedule_totals[0], [775_001.0, 0.0, 48_500.0]);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
        let f = d.to_bir_field_map();
        for (key, value) in [
            ("rdoPg2I4PartVIEmployeeT", "true"),
            ("txtPg2I4PartVIEmployerName1", "OTHERS"),
            ("txtPg2I4PartVIEmployerTIN1", ""),
            ("txtPg2I4PartVIEmployerBranchCode", ""),
            ("txtPg2ISched1c_4REG", "175,000.65"),
            ("txtPg2ISched1d_4CIFR", "0.00"),
            ("txtPg2ISched1e_4TW", "8,500.10"),
            ("txtPg2Pt6I4SubtotalC", "175,000.65"),
            ("txtPg2Pt6I4SubtotalD", "0.00"),
            ("txtPg2Pt6I4SubtotalE", "8,500.10"),
            ("txtPg2ISched1c_5AREG", "775,001.00"),
            ("txtPg2ISched1e_5ATW", "48,500.00"),
            ("txtPg2I42A", "775,001.00"),
            ("txtPg2I47A", "81,500.00"),
            ("txtPg2I59A", "32,900.00"),
        ] {
            assert_eq!(f[&format!("frm1700:{key}")], value, "{key}");
        }
        // Not folded: the popup subtotals keep the page default.
        assert!(
            !sample()
                .to_bir_field_map()
                .contains_key("frm1700:txtPg2Pt6I4SubtotalC")
        );
    }

    #[test]
    fn popup_rows_carry_their_own_alerts() {
        let mut d = folded();
        d.employers[5].tin = String::new();
        d.employers[4].regular = 0.0;
        d.recompute();
        let m = messages(&d);
        assert!(m.contains(&"Please enter Employer's TIN on Part VI Item 4_3B".to_string()));
        assert!(m.contains(&"Page 2 Item 4_2 Compensation Income should not be zero.".to_string()));
        let mut d = folded();
        d.employers[3].name.clear();
        d.recompute();
        assert!(
            messages(&d)
                .contains(&"Please enter Employer's name on Part VI Item 4_1A ".to_string())
        );
    }

    #[test]
    fn print_map_derives_whole_peso_combs() {
        let d = folded();
        let p = d.to_print_field_map();
        // 26A: graduated tax due; 32 Interest / 31 Surcharge follow the sheet.
        assert_eq!(p["derived:amount_42A"], "  775001");
        assert_eq!(p["derived:amount_31A"], "    1000");
        assert!(!p.contains_key("derived:amount_32A"));
        assert!(!p.contains_key("derived:amount_42B"));
        assert_eq!(p["derived:amount_sched1_1c"], "  600001");
        assert_eq!(p["derived:amount_sched1_4c"], "  175001");
        assert_eq!(p["derived:amount_sched1_4e"], "   8500");
        assert_eq!(p["derived:amount_sched1_5Ae"], "  48500");
        assert_eq!(p["derived:employer1_tin"], "12345678800000");
        assert!(!p.contains_key("derived:employer4_tin"));
        assert_eq!(p["derived:tin_digits"], "123456788");
        assert_eq!(p["derived:birth_yyyy"], "1980");
        assert_eq!(p["frm1700:txtPg2I4PartVIEmployerName1"], "OTHERS");
        assert!(!p.contains_key("derived:spouse_rdo"));
        let fields = d.to_bir_field_map();
        assert!(fields.keys().all(|key| !key.starts_with("derived:")));
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut d = sample();
        d.queue(crate::filing_queue::QueueAuthSource::Gui).unwrap();
        assert!(d.revalidate_queued_before_submission().is_ok());
        d.taxpayer.surcharge = 99.0;
        d.keep_entered_penalties();
        assert!(d.revalidate_queued_before_submission().is_err());
    }
}
