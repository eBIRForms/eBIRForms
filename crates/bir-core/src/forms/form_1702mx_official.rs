//! Form 1702-MX (January 2018, ENCS) on the generic submission path.
//!
//! Ported from the official `BIR-Form1702MXv2018C.hta` (eBIRForms 7.9.6.2.1):
//! the compute chain behind every disabled amount (Schedules 2 to 10 and
//! Part II), the NOLCO and MCIT schedules the page fills on its own, the
//! ATC / date-of-incorporation rules, `validate()` with its alert texts, and
//! `saveXMLsubmit` through [`crate::official_xml`] (numbertext amounts with
//! the official numbertext normalization).
//!
//! Every amount is a whole peso, as on the official page. The four base
//! pages and Schedule 6 Item 4's "(Add more...)" rows are supported; the
//! mandatory attachments (Instruction B, one copy per exempt or special-rate
//! activity) and the other "add more" modals are cloned into the official
//! form at run time, so a return that needs them does not validate here.

use super::penalty_items::PenaltyItems;
use std::collections::BTreeMap;

use super::form_1702mx::{
    Form1702MXDeductionMethod, Form1702MXDraft, Form1702MXFilingBasis, Form1702MXNolcoTable,
    Form1702MXOverpaymentDisposition, Form1702MXRegimeAmounts, Form1702MXSchedule6, PercentInput,
    WholePeso, WholePesoInput,
};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};

/// Rule-package id of the official layout.
pub const FORM_1702MX_LAYOUT_ID: &str = "1702mx-v2018c";
/// Official `formType` and PROD SFTP folder (`ftpTargetFolder.PROD['1702MXv2018C']`).
pub const FORM_1702MX_FORM_TYPE: &str = "1702MXv2018C";

/// Item 5 alternative ATCs (`drpPg1I5ATCR2` option values, page order).
pub const FORM_1702MX_ATC_OPTIONS: &[(&str, &str)] = &[
    ("IC010", "IC 010 - In General (domestic corporation)"),
    ("IC011", "IC 011 - Exempt Corporation"),
    ("IC020", "IC 020 - Taxable Partnership"),
    ("IC021", "IC 021 - General Professional Partnership (GPP)"),
    ("IC030", "IC 030 - Proprietary Educational Institutions"),
    ("IC031", "IC 031 - Non-Stock, Non-Profit Hospitals"),
    (
        "IC040",
        "IC 040 - Government Owned and Controlled Corporations (GOCC), Agencies & Instrumentalities",
    ),
    (
        "IC041",
        "IC 041 - National Government and Local Government Units (LGU)",
    ),
    (
        "IC070",
        "IC 070 - In General (resident foreign corporation)",
    ),
    ("IC080", "IC 080 - International Carriers"),
    ("IC101", "IC 101 - Regional Operating Headquarters"),
    ("IC190", "IC 190 - Offshore Banking Units (OBU's)"),
    ("IC191", "IC 191 - Foreign Currency Deposit Units (FCDU)"),
];

const DESCRIPTION_ALERT: &str =
    "(Amount cannot be zero [0] if Description is not empty and vice versa).";

fn v(input: &WholePesoInput) -> i64 {
    input.value_or_zero().0
}

fn set(input: &mut WholePesoInput, value: i64) {
    input.set(WholePeso(value));
}

/// `NegativeValue(formatCurrencyWOC(x))`: whole pesos with thousands
/// separators, negatives in parentheses.
pub fn official_display(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if value < 0 {
        format!("({grouped})")
    } else {
        grouped
    }
}

/// `Math.round(amount * rate / 100)` floored at zero (`product1`).
fn percent_of(amount: i64, rate: &PercentInput) -> i64 {
    let rate = f64::from(rate.hundredths.unwrap_or(0)) / 100.0;
    let product = amount as f64 * (rate / 100.0);
    if product < 0.0 {
        0
    } else {
        (product + 0.5).floor() as i64
    }
}

fn row_total(row: &mut Form1702MXRegimeAmounts) {
    let total = v(&row.exempt) + v(&row.special) + v(&row.regular);
    set(&mut row.total, total);
}

fn column_sums(rows: &[Form1702MXRegimeAmounts], into: &mut Form1702MXRegimeAmounts) {
    for column in 0..4 {
        let total = rows.iter().map(|row| v(column_ref(row, column))).sum();
        set(column_mut(into, column), total);
    }
}

impl Form1702MXSchedule6 {
    /// `summationmodalTablePg3Sc6`: the modal's column subtotals
    /// (`txtPg6Sc6_SubTotalCA` to `CD`).
    pub fn modal_subtotal(&self) -> Form1702MXRegimeAmounts {
        let rows: Vec<Form1702MXRegimeAmounts> = self
            .more_rows
            .iter()
            .map(|row| row.amounts.clone())
            .collect();
        let mut subtotal = Form1702MXRegimeAmounts::default();
        column_sums(&rows, &mut subtotal);
        subtotal
    }

    /// The modal's controls in DOM order, keyed by element id: rows 4.1,
    /// 4.2, ... (`txtPg6Sc6_<n>Col1` to `Col6`), written while Item 4 is
    /// folded.
    pub fn modal_controls(&self) -> Vec<(String, String)> {
        if !self.is_folded() {
            return Vec::new();
        }
        let mut controls = Vec::new();
        for (index, row) in self.more_rows.iter().enumerate() {
            let n = index + 1;
            controls.push((
                format!("frm1702MX:txtPg6Sc6_{n}Col1"),
                row.description.trim().to_string(),
            ));
            controls.push((
                format!("frm1702MX:txtPg6Sc6_{n}Col2"),
                row.legal_basis.trim().to_string(),
            ));
            for column in 0..4 {
                controls.push((
                    format!("frm1702MX:txtPg6Sc6_{n}Col{}", column + 3),
                    v(column_ref(&row.amounts, column)).to_string(),
                ));
            }
        }
        controls
    }
}

fn column_ref(row: &Form1702MXRegimeAmounts, column: usize) -> &WholePesoInput {
    match column {
        0 => &row.exempt,
        1 => &row.special,
        2 => &row.regular,
        _ => &row.total,
    }
}

fn column_mut(row: &mut Form1702MXRegimeAmounts, column: usize) -> &mut WholePesoInput {
    match column {
        0 => &mut row.exempt,
        1 => &mut row.special,
        2 => &mut row.regular,
        _ => &mut row.total,
    }
}

fn copy_row(from: &Form1702MXRegimeAmounts, to: &mut Form1702MXRegimeAmounts) {
    for column in 0..4 {
        set(column_mut(to, column), v(column_ref(from, column)));
    }
}

fn parse_mm_dd_yyyy(value: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(value.trim(), "%m/%d/%Y").ok()
}

/// `computeNolco` / `page4Item7GetValue`: when deductions exceed gross
/// income the page writes the net operating loss as this year's NOLCO row.
fn apply_nolco(
    table: &mut Form1702MXNolcoTable,
    computation: &mut super::form_1702mx::Form1702MXNolcoComputation,
    gross_income: i64,
    deductions: i64,
    has_loss: bool,
    filing_year: u16,
) {
    let row = &mut table.rows[3];
    if has_loss {
        set(&mut computation.item_1_gross_income, gross_income);
        set(
            &mut computation.item_2_ordinary_itemized_deductions,
            deductions,
        );
        let loss = gross_income - deductions;
        set(&mut computation.item_3_net_operating_loss, loss);
        row.year_incurred = filing_year.to_string();
        set(&mut row.amount, loss.abs());
        set(&mut row.applied_previous_years, 0);
        set(&mut row.expired, 0);
        set(&mut row.applied_current_year, 0);
    } else {
        set(&mut computation.item_1_gross_income, 0);
        set(&mut computation.item_2_ordinary_itemized_deductions, 0);
        set(&mut computation.item_3_net_operating_loss, 0);
        row.year_incurred = " ".to_string();
        set(&mut row.amount, 0);
        set(&mut row.applied_current_year, 0);
    }
}

fn recompute_nolco_rows(table: &mut Form1702MXNolcoTable) {
    for row in &mut table.rows {
        let used = v(&row.applied_previous_years) + v(&row.expired) + v(&row.applied_current_year);
        let unapplied = v(&row.amount) - used;
        set(&mut row.unapplied, unapplied);
    }
    let total = table
        .rows
        .iter()
        .map(|row| v(&row.applied_current_year))
        .sum();
    set(&mut table.item_8_total_applied_current_year, total);
}

impl Form1702MXDraft {
    /// `2000 + txtPg1I2YearEnd`: the four-digit filing year.
    fn filing_year(&self) -> u16 {
        2000 + self.taxable_year % 100
    }

    /// Years between incorporation and Item 2 (`checkDateOfIncorporation`).
    fn incorporation_years(&self) -> Option<i32> {
        let date = parse_mm_dd_yyyy(&self.incorporation_date)?;
        Some(i32::from(self.filing_year()) - chrono::Datelike::year(&date))
    }

