//! BIR Form 2200-AN (January 2018) — Excise Tax Return for Automobiles and
//! Non-Essential Goods.
//!
//! Ported from the official `BIR-Form2200ANv2018.hta` (eBIRForms 7.9.6.2.1):
//! Schedules 1A–1C (`computeBasicExciseAuto`, `computeBasicExciseGoods`,
//! `computeSchedule1C`), Part III (`compute17C` … `compute24`, including
//! `compute22` adding Items 21A–21C and their total 21D), the item handlers
//! (`processAmended`, `processItem15`, `processTaxRelief`, `checkDate`),
//! `validate()` with its exact alerts and `saveXMLsubmit` through
//! [`crate::official_xml`].
//!
//! The schedule's units and market values have no handler on the official
//! page (they are submitted as typed) and each row's basic excise tax due is
//! entered by the filer; this port submits values in the official amount
//! format.
//!
//! eBIRForms has no upload folder for `2200ANv2018`, so
//! the official submit passes an undefined upload folder; queueing stays off in
//! the registry until the folder is confirmed.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::excise_places::{ExcisePlace, is_official_place, place_name};
use super::form_2000::{amount_in_official_range, cents, digits_only, split_tin, tin_error};
use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::official_amount;
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_2200AN_FORM_ID: &str = "2200an-v2018";
const PLACE_FORM: &str = "2200AN";

/// One fixed schedule row: ATC, group, bracket and rate as printed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Form2200ANItem {
    pub atc: &'static str,
    pub group: &'static str,
    pub bracket: &'static str,
    pub rate: &'static str,
}

const fn item(
    atc: &'static str,
    group: &'static str,
    bracket: &'static str,
    rate: &'static str,
) -> Form2200ANItem {
    Form2200ANItem {
        atc,
        group,
        bracket,
        rate,
    }
}

const HYBRID: &str = "50% of the applicable tax rate";

/// Schedule 1A (automobiles), in page order.
pub const FORM_2200AN_AUTOMOBILES: [Form2200ANItem; 22] = [
    item("XG021", "Passenger Car", "Up to P600,000", "4%"),
    item(
        "XG022",
        "Passenger Car",
        "Over P600,000 to P1,000,000",
        "10%",
    ),
    item(
        "XG023",
        "Passenger Car",
        "Over P1,000,000 to P4,000,000",
        "20%",
    ),
    item("XG024", "Passenger Car", "Over P4,000,000", "50%"),
    item("XG031", "Utility Vehicles", "Up to P600,000", "4%"),
    item(
        "XG032",
        "Utility Vehicles",
        "Over P600,000 to P1,000,000",
        "10%",
    ),
    item(
        "XG033",
        "Utility Vehicles",
        "Over P1,000,000 to P4,000,000",
        "20%",
    ),
    item("XG034", "Utility Vehicles", "Over P4,000,000", "50%"),
    item("XG055", "Pick-ups", "", "Exempt"),
    item("XG041", "Passenger Vans", "Up to P600,000", "4%"),
    item(
        "XG042",
        "Passenger Vans",
        "Over P600,000 to P1,000,000",
        "10%",
    ),
    item(
        "XG043",
        "Passenger Vans",
        "Over P1,000,000 to P4,000,000",
        "20%",
    ),
    item("XG044", "Passenger Vans", "Over P4,000,000", "50%"),
    item("XG065", "Hybrid Vehicles", "Up to P600,000", HYBRID),
    item(
        "XG071",
        "Hybrid Vehicles",
        "Over P600,000 to P1,000,000",
        HYBRID,
    ),
    item(
        "XG072",
        "Hybrid Vehicles",
        "Over P1,000,000 to P4,000,000",
        HYBRID,
    ),
    item("XG073", "Hybrid Vehicles", "Over P4,000,000", HYBRID),
    item("XG068", "Purely Electric Vehicles", "", "Exempt"),
    item("XG061", "Others", "Up to P600,000", "4%"),
    item("XG062", "Others", "Over P600,000 to P1,000,000", "10%"),
    item("XG063", "Others", "Over P1,000,000 to P4,000,000", "20%"),
    item("XG064", "Others", "Over P4,000,000", "50%"),
];

