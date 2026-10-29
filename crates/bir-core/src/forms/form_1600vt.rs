//! BIR Form 1600-VT — Monthly Remittance Return of Value-Added Tax Withheld
//! (January 2018 ENCS).
//!
//! Ported from the official `BIR-Form1600VTv2018.hta` (eBIRForms 7.9.6.2.1):
//! the ATC popup (`changedrpATCList`, `getATCCode`), the compute chain
//! (`getRequiredWithheld`, `pageOneComputation`, `pageTwoComputation`,
//! `totTaxWithheldAndRemitted`), `validateAll` / `checkDate` /
//! `initialValidateBeforeSave` with their exact alert texts, and
//! the uploaded file (`saveEncryptedProfile(true)`) through
//! [`crate::official_xml`].
//!
//! Value rules: the upload loop strips commas and turns `(x)` into `-x` for
//! controls with `maxLength` 12 or 15. On this page that is only
//! `Pg2Sc1TIN5` (a digits-only TIN), so no field-map value is affected; the
//! amounts keep their `1,234.56` commas.
//!
//! Item 10 makes the page draw the ATC popup (`AtcCode1..n` checkboxes)
//! inside the form, and the upload loop serializes those checkboxes too: 10
//! for a private agent, 12 for a government agent. The submit layout is
//! therefore per category (`1600vt-v2018-private` / `-government`).
//!
//! Part II shows five ATCs (Items 13–17). The popup has no limit: when more
//! than five are ticked, `getATCCode` puts the sixth onward in the "Other
//! Selected ATC" table (`tblPartIIOtherTax`, rows `txtAtcCd6..n` /
//! `txtPg1P2TaxBase6..n` / `txtPg1P2TaxRate6..n` / `txtPg1P2TaxWithheld6..n`),
//! `computeTotalOtherWithheldTax` sums them into `txtTotalOtherTax`, and Item
//! 18 adds that total. Those rows sit after `hPartIITableSize` in the form, so
//! the upload writes them there ([`Form1600VtDraft::official_layout`]).

use super::kept_text::KeptText;
use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use chrono::Datelike;
use serde::{Deserialize, Serialize};

use super::official_inputs::{RowInsertion, extend_layout, has_cent_precision};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout (before Item 10 is answered).
pub const FORM_1600VT_FORM_ID: &str = "1600vt-v2018";
/// Submit layout once Item 10 is Private.
pub const FORM_1600VT_PRIVATE_LAYOUT_ID: &str = "1600vt-v2018-private";
/// Submit layout once Item 10 is Government.
pub const FORM_1600VT_GOVERNMENT_LAYOUT_ID: &str = "1600vt-v2018-government";
/// Part II rows, Items 13–17. ATCs past these go to the "Other Selected
/// ATC" table; the popup itself has no limit beyond the ATCs it offers.
pub const FORM_1600VT_ATC_ROWS: usize = 5;
/// Page 2 Schedule 1 rows.
pub const FORM_1600VT_SCHEDULE_ROWS: usize = 5;
const PREFIX: &str = "frm1600VT";

/// Item 10 (`rdoPg1I10CategoryofWthholdAgentPriv` / `…Govt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1600VtAgentCategory {
    Private,
    Government,
}

/// One ATC the official popup offers (`atcCodes.xml` lines tagged
/// `1600VTv2018`, as `loadATC` reads them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Form1600VtAtcOption {
    pub code: &'static str,
    /// Rate text the popup copies into the rate column, e.g. `"12.0"`.
    pub rate_text: &'static str,
    pub rate: f64,
    /// `"P"`, `"G"` or `"PG"`.
    pub category: &'static str,
    pub description: &'static str,
}

impl Form1600VtAtcOption {
    /// Whether the popup lists it for a withholding-agent category.
    pub fn offered_to(&self, category: Form1600VtAgentCategory) -> bool {
        match category {
            Form1600VtAgentCategory::Private => self.category.contains('P'),
            Form1600VtAgentCategory::Government => self.category.contains('G'),
        }
    }
}

macro_rules! atc {
    ($code:literal, $rate:literal, $category:literal, $description:literal) => {
        Form1600VtAtcOption {
            code: $code,
            rate_text: $rate,
            rate: parse_rate($rate),
            category: $category,
            description: $description,
        }
    };
}

const fn parse_rate(text: &str) -> f64 {
    // "12.0" / "5.0": whole part and one decimal digit.
    let bytes = text.as_bytes();
    let mut whole = 0u32;
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b'.' {
        whole = whole * 10 + (bytes[i] - b'0') as u32;
        i += 1;
    }
    let mut tenths = 0u32;
    if i + 1 < bytes.len() {
        tenths = (bytes[i + 1] - b'0') as u32;
    }
    whole as f64 + tenths as f64 / 10.0
}