    /// The official compute chain, in the order the page settles it.
    pub(super) fn official_recompute(&mut self) {
        self.calculation_issues.clear();
        if self.filing_basis == Form1702MXFilingBasis::Calendar && !self.is_short_period {
            self.month = 12;
        }
        let itemized = self.deduction_method == Form1702MXDeductionMethod::Itemized;
        let optional = self.deduction_method == Form1702MXDeductionMethod::OptionalStandard;

        // checkDateOfIncorporation: IC 055 (MCIT) follows the 4-year rule.
        if let Some(years) = self.incorporation_years() {
            if years >= 4 {
                self.atc.mcit_selected = true;
                self.atc.other_selected = false;
            } else if self.atc.mcit_selected {
                self.atc.mcit_selected = false;
                self.atc.other_selected = true;
            }
        }
        if self.atc.other_code.trim().is_empty() {
            self.atc.other_code = "IC010".to_string();
        }
        let mcit = self.atc.mcit_selected;

        // methodOfDeductionsOptional clears every column-C itemized deduction.
        if optional {
            for row in self.schedule_5.amounts.iter_mut().take(25) {
                set(&mut row.regular, 0);
            }
            for row in self
                .schedule_6
                .rows
                .iter_mut()
                .chain(self.schedule_6.more_rows.iter_mut())
            {
                set(&mut row.amounts.regular, 0);
            }
            for row in self.schedule_7_1.rows.iter_mut().take(3) {
                row.year_incurred.clear();
                for cell in [
                    &mut row.amount,
                    &mut row.applied_previous_years,
                    &mut row.expired,
                    &mut row.applied_current_year,
                ] {
                    set(cell, 0);
                }
            }
        }
        if !self.is_amended {
            for column in 0..3 {
                set(
                    column_mut(&mut self.schedule_3.items_20_to_33[7], column),
                    0,
                );
            }
        }

        // Schedule 5 (itemized deductions) and Schedule 6 (special deductions).
        for row in self.schedule_5.amounts.iter_mut().take(25) {
            row_total(row);
        }
        let (body, total) = self.schedule_5.amounts.split_at_mut(25);
        column_sums(body, &mut total[0]);
        // Item 4 "(Add more...)": each modal row's column D, then Item 4
        // reads OTHERS and the modal subtotals.
        for row in &mut self.schedule_6.more_rows {
            row_total(&mut row.amounts);
        }
        if self.schedule_6.is_folded() {
            let modal = self.schedule_6.modal_subtotal();
            let item_4 = &mut self.schedule_6.rows[3];
            item_4.description = "OTHERS".to_string();
            item_4.legal_basis = "OTHERS".to_string();
            item_4.amounts = modal;
        }
        for row in &mut self.schedule_6.rows {
            row_total(&mut row.amounts);
        }
        let rows: Vec<Form1702MXRegimeAmounts> = self
            .schedule_6
            .rows
            .iter()
            .map(|row| row.amounts.clone())
            .collect();
        column_sums(&rows, &mut self.schedule_6.item_5_total);

        // Schedule 2 Items 1-9.
        let s5_total = self.schedule_5.amounts[25].clone();
        let s6_total = self.schedule_6.item_5_total.clone();
        let s2 = &mut self.schedule_2.items;
        for index in [0, 1, 3, 5] {
            row_total(&mut s2[index]);
        }
        for column in 0..3 {
            let item3 = v(column_ref(&s2[0], column)) - v(column_ref(&s2[1], column));
            set(column_mut(&mut s2[2], column), item3);
            let item5 = item3 - v(column_ref(&s2[3], column));
            set(column_mut(&mut s2[4], column), item5);
            let item7 = item5 + v(column_ref(&s2[5], column));
            set(column_mut(&mut s2[6], column), item7);
        }
        for index in [2, 4, 6] {
            row_total(&mut s2[index]);
        }
        copy_row(&s5_total, &mut s2[7]);
        copy_row(&s6_total, &mut s2[8]);
        row_total(&mut s2[8]);

        // Schedules 7 / 8: the current year's NOLCO row.
        let item7 = s2[6].clone();
        let filing_year = 2000 + self.taxable_year % 100;
        let special_loss = v(&item7.special) < v(&s5_total.special);
        apply_nolco(
            &mut self.schedule_8_1,
            &mut self.special_nolco,
            v(&item7.special),
            v(&s5_total.special),
            special_loss,
            filing_year,
        );
        let regular_loss = v(&s5_total.regular) > v(&item7.regular);
        apply_nolco(
            &mut self.schedule_7_1,
            &mut self.regular_nolco,
            v(&item7.regular),
            v(&s5_total.regular),
            regular_loss,
            filing_year,
        );
        recompute_nolco_rows(&mut self.schedule_7_1);
        recompute_nolco_rows(&mut self.schedule_8_1);
        let nolco_regular = v(&self.schedule_7_1.item_8_total_applied_current_year);
        let nolco_special = v(&self.schedule_8_1.item_8_total_applied_current_year);

        // Schedule 2 Items 10-19.
        let s2 = &mut self.schedule_2.items;
        set(&mut s2[9].exempt, 0);
        set(&mut s2[9].special, nolco_special);
        set(&mut s2[9].regular, nolco_regular);
        row_total(&mut s2[9]);
        let item11a = v(&s2[7].exempt) + v(&s2[8].exempt);
        let item11b = v(&s2[7].special) + v(&s2[8].special) + v(&s2[9].special);
        let item11c = if itemized {
            v(&s2[7].regular) + v(&s2[8].regular) + v(&s2[9].regular)
        } else {
            0
        };
        set(&mut s2[10].exempt, item11a);
        set(&mut s2[10].special, item11b);
        set(&mut s2[10].regular, item11c);
        row_total(&mut s2[10]);
        let item12c = if optional {
            let product = v(&s2[6].regular) as f64 * 0.4;
            if product < 0.0 {
                0
            } else {
                (product + 0.5).floor() as i64
            }
        } else {
            0
        };
        set(&mut s2[11].exempt, 0);
        set(&mut s2[11].special, 0);
        set(&mut s2[11].regular, item12c);
        set(&mut s2[11].total, item12c);
        let item13a = v(&s2[6].exempt) - item11a;
        let item13b = v(&s2[6].special) - item11b;
        set(&mut s2[12].exempt, item13a);
        set(&mut s2[12].special, item13b);
        let item13c = if itemized {
            v(&s2[6].regular) - item11c
        } else if optional {
            v(&s2[6].regular) - item12c
        } else {
            0
        };
        set(&mut s2[12].regular, item13c);
        row_total(&mut s2[12]);

        // Item 15: special rate on gross income or on Item 7 (computePg2Sc2It15).
        let item1b = v(&s2[0].special);
        let item3b = v(&s2[2].special);
        let item7b = v(&s2[6].special);
        let base_b = if item3b >= 0 && item7b >= 0 && item3b >= item7b {
            item1b
        } else {
            item7b
        };
        let item15b = percent_of(base_b, &self.schedule_2.item_14_special_rate);
        let item15c = percent_of(item13c, &self.schedule_2.item_14_regular_rate);
        set(&mut s2[14].exempt, 0);
        set(&mut s2[14].special, item15b);
        set(&mut s2[14].regular, item15c);
        set(&mut s2[14].total, item15b + item15c);
        set(&mut s2[15].exempt, 0);
        set(&mut s2[15].regular, 0);
        let item16b = v(&s2[15].special);
        set(&mut s2[15].total, item16b);
        let item17b = item15b - item16b;
        set(&mut s2[16].exempt, 0);
        set(&mut s2[16].special, item17b);
        set(&mut s2[16].regular, item15c);
        set(&mut s2[16].total, item17b + item15c);
        if !mcit {
            set(&mut s2[17].regular, 0);
        }
        set(&mut s2[17].exempt, 0);
        set(&mut s2[17].special, 0);
        let item18c = v(&s2[17].regular);
        set(&mut s2[17].total, item18c);
        let item19c = if item15c > item18c { item15c } else { item18c };
        set(&mut s2[18].exempt, 0);
        set(&mut s2[18].special, item17b);
        set(&mut s2[18].regular, item19c);
        set(&mut s2[18].total, item17b + item19c);
        let item19 = s2[18].clone();
        let item13 = s2[12].clone();

        // Schedule 9 (MCIT carried over); enableMCITFields clears it without IC 055.
        let s9 = &mut self.schedule_9;
        if !mcit {
            for row in &mut s9.rows {
                row.year.clear();
                for cell in [
                    &mut row.normal_income_tax,
                    &mut row.mcit,
                    &mut row.excess_mcit,
                    &mut row.applied_previous_years,
                    &mut row.expired,
                    &mut row.applied_current_year,
                    &mut row.balance,
                ] {
                    set(cell, 0);
                }
            }
        }
        for row in &mut s9.rows {
            let excess = (v(&row.mcit) - v(&row.normal_income_tax)).max(0);
            set(&mut row.excess_mcit, excess);
            let balance = excess
                - (v(&row.applied_previous_years) + v(&row.expired) + v(&row.applied_current_year));
            set(&mut row.balance, balance);
        }
        let mcit_applied: i64 = s9.rows.iter().map(|row| v(&row.applied_current_year)).sum();
        set(&mut s9.item_4_total_applied_current_year, mcit_applied);

        // Schedule 3 (tax credits).
        let s3 = &mut self.schedule_3.items_20_to_33;
        for index in [1, 3] {
            set(&mut s3[index].exempt, 0);
            set(&mut s3[index].special, 0);
        }
        set(&mut s3[3].regular, mcit_applied);
        for row in s3.iter_mut().take(12) {
            row_total(row);
        }
        let (body, rest) = s3.split_at_mut(12);
        column_sums(body, &mut rest[0]);
        let credits = rest[0].clone();
        set(&mut rest[1].exempt, 0);
        set(
            &mut rest[1].special,
            v(&item19.special) - v(&credits.special),
        );
        set(
            &mut rest[1].regular,
            v(&item19.regular) - v(&credits.regular),
        );
        set(&mut rest[1].total, v(&item19.total) - v(&credits.total));
        let item29 = body[9].clone();

        // Schedule 4 (tax relief availment).
        let s4 = &mut self.schedule_4.items;
        set(&mut s4[0].regular, 0);
        let item1d = v(&s4[0].exempt) + v(&s4[0].special);
        set(&mut s4[0].total, item1d);
        row_total(&mut s4[1]);
        let item3 = [
            v(&s4[0].exempt) + v(&s4[1].exempt),
            v(&s4[0].special) + v(&s4[1].special),
            v(&s4[1].regular),
        ];
        set(&mut s4[2].exempt, item3[0]);
        set(&mut s4[2].special, item3[1]);
        set(&mut s4[2].regular, item3[2]);
        row_total(&mut s4[2]);
        set(&mut s4[3].exempt, 0);
        set(&mut s4[3].special, item15b);
        set(&mut s4[3].regular, 0);
        set(&mut s4[3].total, item15b);
        set(&mut s4[4].exempt, item3[0]);
        set(&mut s4[4].special, item3[1] - item15b);
        set(&mut s4[4].regular, item3[2]);
        row_total(&mut s4[4]);
        copy_row(&item29, &mut s4[5]);
        for column in 0..3 {
            let value = v(column_ref(&s4[4], column)) + v(column_ref(&s4[5], column));
            set(column_mut(&mut s4[6], column), value);
        }
        row_total(&mut s4[6]);

        // Schedule 10 (reconciliation); Item 10 is printed as Schedule 2 Item 10.
        let s10 = &mut self.schedule_10.items;
        for index in [0, 1, 2, 4, 5, 6, 7] {
            row_total(&mut s10[index]);
        }
        let (body, rest) = s10.split_at_mut(3);
        column_sums(body, &mut rest[0]);
        let (body, rest) = s10[4..].split_at_mut(4);
        column_sums(body, &mut rest[0]);
        for column in 0..4 {
            let value = v(column_ref(&s10[3], column)) - v(column_ref(&s10[8], column));
            set(column_mut(&mut s10[9], column), value);
        }

        // Part II.
        let p2 = &mut self.part_ii;
        p2.item_14_total_tax_due_or_overpayment = WholePeso(v(&item19.total));
        p2.item_15_total_tax_credits = WholePeso(v(&credits.total));
        let item16 = v(&item19.total) - v(&credits.total);
        p2.item_16_net_tax_payable_or_overpayment = WholePeso(item16);
        // Items 17-19 file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        let p2 = &mut self.part_ii;
        let penalties =
            v(&p2.item_17_surcharge) + v(&p2.item_18_interest) + v(&p2.item_19_compromise);
        set(&mut p2.item_20_total_penalties, penalties);
        let item21 = if item16 >= 0 {
            item16 + penalties
        } else if penalties > 0 {
            penalties
        } else {
            item16
        };
        set(&mut p2.item_21_total_amount_payable_or_overpayment, item21);
        if item16 >= 0 {
            p2.overpayment_disposition = None;
        }
        let _ = item13;
    }

