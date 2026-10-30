//! BIR Form 1602-Q — Quarterly Remittance Return of Final Taxes Withheld on
//! Interest Paid on Deposits and Yield on Deposit Substitutes/Trusts/Etc.,
//! January 2018 (ENCS).
//!
//! Ported from the official `BIR-Form1602Qv2018.hta` (eBIRForms 7.9.6.2.1):
//! Schedules 1–3 (`computeSched1`, `computeSched1BspRate`, `computeSched2`,
//! `computeSched3`), `updateTaxRate`, the compute chain (`computeAll`),
//! `validateYear` / `validate` / `validateSched2` / `validateSched3` and the
//! code checks (`validateATC`, `validateTreatyCode`, `validateIPA`) with
//! their exact alert texts, and `saveXMLsubmit` (its plaintext follows the
//! generated layout `data/official-xml/1602q-v2018.json` as is).
//!
//! Run-time details this model follows:
//! - Schedule 1 items 1–12 read their rate from `<font id=fntSched1TaxRate…>`
//!   elements that `js/tax-rate-helper.js` fills from `xml/taxRate.xml`
//!   (20/12/5 percent); items 13–14 (foreign currency deposits) use
//!   `updateTaxRate`'s 15%/20% times the BSP reference rate.
//! - Item 15 (WI165) calls `computeSched1(…, 6, …)`, which looks up an element
//!   with id `6` and throws, so the official page never computes it; this
//!   model refuses an Item 15 amount instead of filing a zero tax.
//! - Text fields are uppercased only when `capital()` runs, which on this
//!   form happens when a Schedule 2/3 code field is left (`onblur`).

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
pub const FORM_1602Q_FORM_ID: &str = "1602q-v2018";
/// Schedule 1 has items 1–15.
pub const FORM_1602Q_SCHEDULE1_ROWS: usize = 15;

/// One Schedule 1 item: ATC, the kind of deposit and the rate text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form1602QSchedule1Item {
    pub atc_code: &'static str,
    pub nature: &'static str,
    /// Percent from `taxRate.xml`; `None` for items 13–14 (by period) and 15.
    pub rate: Option<&'static str>,
}

pub const FORM_1602Q_SCHEDULE1_ITEMS: [Form1602QSchedule1Item; FORM_1602Q_SCHEDULE1_ROWS] = [
    Form1602QSchedule1Item {
        atc_code: "WI161",
        nature: "Savings deposit",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WC161",
        nature: "Savings deposit",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WI161",
        nature: "Time deposit",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WC161",
        nature: "Time deposit",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WI162",
        nature: "Government securities",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WC162",
        nature: "Government securities",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WI163",
        nature: "Deposit substitutes / others",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WC163",
        nature: "Deposit substitutes / others",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WI440",
        nature: "Pre-terminated long-term deposits / investments",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WI441",
        nature: "Pre-terminated long-term deposits / investments",
        rate: Some("12"),
    },
    Form1602QSchedule1Item {
        atc_code: "WI442",
        nature: "Pre-terminated long-term deposits / investments",
        rate: Some("5"),
    },
    Form1602QSchedule1Item {
        atc_code: "WC440",
        nature: "Pre-terminated long-term deposits / investments",
        rate: Some("20"),
    },
    Form1602QSchedule1Item {
        atc_code: "WI170",
        nature: "Foreign currency deposit",
        rate: None,
    },
    Form1602QSchedule1Item {
        atc_code: "WC170",
        nature: "Foreign currency deposit",
        rate: None,
    },
    Form1602QSchedule1Item {
        atc_code: "WI165",
        nature: "Amounts withdrawn from a decedent's deposit account",
        rate: None,
    },
];

/// `validateATC`: ATCs Schedule 2 accepts.
pub const FORM_1602Q_SCHEDULE2_ATCS: &[&str] = &[
    "WI161", "WC161", "WI162", "WC162", "WI163", "WC163", "WI440", "WI441", "WI442", "WC440",
    "WI170", "WC170", "WI165",
];
/// `validateTreatyCode`: treaty codes Schedule 2 accepts.
pub const FORM_1602Q_TREATY_CODES: &[&str] = &[
    "AU", "AT", "BH", "BD", "BE", "BR", "CA", "CN", "CZ", "DK", "FI", "FR", "DE", "HU", "IN", "ID",
    "IL", "IT", "JP", "KR", "KW", "MY", "NL", "KZ", "NG", "NO", "PK", "PL", "RO", "RU", "SG", "ES",
    "SE", "CH", "TH", "UAE", "GB", "US", "VN",
];
/// `validateIPA`: investment promotion agencies Schedule 3 accepts.
pub const FORM_1602Q_IPA_CODES: &[&str] = &[
    "APECO",
    "AFAB",
    "BCDA",
    "FOI",
    "CEZA",
    "CDC",
    "JHMC",
    "PEZA",
    "PPMC",
    "RBOI-ARMM",
    "SBMA",
    "TIEZA",
    "ZCSEZA",
];

