//! BIR Form 1707 (April 2021) — Capital Gains Tax Return for Onerous Transfer
//! of Shares of Stock Not Traded Through the Local Stock Exchange.
//!
//! Ported from the official `BIR-Form1707v2021.hta` (eBIRForms 7.9.6.2.1):
//! the transaction-type handlers (`transactionType`, `disableSched*`), the
//! compute chain (`pageTwoSched2Comp`, `pageTwoSched3Comp`,
//! `pageOneComputation`), `validateAll()` with its exact alert texts, and
//! the uploaded file (`saveEncryptedProfile`) through [`crate::official_xml`]. Background information
//! (TIN, name, address, contact, e-mail) comes from the taxpayer profile the
//! way `loadBGData()` fills it.
//!
//! The page holds up to three sellers and three buyers before "Add"; this
//! model covers those rows. Schedules 2 and 3 take any number of rows: beyond
//! row D the official "More" pop-up takes D and the rest, row D then reads
//! "OTHERS" with the pop-up subtotal, and the pop-up rows are written where
//! the page keeps them (see [`Form1707Draft::official_layout`]).

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use chrono::Datelike;
use serde::{Deserialize, Serialize};

use super::official_inputs::{
    RowInsertion, capital, cents, digits_only, extend_layout, format_fixed, group_thousands,
    has_cent_precision, is_calendar_date, split_tin, tin_is_well_formed, to_fixed_text,
    within_round_limit,
};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::official_amount;
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1707_FORM_ID: &str = "1707-v2021";
/// Seller / buyer rows on the page before "Add".
pub const FORM_1707_PARTY_ROWS: usize = 3;
/// Rows A–D of Schedules 2 and 3.
pub const FORM_1707_SCHEDULE_ROWS: usize = 4;
/// `transactionType` puts 15 in Item 13 / Schedule 1 Item 6.
pub const FORM_1707_TAX_RATE: f64 = 15.0;

/// Item 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1707Atc {
    /// II 030.
    Individual,
    /// IC 110, with the Domestic / Foreign pop-up.
    CorporationDomestic,
    CorporationForeign,
    /// Corporation ticked, pop-up not answered yet.
    CorporationUnspecified,
}

impl Form1707Atc {
    pub fn is_corporation(self) -> bool {
        !matches!(self, Self::Individual)
    }
}

/// Item 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1707TransactionType {
    CashSale,
    InstallmentSale,
    ForeclosureSale,
    Others,
}

impl Form1707TransactionType {
    pub const ALL: [Self; 4] = [
        Self::CashSale,
        Self::InstallmentSale,
        Self::ForeclosureSale,
        Self::Others,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::CashSale => "Cash Sale",
            Self::InstallmentSale => "Installment Sale",
            Self::ForeclosureSale => "Foreclosure Sale",
            Self::Others => "Others",
        }
    }

    fn radio(self) -> &'static str {
        match self {
            Self::CashSale => "rdoPg1I9TransDescCash",
            Self::InstallmentSale => "rdoPg1I9TransDescInstallment",
            Self::ForeclosureSale => "rdoPg1I9TransDescForeclosure",
            Self::Others => "rdoPg1I9TransDescOthers",
        }
    }
}

/// One seller (Item 6) or buyer (Item 7) row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1707Party {
    pub name: String,
    pub address: String,
    /// 12–14 digits (TIN and branch code), as typed.
    pub tin: String,
}

impl Form1707Party {
    fn is_blank(&self) -> bool {
        self.name.trim().is_empty() && self.address.trim().is_empty() && self.tin.trim().is_empty()
    }
}

/// Schedule 1, installment sale.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1707Installment {
    pub selling_price: f64,
    pub cost_and_expenses: f64,
    pub mortgage_assumed: f64,
    /// Item 4, as typed (`wholenumber`, 3 digits).
    pub number_of_installments: String,
    /// Item 5.
    pub installment_amount: f64,
    /// Item 7 (computed).
    #[serde(default)]
    pub period_tax_due: f64,
    /// Item 8, `MM/DD/YYYY`.
    pub collection_date: String,
    /// Item 9.
    pub total_collection: f64,
}

/// Schedule 2 row: a block of shares.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1707Shares {
    pub corporation: String,
    /// `roundSharesNum` keeps three decimals; `None` is the untouched `0.00`.
    pub number_of_shares: Option<f64>,
    pub certificate_number: String,
    pub selling_price: f64,
}

