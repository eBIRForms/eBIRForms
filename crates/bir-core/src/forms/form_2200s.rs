//! BIR Form 2200-S (January 2018) — Excise Tax Return for Sweetened
//! Beverages.
//!
//! Ported from the official `BIR-Form2200S.hta` (eBIRForms 7.9.6.2.1):
//! Schedule 1 (`createStaticFieldForSched1` / `getBeverages`,
//! `computeBasicTaxDue`, the "Others" row `computeSched1BasicTaxDue`,
//! `totalTaxDue`), Part III (`compute17C` … `compute24`, `compute21D`,
//! `payPenalties`), the manner of payment rules (`changeMannerOfPayment`,
//! `dateMonthYear`, `recomputePrepayment`), `validateForm` with its exact
//! alerts and `saveXMLsubmit` through [`crate::official_xml`].
//!
//! Amounts on this page are truncated, not rounded: `roundDownWithAlert`
//! cuts typed text to two decimals and `roundDownComputation` cuts
//! `toPrecision(14)` of each computed value.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::excise_places::{ExcisePlace, is_official_place};
use super::form_2000::{digits_only, split_tin, tin_error};
use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_2200S_FORM_ID: &str = "2200s-v2018";
const PLACE_FORM: &str = "2200S";

/// One fixed Schedule 1 beverage row (`getBeverages`, no `2200S` ATCs in
/// `atcCodes.xml`, so the page's built-in list applies).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Form2200SBeverage {
    pub atc: &'static str,
    pub description: &'static str,
    /// The hidden `hideProAppRate` value, pesos per liter.
    pub rate: f64,
}

pub const FORM_2200S_BEVERAGES: [Form2200SBeverage; 10] = [
    Form2200SBeverage {
        atc: "XB010",
        description: "a. Sweetened Juice Drinks",
        rate: 6.0,
    },
    Form2200SBeverage {
        atc: "XB020",
        description: "b. Sweetened Tea",
        rate: 6.0,
    },
    Form2200SBeverage {
        atc: "XB030",
        description: "c. Carbonated Beverages",
        rate: 6.0,
    },
    Form2200SBeverage {
        atc: "XB040",
        description: "d. Flavored Water",
        rate: 6.0,
    },
    Form2200SBeverage {
        atc: "XB050",
        description: "e. Energy and Sports Drinks",
        rate: 6.0,
    },
    Form2200SBeverage {
        atc: "XB060",
        description: "f. Powdered Drinks not classified as Milk, Juice, Tea and Coffee",
        rate: 6.0,
    },
    Form2200SBeverage {
        atc: "XB070",
        description: "g. Cereal and Grain Beverages",
        rate: 6.0,
    },
    Form2200SBeverage {
        atc: "XB080",
        description: "h. Other Non-Alcoholic Beverages that contain added Sugar",
        rate: 6.0,
    },
    Form2200SBeverage {
        atc: "XB090",
        description: "2. Using purely high fructose corn syrup or in combination with any caloric or non-caloric sweeteners",
        rate: 12.0,
    },
    Form2200SBeverage {
        atc: "XB100",
        description: "3. Using purely coconut sap sugar and purely Steviol Glycosides",
        rate: 0.0,
    },
];

// ── Truncating amount helpers (js/string-util2014.js) ──

/// JavaScript `Number#toString` for the amounts this page handles.
fn js_number_text(value: f64) -> String {
    if value == 0.0 {
        "0".to_string()
    } else {
        format!("{value}")
    }
}

/// JavaScript `Number#toPrecision(14)` read back as a number (ties to the
/// larger magnitude, like the spec's "pick the larger n").
fn js_to_precision14(value: f64) -> f64 {
    if value == 0.0 || !value.is_finite() {
        return value;
    }
    let exact = format!("{:.80e}", value.abs());
    let (mantissa, exponent) = exact.split_once('e').unwrap_or((&exact, "0"));
    let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    let mut kept: Vec<u8> = digits[..14].to_vec();
    let mut exponent: i32 = exponent.parse().unwrap_or(0);
    if digits[14] >= b'5' {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                kept.pop();
                exponent += 1;
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let text = format!(
        "{}.{}e{}",
        kept[0] as char,
        std::str::from_utf8(&kept[1..]).unwrap_or("0"),
        exponent
    );
    let magnitude: f64 = text.parse().unwrap_or(0.0);
    if value < 0.0 { -magnitude } else { magnitude }
}

/// `addCommas` on an integer text, sign kept.
fn add_commas(int: &str) -> String {
    let (sign, digits) = match int.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", int),
    };
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{sign}{out}")
}

/// The text a truncated amount shows (`1,234.50`, `-0.50`).
fn truncated_text(number_text: &str) -> String {
    match number_text.split_once('.') {
        Some((int, frac)) => {
            let int = if int == "-0" {
                "-0".to_string()
            } else {
                add_commas(int)
            };
            let cents = if frac.len() == 1 {
                format!("{frac}0")
            } else {
                frac[..2].to_string()
            };
            format!("{int}.{cents}")
        }
        None => format!("{}.00", add_commas(number_text)),
    }
}

/// `roundDownWithAlert` on a typed amount: its text cut to two decimals.
pub fn round_down_input_text(value: f64) -> String {
    truncated_text(&js_number_text(value))
}

/// `roundDownComputation` on a computed amount.
pub fn round_down_computation_text(value: f64) -> String {
    truncated_text(&js_number_text(js_to_precision14(value)))
}

fn parse_amount(text: &str) -> f64 {
    text.replace(',', "").parse().unwrap_or(0.0)
}

/// A typed amount as the field holds it after its blur.
/// The official alert when a Schedule 1 row's Basic Excise Tax Due passes
/// the field's 18 characters (BIR-Form2200S.hta `computeBasicTaxDue`).
pub const FORM_2200S_BASIC_TAX_LIMIT_ALERT: &str =
    "Exceeded maximum amount of 999,999,999,999.99 on Basic Excise Tax Due.";

/// `999,999,999,999.99`: anything longer than its 18 characters alerts.
const BASIC_TAX_DUE_MAX: f64 = 999_999_999_999.99;

fn typed(value: f64) -> f64 {
    parse_amount(&round_down_input_text(value))
}

/// A computed amount as `roundDownComputation` leaves it.
fn computed(value: f64) -> f64 {
    parse_amount(&round_down_computation_text(value))
}

// ── Model ──

fn last_day_of_month(year: u16, month: u8) -> u8 {
    use chrono::Datelike;
    let (next_year, next_month) = if month >= 12 {
        (i32::from(year) + 1, 1)
    } else {
        (i32::from(year), u32::from(month) + 1)
    };
    chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first| first.pred_opt())
        .map(|last| last.day() as u8)
        .unwrap_or(28)
}