/// Schedule 1B (non-essential goods) fixed rows.
pub const FORM_2200AN_GOODS: [Form2200ANItem; 3] = [
    item(
        "XG100",
        "Jewelries, Pearls, Precious and Semi-precious Stones, whether Real or Imitation",
        "",
        "20%",
    ),
    item("XG110", "Perfumes and Toilet Waters", "", "20%"),
    item(
        "XG120",
        "Yachts and Other Vessels for Pleasure or Sports",
        "",
        "20%",
    ),
];

/// Units, market values and the filer's basic excise tax due for one row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200ANRow {
    /// Number of units — exempt/underbond (digits).
    pub exempt_units: String,
    /// Number of units — taxable (digits).
    pub taxable_units: String,
    /// Market value — exempt/underbond.
    pub exempt_value: f64,
    /// Market value — taxable.
    pub taxable_value: f64,
    /// Basic excise tax due.
    pub tax_due: f64,
}

impl Form2200ANRow {
    pub fn is_blank(&self) -> bool {
        self.exempt_units.trim().is_empty()
            && self.taxable_units.trim().is_empty()
            && self.exempt_value == 0.0
            && self.taxable_value == 0.0
            && self.tax_due == 0.0
    }
}

/// One of the two Schedule 1B "Others (specify)" rows.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2200ANOtherGood {
    /// `XG` + up to four characters.
    pub atc: String,
    pub name: String,
    /// Tax rate as written (up to five characters).
    pub rate: String,
    #[serde(flatten)]
    pub row: Form2200ANRow,
}

/// Part II — manner of payment (Items 13–15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2200ANManner {
    #[default]
    Unanswered,
    ActualRemoval,
    Prepayment,
    Other,
}

fn money_text(value: f64) -> String {
    if value == 0.0 {
        String::new()
    } else {
        official_amount(value)
    }
}

/// One schedule row's five controls (`txt<ATC>Exempt1` … `TaxDue`).
fn put_row(fields: &mut BTreeMap<String, String>, prefix: &str, row: &Form2200ANRow) {
    let mut put = |suffix: &str, value: String| {
        fields.insert(format!("frm2200ANv2018:txt{prefix}{suffix}"), value);
    };
    put("Exempt1", row.exempt_units.clone());
    put("Taxable1", row.taxable_units.clone());
    put("Exempt2", money_text(row.exempt_value));
    put("Taxable2", money_text(row.taxable_value));
    put("TaxDue", official_amount(row.tax_due));
}

