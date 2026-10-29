//! Field-kind registry: for each classified form, which field ids are **Auto**
//! (period, identity, computed) and which are **Templatable** (spec "Field
//! kinds"; D8, D18).
//!
//! Field ids are the draft's serde field names, so a template value lands on
//! the same JSON key the draft stores. Repeating schedule rows use a `[]`
//! wildcard in the registry (`schedule_1[].atc`) and a concrete index in a
//! template (`schedule_1[0].atc`).
//!
//! A field id a form does not register is not a form field (status, audit,
//! submission bookkeeping, internal penalty state) and can never be kept or
//! marked. Only 2551Q and 1601C are classified so far; the other forms are
//! classified in Phase 4.

use crate::forms::FilingPeriod;
use crate::forms::registry::{FilingFrequency, find_form};

use super::TaxProfileError;

/// Where an Auto period field takes its value from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodSource {
    TaxableYear,
    Quarter,
    Month,
}

/// Where an Auto identity field takes its value from, after D2 inheritance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    /// Full TIN: root plus the branch's own code.
    Tin,
    TaxpayerName,
    TaxpayerType,
    RdoCode,
    RegisteredAddress,
    ZipCode,
    Phone,
    Email,
}

/// Why an Auto field is never typed, kept or marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoSource {
    /// From the Tax Form Library selection.
    Period(PeriodSource),
    /// From the taxpayer and the branch.
    Identity(IdentitySource),
    /// Computed by the form; the string names the official formula.
    Computed(&'static str),
}

/// What a template may store for a templatable field (D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplatableKind {
    /// A setup answer: may be kept (value stored) or marked.
    Setup,
    /// An amount: may be marked, never kept.
    Amount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Auto(AutoSource),
    Templatable(TemplatableKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    /// Registry id, with `[]` standing for any schedule row index.
    pub id: &'static str,
    pub kind: FieldKind,
}

/// The classified fields of one form, in on-form order (guided-mode order).
#[derive(Debug)]
pub struct FormFieldKinds {
    pub form_code: &'static str,
    pub fields: &'static [FieldSpec],
}

impl FormFieldKinds {
    /// Kind of a concrete field id (`schedule_1[2].atc` resolves through
    /// `schedule_1[].atc`). `None` when the id is not a field of this form.
    pub fn kind_of(&self, field_id: &str) -> Option<FieldKind> {
        self.position_of(field_id)
            .map(|index| self.fields[index].kind)
    }

    /// Position in on-form order, used to order guided-mode stops.
    pub fn position_of(&self, field_id: &str) -> Option<usize> {
        let pattern = row_pattern(field_id)?;
        self.fields.iter().position(|spec| spec.id == pattern)
    }

    pub fn auto_fields(&self) -> impl Iterator<Item = (&'static str, AutoSource)> + '_ {
        self.fields.iter().filter_map(|spec| match spec.kind {
            FieldKind::Auto(source) => Some((spec.id, source)),
            FieldKind::Templatable(_) => None,
        })
    }

    pub fn templatable_fields(&self) -> impl Iterator<Item = (&'static str, TemplatableKind)> + '_ {
        self.fields.iter().filter_map(|spec| match spec.kind {
            FieldKind::Templatable(kind) => Some((spec.id, kind)),
            FieldKind::Auto(_) => None,
        })
    }
}

/// `schedule_1[3].atc` -> `schedule_1[].atc`. Rejects malformed indices so a
/// typo cannot alias a registered id.
fn row_pattern(field_id: &str) -> Option<String> {
    let mut pattern = String::with_capacity(field_id.len());
    let mut rest = field_id;
    while let Some(open) = rest.find('[') {
        pattern.push_str(&rest[..=open]);
        let close = rest[open..].find(']')? + open;
        let index = &rest[open + 1..close];
        if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        pattern.push(']');
        rest = &rest[close + 1..];
    }
    pattern.push_str(rest);
    Some(pattern)
}

/// The registry entry for `form_code`, or `None` while the form is not yet
/// classified (templates and prefill are then unavailable for it).
pub fn field_kinds_for(form_code: &str) -> Option<&'static FormFieldKinds> {
    let code = crate::forms::registry::canonical_form_code(form_code);
    [&FORM_2551Q, &FORM_1601C]
        .into_iter()
        .find(|form| form.form_code == code)
}

/// A Tax Form Library selection resolved to the exact filing period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPeriod {
    pub form_code: String,
    pub frequency: FilingFrequency,
    pub taxable_year: u16,
    pub period: FilingPeriod,
}

