use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::excise_places::ExcisePlace;
use bir_core::forms::form_2200c::{Form2200CDraft, Form2200CManner, Form2200CRow};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2200c() -> Form2200CDraft {
    let mut draft =
        Form2200CDraft::new_from_profile(&dummy_profile("2200Cv2018", "Corporation"), 2025);
    draft.month = 6;
    draft.day = 30;
    draft.is_amended = true;
    draft.previous_payment = 400.0;
    draft.place = ExcisePlace {
        region: "130000000".into(),
        province: "137400000".into(),
        city: "137404000".into(),
    };
    draft.tax_relief = Some(true);
    draft.tax_relief_specify = "special law sample".into();
    draft.manner = Form2200CManner::ActualRemoval;
    let row = |a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64| Form2200CRow {
        exempt_procedures: a,
        excisable_procedures: b,
        non_excisable_procedures: c,
        net_of_vat: d,
        excisable_net: e,
        excisable_vat_exempt: f,
        non_excisable: g,
        ..Default::default()
    };
    draft.schedule = vec![
        row(1.0, 3.0, 2.0, 10_000.0, 50_010.0, 0.0, 5_000.0),
        row(2.0, 5.0, 1.0, 0.0, 12_345.0, 6_789.0, 0.0),
        row(1.0, 1.0, 1.0, 2_500.0, 999.0, 1.0, 0.0),
    ];
    draft.balance_carried_over = 100.0;
    draft.creditable_excise_tax = 8.0;
    draft.surcharge = 25.0;
    draft.compromise = 10.0;
    draft.keep_entered_penalties();
    draft.tax_payment = 3_000.0;
    draft.recompute();
    draft
}

#[test]
fn form_2200c_sample_payload_is_current() {
    let draft = sample_2200c();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200-C must validate: {errors:?}"));
    check_sample(
        "2200C-06302025",
        "2200c-v2018",
        &payload,
        "71c985b171d1b0c4a0e1e34a213deb0428882ab572fbec4b554c29ac1c425826",
    );
}

/// Item 1 is a date the calendar's monthly 2200-C rule is not keyed to;
/// monthly (decision D8), the penalties run from the 10th after its month.
#[test]
fn form_2200c_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2200c,
        // Decision D8 (2026-10-18): the 10th of the month after the sample's Item 1 month (06/30/2025).
        Deadline::Typed {
            due: day(2025, 7, 10),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![serde_json::to_value(sample_2200c()).expect("sample serializes")]
}

/// Decision D8 (2026-10-18): 2200C is monthly and "the excise tax due, if
/// any" is paid with it (html-frozen/2200c-2018/guide.html: "In cases of no
/// invasive cosmetic procedures performed, the return shall still be
/// filed"), so a zero return with no Schedule 1 row validates and builds
/// its upload.
#[test]
fn form_2200c_a_zero_return_validates() {
    let mut draft = sample_2200c();
    draft.is_amended = false;
    draft.schedule.clear();
    draft.balance_carried_over = 0.0;
    draft.creditable_excise_tax = 0.0;
    draft.tax_payment = 0.0;
    draft.penalty.penalty_overrides = Default::default();
    draft.recompute();
    assert_eq!(draft.excise_tax_due, 0.0);
    assert_eq!(draft.amount_payable, 0.0);
    assert_eq!(draft.validate(), Vec::<(String, String)>::new());
    assert!(draft.to_bir_xml_payload().is_ok(), "the upload builds");
}

/// Decision D8 (2026-10-18): 2200C is monthly, due within ten days after
/// the close of Item 1's month, so the penalties run from the 10th of the
/// next month (January 10 of the next year for December), moved to the next
/// business day when it falls on a weekend. Filed on the 10th the return
/// owes none; on the 11th it owes the 25% surcharge.
#[test]
fn form_2200c_penalties_run_from_the_10th_of_the_next_month() {
    use bir_core::forms::penalty_items::with_clock;
    let items = Form2200CDraft::penalty_declaration();
    let mut draft = sample_2200c();
    draft.penalty.penalty_overrides = Default::default();
    items.join_typed_due_date(&mut draft);
    assert_eq!((draft.month, draft.day, draft.year), (6, 30, 2025));
    assert_eq!(items.due_date(&draft), Some(day(2025, 7, 10)));
    for (month, day_of_month, due) in [
        (1, 1, day(2025, 2, 10)),
        (1, 31, day(2025, 2, 10)),
        (11, 15, day(2025, 12, 10)),
        // January 10, 2026 is a Saturday: the next business day, as the
        // app's calendar moves every deadline.
        (12, 31, day(2026, 1, 12)),
    ] {
        let mut other = draft.clone();
        other.month = month;
        other.day = day_of_month;
        assert_eq!(items.due_date(&other), Some(due), "{month}/{day_of_month}");
    }

    let mut on_time = draft.clone();
    with_clock(day(2025, 7, 10), || on_time.recompute());
    assert!(on_time.tax_still_due > 0.0);
    assert_eq!(
        [on_time.surcharge, on_time.interest, on_time.compromise],
        [0.0; 3]
    );

    let mut late = draft.clone();
    with_clock(day(2025, 7, 11), || late.recompute());
    // Whole pesos (the 2200-C items take no centavos).
    assert_eq!(late.surcharge, (late.tax_still_due * 0.25).round());
    assert!(late.compromise > 0.0);
}
