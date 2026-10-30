//! BIR Form 1600-WP — Remittance Return of Percentage Tax on Winnings and
//! Prizes Withheld by Race Track Operators (v2010).
//!
//! Ported from the official `BIR-Form1600WP.hta` (eBIRForms 7.9.6.2.1): the
//! ATC popups (`changedrpATCList`, `getATCCode`, `getSchedIIATCCode`), the
//! compute chain (`getRequiredWithheld`, `computeofTotalWithheldTax`,
//! `computePenalties`, `computeOfTotalAmtDue`, `computeDtShedTaxWithheld`),
//! `validate` / `initialValidateBeforeSave` with their exact alert texts, and
//! `saveXMLsubmit` through [`crate::official_xml`].
//!
//! The page serializes both ATC popups (two ATCs per category, inside the
//! form) and one Part II row per ticked ATC, so the submit layout depends on
//! how many ATCs are ticked: `1600wp-v2010-atc0`, `-atc1` or `-atc2`.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout (before Item 7 is answered).
pub const FORM_1600WP_FORM_ID: &str = "1600wp-v2010";
/// Submit layouts by the number of ticked Part II ATCs.
pub const FORM_1600WP_LAYOUT_IDS: [&str; 3] = [
    "1600wp-v2010-atc0",
    "1600wp-v2010-atc1",
    "1600wp-v2010-atc2",
];
/// Schedule II rows.
pub const FORM_1600WP_SCHEDULE_ROWS: usize = 10;
const PREFIX: &str = "frm1600WP";

/// Item 7 (`CategoryAgent_P` / `CategoryAgent_G`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1600WpAgentCategory {
    Private,
    Government,
}

/// One ATC the official popups offer (`atcCodes.xml` lines tagged `1600WP`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Form1600WpAtcOption {
    pub code: &'static str,
    /// Rate text the popup shows, e.g. `"4.0"`.
    pub rate_text: &'static str,
    pub rate: f64,
    pub category: Form1600WpAgentCategory,
    pub description: &'static str,
}

/// The official list in `atcList` order.
pub const FORM_1600WP_ATC_OPTIONS: &[Form1600WpAtcOption] = &[
    Form1600WpAtcOption {
        code: "WB191",
        rate_text: "4.0",
        rate: 4.0,
        category: Form1600WpAgentCategory::Government,
        description: "TAX ON WINNINGS FROM DOUBLE, FORECAST/QUINELLA AND TRIFECTA BETS ON HORSE RACES PAID BY GOVERNMENT WITHHOLDING TAX AGENT",
    },
    Form1600WpAtcOption {
        code: "WB192",
        rate_text: "10.0",
        rate: 10.0,
        category: Form1600WpAgentCategory::Government,
        description: "TAX ON WINNINGS OR PRIZES PAID TO WINNERS OF WINNING HORSE RACE TICKETS OTHER THAN DOUBLE, FORECAST/QUINELLA AND TRIFECTA BETS; AND OWNERS OF WINNING RACE HORSES PAID BY GOVERNMENT WITHHOLDING TAX AGENT",
    },
    Form1600WpAtcOption {
        code: "WB193",
        rate_text: "4.0",
        rate: 4.0,
        category: Form1600WpAgentCategory::Private,
        description: "TAX ON WINNINGS FROM DOUBLE, FORECAST/QUINELLA AND TRIFECTA BETS ON HORSE RACES PAID BY PRIVATE WITHHOLDING TAX AGENT",
    },
    Form1600WpAtcOption {
        code: "WB194",
        rate_text: "10.0",
        rate: 10.0,
        category: Form1600WpAgentCategory::Private,
        description: "TAX ON WINNINGS OR PRIZES PAID TO WINNERS OF WINNING HORSE RACE TICKETS OTHER THAN DOUBLE, FORECAST/QUINELLA AND TRIFECTA BETS; AND OWNERS OF WINNING RACE HORSES PAID BY PRIVATE WITHHOLDING TAX AGENT",
    },
];

