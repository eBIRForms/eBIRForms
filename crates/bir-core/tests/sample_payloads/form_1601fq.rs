use crate::dummy_profile;
use crate::sample_payloads::penalty_items::{self, Deadline, day};
use bir_core::forms::form_1601fq::{Form1601FqCategory, Form1601FqDraft, Form1601FqTaxRelief};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1601fq() -> Form1601FqDraft {
    let mut draft =
        Form1601FqDraft::new_from_profile(&dummy_profile("1601FQ", "Corporation"), 2025, 2);
    // The sample enters its penalties by hand.
    draft.keep_entered_penalties();
    draft.set_any_tax_withheld(true);
    draft.set_category(Form1601FqCategory::Private);
    draft.set_tax_relief(Some(Form1601FqTaxRelief::InternationalTaxTreaty));
    // Popup positions; WC212 is listed twice (30% at 15, 25% at 29).
    for index in [29, 0, 15, 2, 28] {
        draft.add_atc(index).unwrap();
    }
    for (row, base) in
        draft
            .schedule
            .iter_mut()
            .zip([123_456.789, 1_000.005, 50_000.0, 77_777.775, 999.99])
    {
        row.tax_base = base;
    }
    draft.schedule1[0].treaty_code = "US".into();
    draft.set_schedule1_atc(0, "WI330");
    draft.schedule1[0].income_payment = 100_000.005;
    draft.schedule1[0].tax_rate = 15.0;
    draft.schedule1[1].treaty_code = "JP".into();
    draft.set_schedule1_atc(1, "WC180");
    draft.schedule1[1].income_payment = 50_000.0;
    draft.remittance_first_month = 500.0;
    draft.remittance_second_month = 1_000.505;
    draft.surcharge = 25.5;
    draft.interest = 10.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1601fq_sample_payload_is_current() {
    let draft = sample_1601fq();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1601FQ must validate: {errors:?}"));
    crate::check_sample_with_layout(
        "1601FQ-2025Q2",
        &draft.official_layout().unwrap(),
        &payload,
        "d94b93bb3037e91bf9528fc50b44e03baa174ca2fbc364fc39bad5af3bfc2d69",
    );
}

/// Q2 2025: due the last day of July 2025.
#[test]
fn form_1601fq_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1601fq, Deadline::Calendar(day(2025, 7, 31)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![serde_json::to_value(sample_1601fq()).expect("sample serializes")]
}
