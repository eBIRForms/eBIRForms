//! BIR Form 1604-C — Annual Information Return of Income Taxes Withheld on
//! Compensation (January 2018).
//!
//! Ported from the official `BIR-Form1604C.hta` (eBIRForms 7.9.6.2.1): the
//! Part II monthly remittances (`computeTotalAmount`, `computeTotalWithheld`,
//! `computeTotalAdjustment`, `computeTotalPenalties`, `computeTotal`), the
//! refund items 11–13 (`changeRefund`), the blur checks (`ValidateYear`,
//! `validateDate`), `validateForm` with its exact alert texts, and
//! `saveXMLsubmit` through [`crate::official_xml`]. Queued through the
//! generic [`QueueableForm`] path. The alphalist (DAT) is not part of this
//! return.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1604C_FORM_ID: &str = "1604c-v2018";
/// Part II rows, January to December.
pub const FORM_1604C_MONTHS: usize = 12;
/// Row captions on the official page.
pub const FORM_1604C_MONTH_NAMES: [&str; FORM_1604C_MONTHS] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// `maxlength` of the drawee bank / bank code / agency inputs.
const BANK_MAX: usize = 50;
/// `maxlength` of the TRA/eROR/eAR inputs.
const REFERENCE_MAX: usize = 10;
/// `maxlength` of Item 6.
const NAME_MAX: usize = 70;
/// `maxlength` of Item 7.
const ADDRESS_MAX: usize = 150;
/// `maxlength` of Item 7A.
const ZIP_MAX: usize = 12;
/// `round()` keeps at most 12 digits before the decimal point.
const AMOUNT_LIMIT: f64 = 1e12;

/// Item 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1604cAgentCategory {
    /// Neither box ticked (the official page starts this way).
    #[default]
    Unspecified,
    Private,
    Government,
}

/// One month of Part II and its continuation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1604cRemittance {
    /// Date of remittance, `MM/DD/YYYY`, or blank.
    #[serde(default)]
    pub date: String,
    /// Drawee bank / bank code / agency.
    #[serde(default)]
    pub bank: String,
    /// TRA/eROR/eAR number.
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub taxes_withheld: f64,
    /// May be negative.
    #[serde(default)]
    pub adjustment: f64,
    #[serde(default)]
    pub penalties: f64,
    /// Total amount remitted (computed).
    #[serde(default)]
    pub total_remitted: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1604cDraft {
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
    pub agent_category: Form1604cAgentCategory,
    /// Item 8A.
    #[serde(default)]
    pub top_withholding_agent: bool,
    /// Item 9.
    pub contact_number: String,
    /// Item 10, also the BIR confirmation address.
    pub email: String,
    /// Item 11.
    #[serde(default)]
    pub refunds_released: bool,
    /// Item 11A, `MM/DD/YYYY`.
    #[serde(default)]
    pub refund_date: String,
    /// Item 12.
    #[serde(default)]
    pub overremittance: f64,
    /// Item 13 (1–12; 0 when not chosen).
    #[serde(default)]
    pub first_crediting_month: u8,

    // Part II
    #[serde(default)]
    pub months: [Form1604cRemittance; FORM_1604C_MONTHS],
    #[serde(default)]
    pub total_taxes_withheld: f64,
    #[serde(default)]
    pub total_adjustment: f64,
    #[serde(default)]
    pub total_penalties: f64,
    #[serde(default)]
    pub total_remitted: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `formatCurrency` as a number: cents, keeping the sign of a negative sum
/// that rounds to zero (JavaScript writes it as `-0.00`).
fn js_cents(value: f64) -> f64 {
    let rounded = parse_official_amount(&official_amount(value)).unwrap_or(0.0);
    if rounded == 0.0 && value < 0.0 {
        -0.0
    } else {
        rounded
    }
}

/// The text `formatCurrency` / `round` leave in the field.
fn js_amount(value: f64) -> String {
    if value == 0.0 && value.is_sign_negative() {
        "-0.00".to_string()
    } else {
        official_amount(value)
    }
}

/// `blockNegativeNumber` after `round(this,2)`.
fn entered_amount(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        js_cents(value)
    } else {
        0.0
    }
}

