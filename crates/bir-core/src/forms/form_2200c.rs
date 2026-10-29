//! BIR Form 2200-C (January 2018) — Excise Tax Return for Cosmetic
//! Procedures.
//!
//! Ported from the official `BIR-Form2200Cv2018.hta` (eBIRForms 7.9.6.2.1):
//! Part V Schedule 1 (`partFiveComputation`, `totalExciseTax`), Part III
//! (`partThreeComputation`), the item handlers (`isAmended`,
//! `availTaxRelief`, `modeOfPayment` with `resetPartFive`, `checkYear`),
//! `validateAll` / `checkPartVFields` / `validateDate` with their exact alerts
//! and `saveXMLsubmit` through [`crate::official_xml`].
//!
//! Every computed amount on this page is `formatCurrency(x.toFixed(0))`, so
//! the return is in whole pesos; the typed amount fields accept digits only.
//! Column I (VAT) multiplies by `#vatColumnI`, a span whose `value`
//! `js/tax-rate-helper.js` sets from `xml/taxRate.xml` (12%); the HTA's
//! legacy IE document mode exposes that attribute as the span's `.value`.
//! Column H uses the page's literal 0.05 (taxRate.xml `exciseTaxColumnH` is
//! the same 5%).

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::excise_places::{ExcisePlace, is_official_place};
use super::form_2000::{cents, digits_only, split_tin, tin_error};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::official_amount;
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_2200C_FORM_ID: &str = "2200c-v2018";
/// Rows of Part V Schedule 1.
pub const FORM_2200C_ROWS: usize = 10;
const PLACE_FORM: &str = "2200C";
/// `xml/taxRate.xml` `exciseTaxColumnH` (the page multiplies by literal 0.05).
pub const FORM_2200C_EXCISE_RATE: f64 = 0.05;
/// `xml/taxRate.xml` `vatColumnI`.
pub const FORM_2200C_VAT_RATE: f64 = 0.12;

/// JavaScript `toFixed(0)` read back as a number (exact binary value, ties
/// away from zero).
fn js_to_fixed0(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let exact = format!("{:.80}", value.abs());
    let (whole, frac) = exact.split_once('.').unwrap_or((&exact, "0"));
    let mut magnitude: f64 = whole.parse().unwrap_or(0.0);
    if frac.as_bytes().first().is_some_and(|d| *d >= b'5') {
        magnitude += 1.0;
    }
    if value < 0.0 { -magnitude } else { magnitude }
}

/// `formatCurrency(x.toFixed(0))` as the value the field then holds.
fn pesos(value: f64) -> f64 {
    cents(js_to_fixed0(value))
}

fn whole_pesos(value: f64) -> bool {
    value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value < 1e12
}

/// Part II — Manner of payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2200CManner {
    #[default]
    Unanswered,
    ActualRemoval,
    /// Prepayment/Advance Deposit: `resetPartFive` zeroes Schedule 1.
    Prepayment,
    /// Other similar schemes (specify).
    Other,
}

/// One Part V Schedule 1 row (columns A to J).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200CRow {
    /// (A) Cosmetic procedures performed — exempt.
    pub exempt_procedures: f64,
    /// (B) Excisable.
    pub excisable_procedures: f64,
    /// (C) Non-excisable.
    pub non_excisable_procedures: f64,
    /// (D) Gross receipts — non-invasive, net of VAT.
    pub net_of_vat: f64,
    /// (E) Invasive, excisable, net of VAT and excise.
    pub excisable_net: f64,
    /// (F) Invasive, excisable, VAT exempt.
    pub excisable_vat_exempt: f64,
    /// (G) Invasive, non-excisable, net of VAT.
    pub non_excisable: f64,
    /// (H) = (E + F) × 5%.
    #[serde(default)]
    pub excise_tax: f64,
    /// (I) = (D + E + G + H) × 12% (taxRate.xml).
    #[serde(default)]
    pub vat: f64,
    /// (J) Sum of D to I.
    #[serde(default)]
    pub total_billed: f64,
}

impl Form2200CRow {
    fn counts(&self) -> [f64; 3] {
        [
            self.exempt_procedures,
            self.excisable_procedures,
            self.non_excisable_procedures,
        ]
    }

