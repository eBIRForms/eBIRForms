use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_2200p::{Form2200PDraft, Form2200PPayment, Form2200PPlace};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2200p() -> Form2200PDraft {
    let mut draft =
        Form2200PDraft::new_from_profile(&dummy_profile("2200P", "Corporation"), 2025, 3);
    draft.day = 14;
    draft.number_of_attached_sheets = 2;
    draft.place_of_production = Form2200PPlace {
        region: "130000000".into(),
        province: "137400000".into(),
        city: "137403000".into(),
    };
    draft.place_of_removal = Form2200PPlace {
        region: "040000000".into(),
        province: "042100000".into(),
        city: "042103000".into(),
    };
    draft.tax_relief = Some(true);
    draft.tax_relief_specify = "Sample relief law".into();
    draft.manner_of_payment = Form2200PPayment::OtherScheme;
    draft.other_scheme_description = "Sample scheme".into();

    // Row 6 — XP060 unleaded premium gasoline.
    let row = draft.row_mut(5).unwrap();
    row.place_of_removal = "Batangas depot".into();
    row.local_manufactured = Some(10_000.005);
    row.imported = Some(2_500.0);
    row.total_taxable_removals = Some(12_500.01);
    row.basic_excise_tax_due = Some(125_000.1);
    // Row 16 — XP140 diesel.
    let row = draft.row_mut(15).unwrap();
    row.place_of_removal = "Pasig terminal".into();
    row.under_bond = Some(100.0);
    row.exports = Some(0.0);
    row.total_tax_paid = Some(100.0);
    row.basic_excise_tax_due = Some(999.995);
    // Row 26 — others.
    let row = draft.row_mut(25).unwrap();
    row.atc_digits = "999".into();
    row.description = "Other fuel".into();
    row.unit_of_measure = "Per liter".into();
    row.applicable_rate = Some(2.5);
    row.place_of_removal = "Bataan".into();
    row.imported = Some(40.0);
    row.basic_excise_tax_due = Some(100.0);

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
fn form_2200p_sample_payload_is_current() {
    let draft = sample_2200p();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200P must validate: {errors:?}"));
    check_sample(
        "2200P-03142025",
        "2200p-v2020",
        &payload,
        "71390796ec132328c492e3da1119d3133a6580b3de702b2ed344d196eaa1b647",
    );
}

/// Rows past the page's 27: `addRowSched1` has no row limit once rows 26
/// and 27 are complete, and each added row follows the previous one.
fn sample_2200p_many_rows() -> Form2200PDraft {
    let mut draft = sample_2200p();
    draft.day = 13;
    let row = draft.row_mut(26).unwrap();
    row.atc_digits = "998".into();
    row.description = "Bunker blend".into();
    row.unit_of_measure = "Per liter".into();
    row.applicable_rate = Some(3.0);
    row.place_of_removal = "Cebu".into();
    row.local_manufactured = Some(12.5);
    row.basic_excise_tax_due = Some(37.5);
    for (k, index) in (28..=31u32).zip(27usize..) {
        let row = draft.row_mut(index).unwrap();
        row.atc_digits = format!("9{k}");
        row.description = format!("Added fuel {k}");
        row.unit_of_measure = "Per kg".into();
        row.applicable_rate = Some(f64::from(k) / 10.0);
        row.place_of_removal = format!("Depot {k}");
        row.imported = Some(f64::from(k) * 100.0);
        row.exports = Some(1.005);
        row.basic_excise_tax_due = Some(f64::from(k) * 10.0 + 0.25);
    }
    draft.recompute();
    draft.tax_deposit = draft.tax_still_due + 100.0;
    draft.recompute();
    draft
}

#[test]
fn form_2200p_sample_with_many_rows_is_current() {
    let draft = sample_2200p_many_rows();
    assert_eq!(draft.row_count(), 31);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200P must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "2200P-03132025",
        &layout,
        &payload,
        "140689c12e730396a14557d7c7c34c45a2352ef3d2c3f9ea960026774d5d755a",
    );
}

/// Due per removal (decision D8): the penalties run from Item 1, the
/// removal date; the calendar has no 2200-P deadline.
#[test]
fn form_2200p_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2200p,
        // Decision D8 (2026-10-18): the removal date the sample types (Item 1, 03/14/2025).
        Deadline::Typed {
            due: day(2025, 3, 14),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_2200p()).expect("sample serializes"),
        serde_json::to_value(sample_2200p_many_rows()).expect("sample serializes"),
    ]
}
