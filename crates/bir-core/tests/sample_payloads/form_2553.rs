use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_2553::{FORM_2553_ATC_OPTIONS, Form2553Draft, Form2553TaxTreaty};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2553() -> Form2553Draft {
    let mut draft = Form2553Draft::new_from_profile(&dummy_profile("2553", "Corporation"), 2025, 1);
    draft.tax_treaty = Form2553TaxTreaty::SpecialRate;
    draft.set_atc(0, &FORM_2553_ATC_OPTIONS[0]).unwrap();
    draft.set_atc(1, &FORM_2553_ATC_OPTIONS[1]).unwrap();
    draft.set_atc(2, &FORM_2553_ATC_OPTIONS[3]).unwrap();
    draft.schedule[0].taxable_amount = 123_456.789;
    draft.schedule[1].taxable_amount = 2_500_000.0;
    draft.schedule[1].tax_rate = 3.5;
    draft.schedule[2].taxable_amount = 999.995;
    draft.creditable_tax_withheld = 1_000.0;
    draft.surcharge = 25.5;
    draft.interest = 10.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_2553_sample_payload_is_current() {
    let draft = sample_2553();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2553 must validate: {errors:?}"));
    check_sample(
        "2553-122025Q1",
        "2553-v1999",
        &payload,
        "808dc707b627b5dbaa32f625703fd8413ac2fe471d55dc3dce2d018a17ec9181",
    );
}

/// The calendar lists 2553 as event-based.
#[test]
fn form_2553_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2553,
        Deadline::None {
            assumed: day(2025, 12, 31),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // Overpaid (Item 23 below zero): the only state in which the Item 23
    // choice is kept (`checkOverpayment`), so the probe can bind its boxes.
    let mut overpaid = sample_2553();
    overpaid.creditable_tax_withheld = 5_000_000.0;
    overpaid.recompute();
    assert!(
        overpaid.total_amount_payable < 0.0,
        "the overpaid seed is overpaid"
    );
    overpaid.overpayment = bir_core::forms::form_2553::Form2553Overpayment::Refund;
    overpaid.recompute();
    vec![
        serde_json::to_value(sample_2553()).expect("sample serializes"),
        serde_json::to_value(overpaid).expect("overpaid sample serializes"),
    ]
}