    fn amounts(&self) -> [f64; 4] {
        [
            self.net_of_vat,
            self.excisable_net,
            self.excisable_vat_exempt,
            self.non_excisable,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2200CDraft {
    #[serde(default)]
    pub id: Option<i64>,

    /// Item 1.
    pub month: u8,
    pub day: u8,
    pub year: u16,
    /// Item 2.
    pub is_amended: bool,
    /// Item 5: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    /// Item 6.
    pub rdo_code: String,
    /// Item 7.
    pub taxpayer_name: String,
    /// Item 8.
    pub registered_address: String,
    /// Item 8A.
    pub zip_code: String,
    /// Item 9.
    pub contact_number: String,
    /// Item 10.
    pub email: String,
    /// Hidden `txtLOB`, from the profile.
    #[serde(default)]
    pub line_of_business: String,
    /// Item 11: where the invasive procedures took place.
    #[serde(default)]
    pub place: ExcisePlace,
    /// Item 12: availing of tax relief (unanswered until picked).
    #[serde(default)]
    pub tax_relief: Option<bool>,
    /// Item 12A.
    #[serde(default)]
    pub tax_relief_specify: String,
    /// Part II.
    #[serde(default)]
    pub manner: Form2200CManner,
    /// Item 15.
    #[serde(default)]
    pub manner_other: String,

    /// Part V Schedule 1, up to ten rows.
    #[serde(default)]
    pub schedule: Vec<Form2200CRow>,
    #[serde(default)]
    pub excise_total: f64,

    // Part III
    /// 16.
    #[serde(default)]
    pub excise_tax_due: f64,
    /// 17A–17C.
    #[serde(default)]
    pub balance_carried_over: f64,
    #[serde(default)]
    pub creditable_excise_tax: f64,
    #[serde(default)]
    pub total_credits: f64,
    /// 18.
    #[serde(default)]
    pub net_tax_due: f64,
    /// 19, only on an amended return.
    #[serde(default)]
    pub previous_payment: f64,
    /// 20.
    #[serde(default)]
    pub tax_still_due: f64,
    /// 21A–21D.
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
    /// 22.
    #[serde(default)]
    pub amount_payable: f64,
    /// 23A–23C.
    #[serde(default)]
    pub tax_payment: f64,
    #[serde(default)]
    pub penalties_paid: f64,
    #[serde(default)]
    pub total_payment: f64,
    /// 24.
    #[serde(default)]
    pub balance_carried_forward: f64,

    /// Item 4, number of sheets attached (`txtPg1I4NoSheets`; the page
    /// uploads it as typed, `0` by default).
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

impl Form2200CDraft {
    pub const FORM_CODE: &'static str = "2200C";

    /// A new return for the library tile's `month` of `year`: today when that
    /// month is the current one, else the month's last day. A `month` outside
    /// 1-12 falls back to [`Self::new_from_profile`].
    pub fn new_for_month(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        let mut draft = Self::new_from_profile(profile, year);
        if !(1..=12).contains(&month) {
            return draft;
        }
        let today = chrono::Local::now().date_naive();
        let current = i32::from(year) == chrono::Datelike::year(&today)
            && u32::from(month) == chrono::Datelike::month(&today);
        draft.month = month;
        draft.day = if current {
            chrono::Datelike::day(&today) as u8
        } else {
            let (next_year, next_month) = if month == 12 {
                (i32::from(year) + 1, 1)
            } else {
                (i32::from(year), u32::from(month) + 1)
            };
            chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
                .and_then(|first| first.pred_opt())
                .map(|last| chrono::Datelike::day(&last) as u8)
                .unwrap_or(28)
        };
        draft
    }

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16) -> Self {
        let today = chrono::Local::now().date_naive();
        let (month, day) = if i32::from(year) == chrono::Datelike::year(&today) {
            (
                chrono::Datelike::month(&today) as u8,
                chrono::Datelike::day(&today) as u8,
            )
        } else {
            (12, 31)
        };
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile_typed_due_date(profile),
            id: None,
            month,
            day,
            year,
            is_amended: false,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            line_of_business: profile.line_of_business.clone(),
            place: ExcisePlace::default(),
            tax_relief: None,
            tax_relief_specify: String::new(),
            manner: Form2200CManner::Unanswered,
            manner_other: String::new(),
            schedule: Vec::new(),
            excise_total: 0.0,
            excise_tax_due: 0.0,
            balance_carried_over: 0.0,
            creditable_excise_tax: 0.0,
            total_credits: 0.0,
            net_tax_due: 0.0,
            previous_payment: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            amount_payable: 0.0,
            tax_payment: 0.0,
            penalties_paid: 0.0,
            total_payment: 0.0,
            balance_carried_forward: 0.0,
            number_of_attached_sheets: 0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// The official compute chain.
    pub fn recompute(&mut self) {
        if self.tax_relief != Some(true) {
            self.tax_relief_specify.clear();
        }
        if self.manner != Form2200CManner::Other {
            self.manner_other.clear();
        }
        if !self.is_amended {
            self.previous_payment = 0.0;
        }
        if matches!(
            self.manner,
            Form2200CManner::Prepayment | Form2200CManner::Unanswered
        ) {
            // resetPartFive (also run at init, before a manner is picked).
            self.schedule.clear();
        }
        let mut total = 0.0;
        for row in &mut self.schedule {
            for value in [
                &mut row.exempt_procedures,
                &mut row.excisable_procedures,
                &mut row.non_excisable_procedures,
                &mut row.net_of_vat,
                &mut row.excisable_net,
                &mut row.excisable_vat_exempt,
                &mut row.non_excisable,
            ] {
                *value = cents(*value);
            }
            row.excise_tax =
                pesos((row.excisable_net + row.excisable_vat_exempt) * FORM_2200C_EXCISE_RATE);
            row.vat = pesos(
                (row.net_of_vat + row.excisable_net + row.non_excisable + row.excise_tax)
                    * FORM_2200C_VAT_RATE,
            );
            row.total_billed = pesos(
                row.net_of_vat
                    + row.excisable_net
                    + row.excisable_vat_exempt
                    + row.non_excisable
                    + row.excise_tax
                    + row.vat,
            );
        }
        for row in &self.schedule {
            total = pesos(total + row.excise_tax);
        }
        self.excise_total = total;

        self.excise_tax_due = cents(self.excise_total);
        self.balance_carried_over = cents(self.balance_carried_over);
        self.creditable_excise_tax = cents(self.creditable_excise_tax);
        self.total_credits = pesos(self.balance_carried_over + self.creditable_excise_tax);
        self.net_tax_due = pesos(self.excise_tax_due - self.total_credits);
        self.previous_payment = cents(self.previous_payment);
        self.tax_still_due = pesos(self.net_tax_due - self.previous_payment);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = pesos(self.surcharge + self.interest + self.compromise);
        self.amount_payable = pesos(self.tax_still_due + self.total_penalties);
        self.penalties_paid = pesos(self.total_penalties);
        self.tax_payment = cents(self.tax_payment);
        self.total_payment = pesos(self.tax_payment + self.penalties_paid);
        self.balance_carried_forward = pesos(self.amount_payable - self.total_payment);
    }

    /// The print preview's map: the submit map plus `derived:` keys for the
    /// printed combs that Items 9-10 and the three Item 11 levels share
    /// (one 40-slot comb each, values placed under their own captions), and
    /// `derived:page_header`, the page 2 header comb (TIN then name, one
    /// 40-slot comb on this sheet).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let mut put = |key: &str, value: String| {
            if !value.trim().is_empty() {
                fields.insert(format!("derived:{key}"), value);
            }
        };
        put(
            "contact_email",
            super::excise_places::contact_email_comb_line(
                &self.contact_number,
                &self.email,
                13,
                40,
            ),
        );
        put(
            "place",
            super::excise_places::shared_comb_line(
                &[
                    (0, self.place.region.as_str()),
                    (10, self.place.province.as_str()),
                    (24, self.place.city.as_str()),
                ],
                40,
            ),
        );
        put(
            "page_header",
            super::print_map::tin_name_header(&self.tin, &self.taxpayer_name),
        );
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2200C:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let money = official_amount;

        put("txtPg1I1Month", format!("{:02}", self.month));
        put("txtPg1I1Day", format!("{:02}", self.day));
        put("txtPg1I1Year", self.year.to_string());
        put("rdoPg1I2AmendedYes", flag(self.is_amended));
        put("rdoPg1I2AmendedNo", flag(!self.is_amended));
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        for (key, value) in [
            ("txtPg1TIN1", &tin1),
            ("txtPg1TIN2", &tin2),
            ("txtPg1TIN3", &tin3),
            ("txtPg1TIN4", &branch),
            ("txtPg2TinC1", &tin1),
            ("txtPg2TinC2", &tin2),
            ("txtPg2TinC3", &tin3),
            ("txtPg2TinC4", &branch),
        ] {
            put(key, value.clone());
        }
        put("txtPg1I6RDO", self.rdo_code.trim().to_string());
        put("rdoPg1Pt1I6RDO", self.rdo_code.trim().to_string());
        // loadBGData uppercases the name only; the address keeps the profile
        // casing and splits at 80 characters into the second box.
        put(
            "txtPg1Pt1I7RegisteredName",
            self.taxpayer_name.trim().to_uppercase(),
        );
        let address: Vec<char> = self.registered_address.trim().chars().collect();
        put(
            "txtPg1Pt1I8RegisteredAddress",
            address.iter().take(80).collect(),
        );
        put(
            "txtPg1Pt1I8RegisteredAddress2",
            address.iter().skip(80).take(80).collect(),
        );
        put("txPg1I8ZipCode", self.zip_code.trim().to_string());
        put(
            "txtPg1Pt1I9ContactNumber",
            self.contact_number.trim().to_string(),
        );
        put("txtPg1Pt1I10Email", self.email.trim().to_string());
        let [region, province, city] = self.place.values();
        put("txtPg1Pt1I11Region", region);
        put("txtPg1Pt1I11Province", province);
        put("txtPg1Pt1I11City", city);
        put("rdoPg1I12TaxReliefYes", flag(self.tax_relief == Some(true)));
        put("rdoPg1I12TaxReliefNo", flag(self.tax_relief == Some(false)));
        put(
            "txtPg1I12TaxReliefSpecify",
            self.tax_relief_specify.trim().to_uppercase(),
        );
        put(
            "rdoPg1Pt2MOPPaymentActual",
            flag(self.manner == Form2200CManner::ActualRemoval),
        );
        put(
            "rdoPg1Pt2MOPPrepayment",
            flag(self.manner == Form2200CManner::Prepayment),
        );
        put(
            "rdoPg1Pt2MOPOther",
            flag(self.manner == Form2200CManner::Other),
        );
        put(
            "txtPg1Pt2MOPOtherDesc",
            self.manner_other.trim().to_uppercase(),
        );

        put("txtPg1P3I16ExciseTaxDue", money(self.excise_tax_due));
        put(
            "txtPg1P3I17ABalCarriedOver",
            money(self.balance_carried_over),
        );
        put(
            "txtPg1P3I17BCredExciseTax",
            money(self.creditable_excise_tax),
        );
        put("txtPg1P3I17CTotal", money(self.total_credits));
        put("txtPg1P3I18NetTaxDue", money(self.net_tax_due));
        put(
            "txtPg1P3I19PmntOnRtrnPrevFiled",
            money(self.previous_payment),
        );
        put("txtPg1P3I20TaxStillDue", money(self.tax_still_due));
        put("txtPg1P3I21ASurcharge", money(self.surcharge));
        put("txtPg1P3I21BInterest", money(self.interest));
        put("txtPg1P3I21CCompromise", money(self.compromise));
        put("txtPg1P3I21DTotPenalties", money(self.total_penalties));
        put("txtPg1P3I22AmountPayable", money(self.amount_payable));
        put("txtPg1P3I23ATaxPmntDposit", money(self.tax_payment));
        put("txtPg1P3I23BPenalties", money(self.penalties_paid));
        put("txtPg1P3I23CTotPmntMade", money(self.total_payment));
        put(
            "txtPg1P3I24BalToCarryOver",
            money(self.balance_carried_forward),
        );

        for index in 0..FORM_2200C_ROWS {
            let row = self.schedule.get(index).cloned().unwrap_or_default();
            let n = index + 1;
            put(&format!("CPPexmpt{n}"), money(row.exempt_procedures));
            put(&format!("CPPexcise{n}"), money(row.excisable_procedures));
            put(
                &format!("CPPnonexcise{n}"),
                money(row.non_excisable_procedures),
            );
            put(&format!("GRnetVat{n}"), money(row.net_of_vat));
            put(&format!("GRexciseNet{n}"), money(row.excisable_net));
            put(
                &format!("GRexciseExmpt{n}"),
                money(row.excisable_vat_exempt),
            );
            put(&format!("GRnonexcise{n}"), money(row.non_excisable));
            put(&format!("ACexcise{n}"), money(row.excise_tax));
            put(&format!("ACvat{n}"), money(row.vat));
            put(&format!("ACtotAmountBill{n}"), money(row.total_billed));
        }
        put("ACexciseTotal", money(self.excise_total));
        put("txtLOB", self.line_of_business.trim().to_string());
        // The page the filer is on; the return is validated from page 1.
        put("txtCurrentPage", "1".to_string());
        fields.insert(
            "frm2200C:txtPg1I4NoSheets".to_string(),
            self.number_of_attached_sheets.to_string(),
        );
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    fn date(&self) -> Option<chrono::NaiveDate> {
        chrono::NaiveDate::from_ymd_opt(
            i32::from(self.year),
            u32::from(self.month),
            u32::from(self.day),
        )
    }
}

impl FormValidator for Form2200CDraft {
    /// `validateAll` in order with its alert texts (with `checkYear`,
    /// `validateDate` and `checkPartVFields`), then the typing limits.
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let today = chrono::Local::now().date_naive();
        let this_year = chrono::Datelike::year(&today);

        if !(1..=12).contains(&self.month) {
            err("month", "Month field on Page 1 Item 1 is required.");
        } else if self.day == 0 {
            err("day", "Day field on Page 1 Item 1 is required.");
        } else if self.year == 0 {
            err("year", "Year field on Page 1 Item 1 is required.");
        } else if self.year < 2018 || i32::from(self.year) > this_year {
            err(
                "year",
                "Year shall not be greater than the present year and not earlier than 2018.",
            );
        } else if self.date().is_none() {
            err(
                "day",
                "Please provide a valid date. (MM/DD/YYYY format) in Page 1 Item 1.",
            );
        } else if self.date().is_some_and(|date| date > today) {
            err("day", "Page 1 Item 1 Date cannot be a future date ");
        }
        if let Some(message) = tin_error(&self.tin, "Please check TIN numbers on Page 1 Item 5.") {
            err("tin", &message);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err(
                "rdo_code",
                "Please enter a valid RDO Code on Page 1 Item 6.",
            );
        }
        if self.taxpayer_name.trim().is_empty() {
            err("taxpayer_name", "Name field on Page 1 Item 7 is required.");
        }
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > 160 {
            err(
                "registered_address",
                "Registered Address field on Page 1 Item 8 is required.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Zip Code field on Page 1 Item 8A is required.");
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 45 || !digits_only(phone) {
            err(
                "contact_number",
                "Contact Number field on Page 1 Item 9 is required.",
            );
        }
        let email = self.email.trim();
        // validateAll only checks presence (checkMandatory); the page never
        // calls validateEmail, so there is no format alert to raise here.
        if email.is_empty() {
            err("email", "E-mail address on page 1 item 10 is required.");
        }
        if self.place.region.is_empty() {
            err(
                "place.region",
                "Region field on Page 1 Item 11 is required.",
            );
        } else if self.place.province.is_empty() {
            err(
                "place.province",
                "Province field on Page 1 Item 11 is required.",
            );
        } else if self.place.city.is_empty()
            || !is_official_place(
                PLACE_FORM,
                &self.place.region,
                &self.place.province,
                &self.place.city,
            )
        {
            err("place.city", "City field on Page 1 Item 11 is required.");
        }
        match self.tax_relief {
            None => err(
                "tax_relief",
                "Availing of Tax Relief field on Page 1 Item 12 is required.",
            ),
            Some(true) if self.tax_relief_specify.trim().is_empty() => err(
                "tax_relief_specify",
                "Specify Tax Relief field on Page 1 Item 12A is required.",
            ),
            _ => {}
        }
        if self.manner == Form2200CManner::Unanswered {
            err("manner", "Manner of Payment on Page 1 Part II is required.");
        }
        if self.manner == Form2200CManner::Other && self.manner_other.trim().is_empty() {
            err(
                "manner_other",
                "Specfy Manner of Payment field on Page 1 Part II Item 15 is required.",
            );
        }
        // checkPartVFields: counts and amounts go together, all three counts.
        for (index, row) in self.schedule.iter().enumerate() {
            let any_count_zero = row.counts().contains(&0.0);
            let any_count = row.counts().iter().any(|c| *c != 0.0);
            if (any_count_zero && row.total_billed != 0.0) || (any_count && row.total_billed == 0.0)
            {
                err(
                    &format!("schedule[{index}]"),
                    &format!("Please complete row #{} in Page 2 Part V.", index + 1),
                );
                break;
            }
        }

        // ── Typing limits (digits only) ──
        if self.schedule.len() > FORM_2200C_ROWS {
            err("schedule", "Part V Schedule 1 has ten rows.");
        }
        let inputs = self
            .schedule
            .iter()
            .flat_map(|row| row.counts().into_iter().chain(row.amounts()))
            .chain([
                self.balance_carried_over,
                self.creditable_excise_tax,
                self.previous_payment,
                self.surcharge,
                self.interest,
                self.compromise,
                self.tax_payment,
            ]);
        for value in inputs {
            if !whole_pesos(value) {
                err(
                    "amounts",
                    "Enter whole numbers only (the official fields take digits only).",
                );
                break;
            }
        }
        if self.tax_relief_specify.chars().count() > 50 {
            err("tax_relief_specify", "Item 12A takes up to 50 characters.");
        }
        if self.manner_other.chars().count() > 80 {
            err("manner_other", "Item 15 takes up to 80 characters.");
        }
        let mut expected = self.clone();
        expected.recompute();
        if expected != *self {
            err(
                "balance_carried_forward",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }
}

impl Form2200CDraft {
    /// Items 21A-21C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "21A Surcharge",
                interest: "21B Interest",
                compromise: "21C Compromise",
            ],
            controls: [
                "frm2200C:txtPg1P3I21ASurcharge",
                "frm2200C:txtPg1P3I21BInterest",
                "frm2200C:txtPg1P3I21CCompromise",
            ],
            state: penalty,
            basic_tax_due: "Item 20", |d| d.tax_still_due,
            // Decision D8 (2026-10-18): a monthly return, due within ten days
            // after the close of Item 1's month, so the penalties run from
            // the 10th of the month after it, moved to the next working day
            // on the app's deadline calendar (`penalty_items::next_business_day`,
            // shared with 1800 and 1801 since maintainer decision (1800/1801
            // penalties run from the due date)).
            typed_due_date: Some(|d: &Self| {
                if !(1..=12).contains(&d.month) {
                    return None;
                }
                let (year, month) = if d.month == 12 {
                    (i32::from(d.year).checked_add(1)?, 1)
                } else {
                    (i32::from(d.year), u32::from(d.month) + 1)
                };
                let tenth = chrono::NaiveDate::from_ymd_opt(year, month, 10)?;
                Some(super::penalty_items::next_business_day(tenth))
            }),
            // The typed amount fields take digits only.
            rounding: super::penalty_items::PenaltyRounding::WholePesos,
        )
    }
}

impl QueueableForm for Form2200CDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2200C";
    /// Official `formType` and PROD SFTP folder (`ftpTargetFolder.PROD['2200Cv2018']`).
    const FORM_TYPE: &'static str = "2200Cv2018";
    const LAYOUT_ID: &'static str = FORM_2200C_FORM_ID;

    /// Item 24 Amount Payable.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.amount_payable)
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
    /// One return per Item 1 date: `MMDD` keys the draft in its year.
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::OpenEnded(u32::from(self.month) * 100 + u32::from(self.day))
    }
    /// `month + day + year`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}{:02}{}", self.month, self.day, self.year)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 8 || !digits_only(code) {
            return None;
        }
        let month: u32 = code[..2].parse().ok()?;
        let day: u32 = code[2..4].parse().ok()?;
        let year: u16 = code[4..].parse().ok()?;
        chrono::NaiveDate::from_ymd_opt(i32::from(year), month, day)?;
        Some((year, FilingPeriod::OpenEnded(month * 100 + day)))
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

    pub(crate) fn sample() -> Form2200CDraft {
        let mut draft = Form2200CDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            month: 6,
            day: 30,
            year: 2025,
            is_amended: false,
            tin: "12345678800000".into(),
            rdo_code: "039".into(),
            taxpayer_name: "Sample Clinic Inc".into(),
            registered_address: "123 Sample St Quezon City".into(),
            zip_code: "1100".into(),
            contact_number: "0281234567".into(),
            email: "sample.taxpayer@example.com".into(),
            line_of_business: "Clinic".into(),
            place: ExcisePlace {
                region: "130000000".into(),
                province: "137400000".into(),
                city: "137404000".into(),
            },
            tax_relief: Some(false),
            tax_relief_specify: String::new(),
            manner: Form2200CManner::ActualRemoval,
            manner_other: String::new(),
            schedule: vec![Form2200CRow {
                exempt_procedures: 1.0,
                excisable_procedures: 3.0,
                non_excisable_procedures: 2.0,
                net_of_vat: 10_000.0,
                excisable_net: 50_010.0,
                excisable_vat_exempt: 0.0,
                non_excisable: 5_000.0,
                ..Default::default()
            }],
            excise_total: 0.0,
            excise_tax_due: 0.0,
            balance_carried_over: 0.0,
            creditable_excise_tax: 0.0,
            total_credits: 0.0,
            net_tax_due: 0.0,
            previous_payment: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            amount_payable: 0.0,
            tax_payment: 0.0,
            penalties_paid: 0.0,
            total_payment: 0.0,
            balance_carried_forward: 0.0,
            number_of_attached_sheets: 0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2200CDraft) -> Vec<String> {
        <Form2200CDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn to_fixed0_rounds_like_javascript() {
        assert_eq!(js_to_fixed0(2500.5), 2501.0);
        assert_eq!(js_to_fixed0(-2.5), -3.0);
        assert_eq!(js_to_fixed0(1010.0 * 0.05), 51.0); // 50.50000000000001
        assert_eq!(js_to_fixed0(0.49999999999999994), 0.0);
    }

    #[test]
    fn part_five_and_part_three_follow_the_official_chain() {
        let draft = sample();
        // 50,010 × 5% = 2,500.5 → toFixed(0) → 2,501
        assert_eq!(draft.schedule[0].excise_tax, 2_501.0);
        // I = (10,000 + 50,010 + 5,000 + 2,501) x 12% = 8,101.32 -> 8,101
        assert_eq!(draft.schedule[0].vat, 8_101.0);
        assert_eq!(draft.schedule[0].total_billed, 75_612.0);
        assert_eq!(draft.excise_total, 2_501.0);
        assert_eq!(draft.excise_tax_due, 2_501.0);
        assert_eq!(draft.amount_payable, 2_501.0);
        assert_eq!(draft.balance_carried_forward, 2_501.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm2200C:CPPexcise1"], "3.00");
        assert_eq!(fields["frm2200C:CPPexmpt2"], "0.00");
        assert_eq!(fields["frm2200C:ACvat1"], "8,101.00");
        assert_eq!(
            fields["frm2200C:txtPg1Pt1I7RegisteredName"],
            "SAMPLE CLINIC INC"
        );
        assert_eq!(
            fields["frm2200C:txtPg1Pt1I8RegisteredAddress"],
            "123 Sample St Quezon City"
        );
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2200Cv2018-06302025#sample.taxpayer@example.com#.xml"
        );
    }

    #[test]
    fn prepayment_resets_part_five() {
        let mut draft = sample();
        draft.manner = Form2200CManner::Prepayment;
        draft.surcharge = 100.0;
        draft.keep_entered_penalties();
        draft.tax_payment = 100.0;
        draft.recompute();
        assert!(draft.schedule.is_empty());
        assert_eq!(draft.excise_tax_due, 0.0);
        assert_eq!(draft.penalties_paid, 100.0);
        assert_eq!(draft.balance_carried_forward, -100.0);
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "06302025");
        assert_eq!(
            Form2200CDraft::parse_period_code("06302025"),
            Some((2025, FilingPeriod::OpenEnded(630)))
        );
        assert_eq!(Form2200CDraft::parse_period_code("13012025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2200CDraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            draft.recompute();
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.month = 0,
            "Month field on Page 1 Item 1 is required.",
        );
        check(&|d| d.day = 0, "Day field on Page 1 Item 1 is required.");
        check(&|d| d.year = 0, "Year field on Page 1 Item 1 is required.");
        check(
            &|d| d.year = 2017,
            "Year shall not be greater than the present year and not earlier than 2018.",
        );
        check(
            &|d| d.day = 31,
            "Please provide a valid date. (MM/DD/YYYY format) in Page 1 Item 1.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Page 1 Item 6.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Name field on Page 1 Item 7 is required.",
        );
        check(
            &|d| d.registered_address.clear(),
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
            &|d| d.email.clear(),
            "E-mail address on page 1 item 10 is required.",
        );
        check(
            &|d| d.place = ExcisePlace::default(),
            "Region field on Page 1 Item 11 is required.",
        );
        check(
            &|d| d.place.city.clear(),
            "City field on Page 1 Item 11 is required.",
        );
        check(
            &|d| d.tax_relief = None,
            "Availing of Tax Relief field on Page 1 Item 12 is required.",
        );
        check(
            &|d| d.tax_relief = Some(true),
            "Specify Tax Relief field on Page 1 Item 12A is required.",
        );
        check(
            &|d| d.manner = Form2200CManner::Unanswered,
            "Manner of Payment on Page 1 Part II is required.",
        );
        check(
            &|d| d.manner = Form2200CManner::Other,
            "Specfy Manner of Payment field on Page 1 Part II Item 15 is required.",
        );
        check(
            &|d| d.schedule[0].exempt_procedures = 0.0,
            "Please complete row #1 in Page 2 Part V.",
        );
    }

    #[test]
    fn print_map_adds_only_the_shared_comb_lines() {
        let draft = sample();
        let submit = draft.to_bir_field_map();
        let print = draft.to_print_field_map();
        for (key, value) in &submit {
            assert_eq!(print.get(key), Some(value), "{key}");
        }
        let extra: Vec<&String> = print.keys().filter(|k| !submit.contains_key(*k)).collect();
        assert!(extra.iter().all(|k| k.starts_with("derived:")));
        let line = &print["derived:contact_email"];
        assert!(line.starts_with(draft.contact_number.trim()));
        assert_eq!(&line[13..], draft.email.trim());
    }

    #[test]
    fn email_is_checked_for_presence_only_like_validate_all() {
        let mut draft = sample();
        draft.email = "abc".into();
        assert!(
            FormValidator::validate(&draft)
                .iter()
                .all(|(key, _)| key != "email")
        );
    }

    #[test]
    fn new_for_month_starts_in_the_tiles_month() {
        let profile = crate::filing_queue::mandatory_lab_1601c_profile();
        let draft = Form2200CDraft::new_for_month(&profile, 2019, 2);
        assert_eq!((draft.month, draft.day, draft.year), (2, 28, 2019));
        let draft = Form2200CDraft::new_for_month(&profile, 2019, 12);
        assert_eq!((draft.month, draft.day), (12, 31));
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

    /// save-009 (BIR-Form2200Cv2018.hta saveXML L8860/L8887).
    #[test]
    fn save_009_refuses_amended_no_once_an_original_is_filed() {
        use crate::db::{Database, DbError};
        use crate::filing_queue::QueueAuthSource;
        use crate::forms::queueable::AMENDED_RETURN_ALERT;

        let db = Database::open_in_memory_for_tests().unwrap();
        let d = sample();
        db.conn
            .execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, ?2, ?3, 9001, ?4, 'Submitted', '{}')",
                rusqlite::params![
                    d.tin,
                    Form2200CDraft::FORM_CODE,
                    i64::from(d.year),
                    d.filing_period().to_period_key()
                ],
            )
            .unwrap();
        let submit = |d: &Form2200CDraft| {
            let mut d = d.clone();
            d.id = Some(db.save_queueable_draft(&d).unwrap());
            d.queue(QueueAuthSource::Gui).unwrap();
            db.save_queued_queueable(&d)
        };
        let error = submit(&d).unwrap_err();
        assert!(matches!(error, DbError::AmendedReturnRequired));
        assert_eq!(error.to_string(), AMENDED_RETURN_ALERT);

        let mut amended = d.clone();
        assert!(amended.mark_amended());
        assert_eq!(amended.amended_return_answer(), Some(true));
        assert!(submit(&amended).is_ok());
    }

    /// Review of the excise typed date, and the maintainer decision (1800/1801
    /// penalties run from the due date) sharing the calendar: the 10th moves
    /// past a holiday on the calendar it is given, and the app's deadline
    /// calendar (weekends and the seeded official holidays) moves every 10th
    /// of 2000 to 2100 exactly as the weekends-only calendar 2200C used
    /// before (no seeded holiday is a 10th or the weekday after a weekend
    /// 10th), so 2200C's due dates are unchanged.
    #[test]
    fn the_due_date_moves_past_a_holiday_on_the_10th() {
        use crate::calendar_rules::{BusinessDayCalendar, NonWorkingDay};
        let tenth = chrono::NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        assert_eq!(tenth.format("%A").to_string(), "Tuesday");
        let holiday =
            BusinessDayCalendar::with_non_working_days(vec![NonWorkingDay::regular_holiday(
                tenth,
                "Sample holiday",
            )]);
        assert_eq!(
            holiday.adjust_to_next_business_day(tenth).0,
            chrono::NaiveDate::from_ymd_opt(2026, 3, 11).unwrap()
        );
        let weekends = BusinessDayCalendar::default();
        for year in 2000..=2100 {
            for month in 1..=12 {
                let tenth = chrono::NaiveDate::from_ymd_opt(year, month, 10).unwrap();
                assert_eq!(
                    crate::forms::penalty_items::next_business_day(tenth),
                    weekends.adjust_to_next_business_day(tenth).0,
                    "{tenth}"
                );
            }
        }
    }
}
