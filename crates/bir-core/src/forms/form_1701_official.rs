//! Form 1701 (January 2018) on the generic submission path.
//!
//! Ported from the official `BIR-Form1701v2018.hta` (eBIRForms 7.9.6.2.1):
//! the compute chain for both columns (`computeTxtPg2I6` … `computeTxtPg1I32`,
//! Schedules 4–6, Parts VI, VII and IX), the regime switches the ATC choice
//! drives (`TPATCeffect`, `methodRate`, `disableFieldsForGRRate`, …),
//! `validate()` / `validateSpouseFields()` / `initialValidateBeforeSave()`
//! with their alert texts, and `saveXMLsubmit` through [`crate::official_xml`].
//!
//! The official sums (`getSum`, `getDifference`, `differenceOfElements`,
//! `getProduct`) round to whole pesos (`Math.round`), and the tax tables to
//! whole pesos (`toFixed(0)`); typed amounts keep centavos (`round(this,2)`).
//!
//! Supported: taxpayer and joint-filing spouse columns without exempt or
//! special-rate income. Part X (consolidated schedules) and the attachment
//! pages are only used with exempt/special-rate income, so they keep the
//! page's defaults; such a return does not validate here. Part VIII (Tax
//! Relief Availment) is filed on every return: column A as the filer types
//! it, with its totals, column B 0.00 (shut on the official page).
//!
//! The submission is the `saveEncryptedProfile` loop over the page's
//! controls, including the two RDO selects `getRdo()` creates
//! (`txtPg1I5RDOCode`, `txtPg2I2SpouseRDOCode`).

use std::collections::BTreeMap;

use super::form_1701::{
    Form1701AmountPair, Form1701AmountSection, Form1701Atc, Form1701CivilStatus,
    Form1701DeductionMethod, Form1701Draft, Form1701JointFilingStatus, Form1701NolcoRow,
    Form1701OverpaymentDisposition, Form1701Party, Form1701TaxRate,
};
use super::penalty_items::{PenaltyItems, PenaltyRounding};
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};

pub const FORM_1701_LAYOUT_ID: &str = "1701-v2018";
/// Official `formType` (`1701v2018`).
pub const FORM_1701_FORM_TYPE: &str = "1701v2018";

const P: &str = "frm1701:";
const PARTIES: [Form1701Party; 2] = [Form1701Party::Taxpayer, Form1701Party::Spouse];

/// `round(this, 2)` / `formatCurrency`.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// `formatCurrency(Math.round(x).toFixed(2))`.
fn peso(value: f64) -> f64 {
    if value.is_finite() {
        (value + 0.5).floor()
    } else {
        0.0
    }
}

fn amt(value: Option<f64>) -> f64 {
    value.filter(|v| v.is_finite()).unwrap_or(0.0)
}

/// `calcTaxAmt` then `toFixed(0)`.
fn tax_due(year: u16, taxable: f64) -> f64 {
    let tax = if year >= 2023 {
        match taxable {
            t if t <= 250_000.0 => 0.0,
            t if t <= 400_000.0 => (t - 250_000.0) * 0.15,
            t if t <= 800_000.0 => (t - 400_000.0) * 0.20 + 22_500.0,
            t if t <= 2_000_000.0 => (t - 800_000.0) * 0.25 + 102_500.0,
            t if t <= 8_000_000.0 => (t - 2_000_000.0) * 0.30 + 402_500.0,
            t => (t - 8_000_000.0) * 0.35 + 2_202_500.0,
        }
    } else {
        match taxable {
            t if t <= 250_000.0 => 0.0,
            t if t <= 400_000.0 => (t - 250_000.0) * 0.20,
            t if t <= 800_000.0 => (t - 400_000.0) * 0.25 + 30_000.0,
            t if t <= 2_000_000.0 => (t - 800_000.0) * 0.30 + 130_000.0,
            t if t <= 8_000_000.0 => (t - 2_000_000.0) * 0.32 + 490_000.0,
            t => (t - 8_000_000.0) * 0.35 + 2_410_000.0,
        }
    };
    peso(cents(tax))
}

/// The income regime an ATC selects (`TPATCeffect` / `SPATCeffect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Regime {
    CompensationOnly,
    Graduated { compensation: bool },
    EightPercent { compensation: bool },
}

fn regime(atc: Option<Form1701Atc>) -> Option<Regime> {
    Some(match atc? {
        Form1701Atc::Ii011 => Regime::CompensationOnly,
        Form1701Atc::Ii012 | Form1701Atc::Ii014 => Regime::Graduated {
            compensation: false,
        },
        Form1701Atc::Ii013 => Regime::Graduated { compensation: true },
        Form1701Atc::Ii015 | Form1701Atc::Ii017 => Regime::EightPercent {
            compensation: false,
        },
        Form1701Atc::Ii016 => Regime::EightPercent { compensation: true },
    })
}

fn has_compensation(regime: Option<Regime>) -> bool {
    matches!(
        regime,
        Some(
            Regime::CompensationOnly
                | Regime::Graduated { compensation: true }
                | Regime::EightPercent { compensation: true }
        )
    )
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |start: usize, end: usize| {
        digits
            .get(start..end.min(digits.len()))
            .unwrap_or("")
            .to_string()
    };
    (
        part(0, 3),
        part(3, 6),
        part(6, 9),
        digits.get(9..).unwrap_or("").to_string(),
    )
}

impl Form1701Draft {
    /// Joint filing (Items 16–18) turns on the spouse column.
    pub fn spouse_column_active(&self) -> bool {
        self.civil_status == Some(Form1701CivilStatus::Married)
            && self.spouse_has_income == Some(true)
            && self.joint_filing_status == Some(Form1701JointFilingStatus::Joint)
    }

    fn party_atc_official(&self, party: Form1701Party) -> Option<Form1701Atc> {
        match party {
            Form1701Party::Taxpayer => self.atc,
            Form1701Party::Spouse => self
                .spouse_column_active()
                .then_some(self.spouse.atc)
                .flatten(),
        }
    }

    fn party_method(&self, party: Form1701Party) -> Option<Form1701DeductionMethod> {
        match party {
            Form1701Party::Taxpayer => self.deduction_method,
            Form1701Party::Spouse => self.spouse.deduction_method,
        }
    }

    fn get(&self, section: Form1701AmountSection, item: u8, party: Form1701Party) -> f64 {
        amt(self.amount(section, item, party))
    }

    fn put(&mut self, section: Form1701AmountSection, item: u8, party: Form1701Party, value: f64) {
        self.set_amount(section, item, party, Some(value));
    }

    /// A typed amount, held as `round(this,2)` leaves it.
    fn typed(&mut self, section: Form1701AmountSection, item: u8, party: Form1701Party) -> f64 {
        let value = cents(self.get(section, item, party));
        self.put(section, item, party, value);
        value
    }

