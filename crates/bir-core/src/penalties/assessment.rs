//! Shared penalty model for returns that carry Surcharge, Interest and
//! Compromise items.
//!
//! A return shows the [`ComputedPenalties`] the engine derived, together with
//! the [`PenaltyInputs`] it used, and files the *used* value of each
//! component: the user's [`PenaltyOverrides`] entry when one is set, the
//! computed value otherwise. Overrides are per component, so lowering the
//! compromise after an RDO approval leaves the computed surcharge and interest
//! in place, and an optional note records the reason.

use super::{PenaltyConfig, PenaltyContext, PenaltyEngine, PenaltyProfile, TaxpayerClass};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// One of the three penalty items on a return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenaltyComponent {
    Surcharge,
    Interest,
    Compromise,
}

impl PenaltyComponent {
    pub const ALL: [Self; 3] = [Self::Surcharge, Self::Interest, Self::Compromise];

    pub fn label(self) -> &'static str {
        match self {
            Self::Surcharge => "Surcharge",
            Self::Interest => "Interest",
            Self::Compromise => "Compromise",
        }
    }
}

/// The inputs the engine used, kept with the computed values so the form can
/// show what the penalties were derived from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PenaltyInputs {
    pub due_date: NaiveDate,
    pub filing_date: NaiveDate,
    pub payment_date: NaiveDate,
    /// Days between the due date and the filing date (0 when on time).
    pub days_late: i64,
    /// Tax still due before penalties (never negative).
    pub basic_tax_due: f64,
    pub taxpayer_class: TaxpayerClass,
    /// `false` when the profile carries no EOPT classification and the
    /// standard (non-micro/small) class was assumed.
    pub class_from_profile: bool,
    pub schedule: PenaltyProfile,
}

/// What the engine computed for a return, with its inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputedPenalties {
    pub surcharge: f64,
    pub interest: f64,
    pub compromise: f64,
    pub inputs: PenaltyInputs,
}

impl ComputedPenalties {
    pub fn get(&self, component: PenaltyComponent) -> f64 {
        match component {
            PenaltyComponent::Surcharge => self.surcharge,
            PenaltyComponent::Interest => self.interest,
            PenaltyComponent::Compromise => self.compromise,
        }
    }

    pub fn total(&self) -> f64 {
        round_currency(self.surcharge + self.interest + self.compromise)
    }

    pub fn filed_on_time(&self) -> bool {
        self.inputs.days_late == 0
    }
}

/// The user's per-component overrides and the reason recorded with them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PenaltyOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surcharge: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interest: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compromise: Option<f64>,
    /// Optional note or reference (an RDO approval or abatement letter).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl PenaltyOverrides {
    pub fn get(&self, component: PenaltyComponent) -> Option<f64> {
        match component {
            PenaltyComponent::Surcharge => self.surcharge,
            PenaltyComponent::Interest => self.interest,
            PenaltyComponent::Compromise => self.compromise,
        }
    }

    /// Set (`Some`) or clear (`None`) one component's override.
    pub fn set(&mut self, component: PenaltyComponent, value: Option<f64>) {
        let slot = match component {
            PenaltyComponent::Surcharge => &mut self.surcharge,
            PenaltyComponent::Interest => &mut self.interest,
            PenaltyComponent::Compromise => &mut self.compromise,
        };
        *slot = value;
    }

    pub fn any(&self) -> bool {
        PenaltyComponent::ALL
            .iter()
            .any(|component| self.get(*component).is_some())
    }

    /// The value a return files for `component`: the override when set,
    /// otherwise the computed value (zero when nothing could be computed).
    pub fn used(&self, component: PenaltyComponent, computed: Option<&ComputedPenalties>) -> f64 {
        self.get(component)
            .or_else(|| computed.map(|computed| computed.get(component)))
            .unwrap_or(0.0)
    }

    pub fn used_total(&self, computed: Option<&ComputedPenalties>) -> f64 {
        round_currency(
            PenaltyComponent::ALL
                .iter()
                .map(|component| self.used(*component, computed))
                .sum(),
        )
    }
}