/// Schedule 3 row: cost and other allowable expenses.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1707Expense {
    pub particulars: String,
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1707Draft {
    #[serde(default)]
    pub id: Option<i64>,
    /// Dashboard year and open-ended key that identify this return.
    pub filing_year: u16,
    pub open_ended_key: u32,

    // Items 1–4
    pub transaction_month: u8,
    pub transaction_day: u8,
    pub transaction_year: u16,
    pub is_amended: bool,
    pub atc: Option<Form1707Atc>,
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Background information (taxpayer profile, `loadBGData`)
    /// 14 digits with branch.
    pub tin: String,
    /// Item 5.
    pub rdo_code: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,
    #[serde(default)]
    pub line_of_business: String,

    // Items 6–9
    #[serde(default)]
    pub sellers: Vec<Form1707Party>,
    #[serde(default)]
    pub buyers: Vec<Form1707Party>,
    /// Item 8; `Some` when "Yes", holding Item 8A.
    #[serde(default)]
    pub tax_relief: Option<String>,
    pub transaction_type: Option<Form1707TransactionType>,
    #[serde(default)]
    pub transaction_description: String,

    // Part IV
    #[serde(default)]
    pub installment: Form1707Installment,
    #[serde(default)]
    pub shares: Vec<Form1707Shares>,
    #[serde(default)]
    pub expenses: Vec<Form1707Expense>,
    /// Pop-up subtotals (`Pg2Pt4S2SubTotal` / `Pg2Pt4S3SubTotal`) when a
    /// schedule has more than four rows.
    #[serde(default)]
    pub shares_popup_subtotal: f64,
    #[serde(default)]
    pub expenses_popup_subtotal: f64,
    #[serde(default)]
    pub total_selling_price: f64,
    #[serde(default)]
    pub total_expenses: f64,

    // Part II
    /// 10.
    #[serde(default)]
    pub taxable_base: f64,
    /// 11.
    #[serde(default)]
    pub allowable_expenses: f64,
    /// 12.
    #[serde(default)]
    pub net_capital_gain: f64,
    /// 14 (`txtPg1P2I13TaxDueForTrans`). The page writes a bare `0` when
    /// Item 12 is a loss.
    #[serde(default)]
    pub tax_due: f64,
    #[serde(default)]
    pub tax_due_is_loss_zero: bool,
    /// 15 (`txtPg1P2I14TaxDuePmtPriod`).
    #[serde(default)]
    pub tax_due_this_period: f64,
    /// 16, amended returns only.
    #[serde(default)]
    pub tax_paid_previous: f64,
    /// 17.
    #[serde(default)]
    pub tax_payable: f64,
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
    /// 19.
    #[serde(default)]
    pub total_amount_payable: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

impl Form1707Draft {
    pub const FORM_CODE: &'static str = "1707";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, open_ended_key: u32) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            filing_year: year,
            open_ended_key,
            transaction_month: 0,
            transaction_day: 0,
            transaction_year: year,
            is_amended: false,
            atc: None,
            number_of_attached_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            line_of_business: profile.line_of_business.clone(),
            sellers: Vec::new(),
            buyers: Vec::new(),
            tax_relief: None,
            transaction_type: None,
            transaction_description: String::new(),
            installment: Form1707Installment::default(),
            shares: Vec::new(),
            expenses: Vec::new(),
            shares_popup_subtotal: 0.0,
            expenses_popup_subtotal: 0.0,
            total_selling_price: 0.0,
            total_expenses: 0.0,
            taxable_base: 0.0,
            allowable_expenses: 0.0,
            net_capital_gain: 0.0,
            tax_due: 0.0,
            tax_due_is_loss_zero: false,
            tax_due_this_period: 0.0,
            tax_paid_previous: 0.0,
            tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 9 (`transactionDesc` + `transactionType`): an installment sale
    /// clears Schedules 2 and 3; any other clears Schedule 1.
    pub fn set_transaction_type(&mut self, kind: Form1707TransactionType) {
        self.transaction_type = Some(kind);
        if kind != Form1707TransactionType::Others {
            self.transaction_description.clear();
        }
        if kind == Form1707TransactionType::InstallmentSale {
            self.shares.clear();
            self.expenses.clear();
        } else {
            self.installment = Form1707Installment::default();
        }
        self.recompute();
    }

    /// Item 2. "No" resets Item 16 to `0.00`.
    pub fn set_amended(&mut self, amended: bool) {
        self.is_amended = amended;
        if !amended {
            self.tax_paid_previous = 0.0;
        }
        self.recompute();
    }

    fn is_installment(&self) -> bool {
        self.transaction_type == Some(Form1707TransactionType::InstallmentSale)
    }

    fn shares_popup(&self) -> bool {
        self.shares.len() > FORM_1707_SCHEDULE_ROWS
    }

    fn expenses_popup(&self) -> bool {
        self.expenses.len() > FORM_1707_SCHEDULE_ROWS
    }

    /// Main-table row `index` (0..4) as the page shows it: row D reads
    /// "OTHERS" with the pop-up subtotal once the pop-up holds the rest.
    fn main_share_row(&self, index: usize) -> Form1707Shares {
        if index == FORM_1707_SCHEDULE_ROWS - 1 && self.shares_popup() {
            return Form1707Shares {
                corporation: "OTHERS".into(),
                number_of_shares: None,
                certificate_number: "OTHERS".into(),
                selling_price: self.shares_popup_subtotal,
            };
        }
        self.shares.get(index).cloned().unwrap_or_default()
    }

    fn main_expense_row(&self, index: usize) -> Form1707Expense {
        if index == FORM_1707_SCHEDULE_ROWS - 1 && self.expenses_popup() {
            return Form1707Expense {
                particulars: "OTHERS".into(),
                amount: self.expenses_popup_subtotal,
            };
        }
        self.expenses.get(index).cloned().unwrap_or_default()
    }

    /// The official compute chain, run as if every changed field was blurred.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_paid_previous = 0.0;
        }
        let installment = self.is_installment();
        if installment {
            self.shares.clear();
            self.expenses.clear();
        } else {
            self.installment = Form1707Installment::default();
        }
        let inst = &mut self.installment;
        inst.selling_price = cents(inst.selling_price);
        inst.cost_and_expenses = cents(inst.cost_and_expenses);
        inst.mortgage_assumed = cents(inst.mortgage_assumed);
        inst.installment_amount = cents(inst.installment_amount);
        inst.total_collection = cents(inst.total_collection);
        // Schedule 1 Item 7: (Item 5 × Item 6 / 100).toFixed(2); Item 6 is
        // blank (NaN → 0.00) unless this is an installment sale.
        inst.period_tax_due = if installment {
            format_fixed(inst.installment_amount * FORM_1707_TAX_RATE / 100.0)
        } else {
            0.0
        };

        for row in &mut self.shares {
            row.selling_price = cents(row.selling_price);
            row.number_of_shares = row
                .number_of_shares
                .map(|v| to_fixed_text(v, 3).parse().unwrap_or(0.0));
        }
        for row in &mut self.expenses {
            row.amount = cents(row.amount);
        }
        // Sum_Pg2Pt4S2 / Sum_Pg2Pt4S3: the pop-up adds rows D onwards, then
        // row D carries formatCurrency(subtotal).
        let popup = |values: Vec<f64>| -> f64 {
            if values.len() > FORM_1707_SCHEDULE_ROWS {
                format_fixed(values[FORM_1707_SCHEDULE_ROWS - 1..].iter().sum::<f64>())
            } else {
                0.0
            }
        };
        self.shares_popup_subtotal = popup(self.shares.iter().map(|r| r.selling_price).collect());
        self.expenses_popup_subtotal = popup(self.expenses.iter().map(|r| r.amount).collect());
        let mut total = 0.0;
        for index in 0..self.shares.len().min(FORM_1707_SCHEDULE_ROWS) {
            total = format_fixed(total + self.main_share_row(index).selling_price);
        }
        self.total_selling_price = total;
        let mut total = 0.0;
        for index in 0..self.expenses.len().min(FORM_1707_SCHEDULE_ROWS) {
            total = format_fixed(total + self.main_expense_row(index).amount);
        }
        self.total_expenses = total;

        self.taxable_base = format_fixed(self.total_selling_price);
        self.allowable_expenses = format_fixed(self.total_expenses);
        self.net_capital_gain = format_fixed(self.taxable_base - self.allowable_expenses);
        self.tax_due_is_loss_zero = false;
        match self.transaction_type {
            Some(Form1707TransactionType::InstallmentSale) => {
                self.tax_due_this_period = format_fixed(self.installment.period_tax_due);
                self.tax_due = 0.0;
            }
            Some(_) => {
                self.tax_due = format_fixed(self.net_capital_gain * (FORM_1707_TAX_RATE / 100.0));
                if self.net_capital_gain < 0.0 {
                    self.tax_due = 0.0;
                    self.tax_due_is_loss_zero = true;
                }
                self.tax_due_this_period = 0.0;
            }
            None => {
                self.tax_due = 0.0;
                self.tax_due_this_period = 0.0;
            }
        }
        self.tax_paid_previous = cents(self.tax_paid_previous);
        self.tax_payable = if installment {
            format_fixed(self.tax_due_this_period - self.tax_paid_previous)
        } else {
            format_fixed(self.tax_due - self.tax_paid_previous)
        };
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = format_fixed(self.surcharge + self.interest + self.compromise);
        self.total_amount_payable = if self.total_penalties > 0.0 && self.tax_payable < 0.0 {
            format_fixed(self.total_penalties)
        } else {
            format_fixed(self.tax_payable + self.total_penalties)
        };
    }

    fn date_code(&self) -> String {
        format!(
            "{:02}{:02}{:04}",
            self.transaction_month, self.transaction_day, self.transaction_year
        )
    }

    /// The registered address as `loadBGData` splits it: 80 + 80 characters.
    fn address_parts(&self) -> (String, String) {
        let address: Vec<char> = self.registered_address.chars().collect();
        let first: String = address.iter().take(80).collect();
        let second: String = address.iter().skip(80).take(80).collect();
        (first.to_uppercase(), second.to_uppercase())
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the later pages' header TIN, one 14-digit
    ///   comb (TIN then branch).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm1707:txtPg2TinC1",
                "frm1707:txtPg2TinC2",
                "frm1707:txtPg2TinC3",
                "frm1707:txtPg2TinC4",
            ],
        );
        fields.insert("derived:tin_digits".to_string(), tin_digits);
        fields
    }

    /// The official control values the upload loop writes, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm1707:{key}"), value);
        };
        let flag = |on: bool| on.to_string();
        let installment = self.is_installment();

        put("txtPg1I1Month", format!("{:02}", self.transaction_month));
        put("txtPg1I1Day", format!("{:02}", self.transaction_day));
        put("txtPg1I1Year", self.transaction_year.to_string());
        put("rdoPg1I2AmendedYes", flag(self.is_amended));
        put("rdoPg1I2AmendedNo", flag(!self.is_amended));
        put(
            "rdoPg1I3ATCIndiv",
            flag(self.atc == Some(Form1707Atc::Individual)),
        );
        put("txtPg1I3Indiv", "II 030".to_string());
        put(
            "rdoPg1I3ATCCorp",
            flag(self.atc.is_some_and(Form1707Atc::is_corporation)),
        );
        put("txtPg1I3Corp", "IC 110".to_string());
        put(
            "txtPg1I4NoOfSheets",
            self.number_of_attached_sheets.to_string(),
        );

        // loadBGData sets every control named TIN1..TIN4 on both pages.
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        for (page1, page2, value) in [
            ("txtPg1TIN1", "txtPg2TinC1", &tin1),
            ("txtPg1TIN2", "txtPg2TinC2", &tin2),
            ("txtPg1TIN3", "txtPg2TinC3", &tin3),
            ("txtPg1TIN4", "txtPg2TinC4", &branch),
        ] {
            put(page1, value.clone());
            put(page2, value.clone());
        }
        let name = self.taxpayer_name.to_uppercase();
        put("ProfileName", name.clone());
        put("txtPg2RegisteredName", name);
        let (address1, address2) = self.address_parts();
        put("ProfileAddress1", address1);
        put("ProfileAddress2", address2);
        put("ProfileZipCode", self.zip_code.clone());
        put("ProfileContactNum", self.contact_number.clone());
        put("ProfileEmailAddr", self.email.clone());
        put("rdoPg1Pt1I5RDO", rdo_value(&self.rdo_code));
        put("txtLOB", self.line_of_business.clone());

        for index in 0..FORM_1707_PARTY_ROWS {
            for (prefix, rows) in [("6Seller", &self.sellers), ("7Buyer", &self.buyers)] {
                let row = rows.get(index).cloned().unwrap_or_default();
                put(&format!("txtPg1I{prefix}Name{index}"), capital(&row.name));
                put(
                    &format!("txtPg1I{prefix}Addr{index}"),
                    capital(&row.address),
                );
                put(
                    &format!("txtPg1I{prefix}TIN{index}"),
                    row.tin.trim().to_string(),
                );
            }
        }

        put("rdoPg1I8TaxReliefYes", flag(self.tax_relief.is_some()));
        put("rdoPg1I8TaxReliefNo", flag(self.tax_relief.is_none()));
        put(
            "txtPg1I8TaxReliefSpec",
            self.tax_relief.as_deref().map(capital).unwrap_or_default(),
        );
        for kind in Form1707TransactionType::ALL {
            put(kind.radio(), flag(self.transaction_type == Some(kind)));
        }
        put(
            "txtPg1I9TransDescSpec",
            if self.transaction_type == Some(Form1707TransactionType::Others) {
                capital(&self.transaction_description)
            } else {
                String::new()
            },
        );

        put("txtPg1P2I10TaxBase", official_amount(self.taxable_base));
        put(
            "txtPg1P2I11AllowExpnse",
            official_amount(self.allowable_expenses),
        );
        put(
            "txtPg1P2I12NetCptalChnged",
            official_amount(self.net_capital_gain),
        );
        put(
            "txtPg1P2I13AppTaxRate",
            match self.transaction_type {
                None => "0.00".to_string(),
                Some(Form1707TransactionType::InstallmentSale) => String::new(),
                Some(_) => "15".to_string(),
            },
        );
        put(
            "txtPg1P2I13TaxDueForTrans",
            if self.tax_due_is_loss_zero {
                "0".to_string()
            } else {
                official_amount(self.tax_due)
            },
        );
        put(
            "txtPg1P2I14TaxDuePmtPriod",
            official_amount(self.tax_due_this_period),
        );
        put(
            "txtPg1P2I15TaxPaidInPrevRtrn",
            official_amount(self.tax_paid_previous),
        );
        put("txtPg1P2I16TaxPayable", official_amount(self.tax_payable));
        put("txtPg1P2I17Surcharge", official_amount(self.surcharge));
        put("txtPg1P2I17Interest", official_amount(self.interest));
        put("txtPg1P2I17Compromise", official_amount(self.compromise));
        put(
            "txtPg1P2I17TotPenalties",
            official_amount(self.total_penalties),
        );
        put(
            "txtPg1P2I18TotAmtPyable",
            official_amount(self.total_amount_payable),
        );

        let inst = &self.installment;
        put(
            "txtPg2P4Sch1I1SellPrice",
            official_amount(inst.selling_price),
        );
        put(
            "txtPg2P4Sch1I2CostExpense",
            official_amount(inst.cost_and_expenses),
        );
        put(
            "txtPg2P4Sch1I3AssumMortgage",
            official_amount(inst.mortgage_assumed),
        );
        let count = inst.number_of_installments.trim();
        put(
            "txtPg2P4Sch1I4NumInstallments",
            if count.is_empty() {
                "0".to_string()
            } else {
                count.to_string()
            },
        );
        put(
            "txtPg2P4Sch1I5InstallmentAmt",
            official_amount(inst.installment_amount),
        );
        put(
            "txtPg2P4Sch1I6ApplcRate",
            if installment {
                "15".to_string()
            } else {
                String::new()
            },
        );
        put(
            "txtPg2P4Sch1I7PeriodTaxDue",
            official_amount(inst.period_tax_due),
        );
        put(
            "txtPg2P4Sch1I8DateCollection",
            inst.collection_date.trim().to_string(),
        );
        put(
            "txtPg2P4Sch1I9TotCollection",
            official_amount(inst.total_collection),
        );

        let shares_text = |row: &Form1707Shares| {
            row.number_of_shares
                .map(|v| group_thousands(&to_fixed_text(v, 3)))
                .unwrap_or_else(|| "0.00".to_string())
        };
        for index in 0..FORM_1707_SCHEDULE_ROWS {
            let n = index + 1;
            let row = self.main_share_row(index);
            let popup_row = index == FORM_1707_SCHEDULE_ROWS - 1 && self.shares_popup();
            put(
                &format!("txtPg2P4Sch2NameOfCorpStock{n}"),
                capital(&row.corporation),
            );
            put(
                &format!("txtPg2P4Sch2NoOfShares{n}"),
                if popup_row {
                    "OTHERS".to_string()
                } else {
                    shares_text(&row)
                },
            );
            put(
                &format!("txtPg2P4Sch2StockCertNo{n}"),
                capital(&row.certificate_number),
            );
            put(
                &format!("txtPg2P4Sch2TaxBaseSellPrice{n}"),
                official_amount(row.selling_price),
            );
            let expense = self.main_expense_row(index);
            put(
                &format!("txtPg2P4Sch3Particulars{n}"),
                capital(&expense.particulars),
            );
            put(
                &format!("txtPg2P4Sch3Amount{n}"),
                official_amount(expense.amount),
            );
        }
        if self.shares_popup() {
            for (k, row) in self.shares[FORM_1707_SCHEDULE_ROWS - 1..]
                .iter()
                .enumerate()
            {
                let k = k + 1;
                put(&format!("txtPg2Pt4S2_{k}Col1"), capital(&row.corporation));
                put(&format!("txtPg2Pt4S2_{k}Col2"), shares_text(row));
                put(
                    &format!("txtPg2Pt4S2_{k}Col3"),
                    capital(&row.certificate_number),
                );
                put(
                    &format!("txtPg2Pt4S2_{k}Col4"),
                    official_amount(row.selling_price),
                );
            }
        }
        if self.expenses_popup() {
            for (k, row) in self.expenses[FORM_1707_SCHEDULE_ROWS - 1..]
                .iter()
                .enumerate()
            {
                let k = k + 1;
                put(&format!("txtPg2Pt4S3_{k}Col1"), capital(&row.particulars));
                put(&format!("txtPg2Pt4S3_{k}Col2"), official_amount(row.amount));
            }
        }
        put(
            "txtPg2P4Sch2TotAmount",
            official_amount(self.total_selling_price),
        );
        put(
            "txtPg2P4Sch3TotAmount",
            official_amount(self.total_expenses),
        );
        put(
            "rdoCorpoDomestic",
            flag(self.atc == Some(Form1707Atc::CorporationDomestic)),
        );
        put(
            "rdoCorpoForeign",
            flag(self.atc == Some(Form1707Atc::CorporationForeign)),
        );
        // Outside the frm1707 prefix: pop-up subtotals and lengths.
        if self.shares_popup() {
            fields.insert(
                "Pg2Pt4S2SubTotal".into(),
                official_amount(self.shares_popup_subtotal),
            );
            fields.insert(
                "Pg2Pt4S2PopLength".into(),
                (self.shares.len() - FORM_1707_SCHEDULE_ROWS + 1).to_string(),
            );
        }
        if self.expenses_popup() {
            fields.insert(
                "Pg2Pt4S3SubTotal".into(),
                official_amount(self.expenses_popup_subtotal),
            );
            fields.insert(
                "Pg2Pt4S3PopLength".into(),
                (self.expenses.len() - FORM_1707_SCHEDULE_ROWS + 1).to_string(),
            );
        }
        fields
    }

    /// The generated layout with the "More" pop-up rows spliced in where the
    /// page keeps them: Schedule 2's table (all column A cells, then B–D per
    /// row) before `Pg2Pt4S2SubTotal`, Schedule 3's before `Pg2Pt4S3SubTotal`.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        let base = crate::official_xml::layout(FORM_1707_FORM_ID).map_err(|e| e.to_string())?;
        let template = "Pg2Pt4S2SubTotal".to_string();
        let copy = |key: String| (template.clone(), key);
        let mut insertions = Vec::new();
        if self.shares_popup() {
            let rows = self.shares.len() - FORM_1707_SCHEDULE_ROWS + 1;
            let mut copies: Vec<_> = (1..=rows)
                .map(|k| copy(format!("frm1707:txtPg2Pt4S2_{k}Col1")))
                .collect();
            for k in 1..=rows {
                for col in 2..=4 {
                    copies.push(copy(format!("frm1707:txtPg2Pt4S2_{k}Col{col}")));
                }
            }
            insertions.push(RowInsertion {
                after: "frm1707:txtMaxPage".into(),
                copies,
            });
        }
        if self.expenses_popup() {
            let rows = self.expenses.len() - FORM_1707_SCHEDULE_ROWS + 1;
            let copies = (1..=rows)
                .flat_map(|k| {
                    [
                        copy(format!("frm1707:txtPg2Pt4S3_{k}Col1")),
                        copy(format!("frm1707:txtPg2Pt4S3_{k}Col2")),
                    ]
                })
                .collect();
            insertions.push(RowInsertion {
                after: "Pg2Pt4S2SubTotal".into(),
                copies,
            });
        }
        extend_layout(base, &insertions)
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