/// `round(this,2)` on a field that keeps its sign (adjustments).
fn entered_signed_amount(value: f64) -> f64 {
    if value.is_finite() {
        // round() turns "-0.00"-like input into "0.00".
        let rounded = js_cents(value);
        if rounded == 0.0 { 0.0 } else { rounded }
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

/// A strict `MM/DD/YYYY` calendar date.
fn parse_date(value: &str) -> Option<chrono::NaiveDate> {
    let parts: Vec<&str> = value.split('/').collect();
    let well_formed = parts.len() == 3
        && parts[0].len() == 2
        && parts[1].len() == 2
        && parts[2].len() == 4
        && parts.iter().all(|part| digits_only(part));
    if !well_formed {
        return None;
    }
    chrono::NaiveDate::from_ymd_opt(
        parts[2].parse().ok()?,
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
    )
}

/// Why `validateDate` rejects a date of remittance, if it does.
fn date_problem(value: &str, today: chrono::NaiveDate) -> Option<&'static str> {
    let Some(date) = parse_date(value) else {
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

/// Item 7 over the sheet's two comb lines (40 + 31 slots). Up to 71
/// characters fill the boxes; a longer address (the input allows 150) is
/// split in the same 40:31 proportion and each `derived:line:` value prints
/// whole across its comb, so no character is dropped from the print.
fn print_address_lines(address: &str) -> (String, String) {
    let chars: Vec<char> = address.trim().chars().collect();
    let first = if chars.len() <= 71 {
        40.min(chars.len())
    } else {
        (chars.len() * 40).div_ceil(71)
    };
    (
        chars[..first].iter().collect(),
        chars[first..].iter().collect(),
    )
}

impl Form1604cDraft {
    pub const FORM_CODE: &'static str = "1604C";

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
            agent_category: Form1604cAgentCategory::Unspecified,
            top_withholding_agent: false,
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            refunds_released: false,
            refund_date: String::new(),
            overremittance: 0.0,
            first_crediting_month: 0,
            months: Default::default(),
            total_taxes_withheld: 0.0,
            total_adjustment: 0.0,
            total_penalties: 0.0,
            total_remitted: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// The official compute chain. Every amount is held at cents the way
    /// `round(this,2)` and `blockNegativeNumber` leave it, and each derived
    /// item is the formatted value of the items it reads.
    pub fn recompute(&mut self) {
        // changeRefund clears Items 11A–13 when Item 11 is "No".
        if self.refunds_released {
            self.overremittance = if self.overremittance.is_finite() {
                js_cents(self.overremittance)
            } else {
                0.0
            };
        } else {
            self.refund_date.clear();
            self.overremittance = 0.0;
            self.first_crediting_month = 0;
        }
        let (mut withheld, mut adjustment, mut penalties, mut remitted) = (0.0, 0.0, 0.0, 0.0);
        for row in &mut self.months {
            row.taxes_withheld = entered_amount(row.taxes_withheld);
            row.adjustment = entered_signed_amount(row.adjustment);
            row.penalties = entered_amount(row.penalties);
            row.total_remitted = js_cents(row.taxes_withheld + row.penalties + row.adjustment);
            withheld += row.taxes_withheld;
            adjustment += row.adjustment;
            penalties += row.penalties;
            // computeTotal('1') adds only the positive row totals.
            if row.total_remitted > 0.0 {
                remitted += row.total_remitted;
            }
        }
        self.total_taxes_withheld = js_cents(withheld);
        self.total_adjustment = js_cents(adjustment);
        self.total_penalties = js_cents(penalties);
        self.total_remitted = js_cents(remitted);
    }

    /// Item 11A split into the month select, day and year boxes.
    fn refund_parts(&self) -> (String, String, String) {
        if !self.refunds_released {
            return ("00".to_string(), String::new(), String::new());
        }
        let mut parts = self.refund_date.trim().splitn(3, '/');
        let month = parts.next().unwrap_or("").to_string();
        let day = parts.next().unwrap_or("").to_string();
        let year = parts.next().unwrap_or("").to_string();
        let month = if month.len() == 2 && digits_only(&month) {
            month
        } else {
            "00".to_string()
        };
        (month, day, year)
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1604c:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        // capital() uppercases every text input except txtEmail.
        let text = |value: &str| value.trim().to_uppercase();

        put("txtYear", self.taxable_year.to_string());
        put("amendedRtn_1", flag(self.is_amended));
        put("amendedRtn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("tinA", tin1);
        put("tinB", tin2);
        put("tinC", tin3);
        put("branchCode", branch);
        put("rdoCode", self.rdo_code.trim().to_string());
        put("registeredName", text(&self.taxpayer_name));
        put("registeredAddress", text(&self.registered_address));
        put("zipCode", self.zip_code.trim().to_string());
        put(
            "categoryAgent_1",
            flag(self.agent_category == Form1604cAgentCategory::Private),
        );
        put(
            "categoryAgent_2",
            flag(self.agent_category == Form1604cAgentCategory::Government),
        );
        put("topAgent_1", flag(self.top_withholding_agent));
        put("topAgent_2", flag(!self.top_withholding_agent));
        put("telephoneNumber", text(&self.contact_number));

        put("releasedOfFunds_1", flag(self.refunds_released));
        put("releasedOfFunds_2", flag(!self.refunds_released));
        let (month, day, year) = self.refund_parts();
        put("txtRefMonth", month);
        put("txtRefDate", day);
        put("txtRefYear", year);
        put("txt12", js_amount(self.overremittance));
        put(
            "select13",
            if self.refunds_released && (1..=12).contains(&self.first_crediting_month) {
                format!("{:02}", self.first_crediting_month)
            } else {
                "00".to_string()
            },
        );

        for (index, row) in self.months.iter().enumerate() {
            let m = index + 1;
            put(&format!("txtSched1Date{m}"), row.date.trim().to_string());
            put(&format!("txtSched1BankVal{m}"), text(&row.bank));
            put(&format!("txtTra{m}"), text(&row.reference));
            put(
                &format!("txtSched1TaxWheld{m}"),
                js_amount(row.taxes_withheld),
            );
            put(&format!("txtSched1Adj{m}"), js_amount(row.adjustment));
            put(&format!("txtSched1Pen{m}"), js_amount(row.penalties));
            put(
                &format!("txtSched1TotalAmt{m}"),
                js_amount(row.total_remitted),
            );
        }
        put(
            "txtSched1TaxWheldTotal",
            js_amount(self.total_taxes_withheld),
        );
        put("txtSched1AdjTotal", js_amount(self.total_adjustment));
        put("txtSched1PenTotal", js_amount(self.total_penalties));
        put("txtSched1Total", js_amount(self.total_remitted));

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The field map plus print-only values the frozen 2018 sheet needs
    /// (`derived:` keys, never submitted): Item 7 over its two comb lines
    /// (40 + 31 slots; a longer address prints whole, shrunk across both), and Items 11A-13, which print blank unless Item 11
    /// is "Yes" (the submitted controls hold `00` / `0.00` then).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let (line1, line2) = print_address_lines(&self.registered_address);
        fields.insert("derived:line:address1".to_string(), line1);
        fields.insert("derived:line:address2".to_string(), line2);
        let (month, day, year) = self.refund_parts();
        let released = self.refunds_released;
        let blank_unless = |value: String| if released { value } else { String::new() };
        fields.insert(
            "derived:refund_mm".to_string(),
            blank_unless(if month == "00" { String::new() } else { month }),
        );
        fields.insert("derived:refund_dd".to_string(), blank_unless(day));
        fields.insert("derived:refund_yyyy".to_string(), blank_unless(year));
        fields.insert(
            "derived:overremittance".to_string(),
            blank_unless(official_amount(self.overremittance)),
        );
        fields.insert(
            "derived:crediting_month".to_string(),
            if released && (1..=12).contains(&self.first_crediting_month) {
                format!("{:02}", self.first_crediting_month)
            } else {
                String::new()
            },
        );
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `validateForm`'s Part II row scan: every filled cell of a row (amounts
    /// other than `0.00`) must agree on blank versus filled, first the main
    /// table, then its continuation. The official loop stops before December;
    /// this checks all twelve rows.
    fn row_error(month: usize, row: &Form1604cRemittance) -> Option<(&'static str, String)> {
        let amount = |value: f64| (js_amount(value) != "0.00").then_some(false);
        let main = [
            ("date", Some(row.date.trim().is_empty())),
            ("bank", Some(row.bank.trim().is_empty())),
            ("reference", Some(row.reference.trim().is_empty())),
            ("taxes_withheld", amount(row.taxes_withheld)),
        ];
        let continuation = [
            ("adjustment", amount(row.adjustment)),
            ("penalties", amount(row.penalties)),
        ];
        let x = month + 1;
        let mut first: Option<bool> = None;
        for (cells, message) in [
            (&main[..], format!("Incomplete values on Part II, Row {x}.")),
            (
                &continuation[..],
                format!("Incomplete values on Part II (Continuation), Row {x}."),
            ),
        ] {
            for (field, blank) in cells {
                let Some(blank) = *blank else {
                    continue;
                };
                match first {
                    None => first = Some(blank),
                    Some(previous) if previous != blank => return Some((field, message)),
                    Some(_) => {}
                }
            }
        }
        None
    }
}

impl FormValidator for Form1604cDraft {
    /// `ValidateYear` and `validateForm` in order, with their alert texts,
    /// plus the input limits the page enforces while typing (`maxlength`,
    /// `wholenumber`, `validateDate`, `blockNegativeNumber`, `round`).
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
        } else if year > current_year {
            err(
                "taxable_year",
                "Invalid data entry on Item no. 1. Entry should be current or prior year but not be earlier than effectivity date of January 2018.",
            );
        } else if year < 2018 {
            // ValidateYear on blur, then validateForm's own floor.
            err(
                "taxable_year",
                "Invalid date entry on Item no.1. Entry should not be earlier than effectivity date of January 2018.",
            );
            if year < 1900 {
                err(
                    "taxable_year",
                    "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
                );
            }
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
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer's Name on Item 6.",
            );
        }
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > ADDRESS_MAX {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 7.",
            );
        }
        if self.agent_category == Form1604cAgentCategory::Unspecified {
            err("agent_category", "Please select an option for Item 8.");
        }
        if self.contact_number.trim().is_empty() {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 9.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > ZIP_MAX || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 10.");
        }

        if self.refunds_released {
            let date = self.refund_date.trim();
            let day = date.split('/').nth(1).unwrap_or("");
            if parse_date(date).is_none() {
                if day.len() == 1 && date.split('/').count() == 3 {
                    err(
                        "refund_date",
                        "Please enter a valid day on item 11A. Format should be MM/DD/YYYY.",
                    );
                } else {
                    err("refund_date", "Please specify date of refund in item 11A.");
                }
            }
            // The page only rejects a blank Item 12, which round() never
            // leaves; require an actual overremittance.
            if self.overremittance <= 0.0
                || !has_cent_precision(self.overremittance)
                || self.overremittance >= AMOUNT_LIMIT
            {
                err(
                    "overremittance",
                    "Please enter Amount of Overremittance in Item 12.",
                );
            }
            if !(1..=12).contains(&self.first_crediting_month) {
                err(
                    "first_crediting_month",
                    "Please select a month from the list on Item 13.",
                );
            }
        }

        for (m, row) in self.months.iter().enumerate() {
            let key = |field: &str| format!("months[{m}].{field}");
            if let Some((field, message)) = Self::row_error(m, row) {
                err(&key(field), &message);
            }
            let date = row.date.trim();
            if !date.is_empty()
                && let Some(problem) = date_problem(date, today)
            {
                err(&key("date"), problem);
            }
            if row.bank.trim().chars().count() > BANK_MAX {
                err(
                    &key("bank"),
                    "The drawee bank/bank code/agency holds at most 50 characters.",
                );
            }
            if row.reference.trim().chars().count() > REFERENCE_MAX {
                err(
                    &key("reference"),
                    "A TRA/eROR/eAR number holds at most 10 characters.",
                );
            }
            for (field, value, signed) in [
                ("taxes_withheld", row.taxes_withheld, false),
                ("adjustment", row.adjustment, true),
                ("penalties", row.penalties, false),
            ] {
                if (!signed && value < 0.0)
                    || !has_cent_precision(value)
                    || value.abs() >= AMOUNT_LIMIT
                {
                    err(
                        &key(field),
                        "Enter an amount in pesos and centavos (at most 12 digits before the decimal point).",
                    );
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
            err(
                "total_remitted",
                "Totals are out of date. Recompute the return.",
            );
        }

        errors
    }
}

impl QueueableForm for Form1604cDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    const FORM_CODE: &'static str = "1604C";
    /// Official `formType` (`1604C`).
    const FORM_TYPE: &'static str = "1604C";
    const LAYOUT_ID: &'static str = FORM_1604C_FORM_ID;

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

    fn month(
        date: &str,
        bank: &str,
        reference: &str,
        tax: f64,
        adjustment: f64,
        penalty: f64,
    ) -> Form1604cRemittance {
        Form1604cRemittance {
            date: date.to_string(),
            bank: bank.to_string(),
            reference: reference.to_string(),
            taxes_withheld: tax,
            adjustment,
            penalties: penalty,
            total_remitted: 0.0,
        }
    }

    #[test]
    fn long_address_prints_every_character() {
        let short: String = "A".repeat(60);
        let (l1, l2) = print_address_lines(&short);
        assert_eq!((l1.len(), l2.len()), (40, 20));
        let long: String = ('a'..='z').cycle().take(150).collect();
        let (l1, l2) = print_address_lines(&long);
        assert_eq!(format!("{l1}{l2}"), long);
        assert_eq!(l1.len(), 85);
    }

    pub(crate) fn sample() -> Form1604cDraft {
        let mut draft = Form1604cDraft {
            id: None,
            taxable_year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Taxpayer Inc".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            agent_category: Form1604cAgentCategory::Private,
            top_withholding_agent: false,
            contact_number: "0281234567".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            refunds_released: false,
            refund_date: String::new(),
            overremittance: 0.0,
            first_crediting_month: 0,
            months: Default::default(),
            total_taxes_withheld: 0.0,
            total_adjustment: 0.0,
            total_penalties: 0.0,
            total_remitted: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.months[0] = month(
            "02/10/2025",
            "Sample bank-001",
            "tra1",
            10_000.005,
            0.0,
            25.5,
        );
        draft.months[1] = month("03/10/2025", "sample bank", "tra2", 8_000.5, -500.25, 0.0);
        draft.months[11] = month(
            "01/15/2026",
            "Bank Code 12",
            "TRA-12",
            12_345.675,
            100.0,
            0.0,
        );
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1604cDraft) -> Vec<String> {
        <Form1604cDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.months[0].taxes_withheld, 10_000.0);
        assert_eq!(draft.months[0].total_remitted, 10_025.5);
        assert_eq!(draft.months[1].adjustment, -500.25);
        assert_eq!(draft.months[1].total_remitted, 7_500.25);
        assert_eq!(draft.months[11].taxes_withheld, 12_345.68);
        assert_eq!(draft.total_taxes_withheld, 30_346.18);
        assert_eq!(draft.total_adjustment, -400.25);
        assert_eq!(draft.total_penalties, 25.5);
        assert_eq!(draft.total_remitted, 29_971.43);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn negative_row_totals_are_left_out_of_the_grand_total() {
        let mut draft = sample();
        draft.months[2] = month("04/10/2025", "bank", "tra3", 100.0, -300.0, 0.0);
        draft.recompute();
        assert_eq!(draft.months[2].total_remitted, -200.0);
        assert_eq!(draft.total_remitted, 29_971.43);
        assert_eq!(
            draft.to_bir_field_map()["frm1604c:txtSched1TotalAmt3"],
            "-200.00"
        );
    }

    #[test]
    fn a_negative_sum_that_rounds_to_zero_keeps_its_sign() {
        assert_eq!(js_amount(js_cents(0.1 + 0.7 - 0.8)), "-0.00");
        assert_eq!(js_amount(js_cents(0.0)), "0.00");
        assert_eq!(js_amount(entered_signed_amount(-0.0)), "0.00");
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        assert_eq!(fields["frm1604c:txtYear"], "2025");
        assert_eq!(fields["frm1604c:rdoCode"], "039");
        assert_eq!(fields["frm1604c:registeredName"], "SAMPLE TAXPAYER INC");
        assert_eq!(fields["frm1604c:txtSched1BankVal1"], "SAMPLE BANK-001");
        assert_eq!(fields["frm1604c:txtSched1Adj2"], "-500.25");
        assert_eq!(fields["frm1604c:txtSched1Total"], "29,971.43");
        assert_eq!(fields["frm1604c:txtRefMonth"], "00");
        assert_eq!(fields["frm1604c:txt12"], "0.00");
        assert_eq!(fields["frm1604c:select13"], "00");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1604C-2025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn refund_items_follow_item_11() {
        let mut draft = sample();
        draft.refunds_released = true;
        draft.refund_date = "03/15/2026".into();
        draft.overremittance = 4_321.555;
        draft.first_crediting_month = 2;
        draft.recompute();
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1604c:txtRefMonth"], "03");
        assert_eq!(fields["frm1604c:txtRefDate"], "15");
        assert_eq!(fields["frm1604c:txtRefYear"], "2026");
        assert_eq!(fields["frm1604c:txt12"], "4,321.56");
        assert_eq!(fields["frm1604c:select13"], "02");
        draft.refunds_released = false;
        draft.recompute();
        assert!(draft.refund_date.is_empty());
        assert_eq!(draft.overremittance, 0.0);
        assert_eq!(draft.first_crediting_month, 0);
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "2025");
        assert_eq!(draft.period_column(), 0);
        assert_eq!(
            Form1604cDraft::parse_period_code("2025"),
            Some((2025, FilingPeriod::Annual))
        );
        assert_eq!(Form1604cDraft::parse_period_code("122025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1604cDraft), expected: &str| {
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
        check(
            &|d| d.taxable_year = 2999,
            "Invalid data entry on Item no. 1. Entry should be current or prior year but not be earlier than effectivity date of January 2018.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Invalid date entry on Item no.1. Entry should not be earlier than effectivity date of January 2018.",
        );
        check(
            &|d| d.taxable_year = 1899,
            "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
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
            "Please enter a valid Taxpayer's Name on Item 6.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 7.",
        );
        check(
            &|d| d.agent_category = Form1604cAgentCategory::Unspecified,
            "Please select an option for Item 8.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number on Item 9.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 10.",
        );
        let released = |d: &mut Form1604cDraft| {
            d.refunds_released = true;
            d.refund_date = "03/15/2026".into();
            d.overremittance = 10.0;
            d.first_crediting_month = 2;
        };
        check(
            &|d| {
                released(d);
                d.refund_date = "13/15/2026".into();
            },
            "Please specify date of refund in item 11A.",
        );
        check(
            &|d| {
                released(d);
                d.refund_date = "03/5/2026".into();
            },
            "Please enter a valid day on item 11A. Format should be MM/DD/YYYY.",
        );
        check(
            &|d| {
                released(d);
                d.overremittance = 0.0;
            },
            "Please enter Amount of Overremittance in Item 12.",
        );
        check(
            &|d| {
                released(d);
                d.first_crediting_month = 0;
            },
            "Please select a month from the list on Item 13.",
        );
        check(
            &|d| d.months[0].bank.clear(),
            "Incomplete values on Part II, Row 1.",
        );
        check(
            &|d| {
                d.months[4] = month("", "", "", 0.0, 0.0, 12.0);
                d.recompute();
            },
            "Incomplete values on Part II (Continuation), Row 5.",
        );
        // December, which the official loop skips.
        check(
            &|d| d.months[11].reference.clear(),
            "Incomplete values on Part II, Row 12.",
        );
        check(
            &|d| d.months[0].date = "02/30/2025".into(),
            "Please provide a valid date. (MM/DD/YYYY format)",
        );
        check(
            &|d| d.months[0].date = "12/31/2017".into(),
            "This date cannot be prior to 2018.",
        );
        check(
            &|d| d.months[0].date = "01/01/2999".into(),
            "This date cannot be a future date.",
        );
    }

    #[test]
    fn a_row_with_only_text_and_no_amount_is_complete() {
        let mut draft = sample();
        draft.months[3] = month("05/10/2025", "bank", "tra", 0.0, 0.0, 0.0);
        draft.recompute();
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        assert!(draft.revalidate_queued_before_submission().is_ok());
        draft.months[0].penalties = 99.0;
        assert!(draft.revalidate_queued_before_submission().is_err());
    }

    /// P5-1: the earlier float-tolerance check refused two-decimal
    /// amounts above about 2,000,000 (`8_816_764.47 * 100` lands 1.2e-7
    /// off a whole number); the shared `official_inputs` check accepts
    /// them again, and a three-decimal amount is still refused.
    #[test]
    fn large_two_decimal_amounts_pass_cent_precision() {
        let mut draft = sample();
        draft.months[0].taxes_withheld = 8_816_764.47;
        assert!(
            <Form1604cDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "months[0].taxes_withheld"),
            "a large two-decimal amount must be accepted"
        );
        draft.months[0].taxes_withheld = 1.005;
        assert!(
            <Form1604cDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "months[0].taxes_withheld"),
            "a three-decimal amount must still be refused"
        );
    }
}