/// What a form supplies to compute its penalties.
#[derive(Debug, Clone)]
pub struct PenaltyRequest<'a> {
    pub form_code: &'a str,
    pub taxable_period: String,
    pub schedule: PenaltyProfile,
    /// The profile's EOPT classification; `None` assumes the standard class.
    pub taxpayer_class: Option<TaxpayerClass>,
    pub due_date: NaiveDate,
    pub filing_date: NaiveDate,
    /// `None` when unknown: the engine treats payment as made on filing.
    pub payment_date: Option<NaiveDate>,
    pub basic_tax_due: f64,
    /// Gross sales/receipts for the engine's no-tax-due compromise tier.
    /// `None` for returns that carry no such basis: with no tax due, every
    /// component is then zero rather than derived from an invented basis.
    pub gross_sales_or_receipts: Option<f64>,
    pub is_amended_return: bool,
}

/// Run [`PenaltyEngine`] with the default [`PenaltyConfig`] and keep its inputs.
pub fn compute_penalties(request: &PenaltyRequest<'_>) -> ComputedPenalties {
    let basic_tax_due = round_currency(request.basic_tax_due.max(0.0));
    let payment_date = request.payment_date.unwrap_or(request.filing_date);
    let taxpayer_class = request.taxpayer_class.unwrap_or_default();
    let inputs = PenaltyInputs {
        due_date: request.due_date,
        filing_date: request.filing_date,
        payment_date,
        days_late: (request.filing_date - request.due_date).num_days().max(0),
        basic_tax_due,
        taxpayer_class,
        class_from_profile: request.taxpayer_class.is_some(),
        schedule: request.schedule,
    };

    if basic_tax_due <= 0.0 && request.gross_sales_or_receipts.is_none() {
        return ComputedPenalties {
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            inputs,
        };
    }

    let context = PenaltyContext {
        form_code: request.form_code.to_string(),
        tax_type: request.schedule,
        taxpayer_class,
        taxable_period: request.taxable_period.clone(),
        due_date: request.due_date,
        filing_date: request.filing_date,
        payment_date: Some(payment_date),
        basic_tax_due,
        amount_paid_before_deadline: 0.0,
        gross_sales_or_receipts: request.gross_sales_or_receipts.unwrap_or(0.0),
        is_amended_return: request.is_amended_return,
        // Whether the original return was timely is not recorded, so the
        // EOPT amended-return waiver is never assumed.
        original_was_on_time: false,
        is_fraud_or_willful_neglect: false,
    };
    let result = PenaltyEngine::calculate(&context, &PenaltyConfig::default_rules());
    ComputedPenalties {
        surcharge: result.surcharge,
        interest: result.interest,
        compromise: result.compromise,
        inputs,
    }
}

fn round_currency(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computed(surcharge: f64, interest: f64, compromise: f64) -> ComputedPenalties {
        let date = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        ComputedPenalties {
            surcharge,
            interest,
            compromise,
            inputs: PenaltyInputs {
                due_date: date,
                filing_date: date,
                payment_date: date,
                days_late: 0,
                basic_tax_due: 0.0,
                taxpayer_class: TaxpayerClass::Regular,
                class_from_profile: false,
                schedule: PenaltyProfile::Withholding,
            },
        }
    }

    #[test]
    fn an_override_replaces_only_its_own_component() {
        let computed = computed(5_000.0, 216.99, 5_000.0);
        let mut overrides = PenaltyOverrides::default();
        overrides.set(PenaltyComponent::Compromise, Some(500.0));

        assert_eq!(
            overrides.used(PenaltyComponent::Surcharge, Some(&computed)),
            5_000.0
        );
        assert_eq!(
            overrides.used(PenaltyComponent::Interest, Some(&computed)),
            216.99
        );
        assert_eq!(
            overrides.used(PenaltyComponent::Compromise, Some(&computed)),
            500.0
        );
        assert_eq!(overrides.used_total(Some(&computed)), 5_716.99);

        overrides.set(PenaltyComponent::Compromise, None);
        assert!(!overrides.any());
        assert_eq!(overrides.used_total(Some(&computed)), 10_216.99);
    }
}
