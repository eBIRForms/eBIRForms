//! Form 1701Q (January 2018) on the generic submission path.
//!
//! Ported from the official `BIR-Form1701Qv2018.hta` (eBIRForms 7.9.6.2.1):
//! the compute chain (`computetxt38` … `computetxt68`, `computePartIII`,
//! `computetxt31`), the enable/clear rules of `processTaxType`,
//! `processATC`, `enableSchedule1/2`, `ItemizedDeduct`, `OptionalDeduct`,
//! `item52Validate`, `validate()` / `initialValidateBeforeSave()` with their
//! exact alert texts, and `saveXMLsubmit` through [`crate::official_xml`].
//!
//! The page builds both RDO dropdowns at load (`getRdo`), but the generated
//! layout only knows the taxpayer's. [`QueueableForm::official_payload`]
//! writes the spouse dropdown (`txtSpouseRDOCode`) where the page has it,
//! right after `txtSpouseBranchCode`.

use std::collections::BTreeMap;

use super::form_1701q::{
    Form1701QAtc, Form1701QDeductionMethod, Form1701QDraft, Form1701QFilerType, Form1701QParty,
    Form1701QSpouseType, Form1701QTaxRate,
};
use super::penalty_items::{PenaltyItems, PenaltyRounding};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};

/// Rule-package id of the official layout.
pub const FORM_1701Q_LAYOUT_ID: &str = "1701q-v2018";
/// Official `formType` and PROD SFTP folder (`ftpTargetFolder.PROD['1701Qv2018']`).
pub const FORM_1701Q_FORM_TYPE: &str = "1701Qv2018";

const P: &str = "frm1701q:";

/// Items the filer types for each party (Parts V schedules and credits).
const INPUT_ITEMS: [u8; 19] = [
    36, 37, 39, 42, 43, 44, 47, 48, 50, 52, 55, 56, 57, 58, 59, 60, 61, 64, 65,
];
/// Item 66 completes the inputs; split out only to keep the array readable.
const LAST_INPUT_ITEM: u8 = 66;
/// Items 42A/B and 50A/B take `numbersWithNegative` (losses, decimals); every
/// other input is `wholenumber` (digits only).
const SIGNED_ITEMS: [u8; 2] = [42, 50];

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// JavaScript `x.toFixed(2)`, read back as a number. JavaScript picks the
/// larger candidate on an exact tie, where Rust formatting picks the even one.
fn to_fixed2(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let scaled = value * 100.0;
    if (scaled - scaled.trunc()).abs() == 0.5 && scaled / 100.0 == value {
        return scaled.ceil() / 100.0;
    }
    format!("{value:.2}").parse().unwrap_or(0.0)
}

/// JavaScript `Math.round`: halves go toward +∞.
fn js_round(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    (value + 0.5).floor()
}

/// `Math.round((x).toFixed(2))`, the pattern of most derived items.
fn jsr(value: f64) -> f64 {
    js_round(to_fixed2(value))
}

/// `computetxt46`: the table the page applies for `txtYear`.
pub fn official_tax_due(taxable_year: u16, income: f64) -> f64 {
    let year = taxable_year;
    let tax = if (2018..=2022).contains(&year) {
        if income <= 250_000.0 {
            0.0
        } else if income <= 400_000.0 {
            (income - 250_000.0) * (20.0 / 100.0)
        } else if income <= 800_000.0 {
            (income - 400_000.0) * (25.0 / 100.0) + 30_000.0
        } else if income <= 2_000_000.0 {
            (income - 800_000.0) * (30.0 / 100.0) + 130_000.0
        } else if income <= 8_000_000.0 {
            (income - 2_000_000.0) * (32.0 / 100.0) + 490_000.0
        } else {
            (income - 8_000_000.0) * (35.0 / 100.0) + 2_410_000.0
        }
    } else if year > 2022 {
        if income <= 250_000.0 {
            0.0
        } else if income <= 400_000.0 {
            (income - 250_000.0) * (15.0 / 100.0)
        } else if income <= 800_000.0 {
            (income - 400_000.0) * (20.0 / 100.0) + 22_500.0
        } else if income <= 2_000_000.0 {
            (income - 800_000.0) * (25.0 / 100.0) + 102_500.0
        } else if income <= 8_000_000.0 {
            (income - 2_000_000.0) * (30.0 / 100.0) + 402_500.0
        } else {
            (income - 8_000_000.0) * (35.0 / 100.0) + 2_202_500.0
        }
    } else if income <= 0.0 {
        0.0
    } else if income < 10_000.0 {
        income * (5.0 / 100.0)
    } else if income < 30_000.0 {
        (income - 10_000.0) * (10.0 / 100.0) + 500.0
    } else if income < 70_000.0 {
        (income - 30_000.0) * (15.0 / 100.0) + 2_500.0
    } else if income < 140_000.0 {
        (income - 70_000.0) * (20.0 / 100.0) + 8_500.0
    } else if income < 250_000.0 {
        (income - 140_000.0) * (25.0 / 100.0) + 22_500.0
    } else if income < 500_000.0 {
        (income - 250_000.0) * (30.0 / 100.0) + 50_000.0
    } else {
        (income - 500_000.0) * (32.0 / 100.0) + 125_000.0
    };
    jsr(tax)
}

fn capital(value: &str) -> String {
    value.trim().to_uppercase()
}