    /// The official compute chain for both columns, with Items 27-29 on
    /// the shared penalty model: the chain runs once for each column's tax
    /// still due (Item 26), the penalty items file their override, else the
    /// computed value, and the chain runs again so Items 30-32 add up what
    /// is filed.
    pub(super) fn official_recompute(&mut self) {
        self.official_recompute_chain();
        for items in Self::penalty_declarations() {
            // A column not on the return (the spouse's off a joint return)
            // keeps no entry: its Items 27-29 hold 0.00.
            if !(items.applies)(self) {
                let state = (items.state_mut)(self);
                state.clear_overrides();
                state.computed_penalties = None;
                continue;
            }
            // Until PR 7 the 1701 penalties were typed boxes the model never
            // computed. A draft stored then, or an imported save file (no
            // penalty state: version 0), files every amount it holds, a zero
            // too, and is left exactly as stored. Once the filer types one of
            // the column's amounts (Edit penalties writes its override), the
            // others become overrides at the amounts they held and the column
            // joins the model; "Use the computed amounts" then brings the
            // computed values back.
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
        self.official_recompute_chain();
    }

    /// Items 27-29 (Interest, Surcharge, Compromise) of each column on the
    /// shared penalty model ([`super::penalty_items`]): the taxpayer/filer's
    /// (column A), then the spouse's (column B), which apply only on a joint
    /// return (`spouse_column_active`). The engine runs on Item 26, the
    /// amount of tax payable before penalties (Item 31 adds Item 30 to it).
    pub fn penalty_declarations() -> Vec<PenaltyItems<Self>> {
        use crate::penalties::PenaltyComponent as C;
        const fn item(component: C) -> u8 {
            match component {
                C::Surcharge => 28,
                C::Interest => 27,
                C::Compromise => 29,
            }
        }
        let common = PenaltyItems::<Self> {
            form_code: super::form_1701::FORM_CODE,
            // The calendar's deadline, not a typed trigger date (PR 11's D8
            // is for the excise returns only).
            typed_due_date: None,
            party: Some("Taxpayer/Filer"),
            applies: |_| true,
            schedule: crate::penalties::PenaltyProfile::StandardFiling,
            fields: ["item_28_taxpayer", "item_27_taxpayer", "item_29_taxpayer"],
            controls: [
                "frm1701:txtPg1I28A",
                "frm1701:txtPg1I27A",
                "frm1701:txtPg1I29A",
            ],
            labels: ["28 Surcharge", "27 Interest", "29 Compromise"],
            basic_tax_due_label: "Item 26A",
            rounding: PenaltyRounding::Cents,
            get: |d, component| {
                d.get(
                    Form1701AmountSection::PartIi,
                    item(component),
                    Form1701Party::Taxpayer,
                )
            },
            set: |d, component, value| {
                d.put(
                    Form1701AmountSection::PartIi,
                    item(component),
                    Form1701Party::Taxpayer,
                    value,
                );
            },
            basic_tax_due: |d| d.get(Form1701AmountSection::PartIi, 26, Form1701Party::Taxpayer),
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
            applies: |d| d.spouse_column_active(),
            fields: ["item_28_spouse", "item_27_spouse", "item_29_spouse"],
            controls: [
                "frm1701:txtPg1I28B",
                "frm1701:txtPg1I27B",
                "frm1701:txtPg1I29B",
            ],
            basic_tax_due_label: "Item 26B",
            get: |d, component| {
                d.get(
                    Form1701AmountSection::PartIi,
                    item(component),
                    Form1701Party::Spouse,
                )
            },
            set: |d, component, value| {
                d.put(
                    Form1701AmountSection::PartIi,
                    item(component),
                    Form1701Party::Spouse,
                    value,
                );
            },
            basic_tax_due: |d| d.get(Form1701AmountSection::PartIi, 26, Form1701Party::Spouse),
            state: |d| &d.spouse_penalty.state,
            state_mut: |d| &mut d.spouse_penalty.state,
            ..common
        };
        vec![common, spouse]
    }

    /// The official compute chain for both columns (see
    /// [`Self::official_recompute`]).
    fn official_recompute_chain(&mut self) {
        use Form1701AmountSection::*;
        if !self.is_short_period {
            self.period_end_month = 12;
        }
        // The rate radios follow the ATC (TPATCeffect / SPATCeffect).
        self.tax_rate = self.atc.and_then(Form1701Atc::tax_rate);
        if self.tax_rate != Some(Form1701TaxRate::Graduated) {
            self.deduction_method = None;
        }
        if self.spouse_column_active() {
            self.spouse.enabled = true;
            self.spouse.tax_rate = self.spouse.atc.and_then(Form1701Atc::tax_rate);
            if self.spouse.tax_rate != Some(Form1701TaxRate::Graduated) {
                self.spouse.deduction_method = None;
            }
        } else {
            self.spouse.enabled = false;
        }
        if self.claims_foreign_tax_credits != Some(true) {
            self.foreign_tax_number.clear();
        }
        if self.spouse.claims_foreign_tax_credits != Some(true) {
            self.spouse.foreign_tax_number.clear();
        }

        // Employer rows (Schedule 1): amounts held at centavos.
        for row in &mut self.employers {
            row.compensation_income = Some(cents(amt(row.compensation_income)));
            row.tax_withheld = Some(cents(amt(row.tax_withheld)));
        }

        for party in PARTIES {
            let atc = self.party_atc_official(party);
            let regime = regime(atc);
            let active = party == Form1701Party::Taxpayer || self.spouse_column_active();
            let itemized = matches!(regime, Some(Regime::Graduated { .. }))
                && self.party_method(party) == Some(Form1701DeductionMethod::Itemized);
            let osd = matches!(regime, Some(Regime::Graduated { .. }))
                && self.party_method(party) == Some(Form1701DeductionMethod::Osd);
            let year = self.taxable_year;

            // Schedule 1 totals and Schedule 2.
            let (comp, withheld) = if active && has_compensation(regime) {
                self.employers
                    .iter()
                    .filter(|row| row.owner == Some(party))
                    .fold((0.0, 0.0), |(c, w), row| {
                        (c + amt(row.compensation_income), w + amt(row.tax_withheld))
                    })
            } else {
                (0.0, 0.0)
            };
            let comp = cents(comp);
            let withheld = cents(withheld);
            self.put(Schedule2, 4, party, comp);
            let non_taxable = if active && has_compensation(regime) {
                self.typed(Schedule2, 5, party)
            } else {
                0.0
            };
            self.put(Schedule2, 5, party, non_taxable);
            let item_6 = peso(comp - non_taxable);
            self.put(Schedule2, 6, party, item_6);
            let item_7 = if active { tax_due(year, item_6) } else { 0.0 };
            self.put(Schedule2, 7, party, item_7);

            // Schedule 4 (itemized deductions).
            let mut sched4 = 0.0;
            for item in 1..=16 {
                let value = if itemized && active {
                    self.typed(Schedule4, item, party)
                } else {
                    0.0
                };
                self.put(Schedule4, item, party, value);
                sched4 += value;
            }
            for index in 0..4 {
                let pair = &mut self.computations.schedule_4_item_17[index];
                let value = if itemized && active {
                    cents(amt(pair.value(party)))
                } else {
                    0.0
                };
                pair.set(party, Some(value));
                sched4 += value;
            }
            let sched4_total = if itemized && active {
                peso(sched4)
            } else {
                0.0
            };
            self.put(Schedule4, 18, party, sched4_total);

            // Schedule 5 (special deductions).
            let rows = match party {
                Form1701Party::Taxpayer => &mut self.computations.schedule_5_taxpayer,
                Form1701Party::Spouse => &mut self.computations.schedule_5_spouse,
            };
            let mut sched5 = 0.0;
            for row in rows.iter_mut() {
                if itemized && active {
                    row.amount = Some(cents(amt(row.amount)));
                    sched5 += amt(row.amount);
                } else {
                    row.description.clear();
                    row.legal_basis.clear();
                    row.amount = Some(0.0);
                }
            }
            let sched5_total = if itemized && active {
                peso(sched5)
            } else {
                0.0
            };
            match party {
                Form1701Party::Taxpayer => {
                    self.computations.schedule_5_total_taxpayer = Some(sched5_total)
                }
                Form1701Party::Spouse => {
                    self.computations.schedule_5_total_spouse = Some(sched5_total)
                }
            }

            // Schedule 6 (NOLCO).
            let sched6_total = self.recompute_nolco_official(party, itemized && active);

            // Schedule 3.A (graduated rates).
            let graduated = matches!(regime, Some(Regime::Graduated { .. })) && active;
            let mut item_25 = 0.0;
            if graduated {
                let gross = self.typed(Schedule3, 8, party);
                let cost = self.typed(Schedule3, 9, party);
                let item_10 = peso(gross - cost);
                self.put(Schedule3, 10, party, item_10);
                let item_11 = if itemized {
                    self.typed(Schedule3, 11, party)
                } else {
                    0.0
                };
                self.put(Schedule3, 11, party, item_11);
                let item_12 = peso(item_10 - item_11);
                self.put(Schedule3, 12, party, item_12);
                let (i13, i14, i15) = if itemized {
                    (sched4_total, sched5_total, sched6_total)
                } else {
                    (0.0, 0.0, 0.0)
                };
                self.put(Schedule3, 13, party, i13);
                self.put(Schedule3, 14, party, i14);
                self.put(Schedule3, 15, party, i15);
                let item_16 = if itemized { peso(i13 + i14 + i15) } else { 0.0 };
                self.put(Schedule3, 16, party, item_16);
                let item_17 = if osd { peso(item_10 * 0.40) } else { 0.0 };
                self.put(Schedule3, 17, party, item_17);
                let item_18 = if itemized {
                    peso(item_12 - item_16)
                } else if osd {
                    peso(item_10 - item_17)
                } else {
                    0.0
                };
                self.put(Schedule3, 18, party, item_18);
                let other = self.typed(Schedule3, 19, party)
                    + self.typed(Schedule3, 20, party)
                    + self.typed(Schedule3, 21, party);
                let item_22 = peso(other);
                self.put(Schedule3, 22, party, item_22);
                let item_23 = peso(item_18 + item_22);
                self.put(Schedule3, 23, party, item_23);
                let item_24 = peso(item_6 + item_23);
                self.put(Schedule3, 24, party, item_24);
                item_25 = tax_due(year, item_24);
                self.put(Schedule3, 25, party, item_25);
            } else {
                for item in 8..=25 {
                    self.put(Schedule3, item, party, 0.0);
                }
            }

            // Schedule 3.B (8% rate).
            let eight = matches!(regime, Some(Regime::EightPercent { .. })) && active;
            let mut item_32 = 0.0;
            if eight {
                let gross = self.typed(Schedule3, 26, party);
                let other = self.typed(Schedule3, 27, party);
                let item_28 = peso(gross + other);
                self.put(Schedule3, 28, party, item_28);
                let reduction = if matches!(
                    regime,
                    Some(Regime::EightPercent {
                        compensation: false
                    })
                ) {
                    self.typed(Schedule3, 29, party)
                } else {
                    0.0
                };
                self.put(Schedule3, 29, party, reduction);
                let item_30 = peso(item_28 - reduction);
                self.put(Schedule3, 30, party, item_30);
                let item_31 = if item_30 > 0.0 {
                    peso(item_30 * 0.08)
                } else {
                    0.0
                };
                self.put(Schedule3, 31, party, item_31);
                item_32 = peso(item_7 + item_31);
                self.put(Schedule3, 32, party, item_32);
            } else {
                for item in 26..=32 {
                    self.put(Schedule3, item, party, 0.0);
                }
            }

            // Part VI.
            let regular = match regime {
                Some(Regime::Graduated { .. }) if active => item_25,
                Some(Regime::EightPercent { .. }) if active => item_32,
                _ => 0.0,
            };
            self.put(PartVi, 1, party, regular);
            let vi_2 = if active {
                self.typed(PartVi, 2, party)
            } else {
                0.0
            };
            let vi_3 = if active {
                self.typed(PartVi, 3, party)
            } else {
                0.0
            };
            self.put(PartVi, 2, party, vi_2);
            self.put(PartVi, 3, party, vi_3);
            let vi_4 = peso(vi_2 - vi_3);
            self.put(PartVi, 4, party, vi_4);
            let vi_5 = peso(regular + vi_4);
            self.put(PartVi, 5, party, vi_5);

            // Part VII (credits); Item 5 is Schedule 1 tax withheld.
            let mut credits = 0.0;
            for item in 1..=9 {
                let value = if !active {
                    0.0
                } else if item == 5 {
                    withheld
                } else if item == 6 && !self.is_amended {
                    0.0
                } else {
                    self.typed(PartVii, item, party)
                };
                self.put(PartVii, item, party, value);
                credits += value;
            }
            let credits = peso(credits);
            self.put(PartVii, 10, party, credits);

            // Part VIII, Tax Relief Availment (PR 7, preview.1). The tracked
            // runtime inventory
            // (rules/forms/1701-v2018/fixtures/runtime-control-inventory-v796.json)
            // records column A's items 1, 2, 4, 6, 8 and 9 enabled at load
            // (`disabled: false`, source lines 3362-3440) and every column B
            // control disabled at load (`disabled: true`, lines 3363-3447),
            // with no tracked source that enables them: column A takes the
            // filer's amounts, column B stays 0.00 on every return. Items 3,
            // 5, 7 and 10 are computed (calculations.json
            // 1701-part8-running-totals lists them as outputs of the items
            // before them); the operators are the printed captions'
            // (html-frozen/1701-2018 writer-cells and sheet: "Sum of Items 1
            // and 2", "Item 3 Less Item 4", "Sum of Items 5 and 6", "Sum of
            // Items 8 and 9"), rounded to whole pesos as the module's
            // official sums are. Item 6 "(From Part VII Item 8)" stays typed:
            // the inventory records no function that copies Part VII item 8
            // into it. The inventory's source hash (debd0a57...) is not the
            // local mirror's HTA, so the Part VIII function bodies cannot be
            // checked here. Before PR 7 every item was written 0.00: drafts
            // this app created hold zeros there and file as before; an
            // imported return now files its imported Part VIII column A
            // (form_1701_xml.rs keeps it), which was dropped before.
            let viii = |this: &mut Self, item: u8| {
                if party == Form1701Party::Taxpayer {
                    this.typed(PartViii, item, party)
                } else {
                    this.put(PartViii, item, party, 0.0);
                    0.0
                }
            };
            let (i1, i2) = (viii(self, 1), viii(self, 2));
            let i3 = peso(i1 + i2);
            let i4 = viii(self, 4);
            let i5 = peso(i3 - i4);
            let i6 = viii(self, 6);
            let i7 = peso(i5 + i6);
            let (i8, i9) = (viii(self, 8), viii(self, 9));
            let i10 = peso(i8 + i9);
            for (item, value) in [(3, i3), (5, i5), (7, i7), (10, i10)] {
                let value = if party == Form1701Party::Taxpayer {
                    value
                } else {
                    0.0
                };
                self.put(PartViii, item, party, value);
            }

            // Part IX (reconciliation).
            let sum = |items: std::ops::RangeInclusive<u8>, this: &mut Self| {
                let mut total = 0.0;
                for item in items {
                    total += if active {
                        this.typed(PartIx, item, party)
                    } else {
                        0.0
                    };
                    if !active {
                        this.put(PartIx, item, party, 0.0);
                    }
                }
                peso(total)
            };
            let ix_5 = sum(1..=4, self);
            let ix_10 = sum(6..=9, self);
            self.put(PartIx, 5, party, ix_5);
            self.put(PartIx, 10, party, ix_10);
            self.put(PartIx, 11, party, peso(ix_5 - ix_10));

            // Part II (page 1).
            let item_22 = match regime {
                Some(Regime::CompensationOnly) if active => item_7,
                _ => vi_5,
            };
            self.put(PartIi, 22, party, item_22);
            self.put(PartIi, 23, party, credits);
            let item_24 = cents(item_22 - credits);
            self.put(PartIi, 24, party, item_24);
            let item_25_inst = if item_24 > 0.0 && active {
                self.typed(PartIi, 25, party)
            } else {
                0.0
            };
            self.put(PartIi, 25, party, item_25_inst);
            let item_26 = cents(item_24 - item_25_inst);
            self.put(PartIi, 26, party, item_26);
            let mut penalties = 0.0;
            for item in 27..=29 {
                let value = if active {
                    self.typed(PartIi, item, party)
                } else {
                    0.0
                };
                self.put(PartIi, item, party, value);
                penalties += value;
            }
            let item_30_pen = cents(penalties);
            self.put(PartIi, 30, party, item_30_pen);
            let item_31 = if item_26 < 0.0 && item_30_pen > 0.0 {
                item_30_pen
            } else {
                cents(item_26 + item_30_pen)
            };
            self.put(PartIi, 31, party, item_31);
        }
        self.computations.part_ii_item_32_aggregate = Some(cents(
            self.get(Form1701AmountSection::PartIi, 31, Form1701Party::Taxpayer)
                + self.get(Form1701AmountSection::PartIi, 31, Form1701Party::Spouse),
        ));
        let overpaid = self.get(Form1701AmountSection::PartIi, 26, Form1701Party::Taxpayer) < 0.0
            || self.get(Form1701AmountSection::PartIi, 26, Form1701Party::Spouse) < 0.0;
        if !overpaid {
            self.overpayment_disposition = Form1701OverpaymentDisposition::None;
        }
    }

    /// Schedule 6 for one column; returns Item 8D / 13D.
    fn recompute_nolco_official(&mut self, party: Form1701Party, enabled: bool) -> f64 {
        use Form1701AmountSection::Schedule6;
        let year = self.taxable_year;
        if !enabled {
            for item in 1..=3 {
                self.put(Schedule6, item, party, 0.0);
            }
            let rows = match party {
                Form1701Party::Taxpayer => &mut self.computations.schedule_6_taxpayer_nolco,
                Form1701Party::Spouse => &mut self.computations.schedule_6_spouse_nolco,
            };
            for row in rows.iter_mut() {
                *row = zero_nolco();
            }
            match party {
                Form1701Party::Taxpayer => self.computations.schedule_6_total_taxpayer = Some(0.0),
                Form1701Party::Spouse => self.computations.schedule_6_total_spouse = Some(0.0),
            }
            return 0.0;
        }
        let gross = self.typed(Schedule6, 1, party);
        let deductions = self.typed(Schedule6, 2, party);
        let item_3 = peso(gross - deductions);
        self.put(Schedule6, 3, party, item_3);
        let rows = match party {
            Form1701Party::Taxpayer => &mut self.computations.schedule_6_taxpayer_nolco,
            Form1701Party::Spouse => &mut self.computations.schedule_6_spouse_nolco,
        };
        let mut total = 0.0;
        for (index, row) in rows.iter_mut().enumerate() {
            if index == 3 {
                // Current-year net operating loss (Item 7 / 12).
                if item_3 < 0.0 {
                    row.amount = Some(-item_3);
                    row.year_incurred = year.to_string();
                } else {
                    row.amount = Some(0.0);
                    row.year_incurred.clear();
                }
            } else {
                row.amount = Some(cents(amt(row.amount)));
            }
            row.applied_previous_years = Some(cents(amt(row.applied_previous_years)));
            row.expired = Some(cents(amt(row.expired)));
            row.applied_current_year = Some(cents(amt(row.applied_current_year)));
            row.unapplied = Some(peso(
                amt(row.amount)
                    - (amt(row.applied_previous_years)
                        + amt(row.expired)
                        + amt(row.applied_current_year)),
            ));
            total += amt(row.applied_current_year);
        }
        let total = peso(total);
        match party {
            Form1701Party::Taxpayer => self.computations.schedule_6_total_taxpayer = Some(total),
            Form1701Party::Spouse => self.computations.schedule_6_total_spouse = Some(total),
        }
        total
    }

    /// `validate()`, `validateSpouseFields()` and `initialValidateBeforeSave()`
    /// in order with their alert texts, plus the checks the page makes while
    /// typing.
    pub(super) fn official_errors(&self) -> Vec<(String, String)> {
        use Form1701AmountSection::*;
        let mut errors: Vec<(String, String)> = Vec::new();
        let mut err =
            |field: &str, message: &str| errors.push((field.to_string(), message.to_string()));
        let this_year = chrono::Datelike::year(&chrono::Local::now().date_naive());

        // initialValidateBeforeSave
        let digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        if digits.len() < 12
            || digits.len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 4.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&digits[..9]) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        let rdo = self.rdo_code.trim();
        if rdo.is_empty() || rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err("rdo_code", "Please enter a valid RDO Code on Item 5.");
        }
        if self.taxpayer_name.trim().is_empty() || self.taxpayer_name.chars().count() > 50 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 8.",
            );
        }

        // validate()
        if self.is_short_period && !(1..=12).contains(&self.period_end_month) {
            err(
                "period_end_month",
                "Please choose a month on page 1 Item 1.",
            );
        }
        let year = i32::from(self.taxable_year);
        if self.taxable_year == 0 {
            err(
                "taxable_year",
                "Please enter a valid year on page 1 Item 1.",
            );
        } else if year > this_year {
            err(
                "taxable_year",
                "Invalid date entry on page 1 Item 1. Entry should not be later than Current Date.",
            );
        } else if year < 1900 {
            err(
                "taxable_year",
                "Invalid date entry on page 1 Item 1. Entry should not be lower than 1900.",
            );
        } else if year < 2018 {
            err(
                "taxable_year",
                "Please file using the old version of the form.",
            );
        }
        if self.taxpayer_type.is_none() {
            err(
                "taxpayer_type",
                "Please select an option for page 1 Item 6.",
            );
        }
        if self.atc.is_none() {
            err("atc", "Please select an option for page 1 Item 7.");
        }
        let birth = self.date_of_birth.trim();
        if birth.is_empty() {
            err(
                "date_of_birth",
                "Please indicate birth date on page 1 item 10.",
            );
        } else {
            match chrono::NaiveDate::parse_from_str(birth, "%m/%d/%Y") {
                Ok(date) if birth.len() == 10 => {
                    if chrono::Datelike::year(&date) > this_year {
                        err(
                            "date_of_birth",
                            "Birth year on page 1 Item 10 should not be later than current year.",
                        );
                    }
                }
                _ => err(
                    "date_of_birth",
                    "Invalid birth date on page 1 item 10.  Please check date format.",
                ),
            }
        }
        if self.citizenship.trim().is_empty() {
            err(
                "citizenship",
                "Please fill up citizenship on page 1 Item 12.",
            );
        }
        if self.claims_foreign_tax_credits == Some(true)
            && self.foreign_tax_number.trim().is_empty()
        {
            err(
                "foreign_tax_number",
                "Please fill up foreign tax number on page 1 Item 14.",
            );
        }
        if self.civil_status.is_none() {
            err(
                "civil_status",
                "Please select an option for page 1 Item 16.",
            );
        }
        if self.civil_status == Some(Form1701CivilStatus::Married)
            && self.spouse_has_income.is_none()
        {
            err(
                "spouse_has_income",
                "Please select an option for page 1 Item 17.",
            );
        }
        if self.civil_status == Some(Form1701CivilStatus::Married)
            && self.spouse_has_income == Some(true)
            && self.joint_filing_status.is_none()
        {
            err(
                "joint_filing_status",
                "Please select an option for page 1 Item 18.",
            );
        }
        if self.has_exempt_income.is_none() {
            err(
                "has_exempt_income",
                "Please select an option for page 1 Item 19.",
            );
        }
        if self.has_special_rate_income.is_none() {
            err(
                "has_special_rate_income",
                "Please select an option for page 1 Item 20.",
            );
        }
        let tp_regime = regime(self.atc);
        if matches!(tp_regime, Some(Regime::Graduated { .. })) && self.deduction_method.is_none() {
            err(
                "deduction_method",
                "Please select an option for page 1 Item 21A.",
            );
        }
        for party in PARTIES {
            let due = self.get(PartIi, 22, party);
            if self.get(PartIi, 25, party) > due * 50.0 / 100.0 {
                let (field, column) = match party {
                    Form1701Party::Taxpayer => ("item_25_taxpayer", "A"),
                    Form1701Party::Spouse => ("item_25_spouse", "B"),
                };
                err(
                    field,
                    &format!(
                        "Amount in page 1 Item 25{column} cannot be more than 50% of Item 22."
                    ),
                );
            }
        }
        // validatePg3Schd5A
        let sched5 = self
            .computations
            .schedule_5_taxpayer
            .iter()
            .chain(self.computations.schedule_5_spouse.iter())
            .zip([1, 2, 4, 5]);
        for (row, item) in sched5 {
            let incomplete = row.description.trim().is_empty() || row.legal_basis.trim().is_empty();
            let has_text = !row.description.trim().is_empty() || !row.legal_basis.trim().is_empty();
            if (amt(row.amount) > 0.0 && incomplete) || (amt(row.amount) == 0.0 && has_text) {
                err(
                    &format!("schedule_5_{item}"),
                    &format!("Please complete details in Page 3 Schedule 5 Item #{item}."),
                );
            }
        }
        if amt(self.computations.part_ii_item_32_aggregate) < 0.0
            && self.overpayment_disposition == Form1701OverpaymentDisposition::None
        {
            err(
                "overpayment_disposition",
                "Please select an Overpayment option on Page 1 Part II.",
            );
        }
        // compareTaxableIncForTP / SP
        for party in PARTIES {
            if party == Form1701Party::Spouse && !self.spouse_column_active() {
                continue;
            }
            let regime = regime(self.party_atc_official(party));
            let who = match party {
                Form1701Party::Taxpayer => "Taxpayer",
                Form1701Party::Spouse => "Spouse",
            };
            let ix_11 = self.get(PartIx, 11, party);
            if matches!(regime, Some(Regime::Graduated { .. }))
                && self.party_method(party) == Some(Form1701DeductionMethod::Itemized)
                && ix_11 != self.get(Schedule3, 23, party)
            {
                err(
                    &format!("part_ix_11_{}", who.to_lowercase()),
                    &format!(
                        "Page 4 Part IX Item 11 must be equal to Item 23 on Page 2 Part V Schedule 3.A ({who} is under Graduated Rates)"
                    ),
                );
            } else if matches!(regime, Some(Regime::EightPercent { .. }))
                && ix_11 != self.get(Schedule3, 30, party)
            {
                err(
                    &format!("part_ix_11_{}", who.to_lowercase()),
                    &format!(
                        "Page 4 Part IX Item 11 must be equal to Item 30 on Page 3 Part V Schedule 3.B ({who} is under 8% IT Rate)"
                    ),
                );
            }
        }
        // validateSpouseFields
        if self.spouse_column_active() {
            let sp = &self.spouse;
            let (t1, t2, t3, branch) = split_tin(&sp.tin);
            if t1.len() < 3 || t2.len() < 3 || t3.len() < 3 || branch.len() < 3 {
                err(
                    "spouse_tin",
                    "You have entered an invalid TIN format for Spouse.",
                );
            } else if !crate::validation::relaxed_dev_mode()
                && crate::validation::official_tin_check_code(&format!("{t1}{t2}{t3}")) != 0
            {
                err(
                    "spouse_tin",
                    &format!(
                        "{} on Page 2 Part IV Item 1.",
                        crate::validation::OFFICIAL_INVALID_TIN_MESSAGE
                    ),
                );
            }
            let sp_rdo = self.spouse_rdo_code.trim();
            if sp_rdo.is_empty()
                || sp_rdo == "000"
                || !crate::validation::rdo_code_is_official_option(sp_rdo)
            {
                err(
                    "spouse_rdo_code",
                    "Please enter a valid RDO Code on Page 2 Part IV Item 2.",
                );
            }
            if sp.filer_type.is_none() {
                err(
                    "spouse_type",
                    "Please select a Filer type for Spouse on Page 2 Part IV Item 3.",
                );
            }
            if sp.atc.is_none() {
                err(
                    "spouse_atc",
                    "Please select an ATC code for Spouse on Page 2 Part IV Item 4.",
                );
            }
            if sp.name.trim().is_empty() {
                err(
                    "spouse_name",
                    "Please enter a name for Spouse on Page 2 Part IV Item 5.",
                );
            }
            let phone = sp.contact_number.trim();
            if phone.is_empty() {
                err(
                    "spouse_contact_number",
                    "Please enter a contact number for Spouse on Page 2 Part IV Item 6.",
                );
            } else if phone.len() <= 5
                || !phone.bytes().all(|b| b.is_ascii_digit())
                || phone.len() > 20
            {
                err(
                    "spouse_contact_number",
                    "Please enter a valid contact number for Spouse on Page 2 Part IV Item 6.",
                );
            }
            if sp.citizenship.trim().is_empty() {
                err(
                    "spouse_citizenship",
                    "Please enter a citizenship for Spouse on Page 2 Part IV Item 7.",
                );
            }
            if sp.claims_foreign_tax_credits.is_none() {
                err(
                    "spouse_claims_foreign_tax_credits",
                    "Please select a Foreign Tax Credit for Spouse on Page 2 Part IV Item 8.",
                );
            }
            if sp.claims_foreign_tax_credits == Some(true)
                && sp.foreign_tax_number.trim().is_empty()
            {
                err(
                    "spouse_foreign_tax_number",
                    "Please select a Foreign Tax Number for Spouse on Page 2 Part IV Item 9.",
                );
            }
            if sp.has_exempt_income.is_none() {
                err(
                    "spouse_has_exempt_income",
                    "Please select an option for Spouse on Page 2 Part IV Item 10.",
                );
            }
            if sp.has_special_rate_income.is_none() {
                err(
                    "spouse_has_special_rate_income",
                    "Please select an option for Spouse on Page 2 Part IV Item 11.",
                );
            }
            if matches!(regime(sp.atc), Some(Regime::Graduated { .. }))
                && sp.deduction_method.is_none()
            {
                err(
                    "spouse_deduction_method",
                    "Please select a Method of Deduction for Spouse on Page 2 Part IV Item 12A.",
                );
            }
            if sp.has_exempt_income == Some(true) || sp.has_special_rate_income == Some(true) {
                err(
                    "spouse_part_x",
                    "Exempt or special-rate income needs Part X and its attachments, which cannot be filed electronically here yet.",
                );
            }
        }

        // TPFilerType / TPATCcodes: the ticked filer types enable specific ATCs.
        if let (Some(kind), Some(atc)) = (self.taxpayer_type, self.atc) {
            use super::form_1701::Form1701TaxpayerType as T;
            let mixed = self.taxpayer_also_compensation_earner;
            let allowed: &[Form1701Atc] = match (kind, mixed) {
                (T::SingleProprietor | T::Professional, true) => {
                    &[Form1701Atc::Ii013, Form1701Atc::Ii016]
                }
                (T::SingleProprietor | T::Estate | T::Trust, false) => {
                    &[Form1701Atc::Ii012, Form1701Atc::Ii015]
                }
                (T::Professional, false) => &[Form1701Atc::Ii014, Form1701Atc::Ii017],
                (T::CompensationEarner, _) => &[Form1701Atc::Ii011],
                (T::Estate | T::Trust, true) => &[],
            };
            if allowed.is_empty() {
                err(
                    "taxpayer_type",
                    "The combination of the chosen filer type is invalid.",
                );
            } else if !allowed.contains(&atc) {
                err(
                    "atc",
                    "Item 7 ATC is not available for the Item 6 taxpayer type.",
                );
            }
        }
        if self.spouse_column_active()
            && let (Some(kind), Some(atc)) = (self.spouse.filer_type, self.spouse.atc)
        {
            use super::form_1701::Form1701SpouseType as ST;
            let mixed = self.spouse_also_compensation_earner;
            let allowed: &[Form1701Atc] = match (kind, mixed) {
                (ST::SingleProprietor | ST::Professional, true) => {
                    &[Form1701Atc::Ii013, Form1701Atc::Ii016]
                }
                (ST::SingleProprietor, false) => &[Form1701Atc::Ii012, Form1701Atc::Ii015],
                (ST::Professional, false) => &[Form1701Atc::Ii014, Form1701Atc::Ii017],
                (ST::CompensationEarner, _) => &[Form1701Atc::Ii011],
            };
            if !allowed.contains(&atc) {
                err(
                    "spouse_atc",
                    "Spouse Item 4 ATC is not available for the spouse's Item 3 type.",
                );
            }
        }
        if self.has_exempt_income == Some(true) || self.has_special_rate_income == Some(true) {
            err(
                "part_x",
                "Exempt or special-rate income needs Part X and its attachments, which cannot be filed electronically here yet.",
            );
        }
        // Typing limits and per-field alerts.
        for party in PARTIES {
            if self.get(Schedule3, 26, party) > 3_000_000.0 {
                err(
                    "schedule_3_26",
                    "Your Gross Sales/Receipts and Other Non-Operating Income exceeds VAT Threshold (P3M), thus, not qualified to 8% tax rate and shall be subjected to graduated rates. Please choose a graduated rate ATC and fill in Page 2 Schedule 3.",
                );
            }
            if self.get(Schedule3, 29, party) > 250_000.0 {
                err("schedule_3_29", "Amount cannot be more than 250,000.");
            }
            if self.get(Schedule6, 2, party) < self.get(Schedule6, 1, party)
                && self.get(Schedule6, 2, party) != 0.0
            {
                // computePg3Sc6I3 only reacts when deductions are below gross
                // income on its own blur; mirror the message as a review hint.
            }
        }
        let nolco = self
            .computations
            .schedule_6_taxpayer_nolco
            .iter()
            .chain(self.computations.schedule_6_spouse_nolco.iter())
            .enumerate();
        for (index, row) in nolco {
            let pos = index % 4;
            if amt(row.amount)
                < amt(row.applied_previous_years) + amt(row.expired) + amt(row.applied_current_year)
            {
                err(
                    "schedule_6",
                    "Amount is invalid. Sum of Column B, C and D shall not be greater than the amount in Column E (Net Operating Loss)",
                );
            }
            if pos < 3 && !row.year_incurred.trim().is_empty() {
                match row.year_incurred.trim().parse::<i32>() {
                    Ok(y) if y < year - 3 => err(
                        "schedule_6",
                        "Year incurred cannot be more than 3 years of current year.",
                    ),
                    Ok(y) if y > year => err(
                        "schedule_6",
                        "Year incurred in this field cannot be a future year.",
                    ),
                    Ok(y) if y == year => err(
                        "schedule_6",
                        "Year incurred in this field cannot be the same as current year.",
                    ),
                    Ok(_) => {}
                    Err(_) => err("schedule_6", "Year incurred must be a 4-digit year."),
                }
            }
        }
        for (field, value) in [
            ("taxpayer_name", &self.taxpayer_name),
            ("citizenship", &self.citizenship),
            ("foreign_tax_number", &self.foreign_tax_number),
            ("spouse_name", &self.spouse.name),
        ] {
            let limit = match field {
                "taxpayer_name" | "spouse_name" => 50,
                _ => 20,
            };
            if value.chars().count() > limit {
                err(
                    field,
                    &format!("This entry holds at most {limit} characters."),
                );
            }
        }
        if self.registered_address.chars().count() > 200 {
            err(
                "registered_address",
                "The registered address holds at most 200 characters.",
            );
        }
        let phone = self.contact_number.trim();
        if phone.len() > 20 || !phone.bytes().all(|b| b.is_ascii_digit()) {
            err(
                "contact_number",
                "The contact number holds up to 20 digits.",
            );
        }
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }
        for (index, row) in self.employers.iter().enumerate() {
            if row.owner.is_some() && !self.employer_row_allowed(row.owner.unwrap()) {
                err(
                    &format!("employer_{}", index + 1),
                    "Schedule 1 applies only to compensation income (ATC II011, II013 or II016).",
                );
            }
            if row.owner.is_none()
                && (amt(row.compensation_income) != 0.0 || amt(row.tax_withheld) != 0.0)
            {
                err(
                    &format!("employer_{}", index + 1),
                    "Mark whether Schedule 1 row is for the taxpayer or the spouse.",
                );
            }
        }
        if !matches!(
            tp_regime,
            Some(Regime::Graduated { .. }) | Some(Regime::EightPercent { .. })
        ) && amt(self.amount(PartIx, 11, Form1701Party::Taxpayer)) != 0.0
        {
            // Part IX applies with business income only; nothing to check.
        }
        // Derived items must be what the official chain produces.
        let mut expected = self.clone();
        expected.official_recompute();
        if expected.computations != self.computations || expected.employers != self.employers {
            err(
                "computations",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }

    fn employer_row_allowed(&self, owner: Form1701Party) -> bool {
        has_compensation(regime(self.party_atc_official(owner)))
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id
    /// (layout keys; see the module notes for the RDO selects).
    pub fn to_official_field_map(&self) -> BTreeMap<String, String> {
        use Form1701AmountSection::*;
        let mut f: BTreeMap<String, String> = BTreeMap::new();
        let flag = |on: bool| on.to_string();
        let money = |v: f64| official_amount(v);
        let put = |f: &mut BTreeMap<String, String>, key: &str, value: String| {
            f.insert(format!("{P}{key}"), value);
        };
        let up = |s: &str| s.trim().to_uppercase();

        put(
            &mut f,
            "txtPg1I1Month",
            format!("{:02}", self.period_end_month),
        );
        put(&mut f, "txtPg1I1Year", self.taxable_year.to_string());
        put(&mut f, "rdoPg1I2AmendedYes", flag(self.is_amended));
        put(&mut f, "rdoPg1I2AmendedNo", flag(!self.is_amended));
        put(&mut f, "rdoPg1I3ShortPeriodYes", flag(self.is_short_period));
        put(&mut f, "rdoPg1I3ShortPeriodNo", flag(!self.is_short_period));
        let (t1, t2, t3, branch) = split_tin(&self.tin);
        for prefix in [
            "txtPg1I4", "txtPg2", "txtPg3", "txtPg4", "txtPg1m", "txtPg2m", "txtPg3m", "txtPg4m",
        ] {
            put(&mut f, &format!("{prefix}TIN1"), t1.clone());
            put(&mut f, &format!("{prefix}TIN2"), t2.clone());
            put(&mut f, &format!("{prefix}TIN3"), t3.clone());
            put(&mut f, &format!("{prefix}BranchCode"), branch.clone());
        }
        let short_name = self.page_header_name();
        for page in [
            "txtPg2", "txtPg3", "txtPg4", "txtPg1m", "txtPg2m", "txtPg3m", "txtPg4m",
        ] {
            put(&mut f, &format!("{page}TaxpayerName"), short_name.clone());
        }
        put(&mut f, "txtPg1I5RDOCode", self.rdo_code.trim().to_string());
        put(&mut f, "txtPg2I2SpouseRDOCode", self.spouse_rdo_value());
        use super::form_1701::Form1701TaxpayerType as T;
        let mixed = self.taxpayer_also_compensation_earner
            && matches!(
                self.taxpayer_type,
                Some(T::SingleProprietor | T::Professional)
            );
        for (suffix, kind) in [
            ("S", T::SingleProprietor),
            ("P", T::Professional),
            ("E", T::Estate),
            ("T", T::Trust),
            ("C", T::CompensationEarner),
        ] {
            let ticked =
                self.taxpayer_type == Some(kind) || (mixed && kind == T::CompensationEarner);
            put(
                &mut f,
                &format!("rdoPg1I6TaxpayerType{suffix}"),
                flag(ticked),
            );
        }
        for atc in Form1701Atc::ALL {
            put(
                &mut f,
                &format!("rdoPg1I7ATC_{}", atc.code()),
                flag(self.atc == Some(atc)),
            );
        }
        put(&mut f, "txtPg1I8TaxpayerName", up(&self.taxpayer_name));
        let address = up(&self.registered_address);
        let split = address
            .char_indices()
            .nth(100)
            .map_or(address.len(), |(at, _)| at);
        put(&mut f, "txtPg1I9Address", address[..split].to_string());
        put(&mut f, "txtPg1I9Address2", address[split..].to_string());
        put(&mut f, "txtPg1I9AZipCode", self.zip_code.trim().to_string());
        put(
            &mut f,
            "txtPg1I10BirthDate",
            self.date_of_birth.trim().to_string(),
        );
        f.insert("txtEmail".to_string(), self.email.trim().to_string());
        put(&mut f, "txtPg1I12Citizenship", up(&self.citizenship));
        let yes_no =
            |f: &mut BTreeMap<String, String>, yes: &str, no: &str, value: Option<bool>| {
                f.insert(format!("{P}{yes}"), flag(value == Some(true)));
                f.insert(format!("{P}{no}"), flag(value == Some(false)));
            };
        yes_no(
            &mut f,
            "rdoPg1I13ForeignTaxCreditsYes",
            "rdoPg1I13ForeignTaxCreditsNo",
            self.claims_foreign_tax_credits,
        );
        put(
            &mut f,
            "txtPg1I14ForeignTaxNumber",
            up(&self.foreign_tax_number),
        );
        put(
            &mut f,
            "txtPg1I15TelNum",
            self.contact_number.trim().to_string(),
        );
        for (suffix, status) in [
            ("S", Form1701CivilStatus::Single),
            ("M", Form1701CivilStatus::Married),
            ("LS", Form1701CivilStatus::LegallySeparated),
            ("W", Form1701CivilStatus::Widowed),
        ] {
            put(
                &mut f,
                &format!("rdoPg1I16CivilStatus{suffix}"),
                flag(self.civil_status == Some(status)),
            );
        }
        let married = self.civil_status == Some(Form1701CivilStatus::Married);
        yes_no(
            &mut f,
            "rdoPg1I17SpouseIncomeYes",
            "rdoPg1I17SpouseIncomeNo",
            married.then_some(self.spouse_has_income).flatten(),
        );
        let filing = (married && self.spouse_has_income == Some(true))
            .then_some(self.joint_filing_status)
            .flatten();
        put(
            &mut f,
            "rdoPg1I18FilingStatusJ",
            flag(filing == Some(Form1701JointFilingStatus::Joint)),
        );
        put(
            &mut f,
            "rdoPg1I18FilingStatusS",
            flag(filing == Some(Form1701JointFilingStatus::Separate)),
        );
        yes_no(
            &mut f,
            "rdoPg1I19IncomeExemptYes",
            "rdoPg1I19IncomeExemptNo",
            self.has_exempt_income,
        );
        yes_no(
            &mut f,
            "rdoPg1I20IncomeSpecialYes",
            "rdoPg1I20IncomeSpecialNo",
            self.has_special_rate_income,
        );
        let rate_flags = |f: &mut BTreeMap<String, String>,
                          prefix: &str,
                          rate: Option<Form1701TaxRate>,
                          method: Option<Form1701DeductionMethod>| {
            f.insert(
                format!("{P}{prefix}TaxRateG"),
                flag(rate == Some(Form1701TaxRate::Graduated)),
            );
            f.insert(
                format!("{P}{prefix}AMethodDeductionI"),
                flag(method == Some(Form1701DeductionMethod::Itemized)),
            );
            f.insert(
                format!("{P}{prefix}AMethodDeductionO"),
                flag(method == Some(Form1701DeductionMethod::Osd)),
            );
            f.insert(
                format!("{P}{prefix}TaxRateP"),
                flag(rate == Some(Form1701TaxRate::EightPercent)),
            );
        };
        rate_flags(&mut f, "rdoPg1I21", self.tax_rate, self.deduction_method);

        let pair = |section, item| -> (String, String) {
            (
                money(self.get(section, item, Form1701Party::Taxpayer)),
                money(self.get(section, item, Form1701Party::Spouse)),
            )
        };
        for (item, key_a, key_b) in [
            (22, "txtPg1I22ATaxDue", "txtPg1I22BTaxDue"),
            (23, "txtPg1I23A", "txtPg1I23B"),
            (24, "txtPg1I24ATaxPayable", "txtPg1I24BTaxPayable"),
            (25, "txtPg1I25A", "txtPg1I25B"),
            (26, "txtPg1I26A", "txtPg1I26B"),
            (27, "txtPg1I27A", "txtPg1I27B"),
            (28, "txtPg1I28A", "txtPg1I28B"),
            (29, "txtPg1I29A", "txtPg1I29B"),
            (30, "txtPg1I30A", "txtPg1I30B"),
            (31, "txtPg1I31ATotalAmtPyble", "txtPg1I31BTotalAmtPyble"),
        ] {
            let (a, b) = pair(PartIi, item);
            put(&mut f, key_a, a);
            put(&mut f, key_b, b);
        }
        put(
            &mut f,
            "txtPg1I32AggregateAmtPyble",
            money(amt(self.computations.part_ii_item_32_aggregate)),
        );
        let over = self.overpayment_disposition;
        put(
            &mut f,
            "rdoPg1OverpaymentRefund",
            flag(over == Form1701OverpaymentDisposition::Refund),
        );
        put(
            &mut f,
            "rdoPg1OverpaymentTCC",
            flag(over == Form1701OverpaymentDisposition::TaxCreditCertificate),
        );
        put(
            &mut f,
            "rdoPg1OverpaymentCarryOver",
            flag(over == Form1701OverpaymentDisposition::CarryOver),
        );
        put(
            &mut f,
            "txtPg1I33NumberOfAttachments",
            format!("{:02}", self.number_of_attachments.unwrap_or(0)),
        );

        // Page 2: spouse background.
        let sp = &self.spouse;
        let joint = self.spouse_column_active();
        let (s1, s2, s3, sb) = if joint {
            split_tin(&sp.tin)
        } else {
            Default::default()
        };
        put(&mut f, "txtPg2I1TIN1", s1);
        put(&mut f, "txtPg2I1TIN2", s2);
        put(&mut f, "txtPg2I1TIN3", s3);
        put(&mut f, "txtPg2I1BranchCode", sb);
        use super::form_1701::Form1701SpouseType as ST;
        let sp_mixed = self.spouse_also_compensation_earner
            && matches!(sp.filer_type, Some(ST::SingleProprietor | ST::Professional));
        for (suffix, kind) in [
            ("S", ST::SingleProprietor),
            ("P", ST::Professional),
            ("C", ST::CompensationEarner),
        ] {
            let ticked =
                sp.filer_type == Some(kind) || (sp_mixed && kind == ST::CompensationEarner);
            put(
                &mut f,
                &format!("rdoPg2I3SpouseType{suffix}"),
                flag(joint && ticked),
            );
        }
        for atc in Form1701Atc::ALL {
            put(
                &mut f,
                &format!("rdoPg2I4ATC_{}", atc.code()),
                flag(joint && sp.atc == Some(atc)),
            );
        }
        let text_if = |value: &str| if joint { up(value) } else { String::new() };
        put(&mut f, "txtPg2I5SpouseName", text_if(&sp.name));
        put(
            &mut f,
            "txtPg2I6TelNum",
            if joint {
                sp.contact_number.trim().to_string()
            } else {
                String::new()
            },
        );
        put(&mut f, "txtPg2I7Citizenship", text_if(&sp.citizenship));
        yes_no(
            &mut f,
            "rdoPg2I8ForeignTaxCreditsYes",
            "rdoPg2I8ForeignTaxCreditsNo",
            joint.then_some(sp.claims_foreign_tax_credits).flatten(),
        );
        put(
            &mut f,
            "txtPg2I9ForeignTaxNumber",
            text_if(&sp.foreign_tax_number),
        );
        yes_no(
            &mut f,
            "rdoPg2I10IncomeExemptYes",
            "rdoPg2I10IncomeExemptNo",
            joint.then_some(sp.has_exempt_income).flatten(),
        );
        yes_no(
            &mut f,
            "rdoPg2I11IncomeSpecialYes",
            "rdoPg2I11IncomeSpecialNo",
            joint.then_some(sp.has_special_rate_income).flatten(),
        );
        rate_flags(
            &mut f,
            "rdoPg2I12",
            joint.then_some(sp.tax_rate).flatten(),
            joint.then_some(sp.deduction_method).flatten(),
        );

        // Schedule 1 (employers).
        for (index, row) in self.employers.iter().enumerate() {
            let n = index + 1;
            let owner = row.owner.filter(|owner| self.employer_row_allowed(*owner));
            let base = format!("txtPg2IShed{n}a");
            put(
                &mut f,
                &format!("chkPg2IShed{n}a_{n}Taxpayer"),
                flag(owner == Some(Form1701Party::Taxpayer)),
            );
            put(
                &mut f,
                &format!("chkPg2IShed{n}a_{n}Spouse"),
                flag(owner == Some(Form1701Party::Spouse)),
            );
            put(
                &mut f,
                &format!("{base}_{n}TPName"),
                if owner == Some(Form1701Party::Taxpayer) {
                    up(&row.employer_name)
                } else {
                    String::new()
                },
            );
            put(
                &mut f,
                &format!("{base}_{n}SName"),
                if owner == Some(Form1701Party::Spouse) {
                    up(&row.employer_name)
                } else {
                    String::new()
                },
            );
            let (e1, e2, e3, eb) = if owner.is_some() {
                split_tin(&row.employer_tin)
            } else {
                Default::default()
            };
            put(&mut f, &format!("{base}_TIN1"), e1);
            put(&mut f, &format!("{base}_TIN2"), e2);
            put(&mut f, &format!("{base}_TIN3"), e3);
            put(&mut f, &format!("{base}_BranchCode"), eb);
            let (ci, tw) = if owner.is_some() {
                (amt(row.compensation_income), amt(row.tax_withheld))
            } else {
                (0.0, 0.0)
            };
            put(&mut f, &format!("txtPg2IShed1c_{n}CI"), money(ci));
            put(&mut f, &format!("txtPg2IShed1c_{n}TW"), money(tw));
        }
        for (suffix, party) in [
            ("3A", Form1701Party::Taxpayer),
            ("3B", Form1701Party::Spouse),
        ] {
            put(
                &mut f,
                &format!("txtPg2IShed1c_{suffix}CI"),
                money(self.get(Schedule2, 4, party)),
            );
            put(
                &mut f,
                &format!("txtPg2IShed1c_{suffix}TW"),
                money(self.get(PartVii, 5, party)),
            );
        }
        let named = |f: &mut BTreeMap<String, String>, prefix: &str, section, item| {
            f.insert(
                format!("{P}{prefix}A"),
                money(self.get(section, item, Form1701Party::Taxpayer)),
            );
            f.insert(
                format!("{P}{prefix}B"),
                money(self.get(section, item, Form1701Party::Spouse)),
            );
        };
        for item in 4..=7 {
            named(&mut f, &format!("txtPg2IShed2_{item}"), Schedule2, item);
        }
        for item in 8..=25 {
            named(&mut f, &format!("txtPg2IShed3_{item}"), Schedule3, item);
        }
        let descs = &self.computations.schedule_3_descriptions;
        let desc = |item: u8| up(descs.get(&item).map(String::as_str).unwrap_or(""));
        let any_graduated = PARTIES.iter().any(|p| {
            matches!(
                regime(self.party_atc_official(*p)),
                Some(Regime::Graduated { .. })
            )
        });
        let any_eight = PARTIES.iter().any(|p| {
            matches!(
                regime(self.party_atc_official(*p)),
                Some(Regime::EightPercent { .. })
            )
        });
        put(
            &mut f,
            "txtPg2IShed3_19Desc",
            if any_graduated {
                desc(19)
            } else {
                String::new()
            },
        );
        put(
            &mut f,
            "txtPg2IShed3_20Desc",
            if any_graduated {
                desc(20)
            } else {
                String::new()
            },
        );
        for item in 26..=32 {
            named(&mut f, &format!("txtPg3IShed3_{item}"), Schedule3, item);
        }
        put(
            &mut f,
            "txtPg3IShed3_27Desc",
            if any_eight { desc(27) } else { String::new() },
        );
        for item in 1..=16 {
            named(&mut f, &format!("txtPg3IShed4_{item}"), Schedule4, item);
        }
        for (index, suffix) in ["17a", "17b", "17c", "17d"].iter().enumerate() {
            let pair: &Form1701AmountPair = &self.computations.schedule_4_item_17[index];
            put(
                &mut f,
                &format!("txtPg3IShed4_{suffix}A"),
                money(amt(pair.taxpayer)),
            );
            put(
                &mut f,
                &format!("txtPg3IShed4_{suffix}B"),
                money(amt(pair.spouse)),
            );
        }
        let itemized_any = PARTIES.iter().any(|p| {
            matches!(
                regime(self.party_atc_official(*p)),
                Some(Regime::Graduated { .. })
            ) && self.party_method(*p) == Some(Form1701DeductionMethod::Itemized)
        });
        put(
            &mut f,
            "txtPg3IShed4_17dDesc",
            if itemized_any {
                up(&self.computations.schedule_4_item_17d_description)
            } else {
                String::new()
            },
        );
        named(&mut f, "txtPg3IShed4_18", Schedule4, 18);
        for (rows, first) in [
            (&self.computations.schedule_5_taxpayer, 1usize),
            (&self.computations.schedule_5_spouse, 4usize),
        ] {
            for (offset, row) in rows.iter().enumerate() {
                let n = first + offset;
                put(
                    &mut f,
                    &format!("txtPg3IShed5_{n}Desc"),
                    up(&row.description),
                );
                put(
                    &mut f,
                    &format!("txtPg3IShed5_{n}Legal"),
                    up(&row.legal_basis),
                );
                put(
                    &mut f,
                    &format!("txtPg3IShed5_{n}Amt"),
                    money(amt(row.amount)),
                );
            }
        }
        put(
            &mut f,
            "txtPg3IShed5_3",
            money(amt(self.computations.schedule_5_total_taxpayer)),
        );
        put(
            &mut f,
            "txtPg3IShed5_6",
            money(amt(self.computations.schedule_5_total_spouse)),
        );
        for item in 1..=3 {
            named(&mut f, &format!("txtPg3IShed6_{item}"), Schedule6, item);
        }
        let nolco =
            |f: &mut BTreeMap<String, String>, page: u8, item: usize, row: &Form1701NolcoRow| {
                let prefix = format!("{P}txtPg{page}IShed6_{item}");
                f.insert(
                    format!("{prefix}Year"),
                    row.year_incurred.trim().to_string(),
                );
                for (suffix, value) in [
                    ("A", row.amount),
                    ("B", row.applied_previous_years),
                    ("C", row.expired),
                    ("D", row.applied_current_year),
                    ("E", row.unapplied),
                ] {
                    f.insert(format!("{prefix}{suffix}"), money(amt(value)));
                }
            };
        for (index, row) in self
            .computations
            .schedule_6_taxpayer_nolco
            .iter()
            .enumerate()
        {
            nolco(&mut f, 3, index + 4, row);
        }
        put(
            &mut f,
            "txtPg3IShed6_8D",
            money(amt(self.computations.schedule_6_total_taxpayer)),
        );
        for (index, row) in self.computations.schedule_6_spouse_nolco.iter().enumerate() {
            nolco(&mut f, 4, index + 9, row);
        }
        put(
            &mut f,
            "txtPg4IShed6_13D",
            money(amt(self.computations.schedule_6_total_spouse)),
        );
        for item in 1..=5 {
            named(&mut f, &format!("txtPg4ISc6_{item}"), PartVi, item);
        }
        for item in 1..=10 {
            named(&mut f, &format!("txtPg4IPart7_{item}"), PartVii, item);
        }
        put(
            &mut f,
            "txtPg4IPart7_9Specify",
            up(&self.computations.part_vii_item_9_description),
        );
        for item in 1..=10 {
            named(&mut f, &format!("txtPg4IPart8_{item}"), PartViii, item);
        }
        for item in 1..=11 {
            named(&mut f, &format!("txtPg4IPart9_{item}"), PartIx, item);
        }
        for item in [2u8, 3, 4, 6, 7, 8, 9] {
            put(
                &mut f,
                &format!("txtPg4IPart9_{item}Particulars"),
                up(self
                    .computations
                    .part_ix_descriptions
                    .get(&item)
                    .map(String::as_str)
                    .unwrap_or("")),
            );
        }
        // Payment details are filled at the bank, never on the page.
        put(&mut f, "txtCurrentPage", "1".to_string());
        // capital() uppercases every text control, this flag's "false" too.
        put(&mut f, "txtIsTaxFilerDisabled", "FALSE".to_string());
        f.insert(format!("{P}txtLineBus"), up(&self.line_of_business));
        f
    }

    /// The exact official submit plaintext.
    pub fn to_official_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

fn zero_nolco() -> Form1701NolcoRow {
    Form1701NolcoRow {
        year_incurred: String::new(),
        amount: Some(0.0),
        applied_previous_years: Some(0.0),
        expired: Some(0.0),
        applied_current_year: Some(0.0),
        unapplied: Some(0.0),
    }
}

impl QueueableForm for Form1701Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        Self::penalty_declarations()
    }
    /// A column still on the typed boxes (a draft stored before PR 7, or an
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
    const FORM_CODE: &'static str = "1701";
    const FORM_TYPE: &'static str = FORM_1701_FORM_TYPE;
    const LAYOUT_ID: &'static str = FORM_1701_LAYOUT_ID;

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
    /// `"12" + txtPg1I1Year`, as `createXMLFileName` writes it.
    fn period_code(&self) -> String {
        format!("12{}", self.taxable_year)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 6 || code.get(..2)? != "12" {
            return None;
        }
        let year: u16 = code.get(2..)?.parse().ok()?;
        Some((year, FilingPeriod::Annual))
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
}

impl Form1701Draft {
    /// The page 2-4 header name ("Taxpayer/Filer's Last Name"): the
    /// official loadBGData shows the registered name up to its first comma.
    pub fn page_header_name(&self) -> String {
        self.taxpayer_name
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_uppercase()
    }
}

impl Form1701Draft {
    fn spouse_rdo_value(&self) -> String {
        let rdo = self.spouse_rdo_code.trim();
        if self.spouse_column_active() && !rdo.is_empty() {
            rdo.to_string()
        } else {
            "000".to_string()
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::filing_queue::QueueAuthSource;
    use crate::forms::FilingStatus;
    use crate::forms::form_1701::{Form1701EmployerRow, Form1701TaxpayerType};

    pub(crate) fn sample() -> Form1701Draft {
        let mut d = Form1701Draft {
            tin: "123-456-788-00000".into(),
            taxable_year: 2025,
            rdo_code: "039".into(),
            taxpayer_type: Some(Form1701TaxpayerType::SingleProprietor),
            taxpayer_also_compensation_earner: true,
            atc: Some(Form1701Atc::Ii013),
            taxpayer_name: "Sample Dummy Taxpayer".into(),
            registered_address: "123 Sample Street, Quezon City".into(),
            zip_code: "1100".into(),
            date_of_birth: "01/15/1980".into(),
            email: "sample.taxpayer@example.com".into(),
            citizenship: "Filipino".into(),
            claims_foreign_tax_credits: Some(false),
            contact_number: "09170000000".into(),
            civil_status: Some(Form1701CivilStatus::Single),
            has_exempt_income: Some(false),
            has_special_rate_income: Some(false),
            deduction_method: Some(Form1701DeductionMethod::Osd),
            line_of_business: "Sample Consulting".into(),
            ..Form1701Draft::default()
        };
        d.employers[0] = Form1701EmployerRow {
            owner: Some(Form1701Party::Taxpayer),
            employer_name: "Sample Employer Inc".into(),
            employer_tin: "123-456-788-00000".into(),
            compensation_income: Some(480_000.005),
            tax_withheld: Some(25_000.0),
        };
        d.set_amount(
            Form1701AmountSection::Schedule3,
            8,
            Form1701Party::Taxpayer,
            Some(1_250_000.5),
        );
        d.set_amount(
            Form1701AmountSection::Schedule3,
            9,
            Form1701Party::Taxpayer,
            Some(300_000.0),
        );
        d.set_amount(
            Form1701AmountSection::PartVii,
            1,
            Form1701Party::Taxpayer,
            Some(10_000.0),
        );
        d.set_amount(
            Form1701AmountSection::PartIi,
            27,
            Form1701Party::Taxpayer,
            Some(500.0),
        );
        d.recompute();
        d
    }

    fn messages(d: &Form1701Draft) -> Vec<String> {
        <Form1701Draft as QueueableForm>::validate(d)
            .into_iter()
            .map(|(_, m)| m)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        use Form1701AmountSection::*;
        let d = sample();
        let tp = Form1701Party::Taxpayer;
        assert_eq!(d.amount(Schedule2, 4, tp), Some(480_000.01));
        assert_eq!(d.amount(Schedule2, 6, tp), Some(480_000.0));
        assert_eq!(d.amount(Schedule3, 10, tp), Some(950_001.0));
        assert_eq!(d.amount(Schedule3, 17, tp), Some(380_000.0));
        assert_eq!(d.amount(Schedule3, 18, tp), Some(570_001.0));
        assert_eq!(d.amount(Schedule3, 24, tp), Some(1_050_001.0));
        assert_eq!(d.amount(Schedule3, 25, tp), Some(165_000.0));
        assert_eq!(d.amount(PartIi, 22, tp), Some(165_000.0));
        assert_eq!(d.amount(PartIi, 23, tp), Some(35_000.0));
        assert_eq!(d.amount(PartIi, 31, tp), Some(130_500.0));
        assert!(messages(&d).is_empty(), "{:?}", messages(&d));
    }

    #[test]
    fn tax_tables_follow_calc_tax_amt() {
        assert_eq!(tax_due(2022, 600_000.0), 80_000.0);
        assert_eq!(tax_due(2023, 600_000.0), 62_500.0);
        assert_eq!(tax_due(2025, 250_001.0), 0.0);
        assert_eq!(tax_due(2025, 250_003.0), 0.0);
        assert_eq!(tax_due(2025, 250_004.0), 1.0);
    }

    #[test]
    fn field_map_and_filename_use_official_formats() {
        let d = sample();
        let f = d.to_official_field_map();
        assert_eq!(f["frm1701:txtPg1I8TaxpayerName"], "SAMPLE DUMMY TAXPAYER");
        assert_eq!(f["frm1701:txtPg2TaxpayerName"], "SAMPLE DUMMY TAXPAYER");
        assert_eq!(f["frm1701:txtPg2IShed3_8A"], "1,250,000.50");
        assert_eq!(f["frm1701:txtPg1I4BranchCode"], "00000");
        assert_eq!(f["txtEmail"], "sample.taxpayer@example.com");
        assert_eq!(
            d.submission_filename(),
            "12345678800000-1701v2018-122025#sample.taxpayer@example.com#.xml"
        );
        let payload = d.to_official_xml_payload().unwrap();
        assert!(payload.contains("<div>frm1701:txtPg1I5RDOCode=039frm1701:txtPg1I5RDOCode=</div>"));
        assert!(payload.contains(
            "<div>frm1701:txtPg2I2SpouseRDOCode=000frm1701:txtPg2I2SpouseRDOCode=</div>"
        ));
        assert!(!payload.contains("txtRDOCode="));
    }

    #[test]
    fn period_codes_round_trip() {
        assert_eq!(sample().period_code(), "122025");
        assert_eq!(
            Form1701Draft::parse_period_code("122025"),
            Some((2025, FilingPeriod::Annual))
        );
        assert_eq!(Form1701Draft::parse_period_code("062025"), None);
        assert_eq!(Form1701Draft::parse_period_code("12202"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1701Draft), expected: &str| {
            let mut d = sample();
            mutate(&mut d);
            let found = messages(&d);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.tin = "123".into(),
            "Please enter a valid TIN number on Item 4.",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 5.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 8.",
        );
        check(
            &|d| d.taxable_year = 2099,
            "Invalid date entry on page 1 Item 1. Entry should not be later than Current Date.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Please file using the old version of the form.",
        );
        check(
            &|d| d.taxpayer_type = None,
            "Please select an option for page 1 Item 6.",
        );
        check(
            &|d| d.atc = None,
            "Please select an option for page 1 Item 7.",
        );
        check(
            &|d| d.date_of_birth.clear(),
            "Please indicate birth date on page 1 item 10.",
        );
        check(
            &|d| d.date_of_birth = "1/5/80".into(),
            "Invalid birth date on page 1 item 10.  Please check date format.",
        );
        check(
            &|d| d.citizenship.clear(),
            "Please fill up citizenship on page 1 Item 12.",
        );
        check(
            &|d| d.claims_foreign_tax_credits = Some(true),
            "Please fill up foreign tax number on page 1 Item 14.",
        );
        check(
            &|d| d.civil_status = None,
            "Please select an option for page 1 Item 16.",
        );
        check(
            &|d| {
                d.civil_status = Some(Form1701CivilStatus::Married);
                d.spouse_has_income = None;
            },
            "Please select an option for page 1 Item 17.",
        );
        check(
            &|d| d.has_exempt_income = None,
            "Please select an option for page 1 Item 19.",
        );
        check(
            &|d| d.has_special_rate_income = None,
            "Please select an option for page 1 Item 20.",
        );
        check(
            &|d| d.deduction_method = None,
            "Please select an option for page 1 Item 21A.",
        );
        check(
            &|d| {
                d.set_amount(
                    Form1701AmountSection::PartIi,
                    25,
                    Form1701Party::Taxpayer,
                    Some(100_000.0),
                );
                d.recompute();
            },
            "Amount in page 1 Item 25A cannot be more than 50% of Item 22.",
        );
        check(
            &|d| {
                d.set_amount(
                    Form1701AmountSection::PartVii,
                    1,
                    Form1701Party::Taxpayer,
                    Some(900_000.0),
                );
                d.set_amount(
                    Form1701AmountSection::PartIi,
                    27,
                    Form1701Party::Taxpayer,
                    Some(0.0),
                );
                d.recompute();
            },
            "Please select an Overpayment option on Page 1 Part II.",
        );
        check(
            &|d| {
                d.deduction_method = Some(Form1701DeductionMethod::Itemized);
                d.recompute();
            },
            "Page 4 Part IX Item 11 must be equal to Item 23 on Page 2 Part V Schedule 3.A (Taxpayer is under Graduated Rates)",
        );
        check(
            &|d| {
                d.civil_status = Some(Form1701CivilStatus::Married);
                d.spouse_has_income = Some(true);
                d.joint_filing_status = Some(Form1701JointFilingStatus::Joint);
                d.recompute();
            },
            "You have entered an invalid TIN format for Spouse.",
        );
        check(
            &|d| d.has_exempt_income = Some(true),
            "Exempt or special-rate income needs Part X and its attachments, which cannot be filed electronically here yet.",
        );
        check(
            &|d| d.atc = Some(Form1701Atc::Ii017),
            "Item 7 ATC is not available for the Item 6 taxpayer type.",
        );
        check(
            &|d| {
                d.set_amount(
                    Form1701AmountSection::Schedule3,
                    8,
                    Form1701Party::Taxpayer,
                    Some(1.0),
                )
            },
            "Totals are out of date. Recompute the return.",
        );
    }

    #[test]
    fn eight_percent_and_compensation_only_regimes() {
        use Form1701AmountSection::*;
        let tp = Form1701Party::Taxpayer;
        let mut d = sample();
        d.atc = Some(Form1701Atc::Ii016);
        d.set_amount(Schedule3, 26, tp, Some(1_000_000.0));
        d.recompute();
        assert_eq!(d.amount(Schedule3, 8, tp), Some(0.0));
        assert_eq!(d.amount(Schedule3, 31, tp), Some(80_000.0));
        assert_eq!(
            d.amount(Schedule3, 32, tp),
            Some(d.amount(Schedule2, 7, tp).unwrap() + 80_000.0)
        );
        d.atc = Some(Form1701Atc::Ii011);
        d.taxpayer_type = Some(Form1701TaxpayerType::CompensationEarner);
        d.recompute();
        assert_eq!(d.amount(PartIi, 22, tp), d.amount(Schedule2, 7, tp));
        assert_eq!(d.amount(Schedule3, 32, tp), Some(0.0));
    }

    #[test]
    fn old_stored_json_still_loads() {
        let mut json = serde_json::to_value(sample()).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("line_of_business");
        object.remove("spouse_rdo_code");
        for (key, value) in [
            ("status", serde_json::json!("Draft")),
            ("created_at", serde_json::json!("2025-04-01T00:00:00+00:00")),
            ("updated_at", serde_json::json!("2025-04-02T00:00:00+00:00")),
            ("submitted_at", serde_json::Value::Null),
            ("confirmed_at", serde_json::Value::Null),
            ("submission_filename", serde_json::Value::Null),
            ("receipt_id", serde_json::Value::Null),
            ("submission_attempts", serde_json::json!(1)),
            ("next_retry_at", serde_json::Value::Null),
            ("last_error", serde_json::json!("old")),
        ] {
            object.insert(key.to_string(), value);
        }
        let d: Form1701Draft = serde_json::from_value(json).unwrap();
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
        d.set_amount(
            Form1701AmountSection::PartIi,
            28,
            Form1701Party::Taxpayer,
            Some(9.0),
        );
        assert!(d.revalidate_queued_before_submission().is_err());
    }

    #[test]
    fn page_headers_print_the_last_name_and_the_upload_map_keeps_the_full_name() {
        let mut draft = sample();
        draft.taxpayer_name = "Dela Cruz, Juan Santos".into();
        let print = draft.to_print_field_map();
        let upload = draft.to_bir_field_map();
        for page in [2, 3, 4] {
            let key = format!("frm1701:txtPg{page}TaxpayerName");
            assert_eq!(print[&key], "DELA CRUZ", "{key} prints the last name");
            assert_eq!(
                upload[&key], draft.taxpayer_name,
                "{key} upload map unchanged"
            );
        }
        assert_eq!(draft.page_header_name(), "DELA CRUZ");
    }
}
