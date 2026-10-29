//! The official 1702Q page formats Schedule 1 with
//! `toLocaleString('en-US', {style: 'currency', ...})`. eBIRForms runs on the
//! legacy JScript engine, which ignores those arguments (`1,234.00`); node
//! returns `$1,234`, which the page itself cannot parse back. The steps file
//! therefore starts by giving the page the legacy behaviour; every other step
//! is raw filer input (profile fields are filled the way `loadBGData` does,
//! both `txtTelNum` controls the way a saved return reloads).

use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1702q::{
    Form1702qDeduction, Form1702qDraft, Form1702qOtherCredit, Form1702qSchedule1Column,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1702q() -> Form1702qDraft {
    let mut draft =
        Form1702qDraft::new_from_profile(&dummy_profile("1702Q", "Corporation"), 2025, 2);
    draft.taxpayer_name = "Sample Dummy Corporation".to_string();
    draft.atc_mcit = true;
    draft.atc = "IC010_25%".to_string();
    draft.deduction = Form1702qDeduction::Itemized;
    draft.special = Form1702qSchedule1Column {
        sales: 800_000.5,
        cost_of_sales: 300_000.0,
        other_income: 20_000.0,
        deductions: 100_000.0,
        previous_quarters: 150_000.0,
        rate: 10.0,
        ..Form1702qSchedule1Column::default()
    };
    draft.sales = 5_000_000.5;
    draft.cost_of_sales = 1_200_000.0;
    draft.other_income = 100_000.49;
    draft.deductions = 900_000.0;
    draft.previous_quarters_taxable_income = 500_000.0;
    draft.mcit_gross_income_q1 = 3_000_000.0;
    draft.mcit_gross_income_q2 = 3_900_001.0;
    draft.prior_year_excess_credits = 10_000.0;
    draft.previous_quarters_payments = 150_000.0;
    draft.previous_quarters_mcit_payments = 20_000.0;
    draft.previous_quarters_cwt = 5_000.0;
    draft.cwt_this_quarter = 7_500.5;
    draft.other_credits = vec![Form1702qOtherCredit {
        description: "Sample credit".to_string(),
        amount: 1_234.0,
    }];
    draft.surcharge = 1_000.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1702q_sample_payload_is_current() {
    let draft = sample_1702q();
    assert_eq!(draft.total_amount_payable, 739_265.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1702Q must validate: {errors:?}"));
    check_sample(
        "1702Q-2025Q2",
        "1702q-v2018c",
        &payload,
        "65ad36691688357558676ec0186ae9394d17ef5df5f74769bfdb6ef65d8ddb65",
    );
}

/// Q2 2025 (calendar year): due 60 days after 30 June 2025.
#[test]
fn form_1702q_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1702q, Deadline::Calendar(day(2025, 8, 29)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // The same return in the third quarter, for Schedule 3 Item 3 (the 3rd
    // quarter's gross income, which only a third-quarter return keeps).
    let mut third = sample_1702q();
    third.quarter = 3;
    third.mcit_gross_income_q3 = 2_500_000.0;
    third.recompute();
    vec![
        serde_json::to_value(sample_1702q()).expect("sample serializes"),
        serde_json::to_value(third).expect("sample serializes"),
    ]
}
