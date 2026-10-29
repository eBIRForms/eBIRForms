//! BIR Form 2553 — Return of Percentage Tax Payable Under Special Laws.
//!
//! Ported from the official `BIR-Form2553.hta` (eBIRForms 7.9.6.2.1):
//! the compute chain (`computeTaxDue` … `computeTotalAmountPayable`), the
//! ATC popup (`getATCCode`), `validateForm` with its exact alert texts, and
//! `saveXMLsubmit` through [`crate::official_xml`]. The first form on the
//! generic [`QueueableForm`] submission path.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::form_2551q::TaxPeriodBasis;
use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_2553_FORM_ID: &str = "2553-v1999";
/// ATC rows 14–18 on the official form.
pub const FORM_2553_ATC_ROWS: usize = 5;
const FIRST_ATC_ITEM: usize = 14;

/// One entry of the official ATC popup (`atcCodes.xml` lines tagged `2553`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Form2553AtcOption {
    pub code: &'static str,
    pub description: &'static str,
    /// Rate text the popup copies into column D, e.g. `"5.0"`.
    pub rate_text: &'static str,
    pub rate: f64,
    /// `OT012` leaves the rate to the filer (`getATCCode` enables column D).
    pub rate_is_editable: bool,
}

/// The popup lists these in `atcCodes.xml` order. OT011 appears twice with
/// different descriptions; `getATCCode` treats code + description as the key.
pub const FORM_2553_ATC_OPTIONS: &[Form2553AtcOption] = &[
    Form2553AtcOption {
        code: "OT010",
        description: "PAGCOR",
        rate_text: "5.0",
        rate: 5.0,
        rate_is_editable: false,
    },
    Form2553AtcOption {
        code: "OT012",
        description: "OTHERS",
        rate_text: "5.0",
        rate: 5.0,
        rate_is_editable: true,
    },
    Form2553AtcOption {
        code: "OT011",
        description: "CLARK DEVELOPMENT CORPORATIONS",
        rate_text: "5.0",
        rate: 5.0,
        rate_is_editable: false,
    },
    Form2553AtcOption {
        code: "OT011",
        description: "SPECIAL/REGULAR/ECONOMIC FREE PORT ZONE ENTERPRISES",
        rate_text: "5.0",
        rate: 5.0,
        rate_is_editable: false,
    },
];

/// The official option for an ATC code and description.
pub fn form_2553_atc_option(code: &str, description: &str) -> Option<&'static Form2553AtcOption> {
    FORM_2553_ATC_OPTIONS
        .iter()
        .find(|option| option.code == code && option.description == description)
}

/// What validation says while item 13 is Yes and its list is unanswered
/// ([`Form2553TaxTreaty::YesUnspecified`]), the official alert.
pub const TAX_TREATY_UNCHOSEN: &str = "Please select a Tax Treaty from the list.";

/// Item 13 (`optTreaty` + `lstTaxTreaty`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2553TaxTreaty {
    /// Item 13 "No".
    #[default]
    No,
    /// Item 13 "Yes" with nothing picked from the list yet.
    YesUnspecified,
    SpecialRate,
    InternationalTaxTreaty,
}

impl Form2553TaxTreaty {
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

/// Item 23 overpayment boxes (`ifoverpay_1` / `ifoverpay_2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2553Overpayment {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
}

/// One of rows 14–18: A description, B ATC, C taxable amount, D rate, E due.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2553AtcRow {
    pub atc_code: String,
    pub description: String,
    pub taxable_amount: f64,
    /// Percent, e.g. `5.0`.
    pub tax_rate: f64,
    pub tax_due: f64,
}

