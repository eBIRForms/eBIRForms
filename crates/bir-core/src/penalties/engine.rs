use super::compromise::lookup_compromise;
use super::taxpayer::TaxpayerClass;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PenaltyProfile {
    StandardFiling,
    Withholding,
}

#[derive(Debug, Clone)]
pub struct PenaltyContext {
    pub form_code: String,
    pub tax_type: PenaltyProfile,
    pub taxpayer_class: TaxpayerClass,
    pub taxable_period: String,

    // Dates
    pub due_date: NaiveDate,
    pub filing_date: NaiveDate,
    pub payment_date: Option<NaiveDate>,

    // Financials
    pub basic_tax_due: f64,
    pub amount_paid_before_deadline: f64,
    pub gross_sales_or_receipts: f64,

    // Flags
    pub is_amended_return: bool,
    pub original_was_on_time: bool,
    pub is_fraud_or_willful_neglect: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PenaltyResult {
    pub surcharge: f64,
    pub interest: f64,
    pub compromise: f64,
    pub total_penalties: f64,
}

use super::config::PenaltyConfig;

pub struct PenaltyEngine;

impl PenaltyEngine {
    pub fn calculate(ctx: &PenaltyContext, config: &PenaltyConfig) -> PenaltyResult {
        let payment_or_current_date = ctx.payment_date.unwrap_or(ctx.filing_date);
        let filing_days_late = (ctx.filing_date - ctx.due_date).num_days().max(0);
        let payment_days_late = (payment_or_current_date - ctx.due_date).num_days().max(0);

        if filing_days_late == 0 && payment_days_late == 0 {
            return PenaltyResult::default();
        }

        // EOPT waives amended-return penalties only for covered micro/small
        // taxpayers whose original return and payment were timely.
        if ctx.is_amended_return
            && ctx.original_was_on_time
            && !ctx.is_fraud_or_willful_neglect
            && config.is_amended_return_penalty_waived(ctx.taxpayer_class, ctx.due_date)
        {
            return PenaltyResult::default();
        }

        // Multipliers and Rates from Config
        let surcharge_rate = config.get_surcharge_rate_for_due_date(
            ctx.taxpayer_class,
            ctx.is_fraud_or_willful_neglect,
            ctx.due_date,
        );
        let compromise_multiplier =
            config.get_compromise_multiplier_for_due_date(ctx.taxpayer_class, ctx.due_date);

        let unpaid_tax = (ctx.basic_tax_due - ctx.amount_paid_before_deadline).max(0.0);

        let surcharge = if unpaid_tax > 0.0 && (filing_days_late > 0 || payment_days_late > 0) {
            unpaid_tax * surcharge_rate
        } else {
            0.0
        };

        let interest = if unpaid_tax > 0.0 && payment_days_late > 0 {
            config.calculate_interest(
                unpaid_tax,
                ctx.taxpayer_class,
                ctx.due_date,
                payment_or_current_date,
            )
        } else {
            0.0
        };

        let base_compromise =
            lookup_compromise(unpaid_tax, ctx.gross_sales_or_receipts, ctx.tax_type);
        let compromise = base_compromise * compromise_multiplier;

        PenaltyResult {
            surcharge: (surcharge * 100.0).round() / 100.0,
            interest: (interest * 100.0).round() / 100.0,
            compromise: (compromise * 100.0).round() / 100.0,
            total_penalties: ((surcharge + interest + compromise) * 100.0).round() / 100.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn context_for(
        class: TaxpayerClass,
        due_date: NaiveDate,
        paid_at: NaiveDate,
    ) -> PenaltyContext {
        PenaltyContext {
            form_code: "2551Qv2018".to_string(),
            tax_type: PenaltyProfile::StandardFiling,
            taxpayer_class: class,
            taxable_period: "Q1 2024".to_string(),
            due_date,
            filing_date: paid_at,
            payment_date: Some(paid_at),
            basic_tax_due: 10_000.0,
            amount_paid_before_deadline: 0.0,
            gross_sales_or_receipts: 100_000.0,
            is_amended_return: false,
            original_was_on_time: false,
            is_fraud_or_willful_neglect: false,
        }
    }

    #[test]
    fn micro_small_pre_eopt_due_date_uses_standard_penalties() {
        let config = PenaltyConfig::default_rules();
        let ctx = context_for(TaxpayerClass::Micro, date(2020, 4, 25), date(2020, 5, 25));

        let penalties = PenaltyEngine::calculate(&ctx, &config);

        assert_eq!(penalties.surcharge, 2_500.0);
        assert_eq!(penalties.interest, 98.63);
        assert_eq!(penalties.compromise, 3_000.0);
        assert_eq!(penalties.total_penalties, 5_598.63);
    }

    #[test]
    fn micro_small_current_due_date_uses_reduced_penalties() {
        let config = PenaltyConfig::default_rules();
        let ctx = context_for(TaxpayerClass::Micro, date(2024, 4, 25), date(2024, 5, 25));

        let penalties = PenaltyEngine::calculate(&ctx, &config);

        assert_eq!(penalties.surcharge, 1_000.0);
        assert_eq!(penalties.interest, 49.32);
        assert_eq!(penalties.compromise, 1_500.0);
        assert_eq!(penalties.total_penalties, 2_549.32);
    }

    #[test]
    fn regular_amended_return_is_not_waived_by_micro_small_rule() {
        let config = PenaltyConfig::default_rules();
        let mut ctx = context_for(TaxpayerClass::Regular, date(2024, 4, 25), date(2024, 5, 25));
        ctx.is_amended_return = true;
        ctx.original_was_on_time = true;

        let penalties = PenaltyEngine::calculate(&ctx, &config);

        assert_eq!(penalties.surcharge, 2_500.0);
        assert_eq!(penalties.interest, 98.63);
        assert_eq!(penalties.compromise, 3_000.0);
    }

    #[test]
    fn micro_small_amended_return_is_waived_when_original_was_timely() {
        let config = PenaltyConfig::default_rules();
        let mut ctx = context_for(TaxpayerClass::Micro, date(2024, 4, 25), date(2024, 5, 25));
        ctx.is_amended_return = true;
        ctx.original_was_on_time = true;

        let penalties = PenaltyEngine::calculate(&ctx, &config);

        assert_eq!(penalties.total_penalties, 0.0);
    }

    #[test]
    fn pre_train_interest_splits_at_train_effective_date() {
        let config = PenaltyConfig::default_rules();
        let mut ctx = context_for(TaxpayerClass::Regular, date(2017, 12, 31), date(2018, 1, 2));
        ctx.basic_tax_due = 36_500.0;

        let penalties = PenaltyEngine::calculate(&ctx, &config);

        assert_eq!(penalties.interest, 32.0);
    }
}
