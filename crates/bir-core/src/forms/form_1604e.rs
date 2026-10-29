//! BIR Form 1604-E — Annual Information Return of Creditable Income Taxes
//! Withheld (Expanded)/Income Payments Exempt from Withholding Tax
//! (January 2018, ENCS).
//!
//! Ported from the official `BIR-Form1604Ev2018.hta` (eBIRForms 7.9.6.2.1):
//! the Part II remittance schedules (`computeSched1`, `computeSched1Total`,
//! `computeSched2`, `computeSched2Total`), the blur checks (`validateYear`,
//! `validateDate`), `validate` with `validateScheduleFields` and their exact
//! alert texts, and `saveXMLsubmit` through [`crate::official_xml`]. Queued
//! through the generic [`QueueableForm`] path. The alphalists (DAT) are not
//! part of this return.
//!
//! Unlike 1604-C and 1604-F, this page never runs `capital()`: the submitted
//! text keeps the case it was entered in.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1604E_FORM_ID: &str = "1604e-v2018";
/// Schedule 1 rows (BIR Form 1601-EQ, per quarter).
pub const FORM_1604E_QUARTERS: usize = 4;
/// Schedule 2 rows (BIR Form 1606, per month).
pub const FORM_1604E_MONTHS: usize = 12;
/// Row captions of Schedule 1.
pub const FORM_1604E_QUARTER_NAMES: [&str; FORM_1604E_QUARTERS] =
    ["1st Quarter", "2nd Quarter", "3rd Quarter", "4th Quarter"];
/// Row captions of Schedule 2.
pub const FORM_1604E_MONTH_NAMES: [&str; FORM_1604E_MONTHS] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `maxlength` of the bank and TRA inputs.
const CODE_MAX: usize = 50;
/// `maxlength` of Item 7A.
const ZIP_MAX: usize = 12;
/// `round()` keeps at most 12 digits before the decimal point.
const AMOUNT_LIMIT: f64 = 1e12;

/// `validateYear`'s alert (with its line break).
pub const FORM_1604E_YEAR_MESSAGE: &str = "Invalid data entry on item no. 1. \nEntry should be current or prior year but not be earlier than the effectivity date of January 2018.";

/// Item 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1604eAgentCategory {
    /// Neither box ticked (the official page starts this way).
    #[default]
    Unspecified,
    Private,
    Government,
}

/// One row of Schedule 1 or 2.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1604eRemittance {
    /// Date of remittance, `MM/DD/YYYY`, or blank.
    #[serde(default)]
    pub date: String,
    /// Drawee bank / bank code / agency (letters and digits only).
    #[serde(default)]
    pub bank: String,
    /// TRA/eROR/eAR number (letters and digits only).
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub taxes_withheld: f64,
    #[serde(default)]
    pub penalties: f64,
    /// Total amount remitted (computed).
    #[serde(default)]
    pub total_remitted: f64,
}

/// The TOTAL line of a schedule (computed).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1604eScheduleTotals {
    pub taxes_withheld: f64,
    pub penalties: f64,
    pub total_remitted: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1604eDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–3
    pub taxable_year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub rdo_code: String,
    /// Item 6.
    pub taxpayer_name: String,
    /// Item 7.
    pub registered_address: String,
    /// Item 7A.
    pub zip_code: String,
    /// Item 8.
    #[serde(default)]
    pub agent_category: Form1604eAgentCategory,
    /// Item 8A; the page starts with neither box ticked.
    #[serde(default)]
    pub top_withholding_agent: Option<bool>,
    /// Item 9.
    pub contact_number: String,
    /// Item 10, also the BIR confirmation address.
    pub email: String,
    /// Background information the page carries in a hidden field.
    #[serde(default)]
    pub line_of_business: String,

    // Part II
    /// Schedule 1 — remittances per BIR Form 1601-EQ.
    #[serde(default)]
    pub schedule1: [Form1604eRemittance; FORM_1604E_QUARTERS],
    #[serde(default)]
    pub schedule1_totals: Form1604eScheduleTotals,
    /// Schedule 2 — remittances per BIR Form 1606.
    #[serde(default)]
    pub schedule2: [Form1604eRemittance; FORM_1604E_MONTHS],
    #[serde(default)]
    pub schedule2_totals: Form1604eScheduleTotals,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