/// The popup rows for a category, in list order (`AtcCd1..2`).
pub fn form_1600wp_popup(
    category: Form1600WpAgentCategory,
) -> impl Iterator<Item = &'static Form1600WpAtcOption> {
    FORM_1600WP_ATC_OPTIONS
        .iter()
        .filter(move |option| option.category == category)
}

pub fn form_1600wp_atc(code: &str) -> Option<&'static Form1600WpAtcOption> {
    FORM_1600WP_ATC_OPTIONS
        .iter()
        .find(|option| option.code == code)
}

/// One Part II row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1600WpAtcRow {
    pub atc_code: String,
    /// Held at cents: `round(this,2)` runs before `getRequiredWithheld`.
    pub tax_base: f64,
    pub tax_withheld: f64,
}

/// One Schedule II row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1600WpScheduleRow {
    /// Payee TIN, digits only (12 to 14 digits).
    pub tin: String,
    pub payee_name: String,
    pub atc_code: String,
    pub amount: f64,
    pub tax_withheld: f64,
}

impl Form1600WpScheduleRow {
    pub fn is_blank(&self) -> bool {
        self.tin.trim().is_empty()
            && self.payee_name.trim().is_empty()
            && self.atc_code.is_empty()
            && self.amount == 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1600WpDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–4
    pub month: u8,
    pub day: u8,
    pub year: u16,
    pub is_amended: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,
    /// Item 4; `None` until answered (the page checks neither box).
    #[serde(default)]
    pub taxes_withheld: Option<bool>,

    // Part I
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    pub rdo_code: String,
    #[serde(default)]
    pub agent_category: Option<Form1600WpAgentCategory>,
    pub agent_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub email: String,

    // Part II
    #[serde(default)]
    pub atc_rows: Vec<Form1600WpAtcRow>,
    /// Item 12.
    #[serde(default)]
    pub total_tax_withheld: f64,
    /// Item 13, amended returns only.
    #[serde(default)]
    pub tax_remitted_previous: f64,
    /// Item 14.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 15A–15D.
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
    /// Item 16.
    #[serde(default)]
    pub total_amount_due: f64,

    // Schedule II
    #[serde(default)]
    pub schedule: Vec<Form1600WpScheduleRow>,
    #[serde(default)]
    pub schedule_total: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// JavaScript `Number.prototype.toFixed(2)`: the exact binary value rounded
/// half up on its magnitude.
fn js_to_fixed2(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let text = format!("{:.60}", value.abs());
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let digits = fraction.as_bytes();
    let mut kept: u64 = whole.parse::<u64>().unwrap_or(0) * 100
        + u64::from(digits[0] - b'0') * 10
        + u64::from(digits[1] - b'0');
    if digits[2] >= b'5' {
        kept += 1;
    }
    let magnitude = kept as f64 / 100.0;
    if value < 0.0 { -magnitude } else { magnitude }
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

/// `capital()` uppercases every text input except `txtEmail`.
fn text(value: &str) -> String {
    value.trim().to_uppercase()
}

impl Form1600WpDraft {
    pub const FORM_CODE: &'static str = "1600WP";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        let month = month.clamp(1, 12);
        // The official page fills Item 1's day with today's date
        // (`year.getDate()`); keep it inside the chosen month.
        let today = chrono::Datelike::day(&chrono::Local::now().date_naive()) as u8;
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            month,
            day: today.min(days_in_month(month, year)),
            year,
            is_amended: false,
            number_of_attached_sheets: 0,
            taxes_withheld: None,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            agent_category: None,
            agent_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            email: profile.email.clone(),
            atc_rows: Vec::new(),
            total_tax_withheld: 0.0,
            tax_remitted_previous: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_due: 0.0,
            schedule: Vec::new(),
            schedule_total: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 7. A different category offers other ATCs; the ticked ones and
    /// Schedule II ATCs no longer apply and are cleared.
    pub fn set_agent_category(&mut self, category: Form1600WpAgentCategory) {
        if self.agent_category != Some(category) {
            self.atc_rows.clear();
            for row in &mut self.schedule {
                row.atc_code.clear();
            }
        }
        self.agent_category = Some(category);
        self.recompute();
    }

    /// Item 4. "No" clears Part II and Schedule II (`cancelAllCompute`).
    pub fn set_taxes_withheld(&mut self, withheld: bool) {
        self.taxes_withheld = Some(withheld);
        if !withheld {
            self.atc_rows.clear();
            self.schedule.clear();
        }
        self.recompute();
    }

    /// Tick or untick an ATC in the Part II popup; rows stay in list order.
    pub fn toggle_atc(&mut self, code: &str) -> Result<(), String> {
        let option = form_1600wp_atc(code).ok_or_else(|| "Unknown ATC.".to_string())?;
        if self.taxes_withheld != Some(true) {
            return Err(match self.taxes_withheld {
                None => "Please select an option for Item 4.".to_string(),
                _ => {
                    "Selecting an ATC is not necessary when item no. 4 is set to ' NO '".to_string()
                }
            });
        }
        let Some(category) = self.agent_category else {
            return Err("Please select an option for Item 7.".to_string());
        };
        if option.category != category {
            return Err(format!(
                "{} is not offered to this category of withholding agent.",
                option.code
            ));
        }
        if let Some(at) = self.atc_rows.iter().position(|row| row.atc_code == code) {
            self.atc_rows.remove(at);
        } else {
            self.atc_rows.push(Form1600WpAtcRow {
                atc_code: option.code.to_string(),
                tax_base: 0.0,
                tax_withheld: 0.0,
            });
            self.atc_rows.sort_by_key(|row| {
                FORM_1600WP_ATC_OPTIONS
                    .iter()
                    .position(|o| o.code == row.atc_code)
            });
        }
        self.recompute();
        Ok(())
    }

    /// The official compute chain.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_remitted_previous = 0.0;
        }
        let mut total = 0.0;
        for row in &mut self.atc_rows {
            row.tax_base = cents(row.tax_base);
            let rate = form_1600wp_atc(&row.atc_code)
                .map(|o| o.rate)
                .unwrap_or(0.0);
            // getRequiredWithheld: base * (rate / 100).
            row.tax_withheld = cents(row.tax_base * (rate / 100.0));
            total += row.tax_withheld;
        }
        self.total_tax_withheld = cents(total);
        self.tax_remitted_previous = cents(self.tax_remitted_previous);
        self.tax_still_due = cents(self.total_tax_withheld - self.tax_remitted_previous);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_due = cents(self.total_penalties + self.tax_still_due);

        // computeDtShedTaxWithheld: (amount * (rate / 100).toFixed(2)).toFixed(2).
        let mut total = 0.0;
        for row in &mut self.schedule {
            row.amount = cents(row.amount);
            let rate = form_1600wp_atc(&row.atc_code)
                .map(|o| o.rate)
                .unwrap_or(0.0);
            row.tax_withheld = cents(js_to_fixed2(row.amount * js_to_fixed2(rate / 100.0)));
            total = js_to_fixed2(total + row.tax_withheld);
        }
        self.schedule_total = cents(total);
    }

    fn date_code(&self) -> String {
        format!("{:02}{:02}{}", self.month, self.day, self.year)
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("{PREFIX}:{key}"), value);
        };
        let flag = |on: bool| on.to_string();