/// Item 11, category of withholding agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1602QCategory {
    Private,
    Government,
}

/// Item 13A (`lstSpecialTax`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1602QTaxRelief {
    #[default]
    Unspecified,
    SpecialRate,
    InternationalTaxTreaty,
    Both,
}

impl Form1602QTaxRelief {
    fn option_value(self) -> &'static str {
        match self {
            Self::Unspecified => "0",
            Self::SpecialRate => "1",
            Self::InternationalTaxTreaty => "2",
            Self::Both => "3",
        }
    }
}

/// Item 28 options (`OverRemittance1..3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1602QOverRemittance {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
    CarriedOver,
}

/// A Schedule 2 row (treaty rates).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1602QTreatyRow {
    pub treaty_code: String,
    pub atc_code: String,
    pub interest: f64,
    pub tax_rate: f64,
    #[serde(default)]
    pub tax_withheld: f64,
}

/// A Schedule 3 row (preferential rates).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1602QPreferentialRow {
    pub ipa_code: String,
    pub interest: f64,
    pub tax_rate: f64,
    #[serde(default)]
    pub tax_withheld: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1602QDraft {
    #[serde(default)]
    pub id: Option<i64>,

    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    /// Item 1.
    pub taxable_year: u16,
    /// Item 2 (1–4); 0 while unanswered.
    pub quarter: u8,
    /// Item 3; `None` while unanswered (the page has no default).
    #[serde(default)]
    pub is_amended: Option<bool>,
    /// Item 4; `None` while unanswered.
    #[serde(default)]
    pub any_tax_withheld: Option<bool>,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
    pub rdo_code: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    #[serde(default)]
    pub category: Option<Form1602QCategory>,
    pub email: String,
    /// Item 13: `None` unanswered, `Some(false)` "No", `Some(true)` "Yes".
    #[serde(default)]
    pub availing_tax_relief: Option<bool>,
    /// Item 13A.
    #[serde(default)]
    pub tax_relief: Form1602QTaxRelief,

    // Part IV
    /// Schedule 1 column "amount of interest" for items 1–15.
    #[serde(default)]
    pub schedule1_amount: [f64; FORM_1602Q_SCHEDULE1_ROWS],
    /// BSP reference rate for items 13 and 14.
    #[serde(default)]
    pub bsp_rate: [f64; 2],
    #[serde(default)]
    pub schedule1_tax: [f64; FORM_1602Q_SCHEDULE1_ROWS],
    #[serde(default)]
    pub schedule1_total: f64,
    #[serde(default)]
    pub schedule2: [Form1602QTreatyRow; 2],
    #[serde(default)]
    pub schedule2_total: f64,
    #[serde(default)]
    pub schedule3: [Form1602QPreferentialRow; 2],
    #[serde(default)]
    pub schedule3_total: f64,

    // Part II
    #[serde(default)]
    pub item14: f64,
    #[serde(default)]
    pub item15: f64,
    #[serde(default)]
    pub item16: f64,
    #[serde(default)]
    pub item17: f64,
    /// Items 18–21.
    #[serde(default)]
    pub remittance_first_month: f64,
    #[serde(default)]
    pub remittance_second_month: f64,
    #[serde(default)]
    pub tax_remitted_previous: f64,
    #[serde(default)]
    pub over_remittance_previous_quarter: f64,
    #[serde(default)]
    pub item22: f64,
    #[serde(default)]
    pub item23: f64,
    /// Items 24–26.
    #[serde(default)]
    pub surcharge: f64,
    #[serde(default)]
    pub interest_penalty: f64,
    #[serde(default)]
    pub compromise: f64,
    /// The penalty items' computed values, overrides, note and engine
    /// inputs (the shared model in [`super::penalty_items`]).
    #[serde(flatten)]
    pub penalty: PenaltyState,
    #[serde(default)]
    pub item27: f64,
    #[serde(default)]
    pub item28: f64,
    #[serde(default)]
    pub over_remittance: Form1602QOverRemittance,

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

impl Form1602QDraft {
    pub const FORM_CODE: &'static str = "1602Q";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            quarter,
            is_amended: None,
            any_tax_withheld: None,
            number_of_attached_sheets: 0,
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            category: None,
            email: profile.email.clone(),
            availing_tax_relief: None,
            tax_relief: Form1602QTaxRelief::Unspecified,
            schedule1_amount: [0.0; FORM_1602Q_SCHEDULE1_ROWS],
            bsp_rate: [0.0; 2],
            schedule1_tax: [0.0; FORM_1602Q_SCHEDULE1_ROWS],
            schedule1_total: 0.0,
            schedule2: Default::default(),
            schedule2_total: 0.0,
            schedule3: Default::default(),
            schedule3_total: 0.0,
            item14: 0.0,
            item15: 0.0,
            item16: 0.0,
            item17: 0.0,
            remittance_first_month: 0.0,
            remittance_second_month: 0.0,
            tax_remitted_previous: 0.0,
            over_remittance_previous_quarter: 0.0,
            item22: 0.0,
            item23: 0.0,
            surcharge: 0.0,
            interest_penalty: 0.0,
            compromise: 0.0,
            item27: 0.0,
            item28: 0.0,
            over_remittance: Form1602QOverRemittance::None,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 4. "No" clears Schedules 1–3 like `TaxWithhelOption`.
    pub fn set_any_tax_withheld(&mut self, withheld: bool) {
        self.any_tax_withheld = Some(withheld);
        if !withheld {
            self.schedule1_amount = [0.0; FORM_1602Q_SCHEDULE1_ROWS];
            self.bsp_rate = [0.0; 2];
            self.schedule2 = Default::default();
            self.schedule3 = Default::default();
        }
        self.recompute();
    }

    /// Item 13. "No" clears Schedule 2 like `DrpListTreaty`.
    pub fn set_availing_tax_relief(&mut self, availing: bool) {
        self.availing_tax_relief = Some(availing);
        if !availing {
            self.tax_relief = Form1602QTaxRelief::Unspecified;
            self.schedule2 = Default::default();
        }
        self.recompute();
    }

    /// `updateTaxRate`: the foreign currency deposit rate (items 13–14).
    pub fn foreign_currency_rate_text(&self) -> &'static str {
        let year = self.taxable_year;
        let late_quarter = matches!(self.quarter, 3 | 4);
        if (year >= 2025 && late_quarter) || year > 2025 {
            "20%"
        } else {
            "15%"
        }
    }

    fn foreign_currency_rate(&self) -> f64 {
        self.foreign_currency_rate_text()
            .trim_end_matches('%')
            .parse()
            .unwrap_or(0.0)
    }

    /// The official compute chain over values held at cents.
    pub fn recompute(&mut self) {
        if self.is_amended != Some(true) {
            self.tax_remitted_previous = 0.0;
        }
        let fcd_rate = self.foreign_currency_rate();
        let mut total = 0.0;
        for (index, item) in FORM_1602Q_SCHEDULE1_ITEMS.iter().enumerate() {
            let amount = cents(self.schedule1_amount[index]);
            self.schedule1_amount[index] = amount;
            let tax = match (index, item.rate) {
                // computeSched1: amount * (rate / 100).
                (_, Some(rate)) => amount * (rate.parse::<f64>().unwrap_or(0.0) / 100.0),
                // computeSched1BspRate: amount * (rate / 100) * BSP rate.
                (12 | 13, None) => {
                    let bsp = cents(self.bsp_rate[index - 12]);
                    self.bsp_rate[index - 12] = bsp;
                    amount * (fcd_rate / 100.0) * bsp
                }
                // Item 15: the official handler throws; nothing is computed.
                _ => 0.0,
            };
            self.schedule1_tax[index] = cents(tax);
            total += self.schedule1_tax[index];
        }
        self.schedule1_total = cents(total);
        self.item14 = self.schedule1_total;

        let mut total = 0.0;
        for row in &mut self.schedule2 {
            row.treaty_code = row.treaty_code.trim().to_uppercase();
            row.atc_code = row.atc_code.trim().to_uppercase();
            row.interest = cents(row.interest);
            row.tax_rate = cents(row.tax_rate);
            row.tax_withheld = cents(row.interest * (row.tax_rate / 100.0));
            total += row.tax_withheld;
        }
        self.schedule2_total = cents(total);
        self.item15 = self.schedule2_total;

        let mut total = 0.0;
        for row in &mut self.schedule3 {
            row.ipa_code = row.ipa_code.trim().to_uppercase();
            row.interest = cents(row.interest);
            row.tax_rate = cents(row.tax_rate);
            row.tax_withheld = cents(row.interest * (row.tax_rate / 100.0));
            total += row.tax_withheld;
        }
        self.schedule3_total = cents(total);
        self.item16 = self.schedule3_total;

        self.item17 = cents(self.item14 + self.item15 + self.item16);
        self.remittance_first_month = cents(self.remittance_first_month);
        self.remittance_second_month = cents(self.remittance_second_month);
        self.tax_remitted_previous = cents(self.tax_remitted_previous);
        self.over_remittance_previous_quarter = cents(self.over_remittance_previous_quarter);
        self.item22 = cents(
            self.remittance_first_month
                + self.remittance_second_month
                + self.tax_remitted_previous
                + self.over_remittance_previous_quarter,
        );
        self.item23 = cents(self.item17 - self.item22);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest_penalty = cents(self.interest_penalty);
        self.compromise = cents(self.compromise);
        self.item27 = cents(self.surcharge + self.interest_penalty + self.compromise);
        self.item28 = cents(self.item23 + self.item27);
        // computeItem28 clears the options unless Item 28 < 0.
        if self.item28 >= 0.0 {
            self.over_remittance = Form1602QOverRemittance::None;
        }
    }

    /// `capital()` runs (uppercasing every text input but txtEmail) when a
    /// Schedule 2 or 3 code field is left.
    fn capitalized(&self) -> bool {
        self.schedule2
            .iter()
            .any(|row| !row.treaty_code.is_empty() || !row.atc_code.is_empty())
            || self.schedule3.iter().any(|row| !row.ipa_code.is_empty())
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(key.to_string(), value);
        };
        let p = |key: &str| format!("frm1602Q:{key}");
        let flag = |on: bool| on.to_string();
        let upper = self.capitalized();
        let text = |value: &str| {
            let value = value.trim();
            if upper {
                value.to_uppercase()
            } else {
                value.to_string()
            }
        };

        put(&p("txtYear"), self.taxable_year.to_string());
        for quarter in 1..=4u8 {
            put(&p(&format!("qtr_{quarter}")), flag(self.quarter == quarter));
        }
        put(&p("AmendedRtn1"), flag(self.is_amended == Some(true)));
        put(&p("AmendedRtn2"), flag(self.is_amended == Some(false)));
        put(
            &p("OptTaxWithheld1"),
            flag(self.any_tax_withheld == Some(true)),
        );
        put(
            &p("OptTaxWithheld2"),
            flag(self.any_tax_withheld == Some(false)),
        );
        put(&p("txtSheets"), self.number_of_attached_sheets.to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        let dashed = format!("{tin1}-{tin2}-{tin3}-{branch}");
        put(&p("txtTIN1"), tin1);
        put(&p("txtTIN2"), tin2);
        put(&p("txtTIN3"), tin3);
        put(&p("txtBranchCode"), branch);
        put(&p("txtRDOCode"), self.rdo_code.trim().to_string());
        put(&p("txtTaxpayerName"), text(&self.taxpayer_name));
        put(&p("txtAddress"), text(&self.registered_address));
        put(&p("txtZipCode"), self.zip_code.trim().to_string());
        put(&p("txtTelNum"), self.contact_number.trim().to_string());
        put(
            &p("OptCategoryAgent1"),
            flag(self.category == Some(Form1602QCategory::Private)),
        );
        put(
            &p("OptCategoryAgent2"),
            flag(self.category == Some(Form1602QCategory::Government)),
        );
        put("txtEmail", self.email.trim().to_string());
        put(
            &p("OptSpecialTax1"),
            flag(self.availing_tax_relief == Some(true)),
        );
        put(
            &p("OptSpecialTax2"),
            flag(self.availing_tax_relief == Some(false)),
        );
        put(
            &p("lstSpecialTax"),
            if self.availing_tax_relief == Some(true) {
                self.tax_relief.option_value()
            } else {
                "0"
            }
            .to_string(),
        );

        for (item, value) in [
            (14, self.item14),
            (15, self.item15),
            (16, self.item16),
            (17, self.item17),
            (18, self.remittance_first_month),
            (19, self.remittance_second_month),
            (20, self.tax_remitted_previous),
            (21, self.over_remittance_previous_quarter),
            (22, self.item22),
            (23, self.item23),
            (24, self.surcharge),
            (25, self.interest_penalty),
            (26, self.compromise),
            (27, self.item27),
            (28, self.item28),
        ] {
            put(&p(&format!("txt{item}")), official_amount(value));
        }
        put(
            &p("OverRemittance1"),
            flag(self.over_remittance == Form1602QOverRemittance::Refund),
        );
        put(
            &p("OverRemittance2"),
            flag(self.over_remittance == Form1602QOverRemittance::TaxCreditCertificate),
        );
        put(
            &p("OverRemittance3"),
            flag(self.over_remittance == Form1602QOverRemittance::CarriedOver),
        );

        // Page headers, filled from the registration profile.
        put(&p("txtPage2TIN"), dashed.clone());
        put(&p("txtPage2Agent"), text(&self.taxpayer_name));
        put(&p("txtPage3TIN"), dashed);
        put(&p("txtPage3Agent"), text(&self.taxpayer_name));

        let fcd_rate = self.foreign_currency_rate_text();
        for index in 0..FORM_1602Q_SCHEDULE1_ROWS {
            let n = index + 1;
            put(
                &p(&format!("txtSched1Amount{n}")),
                official_amount(self.schedule1_amount[index]),
            );
            if n == 13 || n == 14 {
                put(&p(&format!("txtSched1TaxRate{n}")), fcd_rate.to_string());
                put(
                    &p(&format!("txtSched1Rate{n}")),
                    official_amount(self.bsp_rate[index - 12]),
                );
            }
            put(
                &p(&format!("txtSched1Tax{n}")),
                official_amount(self.schedule1_tax[index]),
            );
        }
        put(&p("txtSched1Total"), official_amount(self.schedule1_total));
        for (index, row) in self.schedule2.iter().enumerate() {
            let n = index + 1;
            put(
                &p(&format!("txtSched2TreatyCode{n}")),
                row.treaty_code.clone(),
            );
            put(&p(&format!("txtSched2ATC{n}")), row.atc_code.clone());
            put(
                &p(&format!("txtSched2Interest{n}")),
                official_amount(row.interest),
            );
            put(
                &p(&format!("txtSched2TaxRate{n}")),
                official_amount(row.tax_rate),
            );
            put(
                &p(&format!("txtSched2TaxesWithheld{n}")),
                official_amount(row.tax_withheld),
            );
        }
        put(&p("txtSched2Total"), official_amount(self.schedule2_total));
        for (index, row) in self.schedule3.iter().enumerate() {
            let n = index + 1;
            put(&p(&format!("txtSched3IPA{n}")), row.ipa_code.clone());
            put(
                &p(&format!("txtSched3TotInterest{n}")),
                official_amount(row.interest),
            );
            put(
                &p(&format!("txtSched3TaxRate{n}")),
                official_amount(row.tax_rate),
            );
            put(
                &p(&format!("txtSched3TaxWithheld{n}")),
                official_amount(row.tax_withheld),
            );
        }
        put(&p("txtSched3Total"), official_amount(self.schedule3_total));
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `validateYear`, `validate`, `validateSched2`/`3` and the code checks,
    /// against a given "today".
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
                "Invalid date entry on Item 1. Entry should not be lower than 2018.",
            );
        }
        if !(1..=4).contains(&self.quarter) {
            err("quarter", "Please select an option in item number 2.");
        }
        if self.is_amended.is_none() {
            err("is_amended", "Please select an option in item number 3.");
        }

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
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 70 {
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
        let address = self.registered_address.trim();
        if address.is_empty() || address.chars().count() > 150 {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 10.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 11.");
        }
        if self.any_tax_withheld.is_none() {
            err("any_tax_withheld", "Please select an option for Item 4.");
        }
        if self.category.is_none() {
            err("category", "Please select an option for Item 11.");
        }
        match self.availing_tax_relief {
            None => err("availing_tax_relief", "Please select an option in item 13."),
            Some(true) if self.tax_relief == Form1602QTaxRelief::Unspecified => {
                err("tax_relief", "Please select an option in item 13A.")
            }
            _ => {}
        }
        // The page checks Item 15 twice and never Item 16; kept as is.
        if self.any_tax_withheld == Some(true) && self.item14 == 0.0 && self.item15 == 0.0 {
            err(
                "schedule1_amount",
                "Please fill up Part IV if item 4 is set to Yes.",
            );
        }
        if self.item28 < 0.0 && self.over_remittance == Form1602QOverRemittance::None {
            err(
                "over_remittance",
                "Please select an option for over-remittance below item 28.",
            );
        }

        // validateSched2 (its last message says "row 12" for row 2).
        for (index, row) in self.schedule2.iter().enumerate() {
            let n = index + 1;
            let field = |name: &str| format!("schedule2[{index}].{name}");
            if !row.treaty_code.is_empty() && row.atc_code.is_empty() {
                err(
                    &field("atc_code"),
                    &format!("Please fill up ATC field in Schedule 2 row {n}."),
                );
            }
            if !row.atc_code.is_empty() && row.treaty_code.is_empty() {
                err(
                    &field("treaty_code"),
                    &format!("Please fill up Treaty Code field in Schedule 2 row {n}."),
                );
            }
            let incomplete = row.treaty_code.is_empty() || row.atc_code.is_empty();
            if row.interest > 0.0 && incomplete {
                err(
                    &field("interest"),
                    &format!("Please fill up Schedule 2 row {n} completely."),
                );
            }
            if row.tax_rate > 0.0 && incomplete {
                let label = if n == 2 { 12 } else { n };
                err(
                    &field("tax_rate"),
                    &format!("Please fill up Schedule 2 row {label} completely."),
                );
            }
            if !row.atc_code.is_empty()
                && !FORM_1602Q_SCHEDULE2_ATCS.contains(&row.atc_code.as_str())
            {
                err(
                    &field("atc_code"),
                    "Invalid ATC Code! Please refer to the ATC Codes in Schedule 1.",
                );
            }
            if !row.treaty_code.is_empty()
                && !FORM_1602Q_TREATY_CODES.contains(&row.treaty_code.as_str())
            {
                err(
                    &field("treaty_code"),
                    "Invalid Treaty Code! Please refer to the treaty code in schedule 5.",
                );
            }
        }
        // validateSched3.
        for (index, row) in self.schedule3.iter().enumerate() {
            let n = index + 1;
            if (row.interest > 0.0 || row.tax_rate > 0.0) && row.ipa_code.is_empty() {
                err(
                    &format!("schedule3[{index}].ipa_code"),
                    &format!("Please fill up Schedule 3 row {n} completely."),
                );
            }
            if !row.ipa_code.is_empty() && !FORM_1602Q_IPA_CODES.contains(&row.ipa_code.as_str()) {
                err(
                    &format!("schedule3[{index}].ipa_code"),
                    "Invalid IPA Code! Please refer to IPA Codes in Schedule 6.",
                );
            }
        }

        // Limits the page enforces while typing, and what the submission needs.
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
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
        if self.schedule1_amount[14] != 0.0 {
            err(
                "schedule1_amount[14]",
                "Schedule 1 item 15 (WI165) is not computed by the official form; file this amount through eBIRForms directly.",
            );
        }
        if self.any_tax_withheld == Some(false)
            && (self.schedule1_amount.iter().any(|v| *v != 0.0)
                || self.schedule2 != <[Form1602QTreatyRow; 2]>::default()
                || self.schedule3 != <[Form1602QPreferentialRow; 2]>::default())
        {
            err(
                "schedule1_amount",
                "Part IV applies only when Item 4 is \"Yes\".",
            );
        }
        if self.availing_tax_relief != Some(true)
            && self.schedule2 != <[Form1602QTreatyRow; 2]>::default()
        {
            // The way out, in the filer's words: Yes keeps the rows, No
            // (`set_availing_tax_relief`) removes them.
            err(
                "schedule2",
                if self.availing_tax_relief == Some(false) {
                    "Schedule 2 has amounts but item 13 is No. Answer item 13 Yes, or choose No again to remove them."
                } else {
                    "Schedule 2 applies only when Item 13 is \"Yes\"."
                },
            );
        }
        for (index, rate) in self.bsp_rate.iter().enumerate() {
            if *rate < 0.0 || *rate >= 1000.0 || !has_cent_precision(*rate) {
                err(
                    &format!("bsp_rate[{index}]"),
                    "Enter the BSP reference rate (up to 999.99).",
                );
            }
        }
        let rates = self
            .schedule2
            .iter()
            .map(|r| r.tax_rate)
            .chain(self.schedule3.iter().map(|r| r.tax_rate));
        for rate in rates {
            if !(0.0..100.0).contains(&rate) || !has_cent_precision(rate) {
                err("schedule2", "Enter a tax rate from 0 to below 100 percent.");
            }
        }
        let amounts = self
            .schedule1_amount
            .iter()
            .copied()
            .chain(self.schedule2.iter().map(|r| r.interest))
            .chain(self.schedule3.iter().map(|r| r.interest))
            .chain([
                self.remittance_first_month,
                self.remittance_second_month,
                self.tax_remitted_previous,
                self.over_remittance_previous_quarter,
                self.surcharge,
                self.interest_penalty,
                self.compromise,
            ]);
        for value in amounts {
            if value < 0.0 || value >= MAX_AMOUNT || !has_cent_precision(value) {
                err(
                    "amounts",
                    "Enter a non-negative amount in pesos and centavos.",
                );
                break;
            }
        }
        if self.is_amended != Some(true) && self.tax_remitted_previous != 0.0 {
            err(
                "tax_remitted_previous",
                "Item 20 applies only to an amended return.",
            );
        }

        let mut expected = self.clone();
        expected.recompute();
        if expected.item28 != self.item28
            || expected.schedule1_tax != self.schedule1_tax
            || expected.schedule2 != self.schedule2
            || expected.schedule3 != self.schedule3
            || expected.item17 != self.item17
        {
            err("item28", "Totals are out of date. Recompute the return.");
        }
        errors
    }
}

