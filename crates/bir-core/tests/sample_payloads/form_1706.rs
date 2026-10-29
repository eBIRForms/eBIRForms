use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1706::{
    FORM_1706_SCHEDULE_1_CELLS, Form1706Draft, Form1706Overpayment, Form1706PropertyClass,
    Form1706SellerType, Form1706TaxRelief, Form1706TaxableBase, Form1706TransactionType,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1706() -> Form1706Draft {
    let mut draft = Form1706Draft::new_from_profile(&dummy_profile("1706", "Individual"), 2025, 1);
    draft.transaction_month = 3;
    draft.transaction_day = 15;
    draft.transaction_year = 2025;
    draft.number_of_attached_sheets = 1;
    draft.set_seller_type(Form1706SellerType::Individual);
    draft.buyer_tin = "111-222-333".into();
    draft.buyer_branch_code = "00000".into();
    draft.buyer_rdo_code = "040".into();
    draft.buyer_name = "Maria Dela Cruz-Santos".into();
    draft.buyer_address = "45 Example Ave., Makati City".into();
    draft.seller_residence_address = "Unit 5, Sample Residences".into();
    draft.property_location = "Lot 7 Blk 2, Sample Subd., Quezon City".into();
    draft.property_rdo_code = "039".into();
    draft.property_class = Some(Form1706PropertyClass::Others);
    draft.property_class_other = "Mixed use lot".into();
    draft.tct_number = "T-123456".into();
    draft.area_sold = "250.5".into();
    draft.tax_declaration_number = "0123456".into();
    draft.property_other_description = "Corner lot".into();
    draft.set_individual_answers(Some(true), Some(false));
    draft.covers_multiple_properties = Some(false);
    draft.set_tax_relief(Form1706TaxRelief::SpecialLaw);
    draft.set_transaction_type(Form1706TransactionType::CashSale);
    draft.fmv_land_tax_declaration = Some(1_234_567.005);
    draft.fmv_land_zonal = Some(1_500_000.125);
    draft.fmv_improvements_tax_declaration = Some(300_000.555);
    draft.fmv_improvements_bir = Some(250_000.0);
    draft.taxable_base_option = Some(Form1706TaxableBase::GrossSellingPrice);
    draft.gross_selling_price = Some(1_800_000.995);
    draft.schedule_1 = vec![
        "Sales proceeds".into(),
        "1,800,001.00".into(),
        "Utilized".into(),
        "0.00".into(),
    ];
    draft.surcharge = 25.5;
    draft.interest = 10.005;
    draft.compromise = 1_000.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1706_sample_payload_is_current() {
    let draft = sample_1706();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1706 must validate: {errors:?}"));
    check_sample(
        "1706-03152025_T123456",
        "1706-v2018",
        &payload,
        "80132b201b450803fe84940c2b1ad78dd1b7d4fbf3cbcaa4c7611d751f3ffc1c",
    );
}

/// Event-based: the calendar has no 1706 deadline.
#[test]
fn form_1706_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_1706,
        Deadline::None {
            assumed: day(2025, 12, 31),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // PR 9: an amended return overpaid by the tax paid before, the only
    // state in which the Item 36 boxes (`opt37`) keep a choice
    // (`recompute` clears it otherwise), with all twenty Schedule 1 popup
    // cells filled, so the probe binds both. Its payload is never written.
    let mut overpaid = sample_1706();
    overpaid.set_amended(true);
    overpaid.tax_paid_previous = 5_000_000.0;
    overpaid.recompute();
    assert!(
        overpaid.total_amount_payable < 0.0,
        "the overpaid seed is overpaid"
    );
    overpaid.overpayment = Form1706Overpayment::Refund;
    overpaid.schedule_1 = (1..=FORM_1706_SCHEDULE_1_CELLS)
        .map(|cell| format!("Cell {cell}"))
        .collect();
    overpaid.recompute();
    vec![
        serde_json::to_value(sample_1706()).expect("sample serializes"),
        serde_json::to_value(overpaid).expect("overpaid sample serializes"),
    ]
}