        // The "to" date copies the "from" date on blur.
        for prefix in ["DateWithholding", "DateWithholdingTo"] {
            put(&format!("{prefix}Month"), format!("{:02}", self.month));
            put(&format!("{prefix}Day"), format!("{:02}", self.day));
            put(&format!("{prefix}Year"), self.year.to_string());
        }
        put("AmendedReturn_1", flag(self.is_amended));
        put("AmendedReturn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());
        put("AnyTaxHeld_1", flag(self.taxes_withheld == Some(true)));
        put("AnyTaxHeld_2", flag(self.taxes_withheld == Some(false)));
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtTIN1", tin1);
        put("txtTIN2", tin2);
        put("txtTIN3", tin3);
        put("txtBranchCode", branch);
        put("txtRDOCode", self.rdo_code.trim().to_string());
        put(
            "CategoryAgent_P",
            flag(self.agent_category == Some(Form1600WpAgentCategory::Private)),
        );
        put(
            "CategoryAgent_G",
            flag(self.agent_category == Some(Form1600WpAgentCategory::Government)),
        );
        put("txtTaxpayerName", text(&self.agent_name));
        put("txtAddress", text(&self.registered_address));
        put("txtZipCode", self.zip_code.trim().to_string());

        for (index, row) in self.atc_rows.iter().enumerate() {
            let n = index + 1;
            let rate = form_1600wp_atc(&row.atc_code)
                .map(|o| o.rate_text)
                .unwrap_or("");
            put(&format!("txtAtcCode{n}"), row.atc_code.clone());
            put(&format!("txtTaxBase{n}"), official_amount(row.tax_base));
            // The popup cell is "<td> 4.0</td>"; its innerHTML keeps the space.
            put(&format!("txtTaxRate{n}"), format!(" {rate}"));
            put(
                &format!("txtTaxbeWithHeld{n}"),
                official_amount(row.tax_withheld),
            );
        }
        put("txtTax12", official_amount(self.total_tax_withheld));
        put("txtTax13", official_amount(self.tax_remitted_previous));
        put("txtTax14", official_amount(self.tax_still_due));
        put("txtTax15A", official_amount(self.surcharge));
        put("txtTax15B", official_amount(self.interest));
        put("txtTax15C", official_amount(self.compromise));
        put("txtTax15D", official_amount(self.total_penalties));
        put("txtTax16", official_amount(self.total_amount_due));

        for index in 0..FORM_1600WP_SCHEDULE_ROWS {
            let n = index + 1;
            let row = self.schedule.get(index).cloned().unwrap_or_default();
            let option = form_1600wp_atc(&row.atc_code);
            put(&format!("dtSched:txtTin{n}"), row.tin.trim().to_string());
            put(&format!("dtSched:txtFullname{n}"), text(&row.payee_name));
            put(&format!("dtSched:drpAtcCode{n}"), row.atc_code.clone());
            // The popup cell's innerHTML: the description and a space.
            put(
                &format!("dtSched:naturePayment{n}"),
                option
                    .map(|o| format!("{} ", o.description))
                    .unwrap_or_default(),
            );
            put(
                &format!("dtSched:amount{n}"),
                if row.amount == 0.0 && row.is_blank() {
                    String::new()
                } else {
                    official_amount(row.amount)
                },
            );
            put(
                &format!("dtSched:txtRatePercent{n}"),
                option
                    .map(|o| format!(" {}", o.rate_text))
                    .unwrap_or_default(),
            );
            put(
                &format!("dtSched:taxWithheld{n}"),
                official_amount(row.tax_withheld),
            );
        }
        put(
            "dtSched:TotaltaxWithheld",
            official_amount(self.schedule_total),
        );

        // The popups' own controls are serialized too.
        if let Some(category) = self.agent_category {
            // The Schedule II popup is one radio group: the ATC picked last
            // stays checked (taken here as the last filled row's).
            let last_sched = self
                .schedule
                .iter()
                .rev()
                .find(|row| !row.atc_code.is_empty())
                .map(|row| row.atc_code.clone());
            for (n, option) in form_1600wp_popup(category).enumerate() {
                let ticked = self.atc_rows.iter().any(|row| row.atc_code == option.code);
                fields.insert(format!("AtcCd{}", n + 1), ticked.to_string());
                fields.insert(
                    format!("SchedIIAtcCde{}", n + 1),
                    (last_sched.as_deref() == Some(option.code)).to_string(),
                );
            }
        }
        // validate() records the Part II row count.
        fields.insert(
            "hPartIITableSize".to_string(),
            self.atc_rows.len().to_string(),
        );
        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The field map plus print-only values the frozen 2010 sheet needs
    /// (`derived:` keys, never submitted): the "from" date in one comb, Part II
    /// amounts on the sheet's fixed row for each ATC, and the attachment
    /// header (period and TIN).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("derived:{key}"), value);
        };
        put("from_date", self.date_code());
        // A row without an ATC has no fixed row on the sheet to print on.
        for row in self
            .atc_rows
            .iter()
            .filter(|row| !row.atc_code.trim().is_empty())
        {
            put(
                &format!("{}_base", row.atc_code),
                official_amount(row.tax_base),
            );
            put(
                &format!("{}_withheld", row.atc_code),
                official_amount(row.tax_withheld),
            );
        }
        for side in ["from", "to"] {
            put(&format!("att_{side}_mm"), format!("{:02}", self.month));
            put(&format!("att_{side}_dd"), format!("{:02}", self.day));
            put(&format!("att_{side}_yyyy"), self.year.to_string());
        }
        let (a, b, c, d) = split_tin(&self.tin);
        put("att_tin1", a);
        put("att_tin2", b);
        put("att_tin3", c);
        put("att_branch", d);
        fields
    }

    /// The submit layout for the number of Part II rows.
    pub fn layout_id(&self) -> &'static str {
        FORM_1600WP_LAYOUT_IDS[self.atc_rows.len().min(2)]
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

