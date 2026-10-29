use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::excise_places::ExcisePlace;
use bir_core::forms::form_2200s::{Form2200SDraft, Form2200SManner, Form2200SOtherRow};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2200s() -> Form2200SDraft {
    let mut draft =
        Form2200SDraft::new_from_profile(&dummy_profile("2200S", "Corporation"), 2025, 6);
    draft.month = 6;
    draft.day = 30;
    draft.psic = "1104".into();
    draft.place_of_production = ExcisePlace {
        region: "130000000".into(),
        province: "137400000".into(),
        city: "137404000".into(),
    };
    draft.place_of_removal = ExcisePlace {
        region: "040000000".into(),
        province: "043400000".into(),
        city: "043402000".into(),
    };
    draft.manner = Form2200SManner::ActualRemoval;
    draft.recompute();
    draft.beverages[0].sales_value = 98_765.43;
    draft.beverages[0].volume = 12_345.678;
    draft.beverages[2].volume = 10_000.509;
    draft.beverages[8].volume = 100.0;
    draft.others = vec![Form2200SOtherRow {
        atc: "xb200".into(),
        description: "Other sweetened drink".into(),
        tax_bracket: "Per Liter".into(),
        rate: 3.5,
        sales_value: 1_000.0,
        volume: 200.25,
        basic_tax_due: 0.0,
    }];
    draft.balance_carried_over = 100.5;
    draft.creditable_excise_tax = 50.0;
    draft.recompute();
    draft.tax_payment = draft.amount_payable;
    draft.recompute();
    draft
}

#[test]
fn form_2200s_sample_payload_is_current() {
    let draft = sample_2200s();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200-S must validate: {errors:?}"));
    check_sample(
        "2200S-06302025",
        "2200s-v2018",
        &payload,
        "ec1611325d60a52c206ed50cd232125e94cfd8759806964c9338357e6f7871d7",
    );
}

/// Five "Others" rows: `addFieldsForSched1` appends rows to
/// `frm2200SadditionalSched1` without a limit, each after the previous one.
fn sample_2200s_many_rows() -> Form2200SDraft {
    let mut draft = sample_2200s();
    draft.day = 29;
    for k in 1..=4u32 {
        draft.others.push(Form2200SOtherRow {
            atc: format!("XB2{k}0"),
            description: format!("Added drink {k}"),
            tax_bracket: "Per Liter".into(),
            rate: f64::from(k) + 0.25,
            sales_value: f64::from(k) * 1_000.5,
            volume: f64::from(k) * 10.125,
            basic_tax_due: 0.0,
        });
    }
    draft.recompute();
    draft.tax_payment = draft.amount_payable;
    draft.recompute();
    draft
}

#[test]
fn form_2200s_sample_with_many_rows_is_current() {
    let draft = sample_2200s_many_rows();
    assert_eq!(draft.others.len(), 5);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200-S must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "2200S-06292025",
        &layout,
        &payload,
        "4d4a96ac185982f983f2d63f5ad8de61f6a39417daa7c7c71fcd922f96cfa4cc",
    );
}

/// The sample as a prepayment (Item 13 manner of payment).
fn sample_2200s_prepayment() -> Form2200SDraft {
    let mut draft = sample_2200s();
    draft.manner = Form2200SManner::Prepayment;
    draft.recompute();
    draft
}

/// Items 23A-23C are open on a prepayment only, and run from Item 1, the
/// removal date (decision D8).
#[test]
fn form_2200s_prepayment_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2200s_prepayment,
        // Decision D8 (2026-10-18): the removal date the sample types (Item 1, 06/30/2025).
        Deadline::Typed {
            due: day(2025, 6, 30),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // PR 11: a prepayment with Item 17's Other Similar Schemes and its
    // penalties entered (as 0.00, so 25B's Pay Penalties flag stays off and
    // does not move with the manner). Items 23A-23C are open on a
    // prepayment only and Item 17's text only with the scheme ticked
    // (form_2200s.rs recompute clears them otherwise), so the samples above
    // never let the probe bind txtTax21A-txtTax21C or txtOthMannerofPymnt.
    // Its payload is never written.
    let mut prepayment = sample_2200s_prepayment();
    prepayment.other_scheme = true;
    prepayment.other_scheme_text = "Sample scheme".into();
    prepayment.keep_entered_penalties();
    prepayment.recompute();
    assert_eq!(prepayment.other_scheme_text, "Sample scheme");
    assert_eq!(prepayment.penalty.penalty_overrides.surcharge, Some(0.0));
    assert_eq!(prepayment.penalties_paid, 0.0);
    vec![
        serde_json::to_value(sample_2200s()).expect("sample serializes"),
        serde_json::to_value(sample_2200s_many_rows()).expect("sample serializes"),
        serde_json::to_value(prepayment).expect("prepayment sample serializes"),
    ]
}