impl Form2553AtcRow {
    /// A row filled the way `getATCCode` fills it from the popup.
    pub fn from_option(option: &Form2553AtcOption) -> Self {
        Self {
            atc_code: option.code.to_string(),
            description: option.description.to_string(),
            taxable_amount: 0.0,
            tax_rate: if option.rate_is_editable {
                0.0
            } else {
                option.rate
            },
            tax_due: 0.0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.atc_code.is_empty()
    }

    fn option(&self) -> Option<&'static Form2553AtcOption> {
        form_2553_atc_option(&self.atc_code, &self.description)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2553Draft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–5
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub tax_period_basis: TaxPeriodBasis,
    /// Item 2 month (1–12). Calendar filers use 12.
    pub year_end_month: u8,
    /// Item 2 year.
    pub taxable_year: u16,
    /// Item 3.
    pub quarter: u8,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
    pub rdo_code: String,
    pub line_of_business: String,
    pub taxpayer_name: String,
    pub contact_number: String,
    pub registered_address: String,
    pub zip_code: String,
    pub email: String,
    #[serde(default)]
    pub tax_treaty: Form2553TaxTreaty,

    // Part II
    /// Rows 14–18 in order; missing rows are blank.
    #[serde(default)]
    pub schedule: Vec<Form2553AtcRow>,
    /// Item 19.
    #[serde(default)]
    pub total_tax_due: f64,
    /// Item 20A, only on an amended return.
    #[serde(default)]
    pub tax_paid_previous: f64,
    /// Item 20B.
    #[serde(default)]
    pub creditable_tax_withheld: f64,
    /// Item 20C.
    #[serde(default)]
    pub total_tax_credits: f64,
    /// Item 21.
    #[serde(default)]
    pub tax_payable: f64,
    /// Items 22A–22D.
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
    /// Item 23.
    #[serde(default)]
    pub total_amount_payable: f64,
    #[serde(default)]
    pub overpayment: Form2553Overpayment,

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

impl Form2553Draft {
    pub const FORM_CODE: &'static str = "2553";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            tin: profile.tin.full(),
            tax_period_basis: TaxPeriodBasis::Calendar,
            year_end_month: 12,
            taxable_year: year,
            quarter,
            is_amended: false,
            number_of_attached_sheets: 0,
            rdo_code: profile.rdo_code.clone(),
            line_of_business: profile.line_of_business.clone(),
            taxpayer_name: profile.full_name.clone(),
            contact_number: profile.phone.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            email: profile.email.clone(),
            tax_treaty: Form2553TaxTreaty::No,
            schedule: Vec::new(),
            total_tax_due: 0.0,
            tax_paid_previous: 0.0,
            creditable_tax_withheld: 0.0,
            total_tax_credits: 0.0,
            tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            overpayment: Form2553Overpayment::None,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 1 radio click, like the official `dateyear()`: Calendar locks
    /// Item 2 to December; Fiscal with December clears the month and returns
    /// the official alert.
    pub fn set_tax_period_basis(&mut self, basis: TaxPeriodBasis) -> Option<&'static str> {
        self.tax_period_basis = basis;
        match basis {
            TaxPeriodBasis::Calendar => {
                self.year_end_month = 12;
                None
            }
            TaxPeriodBasis::Fiscal if self.year_end_month == 12 => {
                self.year_end_month = 0;
                Some("You have entered invalid month for Fiscal Year")
            }
            TaxPeriodBasis::Fiscal => None,
        }
    }

    /// Item 2 month change, like the official `datemonth()`: the basis
    /// follows the month and the official notice is returned.
    pub fn set_year_end_month(&mut self, month: u8) -> Option<&'static str> {
        self.year_end_month = month;
        match self.tax_period_basis {
            TaxPeriodBasis::Calendar if month != 12 => {
                self.tax_period_basis = TaxPeriodBasis::Fiscal;
                Some(
                    "You have entered a filing year not ending in December. This filing will be considered as a Fiscal Year Filing.",
                )
            }
            TaxPeriodBasis::Fiscal if month == 12 => {
                self.tax_period_basis = TaxPeriodBasis::Calendar;
                Some(
                    "You have entered a filing year ending in December. This filing will be considered as a Calendar Year Filing.",
                )
            }
            _ => None,
        }
    }

