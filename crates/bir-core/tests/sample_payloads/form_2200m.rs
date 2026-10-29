use crate::dummy_profile;
use crate::sample_payloads::penalty_items::{self, Deadline, day};
use bir_core::forms::form_2200m::{Form2200MDraft, Form2200MPayment, Form2200MPlace};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2200m() -> Form2200MDraft {
    let mut draft =
        Form2200MDraft::new_from_profile(&dummy_profile("2200M", "Corporation"), 2025, 3);
    draft.day = 14;
    draft.number_of_attached_sheets = 2;
    draft.place_of_production = Form2200MPlace {
        region: "130000000".into(),
        province: "137400000".into(),
        city: "137403000".into(),
    };
    draft.place_of_removal = Form2200MPlace {
        region: "040000000".into(),
        province: "042100000".into(),
        city: "042103000".into(),
    };
    draft.tax_relief = Some(true);
    draft.tax_relief_specify = "Sample relief law".into();
    draft.manner_of_payment = Form2200MPayment::OtherScheme;
    draft.other_scheme_description = "Sample scheme".into();

    let row = draft.row_mut(0).unwrap();
    row.place_of_removal = "Pasig yard".into();
    row.volume_taxable = Some(1_234.567);
    row.volume_exempt = Some(10.0);
    row.local_value_taxable = Some(250_000.0);
    row.local_tax_due = Some(61_728.355);
    row.total_tax_due = Some(61_728.36);
    let row = draft.row_mut(3).unwrap();
    row.place_of_removal = "Rizal quarry".into();
    row.local_value_taxable = Some(999_999.995);
    row.local_value_exempt = Some(0.0);
    row.local_tax_due = Some(40_000.0);
    row.imported_taxable = Some(12_345.6);
    row.imported_exempt = Some(1.0);
    row.imported_tax_due = Some(493.82);
    row.adjustment = Some(0.5);
    row.total_tax_due = Some(40_494.32);
    let row = draft.row_mut(8).unwrap();
    row.description = "Sand and gravel".into();
    row.place_of_removal = "Laguna site".into();
    row.local_rate = Some(4.0);
    row.local_tax_due = Some(5.005);
    row.imported_rate = Some(2.5);
    let row = draft.row_mut(9).unwrap();
    row.description = "Marble".into();
    row.place_of_removal = "Romblon".into();
    row.volume_taxable = Some(3.0);
    row.local_tax_due = Some(120.0);

    // Two rows past the page's ten, added with "Add row".
    let row = draft.row_mut(10).unwrap();
    row.description = "Clay x".into();
    row.place_of_removal = "Rizal".into();
    row.local_tax_due = Some(10.0);
    row.imported_taxable = Some(7.0);
    let row = draft.row_mut(11).unwrap();
    row.description = "Shale".into();
    row.place_of_removal = "Bulacan".into();
    row.total_tax_due = Some(3.0);

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
fn form_2200m_sample_payload_is_current() {
    let draft = sample_2200m();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200M must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "2200M-03142025",
        &layout,
        &payload,
        "2f76621a8578a8c47dadc7b5045916e0531383662d32da15647e2e0c209af521",
    );
}

/// Six rows past the page's ten: `addRowSched1` has no row limit, and each
/// added row's controls follow the previous row's in every column table.
fn sample_2200m_many_rows() -> Form2200MDraft {
    let mut draft = sample_2200m();
    draft.day = 13;
    for (k, index) in (13..=16u32).zip(12usize..) {
        let row = draft.row_mut(index).unwrap();
        row.description = format!("Added mineral {k}");
        row.place_of_removal = format!("Site {k}");
        row.volume_taxable = Some(f64::from(k) * 1.5);
        row.local_rate = Some(4.0);
        row.local_tax_due = Some(f64::from(k) * 100.0 + 0.25);
        row.total_tax_due = Some(f64::from(k) * 100.0);
    }
    draft.recompute();
    draft.tax_deposit = draft.tax_still_due + 100.0;
    draft.recompute();
    draft
}

#[test]
fn form_2200m_sample_with_many_rows_is_current() {
    let draft = sample_2200m_many_rows();
    assert_eq!(draft.row_count(), 16);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200M must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "2200M-03132025",
        &layout,
        &payload,
        "3e68f97ca2c240fd596325bfea7318f657d708c68e33adb331c30b8d720b1f74",
    );
}

/// The calendar's 2200-M rule is quarterly; this return is dated, and due
/// per removal (decision D8): the penalties run from Item 1.
#[test]
fn form_2200m_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2200m,
        // Decision D8 (2026-10-18): the removal date the sample types (Item 1, 03/14/2025).
        Deadline::Typed {
            due: day(2025, 3, 14),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_2200m()).expect("sample serializes"),
        serde_json::to_value(sample_2200m_many_rows()).expect("sample serializes"),
    ]
}
