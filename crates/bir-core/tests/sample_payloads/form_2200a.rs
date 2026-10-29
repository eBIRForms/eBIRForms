use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_2200a::{Form2200ADraft, Form2200APayment, Form2200APlace};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2200a() -> Form2200ADraft {
    let mut draft =
        Form2200ADraft::new_from_profile(&dummy_profile("2200A", "Corporation"), 2025, 3);
    draft.day = 14;
    draft.number_of_attached_sheets = 1;
    draft.place_of_production = Form2200APlace {
        region: "130000000".into(),
        province: "137400000".into(),
        city: "137403000".into(),
    };
    draft.place_of_removal = Form2200APlace {
        region: "040000000".into(),
        province: "042100000".into(),
        city: "042103000".into(),
    };
    draft.tax_relief = true;
    draft.tax_relief_specify = "Sample relief law".into();
    draft.manner_of_payment = Form2200APayment::OtherScheme;
    draft.other_scheme_description = "Sample scheme".into();

    // Ad valorem 22% on a taxable base with a half-centavo product.
    let row = draft.row_by_key_mut("1A_2024").unwrap();
    row.export_exempt = 50.0;
    row.taxable = 10_000.125;
    // 21% from xml/taxRate.xml (the page itself prints 22%).
    draft.row_by_key_mut("1A_2020B").unwrap().taxable = 1_234.56;
    // Specific tax: 66.00 per proof liter.
    draft.row_by_key_mut("1B_2024").unwrap().taxable = 1_234.567;
    // Fermented liquors XA056 (row key 3B_2020B2 on the official page).
    draft.row_by_key_mut("3B_2020B2").unwrap().taxable = 99.99;
    let other = draft.other_mut(0).unwrap();
    other.atc_digits = "999".into();
    other.description = "Sample other product".into();
    other.bracket = "Per Liter".into();
    other.rate = "5.5".into();
    other.amounts.taxable = 2.0;
    other.amounts.tax_due = 11.0;

    draft.balance_carried_over = 1_000.0;
    draft.creditable_excise_tax = 234.565;
    draft.surcharge = 25.5;
    draft.interest = 10.0;
    draft.compromise = 1_000.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft.tax_deposit = draft.tax_still_due + 100.0;
    draft.recompute();
    draft
}

#[test]
fn form_2200a_sample_payload_is_current() {
    let draft = sample_2200a();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200A must validate: {errors:?}"));
    check_sample(
        "2200A-03142025",
        "2200a-v2020",
        &payload,
        "80270137fded0ec31f50299a555a077dbba2992efe37f23b58f6a446b3f6c61b",
    );
}

/// Due per removal (decision D8): the penalties run from Item 1, the
/// removal date; the calendar has no 2200-A deadline.
#[test]
fn form_2200a_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2200a,
        // Decision D8 (2026-10-18): the removal date the sample types (Item 1, 03/14/2025).
        Deadline::Typed {
            due: day(2025, 3, 14),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // PR 11: the sample's last typed row is 3B_2020B2, and recompute drops
    // the zero rows after it, so the probe never saw Schedule 1's last ten
    // fixed rows (3A_2021B ... 3B_2024) echo. A seed with the last fixed
    // row typed holds all sixty; its payload is never written.
    let mut all_rows = sample_2200a();
    all_rows.row_by_key_mut("3B_2024").unwrap().taxable = 1.0;
    all_rows.recompute();
    assert_eq!(
        all_rows.schedule.len(),
        bir_core::forms::form_2200a::FORM_2200A_FIXED_ROW_COUNT,
        "the all-rows seed holds every fixed row"
    );
    vec![
        serde_json::to_value(sample_2200a()).expect("sample serializes"),
        serde_json::to_value(all_rows).expect("all-rows sample serializes"),
    ]
}

/// Decision D8 (2026-10-18): 2200A is due on removal, and the penalties
/// run from Item 1, the removal date the filer types. Filed on that date
/// the return owes none; filed a month late it owes the 25% surcharge
/// (NIRC Sec. 248(A)), 12% a year interest for the 31 days (Sec. 249(B))
/// and the RMO 7-2015 compromise, all on Item 20's tax still due.
#[test]
fn form_2200a_penalties_run_from_the_removal_date() {
    use bir_core::forms::penalty_items::with_clock;
    use bir_core::penalties::PenaltyProfile;
    use bir_core::penalties::compromise::lookup_compromise;
    let cents = |value: f64| (value * 100.0).round() / 100.0;
    let items = Form2200ADraft::penalty_declaration();
    let mut draft = sample_2200a();
    // The sample keeps typed penalties (stored before D8); a new return is
    // on the typed due date from the start.
    draft.penalty.penalty_overrides = Default::default();
    items.join_typed_due_date(&mut draft);
    let removal = day(2025, 3, 14);
    assert_eq!((draft.month, draft.day, draft.year), (3, 14, 2025));
    assert_eq!(items.due_date(&draft), Some(removal));

    let mut on_time = draft.clone();
    with_clock(removal, || on_time.recompute());
    assert!(on_time.tax_still_due > 0.0);
    assert_eq!(
        [on_time.surcharge, on_time.interest, on_time.compromise],
        [0.0; 3]
    );
    assert_eq!(on_time.total_penalties, 0.0);

    let mut late = draft.clone();
    with_clock(day(2025, 4, 14), || late.recompute());
    let tax = late.tax_still_due;
    assert_eq!(tax, on_time.tax_still_due);
    let compromise = lookup_compromise(tax, 0.0, PenaltyProfile::StandardFiling);
    assert!(compromise > 0.0);
    assert_eq!(late.surcharge, cents(tax * 0.25));
    assert_eq!(late.interest, cents(tax * 0.12 * 31.0 / 365.0));
    assert_eq!(late.compromise, cents(compromise));
    assert_eq!(
        late.total_penalties,
        cents(late.surcharge + late.interest + late.compromise)
    );
    assert_eq!(late.amount_payable, cents(tax + late.total_penalties));
}