impl ResolvedPeriod {
    /// Resolve the library's `DashboardEvent::FileForm { form_code, year,
    /// quarter }`.
    ///
    /// The library already routes by **taxable** year
    /// (`calendar_rules::Deadline::route_year_quarter`): the 1701 due in April
    /// 2026 arrives as year 2025, so no filing-year arithmetic happens here.
    /// `slot` is the month (monthly), the quarter (quarterly), ignored
    /// (annual), or the open-ended sequence number, exactly as the library
    /// emits it. Monthly and quarterly slots out of range are rejected rather
    /// than clamped.
    pub fn from_library_selection(
        form_code: &str,
        taxable_year: u16,
        slot: u8,
    ) -> Result<Self, TaxProfileError> {
        let form_code = crate::forms::registry::canonical_form_code(form_code);
        let frequency = find_form(&form_code)
            .map(|form| form.frequency.clone())
            .ok_or_else(|| TaxProfileError::Invalid(format!("Unknown form {form_code}")))?;
        let valid = match frequency {
            FilingFrequency::Monthly => (1..=12).contains(&slot),
            FilingFrequency::Quarterly => (1..=4).contains(&slot),
            FilingFrequency::Annual | FilingFrequency::OpenEnded => true,
        };
        if !valid {
            return Err(TaxProfileError::Invalid(format!(
                "Period {slot} is not valid for {form_code}"
            )));
        }
        let period = crate::db::period_from_library_slot(&form_code, slot);
        Ok(Self {
            form_code,
            frequency,
            taxable_year,
            period,
        })
    }

    /// The value an Auto period field takes, typed as the draft stores it.
    pub fn value_of(&self, source: PeriodSource) -> Option<serde_json::Value> {
        match (source, &self.period) {
            (PeriodSource::TaxableYear, _) => Some(self.taxable_year.into()),
            (PeriodSource::Quarter, FilingPeriod::Quarterly(quarter)) => Some((*quarter).into()),
            (PeriodSource::Month, FilingPeriod::Monthly(month)) => Some((*month).into()),
            _ => None,
        }
    }
}

const fn period(id: &'static str, source: PeriodSource) -> FieldSpec {
    FieldSpec {
        id,
        kind: FieldKind::Auto(AutoSource::Period(source)),
    }
}

const fn identity(id: &'static str, source: IdentitySource) -> FieldSpec {
    FieldSpec {
        id,
        kind: FieldKind::Auto(AutoSource::Identity(source)),
    }
}

const fn computed(id: &'static str, formula: &'static str) -> FieldSpec {
    FieldSpec {
        id,
        kind: FieldKind::Auto(AutoSource::Computed(formula)),
    }
}

const fn setup(id: &'static str) -> FieldSpec {
    FieldSpec {
        id,
        kind: FieldKind::Templatable(TemplatableKind::Setup),
    }
}

const fn amount(id: &'static str) -> FieldSpec {
    FieldSpec {
        id,
        kind: FieldKind::Templatable(TemplatableKind::Amount),
    }
}

/// Form 2551Q (January 2018 ENCS), fields of `forms::form_2551q::Form2551QDraft`.
pub static FORM_2551Q: FormFieldKinds = FormFieldKinds {
    form_code: "2551Q",
    fields: &[
        period("taxable_year", PeriodSource::TaxableYear),
        period("quarter", PeriodSource::Quarter),
        setup("tax_period_basis"),
        setup("year_end_month"),
        setup("is_amended"),
        setup("original_return_filed_and_paid_on_time"),
        setup("number_of_attached_sheets"),
        identity("tin", IdentitySource::Tin),
        identity("rdo_code", IdentitySource::RdoCode),
        identity("taxpayer_name", IdentitySource::TaxpayerName),
        identity("registered_address", IdentitySource::RegisteredAddress),
        identity("zip_code", IdentitySource::ZipCode),
        identity("contact_number", IdentitySource::Phone),
        identity("email", IdentitySource::Email),
        identity("taxpayer_type", IdentitySource::TaxpayerType),
        setup("business_start_date"),
        setup("eopt_tier"),
        setup("tax_relief"),
        setup("tax_relief_specification"),
        setup("item_13_election"),
        computed(
            "annual_income_tax_election",
            "snapshot of the taxable year's income-tax election",
        ),
        setup("schedule_1[].atc"),
        computed("schedule_1[].atc_description", "ATC table description"),
        amount("schedule_1[].taxable_amount"),
        computed("schedule_1[].tax_rate", "ATC table rate"),
        computed("schedule_1[].tax_due", "taxable amount x tax rate"),
        computed("total_tax_due", "Item 14 = sum of Schedule 1 tax due"),
        amount("creditable_tax_withheld"),
        amount("tax_paid_previous"),
        amount("other_tax_credit"),
        setup("other_tax_credit_description"),
        computed("total_tax_credits", "Item 18 = Items 15 + 16 + 17"),
        computed("tax_payable", "Item 19 = Item 14 less Item 18"),
        setup("auto_compute_penalties"),
        amount("expected_sales_for_penalties"),
        computed("surcharge", "Item 20, penalty engine"),
        computed("interest", "Item 21, penalty engine"),
        computed("compromise", "Item 22, penalty engine"),
        computed("total_penalties", "Item 23 = Items 20 + 21 + 22"),
        computed("total_amount_payable", "Item 24 = Item 19 + Item 23"),
        setup("overpayment_disposition"),
    ],
};