fn rdo_value(code: &str) -> String {
    let code = code.trim();
    if code.is_empty() {
        "000".to_string()
    } else {
        code.to_string()
    }
}

/// `MM/DD/YYYY` with a real date.
fn parse_mmddyyyy(text: &str) -> Option<chrono::NaiveDate> {
    let parts: Vec<&str> = text.split('/').collect();
    if parts.len() != 3
        || parts[0].len() != 2
        || parts[1].len() != 2
        || parts[2].len() != 4
        || !parts.iter().all(|p| digits_only(p))
    {
        return None;
    }
    chrono::NaiveDate::from_ymd_opt(
        parts[2].parse().ok()?,
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
    )
}

impl Form1707Draft {
    /// `validateAll()` against a given "today" (the page uses the clock).
    pub fn validate_on(&self, today: chrono::NaiveDate) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // checkDatePage1 / dateInPage1Item1 / checkVersion.
        if self.transaction_month == 0 {
            err(
                "transaction_month",
                "Month field on Page 1 Item 1 is required.",
            );
        } else if self.transaction_day == 0 {
            err("transaction_day", "Day field on Page 1 Item 1 is required.");
        } else if self.transaction_year == 0 {
            err(
                "transaction_year",
                "Year field on Page 1 Item 1 is required.",
            );
        } else if !is_calendar_date(
            self.transaction_year,
            self.transaction_month,
            self.transaction_day,
        ) || self.transaction_year < 1800
        {
            err(
                "transaction_day",
                "Please provide a valid date. (MM/DD/YYYY format) in Page 1 Item 1.",
            );
        } else {
            let date = chrono::NaiveDate::from_ymd_opt(
                i32::from(self.transaction_year),
                u32::from(self.transaction_month),
                u32::from(self.transaction_day),
            );
            if date.is_some_and(|d| d > today) {
                err(
                    "transaction_day",
                    "Page 1 Item 1 Date cannot be a future date ",
                );
            } else if date
                .is_some_and(|d| d <= chrono::NaiveDate::from_ymd_opt(2021, 3, 31).unwrap_or(d))
            {
                err(
                    "transaction_year",
                    "Year shall not be greater than the present year and not earlier than April 2021.",
                );
            }
        }

