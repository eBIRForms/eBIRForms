//! Form 1702-RT (January 2018, `1702RTv2018C`) on the generic submission path.
//!
//! Ported from the official `BIR-Form1702RTv2018C.hta` (eBIRForms 7.9.6.2.1):
//! the compute chain (`computeP2Pt4I29` … `computeP2Pt5I59`, Schedules I–V,
//! `computeP1Pt2I16` … `computeP1Pt2I21`), `checkDateOfIncorporation` for the
//! IC055 ATC, `validate()` / `initialValidateBeforeSave()` and the field
//! checks with their exact alert texts, and `saveXMLsubmit` through
//! [`crate::official_xml`].
//!
//! Amounts are whole pesos. The page shows a negative amount as `(1,234)`
//! and `saveXMLsubmit` writes it through `NumWithParenthesis`, so the
//! submitted text is `-1,234` ([`WholePeso::format_bir`]).
//!
//! The page picks its tax rules from `taxableYear`, computed at load from
//! `new Date().getYear()`. Under the HTA host (Internet Explorer) that is the
//! four-digit year, so every return filed since 2021 takes the `>= 20`
//! branch: Item 40 (rate), Item 42 (MCIT) and Part V Item 57 are entered by
//! the filer, and Item 41 is Item 39 times the rate.
//!
//! Item 42 on a return for a taxable year before 2020 is computed instead
//! (maintainer decision (pre-2020 MCIT is 2% of gross), 2026-10-19): 2% of Item
//! 33 while MCIT applies,
//! the page's branch below 20 (rules/forms/1702rt-v2018c/calculations.json
//! 1702rt-item42-mcit: "Before 2020 and when IC 055 is selected, MCIT is 2%
//! of gross"), read from the return's own Item 2 year. Before 2020 Items 41
//! and 42 are floored at zero (maintainer decision (pre-2020 1702RT items 41
//! and 42 floored at zero), 2026-10-19): a net loss carries forward as NOLCO
//! so there is no regular tax, and MCIT is 2% of gross income - zero when
//! gross income is zero or less - so Item 43 is never negative.
//!
//! The "(Add more...)" modals of Items 54, 17i, Schedule II Item 4 and
//! Schedule V Items 3, 6 and 8 are modelled ([`Form1702RTModal`]): their
//! rows are written into the modal tables the upload loop reads, with the
//! counter and subtotal. Schedule IIIA Item 7's button is left out: its
//! counter control is commented out of the page, so its `saveModalTable`
//! fails there and the page never reloads those rows.

use super::penalty_items::PenaltyItems;
use std::collections::BTreeMap;

use super::form_1702rt::{
    FORM_1702RT_MODALS, Form1702RTDeductionMethod, Form1702RTDraft, Form1702RTFilingBasis,
    Form1702RTModal, WholePeso,
};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};

/// Rule-package id of the official layout.
pub const FORM_1702RT_LAYOUT_ID: &str = "1702rt-v2018c";
/// Official `formType` (`1702RTv2018C`).
pub const FORM_1702RT_FORM_TYPE: &str = "1702RTv2018C";

const P: &str = "frm1702RT:";
const ZERO: WholePeso = WholePeso(0);

/// The first taxable year whose Item 42 the filer enters
/// (calculations.json 1702rt-item42-mcit: "2020+ leaves the enabled value
/// user-controlled").
pub const MCIT_ENTERED_FROM: u16 = 2020;

/// The income tax rate of a return for a year before [`MCIT_ENTERED_FROM`]
/// (maintainer decision (pre-2020 1702RT computed like BIR), 2026-10-19): calculations.json
/// 1702rt-item41-rate-tax, "For 2020+, tax due uses user-visible rate / 100
/// times Item 39, with nonpositive net forced to zero; before 2020 it uses
/// 30%." Item 40 shows it; Item 41 is floored at zero (maintainer decision
/// (pre-2020 1702RT items 41 and 42 floored at zero)).
pub const RATE_BEFORE_2020_PERCENT: u8 = 30;

/// `percent`% of `amount`, rounded as `getProduct2` does (`Math.round`,
/// half up, toward positive infinity on a half), in whole pesos with
/// integer arithmetic. A negative amount gives a negative result: the
/// official helper has no floor; the items floor it where the maintainer
/// decided (pre-2020 1702RT items 41 and 42 floored at zero).
fn percent_of(amount: WholePeso, percent: u8) -> WholePeso {
    let hundredths = i128::from(amount.0) * i128::from(percent);
    let rounded = (hundredths + 50).div_euclid(100);
    WholePeso(i64::try_from(rounded).expect("a percent of 100 or less of an i64 is one"))
}

/// Item 42 before [`MCIT_ENTERED_FROM`]: 2% of Item 33 (calculations.json
/// 1702rt-item42-mcit: "Before 2020 and when IC 055 is selected, MCIT is 2%
/// of gross"), rounded as `getProduct2` does; a negative Item 33 gives a
/// negative MCIT as the official helper computes it, which Item 42 floors
/// at zero (maintainer decision (pre-2020 1702RT items 41 and 42 floored at
/// zero)).
fn two_percent(gross: WholePeso) -> WholePeso {
    percent_of(gross, 2)
}

fn sum(values: impl IntoIterator<Item = WholePeso>) -> WholePeso {
    WholePeso(values.into_iter().map(|v| v.0).sum())
}

/// `getProduct2`: `Math.round(value * multiplier)`.
fn product(value: WholePeso, multiplier: f64) -> WholePeso {
    WholePeso((value.0 as f64 * multiplier + 0.5).floor() as i64)
}

/// `capitalize()`: `$.trim(value.toUpperCase())`.
fn capital(value: &str) -> String {
    value.to_uppercase().trim().to_string()
}

impl Form1702RTDraft {
    /// The item an "(Add more...)" modal folds into: description, legal
    /// basis (Schedule II only) and amount.
    pub fn modal_item_mut(
        &mut self,
        modal: Form1702RTModal,
    ) -> (&mut String, Option<&mut String>, &mut WholePeso) {
        match modal {
            Form1702RTModal::Item54 => {
                let item = &mut self.part_iv.tax_credits.item_54_other;
                (&mut item.description, None, &mut item.amount)
            }
            Form1702RTModal::Schedule1Item17i => {
                let item = &mut self.schedule_1.other[5];
                (&mut item.description, None, &mut item.amount)
            }
            Form1702RTModal::Schedule2Item4 => {
                let item = &mut self.schedule_2.rows[3];
                (
                    &mut item.description,
                    Some(&mut item.legal_basis),
                    &mut item.amount,
                )
            }
            Form1702RTModal::Schedule5Item3 => {
                let item = &mut self.schedule_5.additions[1];
                (&mut item.description, None, &mut item.amount)
            }
            Form1702RTModal::Schedule5Item6 => {
                let item = &mut self.schedule_5.non_taxable_income[1];
                (&mut item.description, None, &mut item.amount)
            }
            Form1702RTModal::Schedule5Item8 => {
                let item = &mut self.schedule_5.special_deductions[1];
                (&mut item.description, None, &mut item.amount)
            }
        }
    }

    /// The modal's controls in DOM order, keyed by element id
    /// (`<rowName>.<n>C1`…), written while the item is folded.
    pub fn modal_controls(&self, modal: Form1702RTModal) -> Vec<(String, String)> {
        if !self.more_rows.is_folded(modal) {
            return Vec::new();
        }
        let row_name = modal.row_name();
        let mut controls = Vec::new();
        for (index, row) in self.more_rows.saved(modal).into_iter().enumerate() {
            let n = index + 1;
            controls.push((format!("{P}{row_name}.{n}C1"), capital(&row.description)));
            let mut column = 2;
            if modal.has_legal_basis() {
                controls.push((format!("{P}{row_name}.{n}C2"), capital(&row.legal_basis)));
                column = 3;
            }
            controls.push((
                format!("{P}{row_name}.{n}C{column}"),
                row.amount.format_bir(),
            ));
        }
        controls
    }

