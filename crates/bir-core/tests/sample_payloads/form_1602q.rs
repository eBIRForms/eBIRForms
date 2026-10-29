use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1602q::{
    Form1602QCategory, Form1602QDraft, Form1602QPreferentialRow, Form1602QTaxRelief,
    Form1602QTreatyRow,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1602q() -> Form1602QDraft {
    let mut draft =
        Form1602QDraft::new_from_profile(&dummy_profile("1602Q", "Corporation"), 2025, 3);
    draft.is_amended = Some(false);
    draft.category = Some(Form1602QCategory::Private);
    draft.set_any_tax_withheld(true);
    draft.set_availing_tax_relief(true);
    draft.tax_relief = Form1602QTaxRelief::InternationalTaxTreaty;
    draft.schedule1_amount[0] = 100_000.005;
    draft.schedule1_amount[2] = 250_000.0;
    draft.schedule1_amount[8] = 50_000.0;
    draft.schedule1_amount[9] = 1_234.565;
    draft.schedule1_amount[12] = 10_000.0;
    draft.bsp_rate[0] = 56.789;
    draft.schedule2[0] = Form1602QTreatyRow {
        treaty_code: "US".into(),
        atc_code: "WC161".into(),
        interest: 30_000.0,
        tax_rate: 10.0,
        tax_withheld: 0.0,
    };
    draft.schedule3[0] = Form1602QPreferentialRow {
        ipa_code: "PEZA".into(),
        interest: 5_000.0,
        tax_rate: 5.0,
        tax_withheld: 0.0,
    };
    draft.remittance_first_month = 1_000.0;
    draft.remittance_second_month = 2_000.505;
    draft.over_remittance_previous_quarter = 500.0;
    draft.surcharge = 25.5;
    draft.interest_penalty = 10.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1602q_sample_payload_is_current() {
    let draft = sample_1602q();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1602Q must validate: {errors:?}"));
    check_sample(
        "1602Q-2025Q3",
        "1602q-v2018",
        &payload,
        "807e6dbd23ee19917162dbd068f726b9f417db9251bd5d804c0618b63c563f8b",
    );
}

/// Q3 2025: due the last day of October 2025.
#[test]
fn form_1602q_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1602q, Deadline::Calendar(day(2025, 10, 31)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // Overpaid (Item 28 below zero): the only state in which its options
    // are kept, so the probe can bind their radios.
    let mut overpaid = sample_1602q();
    overpaid.remittance_first_month = 5_000_000.0;
    overpaid.recompute();
    assert!(overpaid.item28 < 0.0, "the overpaid seed is overpaid");
    overpaid.over_remittance = bir_core::forms::form_1602q::Form1602QOverRemittance::Refund;
    overpaid.recompute();
    vec![
        serde_json::to_value(sample_1602q()).expect("sample serializes"),
        serde_json::to_value(overpaid).expect("overpaid sample serializes"),
    ]
}