        match self.atc {
            None => err("atc", "ATC Code on Page 1 Item 3 is required."),
            Some(Form1707Atc::CorporationUnspecified) => err(
                "atc",
                "You need to select whether the Corporation is Domestic or Foreign.",
            ),
            Some(_) => {}
        }
        let rdo = self.rdo_code.trim();
        if rdo.is_empty() || rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err(
                "rdo_code",
                "Please enter a valid RDO Code on Page 1 Item 5.",
            );
        }
        if self.taxpayer_name.trim().is_empty() {
            err("taxpayer_name", "Name field on Page 1 Item 6 is required.");
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Registered Address field on Page 1 Item 7 is required.",
            );
        }
        if self.zip_code.trim().is_empty() {
            err("zip_code", "Zip Code field on Page 1 Item 7A is required.");
        }
        if self.contact_number.trim().is_empty() {
            err(
                "contact_number",
                "Contact Number field on Page 1 Item 8 is required.",
            );
        }
        let email = self.email.trim();
        if email.is_empty() {
            err("email", "E-mail address on page 1 item 9 is required.");
        } else if !email.contains('@')
            || email.contains(char::is_whitespace)
            || email.contains('#')
            || email.chars().count() > 50
        {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }

        // checkPage1BuyerAndSeller, then validatePageOneTIN.
        for (label, field, rows) in [
            ("Seller", "sellers", &self.sellers),
            ("Buyer", "buyers", &self.buyers),
        ] {
            if rows.len() > FORM_1707_PARTY_ROWS {
                err(
                    field,
                    &format!("Form 1707 holds {FORM_1707_PARTY_ROWS} {label} rows on Page 1."),
                );
            }
            let first_blank = rows.first().is_none_or(Form1707Party::is_blank);
            if first_blank {
                err(
                    &format!("{field}[0]"),
                    &format!("{label}'s information in Page 1 should have at least one row."),
                );
            }
            for (index, row) in rows.iter().enumerate() {
                let name_or_address_blank =
                    row.name.trim().is_empty() || row.address.trim().is_empty();
                let name_or_address_given =
                    !row.name.trim().is_empty() || !row.address.trim().is_empty();
                let tin_blank = row.tin.trim().is_empty();
                if (name_or_address_blank && !tin_blank) || (name_or_address_given && tin_blank) {
                    err(
                        &format!("{field}[{index}]"),
                        &format!(
                            "Please complete Item #{} in {label}'s information in Page 1.",
                            index + 1
                        ),
                    );
                } else if index > 0 && !row.is_blank() && rows[index - 1].is_blank() {
                    // The page only opens a row once the row above is complete.
                    err(
                        &format!("{field}[{index}]"),
                        &format!(
                            "Please complete Item #{index} in {label}'s information in Page 1."
                        ),
                    );
                }
            }
            for (index, row) in rows.iter().enumerate() {
                let tin = row.tin.trim();
                if !tin.is_empty() && (tin.len() <= 11 || tin.len() > 14 || !digits_only(tin)) {
                    err(
                        &format!("{field}[{index}].tin"),
                        &format!(
                            "Please check TIN number in {label}'s Information Page 1 row #{}.",
                            index + 1
                        ),
                    );
                }
                if row.name.trim().chars().count() > 50 || row.address.trim().chars().count() > 100
                {
                    err(
                        &format!("{field}[{index}]"),
                        &format!("{label} names hold 50 characters and addresses 100."),
                    );
                }
            }
        }

        if let Some(spec) = &self.tax_relief {
            if spec.trim().is_empty() {
                err(
                    "tax_relief",
                    "Specify Tax Relief field on Page 1 Item 8A is required.",
                );
            } else if spec.trim().chars().count() > 100 {
                err("tax_relief", "Item 8A holds at most 100 characters.");
            }
        }
        match self.transaction_type {
            None => err(
                "transaction_type",
                "Description of Transaction on Page 1 Item 9 is required.",
            ),
            Some(Form1707TransactionType::Others) => {
                let spec = self.transaction_description.trim();
                if spec.is_empty() {
                    err(
                        "transaction_description",
                        "Specific description of transaction field on Page 1 Item 9 is required.",
                    );
                } else if spec.chars().count() > 100 {
                    err(
                        "transaction_description",
                        "Item 9 description holds at most 100 characters.",
                    );
                }
            }
            Some(_) => {}
        }

        // checkPartIVSched2Fields / checkPartIVSched3Fields on rows A–D, then
        // CheckEmptyDesc / CheckEmptyDesc2 on the pop-up rows (D.1 onwards).
        let main_shares = if self.shares_popup() {
            FORM_1707_SCHEDULE_ROWS - 1
        } else {
            self.shares.len()
        };
        for (index, row) in self.shares.iter().enumerate() {
            let shares = row.number_of_shares.unwrap_or(0.0);
            if index < main_shares {
                let text =
                    !row.corporation.trim().is_empty() || !row.certificate_number.trim().is_empty();
                let missing_text =
                    row.corporation.trim().is_empty() || row.certificate_number.trim().is_empty();
                let amounts = shares != 0.0 || row.selling_price != 0.0;
                let missing_amount = shares == 0.0 || row.selling_price == 0.0;
                if (missing_text && amounts) || (text && missing_amount) {
                    err(
                        &format!("shares[{index}]"),
                        &format!(
                            "Please complete Item #{} in Part IV Sched 2 Page 2.",
                            index + 1
                        ),
                    );
                }
            } else if row.corporation.trim().is_empty()
                || shares <= 0.0
                || row.certificate_number.trim().is_empty()
                || row.selling_price <= 0.0
            {
                err(
                    &format!("shares[{index}]"),
                    &format!(
                        "Cannot save. You have an empty data in D.{}",
                        index + 2 - FORM_1707_SCHEDULE_ROWS
                    ),
                );
            }
            if row.corporation.trim().chars().count() > 100
                || row.certificate_number.trim().len() > 40
                || !digits_only(row.certificate_number.trim())
            {
                err(
                    &format!("shares[{index}]"),
                    "Corporation names hold 100 characters; certificate numbers up to 40 digits.",
                );
            }
            if shares < 0.0 || !within_round_limit(shares) {
                err(
                    &format!("shares[{index}].number_of_shares"),
                    "Enter a non-negative number of shares below 1,000,000,000,000.",
                );
            }
        }
        let main_expenses = if self.expenses_popup() {
            FORM_1707_SCHEDULE_ROWS - 1
        } else {
            self.expenses.len()
        };
        for (index, row) in self.expenses.iter().enumerate() {
            let blank = row.particulars.trim().is_empty();
            if index < main_expenses {
                if (row.amount == 0.0 && !blank) || (row.amount != 0.0 && blank) {
                    err(
                        &format!("expenses[{index}]"),
                        &format!(
                            "Please complete Item #{} in Part IV Sched 3 Page 2.",
                            index + 1
                        ),
                    );
                }
            } else if blank || row.amount <= 0.0 {
                err(
                    &format!("expenses[{index}]"),
                    &format!(
                        "Cannot save. You have an empty data in D.{}",
                        index + 2 - FORM_1707_SCHEDULE_ROWS
                    ),
                );
            }
            if row.particulars.trim().chars().count() > 100 {
                err(
                    &format!("expenses[{index}]"),
                    "Particulars hold at most 100 characters.",
                );
            }
        }

        // Schedule 1 Item 8 (`validateDate` on blur).
        let date = self.installment.collection_date.trim();
        if !date.is_empty() {
            match parse_mmddyyyy(date) {
                None => err(
                    "installment.collection_date",
                    "Please provide a valid date. (MM/DD/YYYY format)",
                ),
                Some(d) if d > today => err(
                    "installment.collection_date",
                    "This date cannot be a future date.",
                ),
                Some(d) if d.year() < 2018 => err(
                    "installment.collection_date",
                    "This date cannot be prior to 2018.",
                ),
                Some(_) => {}
            }
        }
        let count = self.installment.number_of_installments.trim();
        if count.len() > 3 || !digits_only(count) {
            err(
                "installment.number_of_installments",
                "Schedule 1 Item 4 holds up to three digits.",
            );
        }

        let mut amounts: Vec<(String, f64)> = vec![
            (
                "installment.selling_price".into(),
                self.installment.selling_price,
            ),
            (
                "installment.cost_and_expenses".into(),
                self.installment.cost_and_expenses,
            ),
            (
                "installment.mortgage_assumed".into(),
                self.installment.mortgage_assumed,
            ),
            (
                "installment.installment_amount".into(),
                self.installment.installment_amount,
            ),
            (
                "installment.total_collection".into(),
                self.installment.total_collection,
            ),
            ("tax_paid_previous".into(), self.tax_paid_previous),
            ("surcharge".into(), self.surcharge),
            ("interest".into(), self.interest),
            ("compromise".into(), self.compromise),
        ];
        for (index, row) in self.shares.iter().enumerate() {
            amounts.push((format!("shares[{index}].selling_price"), row.selling_price));
        }
        for (index, row) in self.expenses.iter().enumerate() {
            amounts.push((format!("expenses[{index}].amount"), row.amount));
        }
        for (field, value) in amounts {
            if value < 0.0 || !has_cent_precision(value) || !within_round_limit(value) {
                err(
                    &field,
                    "Enter a non-negative amount in pesos and centavos below 1,000,000,000,000.",
                );
            }
        }
        if !self.is_amended && self.tax_paid_previous != 0.0 {
            err(
                "tax_paid_previous",
                "Item 16 applies only to an amended return.",
            );
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 4 holds at most two digits.",
            );
        }
        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        if !tin_is_well_formed(&self.tin) {
            err("tin", "The taxpayer profile needs a valid TIN.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }

        let mut expected = self.clone();
        expected.recompute();
        if expected != *self {
            err(
                "total_amount_payable",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }
}

impl FormValidator for Form1707Draft {
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1707Draft {
    /// Items 18A-18C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "18A Surcharge",
                interest: "18B Interest",
                compromise: "18C Compromise",
            ],
            controls: [
                "frm1707:txtPg1P2I17Surcharge",
                "frm1707:txtPg1P2I17Interest",
                "frm1707:txtPg1P2I17Compromise",
            ],
            state: penalty,
            basic_tax_due: "Item 17", |d| d.tax_payable,
        )
    }
}

