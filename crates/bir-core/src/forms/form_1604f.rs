//! BIR Form 1604-F — Annual Information Return of Income Payments Subjected
//! to Final Withholding Taxes (January 2018).
//!
//! Ported from the official `BIR-Form1604F.hta` (eBIRForms 7.9.6.2.1): the
//! Part II remittance schedules (`computeTotalAmount`, `computeTotalWithheld`,
//! `computeTotalPenalties`, `computeTotal`), the date checks (`validateDate`,
//! `checkYear`), `validateForm` with its exact alert texts, and
//! `saveXMLsubmit` through [`crate::official_xml`]. Queued through the
//! generic [`QueueableForm`] path. The Part III alphalists are DAT
//! attachments and are not part of this return.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1604F_FORM_ID: &str = "1604f-v2018";
/// Part II schedules 1–3.
pub const FORM_1604F_SCHEDULES: usize = 3;
/// One row per quarter in each schedule.
pub const FORM_1604F_QUARTERS: usize = 4;

/// The quarterly return each schedule summarizes, as the alerts name it.
pub const FORM_1604F_SCHEDULE_SOURCES: [&str; FORM_1604F_SCHEDULES] =
    ["1601-FQ", "1602-Q", "1603-Q"];
/// Schedule titles on the official page.
pub const FORM_1604F_SCHEDULE_TITLES: [&str; FORM_1604F_SCHEDULES] = [
    "Schedule 1 — Remittance per BIR Form No. 1601-FQ on Final Taxes Withheld",
    "Schedule 2 — Remittance per BIR Form No. 1602Q on Interest Payments",
    "Schedule 3 — Remittance per BIR Form No. 1603Q on Fringe Benefits",
];

const QUARTER_NAMES: [&str; FORM_1604F_QUARTERS] = ["1st", "2nd", "3rd", "4th"];
/// `maxlength` of the TRA/eROR/eAR inputs.
const REFERENCE_MAX: usize = 50;
/// `maxlength` of Item 11A.
const TAX_RELIEF_MAX: usize = 25;
/// `round()` keeps at most 12 digits before the decimal point.
const AMOUNT_LIMIT: f64 = 1e12;

/// Item 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1604fAgentCategory {
    /// Neither box ticked (the official page starts this way).
    #[default]
    Unspecified,
    Private,
    Government,
}

/// One quarter of a Part II schedule.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1604fRemittance {
    /// Date of remittance, `MM/DD/YYYY`, or blank.
    #[serde(default)]
    pub date: String,
    /// TRA/eROR/eAR number.
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

impl Form1604fRemittance {
    pub fn is_blank(&self) -> bool {
        self.date.trim().is_empty()
            && self.reference.trim().is_empty()
            && self.taxes_withheld == 0.0
            && self.penalties == 0.0
    }
}

/// The TOTAL line of a schedule (computed).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1604fScheduleTotals {
    pub taxes_withheld: f64,
    pub penalties: f64,
    pub total_remitted: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1604fDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–3
    /// Item 1.
    pub taxable_year: u16,
    /// Item 2.
    pub is_amended: bool,
    /// Item 3.
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
    pub agent_category: Form1604fAgentCategory,
    /// Item 8A.
    #[serde(default)]
    pub top_withholding_agent: bool,
    /// Item 9.
    pub contact_number: String,
    /// Item 10, also the BIR confirmation address.
    pub email: String,
    /// Background information the page carries in a hidden field.
    #[serde(default)]
    pub line_of_business: String,
    /// Item 11.
    #[serde(default)]
    pub tax_relief: bool,
    /// Item 11A.
    #[serde(default)]
    pub tax_relief_details: String,

    // Part II
    /// Schedules 1–3, quarters 1–4.
    #[serde(default)]
    pub schedules: [[Form1604fRemittance; FORM_1604F_QUARTERS]; FORM_1604F_SCHEDULES],
    /// TOTAL line of each schedule.
    #[serde(default)]
    pub totals: [Form1604fScheduleTotals; FORM_1604F_SCHEDULES],

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// `blockNegativeNumber` after `round(this,2)`.
fn entered_amount(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        cents(value)
    } else {
        0.0
    }
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
            let month = parts[0].parse().ok()?;
            let day = parts[1].parse().ok()?;
            let year = parts[2].parse().ok()?;
            chrono::NaiveDate::from_ymd_opt(year, month, day)
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

