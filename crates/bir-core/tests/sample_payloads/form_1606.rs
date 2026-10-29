use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1606::{
    Form1606AgentCategory, Form1606Draft, Form1606PropertyClass, Form1606SellerType,
    Form1606TaxRate, Form1606TaxRelief, Form1606TaxableBaseBox, Form1606Transaction,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1606() -> Form1606Draft {
    let mut draft = Form1606Draft::new_from_profile(&dummy_profile("1606", "Corporation"), 2025, 1);
    draft.transaction_month = 6;
    draft.transaction_day = 15;
    draft.is_amended = true;
    draft.taxes_withheld = Some(true);
    // Fictitious seller; 987-654-321 passes the official check digit.
    draft.seller_tin = "98765432100000".into();
    draft.seller_rdo_code = "040".into();
    draft.seller_name = "Dela Cruz, Sample Seller".into();
    draft.seller_address = "456 Example Avenue, Makati City".into();
    draft.seller_type = Some(Form1606SellerType::Individual);
    draft.agent_category = Some(Form1606AgentCategory::Private);
    draft.property_class = Some(Form1606PropertyClass::Others);
    draft.property_class_other = "Mixed use lot".into();
    draft.property_location = "Lot 1 Block 2, Sample Subdivision, Quezon City".into();
    draft.property_rdo_code = "039".into();
    draft.tct_number = "T-123456".into();
    draft.area_sold = "250".into();
    draft.tax_declaration_number = "0123456789".into();
    draft.other_description = "Corner lot".into();
    draft.covers_more_than_one_property = Some(false);
    draft.tax_relief = Form1606TaxRelief::SpecialLaw;
    draft.set_transaction(Form1606Transaction::InstallmentSale);
    draft.gross_selling_price = 5_000_000.005;
    draft.cost_and_expenses = 3_200_000.0;
    draft.mortgage_assumed = 500_000.0;
    draft.initial_year_payments = 1_250_000.5;
    draft.installment_this_month = 104_166.675;
    draft.number_of_installments = 48.0;
    draft.fmv_land_tax_declaration = Some(3_000_000.0);
    draft.fmv_improvements_tax_declaration = Some(1_500_000.0);
    draft.fmv_land_zonal = Some(4_200_000.0);
    draft.fmv_improvements_bir = Some(0.0);
    draft.taxable_base_box = Some(Form1606TaxableBaseBox::InstallmentCollected);
    draft.installment_collected = 104_166.675;
    draft.seller_habitual = Some(true);
    draft.tax_rate = Some(Form1606TaxRate::OnePointFive);
    draft.tax_remitted_previous = 1_000.0;
    draft.surcharge = 140.625;
    draft.interest = 33.335;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1606_sample_payload_is_current() {
    let draft = sample_1606();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1606 must validate: {errors:?}"));
    check_sample(
        "1606-06152025_T123456",
        "1606-v2018",
        &payload,
        "e6690b744ed1a6131134a0e275dadfb4ac5ab77b3c3604e75653cde06899efd6",
    );
}

/// Transaction in June 2025: due 10 July 2025.
#[test]
fn form_1606_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1606, Deadline::Calendar(day(2025, 7, 10)));
}

