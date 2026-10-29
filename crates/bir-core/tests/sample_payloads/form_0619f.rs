use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_0619f::{Form0619FDraft, Form0619FTaxType};
use bir_core::forms::queueable::QueueableForm as _;

/// Tax type WF (Item 14), amended, rounding cases.
fn sample_0619f() -> Form0619FDraft {
    let mut draft =
        Form0619FDraft::new_from_profile(&dummy_profile("0619F", "Corporation"), 2025, 6);
    draft.is_amended = true;
    draft.any_taxes_withheld = true;
    draft.tax_type = Form0619FTaxType::WF;
    draft.item_14_other_final_tax_withheld = 98_765.435;
    draft.item_16_remitted_previously = 500.005;
    draft.item_18a_surcharge = 25.5;
    draft.item_18b_interest = 10.0;
    draft.item_18c_compromise = 1_000.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_0619f_sample_payload_is_current() {
    let payload = sample_0619f()
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 0619F must validate: {errors:?}"));
    check_sample(
        "0619F-062025WF",
        "0619f-v2018",
        &payload,
        "8d17d3aa0f998171c36a7e1d129332ddaf0aa2092695b82ec89646d1e6ebf22b",
    );
}

/// June 2025: due 10 July 2025.
#[test]
fn form_0619f_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_0619f, Deadline::Calendar(day(2025, 7, 10)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![serde_json::to_value(sample_0619f()).expect("sample serializes")]
}