/// Item 14 — tax relief under a Special Law or International Tax Treaty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2200STaxRelief {
    #[default]
    No,
    /// "Yes" with nothing picked in Item 14A yet.
    YesUnspecified,
    SpecialLaw,
    InternationalTaxTreaty,
}

impl Form2200STaxRelief {
    fn list_value(self) -> &'static str {
        match self {
            Self::No | Self::YesUnspecified => "0",
            Self::SpecialLaw => "1",
            Self::InternationalTaxTreaty => "2",
        }
    }
}

/// Part II — Manner of payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2200SManner {
    #[default]
    Unanswered,
    /// Payment on Actual Removal: Schedule 1 is required, penalties are off.
    ActualRemoval,
    /// Prepayment/Advance Deposit: Item 1 is the filing date, Item 18 is zero.
    Prepayment,
}

/// One Schedule 1 beverage row (the page's ten fixed rows).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200SBeverageRow {
    pub sales_value: f64,
    /// Liters.
    pub volume: f64,
    #[serde(default)]
    pub basic_tax_due: f64,
}

/// One Schedule 1 "Others (specify)" row (`frm2200SadditionalSched1`; the
/// page opens with one and `addFieldsForSched1` appends more).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200SOtherRow {
    /// `XB` + three digits, not one of the fixed rows' ATCs.
    pub atc: String,
    pub description: String,
    pub tax_bracket: String,
    pub rate: f64,
    pub sales_value: f64,
    pub volume: f64,
    #[serde(default)]
    pub basic_tax_due: f64,
}