/// The official list in `atcList` order (the order `getATCCode` fills rows).
/// `atcCodes.xml` tags WV040 and WV110 with the same key, so `loadATC` never
/// reads WV040; the popup does not offer it.
pub const FORM_1600VT_ATC_OPTIONS: &[Form1600VtAtcOption] = &[
    atc!("WV010", "5.0", "G", "VAT WITHHOLDING ON PURCHASE OF GOODS"),
    atc!(
        "WV020",
        "5.0",
        "G",
        "VAT WITHHOLDING ON PURCHASE OF SERVICES"
    ),
    atc!(
        "WV110",
        "12.0",
        "PG",
        "VAT ON LOCAL SALES OF REGISTERED BUSINESS ENTERPRISES (RBEs)"
    ),
    atc!(
        "WV050",
        "12.0",
        "PG",
        "FINAL WITHHOLDING VAT ON PAYMENTS FOR LEASE OR USE OF PROPERTIES OR PROPERTY RIGHTS OWNED BY NONRESIDENTS (PRIVATE WITHHOLDING AGENT)"
    ),
    atc!(
        "WV060",
        "12.0",
        "PG",
        "FINAL WITHHOLDING VAT ON OTHER SERVICES RENDERED IN THE PHILIPPINES BY NON-RESIDENTS (GOVERNMENT WITHHOLDING AGENT)"
    ),
    atc!(
        "WV070",
        "12.0",
        "PG",
        "FINAL WITHHOLDING VAT ON OTHER SERVICES RENDERED IN THE PHILIPPINES BY NON-RESIDENTS (PRIVATE WITHHOLDING AGENT)"
    ),
    atc!(
        "WV012",
        "12.0",
        "PG",
        "VAT WITHHOLDING ON PURCHASES OF GOODS (WITH WAIVER OF PRIVILEGE TO CLAIM INPUT TAX CREDIT) (CREDITABLE)"
    ),
    atc!(
        "WV014",
        "12.0",
        "PG",
        "VAT WITHHOLDING ON PURCHASES OF GOODS (WITH WAIVER OF PRIVILEGE TO CLAIM INPUT TAX CREDIT) (FINAL)"
    ),
    atc!(
        "WV022",
        "12.0",
        "PG",
        "VAT WITHHOLDING ON PURCHASES OF SERVICES (WITH WAIVER OF PRIVILEGE TO CLAIM INPUT TAX CREDIT)(CREDITABLE)"
    ),
    atc!(
        "WV024",
        "12.0",
        "PG",
        "VAT WITHHOLDING ON PURCHASES OF SERVICES (WITH WAIVER OF PRIVILEGE TO CLAIM INPUT TAX CREDIT) (FINAL)"
    ),
    atc!(
        "WV080",
        "12.0",
        "P",
        "FINAL WITHHOLDING VAT ON PURCHASE OF DIGITAL SERVICE CONSUMED IN THE PHILS FROM NON-RESIDENT DSPs (PRIVATE WA)"
    ),
    atc!(
        "WV090",
        "12.0",
        "G",
        "FINAL WITHHOLDING VAT ON PURCHASE OF DIGITAL SERVICE CONSUMED IN THE PHILS FROM NON-RESIDENT DSPs (GOVT WA)"
    ),
    atc!(
        "WV100",
        "12.0",
        "PG",
        "FINAL WITHHOLDING VAT ON THE GROSS AMOUNT BY RESIDENT E-MARKETPLACE TO THE NON-RESIDENT SELLERS/MERCHANTS FOR THE DIGITAL SERVICES SOLD/PAID THRU THEIR PLATFORM/FACILITY"
    ),
];

/// The popup rows for a category, in `atcList` order (`AtcCode1..n`).
pub fn form_1600vt_popup(
    category: Form1600VtAgentCategory,
) -> impl Iterator<Item = (usize, &'static Form1600VtAtcOption)> {
    FORM_1600VT_ATC_OPTIONS
        .iter()
        .enumerate()
        .filter(move |(_, option)| option.offered_to(category))
}

/// Index of an ATC (code + rate) in [`FORM_1600VT_ATC_OPTIONS`].
pub fn form_1600vt_atc_index(code: &str, rate_text: &str) -> Option<usize> {
    FORM_1600VT_ATC_OPTIONS
        .iter()
        .position(|option| option.code == code && option.rate_text == rate_text)
}

/// One Part II row (Items 13–17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1600VtAtcRow {
    pub atc_code: String,
    /// Rate text from the popup, e.g. `"12.0"`.
    pub rate_text: String,
    /// The tax base as typed. `getRequiredWithheld` runs before
    /// `round(this,2)`, so the withheld amount uses the unrounded entry.
    pub tax_base: f64,
    pub tax_withheld: f64,
}

impl Form1600VtAtcRow {
    pub fn option(&self) -> Option<&'static Form1600VtAtcOption> {
        form_1600vt_atc_index(&self.atc_code, &self.rate_text).map(|i| &FORM_1600VT_ATC_OPTIONS[i])
    }
}

/// One Page 2 Schedule 1 row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1600VtScheduleRow {
    /// Payee TIN, digits only (12 to 14 digits).
    pub tin: String,
    pub payee_name: String,
    pub atc: String,
    pub income_payment: f64,
    /// Percent.
    pub tax_rate: f64,
    pub tax_withheld: f64,
}

