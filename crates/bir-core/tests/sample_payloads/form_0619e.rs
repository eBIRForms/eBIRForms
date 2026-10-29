use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_0619e::Form0619EDraft;
use bir_core::forms::queueable::QueueableForm as _;

/// Amended, with tax withheld, rounding cases and an address long enough to
/// use the official second address line.
fn sample_0619e() -> Form0619EDraft {
    let mut draft =
        Form0619EDraft::new_from_profile(&dummy_profile("0619E", "Corporation"), 2025, 6);
    draft.registered_address =
        "Unit 1203 Sample Tower, 456 Example Avenue corner Placeholder Street, Barangay Sample, Quezon City"
            .into();
    draft.is_amended = true;
    draft.any_taxes_withheld = true;
    draft.item_14_amount_of_remittance = 123_456.785;
    draft.item_15_amount_remitted_previously = 1_000.005;
    draft.item_17a_surcharge = 25.5;
    draft.item_17b_interest = 10.0;
    draft.item_17c_compromise = 1_000.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_0619e_sample_payload_is_current() {
    let payload = sample_0619e()
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 0619E must validate: {errors:?}"));
    check_sample(
        "0619E-062025",
        "0619e-v2018",
        &payload,
        "de77106ec739d47ed7e70b618c2d274e536d0f66b79f40dcc8f2d92fec8f0b6a",
    );
}

/// June 2025: due 10 July 2025.
#[test]
fn form_0619e_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_0619e, Deadline::Calendar(day(2025, 7, 10)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![serde_json::to_value(sample_0619e()).expect("sample serializes")]
}
