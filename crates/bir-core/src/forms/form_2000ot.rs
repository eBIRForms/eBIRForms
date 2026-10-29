//! BIR Form 2000-OT (v2018) — Documentary Stamp Tax Declaration/Return
//! (One-Time Transactions).
//!
//! Ported from the official `BIR-Form2000OTv2018.hta` (eBIRForms 7.9.6.2.1):
//! `SelectedATC`, `TaxComputation` with `ComputeSharesOfStock` /
//! `ComputeRealProperty` (Schedules 1.A, 1.B and 2), `validateDate`,
//! `validate()` with its exact alert texts and `saveXMLsubmit` through
//! [`crate::official_xml`].
//!
//! The Validate button runs `validate();capital()`, and a return can only be
//! submitted after it, so every text box except `txtEmail` reaches the
//! submit plaintext in capitals.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::form_2000::{
    RowGroup, amount_in_official_range, cents, digits_only, email_is_plausible, fixed_cents,
    layout_with_rows, split_tin, tin_error,
};
use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::official_amount;
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_2000OT_FORM_ID: &str = "2000ot-v2018";
/// Rows Schedule 1.A (and its continuation) starts with on the official
/// page; "Add" appends a row to both tables.
pub const FORM_2000OT_REAL_PROPERTY_ROWS: usize = 5;
/// Rows Schedule 1.B starts with on the official page.
pub const FORM_2000OT_SHARE_ROWS: usize = 3;
/// Rows a schedule may hold here (the page itself has no limit).
pub const FORM_2000OT_MAX_ROWS: usize = 50;

/// Item 3 — ATC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Form2000OTAtc {
    #[default]
    Unanswered,
    /// DO102: shares of stock with par value, P1.50 / P200.
    Do102,
    /// DO122: deeds of sale, conveyances and donations of real property,
    /// P15 / P1,000.
    Do122,
    /// DO125: stock without par value, 50% of the DST paid on original issue.
    Do125,
}

impl Form2000OTAtc {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unanswered => "",
            Self::Do102 => "DO102",
            Self::Do122 => "DO122",
            Self::Do125 => "DO125",
        }
    }

    /// Item 16, as `SelectedATC` writes it.
    pub fn rate_text(self) -> &'static str {
        match self {
            Self::Unanswered => "0.00",
            Self::Do102 => "P1.50 / P200",
            Self::Do122 => "P15 / P1,000",
            Self::Do125 => "50% DST paid on original issue",
        }
    }

    fn is_shares(self) -> bool {
        matches!(self, Self::Do102 | Self::Do125)
    }
}

/// Item 12 — Nature of transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2000OTNature {
    #[default]
    Unanswered,
    /// Transfer of shares of stock not traded through the local exchange.
    SharesOfStock,
    /// Transfer of real property classified as capital asset.
    RealPropertyCapitalAsset,
    /// Transfer of real property other than capital asset.
    RealPropertyOrdinaryAsset,
}

impl Form2000OTNature {
    pub fn is_real_property(self) -> bool {
        matches!(
            self,
            Self::RealPropertyCapitalAsset | Self::RealPropertyOrdinaryAsset
        )
    }
}

/// Item 11 — other party to the transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form2000OTOtherParty {
    #[default]
    Unanswered,
    /// Creditor/Mortgagor/etc.
    Creditor,
    /// Debtor/Mortgagee/etc.
    Debtor,
}

/// Schedule 1.A row (title and tax declaration) with its continuation row
/// (lot, classification, area and the two fair market values).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2000OTPropertyRow {
    pub title_number: String,
    pub tax_declaration_number: String,
    pub location: String,
    pub lot: String,
    pub classification: String,
    pub area: String,
    /// Column 1: FMV per tax declaration.
    pub fmv_per_td: f64,
    /// Column 2: FMV per BIR zonal value.
    pub fmv_zonal: f64,
    /// Whichever is higher (computed).
    #[serde(default)]
    pub fmv: f64,
}

impl Form2000OTPropertyRow {
    fn is_blank(&self) -> bool {
        self.title_number.trim().is_empty()
            && self.tax_declaration_number.trim().is_empty()
            && self.location.trim().is_empty()
            && self.lot.trim().is_empty()
            && self.classification.trim().is_empty()
            && self.area.trim().is_empty()
            && self.fmv_per_td == 0.0
            && self.fmv_zonal == 0.0
    }
}

/// Schedule 1.B row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form2000OTShareRow {
    pub corporation: String,
    pub shares_sold: String,
    pub certificate_number: String,
    /// (d) Par value of shares (with par value).
    pub par_value: f64,
    /// (e) DST paid on original issue (without par value).
    pub dst_paid_on_issue: f64,
}