    /// `validate()` and `initialValidateBeforeSave()` with their alert
    /// texts, the per-field checks the page runs while typing, and the
    /// limits of the four-page electronic return.
    pub(super) fn official_errors(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: String| errors.push((field.to_string(), message));
        let s2 = &self.schedule_2.items;
        let nonzero =
            |row: &Form1702MXRegimeAmounts, column: usize| v(column_ref(row, column)) != 0;

        // validateBasistTaxtRelief: each column with amounts needs its Part IV details.
        if self.relief_basis.instruction_multiple_activities {
            // validateAttachments: Instruction B files through attachments,
            // and this port holds no attachment Schedule A, so Item 1 is
            // blank. The special-rate loop is an else-if, so exempt wins.
            let special_only =
                !s2.iter().any(|row| nonzero(row, 0)) && s2.iter().any(|row| nonzero(row, 1));
            err(
                "relief_basis",
                if special_only {
                    "Please provide value on Special Rate Attachment Page 1 Schedule A Item 1."
                } else {
                    "Please provide value on Exempt Attachment Page 1 Schedule A Item 1."
                }
                .to_string(),
            );
        } else {
            let rd = &self.relief_details;
            for (column, letter, items) in [
                (0usize, 'A', &[0usize, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12][..]),
                (1, 'B', &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12][..]),
                (2, 'C', &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12][..]),
            ] {
                // The page checks only the first column holding amounts.
                if !items.iter().any(|&i| nonzero(&s2[i], column)) {
                    continue;
                }
                let missing = if rd.investment_promotion_agency[column].trim().is_empty() {
                    Some(1)
                } else if rd.legal_basis[column].trim().is_empty() {
                    Some(2)
                } else if rd.registered_activity[column].trim().is_empty() {
                    Some(3)
                } else if column == 1
                    && self.relief_basis.special_tax_rate.hundredths.unwrap_or(0) <= 0
                {
                    Some(4)
                } else if rd.effectivity_from[column].trim().is_empty() {
                    Some(5)
                } else if rd.effectivity_until[column].trim().is_empty() {
                    Some(6)
                } else {
                    None
                };
                if let Some(item) = missing {
                    err(
                        "relief_details",
                        format!("Please provide value on Page 2 Schedule 1 Item {item}{letter}."),
                    );
                }
                break;
            }
        }

        // initialValidateBeforeSave.
        if self.rdo_code.trim().is_empty()
            || self.rdo_code.trim() == "000"
            || !crate::validation::rdo_code_is_official_option(self.rdo_code.trim())
        {
            err(
                "rdo_code",
                "Please select an RDO Code (Part I Item 7).".to_string(),
            );
        }
        if self.taxpayer_name.trim().is_empty() {
            err(
                "taxpayer_name",
                "Please provide a Registered Name (Part I Item 9).".to_string(),
            );
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Please provide a Registered Address (Part I Item 10).".to_string(),
            );
        }
        if self.contact_number.trim().is_empty() {
            err(
                "contact_number",
                "Please provide a Contact Number (Part I Item 11).".to_string(),
            );
        }
        if self.email.trim().is_empty() {
            err(
                "email",
                "Please provide an Email Address (Part I Item 12).".to_string(),
            );
        } else if !super::form_1702rt_official::email_matches_official_format(self.email.trim()) {
            // validateEmail (onblur on txtPg1Pt1I12Email).
            err(
                "email",
                "You have entered an invalid email address format!".to_string(),
            );
        }

        // validateComputationIncomeTax.
        let s4 = &self.schedule_4.items;
        for (cond, message) in [
            (
                v(&s2[12].exempt) > 0 && v(&s4[0].exempt) <= 0,
                "Please provide value on Page 3 Schedule 4 Item 1A",
            ),
            (
                v(&s2[12].special) > 0 && v(&s4[0].special) <= 0,
                "Please provide value on Page 3 Schedule 4 Item 1B",
            ),
            (
                v(&s2[8].exempt) > 0 && v(&s4[1].exempt) <= 0,
                "Please provide value on Page 3 Schedule 4 Item 2A",
            ),
            (
                v(&s2[8].special) > 0 && v(&s4[1].special) <= 0,
                "Please provide value on Page 3 Schedule 4 Item 2B",
            ),
            (
                v(&s2[8].regular) > 0 && v(&s4[1].regular) <= 0,
                "Please provide value on Page 3 Schedule 4 Item 2C",
            ),
        ] {
            if cond {
                err("schedule_4", message.to_string());
            }
        }

        // Item 2 (validateYearEnd).
        let today = chrono::Local::now().date_naive();
        let current_year = chrono::Datelike::year(&today);
        let current_month0 = chrono::Datelike::month0(&today) as i32;
        let year = i32::from(self.filing_year());
        let month0 = i32::from(self.month) - 1;
        if !(1..=12).contains(&self.month) || self.taxable_year == 0 {
            err(
                "month",
                "Please select Month and Year for Year Ended (Page 1 Item 2).".to_string(),
            );
        } else {
            let fiscal = self.filing_basis == Form1702MXFilingBasis::Fiscal;
            let message = if fiscal
                && (year > current_year || (month0 > current_month0 && year == current_year))
            {
                Some(
                    "Date (Page 1 Item 2) cannot be greater than current date when filing for Fiscal Year.",
                )
            } else if fiscal && month0 == 11 {
                Some("Date (Page 1 Item 2) Month cannot be equal to December.")
            } else if year < 2013 || (year <= 2013 && month0 < 8) {
                Some("Date (Page 1 Item 2) should not be earlier than September 2013.")
            } else if !fiscal && !self.is_short_period && year >= current_year {
                Some(
                    "Year (Page 1 Item 2) cannot be greater than or equal to current year when filing for Calendar Year.",
                )
            } else if !fiscal && self.is_short_period && year > current_year {
                Some(
                    "Year (Page 1 Item 2) cannot be greater than the current year when filing for Calendar Year.",
                )
            } else if !fiscal
                && self.is_short_period
                && year == current_year
                && month0 > current_month0
            {
                Some(
                    "Month (Page 1 Item 2) cannot be greater than  current month date when filing for Calendar Year  and  Short Period Return.",
                )
            } else if !fiscal && self.is_short_period && year == current_year && month0 == 11 {
                Some(
                    "Month (Page 1 Item 2) cannot be equal to december  when filing for Calendar Year and  Short Period Return.",
                )
            } else if !fiscal && !self.is_short_period && self.month != 12 {
                Some("Please select Month and Year for Year Ended (Page 1 Item 2).")
            } else {
                None
            };
            if let Some(message) = message {
                err("taxable_year", message.to_string());
            }
        }

        // Item 5 and Part I Item 8 (date of incorporation).
        if !self.atc.mcit_selected && !self.atc.other_selected {
            err(
                "atc",
                "Please tick at least one ATC option (Page 1 Item 5).".to_string(),
            );
        }
        if self.atc.other_selected
            && !FORM_1702MX_ATC_OPTIONS
                .iter()
                .any(|(code, _)| *code == self.atc.other_code)
        {
            err("atc", "Select an ATC from the Item 5 list.".to_string());
        }
        match parse_mm_dd_yyyy(&self.incorporation_date) {
            None if self.incorporation_date.trim().is_empty() => err(
                "incorporation_date",
                "Please provide Date of Incorporation / Organization (Part I Item 10).".to_string(),
            ),
            None => err(
                "incorporation_date",
                "Enter the Date of Incorporation as MM/DD/YYYY.".to_string(),
            ),
            Some(date) => {
                let incorporation_year = chrono::Datelike::year(&date);
                let incorporation_month = chrono::Datelike::month(&date) as i32;
                if incorporation_year > year
                    || (incorporation_month > i32::from(self.month) && incorporation_year == year)
                {
                    err(
                        "incorporation_date",
                        "Date of Incorporation cannot be greater than Page 1 Item 2 Date."
                            .to_string(),
                    );
                }
            }
        }

        // checkOverPayment.
        let p2 = &self.part_ii;
        if (p2.item_16_net_tax_payable_or_overpayment.0 < 0
            || v(&p2.item_21_total_amount_payable_or_overpayment) < 0)
            && p2.overpayment_disposition.is_none()
        {
            err(
                "part_ii.overpayment_disposition",
                "Please select an Overpayment option (Page 1 Part II Item 21).".to_string(),
            );
        }

        // Schedule 2 Item 13 against Schedule 10 Item 10.
        let item10 = &self.schedule_10.items[9];
        if (0..4).any(|c| v(column_ref(&s2[12], c)) != v(column_ref(item10, c))) {
            err(
                "schedule_10",
                "Page 2 Schedule 2 Item 13 Columns A, B, C & D must be equal to Page 4 Schedule 10 Item 10 Columns A, B, C & D.".to_string(),
            );
        }

        // validate_nullDescription for every described row.
        let described = |amounts: &[i64], descriptions: &[&str]| -> bool {
            let any_amount = amounts.iter().any(|a| *a > 0);
            descriptions
                .iter()
                .all(|d| d.trim().is_empty() != any_amount)
        };
        let abc =
            |row: &Form1702MXRegimeAmounts| vec![v(&row.exempt), v(&row.special), v(&row.regular)];
        let mut described_rows: Vec<(Vec<i64>, Vec<&str>, String)> = Vec::new();
        for (offset, letter) in ['d', 'e', 'f', 'g', 'h', 'i'].into_iter().enumerate() {
            described_rows.push((
                abc(&self.schedule_5.amounts[19 + offset]),
                vec![self.schedule_5.other_descriptions_17d_to_17i[offset].as_str()],
                format!("Page 3 Schedule 5 Item 17{letter}"),
            ));
        }
        described_rows.push((
            abc(&self.schedule_3.items_20_to_33[10]),
            vec![self.schedule_3.item_30_description.as_str()],
            "Page 2 Schedule 3 Item 30".to_string(),
        ));
        described_rows.push((
            abc(&self.schedule_3.items_20_to_33[11]),
            vec![self.schedule_3.item_31_description.as_str()],
            "Page 2 Schedule 3 Item 31".to_string(),
        ));
        for (index, row) in self.schedule_6.rows.iter().enumerate() {
            described_rows.push((
                abc(&row.amounts),
                vec![row.description.as_str(), row.legal_basis.as_str()],
                format!("Page 3 Schedule 6 Item {}", index + 1),
            ));
        }
        for (index, row) in self.schedule_7_1.rows.iter().enumerate() {
            described_rows.push((
                vec![
                    v(&row.amount),
                    v(&row.applied_previous_years),
                    v(&row.expired),
                    v(&row.applied_current_year),
                ],
                vec![row.year_incurred.as_str()],
                format!("Page 4 Schedule 7 Item {}", index + 4),
            ));
        }
        for (index, row) in self.schedule_8_1.rows.iter().enumerate() {
            described_rows.push((
                vec![
                    v(&row.amount),
                    v(&row.applied_previous_years),
                    v(&row.expired),
                    v(&row.applied_current_year),
                ],
                vec![row.year_incurred.as_str()],
                format!("Page 4 Schedule 8.1 Item {}", index + 4),
            ));
        }
        for (index, row) in self.schedule_9.rows.iter().enumerate() {
            described_rows.push((
                vec![
                    v(&row.normal_income_tax),
                    v(&row.mcit),
                    v(&row.applied_previous_years),
                    v(&row.expired),
                    v(&row.applied_current_year),
                ],
                vec![row.year.as_str()],
                format!("Page 4 Schedule 9 Item {}", index + 1),
            ));
        }
        for index in [1usize, 2, 4, 5, 6, 7] {
            described_rows.push((
                abc(&self.schedule_10.items[index]),
                vec![self.schedule_10.descriptions[index].as_str()],
                format!("Page 4 Schedule 10 Item {}", index + 1),
            ));
        }
        for (amounts, descriptions, label) in &described_rows {
            // The page's own NOLCO row holds " " when there is no loss.
            let descriptions: Vec<&str> = descriptions.iter().map(|d| d.trim()).collect();
            if !described(amounts, &descriptions) {
                err(
                    "descriptions",
                    format!("Please provide complete data on {label}.\n{DESCRIPTION_ALERT}"),
                );
            }
        }

        // Item 4 "(Add more...)" (`SaveChecking_ModalTable2`, init 0).
        let modal = &self.schedule_6.more_rows;
        if modal.len() == 1 {
            err(
                "schedule_6.more_rows",
                "The Schedule 6 Item 4 (Add more...) list needs two or more rows; enter a single row on Item 4.".to_string(),
            );
        }
        if modal.iter().any(|row| {
            row.description.trim().is_empty()
                || row.legal_basis.trim().is_empty()
                || v(&row.amounts.total) == 0
        }) {
            err(
                "schedule_6.more_rows",
                "Please fill up Description and Legal Basis and Amount for A or B or C before you can save".to_string(),
            );
        }
        if modal.iter().any(|row| {
            row.description.trim().chars().count() > 8 || row.legal_basis.trim().chars().count() > 8
        }) {
            err(
                "schedule_6.more_rows",
                "Schedule 6 Item 4 (Add more...) Description and Legal Basis hold at most 8 characters each.".to_string(),
            );
        }

        if self.deduction_method == Form1702MXDeductionMethod::Unresolved {
            err(
                "deduction_method",
                "Please select a Method of Deduction in page 1 Item 13.".to_string(),
            );
        }

        // Checks the page makes while typing.
        for (index, row) in self.schedule_9.rows.iter().enumerate() {
            if v(&row.balance) < 0 {
                err(
                    &format!("schedule_9.rows[{index}]"),
                    "Page 4, Schedule 9: The sum of Columns D,E & F should not be greater than the amount in Column C. Please re-enter the correct values.".to_string(),
                );
            }
        }
        for (table, name) in [
            (&self.schedule_7_1, "schedule_7_1"),
            (&self.schedule_8_1, "schedule_8_1"),
        ] {
            for (index, row) in table.rows.iter().enumerate() {
                if v(&row.unapplied) < 0 {
                    err(
                        &format!("{name}.rows[{index}]"),
                        "Amount is invalid. Sum of Column B, C and D shall not be greater than the amount in Column A".to_string(),
                    );
                }
            }
        }
        for (rate, field) in [
            (
                &self.schedule_2.item_14_special_rate,
                "schedule_2.item_14_special_rate",
            ),
            (
                &self.schedule_2.item_14_regular_rate,
                "schedule_2.item_14_regular_rate",
            ),
            (
                &self.relief_basis.special_tax_rate,
                "relief_basis.special_tax_rate",
            ),
        ] {
            if rate.hundredths.is_some_and(|h| !(0..10_000).contains(&h)) {
                err(
                    field,
                    "Percentage cannot be greater than or equal to 100%".to_string(),
                );
            }
        }
        if self
            .relief_basis
            .special_tax_rate
            .hundredths
            .is_some_and(|h| h % 10 != 0)
        {
            err(
                "relief_basis.special_tax_rate",
                "Item 4B holds one decimal place.".to_string(),
            );
        }
        if self.optional_column_c_entered() {
            err(
                "deduction_method",
                "With the Optional Standard Deduction, column C of Schedules 5, 6 and 7 stays zero.".to_string(),
            );
        }
        if !self.atc.mcit_selected && v(&self.schedule_2.items[17].regular) != 0 {
            err(
                "schedule_2.item_18",
                "Item 18 (MCIT) applies only with ATC IC 055.".to_string(),
            );
        }
        if self.incorporation_years().is_some_and(|years| years < 4) && self.atc.mcit_selected {
            err(
                "atc",
                "Less than 4 years has passed since Date of Incorporation and the Filing Year, the mark in ATC - IC 055 of Page 1 Item 5 will be removed.".to_string(),
            );
        }
        if !self.is_amended
            && (0..3).any(|c| v(column_ref(&self.schedule_3.items_20_to_33[7], c)) != 0)
        {
            err(
                "schedule_3.item_27",
                "Item 27 (tax paid in a previously filed return) applies only to an amended return.".to_string(),
            );
        }
        if self.negative_input() {
            err(
                "amounts",
                "Amounts are whole pesos and cannot be negative (only Schedule 10 Item 1 can be)."
                    .to_string(),
            );
        }
        for (value, field, limit) in [
            (&self.taxpayer_name, "taxpayer_name", 80usize),
            (&self.registered_address, "registered_address", 62),
            (&self.contact_number, "contact_number", 25),
            (&self.email, "email", 112),
        ] {
            if value.chars().count() > limit {
                err(
                    field,
                    format!("This entry holds at most {limit} characters."),
                );
            }
        }
        let rd = &self.relief_details;
        for (column, letter) in ['A', 'B', 'C'].into_iter().enumerate() {
            for (item, value) in [
                (1, &rd.investment_promotion_agency[column]),
                (2, &rd.legal_basis[column]),
                (3, &rd.registered_activity[column]),
            ] {
                if value.chars().count() > 15 {
                    err(
                        "relief_details",
                        format!(
                            "Page 2 Schedule 1 Item {item}{letter} holds at most 15 characters."
                        ),
                    );
                }
            }
            for (item, value) in [
                (5, &rd.effectivity_from[column]),
                (6, &rd.effectivity_until[column]),
            ] {
                if !value.trim().is_empty() && parse_mm_dd_yyyy(value).is_none() {
                    err(
                        "relief_details",
                        format!("Page 2 Schedule 1 Item {item}{letter} must be a MM/DD/YYYY date."),
                    );
                }
            }
        }

        // Derived items must be what the official compute chain produces.
        let mut expected = self.clone();
        expected.official_recompute();
        if expected.schedule_2 != self.schedule_2
            || expected.schedule_3 != self.schedule_3
            || expected.part_ii != self.part_ii
            || expected.schedule_10 != self.schedule_10
            || expected.schedule_4 != self.schedule_4
        {
            err(
                "part_ii",
                "Totals are out of date. Recompute the return.".to_string(),
            );
        }
        errors
    }

    fn optional_column_c_entered(&self) -> bool {
        self.deduction_method == Form1702MXDeductionMethod::OptionalStandard
            && (self
                .schedule_5
                .amounts
                .iter()
                .take(25)
                .any(|row| v(&row.regular) != 0)
                || self
                    .schedule_6
                    .rows
                    .iter()
                    .any(|row| v(&row.amounts.regular) != 0))
    }

    fn negative_input(&self) -> bool {
        let abc_negative = |row: &Form1702MXRegimeAmounts| {
            v(&row.exempt) < 0 || v(&row.special) < 0 || v(&row.regular) < 0
        };
        let s2 = &self.schedule_2.items;
        [0usize, 1, 3, 5].iter().any(|&i| abc_negative(&s2[i]))
            || v(&s2[15].special) < 0
            || v(&s2[17].regular) < 0
            || self
                .schedule_3
                .items_20_to_33
                .iter()
                .take(12)
                .any(abc_negative)
            || self.schedule_5.amounts.iter().take(25).any(abc_negative)
            || self
                .schedule_6
                .rows
                .iter()
                .chain(self.schedule_6.more_rows.iter())
                .any(|row| abc_negative(&row.amounts))
            || [1usize, 2, 4, 5, 6, 7]
                .iter()
                .any(|&i| abc_negative(&self.schedule_10.items[i]))
            || abc_negative(&self.schedule_4.items[0])
            || abc_negative(&self.schedule_4.items[1])
            || [
                &self.part_ii.item_17_surcharge,
                &self.part_ii.item_18_interest,
                &self.part_ii.item_19_compromise,
            ]
            .iter()
            .any(|cell| v(cell) < 0)
    }

    /// The official field values the saveEncryptedProfile loop reads, keyed by
    /// element id. Amounts are given as numbers; the layout normalizes them.
    pub fn to_official_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        macro_rules! put {
            ($key:expr, $value:expr $(,)?) => {
                fields.insert(format!("frm1702MX:{}", $key), $value)
            };
        }
        let flag = |on: bool| on.to_string();
        let calendar = self.filing_basis == Form1702MXFilingBasis::Calendar;
        put!("rdoPg1I1Calendar", flag(calendar));
        put!("rdoPg1I1Fiscal", flag(!calendar));
        put!("ddlPg1I2Date", format!("{:02}", self.month));
        put!("txtPg1I2YearEnd", format!("{:02}", self.taxable_year % 100));
        put!("rdoPg1I3AmendedYes", flag(self.is_amended));
        put!("rdoPg1I3AmendedNo", flag(!self.is_amended));
        put!("rdoPg1I4ShortPeriodYes", flag(self.is_short_period));
        put!("rdoPg1I4ShortPeriodNo", flag(!self.is_short_period));
        put!("chkPg1I5ATCR1", flag(self.atc.mcit_selected));
        put!("drpPg1I5ATCR2", self.atc.other_code.clone());
        put!("chkPg1I5ATCR2", flag(self.atc.other_selected));

        let digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
        let branch = digits.get(9..).unwrap_or("").to_string();
        for (index, value) in [part(0..3), part(3..6), part(6..9), branch]
            .into_iter()
            .enumerate()
        {
            let n = index + 1;
            put!(&format!("txtPg1Pt1I6TINC{n}"), value.clone());
            for page in ["Pg2", "Pg3", "Pg4", "Pg1AttM", "Pg2AttM"] {
                put!(&format!("txt{page}TIN{n}"), value.clone());
            }
        }
        put!("txtPg1Pt1I7RDO", self.rdo_code.trim().to_string());
        put!("drpPg1Pt1I7RDO", self.rdo_code.trim().to_string());
        put!(
            "drpPg3Sc1I11CB",
            self.special_rate_option()
                .unwrap_or_else(|| "0".to_string())
        );
        put!(
            "txtPg1Pt1I9RegisteredName",
            self.taxpayer_name.trim().to_string()
        );
        put!(
            "txtPg1Pt1I9RegisteredName2",
            self.registered_name_lines[1].trim().to_string(),
        );
        put!(
            "txtPg1Pt1I9RegisteredName3",
            self.registered_name_lines[2].trim().to_string(),
        );
        put!(
            "txtPg1Pt1I10RegisteredAddress",
            self.registered_address.trim().to_string(),
        );
        put!(
            "txtPg1Pt1I10RegisteredAddress2",
            self.registered_address_lines[1].trim().to_string(),
        );
        put!(
            "txtPg1Pt1I10RegisteredAddress3",
            self.registered_address_lines[2].trim().to_string(),
        );
        for page in ["Pg2", "Pg3", "Pg4"] {
            put!(
                &format!("txt{page}RegisteredName"),
                self.taxpayer_name.trim().to_string(),
            );
        }
        put!("txtZIP", self.zip_code.trim().to_string());
        put!("txtPg1Pt1I8", self.incorporation_date.trim().to_string());
        put!(
            "txtPg1Pt1I11ContactNumber",
            self.contact_number.trim().to_string()
        );
        put!("txtPg1Pt1I12Email", self.email.trim().to_string());
        put!(
            "rdoPg1Pt1I13MethodOfDeducItemized",
            flag(self.deduction_method == Form1702MXDeductionMethod::Itemized),
        );
        put!(
            "rdoPg1Pt1I13MethodOfDeducOptional",
            flag(self.deduction_method == Form1702MXDeductionMethod::OptionalStandard),
        );

        let p2 = &self.part_ii;
        let amount = |value: i64| value.to_string();
        put!(
            "txtPg1Pt2I14TotalIncome",
            amount(p2.item_14_total_tax_due_or_overpayment.0)
        );
        put!(
            "txtPg1Pt2I15LessTotalTax",
            amount(p2.item_15_total_tax_credits.0)
        );
        put!(
            "txtPg1Pt2I16NetTaxPayable",
            amount(p2.item_16_net_tax_payable_or_overpayment.0),
        );
        put!("txtPg1Pt2I17", amount(v(&p2.item_17_surcharge)));
        put!("txtPg1Pt2I18", amount(v(&p2.item_18_interest)));
        put!("txtPg1Pt2I19", amount(v(&p2.item_19_compromise)));
        put!(
            "txtPg1Pt2I20TotalPenalties",
            amount(v(&p2.item_20_total_penalties))
        );
        put!(
            "txtPg1Pt2I21TotalAmount",
            amount(v(&p2.item_21_total_amount_payable_or_overpayment)),
        );
        let over = p2.overpayment_disposition;
        put!(
            "rdoPg1Pt2I21Refund",
            flag(over == Some(Form1702MXOverpaymentDisposition::Refund)),
        );
        put!(
            "rdoPg1Pt2I21IssueTCC",
            flag(over == Some(Form1702MXOverpaymentDisposition::TaxCreditCertificate)),
        );
        put!(
            "rdoPg1Pt2I21CarriedOver",
            flag(over == Some(Form1702MXOverpaymentDisposition::CarryOver)),
        );
        put!(
            "txtPg1P2I23NumOfAttachments",
            if self.number_of_attachments.trim().is_empty() {
                "00".to_string()
            } else {
                self.number_of_attachments.trim().to_string()
            },
        );

        // Part IV Schedule 1.
        put!(
            "InstAPg2Part4",
            flag(!self.relief_basis.instruction_multiple_activities),
        );
        put!(
            "InstBPg2Part4",
            flag(self.relief_basis.instruction_multiple_activities),
        );
        let rd = &self.relief_details;
        for (column, letter) in ['A', 'B', 'C'].into_iter().enumerate() {
            put!(
                &format!("txtPg2Pt4I31C{letter}"),
                rd.investment_promotion_agency[column].trim().to_string(),
            );
            put!(
                &format!("txtPg2Pt4I32C{letter}"),
                rd.legal_basis[column].trim().to_string(),
            );
            put!(
                &format!("txtPg2Pt4I33C{letter}"),
                rd.registered_activity[column].trim().to_string(),
            );
            put!(
                &format!("txtPg2Pt4I35C{letter}"),
                rd.effectivity_from[column].trim().to_string(),
            );
            put!(
                &format!("txtPg2Pt4I36C{letter}"),
                rd.effectivity_until[column].trim().to_string(),
            );
        }
        let percent = |rate: &PercentInput, decimals: usize| {
            format!(
                "{:.*}",
                decimals,
                f64::from(rate.hundredths.unwrap_or(0)) / 100.0
            )
        };
        put!(
            "txtPg2Pt4I34SpecialTaxRate",
            percent(&self.relief_basis.special_tax_rate, 1),
        );
        put!(
            "txtPg2Sc2It14B",
            percent(&self.schedule_2.item_14_special_rate, 2)
        );
        put!(
            "txtPg2Sc2It14C",
            percent(&self.schedule_2.item_14_regular_rate, 2)
        );

        // Schedule 2.
        let s2 = &self.schedule_2.items;
        let cols = ['A', 'B', 'C', 'D'];
        macro_rules! row {
            ($key:expr, $amounts:expr, $which:expr $(,)?) => {
                for letter in $which.chars() {
                    let column = cols.iter().position(|c| *c == letter).unwrap_or(3);
                    put!(
                        format!("{}{letter}", $key),
                        amount(v(column_ref($amounts, column)))
                    );
                }
            };
        }
        for item in 1..=9 {
            row!(&format!("txtPg2Sc2Itm{item}"), &s2[item - 1], "ABCD");
        }
        row!("txtPg2Sc2It10", &s2[9], "BCD");
        row!("txtPg2Sc2It11", &s2[10], "ABCD");
        row!("txtPg2Sc2It12", &s2[11], "CD");
        row!("txtPg2Sc2It13", &s2[12], "ABCD");
        row!("txtPg2Sc2It15", &s2[14], "BCD");
        row!("txtPg2Sc2It16", &s2[15], "BD");
        row!("txtPg2Sc2It17", &s2[16], "BCD");
        row!("txtPg2Sc2It18", &s2[17], "CD");
        row!("txtPg2Sc2It19", &s2[18], "BCD");
        // Schedule 10 Item 10 sits on page 2 under Schedule 2's prefix.
        row!("txtPg2Sc2Itm10", &self.schedule_10.items[9], "ABCD");

        // Schedule 3.
        for (offset, amounts) in self.schedule_3.items_20_to_33.iter().enumerate() {
            let item = 20 + offset;
            let which = if item == 21 || item == 23 {
                "CD"
            } else {
                "ABCD"
            };
            row!(&format!("txtPg2Sc3It{item}"), amounts, which);
        }
        // Schedule 4.
        for (offset, amounts) in self.schedule_4.items.iter().enumerate() {
            let item = offset + 1;
            match item {
                1 => row!("txtPg3Sc4Itm1", amounts, "ABD"),
                4 => row!("txtPg2Sc4Itm4", amounts, "BD"),
                _ => row!(&format!("txtPg2Sc4Itm{item}"), amounts, "ABCD"),
            }
        }
        // Schedule 5.
        let s5_keys = (1..=16).map(|n| n.to_string()).chain(
            [
                "17a", "17b", "17c", "17d", "17e", "17f", "17g", "17h", "17i", "18",
            ]
            .map(String::from),
        );
        for (key, amounts) in s5_keys.zip(self.schedule_5.amounts.iter()) {
            row!(&format!("txtPg3Sc5Itm{key}"), amounts, "ABCD");
        }
        // Schedule 6 (the official ids mix page numbers).
        let s6_ids = [
            [
                "txtPg6Sc6I1CA",
                "txtPg3Sc6I1CB",
                "txtPg6Sc6I1CC",
                "txtPg6Sc6I1CD",
            ],
            [
                "txtPg6Sc6I2CA",
                "txtPg3Sc6I2CB",
                "txtPg6Sc6I2CC",
                "txtPg6Sc6I2CD",
            ],
            [
                "txtPg6Sc6I3CA",
                "txtPg4Sc6I3CB",
                "txtPg6Sc6I3CC",
                "txtPg6Sc6I3CD",
            ],
            [
                "txtPg3Sc6I4CA",
                "txtPg3Sc6I4CB",
                "txtPg3Sc6I4CC",
                "txtPg3Sc6I4CD",
            ],
        ];
        for (ids, entry) in s6_ids.iter().zip(self.schedule_6.rows.iter()) {
            for (column, id) in ids.iter().enumerate() {
                put!(id, amount(v(column_ref(&entry.amounts, column))));
            }
        }
        for (column, letter) in cols.iter().enumerate() {
            put!(
                &format!("txtPg6Sc6I5C{letter}"),
                amount(v(column_ref(&self.schedule_6.item_5_total, column))),
            );
        }
        // Item 4 "(Add more...)": the saved rows, their count and subtotals.
        if self.schedule_6.is_folded() {
            let subtotal = self.schedule_6.modal_subtotal();
            put!(
                "txtCtrmodalPg6Sc6",
                self.schedule_6.more_rows.len().to_string()
            );
            for (column, letter) in cols.iter().enumerate() {
                put!(
                    &format!("txtPg6Sc6_SubTotalC{letter}"),
                    amount(v(column_ref(&subtotal, column))),
                );
            }
            for (key, value) in self.schedule_6.modal_controls() {
                fields.insert(key, value);
            }
        }
        // Schedules 7 and 8 (NOLCO).
        let rn = &self.regular_nolco;
        put!("txtPg6Sc7I1", amount(v(&rn.item_1_gross_income)));
        put!(
            "txtPg6Sc7I2",
            amount(v(&rn.item_2_ordinary_itemized_deductions))
        );
        put!("txtPg6Sc7I3", amount(v(&rn.item_3_net_operating_loss)));
        let sn = &self.special_nolco;
        put!("txtPg4Sc8It1", amount(v(&sn.item_1_gross_income)));
        put!(
            "txtPg4Sc8It2",
            amount(v(&sn.item_2_ordinary_itemized_deductions))
        );
        put!("txtPg4Sc8It3", amount(v(&sn.item_3_net_operating_loss)));
        // The NOLCO tables are plain text: typed amounts as entered, computed
        // ones formatted (`1,234`), the page's own row 7 formatted too.
        for (table, prefix) in [(&self.schedule_7_1, '7'), (&self.schedule_8_1, '8')] {
            for (offset, entry) in table.rows.iter().enumerate() {
                let item = offset + 4;
                let page = |column: char| -> &'static str {
                    match (prefix, item, column) {
                        ('7', _, _) => "Pg3",
                        ('8', 7, _) => "Pg4",
                        ('8', 6, 'C') => "Pg3",
                        ('8', _, 'Y') => "Pg3",
                        _ => "Pg4",
                    }
                };
                put!(
                    &format!("txt{}IShed{prefix}_{item}Year", page('Y')),
                    entry.year_incurred.clone(),
                );
                // Typed cells pass through round(this,2) on blur (`1,234.00`);
                // the page's own row 7 uses formatCurrencyWOC (`1,234`).
                let computed_row = item == 7;
                let typed = |value: i64| {
                    if computed_row {
                        official_display(value)
                    } else if value == 0 {
                        "0".to_string()
                    } else {
                        crate::official_xml::official_amount(value as f64)
                    }
                };
                for (letter, value) in [
                    ('A', typed(v(&entry.amount))),
                    ('B', typed(v(&entry.applied_previous_years))),
                    ('C', typed(v(&entry.expired))),
                    ('D', typed(v(&entry.applied_current_year))),
                    ('E', official_display(v(&entry.unapplied))),
                ] {
                    put!(
                        &format!("txt{}IShed{prefix}_{item}{letter}", page(letter)),
                        value
                    );
                }
            }
            put!(
                &format!("txtPg3IShed{prefix}8D"),
                official_display(v(&table.item_8_total_applied_current_year)),
            );
        }
        // Schedule 9 (MCIT).
        for (offset, entry) in self.schedule_9.rows.iter().enumerate() {
            let n = offset + 1;
            put!(&format!("txtPg7Sc9I{n}"), entry.year.trim().to_string());
            for (letter, value) in [
                ('A', &entry.normal_income_tax),
                ('B', &entry.mcit),
                ('C', &entry.excess_mcit),
                ('D', &entry.applied_previous_years),
                ('E', &entry.expired),
                ('F', &entry.applied_current_year),
                ('G', &entry.balance),
            ] {
                put!(&format!("txtPg7Sc9I{n}C{letter}"), amount(v(value)));
            }
        }
        put!(
            "txtPg7Sc9I4",
            amount(v(&self.schedule_9.item_4_total_applied_current_year)),
        );
        // Schedule 10.
        for (offset, amounts) in self.schedule_10.items.iter().take(9).enumerate() {
            row!(&format!("txtPg4Sc10Itm{}", offset + 1), amounts, "ABCD");
        }

        // Descriptions.
        put!(
            "txtPg2Sc3It30",
            self.schedule_3.item_30_description.trim().to_string()
        );
        put!(
            "txtPg2Sc3It31",
            self.schedule_3.item_31_description.trim().to_string()
        );
        for (offset, letter) in ['d', 'e', 'f', 'g', 'h', 'i'].into_iter().enumerate() {
            put!(
                &format!("txtPg3Sc5It17{letter}"),
                self.schedule_5.other_descriptions_17d_to_17i[offset]
                    .trim()
                    .to_string(),
            );
        }
        for (index, entry) in self.schedule_6.rows.iter().enumerate() {
            let page = if index < 3 { 6 } else { 3 };
            put!(
                &format!("txtPg{page}Sc6I{}description", index + 1),
                entry.description.trim().to_string(),
            );
            put!(
                &format!("txtPg{page}Sc6I{}legal", index + 1),
                entry.legal_basis.trim().to_string(),
            );
        }
        for index in [1usize, 2, 4, 5, 6, 7] {
            put!(
                &format!("txtPg4Sc10Itm{}", index + 1),
                self.schedule_10.descriptions[index].trim().to_string(),
            );
        }
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_official_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// The generated layout with Schedule 6 Item 4's modal rows spliced in
    /// ahead of the subtotal box `txtPg6Sc6_SubTotalCA`, where the page's
    /// `modalTablePg3Sc6` puts them.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        use crate::official_xml::Entry;
        let base = crate::official_xml::layout(FORM_1702MX_LAYOUT_ID).map_err(|e| e.to_string())?;
        let controls = self.schedule_6.modal_controls();
        if controls.is_empty() {
            return Ok(base.clone());
        }
        let key_of = |entry: &Entry| match entry {
            Entry::Bool { key, .. } | Entry::Value { key, .. } => key.clone(),
        };
        let subtotal = "frm1702MX:txtPg6Sc6_SubTotalCA";
        let at = base
            .entries
            .iter()
            .position(|entry| key_of(entry) == subtotal)
            .filter(|&at| at > 0)
            .ok_or_else(|| format!("no entry ahead of {subtotal}"))?;
        let copies = controls
            .into_iter()
            .map(|(id, _)| {
                // Description and legal basis are text (uppercased), the
                // rest numbertext.
                let template = if id.ends_with("Col1") || id.ends_with("Col2") {
                    "frm1702MX:txtPg3Sc6I4description"
                } else {
                    subtotal
                };
                (template.to_string(), id)
            })
            .collect();
        crate::forms::official_inputs::extend_layout(
            base,
            &[crate::forms::official_inputs::RowInsertion {
                after: key_of(&base.entries[at - 1]),
                copies,
            }],
        )
    }
}

