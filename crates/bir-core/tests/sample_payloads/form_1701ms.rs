use crate::sample_payloads::penalty_items::{self, Deadline, day};
// The samples enter their penalties by hand (`keep_entered_penalties`).
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1701ms::{
    Form1701MsCivilStatus, Form1701MsColumn, Form1701MsDeduction, Form1701MsDraft,
    Form1701MsFiling, Form1701MsSource, Form1701MsSpouseInfo, Form1701MsTaxOption,
};
use bir_core::forms::queueable::QueueableForm as _;

/// Married, filing jointly. Taxpayer: mixed income, graduated rates, OSD.
/// Spouse: compensation income only.
fn sample_1701ms() -> Form1701MsDraft {
    let mut draft = Form1701MsDraft::new_from_profile(&dummy_profile("1701MS", "Individual"), 2025);
    draft.civil_status = Form1701MsCivilStatus::Married;
    draft.filing = Form1701MsFiling::Jointly;
    draft.spouse_info = Form1701MsSpouseInfo {
        tin: "12345678800000".to_string(),
        rdo_code: "039".to_string(),
        name: "Sample Dummy Spouse".to_string(),
        email: "sample.spouse@example.com".to_string(),
        contact_number: "09170000001".to_string(),
    };
    draft.taxpayer = Form1701MsColumn {
        source: Form1701MsSource::Mixed,
        tax_option: Form1701MsTaxOption::Graduated,
        deduction: Form1701MsDeduction::Osd,
        gross_compensation: 900_000.5,
        non_taxable_compensation: 90_000.0,
        sales: 1_500_000.0,
        sales_returns: 10_000.0,
        quarterly_payments: 50_000.0,
        cwt_2316: 80_000.0,
        ..Form1701MsColumn::default()
    };
    draft.spouse = Form1701MsColumn {
        source: Form1701MsSource::Compensation,
        gross_compensation: 600_000.0,
        non_taxable_compensation: 90_000.0,
        cwt_2316: 44_500.0,
        ..Form1701MsColumn::default()
    };
    draft.perjury_agreed = true;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1701ms_sample_payload_is_current() {
    let draft = sample_1701ms();
    assert_eq!(draft.aggregate_amount_payable, 198_500.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1701MS must validate: {errors:?}"));
    check_sample(
        "1701MS-122025",
        "1701ms-v2024",
        &payload,
        "d989cb983e5ce202703632e4061a20ff11c0a2e0903a83dd5fbf829bcdb295b7",
    );
}

/// Single; business income at a special rate with Items 13–16, cost of
/// sales, a foreign tax credit (its blur upper-cases every text control,
/// emails included) and an overpayment to be refunded.
fn sample_1701ms_special() -> Form1701MsDraft {
    let mut draft = Form1701MsDraft::new_from_profile(&dummy_profile("1701MS", "Individual"), 2025);
    draft.taxpayer_name = "Dummy, Sample Taxpayer".to_string();
    draft.civil_status = Form1701MsCivilStatus::Single;
    draft.taxpayer = Form1701MsColumn {
        source: Form1701MsSource::Business,
        tax_option: Form1701MsTaxOption::Special,
        legal_basis: "Sample legal basis".to_string(),
        promotion_agency: "Sample agency".to_string(),
        registered_activity: "Sample activity 001".to_string(),
        effectivity_from: "01/01/2020".to_string(),
        effectivity_to: "12/31/2030".to_string(),
        sales: 2_000_000.0,
        cost_of_sales: 500_000.0,
        special_rate: 5.0,
        quarterly_payments: 100_000.0,
        foreign_tax_credits: 1_000.0,
        ..Form1701MsColumn::default()
    };
    draft.foreign_tax_credits_description = "Sample foreign credit".to_string();
    draft.to_be_refunded = true;
    draft.refund_amount = 26_000.0;
    draft.perjury_agreed = true;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1701ms_special_rate_sample_payload_is_current() {
    let draft = sample_1701ms_special();
    assert_eq!(draft.aggregate_amount_payable, -26_000.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1701MS must validate: {errors:?}"));
    check_sample(
        "1701MS-122025-special",
        "1701ms-v2024",
        &payload,
        "2a6afbae648ac467879b2165988d642b371e5560ae93e8e9b4188c6386604e05",
    );
}

/// Joint calendar year 2025: due 15 April 2026.
#[test]
fn form_1701ms_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1701ms, Deadline::Calendar(day(2026, 4, 15)));
}

/// Married, filing jointly, with the spouse's column on the entries the
/// samples above leave blank: business income at a special rate (Items
/// 13B-16B, cost of sales, itemized, special and NOLCO deductions, the share
/// of another government agency). For the agent-form probe only (PR 7):
/// without it the probe saw those spouse boxes blank on every sample and
/// could not bind them.
fn sample_1701ms_joint_special() -> Form1701MsDraft {
    let mut draft = sample_1701ms();
    draft.spouse = Form1701MsColumn {
        source: Form1701MsSource::Business,
        tax_option: Form1701MsTaxOption::Special,
        legal_basis: "Spouse legal basis".to_string(),
        promotion_agency: "Spouse agency".to_string(),
        registered_activity: "Spouse activity 002".to_string(),
        effectivity_from: "02/01/2021".to_string(),
        effectivity_to: "01/31/2031".to_string(),
        sales: 900_000.0,
        cost_of_sales: 200_000.0,
        itemized_deductions: 40_000.0,
        special_allowable_deductions: 10_000.0,
        nolco: 5_000.0,
        special_rate: 3.0,
        share_of_other_agencies: 1_000.0,
        quarterly_payments: 2_000.0,
        ..Form1701MsColumn::default()
    };
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

/// Married, filing jointly, the spouse on the 8% rate (Items 19B and 20B).
/// For the agent-form probe only (PR 7), as above.
fn sample_1701ms_joint_eight() -> Form1701MsDraft {
    let mut draft = sample_1701ms();
    draft.spouse = Form1701MsColumn {
        source: Form1701MsSource::Profession,
        tax_option: Form1701MsTaxOption::EightPercent,
        eight_sales: 1_200_000.0,
        eight_other_income: 30_000.0,
        ..Form1701MsColumn::default()
    };
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1701ms()).expect("sample serializes"),
        serde_json::to_value(sample_1701ms_special()).expect("sample serializes"),
        serde_json::to_value(sample_1701ms_joint_special()).expect("sample serializes"),
        serde_json::to_value(sample_1701ms_joint_eight()).expect("sample serializes"),
    ]
}