impl Form1600VtScheduleRow {
    pub fn is_blank(&self) -> bool {
        self.tin.trim().is_empty()
            && self.payee_name.trim().is_empty()
            && self.atc.trim().is_empty()
            && self.income_payment == 0.0
            && self.tax_rate == 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1600VtDraft {
    #[serde(default)]
    pub id: Option<i64>,

    // Items 1–4
    /// Item 1 month (1–12).
    pub month: u8,
    /// Item 1 year (four digits; the page shows the last two).
    pub year: u16,
    pub is_amended: bool,
    /// Item 3 "Any taxes withheld?".
    #[serde(default)]
    pub taxes_withheld: bool,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I (profile)
    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    // Part I's text boxes may be missing from a draft an older shape
    // stored (maintainer decision P2-4): it opens with the box blank, Check
    // asks for it, and a save writes back no key it did not read
    // ([`KeptText`]).
    #[serde(default, skip_serializing_if = "KeptText::is_absent")]
    pub rdo_code: KeptText,
    #[serde(default, skip_serializing_if = "KeptText::is_absent")]
    pub agent_name: KeptText,
    /// Item 8; the page splits it into two 80-character lines.
    #[serde(default, skip_serializing_if = "KeptText::is_absent")]
    pub registered_address: KeptText,
    #[serde(default, skip_serializing_if = "KeptText::is_absent")]
    pub zip_code: KeptText,
    #[serde(default, skip_serializing_if = "KeptText::is_absent")]
    pub contact_number: KeptText,
    #[serde(default)]
    pub agent_category: Option<Form1600VtAgentCategory>,
    #[serde(default, skip_serializing_if = "KeptText::is_absent")]
    pub email: KeptText,
    #[serde(default)]
    pub tax_relief: bool,
    #[serde(default)]
    pub tax_relief_specify: String,
    /// Kept for the hidden `txtLOB` control (profile line of business).
    #[serde(default)]
    pub line_of_business: String,

    // Part II
    /// Items 13–17 in popup order, then the "Other Selected ATC" rows.
    #[serde(default)]
    pub atc_rows: Vec<Form1600VtAtcRow>,
    /// `txtTotalOtherTax`: tax withheld on the sixth ATC onward.
    #[serde(default)]
    pub total_other_tax: f64,
    /// Item 18.
    #[serde(default)]
    pub total_tax_withheld: f64,
    /// Item 19, amended returns only.
    #[serde(default)]
    pub tax_remitted_previous: f64,
    /// Item 20.
    #[serde(default)]
    pub other_payments: f64,
    /// Item 21.
    #[serde(default)]
    pub total_payments: f64,
    /// Item 22.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 23–26.
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
    /// Item 27.
    #[serde(default)]
    pub total_amount_due: f64,

    // Page 2
    #[serde(default)]
    pub schedule: Vec<Form1600VtScheduleRow>,
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
/// half up on its magnitude (so `1.005.toFixed(2)` is `"1.00"`).
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

/// `capitalize()`: trimmed and uppercased.
fn capitalized(value: &str) -> String {
    value.trim().to_uppercase()
}

/// `loadBGData` splits the registered address at 80 characters.
fn split_address(address: &str) -> (String, String) {
    let upper: Vec<char> = address.trim().to_uppercase().chars().collect();
    let first: String = upper.iter().take(80).collect();
    let second: String = upper.iter().skip(80).take(80).collect();
    (first, second)
}

impl Form1600VtDraft {
    pub const FORM_CODE: &'static str = "1600VT";

    /// Part I's text boxes as the desktop editor types them (name, address,
    /// ZIP, contact number, email), written in place so each keeps the
    /// presence it was read with ([`KeptText`]): a box the stored draft
    /// lacked stays absent while it is blank, and files once it holds text
    /// (P2-4: a save writes back no key it did not read).
    pub fn set_typed_part_i(
        &mut self,
        name: String,
        address: String,
        zip: String,
        contact: String,
        email: String,
    ) {
        *self.agent_name = name;
        *self.registered_address = address;
        *self.zip_code = zip;
        *self.contact_number = contact;
        *self.email = email;
    }

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            month: month.clamp(1, 12),
            year,
            is_amended: false,
            taxes_withheld: false,
            number_of_attached_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone().into(),
            agent_name: profile.full_name.clone().into(),
            registered_address: profile.registered_address.clone().into(),
            zip_code: profile.zip_code.clone().into(),
            contact_number: profile.phone.clone().into(),
            agent_category: None,
            email: profile.email.clone().into(),
            tax_relief: false,
            tax_relief_specify: String::new(),
            line_of_business: profile.line_of_business.clone(),
            atc_rows: Vec::new(),
            total_other_tax: 0.0,
            total_tax_withheld: 0.0,
            tax_remitted_previous: 0.0,
            other_payments: 0.0,
            total_payments: 0.0,
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

    /// Item 10. Changing the category clears the selected ATCs
    /// (`changeCategory`).
    pub fn set_agent_category(&mut self, category: Form1600VtAgentCategory) {
        if self.agent_category != Some(category) {
            self.atc_rows.clear();
        }
        self.agent_category = Some(category);
        self.recompute();
    }

    /// Item 3. "No" clears the selected ATCs (`changeTaxWithheldNO`).
    pub fn set_taxes_withheld(&mut self, withheld: bool) {
        self.taxes_withheld = withheld;
        if !withheld {
            self.atc_rows.clear();
        }
        self.recompute();
    }

    /// Tick or untick an ATC in the popup. Rows stay in popup order, the way
    /// `getATCCode` redraws them; a ticked ATC keeps its tax base.
    pub fn toggle_atc(&mut self, index: usize) -> Result<(), String> {
        let option = FORM_1600VT_ATC_OPTIONS
            .get(index)
            .ok_or_else(|| "Unknown ATC.".to_string())?;
        if !self.taxes_withheld {
            return Err(
                "Selecting an ATC is not necessary when item no. 3 is set to ' NO '".to_string(),
            );
        }
        let Some(category) = self.agent_category else {
            return Err("Please select an option for Item 10.".to_string());
        };
        if !option.offered_to(category) {
            return Err(format!(
                "{} is not offered to this category of withholding agent.",
                option.code
            ));
        }
        if let Some(at) = self
            .atc_rows
            .iter()
            .position(|row| row.atc_code == option.code && row.rate_text == option.rate_text)
        {
            self.atc_rows.remove(at);
        } else {
            self.atc_rows.push(Form1600VtAtcRow {
                atc_code: option.code.to_string(),
                rate_text: option.rate_text.to_string(),
                tax_base: 0.0,
                tax_withheld: 0.0,
            });
            self.atc_rows
                .sort_by_key(|row| form_1600vt_atc_index(&row.atc_code, &row.rate_text));
        }
        self.recompute();
        Ok(())
    }

    /// The official compute chain (`getRequiredWithheld`,
    /// `pageOneComputation`, `pageTwoComputation`).
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_remitted_previous = 0.0;
        }
        if !self.tax_relief {
            self.tax_relief_specify.clear();
        }
        for row in &mut self.atc_rows {
            let rate = row.option().map(|o| o.rate).unwrap_or(0.0);
            row.tax_withheld = cents(row.tax_base * rate / 100.0);
        }
        // computeTotalOtherWithheldTax: rows 6 onward.
        let mut other = 0.0;
        for row in self.atc_rows.iter().skip(FORM_1600VT_ATC_ROWS) {
            other += row.tax_withheld;
        }
        self.total_other_tax = cents(other);
        // pageOneComputation adds Items 13–16, the other total, then Item 17.
        let withheld = |index: usize| self.atc_rows.get(index).map_or(0.0, |row| row.tax_withheld);
        let total = withheld(0)
            + withheld(1)
            + withheld(2)
            + withheld(3)
            + self.total_other_tax
            + withheld(4);
        self.total_tax_withheld = cents(total);
        self.tax_remitted_previous = cents(self.tax_remitted_previous);
        self.other_payments = cents(self.other_payments);
        self.total_payments = cents(self.tax_remitted_previous + self.other_payments);
        self.tax_still_due = cents(self.total_tax_withheld - self.total_payments);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        // Item 27: an overremittance with penalties shows the penalties only.
        self.total_amount_due = if self.tax_still_due < 0.0 && self.total_penalties > 0.0 {
            self.total_penalties
        } else {
            cents(self.tax_still_due + self.total_penalties)
        };

        let mut schedule_total = 0.0;
        for row in &mut self.schedule {
            row.income_payment = cents(row.income_payment);
            row.tax_rate = cents(row.tax_rate);
            row.tax_withheld = cents(js_to_fixed2(row.income_payment * row.tax_rate / 100.0));
            schedule_total = cents(js_to_fixed2(schedule_total + row.tax_withheld));
        }
        self.schedule_total = schedule_total;
    }

    /// The official field values the upload loop reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("{PREFIX}:{key}"), value);
        };
        let flag = |on: bool| on.to_string();