/// Form 1601C, fields of `forms::form_1601c::Form1601CDraft`.
///
/// `registered_address_2` (a legacy second address line outside the official
/// field set) and the flattened penalty-engine state are not form fields.
pub static FORM_1601C: FormFieldKinds = FormFieldKinds {
    form_code: "1601C",
    fields: &[
        period("taxable_year", PeriodSource::TaxableYear),
        period("month", PeriodSource::Month),
        setup("is_amended"),
        setup("any_taxes_withheld"),
        setup("any_taxes_withheld_unanswered"),
        setup("number_of_sheets"),
        setup("atc"),
        identity("tin", IdentitySource::Tin),
        identity("rdo_code", IdentitySource::RdoCode),
        setup("line_of_business"),
        identity("taxpayer_name", IdentitySource::TaxpayerName),
        identity("contact_number", IdentitySource::Phone),
        identity("registered_address", IdentitySource::RegisteredAddress),
        identity("zip_code", IdentitySource::ZipCode),
        setup("category_of_agent"),
        identity("email_address", IdentitySource::Email),
        setup("tax_relief"),
        setup("tax_relief_specification"),
        amount("tax_14_total_compensation"),
        amount("tax_15_statutory_minimum_wage"),
        amount("tax_16_holiday_pay"),
        amount("tax_17_13th_month_pay"),
        amount("tax_18_de_minimis"),
        amount("tax_19_sss_gsis"),
        setup("tax_20_other_name"),
        amount("tax_20_other_amount"),
        computed("tax_21_total_non_taxable", "Item 21 = Items 15 to 20"),
        computed("tax_22_total_taxable", "Item 22 = Item 14 less Item 21"),
        amount("tax_23_not_subject"),
        computed("tax_24_net_taxable", "Item 24 = Item 22 less Item 23"),
        amount("tax_25_total_taxes_withheld"),
        computed("tax_26_adjustment", "Item 26 = Schedule I total adjustment"),
        setup("schedule_1[].previous_month"),
        setup("schedule_1[].date_paid"),
        setup("schedule_1[].drawee_bank_code_or_agency"),
        setup("schedule_1[].payment_number"),
        amount("schedule_1[].tax_paid"),
        amount("schedule_1[].should_be_tax_due"),
        computed(
            "schedule_1[].adjustment",
            "Schedule I Item 7 = Item 6 less Item 5",
        ),
        computed(
            "tax_27_taxes_withheld_for_remittance",
            "Item 27 = Item 25 + Item 26",
        ),
        amount("tax_28_tax_remitted_previously"),
        setup("tax_29_other_remittances_name"),
        amount("tax_29_other_remittances_amount"),
        computed(
            "tax_30_total_tax_remittances",
            "Item 30 = Item 28 + Item 29",
        ),
        computed("tax_31_tax_still_due", "Item 31 = Item 27 less Item 30"),
        computed("tax_32_surcharge", "Item 32, penalty engine or override"),
        computed("tax_33_interest", "Item 33, penalty engine or override"),
        computed("tax_34_compromise", "Item 34, penalty engine or override"),
        computed("tax_35_total_penalties", "Item 35 = Items 32 + 33 + 34"),
        computed("tax_36_total_amount_payable", "Item 36 = Item 31 + Item 35"),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every registered top-level id must be a key the draft actually
    /// serializes, or a kept value would land nowhere. Rows are checked
    /// against a serialized row.
    fn assert_ids_exist_in(kinds: &FormFieldKinds, draft: serde_json::Value) {
        let top: BTreeSet<_> = draft.as_object().unwrap().keys().cloned().collect();
        let row: BTreeSet<_> = draft["schedule_1"][0]
            .as_object()
            .map(|row| row.keys().cloned().collect())
            .unwrap_or_default();
        for spec in kinds.fields {
            match spec.id.split_once("[].") {
                Some((list, field)) => {
                    assert!(top.contains(list), "{}: {list}", kinds.form_code);
                    assert!(row.contains(field), "{}: {}", kinds.form_code, spec.id);
                }
                None => assert!(top.contains(spec.id), "{}: {}", kinds.form_code, spec.id),
            }
        }
    }

    #[test]
    fn registered_ids_are_real_draft_fields() {
        let profile: crate::profile::TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Juan", "tin": {"segment1": "123", "segment2": "456",
            "segment3": "789", "branch": "00000"}, "rdo_code": "039", "line_of_business": "",
            "registered_address": "", "zip_code": "", "phone": "", "email": "",
            "default_form_type": "2551Q"
        }))
        .unwrap();
        let draft_2551q = crate::forms::Form2551QDraft::new_from_profile(&profile, 2026, 1);
        assert_ids_exist_in(&FORM_2551Q, serde_json::to_value(draft_2551q).unwrap());

        let mut draft_1601c =
            crate::forms::form_1601c::Form1601CDraft::new_from_profile(&profile, 2026, 2);
        draft_1601c
            .schedule_1
            .push(crate::forms::form_1601c::Form1601CSchedule1Row::default());
        draft_1601c.any_taxes_withheld_unanswered = true;
        assert_ids_exist_in(&FORM_1601C, serde_json::to_value(draft_1601c).unwrap());
    }

    #[test]
    fn concrete_row_ids_resolve_through_the_wildcard_and_typos_do_not() {
        assert_eq!(
            FORM_2551Q.kind_of("schedule_1[4].taxable_amount"),
            Some(FieldKind::Templatable(TemplatableKind::Amount))
        );
        for bogus in [
            "schedule_1[].atc",
            "schedule_1[x].atc",
            "schedule_1[0.atc",
            "status",
        ] {
            assert_eq!(FORM_2551Q.kind_of(bogus), None, "{bogus}");
        }
    }

    #[test]
    fn library_selection_resolves_each_frequency() {
        let feb = ResolvedPeriod::from_library_selection("1601C", 2026, 2).unwrap();
        assert_eq!(feb.period, FilingPeriod::Monthly(2));
        assert_eq!(feb.value_of(PeriodSource::Month), Some(2.into()));
        assert_eq!(feb.value_of(PeriodSource::TaxableYear), Some(2026.into()));

        let q3 = ResolvedPeriod::from_library_selection("2551Q", 2026, 3).unwrap();
        assert_eq!(q3.period, FilingPeriod::Quarterly(3));

        // The library routes the 1701 due in April 2026 as taxable year 2025.
        let annual = ResolvedPeriod::from_library_selection("1701", 2025, 0).unwrap();
        assert_eq!(annual.period, FilingPeriod::Annual);
        assert_eq!(annual.taxable_year, 2025);

        for (form, slot) in [("1601C", 0), ("1601C", 13), ("2551Q", 5)] {
            assert!(
                ResolvedPeriod::from_library_selection(form, 2026, slot).is_err(),
                "{form} {slot}"
            );
        }
    }

    #[test]
    fn annual_return_filed_in_2026_routes_to_taxable_year_2025() {
        use crate::calendar_rules::{DeadlinePeriod, DeadlineResolver};
        let deadline = DeadlineResolver::resolve_deadline_calendar_year(2026)
            .into_iter()
            .find(|deadline| {
                deadline.form_code == "1701"
                    && matches!(deadline.period, DeadlinePeriod::Annual { .. })
            })
            .expect("a 1701 annual deadline falls in calendar 2026");
        let (year, slot) = deadline.route_year_quarter().unwrap();
        let resolved = ResolvedPeriod::from_library_selection("1701", year, slot).unwrap();
        assert_eq!(resolved.taxable_year, 2025);
        assert_eq!(resolved.period, FilingPeriod::Annual);
    }
}