    /// The generated layout with each folded modal's rows spliced in ahead
    /// of its subtotal box (`<rowName>Subtotal`), where the page's modal
    /// table puts them.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        use crate::forms::official_inputs::{RowInsertion, extend_layout};
        use crate::official_xml::Entry;
        let base = crate::official_xml::layout(FORM_1702RT_LAYOUT_ID).map_err(|e| e.to_string())?;
        let key_of = |entry: &Entry| match entry {
            Entry::Bool { key, .. } | Entry::Value { key, .. } => key.clone(),
        };
        let mut insertions = Vec::new();
        for modal in FORM_1702RT_MODALS {
            let controls = self.modal_controls(modal);
            if controls.is_empty() {
                continue;
            }
            let row_name = modal.row_name();
            let subtotal = format!("{P}{row_name}Subtotal");
            let at = base
                .entries
                .iter()
                .position(|entry| key_of(entry) == subtotal)
                .filter(|&at| at > 0)
                .ok_or_else(|| format!("no entry ahead of {subtotal}"))?;
            // Every modal control is a raw text box, like the item's own.
            let template = format!("{P}{row_name}C1");
            insertions.push(RowInsertion {
                after: key_of(&base.entries[at - 1]),
                copies: controls
                    .into_iter()
                    .map(|(id, _)| (template.clone(), id))
                    .collect(),
            });
        }
        extend_layout(base, &insertions)
    }

    pub fn is_itemized(&self) -> bool {
        self.deduction_method == Form1702RTDeductionMethod::Itemized
    }

    /// `checkDateOfIncorporation`: IC055 (MCIT) is ticked once four or more
    /// years separate the incorporation year from the Item 2 year.
    pub fn subject_to_mcit(&self) -> bool {
        self.incorporation_date
            .is_some_and(|date| i32::from(self.taxable_year) - i32::from(date.year) >= 4)
    }

    /// The official compute chain, settled the way the page's handlers leave
    /// it once every entry has been blurred.
    pub(super) fn official_recompute(&mut self) {
        if self.filing_basis == Form1702RTFilingBasis::Calendar && !self.is_short_period {
            // checkFilingYear: calendar returns end in December.
            self.month = 12;
        }
        let mcit = self.subject_to_mcit();
        self.atc.printed_mcit_selected = mcit;
        let itemized = self.is_itemized();

        if !itemized {
            // methodOfDeductionsOptional clears Schedules I, II and IIIA.
            let s1 = &mut self.schedule_1;
            for amount in [
                &mut s1.amortizations,
                &mut s1.bad_debts,
                &mut s1.charitable_contributions,
                &mut s1.depletion,
                &mut s1.depreciation,
                &mut s1.entertainment,
                &mut s1.fringe_benefits,
                &mut s1.interest,
                &mut s1.losses,
                &mut s1.pension_trusts,
                &mut s1.rental,
                &mut s1.research_and_development,
                &mut s1.salaries_wages_allowances,
                &mut s1.statutory_contributions,
                &mut s1.taxes_and_licenses,
                &mut s1.transportation_and_travel,
                &mut s1.janitorial_and_messengerial,
                &mut s1.professional_fees,
                &mut s1.security_services,
            ] {
                *amount = ZERO;
            }
            for row in &mut s1.other {
                *row = Default::default();
            }
            for row in &mut self.schedule_2.rows {
                *row = Default::default();
            }
            for row in &mut self.schedule_3.rows {
                *row = Default::default();
            }
            self.more_rows.schedule_1_item_17i.clear();
            self.more_rows.schedule_2_item_4.clear();
            self.part_v.item_57_special_allowable_deductions_tax_effect = ZERO;
        }
        // "(Add more...)": with two or more saved rows the item reads
        // OTHERS and the modal subtotal (saveModalTable, othersSubtotal).
        for modal in FORM_1702RT_MODALS {
            if !self.more_rows.is_folded(modal) {
                continue;
            }
            let subtotal = self.more_rows.subtotal(modal);
            let (description, legal_basis, amount) = self.modal_item_mut(modal);
            *description = "OTHERS".to_string();
            if let Some(legal_basis) = legal_basis {
                *legal_basis = "OTHERS".to_string();
            }
            *amount = subtotal;
        }
        if !mcit {
            // enableMCITFields(false)
            self.part_iv.item_42_mcit_due = ZERO;
            self.part_iv
                .tax_credits
                .item_45_previous_quarter_mcit_payments = ZERO;
            for row in &mut self.schedule_4.rows {
                *row = Default::default();
            }
        }
        if !self.is_amended {
            self.part_iv.tax_credits.item_51_tax_paid_on_previous_return = ZERO;
        }

        // Schedule I (computeP3Sc1I18TotalOrdinaryAllowable)
        let s1 = &mut self.schedule_1;
        s1.item_18_total = sum(s1
            .source_amounts()
            .into_iter()
            .chain(s1.other.iter().map(|row| row.amount)));
        // Schedule II (computeP3Sc2I5TotalSpecialAllowable)
        self.schedule_2.item_5_total = sum(self.schedule_2.rows.iter().map(|row| row.amount));

        let p4 = &mut self.part_iv;
        p4.item_29_net_sales = WholePeso(p4.item_27_sales.0 - p4.item_28_sales_returns.0);
        p4.item_31_gross_income_from_operations =
            WholePeso(p4.item_29_net_sales.0 - p4.item_30_cost_of_sales_or_services.0);
        p4.item_33_total_taxable_income = WholePeso(
            p4.item_31_gross_income_from_operations.0 + p4.item_32_other_taxable_income.0,
        );
        let gross = p4.item_33_total_taxable_income;
        let before_2020 = self.taxable_year < MCIT_ENTERED_FROM;
        if mcit && before_2020 {
            // Maintainer decision (pre-2020 MCIT is 2% of gross):
            // computeP2Pt4I42's branch for a year before 2020
            // with IC 055 ticked, 2% of gross (getProduct2's Math.round),
            // floored at zero (maintainer decision (pre-2020 1702RT items
            // 41 and 42 floored at zero)): MCIT is 2% of gross income,
            // zero when gross income is zero or less.
            p4.item_42_mcit_due = two_percent(gross).max(ZERO);
        }
        let ordinary = self.schedule_1.item_18_total;
        p4.item_34_ordinary_itemized_deductions = ordinary;

        // Schedule III (computeItem18and33, fillNetOperatingLossItems).
        let s3 = &mut self.schedule_3;
        if itemized && ordinary.0 > gross.0 {
            s3.item_1_gross_income = gross;
            s3.item_2_ordinary_deductions = ordinary;
            s3.item_3_net_operating_loss = WholePeso(gross.0 - ordinary.0);
        } else {
            s3.item_1_gross_income = ZERO;
            s3.item_2_ordinary_deductions = ZERO;
            s3.item_3_net_operating_loss = ZERO;
        }
        // Item 4 is this year's loss; its columns B to D stay disabled.
        let current = &mut s3.rows[0];
        current.applied_previous_years = ZERO;
        current.expired = ZERO;
        current.applied_current_year = ZERO;
        if s3.item_3_net_operating_loss.0 < 0 {
            current.year_incurred = format!("20{:02}", self.taxable_year % 100);
            current.amount = WholePeso(s3.item_3_net_operating_loss.0.abs());
        } else {
            current.year_incurred = String::new();
            current.amount = ZERO;
        }
        for row in &mut s3.rows {
            row.unapplied_balance = WholePeso(
                row.amount.0
                    - (row.applied_previous_years.0 + row.expired.0 + row.applied_current_year.0),
            );
        }
        s3.item_8_total_applied_current_year =
            sum(s3.rows.iter().map(|row| row.applied_current_year));

        // Schedule IV (computeP4Sc4I1C4 … computeP4Sc4I4TotalExcessMCIT).
        let s4 = &mut self.schedule_4;
        for row in &mut s4.rows {
            row.excess_mcit = WholePeso((row.mcit.0 - row.normal_income_tax.0).max(0));
            row.allowable_balance = WholePeso(
                row.excess_mcit.0
                    - (row.applied_previous_years.0 + row.expired.0 + row.applied_current_year.0),
            );
        }
        s4.item_4_total_applied_current_year =
            sum(s4.rows.iter().map(|row| row.applied_current_year));

        let p4 = &mut self.part_iv;
        if itemized {
            p4.item_35_special_itemized_deductions = self.schedule_2.item_5_total;
            p4.item_36_nolco = self.schedule_3.item_8_total_applied_current_year;
            p4.item_37_total_itemized_deductions = sum([
                p4.item_34_ordinary_itemized_deductions,
                p4.item_35_special_itemized_deductions,
                p4.item_36_nolco,
            ]);
            p4.item_38_optional_standard_deduction = ZERO;
            p4.item_39_net_taxable_income_or_loss =
                WholePeso(gross.0 - p4.item_37_total_itemized_deductions.0);
        } else {
            p4.item_35_special_itemized_deductions = ZERO;
            p4.item_36_nolco = self.schedule_3.item_8_total_applied_current_year;
            p4.item_37_total_itemized_deductions = ZERO;
            p4.item_38_optional_standard_deduction = product(gross, 0.40);
            p4.item_39_net_taxable_income_or_loss =
                WholePeso(gross.0 - p4.item_38_optional_standard_deduction.0);
        }
        // computeP2Pt4I40 / computeP2Pt4I41 / computeP2Pt4I43
        let net = p4.item_39_net_taxable_income_or_loss;
        if before_2020 {
            // Maintainer decision (pre-2020 1702RT computed like BIR): before
            // 2020 the page's branch below 20 taxes
            // Item 39 at 30% (1702rt-item41-rate-tax), which Item 40 shows,
            // and Part V Item 57 is Item 35 times that rate / 100
            // (1702rt-item57-special-tax: "Only before 2020, Item 57 = Item
            // 35 times income-tax rate / 100; for 2020+ this function leaves
            // Item 57 untouched."); OSD has already cleared Item 57.
            //
            // The rule puts the zero floor on 2020+ only ("For 2020+, ...
            // nonpositive net forced to zero; before 2020 it uses 30%") and
            // the official body is not in the tree; the maintainer decided
            // (pre-2020 1702RT items 41 and 42 floored at zero) the floor
            // applies anyway: a net loss carries forward as NOLCO, so
            // there is no regular tax, and Item 43 is never negative.
            p4.item_40_income_tax_rate_percent = RATE_BEFORE_2020_PERCENT;
            p4.item_41_normal_income_tax_due = percent_of(net, RATE_BEFORE_2020_PERCENT).max(ZERO);
            if itemized {
                self.part_v.item_57_special_allowable_deductions_tax_effect = percent_of(
                    p4.item_35_special_itemized_deductions,
                    RATE_BEFORE_2020_PERCENT,
                );
            }
        } else {
            p4.item_41_normal_income_tax_due = if net.0 <= 0 {
                ZERO
            } else {
                product(net, f64::from(p4.item_40_income_tax_rate_percent) / 100.0)
            };
        }
        p4.item_43_tax_due = if p4.item_41_normal_income_tax_due.0 >= p4.item_42_mcit_due.0 {
            p4.item_41_normal_income_tax_due
        } else {
            p4.item_42_mcit_due
        };
        let credits = &mut p4.tax_credits;
        credits.item_47_excess_mcit_applied = self.schedule_4.item_4_total_applied_current_year;
        credits.item_55_total = sum([
            credits.item_44_prior_year_excess_credits,
            credits.item_45_previous_quarter_mcit_payments,
            credits.item_46_previous_quarter_regular_payments,
            credits.item_47_excess_mcit_applied,
            credits.item_48_previous_quarter_withholding,
            credits.item_49_fourth_quarter_withholding,
            credits.item_50_foreign_tax_credits,
            credits.item_51_tax_paid_on_previous_return,
            credits.item_52_special_tax_credits,
            credits.item_53_other.amount,
            credits.item_54_other.amount,
        ]);
        p4.item_56_net_tax_payable_or_overpayment =
            WholePeso(p4.item_43_tax_due.0 - credits.item_55_total.0);

        // Part V
        let p5 = &mut self.part_v;
        p5.item_58_special_tax_credits = p4.tax_credits.item_52_special_tax_credits;
        p5.item_59_total_tax_relief = sum([
            p5.item_57_special_allowable_deductions_tax_effect,
            p5.item_58_special_tax_credits,
        ]);

        // Schedule V
        let s5 = &mut self.schedule_5;
        s5.item_4_total = sum(std::iter::once(s5.item_1_net_income_or_loss_per_books)
            .chain(s5.additions.iter().map(|row| row.amount)));
        s5.item_9_total = sum(s5
            .non_taxable_income
            .iter()
            .chain(s5.special_deductions.iter())
            .map(|row| row.amount));
        s5.item_10_net_taxable_income_or_loss = WholePeso(s5.item_4_total.0 - s5.item_9_total.0);

        // Part II (computeP1Pt2I16 … computeP1Pt2I21)
        let p2 = &mut self.part_ii;
        p2.item_14_tax_due = p4.item_43_tax_due;
        p2.item_15_total_tax_credits = p4.tax_credits.item_55_total;
        p2.item_16_net_tax_payable_or_overpayment = p4.item_56_net_tax_payable_or_overpayment;
        // Items 17-19 file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        let p2 = &mut self.part_ii;
        p2.item_20_total_penalties = sum([
            p2.item_17_surcharge,
            p2.item_18_interest,
            p2.item_19_compromise,
        ]);
        let net_tax = p2.item_16_net_tax_payable_or_overpayment;
        p2.item_21_total_amount_payable_or_overpayment = if net_tax.0 >= 0 {
            WholePeso(net_tax.0 + p2.item_20_total_penalties.0)
        } else if p2.item_20_total_penalties.0 > 0 {
            p2.item_20_total_penalties
        } else {
            net_tax
        };
        if net_tax.0 >= 0 {
            p2.overpayment_disposition = None;
        }
    }

    /// `validate()` and `initialValidateBeforeSave()` in order with their
    /// alert texts, plus the checks the page makes while typing.
    pub(super) fn official_errors(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let today = chrono::Local::now().date_naive();
        let current_year = chrono::Datelike::year(&today);

        // Page 1 Item 2 (validate, validateYearEnd, checkFilingYear)
        let year = i32::from(self.taxable_year);
        if !(1..=12).contains(&self.month)
            || self.taxable_year.is_multiple_of(100)
            || !(2000..=2099).contains(&year)
        {
            err(
                "taxable_year",
                "Please provide a valid Year Ended on Page 1 Item 2",
            );
        } else if year < 2018 {
            err(
                "taxable_year",
                "Invalid Year. Year should not be earlier than 2018.",
            );
        } else {
            match self.filing_basis {
                Form1702RTFilingBasis::Fiscal => {
                    let month = i32::from(self.month);
                    if year > current_year
                        || (year == current_year && month > chrono::Datelike::month(&today) as i32)
                    {
                        err(
                            "taxable_year",
                            "Date (Page 1 Item 2) cannot be greater than current date when filing for Fiscal Year.",
                        );
                    } else if self.month == 12 {
                        err(
                            "month",
                            "Date (Page 1 Item 2) Month cannot be equal to December.",
                        );
                    }
                }
                Form1702RTFilingBasis::Calendar => {
                    if !self.is_short_period && year >= current_year {
                        err(
                            "taxable_year",
                            "Year (Page 1 Item 2) cannot be greater than or equal to current year when filing for Calendar Year.",
                        );
                    } else if self.is_short_period && year > current_year {
                        err(
                            "taxable_year",
                            "Year (Page 1 Item 2) cannot be greater than the current year when filing for Calendar Year.",
                        );
                    } else if !self.is_short_period && self.month != 12 {
                        err(
                            "month",
                            "A calendar-year return ends in December (Page 1 Item 2).",
                        );
                    }
                }
            }
        }

        // Page 1 Item 6
        let digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        if !(12..=14).contains(&digits.len())
            || !self.tin.chars().all(|ch| ch.is_ascii_digit() || ch == '-')
        {
            err(
                "tin",
                "Please provide a valid TIN (must have 3 numbers per box).",
            );
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&digits[..9]) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        // Page 1 Item 7
        let rdo = self.rdo_code.trim();
        if rdo.is_empty() || rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err(
                "rdo_code",
                "Please select a RDO code on Page 1 Part 1 Item 7",
            );
        }
        // Page 1 Item 10
        match self.incorporation_date {
            None => err(
                "incorporation_date",
                "Please provide a valid Date of Incorporation/Organization on Page 1 Part I Item 10",
            ),
            Some(date) => {
                let naive = chrono::NaiveDate::from_ymd_opt(
                    i32::from(date.year),
                    u32::from(date.month),
                    u32::from(date.day),
                );
                if naive.is_none() || date.year < 1800 {
                    err(
                        "incorporation_date",
                        "Please provide a valid date. (MM/DD/YYYY format)",
                    );
                } else if naive.is_some_and(|day| day > today) {
                    err("incorporation_date", "This date cannot be a future date.");
                } else if i32::from(date.year) > year
                    || (i32::from(date.year) == year && date.month > self.month)
                {
                    err(
                        "incorporation_date",
                        "Date of Incorporation cannot be greater than Page 1 Item 2 Date.",
                    );
                }
            }
        }
        // Page 1 Items 8, 9, 11, 12
        if self.registered_name_lines[0].trim().is_empty() {
            err(
                "registered_name_lines",
                "Please provide a Registered Name on Page 1 Part I Item 8",
            );
        }
        if self.registered_address_lines[0].trim().is_empty() {
            err(
                "registered_address_lines",
                "Please provide a Registered Address on Page 1 Part I Item 9",
            );
        }
        let contact = self.contact_number.trim();
        if contact.is_empty() || contact.len() > 20 || !contact.bytes().all(|b| b.is_ascii_digit())
        {
            err(
                "contact_number",
                "Please provide a valid Contact Number on Page 1 Part I Item 11",
            );
        }
        let email = self.email.trim();
        if email.is_empty() {
            err(
                "email",
                "Please provide a valid Email Address on Page 1 Part I Item 12",
            );
        } else if !email_matches_official_format(email) {
            err("email", "You have entered an invalid email address format!");
        }
        for (field, lines, max) in [
            ("registered_name_lines", &self.registered_name_lines, 100),
            (
                "registered_address_lines",
                &self.registered_address_lines,
                100,
            ),
        ] {
            if lines.iter().any(|line| line.chars().count() > max) {
                err(field, "Each line holds at most 100 characters.");
            }
        }
        if self.deduction_method == Form1702RTDeductionMethod::Unresolved {
            err(
                "deduction_method",
                "Select the method of deductions on Page 1 Part I Item 13.",
            );
        }

        // Page 1 Part II Items 16 and 21
        let p2 = &self.part_ii;
        if p2.item_16_net_tax_payable_or_overpayment.0 < 0 && p2.overpayment_disposition.is_none() {
            err(
                "part_ii.overpayment_disposition",
                "Please select an option for Overpayment Radio Button on Page 1 Part II ",
            );
        } else if p2.item_21_total_amount_payable_or_overpayment.0 < 0
            && p2.overpayment_disposition.is_none()
        {
            err(
                "part_ii.overpayment_disposition",
                "Please select an option for Overpayment Radio Button on Page 1 Part",
            );
        }

        // Page 1 Part III (validate_nullDescription; the official labels are
        // numbered three ahead of the items they check).
        let pay = &self.payment_details;
        for (index, label, texts) in [
            (
                0,
                "Page 1 Part III Item 26",
                vec![&pay[0].drawee_bank_or_agency, &pay[0].number],
            ),
            (
                1,
                "Page 1 Part III Item 27",
                vec![&pay[1].drawee_bank_or_agency, &pay[1].number],
            ),
            (2, "Page 1 Part III Item 28", vec![&pay[2].number]),
            (
                3,
                "Page 1 Part III Item 29",
                vec![
                    &pay[3].specification,
                    &pay[3].drawee_bank_or_agency,
                    &pay[3].number,
                ],
            ),
        ] {
            let row = &pay[index];
            let filled: Vec<bool> = texts
                .iter()
                .map(|text| !text.trim().is_empty())
                .chain(std::iter::once(row.date.is_some()))
                .collect();
            if null_description(row.amount, &filled) {
                err(
                    &format!("payment_details[{index}]"),
                    &format!("Please provide data on {label}."),
                );
            }
        }
        let paid = sum(pay.iter().map(|row| row.amount));
        if paid.0 != 0 && paid != p2.item_21_total_amount_payable_or_overpayment {
            err(
                "payment_details",
                "Sum of Amount fields in Details of Payment (Page 1 Part III) Segment must be equal to TOTAL AMOUNT PAYABLE",
            );
        }

        // Page 2 Items 53 and 54
        let credits = &self.part_iv.tax_credits;
        for (field, row, label) in [
            (
                "part_iv.tax_credits.item_53_other",
                &credits.item_53_other,
                "Page 2 Part 4 Item 53",
            ),
            (
                "part_iv.tax_credits.item_54_other",
                &credits.item_54_other,
                "Page 2 Part 4 Item 54",
            ),
        ] {
            if null_description(row.amount, &[!row.description.trim().is_empty()]) {
                err(field, &format!("Please provide data on {label}."));
            }
        }
        if self.part_iv.tax_credits.item_54_other.amount.0 != 0
            && self
                .part_iv
                .tax_credits
                .item_53_other
                .description
                .trim()
                .is_empty()
        {
            err(
                "part_iv.tax_credits.item_54_other",
                "Item 54 opens only after Item 53 is complete.",
            );
        }
        if self.is_itemized()
            && self.part_iv.item_35_special_itemized_deductions.0 > 0
            && self
                .part_v
                .item_57_special_allowable_deductions_tax_effect
                .0
                == 0
        {
            err(
                "part_v.item_57_special_allowable_deductions_tax_effect",
                "Please provide a value for this field (Part V Item 57)",
            );
        }

        // Page 3 Schedule I Items 17d to 17i (17h reuses the 17g label).
        for (index, label) in ["17d", "17e", "17f", "17g", "17g", "17i"]
            .into_iter()
            .enumerate()
        {
            let row = &self.schedule_1.other[index];
            if null_description(row.amount, &[!row.description.trim().is_empty()]) {
                err(
                    &format!("schedule_1.other[{index}]"),
                    &format!("Please provide data on Page 3 Schedule 1 Item {label}."),
                );
            }
        }
        // Page 3 Schedule II
        for (index, row) in self.schedule_2.rows.iter().enumerate() {
            if null_description(
                row.amount,
                &[
                    !row.description.trim().is_empty(),
                    !row.legal_basis.trim().is_empty(),
                ],
            ) {
                err(
                    &format!("schedule_2.rows[{index}]"),
                    &format!(
                        "Please provide data on Page 3 Schedule 2 Item {}.",
                        index + 1
                    ),
                );
            }
        }
        // Page 4 Schedule IIIA (validate_nullDescription, checkYear,
        // computeP4Sc3AI8TotalNOLCO).
        let s3 = &self.schedule_3;
        if s3.item_1_gross_income.0 > s3.item_2_ordinary_deductions.0
            && s3.item_2_ordinary_deductions.0 > 0
        {
            err(
                "schedule_3",
                "In Page 4 Schedule 3 Computation for NOLCO, Gross Income should not be greater than Ordinary Allowable Deductions.",
            );
        }
        for (index, row) in s3.rows.iter().enumerate() {
            let item = index + 4;
            if null_description(row.amount, &[!row.year_incurred.trim().is_empty()]) {
                err(
                    &format!("schedule_3.rows[{index}]"),
                    &format!("Please provide data on Page 4 Schedule 3A Item {item}."),
                );
            }
            if row.applied_current_year.0 > row.amount.0 {
                err(
                    &format!("schedule_3.rows[{index}].applied_current_year"),
                    &format!(
                        "Page 4 Schedule IIIA Item {item} Column D (NOLCO Applied Current Year) cannot be more than Page 4 Schedule 3A Item {item} Column A (Amount)"
                    ),
                );
            } else if index > 0
                && row.applied_previous_years.0 + row.expired.0 + row.applied_current_year.0
                    > row.amount.0
            {
                err(
                    &format!("schedule_3.rows[{index}]"),
                    &format!(
                        "Page 4 Schedule IIIA Item {item}: Column B + C + D cannot be greater than Column A"
                    ),
                );
            }
        }
        let years: Vec<&str> = s3.rows.iter().map(|row| row.year_incurred.trim()).collect();
        for (index, row) in s3.rows.iter().enumerate().skip(1) {
            let text = row.year_incurred.trim();
            if text.is_empty() {
                continue;
            }
            let field = format!("schedule_3.rows[{index}].year_incurred");
            let Ok(value) = text.parse::<i32>() else {
                err(&field, "Year Incurred must be a four-digit year.");
                continue;
            };
            let incorporated = self.incorporation_date.map(|date| i32::from(date.year));
            if value == year {
                err(
                    &field,
                    "Page 4 Schedule IIIA Item 5-7 Year Incurred cannot be equal to Page 1 Item 2 Year Ended.",
                );
            } else if value >= year - 3 && value <= year {
                if incorporated.is_some_and(|incorporated| incorporated > value) {
                    err(
                        &field,
                        "Year Incurred cannot be less than year in Page 1 Part 1 Item 10 (Date of Incorporation/Organization)",
                    );
                } else if years.iter().filter(|other| **other == text).count() != 1 {
                    err(&field, "Year Incurred per row must be unique.");
                }
            } else if value > year {
                err(
                    &field,
                    "Year Incurred cannot be more than Page 1 Part 1 Item 2 (Year Ended)",
                );
            } else {
                err(
                    &field,
                    "Year Incurred Cannot be less than 3 years in Page 1 Item 2 (Year Ended)",
                );
            }
        }
        // Page 4 Schedule V
        for (field, row, item) in [
            ("schedule_5.additions[0]", &self.schedule_5.additions[0], 2),
            ("schedule_5.additions[1]", &self.schedule_5.additions[1], 3),
            (
                "schedule_5.non_taxable_income[0]",
                &self.schedule_5.non_taxable_income[0],
                5,
            ),
            (
                "schedule_5.non_taxable_income[1]",
                &self.schedule_5.non_taxable_income[1],
                6,
            ),
            (
                "schedule_5.special_deductions[0]",
                &self.schedule_5.special_deductions[0],
                7,
            ),
            (
                "schedule_5.special_deductions[1]",
                &self.schedule_5.special_deductions[1],
                8,
            ),
        ] {
            if null_description(row.amount, &[!row.description.trim().is_empty()]) {
                err(
                    field,
                    &format!("Please provide data on Page 4 Schedule 5 Item {item}."),
                );
            }
        }
        if self.part_iv.item_39_net_taxable_income_or_loss
            != self.schedule_5.item_10_net_taxable_income_or_loss
        {
            err(
                "schedule_5.item_10_net_taxable_income_or_loss",
                "Item Page 4 Schedule V Item 10 must be equal Item 39 on Page 2 Part IV",
            );
        }
        // Page 4 Schedule IV
        let mcit_years: Vec<&str> = self
            .schedule_4
            .rows
            .iter()
            .map(|row| row.year.trim())
            .collect();
        for (index, row) in self.schedule_4.rows.iter().enumerate() {
            if row.allowable_balance.0 < 0 {
                err(
                    &format!("schedule_4.rows[{index}]"),
                    "Page 4, Schedule IV: The sum of Columns D,E & F should not be greater than the amount in Column C. Please re-enter the correct values.",
                );
            }
            let text = row.year.trim();
            if text.is_empty() {
                continue;
            }
            let field = format!("schedule_4.rows[{index}].year");
            let value = text.parse::<i32>().unwrap_or(0);
            let incorporated = self.incorporation_date.map(|date| i32::from(date.year));
            if mcit_years.iter().filter(|other| **other == text).count() > 1 {
                err(&field, "Year cannot have duplicates.");
            } else if value >= year || value < year - 3 {
                err(
                    &field,
                    &format!(
                        "Please enter a value for the year between {} and {}.",
                        year - 3,
                        year - 1
                    ),
                );
            } else if incorporated.is_some_and(|incorporated| value < incorporated) {
                err(
                    &field,
                    &format!(
                        "Year incurred cannot be lower than Year of Incorporation [{}] (Page 1 Item 8).",
                        incorporated.unwrap_or_default()
                    ),
                );
            }
        }

        // Amounts the filer types (checkNumValue turns a negative into 0).
        let p4 = &self.part_iv;
        if p4.item_40_income_tax_rate_percent == 0 || p4.item_40_income_tax_rate_percent >= 100 {
            err(
                "part_iv.item_40_income_tax_rate_percent",
                "Percentage cannot be greater than or equal to 100%",
            );
        }
        let negative = [
            p2.item_17_surcharge,
            p2.item_18_interest,
            p2.item_19_compromise,
            p4.item_27_sales,
            p4.item_28_sales_returns,
            p4.item_30_cost_of_sales_or_services,
            p4.item_32_other_taxable_income,
            p4.item_42_mcit_due,
            credits.item_44_prior_year_excess_credits,
            credits.item_45_previous_quarter_mcit_payments,
            credits.item_46_previous_quarter_regular_payments,
            credits.item_48_previous_quarter_withholding,
            credits.item_49_fourth_quarter_withholding,
            credits.item_50_foreign_tax_credits,
            credits.item_51_tax_paid_on_previous_return,
            credits.item_52_special_tax_credits,
            credits.item_53_other.amount,
            credits.item_54_other.amount,
            self.part_v.item_57_special_allowable_deductions_tax_effect,
        ]
        .into_iter()
        .chain(self.schedule_1.source_amounts())
        .chain(self.schedule_1.other.iter().map(|row| row.amount))
        .chain(self.schedule_2.rows.iter().map(|row| row.amount))
        .chain(self.payment_details.iter().map(|row| row.amount))
        .any(|amount| amount.0 < 0);
        if negative {
            err("amounts", "Amounts are whole pesos and cannot be negative.");
        }
        if self
            .registered_name_lines
            .iter()
            .chain(std::iter::once(&self.taxpayer_name))
            .any(|line| line.contains(char::is_control))
        {
            err(
                "registered_name_lines",
                "Names cannot contain control characters.",
            );
        }

        // "(Add more...)" modals (addRowModalTable / saveModalTable).
        for modal in FORM_1702RT_MODALS {
            let rows = self.more_rows.rows(modal);
            if rows.is_empty() {
                continue;
            }
            let field = format!("more_rows.{}", modal.row_name());
            let label = |n: usize| format!("Page {} Item {}.{n}", modal.page(), modal.item());
            for (index, row) in rows.iter().enumerate() {
                let n = index + 1;
                let described = !row.description.is_empty();
                let legal = !row.legal_basis.is_empty();
                let touched = described || row.amount.0 != 0 || modal.has_legal_basis() && legal;
                if !touched {
                    continue;
                }
                let message = if !described {
                    Some("Description should not be blank.")
                } else if modal.has_legal_basis() && !legal {
                    Some("Legal Basis should not be blank.")
                } else if row.amount.0 == 0 {
                    Some("Amount should not be zero.")
                } else {
                    None
                };
                if let Some(message) = message {
                    err(&field, &format!("{} {message}", label(n)));
                    break;
                }
            }
            if self.more_rows.saved(modal).len() == 1 {
                err(
                    &field,
                    &format!(
                        "{}: the (Add more...) list needs two or more rows; enter a single row on the item.",
                        modal.title()
                    ),
                );
            }
            if rows.iter().any(|row| {
                row.description.chars().count() > modal.description_limit()
                    || row.legal_basis.chars().count() > 15
            }) {
                err(
                    &field,
                    &format!(
                        "{}: a description holds at most {} characters{}.",
                        modal.title(),
                        modal.description_limit(),
                        if modal.has_legal_basis() {
                            " and a legal basis 15"
                        } else {
                            ""
                        }
                    ),
                );
            }
            if rows.iter().any(|row| row.amount.0 < 0) {
                err("amounts", "Amounts are whole pesos and cannot be negative.");
            }
        }

        // Derived items must be what the official compute chain produces.
        let mut expected = self.clone();
        expected.official_recompute();
        if expected.part_ii != self.part_ii
            || expected.part_iv != self.part_iv
            || expected.part_v != self.part_v
            || expected.schedule_1 != self.schedule_1
            || expected.schedule_2 != self.schedule_2
            || expected.schedule_3 != self.schedule_3
            || expected.schedule_4 != self.schedule_4
            || expected.schedule_5 != self.schedule_5
            || expected.atc != self.atc
        {
            err(
                "part_ii.item_21_total_amount_payable_or_overpayment",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }

    /// The official field values `saveXMLsubmit` writes, keyed by element id.
    pub fn to_official_field_map(&self) -> BTreeMap<String, String> {
        let layout =
            crate::official_xml::layout(FORM_1702RT_LAYOUT_ID).expect("1702RT layout is packaged");
        let keys = layout.keys();
        let mut fields: BTreeMap<String, String> = self
            .to_bir_field_map()
            .into_iter()
            .filter(|(key, _)| keys.contains(key.as_str()))
            .collect();
        // Page state the filer never edits keeps its official default.
        for key in [
            "BranchMaskP1",
            "txtBranchMaskP2",
            "txtBranchMaskP3",
            "txtBranchMaskP4",
            "driveSelectTPExport",
            "txtFinalFlag",
            "txtEnroll",
            "ebirOnlineConfirmUsername",
            "ebirOnlineUsername",
            "ebirOnlineSecret",
        ] {
            fields.remove(key);
        }
        fields.retain(|key, _| !key.ends_with("CtrModal") && !key.ends_with("Subtotal"));
        let email_key = format!("{P}txtPg1Pt1I12Email");
        for (key, value) in fields.iter_mut() {
            if *key != email_key && value != "true" && value != "false" {
                *value = capital(value);
            }
        }
        let mut put = |key: &str, value: String| {
            fields.insert(format!("{P}{key}"), value);
        };
        put("txtPg1Pt1I12Email", self.email.trim().to_string());
        put(
            "drpPg1I5AtcOther",
            if self.atc.other_code.trim().is_empty() {
                "IC010".to_string()
            } else {
                self.atc.other_code.trim().to_string()
            },
        );
        for page in 2..=4 {
            put(
                &format!("txtPg{page}RegisteredName"),
                capital(&self.taxpayer_name),
            );
        }
        // getRdo() builds this select inside div#rdoContainer at load; the
        // packaged layout (generated from the static page) lacks it.
        put("drpPg1Pt1I7RDOCode", self.rdo_code.trim().to_string());
        // "(Add more...)": the saved rows, their count and subtotal.
        for modal in FORM_1702RT_MODALS {
            let controls = self.modal_controls(modal);
            if controls.is_empty() {
                continue;
            }
            let row_name = modal.row_name();
            put(
                &format!("{row_name}CtrModal"),
                self.more_rows.saved(modal).len().to_string(),
            );
            put(
                &format!("{row_name}Subtotal"),
                self.more_rows.subtotal(modal).format_bir(),
            );
            for (key, value) in controls {
                put(key.trim_start_matches(P), value);
            }
        }
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_official_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

/// `validate_nullDescription` for one amount: a non-zero amount needs every
/// description, and a zero amount allows none.
fn null_description(amount: WholePeso, filled: &[bool]) -> bool {
    if amount.0 != 0 {
        filled.iter().any(|filled| !filled)
    } else {
        filled.iter().any(|filled| *filled)
    }
}

/// `validateEmail`: `/\b[a-zA-Z0-9._%+-]+@(?:[a-zA-Z0-9-]+\.)+[a-zA-Z]{2,4}\b/`.
pub(crate) fn email_matches_official_format(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && local
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._%+-".contains(ch));
    let labels: Vec<&str> = domain.split('.').collect();
    let domain_ok = labels.len() >= 2
        && labels[..labels.len() - 1].iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
        && labels.last().is_some_and(|tld| {
            (2..=4).contains(&tld.len()) && tld.chars().all(|ch| ch.is_ascii_alphabetic())
        });
    local_ok && domain_ok
}

impl Form1702RTDraft {
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
                "frm1702RT:txtPg1Pt2I17Surcharge",
                "frm1702RT:txtPg1Pt2I18Interest",
                "frm1702RT:txtPg1Pt2I19Compromise",
            ],
            state: penalty,
            basic_tax_due: "Item 16", |d| d.part_ii.item_16_net_tax_payable_or_overpayment.0 as f64,
            rounding: super::penalty_items::PenaltyRounding::WholePesos,
            // The calendar's annual deadline is for a full calendar year.
            deadline_period: |d: &Self| {
                (d.filing_basis == Form1702RTFilingBasis::Calendar
                    && !d.is_short_period
                    && d.month == 12)
                    .then(|| super::penalty_items::calendar_period(d))
                    .flatten()
            },
        )
    }
}

