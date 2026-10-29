use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{dummy_profile, samples_dir};
use bir_core::forms::form_1702rt::{
    Form1702RTDate, Form1702RTDeductionMethod, Form1702RTDraft, Form1702RTNamedAmount,
    Form1702RTSpecialDeductionRow, WholePeso,
};
use bir_core::forms::queueable::QueueableForm as _;

fn named(description: &str, amount: i64) -> Form1702RTNamedAmount {
    Form1702RTNamedAmount {
        description: description.to_string(),
        amount: WholePeso(amount),
    }
}

/// Itemized deductions with Schedules I to V, NOLCO and MCIT carry-overs,
/// credits, penalties and one payment row.
fn sample_1702rt() -> Form1702RTDraft {
    let mut draft =
        Form1702RTDraft::new_from_profile(&dummy_profile("1702RTv2018C", "Corporation"), 2025, 12);
    draft.deduction_method = Form1702RTDeductionMethod::Itemized;
    draft.president_signatory_title = "President".to_string();
    draft.president_signatory_tin = "123456788000".to_string();

    let p4 = &mut draft.part_iv;
    p4.item_40_income_tax_rate_percent = 25;
    p4.item_27_sales = WholePeso(8_500_000);
    p4.item_28_sales_returns = WholePeso(125_000);
    p4.item_30_cost_of_sales_or_services = WholePeso(4_200_000);
    p4.item_32_other_taxable_income = WholePeso(75_500);
    p4.item_42_mcit_due = WholePeso(85_010);
    let credits = &mut p4.tax_credits;
    credits.item_44_prior_year_excess_credits = WholePeso(10_000);
    credits.item_46_previous_quarter_regular_payments = WholePeso(300_000);
    credits.item_48_previous_quarter_withholding = WholePeso(40_000);
    credits.item_49_fourth_quarter_withholding = WholePeso(15_500);
    credits.item_52_special_tax_credits = WholePeso(1_000);
    credits.item_53_other = named("Other credit", 2_500);
    draft.part_v.item_57_special_allowable_deductions_tax_effect = WholePeso(12_500);

    let s1 = &mut draft.schedule_1;
    s1.salaries_wages_allowances = WholePeso(1_500_000);
    s1.rental = WholePeso(360_000);
    s1.depreciation = WholePeso(120_000);
    s1.taxes_and_licenses = WholePeso(45_250);
    s1.professional_fees = WholePeso(80_000);
    s1.other[0] = named("Office supplies", 15_750);
    let row = &mut draft.schedule_2.rows[0];
    row.description = "Special deduction".to_string();
    row.legal_basis = "RA 9999".to_string();
    row.amount = WholePeso(50_000);
    let nolco = &mut draft.schedule_3.rows[1];
    nolco.year_incurred = "2023".to_string();
    nolco.amount = WholePeso(300_000);
    nolco.applied_previous_years = WholePeso(100_000);
    nolco.applied_current_year = WholePeso(150_000);
    let mcit = &mut draft.schedule_4.rows[0];
    mcit.year = "2023".to_string();
    mcit.normal_income_tax = WholePeso(50_000);
    mcit.mcit = WholePeso(70_000);
    mcit.applied_current_year = WholePeso(20_000);
    let s5 = &mut draft.schedule_5;
    s5.item_1_net_income_or_loss_per_books = WholePeso(1_800_000);
    s5.additions[0] = named("Non-deductible expenses", 179_500);
    s5.non_taxable_income[0] = named("Interest income subject to final tax", 50_000);

    let p2 = &mut draft.part_ii;
    p2.item_17_surcharge = WholePeso(23_343);
    p2.item_18_interest = WholePeso(4_000);
    p2.item_19_compromise = WholePeso(1_000);
    draft.keep_entered_penalties();
    draft.recompute();
    let cash = &mut draft.payment_details[0];
    cash.drawee_bank_or_agency = "Sample Bank".to_string();
    cash.number = "12345".to_string();
    cash.date = Form1702RTDate::new(2026, 4, 15).ok();
    cash.amount = draft.part_ii.item_21_total_amount_payable_or_overpayment;
    draft.recompute();
    draft
}

/// Checks the 1702RT sample against the official upload.
#[test]
fn form_1702rt_sample_payload_is_current() {
    let payload = sample_1702rt()
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1702RT must validate: {errors:?}"));
    let stem = "1702RT-122025";
    let dir = samples_dir();
    if std::env::var_os("UPDATE_SAMPLE_PAYLOADS").is_some() {
        std::fs::write(dir.join(format!("{stem}.plain.xml")), &payload).unwrap();
    }
    let official = std::fs::read_to_string(dir.join(format!("{stem}.official.xml"))).unwrap();
    assert_eq!(
        payload, official,
        "{stem}: differs from the official saveXMLsubmit() output"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(format!("{stem}.plain.xml"))).unwrap(),
        payload
    );
}

fn special(description: &str, legal_basis: &str, amount: i64) -> Form1702RTSpecialDeductionRow {
    Form1702RTSpecialDeductionRow {
        description: description.to_string(),
        legal_basis: legal_basis.to_string(),
        amount: WholePeso(amount),
    }
}