impl FormValidator for Form1602QDraft {
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1602QDraft {
    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the page-2 and page-3 header TIN, one
    ///   14-digit comb (the upload's `txtPage2TIN` is dashed).
    /// - `derived:address_line1` / `derived:line:address_line2`: Item 9 over
    ///   its 40-slot first line, broken at a space, and the 31-slot second.
    /// - `derived:line:tax_relief_specify`: Item 13A, the relief named on a
    ///   Yes.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        let specify = super::print_map::tax_relief_label(
            fields
                .get("frm1602Q:lstSpecialTax")
                .map_or("", String::as_str),
        );
        for (key, value) in [
            ("tin_digits", format!("{tin1}{tin2}{tin3}{branch}")),
            ("address_line1", line1),
            ("line:address_line2", line2),
            ("line:tax_relief_specify", specify.to_string()),
        ] {
            fields.insert(format!("derived:{key}"), value);
        }
        fields
    }

    /// Items 24-26 on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: Withholding,
            items: [
                surcharge: "24 Surcharge",
                interest_penalty: "25 Interest",
                compromise: "26 Compromise",
            ],
            controls: [
                "frm1602Q:txt24",
                "frm1602Q:txt25",
                "frm1602Q:txt26",
            ],
            state: penalty,
            basic_tax_due: "Item 23", |d| d.item23,
        )
    }
}