fn is_zero_sheets(sheets: &u16) -> bool {
    *sheets == 0
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2200ANDraft {
    #[serde(default)]
    pub id: Option<i64>,

    /// Item 1.
    pub month: u8,
    pub day: u8,
    pub year: u16,
    /// Item 2.
    pub is_amended: bool,
    /// Item 3, Number of Sheet/s Attached: the official `txtSheets` is an
    /// open box (`maxlength="2"`, default `0`). Left out of a stored draft
    /// while it is 0, so a draft saved before it existed keeps its shape.
    #[serde(default, skip_serializing_if = "is_zero_sheets")]
    pub number_of_attached_sheets: u16,
    /// Item 5: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    /// Item 6.
    pub rdo_code: String,
    /// Item 8.
    pub taxpayer_name: String,
    /// Item 10 (registered address).
    pub registered_address: String,
    /// Item 11 (zip code).
    pub zip_code: String,
    /// Item 9.
    pub contact_number: String,
    pub email: String,
    /// Place of production.
    #[serde(default)]
    pub place_of_production: ExcisePlace,
    /// Place of removal.
    #[serde(default)]
    pub place_of_removal: ExcisePlace,
    /// Item 12: availing of tax relief (unanswered until picked).
    #[serde(default)]
    pub tax_relief: Option<bool>,
    /// Item 12A.
    #[serde(default)]
    pub tax_relief_specify: String,
    /// Items 13–15.
    #[serde(default)]
    pub manner: Form2200ANManner,
    #[serde(default)]
    pub manner_other: String,

    /// Schedule 1A rows in `FORM_2200AN_AUTOMOBILES` order.
    #[serde(default)]
    pub automobiles: Vec<Form2200ANRow>,
    /// Schedule 1B rows in `FORM_2200AN_GOODS` order.
    #[serde(default)]
    pub goods: Vec<Form2200ANRow>,
    /// Schedule 1B "Others" rows (at most two).
    #[serde(default)]
    pub other_goods: Vec<Form2200ANOtherGood>,
    #[serde(default)]
    pub automobiles_total: f64,
    #[serde(default)]
    pub goods_total: f64,
    /// Schedule 1C.
    #[serde(default)]
    pub schedule_total: f64,

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

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

impl Form2200ANDraft {
    pub const FORM_CODE: &'static str = "2200AN";

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
            email: profile.email.clone(),
            place_of_production: ExcisePlace::default(),
            place_of_removal: ExcisePlace::default(),
            tax_relief: None,
            tax_relief_specify: String::new(),
            manner: Form2200ANManner::Unanswered,
            manner_other: String::new(),
            automobiles: Vec::new(),
            goods: Vec::new(),
            other_goods: Vec::new(),
            automobiles_total: 0.0,
            goods_total: 0.0,
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

    fn normalize_row(row: &mut Form2200ANRow) {
        row.exempt_units = row.exempt_units.trim().to_string();
        row.taxable_units = row.taxable_units.trim().to_string();
        row.exempt_value = cents(row.exempt_value);
        row.taxable_value = cents(row.taxable_value);
        row.tax_due = cents(row.tax_due);
    }

    /// The official compute chain.
    pub fn recompute(&mut self) {
        if self.tax_relief != Some(true) {
            self.tax_relief_specify.clear();
        }
        if self.manner != Form2200ANManner::Other {
            self.manner_other.clear();
        }
        if !self.is_amended {
            self.previous_payment = 0.0;
        }
        self.automobiles
            .resize_with(FORM_2200AN_AUTOMOBILES.len(), Default::default);
        self.goods
            .resize_with(FORM_2200AN_GOODS.len(), Default::default);
        self.other_goods.truncate(2);
        for row in self.automobiles.iter_mut().chain(self.goods.iter_mut()) {
            Self::normalize_row(row);
        }
        for other in &mut self.other_goods {
            other.atc = other.atc.trim().to_string();
            other.name = other.name.trim().to_string();
            other.rate = other.rate.trim().to_string();
            Self::normalize_row(&mut other.row);
        }
        // Float sums of the formatted fields, formatted once.
        let mut auto = 0.0;
        for row in &self.automobiles {
            auto += row.tax_due;
        }
        self.automobiles_total = cents(auto);
        let mut goods = 0.0;
        for row in &self.goods {
            goods += row.tax_due;
        }
        for index in 0..2 {
            goods += self
                .other_goods
                .get(index)
                .map(|o| o.row.tax_due)
                .unwrap_or(0.0);
        }
        self.goods_total = cents(goods);
        self.schedule_total = cents(self.automobiles_total + self.goods_total);

        self.excise_tax_due = self.schedule_total;
        self.balance_carried_over = cents(self.balance_carried_over);
        self.creditable_excise_tax = cents(self.creditable_excise_tax);
        self.total_credits = cents(self.balance_carried_over + self.creditable_excise_tax);
        self.net_tax_due = cents(self.excise_tax_due - self.total_credits);
        self.previous_payment = cents(self.previous_payment);
        self.tax_still_due = cents(self.net_tax_due - self.previous_payment);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.penalties_paid = self.total_penalties;
        // compute22 adds Items 21A, 21B, 21C and their total 21D.
        self.amount_payable = cents(
            self.tax_still_due
                + self.surcharge
                + self.interest
                + self.compromise
                + self.total_penalties,
        );
        self.tax_payment = cents(self.tax_payment);
        self.total_payment = cents(self.tax_payment + self.penalties_paid);
        self.balance_carried_forward = cents(self.amount_payable - self.total_payment);
    }

    /// The field map plus the print-only `derived:line:` values the printed
    /// sheet needs (`html-frozen/2200an-2018/writer-cells.json`): the Item 10
    /// and 11 region, province and city names (the upload carries codes) and
    /// `derived:tin_digits`, the page 2 header TIN as 14 digits (the upload's
    /// `txtPage2TIN` is dashed).
    /// A region prints its short name (`REGION IV-A`, `NCR`). Never submitted.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        fields.insert(
            "derived:tin_digits".to_string(),
            super::print_map::tin_digits(&self.tin),
        );
        for (item, place) in [
            (10, &self.place_of_production),
            (11, &self.place_of_removal),
        ] {
            for (level, code) in [
                ("region", &place.region),
                ("province", &place.province),
                ("city", &place.city),
            ] {
                let Some(name) = place_name(code.trim()).filter(|_| !code.trim().is_empty()) else {
                    continue;
                };
                let name = if level == "region" {
                    name.split(" - ").next().unwrap_or(name)
                } else {
                    name
                };
                let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
                fields.insert(format!("derived:line:item{item}_{level}"), name);
            }
        }
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2200ANv2018:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let money = official_amount;

        put("txtMonth", format!("{:02}", self.month));
        put("txtDate", format!("{:02}", self.day));
        put("txtYear", self.year.to_string());
        put("AmendedRtn1", flag(self.is_amended));
        put("AmendedRtn2", flag(!self.is_amended));
        put("txtSheets", self.number_of_attached_sheets.to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put("txtPage2TIN", format!("{tin1}-{tin2}-{tin3}-{branch}"));
        put("txtTIN1", tin1);
        put("txtTIN2", tin2);
        put("txtTIN3", tin3);
        put("txtBranchCode", branch);
        put("txtRDOCode", self.rdo_code.trim().to_string());
        // loadBGData keeps the profile casing here.
        put("txtTaxpayerName", self.taxpayer_name.trim().to_string());
        put("txtPage2Taxpayer", self.taxpayer_name.trim().to_string());
        put("txtAddress", self.registered_address.trim().to_string());
        put("txtZipCode", self.zip_code.trim().to_string());
        put("txtTelNum", self.contact_number.trim().to_string());
        put("OptSpecialTax1", flag(self.tax_relief == Some(true)));
        put("OptSpecialTax2", flag(self.tax_relief == Some(false)));
        put("lstSpecialTax", self.tax_relief_specify.trim().to_string());
        put(
            "MannerOfPayment1",
            flag(self.manner == Form2200ANManner::ActualRemoval),
        );
        put(
            "MannerOfPayment2",
            flag(self.manner == Form2200ANManner::Prepayment),
        );
        put(
            "MannerOfPayment3",
            flag(self.manner == Form2200ANManner::Other),
        );
        put(
            "MannerOfPaymentOthers",
            self.manner_other.trim().to_string(),
        );

        put("txt16", money(self.excise_tax_due));
        put("txt17A", money(self.balance_carried_over));
        put("txt17B", money(self.creditable_excise_tax));
        put("txt17C", money(self.total_credits));
        put("txt18", money(self.net_tax_due));
        put("txt19", money(self.previous_payment));
        put("txt20", money(self.tax_still_due));
        put("txt21A", money(self.surcharge));
        put("txt21B", money(self.interest));
        put("txt21C", money(self.compromise));
        put("txt21D", money(self.total_penalties));
        put("txt22", money(self.amount_payable));
        put("txt23A", money(self.tax_payment));
        put("txt23B", money(self.penalties_paid));
        put("txt23C", money(self.total_payment));
        put("txt24", money(self.balance_carried_forward));

        for (item, row) in FORM_2200AN_AUTOMOBILES.iter().zip(&self.automobiles) {
            put_row(&mut fields, item.atc, row);
        }
        for (item, row) in FORM_2200AN_GOODS.iter().zip(&self.goods) {
            put_row(&mut fields, item.atc, row);
        }
        for index in 0..2 {
            let other = self.other_goods.get(index).cloned().unwrap_or_default();
            let prefix = format!("XGOthers{}", index + 1);
            put_row(&mut fields, &prefix, &other.row);
            let atc = if other.atc.is_empty() {
                "XG".to_string()
            } else {
                other.atc.clone()
            };
            fields.insert(format!("frm2200ANv2018:txt{prefix}"), atc);
            fields.insert(
                format!("frm2200ANv2018:txt{prefix}Name"),
                other.name.clone(),
            );
            fields.insert(format!("frm2200ANv2018:txt{prefix}Tax"), other.rate.clone());
        }
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2200ANv2018:{key}"), value);
        };
        put("txtBasicExciseTaxDue", money(self.automobiles_total));
        put(
            "txtBasicExciseTaxDueNonEssentialGoods",
            money(self.goods_total),
        );
        put("txtSchedule1C", money(self.schedule_total));

        let [region, province, city] = self.place_of_production.values();
        fields.insert("frm2200ANv2018optRegion".into(), region);
        fields.insert("frm2200ANv2018optProvince".into(), province);
        fields.insert("frm2200ANv2018optCity".into(), city);
        let [region, province, city] = self.place_of_removal.values();
        fields.insert("frm2200ANv2018optRegion1".into(), region);
        fields.insert("frm2200ANv2018optProvince1".into(), province);
        fields.insert("frm2200ANv2018optCity1".into(), city);
        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
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

    fn all_rows(&self) -> impl Iterator<Item = &Form2200ANRow> {
        self.automobiles
            .iter()
            .chain(self.goods.iter())
            .chain(self.other_goods.iter().map(|o| &o.row))
    }
}

impl FormValidator for Form2200ANDraft {
    /// `validate()` in order with its alert texts, then the typing limits.
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let today = chrono::Local::now().date_naive();

        match self.date() {
            None => err("day", "Enter a valid date (MM/DD/YYYY) in Item 1."),
            Some(date) if date > today => {
                err("day", "Date should not be later than the current date.")
            }
            _ => {}
        }
        let place_ok = |place: &ExcisePlace| {
            is_official_place(PLACE_FORM, &place.region, &place.province, &place.city)
        };
        if !place_ok(&self.place_of_production) {
            err(
                "place_of_production",
                "Please select the place of production in item 10.",
            );
        }
        if !place_ok(&self.place_of_removal) {
            err(
                "place_of_removal",
                "Please select the place of removal in item 11.",
            );
        }
        match self.tax_relief {
            None => err("tax_relief", "Please select an option in item number 12."),
            Some(true) if self.tax_relief_specify.trim().is_empty() => err(
                "tax_relief_specify",
                "Please specify tax relief in item 12A.",
            ),
            _ => {}
        }
        if self.manner == Form2200ANManner::Unanswered {
            err(
                "manner",
                "Please select from items 13, 14 and 15 the manner of payment.",
            );
        }
        if self.manner == Form2200ANManner::Other && self.manner_other.trim().is_empty() {
            err(
                "manner_other",
                "Please specify the other similar schemes in item 15.",
            );
        }
        if let Some(message) = tin_error(&self.tin, "Please enter a valid TIN number on Item 5.") {
            err("tin", &message);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 6.");
        }
        if self.taxpayer_name.trim().is_empty() {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 8.",
            );
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 20 || !digits_only(phone) {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 9.",
            );
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 10.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 11.");
        }

        // ── Typing limits ──
        for row in self.all_rows() {
            let units_ok = [&row.exempt_units, &row.taxable_units]
                .iter()
                .all(|u| u.len() <= 20 && digits_only(u.trim()));
            let amounts_ok = [row.exempt_value, row.taxable_value, row.tax_due]
                .iter()
                .all(|v| *v >= 0.0 && has_cent_precision(*v) && amount_in_official_range(*v));
            if !units_ok || !amounts_ok {
                err(
                    "schedule",
                    "Schedule 1: units are whole numbers; values and tax due are pesos and centavos.",
                );
                break;
            }
        }
        for (index, other) in self.other_goods.iter().enumerate() {
            if other.row.is_blank() && other.name.is_empty() {
                continue;
            }
            let atc_ok = other.atc.starts_with("XG")
                && other.atc.len() > 2
                && other.atc.len() <= 6
                && other.atc.chars().all(|c| c.is_ascii_alphanumeric());
            if !atc_ok || other.name.is_empty() || other.name.chars().count() > 30 {
                err(
                    &format!("other_goods[{index}]"),
                    &format!(
                        "Schedule 1B Others row {}: enter an XG ATC (up to 6 characters) and the item name (up to 30).",
                        index + 1
                    ),
                );
            }
            if other.rate.chars().count() > 5 {
                err(
                    &format!("other_goods[{index}].rate"),
                    "Schedule 1B Others: the tax rate takes up to 5 characters.",
                );
            }
        }
        if self.other_goods.len() > 2 {
            err("other_goods", "Schedule 1B has two Others rows.");
        }
        for (field, value) in [
            ("balance_carried_over", self.balance_carried_over),
            ("creditable_excise_tax", self.creditable_excise_tax),
            ("previous_payment", self.previous_payment),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
            ("tax_payment", self.tax_payment),
        ] {
            if value < 0.0 || !has_cent_precision(value) || !amount_in_official_range(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }
        if self.tax_relief_specify.chars().count() > 20 {
            err("tax_relief_specify", "Item 12A takes up to 20 characters.");
        }
        if self.manner_other.chars().count() > 50 {
            err("manner_other", "Item 15 takes up to 50 characters.");
        }
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
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

impl Form2200ANDraft {
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
                "frm2200ANv2018:txt21A",
                "frm2200ANv2018:txt21B",
                "frm2200ANv2018:txt21C",
            ],
            state: penalty,
            basic_tax_due: "Item 20", |d| d.tax_still_due,
            // Decision D8 (2026-10-18): due on removal, so the penalties run
            // from Item 1, the removal date the filer types.
            typed_due_date: Some(|d: &Self| {
                chrono::NaiveDate::from_ymd_opt(
                    i32::from(d.year),
                    u32::from(d.month),
                    u32::from(d.day),
                )
            }),
        )
    }
}