impl Form2000OTShareRow {
    fn is_blank(&self) -> bool {
        self.corporation.trim().is_empty()
            && self.shares_sold.trim().is_empty()
            && self.certificate_number.trim().is_empty()
            && self.par_value == 0.0
            && self.dst_paid_on_issue == 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form2000OTDraft {
    #[serde(default)]
    pub id: Option<i64>,

    /// Item 1, `MM/DD/YYYY`.
    pub transaction_date: String,
    /// Dashboard year, used until Item 1 holds a valid date.
    pub year: u16,
    /// Item 2.
    pub is_amended: bool,
    /// Item 3.
    #[serde(default)]
    pub atc: Form2000OTAtc,
    /// Item 4.
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
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
    /// Hidden `txtLineBus`, loaded from the profile.
    #[serde(default)]
    pub line_of_business: String,
    /// Item 11.
    #[serde(default)]
    pub other_party: Form2000OTOtherParty,
    /// Item 11A.
    #[serde(default)]
    pub other_party_name: String,
    /// Item 11B.
    #[serde(default)]
    pub other_party_tin: String,
    /// Item 12.
    #[serde(default)]
    pub nature: Form2000OTNature,
    /// Item 13.
    #[serde(default)]
    pub real_property_location: String,

    // Schedules
    #[serde(default)]
    pub properties: Vec<Form2000OTPropertyRow>,
    #[serde(default)]
    pub shares: Vec<Form2000OTShareRow>,
    /// Schedule 2 A.
    #[serde(default)]
    pub gross_selling_price: f64,
    /// Schedule 2 B (computed: Schedule 1.A total).
    #[serde(default)]
    pub total_fair_market_value: f64,
    /// Schedule 2 C description.
    #[serde(default)]
    pub others_description: String,
    /// Schedule 2 C amount.
    #[serde(default)]
    pub others_amount: f64,
    /// Schedule 2 item 2 (computed).
    #[serde(default)]
    pub real_property_taxable_base: f64,

    // Part II
    /// Item 14.
    #[serde(default)]
    pub shares_taxable_base: f64,
    /// Item 15.
    #[serde(default)]
    pub real_property_base: f64,
    /// Item 17.
    #[serde(default)]
    pub tax_due: f64,
    /// Item 18, only on an amended return.
    #[serde(default)]
    pub tax_paid_previous: f64,
    /// Item 19.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 20A–20D.
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
    /// Item 21.
    #[serde(default)]
    pub total_amount_payable: f64,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `(month, day, year)` of a well-formed `MM/DD/YYYY` date.
pub(crate) fn date_parts(value: &str) -> Option<(u32, u32, i32)> {
    let parts: Vec<&str> = value.trim().split('/').collect();
    if parts.len() != 3
        || parts[0].len() != 2
        || parts[1].len() != 2
        || parts[2].len() != 4
        || !parts.iter().all(|p| digits_only(p))
    {
        return None;
    }
    let month = parts[0].parse().ok()?;
    let day = parts[1].parse().ok()?;
    let year = parts[2].parse().ok()?;
    chrono::NaiveDate::from_ymd_opt(year, month, day).map(|_| (month, day, year))
}

impl Form2000OTDraft {
    pub const FORM_CODE: &'static str = "2000OT";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            transaction_date: String::new(),
            year,
            is_amended: false,
            atc: Form2000OTAtc::Unanswered,
            number_of_attached_sheets: 0,
            tin: profile.tin.full(),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            line_of_business: profile.line_of_business.clone(),
            other_party: Form2000OTOtherParty::Unanswered,
            other_party_name: String::new(),
            other_party_tin: String::new(),
            nature: Form2000OTNature::Unanswered,
            real_property_location: String::new(),
            properties: Vec::new(),
            shares: Vec::new(),
            gross_selling_price: 0.0,
            total_fair_market_value: 0.0,
            others_description: String::new(),
            others_amount: 0.0,
            real_property_taxable_base: 0.0,
            shares_taxable_base: 0.0,
            real_property_base: 0.0,
            tax_due: 0.0,
            tax_paid_previous: 0.0,
            tax_still_due: 0.0,
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

    /// The official compute chain (`TaxComputation`). Amounts are held at
    /// cents the way `round(this,2)` leaves them.
    pub fn recompute(&mut self) {
        if !self.is_amended {
            self.tax_paid_previous = 0.0;
        }
        if !self.nature.is_real_property() {
            // processNatureOfTransaction clears Item 13 for shares.
            self.real_property_location.clear();
        }
        self.gross_selling_price = cents(self.gross_selling_price);
        self.others_amount = cents(self.others_amount);

        // ComputeSharesOfStock: float sums of the formatted columns.
        let mut with_par = 0.0;
        let mut without_par = 0.0;
        for row in &mut self.shares {
            row.par_value = cents(row.par_value);
            row.dst_paid_on_issue = cents(row.dst_paid_on_issue);
            with_par += row.par_value;
            without_par += row.dst_paid_on_issue;
        }
        // ComputeRealProperty: the higher FMV per row, then a float sum.
        let mut fmv_total = 0.0;
        for row in &mut self.properties {
            row.fmv_per_td = cents(row.fmv_per_td);
            row.fmv_zonal = cents(row.fmv_zonal);
            row.fmv = if row.fmv_per_td >= row.fmv_zonal {
                row.fmv_per_td
            } else {
                row.fmv_zonal
            };
            fmv_total += row.fmv;
        }
        self.total_fair_market_value = fixed_cents(fmv_total);

        let shares_base = match self.atc {
            Form2000OTAtc::Do102 => Some(with_par),
            Form2000OTAtc::Do125 => Some(without_par),
            _ => None,
        };
        self.shares_taxable_base = shares_base.map(fixed_cents).unwrap_or(0.0);
        let real_base = (self.atc == Form2000OTAtc::Do122).then_some({
            if self.others_description.is_empty() && self.others_amount == 0.0 {
                if self.gross_selling_price >= self.total_fair_market_value {
                    self.gross_selling_price
                } else {
                    self.total_fair_market_value
                }
            } else {
                self.others_amount
            }
        });
        self.real_property_taxable_base = real_base.map(fixed_cents).unwrap_or(0.0);
        self.real_property_base = self.real_property_taxable_base;

        // Item 17. An ATC that does not match the nature computes NaN, which
        // formatCurrency prints as 0.00.
        self.tax_due = match (self.nature, self.atc) {
            (Form2000OTNature::SharesOfStock, Form2000OTAtc::Do102) => {
                let base = shares_base.unwrap_or(0.0);
                if base != 0.0 {
                    fixed_cents((base / 200.0).floor() * 1.50)
                } else {
                    0.0
                }
            }
            (Form2000OTNature::SharesOfStock, Form2000OTAtc::Do125) => {
                fixed_cents(shares_base.unwrap_or(0.0) * 0.50)
            }
            (nature, Form2000OTAtc::Do122) if nature.is_real_property() => {
                fixed_cents((real_base.unwrap_or(0.0) / 1000.0).round() * 15.0)
            }
            _ => 0.0,
        };
        self.tax_paid_previous = cents(self.tax_paid_previous);
        self.tax_still_due = fixed_cents(self.tax_due - self.tax_paid_previous);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = fixed_cents(self.surcharge + self.interest + self.compromise);
        // Penalties are not offset by an overpayment in Item 19.
        self.total_amount_payable = fixed_cents(self.tax_still_due.max(0.0) + self.total_penalties);
    }

    /// Print preview fields: the upload map plus the combs the sheet prints
    /// as one cell each: the Item 1 date as MM, DD and YYYY, and the Item 5
    /// and page 2 TIN as one 14-slot value (9 digits + 5-digit branch).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let mut put = |key: &str, value: String| {
            if !value.trim().is_empty() {
                fields.insert(format!("derived:{key}"), value);
            }
        };
        let date: Vec<&str> = self.transaction_date.trim().split('/').collect();
        if let [month, day, year] = date.as_slice() {
            put("transaction_mm", month.trim().to_string());
            put("transaction_dd", day.trim().to_string());
            put("transaction_yyyy", year.trim().to_string());
        }
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        if !tin1.is_empty() {
            let tin = format!("{tin1}{tin2}{tin3}{branch}");
            put("tin", tin.clone());
            put("pg2_tin", tin);
        }
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        // validate() is followed by capital() on the Validate button.
        let text = |value: &str| value.trim().to_uppercase();
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2000OT:{key}"), value);
        };
        let flag = |on: bool| on.to_string();

        put(
            "txtTransactionDate",
            self.transaction_date.trim().to_string(),
        );
        put("AmendedRtn_1", flag(self.is_amended));
        put("AmendedRtn_2", flag(!self.is_amended));
        put("ATC_1", flag(self.atc == Form2000OTAtc::Do102));
        put("ATC_2", flag(self.atc == Form2000OTAtc::Do122));
        put("ATC_3", flag(self.atc == Form2000OTAtc::Do125));
        put("txtSheets", self.number_of_attached_sheets.to_string());
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        for (key, value) in [
            ("txtTIN1", &tin1),
            ("txtTIN2", &tin2),
            ("txtTIN3", &tin3),
            ("txtBranchCode", &branch),
            ("txtPg2TIN1", &tin1),
            ("txtPg2TIN2", &tin2),
            ("txtPg2TIN3", &tin3),
            ("txtPg2BranchCode", &branch),
        ] {
            put(key, value.clone());
        }
        put("txtRDOCode", text(&self.rdo_code));
        put("txtTaxpayerName", text(&self.taxpayer_name));
        put("txtPg2TaxpayerName", text(&self.taxpayer_name));
        put("txtAddress", text(&self.registered_address));
        put("txtAddress2", String::new());
        put("txtZipCode", self.zip_code.trim().to_string());
        put("txtTelNum", self.contact_number.trim().to_string());
        put("txtLineBus", text(&self.line_of_business));
        // The page the filer is on; the return is validated from page 1.
        put("txtCurrentPage", "1".to_string());
        put(
            "optParty_1",
            flag(self.other_party == Form2000OTOtherParty::Creditor),
        );
        put(
            "optParty_2",
            flag(self.other_party == Form2000OTOtherParty::Debtor),
        );
        put("txtOtherName", text(&self.other_party_name));
        put("txtOtherName2", String::new());
        put("txtOtherTIN", self.other_party_tin.trim().to_string());
        put(
            "rbTransNature_1",
            flag(self.nature == Form2000OTNature::SharesOfStock),
        );
        put(
            "rbTransNature_2",
            flag(self.nature == Form2000OTNature::RealPropertyCapitalAsset),
        );
        put(
            "rbTransNature_3",
            flag(self.nature == Form2000OTNature::RealPropertyOrdinaryAsset),
        );
        put("txtRealLocation", text(&self.real_property_location));

        put("txtTax14", official_amount(self.shares_taxable_base));
        put("txtTax15", official_amount(self.real_property_base));
        put("txtTax16", text(self.atc.rate_text()));
        put("txtTax17", official_amount(self.tax_due));
        put("txtTax18", official_amount(self.tax_paid_previous));
        put("txtTax19", official_amount(self.tax_still_due));
        put("txtTax20A", official_amount(self.surcharge));
        put("txtTax20B", official_amount(self.interest));
        put("txtTax20C", official_amount(self.compromise));
        put("txtTax20D", official_amount(self.total_penalties));
        put("txtTax21", official_amount(self.total_amount_payable));

        for index in 0..self.properties.len().max(FORM_2000OT_REAL_PROPERTY_ROWS) {
            let row = self.properties.get(index).cloned().unwrap_or_default();
            let mut put = |key: &str, value: String| {
                fields.insert(format!("frm2000OT:sched1A:{key}{index}"), value);
            };
            put("txtOCTNo", text(&row.title_number));
            put("txtTaxDecNo", text(&row.tax_declaration_number));
            put("txtLoc", text(&row.location));
            put("txtLot", text(&row.lot));
            put("txtClassification", text(&row.classification));
            put("txtArea", text(&row.area));
            put("txtFMVCol1", official_amount(row.fmv_per_td));
            put("txtFMVCol2", official_amount(row.fmv_zonal));
            put("txtFMVSubTot", official_amount(row.fmv));
        }
        fields.insert(
            "frm2000OT:sched1A:txtFMVTotal".to_string(),
            official_amount(self.total_fair_market_value),
        );
        for index in 0..self.shares.len().max(FORM_2000OT_SHARE_ROWS) {
            let row = self.shares.get(index).cloned().unwrap_or_default();
            let mut put = |key: &str, value: String| {
                fields.insert(format!("frm2000OT:sched1B:{key}{index}"), value);
            };
            put("txtNameOfCorpStock", text(&row.corporation));
            put("txtNoOfSharesSold", text(&row.shares_sold));
            put("txtStockCertNo", text(&row.certificate_number));
            put("txtParValOfShares", official_amount(row.par_value));
            put("txtDSTPaid", official_amount(row.dst_paid_on_issue));
        }
        let mut put = |key: &str, value: String| {
            fields.insert(format!("frm2000OT:Sched2:{key}"), value);
        };
        put(
            "txtGrossSellingPrice",
            official_amount(self.gross_selling_price),
        );
        put(
            "txtTotalFairMarket",
            official_amount(self.total_fair_market_value),
        );
        put("txtOthers", text(&self.others_description));
        put("txtOthersAmount", official_amount(self.others_amount));
        put(
            "txtTaxableBase",
            official_amount(self.real_property_taxable_base),
        );

        fields.insert("txtEmail".to_string(), self.email.trim().to_string());
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// The official layout with this return's added schedule rows.
    pub fn official_layout(
        &self,
    ) -> Result<crate::official_xml::OfficialLayout, Vec<(String, String)>> {
        let base = crate::official_xml::layout(FORM_2000OT_FORM_ID)
            .map_err(|error| vec![("xml".to_string(), error.to_string())])?;
        let groups = [
            RowGroup {
                prefixes: &[
                    "chkSchedule1ADelete",
                    "frm2000OT:sched1A:txtOCTNo",
                    "frm2000OT:sched1A:txtTaxDecNo",
                    "frm2000OT:sched1A:txtLoc",
                ],
                base_rows: FORM_2000OT_REAL_PROPERTY_ROWS,
                rows: self.properties.len(),
            },
            RowGroup {
                prefixes: &[
                    "chkSchedule1A1Delete",
                    "frm2000OT:sched1A:txtLot",
                    "frm2000OT:sched1A:txtClassification",
                    "frm2000OT:sched1A:txtArea",
                    "frm2000OT:sched1A:txtFMVCol1",
                    "frm2000OT:sched1A:txtFMVCol2",
                    "frm2000OT:sched1A:txtFMVSubTot",
                ],
                base_rows: FORM_2000OT_REAL_PROPERTY_ROWS,
                rows: self.properties.len(),
            },
            RowGroup {
                prefixes: &[
                    "chkSchedule1BDelete",
                    "frm2000OT:sched1B:txtNameOfCorpStock",
                    "frm2000OT:sched1B:txtNoOfSharesSold",
                    "frm2000OT:sched1B:txtStockCertNo",
                    "frm2000OT:sched1B:txtParValOfShares",
                    "frm2000OT:sched1B:txtDSTPaid",
                ],
                base_rows: FORM_2000OT_SHARE_ROWS,
                rows: self.shares.len(),
            },
        ];
        Ok(layout_with_rows(base, &groups))
    }
}

impl FormValidator for Form2000OTDraft {
    /// `validateDate` on Item 1, then `validate()` in order with its alert
    /// texts, then the limits the page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        let date = self.transaction_date.trim();
        if date.is_empty() {
            err("transaction_date", "Please enter a valid date on Item 1.");
        } else {
            match date_parts(date) {
                None => err(
                    "transaction_date",
                    "Please provide a valid date. (MM/DD/YYYY format)",
                ),
                Some((month, day, year)) => {
                    let today = chrono::Local::now().date_naive();
                    let entered = chrono::NaiveDate::from_ymd_opt(year, month, day);
                    if entered.is_some_and(|entered| entered > today) {
                        err("transaction_date", "This date cannot be a future date.");
                    } else if year < 2018 {
                        err("transaction_date", "This date cannot be prior to 2018.");
                    }
                }
            }
        }
        if self.atc == Form2000OTAtc::Unanswered {
            err("atc", "Please choose an ATC on item 3.");
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
                "Please enter a valid Taxpayer Name on Item 7.",
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
                "Please enter Taxpayer's Registered Address on Item 8.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 8A.");
        }
        if self.other_party == Form2000OTOtherParty::Unanswered {
            err(
                "other_party",
                "Please choose one (1) Other Party to the transaction on item 11.",
            );
        }
        let other_name = self.other_party_name.trim();
        if other_name.is_empty() || other_name.chars().count() > 130 {
            err(
                "other_party_name",
                "Please enter a name for item 11A. The name that you will enter corresponds to your choice in item 11.",
            );
        }
        let other_tin = self.other_party_tin.trim();
        if other_tin.is_empty() || other_tin.len() > 14 || !digits_only(other_tin) {
            err(
                "other_party_tin",
                "Please enter TIN for item 11B. The TIN that you will enter is the TIN of your entry in item 11A.",
            );
        }
        if self.nature == Form2000OTNature::Unanswered {
            err(
                "nature",
                "Please choose one (1) Nature of Transaction on item 12.",
            );
        }
        if self.nature.is_real_property() {
            let location = self.real_property_location.trim();
            if location.is_empty() || location.chars().count() > 100 {
                err(
                    "real_property_location",
                    "Please enter the address of the Location of Real Property in item 13.",
                );
            }
        }

