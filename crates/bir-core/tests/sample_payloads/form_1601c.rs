use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1601c::{Form1601CDraft, Form1601CSchedule1Row};

fn sample_1601c() -> Form1601CDraft {
    let mut draft =
        Form1601CDraft::new_from_profile(&dummy_profile("1601Cv2018", "Corporation"), 2025, 6);
    draft.auto_compute_penalties = false;
    // Pin Items 32-34 at zero: the period is past its deadline, so the
    // computed penalties would otherwise grow with the clock and change the
    // hash-locked payload every day.
    for component in bir_core::penalties::PenaltyComponent::ALL {
        draft.set_penalty_override(component, Some(0.0));
    }
    draft.tax_14_total_compensation = 250_000.0;
    draft.tax_15_statutory_minimum_wage = 0.0;
    draft.tax_17_13th_month_pay = 20_000.0;
    draft.tax_18_de_minimis = 5_000.0;
    draft.tax_19_sss_gsis = 12_500.0;
    draft.tax_25_total_taxes_withheld = 18_750.0;
    draft.schedule_1 = vec![Form1601CSchedule1Row {
        previous_month: "04/2025".to_string(),
        date_paid: "05/09/2025".to_string(),
        drawee_bank_code_or_agency: "SAMPLE BANK".to_string(),
        payment_number: "REF-0001".to_string(),
        tax_paid: 15_000.0,
        should_be_tax_due: 15_250.0,
        adjustment: 0.0,
    }];
    draft.compute();
    draft
}

#[test]
fn form_1601c_sample_payload_is_current() {
    let draft = sample_1601c();
    let payload = draft
        .try_to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1601C must validate: {errors:?}"));
    check_sample(
        "1601C-062025",
        "1601c-v2018",
        &payload,
        "3998c1b12d3c00f4e3c7ea6d36db21ac214721900c04a0aae4b2637f620126fd",
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![serde_json::to_value(sample_1601c()).expect("sample serializes")]
}