/// `others` as saved now (a list) or before rows could be added (one
/// optional row).
fn others_rows<'de, D>(deserializer: D) -> Result<Vec<Form2200SOtherRow>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Rows(Vec<Form2200SOtherRow>),
        One(Option<Form2200SOtherRow>),
    }
    Ok(match Stored::deserialize(deserializer)? {
        Stored::Rows(rows) => rows,
        Stored::One(row) => row.into_iter().collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2200SDraft {
    #[serde(default)]
    pub id: Option<i64>,

    /// Item 1.
    pub month: u8,
    pub day: u8,
    pub year: u16,
    /// Item 2.
    pub is_amended: bool,
    /// Item 3 — number of sheet/s attached (`txtSheets`, two digits).
    #[serde(default)]
    pub number_of_attached_sheets: u16,
    /// Item 4: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    /// Item 5.
    pub rdo_code: String,
    /// Item 6.
    pub taxpayer_name: String,
    /// Item 7.
    pub registered_address: String,
    /// Item 7A.
    pub zip_code: String,
    /// Item 8.
    pub contact_number: String,
    /// Item 9.
    pub line_of_business: String,
    /// Item 10 (4 to 6 digits).
    #[serde(default)]
    pub psic: String,
    /// Item 11.
    pub email: String,
    /// Item 12.
    #[serde(default)]
    pub place_of_production: ExcisePlace,
    /// Item 13.
    #[serde(default)]
    pub place_of_removal: ExcisePlace,
    /// Items 14 / 14A.
    #[serde(default)]
    pub tax_relief: Form2200STaxRelief,
    /// Items 15 / 16.
    #[serde(default)]
    pub manner: Form2200SManner,
    /// Item 17 "Other similar schemes", only with a prepayment.
    #[serde(default)]
    pub other_scheme: bool,
    #[serde(default)]
    pub other_scheme_text: String,

    /// Schedule 1 fixed rows, in `FORM_2200S_BEVERAGES` order.
    #[serde(default)]
    pub beverages: Vec<Form2200SBeverageRow>,
    /// "Others" rows in page order; empty when the first row is untouched.
    /// Drafts saved with a single optional row still load.
    #[serde(default, deserialize_with = "others_rows")]
    pub others: Vec<Form2200SOtherRow>,
    /// Schedule 1 total tax due.
    #[serde(default)]
    pub schedule_total: f64,

    // Part III (printed item numbers)
    /// 18.
    #[serde(default)]
    pub excise_tax_due: f64,
    /// 19A.
    #[serde(default)]
    pub balance_carried_over: f64,
    /// 19B.
    #[serde(default)]
    pub creditable_excise_tax: f64,
    /// 19C.
    #[serde(default)]
    pub total_credits: f64,
    /// 20.
    #[serde(default)]
    pub net_tax_due: f64,
    /// 21, only on an amended return.
    #[serde(default)]
    pub previous_payment: f64,
    /// 22.
    #[serde(default)]
    pub tax_still_due: f64,
    /// 23A–23D (prepayment only).
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
    /// 24.
    #[serde(default)]
    pub amount_payable: f64,
    /// 25A.
    #[serde(default)]
    pub tax_payment: f64,
    /// 25B (penalties paid with the return).
    #[serde(default)]
    pub penalties_paid: f64,
    /// 25C.
    #[serde(default)]
    pub total_payment: f64,
    /// 26.
    #[serde(default)]
    pub balance_carried_forward: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

impl Form2200SDraft {
    pub const FORM_CODE: &'static str = "2200S";

    /// A new return for the dashboard month. Item 1 defaults to the last day
    /// of that month, or today when the month is the current one.
    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        use chrono::Datelike;
        let month = month.clamp(1, 12);
        let today = chrono::Local::now().date_naive();
        let day = if i32::from(year) == today.year() && u32::from(month) == today.month() {
            today.day() as u8
        } else {
            last_day_of_month(year, month)
        };
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile_typed_due_date(profile),
            id: None,
            month,
            day,
            year,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            line_of_business: profile.line_of_business.clone(),
            psic: String::new(),
            email: profile.email.clone(),
            place_of_production: ExcisePlace::default(),
            place_of_removal: ExcisePlace::default(),
            tax_relief: Form2200STaxRelief::No,
            manner: Form2200SManner::Unanswered,
            other_scheme: false,
            other_scheme_text: String::new(),
            beverages: Vec::new(),
            others: Vec::new(),
            schedule_total: 0.0,
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
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// The `Others` row's rate as `setApplicableTaxrate` leaves its text.
    fn others_rate_text(rate: f64) -> String {
        if !(0.0..=999.99).contains(&rate) {
            return "0.00".to_string();
        }
        round_down_input_text(rate)
    }

    /// The official compute chain.
    pub fn recompute(&mut self) {
        self.recompute_reporting();
    }

    /// [`Self::recompute`], returning true when a Schedule 1 row's Basic
    /// Excise Tax Due passed 999,999,999,999.99 and the row was reset, as
    /// `computeBasicTaxDue` / `computeSched1BasicTaxDue` do after their alert
    /// ([`FORM_2200S_BASIC_TAX_LIMIT_ALERT`]).
    pub fn recompute_reporting(&mut self) -> bool {
        let mut reset = false;
        let actual = self.manner == Form2200SManner::ActualRemoval;
        if !actual {
            // Schedule 1 belongs to a payment on actual removal.
            self.beverages.clear();
            self.others.clear();
        }
        if self.manner != Form2200SManner::Prepayment {
            self.other_scheme = false;
            // disablePenalties
            self.surcharge = 0.0;
            self.interest = 0.0;
            self.compromise = 0.0;
            // The three items are not on the return, so an amount entered
            // for them while it was a prepayment goes with them.
            self.penalty.clear_overrides();
        }
        if !self.other_scheme {
            self.other_scheme_text.clear();
        }
        if !self.is_amended {
            self.previous_payment = 0.0;
        }
        self.beverages
            .resize_with(if actual { 10 } else { 0 }, Default::default);

        // totalTaxDue: rows 0–8 of the fixed table, then the Others row.
        let mut total = 0.0;
        for (row, beverage) in self.beverages.iter_mut().zip(FORM_2200S_BEVERAGES) {
            row.sales_value = typed(row.sales_value);
            row.volume = typed(row.volume);
            row.basic_tax_due = computed(beverage.rate * row.volume);
            if row.basic_tax_due > BASIC_TAX_DUE_MAX {
                row.sales_value = 0.0;
                row.volume = 0.0;
                row.basic_tax_due = 0.0;
                reset = true;
            }
        }
        for row in self.beverages.iter().take(9) {
            total += row.basic_tax_due;
        }
        for other in &mut self.others {
            other.atc = other.atc.trim().to_uppercase();
            other.rate = parse_amount(&Self::others_rate_text(other.rate));
            other.sales_value = typed(other.sales_value);
            other.volume = typed(other.volume);
            other.basic_tax_due = computed(other.rate * other.volume);
            if other.basic_tax_due > BASIC_TAX_DUE_MAX {
                *other = Form2200SOtherRow {
                    atc: "XB".to_string(),
                    ..Form2200SOtherRow::default()
                };
                reset = true;
            }
            total += other.basic_tax_due;
        }
        self.schedule_total = computed(total);

        self.excise_tax_due = if actual { self.schedule_total } else { 0.0 };
        self.balance_carried_over = typed(self.balance_carried_over);
        self.creditable_excise_tax = typed(self.creditable_excise_tax);
        self.total_credits = computed(self.balance_carried_over + self.creditable_excise_tax);
        self.net_tax_due = computed(self.excise_tax_due - self.total_credits);
        self.previous_payment = typed(self.previous_payment);
        self.tax_still_due = computed(self.net_tax_due - self.previous_payment);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = typed(self.surcharge);
        self.interest = typed(self.interest);
        self.compromise = typed(self.compromise);
        self.total_penalties = computed(self.surcharge + self.interest + self.compromise);
        self.amount_payable = computed(self.tax_still_due + self.total_penalties);
        self.tax_payment = typed(self.tax_payment);
        // compute21D ticks "Pay Penalties?" whenever there are penalties.
        self.penalties_paid = if self.total_penalties != 0.0 {
            self.total_penalties
        } else {
            0.0
        };
        self.total_payment = computed(self.tax_payment + self.penalties_paid);
        self.balance_carried_forward = computed(self.amount_payable - self.total_payment);
        reset
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits` / `derived:page2_name`: the page 2 header TIN
    ///   (one 14-digit comb) and name, printed whether or not Schedule 1 is
    ///   open (the upload writes them only then).
    /// - `derived:address_line1` / `derived:line:address_line2`: Item 7
    ///   over its 36-slot first line, broken at a space, and the second.
    /// - `derived:contact_line`: Items 8-10 (contact number, line of
    ///   business, PSIC), one 36-slot comb under the three captions.
    /// - `derived:place_of_production` / `derived:place_of_removal`: Items
    ///   12-13 as region, province and city names on one comb each.
    /// - `derived:line:tax_relief_specify`: Item 14A, the relief named on a
    ///   Yes (the upload carries the `lstTaxTreaty` code).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        use super::excise_places::shared_comb_line;
        let mut fields = self.to_bir_field_map();
        // The upload copies the TIN and name into the page 2 header only
        // when Schedule 1 is opened; the printed header always carries them.
        let tin_digits = super::print_map::tin_digits(&self.tin);
        let page2_name = self.taxpayer_name.trim().to_uppercase();
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 36);
        let contact = shared_comb_line(
            &[
                (0, self.contact_number.as_str()),
                (12, &self.line_of_business.trim().to_uppercase()),
                (31, self.psic.as_str()),
            ],
            36,
        );
        let place = |place: &ExcisePlace| {
            let [region, province, city] =
                super::print_map::place_names(&place.region, &place.province, &place.city);
            shared_comb_line(&[(1, &region), (5, &province), (23, &city)], 36)
        };
        let specify = match self.tax_relief {
            Form2200STaxRelief::SpecialLaw => "SPECIAL LAW",
            Form2200STaxRelief::InternationalTaxTreaty => "INTERNATIONAL TAX TREATY",
            Form2200STaxRelief::No | Form2200STaxRelief::YesUnspecified => "",
        }
        .to_string();
        for (key, value) in [
            ("tin_digits", tin_digits),
            ("page2_name", page2_name),
            ("address_line1", line1),
            ("line:address_line2", line2),
            ("contact_line", contact),
            ("place_of_production", place(&self.place_of_production)),
            ("place_of_removal", place(&self.place_of_removal)),
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
            fields.insert(format!("frm2200S:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let money = round_down_input_text;
        let sum = round_down_computation_text;

        put("txtMonth1", format!("{:02}", self.month));
        put("txtDate", format!("{:02}", self.day));
        put("txtForYr", self.year.to_string());
        put("amendedRtn_1", flag(self.is_amended));
        put("amendedRtn_2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtTIN1", tin1.clone());
        put("txtTIN2", tin2.clone());
        put("txtTIN3", tin3.clone());
        put("txtBranchCode", branch.clone());
        put("txtRDOCode", self.rdo_code.trim().to_string());
        // loadBGData uppercases the profile name, address and line of business.
        let name = self.taxpayer_name.trim().to_uppercase();
        put("taxpayerName", name.clone());
        put("txtAddress", self.registered_address.trim().to_uppercase());
        put("txtZipCode", self.zip_code.trim().to_string());
        put("txtTelNum", self.contact_number.trim().to_string());
        put("txtLineBus", self.line_of_business.trim().to_uppercase());
        put("txtPSIC", self.psic.trim().to_string());
        put(
            "optTreaty_1",
            flag(self.tax_relief != Form2200STaxRelief::No),
        );
        put(
            "optTreaty_2",
            flag(self.tax_relief == Form2200STaxRelief::No),
        );
        put("lstTaxTreaty", self.tax_relief.list_value().to_string());
        put(
            "chkPymntManner_1",
            flag(self.manner == Form2200SManner::ActualRemoval),
        );
        put(
            "chkPymntManner_2",
            flag(self.manner == Form2200SManner::Prepayment),
        );
        put("chkPymntManner_3", flag(self.other_scheme));
        put(
            "txtOthMannerofPymnt",
            self.other_scheme_text.trim().to_string(),
        );

        put("txtTax16", sum(self.excise_tax_due));
        put("txtTax17A", money(self.balance_carried_over));
        put("txtTax17B", money(self.creditable_excise_tax));
        put("txtTax17C", sum(self.total_credits));
        put("txtTax18", sum(self.net_tax_due));
        put("txtTax19", money(self.previous_payment));
        put("txtTax20", sum(self.tax_still_due));
        put("txtTax21A", money(self.surcharge));
        put("txtTax21B", money(self.interest));
        put("txtTax21C", money(self.compromise));
        put("txtTax21D", sum(self.total_penalties));
        put("txtTax22", sum(self.amount_payable));
        put("txtTax23A", money(self.tax_payment));
        put("PayPenalties", flag(self.penalties_paid != 0.0));
        put("txtTax23B", sum(self.penalties_paid));
        put("txtTax23C", sum(self.total_payment));
        put("txtTax24", sum(self.balance_carried_forward));

        // showSched1 copies the TIN and name into the Schedule 1 header when
        // the schedule is opened, which a payment on actual removal requires.
        if self.manner == Form2200SManner::ActualRemoval {
            put("txtSched1TIN1", tin1);
            put("txtSched1TIN2", tin2);
            put("txtSched1TIN3", tin3);
            put("txtSched1BranchCode", branch);
            put("txtSched1TaxpayerName", name);
        }
        for (index, row) in self.beverages.iter().enumerate().take(10) {
            put(&format!("txtSalesValue{index}"), money(row.sales_value));
            put(&format!("txtVolumeRemovals{index}"), money(row.volume));
            put(&format!("txtBasicTaxDue{index}"), sum(row.basic_tax_due));
        }
        for (index, other) in self.others.iter().enumerate() {
            if index > 0 {
                // sched1Fields: an added row's checkbox starts unticked.
                put(&format!("sched1Chk{index}"), flag(false));
            }
            put(&format!("txtsched1Atc{index}"), other.atc.clone());
            put(
                &format!("txtsched1Desc{index}"),
                other.description.trim().to_string(),
            );
            put(
                &format!("txtsched1TaxBracket{index}"),
                other.tax_bracket.trim().to_string(),
            );
            put(
                &format!("txtsched1AppTaxRate{index}"),
                Self::others_rate_text(other.rate),
            );
            put(
                &format!("txtsched1SalesValue{index}"),
                money(other.sales_value),
            );
            put(
                &format!("txtsched1VolumeRemovals{index}"),
                money(other.volume),
            );
            put(
                &format!("txtsched1BasicTaxDue{index}"),
                sum(other.basic_tax_due),
            );
        }
        put("totalTaxDue", sum(self.schedule_total));

        let [region, province, city] = self.place_of_production.values();
        fields.insert("frm2200SoptRegion".into(), region);
        fields.insert("frm2200SoptProvince".into(), province);
        fields.insert("frm2200SoptCity".into(), city);
        let [region, province, city] = self.place_of_removal.values();
        fields.insert("frm2200SoptRegion1".into(), region);
        fields.insert("frm2200SoptProvince1".into(), province);
        fields.insert("frm2200SoptCity1".into(), city);
        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The generated layout (one "Others" row) with every added row's
    /// controls spliced in after the previous row's, as `addRow` appends
    /// them to `frm2200SadditionalSched1`.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        use super::official_inputs::{RowInsertion, extend_layout};
        const COLUMNS: [&str; 8] = [
            "sched1Chk",
            "txtsched1Atc",
            "txtsched1Desc",
            "txtsched1TaxBracket",
            "txtsched1AppTaxRate",
            "txtsched1SalesValue",
            "txtsched1VolumeRemovals",
            "txtsched1BasicTaxDue",
        ];
        let base = crate::official_xml::layout(FORM_2200S_FORM_ID).map_err(|e| e.to_string())?;
        let key = |column: &str, n: usize| format!("frm2200S:{column}{n}");
        let insertions: Vec<RowInsertion> = (1..self.others.len())
            .map(|n| RowInsertion {
                after: key("txtsched1BasicTaxDue", n - 1),
                copies: COLUMNS
                    .iter()
                    .map(|column| (key(column, 0), key(column, n)))
                    .collect(),
            })
            .collect();
        extend_layout(base, &insertions)
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

    /// `HHMMSS` of the moment the return was queued (Philippine time): the
    /// official file name carries the save time after the date.
    fn file_time(&self) -> String {
        let stamp = self
            .lifecycle
            .queue_authorization
            .as_ref()
            .map(|auth| auth.authorized_at.clone())
            .unwrap_or_else(|| self.lifecycle.updated_at.clone());
        let manila = chrono::FixedOffset::east_opt(8 * 3600).expect("UTC+8");
        chrono::DateTime::parse_from_rfc3339(&stamp)
            .map(|time| time.with_timezone(&manila).format("%H%M%S").to_string())
            .unwrap_or_else(|_| "000000".to_string())
    }
}

impl Form2200SDraft {
    /// `initialValidateBeforeSave` (BIR-Form2200S.hta): the first official
    /// Save alert and the key it concerns, or `None`. The app still saves
    /// the draft and shows this alert as a message after saving.
    pub fn save_alert(&self) -> Option<(&'static str, &'static str)> {
        let leap = chrono::NaiveDate::from_ymd_opt(i32::from(self.year), 2, 29).is_some();
        if !leap && self.month == 2 && self.day > 28 {
            return Some(("day", "Filing year is not a leap year."));
        }
        if leap && self.month == 2 && self.day > 29 {
            return Some(("day", "Invalid date entry."));
        }
        if self.day == 0 {
            return Some(("day", "Please enter valid day on item 1."));
        }
        if self.year == 0 {
            return Some(("year", "Please enter valid year on item 1."));
        }
        if self.year < 1900 {
            return Some((
                "year",
                "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
            ));
        }
        if self.day > 31 {
            return Some(("day", "Please indicate a valid Day."));
        }
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        if [tin1, tin2, tin3, branch]
            .iter()
            .any(|part| part.is_empty())
        {
            return Some(("tin", "Please enter a valid TIN number on Item 4."));
        }
        if self.rdo_code.trim().trim_start_matches('0').is_empty() {
            return Some(("rdo_code", "Please enter a valid RDO Code on Item 5."));
        }
        if self.taxpayer_name.is_empty() {
            return Some((
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 6.",
            ));
        }
        if self.registered_address.is_empty() {
            return Some((
                "registered_address",
                "Please enter a valid Registered Address on Item 7.",
            ));
        }
        if self.zip_code.chars().count() < 4 {
            return Some(("zip_code", "Please enter Taxpayer's Zip Code on Item 7A."));
        }
        if self.contact_number.is_empty() {
            return Some((
                "contact_number",
                "Please enter a valid Contact Number on Item 8.",
            ));
        }
        if self.line_of_business.is_empty() {
            return Some((
                "line_of_business",
                "Please enter a valid Main Line of Business on Item 9.",
            ));
        }
        None
    }
}

impl FormValidator for Form2200SDraft {
    /// `validateForm` in order with its alert texts, the Schedule 1 checks
    /// (`isValidDataOnSched1`, `validateSchedule1`) and the typing limits.
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let leap = chrono::NaiveDate::from_ymd_opt(i32::from(self.year), 2, 29).is_some();
        let today = chrono::Local::now().date_naive();

        if !(1..=12).contains(&self.month) {
            err("month", "Invalid date entry on Item no.1.");
        } else if !leap && self.month == 2 && self.day == 29 {
            err("day", "Filing year is not a leap year.");
        } else if self.month == 2 && self.day > 29 {
            err("day", "Invalid date entry on item 1.");
        } else if self.day == 0 {
            err("day", "Please enter valid day on item 1.");
        } else if self.year == 0 {
            err("year", "Please enter valid year on item 1.");
        } else if self.year < 2013 {
            err(
                "year",
                "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
            );
        } else if self.date().is_none() {
            err("day", "Invalid date entry on Item no.1.");
        } else if self.date().is_some_and(|date| date > today) {
            err("day", "Date on Item 1 cannot be a future date.");
        }
        if let Some(message) = tin_error(&self.tin, "Please enter a valid TIN number on Item 4.") {
            err("tin", &message);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        if self.taxpayer_name.trim().is_empty() {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 6.",
            );
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 7.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.len() != 4 || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 7A.");
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 20 || !digits_only(phone) {
            err("contact_number", "Please enter Contact Number on Item 8.");
        }
        if self.line_of_business.trim().is_empty() {
            err(
                "line_of_business",
                "Please enter a valid Main Line of Business on Item 9.",
            );
        }
        let psic = self.psic.trim();
        if !(4..=6).contains(&psic.len()) || !digits_only(psic) {
            err("psic", "Please enter a valid PSIC on Item 10.");
        }
        let email = self.email.trim();
        if email.is_empty() {
            err("email", "Please enter Email Address on Item 11.");
        } else if !email_matches_official_pattern(email) {
            err("email", "Please enter a valid Email Address on Item 11.");
        }
        let place_ok = |place: &ExcisePlace| {
            is_official_place(PLACE_FORM, &place.region, &place.province, &place.city)
        };
        if !place_ok(&self.place_of_production) {
            err(
                "place_of_production",
                "Please enter values on item no. 12. All entries must not be empty.",
            );
        }
        if !place_ok(&self.place_of_removal) {
            err(
                "place_of_removal",
                "Please enter values on item no. 13. All entries must not be empty.",
            );
        }
        if self.tax_relief == Form2200STaxRelief::YesUnspecified {
            err(
                "tax_relief",
                "Please select If yes, please specify on Item 14A.",
            );
        }
        if self.manner == Form2200SManner::Unanswered {
            err(
                "manner",
                "Please select an option on Part II Manner of Payment",
            );
        }
        if self.other_scheme && self.other_scheme_text.trim().is_empty() {
            err("other_scheme_text", "Please enter a value on Item 17.");
        }
        if self.manner == Form2200SManner::Prepayment && self.tax_payment <= 0.0 {
            err(
                "tax_payment",
                "Please enter Tax Payment / Deposit on Item 25A.",
            );
        }
        if self.balance_carried_forward > 0.0 {
            err(
                "tax_payment",
                "Payment not sufficient to cover amount payable. Please check amount in Item 25C.",
            );
        }
        if self.manner == Form2200SManner::ActualRemoval {
            if !self.beverages.iter().any(|row| row.volume > 0.0)
                && !self.others.iter().any(|o| o.volume > 0.0)
            {
                err("beverages", "Please fill up Schedule 1.");
            }
        }
        // isValidDataOnSched1, row by row.
        for (index, other) in self.others.iter().enumerate() {
            let n = index + 1;
            let quoted = "\"Others\"";
            let atc = other.atc.trim().to_uppercase();
            if atc.is_empty() || atc == "XB" {
                err(
                    &format!("others[{index}].atc"),
                    &format!("Please enter a valid ATC in {quoted} at Row {n}."),
                );
            } else if !atc.starts_with("XB") {
                err(
                    &format!("others[{index}].atc"),
                    &format!(
                        "The supplied ATC code in {quoted} at Row {n} should start with 'XB'."
                    ),
                );
            } else if FORM_2200S_BEVERAGES.iter().any(|b| b.atc == atc) {
                err(
                    &format!("others[{index}].atc"),
                    &format!(
                        "Please enter a valid ATC code in {quoted} at Row {n}.\n\nAdded ATC code should be differ from table."
                    ),
                );
            } else if atc.len() != 5 || !digits_only(&atc[2..]) {
                err(&format!("others[{index}].atc"), "Invalid ATC.");
            }
            if other.description.trim().is_empty() {
                err(
                    &format!("others[{index}].description"),
                    &format!("Please enter a Description in {quoted} at Row {n}."),
                );
            } else if other.tax_bracket.trim().is_empty() {
                err(
                    &format!("others[{index}].tax_bracket"),
                    &format!("Please enter a Tax Bracket in {quoted} at Row {n}."),
                );
            } else if other.rate <= 0.0 || other.rate > 999.99 {
                err(
                    &format!("others[{index}].rate"),
                    &format!("Please enter a valid Applicable rate in {quoted} at Row {n}."),
                );
            } else if other.volume <= 0.0 {
                err(
                    &format!("others[{index}].volume"),
                    &format!(
                        "Please enter a value on Volume Removals column in {quoted} at Row {n}."
                    ),
                );
            } else if self
                .others
                .iter()
                .filter(|o| {
                    o.atc.trim().to_uppercase() == atc
                        && Self::others_rate_text(o.rate) == Self::others_rate_text(other.rate)
                })
                .count()
                > 1
            {
                // checkTaxRate: the same ATC and rate on two "Others" rows.
                err(
                    &format!("others[{index}].rate"),
                    &format!("Please enter a unique Applicable rate in {quoted} at Row {n}."),
                );
            }
            let allowed = |c: char| c.is_ascii_alphanumeric() || " .,#@'()_-".contains(c);
            if !other.description.chars().all(allowed) || !other.tax_bracket.chars().all(allowed) {
                err(
                    &format!("others[{index}].description"),
                    "Schedule 1 Others: use letters, digits, spaces and . , # @ ' ( ) _ - only.",
                );
            }
        }

        // ── Implicit rules and typing limits ──
        if self.manner == Form2200SManner::Prepayment && self.date() != Some(today) {
            err(
                "day",
                "For a Prepayment/Advance Deposit, Item 1 is today's filing date.",
            );
        }
        if self.beverages.len() > 10 {
            err("beverages", "Schedule 1 has ten beverage rows.");
        }
        let amounts = self
            .beverages
            .iter()
            .flat_map(|row| [row.sales_value, row.volume])
            .chain(
                self.others
                    .iter()
                    .flat_map(|other| [other.sales_value, other.volume]),
            )
            .chain([
                self.balance_carried_over,
                self.creditable_excise_tax,
                self.previous_payment,
                self.surcharge,
                self.interest,
                self.compromise,
                self.tax_payment,
            ]);
        for value in amounts {
            if value < 0.0 || !has_cent_precision(value) || value >= 1e12 {
                err(
                    "amounts",
                    "Enter non-negative amounts in pesos and centavos below one trillion.",
                );
                break;
            }
        }
        if self.other_scheme_text.chars().count() > 50 {
            err("other_scheme_text", "Item 17 takes up to 50 characters.");
        }
        // Item 21 on an original return and stale totals are not checked
        // here: recompute() clears/refreshes them, and queue() recomputes
        // before the payload, so the official page has nothing to alert.
        errors
    }
}

/// `validateEmail`'s pattern:
/// `\b[a-zA-Z0-9._%+-]+@(?:[a-zA-Z0-9-]+\.)+[a-zA-Z]{2,4}\b`, which the
/// official check only needs to find somewhere in the address.
fn email_matches_official_pattern(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    let local_ok = local
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c));
    let labels: Vec<&str> = domain.split('.').collect();
    let tld = labels.last().copied().unwrap_or("");
    let tld_ok = tld.len() >= 2 && tld.chars().take(4).all(|c| c.is_ascii_alphabetic());
    local_ok
        && labels.len() >= 2
        && labels[..labels.len() - 1]
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && tld_ok
        && !email.contains(char::is_whitespace)
}

impl Form2200SDraft {
    /// Items 23A-23C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "23A Surcharge",
                interest: "23B Interest",
                compromise: "23C Compromise",
            ],
            controls: [
                "frm2200S:txtTax21A",
                "frm2200S:txtTax21B",
                "frm2200S:txtTax21C",
            ],
            state: penalty,
            basic_tax_due: "Item 22", |d| d.tax_still_due,
            // Decision D8 (2026-10-18): due on removal, so the penalties run
            // from Item 1, the removal date the filer types.
            typed_due_date: Some(|d: &Self| {
                chrono::NaiveDate::from_ymd_opt(
                    i32::from(d.year),
                    u32::from(d.month),
                    u32::from(d.day),
                )
            }),
            // disablePenalties: Items 23A-23C are open on a prepayment only.
            applies: |d: &Self| d.manner == Form2200SManner::Prepayment,
        )
    }
}

impl QueueableForm for Form2200SDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2200S";
    /// Official `formType` and PROD SFTP folder (`ftpTargetFolder.PROD['2200S']`).
    const FORM_TYPE: &'static str = "2200S";
    const LAYOUT_ID: &'static str = FORM_2200S_FORM_ID;

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
    /// `txtMonth1 + txtDate + txtForYr` (`createXMLFileName` then appends
    /// the save time; see [`QueueableForm::submission_filename`]).
    fn period_code(&self) -> String {
        format!("{:02}{:02}{}", self.month, self.day, self.year)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if !(code.len() == 8 || code.len() == 14) || !digits_only(code) {
            return None;
        }
        let month: u32 = code[..2].parse().ok()?;
        let day: u32 = code[2..4].parse().ok()?;
        let year: u16 = code[4..8].parse().ok()?;
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
    /// Added "Others" rows extend the page, so the plaintext follows
    /// [`Form2200SDraft::official_layout`].
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
    /// `TIN+branch-2200S-MMDDYYYYHHMMSS#email#.xml`: `createXMLFileName`
    /// appends `getHHMMSS()` to the Item 1 date.
    fn submission_filename(&self) -> String {
        format!(
            "{}-{}-{}{}#{}#.xml",
            self.tin.replace('-', ""),
            Self::FORM_TYPE,
            self.period_code(),
            self.file_time(),
            self.submission_email()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place() -> ExcisePlace {
        ExcisePlace {
            region: "130000000".into(),
            province: "137400000".into(),
            city: "137404000".into(),
        }
    }

    pub(crate) fn sample() -> Form2200SDraft {
        let mut draft = Form2200SDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            month: 6,
            day: 30,
            year: 2025,
            is_amended: false,
            number_of_attached_sheets: 0,
            tin: "12345678800000".into(),
            rdo_code: "039".into(),
            taxpayer_name: "Sample Beverages Inc".into(),
            registered_address: "123 Sample St Quezon City".into(),
            zip_code: "1100".into(),
            contact_number: "0281234567".into(),
            line_of_business: "Beverages".into(),
            psic: "1104".into(),
            email: "sample.taxpayer@example.com".into(),
            place_of_production: place(),
            place_of_removal: place(),
            tax_relief: Form2200STaxRelief::No,
            manner: Form2200SManner::ActualRemoval,
            other_scheme: false,
            other_scheme_text: String::new(),
            beverages: Vec::new(),
            others: Vec::new(),
            schedule_total: 0.0,
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
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft.beverages[2].sales_value = 500_000.0;
        draft.beverages[2].volume = 10_000.509;
        draft.beverages[8].volume = 100.0;
        draft.tax_payment = 61_203.0;
        draft.recompute();
        draft
    }

    fn messages(draft: &Form2200SDraft) -> Vec<String> {
        <Form2200SDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn basic_tax_due_over_the_limit_resets_the_row() {
        let mut draft = sample();
        assert!(!draft.recompute_reporting());
        // XB010 is 6.00 per liter: 200 billion liters is 1.2 trillion pesos.
        draft.beverages[0].sales_value = 5.0;
        draft.beverages[0].volume = 200_000_000_000.0;
        assert!(draft.recompute_reporting());
        assert_eq!(draft.beverages[0].sales_value, 0.0);
        assert_eq!(draft.beverages[0].volume, 0.0);
        assert_eq!(draft.beverages[0].basic_tax_due, 0.0);
        assert_eq!(draft.beverages[2].sales_value, 500_000.0);
        draft.others.push(Form2200SOtherRow {
            atc: "XB099".into(),
            description: "Other".into(),
            tax_bracket: "x".into(),
            rate: 12.0,
            sales_value: 1.0,
            volume: 100_000_000_000.0,
            basic_tax_due: 0.0,
        });
        assert!(draft.recompute_reporting());
        assert_eq!(
            draft.others[0],
            Form2200SOtherRow {
                atc: "XB".into(),
                ..Form2200SOtherRow::default()
            }
        );
        assert_eq!(
            FORM_2200S_BASIC_TAX_LIMIT_ALERT,
            "Exceeded maximum amount of 999,999,999,999.99 on Basic Excise Tax Due."
        );
    }

    #[test]
    fn save_alerts_follow_initial_validate_before_save() {
        let draft = sample();
        assert_eq!(draft.save_alert(), None);
        let alert = |change: &dyn Fn(&mut Form2200SDraft)| {
            let mut d = sample();
            change(&mut d);
            d.save_alert().map(|(_, message)| message)
        };
        assert_eq!(
            alert(&|d| {
                d.month = 2;
                d.day = 29;
            }),
            Some("Filing year is not a leap year.")
        );
        assert_eq!(
            alert(&|d| {
                d.year = 2024;
                d.month = 2;
                d.day = 30;
            }),
            Some("Invalid date entry.")
        );
        assert_eq!(
            alert(&|d| d.day = 0),
            Some("Please enter valid day on item 1.")
        );
        assert_eq!(
            alert(&|d| d.year = 1899),
            Some("Invalid date entry on Item no.1. Entry should not be lower than 1900.")
        );
        assert_eq!(alert(&|d| d.day = 32), Some("Please indicate a valid Day."));
        assert_eq!(
            alert(&|d| d.tin = "12345".into()),
            Some("Please enter a valid TIN number on Item 4.")
        );
        assert_eq!(
            alert(&|d| d.rdo_code = "000".into()),
            Some("Please enter a valid RDO Code on Item 5.")
        );
        assert_eq!(
            alert(&|d| d.taxpayer_name.clear()),
            Some("Please enter a valid Taxpayer Name on Item 6.")
        );
        assert_eq!(
            alert(&|d| d.registered_address.clear()),
            Some("Please enter a valid Registered Address on Item 7.")
        );
        assert_eq!(
            alert(&|d| d.zip_code = "110".into()),
            Some("Please enter Taxpayer's Zip Code on Item 7A.")
        );
        assert_eq!(
            alert(&|d| d.contact_number.clear()),
            Some("Please enter a valid Contact Number on Item 8.")
        );
        assert_eq!(
            alert(&|d| d.line_of_business.clear()),
            Some("Please enter a valid Main Line of Business on Item 9.")
        );
        // Save is sparse: Validate-only items such as the email raise no
        // Save alert. (The editor saves the draft either way and shows the
        // alert afterwards; queueing blocks through validate().)
        assert_eq!(alert(&|d| d.email.clear()), None);
    }

    #[test]
    fn stale_or_amended_only_values_raise_no_app_text() {
        let mut draft = sample();
        draft.previous_payment = 10.0;
        draft.balance_carried_forward = 123.0;
        let messages = messages(&draft);
        assert!(!messages.iter().any(|m| m.contains("Item 21 applies")));
        assert!(!messages.iter().any(|m| m.contains("out of date")));
    }

    #[test]
    fn truncating_helpers_match_string_util2014() {
        assert_eq!(round_down_input_text(1234.567), "1,234.56");
        assert_eq!(round_down_input_text(1000.0), "1,000.00");
        assert_eq!(round_down_input_text(0.5), "0.50");
        assert_eq!(round_down_computation_text(6.0 * 10_000.5), "60,003.00");
        assert_eq!(round_down_computation_text(-0.5), "-0.50");
        assert_eq!(round_down_computation_text(0.1 + 0.2), "0.30");
        assert_eq!(round_down_computation_text(1.005 * 3.0), "3.01");
        assert_eq!(js_to_precision14(0.1 + 0.2), 0.3);
    }

    #[test]
    fn schedule_one_and_part_three_follow_the_official_chain() {
        let draft = sample();
        assert_eq!(draft.beverages[2].volume, 10_000.50);
        assert_eq!(draft.beverages[2].basic_tax_due, 60_003.0);
        assert_eq!(draft.beverages[8].basic_tax_due, 1_200.0);
        assert_eq!(draft.schedule_total, 61_203.0);
        assert_eq!(draft.excise_tax_due, 61_203.0);
        assert_eq!(draft.amount_payable, 61_203.0);
        assert_eq!(draft.balance_carried_forward, 0.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm2200S:txtVolumeRemovals2"], "10,000.50");
        assert_eq!(fields["frm2200S:txtTax16"], "61,203.00");
        assert_eq!(fields["frm2200S:taxpayerName"], "SAMPLE BEVERAGES INC");
        assert_eq!(
            fields["frm2200S:txtSched1TaxpayerName"],
            "SAMPLE BEVERAGES INC"
        );
        assert_eq!(fields["frm2200SoptCity"], "137404000");
    }

    #[test]
    fn prepayment_has_no_schedule_and_pays_penalties() {
        let mut draft = sample();
        draft.manner = Form2200SManner::Prepayment;
        draft.surcharge = 100.0;
        draft.keep_entered_penalties();
        draft.tax_payment = 0.0;
        draft.recompute();
        assert!(draft.beverages.is_empty());
        assert_eq!(draft.excise_tax_due, 0.0);
        assert_eq!(draft.total_penalties, 100.0);
        assert_eq!(draft.penalties_paid, 100.0);
        assert_eq!(draft.balance_carried_forward, 0.0);
        assert!(
            messages(&draft)
                .contains(&"Please enter Tax Payment / Deposit on Item 25A.".to_string())
        );
        assert_eq!(draft.to_bir_field_map()["frm2200S:PayPenalties"], "true");
    }

    #[test]
    fn filename_carries_the_queue_time() {
        let mut draft = sample();
        draft
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        let name = draft.submission_filename();
        assert!(name.starts_with("12345678800000-2200S-06302025"), "{name}");
        assert_eq!(
            name.len(),
            "12345678800000-2200S-06302025HHMMSS#sample.taxpayer@example.com#.xml".len()
        );
        assert!(draft.revalidate_queued_before_submission().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "06302025");
        assert_eq!(
            Form2200SDraft::parse_period_code("06302025"),
            Some((2025, FilingPeriod::OpenEnded(630)))
        );
        assert_eq!(
            Form2200SDraft::parse_period_code("06302025101112"),
            Some((2025, FilingPeriod::OpenEnded(630)))
        );
        assert_eq!(Form2200SDraft::parse_period_code("02302025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2200SDraft), expected: &str| {
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
            &|d| {
                d.month = 2;
                d.day = 29;
            },
            "Filing year is not a leap year.",
        );
        check(&|d| d.day = 0, "Please enter valid day on item 1.");
        check(
            &|d| d.year = 2012,
            "Invalid date entry on Item no.1. Entry should not be lower than 1900.",
        );
        check(
            &|d| {
                d.month = 4;
                d.day = 31;
            },
            "Invalid date entry on Item no.1.",
        );
        check(
            &|d| d.year = 2999,
            "Date on Item 1 cannot be a future date.",
        );
        check(
            &|d| d.tin = "1".into(),
            "Please enter a valid TIN number on Item 4.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 5.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 6.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 7.",
        );
        check(
            &|d| d.zip_code = "110".into(),
            "Please enter Taxpayer's Zip Code on Item 7A.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter Contact Number on Item 8.",
        );
        check(
            &|d| d.line_of_business.clear(),
            "Please enter a valid Main Line of Business on Item 9.",
        );
        check(
            &|d| d.psic = "12".into(),
            "Please enter a valid PSIC on Item 10.",
        );
        check(
            &|d| d.email.clear(),
            "Please enter Email Address on Item 11.",
        );
        check(
            &|d| d.place_of_production.city.clear(),
            "Please enter values on item no. 12. All entries must not be empty.",
        );
        check(
            &|d| d.place_of_removal = ExcisePlace::default(),
            "Please enter values on item no. 13. All entries must not be empty.",
        );
        check(
            &|d| d.tax_relief = Form2200STaxRelief::YesUnspecified,
            "Please select If yes, please specify on Item 14A.",
        );
        check(
            &|d| d.manner = Form2200SManner::Unanswered,
            "Please select an option on Part II Manner of Payment",
        );
        check(
            &|d| d.tax_payment = 0.0,
            "Payment not sufficient to cover amount payable. Please check amount in Item 25C.",
        );
        check(
            &|d| {
                for row in &mut d.beverages {
                    row.volume = 0.0;
                }
                d.tax_payment = 0.0;
            },
            "Please fill up Schedule 1.",
        );
        check(
            &|d| {
                d.others = vec![Form2200SOtherRow {
                    atc: "XB010".into(),
                    ..Default::default()
                }]
            },
            "Please enter a valid ATC code in \"Others\" at Row 1.\n\nAdded ATC code should be differ from table.",
        );
        check(
            &|d| {
                d.others = vec![Form2200SOtherRow {
                    atc: "XB200".into(),
                    description: "Other drink".into(),
                    tax_bracket: "Per liter".into(),
                    rate: 0.0,
                    ..Default::default()
                }]
            },
            "Please enter a valid Applicable rate in \"Others\" at Row 1.",
        );
        let other = |atc: &str, rate: f64| Form2200SOtherRow {
            atc: atc.into(),
            description: "Other drink".into(),
            tax_bracket: "Per liter".into(),
            rate,
            volume: 10.0,
            ..Default::default()
        };
        check(
            &|d| d.others = vec![other("XB200", 3.0), other("XB", 3.0)],
            "Please enter a valid ATC in \"Others\" at Row 2.",
        );
        check(
            &|d| d.others = vec![other("XB200", 3.0), other("AB201", 3.0)],
            "The supplied ATC code in \"Others\" at Row 2 should start with 'XB'.",
        );
        check(
            &|d| {
                d.others = vec![other("XB200", 3.0), other("XB201", 3.0)];
                d.others[1].description.clear();
            },
            "Please enter a Description in \"Others\" at Row 2.",
        );
        check(
            &|d| {
                d.others = vec![other("XB200", 3.0), other("XB201", 3.0)];
                d.others[1].tax_bracket.clear();
            },
            "Please enter a Tax Bracket in \"Others\" at Row 2.",
        );
        check(
            &|d| {
                d.others = vec![other("XB200", 3.0), other("XB201", 3.0)];
                d.others[1].volume = 0.0;
            },
            "Please enter a value on Volume Removals column in \"Others\" at Row 2.",
        );
        check(
            &|d| d.others = vec![other("XB200", 3.0), other("XB200", 3.0)],
            "Please enter a unique Applicable rate in \"Others\" at Row 1.",
        );
    }

    #[test]
    fn drafts_saved_with_one_optional_others_row_still_load() {
        let mut draft = sample();
        draft.others = vec![Form2200SOtherRow {
            atc: "XB200".into(),
            ..Default::default()
        }];
        let mut json = serde_json::to_value(&draft).unwrap();
        json["others"] = serde_json::to_value(&draft.others[0]).unwrap();
        let loaded: Form2200SDraft = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(loaded.others, draft.others);
        json["others"] = serde_json::Value::Null;
        let loaded: Form2200SDraft = serde_json::from_value(json).unwrap();
        assert!(loaded.others.is_empty());
    }

    #[test]
    fn tile_month_defaults_to_its_last_day() {
        assert_eq!(last_day_of_month(2025, 12), 31);
        assert_eq!(last_day_of_month(2024, 2), 29);
        assert_eq!(last_day_of_month(2025, 6), 30);
    }

    #[test]
    fn sheets_go_to_txt_sheets() {
        let mut draft = sample();
        assert_eq!(draft.to_bir_field_map()["frm2200S:txtSheets"], "0");
        draft.number_of_attached_sheets = 3;
        assert_eq!(draft.to_bir_field_map()["frm2200S:txtSheets"], "3");
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
            <Form2200SDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "amounts"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2200SDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "amounts"),
            "a three-decimal amount must still be refused"
        );
    }
}
