use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_2200t::{Form2200TDraft, Form2200TPayment, Form2200TPlace};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2200t() -> Form2200TDraft {
    let mut draft =
        Form2200TDraft::new_from_profile(&dummy_profile("2200T", "Corporation"), 2025, 3);
    draft.day = 14;
    draft.number_of_attached_sheets = 1;
    draft.place_of_production = Form2200TPlace {
        region: "130000000".into(),
        province: "137400000".into(),
        city: "137403000".into(),
    };
    draft.place_of_removal = Form2200TPlace {
        region: "040000000".into(),
        province: "042100000".into(),
        city: "042103000".into(),
    };
    draft.tax_relief = true;
    draft.tax_relief_specify = "Sample relief law".into();
    draft.manner_of_payment = Form2200TPayment::OtherScheme;
    draft.other_scheme_description = "Sample scheme".into();

    let row = draft.row_by_key_mut("4B_2023").unwrap();
    row.export_exempt = 120.0;
    row.taxable = 1_000.5;
    row.tax_due = 60_030.005;
    let row = draft.row_by_key_mut("5_1.23.2020").unwrap();
    row.taxable = 10.0;
    row.tax_due = 250.0;
    let row = draft.row_by_key_mut("xt090").unwrap();
    row.taxable = 1_234_567.891;
    row.tax_due = 123.46;
    let other = draft.other_mut(0).unwrap();
    other.atc_digits = "999".into();
    other.description = "Sample other product".into();
    other.bracket = "Per Pack".into();
    other.rate = "5.00".into();
    other.amounts.taxable = 2.0;
    other.amounts.tax_due = 10.0;

    draft.balance_carried_over = 1_000.0;
    draft.creditable_excise_tax = 234.565;
    draft.surcharge = 25.5;
    draft.interest = 10.0;
    draft.compromise = 1_000.0;
    draft.keep_entered_penalties();
    draft.tax_deposit = 50_000.0;
    draft.recompute();
    draft
}

#[test]
fn form_2200t_sample_payload_is_current() {
    let draft = sample_2200t();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200T must validate: {errors:?}"));
    check_sample(
        "2200T-03142025",
        "2200t-v2020",
        &payload,
        "7d0327c4ba19b3ed02d29a6b5ea5e94fa4f16bba9b864666ea110f01358ef123",
    );
}

/// Due per removal (decision D8): the penalties run from Item 1, the
/// removal date; the calendar has no 2200-T deadline.
#[test]
fn form_2200t_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2200t,
        // Decision D8 (2026-10-18): the removal date the sample types (Item 1, 03/14/2025).
        Deadline::Typed {
            due: day(2025, 3, 14),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // PR 11: the sample's last typed row is xt090, and recompute drops the
    // zero rows after it, so the probe never saw Schedule 1's last six fixed
    // rows (xt100 ... xt200) echo. A seed with the last fixed row typed
    // holds them all; its payload is never written.
    let mut all_rows = sample_2200t();
    let row = all_rows.row_by_key_mut("xt200").unwrap();
    row.taxable = 1.0;
    row.tax_due = 0.01;
    all_rows.recompute();
    assert_eq!(
        all_rows.schedule.len(),
        bir_core::forms::form_2200t::FORM_2200T_FIXED_ROW_COUNT,
        "the all-rows seed holds every fixed row"
    );
    vec![
        serde_json::to_value(sample_2200t()).expect("sample serializes"),
        serde_json::to_value(all_rows).expect("all-rows sample serializes"),
    ]
}