impl Form1702MXDraft {
    /// Items 17-19 on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                part_ii.item_17_surcharge: "17 Surcharge",
                part_ii.item_18_interest: "18 Interest",
                part_ii.item_19_compromise: "19 Compromise",
            ],
            controls: [
                "frm1702MX:txtPg1Pt2I17",
                "frm1702MX:txtPg1Pt2I18",
                "frm1702MX:txtPg1Pt2I19",
            ],
            state: penalty,
            basic_tax_due: "Item 16", |d| d.part_ii.item_16_net_tax_payable_or_overpayment.0 as f64,
            rounding: super::penalty_items::PenaltyRounding::WholePesos,
            // The calendar's annual deadline is for a full calendar year.
            deadline_period: |d: &Self| {
                (d.filing_basis == Form1702MXFilingBasis::Calendar
                    && !d.is_short_period
                    && d.month == 12)
                    .then(|| super::penalty_items::calendar_period(d))
                    .flatten()
            },
        )
    }
}

impl QueueableForm for Form1702MXDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1702MX";
    const FORM_TYPE: &'static str = FORM_1702MX_FORM_TYPE;
    const LAYOUT_ID: &'static str = FORM_1702MX_LAYOUT_ID;

    /// Item 21 Total Amount Payable/(Overpayment), whole pesos; unknown while blank.
    fn amount_payable(&self) -> Option<f64> {
        self.part_ii
            .item_21_total_amount_payable_or_overpayment
            .amount
            .map(|peso| peso.0 as f64)
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
        FilingPeriod::Annual
    }
    /// `ddlPg1I2Date + txtPg1I2YearEnd` (`MMYY`), as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}{:02}", self.month, self.taxable_year % 100)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 4 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let year: u16 = code.get(2..)?.parse().ok()?;
        (1..=12)
            .contains(&month)
            .then_some((2000 + year, FilingPeriod::Annual))
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
    /// The upload with Schedule 6 Item 4's modal rows in their DOM place.
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as QueueableForm>::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let xml_error = |error: String| vec![("xml".to_string(), error)];
        let layout = self.official_layout().map_err(xml_error)?;
        crate::official_xml::write(&layout, &self.field_map())
            .map_err(|error| xml_error(error.to_string()))
    }
}

