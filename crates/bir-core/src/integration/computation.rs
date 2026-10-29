#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorporateTaxResult {
    pub regular_corporate_income_tax: f64,
    pub minimum_corporate_income_tax: f64,
    pub final_tax_due: f64,
    pub effective_rate: f64,
    pub applied_rule: &'static str,
}

/// Computes Corporate Income Tax (CIT) based on the CREATE Act rules.
///
/// Under the CREATE Act (RA 11534):
/// - Domestic corporations with Net Taxable Income ≤ ₱5,000,000 AND
///   Total Assets ≤ ₱100,000,000 (excluding land) are taxed at 20%.
/// - All other corporations are taxed at 25%.
/// - Minimum Corporate Income Tax (MCIT) of 2% of Gross Income applies
///   beginning the 4th taxable year immediately following the year in which
///   the corporation commenced its business operations.
use chrono::NaiveDate;

pub fn compute_corporate_income_tax(
    gross_income: f64,
    net_taxable_income: f64,
    total_assets_excluding_land: f64,
    years_operating: u32,
    effective_date: NaiveDate,
) -> CorporateTaxResult {
    let mut applied_rule = "Standard 25% CIT";
    let mut rcit_rate = 0.25;

    // Pre-CREATE Act check (before July 1, 2020)
    let create_act_start = NaiveDate::from_ymd_opt(2020, 7, 1).unwrap();
    if effective_date < create_act_start {
        rcit_rate = 0.30;
        applied_rule = "Standard 30% CIT (Pre-CREATE Act)";
    } else if net_taxable_income <= 5_000_000.0 && total_assets_excluding_land <= 100_000_000.0 {
        // Check MSME qualification under CREATE Act
        rcit_rate = 0.20;
        applied_rule = "MSME 20% CIT (CREATE Act)";
    }

    let regular_corporate_income_tax = (net_taxable_income * rcit_rate).max(0.0);

    // MCIT is 2% of gross income, but historically 1% from July 1, 2020 to June 30, 2023
    let mcit_end_1_percent = NaiveDate::from_ymd_opt(2023, 6, 30).unwrap();
    let mut mcit_rate = 0.02;
    if effective_date >= create_act_start && effective_date <= mcit_end_1_percent {
        mcit_rate = 0.01;
    }

    let minimum_corporate_income_tax = if years_operating >= 4 {
        gross_income * mcit_rate
    } else {
        0.0
    };

    // The final tax is the higher of RCIT or MCIT
    let final_tax_due = regular_corporate_income_tax.max(minimum_corporate_income_tax);

    // Update rule string if MCIT is applied
    if minimum_corporate_income_tax > regular_corporate_income_tax {
        applied_rule = if mcit_rate == 0.01 {
            "MCIT 1% (CREATE Act Historical)"
        } else {
            "MCIT 2% (CREATE Act)"
        };
    }

    CorporateTaxResult {
        regular_corporate_income_tax,
        minimum_corporate_income_tax,
        final_tax_due,
        effective_rate: if net_taxable_income > 0.0 {
            final_tax_due / net_taxable_income
        } else {
            0.0
        },
        applied_rule,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_msme_qualification() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        // 4M net income, 50M assets -> 20%
        let res = compute_corporate_income_tax(10_000_000.0, 4_000_000.0, 50_000_000.0, 2, date);
        assert_eq!(res.regular_corporate_income_tax, 800_000.0); // 4M * 20%
        assert_eq!(res.minimum_corporate_income_tax, 0.0); // < 4 years
        assert_eq!(res.final_tax_due, 800_000.0);
        assert_eq!(res.applied_rule, "MSME 20% CIT (CREATE Act)");
    }

    #[test]
    fn test_standard_cit() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        // 6M net income, 50M assets -> 25% (income exceeds 5M)
        let res = compute_corporate_income_tax(15_000_000.0, 6_000_000.0, 50_000_000.0, 2, date);
        assert_eq!(res.regular_corporate_income_tax, 1_500_000.0); // 6M * 25%
        assert_eq!(res.final_tax_due, 1_500_000.0);
        assert_eq!(res.applied_rule, "Standard 25% CIT");

        // 4M net income, 150M assets -> 25% (assets exceed 100M)
        let res2 = compute_corporate_income_tax(15_000_000.0, 4_000_000.0, 150_000_000.0, 2, date);
        assert_eq!(res2.regular_corporate_income_tax, 1_000_000.0); // 4M * 25%
        assert_eq!(res2.applied_rule, "Standard 25% CIT");
    }

    #[test]
    fn test_mcit() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        // High gross, very low net -> MCIT should trigger
        // Gross: 10M, Net: 100k, Assets: 50M, Years: 5
        let res = compute_corporate_income_tax(10_000_000.0, 100_000.0, 50_000_000.0, 5, date);

        let expected_rcit = 100_000.0 * 0.20; // 20k (qualifies for 20%)
        let expected_mcit = 10_000_000.0 * 0.02; // 200k

        assert_eq!(res.regular_corporate_income_tax, expected_rcit);
        assert_eq!(res.minimum_corporate_income_tax, expected_mcit);
        assert_eq!(res.final_tax_due, expected_mcit); // MCIT is higher
        assert_eq!(res.applied_rule, "MCIT 2% (CREATE Act)");
    }

    #[test]
    fn test_historical_rule_versioning() {
        // Test Pre-CREATE Act (2019) -> 30% RCIT
        let date_2019 = NaiveDate::from_ymd_opt(2019, 1, 1).unwrap();
        let res_2019 =
            compute_corporate_income_tax(10_000_000.0, 4_000_000.0, 50_000_000.0, 2, date_2019);
        assert_eq!(res_2019.applied_rule, "Standard 30% CIT (Pre-CREATE Act)");
        assert_eq!(res_2019.regular_corporate_income_tax, 1_200_000.0); // 4M * 30%

        // Test 1% MCIT Period (Aug 2021)
        let date_2021 = NaiveDate::from_ymd_opt(2021, 8, 1).unwrap();
        let res_2021 =
            compute_corporate_income_tax(10_000_000.0, 100_000.0, 50_000_000.0, 5, date_2021);
        assert_eq!(res_2021.applied_rule, "MCIT 1% (CREATE Act Historical)");
        assert_eq!(res_2021.minimum_corporate_income_tax, 100_000.0); // 10M * 1%
    }
}
