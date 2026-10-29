use crate::dummy_profile;
use crate::sample_payloads::penalty_items::{self, Deadline, day};
use bir_core::forms::form_2000::{
    Form2000AffixtureMode, Form2000Draft, Form2000OtherParty, Form2000PaymentRow,
    Form2000RemittanceRow, form_2000_atc_option,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_2000() -> Form2000Draft {
    let mut draft =
        Form2000Draft::new_from_profile(&dummy_profile("2000v2018", "Corporation"), 2025, 6);
    // A creditor with a mixed-case name: Item 11's blur runs capital().
    draft.other_party = Form2000OtherParty::Creditor;
    draft.other_party_name = "Sample Creditor Corp".into();
    draft.other_party_tin = "987654321000".into();
    draft.affixture_mode = Form2000AffixtureMode::Constructive;
    let pick = |code, description| form_2000_atc_option(code, description).unwrap();
    draft
        .set_atc(0, pick("DS101", "ORIGINAL ISSUE OF SHARES OF STOCKS"))
        .unwrap();
    draft
        .set_atc(1, pick("DS112", "ON PRE-NEED PLANS"))
        .unwrap();
    draft
        .set_atc(2, pick("DS106", "ORIGINAL ISSUE OF ALL DEBT INSTRUMENTS"))
        .unwrap();
    draft.schedule1[0].tax_base = 123_456.789;
    draft.schedule1[1].tax_base = 999.995;
    draft.schedule1[2].tax_base = 2_500_000.0;
    // A fourth Schedule 1 row and two more Schedule 2 rows ("Add").
    draft
        .set_atc(
            3,
            pick(
                "DS104",
                "CERTIFICATE OF PROFITS OR INTEREST IN PROPERTY OR ACCUMULATIONS",
            ),
        )
        .unwrap();
    draft.schedule1[3].tax_base = 40_000.004;
    draft.ds106_term_under_a_year = Some(true);
    draft.ds106_term_days = 90;
    draft.schedule2 = vec![
        Form2000PaymentRow {
            date: "06/05/2025".into(),
            receipt_number: "Rcpt1001".into(),
            amount: 1_000.5,
        },
        Form2000PaymentRow {
            date: "06/20/2025".into(),
            receipt_number: "Rcpt1002".into(),
            amount: 2_000.005,
        },
        Form2000PaymentRow {
            date: "06/21/2025".into(),
            receipt_number: "Rcpt1003".into(),
            amount: 300.0,
        },
        Form2000PaymentRow {
            date: "06/22/2025".into(),
            receipt_number: "Rcpt1004".into(),
            amount: 400.25,
        },
    ];
    draft.schedule4 = vec![Form2000RemittanceRow {
        rco_code: "Rco01".into(),
        date: "06/25/2025".into(),
        bank: "Sample Bank".into(),
        amount: 5_000.0,
        number_from: "1001".into(),
        number_to: "1100".into(),
    }];
    draft.surcharge = 25.5;
    draft.interest = 10.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_2000_sample_payload_is_current() {
    let draft = sample_2000();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2000 must validate: {errors:?}"));
    check_sample_with_layout(
        "2000-062025",
        &draft.official_layout().expect("layout"),
        &payload,
        "3c5dce91bef0a65596313240079aec0e6898b0aabbafd2e1694f2dd9c53a7649",
    );
}

/// `check_sample` over a return's own layout (schedules with added rows).
/// The 2000 page also has four read-only `frm2000:modLabel` boxes with
/// different texts under one id, which `official_xml::read` rejects as one
/// control written twice with different values; the layout check runs on a
/// copy with those values equalized. Official parity, IAF crypto and the
/// committed files are checked unchanged.
pub(crate) fn check_sample_with_layout(
    stem: &str,
    layout: &bir_core::official_xml::OfficialLayout,
    plaintext: &str,
    _official_encrypt_sha256: &str,
) {
    let open = "<div>frm2000:modLabel=";
    let close = "frm2000:modLabel=</div>";
    let mut normalized = String::new();
    let mut rest = plaintext;
    while let Some(start) = rest.find(open) {
        let end = start + rest[start..].find(close).expect("closed modLabel");
        normalized.push_str(&rest[..start + open.len()]);
        normalized.push_str("LABEL");
        rest = &rest[end..];
    }
    normalized.push_str(rest);
    bir_core::official_xml::read(layout, &normalized).expect("sample follows the official layout");

    let dir = crate::samples_dir();
    let plain_path = dir.join(format!("{stem}.plain.xml"));
    let updating = std::env::var_os("UPDATE_SAMPLE_PAYLOADS").is_some();
    if updating {
        std::fs::write(&plain_path, plaintext).unwrap();
    }
    let official = std::fs::read_to_string(dir.join(format!("{stem}.official.xml"))).unwrap();
    assert_eq!(
        plaintext, official,
        "{stem}: our plaintext differs from the official saveXMLsubmit() output"
    );
    if updating {
        return;
    }
    assert_eq!(std::fs::read_to_string(&plain_path).unwrap(), plaintext);
}

/// The calendar lists 2000 as event-based.
#[test]
fn form_2000_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2000,
        Deadline::None {
            assumed: day(2025, 12, 31),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // eDST System (Item 13): the only mode whose Schedule 3 (advance
    // payments, Item 15C) the draft keeps, so the probe can bind its rows.
    let mut edst = sample_2000();
    edst.affixture_mode = Form2000AffixtureMode::Edst;
    edst.schedule3 = vec![
        Form2000PaymentRow {
            date: "06/03/2025".into(),
            receipt_number: "Adv2001".into(),
            amount: 700.25,
        },
        Form2000PaymentRow {
            date: "06/09/2025".into(),
            receipt_number: "Adv2002".into(),
            amount: 800.0,
        },
    ];
    edst.recompute();
    assert!(
        edst.advance_payments > 0.0,
        "the eDST seed keeps its Schedule 3"
    );
    vec![
        serde_json::to_value(sample_2000()).expect("sample serializes"),
        serde_json::to_value(edst).expect("eDST sample serializes"),
    ]
}