    /// Pick an ATC for a row from the official popup. Rejects a code and
    /// description already on another row, like `getATCCode`.
    pub fn set_atc(&mut self, row: usize, option: &Form2553AtcOption) -> Result<(), String> {
        if row >= FORM_2553_ATC_ROWS {
            return Err(format!("Form 2553 has {FORM_2553_ATC_ROWS} ATC rows."));
        }
        let duplicate = self.schedule.iter().enumerate().any(|(index, other)| {
            index != row && other.atc_code == option.code && other.description == option.description
        });
        if duplicate {
            return Err("Invalid input. Selected ATC already defined.".to_string());
        }
        if self.schedule.len() <= row {
            self.schedule.resize_with(row + 1, Form2553AtcRow::default);
        }
        let amount = self.schedule[row].taxable_amount;
        self.schedule[row] = Form2553AtcRow::from_option(option);
        self.schedule[row].taxable_amount = amount;
        self.recompute();
        Ok(())
    }

    /// Clear a row back to the blank official state.
    pub fn clear_atc(&mut self, row: usize) {
        if let Some(entry) = self.schedule.get_mut(row) {
            *entry = Form2553AtcRow::default();
        }
        while self.schedule.last().is_some_and(Form2553AtcRow::is_empty) {
            self.schedule.pop();
        }
        self.recompute();
    }