fn cents(value: f64) -> f64 {
    if value.is_finite() {
        parse_official_amount(&official_amount(value)).unwrap_or(0.0)
    } else {
        0.0
    }
}

fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// What `letternumber` lets a filer type.
fn alphanumeric(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

/// Why `validateDate` rejects a date of remittance, if it does.
fn date_problem(value: &str, today: chrono::NaiveDate) -> Option<&'static str> {
    let parts: Vec<&str> = value.split('/').collect();
    let well_formed = parts.len() == 3
        && parts[0].len() == 2
        && parts[1].len() == 2
        && parts[2].len() == 4
        && parts.iter().all(|part| digits_only(part));
    let date = well_formed
        .then(|| {
            chrono::NaiveDate::from_ymd_opt(
                parts[2].parse().ok()?,
                parts[0].parse().ok()?,
                parts[1].parse().ok()?,
            )
        })
        .flatten();
    let Some(date) = date else {
        return Some("Please provide a valid date. (MM/DD/YYYY format)");
    };
    if date > today {
        Some("This date cannot be a future date.")
    } else if chrono::Datelike::year(&date) < 2018 {
        Some("This date cannot be prior to 2018.")
    } else {
        None
    }
}

/// `computeSched1`/`computeSched2` for each row, then the schedule total.
fn compute_schedule(rows: &mut [Form1604eRemittance], totals: &mut Form1604eScheduleTotals) {
    let (mut withheld, mut penalties, mut remitted) = (0.0, 0.0, 0.0);
    for row in rows.iter_mut() {
        row.taxes_withheld = cents(row.taxes_withheld);
        row.penalties = cents(row.penalties);
        row.total_remitted = cents(row.taxes_withheld + row.penalties);
        withheld += row.taxes_withheld;
        penalties += row.penalties;
        remitted += row.total_remitted;
    }
    totals.taxes_withheld = cents(withheld);
    totals.penalties = cents(penalties);
    totals.total_remitted = cents(remitted);
}

/// The `validateScheduleFields` result codes that raise an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowProblem {
    Date,
    Bank,
    Reference,
    TaxesWithheld,
}

/// `validateScheduleFields`: a blank row is fine; any filled row needs the
/// date, bank, reference and a nonzero tax withheld (penalties optional).
fn row_problem(row: &Form1604eRemittance) -> Option<RowProblem> {
    let date = row.date.trim();
    let bank = row.bank.trim();
    let reference = row.reference.trim();
    let tax = official_amount(row.taxes_withheld) != "0.00";
    let penalty = official_amount(row.penalties) != "0.00";
    if date.is_empty() {
        return (!bank.is_empty() || !reference.is_empty() || tax || penalty)
            .then_some(RowProblem::Date);
    }
    if bank.is_empty() {
        return Some(RowProblem::Bank);
    }
    if reference.is_empty() {
        return Some(RowProblem::Reference);
    }
    (!tax).then_some(RowProblem::TaxesWithheld)
}