/// Item 5 ATCs and the rates `atcCodes.xml` lists for them under 1702MX.
const ATC_RATES: &[(&str, &[f64])] = &[
    ("IC010", &[30.0]),
    ("IC020", &[30.0]),
    ("IC030", &[30.0, 10.0]),
    ("IC031", &[30.0, 10.0]),
    ("IC040", &[30.0]),
    ("IC041", &[0.0]),
    ("IC070", &[30.0]),
    ("IC190", &[30.0, 10.0]),
    ("IC191", &[30.0, 10.0]),
    ("IC011", &[0.0]),
    ("IC080", &[2.5]),
    ("IC101", &[10.0]),
    ("IC021", &[0.0]),
];

impl Form1702MXDraft {
    /// The value `drpPg3Sc1I11CB` holds when the page has built it: its first
    /// (lowest) option, `rate / 100` written the way JavaScript prints it.
    /// The page builds the list once Item 4B, Items 1B / 2B or the other ATC
    /// are entered; `populatePg3Sc1I11`'s preferred-option match compares
    /// `value * 100` to 5 or 30 exactly, which floating point never meets.
    fn special_rate_option(&self) -> Option<String> {
        let rate = f64::from(self.relief_basis.special_tax_rate.hundredths.unwrap_or(0)) / 100.0;
        let touched = rate != 0.0
            || !self.relief_details.investment_promotion_agency[1]
                .trim()
                .is_empty()
            || !self.relief_details.legal_basis[1].trim().is_empty()
            || self.atc.other_selected;
        if !touched {
            return None;
        }
        let mut rates = vec![rate];
        if self.atc.other_selected
            && let Some((_, list)) = ATC_RATES
                .iter()
                .find(|(code, _)| *code == self.atc.other_code)
        {
            rates.extend_from_slice(list);
        }
        let lowest = rates.into_iter().fold(f64::INFINITY, f64::min);
        Some(format!("{}", lowest / 100.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filing_queue::QueueAuthSource;
    use crate::forms::FilingStatus;

    fn peso(value: i64) -> WholePesoInput {
        WholePesoInput::from_amount(WholePeso(value))
    }

    fn profile() -> crate::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Sample Dummy Taxpayer",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Sample Trading",
            "registered_address": "123 Sample Street, Quezon City",
            "zip_code": "1100",
            "phone": "09170000000",
            "email": "sample.taxpayer@example.com",
            "default_form_type": "1702MXv2018C",
            "taxpayer_type": "Corporation"
        }))
        .expect("dummy profile")
    }

    /// The same entries as the sample payload, whose official run gives the
    /// values asserted below.
    fn sample() -> Form1702MXDraft {
        let mut d = Form1702MXDraft::new_from_profile(&profile(), 2025);
        d.incorporation_date = "01/15/2015".into();
        d.deduction_method = Form1702MXDeductionMethod::Itemized;
        d.relief_details.investment_promotion_agency = ["Peza".into(), "Boi".into(), String::new()];
        d.relief_details.legal_basis = ["Ra 7916".into(), "Eo 226".into(), String::new()];
        d.relief_details.registered_activity = ["Export".into(), "Tourism".into(), String::new()];
        d.relief_details.effectivity_from =
            ["01/01/2020".into(), "01/01/2021".into(), String::new()];
        d.relief_details.effectivity_until =
            ["12/31/2026".into(), "12/31/2027".into(), String::new()];
        d.relief_basis.special_tax_rate = PercentInput::from_hundredths(500);
        let s2 = &mut d.schedule_2.items;
        s2[0].exempt = peso(1_000_000);
        s2[0].special = peso(2_000_000);
        s2[0].regular = peso(5_000_000);
        s2[1].exempt = peso(100_000);
        s2[1].special = peso(500_000);
        s2[1].regular = peso(2_000_000);
        s2[3].regular = peso(25_000);
        s2[5].regular = peso(150_000);
        s2[15].special = peso(1_000);
        s2[17].regular = peso(63_000);
        d.schedule_2.item_14_special_rate = PercentInput::from_hundredths(500);
        d.schedule_2.item_14_regular_rate = PercentInput::from_hundredths(2_500);
        d.schedule_5.amounts[0].exempt = peso(50_000);
        d.schedule_5.amounts[0].special = peso(100_000);
        d.schedule_5.amounts[0].regular = peso(800_000);
        d.schedule_5.amounts[2].regular = peso(120_500);
        d.schedule_5.amounts[19].regular = peso(10_000);
        d.schedule_5.other_descriptions_17d_to_17i[0] = "Training costs".into();
        d.schedule_6.rows[0].description = "Pension fund".into();
        d.schedule_6.rows[0].legal_basis = "Ra 4917".into();
        d.schedule_6.rows[0].amounts.exempt = peso(5_000);
        let row = &mut d.schedule_7_1.rows[0];
        row.year_incurred = "2022".into();
        row.amount = peso(300_000);
        row.applied_previous_years = peso(100_000);
        row.applied_current_year = peso(50_000);
        let mcit = &mut d.schedule_9.rows[0];
        mcit.year = "2023".into();
        mcit.normal_income_tax = peso(10_000);
        mcit.mcit = peso(30_000);
        mcit.applied_current_year = peso(5_000);
        d.schedule_3.items_20_to_33[0].regular = peso(50_000);
        d.schedule_3.items_20_to_33[2].special = peso(3_000);
        d.schedule_3.items_20_to_33[4].regular = peso(20_000);
        d.schedule_4.items[0].exempt = peso(90_000);
        d.schedule_4.items[0].special = peso(30_000);
        d.schedule_4.items[1].exempt = peso(1_000);
        d.part_ii.item_17_surcharge = peso(1_000);
        d.keep_entered_penalties();
        d.schedule_10.items[0].exempt = peso(845_000);
        d.schedule_10.items[0].special = peso(1_400_000);
        d.schedule_10.items[0].regular = peso(2_144_500);
        d.recompute();
        d
    }

    fn messages(draft: &Form1702MXDraft) -> Vec<String> {
        <Form1702MXDraft as QueueableForm>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let d = sample();
        let s2 = &d.schedule_2.items;
        assert_eq!(v(&s2[6].regular), 3_125_000);
        assert_eq!(v(&s2[7].regular), 930_500);
        assert_eq!(v(&s2[9].regular), 50_000);
        assert_eq!(v(&s2[12].total), 4_389_500);
        assert_eq!(v(&s2[14].special), 100_000);
        assert_eq!(v(&s2[14].regular), 536_125);
        assert_eq!(v(&s2[18].total), 635_125);
        assert_eq!(v(&d.schedule_3.items_20_to_33[3].regular), 5_000);
        assert_eq!(v(&d.schedule_4.items[4].special), -70_000);
        assert_eq!(v(&d.schedule_7_1.rows[0].unapplied), 150_000);
        assert_eq!(d.schedule_7_1.rows[3].year_incurred, " ");
        assert_eq!(
            d.part_ii.item_16_net_tax_payable_or_overpayment,
            WholePeso(557_125)
        );
        assert_eq!(
            v(&d.part_ii.item_21_total_amount_payable_or_overpayment),
            558_125
        );
        assert!(d.atc.mcit_selected && !d.atc.other_selected);
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn net_operating_loss_fills_this_years_nolco_row() {
        let mut d = sample();
        d.schedule_5.amounts[0].special = peso(2_000_000);
        d.recompute();
        assert_eq!(v(&d.special_nolco.item_3_net_operating_loss), -500_000);
        let row = &d.schedule_8_1.rows[3];
        assert_eq!(row.year_incurred, "2025");
        assert_eq!(v(&row.amount), 500_000);
        assert_eq!(v(&row.unapplied), 500_000);
    }

    #[test]
    fn field_map_uses_official_formats() {
        let d = sample();
        let fields = d.to_official_field_map();
        assert_eq!(fields["frm1702MX:txtPg1I2YearEnd"], "25");
        assert_eq!(fields["frm1702MX:txtPg3IShed7_4A"], "300,000.00");
        assert_eq!(fields["frm1702MX:txtPg3IShed7_4E"], "150,000");
        assert_eq!(fields["frm1702MX:txtPg2Sc4Itm5B"], "-70000");
        assert_eq!(fields["frm1702MX:txtPg2Sc2It14C"], "25.00");
        assert_eq!(fields["frm1702MX:txtPg2Pt4I34SpecialTaxRate"], "5.0");
        assert_eq!(official_display(-1_234_567), "(1,234,567)");
        assert_eq!(d.special_rate_option().as_deref(), Some("0.05"));
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1702MXv2018C-1225#sample.taxpayer@example.com#.xml"
        );
        let payload = d.to_official_xml_payload().unwrap();
        assert!(payload.contains(
            "<div>frm1702MX:txtPg1Pt1I7RDO=039frm1702MX:txtPg1Pt1I7RDO=</div>\t\n            <div>frm1702MX:drpPg1Pt1I7RDO=039frm1702MX:drpPg1Pt1I7RDO=</div>"
        ));
        assert!(
            payload.contains("<div>frm1702MX:drpPg3Sc1I11CB=0.05frm1702MX:drpPg3Sc1I11CB=</div>")
        );
        assert!(payload.contains("<div>frm1702MX:txtPg2Sc2Itm4A=0frm1702MX:txtPg2Sc2Itm4A=</div>"));
    }

    #[test]
    fn period_codes_round_trip() {
        let d = sample();
        assert_eq!(d.period_code(), "1225");
        assert_eq!(
            Form1702MXDraft::parse_period_code("1225"),
            Some((2025, FilingPeriod::Annual))
        );
        assert_eq!(Form1702MXDraft::parse_period_code("1325"), None);
        assert_eq!(Form1702MXDraft::parse_period_code("122025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1702MXDraft), expected: &str| {
            let mut d = sample();
            mutate(&mut d);
            let found = messages(&d);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.relief_details.legal_basis[0].clear(),
            "Please provide value on Page 2 Schedule 1 Item 2A.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please select an RDO Code (Part I Item 7).",
        );
        // Schedule 6 Item 4 "(Add more...)": a blank added row is refused
        // on Save, like `SaveChecking_ModalTable2`.
        check(
            &|d| {
                let row = crate::forms::form_1702mx::Form1702MXSpecialDeductionRow {
                    description: "Seminar".into(),
                    legal_basis: "Rr 2-98".into(),
                    amounts: Form1702MXRegimeAmounts {
                        exempt: peso(1_000),
                        ..Default::default()
                    },
                };
                d.schedule_6.more_rows = vec![row, Default::default()];
                d.recompute();
            },
            "Please fill up Description and Legal Basis and Amount for A or B or C before you can save",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please provide a Registered Name (Part I Item 9).",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please provide a Contact Number (Part I Item 11).",
        );
        check(
            &|d| d.email.clear(),
            "Please provide an Email Address (Part I Item 12).",
        );
        check(
            &|d| {
                d.schedule_4.items[0].exempt = peso(0);
                d.recompute();
            },
            "Please provide value on Page 3 Schedule 4 Item 1A",
        );
        check(
            &|d| d.taxable_year = 2026,
            "Year (Page 1 Item 2) cannot be greater than or equal to current year when filing for Calendar Year.",
        );
        check(
            &|d| {
                d.filing_basis = Form1702MXFilingBasis::Fiscal;
                d.month = 12;
            },
            "Date (Page 1 Item 2) Month cannot be equal to December.",
        );
        check(
            &|d| d.taxable_year = 2012,
            "Date (Page 1 Item 2) should not be earlier than September 2013.",
        );
        check(
            &|d| d.incorporation_date.clear(),
            "Please provide Date of Incorporation / Organization (Part I Item 10).",
        );
        check(
            &|d| {
                d.incorporation_date = "06/01/2026".into();
            },
            "Date of Incorporation cannot be greater than Page 1 Item 2 Date.",
        );
        check(
            &|d| {
                d.schedule_3.items_20_to_33[0].regular = peso(5_000_000);
                d.recompute();
            },
            "Please select an Overpayment option (Page 1 Part II Item 21).",
        );
        check(
            &|d| {
                d.schedule_10.items[0].regular = peso(1);
                d.recompute();
            },
            "Page 2 Schedule 2 Item 13 Columns A, B, C & D must be equal to Page 4 Schedule 10 Item 10 Columns A, B, C & D.",
        );
        check(
            &|d| d.schedule_5.other_descriptions_17d_to_17i[0].clear(),
            "Please provide complete data on Page 3 Schedule 5 Item 17d.\n(Amount cannot be zero [0] if Description is not empty and vice versa).",
        );
        check(
            &|d| d.deduction_method = Form1702MXDeductionMethod::Unresolved,
            "Please select a Method of Deduction in page 1 Item 13.",
        );
        check(
            &|d| {
                d.schedule_9.rows[0].expired = peso(30_000);
                d.recompute();
            },
            "Page 4, Schedule 9: The sum of Columns D,E & F should not be greater than the amount in Column C. Please re-enter the correct values.",
        );
        check(
            &|d| {
                d.schedule_7_1.rows[0].expired = peso(500_000);
                d.recompute();
            },
            "Amount is invalid. Sum of Column B, C and D shall not be greater than the amount in Column A",
        );
        check(
            &|d| d.relief_basis.instruction_multiple_activities = true,
            "Please provide value on Exempt Attachment Page 1 Schedule A Item 1.",
        );
        check(
            &|d| {
                d.relief_basis.instruction_multiple_activities = true;
                for row in d.schedule_2.items.iter_mut() {
                    row.exempt = peso(0);
                }
            },
            "Please provide value on Special Rate Attachment Page 1 Schedule A Item 1.",
        );
        check(
            &|d| d.email = "abc".into(),
            "You have entered an invalid email address format!",
        );
        check(
            &|d| d.schedule_2.items[0].regular = peso(1),
            "Totals are out of date. Recompute the return.",
        );
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut d = sample();
        d.queue(QueueAuthSource::Gui).unwrap();
        assert_eq!(d.lifecycle.status, FilingStatus::Queued);
        assert!(d.clone().revalidate_queued_before_submission().is_ok());
        d.part_ii.item_17_surcharge = peso(9);
        d.keep_entered_penalties();
        assert!(d.revalidate_queued_before_submission().is_err());
        assert_eq!(d.lifecycle.status, FilingStatus::Draft);
    }

    #[test]
    fn old_stored_json_still_loads() {
        let mut json = serde_json::to_value(sample()).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("relief_details");
        object.insert("submission_attempts".into(), serde_json::json!(1));
        object.insert("next_retry_at".into(), serde_json::Value::Null);
        object.insert("submitted_at".into(), serde_json::Value::Null);
        object.insert("last_error".into(), serde_json::json!("old"));
        let draft: Form1702MXDraft = serde_json::from_value(json).unwrap();
        assert_eq!(draft.lifecycle.submission_attempts, 1);
        assert_eq!(draft.last_error.as_deref(), Some("old"));
        assert!(draft.relief_details.legal_basis[0].is_empty());
        let back = serde_json::to_value(&draft).unwrap();
        assert!(back.get("lifecycle").is_none());
        assert_eq!(back["status"], "Draft");
    }

    #[test]
    fn instruction_radios_agree_between_the_official_page_and_the_upload_map() {
        for multiple in [false, true] {
            let mut d = sample();
            d.relief_basis.instruction_multiple_activities = multiple;
            let page = d.to_official_field_map();
            let upload = d.to_bir_field_map();
            for (key, want) in [
                ("frm1702MX:InstAPg2Part4", !multiple),
                ("frm1702MX:InstBPg2Part4", multiple),
            ] {
                assert_eq!(
                    page[key],
                    want.to_string(),
                    "{key} on the page, multiple={multiple}"
                );
                assert_eq!(
                    upload[key],
                    want.to_string(),
                    "{key} uploaded, multiple={multiple}"
                );
            }
        }
    }

    /// The print's Item 9: the registered address the return files on its
    /// first line, and a later line printed only when the address does not
    /// already hold it, so nothing prints twice.
    #[test]
    fn print_address_lines_are_never_printed_twice() {
        let mut d = Form1702MXDraft::new_from_profile(&profile(), 2025);
        d.registered_address = "Unit 5 Bldg A, 123 Ayala Avenue, Makati City".to_string();
        d.registered_address_lines = [
            "Unit 5 Bldg A,".to_string(),
            "123 Ayala Avenue, Makati City".to_string(),
            "Annex Tower".to_string(),
        ];
        let fields = d.to_print_field_map();
        let line = |n: &str| fields[&format!("frm1702MX:txtPg1Pt1I10RegisteredAddress{n}")].clone();
        assert_eq!(line(""), "Unit 5 Bldg A, 123 Ayala Avenue, Makati City");
        assert_eq!(line("2"), "", "already in the address");
        assert_eq!(line("3"), "Annex Tower", "a line of its own still prints");
        // The upload keeps every line as stored.
        let upload = d.to_bir_field_map();
        assert_eq!(
            upload["frm1702MX:txtPg1Pt1I10RegisteredAddress2"],
            "123 Ayala Avenue, Makati City"
        );
    }
}