impl Form1604fDraft {
    pub const FORM_CODE: &'static str = "1604F";

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
            agent_category: Form1604fAgentCategory::Unspecified,
            top_withholding_agent: false,
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            line_of_business: profile.line_of_business.clone(),
            tax_relief: false,
            tax_relief_details: String::new(),
            schedules: Default::default(),
            totals: Default::default(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// The official compute chain. Every amount is held at cents the way
    /// `round(this,2)` and `blockNegativeNumber` leave it, and each derived
    /// item is the formatted value of the items it reads.
    pub fn recompute(&mut self) {
        // TaxReliefEnable clears Item 11A when Item 11 is "No".
        if !self.tax_relief {
            self.tax_relief_details.clear();
        }
        for (schedule, totals) in self.schedules.iter_mut().zip(self.totals.iter_mut()) {
            let mut withheld = 0.0;
            let mut penalties = 0.0;
            for row in schedule.iter_mut() {
                row.taxes_withheld = entered_amount(row.taxes_withheld);
                row.penalties = entered_amount(row.penalties);
                row.total_remitted = cents(row.taxes_withheld + row.penalties);
                withheld += row.taxes_withheld;
                penalties += row.penalties;
            }
            totals.taxes_withheld = cents(withheld);
            totals.penalties = cents(penalties);
            totals.total_remitted = cents(totals.taxes_withheld + totals.penalties);
        }
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1604f:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        // capital() uppercases every text input except txtEmail, including
        // the Item 10 copy of the email.
        let text = |value: &str| value.trim().to_uppercase();

        put("txtYear", self.taxable_year.to_string());
        put("amendedRtn_1", flag(self.is_amended));
        put("amendedRtn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("tinA", tin1.clone());
        put("tinB", tin2.clone());
        put("tinC", tin3.clone());
        put("branchCode", branch.clone());
        put("rdoCode", self.rdo_code.trim().to_string());
        put("registeredName", text(&self.taxpayer_name));
        put("RegisteredAddress", text(&self.registered_address));
        put("zipCode", self.zip_code.trim().to_string());
        put(
            "categoryAgent_1",
            flag(self.agent_category == Form1604fAgentCategory::Private),
        );
        put(
            "categoryAgent_2",
            flag(self.agent_category == Form1604fAgentCategory::Government),
        );
        put("privateAgent_1", flag(self.top_withholding_agent));
        put("privateAgent_2", flag(!self.top_withholding_agent));
        put("telephoneNumber", self.contact_number.trim().to_string());
        put("email", text(&self.email));
        put("taxRelief_1", flag(self.tax_relief));
        put("taxRelief_2", flag(!self.tax_relief));
        put("availedTaxRelief", text(&self.tax_relief_details));

        for (index, (schedule, totals)) in self.schedules.iter().zip(&self.totals).enumerate() {
            let s = index + 1;
            for (quarter, row) in schedule.iter().enumerate() {
                let q = quarter + 1;
                put(&format!("txtSched{s}Date{q}"), row.date.trim().to_string());
                put(&format!("txtSched{s}TRA{q}"), text(&row.reference));
                put(
                    &format!("txtSched{s}TaxWheld{q}"),
                    official_amount(row.taxes_withheld),
                );
                put(
                    &format!("txtSched{s}Pen{q}"),
                    official_amount(row.penalties),
                );
                put(
                    &format!("txtSched{s}TotalAmt{q}"),
                    official_amount(row.total_remitted),
                );
            }
            put(
                &format!("txtSched{s}TaxWheldTotal"),
                official_amount(totals.taxes_withheld),
            );
            put(
                &format!("txtSched{s}PenTotal"),
                official_amount(totals.penalties),
            );
            put(
                &format!("txtSched{s}Total"),
                official_amount(totals.total_remitted),
            );
        }

        // Page 2 header, filled from the background information.
        put("txtPg2TIN1", tin1);
        put("txtPg2TIN2", tin2);
        put("txtPg2TIN3", tin3);
        put("txtPg2BranchCode", branch);
        put("txtPg2TaxpayerName", text(&self.taxpayer_name));
        put("txtCurrentPage", "1".to_string());
        put("txtLineBus", text(&self.line_of_business));

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The field map plus print-only values the frozen 2018 sheet needs
    /// (`derived:` keys, never submitted): Item 7 over its two comb lines
    /// (40 slots, then 31 fillable slots followed by the 4-slot 7A ZIP Code) and the
    /// whole 14-digit TIN for the page-2 header.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let address: Vec<char> = self.registered_address.trim().chars().collect();
        let first: String = address.iter().take(40).collect();
        let rest: String = address.iter().skip(40).take(31).collect();
        let zip: String = self.zip_code.trim().chars().take(4).collect();
        fields.insert("derived:address1".to_string(), first);
        fields.insert(
            "derived:address2_zip".to_string(),
            format!("{rest:<31}{zip}"),
        );
        let (a, b, c, d) = split_tin(&self.tin);
        fields.insert("derived:tin_digits".to_string(), format!("{a}{b}{c}{d}"));
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `validateForm`'s checks of one schedule row, in official order; the
    /// first failing check, like the official `return`.
    fn remittance_error(
        schedule: usize,
        quarter: usize,
        row: &Form1604fRemittance,
    ) -> Option<String> {
        let source = FORM_1604F_SCHEDULE_SOURCES[schedule];
        let ord = QUARTER_NAMES[quarter];
        let date_message = || {
            format!(
                "Please enter the Date of Remittance for the {ord} Quarter. You may refer to your {source} for the said quarter."
            )
        };
        let reference_message = || {
            format!(
                "Please enter the following details TRA/eROR/eAR Number for the {ord} Quarter. You may refer to your {source} for the said quarter."
            )
        };
        let date = row.date.trim();
        let reference = row.reference.trim();
        if row.taxes_withheld > 0.0 || row.penalties > 0.0 {
            if schedule == 0 {
                if date.is_empty() && !reference.is_empty() {
                    return Some(date_message());
                } else if reference.is_empty() {
                    // The official text keeps its "<month>" placeholder.
                    return Some(
                        "Please enter any of the following details TRA/eROR/eAR Number for the <month>. You may refer to your 1601-FQ for the said Quarter."
                            .to_string(),
                    );
                }
            } else if date.is_empty() {
                return Some(date_message());
            } else if reference.is_empty() {
                return Some(reference_message());
            }
        }
        if !reference.is_empty() || !date.is_empty() {
            if date.is_empty() {
                return Some(date_message());
            } else if reference.is_empty() {
                return Some(reference_message());
            }
        }
        if row.taxes_withheld == 0.0 && !date.is_empty() {
            return Some(format!(
                "Please enter the Taxes Withheld for the {ord} Quarter. You may refer to your {source} for the said quarter."
            ));
        }
        None
    }
}

impl FormValidator for Form1604fDraft {
    /// `validateForm` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing (`maxlength`, `wholenumber`,
    /// `validateDate`, `blockNegativeNumber`, `round`).
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let today = chrono::Local::now().date_naive();
        let current_year = chrono::Datelike::year(&today);

        // checkYear (alerts, then validation continues).
        let year = i32::from(self.taxable_year);
        if self.taxable_year != 0 && (year <= 2017 || year > current_year) {
            err(
                "taxable_year",
                "Invalid data entry on item no. 1. Entry should be current or prior year but not be earlier than the effectivity date of January 2018.",
            );
        }
        if self.taxable_year == 0 {
            err("taxable_year", "Please enter a valid year on Item 1.");
        }
        let details = self.tax_relief_details.trim();
        if self.tax_relief && details.is_empty() {
            err(
                "tax_relief_details",
                "Please specify the Special Treaty or International Law the payee is availing in item 11A.",
            );
        }
        if details.chars().count() > TAX_RELIEF_MAX {
            err(
                "tax_relief_details",
                "Item 11A holds at most 25 characters.",
            );
        }
        if self.taxable_year != 0 && self.taxable_year < 1900 {
            err(
                "taxable_year",
                "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
            );
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
        // The page's own check reads `selectedIndex` of what is a text box
        // here, so it never fires; initialValidateBeforeSave rejects "000".
        // Require an official RDO.
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 70 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer's Name on Item 7.",
            );
        }
        if self.contact_number.trim().is_empty() {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 8.",
            );
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 9.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 10.");
        }
        if self.agent_category == Form1604fAgentCategory::Unspecified {
            err("agent_category", "Please select an option for Item 8.");
        }

        for (s, schedule) in self.schedules.iter().enumerate() {
            for (q, row) in schedule.iter().enumerate() {
                let key = |field: &str| format!("schedules[{s}][{q}].{field}");
                if let Some(message) = Self::remittance_error(s, q, row) {
                    let field = if message.contains("Date of Remittance") {
                        "date"
                    } else if message.contains("TRA/eROR/eAR") {
                        "reference"
                    } else {
                        "taxes_withheld"
                    };
                    err(&key(field), &message);
                }
                let date = row.date.trim();
                if !date.is_empty()
                    && let Some(problem) = date_problem(date, today)
                {
                    err(&key("date"), problem);
                }
                if row.reference.trim().chars().count() > REFERENCE_MAX {
                    err(
                        &key("reference"),
                        "A TRA/eROR/eAR number holds at most 50 characters.",
                    );
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
        if expected.schedules != self.schedules || expected.totals != self.totals {
            err("totals", "Totals are out of date. Recompute the return.");
        }

        errors
    }
}

impl QueueableForm for Form1604fDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    const FORM_CODE: &'static str = "1604F";
    /// Official `formType` (`1604F`).
    const FORM_TYPE: &'static str = "1604F";
    const LAYOUT_ID: &'static str = FORM_1604F_FORM_ID;

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

    fn remittance(date: &str, reference: &str, tax: f64, penalty: f64) -> Form1604fRemittance {
        Form1604fRemittance {
            date: date.to_string(),
            reference: reference.to_string(),
            taxes_withheld: tax,
            penalties: penalty,
            total_remitted: 0.0,
        }
    }

    pub(crate) fn sample() -> Form1604fDraft {
        let mut draft = Form1604fDraft {
            id: None,
            taxable_year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Taxpayer Inc".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            agent_category: Form1604fAgentCategory::Private,
            top_withholding_agent: false,
            contact_number: "0281234567".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            line_of_business: "Consulting".to_string(),
            tax_relief: false,
            tax_relief_details: String::new(),
            schedules: Default::default(),
            totals: Default::default(),
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.schedules[0][0] = remittance("04/25/2025", "tra-0001", 12_345.675, 100.005);
        draft.schedules[0][1] = remittance("07/25/2025", "eror 0002", 999.995, 0.0);
        draft.schedules[1][3] = remittance("01/30/2026", "ear-4", 5_000.0, 0.0);
        draft.schedules[2][2] = remittance("10/30/2025", "x3", 1.5, 2.25);
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1604fDraft) -> Vec<String> {
        <Form1604fDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.schedules[0][0].taxes_withheld, 12_345.68);
        assert_eq!(draft.schedules[0][0].penalties, 100.01);
        assert_eq!(draft.schedules[0][0].total_remitted, 12_445.69);
        assert_eq!(draft.schedules[0][1].taxes_withheld, 1_000.0);
        assert_eq!(draft.totals[0].taxes_withheld, 13_345.68);
        assert_eq!(draft.totals[0].penalties, 100.01);
        assert_eq!(draft.totals[0].total_remitted, 13_445.69);
        assert_eq!(draft.totals[1].total_remitted, 5_000.0);
        assert_eq!(draft.schedules[2][2].total_remitted, 3.75);
        assert_eq!(draft.totals[2].total_remitted, 3.75);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn negative_amounts_become_zero_like_block_negative_number() {
        let mut draft = sample();
        draft.schedules[0][2] = remittance("", "", -5.0, -1.0);
        draft.recompute();
        assert_eq!(draft.schedules[0][2].taxes_withheld, 0.0);
        assert_eq!(draft.schedules[0][2].penalties, 0.0);
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        assert_eq!(fields["frm1604f:txtYear"], "2025");
        assert_eq!(fields["frm1604f:registeredName"], "SAMPLE TAXPAYER INC");
        assert_eq!(fields["frm1604f:txtPg2TaxpayerName"], "SAMPLE TAXPAYER INC");
        assert_eq!(fields["frm1604f:email"], "SAMPLE.TAXPAYER@EXAMPLE.COM");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        assert_eq!(fields["frm1604f:branchCode"], "00000");
        assert_eq!(fields["frm1604f:rdoCode"], "039");
        assert_eq!(fields["frm1604f:txtSched1TRA1"], "TRA-0001");
        assert_eq!(fields["frm1604f:txtSched1TaxWheld1"], "12,345.68");
        assert_eq!(fields["frm1604f:txtSched1Total"], "13,445.69");
        assert_eq!(fields["frm1604f:txtSched2Date1"], "");
        assert_eq!(fields["frm1604f:txtSched2Pen1"], "0.00");
        assert_eq!(fields["frm1604f:categoryAgent_1"], "true");
        assert_eq!(fields["frm1604f:privateAgent_2"], "true");
        assert_eq!(fields["frm1604f:txtCurrentPage"], "1");
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1604F-2025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "2025");
        assert_eq!(draft.filing_period(), FilingPeriod::Annual);
        assert_eq!(draft.period_column(), 0);
        assert_eq!(
            Form1604fDraft::parse_period_code("2025"),
            Some((2025, FilingPeriod::Annual))
        );
        assert_eq!(Form1604fDraft::parse_period_code("122025"), None);
        assert_eq!(Form1604fDraft::parse_period_code("20a5"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1604fDraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.taxable_year = 2017,
            "Invalid data entry on item no. 1. Entry should be current or prior year but not be earlier than the effectivity date of January 2018.",
        );
        check(
            &|d| d.taxable_year = 0,
            "Please enter a valid year on Item 1.",
        );
        check(
            &|d| d.taxable_year = 1899,
            "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
        );
        check(
            &|d| {
                d.tax_relief = true;
                d.recompute();
            },
            "Please specify the Special Treaty or International Law the payee is availing in item 11A.",
        );
        check(
            &|d| d.tin = "12345".into(),
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
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer's Name on Item 7.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number on Item 8.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 9.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 10.",
        );
        check(
            &|d| d.agent_category = Form1604fAgentCategory::Unspecified,
            "Please select an option for Item 8.",
        );
        // Schedule 1, amount with neither date nor reference.
        check(
            &|d| {
                d.schedules[0][2] = remittance("", "", 10.0, 0.0);
                d.recompute();
            },
            "Please enter any of the following details TRA/eROR/eAR Number for the <month>. You may refer to your 1601-FQ for the said Quarter.",
        );
        check(
            &|d| d.schedules[0][0].date.clear(),
            "Please enter the Date of Remittance for the 1st Quarter. You may refer to your 1601-FQ for the said quarter.",
        );
        check(
            &|d| {
                d.schedules[0][3] = remittance("12/01/2025", "", 0.0, 0.0);
            },
            "Please enter the following details TRA/eROR/eAR Number for the 4th Quarter. You may refer to your 1601-FQ for the said quarter.",
        );
        check(
            &|d| {
                d.schedules[0][2] = remittance("09/30/2025", "tra", 0.0, 5.0);
                d.recompute();
            },
            "Please enter the Taxes Withheld for the 3rd Quarter. You may refer to your 1601-FQ for the said quarter.",
        );
        check(
            &|d| d.schedules[1][3].date.clear(),
            "Please enter the Date of Remittance for the 4th Quarter. You may refer to your 1602-Q for the said quarter.",
        );
        check(
            &|d| d.schedules[1][3].reference.clear(),
            "Please enter the following details TRA/eROR/eAR Number for the 4th Quarter. You may refer to your 1602-Q for the said quarter.",
        );
        check(
            &|d| d.schedules[2][2].reference.clear(),
            "Please enter the following details TRA/eROR/eAR Number for the 3rd Quarter. You may refer to your 1603-Q for the said quarter.",
        );
        check(
            &|d| d.schedules[2][0] = remittance("", "x", 0.0, 0.0),
            "Please enter the Date of Remittance for the 1st Quarter. You may refer to your 1603-Q for the said quarter.",
        );
        // validateDate on blur.
        check(
            &|d| d.schedules[0][0].date = "02/30/2025".into(),
            "Please provide a valid date. (MM/DD/YYYY format)",
        );
        check(
            &|d| d.schedules[0][0].date = "4/25/2025".into(),
            "Please provide a valid date. (MM/DD/YYYY format)",
        );
        check(
            &|d| d.schedules[0][0].date = "12/31/2017".into(),
            "This date cannot be prior to 2018.",
        );
        check(
            &|d| d.schedules[0][0].date = "01/01/2999".into(),
            "This date cannot be a future date.",
        );
    }

    #[test]
    fn tax_relief_details_follow_item_11() {
        let mut draft = sample();
        draft.tax_relief = true;
        draft.tax_relief_details = "Rp-us treaty".into();
        draft.recompute();
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        assert_eq!(
            draft.to_bir_field_map()["frm1604f:availedTaxRelief"],
            "RP-US TREATY"
        );
        draft.tax_relief = false;
        draft.recompute();
        assert!(draft.tax_relief_details.is_empty());
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        assert!(draft.revalidate_queued_before_submission().is_ok());
        draft.schedules[0][0].penalties = 99.0;
        assert!(draft.revalidate_queued_before_submission().is_err());
    }

    /// P5-1: the earlier float-tolerance check refused two-decimal
    /// amounts above about 2,000,000 (`8_816_764.47 * 100` lands 1.2e-7
    /// off a whole number); the shared `official_inputs` check accepts
    /// them again, and a three-decimal amount is still refused.
    #[test]
    fn large_two_decimal_amounts_pass_cent_precision() {
        let mut draft = sample();
        draft.schedules[0][0].taxes_withheld = 8_816_764.47;
        assert!(
            <Form1604fDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "schedules[0][0].taxes_withheld"),
            "a large two-decimal amount must be accepted"
        );
        draft.schedules[0][0].taxes_withheld = 1.005;
        assert!(
            <Form1604fDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "schedules[0][0].taxes_withheld"),
            "a three-decimal amount must still be refused"
        );
    }
}
