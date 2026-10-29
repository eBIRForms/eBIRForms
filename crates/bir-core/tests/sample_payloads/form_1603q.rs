use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1603q::{Form1603QCategory, Form1603QDraft, Form1603QTaxRelief};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1603q() -> Form1603QDraft {
    let mut draft =
        Form1603QDraft::new_from_profile(&dummy_profile("1603Q", "Corporation"), 2025, 3);
    draft.category = Some(Form1603QCategory::Government);
    draft.tax_relief = Some(Form1603QTaxRelief::Both);
    draft.set_any_tax_withheld(true);
    draft.monetary_value = [100_000.005, 54_321.1];
    draft.other_remittances_specify = "Bank remittance".into();
    draft.other_remittances = 1_000.505;
    draft.surcharge = 25.5;
    draft.interest = 10.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1603q_sample_payload_is_current() {
    let draft = sample_1603q();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1603Q must validate: {errors:?}"));
    check_sample(
        "1603Q-2025Q3",
        "1603q-v2018",
        &payload,
        "33acf73277ef10fc2743ec4eeba370b40c57aac22b82825d59aa09aad5ad869d",
    );
}

/// Q3 2025: due the last day of October 2025.
#[test]
fn form_1603q_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1603q, Deadline::Calendar(day(2025, 10, 31)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![serde_json::to_value(sample_1603q()).expect("sample serializes")]
}
