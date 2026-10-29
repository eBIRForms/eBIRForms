//! BIR Form 1603-Q — Quarterly Remittance Return of Final Income Taxes
//! Withheld on Fringe Benefits Paid to Employees Other Than Rank and File,
//! January 2018 (ENCS).
//!
//! Ported from the official `BIR-Form1603Qv2018.hta` (eBIRForms 7.9.6.2.1):
//! Schedule 1 (`computeTaxBase360` / `computeTaxBase330` and their tax
//! steps), the compute chain (`computeTax17` … `computeTotalTaxStillDue`),
//! `validateYear` / `validateForm` with their exact alert texts, and the
//! uploaded file `saveEncryptedProfile` writes (`data/official-xml/1603q-v2018.json`).
//!
//! - `getRdo()` replaces the static RDO select with one whose id is
//!   `frm1603Q:rdoCode`, so that is the key the page submits;
//! - the Schedule 1 percentage divisors and tax rates come from
//!   `xml/taxRate.xml` through `js/tax-rate-helper.js` (`65%`, `35%`, `75%`,
//!   `25%`), exactly as the helper writes them into the inputs.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1603Q_FORM_ID: &str = "1603q-v2018";

/// One Schedule 1 line: ATC, percentage divisor and tax rate as
/// `taxRate.xml` writes them into the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form1603QScheduleLine {
    pub atc_code: &'static str,
    pub description: &'static str,
    pub divisor_text: &'static str,
    pub rate_text: &'static str,
}

impl Form1603QScheduleLine {
    fn percent(text: &str) -> f64 {
        text.trim_end_matches('%').trim().parse().unwrap_or(0.0)
    }
    pub fn divisor(&self) -> f64 {
        Self::percent(self.divisor_text)
    }
    pub fn rate(&self) -> f64 {
        Self::percent(self.rate_text)
    }
}

/// Schedule 1 lines 1 (WF360) and 2 (WF330).
pub const FORM_1603Q_SCHEDULE_LINES: [Form1603QScheduleLine; 2] = [
    Form1603QScheduleLine {
        atc_code: "WF360",
        description: "Fringe benefits to employees other than rank and file (citizens, residents, NRAETB)",
        divisor_text: "65%",
        rate_text: "35%",
    },
    Form1603QScheduleLine {
        atc_code: "WF330",
        description: "Fringe benefits to NRANETB and to alien employees of OBUs, foreign petroleum service contractors and regional or area headquarters",
        divisor_text: "75%",
        rate_text: "25%",
    },
];

/// Item 11, category of withholding agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1603QCategory {
    Private,
    Government,
}

/// Item 13A (`selTreaty`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1603QTaxRelief {
    /// Item 13 "Yes" with the blank option (`enableSelTreaty` leaves it blank).
    #[default]
    Unspecified,
    SpecialRate,
    InternationalTaxTreaty,
    Both,
}