        put("txtPg1I1Month", format!("{:02}", self.month));
        put("txtPg1I1Year", format!("{:02}", self.year % 100));
        put("rdoPg1I2AmendedYes", flag(self.is_amended));
        put("rdoPg1I2AmendedNo", flag(!self.is_amended));
        put("rdoPg1I3TaxWithheldYes", flag(self.taxes_withheld));
        put("rdoPg1I3TaxWithheldNo", flag(!self.taxes_withheld));
        put(
            "txtPg1I4NoOfSheets",
            self.number_of_attached_sheets.to_string(),
        );
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtPg1TIN1", tin1.clone());
        put("txtPg1TIN2", tin2.clone());
        put("txtPg1TIN3", tin3.clone());
        put("txtPg1TIN4", branch.clone());
        let rdo = self.rdo_code.trim().to_string();
        put("txtPg1I6RDO", rdo.clone());
        put("rdoPg1Pt1I6RDO", rdo);
        let name = capitalized(&self.agent_name);
        put("txtPg1Pt1I7WithholdingAgentsName", name.clone());
        let (address1, address2) = split_address(&self.registered_address);
        put("txtPg1Pt1I8RegisteredAddress", address1);
        put("txtPg1Pt1I8RegisteredAddress2", address2);
        put("txPg1I8ZipCode", self.zip_code.trim().to_string());
        put(
            "txtPg1Pt1I9ContactNumber",
            self.contact_number.trim().to_string(),
        );
        put(
            "rdoPg1I10CategoryofWthholdAgentPriv",
            flag(self.agent_category == Some(Form1600VtAgentCategory::Private)),
        );
        put(
            "rdoPg1I10CategoryofWthholdAgentGovt",
            flag(self.agent_category == Some(Form1600VtAgentCategory::Government)),
        );
        put("txtPg1Pt1I11Email", self.email.trim().to_string());
        put("rdoPg1I12TaxReliefYes", flag(self.tax_relief));
        put("rdoPg1I12TaxReliefNo", flag(!self.tax_relief));
        put(
            "txtPg1Pt1I12TaxReliefSpec",
            capitalized(&self.tax_relief_specify),
        );