        // ── Rules the page leaves implicit, and its typing limits ──
        // An ATC that does not fit Item 12 makes TaxComputation compute NaN
        // (shown as a zero tax due); require a matching pair.
        if self.atc != Form2000OTAtc::Unanswered
            && self.nature != Form2000OTNature::Unanswered
            && self.atc.is_shares() == self.nature.is_real_property()
        {
            err(
                "atc",
                "Item 3 must match Item 12: DO102 or DO125 for shares of stock, DO122 for real property.",
            );
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 4 holds at most two digits.",
            );
        }
        if self.properties.len() > FORM_2000OT_MAX_ROWS {
            err("properties", "Schedule 1.A holds up to 50 rows here.");
        }
        if self.shares.len() > FORM_2000OT_MAX_ROWS {
            err("shares", "Schedule 1.B holds up to 50 rows here.");
        }
        let long = |value: &str, max: usize| value.trim().chars().count() > max;
        for (index, row) in self.properties.iter().enumerate() {
            let label = index + 1;
            if [
                &row.title_number,
                &row.tax_declaration_number,
                &row.location,
                &row.lot,
                &row.classification,
                &row.area,
            ]
            .iter()
            .any(|value| long(value, 50))
            {
                err(
                    &format!("properties[{index}].location"),
                    &format!("Schedule 1.A row {label}: entries take up to 50 characters."),
                );
            }
            for value in [row.fmv_per_td, row.fmv_zonal] {
                if value < 0.0 || !has_cent_precision(value) || value.abs() >= 1e13 {
                    err(
                        &format!("properties[{index}].fmv_per_td"),
                        &format!(
                            "Schedule 1.A row {label}: enter fair market values in pesos and centavos."
                        ),
                    );
                    break;
                }
            }
        }
        for (index, row) in self.shares.iter().enumerate() {
            let label = index + 1;
            if [&row.corporation, &row.shares_sold, &row.certificate_number]
                .iter()
                .any(|value| long(value, 50))
            {
                err(
                    &format!("shares[{index}].corporation"),
                    &format!("Schedule 1.B row {label}: entries take up to 50 characters."),
                );
            }
            for value in [row.par_value, row.dst_paid_on_issue] {
                if value < 0.0 || !has_cent_precision(value) || !amount_in_official_range(value) {
                    err(
                        &format!("shares[{index}].par_value"),
                        &format!("Schedule 1.B row {label}: enter amounts in pesos and centavos."),
                    );
                    break;
                }
            }
        }
        if self.atc.is_shares() && self.shares.iter().all(Form2000OTShareRow::is_blank) {
            err("shares", "Enter the shares of stock sold in Schedule 1.B.");
        }
        if self.atc == Form2000OTAtc::Do122 {
            if self.properties.iter().all(Form2000OTPropertyRow::is_blank) {
                err(
                    "properties",
                    "Enter the real property sold in Schedule 1.A.",
                );
            }
            if long(&self.others_description, 25) {
                err(
                    "others_description",
                    "Schedule 2 C takes up to 25 characters.",
                );
            }
            if self.others_description.trim().is_empty() != (self.others_amount == 0.0) {
                err(
                    "others_amount",
                    "Schedule 2 C needs both a description and an amount.",
                );
            }
        }
        for (field, value) in [
            ("gross_selling_price", self.gross_selling_price),
            ("others_amount", self.others_amount),
            ("tax_paid_previous", self.tax_paid_previous),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ] {
            if value < 0.0 || !has_cent_precision(value) || !amount_in_official_range(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }
        if !self.is_amended && self.tax_paid_previous != 0.0 {
            err(
                "tax_paid_previous",
                "Item 18 applies only to an amended return.",
            );
        }
        if self.tax_due <= 0.0 && self.atc != Form2000OTAtc::Unanswered {
            err(
                "tax_due",
                "Item 17 tax due is zero; check Schedules 1 and 2.",
            );
        }
        if !email_is_plausible(&self.email) {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
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

impl Form2000OTDraft {
    /// Items 20A-20C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                surcharge: "20A Surcharge",
                interest: "20B Interest",
                compromise: "20C Compromise",
            ],
            controls: [
                "frm2000OT:txtTax20A",
                "frm2000OT:txtTax20B",
                "frm2000OT:txtTax20C",
            ],
            state: penalty,
            basic_tax_due: "Item 19", |d| d.tax_still_due,
        )
    }
}