/// Decision P5-7 on the desktop path: a draft the 1606 view creates carries
/// the typed-trigger marker, and its sale date counts once the user types
/// it. From then on its penalties compute, it queues, and the same day's
/// pre-submission revalidation finds nothing changed.
#[test]
fn a_desktop_1606_with_a_typed_sale_date_computes_and_queues_without_drift() {
    use bir_core::filing_queue::QueueAuthSource;
    use bir_core::forms::FilingStatus;
    use bir_core::forms::event_trigger::note_trigger_typed;

    bir_core::forms::penalty_items::with_clock(day(2026, 10, 19), || {
        let mut draft = sample_1606();
        draft.penalty.penalty_overrides = Default::default();
        draft.surcharge = 0.0;
        draft.interest = 0.0;
        draft.compromise = 0.0;
        // As the view's `new_draft` leaves it: marked, nothing typed yet.
        draft.lifecycle.trigger_date_model = true;
        draft.lifecycle.trigger_date_entered = false;
        draft.lifecycle.trigger_parts_typed.clear();
        draft.recompute();
        assert_eq!(draft.penalty.computed_penalties, None, "undated: none");
        assert_eq!(draft.total_penalties, 0.0);

        // The view's edit events: month and day typed over the pre-filled
        // values, the year retyped as it was.
        note_trigger_typed("1606", &mut draft.lifecycle, "transaction_month", 6);
        note_trigger_typed("1606", &mut draft.lifecycle, "transaction_day", 15);
        assert!(!draft.lifecycle.trigger_date_entered, "year not typed yet");
        note_trigger_typed("1606", &mut draft.lifecycle, "transaction_year", 2025);
        assert!(draft.lifecycle.trigger_date_entered);
        draft.recompute();
        assert_eq!(
            (
                draft.surcharge,
                draft.interest,
                draft.compromise,
                draft.total_penalties
            ),
            (140.63, 86.18, 1000.0, 1226.81)
        );
        let computed = draft.penalty.computed_penalties.clone().expect("computed");
        assert_eq!(computed.inputs.due_date, day(2025, 7, 10));

        draft
            .queue(QueueAuthSource::Gui)
            .unwrap_or_else(|errors| panic!("queues: {errors:?}"));
        draft
            .revalidate_queued_before_submission()
            .unwrap_or_else(|errors| panic!("no drift at submission: {errors:?}"));
        assert_eq!(draft.lifecycle.status, FilingStatus::Queued);
        assert_eq!(draft.penalty.computed_penalties, Some(computed));
    });
}

/// A 1606 Draft stored before the marker existed (built and computed at
/// commit b45c5386 as of 19 October 2026, kept here as it serialized) has no
/// marker, so it computes exactly what it did then.
#[test]
fn an_unmarked_stored_1606_draft_computes_what_it_did_before() {
    let text = include_str!("fixtures/legacy_1606_draft_b45c5386.json");
    let stored: serde_json::Value = serde_json::from_str(text).unwrap();
    for key in [
        "trigger_date_model",
        "trigger_date_entered",
        "trigger_parts_typed",
    ] {
        assert!(
            stored.get(key).is_none(),
            "{key} is absent from the old row"
        );
    }
    let mut draft: Form1606Draft = serde_json::from_str(text).unwrap();
    assert!(!draft.lifecycle.trigger_date_model);
    let before = draft.penalty.computed_penalties.clone();
    assert!(before.is_some());
    bir_core::forms::penalty_items::with_clock(day(2026, 10, 19), || draft.recompute());
    assert_eq!(
        (
            draft.surcharge,
            draft.interest,
            draft.compromise,
            draft.total_penalties
        ),
        (140.63, 86.18, 1000.0, 1226.81)
    );
    assert_eq!(draft.penalty.computed_penalties, before);
    assert_eq!(
        draft.penalty.penalty_overrides,
        Default::default(),
        "nothing computed becomes an override"
    );
}

/// The same row marked as created under the typed-trigger model, with no
/// sale date typed (the shell's pre-filled Draft): no penalties at all.
#[test]
fn a_marked_undated_stored_1606_draft_computes_no_penalties() {
    let text = include_str!("fixtures/legacy_1606_draft_b45c5386.json");
    let mut draft: Form1606Draft = serde_json::from_str(text).unwrap();
    draft.lifecycle.trigger_date_model = true;
    bir_core::forms::penalty_items::with_clock(day(2026, 10, 19), || draft.recompute());
    assert_eq!(draft.penalty.computed_penalties, None);
    assert_eq!(draft.penalty.penalty_due_date, None);
    assert_eq!(
        (
            draft.surcharge,
            draft.interest,
            draft.compromise,
            draft.total_penalties
        ),
        (0.0, 0.0, 0.0, 0.0)
    );
    assert_eq!(draft.penalty.penalty_overrides, Default::default());
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![serde_json::to_value(sample_1606()).expect("sample serializes")]
}