        for index in 0..self.atc_rows.len().max(FORM_1600VT_ATC_ROWS) {
            let n = index + 1;
            match self.atc_rows.get(index) {
                Some(row) => {
                    put(&format!("txtAtcCd{n}"), row.atc_code.clone());
                    put(
                        &format!("txtPg1P2TaxBase{n}"),
                        official_amount(row.tax_base),
                    );
                    // The popup cell is "<td> 12.0</td>"; its innerHTML keeps
                    // the leading space.
                    put(
                        &format!("txtPg1P2TaxRate{n}"),
                        format!(" {}", row.rate_text),
                    );
                    put(
                        &format!("txtPg1P2TaxWithheld{n}"),
                        official_amount(row.tax_withheld),
                    );
                }
                None => {
                    put(&format!("txtAtcCd{n}"), String::new());
                    put(&format!("txtPg1P2TaxBase{n}"), String::new());
                    put(&format!("txtPg1P2TaxRate{n}"), String::new());
                    put(&format!("txtPg1P2TaxWithheld{n}"), "0.00".to_string());
                }
            }
        }
        put("txtTotalOtherTax", official_amount(self.total_other_tax));
        put(
            "txtPg1P2I18TotTaxWithheld",
            official_amount(self.total_tax_withheld),
        );
        put(
            "txtPg1P2I19TaxRemittedInRtrn",
            official_amount(self.tax_remitted_previous),
        );
        put(
            "txtPg1P2I20OthrPymntsMade",
            official_amount(self.other_payments),
        );
        put(
            "txtPg1P2I21TotTaxPymntsMade",
            official_amount(self.total_payments),
        );
        put(
            "txtPg1P2I22TaxStillDueOverremit",
            official_amount(self.tax_still_due),
        );
        put("txtPg1P2I23Surcharge", official_amount(self.surcharge));
        put("txtPg1P2I24Interest", official_amount(self.interest));
        put("txtPg1P2I25Compromise", official_amount(self.compromise));
        put(
            "txtPg1P2I26TotPenalties",
            official_amount(self.total_penalties),
        );
        put(
            "txtPg1P2I27TotAmtStillDueOverremit",
            official_amount(self.total_amount_due),
        );

        // Page 2 header: loadBGData fills every TIN1..TIN4 by name and
        // sleeptime copies the name into RegName.
        put("txtPg2TinC1", tin1);
        put("txtPg2TinC2", tin2);
        put("txtPg2TinC3", tin3);
        put("txtPg2TinC4", branch);
        put("txtPg2RegisteredName", name);
        for index in 0..FORM_1600VT_SCHEDULE_ROWS {
            let n = index + 1;
            let row = self.schedule.get(index).cloned().unwrap_or_default();
            put(&format!("Pg2Sc1TIN{n}"), row.tin.trim().to_string());
            put(&format!("Pg2Sc1Name{n}"), capitalized(&row.payee_name));
            put(&format!("Pg2Sc1ATC{n}"), capitalized(&row.atc));
            put(
                &format!("Pg2Sc1AmtOfIncPmt{n}"),
                official_amount(row.income_payment),
            );
            put(&format!("Pg2Sc1TaxRate{n}"), official_amount(row.tax_rate));
            put(
                &format!("Pg2Sc1AmtOfTxWthhld{n}"),
                official_amount(row.tax_withheld),
            );
        }
        put(
            "Pg2Sc1TotTxWthhldAndRmtted",
            official_amount(self.schedule_total),
        );
        put("txtLOB", self.line_of_business.trim().to_string());

        // sleeptime sets the pager to page 1.
        put("txtCurrentPage", "1".to_string());
        // The ATC popup's checkboxes; getATCCode leaves the chosen ones ticked.
        if let Some(category) = self.agent_category {
            for (n, (_, option)) in form_1600vt_popup(category).enumerate() {
                let ticked = self
                    .atc_rows
                    .iter()
                    .any(|row| row.atc_code == option.code && row.rate_text == option.rate_text);
                fields.insert(format!("AtcCode{}", n + 1), ticked.to_string());
            }
        }
        fields
    }

    /// The field map plus print-only values the frozen 2018 sheet needs
    /// (`derived:` keys, never submitted): the 14-digit TIN for page 2 and
    /// each Part II rate without its decimal (`12.0` prints as `12`).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let (a, b, c, d) = split_tin(&self.tin);
        fields.insert("derived:tin_digits".to_string(), format!("{a}{b}{c}{d}"));
        for (index, row) in self.atc_rows.iter().enumerate().take(FORM_1600VT_ATC_ROWS) {
            let rate = row.rate_text.trim_end_matches(".0");
            let rate = rate
                .strip_prefix('0')
                .filter(|r| r.starts_with('.'))
                .unwrap_or(rate);
            fields.insert(format!("derived:rate{}", index + 1), rate.to_string());
        }
        fields
    }

    /// The submit layout for the Item 10 category.
    pub fn layout_id(&self) -> &'static str {
        match self.agent_category {
            Some(Form1600VtAgentCategory::Private) => FORM_1600VT_PRIVATE_LAYOUT_ID,
            _ => FORM_1600VT_GOVERNMENT_LAYOUT_ID,
        }
    }

    /// The generated layout with the "Other Selected ATC" rows (`txtAtcCd6`
    /// onward) spliced in after `hPartIITableSize`, where `getATCCode` draws
    /// them inside the form.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        let base = crate::official_xml::layout(self.layout_id()).map_err(|e| e.to_string())?;
        let columns = [
            "txtAtcCd",
            "txtPg1P2TaxBase",
            "txtPg1P2TaxRate",
            "txtPg1P2TaxWithheld",
        ];
        let last = FORM_1600VT_ATC_ROWS;
        let insertions: Vec<RowInsertion> = (last + 1..=self.atc_rows.len())
            .map(|n| RowInsertion {
                after: if n == last + 1 {
                    "hPartIITableSize".to_string()
                } else {
                    format!("{PREFIX}:txtPg1P2TaxWithheld{}", n - 1)
                },
                copies: columns
                    .iter()
                    .map(|column| {
                        (
                            format!("{PREFIX}:{column}{last}"),
                            format!("{PREFIX}:{column}{n}"),
                        )
                    })
                    .collect(),
            })
            .collect();
        extend_layout(base, &insertions)
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