impl FormValidator for Form1600WpDraft {
    /// `validate` in order, with its alert texts, then
    /// `initialValidateBeforeSave`, plus the input limits the page enforces
    /// while typing and a few stricter checks (noted inline).
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // Item 1.
        if !(1..=12).contains(&self.month) {
            err("month", "Please enter a valid month on Item 1.");
        }
        if self.day == 0 {
            err("day", "Please enter a valid day on Item 1.");
        }
        if self.year == 0 {
            err("year", "Please enter a valid year on Item 1.");
        } else if self.year < 1900 || self.year > 9999 {
            err(
                "year",
                "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
            );
        }
        if self.month == 2 && self.day == 29 && days_in_month(2, self.year) == 28 {
            err("day", "Filing year is not a leap year.");
        } else if (1..=12).contains(&self.month) && self.day > days_in_month(self.month, self.year)
        {
            err("day", "Invalid date entry on item 1.");
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

        // Part I.
        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        let tin_digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        if tin1.len() != 3
            || tin2.len() != 3
            || tin3.len() != 3
            || tin_digits.len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 5.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 6.");
        }
        if self.agent_category.is_none() {
            err("agent_category", "Please select an option for Item 7.");
        }
        let name = self.agent_name.trim();
        if name.is_empty() {
            err(
                "agent_name",
                "Please enter a valid Withholding Agent's Name on Item 8.",
            );
        } else if name.chars().count() > 50 {
            err("agent_name", "Item 8 holds at most 50 characters.");
        }
        let address = self.registered_address.trim();
        if address.is_empty() {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 9.",
            );
        } else if address.chars().count() > 150 {
            err("registered_address", "Item 9 holds at most 150 characters.");
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 10.");
        } else if zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Item 10 holds at most 12 digits.");
        }

        // Part II.
        if self.taxes_withheld == Some(true) {
            if self.atc_rows.is_empty() {
                err(
                    "atc_rows",
                    "Please fill up Part II Computation of Tax if item 4 is set to Yes.",
                );
            }
            for (index, row) in self.atc_rows.iter().enumerate() {
                if row.tax_base <= 0.0 {
                    err(
                        &format!("atc_rows[{index}].tax_base"),
                        &format!("Please enter Tax Base for ATC <{}>.", row.atc_code),
                    );
                }
            }
            for (index, row) in self.schedule.iter().enumerate() {
                let n = index + 1;
                if row.is_blank() {
                    continue;
                }
                let tin = row.tin.trim();
                if tin.len() < 12 || tin.len() > 14 || !digits_only(tin) {
                    err(
                        &format!("schedule[{index}].tin"),
                        &format!("Please enter a valid TIN Number for Sequence {n}."),
                    );
                } else if row.payee_name.trim().is_empty() {
                    err(
                        &format!("schedule[{index}].payee_name"),
                        &format!(
                            "Please enter a valid Name of Individual/Corporation for Sequence {n}."
                        ),
                    );
                } else if row.atc_code.is_empty() {
                    err(
                        &format!("schedule[{index}].atc_code"),
                        &format!("Please select an ATC from the list for Sequence {n}."),
                    );
                } else if row.amount <= 0.0 {
                    err(
                        &format!("schedule[{index}].amount"),
                        &format!(
                            "Please enter Tax Base for ATC {}. Value must be greater than 0.",
                            row.atc_code
                        ),
                    );
                }
            }
        } else if !self.atc_rows.is_empty() || !self.schedule.is_empty() {
            err(
                "atc_rows",
                "Selecting an ATC is not necessary when item no. 4 is set to ' NO '",
            );
        }
        for (index, row) in self.atc_rows.iter().enumerate() {
            match (form_1600wp_atc(&row.atc_code), self.agent_category) {
                (Some(option), Some(category)) if option.category == category => {}
                _ => err(
                    &format!("atc_rows[{index}].atc_code"),
                    &format!(
                        "{} is not offered to this category of withholding agent.",
                        row.atc_code
                    ),
                ),
            }
            if self.atc_rows[..index]
                .iter()
                .any(|other| other.atc_code == row.atc_code)
            {
                err(
                    &format!("atc_rows[{index}].atc_code"),
                    "Each ATC is selected once.",
                );
            }
            if !has_cent_precision(row.tax_base) || row.tax_base >= 1e14 {
                err(
                    &format!("atc_rows[{index}].tax_base"),
                    "Item 12 tax bases hold at most 14 digits before the decimal point.",
                );
            }
        }
        if self.schedule.len() > FORM_1600WP_SCHEDULE_ROWS {
            err("schedule", "Schedule II has ten rows.");
        }
        for (index, row) in self.schedule.iter().enumerate() {
            let n = index + 1;
            let tin = row.tin.trim();
            if !tin.is_empty() && tin.len() < 12 {
                err(
                    &format!("schedule[{index}].tin"),
                    &format!("Please enter a valid TIN Number for Sequence {n}."),
                );
            }
            if row.payee_name.trim().chars().count() > 50 {
                err(
                    &format!("schedule[{index}].payee_name"),
                    "Schedule II names hold at most 50 characters.",
                );
            }
            if !row.atc_code.is_empty() {
                match (form_1600wp_atc(&row.atc_code), self.agent_category) {
                    (Some(option), Some(category)) if option.category == category => {}
                    _ => err(
                        &format!("schedule[{index}].atc_code"),
                        &format!("Please select an ATC from the list for Sequence {n}."),
                    ),
                }
            }
            if row.amount < 0.0 || row.amount >= 1e14 {
                err(
                    &format!("schedule[{index}].amount"),
                    "Schedule II amounts hold at most 14 digits before the decimal point.",
                );
            }
        }
        for (field, value) in [
            ("tax_remitted_previous", self.tax_remitted_previous),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ] {
            if value < 0.0 || value >= 1e15 || !has_cent_precision(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }
        if !self.is_amended && self.tax_remitted_previous != 0.0 {
            err(
                "tax_remitted_previous",
                "Item 13 applies only to an amended return.",
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

impl Form1600WpDraft {
    /// Items 15A-15C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: Withholding,
            items: [
                surcharge: "15A Surcharge",
                interest: "15B Interest",
                compromise: "15C Compromise",
            ],
            controls: [
                "frm1600WP:txtTax15A",
                "frm1600WP:txtTax15B",
                "frm1600WP:txtTax15C",
            ],
            state: penalty,
            basic_tax_due: "Item 14", |d| d.tax_still_due,
        )
    }
}

impl QueueableForm for Form1600WpDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1600WP";
    /// Official `formType` (`1600WP`).
    const FORM_TYPE: &'static str = "1600WP";
    /// The two-ATC layout; [`Self::official_payload`] picks the one for the
    /// number of Part II rows.
    const LAYOUT_ID: &'static str = "1600wp-v2010-atc2";

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
        self.year
    }
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Monthly(self.month)
    }
    /// `MM + DD + YYYY` of Item 1, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        self.date_code()
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 8 || !digits_only(code) {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let day: u8 = code.get(2..4)?.parse().ok()?;
        let year: u16 = code.get(4..)?.parse().ok()?;
        ((1..=12).contains(&month) && (1..=31).contains(&day))
            .then_some((year, FilingPeriod::Monthly(month)))
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
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as FormValidator>::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let layout = crate::official_xml::layout(self.layout_id())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])?;
        crate::official_xml::write(layout, &self.field_map())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Form1600WpDraft {
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Sample Racing Club",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Race track",
            "registered_address": "123 Sample St Quezon City",
            "zip_code": "1100",
            "phone": "0281234567",
            "email": "sample.club@example.com",
            "default_form_type": "1600WP",
            "taxpayer_type": "Corporation"
        }))
        .unwrap();
        let mut draft = Form1600WpDraft::new_from_profile(&profile, 2025, 6);
        draft.day = 15;
        draft.set_taxes_withheld(true);
        draft.set_agent_category(Form1600WpAgentCategory::Private);
        draft.toggle_atc("WB194").unwrap();
        draft.toggle_atc("WB193").unwrap();
        draft.atc_rows[0].tax_base = 10_000.005;
        draft.atc_rows[1].tax_base = 2_500.0;
        draft.schedule.push(Form1600WpScheduleRow {
            tin: "987654321000".into(),
            payee_name: "Dela Cruz, Juan".into(),
            atc_code: "WB194".into(),
            amount: 2_500.0,
            tax_withheld: 0.0,
        });
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1600WpDraft) -> Vec<String> {
        <Form1600WpDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        // List order: WB193 (4%) before WB194 (10%). 10,000.005 is stored
        // as 10,000.00499…, which round() (like the official page) keeps at
        // 10,000.00.
        assert_eq!(draft.atc_rows[0].atc_code, "WB193");
        assert_eq!(draft.atc_rows[0].tax_base, 10_000.0);
        assert_eq!(draft.atc_rows[0].tax_withheld, 400.0);
        assert_eq!(draft.atc_rows[1].tax_withheld, 250.0);
        assert_eq!(draft.total_tax_withheld, 650.0);
        assert_eq!(draft.total_amount_due, 650.0);
        assert_eq!(draft.schedule[0].tax_withheld, 250.0);
        assert_eq!(draft.schedule_total, 250.0);
    }

    #[test]
    fn field_map_uses_official_formats() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1600WP:DateWithholdingToDay"], "15");
        assert_eq!(fields["frm1600WP:txtTaxRate1"], " 4.0");
        assert_eq!(fields["frm1600WP:txtTaxBase1"], "10,000.00");
        assert_eq!(fields["frm1600WP:dtSched:txtRatePercent1"], " 10.0");
        assert!(fields["frm1600WP:dtSched:naturePayment1"].ends_with("AGENT "));
        assert_eq!(fields["frm1600WP:dtSched:amount2"], "");
        assert_eq!(fields["AtcCd2"], "true");
        assert_eq!(fields["SchedIIAtcCde2"], "true");
        assert_eq!(fields["SchedIIAtcCde1"], "false");
        assert_eq!(fields["hPartIITableSize"], "2");
        assert_eq!(draft.layout_id(), "1600wp-v2010-atc2");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1600WP-06152025#sample.club@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn print_map_places_part_ii_rows_on_their_atc_rows_only() {
        let mut draft = sample();
        draft.atc_rows[1].atc_code.clear();
        let fields = draft.to_print_field_map();
        assert_eq!(fields["derived:WB193_base"], "10,000.00");
        assert!(!fields.contains_key("derived:WB194_base"));
        assert!(!fields.contains_key("derived:_base"));
        assert!(!fields.contains_key("derived:_withheld"));
    }

    #[test]
    fn period_codes_round_trip() {
        assert_eq!(sample().period_code(), "06152025");
        assert_eq!(
            Form1600WpDraft::parse_period_code("06152025"),
            Some((2025, FilingPeriod::Monthly(6)))
        );
        assert_eq!(Form1600WpDraft::parse_period_code("13152025"), None);
        assert_eq!(Form1600WpDraft::parse_period_code("062025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1600WpDraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(&|d| d.month = 0, "Please enter a valid month on Item 1.");
        check(&|d| d.day = 0, "Please enter a valid day on Item 1.");
        check(&|d| d.year = 0, "Please enter a valid year on Item 1.");
        check(&|d| d.day = 31, "Invalid date entry on item 1.");
        check(
            &|d| {
                d.month = 2;
                d.day = 29;
            },
            "Filing year is not a leap year.",
        );
        check(
            &|d| d.year = 1899,
            "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
        );
        check(
            &|d| d.taxes_withheld = None,
            "Please select an option for Item 4.",
        );
        check(
            &|d| d.tin = "123".into(),
            "Please enter a valid TIN number on Item 5.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 6.",
        );
        check(
            &|d| d.agent_category = None,
            "Please select an option for Item 7.",
        );
        check(
            &|d| d.agent_name.clear(),
            "Please enter a valid Withholding Agent's Name on Item 8.",
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
            &|d| {
                d.atc_rows.clear();
                d.recompute();
            },
            "Please fill up Part II Computation of Tax if item 4 is set to Yes.",
        );
        check(
            &|d| {
                d.atc_rows[0].tax_base = 0.0;
                d.recompute();
            },
            "Please enter Tax Base for ATC <WB193>.",
        );
        check(
            &|d| d.schedule[0].tin = "12345".into(),
            "Please enter a valid TIN Number for Sequence 1.",
        );
        check(
            &|d| d.schedule[0].payee_name.clear(),
            "Please enter a valid Name of Individual/Corporation for Sequence 1.",
        );
        check(
            &|d| {
                d.schedule[0].atc_code.clear();
                d.recompute();
            },
            "Please select an ATC from the list for Sequence 1.",
        );
        check(
            &|d| {
                d.schedule[0].amount = 0.0;
                d.recompute();
            },
            "Please enter Tax Base for ATC WB194. Value must be greater than 0.",
        );
        check(
            &|d| d.taxes_withheld = Some(false),
            "Selecting an ATC is not necessary when item no. 4 is set to ' NO '",
        );
        check(
            &|d| d.surcharge = 1.0,
            "Totals are out of date. Recompute the return.",
        );
    }

    #[test]
    fn atc_popup_rules() {
        let mut draft = sample();
        assert!(draft.toggle_atc("WB191").is_err());
        draft.set_agent_category(Form1600WpAgentCategory::Government);
        assert!(draft.atc_rows.is_empty());
        assert!(draft.schedule[0].atc_code.is_empty());
        draft.toggle_atc("WB191").unwrap();
        assert_eq!(draft.layout_id(), "1600wp-v2010-atc1");
        draft.set_taxes_withheld(false);
        assert!(draft.atc_rows.is_empty() && draft.schedule.is_empty());
        assert_eq!(draft.layout_id(), "1600wp-v2010-atc0");
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
            <Form1600WpDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form1600WpDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