impl Form1603QTaxRelief {
    fn option_value(self) -> &'static str {
        match self {
            Self::Unspecified => "0",
            Self::SpecialRate => "1",
            Self::InternationalTaxTreaty => "2",
            Self::Both => "3",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1603QDraft {
    #[serde(default)]
    pub id: Option<i64>,

    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    /// Item 1.
    pub taxable_year: u16,
    /// Item 2 (1–4); 0 while unanswered.
    pub quarter: u8,
    /// Item 3.
    pub is_amended: bool,
    /// Item 4, "Any taxes withheld?"; `None` while unanswered.
    #[serde(default)]
    pub any_tax_withheld: Option<bool>,
    /// Item 5.
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
    pub rdo_code: String,
    /// Not printed on the form; the page keeps it in a hidden field.
    pub line_of_business: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    #[serde(default)]
    pub category: Option<Form1603QCategory>,
    pub email: String,
    /// Items 13/13A: `None` is "No".
    #[serde(default)]
    pub tax_relief: Option<Form1603QTaxRelief>,

    // Part IV — Schedule 1 (column B: monetary value of fringe benefits)
    #[serde(default)]
    pub monetary_value: [f64; 2],
    /// Column E (computed).
    #[serde(default)]
    pub grossed_up_value: [f64; 2],
    /// Column G (computed).
    #[serde(default)]
    pub fringe_benefit_tax: [f64; 2],
    #[serde(default)]
    pub schedule_total: f64,

    // Part II
    /// Item 14.
    #[serde(default)]
    pub total_taxes_withheld: f64,
    /// Item 15.
    #[serde(default)]
    pub tax_remitted_previous: f64,
    /// Item 16 (specify / amount).
    #[serde(default)]
    pub other_remittances_specify: String,
    #[serde(default)]
    pub other_remittances: f64,
    /// Item 17.
    #[serde(default)]
    pub total_remittances: f64,
    /// Item 18.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 19–22.
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
    pub total_amount_due: f64,

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

/// `round()` keeps at most 12 integer digits.
const MAX_AMOUNT: f64 = 1e12;

impl Form1603QDraft {
    pub const FORM_CODE: &'static str = "1603Q";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            quarter,
            is_amended: false,
            any_tax_withheld: None,
            number_of_attached_sheets: 0,
            rdo_code: profile.rdo_code.clone(),
            line_of_business: profile.line_of_business.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            category: None,
            email: profile.email.clone(),
            tax_relief: None,
            monetary_value: [0.0; 2],
            grossed_up_value: [0.0; 2],
            fringe_benefit_tax: [0.0; 2],
            schedule_total: 0.0,
            total_taxes_withheld: 0.0,
            tax_remitted_previous: 0.0,
            other_remittances_specify: String::new(),
            other_remittances: 0.0,
            total_remittances: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_due: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 4. Like `changeTaxWitheld`, answering it clears every computed
    /// amount (and "No" closes Schedule 1).
    pub fn set_any_tax_withheld(&mut self, withheld: bool) {
        if self.any_tax_withheld != Some(withheld) {
            self.monetary_value = [0.0; 2];
            self.tax_remitted_previous = 0.0;
            self.other_remittances = 0.0;
            self.surcharge = 0.0;
            self.interest = 0.0;
            self.compromise = 0.0;
            self.penalty.clear_overrides();
        }
        self.any_tax_withheld = Some(withheld);
        self.recompute();
    }

    /// The official compute chain over values held at cents.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_remitted_previous = 0.0;
        }
        let mut total = 0.0;
        for (index, line) in FORM_1603Q_SCHEDULE_LINES.iter().enumerate() {
            self.monetary_value[index] = cents(self.monetary_value[index]);
            // computeTaxBase360/330: value / (divisor / 100).
            self.grossed_up_value[index] =
                cents(self.monetary_value[index] / (line.divisor() / 100.0));
            // compute360/330: base * rate / 100.
            self.fringe_benefit_tax[index] =
                cents(self.grossed_up_value[index] * line.rate() / 100.0);
            total += self.fringe_benefit_tax[index];
        }
        self.schedule_total = cents(total);
        self.total_taxes_withheld = self.schedule_total;
        self.tax_remitted_previous = cents(self.tax_remitted_previous);
        self.other_remittances = cents(self.other_remittances);
        self.total_remittances = cents(self.tax_remitted_previous + self.other_remittances);
        self.tax_still_due = cents(self.total_taxes_withheld - self.total_remittances);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_due = cents(self.tax_still_due + self.total_penalties);
    }

    /// Page 2 header name: `loadBGData` keeps the part before the first comma.
    fn page2_name(&self) -> String {
        self.taxpayer_name
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_uppercase()
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the later pages' header TIN, one 14-digit
    ///   comb (TIN then branch).
    /// - `derived:address_line1` / `derived:line:address_line2`: the
    ///   registered address over its 40-slot first line, broken at a
    ///   space, and the second line (printed whole when longer).
    /// - `derived:line:tax_relief_specify`: "If yes, specify", the relief
    ///   named by `frm1603Q:selTreaty` (the upload carries the dropdown code).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm1603Q:txtPg2TIN1",
                "frm1603Q:txtPg2TIN2",
                "frm1603Q:txtPg2TIN3",
                "frm1603Q:txtPg2BranchCode",
            ],
        );
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        let specify = super::print_map::tax_relief_label(
            fields.get("frm1603Q:selTreaty").map_or("", String::as_str),
        )
        .to_string();
        for (key, value) in [
            ("tin_digits", tin_digits),
            ("address_line1", line1),
            ("line:address_line2", line2),
            ("line:tax_relief_specify", specify),
        ] {
            fields.insert(format!("derived:{key}"), value);
        }
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(key.to_string(), value);
        };
        let p = |key: &str| format!("frm1603Q:{key}");
        let flag = |on: bool| on.to_string();
        // computeTotalTaxStillDue calls capital(), which uppercases every
        // text input except txtEmail.
        let text = |value: &str| value.trim().to_uppercase();

        put(&p("txtYear"), self.taxable_year.to_string());
        for quarter in 1..=4u8 {
            put(
                &p(&format!("optQuarter{quarter}")),
                flag(self.quarter == quarter),
            );
        }
        put(&p("AmendedRtn_1"), flag(self.is_amended));
        put(&p("AmendedRtn_2"), flag(!self.is_amended));
        put(
            &p("TaxWithheld_1"),
            flag(self.any_tax_withheld == Some(true)),
        );
        put(
            &p("TaxWithheld_2"),
            flag(self.any_tax_withheld == Some(false)),
        );
        put(&p("txtSheets"), self.number_of_attached_sheets.to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put(&p("txtTIN1"), tin1.clone());
        put(&p("txtTIN2"), tin2.clone());
        put(&p("txtTIN3"), tin3.clone());
        put(&p("txtBranchCode"), branch.clone());
        put(&p("rdoCode"), self.rdo_code.trim().to_string());
        put(&p("txtTaxpayerName"), text(&self.taxpayer_name));
        // Address lines 1 and 2 are written back to back into one value.
        put(&p("txtAddress"), text(&self.registered_address));
        put(&p("txtAddress2"), String::new());
        put(&p("txtZipCode"), self.zip_code.trim().to_string());
        put(&p("txtTelNum"), self.contact_number.trim().to_string());
        put(
            &p("CatAgent_P"),
            flag(self.category == Some(Form1603QCategory::Private)),
        );
        put(
            &p("CatAgent_G"),
            flag(self.category == Some(Form1603QCategory::Government)),
        );
        put("txtEmail", self.email.trim().to_string());
        put(&p("SpecialTax_1"), flag(self.tax_relief.is_some()));
        put(&p("SpecialTax_2"), flag(self.tax_relief.is_none()));
        put(
            &p("selTreaty"),
            self.tax_relief
                .map_or("0", Form1603QTaxRelief::option_value)
                .to_string(),
        );

        put(&p("txtTax14"), official_amount(self.total_taxes_withheld));
        put(&p("txtTax15"), official_amount(self.tax_remitted_previous));
        put(&p("txt16Other"), text(&self.other_remittances_specify));
        put(&p("txtTax16"), official_amount(self.other_remittances));
        put(&p("txtTax17"), official_amount(self.total_remittances));
        put(&p("txtTax18"), official_amount(self.tax_still_due));
        put(&p("txtTax19"), official_amount(self.surcharge));
        put(&p("txtTax20"), official_amount(self.interest));
        put(&p("txtTax21"), official_amount(self.compromise));
        put(&p("txtTax22"), official_amount(self.total_penalties));
        put(&p("txtTax23"), official_amount(self.total_amount_due));

        put(&p("txtPg2TIN1"), tin1);
        put(&p("txtPg2TIN2"), tin2);
        put(&p("txtPg2TIN3"), tin3);
        put(&p("txtPg2BranchCode"), branch);
        put(&p("txtPg2TaxpayerName"), self.page2_name());
        for (index, line) in FORM_1603Q_SCHEDULE_LINES.iter().enumerate() {
            let n = index + 1;
            put(&p(&format!("Sched1:txtATC{n}")), line.atc_code.to_string());
            put(
                &p(&format!("Sched1:txtValue{n}")),
                official_amount(self.monetary_value[index]),
            );
            put(
                &p(&format!("Sched1:txtDivisor{n}")),
                line.divisor_text.to_string(),
            );
            put(
                &p(&format!("Sched1:txtTaxBase{n}")),
                official_amount(self.grossed_up_value[index]),
            );
            put(
                &p(&format!("Sched1:txtTaxRate{n}")),
                line.rate_text.to_string(),
            );
            put(
                &p(&format!("Sched1:txtTaxWithheld{n}")),
                official_amount(self.fringe_benefit_tax[index]),
            );
        }
        put(
            &p("Sched1:txtTotalTax"),
            official_amount(self.schedule_total),
        );
        put(&p("txtLineBus"), text(&self.line_of_business));
        // init() shows page 1; the page is validated and submitted from it.
        put(&p("txtCurrentPage"), "1".to_string());
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `validateYear` and `validateForm` against a given "today".
    pub fn validate_on(&self, today: chrono::NaiveDate) -> Vec<(String, String)> {
        use chrono::Datelike;
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let current_year = u16::try_from(today.year()).unwrap_or(u16::MAX);

        let year = self.taxable_year;
        if year == 0 {
            err("taxable_year", "Please enter a valid year on Item 1.");
        } else if year > current_year {
            // validateYear (on blur), then validateForm.
            err(
                "taxable_year",
                "Invalid year. Year should not be later than the current year.",
            );
            err(
                "taxable_year",
                "Invalid date entry on Item no.1. Entry should not be later than Current Date.",
            );
        } else if year < 2018 {
            err(
                "taxable_year",
                crate::validation::OFFICIAL_OLD_VERSION_MESSAGE,
            );
            err(
                "taxable_year",
                "Invalid date entry on Item no.1. Entry should not be lower than 2018.",
            );
        }
        if !(1..=4).contains(&self.quarter) {
            err("quarter", "Please select quarter on Item 2.");
        }
        if self.any_tax_withheld.is_none() {
            err("any_tax_withheld", "Please select an option for Item 4.");
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
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 50 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 8.",
            );
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 20 || !digits_only(phone) {
            // The official text has two spaces before "on".
            err(
                "contact_number",
                "Please enter a valid Telephone Number  on Item 10.",
            );
        }
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > 150 {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 9.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 9A.");
        }
        if self.category.is_none() {
            err("category", "Please select an option for Item 11.");
        }
        if self.any_tax_withheld == Some(true) && self.total_taxes_withheld == 0.0 {
            err("monetary_value", "Please fill up Schedule 1.");
        }

        // Limits the page enforces while typing, and what the submission needs.
        let email = self.email.trim();
        if email.is_empty()
            || email.len() > 60
            || !email.contains('@')
            || email.contains(char::is_whitespace)
        {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 5 holds at most two digits.",
            );
        }
        // Item 13A: the official validation has no 13A check; a blank choice
        // is written as selTreaty value 0, so it does not block Queue.
        if self.any_tax_withheld == Some(false) && self.monetary_value.iter().any(|v| *v != 0.0) {
            err(
                "monetary_value",
                "Schedule 1 applies only when Item 4 is \"Yes\".",
            );
        }
        if self.other_remittances_specify.trim().chars().count() > 25 {
            err(
                "other_remittances_specify",
                "Item 16 description holds at most 25 characters.",
            );
        }
        for (field, value) in [
            ("monetary_value[0]", self.monetary_value[0]),
            ("monetary_value[1]", self.monetary_value[1]),
            ("tax_remitted_previous", self.tax_remitted_previous),
            ("other_remittances", self.other_remittances),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ] {
            if value < 0.0 || value >= MAX_AMOUNT || !has_cent_precision(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }
        if !self.is_amended && self.tax_remitted_previous != 0.0 {
            err(
                "tax_remitted_previous",
                "Item 15 applies only to an amended return.",
            );
        }

        let mut expected = self.clone();
        expected.recompute();
        if expected.total_amount_due != self.total_amount_due
            || expected.total_taxes_withheld != self.total_taxes_withheld
            || expected.fringe_benefit_tax != self.fringe_benefit_tax
        {
            err(
                "total_amount_due",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }
}

impl FormValidator for Form1603QDraft {
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1603QDraft {
    /// Items 19-21 on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: Withholding,
            items: [
                surcharge: "19 Surcharge",
                interest: "20 Interest",
                compromise: "21 Compromise",
            ],
            controls: [
                "frm1603Q:txtTax19",
                "frm1603Q:txtTax20",
                "frm1603Q:txtTax21",
            ],
            state: penalty,
            basic_tax_due: "Item 18", |d| d.tax_still_due,
        )
    }
}

impl QueueableForm for Form1603QDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1603Q";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1603Qv2018']`).
    const FORM_TYPE: &'static str = "1603Qv2018";
    const LAYOUT_ID: &'static str = FORM_1603Q_FORM_ID;

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
        self.taxable_year
    }
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Quarterly(self.quarter)
    }
    /// `txtYear + "Q" + n`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{}Q{}", self.taxable_year, self.quarter)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 6 || code.get(4..5)? != "Q" {
            return None;
        }
        let year: u16 = code.get(..4)?.parse().ok()?;
        let quarter: u8 = code.get(5..)?.parse().ok()?;
        (1..=4)
            .contains(&quarter)
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
    use chrono::NaiveDate;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 10).unwrap()
    }

    pub(crate) fn sample() -> Form1603QDraft {
        let mut draft = Form1603QDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            tin: "12345678800000".to_string(),
            taxable_year: 2025,
            quarter: 3,
            is_amended: false,
            any_tax_withheld: None,
            number_of_attached_sheets: 0,
            rdo_code: "039".to_string(),
            line_of_business: "Consulting".to_string(),
            taxpayer_name: "Sample Taxpayer, Inc".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            contact_number: "0281234567".to_string(),
            category: Some(Form1603QCategory::Government),
            email: "sample.taxpayer@example.com".to_string(),
            tax_relief: Some(Form1603QTaxRelief::Both),
            monetary_value: [0.0; 2],
            grossed_up_value: [0.0; 2],
            fringe_benefit_tax: [0.0; 2],
            schedule_total: 0.0,
            total_taxes_withheld: 0.0,
            tax_remitted_previous: 0.0,
            other_remittances_specify: String::new(),
            other_remittances: 0.0,
            total_remittances: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_due: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.set_any_tax_withheld(true);
        draft.monetary_value = [100_000.005, 54_321.1];
        draft.other_remittances_specify = "Bank remittance".into();
        draft.other_remittances = 1_000.505;
        draft.surcharge = 25.5;
        draft.interest = 10.0;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1603QDraft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.monetary_value, [100_000.01, 54_321.1]);
        assert_eq!(draft.grossed_up_value, [153_846.17, 72_428.13]);
        assert_eq!(draft.fringe_benefit_tax, [53_846.16, 18_107.03]);
        assert_eq!(draft.total_taxes_withheld, 71_953.19);
        assert_eq!(draft.total_remittances, 1_000.51);
        assert_eq!(draft.tax_still_due, 70_952.68);
        assert_eq!(draft.total_penalties, 35.5);
        assert_eq!(draft.total_amount_due, 70_988.18);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1603Q:rdoCode"], "039");
        assert_eq!(fields["frm1603Q:Sched1:txtDivisor1"], "65%");
        assert_eq!(fields["frm1603Q:Sched1:txtTaxRate2"], "25%");
        assert_eq!(fields["frm1603Q:txt16Other"], "BANK REMITTANCE");
        assert_eq!(fields["frm1603Q:txtTaxpayerName"], "SAMPLE TAXPAYER, INC");
        assert_eq!(fields["frm1603Q:txtPg2TaxpayerName"], "SAMPLE TAXPAYER");
        assert_eq!(fields["frm1603Q:selTreaty"], "3");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1603Qv2018-2025Q3#sample.taxpayer@example.com#.xml"
        );
        let layout = crate::official_xml::layout(FORM_1603Q_FORM_ID).unwrap();
        let keys = layout.keys();
        assert!(!keys.contains("frm1603Q:txtRDOCode"));
        for key in fields.keys() {
            assert!(keys.contains(key.as_str()), "{key}");
        }
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn blank_item_13a_does_not_block_like_the_official_page() {
        let mut draft = sample();
        draft.tax_relief = Some(Form1603QTaxRelief::Unspecified);
        assert!(
            !draft
                .validate_on(today())
                .iter()
                .any(|(k, _)| k == "tax_relief")
        );
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "2025Q3");
        assert_eq!(
            Form1603QDraft::parse_period_code("2025Q3"),
            Some((2025, FilingPeriod::Quarterly(3)))
        );
        assert_eq!(Form1603QDraft::parse_period_code("2025Q9"), None);
        assert_eq!(Form1603QDraft::parse_period_code("2025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1603QDraft), expected: &str| {
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
            &|d| d.taxable_year = 2027,
            "Invalid date entry on Item no.1. Entry should not be later than Current Date.",
        );
        check(
            &|d| d.taxable_year = 2027,
            "Invalid year. Year should not be later than the current year.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Invalid date entry on Item no.1. Entry should not be lower than 2018.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Please file using the old version of the form.",
        );
        check(&|d| d.quarter = 0, "Please select quarter on Item 2.");
        check(
            &|d| d.any_tax_withheld = None,
            "Please select an option for Item 4.",
        );
        check(
            &|d| d.tin = "12".into(),
            "Please enter a valid TIN number on Item 6.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 7.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 8.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number  on Item 10.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 9.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 9A.",
        );
        check(
            &|d| d.category = None,
            "Please select an option for Item 11.",
        );
        check(
            &|d| {
                d.monetary_value = [0.0; 2];
                d.recompute();
            },
            "Please fill up Schedule 1.",
        );
    }

    #[test]
    fn withheld_no_clears_schedule_one() {
        let mut draft = sample();
        draft.set_any_tax_withheld(false);
        assert_eq!(draft.total_taxes_withheld, 0.0);
        assert_eq!(draft.surcharge, 0.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        assert_eq!(
            draft.to_bir_field_map()["frm1603Q:Sched1:txtTaxBase1"],
            "0.00"
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
            <Form1603QDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form1603QDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