impl FormValidator for Form1600VtDraft {
    /// `validateAll` in order (with `checkDate`), then
    /// `initialValidateBeforeSave`, with their alert texts, plus the input
    /// limits the page enforces while typing and a few stricter checks where
    /// the page lets an inconsistent return through (noted inline).
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // checkDate (Item 1).
        let today = chrono::Local::now().date_naive();
        let current_year = u16::try_from(today.year()).unwrap_or(u16::MAX);
        if self.year < 2018 || self.year > current_year {
            err(
                "year",
                "Year shall not be greater than the present year and not earlier than 2018.",
            );
        } else if self.year == current_year && u32::from(self.month) > today.month() {
            err("month", "Date (Page 1 Item 1) cannot be a future date.");
        }
        if !(1..=12).contains(&self.month) {
            err("month", "Please select a Month in Page 1 Item 1.");
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 4 holds at most two digits.",
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
            err("tin", "Please enter a valid TIN on Item 5.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err(
                "rdo_code",
                "Please enter a valid RDO Code on Page 1 Item 6.",
            );
        }
        let name = self.agent_name.trim();
        if name.is_empty() {
            err("agent_name", "Name field on Page 1 Item 7 is required.");
        } else if name.chars().count() > 60 {
            err("agent_name", "Item 7 holds at most 60 characters.");
        }
        let address = self.registered_address.trim();
        if address.is_empty() {
            err(
                "registered_address",
                "Registered Address field on Page 1 Item 8 is required.",
            );
        } else if address.chars().count() > 160 {
            err(
                "registered_address",
                "Item 8 holds at most 160 characters (two lines of 80).",
            );
        }
        if self.zip_code.trim().is_empty() {
            err("zip_code", "Zip Code field on Page 1 Item 8A is required.");
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() {
            err(
                "contact_number",
                "Contact Number field on Page 1 Item 9 is required.",
            );
        } else if phone.len() > 25 || !digits_only(phone) {
            err("contact_number", "Item 9 holds at most 25 digits.");
        }
        if self.agent_category.is_none() {
            err("agent_category", "Please select an option for item 10");
        }
        let email = self.email.trim();
        if email.is_empty() {
            err("email", "E-mail address on page 1 item 11 is required.");
        } else if email.len() > 112 || !email.contains('@') || email.contains(char::is_whitespace) {
            err(
                "email",
                "Please enter a valid e-mail address on page 1 item 10",
            );
        }
        if self.tax_relief && self.tax_relief_specify.trim().is_empty() {
            err(
                "tax_relief_specify",
                "Specify Tax Relief field on Page 1 Item 12A is required.",
            );
        }
        if self.tax_relief_specify.trim().chars().count() > 100 {
            err(
                "tax_relief_specify",
                "Item 12A holds at most 100 characters.",
            );
        }

        // Part II.
        if self.taxes_withheld {
            if self.atc_rows.is_empty() {
                err(
                    "atc_rows",
                    "Please fill up Part II Computation of Tax if item 3 is set to Yes.",
                );
            }
            for (index, row) in self.atc_rows.iter().enumerate() {
                if row.tax_base == 0.0 {
                    err(
                        &format!("atc_rows[{index}].tax_base"),
                        &format!(
                            "Please enter a valid value for tax base for {}.",
                            row.atc_code
                        ),
                    );
                }
            }
        } else if !self.atc_rows.is_empty() {
            err(
                "atc_rows",
                "Selecting an ATC is not necessary when item no. 3 is set to ' NO '",
            );
        }
        for (index, row) in self.atc_rows.iter().enumerate() {
            match (row.option(), self.agent_category) {
                (None, _) => err(
                    &format!("atc_rows[{index}].atc_code"),
                    &format!("{} is not an ATC the official form offers.", row.atc_code),
                ),
                (Some(option), Some(category)) if !option.offered_to(category) => err(
                    &format!("atc_rows[{index}].atc_code"),
                    &format!(
                        "{} is not offered to this category of withholding agent.",
                        option.code
                    ),
                ),
                _ => {}
            }
            if self.atc_rows[..index]
                .iter()
                .any(|other| other.atc_code == row.atc_code && other.rate_text == row.rate_text)
            {
                err(
                    &format!("atc_rows[{index}].atc_code"),
                    "Each ATC is selected once.",
                );
            }
            if row.tax_base < 0.0 || !row.tax_base.is_finite() || row.tax_base >= 1e15 {
                err(
                    &format!("atc_rows[{index}].tax_base"),
                    "Enter a non-negative tax base.",
                );
            }
        }
        for (field, value) in [
            ("tax_remitted_previous", self.tax_remitted_previous),
            ("other_payments", self.other_payments),
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
                "Item 19 applies only to an amended return.",
            );
        }

