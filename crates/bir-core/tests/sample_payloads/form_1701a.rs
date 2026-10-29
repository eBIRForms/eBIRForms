//! The steps answer the radio items in form order, then type amounts (see
//! `Form1701ADraft::to_bir_field_map`); profile fields are filled the way
//! `loadBGData` does.

use crate::sample_payloads::penalty_items::{self, Deadline, day};
// The samples enter their penalties by hand (`keep_entered_penalties`).
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1701a::{
    Form1701AAtc, Form1701ACivilStatus, Form1701AColumn, Form1701ADraft, Form1701AFilerType,
    Form1701AFilingStatus, Form1701AMoreRow, Form1701ASpouse,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1701a() -> Form1701ADraft {
    let mut draft = Form1701ADraft::new_from_profile(&dummy_profile("1701A", "Individual"), 2025);
    draft.taxpayer_name = "Dummy, Sample Taxpayer".to_string();
    draft.birth_date = "01/15/1980".to_string();
    draft.citizenship = "Filipino".to_string();
    draft.filer_type = Form1701AFilerType::SingleProprietor;
    draft.atc = Form1701AAtc::II012;
    draft.civil_status = Form1701ACivilStatus::Single;
    draft.taxpayer = Form1701AColumn {
        sales: 1_234_567.89,
        sales_returns: 10_000.5,
        other_income_41: 5_000.55,
        gpp_share: 20_000.0,
        prior_year_excess: 1_000.0,
        quarterly_payments: 30_000.0,
        cwt_q1_q3: 5_000.25,
        cwt_q4: 2_500.0,
        other_credits: 100.0,
        surcharge: 1_000.0,
        ..Form1701AColumn::default()
    };
    draft.other_income_41_description = "Interest income".to_string();
    draft.other_credits_description = "Sample credit".to_string();
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1701a_sample_payload_is_current() {
    let draft = sample_1701a();
    assert_eq!(draft.aggregate_amount_payable, 56_848.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1701A must validate: {errors:?}"));
    check_sample(
        "1701A-122025",
        "1701a-v2018",
        &payload,
        "70833befa59b7c8055b92fb8414a1cd2c8732e9ccc99f8d40745448b6f5749f8",
    );
}

/// Joint filing: the spouse uses the 8% rate as a professional.
fn sample_1701a_joint() -> Form1701ADraft {
    let mut draft = sample_1701a();
    draft.civil_status = Form1701ACivilStatus::Married;
    draft.spouse_has_income = Some(true);
    draft.filing_status = Form1701AFilingStatus::Joint;
    draft.spouse = Form1701ASpouse {
        tin: "12345678800000".to_string(),
        rdo_code: "039".to_string(),
        filer_type: Form1701AFilerType::Professional,
        atc: Form1701AAtc::II017,
        name: "Dummy, Sample Spouse".to_string(),
        contact_number: "09170000001".to_string(),
        citizenship: "Filipino".to_string(),
        foreign_tax_credits: Some(false),
        foreign_tax_number: String::new(),
    };
    draft.spouse_column = Form1701AColumn {
        eight_sales: 900_000.0,
        eight_reduction: 250_000.0,
        cwt_q1_q3: 4_500.5,
        ..Form1701AColumn::default()
    };
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1701a_joint_sample_payload_is_current() {
    let draft = sample_1701a_joint();
    assert_eq!(draft.aggregate_amount_payable, 104_347.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy joint 1701A must validate: {errors:?}"));
    check_sample(
        "1701A-122025-joint",
        "1701a-v2018",
        &payload,
        "0c99ea1146e34bc57f1058bf57cd430a0f5679364f7e3b86451746be1cb2387e",
    );
}

fn more_row(description: &str, taxpayer: f64, spouse: f64) -> Form1701AMoreRow {
    Form1701AMoreRow {
        description: description.to_string(),
        taxpayer,
        spouse,
    }
}

/// The joint return with three Part IV.A "(add more...)" rows.
fn sample_1701a_joint_more() -> Form1701ADraft {
    let mut draft = sample_1701a_joint();
    draft.other_income_more = vec![
        more_row("Rental income", 500.0, 0.0),
        more_row("Royalty income", 12_000.4, 0.0),
        more_row("Prize", 750.0, 0.0),
    ];
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

/// An 8% return with two Part IV.B "(add more...)" rows.
fn sample_1701a_more() -> Form1701ADraft {
    let mut draft = sample_1701a();
    draft.atc = Form1701AAtc::II015;
    draft.taxpayer.sales = 0.0;
    draft.taxpayer.sales_returns = 0.0;
    draft.taxpayer.other_income_41 = 0.0;
    draft.taxpayer.gpp_share = 0.0;
    draft.other_income_41_description = String::new();
    draft.taxpayer.eight_sales = 1_234_567.89;
    draft.taxpayer.eight_sales_returns = 10_000.5;
    draft.eight_other_income_50_description = "Interest income".to_string();
    draft.taxpayer.eight_other_income_50 = 5_000.55;
    draft.eight_other_income_more = vec![
        more_row("Rental income", 500.0, 0.0),
        more_row("Consulting", 1_500.25, 0.0),
    ];
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

/// Part IV.A "(add more...)" with three rows on the joint return: Item 42
/// is typed, transferred into the modal as row 1, two rows are added, and
/// SAVE AND CLOSE folds Item 42 into OTHERS with the whole-peso subtotal.
#[test]
fn form_1701a_joint_more_sample_payload_is_current() {
    let draft = sample_1701a_joint_more();
    assert_eq!(draft.other_income_42_description, "OTHERS");
    assert_eq!(draft.taxpayer.other_income_42, 13_250.0);
    assert_eq!(draft.aggregate_amount_payable, 106_997.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1701A must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1701A-122025-joint-more",
        &layout,
        &payload,
        "b2763dd4b9e0fccd327f838b4c43fe366508eeb73d6ffede62548e1313fa51d5",
    );
}

/// Part IV.B "(add more...)" on an 8% return: Items 50 and 51 typed, then
/// a second row through the modal.
#[test]
fn form_1701a_more_sample_payload_is_current() {
    let draft = sample_1701a_more();
    assert_eq!(draft.eight_other_income_51_description, "OTHERS");
    assert_eq!(draft.taxpayer.eight_other_income_51, 2_000.0);
    assert_eq!(draft.aggregate_amount_payable, 60_925.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1701A must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1701A-122025-more",
        &layout,
        &payload,
        "2b2af4f60e43676fe32b8f0d508c3684018bdb0249095fa7a01734f9568ddb5d",
    );
}

/// Part IV.A "(add more...)" with twelve rows: `addRow` has no row limit,
/// and every row is written in DOM order with `txtCtrmodalPartIVA=12`.
#[test]
fn form_1701a_joint_many_rows_sample_payload_is_current() {
    let mut draft = sample_1701a_joint();
    draft.other_income_more = vec![more_row("Rental income", 500.0, 0.0)];
    for n in 2..=12u32 {
        let amount = f64::from(1000 * n + 5) + f64::from(n % 10) / 10.0;
        draft
            .other_income_more
            .push(more_row(&format!("Other income {n}"), amount, 0.0));
    }
    draft.keep_entered_penalties();
    draft.recompute();
    assert_eq!(draft.other_income_more.len(), 12);
    assert_eq!(draft.taxpayer.other_income_42, 77_560.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1701A must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1701A-122025-joint-many",
        &layout,
        &payload,
        "1f22c121e80d3d392f0bf3e5f04b1cbac82c2c0bff0b0fa445b31b40c51f5943",
    );
}

/// Calendar year 2025: due 15 April 2026.
#[test]
fn form_1701a_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1701a, Deadline::Calendar(day(2026, 4, 15)));
}

/// Joint return: the spouse column too.
#[test]
fn form_1701a_joint_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1701a_joint, Deadline::Calendar(day(2026, 4, 15)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1701a()).expect("sample serializes"),
        serde_json::to_value(sample_1701a_joint()).expect("sample serializes"),
        // The "(add more...)" rows reach the upload only once two are saved.
        serde_json::to_value(sample_1701a_joint_more()).expect("sample serializes"),
        serde_json::to_value(sample_1701a_more()).expect("sample serializes"),
    ]
}