impl QueueableForm for Form1602QDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1602Q";
    /// Official `formType` (`1602Qv2018`).
    const FORM_TYPE: &'static str = "1602Qv2018";
    const LAYOUT_ID: &'static str = FORM_1602Q_FORM_ID;

    /// Item 28 Total Amount Still Due/(Over-remittance).
    fn amount_payable(&self) -> Option<f64> {
        Some(self.item28)
    }

    fn lifecycle(&self) -> &SubmissionLifecycle {
        &self.lifecycle
    }
    fn lifecycle_mut(&mut self) -> &mut SubmissionLifecycle {
        &mut self.lifecycle
    }

    /// Item "Amended Return?" = Yes, as the editor sets it.
    fn mark_amended(&mut self) -> bool {
        self.is_amended = Some(true);
        self.compute();
        true
    }
    fn amended_return_answer(&self) -> Option<bool> {
        Some(self.is_amended == Some(true))
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
    /// `txtYear + quarterPeriod()`, as in `createXMLFileName`.
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

    pub(crate) fn sample() -> Form1602QDraft {
        let mut draft = Form1602QDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            tin: "12345678800000".to_string(),
            taxable_year: 2025,
            quarter: 3,
            is_amended: Some(false),
            any_tax_withheld: None,
            number_of_attached_sheets: 0,
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Bank Inc".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            contact_number: "0281234567".to_string(),
            category: Some(Form1602QCategory::Private),
            email: "sample.taxpayer@example.com".to_string(),
            availing_tax_relief: None,
            tax_relief: Form1602QTaxRelief::Unspecified,
            schedule1_amount: [0.0; FORM_1602Q_SCHEDULE1_ROWS],
            bsp_rate: [0.0; 2],
            schedule1_tax: [0.0; FORM_1602Q_SCHEDULE1_ROWS],
            schedule1_total: 0.0,
            schedule2: Default::default(),
            schedule2_total: 0.0,
            schedule3: Default::default(),
            schedule3_total: 0.0,
            item14: 0.0,
            item15: 0.0,
            item16: 0.0,
            item17: 0.0,
            remittance_first_month: 0.0,
            remittance_second_month: 0.0,
            tax_remitted_previous: 0.0,
            over_remittance_previous_quarter: 0.0,
            item22: 0.0,
            item23: 0.0,
            surcharge: 0.0,
            interest_penalty: 0.0,
            compromise: 0.0,
            item27: 0.0,
            item28: 0.0,
            over_remittance: Form1602QOverRemittance::None,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.set_any_tax_withheld(true);
        draft.set_availing_tax_relief(true);
        draft.tax_relief = Form1602QTaxRelief::InternationalTaxTreaty;
        draft.schedule1_amount[0] = 100_000.005;
        draft.schedule1_amount[2] = 250_000.0;
        draft.schedule1_amount[8] = 50_000.0;
        draft.schedule1_amount[9] = 1_234.565;
        draft.schedule1_amount[12] = 10_000.0;
        draft.bsp_rate[0] = 56.789;
        draft.schedule2[0] = Form1602QTreatyRow {
            treaty_code: "us".into(),
            atc_code: "WC161".into(),
            interest: 30_000.0,
            tax_rate: 10.0,
            tax_withheld: 0.0,
        };
        draft.schedule3[0] = Form1602QPreferentialRow {
            ipa_code: "PEZA".into(),
            interest: 5_000.0,
            tax_rate: 5.0,
            tax_withheld: 0.0,
        };
        draft.remittance_first_month = 1_000.0;
        draft.remittance_second_month = 2_000.505;
        draft.over_remittance_previous_quarter = 500.0;
        draft.surcharge = 25.5;
        draft.interest_penalty = 10.0;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1602QDraft) -> Vec<String> {
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
        assert_eq!(draft.schedule1_tax[0], 20_000.0);
        assert_eq!(draft.schedule1_tax[2], 50_000.0);
        assert_eq!(draft.schedule1_tax[8], 10_000.0);
        assert_eq!(draft.schedule1_tax[9], 148.15);
        assert_eq!(draft.bsp_rate[0], 56.79);
        assert_eq!(draft.schedule1_tax[12], 113_580.0);
        assert_eq!(draft.item14, 193_728.15);
        assert_eq!(draft.item15, 3_000.0);
        assert_eq!(draft.item16, 250.0);
        assert_eq!(draft.item17, 196_978.15);
        assert_eq!(draft.item22, 3_500.51);
        assert_eq!(draft.item23, 193_477.64);
        assert_eq!(draft.item27, 35.5);
        assert_eq!(draft.item28, 193_513.14);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1602Q:txtSched1TaxRate13"], "20%");
        assert_eq!(fields["frm1602Q:txtSched1Rate13"], "56.79");
        assert_eq!(fields["frm1602Q:txtSched2TreatyCode1"], "US");
        assert_eq!(fields["frm1602Q:txtSched2TaxRate1"], "10.00");
        assert_eq!(fields["frm1602Q:txtPage2TIN"], "123-456-788-00000");
        // capital() ran (Schedule 2/3 codes), so names are uppercase.
        assert_eq!(fields["frm1602Q:txtPage3Agent"], "SAMPLE BANK INC");
        assert_eq!(fields["frm1602Q:lstSpecialTax"], "2");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1602Qv2018-2025Q3#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn names_keep_their_case_until_capital_runs() {
        let mut draft = sample();
        draft.set_availing_tax_relief(false);
        draft.schedule3 = Default::default();
        draft.recompute();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1602Q:txtTaxpayerName"], "Sample Bank Inc");
        assert_eq!(fields["frm1602Q:lstSpecialTax"], "0");
    }

    #[test]
    fn foreign_currency_rate_follows_update_tax_rate() {
        let mut draft = sample();
        draft.quarter = 2;
        assert_eq!(draft.foreign_currency_rate_text(), "15%");
        draft.taxable_year = 2026;
        draft.quarter = 1;
        assert_eq!(draft.foreign_currency_rate_text(), "20%");
        draft.taxable_year = 2024;
        draft.quarter = 4;
        assert_eq!(draft.foreign_currency_rate_text(), "15%");
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "2025Q3");
        assert_eq!(
            Form1602QDraft::parse_period_code("2025Q3"),
            Some((2025, FilingPeriod::Quarterly(3)))
        );
        assert_eq!(Form1602QDraft::parse_period_code("2025Q7"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1602QDraft), expected: &str| {
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
            "Invalid date entry on Item 1. Entry should not be lower than 2018.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Please file using the old version of the form.",
        );
        check(
            &|d| d.quarter = 0,
            "Please select an option in item number 2.",
        );
        check(
            &|d| d.is_amended = None,
            "Please select an option in item number 3.",
        );
        check(
            &|d| d.tin = "1".into(),
            "Please enter a valid TIN number on Item 5.",
        );
        check(
            &|d| d.rdo_code.clear(),
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
        check(
            &|d| d.any_tax_withheld = None,
            "Please select an option for Item 4.",
        );
        check(
            &|d| d.category = None,
            "Please select an option for Item 11.",
        );
        check(
            &|d| d.availing_tax_relief = None,
            "Please select an option in item 13.",
        );
        check(
            &|d| d.tax_relief = Form1602QTaxRelief::Unspecified,
            "Please select an option in item 13A.",
        );
        check(
            &|d| {
                d.schedule1_amount = [0.0; FORM_1602Q_SCHEDULE1_ROWS];
                d.schedule2 = Default::default();
            },
            "Please fill up Part IV if item 4 is set to Yes.",
        );
        check(
            &|d| d.remittance_first_month = 1_000_000.0,
            "Please select an option for over-remittance below item 28.",
        );
        check(
            &|d| d.schedule2[1].treaty_code = "JP".into(),
            "Please fill up ATC field in Schedule 2 row 2.",
        );
        check(
            &|d| d.schedule2[1].atc_code = "WI161".into(),
            "Please fill up Treaty Code field in Schedule 2 row 2.",
        );
        check(
            &|d| d.schedule2[1].interest = 10.0,
            "Please fill up Schedule 2 row 2 completely.",
        );
        check(
            &|d| d.schedule2[1].tax_rate = 10.0,
            "Please fill up Schedule 2 row 12 completely.",
        );
        check(
            &|d| d.schedule3[1].interest = 10.0,
            "Please fill up Schedule 3 row 2 completely.",
        );
        check(
            &|d| d.schedule2[0].atc_code = "WI999".into(),
            "Invalid ATC Code! Please refer to the ATC Codes in Schedule 1.",
        );
        check(
            &|d| d.schedule2[0].treaty_code = "XX".into(),
            "Invalid Treaty Code! Please refer to the treaty code in schedule 5.",
        );
        check(
            &|d| d.schedule3[0].ipa_code = "XYZ".into(),
            "Invalid IPA Code! Please refer to IPA Codes in Schedule 6.",
        );
    }

    #[test]
    fn item_fifteen_is_refused() {
        let mut draft = sample();
        draft.schedule1_amount[14] = 100.0;
        draft.recompute();
        assert_eq!(draft.schedule1_tax[14], 0.0);
        assert!(
            messages(&draft)
                .iter()
                .any(|m| m.contains("item 15 (WI165)"))
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
            <Form1602QDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "amounts"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form1602QDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "amounts"),
            "a three-decimal amount must still be refused"
        );
    }
}
