//! Form 0619-E (January 2018) on the generic submission path.
//!
//! Ported from the official `BIR-Form0619E.hta` (eBIRForms 7.9.6.2.1): the
//! compute chain (`computeNetAmtRem`, `computePenalties`,
//! `computeTotalAmtRem`, `computeDueDate`), `validateForm` /
//! `initialValidateBeforeSave` with their exact alert texts, the background
//! fill (`loadBGData`: name as stored, address upper-cased and split at 127
//! escaped characters) and `saveXMLsubmit` through [`crate::official_xml`].
//!
//! No `capital()` runs on this page in practice: it is bound only to the name
//! and address controls, which `init()` disables. Values keep their casing.

use super::penalty_items::PenaltyItems;
use std::collections::BTreeMap;

use super::form_0619e::{ATC_CODE, Form0619EDraft, TAX_TYPE_CODE, WithholdingAgentCategory};
use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{js_escape, js_unescape, official_amount, parse_official_amount};

/// Rule-package id of the official layout.
pub const FORM_0619E_LAYOUT_ID: &str = "0619e-v2018";
/// Official `formType` (`0619E`).
pub const FORM_0619E_FORM_TYPE: &str = "0619E";

/// `saveXML` alert (0619e-save-008) for a month that already has a filed return
/// while Item 3 is No. The port keeps one return per month, so the page
/// shows it on a filed month instead of opening an amendment.
pub const FORM_0619E_AMENDED_ALERT: &str = "If your intention is to make an amended return, kindly tick Amended Return to 'Yes' before hitting 'Save' or 'Final Copy' or 'Submit'.";

/// `loadBGData` splits the escaped, upper-cased address at this length.
const ADDRESS_SPLIT: usize = 127;

/// Why an address whose official cut lands inside an escape is refused.
pub(crate) const ADDRESS_SPLIT_REFUSAL: &str = "BIR's program splits this address at character 127, inside a special character. Reword or shorten it near that point.";

fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// `txtAddress` / `txtAddress2` as `loadBGData` fills them, or `None` when
/// the address cannot be represented the way the official page splits it:
/// a character outside Latin-1, or the official cut at character 127 of
/// the escaped address landing inside a `%XX` escape (the official runtime
/// cuts there literally; the port refuses rather than file different
/// lines).
pub(super) fn official_address_lines(address: &str) -> Option<(String, String)> {
    if address.chars().any(|c| u32::from(c) > 0xFF) {
        return None;
    }
    let escaped = js_escape(address).to_ascii_uppercase();
    if escaped.len() <= ADDRESS_SPLIT {
        return Some((js_unescape(&escaped)?, String::new()));
    }
    // A cut inside a %XX sequence would leave literal fragments.
    let cut = &escaped[..ADDRESS_SPLIT];
    if cut.ends_with('%') || cut.as_bytes()[ADDRESS_SPLIT - 2] == b'%' {
        return None;
    }
    Some((js_unescape(cut)?, js_unescape(&escaped[ADDRESS_SPLIT..])?))
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    (
        part(0..3),
        part(3..6),
        part(6..9),
        digits.get(9..).unwrap_or("").to_string(),
    )
}

impl Form0619EDraft {
    /// The full registered address the background fill reads.
    fn full_address(&self) -> String {
        format!("{}{}", self.registered_address, self.registered_address_2)
    }

    /// The official compute chain at cents, as `round(this,2)` and
    /// `formatCurrency` leave each field.
    pub(super) fn official_recompute(&mut self) {
        if !self.any_taxes_withheld {
            self.item_14_amount_of_remittance = 0.0;
        }
        if !self.is_amended {
            self.item_15_amount_remitted_previously = 0.0;
        }
        self.item_14_amount_of_remittance = cents(self.item_14_amount_of_remittance);
        self.item_15_amount_remitted_previously = cents(self.item_15_amount_remitted_previously);
        self.item_16_net_amount_of_remittance =
            cents(self.item_14_amount_of_remittance - self.item_15_amount_remitted_previously);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.item_17a_surcharge = cents(self.item_17a_surcharge);
        self.item_17b_interest = cents(self.item_17b_interest);
        self.item_17c_compromise = cents(self.item_17c_compromise);
        self.item_17d_total_penalties =
            cents(self.item_17a_surcharge + self.item_17b_interest + self.item_17c_compromise);
        self.item_18_total_amount_of_remittance =
            cents(self.item_16_net_amount_of_remittance + self.item_17d_total_penalties);
    }

