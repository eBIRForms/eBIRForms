use crate::dummy_profile;
use crate::sample_payloads::form_2000::check_sample_with_layout;
use crate::sample_payloads::penalty_items::{self, Deadline, day};
use bir_core::forms::form_2000ot::{
    Form2000OTAtc, Form2000OTDraft, Form2000OTNature, Form2000OTOtherParty, Form2000OTPropertyRow,
    Form2000OTShareRow,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2000ot() -> Form2000OTDraft {
    let mut draft =
        Form2000OTDraft::new_from_profile(&dummy_profile("2000OTv2018", "Corporation"), 2025);
    draft.transaction_date = "06/15/2025".into();
    draft.is_amended = true;
    draft.tax_paid_previous = 1_000.0;
    draft.atc = Form2000OTAtc::Do122;
    draft.other_party = Form2000OTOtherParty::Creditor;
    draft.other_party_name = "Sample Buyer Corp".into();
    draft.other_party_tin = "987654321000".into();
    draft.nature = Form2000OTNature::RealPropertyCapitalAsset;
    draft.real_property_location = "Lot 1 Block 2, Sample Subdivision, Quezon City".into();
    draft.properties = vec![
        Form2000OTPropertyRow {
            title_number: "Tct-12345".into(),
            tax_declaration_number: "Td-0001".into(),
            location: "Barangay Example".into(),
            lot: "Lot 1".into(),
            classification: "Residential".into(),
            area: "250".into(),
            fmv_per_td: 1_250_000.005,
            fmv_zonal: 1_300_000.0,
            fmv: 0.0,
        },
        Form2000OTPropertyRow {
            title_number: "Tct-12346".into(),
            tax_declaration_number: "Td-0002".into(),
            location: "Barangay Example".into(),
            lot: "Improvement".into(),
            classification: "Residential".into(),
            area: "120".into(),
            fmv_per_td: 800_000.0,
            fmv_zonal: 750_000.5,
            fmv: 0.0,
        },
    ];
    // Four more rows ("Add" appends to Schedule 1.A and its continuation).
    for n in 2..6 {
        draft.properties.push(Form2000OTPropertyRow {
            title_number: format!("Tct-2000{n}"),
            tax_declaration_number: format!("Td-000{n}"),
            location: "Barangay Example".into(),
            lot: format!("Lot {n}"),
            classification: "Residential".into(),
            area: "100".into(),
            fmv_per_td: 100_000.0 * n as f64,
            fmv_zonal: 90_000.0,
            fmv: 0.0,
        });
    }
    draft.gross_selling_price = 2_750_000.005;
    draft.surcharge = 25.5;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_2000ot_sample_payload_is_current() {
    let draft = sample_2000ot();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2000-OT must validate: {errors:?}"));
    check_sample_with_layout(
        "2000OT-06152025",
        &draft.official_layout().expect("layout"),
        &payload,
        "f1bde5e95d95ebd47a3b1adbe51d5f584280b1a3377719183f7ff54d86ebff03",
    );
}

fn sample_2000ot_shares() -> Form2000OTDraft {
    let mut draft =
        Form2000OTDraft::new_from_profile(&dummy_profile("2000OTv2018", "Corporation"), 2025);
    draft.transaction_date = "06/16/2025".into();
    draft.atc = Form2000OTAtc::Do102;
    draft.other_party = Form2000OTOtherParty::Debtor;
    draft.other_party_name = "Sample Buyer Corp".into();
    draft.other_party_tin = "987654321000".into();
    draft.nature = Form2000OTNature::SharesOfStock;
    for n in 0..4 {
        draft.shares.push(Form2000OTShareRow {
            corporation: format!("Sample Corp {n}"),
            shares_sold: format!("{}", 100 * (n + 1)),
            certificate_number: format!("Cert-{n}"),
            par_value: 25_000.005 * (n + 1) as f64,
            dst_paid_on_issue: 0.0,
        });
    }
    draft.recompute();
    draft
}

#[test]
fn form_2000ot_shares_sample_with_added_rows_is_current() {
    let draft = sample_2000ot_shares();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2000-OT shares must validate: {errors:?}"));
    check_sample_with_layout(
        "2000OT-06162025",
        &draft.official_layout().expect("layout"),
        &payload,
        "22986716d723a37071db3f4143f6a1f30587c8c4ed03edf367fb764701af432d",
    );
}

/// Event-based: the calendar has no 2000-OT deadline.
#[test]
fn form_2000ot_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2000ot,
        Deadline::None {
            assumed: day(2025, 12, 31),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_2000ot()).expect("sample serializes"),
        serde_json::to_value(sample_2000ot_shares()).expect("sample serializes"),
    ]
}