impl Form1604eDraft {
    pub const FORM_CODE: &'static str = "1604E";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16) -> Self {
        let mut draft = Self {
            id: None,
            taxable_year: year,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            agent_category: Form1604eAgentCategory::Unspecified,
            top_withholding_agent: None,
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            line_of_business: profile.line_of_business.clone(),
            schedule1: Default::default(),
            schedule1_totals: Form1604eScheduleTotals::default(),
            schedule2: Default::default(),
            schedule2_totals: Form1604eScheduleTotals::default(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// The official compute chain. Every amount is held at cents the way
    /// `round(this,2)` leaves it, and each derived item is the formatted
    /// value of the items it reads.
    pub fn recompute(&mut self) {
        compute_schedule(&mut self.schedule1, &mut self.schedule1_totals);
        compute_schedule(&mut self.schedule2, &mut self.schedule2_totals);
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1604e:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        // No capital() on this page: text keeps its case.
        let text = |value: &str| value.trim().to_string();

        put("txtYear", self.taxable_year.to_string());
        put("AmendedRtn_1", flag(self.is_amended));
        put("AmendedRtn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtTIN1", tin1.clone());
        put("txtTIN2", tin2.clone());
        put("txtTIN3", tin3.clone());
        put("txtBranchCode", branch.clone());
        put("txtRDOCode", self.rdo_code.trim().to_string());
        put("txtWthhldngAgntsNme", text(&self.taxpayer_name));
        // Items 7's two boxes are written as one escaped value.
        put("txtAddress", text(&self.registered_address));
        put("txtAddress2", String::new());
        put("txtZipCode", self.zip_code.trim().to_string());
        put(
            "WthldngAgntCtgry_1",
            flag(self.agent_category == Form1604eAgentCategory::Private),
        );
        put(
            "WthldngAgntCtgry_2",
            flag(self.agent_category == Form1604eAgentCategory::Government),
        );
        put(
            "TpWthldngAgnt_1",
            flag(self.top_withholding_agent == Some(true)),
        );
        put(
            "TpWthldngAgnt_2",
            flag(self.top_withholding_agent == Some(false)),
        );
        put("txtTelNum", self.contact_number.trim().to_string());

        for (schedule, rows, totals) in [
            (1, &self.schedule1[..], &self.schedule1_totals),
            (2, &self.schedule2[..], &self.schedule2_totals),
        ] {
            for (index, row) in rows.iter().enumerate() {
                let i = index + 1;
                put(
                    &format!("txtSched{schedule}RemDate{i}"),
                    row.date.trim().to_string(),
                );
                put(&format!("txtSched{schedule}BankCode{i}"), text(&row.bank));
                put(&format!("txtSched{schedule}TRANo{i}"), text(&row.reference));
                put(
                    &format!("txtSched{schedule}TaxWithheld{i}"),
                    official_amount(row.taxes_withheld),
                );
                put(
                    &format!("txtSched{schedule}Penalties{i}"),
                    official_amount(row.penalties),
                );
                put(
                    &format!("txtSched{schedule}TotRemAmt{i}"),
                    official_amount(row.total_remitted),
                );
            }
            put(
                &format!("txtSched{schedule}TaxWithheldTtl"),
                official_amount(totals.taxes_withheld),
            );
            put(
                &format!("txtSched{schedule}PenaltiesTtl"),
                official_amount(totals.penalties),
            );
            put(
                &format!("txtSched{schedule}TotRemAmtTtl"),
                official_amount(totals.total_remitted),
            );
        }

        // Page 2 header, filled from the background information.
        put("txtPg2TIN1", tin1);
        put("txtPg2TIN2", tin2);
        put("txtPg2TIN3", tin3);
        put("txtPg2BranchCode", branch);
        put("txtPg2TaxpayerName", text(&self.taxpayer_name));
        // init() sets page 1; the upload loop writes the field as it stands.
        put("txtCurrentPage", "1".to_string());
        put("txtLineBus", text(&self.line_of_business));

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The field map plus print-only values the frozen 2018 sheet needs
    /// (`derived:` keys, never submitted): Item 7 over its two comb lines
    /// (40 + 31 slots) and the whole 14-digit TIN for the page-2 header.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let address: Vec<char> = self.registered_address.trim().chars().collect();
        fields.insert(
            "derived:address1".to_string(),
            address.iter().take(40).collect(),
        );
        fields.insert(
            "derived:address2".to_string(),
            address.iter().skip(40).take(31).collect(),
        );
        let (a, b, c, d) = split_tin(&self.tin);
        fields.insert("derived:tin_digits".to_string(), format!("{a}{b}{c}{d}"));
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

impl FormValidator for Form1604eDraft {
    /// `validateYear` and `validate` in order, with their alert texts, plus
    /// the input limits the page enforces while typing (`maxlength`,
    /// `letternumber`, `wholenumber`, `validateDate`, `round`).
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let today = chrono::Local::now().date_naive();
        let current_year = chrono::Datelike::year(&today);
        let year = i32::from(self.taxable_year);

        if self.taxable_year == 0 {
            err("taxable_year", "Please enter a valid year on Item 1.");
        } else if year < 2018 || year > current_year {
            err("taxable_year", FORM_1604E_YEAR_MESSAGE);
        }
        if self.number_of_attached_sheets > 99 {
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
        // The page only rejects a blank Item 5; require an official RDO.
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 6.",
            );
        }
        if self.contact_number.trim().is_empty() {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 9.",
            );
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 7.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > ZIP_MAX || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 7A.");
        }
        if self.agent_category == Form1604eAgentCategory::Unspecified {
            err("agent_category", "Please select an option in Item 8.");
        }

        let schedules: [(&str, &[Form1604eRemittance]); 2] = [
            ("schedule1", &self.schedule1),
            ("schedule2", &self.schedule2),
        ];
        for (schedule, rows) in schedules {
            for (index, row) in rows.iter().enumerate() {
                let key = |field: &str| format!("{schedule}[{index}].{field}");
                let (period, source, unit) = if schedule == "schedule1" {
                    let ord = ["1st", "2nd", "3rd", "4th"][index];
                    (format!("the {ord} Quarter"), "1601EQ", "Quarter")
                } else {
                    (FORM_1604E_MONTH_NAMES[index].to_string(), "1606", "month")
                };
                match row_problem(row) {
                    Some(RowProblem::Date) => err(
                        &key("date"),
                        &format!(
                            "Please enter Date of Remittance for {period}. You may refer to your {source} for the said {unit}."
                        ),
                    ),
                    Some(RowProblem::Bank) => err(
                        &key("bank"),
                        &format!(
                            "Please enter any of the following details Drawee Bank / Bank Code / Agency for {period}. You may refer to your {source} for the said {unit}. "
                        ),
                    ),
                    Some(RowProblem::Reference) => err(
                        &key("reference"),
                        &format!(
                            "Please enter any of the following details TRA / eROR / eAR Number for {period}. You may refer to your {source} for the said {unit}. "
                        ),
                    ),
                    Some(RowProblem::TaxesWithheld) => err(
                        &key("taxes_withheld"),
                        &format!(
                            "Please enter the Taxes Withheld for {period}. You may refer to your {source} for the said {unit}."
                        ),
                    ),
                    None => {}
                }
                let date = row.date.trim();
                if !date.is_empty()
                    && let Some(problem) = date_problem(date, today)
                {
                    err(&key("date"), problem);
                }
                for (field, value, label) in [
                    ("bank", row.bank.trim(), "drawee bank/bank code/agency"),
                    ("reference", row.reference.trim(), "TRA/eROR/eAR number"),
                ] {
                    if value.chars().count() > CODE_MAX || !alphanumeric(value) {
                        err(
                            &key(field),
                            &format!(
                                "The {label} takes letters and digits only, at most 50 characters."
                            ),
                        );
                    }
                }
                for (field, value) in [
                    ("taxes_withheld", row.taxes_withheld),
                    ("penalties", row.penalties),
                ] {
                    if value < 0.0 || !has_cent_precision(value) || value >= AMOUNT_LIMIT {
                        err(
                            &key(field),
                            "Enter a non-negative amount in pesos and centavos (at most 12 digits before the decimal point).",
                        );
                    }
                }
            }
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
        if expected.to_bir_field_map() != self.to_bir_field_map() {
            err("totals", "Totals are out of date. Recompute the return.");
        }

        errors
    }
}

impl QueueableForm for Form1604eDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    const FORM_CODE: &'static str = "1604E";
    /// Official `formType` and SFTP folder
    /// (`ftpTargetFolder.PROD['1604Ev2018']`).
    const FORM_TYPE: &'static str = "1604Ev2018";
    const LAYOUT_ID: &'static str = FORM_1604E_FORM_ID;

    /// An annual information return: nothing is paid with it.
    fn amount_payable(&self) -> Option<f64> {
        Some(0.0)
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
    /// `txtYear`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:04}", self.taxable_year)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 4 || !digits_only(code) {
            return None;
        }
        let year: u16 = code.parse().ok()?;
        (year >= 1900).then_some((year, FilingPeriod::Annual))
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

    fn row(date: &str, bank: &str, reference: &str, tax: f64, penalty: f64) -> Form1604eRemittance {
        Form1604eRemittance {
            date: date.to_string(),
            bank: bank.to_string(),
            reference: reference.to_string(),
            taxes_withheld: tax,
            penalties: penalty,
            total_remitted: 0.0,
        }
    }

    pub(crate) fn sample() -> Form1604eDraft {
        let mut draft = Form1604eDraft {
            id: None,
            taxable_year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Taxpayer Inc".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            agent_category: Form1604eAgentCategory::Government,
            top_withholding_agent: None,
            contact_number: "0281234567".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            line_of_business: "Consulting".to_string(),
            schedule1: Default::default(),
            schedule1_totals: Form1604eScheduleTotals::default(),
            schedule2: Default::default(),
            schedule2_totals: Form1604eScheduleTotals::default(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.schedule1[0] = row("04/30/2025", "SampleBank01", "traQ1", 12_345.675, 100.005);
        draft.schedule1[1] = row("07/31/2025", "Agency2", "eROR2", 999.995, 0.0);
        draft.schedule2[0] = row("02/10/2025", "BankJan", "TRA0101", 1_000.5, 0.5);
        draft.schedule2[5] = row("07/10/2025", "bankjun", "tra06", 2_500.125, 0.0);
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1604eDraft) -> Vec<String> {
        <Form1604eDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.schedule1[0].taxes_withheld, 12_345.68);
        assert_eq!(draft.schedule1[0].penalties, 100.01);
        assert_eq!(draft.schedule1[0].total_remitted, 12_445.69);
        assert_eq!(draft.schedule1[1].taxes_withheld, 1_000.0);
        assert_eq!(draft.schedule1_totals.taxes_withheld, 13_345.68);
        assert_eq!(draft.schedule1_totals.total_remitted, 13_445.69);
        assert_eq!(draft.schedule2[0].total_remitted, 1_001.0);
        assert_eq!(draft.schedule2[5].taxes_withheld, 2_500.13);
        assert_eq!(draft.schedule2_totals.taxes_withheld, 3_500.63);
        assert_eq!(draft.schedule2_totals.penalties, 0.5);
        assert_eq!(draft.schedule2_totals.total_remitted, 3_501.13);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_keeps_the_entered_case() {
        let fields = sample().to_bir_field_map();
        assert_eq!(
            fields["frm1604e:txtWthhldngAgntsNme"],
            "Sample Taxpayer Inc"
        );
        assert_eq!(fields["frm1604e:txtPg2TaxpayerName"], "Sample Taxpayer Inc");
        assert_eq!(fields["frm1604e:txtSched2BankCode6"], "bankjun");
        assert_eq!(fields["frm1604e:txtSched1TaxWithheld1"], "12,345.68");
        assert_eq!(fields["frm1604e:txtSched2TotRemAmtTtl"], "3,501.13");
        assert_eq!(fields["frm1604e:TpWthldngAgnt_1"], "false");
        assert_eq!(fields["frm1604e:TpWthldngAgnt_2"], "false");
        assert_eq!(fields["frm1604e:WthldngAgntCtgry_2"], "true");
        assert_eq!(fields["frm1604e:txtRDOCode"], "039");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1604Ev2018-2025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "2025");
        assert_eq!(draft.period_column(), 0);
        assert_eq!(
            Form1604eDraft::parse_period_code("2025"),
            Some((2025, FilingPeriod::Annual))
        );
        assert_eq!(Form1604eDraft::parse_period_code("12025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1604eDraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.taxable_year = 0,
            "Please enter a valid year on Item 1.",
        );
        check(&|d| d.taxable_year = 2017, FORM_1604E_YEAR_MESSAGE);
        check(&|d| d.taxable_year = 2999, FORM_1604E_YEAR_MESSAGE);
        check(
            &|d| d.tin = "12345".into(),
            "Please enter a valid TIN number on Item 4.",
        );
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code.clear(),
            "Please enter a valid RDO Code on Item 5.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 6.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number on Item 9.",
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
            &|d| d.agent_category = Form1604eAgentCategory::Unspecified,
            "Please select an option in Item 8.",
        );
        check(
            &|d| d.schedule1[0].date.clear(),
            "Please enter Date of Remittance for the 1st Quarter. You may refer to your 1601EQ for the said Quarter.",
        );
        check(
            &|d| d.schedule1[1].bank.clear(),
            "Please enter any of the following details Drawee Bank / Bank Code / Agency for the 2nd Quarter. You may refer to your 1601EQ for the said Quarter. ",
        );
        check(
            &|d| d.schedule1[3] = row("12/31/2025", "Bank", "", 1.0, 0.0),
            "Please enter any of the following details TRA / eROR / eAR Number for the 4th Quarter. You may refer to your 1601EQ for the said Quarter. ",
        );
        check(
            &|d| {
                d.schedule1[2] = row("10/31/2025", "Bank", "Tra", 0.0, 5.0);
                d.recompute();
            },
            "Please enter the Taxes Withheld for the 3rd Quarter. You may refer to your 1601EQ for the said Quarter.",
        );
        check(
            &|d| {
                d.schedule2[11] = row("", "", "", 0.0, 3.0);
                d.recompute();
            },
            "Please enter Date of Remittance for December. You may refer to your 1606 for the said month.",
        );
        check(
            &|d| d.schedule2[5].reference.clear(),
            "Please enter any of the following details TRA / eROR / eAR Number for June. You may refer to your 1606 for the said month. ",
        );
        check(
            &|d| d.schedule2[0].date = "02/29/2025".into(),
            "Please provide a valid date. (MM/DD/YYYY format)",
        );
        check(
            &|d| d.schedule2[0].date = "12/31/2017".into(),
            "This date cannot be prior to 2018.",
        );
        check(
            &|d| d.schedule2[0].date = "01/01/2999".into(),
            "This date cannot be a future date.",
        );
        check(
            &|d| d.schedule2[0].bank = "Bank Jan".into(),
            "The drawee bank/bank code/agency takes letters and digits only, at most 50 characters.",
        );
    }

    #[test]
    fn penalties_are_optional_but_taxes_are_not() {
        let mut draft = sample();
        draft.schedule2[2] = row("04/10/2025", "Bank", "Tra", 10.0, 0.0);
        draft.recompute();
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        assert_eq!(row_problem(&row("", "", "", 0.0, 0.0)), None);
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        assert!(draft.revalidate_queued_before_submission().is_ok());
        draft.schedule2[0].penalties = 99.0;
        assert!(draft.revalidate_queued_before_submission().is_err());
    }

    /// P5-1: the earlier float-tolerance check refused two-decimal
    /// amounts above about 2,000,000 (`8_816_764.47 * 100` lands 1.2e-7
    /// off a whole number); the shared `official_inputs` check accepts
    /// them again, and a three-decimal amount is still refused.
    #[test]
    fn large_two_decimal_amounts_pass_cent_precision() {
        let mut draft = sample();
        draft.schedule1[0].taxes_withheld = 8_816_764.47;
        assert!(
            <Form1604eDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "schedule1[0].taxes_withheld"),
            "a large two-decimal amount must be accepted"
        );
        draft.schedule1[0].taxes_withheld = 1.005;
        assert!(
            <Form1604eDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "schedule1[0].taxes_withheld"),
            "a three-decimal amount must still be refused"
        );
    }
}