impl QueueableForm for Form2000OTDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "2000OT";
    /// Official `formType` and PROD SFTP folder (`ftpTargetFolder.PROD['2000OTv2018']`).
    const FORM_TYPE: &'static str = "2000OTv2018";
    const LAYOUT_ID: &'static str = FORM_2000OT_FORM_ID;

    /// Total Amount Payable.
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
    /// The transaction's year once Item 1 holds a date.
    fn taxable_year(&self) -> u16 {
        date_parts(&self.transaction_date)
            .and_then(|(_, _, year)| u16::try_from(year).ok())
            .unwrap_or(self.year)
    }
    /// One return per transaction date: `MMDD` keys the draft in its year.
    fn filing_period(&self) -> FilingPeriod {
        let key = date_parts(&self.transaction_date)
            .map(|(month, day, _)| month * 100 + day)
            .unwrap_or(0);
        FilingPeriod::OpenEnded(key)
    }
    /// `txtTransactionDate` without its slashes, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        self.transaction_date.trim().replace('/', "")
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 8 || !digits_only(code) {
            return None;
        }
        let (month, day, year) =
            date_parts(&format!("{}/{}/{}", &code[..2], &code[2..4], &code[4..]))?;
        Some((
            u16::try_from(year).ok()?,
            FilingPeriod::OpenEnded(month * 100 + day),
        ))
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

    /// The layout writer over the layout with any added schedule rows.
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as QueueableForm>::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let layout = self.official_layout()?;
        crate::official_xml::write(&layout, &self.field_map())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_map_splits_the_date_and_joins_the_tin() {
        let fields = sample().to_print_field_map();
        assert_eq!(fields["derived:transaction_mm"], "06");
        assert_eq!(fields["derived:transaction_dd"], "15");
        assert_eq!(fields["derived:transaction_yyyy"], "2025");
        assert_eq!(fields["derived:tin"], "12345678800000");
        assert_eq!(fields["derived:pg2_tin"], "12345678800000");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
    }

    /// Item 17 follows the official v2018 page: DO102 floors per P200 block
    /// and DO122 Math.round()s per P1,000 block (BIR-Form2000OTv2018.hta).
    #[test]
    fn tax_due_matches_the_official_block_formulas() {
        let mut draft = sample();
        draft.nature = Form2000OTNature::SharesOfStock;
        draft.atc = Form2000OTAtc::Do102;
        draft.shares = vec![Form2000OTShareRow {
            par_value: 250.0,
            ..Default::default()
        }];
        draft.recompute();
        assert_eq!(draft.tax_due, 1.50);
    }

    pub(crate) fn sample() -> Form2000OTDraft {
        let mut draft = Form2000OTDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            transaction_date: "06/15/2025".into(),
            year: 2025,
            is_amended: false,
            atc: Form2000OTAtc::Do102,
            number_of_attached_sheets: 0,
            tin: "12345678800000".into(),
            rdo_code: "039".into(),
            taxpayer_name: "Sample Seller Inc".into(),
            registered_address: "123 Sample St Quezon City".into(),
            zip_code: "1100".into(),
            contact_number: "0281234567".into(),
            email: "sample.taxpayer@example.com".into(),
            line_of_business: "Holding".into(),
            other_party: Form2000OTOtherParty::Debtor,
            other_party_name: "Sample Buyer Corp".into(),
            other_party_tin: "987654321".into(),
            nature: Form2000OTNature::SharesOfStock,
            real_property_location: String::new(),
            properties: Vec::new(),
            shares: vec![Form2000OTShareRow {
                corporation: "Sample Corp".into(),
                shares_sold: "1000".into(),
                certificate_number: "C-1".into(),
                par_value: 100_399.995,
                dst_paid_on_issue: 0.0,
            }],
            gross_selling_price: 0.0,
            total_fair_market_value: 0.0,
            others_description: String::new(),
            others_amount: 0.0,
            real_property_taxable_base: 0.0,
            shares_taxable_base: 0.0,
            real_property_base: 0.0,
            tax_due: 0.0,
            tax_paid_previous: 0.0,
            tax_still_due: 0.0,
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

    fn messages(draft: &Form2000OTDraft) -> Vec<String> {
        <Form2000OTDraft as FormValidator>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn shares_with_par_value_follow_tax_computation() {
        let draft = sample();
        assert_eq!(draft.shares[0].par_value, 100_400.0);
        assert_eq!(draft.shares_taxable_base, 100_400.0);
        // floor(100,400 / 200) = 502 → × 1.50
        assert_eq!(draft.tax_due, 753.0);
        assert_eq!(draft.total_amount_payable, 753.0);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn stock_without_par_value_is_half_the_dst_paid() {
        let mut draft = sample();
        draft.atc = Form2000OTAtc::Do125;
        draft.shares[0].dst_paid_on_issue = 1_000.01;
        draft.recompute();
        assert_eq!(draft.shares_taxable_base, 1_000.01);
        // 500.005 is 500.00499999… in binary, so toFixed(2) gives 500.00.
        assert_eq!(draft.tax_due, 500.0);
        assert_eq!(
            draft.to_bir_field_map()["frm2000OT:txtTax16"],
            "50% DST PAID ON ORIGINAL ISSUE"
        );
    }

    #[test]
    fn real_property_uses_the_higher_value_and_rounds_per_thousand() {
        let mut draft = sample();
        draft.atc = Form2000OTAtc::Do122;
        draft.nature = Form2000OTNature::RealPropertyCapitalAsset;
        draft.real_property_location = "Lot 1 Sample Subd".into();
        draft.shares.clear();
        draft.properties = vec![Form2000OTPropertyRow {
            title_number: "TCT-1".into(),
            location: "Quezon City".into(),
            fmv_per_td: 1_000_000.0,
            fmv_zonal: 2_500_499.99,
            ..Default::default()
        }];
        draft.gross_selling_price = 2_000_000.0;
        draft.recompute();
        assert_eq!(draft.properties[0].fmv, 2_500_499.99);
        assert_eq!(draft.total_fair_market_value, 2_500_499.99);
        assert_eq!(draft.real_property_base, 2_500_499.99);
        assert_eq!(draft.tax_due, 37_500.0); // round(2500.49999) × 15
        draft.others_description = "Net gift".into();
        draft.others_amount = 3_000_500.0;
        draft.recompute();
        assert_eq!(draft.tax_due, 45_015.0); // round(3000.5) = 3001
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn overpayment_does_not_offset_penalties() {
        let mut draft = sample();
        draft.is_amended = true;
        draft.tax_paid_previous = 1_000.0;
        draft.surcharge = 25.0;
        draft.keep_entered_penalties();
        draft.recompute();
        assert_eq!(draft.tax_still_due, -247.0);
        assert_eq!(draft.total_amount_payable, 25.0);
    }

    #[test]
    fn field_map_is_capitalized_and_filename_uses_the_date() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm2000OT:txtTaxpayerName"], "SAMPLE SELLER INC");
        assert_eq!(
            fields["frm2000OT:sched1B:txtNameOfCorpStock0"],
            "SAMPLE CORP"
        );
        assert_eq!(fields["frm2000OT:sched1B:txtParValOfShares0"], "100,400.00");
        assert_eq!(fields["frm2000OT:txtTax16"], "P1.50 / P200");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-2000OTv2018-06152025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "06152025");
        assert_eq!(draft.filing_period(), FilingPeriod::OpenEnded(615));
        assert_eq!(
            Form2000OTDraft::parse_period_code("06152025"),
            Some((2025, FilingPeriod::OpenEnded(615)))
        );
        assert_eq!(Form2000OTDraft::parse_period_code("02302025"), None);
        assert_eq!(Form2000OTDraft::parse_period_code("062025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form2000OTDraft), expected: &str| {
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
            &|d| d.transaction_date.clear(),
            "Please enter a valid date on Item 1.",
        );
        check(
            &|d| d.transaction_date = "6/15/2025".into(),
            "Please provide a valid date. (MM/DD/YYYY format)",
        );
        check(
            &|d| d.transaction_date = "12/31/2017".into(),
            "This date cannot be prior to 2018.",
        );
        check(
            &|d| d.transaction_date = "01/01/2999".into(),
            "This date cannot be a future date.",
        );
        check(
            &|d| d.atc = Form2000OTAtc::Unanswered,
            "Please choose an ATC on item 3.",
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
            "Please enter a valid Taxpayer Name on Item 7.",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please enter a valid Telephone Number on Item 9.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 8.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 8A.",
        );
        check(
            &|d| d.other_party = Form2000OTOtherParty::Unanswered,
            "Please choose one (1) Other Party to the transaction on item 11.",
        );
        check(
            &|d| d.other_party_name.clear(),
            "Please enter a name for item 11A. The name that you will enter corresponds to your choice in item 11.",
        );
        check(
            &|d| d.other_party_tin.clear(),
            "Please enter TIN for item 11B. The TIN that you will enter is the TIN of your entry in item 11A.",
        );
        check(
            &|d| d.nature = Form2000OTNature::Unanswered,
            "Please choose one (1) Nature of Transaction on item 12.",
        );
        check(
            &|d| {
                d.atc = Form2000OTAtc::Do122;
                d.nature = Form2000OTNature::RealPropertyOrdinaryAsset;
            },
            "Please enter the address of the Location of Real Property in item 13.",
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
            <Form2000OTDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form2000OTDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