impl QueueableForm for Form1702RTDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1702RT";
    const FORM_TYPE: &'static str = FORM_1702RT_FORM_TYPE;
    const LAYOUT_ID: &'static str = FORM_1702RT_LAYOUT_ID;

    /// Item 21 Total Amount Payable/(Overpayment), whole pesos.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.part_ii.item_21_total_amount_payable_or_overpayment.0 as f64)
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
    /// Annual. Rows saved before the generic queue have a NULL period column
    /// with period key `A`; the generic save adopts them.
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Annual
    }
    /// `ddlPg1I2Month + "20" + txtPg1I2Year`, as in `createEncXMLFileName`.
    fn period_code(&self) -> String {
        format!("{:02}20{:02}", self.month, self.taxable_year % 100)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let month: u8 = code.get(..2)?.parse().ok()?;
        let year: u16 = code.get(2..)?.parse().ok()?;
        (1..=12)
            .contains(&month)
            .then_some((year, FilingPeriod::Annual))
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
    /// The upload with the "(Add more...)" rows in their DOM place.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filing_queue::QueueAuthSource;
    use crate::forms::FilingStatus;
    use crate::forms::form_1702rt::{
        Form1702RTDate, Form1702RTNamedAmount, Form1702RTOverpaymentDisposition,
    };

    fn profile() -> crate::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Sample Dummy Corporation",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Sample Trading",
            "registered_address": "123 Sample Street, Quezon City",
            "zip_code": "1100",
            "phone": "0281234567",
            "email": "sample.taxpayer@example.com",
            "default_form_type": "1702RTv2018C",
            "taxpayer_type": "Corporation",
            "business_start_date": "2015-03-10"
        }))
        .expect("dummy profile")
    }

    pub(crate) fn sample() -> Form1702RTDraft {
        let mut draft = Form1702RTDraft::new_from_profile(&profile(), 2025, 12);
        draft.deduction_method = Form1702RTDeductionMethod::Itemized;
        let p4 = &mut draft.part_iv;
        p4.item_40_income_tax_rate_percent = 25;
        p4.item_27_sales = WholePeso(5_000_000);
        p4.item_28_sales_returns = WholePeso(100_000);
        p4.item_30_cost_of_sales_or_services = WholePeso(2_000_000);
        p4.item_32_other_taxable_income = WholePeso(50_001);
        p4.item_42_mcit_due = WholePeso(59_000);
        p4.tax_credits.item_48_previous_quarter_withholding = WholePeso(120_000);
        draft.schedule_1.salaries_wages_allowances = WholePeso(1_200_000);
        draft.schedule_1.rental = WholePeso(240_000);
        // This sample enters its penalties by hand.
        draft.keep_entered_penalties();
        draft.recompute();
        let net = draft.part_iv.item_39_net_taxable_income_or_loss;
        draft.schedule_5.item_1_net_income_or_loss_per_books = net;
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1702RTDraft) -> Vec<String> {
        <Form1702RTDraft as QueueableForm>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    /// Maintainer decision (pre-2020 MCIT is 2% of gross): a 2019 return
    /// subject to MCIT (incorporated 2015, IC
    /// 055 ticked) computes item 42 as 2% of item 33, rounded half up, over
    /// whatever it held; item 43 takes it as the larger of items 41 and 42,
    /// and items 56, 14, 16 and 21 follow.
    #[test]
    fn before_2020_item_42_is_two_percent_of_gross_income() {
        let mut draft = sample();
        draft.taxable_year = 2019;
        draft.part_iv.item_32_other_taxable_income = WholePeso(50_026);
        draft.schedule_1.salaries_wages_allowances = WholePeso(2_900_000);
        draft.part_iv.item_42_mcit_due = WholePeso(1);
        draft.recompute();
        assert!(draft.subject_to_mcit());
        let p4 = &draft.part_iv;
        assert_eq!(p4.item_33_total_taxable_income, WholePeso(2_950_026));
        // 59,000.52 rounds to 59,001.
        assert_eq!(p4.item_42_mcit_due, WholePeso(59_001));
        // Item 39 is 2,950,026 - 3,140,000 = -189,974; the pre-2020 floor
        // (maintainer decision (pre-2020 1702RT items 41 and 42 floored at
        // zero)) keeps item 41 at zero rather than -56,992.
        assert_eq!(p4.item_41_normal_income_tax_due, ZERO);
        assert_eq!(p4.item_43_tax_due, WholePeso(59_001));
        assert_eq!(
            p4.item_56_net_tax_payable_or_overpayment,
            WholePeso(-60_999)
        );
        let p2 = &draft.part_ii;
        assert_eq!(p2.item_14_tax_due, WholePeso(59_001));
        assert_eq!(
            p2.item_16_net_tax_payable_or_overpayment,
            WholePeso(-60_999)
        );
        assert_eq!(
            p2.item_21_total_amount_payable_or_overpayment,
            WholePeso(-60_999)
        );
        let fields = draft.field_map();
        assert_eq!(fields["frm1702RT:txtPg2Pt4I42MinimumCorporate"], "59,001");
        assert_eq!(fields["frm1702RT:txtPg2Pt4I43TotalIncomeTax"], "59,001");
        // Where the regular tax is larger, item 43 stays item 41 (at the
        // pre-2020 30%, maintainer decision (pre-2020 1702RT computed like
        // BIR): 30% of 1,510,001 is 453,000.30).
        let mut earning = sample();
        earning.taxable_year = 2019;
        earning.recompute();
        assert_eq!(earning.part_iv.item_42_mcit_due, WholePeso(59_000));
        assert_eq!(earning.part_iv.item_43_tax_due, WholePeso(453_000));
        // Not yet subject to MCIT (incorporated 2017): item 42 is cleared.
        let mut young = sample();
        young.taxable_year = 2019;
        young.incorporation_date = Some(Form1702RTDate {
            year: 2017,
            month: 1,
            day: 2,
        });
        young.recompute();
        assert!(!young.subject_to_mcit());
        assert_eq!(young.part_iv.item_42_mcit_due, ZERO);
        // Rounding is half up on the hundredths, negatives included.
        assert_eq!(two_percent(WholePeso(25)), WholePeso(1));
        assert_eq!(two_percent(WholePeso(24)), WholePeso(0));
        assert_eq!(two_percent(WholePeso(-25)), WholePeso(0));
        assert_eq!(two_percent(WholePeso(-26)), WholePeso(-1));
    }

    /// Maintainer decision (pre-2020 1702RT items 41 and 42 floored at
    /// zero): a 2019 loss year floors item 41 (the net loss carries forward
    /// as NOLCO, so there is no regular tax) and item 42 (MCIT is 2% of
    /// gross income - zero when gross income is zero or less), so item 43
    /// is never negative and no overpayment is manufactured. Replaces the
    /// literal-reading test that pinned a negative item 42.
    #[test]
    fn before_2020_a_loss_year_floors_items_41_and_42_at_zero() {
        let mut draft = sample();
        draft.taxable_year = 2019;
        draft.part_iv.item_30_cost_of_sales_or_services = WholePeso(5_000_000);
        draft.recompute();
        let p4 = &draft.part_iv;
        // 4,900,000 - 5,000,000 + 50,001 = -49,999; 2% of it floors to zero.
        assert_eq!(p4.item_33_total_taxable_income, WholePeso(-49_999));
        assert_eq!(p4.item_42_mcit_due, ZERO);
        // Item 39 is -49,999 - 1,440,000 = -1,489,999; 30% floors to zero.
        assert_eq!(p4.item_39_net_taxable_income_or_loss, WholePeso(-1_489_999));
        assert_eq!(p4.item_41_normal_income_tax_due, ZERO);
        // Item 43 is the larger of the two: zero.
        assert_eq!(p4.item_43_tax_due, ZERO);
        // The overpayment left is exactly the withholding credit the return
        // carries - the tax itself contributes nothing negative.
        assert_eq!(
            p4.item_56_net_tax_payable_or_overpayment,
            WholePeso(-120_000)
        );
        // The penalty engine reads item 16 as its basic tax due; a negative
        // base is clamped (compute_penalties: `basic_tax_due.max(0.0)`), and
        // with no gross-sales basis every component computes zero.
        let computed = Form1702RTDraft::penalty_declaration()
            .if_filed_on(
                &draft,
                chrono::NaiveDate::from_ymd_opt(2020, 5, 15).unwrap(),
            )
            .expect("the 2019 calendar deadline resolves");
        assert_eq!(computed.surcharge, 0.0);
        assert_eq!(computed.interest, 0.0);
        assert_eq!(computed.compromise, 0.0);
        // With no credits there is no overpayment at all: item 16 is zero,
        // the return asks no disposition, and item 43 files "0".
        let mut bare = sample();
        bare.taxable_year = 2019;
        bare.part_iv.item_30_cost_of_sales_or_services = WholePeso(5_000_000);
        bare.part_iv
            .tax_credits
            .item_48_previous_quarter_withholding = ZERO;
        bare.recompute();
        assert_eq!(bare.part_iv.item_43_tax_due, ZERO);
        assert_eq!(bare.part_ii.item_16_net_tax_payable_or_overpayment, ZERO);
        assert_eq!(bare.part_ii.overpayment_disposition, None);
        assert!(
            messages(&bare)
                .iter()
                .all(|message| !message.contains("Overpayment")),
            "{:?}",
            messages(&bare)
        );
        let fields = bare.field_map();
        assert_eq!(fields["frm1702RT:txtPg2Pt4I43TotalIncomeTax"], "0");
        assert_eq!(fields["frm1702RT:txtPg1Pt2I16NetTax"], "0");
    }

    /// Maintainer decision (pre-2020 1702RT computed like BIR): a 2019 return
    /// typed at 25% computes at 30% (item 40
    /// shows 30), so item 41 is 30% of item 39 and items 43, 56, 14, 16 and
    /// 21 follow; item 57 is item 35 times 30 / 100 (calculations.json
    /// 1702rt-item57-special-tax), and an OSD return keeps it cleared.
    #[test]
    fn before_2020_the_rate_is_30_percent_and_item_57_follows() {
        let mut draft = sample();
        draft.taxable_year = 2019;
        draft.schedule_2.rows[0].description = "Seminar".into();
        draft.schedule_2.rows[0].legal_basis = "RA 1".into();
        draft.schedule_2.rows[0].amount = WholePeso(100_001);
        assert_eq!(draft.part_iv.item_40_income_tax_rate_percent, 25);
        draft.recompute();
        let p4 = &draft.part_iv;
        assert_eq!(p4.item_40_income_tax_rate_percent, 30);
        assert_eq!(p4.item_35_special_itemized_deductions, WholePeso(100_001));
        // 2,950,001 - (1,440,000 + 100,001) = 1,410,000; 30% is 423,000.
        assert_eq!(p4.item_39_net_taxable_income_or_loss, WholePeso(1_410_000));
        assert_eq!(p4.item_41_normal_income_tax_due, WholePeso(423_000));
        assert_eq!(p4.item_43_tax_due, WholePeso(423_000));
        // 30% of 100,001 is 30,000.30.
        assert_eq!(
            draft.part_v.item_57_special_allowable_deductions_tax_effect,
            WholePeso(30_000)
        );
        let credits = p4.tax_credits.item_55_total;
        assert_eq!(
            p4.item_56_net_tax_payable_or_overpayment,
            WholePeso(423_000 - credits.0)
        );
        let p2 = &draft.part_ii;
        assert_eq!(p2.item_14_tax_due, WholePeso(423_000));
        assert_eq!(
            p2.item_16_net_tax_payable_or_overpayment,
            p4.item_56_net_tax_payable_or_overpayment
        );
        let fields = draft.field_map();
        assert_eq!(fields["frm1702RT:Pg2Pt4I40IncomeTaxRate"], "30");
        assert_eq!(fields["frm1702RT:txtPg2Pt4I41IncomeTaxDue"], "423,000");
        assert_eq!(fields["frm1702RT:txtPg2Pt5I57SpecialAllowable"], "30,000");
        // OSD: Schedule II and item 57 are cleared; item 41 is 30% of 60%.
        let mut osd = sample();
        osd.taxable_year = 2019;
        osd.deduction_method = Form1702RTDeductionMethod::OptionalStandard;
        osd.recompute();
        assert_eq!(
            osd.part_v.item_57_special_allowable_deductions_tax_effect,
            ZERO
        );
        assert_eq!(
            osd.part_iv.item_41_normal_income_tax_due,
            percent_of(osd.part_iv.item_39_net_taxable_income_or_loss, 30)
        );
    }

    /// Maintainer decisions (pre-2020 MCIT is 2% of gross; pre-2020 1702RT
    /// computed like BIR) leave 2020 and later as they were: item 42
    /// is the filer's entry while MCIT applies, item 40 the typed rate and
    /// item 57 as typed, and the totals are the same as before (the sample's
    /// pinned values, and the 1702RT sample payload hashes in
    /// tests/sample_payloads, are unchanged).
    #[test]
    fn from_2020_item_42_stays_as_entered() {
        for year in [2020, 2025] {
            let mut draft = sample();
            draft.taxable_year = year;
            draft.recompute();
            let p4 = &draft.part_iv;
            assert_eq!(p4.item_42_mcit_due, WholePeso(59_000), "{year}");
            assert_eq!(p4.item_40_income_tax_rate_percent, 25, "{year}");
            assert_eq!(
                p4.item_41_normal_income_tax_due,
                WholePeso(377_500),
                "{year}"
            );
            assert_eq!(p4.item_43_tax_due, WholePeso(377_500), "{year}");
            assert_eq!(
                draft.part_v.item_57_special_allowable_deductions_tax_effect, ZERO,
                "{year}"
            );
            assert_eq!(
                draft.part_ii.item_21_total_amount_payable_or_overpayment,
                WholePeso(257_500),
                "{year}"
            );
        }
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        let p4 = &draft.part_iv;
        assert!(draft.atc.printed_mcit_selected);
        assert_eq!(p4.item_33_total_taxable_income, WholePeso(2_950_001));
        assert_eq!(p4.item_37_total_itemized_deductions, WholePeso(1_440_000));
        assert_eq!(p4.item_39_net_taxable_income_or_loss, WholePeso(1_510_001));
        assert_eq!(p4.item_41_normal_income_tax_due, WholePeso(377_500));
        assert_eq!(p4.item_43_tax_due, WholePeso(377_500));
        assert_eq!(
            draft.part_ii.item_21_total_amount_payable_or_overpayment,
            WholePeso(257_500)
        );
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn optional_standard_deduction_and_net_operating_loss() {
        let mut draft = sample();
        draft.deduction_method = Form1702RTDeductionMethod::OptionalStandard;
        draft.recompute();
        assert_eq!(
            draft.part_iv.item_38_optional_standard_deduction,
            WholePeso(1_180_000)
        );
        assert_eq!(draft.schedule_1.item_18_total, WholePeso(0));

        let mut loss = sample();
        loss.schedule_1.salaries_wages_allowances = WholePeso(4_000_000);
        loss.recompute();
        assert_eq!(
            loss.schedule_3.item_3_net_operating_loss,
            WholePeso(-1_289_999)
        );
        assert_eq!(loss.schedule_3.rows[0].year_incurred, "2025");
        assert_eq!(loss.schedule_3.rows[0].amount, WholePeso(1_289_999));
        assert_eq!(loss.part_iv.item_41_normal_income_tax_due, WholePeso(0));
        assert_eq!(loss.part_iv.item_43_tax_due, WholePeso(59_000));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let mut draft = sample();
        draft
            .part_iv
            .tax_credits
            .item_48_previous_quarter_withholding = WholePeso(500_000);
        draft.part_ii.overpayment_disposition = Some(Form1702RTOverpaymentDisposition::CarryOver);
        draft.recompute();
        let fields = draft.to_official_field_map();
        assert_eq!(fields["frm1702RT:txtPg1Pt2I16NetTax"], "-122,500");
        assert_eq!(fields["frm1702RT:txtPg1I2Year"], "25");
        assert_eq!(fields["frm1702RT:ddlPg1I2Month"], "12");
        assert_eq!(
            fields["frm1702RT:txtPg1Pt1I8Name1"],
            "SAMPLE DUMMY CORPORATION"
        );
        assert_eq!(
            fields["frm1702RT:txtPg1Pt1I12Email"],
            "sample.taxpayer@example.com"
        );
        assert_eq!(fields["frm1702RT:Pg2Pt4I40IncomeTaxRate"], "25");
        assert!(!fields.contains_key("txtFinalFlag"));
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1702RTv2018C-122025#sample.taxpayer@example.com#.xml"
        );
        assert!(draft.to_official_xml_payload().is_ok());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "122025");
        assert_eq!(
            Form1702RTDraft::parse_period_code("122025"),
            Some((2025, FilingPeriod::Annual))
        );
        assert_eq!(Form1702RTDraft::parse_period_code("132025"), None);
        assert_eq!(Form1702RTDraft::parse_period_code("122025Q1"), None);
        assert_eq!(draft.period_column(), 0);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1702RTDraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.month = 0,
            "Please provide a valid Year Ended on Page 1 Item 2",
        );
        // "(Add more...)" rows (addRowModalTable / saveModalTable).
        let modal_row = |description: &str, legal_basis: &str, amount: i64| {
            crate::forms::form_1702rt::Form1702RTSpecialDeductionRow {
                description: description.into(),
                legal_basis: legal_basis.into(),
                amount: WholePeso(amount),
            }
        };
        check(
            &|d| {
                d.more_rows.item_54 = vec![modal_row("Credit", "", 100), modal_row("", "", 50)];
            },
            "Page 2 Item 54.2 Description should not be blank.",
        );
        check(
            &|d| {
                d.more_rows.schedule_2_item_4 = vec![
                    modal_row("Deduction", "RA 1", 100),
                    modal_row("Other", "", 50),
                ];
            },
            "Page 3 Item 4.2 Legal Basis should not be blank.",
        );
        check(
            &|d| {
                d.more_rows.schedule_1_item_17i =
                    vec![modal_row("Expense", "", 100), modal_row("Other", "", 0)];
            },
            "Page 3 Item i.2 Amount should not be zero.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Invalid Year. Year should not be earlier than 2018.",
        );
        check(
            &|d| d.taxable_year = 2099,
            "Year (Page 1 Item 2) cannot be greater than or equal to current year when filing for Calendar Year.",
        );
        check(
            &|d| {
                d.filing_basis = Form1702RTFilingBasis::Fiscal;
                d.month = 12;
            },
            "Date (Page 1 Item 2) Month cannot be equal to December.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please select a RDO code on Page 1 Part 1 Item 7",
        );
        check(
            &|d| d.incorporation_date = None,
            "Please provide a valid Date of Incorporation/Organization on Page 1 Part I Item 10",
        );
        check(
            &|d| d.incorporation_date = Form1702RTDate::new(2026, 1, 1).ok(),
            "Date of Incorporation cannot be greater than Page 1 Item 2 Date.",
        );
        check(
            &|d| d.registered_name_lines[0].clear(),
            "Please provide a Registered Name on Page 1 Part I Item 8",
        );
        check(
            &|d| d.registered_address_lines[0].clear(),
            "Please provide a Registered Address on Page 1 Part I Item 9",
        );
        check(
            &|d| d.contact_number.clear(),
            "Please provide a valid Contact Number on Page 1 Part I Item 11",
        );
        check(
            &|d| d.email.clear(),
            "Please provide a valid Email Address on Page 1 Part I Item 12",
        );
        check(
            &|d| d.email = "not-an-email".into(),
            "You have entered an invalid email address format!",
        );
        check(
            &|d| {
                d.part_iv.tax_credits.item_48_previous_quarter_withholding = WholePeso(500_000);
                d.recompute();
            },
            "Please select an option for Overpayment Radio Button on Page 1 Part II ",
        );
        check(
            &|d| d.payment_details[1].amount = WholePeso(257_500),
            "Please provide data on Page 1 Part III Item 27.",
        );
        check(
            &|d| {
                d.payment_details[0].amount = WholePeso(1_000);
                d.payment_details[0].drawee_bank_or_agency = "BANK".into();
                d.payment_details[0].number = "123".into();
                d.payment_details[0].date = Form1702RTDate::new(2026, 1, 5).ok();
            },
            "Sum of Amount fields in Details of Payment (Page 1 Part III) Segment must be equal to TOTAL AMOUNT PAYABLE",
        );
        check(
            &|d| {
                d.part_iv.tax_credits.item_53_other = Form1702RTNamedAmount {
                    description: String::new(),
                    amount: WholePeso(10),
                };
                d.recompute();
            },
            "Please provide data on Page 2 Part 4 Item 53.",
        );
        check(
            &|d| {
                d.schedule_2.rows[0].amount = WholePeso(10_000);
                d.schedule_2.rows[0].description = "Special".into();
                d.recompute();
            },
            "Please provide data on Page 3 Schedule 2 Item 1.",
        );
        check(
            &|d| {
                d.schedule_2.rows[0].amount = WholePeso(10_000);
                d.schedule_2.rows[0].description = "Special".into();
                d.schedule_2.rows[0].legal_basis = "RA 1".into();
                d.recompute();
            },
            "Please provide a value for this field (Part V Item 57)",
        );
        check(
            &|d| d.schedule_1.other[4].description = "Misc".into(),
            "Please provide data on Page 3 Schedule 1 Item 17g.",
        );
        check(
            &|d| {
                d.schedule_5.item_1_net_income_or_loss_per_books = WholePeso(1);
                d.recompute();
            },
            "Item Page 4 Schedule V Item 10 must be equal Item 39 on Page 2 Part IV",
        );
        check(
            &|d| {
                d.schedule_3.rows[1].year_incurred = "2025".into();
                d.schedule_3.rows[1].amount = WholePeso(1);
            },
            "Page 4 Schedule IIIA Item 5-7 Year Incurred cannot be equal to Page 1 Item 2 Year Ended.",
        );
        check(
            &|d| {
                d.schedule_3.rows[1].year_incurred = "2019".into();
                d.schedule_3.rows[1].amount = WholePeso(1);
            },
            "Year Incurred Cannot be less than 3 years in Page 1 Item 2 (Year Ended)",
        );
        check(
            &|d| {
                d.schedule_4.rows[0].year = "2024".into();
                d.schedule_4.rows[0].mcit = WholePeso(100);
                d.schedule_4.rows[0].applied_current_year = WholePeso(500);
                d.recompute();
            },
            "Page 4, Schedule IV: The sum of Columns D,E & F should not be greater than the amount in Column C. Please re-enter the correct values.",
        );
        check(
            &|d| {
                d.schedule_4.rows[0].year = "2020".into();
            },
            "Please enter a value for the year between 2022 and 2024.",
        );
        check(
            &|d| d.part_iv.item_27_sales = WholePeso(1),
            "Totals are out of date. Recompute the return.",
        );
    }

    #[test]
    fn old_stored_json_still_loads() {
        let mut json = serde_json::to_value(sample()).unwrap();
        let object = json.as_object_mut().unwrap();
        for key in ["queued_submission_fingerprint", "queue_authorization"] {
            object.remove(key);
        }
        for key in [
            "submitted_at",
            "confirmed_at",
            "submission_filename",
            "receipt_id",
            "next_retry_at",
        ] {
            object.insert(key.into(), serde_json::Value::Null);
        }
        object.insert("submission_attempts".into(), serde_json::json!(1));
        object.insert("last_error".into(), serde_json::json!("old"));
        object.insert("status".into(), serde_json::json!("Draft"));
        object.insert(
            "created_at".into(),
            serde_json::json!("2025-04-01T00:00:00+00:00"),
        );
        object.insert(
            "updated_at".into(),
            serde_json::json!("2025-04-02T00:00:00+00:00"),
        );
        let draft: Form1702RTDraft = serde_json::from_value(json).unwrap();
        assert_eq!(draft.lifecycle.status, FilingStatus::Draft);
        assert_eq!(draft.lifecycle.created_at, "2025-04-01T00:00:00+00:00");
        assert_eq!(draft.lifecycle.submission_attempts, 1);
        assert_eq!(draft.last_error.as_deref(), Some("old"));
        let back = serde_json::to_value(&draft).unwrap();
        assert_eq!(back["status"], "Draft");
        assert!(back.get("lifecycle").is_none());
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft.queue(QueueAuthSource::Gui).unwrap();
        assert_eq!(draft.lifecycle.status, FilingStatus::Queued);
        assert!(draft.clone().revalidate_queued_before_submission().is_ok());
        draft.part_ii.item_17_surcharge = WholePeso(99);
        draft.keep_entered_penalties();
        assert!(draft.revalidate_queued_before_submission().is_err());
    }

    #[test]
    fn other_atc_prints_the_election_and_uploads_the_official_default() {
        let mut draft = sample();
        draft.atc = Default::default();
        let shown = |d: &Form1702RTDraft| {
            (
                d.to_print_field_map()["frm1702RT:drpPg1I5AtcOther"].clone(),
                d.to_official_field_map()["frm1702RT:drpPg1I5AtcOther"].clone(),
            )
        };
        // Inactive: nothing elected; the official select keeps IC010.
        assert_eq!(shown(&draft), (String::new(), "IC010".to_string()));
        // "Other" chosen, no code picked: the select's IC010 is the election.
        draft.atc.other_selected = true;
        assert_eq!(shown(&draft), ("IC010".to_string(), "IC010".to_string()));
        // A picked code.
        draft.atc.other_code = "IC011".into();
        assert_eq!(shown(&draft), ("IC011".to_string(), "IC011".to_string()));
        // Cleared again.
        draft.atc.other_selected = false;
        assert_eq!(shown(&draft).0, "");
    }
}