        // Page 2 (checkATCinPageTwo, validateSchOneTIN, checkRate).
        if self.schedule.len() > FORM_1600VT_SCHEDULE_ROWS {
            err("schedule", "Schedule 1 has five rows on Page 2.");
        }
        for (index, row) in self
            .schedule
            .iter()
            .enumerate()
            .take(FORM_1600VT_SCHEDULE_ROWS)
        {
            let n = index + 1;
            let text_missing = row.tin.trim().is_empty()
                || row.payee_name.trim().is_empty()
                || row.atc.trim().is_empty();
            let text_present = !row.tin.trim().is_empty()
                || !row.payee_name.trim().is_empty()
                || !row.atc.trim().is_empty();
            let amount_present =
                row.income_payment != 0.0 || row.tax_rate != 0.0 || row.tax_withheld != 0.0;
            let amount_missing =
                row.income_payment == 0.0 || row.tax_rate == 0.0 || row.tax_withheld == 0.0;
            if (text_missing && amount_present) || (text_present && amount_missing) {
                err(
                    &format!("schedule[{index}]"),
                    &format!("Please complete Item #{n} in Page 2."),
                );
            }
            let tin = row.tin.trim();
            let max_tin = if n == 5 { 15 } else { 14 };
            if !tin.is_empty() && (tin.len() <= 11 || tin.len() > max_tin || !digits_only(tin)) {
                err(
                    &format!("schedule[{index}].tin"),
                    &format!("Please check TIN number in Page 2 row #{n}."),
                );
            }
            if row.payee_name.trim().chars().count() > 40 {
                err(
                    &format!("schedule[{index}].payee_name"),
                    "Page 2 payee names hold at most 40 characters.",
                );
            }
            if row.atc.trim().chars().count() > 5 {
                err(
                    &format!("schedule[{index}].atc"),
                    "Page 2 ATCs hold at most 5 characters.",
                );
            }
            if row.tax_rate >= 100.0 {
                err(
                    &format!("schedule[{index}].tax_rate"),
                    "Tax rate should be below 100.",
                );
            }
            if row.income_payment < 0.0 || row.tax_rate < 0.0 {
                err(
                    &format!("schedule[{index}].income_payment"),
                    "Enter non-negative amounts on Page 2.",
                );
            }
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

impl Form1600VtDraft {
    /// Items 23-25 on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: Withholding,
            items: [
                surcharge: "23 Surcharge",
                interest: "24 Interest",
                compromise: "25 Compromise",
            ],
            controls: [
                "frm1600VT:txtPg1P2I23Surcharge",
                "frm1600VT:txtPg1P2I24Interest",
                "frm1600VT:txtPg1P2I25Compromise",
            ],
            state: penalty,
            basic_tax_due: "Item 22", |d| d.tax_still_due,
        )
    }
}