fn digits(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

fn split_birth_date(value: &str) -> (String, String, String) {
    let mut parts = value.trim().splitn(3, '/');
    (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    )
}

/// `validateMonthDayYearDate`: true when the MM/DD/YYYY text is invalid.
fn birth_date_is_invalid(month: &str, day: &str, year: &str) -> bool {
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !numeric(month) || !numeric(day) || !numeric(year) {
        return true;
    }
    if month.len() != 2 || day.len() != 2 || year.len() != 4 {
        return true;
    }
    let (m, d, y): (u32, u32, i32) = (
        month.parse().unwrap_or(0),
        day.parse().unwrap_or(0),
        year.parse().unwrap_or(0),
    );
    chrono::NaiveDate::from_ymd_opt(y, m, d).is_none()
}

/// What the page leaves enabled for one party.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Regime {
    /// No income tax computation (no spouse, spouse compensation earner, or
    /// nothing chosen yet): every amount stays 0.00.
    None,
    Graduated(Option<Form1701QDeductionMethod>),
    EightPercent,
}

impl Form1701QDraft {
    fn regime(&self, party: Form1701QParty) -> Regime {
        let (rate, method) = match party {
            Form1701QParty::Taxpayer => (self.tax_rate, self.deduction_method),
            Form1701QParty::Spouse if self.spouse_active() => {
                (self.spouse_tax_rate, self.spouse_deduction_method)
            }
            Form1701QParty::Spouse => (None, None),
        };
        match rate {
            Some(Form1701QTaxRate::Graduated) => Regime::Graduated(method),
            Some(Form1701QTaxRate::EightPercent) => Regime::EightPercent,
            None => Regime::None,
        }
    }

    /// Estates and trusts cannot report a spouse (`disableSpouse`).
    fn spouse_allowed(&self) -> bool {
        !matches!(
            self.filer_type,
            Some(Form1701QFilerType::Estate | Form1701QFilerType::Trust)
        )
    }

    fn spouse_active(&self) -> bool {
        self.has_spouse && self.spouse_allowed()
    }

    fn input(&self, item: u8, party: Form1701QParty) -> f64 {
        self.amount(item, party)
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    }

    /// The official compute chain plus the clear/enable rules of the radios.
    pub(super) fn official_recompute(&mut self) {
        // processTaxType / processATC: estates and trusts file II012 at
        // graduated rates; the ATC fixes the rate; 8% has no deduction method.
        if !self.spouse_allowed() {
            self.atc = Some(Form1701QAtc::Ii012);
        }
        if let Some(rate) = self.atc.and_then(Form1701QAtc::tax_rate) {
            self.tax_rate = Some(rate);
        }
        if self.tax_rate != Some(Form1701QTaxRate::Graduated) {
            self.deduction_method = None;
        }
        if self.spouse_type == Some(Form1701QSpouseType::CompensationEarner) {
            self.spouse_atc = Some(Form1701QAtc::Ii011);
        }
        match self.spouse_atc.map(|atc| (atc, atc.tax_rate())) {
            Some((_, Some(rate))) => self.spouse_tax_rate = Some(rate),
            Some((Form1701QAtc::Ii011, None)) => self.spouse_tax_rate = None,
            _ => {}
        }
        if self.spouse_tax_rate != Some(Form1701QTaxRate::Graduated) {
            self.spouse_deduction_method = None;
        }

        for party in [Form1701QParty::Taxpayer, Form1701QParty::Spouse] {
            self.recompute_official_party(party);
        }
        // Items 64-66 file their override, else the computed value (the
        // shared penalty model, from Item 63's tax still due), and Items
        // 67-68 and Part III add up what is filed.
        // Until PR 5's calendar fix the app resolved the third quarter's
        // deadline as October 15; the January 2018 guidelines give November
        // 15 (calendar_rules.rs). A Draft keeps the deadline it was opened
        // with only as a cache, so an October one stored for the third
        // quarter is dropped and the calendar resolves it again; a return
        // that left Draft keeps what it was reviewed with.
        if self.quarter == 3 && self.lifecycle.is_editable() {
            use chrono::Datelike;
            for state in [&mut self.penalty.state, &mut self.spouse_penalty.state] {
                if state.penalty_due_date.is_some_and(|due| {
                    due.year() == i32::from(self.taxable_year) && due.month() == 10
                }) {
                    state.penalty_due_date = None;
                }
            }
        }
        for items in Self::penalty_declarations() {
            // A column not on the return (the spouse's, while there is no
            // spouse or a compensation earner only) keeps no entry, as its
            // inputs hold 0.00 (`recompute_official_party`).
            if !(items.applies)(self) {
                let state = (items.state_mut)(self);
                state.clear_overrides();
                state.computed_penalties = None;
                continue;
            }
            // Until PR 5 the 1701Q penalties were typed boxes the model never
            // computed. A draft stored then, or an imported save file (no
            // penalty state: version 0), files every amount it holds, a zero
            // too, and is left exactly as stored. Once the filer types one of
            // the column's amounts (Edit penalties writes its override), the
            // others become overrides at the amounts they held and the
            // column joins the model; "Use the computed amounts" then brings
            // the computed values back.
            if (items.state)(self).penalty_model_version
                < super::penalty_items::PENALTY_MODEL_VERSION
            {
                use crate::penalties::PenaltyComponent;
                let typed = PenaltyComponent::ALL.into_iter().any(|component| {
                    (items.state)(self)
                        .penalty_overrides
                        .get(component)
                        .is_some()
                });
                if !typed {
                    continue;
                }
                let filed = items.filed(self);
                for (component, value) in PenaltyComponent::ALL.into_iter().zip(filed) {
                    if (items.state)(self)
                        .penalty_overrides
                        .get(component)
                        .is_none()
                    {
                        items.set_override(self, component, Some(value));
                    }
                }
                (items.state_mut)(self).penalty_model_version =
                    super::penalty_items::PENALTY_MODEL_VERSION;
            }
            items.apply(self);
        }
        for party in [Form1701QParty::Taxpayer, Form1701QParty::Spouse] {
            self.recompute_official_party(party);
        }
        // computetxt31
        let item_31 = to_fixed2(
            self.input(30, Form1701QParty::Taxpayer) + self.input(30, Form1701QParty::Spouse),
        );
        self.item_31_aggregate_amount_payable = Some(item_31);
        self.total_tax_due =
            self.input(26, Form1701QParty::Taxpayer) + self.input(26, Form1701QParty::Spouse);
        self.total_amount_payable = item_31;
    }

