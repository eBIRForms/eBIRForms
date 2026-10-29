use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::excise_places::ExcisePlace;
use bir_core::forms::form_2200an::{
    Form2200ANDraft, Form2200ANManner, Form2200ANOtherGood, Form2200ANRow,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2200an() -> Form2200ANDraft {
    let mut draft =
        Form2200ANDraft::new_from_profile(&dummy_profile("2200ANv2018", "Corporation"), 2025, 6);
    draft.month = 6;
    draft.day = 30;
    draft.is_amended = true;
    draft.previous_payment = 1_000.0;
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
    draft.tax_relief = Some(true);
    draft.tax_relief_specify = "Sample Relief".into();
    draft.manner = Form2200ANManner::Other;
    draft.manner_other = "Sample scheme".into();
    let row = |eu: &str, tu: &str, ev: f64, tv: f64, due: f64| Form2200ANRow {
        exempt_units: eu.into(),
        taxable_units: tu.into(),
        exempt_value: ev,
        taxable_value: tv,
        tax_due: due,
    };
    draft.recompute();
    draft.automobiles[0] = row("", "2", 0.0, 1_000_000.0, 40_000.0);
    // XG033 is the seventh automobile row.
    draft.automobiles[6] = row("1", "3", 2_500_000.0, 7_500_000.0, 1_500_000.005);
    draft.goods[1] = row("", "10", 0.0, 250_000.5, 50_000.1);
    draft.other_goods = vec![Form2200ANOtherGood {
        atc: "XG130".into(),
        name: "Sample luxury item".into(),
        rate: "20%".into(),
        row: row("", "1", 0.0, 10_000.0, 2_000.0),
    }];
    draft.balance_carried_over = 500.0;
    draft.creditable_excise_tax = 250.25;
    draft.surcharge = 100.0;
    draft.interest = 12.345;
    draft.keep_entered_penalties();
    draft.tax_payment = 1_500_000.0;
    draft.recompute();
    draft
}

#[test]
fn form_2200an_sample_payload_is_current() {
    let draft = sample_2200an();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2200-AN must validate: {errors:?}"));
    check_sample(
        "2200AN-06302025",
        "2200an-v2018",
        &payload,
        "eeca56971ee38ec266aa889ca9cff22cf9ac0609830635edd683946d234f14b6",
    );
}

/// Due per removal (decision D8): the penalties run from Item 1, the
/// removal date; the calendar has no 2200-AN deadline.
#[test]
fn form_2200an_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2200an,
        // Decision D8 (2026-10-18): the removal date the sample types (Item 1, 06/30/2025).
        Deadline::Typed {
            due: day(2025, 6, 30),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
/// PR 11 (2200AN preview.0): Item 3 is the official open `txtSheets` box
/// (rules/forms/2200an-v2018/fixtures/runtime-control-inventory-v796.json:
/// not disabled, maxlength 2, default "0"), so the draft keeps what the
/// filer types and the upload writes it. A draft stored before the field
/// existed opens at 0, writes the official default "0" as before, and saves
/// back without the key.
#[test]
fn form_2200an_item_3_files_the_sheets_typed() {
    let sample = sample_2200an();
    assert_eq!(sample.field_map()["frm2200ANv2018:txtSheets"], "0");
    let stored = serde_json::to_value(&sample).unwrap();
    assert!(stored.get("number_of_attached_sheets").is_none());
    let reopened: Form2200ANDraft = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(reopened.number_of_attached_sheets, 0);
    assert_eq!(serde_json::to_value(&reopened).unwrap(), stored);

    let mut typed = sample;
    typed.number_of_attached_sheets = 12;
    typed.recompute();
    assert_eq!(typed.field_map()["frm2200ANv2018:txtSheets"], "12");
    let payload = typed.to_bir_xml_payload().unwrap();
    assert!(
        payload.contains("<div>frm2200ANv2018:txtSheets=12frm2200ANv2018:txtSheets=</div>"),
        "the upload carries Item 3"
    );
}

pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // PR 11: Item 3 (txtSheets) is now the draft's number of attached
    // sheets, left out of the stored draft while 0; a seed with sheets
    // attached lets the probe bind it. Its payload is never written.
    let mut sheets = sample_2200an();
    sheets.number_of_attached_sheets = 2;
    sheets.recompute();
    vec![
        serde_json::to_value(sample_2200an()).expect("sample serializes"),
        serde_json::to_value(sheets).expect("sheets sample serializes"),
    ]
}