impl QueueableForm for Form1707Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1707";
    /// Official `formType` and SFTP folder (`ftpTargetFolder.PROD['1707v2021']`).
    const FORM_TYPE: &'static str = "1707v2021";
    const LAYOUT_ID: &'static str = FORM_1707_FORM_ID;

    /// Total Amount Payable/(Overpayment).
    fn amount_payable(&self) -> Option<f64> {
        Some(self.total_amount_payable)
    }

    fn lifecycle(&self) -> &SubmissionLifecycle {
        &self.lifecycle
    }
    fn lifecycle_mut(&mut self) -> &mut SubmissionLifecycle {
        &mut self.lifecycle
    }

    /// Item "Amended Return?" = Yes, as the editor sets it.
    fn mark_amended(&mut self) -> bool {
        self.set_amended(true);
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
        self.filing_year
    }
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::OpenEnded(self.open_ended_key)
    }
    /// `MM + DD + YYYY` of the transaction, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        self.date_code()
    }
    /// The filename names the sale date, not the dashboard's open-ended key,
    /// so a receipt cannot be mapped back to a draft row.
    fn parse_period_code(_code: &str) -> Option<(u16, FilingPeriod)> {
        None
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
    /// The "More" pop-ups add rows at run time, so the plaintext follows
    /// [`Form1707Draft::official_layout`] rather than the fixed layout.
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

    fn today() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()
    }

    pub(crate) fn sample() -> Form1707Draft {
        let mut draft = Form1707Draft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            filing_year: 2025,
            open_ended_key: 1,
            transaction_month: 6,
            transaction_day: 30,
            transaction_year: 2025,
            is_amended: false,
            atc: Some(Form1707Atc::Individual),
            number_of_attached_sheets: 0,
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            taxpayer_name: "Sample Seller".to_string(),
            registered_address: "1 Sample St".to_string(),
            zip_code: "1100".to_string(),
            contact_number: "09170000000".to_string(),
            email: "sample.taxpayer@example.com".to_string(),
            line_of_business: "Investor".to_string(),
            sellers: vec![Form1707Party {
                name: "Sample Seller".into(),
                address: "1 Sample St".into(),
                tin: "123456788000".into(),
            }],
            buyers: vec![Form1707Party {
                name: "Sample Buyer".into(),
                address: "2 Sample St".into(),
                tin: "111222333000".into(),
            }],
            tax_relief: None,
            transaction_type: None,
            transaction_description: String::new(),
            installment: Form1707Installment::default(),
            shares: Vec::new(),
            expenses: Vec::new(),
            shares_popup_subtotal: 0.0,
            expenses_popup_subtotal: 0.0,
            total_selling_price: 0.0,
            total_expenses: 0.0,
            taxable_base: 0.0,
            allowable_expenses: 0.0,
            net_capital_gain: 0.0,
            tax_due: 0.0,
            tax_due_is_loss_zero: false,
            tax_due_this_period: 0.0,
            tax_paid_previous: 0.0,
            tax_payable: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_payable: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.set_transaction_type(Form1707TransactionType::CashSale);
        draft.shares = vec![Form1707Shares {
            corporation: "Sample Corp".into(),
            number_of_shares: Some(1000.0),
            certificate_number: "123".into(),
            selling_price: 500_000.005,
        }];
        draft.expenses = vec![Form1707Expense {
            particulars: "Cost of shares".into(),
            amount: 100_000.0,
        }];
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1707Draft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert_eq!(draft.taxable_base, 500_000.01);
        assert_eq!(draft.net_capital_gain, 400_000.01);
        assert_eq!(draft.tax_due, 60_000.0);
        assert_eq!(draft.total_amount_payable, 60_000.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn a_loss_writes_a_bare_zero_and_installments_use_schedule_1() {
        let mut draft = sample();
        draft.expenses[0].amount = 600_000.0;
        draft.recompute();
        assert_eq!(
            draft.to_bir_field_map()["frm1707:txtPg1P2I13TaxDueForTrans"],
            "0"
        );
        draft.set_transaction_type(Form1707TransactionType::InstallmentSale);
        assert!(draft.shares.is_empty());
        draft.installment.installment_amount = 10_000.0;
        draft.recompute();
        assert_eq!(draft.installment.period_tax_due, 1_500.0);
        assert_eq!(draft.tax_due_this_period, 1_500.0);
        assert_eq!(draft.tax_payable, 1_500.0);
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1707:txtPg2P4Sch1I6ApplcRate"], "15");
        assert_eq!(fields["frm1707:txtPg1P2I13AppTaxRate"], "");
    }

    #[test]
    fn penalties_on_an_overpayment_are_payable_alone() {
        let mut draft = sample();
        draft.set_amended(true);
        draft.tax_paid_previous = 70_000.0;
        draft.surcharge = 100.0;
        draft.keep_entered_penalties();
        draft.recompute();
        assert_eq!(draft.tax_payable, -10_000.0);
        assert_eq!(draft.total_amount_payable, 100.0);
    }

    #[test]
    fn field_map_uses_official_formats() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1707:txtPg1I1Month"], "06");
        assert_eq!(fields["frm1707:txtPg2P4Sch2NoOfShares1"], "1,000.000");
        assert_eq!(fields["frm1707:txtPg2P4Sch2NoOfShares2"], "0.00");
        assert_eq!(fields["frm1707:txtPg1P2I13AppTaxRate"], "15");
        assert_eq!(fields["frm1707:ProfileName"], "SAMPLE SELLER");
        assert_eq!(fields["frm1707:txtPg2TinC4"], "00000");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1707v2021-06302025#sample.taxpayer@example.com#.xml"
        );
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1707Draft), expected: &str| {
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
            &|d| d.transaction_month = 0,
            "Month field on Page 1 Item 1 is required.",
        );
        check(
            &|d| d.transaction_day = 0,
            "Day field on Page 1 Item 1 is required.",
        );
        check(
            &|d| d.transaction_year = 0,
            "Year field on Page 1 Item 1 is required.",
        );
        check(
            &|d| d.transaction_day = 31,
            "Please provide a valid date. (MM/DD/YYYY format) in Page 1 Item 1.",
        );
        check(
            &|d| d.transaction_year = 2026,
            "Page 1 Item 1 Date cannot be a future date ",
        );
        check(
            &|d| {
                d.transaction_year = 2021;
                d.transaction_month = 3;
            },
            "Year shall not be greater than the present year and not earlier than April 2021.",
        );
        check(&|d| d.atc = None, "ATC Code on Page 1 Item 3 is required.");
        check(
            &|d| d.atc = Some(Form1707Atc::CorporationUnspecified),
            "You need to select whether the Corporation is Domestic or Foreign.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Page 1 Item 5.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Name field on Page 1 Item 6 is required.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Registered Address field on Page 1 Item 7 is required.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Zip Code field on Page 1 Item 7A is required.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Contact Number field on Page 1 Item 8 is required.",
        );
        check(
            &|d| d.email.clear(),
            "E-mail address on page 1 item 9 is required.",
        );
        check(
            &|d| d.sellers[0].tin.clear(),
            "Please complete Item #1 in Seller's information in Page 1.",
        );
        check(
            &|d| d.buyers.clear(),
            "Buyer's information in Page 1 should have at least one row.",
        );
        check(
            &|d| d.buyers[0].tin = "11122233".into(),
            "Please check TIN number in Buyer's Information Page 1 row #1.",
        );
        check(
            &|d| d.tax_relief = Some(String::new()),
            "Specify Tax Relief field on Page 1 Item 8A is required.",
        );
        check(
            &|d| d.transaction_type = None,
            "Description of Transaction on Page 1 Item 9 is required.",
        );
        check(
            &|d| d.set_transaction_type(Form1707TransactionType::Others),
            "Specific description of transaction field on Page 1 Item 9 is required.",
        );
        check(
            &|d| d.shares[0].certificate_number.clear(),
            "Please complete Item #1 in Part IV Sched 2 Page 2.",
        );
        check(
            &|d| d.expenses[0].amount = 0.0,
            "Please complete Item #1 in Part IV Sched 3 Page 2.",
        );
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft.transaction_year = 2025;
        draft
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        assert!(draft.revalidate_queued_before_submission().is_ok());
        draft.surcharge = 99.0;
        draft.keep_entered_penalties();
        assert!(draft.revalidate_queued_before_submission().is_err());
    }
}