    fn recompute_official_party(&mut self, party: Form1701QParty) {
        let regime = self.regime(party);
        let spouse_off = party == Form1701QParty::Spouse
            && (!self.spouse_active() || self.spouse_atc == Some(Form1701QAtc::Ii011));
        let amended = self.is_amended;
        let set = |draft: &mut Self, item: u8, value: f64| {
            draft.set_amount(item, party, Some(value));
        };
        // Inputs hold round(this,2); disabled inputs hold 0.00.
        for item in INPUT_ITEMS.iter().copied().chain([LAST_INPUT_ITEM]) {
            let enabled = !spouse_off
                && match item {
                    36 | 42 | 43 | 44 => matches!(regime, Regime::Graduated(_)),
                    37 | 39 => matches!(
                        regime,
                        Regime::Graduated(Some(Form1701QDeductionMethod::Itemized))
                    ),
                    47 | 48 | 50 => regime == Regime::EightPercent,
                    52 => {
                        regime == Regime::EightPercent
                            && match party {
                                Form1701QParty::Taxpayer => self.atc != Some(Form1701QAtc::Ii016),
                                Form1701QParty::Spouse => {
                                    self.spouse_atc != Some(Form1701QAtc::Ii016)
                                }
                            }
                    }
                    59 => amended,
                    _ => true,
                };
            let value = if enabled {
                cents(self.input(item, party))
            } else {
                0.0
            };
            set(self, item, value);
        }
        let v = |draft: &Self, item: u8| draft.input(item, party);

        // Schedule 1 (graduated).
        let (i38, i40, i41, i45, i46) = if let Regime::Graduated(method) = regime {
            let i38 = jsr(v(self, 36) - v(self, 37));
            let i40 = if method == Some(Form1701QDeductionMethod::Osd) {
                js_round(v(self, 36) * 40.0 / 100.0)
            } else {
                0.0
            };
            let i41 = if method == Some(Form1701QDeductionMethod::Itemized) {
                jsr(i38 - v(self, 39))
            } else {
                jsr(i38 - i40)
            };
            let i45 = js_round(i41 + v(self, 42) + v(self, 43) + v(self, 44));
            (i38, i40, i41, i45, official_tax_due(self.taxable_year, i45))
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        };
        set(self, 38, i38);
        set(self, 40, i40);
        set(self, 41, i41);
        set(self, 45, i45);
        set(self, 46, i46);

        // Schedule 2 (8%).
        let (i49, i51, i53, i54) = if regime == Regime::EightPercent {
            let i49 = jsr(v(self, 47) + v(self, 48));
            let i51 = jsr(i49 + v(self, 50));
            let i53 = jsr(i51 - v(self, 52));
            let i54 = jsr(i53 * 8.0 / 100.0).max(0.0);
            (i49, i51, i53, i54)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        set(self, 49, i49);
        set(self, 51, i51);
        set(self, 53, i53);
        set(self, 54, i54);

        // Credits, penalties and Part III.
        let i62 = jsr((55..=61).fold(0.0, |total, item| total + v(self, item)));
        let tax_due = if matches!(regime, Regime::Graduated(_)) {
            i46
        } else {
            i54
        };
        let i63 = jsr(tax_due - i62);
        let i67 = jsr(v(self, 64) + v(self, 65) + v(self, LAST_INPUT_ITEM));
        let i68 = jsr(i63 + i67);
        set(self, 62, i62);
        set(self, 63, i63);
        set(self, 67, i67);
        set(self, 68, i68);
        let i26 = to_fixed2(tax_due);
        let i27 = to_fixed2(i62);
        let i28 = to_fixed2(i26 - i27);
        let i29 = to_fixed2(i67);
        set(self, 26, i26);
        set(self, 27, i27);
        set(self, 28, i28);
        set(self, 29, i29);
        set(self, 30, to_fixed2(i28 + i29));
    }

    /// `validate()` and `initialValidateBeforeSave()` in order with their
    /// alert texts, plus the limits the page enforces while typing.
    pub(super) fn official_errors(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let this_year = chrono::Datelike::year(&chrono::Local::now().date_naive());

        // Item 1
        if self.taxable_year == 0 {
            err("taxable_year", "Please enter a valid year in Item 1.");
        } else if i32::from(self.taxable_year) > this_year {
            err(
                "taxable_year",
                "Invalid date entry on Item no.1. Entry should not be later than Current Date.",
            );
        } else if self.taxable_year < 1900 {
            err(
                "taxable_year",
                "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
            );
        }
        // Item 2
        if !(1..=3).contains(&self.quarter) {
            err("quarter", "Please select quarter in Item 2.");
        }
        if self.number_of_sheets > 99 {
            err("number_of_sheets", "Item 4 holds at most two digits.");
        }
        // Item 5
        let tin = digits(&self.tin);
        let branch = tin.get(9..).unwrap_or("");
        if tin.len() < 12
            || tin.len() > 14
            || branch.len() < 3
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 5.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&tin[..9]) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        // Item 6
        let rdo = self.rdo_code.trim();
        if rdo.is_empty() || rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err("rdo_code", "Please enter a valid RDO Code on Item 6.");
        }
        // Item 9
        let name = self.taxpayer_name.trim();
        if name.is_empty() {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 9.",
            );
        }
        // Item 10
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 10.",
            );
        }
        // Item 11
        let (month, day, year) = split_birth_date(&self.date_of_birth);
        if month.is_empty() && day.is_empty() && year.is_empty() {
            err(
                "date_of_birth",
                "Please indicate Birth Date of Taxpayer on item 11.",
            );
        } else if birth_date_is_invalid(&month, &day, &year) {
            err(
                "date_of_birth",
                "Invalid birth date on item 11 of Taxpayer.  Please check date format.",
            );
        } else if year.parse::<i32>().unwrap_or(0) > this_year {
            err(
                "date_of_birth",
                "Birth Year on Item 11 should not be later than current year.",
            );
        }
        // Item 10A
        let zip = self.zip_code.trim();
        if zip.is_empty() {
            err("zip_code", "Please enter Zip Code on Item 10A.");
        } else if zip.len() > 4 || !zip.bytes().all(|b| b.is_ascii_digit()) {
            err("zip_code", "Item 10A holds a 4-digit ZIP code.");
        }

        // Spouse (Items 17-25A)
        if self.spouse_active() {
            let spouse_tin = digits(&self.spouse_tin);
            let spouse_branch = spouse_tin.get(9..).unwrap_or("");
            if spouse_tin.len() < 9
                || spouse_branch.len() > 5
                || !self
                    .spouse_tin
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '-')
            {
                err("spouse_tin", "Please enter a valid TIN number on Item 17.");
            } else if !crate::validation::relaxed_dev_mode()
                && crate::validation::official_tin_check_code(&spouse_tin[..9]) != 0
            {
                err(
                    "spouse_tin",
                    &format!(
                        "{} on Item 17.",
                        crate::validation::OFFICIAL_INVALID_TIN_MESSAGE
                    ),
                );
            }
            let spouse_rdo = self.spouse_rdo_code.trim();
            if spouse_rdo.is_empty()
                || spouse_rdo == "000"
                || !crate::validation::rdo_code_is_official_option(spouse_rdo)
            {
                err(
                    "spouse_rdo_code",
                    "Please enter a valid RDO Code on Item 18.",
                );
            }
            if self.spouse_name.trim().is_empty() {
                err("spouse_name", "Please enter Spouse Name on Item 21.");
            }
            if self.spouse_atc.is_none() {
                err("spouse_atc", "Please select an option for Item 20.");
            }
        }

        // Items 7, 8, 16, 16A
        if self.filer_type.is_none() {
            err("filer_type", "Please select an option for Item 7.");
        }
        if self.atc.is_none() || self.atc == Some(Form1701QAtc::Ii011) {
            err("atc", "Please select an option for Item 8.");
        }
        if self.tax_rate.is_none() {
            err("tax_rate", "Please select an option for Item 16.");
        }
        if self.tax_rate == Some(Form1701QTaxRate::Graduated) && self.deduction_method.is_none() {
            err("deduction_method", "Please select an option for Item 16A.");
        }
        // Items 19, 20, 25
        if self.spouse_active() {
            if self.spouse_type.is_none() {
                err("spouse_type", "Please select an option for Item 19.");
            }
            if self.spouse_type != Some(Form1701QSpouseType::CompensationEarner) {
                if self.spouse_tax_rate.is_none() {
                    err("spouse_tax_rate", "Please select an option for Item 25.");
                }
                if self.spouse_tax_rate == Some(Form1701QTaxRate::Graduated)
                    && self.spouse_deduction_method.is_none()
                {
                    // validate() does not check 25A; computing Items 40/41
                    // needs it, so be as strict as Item 16A.
                    err(
                        "spouse_deduction_method",
                        "Please select an option for Item 25A.",
                    );
                }
            }
        }

        // item52Validate (the page shows the 52A text for both columns).
        for party in [Form1701QParty::Taxpayer, Form1701QParty::Spouse] {
            if self.input(52, party) > 250_000.0 {
                err(
                    &format!("item_52_{}", party_name(party)),
                    "Item 52A cannot be more than P250,000.",
                );
            }
        }
        // Typing limits: wholenumber inputs take digits only; 42 and 50 take
        // a sign and centavos (numbersWithNegative).
        for party in [Form1701QParty::Taxpayer, Form1701QParty::Spouse] {
            for item in INPUT_ITEMS.iter().copied().chain([LAST_INPUT_ITEM]) {
                let value = self.input(item, party);
                let ok = if SIGNED_ITEMS.contains(&item) {
                    ((value * 100.0) - (value * 100.0).round()).abs() < 1e-7
                } else {
                    value >= 0.0 && value.fract() == 0.0
                };
                if !ok {
                    err(
                        &format!("item_{item}_{}", party_name(party)),
                        &format!(
                            "Item {item}{} takes whole pesos only.",
                            if party == Form1701QParty::Taxpayer {
                                "A"
                            } else {
                                "B"
                            }
                        ),
                    );
                }
            }
        }
        if !self.is_amended
            && (self.input(59, Form1701QParty::Taxpayer) != 0.0
                || self.input(59, Form1701QParty::Spouse) != 0.0)
        {
            err("item_59", "Item 59 applies only to an amended return.");
        }
        for (field, value, max) in [
            ("citizenship", &self.citizenship, 20),
            ("foreign_tax_number", &self.foreign_tax_number, 20),
            ("spouse_citizenship", &self.spouse_citizenship, 20),
            (
                "spouse_foreign_tax_number",
                &self.spouse_foreign_tax_number,
                20,
            ),
            ("spouse_name", &self.spouse_name, 50),
            (
                "item_43_description",
                &self.item_43_non_operating_income_description,
                25,
            ),
            (
                "item_48_description",
                &self.item_48_non_operating_income_description,
                25,
            ),
            (
                "item_61_description",
                &self.item_61_other_tax_credit_description,
                25,
            ),
        ] {
            if value.chars().count() > max {
                err(
                    field,
                    &format!("This entry holds at most {max} characters."),
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

        // Derived items must be what the official compute chain produces.
        let mut expected = self.clone();
        expected.official_recompute();
        if expected.amounts != self.amounts
            || expected.item_31_aggregate_amount_payable != self.item_31_aggregate_amount_payable
        {
            err("item_31", "Totals are out of date. Recompute the return.");
        }
        errors
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_official_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("{P}{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let money = |value: Option<f64>| official_amount(value.unwrap_or(0.0));

        put("txtYear", self.taxable_year.to_string());
        for quarter in 1..=3u8 {
            put(
                &format!("DateQuarter_{quarter}"),
                flag(self.quarter == quarter),
            );
        }
        put("AmendedRtn_1", flag(self.is_amended));
        put("AmendedRtn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_sheets.to_string());
        let tin = digits(&self.tin);
        let part =
            |text: &str, range: std::ops::Range<usize>| text.get(range).unwrap_or("").to_string();
        let branch = tin.get(9..).unwrap_or("").to_string();
        for prefix in ["txt", "txtPg2"] {
            put(&format!("{prefix}TIN1"), part(&tin, 0..3));
            put(&format!("{prefix}TIN2"), part(&tin, 3..6));
            put(&format!("{prefix}TIN3"), part(&tin, 6..9));
            put(&format!("{prefix}BranchCode"), branch.clone());
        }
        put("txtRDOCode", self.rdo_code.trim().to_string());
        put("txtCurrentPage", "1".to_string());
        for (index, filer) in [
            Form1701QFilerType::SingleProprietor,
            Form1701QFilerType::Professional,
            Form1701QFilerType::Estate,
            Form1701QFilerType::Trust,
        ]
        .into_iter()
        .enumerate()
        {
            put(
                &format!("optType_{}", index + 1),
                flag(self.filer_type == Some(filer)),
            );
        }
        for (index, atc) in Form1701QAtc::TAXPAYER_CHOICES.into_iter().enumerate() {
            put(
                &format!("optATC_{}", index + 1),
                flag(self.atc == Some(atc)),
            );
        }
        // capital() uppercases every text control except txtEmail.
        put("txtTaxpayerName", capital(&self.taxpayer_name));
        // txtAddress and txtAddress2 are written as one value.
        put(
            "txtAddress",
            capital(&format!(
                "{}{}",
                self.registered_address, self.registered_address_2
            )),
        );
        put("txtAddress2", String::new());
        put("txtZipCode", self.zip_code.trim().to_string());
        let (month, day, year) = split_birth_date(&self.date_of_birth);
        put("txtBirthMonth", month);
        put("txtBirthDay", day);
        put("txtBirthYear", year);
        put("txtCitizenship", capital(&self.citizenship));
        put("txtForeignTaxNumber", capital(&self.foreign_tax_number));
        put(
            "optForeignTaxCredits_1",
            flag(self.claims_foreign_tax_credits == Some(true)),
        );
        put(
            "optForeignTaxCredits_2",
            flag(self.claims_foreign_tax_credits == Some(false)),
        );
        put(
            "optTaxRate_1",
            flag(self.tax_rate == Some(Form1701QTaxRate::Graduated)),
        );
        put(
            "optMethodOfDeduction:_1",
            flag(self.deduction_method == Some(Form1701QDeductionMethod::Itemized)),
        );
        put(
            "optMethodOfDeduction:_2",
            flag(self.deduction_method == Some(Form1701QDeductionMethod::Osd)),
        );
        put(
            "optTaxRate_2",
            flag(self.tax_rate == Some(Form1701QTaxRate::EightPercent)),
        );

        let spouse = self.spouse_active();
        let spouse_tin = if spouse {
            digits(&self.spouse_tin)
        } else {
            String::new()
        };
        put("txtSpouseTIN1", part(&spouse_tin, 0..3));
        put("txtSpouseTIN2", part(&spouse_tin, 3..6));
        put("txtSpouseTIN3", part(&spouse_tin, 6..9));
        // disableSpouse (estates and trusts) leaves "00000" and a blank RDO.
        let (spouse_branch, spouse_rdo) = if spouse {
            (
                spouse_tin.get(9..).unwrap_or("").to_string(),
                self.spouse_rdo_code.trim().to_string(),
            )
        } else if self.spouse_allowed() {
            (String::new(), "000".to_string())
        } else {
            ("00000".to_string(), String::new())
        };
        put("txtSpouseBranchCode", spouse_branch);
        put("txtSpouseRDOCode", spouse_rdo);
        for (index, kind) in Form1701QSpouseType::ALL.into_iter().enumerate() {
            put(
                &format!("optSpouseType_{}", index + 1),
                flag(spouse && self.spouse_type == Some(kind)),
            );
        }
        for (index, atc) in Form1701QAtc::SPOUSE_CHOICES.into_iter().enumerate() {
            put(
                &format!("optSpouseATC_{}", index + 1),
                flag(spouse && self.spouse_atc == Some(atc)),
            );
        }
        let text = |value: &str| {
            if spouse {
                capital(value)
            } else {
                String::new()
            }
        };
        put("txtSpouseName", text(&self.spouse_name));
        put("txtSpouseCitizenship", text(&self.spouse_citizenship));
        put(
            "txtSpouseForeignTaxNum",
            text(&self.spouse_foreign_tax_number),
        );
        put(
            "optSpouseForeignTaxCred_1",
            flag(spouse && self.spouse_claims_foreign_tax_credits == Some(true)),
        );
        put(
            "optSpouseForeignTaxCred_2",
            flag(spouse && self.spouse_claims_foreign_tax_credits == Some(false)),
        );
        put(
            "optSpouseTaxRate_1",
            flag(spouse && self.spouse_tax_rate == Some(Form1701QTaxRate::Graduated)),
        );
        put(
            "optSpouseMethod:_1",
            flag(
                spouse && self.spouse_deduction_method == Some(Form1701QDeductionMethod::Itemized),
            ),
        );
        put(
            "optSpouseMethod:_2",
            flag(spouse && self.spouse_deduction_method == Some(Form1701QDeductionMethod::Osd)),
        );
        put(
            "optSpouseTaxRate_2",
            flag(spouse && self.spouse_tax_rate == Some(Form1701QTaxRate::EightPercent)),
        );

        for item in (26..=30).chain(36..=68) {
            for (suffix, party) in [
                ("A", Form1701QParty::Taxpayer),
                ("B", Form1701QParty::Spouse),
            ] {
                put(
                    &format!("txt{item}{suffix}"),
                    money(self.amount(item, party)),
                );
            }
        }
        put("txt31", money(self.item_31_aggregate_amount_payable));
        put(
            "txt43Desc",
            capital(&self.item_43_non_operating_income_description),
        );
        put(
            "txt48Desc",
            capital(&self.item_48_non_operating_income_description),
        );
        put(
            "txt61Desc",
            capital(&self.item_61_other_tax_credit_description),
        );
        // loadBGData: page 2 shows the name up to its first comma.
        put(
            "txtPg2TaxpayerName",
            capital(self.taxpayer_name.split(',').next().unwrap_or("")),
        );
        put("txtLOB", capital(&self.line_of_business));
        put("txtTelno", capital(&self.contact_number));
        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_official_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

impl Form1701QDraft {
    /// Items 64-66 of each column on the shared penalty model
    /// ([`super::penalty_items`]): the taxpayer/filer's (column A), then the
    /// spouse's (column B), which apply only while the spouse's column is
    /// open (a married individual whose spouse is not a compensation earner
    /// only, `recompute_official_party`'s `spouse_off`). Amounts are whole
    /// pesos, as every 1701Q input (`wholenumber`).
    pub fn penalty_declarations() -> Vec<PenaltyItems<Self>> {
        use crate::penalties::PenaltyComponent as C;
        const fn item(component: C) -> u8 {
            match component {
                C::Surcharge => 64,
                C::Interest => 65,
                C::Compromise => 66,
            }
        }
        let common = PenaltyItems::<Self> {
            form_code: super::form_1701q::FORM_CODE,
            // The calendar's deadline, not a typed trigger date (PR 11's D8
            // is for the excise returns only).
            typed_due_date: None,
            party: Some("Taxpayer/Filer"),
            applies: |_| true,
            schedule: crate::penalties::PenaltyProfile::StandardFiling,
            // The validation's field names for these boxes (`item_64_taxpayer`,
            // "Item 64A takes whole pesos only."), so a message lands on its box.
            fields: ["item_64_taxpayer", "item_65_taxpayer", "item_66_taxpayer"],
            controls: ["frm1701q:txt64A", "frm1701q:txt65A", "frm1701q:txt66A"],
            labels: ["64 Surcharge", "65 Interest", "66 Compromise"],
            basic_tax_due_label: "Item 63A",
            rounding: PenaltyRounding::WholePesos,
            get: |d, component| d.input(item(component), Form1701QParty::Taxpayer),
            set: |d, component, value| {
                d.set_amount(item(component), Form1701QParty::Taxpayer, Some(value));
            },
            basic_tax_due: |d| d.input(63, Form1701QParty::Taxpayer),
            gross_sales: None,
            state: |d| &d.penalty.state,
            state_mut: |d| &mut d.penalty.state,
            status: |d| d.lifecycle.status.clone(),
            trigger_date_pending: |d| {
                d.lifecycle.trigger_date_model && !d.lifecycle.trigger_date_entered
            },
            deadline_period: super::penalty_items::calendar_period::<Self>,
            period_code: |d| QueueableForm::period_code(d),
            is_amended: |d| d.is_amended,
            recompute: |d| d.recompute(),
        };
        let spouse = PenaltyItems::<Self> {
            party: Some("Spouse"),
            fields: ["item_64_spouse", "item_65_spouse", "item_66_spouse"],
            applies: |d| d.spouse_active() && d.spouse_atc != Some(Form1701QAtc::Ii011),
            controls: ["frm1701q:txt64B", "frm1701q:txt65B", "frm1701q:txt66B"],
            basic_tax_due_label: "Item 63B",
            get: |d, component| d.input(item(component), Form1701QParty::Spouse),
            set: |d, component, value| {
                d.set_amount(item(component), Form1701QParty::Spouse, Some(value));
            },
            basic_tax_due: |d| d.input(63, Form1701QParty::Spouse),
            state: |d| &d.spouse_penalty.state,
            state_mut: |d| &mut d.spouse_penalty.state,
            ..common
        };
        vec![common, spouse]
    }
}

fn party_name(party: Form1701QParty) -> &'static str {
    match party {
        Form1701QParty::Taxpayer => "taxpayer",
        Form1701QParty::Spouse => "spouse",
    }
}

impl QueueableForm for Form1701QDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    const FORM_CODE: &'static str = "1701Q";
    const FORM_TYPE: &'static str = FORM_1701Q_FORM_TYPE;
    const LAYOUT_ID: &'static str = FORM_1701Q_LAYOUT_ID;

    /// Item 31 Aggregate Amount Payable/(Overpayment).
    fn amount_payable(&self) -> Option<f64> {
        self.item_31_aggregate_amount_payable
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
        (1..=3)
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
        self.to_official_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        Self::penalty_declarations()
    }
    /// A column still on the typed boxes (a draft stored before PR 5, or an
    /// imported save file: penalty_model_version 0) computes nothing, so its
    /// stored state is left exactly as it is; the others resolve their
    /// deadline and class from the profile as on every wired form.
    fn apply_penalty_profile(
        &mut self,
        profile: &crate::profile::TaxpayerProfile,
        global_overrides: &[crate::calendar_rules::DeadlineOverride],
    ) {
        for items in Self::penalty_items() {
            if (items.state)(self).penalty_model_version
                >= super::penalty_items::PENALTY_MODEL_VERSION
            {
                items.apply_profile(self, profile, global_overrides);
            }
        }
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
            "full_name": "Dela Cruz, Juan Sample",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Sample Consulting",
            "registered_address": "123 Sample Street, Quezon City",
            "zip_code": "1100",
            "phone": "09170000000",
            "email": "sample.taxpayer@example.com",
            "default_form_type": "1701Qv2018",
            "taxpayer_type": "Individual"
        }))
        .expect("dummy profile")
    }

    pub(crate) fn sample() -> Form1701QDraft {
        let mut draft = Form1701QDraft::new_from_profile(&profile(), 2025, 2);
        draft.filer_type = Some(Form1701QFilerType::Professional);
        draft.atc = Some(Form1701QAtc::Ii014);
        draft.deduction_method = Some(Form1701QDeductionMethod::Osd);
        draft.date_of_birth = "05/17/1985".into();
        draft.citizenship = "Filipino".into();
        draft.set_amount(36, Form1701QParty::Taxpayer, Some(1_234_567.0));
        draft.set_amount(42, Form1701QParty::Taxpayer, Some(-1_000.505));
        draft.set_amount(55, Form1701QParty::Taxpayer, Some(10_000.0));
        draft.set_amount(64, Form1701QParty::Taxpayer, Some(1_000.0));
        // An entered surcharge (Items 64-66 are on the shared penalty model
        // since PR 5): it files as typed.
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1701QDraft) -> Vec<String> {
        <Form1701QDraft as QueueableForm>::validate(draft)
            .into_iter()
            .map(|(_, m)| m)
            .collect()
    }

    #[test]
    fn javascript_rounding_helpers() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(to_fixed2(0.125), 0.13);
        assert_eq!(jsr(-0.5), 0.0);
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let d = sample();
        let a = |item| d.amount(item, Form1701QParty::Taxpayer).unwrap();
        assert_eq!(a(38), 1_234_567.0);
        assert_eq!(a(40), 493_827.0);
        assert_eq!(a(41), 740_740.0);
        assert_eq!(a(42), -1_000.51);
        assert_eq!(a(45), 739_739.0);
        assert_eq!(a(46), 90_448.0);
        assert_eq!(a(46), official_tax_due(2025, 739_739.0));
        assert_eq!(a(63), 80_448.0);
        assert_eq!(a(68), 81_448.0);
        assert_eq!(d.item_31_aggregate_amount_payable, Some(81_448.0));
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn field_map_and_filename_use_official_formats() {
        let d = sample();
        let f = d.to_official_field_map();
        assert_eq!(f["frm1701q:txt36A"], "1,234,567.00");
        assert_eq!(f["frm1701q:txtTaxpayerName"], "DELA CRUZ, JUAN SAMPLE");
        assert_eq!(f["frm1701q:txtPg2TaxpayerName"], "DELA CRUZ");
        assert_eq!(f["frm1701q:optATC_2"], "true");
        assert_eq!(f["frm1701q:txtSpouseRDOCode"], "000");
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1701Qv2018-2025Q2#sample.taxpayer@example.com#.xml"
        );
        let payload = d.to_official_xml_payload().unwrap();
        assert!(payload.contains(
            "frm1701q:txtSpouseBranchCode=</div>\t\n            <div>frm1701q:txtSpouseRDOCode=000frm1701q:txtSpouseRDOCode=</div>\t\n            <div>frm1701q:optSpouseType_1="
        ));
    }

    #[test]
    fn period_codes_round_trip() {
        assert_eq!(sample().period_code(), "2025Q2");
        assert_eq!(
            Form1701QDraft::parse_period_code("2025Q3"),
            Some((2025, FilingPeriod::Quarterly(3)))
        );
        assert_eq!(Form1701QDraft::parse_period_code("2025Q4"), None);
        assert_eq!(Form1701QDraft::parse_period_code("122025Q1"), None);
    }

    #[test]
    fn eight_percent_and_item_52() {
        let mut d = sample();
        d.atc = Some(Form1701QAtc::Ii017);
        d.set_amount(47, Form1701QParty::Taxpayer, Some(900_000.0));
        d.set_amount(52, Form1701QParty::Taxpayer, Some(250_000.0));
        d.recompute();
        let a = |item| d.amount(item, Form1701QParty::Taxpayer).unwrap();
        assert_eq!(d.tax_rate, Some(Form1701QTaxRate::EightPercent));
        assert_eq!(d.deduction_method, None);
        assert_eq!(a(36), 0.0);
        assert_eq!(a(53), 650_000.0);
        assert_eq!(a(54), 52_000.0);
        assert_eq!(a(26), 52_000.0);
        d.set_amount(52, Form1701QParty::Taxpayer, Some(260_000.0));
        d.recompute();
        assert!(
            messages(&d)
                .iter()
                .any(|m| m == "Item 52A cannot be more than P250,000.")
        );
        // II016 (mixed income) leaves 52A at zero.
        d.atc = Some(Form1701QAtc::Ii016);
        d.recompute();
        assert_eq!(d.amount(52, Form1701QParty::Taxpayer), Some(0.0));
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1701QDraft), expected: &str| {
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
            &|d| d.taxable_year = 2999,
            "Invalid date entry on Item no.1. Entry should not be later than Current Date.",
        );
        check(
            &|d| d.taxable_year = 1899,
            "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
        );
        check(&|d| d.quarter = 4, "Please select quarter in Item 2.");
        check(
            &|d| d.tin = "1234".into(),
            "Please enter a valid TIN number on Item 5.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 6.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 9.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 10.",
        );
        check(
            &|d| d.date_of_birth.clear(),
            "Please indicate Birth Date of Taxpayer on item 11.",
        );
        check(
            &|d| d.date_of_birth = "02/30/1985".into(),
            "Invalid birth date on item 11 of Taxpayer.  Please check date format.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Zip Code on Item 10A.",
        );
        check(
            &|d| d.filer_type = None,
            "Please select an option for Item 7.",
        );
        check(
            &|d| {
                d.atc = None;
                d.tax_rate = None;
            },
            "Please select an option for Item 8.",
        );
        check(
            &|d| {
                d.atc = None;
                d.tax_rate = None;
            },
            "Please select an option for Item 16.",
        );
        check(
            &|d| d.deduction_method = None,
            "Please select an option for Item 16A.",
        );
        let spouse = |d: &mut Form1701QDraft| {
            d.has_spouse = true;
            d.spouse_tin = "987-654-321-00000".into();
            d.spouse_rdo_code = "040".into();
            d.spouse_name = "Spouse".into();
            d.spouse_type = Some(Form1701QSpouseType::SingleProprietor);
            d.spouse_atc = Some(Form1701QAtc::Ii015);
        };
        check(
            &|d| {
                spouse(d);
                d.spouse_tin = "12".into();
            },
            "Please enter a valid TIN number on Item 17.",
        );
        check(
            &|d| {
                spouse(d);
                d.spouse_tin = "987-654-322-00000".into();
            },
            "You have entered an incorrect TIN on Item 17.",
        );
        check(
            &|d| {
                spouse(d);
                d.spouse_rdo_code.clear();
            },
            "Please enter a valid RDO Code on Item 18.",
        );
        check(
            &|d| {
                spouse(d);
                d.spouse_name.clear();
            },
            "Please enter Spouse Name on Item 21.",
        );
        check(
            &|d| {
                spouse(d);
                d.spouse_type = None;
            },
            "Please select an option for Item 19.",
        );
        check(
            &|d| {
                spouse(d);
                d.spouse_atc = None;
            },
            "Please select an option for Item 20.",
        );
        check(
            &|d| {
                spouse(d);
                d.spouse_atc = None;
                d.spouse_tax_rate = None;
            },
            "Please select an option for Item 25.",
        );
        check(
            &|d| d.set_amount(36, Form1701QParty::Taxpayer, Some(1.5)),
            "Item 36A takes whole pesos only.",
        );
        check(
            &|d| d.set_amount(36, Form1701QParty::Taxpayer, Some(5.0)),
            "Totals are out of date. Recompute the return.",
        );
    }

    #[test]
    fn spouse_compensation_earner_has_no_computation() {
        let mut d = sample();
        d.has_spouse = true;
        d.spouse_tin = "987-654-321-00000".into();
        d.spouse_rdo_code = "040".into();
        d.spouse_name = "Spouse".into();
        d.spouse_type = Some(Form1701QSpouseType::CompensationEarner);
        d.set_amount(55, Form1701QParty::Spouse, Some(1_000.0));
        d.recompute();
        assert_eq!(d.spouse_atc, Some(Form1701QAtc::Ii011));
        assert_eq!(d.amount(55, Form1701QParty::Spouse), Some(0.0));
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn old_stored_json_still_loads() {
        let mut json = serde_json::to_value(sample()).unwrap();
        let object = json.as_object_mut().unwrap();
        object.insert("status".into(), serde_json::json!("Draft"));
        object.insert(
            "created_at".into(),
            serde_json::json!("2025-04-01T00:00:00+00:00"),
        );
        object.insert(
            "updated_at".into(),
            serde_json::json!("2025-04-02T00:00:00+00:00"),
        );
        object.insert("submitted_at".into(), serde_json::Value::Null);
        object.insert("confirmed_at".into(), serde_json::Value::Null);
        object.insert("submission_filename".into(), serde_json::Value::Null);
        object.insert("receipt_id".into(), serde_json::Value::Null);
        object.insert("next_retry_at".into(), serde_json::Value::Null);
        object.insert("submission_attempts".into(), serde_json::json!(1));
        object.insert("last_error".into(), serde_json::json!("old"));
        let d: Form1701QDraft = serde_json::from_value(json).unwrap();
        assert_eq!(d.lifecycle.status, FilingStatus::Draft);
        assert_eq!(d.lifecycle.created_at, "2025-04-01T00:00:00+00:00");
        assert_eq!(d.lifecycle.submission_attempts, 1);
        assert_eq!(d.last_error.as_deref(), Some("old"));
        let back = serde_json::to_value(&d).unwrap();
        assert_eq!(back["status"], "Draft");
        assert!(back.get("lifecycle").is_none());
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut d = sample();
        d.queue(QueueAuthSource::Gui).unwrap();
        assert_eq!(d.lifecycle.status, FilingStatus::Queued);
        assert!(d.clone().revalidate_queued_before_submission().is_ok());
        d.set_amount(64, Form1701QParty::Taxpayer, Some(2_000.0));
        assert!(d.revalidate_queued_before_submission().is_err());
        assert_eq!(d.lifecycle.status, FilingStatus::Draft);
    }
}