    /// The official compute chain. Every input is held at cents the way
    /// `round(this,2)` leaves it, and each derived item is the formatted
    /// value of the items it reads.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_paid_previous = 0.0;
        }
        let mut total = 0.0;
        for row in &mut self.schedule {
            row.taxable_amount = cents(row.taxable_amount);
            row.tax_due = cents(row.taxable_amount / 100.0 * row.tax_rate);
            total += row.tax_due;
        }
        self.total_tax_due = cents(total);
        self.tax_paid_previous = cents(self.tax_paid_previous);
        self.creditable_tax_withheld = cents(self.creditable_tax_withheld);
        self.total_tax_credits = cents(self.tax_paid_previous + self.creditable_tax_withheld);
        self.tax_payable = cents(self.total_tax_due - self.total_tax_credits);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_payable = cents(self.tax_payable + self.total_penalties);
        // checkOverpayment clears both boxes whenever Item 23 is recomputed;
        // they only stay meaningful while there is an overpayment.
        if self.total_amount_payable >= 0.0 {
            self.overpayment = Form2553Overpayment::None;
        }
    }

    fn row(&self, index: usize) -> Form2553AtcRow {
        self.schedule.get(index).cloned().unwrap_or_default()
    }

    fn rate_text(row: &Form2553AtcRow) -> String {
        match row.option() {
            Some(option) if !option.rate_is_editable => option.rate_text.to_string(),
            _ if row.is_empty() => "0.00".to_string(),
            _ => format!("{:.2}", row.tax_rate),
        }
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:line:tax_relief_specify`: "If yes, specify", the relief
    ///   named by `frm2553:lstTaxTreaty` (the upload carries the dropdown code).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let specify = super::print_map::tax_relief_label(
            fields
                .get("frm2553:lstTaxTreaty")
                .map_or("", String::as_str),
        )
        .to_string();
        fields.insert("derived:line:tax_relief_specify".to_string(), specify);
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2553:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        // capital() uppercases every text input except txtEmail.
        let text = |value: &str| value.trim().to_uppercase();

        let calendar = self.tax_period_basis == TaxPeriodBasis::Calendar;
        put("itemFiscalStartMonth:_1", flag(calendar));
        put("itemFiscalStartMonth:_2", flag(!calendar));
        put("itemYearEndMonth", format!("{:02}", self.year_end_month));
        put("txtYearEnded", self.taxable_year.to_string());
        for quarter in 1..=4u8 {
            put(&format!("optQtr:_{quarter}"), flag(self.quarter == quarter));
        }
        put("optAmended_1", flag(self.is_amended));
        put("optAmended_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtTIN1", tin1);
        put("txtTIN2", tin2);
        put("txtTIN3", tin3);
        put("txtBranchCode", branch);
        put("txtRDOCode", self.rdo_code.trim().to_string());
        put("txtDescription", text(&self.line_of_business));
        put("txtTPName", text(&self.taxpayer_name));
        put("txtTelNum", self.contact_number.trim().to_string());
        put("txtAddress", text(&self.registered_address));
        put("txtZipCode", self.zip_code.trim().to_string());
        put("optTreaty_1", flag(self.tax_treaty.is_yes()));
        put("optTreaty_2", flag(!self.tax_treaty.is_yes()));
        put("lstTaxTreaty", self.tax_treaty.list_value().to_string());

        for index in 0..FORM_2553_ATC_ROWS {
            let row = self.row(index);
            let item = FIRST_ATC_ITEM + index;
            put(&format!("txt{item}A"), text(&row.description));
            put(&format!("txt{item}B"), text(&row.atc_code));
            put(&format!("txt{item}C"), official_amount(row.taxable_amount));
            put(&format!("txt{item}D"), Self::rate_text(&row));
            put(&format!("txt{item}E"), official_amount(row.tax_due));
        }

        put("txt19", official_amount(self.total_tax_due));
        put("txt20A", official_amount(self.tax_paid_previous));
        put("txt20B", official_amount(self.creditable_tax_withheld));
        put("txt20C", official_amount(self.total_tax_credits));
        put("txt21", official_amount(self.tax_payable));
        put("txt22A", official_amount(self.surcharge));
        put("txt22B", official_amount(self.interest));
        put("txt22C", official_amount(self.compromise));
        put("txt22D", official_amount(self.total_penalties));
        put("txt23", official_amount(self.total_amount_payable));
        put(
            "ifoverpay_1",
            flag(self.overpayment == Form2553Overpayment::Refund),
        );
        put(
            "ifoverpay_2",
            flag(self.overpayment == Form2553Overpayment::TaxCreditCertificate),
        );

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

impl FormValidator for Form2553Draft {
    /// `validateForm` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing (`maxlength`, `wholenumber`,
    /// `numbersonly`, the ATC popup and `dateyear` / `datemonth`).
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // Item 1 is always answered; Item 2 must agree with it.
        if !(1..=12).contains(&self.year_end_month) {
            err("year_end_month", "Please select valid month on item 2.");
        } else if self.tax_period_basis == TaxPeriodBasis::Calendar && self.year_end_month != 12 {
            err(
                "year_end_month",
                "You have entered a filing year not ending in December. This filing will be considered as a Fiscal Year Filing.",
            );
        } else if self.tax_period_basis == TaxPeriodBasis::Fiscal && self.year_end_month == 12 {
            err(
                "year_end_month",
                "You have entered invalid month for Fiscal Year",
            );
        }
        if self.taxable_year == 0 {
            err("taxable_year", "Please enter valid year on item 2.");
        } else if self.taxable_year < 1900 {
            // The official future-year check (2553-defect-023) is commented
            // out in validateForm, so later years pass as they do there.
            err(
                "taxable_year",
                "Invalid date entry on Item no.2. Entry should not be lower than 1900.",
            );
        }
        if !(1..=4).contains(&self.quarter) {
            err("quarter", "Select a Quarter on Item no. 3.");
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 5 holds at most two digits.",
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
            err("tin", "Please enter a valid TIN number on Item 6.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }

        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 7.");
        }
        if self.line_of_business.trim().is_empty() {
            err(
                "line_of_business",
                "Please enter a valid Line of Business/Occupation on Item 8.",
            );
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 70 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 9.",
            );
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 20 || !digits_only(phone) {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 10.",
            );
        }
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > 70 {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 11.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 12.");
        }

        // The official check reads the formatted Item 23 text, so it only
        // fires below -999.99 by accident; require a box for any overpayment.
        if self.total_amount_payable < 0.0 && self.overpayment == Form2553Overpayment::None {
            err(
                "overpayment",
                "Please indicate refund type for overpayment in Item 23.",
            );
        }

        if self.schedule.len() > FORM_2553_ATC_ROWS {
            err("schedule", "Form 2553 has 5 ATC rows (Items 14 to 18).");
        }
        let mut any_atc = false;
        for (index, row) in self.schedule.iter().enumerate().take(FORM_2553_ATC_ROWS) {
            let item = FIRST_ATC_ITEM + index;
            if row.is_empty() {
                if row.taxable_amount != 0.0 {
                    err(
                        &format!("schedule[{index}].atc_code"),
                        &format!("Select an ATC for Item {item}B before entering an amount."),
                    );
                }
                continue;
            }
            any_atc = true;
            let Some(option) = row.option() else {
                err(
                    &format!("schedule[{index}].atc_code"),
                    &format!("Item {item}B is not an ATC the official form offers."),
                );
                continue;
            };
            if self.schedule[..index]
                .iter()
                .any(|other| other.atc_code == row.atc_code && other.description == row.description)
            {
                err(
                    &format!("schedule[{index}].atc_code"),
                    "Invalid input. Selected ATC already defined.",
                );
            }
            if row.taxable_amount <= 0.0 || !has_cent_precision(row.taxable_amount) {
                err(
                    &format!("schedule[{index}].taxable_amount"),
                    &format!("Please enter a valid amount for Item {item}C."),
                );
            }
            if option.rate_is_editable {
                if !(row.tax_rate > 0.0 && row.tax_rate < 100.0)
                    || !has_cent_precision(row.tax_rate)
                {
                    err(
                        &format!("schedule[{index}].tax_rate"),
                        &format!("Enter the tax rate for Item {item}D (above 0 and below 100)."),
                    );
                }
            } else if (row.tax_rate - option.rate).abs() > 1e-9 {
                err(
                    &format!("schedule[{index}].tax_rate"),
                    &format!(
                        "Item {item}D must be the official rate {}%.",
                        option.rate_text
                    ),
                );
            }
        }
        if !any_atc {
            err("schedule", "Select at least one ATC in Items 14 to 18.");
        }

        if self.tax_treaty == Form2553TaxTreaty::YesUnspecified {
            err("tax_treaty", TAX_TREATY_UNCHOSEN);
        }

        for (field, value) in [
            ("tax_paid_previous", self.tax_paid_previous),
            ("creditable_tax_withheld", self.creditable_tax_withheld),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ] {
            if value < 0.0 || !has_cent_precision(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }
        if !self.is_amended && self.tax_paid_previous != 0.0 {
            err(
                "tax_paid_previous",
                "Item 20A applies only to an amended return.",
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
        if expected.total_amount_payable != self.total_amount_payable
            || expected.total_tax_due != self.total_tax_due
            || expected.schedule != self.schedule
        {
            err(
                "total_amount_payable",
                "Totals are out of date. Recompute the return.",
            );
        }

        errors
    }
}

impl Form2553Draft {
    /// Items 22A-22C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "22A Surcharge",
                interest: "22B Interest",
                compromise: "22C Compromise",
            ],
            controls: [
                "frm2553:txt22A",
                "frm2553:txt22B",
                "frm2553:txt22C",
            ],
            state: penalty,
            basic_tax_due: "Item 21", |d| d.tax_payable,
        )
    }
}

impl QueueableForm for Form2553Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2553";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['2553']`).
    const FORM_TYPE: &'static str = "2553";
    const LAYOUT_ID: &'static str = FORM_2553_FORM_ID;

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
        FilingPeriod::Quarterly(self.quarter)
    }
    /// `itemYearEndMonth + txtYearEnded + "Q" + n`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!(
            "{:02}{}Q{}",
            self.year_end_month, self.taxable_year, self.quarter
        )
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 8 || code.get(6..7)? != "Q" {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let year: u16 = code.get(2..6)?.parse().ok()?;
        let quarter: u8 = code.get(7..)?.parse().ok()?;
        ((1..=12).contains(&month) && (1..=4).contains(&quarter))
            .then_some((year, FilingPeriod::Quarterly(quarter)))
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

    pub(crate) fn sample() -> Form2553Draft {
        let mut draft = Form2553Draft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            tin: "12345678800000".to_string(),
            tax_period_basis: TaxPeriodBasis::Calendar,
            year_end_month: 12,
            taxable_year: 2025,
            quarter: 1,
            is_amended: false,
            number_of_attached_sheets: 0,
            rdo_code: "039".to_string(),
            line_of_business: "Gaming operations".to_string(),
            taxpayer_name: "Sample Taxpayer Inc".to_string(),
            contact_number: "0281234567".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            tax_treaty: Form2553TaxTreaty::No,
            schedule: Vec::new(),
            total_tax_due: 0.0,
            tax_paid_previous: 0.0,
            creditable_tax_withheld: 0.0,
            total_tax_credits: 0.0,
            tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            overpayment: Form2553Overpayment::None,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.set_atc(0, &FORM_2553_ATC_OPTIONS[0]).unwrap();
        draft.schedule[0].taxable_amount = 123_456.789;
        draft.creditable_tax_withheld = 1_000.0;
        draft.surcharge = 25.5;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2553Draft) -> Vec<String> {
        <Form2553Draft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn item_2_year_follows_the_official_checks() {
        let mut draft = sample();
        draft.taxable_year = 1899;
        assert_eq!(
            messages(&draft),
            ["Invalid date entry on Item no.2. Entry should not be lower than 1900."]
        );
        // 2553-defect-023: the official future-year branch is commented out.
        draft.taxable_year = 9999;
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn fiscal_december_becomes_calendar_like_datemonth() {
        let mut draft = sample();
        draft.tax_period_basis = TaxPeriodBasis::Fiscal;
        draft.year_end_month = 6;
        assert_eq!(
            draft.set_year_end_month(12),
            Some(
                "You have entered a filing year ending in December. This filing will be considered as a Calendar Year Filing."
            )
        );
        assert_eq!(draft.tax_period_basis, TaxPeriodBasis::Calendar);
        assert_eq!(draft.year_end_month, 12);
        assert_eq!(
            draft.set_year_end_month(6),
            Some(
                "You have entered a filing year not ending in December. This filing will be considered as a Fiscal Year Filing."
            )
        );
        assert_eq!(draft.tax_period_basis, TaxPeriodBasis::Fiscal);
        assert_eq!(draft.set_year_end_month(3), None);
        // dateyear(): Fiscal with December clears the month, never June.
        draft.set_tax_period_basis(TaxPeriodBasis::Calendar);
        assert_eq!(
            draft.set_tax_period_basis(TaxPeriodBasis::Fiscal),
            Some("You have entered invalid month for Fiscal Year")
        );
        assert_eq!(draft.year_end_month, 0);
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.schedule[0].taxable_amount, 123_456.79);
        assert_eq!(draft.schedule[0].tax_due, 6_172.84);
        assert_eq!(draft.total_tax_due, 6_172.84);
        assert_eq!(draft.total_tax_credits, 1_000.0);
        assert_eq!(draft.tax_payable, 5_172.84);
        assert_eq!(draft.total_penalties, 25.5);
        assert_eq!(draft.total_amount_payable, 5_198.34);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        assert_eq!(fields["frm2553:txt14A"], "PAGCOR");
        assert_eq!(fields["frm2553:txt14B"], "OT010");
        assert_eq!(fields["frm2553:txt14C"], "123,456.79");
        assert_eq!(fields["frm2553:txt14D"], "5.0");
        assert_eq!(fields["frm2553:txt14E"], "6,172.84");
        assert_eq!(fields["frm2553:txt15D"], "0.00");
        assert_eq!(fields["frm2553:txt15A"], "");
        assert_eq!(fields["frm2553:itemYearEndMonth"], "12");
        assert_eq!(fields["frm2553:txtTPName"], "SAMPLE TAXPAYER INC");
        assert_eq!(fields["frm2553:txtBranchCode"], "00000");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        assert_eq!(fields["frm2553:lstTaxTreaty"], "0");
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2553-122025Q1#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "122025Q1");
        assert_eq!(
            Form2553Draft::parse_period_code("122025Q1"),
            Some((2025, FilingPeriod::Quarterly(1)))
        );
        assert_eq!(Form2553Draft::parse_period_code("132025Q1"), None);
        assert_eq!(Form2553Draft::parse_period_code("122025Q5"), None);
        assert_eq!(Form2553Draft::parse_period_code("122025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2553Draft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.year_end_month = 0,
            "Please select valid month on item 2.",
        );
        check(
            &|d| d.taxable_year = 0,
            "Please enter valid year on item 2.",
        );
        check(
            &|d| d.taxable_year = 1899,
            "Invalid date entry on Item no.2. Entry should not be lower than 1900.",
        );
        check(&|d| d.quarter = 0, "Select a Quarter on Item no. 3.");
        check(
            &|d| d.tin = "12345".into(),
            "Please enter a valid TIN number on Item 6.",
        );
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code = String::new(),
            "Please enter a valid RDO Code on Item 7.",
        );
        check(
            &|d| d.line_of_business = " ".into(),
            "Please enter a valid Line of Business/Occupation on Item 8.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 9.",
        );
        check(
            &|d| d.contact_number = "02-123".into(),
            "Please enter a valid Telephone Number on Item 10.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 11.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 12.",
        );
        check(
            &|d| {
                d.creditable_tax_withheld = 10_000.0;
                d.recompute();
            },
            "Please indicate refund type for overpayment in Item 23.",
        );
        check(
            &|d| {
                d.schedule[0].taxable_amount = 0.0;
                d.recompute();
            },
            "Please enter a valid amount for Item 14C.",
        );
        check(
            &|d| d.tax_treaty = Form2553TaxTreaty::YesUnspecified,
            "Please select a Tax Treaty from the list.",
        );
        check(
            &|d| {
                d.schedule.push(d.schedule[0].clone());
                d.recompute();
            },
            "Invalid input. Selected ATC already defined.",
        );
        check(
            &|d| {
                d.tax_period_basis = TaxPeriodBasis::Fiscal;
            },
            "You have entered invalid month for Fiscal Year",
        );
    }

    #[test]
    fn atc_popup_rules() {
        let mut draft = sample();
        assert_eq!(
            draft.set_atc(1, &FORM_2553_ATC_OPTIONS[0]),
            Err("Invalid input. Selected ATC already defined.".to_string())
        );
        // OT011 twice with different descriptions is allowed.
        draft.set_atc(1, &FORM_2553_ATC_OPTIONS[2]).unwrap();
        draft.set_atc(2, &FORM_2553_ATC_OPTIONS[3]).unwrap();
        // OT012 leaves the rate to the filer.
        draft.set_atc(3, &FORM_2553_ATC_OPTIONS[1]).unwrap();
        assert_eq!(draft.schedule[3].tax_rate, 0.0);
        draft.schedule[3].taxable_amount = 1_000.0;
        assert!(
            messages(&draft)
                .iter()
                .any(|m| m == "Enter the tax rate for Item 17D (above 0 and below 100).")
        );
        draft.schedule[3].tax_rate = 3.0;
        draft.schedule[1].taxable_amount = 10.0;
        draft.schedule[2].taxable_amount = 10.0;
        draft.recompute();
        assert_eq!(draft.schedule[3].tax_due, 30.0);
        assert_eq!(draft.to_bir_field_map()["frm2553:txt17D"], "3.00");
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        draft.clear_atc(3);
        assert_eq!(draft.schedule.len(), 3);
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
            <Form2553Draft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2553Draft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