impl QueueableForm for Form2200ANDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2200AN";
    /// Official `formType` (`saveXMLsubmit`, `createXMLFileName`). PROD has no
    /// upload folder for it; see the module docs.
    const FORM_TYPE: &'static str = "2200ANv2018";
    const LAYOUT_ID: &'static str = FORM_2200AN_FORM_ID;

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
    /// `txtMonth + txtDate + txtYear`, as in `createXMLFileName`.
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

    fn place() -> ExcisePlace {
        ExcisePlace {
            region: "130000000".into(),
            province: "137400000".into(),
            city: "137404000".into(),
        }
    }

    pub(crate) fn sample() -> Form2200ANDraft {
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Sample Motors Inc", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Automobiles", "registered_address": "1 Sample St",
            "zip_code": "1100", "phone": "0281234567", "email": "sample.taxpayer@example.com",
            "default_form_type": "2200ANv2018", "taxpayer_type": "Corporation"
        }))
        .unwrap();
        let mut draft = Form2200ANDraft::new_from_profile(&profile, 2025, 6);
        draft.month = 6;
        draft.day = 30;
        draft.place_of_production = place();
        draft.place_of_removal = place();
        draft.tax_relief = Some(false);
        draft.manner = Form2200ANManner::ActualRemoval;
        draft.automobiles[0] = Form2200ANRow {
            exempt_units: String::new(),
            taxable_units: "2".into(),
            exempt_value: 0.0,
            taxable_value: 1_000_000.0,
            tax_due: 40_000.0,
        };
        draft.goods[1].tax_due = 1_234.567;
        draft.surcharge = 100.0;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    #[test]
    fn print_map_names_the_places_and_keeps_the_upload_codes() {
        let draft = sample();
        let print = draft.to_print_field_map();
        assert_eq!(print["derived:line:item10_region"], "NCR");
        assert!(!print["derived:line:item10_province"].is_empty());
        assert!(!print["derived:line:item11_city"].is_empty());
        assert_eq!(print["frm2200ANv2018optRegion"], "130000000");
        assert!(
            !draft
                .to_bir_field_map()
                .keys()
                .any(|key| key.starts_with("derived:"))
        );
    }

    fn messages(draft: &Form2200ANDraft) -> Vec<String> {
        <Form2200ANDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn schedule_and_part_three_follow_the_official_chain() {
        let draft = sample();
        assert_eq!(draft.goods[1].tax_due, 1_234.57);
        assert_eq!(draft.automobiles_total, 40_000.0);
        assert_eq!(draft.goods_total, 1_234.57);
        assert_eq!(draft.schedule_total, 41_234.57);
        assert_eq!(draft.excise_tax_due, 41_234.57);
        assert_eq!(draft.total_penalties, 100.0);
        // compute22 counts the penalties twice (21A–21C and 21D).
        assert_eq!(draft.amount_payable, 41_434.57);
        assert_eq!(draft.penalties_paid, 100.0);
        assert_eq!(draft.balance_carried_forward, 41_334.57);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm2200ANv2018:txtXG021Taxable2"], "1,000,000.00");
        assert_eq!(fields["frm2200ANv2018:txtXG021Exempt2"], "");
        assert_eq!(fields["frm2200ANv2018:txtPage2TIN"], "123-456-788-00000");
        assert_eq!(
            fields["frm2200ANv2018:txtTaxpayerName"],
            "Sample Motors Inc"
        );
        assert_eq!(fields["frm2200ANv2018:txtXGOthers1"], "XG");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2200ANv2018-06302025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "06302025");
        assert_eq!(
            Form2200ANDraft::parse_period_code("06302025"),
            Some((2025, FilingPeriod::OpenEnded(630)))
        );
        assert_eq!(Form2200ANDraft::parse_period_code("06312025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2200ANDraft), expected: &str| {
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
            &|d| d.year = 2999,
            "Date should not be later than the current date.",
        );
        check(
            &|d| d.place_of_production.city.clear(),
            "Please select the place of production in item 10.",
        );
        check(
            &|d| d.place_of_removal = ExcisePlace::default(),
            "Please select the place of removal in item 11.",
        );
        check(
            &|d| d.tax_relief = None,
            "Please select an option in item number 12.",
        );
        check(
            &|d| d.tax_relief = Some(true),
            "Please specify tax relief in item 12A.",
        );
        check(
            &|d| d.manner = Form2200ANManner::Unanswered,
            "Please select from items 13, 14 and 15 the manner of payment.",
        );
        check(
            &|d| d.manner = Form2200ANManner::Other,
            "Please specify the other similar schemes in item 15.",
        );
        check(
            &|d| d.tin = "12".into(),
            "Please enter a valid TIN number on Item 5.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 6.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 8.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number on Item 9.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 10.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 11.",
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

    #[test]
    fn tile_month_sets_item_1_month() {
        assert_eq!(last_day_of_month(2025, 12), 31);
        assert_eq!(last_day_of_month(2024, 2), 29);
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Sample Motors Inc", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Automobiles", "registered_address": "1 Sample St",
            "zip_code": "1100", "phone": "0281234567", "email": "sample.taxpayer@example.com",
            "default_form_type": "2200ANv2018", "taxpayer_type": "Corporation"
        }))
        .unwrap();
        let draft = Form2200ANDraft::new_from_profile(&profile, 2020, 2);
        assert_eq!((draft.month, draft.day), (2, 29));
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
            <Form2200ANDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2200ANDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