impl QueueableForm for Form1600VtDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1600VT";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1600VTv2018']`).
    const FORM_TYPE: &'static str = "1600VTv2018";
    /// The government layout; [`Self::official_payload`] picks the one for
    /// the Item 10 category.
    const LAYOUT_ID: &'static str = FORM_1600VT_GOVERNMENT_LAYOUT_ID;

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
    /// `month + "20" + year`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}20{:02}", self.month, self.year % 100)
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
        self.to_bir_field_map()
    }
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

    pub(crate) fn sample() -> Form1600VtDraft {
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Sample Agent Corp",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Trading",
            "registered_address": "123 Sample St Quezon City",
            "zip_code": "1100",
            "phone": "0281234567",
            "email": "sample.agent@example.com",
            "default_form_type": "1600VT",
            "taxpayer_type": "Corporation"
        }))
        .unwrap();
        let mut draft = Form1600VtDraft::new_from_profile(&profile, 2025, 6);
        draft.set_agent_category(Form1600VtAgentCategory::Government);
        draft.set_taxes_withheld(true);
        draft.toggle_atc(0).unwrap();
        draft.toggle_atc(2).unwrap();
        draft.atc_rows[0].tax_base = 100_000.005;
        draft.atc_rows[1].tax_base = 50_000.0;
        draft.surcharge = 25.0;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1600VtDraft) -> Vec<String> {
        <Form1600VtDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn js_to_fixed_matches_javascript() {
        assert_eq!(js_to_fixed2(1.005), 1.0);
        assert_eq!(js_to_fixed2(0.125), 0.13);
        assert_eq!(js_to_fixed2(-0.125), -0.13);
        assert_eq!(js_to_fixed2(2.675), 2.67);
        assert_eq!(js_to_fixed2(1234.5678), 1234.57);
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        // 100,000.005 x 5% = 5,000.00025 from the unrounded entry.
        assert_eq!(draft.atc_rows[0].tax_withheld, 5_000.0);
        assert_eq!(draft.atc_rows[1].tax_withheld, 6_000.0);
        assert_eq!(draft.total_tax_withheld, 11_000.0);
        assert_eq!(draft.tax_still_due, 11_000.0);
        assert_eq!(draft.total_amount_due, 11_025.0);

        let mut over = sample();
        over.other_payments = 20_000.0;
        over.recompute();
        assert_eq!(over.tax_still_due, -9_000.0);
        // Overremittance with penalties: Item 27 shows the penalties only.
        assert_eq!(over.total_amount_due, 25.0);
    }

    #[test]
    fn field_map_uses_official_formats() {
        let fields = sample().to_bir_field_map();
        assert_eq!(fields["frm1600VT:txtPg1I1Month"], "06");
        assert_eq!(fields["frm1600VT:txtPg1I1Year"], "25");
        assert_eq!(fields["frm1600VT:txtAtcCd1"], "WV010");
        assert_eq!(fields["frm1600VT:txtPg1P2TaxBase1"], "100,000.01");
        assert_eq!(fields["frm1600VT:txtPg1P2TaxRate1"], " 5.0");
        assert_eq!(fields["frm1600VT:txtAtcCd2"], "WV110");
        assert_eq!(fields["frm1600VT:txtPg1P2TaxRate2"], " 12.0");
        assert_eq!(fields["frm1600VT:txtAtcCd3"], "");
        assert_eq!(fields["frm1600VT:txtPg1P2TaxWithheld3"], "0.00");
        assert_eq!(
            fields["frm1600VT:txtPg1Pt1I7WithholdingAgentsName"],
            "SAMPLE AGENT CORP"
        );
        assert_eq!(fields["frm1600VT:txtPg2TinC4"], "00000");
        assert_eq!(fields["frm1600VT:txtCurrentPage"], "1");
        let draft = sample();
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1600VTv2018-062025#sample.agent@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "062025");
        assert_eq!(
            Form1600VtDraft::parse_period_code("062025"),
            Some((2025, FilingPeriod::Monthly(6)))
        );
        assert_eq!(Form1600VtDraft::parse_period_code("132025"), None);
        assert_eq!(Form1600VtDraft::parse_period_code("122025Q1"), None);
    }

    #[test]
    fn atc_popup_rules() {
        let mut draft = sample();
        // WV080 is private-only.
        let wv080 = form_1600vt_atc_index("WV080", "12.0").unwrap();
        assert!(draft.toggle_atc(wv080).is_err());
        draft.set_agent_category(Form1600VtAgentCategory::Private);
        assert!(draft.atc_rows.is_empty());
        draft.toggle_atc(wv080).unwrap();
        draft.toggle_atc(2).unwrap();
        // Popup order, not click order.
        assert_eq!(draft.atc_rows[0].atc_code, "WV110");
        draft.set_taxes_withheld(false);
        assert!(draft.atc_rows.is_empty());
        assert!(draft.toggle_atc(2).is_err());
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1600VtDraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.year = 2017,
            "Year shall not be greater than the present year and not earlier than 2018.",
        );
        check(&|d| d.month = 0, "Please select a Month in Page 1 Item 1.");
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Page 1 Item 6.",
        );
        check(
            &|d| d.agent_name.clear(),
            "Name field on Page 1 Item 7 is required.",
        );
        check(
            &|d| d.registered_address = " ".into(),
            "Registered Address field on Page 1 Item 8 is required.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Zip Code field on Page 1 Item 8A is required.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Contact Number field on Page 1 Item 9 is required.",
        );
        check(
            &|d| d.agent_category = None,
            "Please select an option for item 10",
        );
        check(
            &|d| d.email.clear(),
            "E-mail address on page 1 item 11 is required.",
        );
        check(
            &|d| d.email = "not-an-email".into(),
            "Please enter a valid e-mail address on page 1 item 10",
        );
        check(
            &|d| d.tax_relief = true,
            "Specify Tax Relief field on Page 1 Item 12A is required.",
        );
        check(
            &|d| {
                d.atc_rows.clear();
                d.recompute();
            },
            "Please fill up Part II Computation of Tax if item 3 is set to Yes.",
        );
        check(
            &|d| {
                d.atc_rows[1].tax_base = 0.0;
                d.recompute();
            },
            "Please enter a valid value for tax base for WV110.",
        );
        check(
            &|d| {
                d.schedule.push(Form1600VtScheduleRow {
                    tin: "123456789000".into(),
                    payee_name: "Payee".into(),
                    atc: "WV010".into(),
                    ..Default::default()
                });
                d.recompute();
            },
            "Please complete Item #1 in Page 2.",
        );
        check(
            &|d| {
                d.schedule.push(Form1600VtScheduleRow {
                    tin: "12345".into(),
                    payee_name: "Payee".into(),
                    atc: "WV010".into(),
                    income_payment: 100.0,
                    tax_rate: 5.0,
                    tax_withheld: 0.0,
                });
                d.recompute();
            },
            "Please check TIN number in Page 2 row #1.",
        );
        check(
            &|d| {
                d.schedule.push(Form1600VtScheduleRow {
                    tin: "123456789000".into(),
                    payee_name: "Payee".into(),
                    atc: "WV010".into(),
                    income_payment: 100.0,
                    tax_rate: 100.0,
                    tax_withheld: 0.0,
                });
                d.recompute();
            },
            "Tax rate should be below 100.",
        );
        check(
            &|d| {
                d.taxes_withheld = false;
            },
            "Selecting an ATC is not necessary when item no. 3 is set to ' NO '",
        );
        check(
            &|d| d.surcharge = 1.0,
            "Totals are out of date. Recompute the return.",
        );
    }

    #[test]
    fn sixth_atc_onward_goes_to_other_selected_atc() {
        let mut draft = sample();
        draft.set_agent_category(Form1600VtAgentCategory::Government);
        let offered: Vec<usize> = form_1600vt_popup(Form1600VtAgentCategory::Government)
            .map(|(index, _)| index)
            .collect();
        for index in offered.iter().copied() {
            if !draft
                .atc_rows
                .iter()
                .any(|row| form_1600vt_atc_index(&row.atc_code, &row.rate_text) == Some(index))
            {
                draft.toggle_atc(index).unwrap();
            }
        }
        assert_eq!(draft.atc_rows.len(), offered.len());
        for row in &mut draft.atc_rows {
            row.tax_base = 1_000.0;
        }
        draft.recompute();
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        let other: f64 = draft.atc_rows[FORM_1600VT_ATC_ROWS..]
            .iter()
            .map(|row| row.tax_withheld)
            .sum();
        assert_eq!(draft.total_other_tax, cents(other));
        let fields = draft.to_bir_field_map();
        let last = draft.atc_rows.len();
        assert_eq!(
            fields[&format!("frm1600VT:txtAtcCd{last}")],
            draft.atc_rows[last - 1].atc_code
        );
        assert_eq!(
            fields["frm1600VT:txtTotalOtherTax"],
            official_amount(draft.total_other_tax)
        );
        let payload = draft.to_bir_xml_payload().unwrap();
        let table = payload.find("hPartIITableSize=").unwrap();
        assert!(payload[table..].contains("frm1600VT:txtAtcCd6="));
        assert!(payload[..table].contains("frm1600VT:txtAtcCd5="));
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
            <Form1600VtDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form1600VtDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