/// Every "(Add more...)" modal of the page with more rows than the page
/// shows: the item's own entry becomes row 1 and Add Row adds the rest
/// (`addRowModalTable` has no limit). The steps start by giving the page
/// the IE7 document mode of its HTA host (case-insensitive
/// `getElementById`, no whitespace-only text nodes in the item rows), which
/// `saveModalTable` and `loadModalTable` rely on.
fn sample_1702rt_more_rows() -> Form1702RTDraft {
    let mut draft = sample_1702rt();
    draft.part_iv.tax_credits.item_54_other = named("Other credit 2", 1_000);
    let s1 = &mut draft.schedule_1;
    s1.other[1] = named("Repairs", 9_000);
    s1.other[2] = named("Utilities", 12_000);
    s1.other[3] = named("Insurance", 8_000);
    s1.other[4] = named("Communication", 6_500);
    s1.other[5] = named("Trainings", 4_000);
    draft.schedule_2.rows[1] = special("Second deduction", "RA 8888", 20_000);
    draft.schedule_2.rows[2] = special("Third deduction", "RA 7777", 10_000);
    draft.schedule_2.rows[3] = special("Fourth deduction", "RA 6666", 5_000);
    let s5 = &mut draft.schedule_5;
    s5.additions[1] = named("Penalties", 2_000);
    s5.non_taxable_income[1] = named("Dividends", 3_000);
    s5.special_deductions[0] = named("Special", 4_000);
    s5.special_deductions[1] = named("Special 2", 1_500);

    let more = &mut draft.more_rows;
    more.item_54 = vec![special("Other credit 2", "", 1_000)];
    more.schedule_1_item_17i = vec![special("Trainings", "", 4_000)];
    more.schedule_2_item_4 = vec![special("Fourth deduction", "RA 6666", 5_000)];
    more.schedule_5_item_3 = vec![special("Penalties", "", 2_000)];
    more.schedule_5_item_6 = vec![special("Dividends", "", 3_000)];
    more.schedule_5_item_8 = vec![special("Special 2", "", 1_500)];
    for n in 2..=4i64 {
        more.item_54
            .push(special(&format!("Credit {n}"), "", 250 * n));
    }
    for n in 2..=6i64 {
        more.schedule_1_item_17i
            .push(special(&format!("Expense {n}"), "", 1_000 * n + 125));
    }
    for n in 2..=5i64 {
        more.schedule_2_item_4.push(special(
            &format!("Deduction {n}"),
            &format!("RR {n}-2025"),
            2_000 * n,
        ));
    }
    for n in 2..=3i64 {
        more.schedule_5_item_3
            .push(special(&format!("Addition {n}"), "", 700 * n));
        more.schedule_5_item_6
            .push(special(&format!("Exempt {n}"), "", 900 * n));
    }
    for n in 2..=4i64 {
        more.schedule_5_item_8
            .push(special(&format!("Deduct {n}"), "", 300 * n));
    }
    draft.recompute();
    // Schedule V Item 10 reconciles to Part IV Item 39.
    let gap = draft.part_iv.item_39_net_taxable_income_or_loss.0
        - draft.schedule_5.item_10_net_taxable_income_or_loss.0;
    draft.schedule_5.item_1_net_income_or_loss_per_books.0 += gap;
    draft.recompute();
    draft.payment_details[0].amount = draft.part_ii.item_21_total_amount_payable_or_overpayment;
    draft.recompute();
    draft
}

#[test]
fn form_1702rt_more_rows_sample_payload_is_current() {
    let draft = sample_1702rt_more_rows();
    assert_eq!(
        draft.schedule_5.item_1_net_income_or_loss_per_books.0,
        1_687_075
    );
    assert_eq!(
        draft.part_ii.item_21_total_amount_payable_or_overpayment.0,
        87_687
    );
    assert_eq!(
        draft.part_iv.tax_credits.item_54_other.description,
        "OTHERS"
    );
    let payload = draft
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1702RT must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1702RT-122025-more",
        &layout,
        &payload,
        "62bf0aec0324d90a04ff9f313c03479181b540046b350ce0775753c5d2b704ed",
    );
}

/// The sample before its payment is recorded: Part III must equal Item 21,
/// which moves with the computed penalties.
fn sample_1702rt_unpaid() -> Form1702RTDraft {
    let mut draft = sample_1702rt();
    draft.payment_details[0] = Default::default();
    draft.recompute();
    draft
}

/// Calendar year 2025: due 15 April 2026.
#[test]
fn form_1702rt_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1702rt_unpaid, Deadline::Calendar(day(2026, 4, 15)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // Overpaid (Item 16 below zero): the only state in which the Item 21
    // overpayment choice is kept (form_1702rt_official.rs recompute clears
    // it otherwise), so the probe can bind its three boxes.
    let mut overpaid = sample_1702rt();
    overpaid
        .part_iv
        .tax_credits
        .item_48_previous_quarter_withholding = WholePeso(5_000_000);
    overpaid.recompute();
    assert!(
        overpaid.part_ii.item_16_net_tax_payable_or_overpayment.0 < 0,
        "the overpaid seed is overpaid"
    );
    overpaid.part_ii.overpayment_disposition =
        Some(bir_core::forms::form_1702rt::Form1702RTOverpaymentDisposition::Refund);
    overpaid.recompute();
    vec![
        serde_json::to_value(sample_1702rt()).expect("sample serializes"),
        serde_json::to_value(sample_1702rt_more_rows()).expect("sample serializes"),
        serde_json::to_value(overpaid).expect("overpaid sample serializes"),
    ]
}
