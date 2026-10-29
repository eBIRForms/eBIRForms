use crate::penalties::engine::PenaltyContext;

/// Heuristic anomaly detector comparing declared sales against expected sales
/// from historical filing trends or imported ERP data.
pub fn detect_under_declaration(expected_sales: f64, declared_sales: f64) -> bool {
    if expected_sales <= 0.0 {
        return false;
    }

    // Trigger a substantial under-declaration flag if declared sales are >= 30% below expected values
    let threshold = expected_sales * 0.70; // 30% below expected

    declared_sales <= threshold
}

/// Helper function to flag a PenaltyContext as fraudulent if an anomaly is detected
pub fn apply_fraud_flag_if_anomalous(
    context: &mut PenaltyContext,
    expected_sales: f64,
    declared_sales: f64,
) {
    if detect_under_declaration(expected_sales, declared_sales) {
        context.is_fraud_or_willful_neglect = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::penalties::PenaltyProfile;
    use crate::penalties::{PenaltyContext, TaxpayerClass};
    use chrono::NaiveDate;

    #[test]
    fn test_detect_under_declaration() {
        // Expected 100k, Declared 71k -> Not flagged (29% under)
        assert!(!detect_under_declaration(100_000.0, 71_000.0));

        // Expected 100k, Declared 70k -> Flagged (30% under)
        assert!(detect_under_declaration(100_000.0, 70_000.0));

        // Expected 100k, Declared 60k -> Flagged (40% under)
        assert!(detect_under_declaration(100_000.0, 60_000.0));
    }

    #[test]
    fn test_apply_fraud_flag() {
        let mut ctx = PenaltyContext {
            form_code: "2551Qv2018".to_string(),
            tax_type: PenaltyProfile::StandardFiling,
            taxpayer_class: TaxpayerClass::Micro,
            taxable_period: "Q1 2026".to_string(),
            is_amended_return: false,
            original_was_on_time: true,
            is_fraud_or_willful_neglect: false,
            basic_tax_due: 1000.0,
            amount_paid_before_deadline: 0.0,
            gross_sales_or_receipts: 100_000.0,
            due_date: NaiveDate::from_ymd_opt(2026, 4, 25).unwrap(),
            filing_date: NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            payment_date: None,
        };

        // Normal declaration
        apply_fraud_flag_if_anomalous(&mut ctx, 100_000.0, 90_000.0);
        assert!(!ctx.is_fraud_or_willful_neglect);

        // Substantial under-declaration
        apply_fraud_flag_if_anomalous(&mut ctx, 100_000.0, 60_000.0);
        assert!(ctx.is_fraud_or_willful_neglect); // Flag is flipped!
    }
}