    /// `validateForm` and `initialValidateBeforeSave` in order with their
    /// alert texts, plus the limits the page enforces while typing.
    pub(super) fn official_errors(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let today = chrono::Local::now().date_naive();
        let this_year = chrono::Datelike::year(&today) as u16;
        let this_month = chrono::Datelike::month(&today) as u8;

        // Item 1
        if !(1..=12).contains(&self.month) {
            err("month", "Please enter a valid month on Item 1.");
        } else if self.taxable_year == this_year && self.month == this_month {
            err(
                "month",
                "Invalid month on Item 1. Month should not be a current Date",
            );
        } else if self.taxable_year == this_year && self.month > this_month {
            err(
                "month",
                "Invalid month on Item 1. Month should not be a future Date",
            );
        }
        if self.taxable_year == 0 {
            err("taxable_year", "Please enter a valid year on Item 1.");
        } else if self.taxable_year > this_year {
            err(
                "taxable_year",
                "Invalid year on Item 1. Year should not be a future Date.",
            );
        } else if self.taxable_year < 2018 {
            err(
                "taxable_year",
                "Invalid entry on Item 1. Entry should not be a previous year from 2018.",
            );
        }

        // Item 2 (due month/year follow Item 1; the day is typed)
        let (due_month, due_year) = self.due_month_and_year();
        match self.due_day {
            None => err("due_day", "Please enter a valid Date on Item 2"),
            Some(day) => {
                if !(1..=31).contains(&day) {
                    err("due_day", "Please enter a valid Day on Item 2");
                } else if due_year > this_year {
                    err(
                        "due_day",
                        "Year should not be a future Date. Please enter a valid Year on Item 2",
                    );
                } else if due_year < 2018 {
                    err(
                        "due_day",
                        "Previous year from 2018 is not applicable for this Form. Please enter a valid Year on Item 2",
                    );
                } else if chrono::NaiveDate::from_ymd_opt(
                    i32::from(due_year),
                    u32::from(due_month),
                    u32::from(day),
                )
                .is_none()
                {
                    err("due_day", "Please enter a valid Day on Item 2");
                }
            }
        }

        // Items 4 and 12 are always answered in this model.

        // Item 7
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        if tin1.len() != 3
            || tin2.len() != 3
            || tin3.len() != 3
            || branch.is_empty()
            || branch.len() > 5
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 7.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        let rdo = self.rdo_code.trim();
        if rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err("rdo_code", "Please enter a valid RDO Code on Item 8.");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 9.",
            );
        } else if name.chars().count() > 50 {
            err("taxpayer_name", "Item 9 holds at most 50 characters.");
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() {
            err(
                "contact_number",
                "Please enter a valid Contact Number on Item 11.",
            );
        } else if phone.len() > 20 || !digits_only(phone) {
            err(
                "contact_number",
                "Item 11 holds at most 20 digits (numbers only).",
            );
        }
        if self.full_address().trim().is_empty() {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 10.",
            );
        } else if self.full_address().chars().any(|c| u32::from(c) > 0xFF) {
            err(
                "registered_address",
                "Item 10 has a character BIR's program cannot carry (outside Latin-1); remove it.",
            );
        } else if official_address_lines(&self.full_address()).is_none() {
            err("registered_address", ADDRESS_SPLIT_REFUSAL);
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 10A.");
        } else if zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Item 10A holds at most 12 digits.");
        }
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
            err("email", "Please enter valid Email Address on Item 13.");
        }
        if self.line_of_business.chars().count() > 150 {
            err(
                "line_of_business",
                "Item 8 Line of Business holds at most 150 characters.",
            );
        }
        if self.any_taxes_withheld && self.item_14_amount_of_remittance == 0.0 {
            err(
                "item_14_amount_of_remittance",
                "Please fill up Part II - Tax Remittance if item 4 is set to Yes.",
            );
        }

        for (field, value) in [
            (
                "item_14_amount_of_remittance",
                self.item_14_amount_of_remittance,
            ),
            (
                "item_15_amount_remitted_previously",
                self.item_15_amount_remitted_previously,
            ),
            ("item_17a_surcharge", self.item_17a_surcharge),
            ("item_17b_interest", self.item_17b_interest),
            ("item_17c_compromise", self.item_17c_compromise),
        ] {
            if value < 0.0 || !has_cent_precision(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }

        let mut expected = self.clone();
        expected.official_recompute();
        let penalties = Self::penalty_declaration();
        if expected.item_18_total_amount_of_remittance != self.item_18_total_amount_of_remittance
            || penalties.filed(&expected) != penalties.filed(self)
            || expected.item_16_net_amount_of_remittance != self.item_16_net_amount_of_remittance
            || expected.item_17d_total_penalties != self.item_17d_total_penalties
            || expected.item_14_amount_of_remittance != self.item_14_amount_of_remittance
            || expected.item_15_amount_remitted_previously
                != self.item_15_amount_remitted_previously
        {
            err(
                "item_18_total_amount_of_remittance",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }

    /// The official control values `saveXMLsubmit` reads, keyed by id.
    pub fn to_official_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(key.to_string(), value);
        };
        let flag = |on: bool| on.to_string();
        let (due_month, due_year) = self.due_month_and_year();
        put("frm0619E:txtMonth", format!("{:02}", self.month));
        put("frm0619E:txtYear", self.taxable_year.to_string());
        put("frm0619E:txtDueMonth", format!("{due_month:02}"));
        put(
            "frm0619E:txtDueDay",
            self.due_day.map(|d| format!("{d:02}")).unwrap_or_default(),
        );
        put("frm0619E:txtDueYear", due_year.to_string());
        put("frm0619E:optAmend:Y", flag(self.is_amended));
        put("frm0619E:optAmend:N", flag(!self.is_amended));
        put("frm0619E:optWithheld:Y", flag(self.any_taxes_withheld));
        put("frm0619E:optWithheld:N", flag(!self.any_taxes_withheld));
        put("frm0619E:txtAtc", ATC_CODE.to_string());
        put("frm0619E:txtTaxTypeCode", TAX_TYPE_CODE.to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("frm0619E:txtTIN1", tin1);
        put("frm0619E:txtTIN2", tin2);
        put("frm0619E:txtTIN3", tin3);
        put("frm0619E:txtBranchCode", branch);
        put("frm0619E:txtRDOCode", self.rdo_code.trim().to_string());
        put(
            "frm0619E:txtTaxpayerName",
            self.taxpayer_name.trim().to_string(),
        );
        put(
            "frm0619E:txtLineBus",
            self.line_of_business.trim().to_string(),
        );
        let (line1, line2) = official_address_lines(&self.full_address()).unwrap_or_default();
        put("frm0619E:txtAddress", line1);
        put("frm0619E:txtAddress2", line2);
        put("frm0619E:txtZipCode", self.zip_code.trim().to_string());
        put("frm0619E:txtTelNum", self.contact_number.trim().to_string());
        let government = self.withholding_agent_category == WithholdingAgentCategory::Government;
        put("frm0619E:optCategory:P", flag(!government));
        put("frm0619E:optCategory:G", flag(government));
        put("txtEmail", self.email.trim().to_string());
        for (key, value) in [
            ("frm0619E:txtTax14", self.item_14_amount_of_remittance),
            ("frm0619E:txtTax15", self.item_15_amount_remitted_previously),
            ("frm0619E:txtTax16", self.item_16_net_amount_of_remittance),
            ("frm0619E:txtTax17A", self.item_17a_surcharge),
            ("frm0619E:txtTax17B", self.item_17b_interest),
            ("frm0619E:txtTax17C", self.item_17c_compromise),
            ("frm0619E:txtTax17D", self.item_17d_total_penalties),
            ("frm0619E:txtTax18", self.item_18_total_amount_of_remittance),
        ] {
            put(key, official_amount(value));
        }
        // The tax-agent and Part III payment controls are disabled on the
        // official page, so they keep their blank defaults. `init()` never
        // calls getDrives() (commented out), so the export-drive select is
        // empty rather than the usual "0" placeholder.
        fields.insert("driveSelectTPExport".to_string(), String::new());
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_official_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

impl Form0619EDraft {
    /// Items 17A-17C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: Withholding,
            items: [
                item_17a_surcharge: "17A Surcharge",
                item_17b_interest: "17B Interest",
                item_17c_compromise: "17C Compromise",
            ],
            controls: [
                "frm0619E:txtTax17A",
                "frm0619E:txtTax17B",
                "frm0619E:txtTax17C",
            ],
            state: penalty,
            basic_tax_due: "Item 16", |d| d.item_16_net_amount_of_remittance,
        )
    }
}

