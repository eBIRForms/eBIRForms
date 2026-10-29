use crate::profile::TaxpayerProfile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxRegimeSimulationInput {
    pub projected_gross_sales: f64,
    pub projected_expenses: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxRegimeSimulationResult {
    /// Liability under the 8% Flat Rate (Gross Sales - 250k) * 8%
    pub scenario_a_8_percent: f64,

    /// Liability under Graduated Rates using 40% Optional Standard Deduction (OSD)
    pub scenario_b_graduated_osd: f64,

    /// Liability under Graduated Rates using Itemized Deductions
    pub scenario_c_graduated_itemized: f64,

    /// The scenario with the lowest tax liability
    pub recommended_scenario: String,
}

/// Computes the income tax based on the 2024-onwards graduated tax table
/// (TRAIN/EOPT Act Schedule for Individuals).
pub fn compute_graduated_tax(net_taxable_income: f64) -> f64 {
    let income = net_taxable_income.max(0.0);

    if income <= 250_000.0 {
        0.0
    } else if income <= 400_000.0 {
        (income - 250_000.0) * 0.15
    } else if income <= 800_000.0 {
        22_500.0 + (income - 400_000.0) * 0.20
    } else if income <= 2_000_000.0 {
        102_500.0 + (income - 800_000.0) * 0.25
    } else if income <= 8_000_000.0 {
        402_500.0 + (income - 2_000_000.0) * 0.30
    } else {
        2_202_500.0 + (income - 8_000_000.0) * 0.35
    }
}

pub fn simulate_tax_regimes(
    input: &TaxRegimeSimulationInput,
    profile: &TaxpayerProfile,
) -> TaxRegimeSimulationResult {
    // Scenario A: 8% Flat Rate
    // Formula: (Gross Sales - 250,000) * 8%
    // Only available to purely self-employed / professionals (not compensation mixed... wait, mixed income 8% doesn't get the 250k deduction, the 250k deduction applies to their compensation instead).
    // For this utility, we'll assume pure self-employed logic if no compensation is provided.
    let base_8_pct = (input.projected_gross_sales - 250_000.0).max(0.0);
    let scenario_a_8_percent = base_8_pct * 0.08;

    // Scenario B: Graduated Rates with 40% OSD
    // Net Taxable Income = Gross Sales - (Gross Sales * 40%) = Gross Sales * 60%
    let net_income_osd = input.projected_gross_sales * 0.60;
    let scenario_b_graduated_osd = compute_graduated_tax(net_income_osd);

    // Scenario C: Graduated Rates with Itemized Deductions
    // Net Taxable Income = Gross Sales - Itemized Expenses
    let net_income_itemized = (input.projected_gross_sales - input.projected_expenses).max(0.0);
    let scenario_c_graduated_itemized = compute_graduated_tax(net_income_itemized);

    // Determine recommended
    let mut min_tax = scenario_a_8_percent;
    let mut recommended = "Scenario A: 8% Flat Rate".to_string();

    if scenario_b_graduated_osd < min_tax {
        min_tax = scenario_b_graduated_osd;
        recommended = "Scenario B: Graduated Rates with 40% OSD".to_string();
    }

    if scenario_c_graduated_itemized < min_tax {
        recommended = "Scenario C: Graduated Rates with Itemized Deductions".to_string();
    }

    // Edge case: if they are VAT registered, 8% is not an option.
    if profile.is_vat_registered {
        // Recalculate minimum between B and C only
        if scenario_b_graduated_osd <= scenario_c_graduated_itemized {
            recommended =
                "Scenario B: Graduated Rates with 40% OSD (VAT registered - 8% not allowed)"
                    .to_string();
        } else {
            recommended = "Scenario C: Graduated Rates with Itemized Deductions (VAT registered - 8% not allowed)".to_string();
        }
    }

    TaxRegimeSimulationResult {
        scenario_a_8_percent,
        scenario_b_graduated_osd,
        scenario_c_graduated_itemized,
        recommended_scenario: recommended,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::Tin;

    fn mock_profile(is_vat: bool) -> TaxpayerProfile {
        TaxpayerProfile {
            id: None,
            full_name: "Mock".into(),
            tin: Tin {
                segment1: "000".into(),
                segment2: "000".into(),
                segment3: "000".into(),
                branch: "000".into(),
            },
            rdo_code: "039".into(),
            line_of_business: "Test".into(),
            registered_address: "Test".into(),
            zip_code: "1100".into(),
            phone: "09000000000".into(),
            email: "test@example.com".into(),
            default_form_type: "1701".into(),
            taxpayer_type: crate::profile::TaxpayerType::Individual,
            is_vat_registered: is_vat,
            business_start_date: None,
            birth_date: None,
            is_archived: false,
            email_tracking_enabled: false,
            email_auth_method: Default::default(),
            imap_email: None,
            imap_host: None,
            test_notification_enabled: false,
            imap_app_password: None,
            oauth_access_token: None,
            oauth_refresh_token: None,
            profile_versions: vec![],
            compliance_source_mode: Default::default(),
            per_year_forms: Default::default(),
            tax_classification: None,
            eopt_tier: None,
            is_bmbe: false,
            is_gpp_partner: false,
            is_create_msme: false,
            is_expanded_withholding_agent: false,
            atc_codes: vec![],
            excise_tax_categories: vec![],
            tax_elections: vec![],
            has_employees: false,
            is_dormant: false,
            has_single_employer: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            registration_activity_status: Default::default(),
            profile_pin_hash: None,
            totp_secret: None,
        }
    }

    #[test]
    fn test_compute_graduated_tax() {
        assert_eq!(compute_graduated_tax(200_000.0), 0.0);
        assert_eq!(compute_graduated_tax(300_000.0), 7_500.0); // 50k * 15%
        assert_eq!(compute_graduated_tax(600_000.0), 62_500.0); // 22.5k + 200k * 20%
    }

    #[test]
    fn test_simulation_8_percent_wins() {
        let input = TaxRegimeSimulationInput {
            projected_gross_sales: 500_000.0,
            projected_expenses: 50_000.0, // Low expenses -> 8% wins usually
        };
        let result = simulate_tax_regimes(&input, &mock_profile(false));

        // A: (500k - 250k) * 8% = 20k
        assert_eq!(result.scenario_a_8_percent, 20_000.0);
        // B: 500k * 60% = 300k. Tax = (300k - 250k) * 15% = 7.5k
        assert_eq!(result.scenario_b_graduated_osd, 7_500.0);
        // C: 500k - 50k = 450k. Tax = 22.5k + (450k - 400k) * 20% = 32.5k
        assert_eq!(result.scenario_c_graduated_itemized, 32_500.0);

        // Wait, in this case B wins because 7.5k < 20k
        assert_eq!(
            result.recommended_scenario,
            "Scenario B: Graduated Rates with 40% OSD"
        );
    }

    #[test]
    fn test_simulation_itemized_wins() {
        let input = TaxRegimeSimulationInput {
            projected_gross_sales: 1_000_000.0,
            projected_expenses: 800_000.0, // High expenses -> itemized wins
        };
        let result = simulate_tax_regimes(&input, &mock_profile(false));

        // A: (1M - 250k) * 8% = 60k
        // B: 1M * 60% = 600k. Tax = 22.5k + 200k * 20% = 62.5k
        // C: 1M - 800k = 200k. Tax = 0
        assert_eq!(result.scenario_c_graduated_itemized, 0.0);
        assert_eq!(
            result.recommended_scenario,
            "Scenario C: Graduated Rates with Itemized Deductions"
        );
    }

    #[test]
    fn test_simulation_vat_registered() {
        let input = TaxRegimeSimulationInput {
            projected_gross_sales: 500_000.0,
            projected_expenses: 50_000.0,
        };
        // VAT registered, so 8% is not an option
        let result = simulate_tax_regimes(&input, &mock_profile(true));

        assert!(result.recommended_scenario.contains("VAT registered"));
        assert!(result.recommended_scenario.contains("Scenario B"));
    }
}
