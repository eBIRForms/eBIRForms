use crate::{check_sample, dummy_profile};
use bir_core::forms::form_2551q::{Form2551QDraft, Item13Election};

fn sample_2551q() -> Form2551QDraft {
    let mut draft =
        Form2551QDraft::new_from_profile(&dummy_profile("2551Qv2018", "Individual"), 2025, 1);
    draft.item_13_election = Item13Election::Graduated;
    draft.auto_compute_penalties = false;
    draft.surcharge = 0.0;
    draft.interest = 0.0;
    draft.compromise = 0.0;
    draft.schedule_1[0].taxable_amount = 180_000.0;
    draft.creditable_tax_withheld = 500.0;
    draft.recompute(None);
    draft
}

#[test]
fn form_2551q_sample_payload_is_current() {
    let draft = sample_2551q();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2551Q must validate: {errors:?}"));
    check_sample(
        "2551Q-122025Q1",
        "2551q-v2018",
        &payload,
        "03ae790bfe2eb3593248f8204aea9d4614f4cecfadd642da61732d963f584d8d",
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![serde_json::to_value(sample_2551q()).expect("sample serializes")]
}