impl QueueableForm for Form0619EDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_bir_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "0619E";
    const FORM_TYPE: &'static str = FORM_0619E_FORM_TYPE;
    const LAYOUT_ID: &'static str = FORM_0619E_LAYOUT_ID;

    /// Item 18 Total Amount of Remittance.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.item_18_total_amount_of_remittance)
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
        FilingPeriod::Monthly(self.month)
    }
    /// `txtMonth + txtYear`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}{}", self.month, self.taxable_year)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 6 || !digits_only(code) {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let year: u16 = code.get(2..)?.parse().ok()?;
        (1..=12)
            .contains(&month)
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
        self.to_official_field_map()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filing_queue::QueueAuthSource;
    use crate::forms::FilingStatus;

    fn profile() -> crate::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Sample Dummy Taxpayer",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Sample Consulting Services",
            "registered_address": "123 Sample Street, Quezon City",
            "zip_code": "1100",
            "phone": "09170000000",
            "email": "sample.taxpayer@example.com",
            "default_form_type": "0619E",
            "taxpayer_type": "Corporation"
        }))
        .expect("dummy profile")
    }

    fn sample() -> Form0619EDraft {
        let mut draft = Form0619EDraft::new_from_profile(&profile(), 2025, 6);
        draft.any_taxes_withheld = true;
        draft.item_14_amount_of_remittance = 12_345.675;
        draft.item_17a_surcharge = 25.5;
        draft.item_17b_interest = 10.005;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form0619EDraft) -> Vec<String> {
        <Form0619EDraft as QueueableForm>::validate(draft)
            .into_iter()
            .map(|(_, m)| m)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert_eq!(draft.item_14_amount_of_remittance, 12_345.68);
        assert_eq!(draft.item_16_net_amount_of_remittance, 12_345.68);
        assert_eq!(draft.item_17d_total_penalties, 35.51);
        assert_eq!(draft.item_18_total_amount_of_remittance, 12_381.19);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        let mut no = draft.clone();
        no.any_taxes_withheld = false;
        no.recompute();
        assert_eq!(no.item_14_amount_of_remittance, 0.0);
    }

    #[test]
    fn field_map_and_filename_follow_the_official_page() {
        let draft = sample();
        let fields = draft.to_official_field_map();
        assert_eq!(fields["frm0619E:txtDueMonth"], "07");
        assert_eq!(fields["frm0619E:txtDueDay"], "10");
        assert_eq!(
            fields["frm0619E:txtAddress"],
            "123 SAMPLE STREET, QUEZON CITY"
        );
        assert_eq!(fields["frm0619E:txtTaxpayerName"], "Sample Dummy Taxpayer");
        assert_eq!(fields["frm0619E:txtTax14"], "12,345.68");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-0619E-062025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_official_xml_payload().is_ok());
    }

    #[test]
    fn long_addresses_split_like_load_bg_data() {
        let address = "A".repeat(130);
        let (one, two) = official_address_lines(&address).unwrap();
        assert_eq!(one.len(), 127);
        assert_eq!(two, "AAA");
        // 126 plain characters then a space: the cut would split "%20", as
        // would 125 (the cut after "%2"). The official runtime cuts there
        // literally; the port refuses.
        let tricky = format!("{} x", "a".repeat(126));
        assert_eq!(official_address_lines(&tricky), None);
        let tricky = format!("{} x", "a".repeat(125));
        assert_eq!(official_address_lines(&tricky), None);
        // 124: the escape ends before the cut, and the lines split there.
        let fine = format!("{} x", "a".repeat(124));
        let (one, two) = official_address_lines(&fine).unwrap();
        assert_eq!(format!("{one}{two}"), fine.to_ascii_uppercase());
        // A character outside Latin-1 is refused too.
        assert!(official_address_lines("UNIT \u{4E00} CITY").is_none());
    }

    #[test]
    fn period_codes_round_trip() {
        assert_eq!(sample().period_code(), "062025");
        assert_eq!(
            Form0619EDraft::parse_period_code("122025"),
            Some((2025, FilingPeriod::Monthly(12)))
        );
        assert_eq!(Form0619EDraft::parse_period_code("132025"), None);
        assert_eq!(Form0619EDraft::parse_period_code("1225"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form0619EDraft), expected: &str| {
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
            &|d| d.taxable_year = 2099,
            "Invalid year on Item 1. Year should not be a future Date.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Invalid entry on Item 1. Entry should not be a previous year from 2018.",
        );
        check(&|d| d.due_day = None, "Please enter a valid Date on Item 2");
        check(
            &|d| d.due_day = Some(32),
            "Please enter a valid Day on Item 2",
        );
        check(
            &|d| d.tin = "12345".into(),
            "Please enter a valid TIN number on Item 7.",
        );
        check(
            &|d| d.tin = "123-456-789-00000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 8.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 9.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Contact Number on Item 11.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 10.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 10A.",
        );
        check(
            &|d| d.email.clear(),
            "Please enter valid Email Address on Item 13.",
        );
        check(
            &|d| {
                d.item_14_amount_of_remittance = 0.0;
                d.recompute();
            },
            "Please fill up Part II - Tax Remittance if item 4 is set to Yes.",
        );
        check(
            &|d| d.item_17a_surcharge = 1.0,
            "Totals are out of date. Recompute the return.",
        );
    }

    #[test]
    fn old_stored_json_still_loads() {
        let mut json = serde_json::to_value(sample()).unwrap();
        let object = json.as_object_mut().unwrap();
        object.insert("submitted_at".into(), serde_json::Value::Null);
        object.insert("receipt_id".into(), serde_json::Value::Null);
        object.insert("next_retry_at".into(), serde_json::Value::Null);
        object.insert("submission_attempts".into(), serde_json::json!(0));
        object.insert("last_error".into(), serde_json::json!("old"));
        object.insert(
            "created_at".into(),
            serde_json::json!("2025-07-01T00:00:00+00:00"),
        );
        let draft: Form0619EDraft = serde_json::from_value(json).unwrap();
        assert_eq!(draft.lifecycle.status, FilingStatus::Draft);
        assert_eq!(draft.lifecycle.created_at, "2025-07-01T00:00:00+00:00");
        assert_eq!(draft.last_error.as_deref(), Some("old"));
        let back = serde_json::to_value(&draft).unwrap();
        assert_eq!(back["status"], "Draft");
        assert!(back.get("lifecycle").is_none());
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft.queue(QueueAuthSource::Gui).unwrap();
        assert!(draft.clone().revalidate_queued_before_submission().is_ok());
        draft.item_17c_compromise = 99.0;
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
        draft.item_17a_surcharge = 8_816_764.47;
        assert!(
            <Form0619EDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "item_17a_surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.item_17a_surcharge = 1.005;
        assert!(
            <Form0619EDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "item_17a_surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
